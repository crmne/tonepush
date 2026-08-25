//! Recording a live session's raw byte transport, and replaying it offline.
//!
//! The command layer - every `set_model`, `read_preset`, handshake frame - can
//! only be exercised against hardware. That is a problem the day the hardware
//! is sold: nothing offline proves a refactor did not change the bytes a
//! command sends. So a session can run against a [`RecordingWire`] that logs
//! every transfer, and the log can be replayed through a [`ReplayWire`] with no
//! device present. The replay returns each recorded response in order *and*
//! checks that each request still matches what was recorded, so a command whose
//! encoding drifts fails offline, as a diff.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hx_proto::frame::{ChannelHeader, MSG_ACK};
use hx_proto::Frame;

use crate::{Error, Result, Wire};

/// Which way one transfer went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Client to device.
    Out,
    /// Device to client.
    In,
}

/// Bytes as the transcript writes them, for putting two side by side.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A recorded conversation: every transfer, in order.
#[derive(Clone, Default)]
pub struct Transcript(pub Vec<(Dir, Vec<u8>)>);

impl Transcript {
    /// One `O <hex>` (out) or `I <hex>` (in) line per transfer.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (dir, bytes) in &self.0 {
            let tag = match dir {
                Dir::Out => 'O',
                Dir::In => 'I',
            };
            out.push(tag);
            out.push(' ');
            for b in bytes {
                out.push_str(&format!("{b:02x}"));
            }
            out.push('\n');
        }
        out
    }

    /// Parse the format [`to_text`](Self::to_text) writes.
    pub fn from_text(text: &str) -> Transcript {
        let mut transfers = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (tag, hex) = line.split_at(1);
            let dir = match tag {
                "O" => Dir::Out,
                "I" => Dir::In,
                _ => continue,
            };
            let hex = hex.trim();
            let bytes = (0..hex.len())
                .step_by(2)
                .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                .collect();
            transfers.push((dir, bytes));
        }
        Transcript(transfers)
    }
}

/// A shared, growable log of transfers, filled by a [`RecordingWire`].
pub type Log = Arc<Mutex<Vec<(Dir, Vec<u8>)>>>;

/// A fresh, empty recording log.
pub fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

/// Turn a filled log into a transcript.
pub fn finish(log: &Log) -> Transcript {
    Transcript(log.lock().unwrap().clone())
}

/// Wraps a live wire and copies every transfer into a shared log, unchanged.
pub struct RecordingWire {
    inner: Box<dyn Wire>,
    log: Log,
}

impl RecordingWire {
    pub fn new(inner: Box<dyn Wire>, log: Log) -> RecordingWire {
        RecordingWire { inner, log }
    }
}

impl Wire for RecordingWire {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner.send(bytes)?;
        self.log.lock().unwrap().push((Dir::Out, bytes.to_vec()));
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let data = self.inner.recv(timeout)?;
        self.log.lock().unwrap().push((Dir::In, data.clone()));
        Ok(data)
    }
}

/// Replays a transcript with no device. Returns each recorded response in
/// order; on a send, checks the client's bytes still match what was recorded.
/// When the transcript runs out, a read reports "nothing more" the way a real
/// idle endpoint does (an empty transfer), so a drain loop ends cleanly.
pub struct ReplayWire {
    transfers: VecDeque<(Dir, Vec<u8>)>,
    drifted: Arc<Mutex<Vec<String>>>,
    skipped_acks: BTreeMap<(u16, u16), u16>,
}

impl ReplayWire {
    pub fn new(transcript: Transcript) -> ReplayWire {
        ReplayWire {
            transfers: transcript.0.into(),
            drifted: Arc::new(Mutex::new(Vec::new())),
            skipped_acks: BTreeMap::new(),
        }
    }

    /// Every request whose bytes no longer match what was recorded.
    ///
    /// Returning the error from `send` is not enough on its own to fail a
    /// replay. The commands being exercised are write commands, and a caller
    /// checking that a *write* still encodes the same has no use for its
    /// result - so they are called as `let _ = …`, and the error went nowhere.
    /// The whole point of the fixture is that an encoding change fails offline
    /// as a diff, and for a while it did not: a corrected assign message
    /// changed recorded bytes and the suite stayed green. This is the record a
    /// test can assert on.
    pub fn drifted(&self) -> Arc<Mutex<Vec<String>>> {
        self.drifted.clone()
    }
}

