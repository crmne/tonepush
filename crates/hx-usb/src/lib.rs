//! USB transport for HX-family devices.
//!
//! Wraps the codec in `hx-proto` with device discovery and a session that keeps
//! the per-channel bookkeeping straight: sequence numbers, cumulative
//! acknowledgements, and reassembly of streams that straddle transfers.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hx_proto::frame::{ChannelHeader, MSG_ACK, MSG_DATA, MSG_HELLO, MSG_KEEPALIVE};
use hx_proto::msgpack::Value;
use hx_proto::rpc::{self, Message, StreamReader};
use hx_proto::{ChannelId, DeviceProfile, Frame, Preset, EP_IN, EP_OUT, INTERFACE, VENDOR_ID};
use nusb::transfer::{Buffer, Bulk, In, Out};
use nusb::MaybeFuture;

mod commands;
pub use commands::{Assignment, Carried, Switch};
pub mod backup;
pub mod ir;
pub mod replay;

/// The claimed interface and its two bulk endpoints, ready to become a wire.
type Endpoints = (
    nusb::Interface,
    nusb::Endpoint<Bulk, Out>,
    nusb::Endpoint<Bulk, In>,
);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no supported HX device found")]
    NotFound,
    #[error(
        "could not claim interface 0 ({0}).\n\
         If HX Edit is running, quit it - it holds the interface exclusively.\n\
         If nothing is running, the device is wedged: disconnect its 9V adapter \
         for a few seconds. A USB replug is not enough, because the unit is \
         externally powered and keeps its session across re-enumeration."
    )]
    Claim(String),
    #[error("usb error: {0}")]
    Usb(String),
    #[error("the device refused: error {0}")]
    Device(i64),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("timed out waiting for a reply to transaction {0}")]
    Timeout(i64),
}

impl Error {
    /// Whether continuing on this open session risks speaking on a transport
    /// whose sequence/transaction state is no longer known. A device refusal
    /// is a complete, aligned reply and is therefore the sole recoverable
    /// command error; transport silence and malformed protocol are not.
    pub fn loses_session(&self) -> bool {
        !matches!(self, Self::Device(_))
    }
}

/// A device found on the bus.
#[derive(Debug, Clone)]
pub struct Found {
    pub profile: DeviceProfile,
    pub serial: Option<String>,
    info: nusb::DeviceInfo,
}

/// List every supported HX device currently attached.
pub fn list() -> Result<Vec<Found>> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(|e| Error::Usb(e.to_string()))?;
    Ok(devices
        .filter(|d| d.vendor_id() == VENDOR_ID)
        .filter_map(|d| {
            hx_proto::profile_for(d.product_id()).map(|p| Found {
                profile: *p,
                serial: d.serial_number().map(str::to_owned),
                info: d,
            })
        })
        .collect())
}

/// Per-channel state: what we have sent, and how much we have received.
struct Channel {
    seq: u16,
    /// Bytes received from the device on this channel. The acknowledgement
    /// field is this count plus a fixed base, which is how the device paces us.
    rx_bytes: u32,
    /// How much of `rx_bytes` the device has been told about. It paces itself
    /// by the difference, so a channel that is never acknowledged eventually
    /// stops the device dead - including the channels nobody is waiting on.
    acked: u32,
    reader: StreamReader,
    txn: i64,
}

impl Channel {
    /// Acknowledgements do not start at zero - the host advertises a base of
    /// 0x1000 and adds the bytes it has consumed. Sending a bare byte count
    /// makes the device stop responding.
    const ACK_BASE: u32 = 0x1000;

    fn new() -> Self {
        Channel {
            seq: 0,
            rx_bytes: 0,
            acked: 0,
            reader: StreamReader::new(),
            txn: rpc::FIRST_TXN,
        }
    }

    fn ack(&self) -> u32 {
        Self::ACK_BASE.wrapping_add(self.rx_bytes)
    }

    fn next_txn(&mut self) -> i64 {
        let t = self.txn;
        self.txn += 1;
        t
    }
}

pub struct Session {
    /// Held purely to keep the interface claimed: dropping it releases the
    /// claim and the endpoints stop working. `None` for a replay session,
    /// which has no hardware to hold.
    #[allow(dead_code)]
    interface: Option<nusb::Interface>,
    /// The raw byte transport under the frame protocol: the USB endpoints in a
    /// live session, a recorded transcript in a replay.
    wire: Box<dyn Wire>,
    channels: BTreeMap<u16, Channel>,
    /// Set when a transfer failed part-way through a message.
    ///
    /// A stream message that is only half-sent leaves the device waiting for
    /// bytes that will never arrive, and it then refuses new sessions until its
    /// power is pulled. Once that has happened there is nothing useful left to
    /// do on this session, and continuing to write only digs deeper - so the
    /// session refuses further work and says why.
    poisoned: Option<String>,
    pub profile: DeviceProfile,
}

/// The raw byte transport beneath the frame protocol.
///
/// The live transport is the pair of USB bulk endpoints. Abstracting it lets a
/// session run against a recorded transcript instead of hardware, so the
/// command layer stays regression-testable offline - the point being to keep
/// it correct after the device is sold. See the `replay` module.
pub trait Wire: Send {
    /// Send one frame's encoded bytes, blocking until the write completes.
    fn send(&mut self, bytes: &[u8]) -> Result<()>;
    /// Receive the device's next chunk of bytes. An empty vec is a zero-length
    /// transfer (no payload); a timeout is an error.
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>>;
}

/// The live USB transport. Exactly one read is kept posted so the device's
/// unsolicited notifications are never dropped.
struct UsbWire {
    ep_out: nusb::Endpoint<Bulk, Out>,
    ep_in: nusb::Endpoint<Bulk, In>,
    read_posted: bool,
}

