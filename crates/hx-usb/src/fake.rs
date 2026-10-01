//! A pretend pedal on the far side of a [`Wire`], for tests that need the
//! device to *behave* rather than repeat a recording.
//!
//! A replay transcript checks that a command still sends the same bytes; it
//! cannot say what happens when the device is slow, refuses, or answers
//! something stale, because the recording only ever holds what one real
//! session did. This answers requests from a small model of the device's
//! state instead, so a test can arrange exactly the misbehaviour it is about.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use hx_proto::frame::{ChannelHeader, MSG_DATA};
use hx_proto::msgpack::{Encoder, Value};
use hx_proto::rpc::{key, op, Message, StreamReader};
use hx_proto::{ChannelId, Frame};

use crate::{Error, Result, Session, Wire};

/// An impulse response slot: its name, and the checksum it was uploaded with.
pub type Ir = (String, u64);

pub struct Pedal {
    /// The edit buffer, as the device would serialise it.
    pub buffer: Vec<u8>,
    /// Whether a document write changes the buffer. A pedal that accepts a
    /// write and never applies it is what the read-back has to catch.
    pub applies_writes: bool,
    /// Answer a document write "accepted, completes later" rather than "done".
    pub defers_writes: bool,
    /// What each IR slot holds.
    pub irs: BTreeMap<i64, Ir>,
    /// An upload accepted and not yet written: the slot, and what it will hold.
    pub writing_ir: Option<(i64, Ir)>,
    /// How many more questions about IRs are answered from the old contents
    /// before an accepted upload lands.
    pub ir_delay: usize,
    /// Every request received, as `(channel, transaction, opcode, arguments)`.
    pub requests: Vec<(u16, i64, i64, Value)>,
    /// Transfers waiting for the host to read.
    pub outbox: VecDeque<Vec<u8>>,
    /// Fail every read with this error once the next data frame arrives.
    pub fail_reads_after_data: Option<String>,
    /// Fail every read with this error, now.
    pub failing_reads: Option<String>,
    /// Fail the next send with this error.
    pub fail_next_send: Option<String>,
    /// Data frames received, for telling whether anything reached the wire.
    pub frames_in: usize,
    inbox: BTreeMap<u16, StreamReader>,
    seq: BTreeMap<u16, u16>,
}

impl Pedal {
    pub fn new() -> Arc<Mutex<Pedal>> {
        Arc::new(Mutex::new(Pedal {
            buffer: include_bytes!("../../hx-proto/tests/preset.bin").to_vec(),
            applies_writes: true,
            defers_writes: false,
            irs: BTreeMap::new(),
            writing_ir: None,
            ir_delay: 0,
            requests: Vec::new(),
            outbox: VecDeque::new(),
            fail_reads_after_data: None,
            failing_reads: None,
            fail_next_send: None,
            frames_in: 0,
            inbox: BTreeMap::new(),
            seq: BTreeMap::new(),
        }))
    }

    pub fn opcodes(&self) -> Vec<i64> {
        self.requests
            .iter()
            .map(|(_, _, opcode, _)| *opcode)
            .collect()
    }

    fn receive(&mut self, bytes: &[u8]) {
        let Ok(frame) = Frame::decode(bytes) else {
            return;
        };
        let Some((header, rest)) = ChannelHeader::decode(&frame.payload) else {
            return;
        };
        if !header.has_data() || rest.is_empty() {
            return;
        }
        self.frames_in += 1;
        if let Some(why) = self.fail_reads_after_data.take() {
            self.failing_reads = Some(why);
        }
        let node = frame.dst;
        let reader = self.inbox.entry(node).or_default();
        reader.push(rest);
        let messages = reader.take_messages().unwrap_or_default();
        for message in messages {
            if let Ok(Message::Request { txn, opcode, args }) =
                Message::try_from_value(message.body)
            {
                self.requests.push((node, txn, opcode, args.clone()));
                let (status, result) = self.answer(opcode, &args);
                self.reply(
                    node,
                    &Message::Response {
                        txn,
                        status,
                        result,
                    },
                );
            }
        }
    }