impl Wire for ReplayWire {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        // Older transcripts were captured while the client emitted a bare ACK
        // immediately before a request whose own header already carried the
        // same cumulative acknowledgement. The live transport no longer burns
        // that redundant sequence number. Skip only those legacy bare ACKs;
        // request and payload frames remain byte-exact checks.
        while !is_bare_ack(bytes) {
            let route = match self.transfers.front() {
                Some((Dir::Out, expected)) => legacy_ack_repeated_by(expected, bytes),
                _ => None,
            };
            let Some(route) = route else { break };
            self.transfers.pop_front();
            let skipped = self.skipped_acks.entry(route).or_default();
            *skipped = skipped.wrapping_add(1);
        }
        match self.transfers.pop_front() {
            Some((Dir::Out, expected))
                if expected == bytes
                    || matches_after_legacy_acks(&expected, bytes, &self.skipped_acks) =>
            {
                Ok(())
            }
            Some((Dir::Out, expected)) => {
                // Recorded as well as returned, because the caller of a write
                // command has no reason to look at its result and so never
                // sees this.
                let complaint = format!(
                    "a request no longer matches what was recorded\n     sent {}\n     was  {}",
                    hex(bytes),
                    hex(&expected)
                );
                self.drifted.lock().unwrap().push(complaint.clone());
                Err(Error::Protocol(format!("replay: {complaint}")))
            }
            Some((Dir::In, _)) => Err(Error::Protocol(
                "replay: the client sent where the device was recorded speaking".into(),
            )),
            None => Err(Error::Protocol(
                "replay: the client sent past the end of the transcript".into(),
            )),
        }
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        match self.transfers.front() {
            // A recorded receive - possibly an empty (zero-length) transfer,
            // which the device really does send and the reader treats as "no
            // frame this time".
            Some((Dir::In, _)) => Ok(self.transfers.pop_front().unwrap().1),
            // Exhausted, or the recording shows the client sending next: report
            // an idle endpoint the way a real read times out. A drain loop
            // counts these as quiet and stops; a request loop would have found
            // its reply already.
            _ => Err(Error::Usb("read timed out".into())),
        }
    }
}

fn is_bare_ack(bytes: &[u8]) -> bool {
    let Ok(frame) = Frame::decode(bytes) else {
        return false;
    };
    let Some((header, rest)) = ChannelHeader::decode(&frame.payload) else {
        return false;
    };
    header.msg_type == MSG_ACK && rest.is_empty()
}

/// Whether `expected` is the old standalone form of the acknowledgement that
/// `actual` already carries. Requiring the same route and cumulative count
/// keeps replay strict about every ACK that was not genuinely redundant.
fn legacy_ack_repeated_by(expected: &[u8], actual: &[u8]) -> Option<(u16, u16)> {
    let (Ok(expected), Ok(actual)) = (Frame::decode(expected), Frame::decode(actual)) else {
        return None;
    };
    if (expected.src, expected.dst) != (actual.src, actual.dst) {
        return None;
    }
    let (Some((expected_header, expected_body)), Some((actual_header, _))) = (
        ChannelHeader::decode(&expected.payload),
        ChannelHeader::decode(&actual.payload),
    ) else {
        return None;
    };
    (expected_header.msg_type == MSG_ACK
        && expected_body.is_empty()
        && actual_header.msg_type != MSG_ACK
        && expected_header.ack == actual_header.ack)
        .then_some((expected.src, expected.dst))
}

fn matches_after_legacy_acks(
    expected: &[u8],
    actual: &[u8],
    skipped: &BTreeMap<(u16, u16), u16>,
) -> bool {
    let (Ok(expected), Ok(actual)) = (Frame::decode(expected), Frame::decode(actual)) else {
        return false;
    };
    if (expected.flags, expected.src, expected.dst) != (actual.flags, actual.src, actual.dst) {
        return false;
    }
    let (Some((expected_header, expected_body)), Some((actual_header, actual_body))) = (
        ChannelHeader::decode(&expected.payload),
        ChannelHeader::decode(&actual.payload),
    ) else {
        return false;
    };
    let offset = skipped
        .get(&(expected.src, expected.dst))
        .copied()
        .unwrap_or(0);
    expected_header.seq.wrapping_sub(offset) == actual_header.seq
        && expected_header.msg_type == actual_header.msg_type
        && expected_header.ack == actual_header.ack
        && expected_body == actual_body
}