impl Wire for UsbWire {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.ep_out.submit(Buffer::from(bytes.to_vec()));
        let completion = self
            .ep_out
            .wait_next_complete(Session::WRITE)
            .ok_or_else(|| Error::Usb("write timed out".into()))?;
        completion
            .into_result()
            .map_err(|e| Error::Usb(e.to_string()))?;
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        if !self.read_posted {
            self.ep_in.submit(Buffer::new(512));
            self.read_posted = true;
        }
        let Some(c) = self.ep_in.wait_next_complete(timeout) else {
            return Err(Error::Usb("read timed out".into()));
        };
        self.read_posted = false;
        let data = c.into_result().map_err(|e| Error::Usb(e.to_string()))?;
        Ok(data.to_vec())
    }
}

impl Found {
    /// Open the device and bring every channel up.
    ///
    /// Retried once, because the device ignores a fresh session's opening
    /// handshake on almost exactly every other attempt - a deterministic
    /// alternation we have not explained. Opening again clears it.
    ///
    /// This belongs here rather than in each caller: it was in the CLI for a
    /// while and every other consumer was quietly unreliable, which is what the
    /// integration tests surfaced.
    pub fn open(&self) -> Result<Session> {
        match self.open_once() {
            Err(Error::Timeout(_)) | Err(Error::Protocol(_)) => self.open_once(),
            other => other,
        }
    }

    fn open_once(&self) -> Result<Session> {
        let (interface, ep_out, ep_in) = self.claim()?;
        let wire: Box<dyn Wire> = Box::new(UsbWire {
            ep_out,
            ep_in,
            read_posted: false,
        });
        Session::bring_up(Some(interface), wire, self.profile)
    }

    /// Open the device with every transfer copied into `log`, to capture a
    /// transcript that can be replayed offline later. See the `replay` module.
    ///
    /// Retries the device's every-other-attempt deafness like [`open`](Self::open)
    /// does, clearing the log first so the retry records a clean session.
    pub fn open_recording(&self, log: replay::Log) -> Result<Session> {
        match self.open_recording_once(&log) {
            Err(Error::Timeout(_)) | Err(Error::Protocol(_)) => {
                log.lock().unwrap().clear();
                self.open_recording_once(&log)
            }
            other => other,
        }
    }

    fn open_recording_once(&self, log: &replay::Log) -> Result<Session> {
        let (interface, ep_out, ep_in) = self.claim()?;
        let usb: Box<dyn Wire> = Box::new(UsbWire {
            ep_out,
            ep_in,
            read_posted: false,
        });
        let wire: Box<dyn Wire> = Box::new(replay::RecordingWire::new(usb, log.clone()));
        Session::bring_up(Some(interface), wire, self.profile)
    }

    /// Claim the interface and open its bulk endpoints, cleared and ready.
    fn claim(&self) -> Result<Endpoints> {
        let device = self
            .info
            .open()
            .wait()
            .map_err(|e| Error::Usb(e.to_string()))?;
        // HX Edit claims interface 0, releases it, and claims it again before
        // saying anything. That looked like startup noise until reconnecting
        // without it failed on exactly every other attempt: the device carries
        // channel sequence state across connections, and the release is what
        // clears it.
        drop(claim_interface(&device)?);
        let interface = claim_interface(&device)?;

        let mut ep_out = interface
            .endpoint::<Bulk, Out>(EP_OUT)
            .map_err(|e| Error::Usb(e.to_string()))?;
        let mut ep_in = interface
            .endpoint::<Bulk, In>(EP_IN)
            .map_err(|e| Error::Usb(e.to_string()))?;

        // The device keeps per-channel state, and a client that exits without
        // closing leaves the endpoints holding stale data - which made every
        // other session start out of phase. Clearing halts and draining gives
        // each session a known-clean starting point.
        let _ = ep_out.clear_halt().wait();
        let _ = ep_in.clear_halt().wait();
        Ok((interface, ep_out, ep_in))
    }
}