    /// One question about IRs has been asked: an accepted upload comes one
    /// step closer to landing.
    fn ir_tick(&mut self) {
        if self.writing_ir.is_none() {
            return;
        }
        if self.ir_delay == 0 {
            let (slot, ir) = self.writing_ir.take().unwrap();
            self.irs.insert(slot, ir);
        } else {
            self.ir_delay -= 1;
        }
    }

    fn answer(&mut self, opcode: i64, args: &Value) -> (i64, Value) {
        let number = |k| args.get(k).and_then(Value::as_i64).unwrap_or_default();
        match opcode {
            op::PRESET_INFO => (
                0,
                hx_proto::msgmap! {
                    key::SETLIST => Value::Int(0),
                    key::PRESET_INDEX => Value::Int(0),
                    key::NAME => Value::Str("Test".into()),
                },
            ),
            op::READ_PRESET => (0, Value::Bin(self.buffer.clone(), 2)),
            op::WRITE_PRESET => {
                if self.applies_writes {
                    if let Some(document) = args.get(key::DOCUMENT).and_then(Value::as_raw) {
                        self.buffer = document.to_vec();
                    }
                }
                (i64::from(self.defers_writes), Value::Nil)
            }
            op::LIST_IRS => {
                self.ir_tick();
                if self.irs.is_empty() {
                    return (0, Value::Nil);
                }
                let listed = self
                    .irs
                    .iter()
                    .map(|(slot, (name, _))| {
                        hx_proto::msgmap! {
                            key::IR_SLOT => Value::Int(*slot),
                            key::NAME => Value::Str(name.clone()),
                        }
                    })
                    .collect();
                (0, Value::Array(listed))
            }
            op::UPLOAD_IR => {
                let name = args.get(key::NAME).and_then(Value::as_str);
                let sum = args
                    .get(key::IR_CHECKSUM)
                    .and_then(Value::as_i64)
                    .unwrap_or_default() as u64;
                self.writing_ir = Some((
                    number(key::IR_SLOT),
                    (name.unwrap_or_default().to_owned(), sum),
                ));
                (1, Value::Nil)
            }
            op::CLEAR_IR => {
                self.irs.remove(&number(key::IR_SLOT));
                (0, Value::Nil)
            }
            _ => (0, Value::Nil),
        }
    }

    /// Queue one message for the host, on the channel `node` names.
    fn reply(&mut self, node: u16, message: &Message) {
        let body = Encoder::encode(&message.to_value());
        let seq = self.seq.entry(node).or_insert(0);
        let mut payload = Vec::new();
        ChannelHeader {
            seq: *seq,
            msg_type: MSG_DATA,
            ack: 0x1000,
        }
        .encode_into(&mut payload);
        *seq = seq.wrapping_add(1);
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.extend_from_slice(&(body.len() as u32).to_le_bytes());
        payload.extend_from_slice(&body);
        let host = ChannelId::ALL
            .iter()
            .find(|channel| channel.device == node)
            .map_or(0, |channel| channel.host);
        self.outbox
            .push_back(Frame::new(host, node, payload).encode().unwrap());
    }
}

/// The USB cable, as far as the session can tell.
pub struct Cable(pub Arc<Mutex<Pedal>>);

impl Cable {
    fn pedal(&self) -> MutexGuard<'_, Pedal> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Wire for Cable {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let mut pedal = self.pedal();
        if let Some(why) = pedal.fail_next_send.take() {
            return Err(Error::Usb(why));
        }
        pedal.receive(bytes);
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        let mut pedal = self.pedal();
        if let Some(why) = &pedal.failing_reads {
            return Err(Error::Usb(why.clone()));
        }
        pedal
            .outbox
            .pop_front()
            .ok_or_else(|| Error::Usb("read timed out".into()))
    }
}

/// A session with `pedal` on the other end of the cable.
pub fn session(pedal: &Arc<Mutex<Pedal>>) -> Session {
    Session::replaying(Box::new(Cable(pedal.clone())), hx_proto::HX_STOMP)
        .expect("the pretend pedal answers")
}