/// Claiming immediately after another session releases the interface can race
/// Linux's asynchronous usbfs cleanup and briefly return `EBUSY`. It is not a
/// wedged pedal (the next attempt succeeds), so absorb that short hand-off
/// window here. A genuinely competing editor still produces the ordinary
/// claim error after a bounded 300 ms.
fn claim_interface(device: &nusb::Device) -> Result<nusb::Interface> {
    const ATTEMPTS: usize = 7;
    let mut last = String::from("interface remained busy");
    for attempt in 0..ATTEMPTS {
        match device.claim_interface(INTERFACE).wait() {
            Ok(interface) => return Ok(interface),
            Err(error) => {
                let busy = error.kind() == nusb::ErrorKind::Busy;
                last = error.to_string();
                if !busy {
                    break;
                }
            }
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Err(Error::Claim(last))
}

impl Session {
    /// Construct a session over `wire` and bring it up: handshake, then a
    /// liveness read. Shared by a live open, a recording open, and a replay.
    fn bring_up(
        interface: Option<nusb::Interface>,
        wire: Box<dyn Wire>,
        profile: DeviceProfile,
    ) -> Result<Session> {
        let mut s = Session {
            interface,
            wire,
            channels: BTreeMap::new(),
            poisoned: None,
            profile,
        };
        s.handshake()?;
        // A handshake can complete and the session still be deaf: the device
        // ignores a reconnecting client on roughly every other attempt, and the
        // symptom is the *first request* timing out rather than the handshake
        // failing. Prove the session works before handing it over.
        s.preset_info()?;
        Ok(s)
    }

    /// A session that talks to `wire` - a recorded transcript - instead of
    /// hardware. Runs the same handshake and liveness read a live open does,
    /// against the recorded responses, so replaying reproduces a real session.
    pub fn replaying(wire: Box<dyn Wire>, profile: DeviceProfile) -> Result<Session> {
        Session::bring_up(None, wire, profile)
    }

    /// Payload of the channel handshake, taken verbatim from HX Edit. The
    /// trailing fields look like a capability exchange but have not been
    /// decoded, so they are replayed rather than constructed.
    const HELLO_TAIL: [u8; 4] = [0x00, 0x10, 0x00, 0x00];
    /// Occupies the acknowledgement slot in a handshake; meaning unknown.
    const HELLO_FIELD: u32 = 0x2100_0100;

    fn handshake(&mut self) -> Result<()> {
        // Anything the device queued before we attached would desynchronise the
        // streams, so start from a known-empty endpoint.
        self.drain();

        // A throwaway handshake to absorb stale state was tried here and made
        // things strictly worse: every run failed rather than every other one.
        // The device evidently does not tolerate two openings in a session.

        for id in ChannelId::ALL {
            // The control channel is opened twice: once for service 5, then
            // again from scratch for service 2, which is where its requests
            // ride. It is a full second handshake - sequence restarting at
            // zero - not a second service opened on the same one. Doing it the
            // latter way silently breaks every channel.
            for (n, &service) in services(id).iter().enumerate() {
                if n > 0 {
                    // Close the previous service before re-opening. HX Edit
                    // sends this and the device will not answer on the new
                    // service without it.
                    self.close_service(id)?;
                }
                self.open_service(id, service, n == 0)?;
            }
        }
        Ok(())
    }

    /// Send the bare type-2 frame that ends the current service.
    fn close_service(&mut self, id: ChannelId) -> Result<()> {
        let (seq, ack) = self.tick(id)?;
        let mut payload = Vec::new();
        ChannelHeader {
            seq,
            msg_type: MSG_HELLO,
            ack,
        }
        .encode_into(&mut payload);
        self.write(&Frame::new(id.device, id.host, payload))?;
        self.mark_acknowledged(id);
        self.read_available(Self::REPLY)?;
        Ok(())
    }

    /// Bring one service up on a channel.
    ///
    /// `fresh` starts the byte accounting from zero. A re-open keeps counting,
    /// because the device's acknowledgements carry on from where the previous
    /// service left off.
    fn open_service(&mut self, id: ChannelId, service: u16, fresh: bool) -> Result<()> {
        let carried = if fresh {
            0
        } else {
            self.channels.get(&id.device).map_or(0, |c| c.rx_bytes)
        };
        self.channels.insert(id.device, Channel::new());

        let mut payload = Vec::new();
        ChannelHeader {
            seq: 0,
            msg_type: MSG_HELLO,
            ack: Self::HELLO_FIELD,
        }
        .encode_into(&mut payload);
        payload.extend_from_slice(&Self::HELLO_TAIL);
        let mut hello = Frame::new(id.device, id.host, payload);
        hello.flags = hx_proto::frame::FLAG_HANDSHAKE;
        self.write(&hello)?;
        self.read_available(Self::REPLY)?;

        // Byte accounting starts here; the handshake restarts the channel.
        if let Some(ch) = self.channels.get_mut(&id.device) {
            ch.reader
                .take_messages()
                .map_err(|error| Error::Protocol(error.to_string()))?;
            ch.rx_bytes = carried;
            // HX Edit's counter jumps 0 -> 2 here. The device stops responding
            // if we send 1, so the observed numbering is reproduced.
            ch.seq = 2;
        }

        self.send_stream(id, service, &Value::UInt(service as u64))?;
        self.read_available(Self::REPLY)?;
        if let Some(ch) = self.channels.get_mut(&id.device) {
            ch.reader
                .take_messages()
                .map_err(|error| Error::Protocol(error.to_string()))?;
        }
        self.ack_channel(id)
    }

    /// Empty the endpoint before saying anything.
    ///
    /// A client that exits without closing leaves its channels running, and the
    /// device keeps acknowledging into a buffer nobody is reading. That backlog
    /// survives process restarts and even a device power cycle, because it is
    /// queued on the host - so the next session reads thousands of stale
    /// acknowledgements instead of its own handshake reply and concludes the
    /// device is dead.
    ///
    /// Draining continues until reads time out, not until the interesting
    /// messages stop: acknowledgements carry no data, and an earlier version
    /// that watched only for data gave up with the queue still full. A
    /// zero-length transfer counts as activity - the device does send them, so
    /// treating one as silence would end the drain early.
    ///
    /// It is bounded, though. An unbounded drain kept the endpoint under
    /// sustained load and coincided with the device locking up hard enough to
    /// need its power pulled, so a few seconds of clearing is the most we ask
    /// for before proceeding regardless.
    fn drain(&mut self) {
        let give_up = Instant::now() + Self::DRAIN_BUDGET;
        let mut quiet = 0;
        let mut discarded = 0usize;

        while quiet < 3 && Instant::now() < give_up {
            match self.read_once(Self::DRAIN_READ) {
                Ok(_) => {
                    quiet = 0;
                    discarded += 1;
                }
                Err(_) => quiet += 1,
            }
        }

        if debug() && discarded > 0 {
            eprintln!("drained {discarded} stale frames");
        }
    }

    fn write(&mut self, f: &Frame) -> Result<()> {
        let bytes = f
            .encode()
            .map_err(|error| Error::Protocol(error.to_string()))?;
        if debug() {
            eprintln!("TX {:#06x}->{:#06x} {}", f.src, f.dst, hex(&bytes));
        }
        self.wire.send(&bytes)
    }

    /// Read one frame and route its payload into the owning channel.
    fn read_once(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        let data = self.wire.recv(timeout)?;
        if data.is_empty() {
            return Ok(None);
        }
        if debug() {
            eprintln!("RX {}", hex(&data));
        }
        let frame = Frame::decode(&data).map_err(|e| Error::Protocol(e.to_string()))?;
        self.route(&frame);
        Ok(Some(frame))
    }

    /// Read when silence is expected, distinguishing an idle endpoint from an
    /// actual USB or protocol failure.
    fn read_available(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        match self.read_once(timeout) {
            Err(Error::Usb(message)) if message == "read timed out" => Ok(None),
            other => other,
        }
    }

    fn route(&mut self, f: &Frame) {
        let Some((hdr, rest)) = ChannelHeader::decode(&f.payload) else {
            return;
        };
        let Some(ch) = self.channels.get_mut(&f.src) else {
            return;
        };
        if hdr.has_data() && !rest.is_empty() {
            ch.rx_bytes = ch.rx_bytes.wrapping_add(rest.len() as u32);
            ch.reader.push(rest);
        }
    }

    fn rx_bytes(&self, id: ChannelId) -> u32 {
        self.channels.get(&id.device).map_or(0, |c| c.rx_bytes)
    }

    /// Take the next sequence number and current acknowledgement for a channel.
    ///
    /// Channels stay in the map throughout - an earlier version removed them
    /// while a request was in flight, which silently discarded every byte the
    /// device sent back because routing could no longer find the channel.
    fn tick(&mut self, id: ChannelId) -> Result<(u16, u32)> {
        let ch = self
            .channels
            .get_mut(&id.device)
            .ok_or_else(|| Error::Protocol(format!("channel {:#06x} not open", id.device)))?;
        let seq = ch.seq;
        ch.seq = ch.seq.wrapping_add(1);
        Ok((seq, ch.ack()))
    }

    /// How long to wait for the device to answer a handshake or service open.
    const REPLY: Duration = Duration::from_millis(800);
    /// Per-read timeout while clearing a backlog.
    const DRAIN_READ: Duration = Duration::from_millis(150);
    /// Total budget for clearing a backlog. Bounded because an unbounded drain
    /// kept the endpoint busy and coincided with device lock-ups.
    const DRAIN_BUDGET: Duration = Duration::from_secs(3);
    /// How long a single bulk write may take.
    const WRITE: Duration = Duration::from_secs(2);
    /// How long to wait for a reply before giving up on a request.
    const REPLY_BUDGET: Duration = Duration::from_secs(6);
    /// Per-read timeout while waiting for a reply.
    const REPLY_READ: Duration = Duration::from_millis(300);
    /// Pause between chunks of a large send, letting the device's
    /// acknowledgements through so its receive window reopens.
    const BETWEEN_CHUNKS: Duration = Duration::from_millis(80);

    /// Bytes of stream data per frame.
    ///
    /// The device chunks its own large transfers at 256 and will not accept a
    /// single oversized frame - an 8 KB impulse response sent whole simply
    /// times out. Matching its size is the safe choice.
    const CHUNK: usize = 256;

    fn send_stream(&mut self, id: ChannelId, service: u16, body: &Value) -> Result<()> {
        if let Some(why) = &self.poisoned {
            return Err(Error::Protocol(why.clone()));
        }
        let encoded = hx_proto::msgpack::Encoder::encode(body);
        let encoded_len = u32::try_from(encoded.len())
            .map_err(|_| Error::Protocol("an RPC message is larger than 4 GiB".into()))?;

        // The message header rides with the first chunk; the rest is a plain
        // byte stream that the peer reassembles.
        let mut stream = Vec::with_capacity(encoded.len() + 8);
        stream.extend_from_slice(&1u16.to_le_bytes());
        stream.extend_from_slice(&service.to_le_bytes());
        stream.extend_from_slice(&encoded_len.to_le_bytes());
        stream.extend_from_slice(&encoded);

        let chunks: Vec<_> = stream.chunks(Self::CHUNK).collect();
        for (n, chunk) in chunks.iter().enumerate() {
            let (seq, ack) = self.tick(id)?;
            let mut payload = Vec::with_capacity(ChannelHeader::SIZE + chunk.len());
            ChannelHeader {
                seq,
                msg_type: MSG_DATA,
                ack,
            }
            .encode_into(&mut payload);
            payload.extend_from_slice(chunk);
            // A failure after the first chunk has left a partial message on the
            // wire, and no later request can recover from that.
            if let Err(e) = self.write(&Frame::new(id.device, id.host, payload)) {
                if n > 0 {
                    self.poisoned = Some(format!(
                        "a transfer failed part-way through ({e}); the device needs its \
                         9V adapter pulled before it will accept a new session"
                    ));
                }
                return Err(e);
            }
            self.mark_acknowledged(id);

            // Read between chunks on a long send. The device paces us with
            // acknowledgements, and writing a whole impulse response blind
            // fills its receive window and stalls the endpoint - the transfer
            // then times out with nothing obviously wrong. HX Edit interleaves
            // the same way. One chunk needs none of this, so skip it there.
            if n + 1 < chunks.len() {
                self.read_available(Self::BETWEEN_CHUNKS)?;
            }
        }
        Ok(())
    }

    /// Send a deferred request and wait for the device to finish it.
    ///
    /// Select-preset, write-preset and IR upload answer status 1 - accepted -
    /// and complete afterwards, announcing the completion as a notification
    /// carrying the same transaction. Its event id varies by operation; HX
    /// Edit will not start the next such
    /// operation until that notification arrives; fourteen consecutive undo
    /// writes captured from it all follow the pattern. Not waiting is what our
    /// sustained document writes did, and the device tolerates roughly a dozen
    /// racing commits before its transfer state machine jams for good.
    fn command_deferred(&mut self, id: ChannelId, opcode: i64, args: Value) -> Result<()> {
        let (txn, status, _) = self.request_raw(id, opcode, args)?;
        if status != 1 {
            return Ok(()); // completed synchronously
        }
        let deadline = Instant::now() + Self::COMPLETION_BUDGET;
        while Instant::now() < deadline {
            let before = self.rx_bytes(id);
            self.read_available(Self::REPLY_READ)?;
            let got = self.rx_bytes(id) != before;
            let messages = self
                .channels
                .get_mut(&ChannelId::EVENTS.device)
                .map(|ch| ch.reader.take_messages())
                .transpose()
                .map_err(|error| Error::Protocol(error.to_string()))?
                .unwrap_or_default();
            let mut done = false;
            for sm in messages {
                if let Message::Notification { args, .. } = decode_message(sm.body)? {
                    done |= completion_matches(&args, txn);
                }
            }
            if got {
                self.ack_channel(id)?;
            }
            // The completion itself arrives on the events channel. Tell the
            // pedal we consumed it before returning; otherwise a string of
            // short-lived sessions accumulates unacknowledged completions.
            self.ack_idle_channels(id)?;
            if done {
                return Ok(());
            }
        }
        // No announcement inside the budget. That does not mean the device is
        // stuck - not every deferred operation emits notification 20, and
        // select-preset frequently does not. What the caller actually needs is
        // for the device to be free again, so ask it something cheap and take
        // an answer as proof. Only silence here is a real failure.
        let deadline = Instant::now() + Self::READY_BUDGET;
        while Instant::now() < deadline {
            if self
                .request_raw(id, rpc::op::PRESET_INFO, Value::Nil)
                .is_ok()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let why = format!("the device accepted transaction {txn} and has not answered since");
        // The deferred operation may still exist inside the pedal. Refuse any
        // later write on this Session even if a caller forgets to drop it: a
        // second command in this unknown state is what turns a lost connection
        // into a pedal that needs its 9V power pulled.
        self.poisoned = Some(why.clone());
        Err(Error::Protocol(why))
    }

    /// How long to keep asking whether the device is free after a deferred
    /// operation went unannounced.
    const READY_BUDGET: Duration = Duration::from_secs(5);

    /// How long to wait for a completion announcement. Captured commits take
    /// about 300 ms, so a second is generous; past that it is not coming and
    /// the readiness poll below is the better question to be asking. Waiting
    /// longer costs seconds on every preset change, which never announces.
    const COMPLETION_BUDGET: Duration = Duration::from_millis(900);

    /// Send a request whose reply carries nothing worth returning.
    ///
    /// Most device operations are like this, and spelling out `?;` then
    /// `Ok(())` twelve times says nothing the name does not.
    fn command(&mut self, id: ChannelId, opcode: i64, args: Value) -> Result<()> {
        self.request(id, opcode, args)?;
        Ok(())
    }

    /// Send a request and wait for its reply.
    ///
    /// A timeout is reported, not retried. An earlier version answered failures
    /// by re-running the whole handshake up to four times, which meant sending
    /// fresh HELLO frames on channels the device already had open - and every
    /// failure amplified into a burst of them. That correlated with the device
    /// locking up hard enough to need its power pulled, so the retry is gone.
    /// If a request goes unanswered the honest thing is to say so and let the
    /// caller decide.
    /// Like [`request`](Self::request), but returning the reply's status too.
    ///
    /// Exists for protocol experiments; ordinary callers use `request`, which
    /// deliberately does not judge statuses (see the comment inside).
    pub fn request_full(
        &mut self,
        id: ChannelId,
        opcode: i64,
        args: Value,
    ) -> Result<(i64, Value)> {
        self.request_raw(id, opcode, args).map(|(_, s, v)| (s, v))
    }

    pub fn request(&mut self, id: ChannelId, opcode: i64, args: Value) -> Result<Value> {
        self.request_raw(id, opcode, args).map(|(_, _, v)| v)
    }

    fn request_raw(
        &mut self,
        id: ChannelId,
        opcode: i64,
        args: Value,
    ) -> Result<(i64, i64, Value)> {
        let txn = {
            let ch = self
                .channels
                .get_mut(&id.device)
                .ok_or_else(|| Error::Protocol(format!("channel {:#06x} not open", id.device)))?;
            ch.next_txn()
        };
        // Take anything the device has been holding before adding to it.
        //
        // A read buffer is only posted while a request is in flight, so between
        // operations the device has nowhere to put the notifications it emits
        // unasked. Once its outgoing buffer is full it stops draining the
        // incoming endpoint as well, and the next write simply times out with
        // nothing visibly wrong. That is the lock-up that needed the 9V adapter
        // pulled, and why it arrived sooner the more work had been done first.
        self.drain_pending(id)?;

        let msg = Message::Request { txn, opcode, args };
        self.send_stream(id, service(id), &msg.to_value())?;

        let deadline = Instant::now() + Self::REPLY_BUDGET;
        while Instant::now() < deadline {
            // Acknowledge only when stream bytes actually arrived. The device
            // sends zero-length transfers when it has nothing to say, and
            // acking those burns a sequence number, which desynchronises the
            // channel and stalls the transfer partway through.
            let before = self.rx_bytes(id);
            self.read_available(Self::REPLY_READ)?;
            let got = self.rx_bytes(id) != before;

            let ready: Vec<_> = self
                .channels
                .get_mut(&id.device)
                .map(|ch| ch.reader.take_messages())
                .transpose()
                .map_err(|error| Error::Protocol(error.to_string()))?
                .unwrap_or_default();
            for sm in ready {
                if let Message::Response {
                    txn: t,
                    status,
                    result,
                } = decode_message(sm.body)?
                {
                    if t == txn {
                        // Key 103 is not a plain error code. A successful
                        // select-preset answers with 1 and a nil result, while
                        // a preset read answers with 0 and a blob - HX Edit
                        // sees the same values. Since no value is known to mean
                        // failure, the status is reported rather than judged.
                        // 0 is done and 1 is accepted-completes-later; 255 is
                        // the device refusing, with a signed error code under
                        // key 111 (-3 bad reference, -46 bad snapshot, -302
                        // unknown model - the map is in PROTOCOL.md). Statuses
                        // were unjudged for a long time because every value HX
                        // Edit's own traffic shows is 0 or 1; the refusals only
                        // appeared once we sent deliberately bad requests.
                        if status == 255 {
                            let code = result
                                .get(rpc::key::ERROR_CODE)
                                .and_then(Value::as_i64)
                                .unwrap_or(0);
                            return Err(Error::Device(code));
                        }
                        return Ok((txn, status, result));
                    }
                }
            }
            // Large results arrive in 256-byte chunks, and each one is released
            // by acknowledging the bytes already received.
            if got {
                self.ack_channel(id)?;
            }
            // And the channels nobody is waiting on need it too - see
            // `ack_idle_channels`.
            self.ack_idle_channels(id)?;
        }
        Err(Error::Timeout(txn))
    }

    /// Read whatever the device is holding, without waiting for more.
    ///
    /// Bounded: a device that never stops talking must not stop us working.
    fn drain_pending(&mut self, busy: ChannelId) -> Result<()> {
        const BUDGET: Duration = Duration::from_millis(60);
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            match self.read_available(Duration::from_millis(5)) {
                Ok(Some(_)) => {}
                // Nothing there, or nothing more: either way we are current.
                Ok(None) => break,
                Err(error) => return Err(error),
            }
        }
        // Whatever arrived on a channel nobody is waiting on still has to be
        // acknowledged, or the device keeps counting it against us.
        // The first frame of the request about to be sent carries the busy
        // channel's acknowledgement. Sending a separate ACK immediately before
        // it needlessly consumes a sequence number. Only idle channels need an
        // explicit frame here.
        self.ack_pending(Some(busy.device))
    }

    /// Per-channel counters, for diagnosing pacing problems.
    pub fn channel_stats(&self) -> Vec<(u16, u16, u32, u32)> {
        ChannelId::ALL
            .iter()
            .filter_map(|id| {
                let c = self.channels.get(&id.device)?;
                Some((id.device, c.seq, c.rx_bytes, c.acked))
            })
            .collect()
    }

    /// Acknowledge the channels nobody is currently talking on.
    ///
    /// Every frame we send carries an acknowledgement in its header, so a
    /// channel being used keeps itself current for free. The events channel is
    /// different: the device pushes notifications onto it whether or not
    /// anyone asked, and we never send anything back, so its acknowledgement
    /// never advances. The device paces itself by that number, and once the
    /// unacknowledged bytes pile up high enough it stops accepting writes
    /// altogether - the lock-up that needed the 9V adapter pulled. It looked
    /// count-based because it was: every preset write pushes another burst of
    /// notifications nobody was acknowledging.
    ///
    /// The busy channel is deliberately excluded. Acknowledging it here as
    /// well, on top of the acknowledgement its own frames already carry, burns
    /// a sequence number mid-transaction and desynchronises the stream - which
    /// wedges the device faster than the problem being fixed.
    fn ack_idle_channels(&mut self, busy: ChannelId) -> Result<()> {
        self.ack_pending(Some(busy.device))
    }

    /// Explicitly acknowledge channels that have no ordinary outgoing frame
    /// available to carry their current byte count.
    fn ack_pending(&mut self, except: Option<u16>) -> Result<()> {
        for id in ChannelId::ALL {
            if except == Some(id.device) {
                continue;
            }
            let behind = self
                .channels
                .get(&id.device)
                .is_some_and(|c| c.rx_bytes != c.acked);
            if behind {
                self.ack_channel(id)?;
            }
        }
        Ok(())
    }

    /// Record that a successfully written channel frame carried the current
    /// cumulative acknowledgement in its header.
    fn mark_acknowledged(&mut self, id: ChannelId) {
        if let Some(channel) = self.channels.get_mut(&id.device) {
            channel.acked = channel.rx_bytes;
        }
    }

    /// Acknowledge everything received so far on one channel.
    fn ack_channel(&mut self, id: ChannelId) -> Result<()> {
        let (seq, ack) = self.tick(id)?;
        let mut payload = Vec::new();
        ChannelHeader {
            seq,
            msg_type: MSG_ACK,
            ack,
        }
        .encode_into(&mut payload);
        self.write(&Frame::new(id.device, id.host, payload))?;
        self.mark_acknowledged(id);
        Ok(())
    }

    /// Keep every channel alive; the device drops idle sessions.
    pub fn keepalive(&mut self) -> Result<()> {
        for id in ChannelId::ALL {
            if !self.channels.contains_key(&id.device) {
                continue;
            }
            let (seq, ack) = self.tick(id)?;
            let mut payload = Vec::new();
            ChannelHeader {
                seq,
                msg_type: MSG_KEEPALIVE,
                ack,
            }
            .encode_into(&mut payload);
            self.write(&Frame::new(id.device, id.host, payload))?;
            self.mark_acknowledged(id);
        }
        Ok(())
    }

    /// Load a preset by setlist and zero-based index within it, and do not
    /// return until the device says that preset is the one loaded.
    ///
    /// A select is deferred and frequently completes unannounced, and the
    /// device answers ordinary questions while the switch is still in
    /// flight - so "it answers again" is not "it finished". Starting the
    /// next select inside that window stacks racing commits, and the device
    /// jams for good after roughly a dozen; browsing quickly through a
    /// setlist wedged a unit exactly that way. The one honest completion
    /// signal is the device reporting the requested index as current.
    pub fn select_preset(&mut self, setlist: i64, index: i64) -> Result<()> {
        self.command_deferred(
            ChannelId::DATA,
            rpc::op::SELECT_PRESET,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::PRESET_INDEX => Value::Int(index),
            },
        )?;

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Every switch streams notifications, and a browse through a
            // setlist streams a flood of them; drained here, they cannot
            // back the control channel up. The keepalive feeds the channels
            // the storm is not using, because the device drops quiet ones.
            let _ = self.poll_notifications();
            let _ = self.keepalive();
            // A busy device may refuse the question; that is patience, not
            // failure, until the deadline says otherwise.
            if let Ok((_, current, _)) = self.preset_info() {
                if current == index {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                let why = format!("the device did not finish switching to preset {index}");
                self.poisoned = Some(why.clone());
                return Err(Error::Protocol(why));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Read the preset currently loaded, as a parsed document.
    pub fn read_preset(&mut self) -> Result<Preset> {
        let v = self.request(ChannelId::DATA, rpc::op::READ_PRESET, Value::Nil)?;
        let blob = v
            .as_raw()
            .ok_or_else(|| Error::Protocol("preset response was not a blob".into()))?;
        Preset::parse(blob)
            .ok_or_else(|| Error::Protocol("preset blob was not an l6-helix document".into()))
    }

    /// Read any preset by slot, without loading it.
    ///
    /// [`read_preset`](Self::read_preset) returns whatever is loaded, and
    /// loading each preset in turn is what makes a whole-pedal backup take two
    /// minutes. This is the opcode HX Edit's own backup uses instead: it names
    /// the slot, answers in about 20 ms, and leaves the loaded preset and the
    /// player's sound alone.
    ///
    /// `None` is an empty slot, which the device answers with no document at
    /// all. That is a state worth recording rather than skipping: restoring a
    /// backup has to blank those slots to put the pedal back as it was.
    ///
    /// The bytes are not identical to loading the slot and reading it - a
    /// loaded document carries the firmware build string a stored one does not,
    /// which shifts every section offset - but the tone is the same, blocks,
    /// values, bypasses, tempo and snapshots alike.
    pub fn read_preset_at(&mut self, setlist: i64, index: i64) -> Result<Option<Preset>> {
        let v = self.request(
            ChannelId::DATA,
            rpc::op::FETCH_PRESET,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::PRESET_INDEX => Value::Int(index),
                rpc::key::ARGS => Value::Int(2),
            },
        )?;
        let Some(blob) = v.as_raw() else {
            return Ok(None);
        };
        Preset::parse(blob)
            .map(Some)
            .ok_or_else(|| Error::Protocol("preset blob was not an l6-helix document".into()))
    }

    /// Write a document straight into a slot, naming it.
    ///
    /// This is how HX Edit restores a backup and how it pastes or imports a
    /// preset: the document goes to the slot in one message, with no edit
    /// buffer and no separate save. It is a flash write, so it is paced like
    /// every other one - see `settle_flash`.
    ///
    /// The bytes go out **exactly as given**. [`write_preset`](Self::write_preset)
    /// settles empty branches first, because a non-zero attach over an empty
    /// branch makes the device wipe its *edit buffer* - but that is a rule about
    /// the edit buffer, and this writes to flash instead. Settling here would
    /// mean a restored preset did not match the backup it came from: presets
    /// come off the device with those attach points set, so normalising them
    /// would quietly rewrite what it is this function's whole job to preserve.
    pub fn write_preset_at(
        &mut self,
        setlist: i64,
        index: i64,
        name: &str,
        preset: &Preset,
    ) -> Result<()> {
        self.request(
            ChannelId::DATA,
            rpc::op::WRITE_SLOT_NAMED,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::PRESET_INDEX => Value::Int(index),
                rpc::key::NAME => Value::Str(name.to_owned()),
                rpc::key::DOCUMENT => Value::Bin(preset.encode(), 2),
            },
        )?;
        self.settle_flash();
        Ok(())
    }

    /// Empty a slot, the way HX Edit's restore blanks the slots a backup holds
    /// nothing for.
    pub fn clear_preset_at(&mut self, setlist: i64, index: i64) -> Result<()> {
        self.request(
            ChannelId::DATA,
            rpc::op::CLEAR_SLOT,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::PRESET_INDEX => Value::Int(index),
            },
        )?;
        self.settle_flash();
        Ok(())
    }
}

/// The field accompanying an IR upload: the samples summed as little-endian
/// 32-bit words, wrapping.
///
/// Not a CRC, which cost some time to establish - two captured uploads were
/// checked against CRC-32, Adler-32, byte sum and length before this plain
/// word sum reproduced both exactly.
/// The word-sum checksum opcode 9 carries (key 113): wrapping sum of LE u32s.
pub fn checksum(bytes: &[u8]) -> u64 {
    bytes
        .chunks_exact(4)
        .fold(0u32, |acc, w| {
            acc.wrapping_add(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        })
        .into()
}

impl Drop for Session {
    /// Close the session the way HX Edit does.
    ///
    /// A captured clean quit shows the actual teardown: acknowledge whatever
    /// is outstanding, send a bare type-0x02 (HELLO) frame on each channel,
    /// collect the device's answering HELLOs, release the interface. The 0x02
    /// message is a session boundary marker, not just an opening handshake -
    /// it appears at both ends of the conversation. A session that vanishes
    /// without this is what leaves the device refusing new connections until
    /// its power is pulled.
    fn drop(&mut self) {
        if self.poisoned.is_some() {
            return;
        }
        // Take and acknowledge anything still in flight.
        let give_up = Instant::now() + Duration::from_millis(900);
        let mut quiet = 0;
        while quiet < 3 && Instant::now() < give_up {
            match self.read_once(Duration::from_millis(100)) {
                Ok(Some(_)) => {
                    quiet = 0;
                    let _ = self.ack_pending(None);
                }
                Ok(None) => quiet = 0,
                Err(Error::Usb(message)) if message == "read timed out" => quiet += 1,
                Err(_) => break,
            }
        }
        // Reading is not enough: the device paces all of its channels by the
        // cumulative byte count we send back. In particular, edit and flash
        // operations leave notifications on the events channel. Draining
        // those bytes and then dropping the interface without acknowledging
        // them carries an ever-growing debt into later short-lived sessions,
        // until an otherwise innocent write stops the pedal. There is no
        // transaction in flight at this boundary, so a final ACK is safe on
        // every channel that advanced.
        let _ = self.ack_pending(None);
        // No closing handshake. HX Edit sends a bare type-0x02 frame per
        // channel when it quits, and this used to imitate that - but the
        // capture behind it turned out to record HX Edit failing against an
        // already-wedged device, so it was never evidence of a clean teardown.
        // Sending it per session, rather than once when an application exits,
        // degrades the device: with it, a second consecutive run of the
        // hardware suite could not open the device at all; without it, the
        // suite runs repeatedly. Draining is enough.
    }
}

fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HX_DEBUG").is_some())
}

/// A deferred completion is identified by the transaction embedded in its
/// arguments, not by the outer notification id. Rename completes as event 1,
/// preset selection as event 20, and other operations use still other ids.
fn completion_matches(args: &Value, txn: i64) -> bool {
    args.get(rpc::key::TXN).and_then(Value::as_i64) == Some(txn)
        && args.get(rpc::key::STATUS).and_then(Value::as_i64) == Some(0)
}

fn decode_message(value: Value) -> Result<Message> {
    Message::try_from_value(value)
        .map_err(|error| Error::Protocol(format!("malformed RPC message: {error}")))
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The service each channel opens, as observed from HX Edit.
///
/// HX Edit opens a second service (2) on the control channel and sends its
/// control requests there. Replaying that was tried twice: it does not make
/// control-channel requests work, and it breaks the data channel as well, so
/// something about how we open the second service is wrong rather than the
/// idea being wrong. One service per channel is what currently works.
fn services(id: ChannelId) -> &'static [u16] {
    match id {
        ChannelId::CONTROL => &[5, 2],
        ChannelId::EVENTS => &[4],
        ChannelId::DATA => &[6],
        _ => &[],
    }
}

/// The service requests ride on: the last one opened.
fn service(id: ChannelId) -> u16 {
    services(id).last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Acknowledgements are a base plus the bytes consumed, not a bare count.
    /// Sending the count alone makes the device stop answering, so this is the
    /// arithmetic that matters most in the crate.
    #[test]
    fn acknowledgements_advance_from_a_base() {
        let mut channel = Channel::new();
        assert_eq!(channel.ack(), Channel::ACK_BASE);

        channel.rx_bytes += 256;
        assert_eq!(channel.ack(), Channel::ACK_BASE + 0x100);
        channel.rx_bytes += 256;
        assert_eq!(channel.ack(), Channel::ACK_BASE + 0x200);

        channel.rx_bytes = u32::MAX;
        assert_eq!(
            channel.ack(),
            Channel::ACK_BASE.wrapping_sub(1),
            "the protocol counter wraps instead of panicking after 4 GiB"
        );
    }

    #[test]
    fn transactions_start_at_the_observed_value_and_count_up() {
        let mut channel = Channel::new();
        assert_eq!(channel.next_txn(), rpc::FIRST_TXN);
        assert_eq!(channel.next_txn(), rpc::FIRST_TXN + 1);
    }

    #[test]
    fn deferred_completions_match_transaction_and_success_status() {
        let completion = hx_proto::msgmap! {
            rpc::key::TXN => Value::Int(1005),
            rpc::key::STATUS => Value::Int(0),
            rpc::key::RESULT => Value::Nil,
        };
        assert!(completion_matches(&completion, 1005));
        assert!(!completion_matches(&completion, 1006));

        let still_pending = hx_proto::msgmap! {
            rpc::key::TXN => Value::Int(1005),
            rpc::key::STATUS => Value::Int(1),
        };
        assert!(!completion_matches(&still_pending, 1005));
    }

    #[test]
    fn only_a_complete_device_refusal_leaves_the_session_usable() {
        assert!(!Error::Device(-3).loses_session());
        assert!(Error::Timeout(7).loses_session());
        assert!(Error::Protocol("out of sequence".into()).loses_session());
        assert!(Error::Usb("read timed out".into()).loses_session());
        assert!(Error::NotFound.loses_session());
        assert!(Error::Claim("busy".into()).loses_session());
    }

    /// The control channel opens two services in turn and talks on the second;
    /// the others open one. Sending control requests to service 5 instead of 2
    /// times out silently, which is easy to mistake for general flakiness.
    #[test]
    fn control_talks_on_its_second_service() {
        assert_eq!(services(ChannelId::CONTROL), &[5, 2]);
        assert_eq!(service(ChannelId::CONTROL), 2);

        assert_eq!(services(ChannelId::DATA), &[6]);
        assert_eq!(service(ChannelId::DATA), 6);
        assert_eq!(service(ChannelId::EVENTS), 4);
    }

    /// A large message is split at the size the device itself uses. Getting
    /// this wrong stalls the endpoint rather than failing cleanly.
    #[test]
    fn a_large_message_splits_into_device_sized_chunks() {
        let body = vec![0u8; 4096];
        let chunks: Vec<_> = body.chunks(Session::CHUNK).collect();
        assert_eq!(chunks.len(), 16);
        assert!(chunks.iter().all(|c| c.len() <= Session::CHUNK));
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), body.len());
    }

    /// Two real uploads captured from HX Edit, each with the sum it declared.
    #[test]
    fn checksum_matches_hx_edit() {
        // Four words summing with a deliberate wrap.
        let words: Vec<u8> = [0x1000_0000u32, 0x2000_0000, 0xF000_0000, 0x0000_0001]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        assert_eq!(checksum(&words), 0x2000_0001);
        assert_eq!(checksum(&[]), 0);
        // A trailing partial word is ignored, as chunks_exact implies.
        assert_eq!(checksum(&[1, 0, 0, 0, 9]), 1);
    }
}
