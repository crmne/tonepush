//! Layer 3 and 4: the per-channel byte stream and the MessagePack RPC on top.

use crate::msgpack::{Decoder, Encoder, Key, Value};
use std::fmt;

/// Field keys. The protocol uses bare integers, so naming them is the
/// difference between readable code and a wall of magic numbers.
pub mod key {
    /// Request: opcode.
    pub const OPCODE: i64 = 100;
    /// Request: arguments.
    pub const ARGS: i64 = 101;
    /// Request and response: transaction id.
    pub const TXN: i64 = 102;
    /// Response: how the request completed. Not an error code - a successful
    /// select-preset answers 1 and a preset read answers 0.
    pub const STATUS: i64 = 103;
    /// Response: result payload.
    pub const RESULT: i64 = 104;
    /// Signed error code inside a status-255 reply's result.
    pub const ERROR_CODE: i64 = 111;
    /// "Enabled" / "in effect", shared by the global-EQ and external-clock
    /// replies. Distinct from `ENABLED` (59), which bypasses a block.
    pub const IN_EFFECT: i64 = 63;
    /// Notification: event id.
    pub const EVENT: i64 = 105;
    /// Notification: event arguments.
    pub const EVENT_ARGS: i64 = 106;

    /// Setlist number.
    pub const SETLIST: i64 = 107;
    /// Preset index within the setlist, zero-based and linear - index 7 is the
    /// slot the front panel calls 03B, not "bank 7".
    pub const PRESET_INDEX: i64 = 108;
    pub const NAME: i64 = 109;
    pub const OBJECT_ID: i64 = 118;
    /// Rides with a favourite write, always true in every capture of one.
    /// **[inferred]** - its meaning is unknown, it is replayed as observed.
    pub const FAVOURITE_FLAG: i64 = 31;
    pub const VALUE: i64 = 119;
    /// Slot position in the signal chain.
    pub const BLOCK: i64 = 98;
    /// Parameter position within the block's model, in `Helix.sym` order.
    pub const PARAM_INDEX: i64 = 28;
    /// Signal path the block sits on. Zero on a single-path device.
    pub const PATH: i64 = 26;
    /// Present and true on every parameter write we have seen. Purpose
    /// unknown, so it is replayed rather than omitted.
    pub const COMMIT: i64 = 29;
    /// True when a block is in the signal path. Note the polarity: this is
    /// "enabled", not "bypassed", which is the opposite of how the front panel
    /// describes it.
    pub const ENABLED: i64 = 59;
    /// Model number, inside a model descriptor.
    pub const MODEL: i64 = 25;
    /// The model descriptor itself, on a set-model request. Note this reuses
    /// the number that means "opcode" at the top level of a message.
    pub const MODEL_REF: i64 = 100;
    /// Whether the slot holds a paired model, e.g. an amp with its cab.
    pub const PAIRED: i64 = 23;
    /// The paired model's number, or -1 for none.
    pub const PAIRED_MODEL: i64 = 26;
    /// Preset document on a write-preset request.
    pub const DOCUMENT: i64 = 110;
    /// Zero-based snapshot index.
    pub const SNAPSHOT: i64 = 92;

    /// MIDI CC number on a controller assignment.
    /// Routing destination on opcode 42.
    pub const ROUTING: i64 = 51;
    pub const CC: i64 = 71;
    /// What is being controlled. 5 is a block's bypass.
    pub const ASSIGN_TARGET: i64 = 95;
    /// Two further fields on an assignment whose meaning is not known; they
    /// were constant across every assignment captured.
    pub const ASSIGN_SCOPE: i64 = 96;
    /// Which controller drives a parameter, on the way *in* - see `Source`.
    pub const ASSIGN_FLAGS: i64 = 74;
    /// And which drives it on the way *out*. Opcode 36 does not answer with the
    /// key it was asked with: the reply puts the source ordinal at key 0.
    /// Confirmed by assigning through opcode 37 and watching this move while 74
    /// stayed absent. An unassigned parameter answers `nil` rather than a map.
    pub const ASSIGN_SOURCE: i64 = 0;
    /// Whether a footswitch is momentary rather than latching (opcodes 33, 58).
    pub const MOMENTARY: i64 = 65;
    /// A footswitch's LED colour as `0xRRGGBB`, and the colour of a block on a
    /// switch's assignment list (opcodes 33, 61).
    pub const LED_COLOUR: i64 = 66;
    /// What a footswitch controls: an array, one entry per assignment.
    pub const SWITCH_ASSIGNED: i64 = 67;
    /// Inside one of those entries, what it points at.
    pub const SWITCH_TARGET: i64 = 69;
    /// Footswitch index on opcodes 56, 57 and 33.
    pub const SWITCH: i64 = 102;
    /// Low and high ends of a controller's travel, normalised 0..1.
    pub const ASSIGN_MIN: i64 = 72;
    pub const ASSIGN_MAX: i64 = 73;
    /// The same number as [`CC`], and what it means depends on which shape of
    /// assignment message it is in. **[confirmed]**
    ///
    /// On a *parameter* assignment - the form carrying `{29, 26, 28}` - it is
    /// an on switch: 4 when an assignment exists and 0 when it is taken off,
    /// whatever the source. Captured against a footswitch and an expression
    /// pedal as well as MIDI, all of which send 4.
    ///
    /// On a *bypass* assignment - the form carrying [`ASSIGN_TARGET`] - it is
    /// the MIDI CC number itself. It looked like a constant 4 for a long time
    /// because 4 is the CC the pedal picks by default; setting the row to 42,
    /// then 43, 44, 45 and 46 sends exactly those, and 0 takes the row off.
    ///
    /// A parameter's CC does not travel here at all. It has its own opcode,
    /// [`op::SET_ASSIGN_CC`].
    pub const ASSIGN_KIND: i64 = 71;
    /// Constant false on every captured controller assignment.
    pub const ASSIGN_EXTRA: i64 = 129;

    /// Zero-based impulse response slot.
    pub const IR_SLOT: i64 = 112;
    /// Accompanies an IR upload. Looks like a checksum - it changed between two
    /// uploads of the same length - but the algorithm is not known, so it is
    /// sent as observed and may need to be right. **Unverified.**
    pub const IR_CHECKSUM: i64 = 113;
    /// Sample data: little-endian `f32`, mono.
    pub const IR_SAMPLES: i64 = 110;
    /// Format descriptors, seen as 1 and 3. Meaning not isolated.
    pub const IR_FORMAT_A: i64 = 114;
    pub const IR_FORMAT_B: i64 = 115;
}

/// Operation codes carried in [`key::OPCODE`].
pub mod op {
    /// `{107: setlist, 101: 2}` - every preset name in a setlist.
    pub const LIST_PRESETS: i64 = 1;
    /// `{107: setlist, 108: preset index}`
    pub const SELECT_PRESET: i64 = 20;
    /// Read the currently loaded preset as an `l6-helix` document.
    pub const READ_PRESET: i64 = 22;
    /// Metadata for the current preset: bank, slot and name.
    pub const PRESET_INFO: i64 = 23;
    /// `{118: id}` - fetch an object by id.
    pub const FETCH_OBJECT: i64 = 24;
    /// Write a device object - the mirror of [`FETCH_OBJECT`].
    /// `{118: object id, 119: value}`.
    pub const SET_OBJECT: i64 = 25;
    /// `{98: block, 29: true, 26: path, 28: index, 119: value}`
    pub const SET_PARAM: i64 = 30;
    /// `{98: block, 59: enabled}`
    pub const SET_ENABLED: i64 = 41;
    /// Route an endpoint: `{98: slot, 51: destination}`.
    pub const SET_ROUTING: i64 = 42;
    /// Is the tempo being driven by external MIDI clock? `{63: bool}`.
    pub const TEMPO_IS_EXTERNAL: i64 = 99;
    /// Global EQ: `{63: enabled, 55: [11 floats]}`.
    pub const GLOBAL_EQ: i64 = 76;
    /// `{107: setlist, 108: preset index, 109: name}`
    pub const RENAME_PRESET: i64 = 6;
    /// Commit the edit buffer to a preset slot: `{107, 108, 109: name}`.
    /// Without this, every edit lives only until the preset is reloaded.
    pub const SAVE_PRESET: i64 = 71;
    /// Setlist names. Control channel.
    pub const LIST_SETLISTS: i64 = 0;
    /// `{101: 2}` - impulse response slots. Control channel.
    pub const LIST_IRS: i64 = 13;
    /// Opens and closes each control-channel exchange in HX Edit's traffic.
    pub const BEGIN: i64 = 255;
    pub const END: i64 = 254;
    /// Sent once during control-channel setup; purpose unknown.
    pub const READY: i64 = 112;
    /// `{110: document}` - write a whole preset back.
    pub const WRITE_PRESET: i64 = 21;
    /// Assign a MIDI controller to a block's parameter.
    ///
    /// `{98: block, 95: target, 96: ?, 74: ?, 71: cc}`. Captured by stepping
    /// HX Edit's MIDI In field on a block's Bypass, where only key 71 moved -
    /// so that is the CC number and the rest describe what is being controlled.
    /// Keys 95, 96 and 74 are replayed as observed. **[inferred]**
    pub const ASSIGN_CONTROLLER: i64 = 37;
    /// Read a parameter's controller assignment: `{98, 29, 26, 28}`.
    pub const READ_ASSIGNMENT: i64 = 36;
    /// Which MIDI CC drives a *parameter*: `{98, 29, 26, 28, 71: cc}`.
    /// **[confirmed]**
    ///
    /// The question the assign page left open for a long time. A parameter's
    /// assignment message says only that a controller exists (key 71 is 4
    /// there); the number itself is set through here, and HX Edit sends one of
    /// these per intermediate value as the field is spun, the same way a knob
    /// drag streams. Captured landing on 42, then 43, then 44.
    ///
    /// A *bypass* is the other way round - its CC rides key 71 of
    /// [`ASSIGN_CONTROLLER`] and never comes through here.
    pub const SET_ASSIGN_CC: i64 = 64;
    /// The low end of a controller's travel: `{98, 29, 26, 28, 119: value}`.
    ///
    /// Min and Max are opcodes of their own rather than keys on the assign
    /// message - keys 72 and 73 are what a *read* returns, not what a write
    /// takes. Dragging either end streams one write per intermediate value, the
    /// way the global EQ does.
    pub const ASSIGN_MIN_OP: i64 = 65;
    /// The high end of a controller's travel. Same arguments as 65.
    pub const ASSIGN_MAX_OP: i64 = 66;
    /// Assign a block's bypass to a footswitch: `{98: block, 102: switch}`.
    pub const ASSIGN_FOOTSWITCH: i64 = 56;
    /// Take a block's bypass off a footswitch. Same arguments as 56.
    pub const UNASSIGN_FOOTSWITCH: i64 = 57;
    /// Read a footswitch's configuration: `{102: switch}`.
    pub const FOOTSWITCH_CONFIG: i64 = 33;
    /// `{102: switch, 65: momentary}` - latching or momentary.
    pub const SWITCH_TYPE: i64 = 58;
    /// `{102: switch, 109: label}` - the name written under your foot.
    pub const SWITCH_LABEL: i64 = 59;
    /// `{102: switch}` - clear the name again, back to what it carries.
    pub const SWITCH_LABEL_CLEAR: i64 = 60;
    /// `{102: switch, 66: colour}` - the LED colour, as an index into HX Edit's
    /// own `footswitchLED` list rather than an RGB value.
    pub const SWITCH_COLOUR: i64 = 61;
    /// `{102: switch}` - back to Auto Color, which is index 0 of that list and
    /// has an opcode of its own rather than a value.
    pub const SWITCH_COLOUR_AUTO: i64 = 62;
    /// `{98: block}` - empty a slot.
    pub const CLEAR_BLOCK: i64 = 28;
    /// Upload an impulse response. Control channel.
    pub const UPLOAD_IR: i64 = 9;
    /// `{112: slot}` - empty an impulse response slot. Control channel.
    pub const CLEAR_IR: i64 = 15;
    /// `{98: block, 100: {23: paired, 25: model, 26: second model or -1}}`
    pub const SET_MODEL: i64 = 40;
    /// `{98: block, 26: path}` - move the editing cursor, which the front panel
    /// follows.
    pub const SELECT_BLOCK: i64 = 78;
    /// `{92: index}` - switch snapshot.
    pub const SELECT_SNAPSHOT: i64 = 88;

    // ------------------------------------------------------ addressed slots ---
    // The fast path, read off HX Edit's own backup, restore and library exports
    // (see docs/backup-and-restore.md). Everything above works on the *loaded*
    // preset; these name the slot, so a whole-pedal backup never loads anything
    // and answers in about 50 ms a preset instead of the seconds a load costs.
    /// `{107: setlist, 108: index, 101: 2}` - read any slot's document.
    pub const FETCH_PRESET: i64 = 4;
    /// `{107, 108, 110: document}` - write a document straight into a slot.
    pub const WRITE_SLOT: i64 = 5;
    /// `{107, 108, 109: name, 110: document}` - write a slot and name it, which
    /// is what a paste or an import does.
    pub const WRITE_SLOT_NAMED: i64 = 8;
    /// `{107: setlist, 108: index}` - empty a slot.
    pub const CLEAR_SLOT: i64 = 16;

    // ------------------------------------------------------------ the store ---
    /// Stream one object out of the device's object store:
    /// `{64: id, 106: false}` to start, `{64: id, 106: true, 105: offset}` to
    /// continue. HX Edit's backup walks 803 ids this way.
    pub const FETCH_BLOB: i64 = 109;
    /// The write inverse of [`FETCH_BLOB`], same argument shape.
    pub const WRITE_BLOB: i64 = 111;

    // ----------------------------------------------------------- the globals ---
    /// Write the whole global-settings block as one msgpack blob. What a
    /// "restore global settings only" sends, in a single message.
    pub const WRITE_GLOBALS: i64 = 86;
    /// Global EQ reset - answers with the same shape as [`GLOBAL_EQ`].
    pub const GLOBAL_EQ_RESET: i64 = 77;

    // --------------------------------------------------------------- the IRs ---
    /// `{112: slot}` - an IR slot's descriptor: name, checksum and format, the
    /// same argument map [`UPLOAD_IR`] sends.
    pub const IR_DESCRIPTOR: i64 = 12;
    /// `{112: slot, 101: 2}` - an IR slot's samples. Everything comes back
    /// 48 kHz mono `f32`, always 2048 samples, whatever was uploaded.
    pub const IR_SAMPLES: i64 = 11;
    /// `{112: slot, 109: name}` - rename an IR slot.
    pub const RENAME_IR: i64 = 10;

    // ----------------------------------------------------------- favourites ---
    // A favourite is a block with its settings, kept by the device so it can be
    // dropped into any preset. Its own small family of opcodes, not object-store
    // entries. Control channel.
    /// List the favourites.
    pub const LIST_FAVOURITES: i64 = 112;
    /// `{98: block}` - read a block back in the shape a favourite is stored in.
    pub const READ_AS_FAVOURITE: i64 = 45;
    /// Read one favourite: `{118: index}`.
    pub const FETCH_FAVOURITE: i64 = 113;
    /// Keep a block as a favourite, the editor's own "save to favourites":
    /// `{98: block, 118: index, 31: true, 109: name}`. Control channel.
    pub const SAVE_FAVOURITE: i64 = 119;
    /// Write one favourite.
    pub const WRITE_FAVOURITE: i64 = 114;
    /// Empty a favourite slot.
    pub const CLEAR_FAVOURITE: i64 = 116;
    /// Rename a favourite.
    pub const RENAME_FAVOURITE: i64 = 117;
}

/// Transaction ids start here on each channel and count up.
pub const FIRST_TXN: i64 = 1000;

/// A decoded application-layer message.
#[derive(Debug, Clone)]
pub enum Message {
    Request {
        txn: i64,
        opcode: i64,
        args: Value,
    },
    Response {
        txn: i64,
        status: i64,
        result: Value,
    },
    Notification {
        event: i64,
        args: Value,
    },
    /// Well-formed MessagePack that matches none of the three shapes.
    Other(Value),
}

impl Message {
    pub fn from_value(v: Value) -> Message {
        let get = |k: i64| v.get(k).cloned().unwrap_or(Value::Nil);
        match (v.get(key::TXN), v.get(key::EVENT)) {
            (Some(t), _) if v.get(key::OPCODE).is_some() => Message::Request {
                txn: t.as_i64().unwrap_or(0),
                opcode: get(key::OPCODE).as_i64().unwrap_or(-1),
                args: get(key::ARGS),
            },
            (Some(t), _) if v.get(key::STATUS).is_some() => Message::Response {
                txn: t.as_i64().unwrap_or(0),
                status: get(key::STATUS).as_i64().unwrap_or(-1),
                result: get(key::RESULT),
            },
            (None, Some(e)) => Message::Notification {
                event: e.as_i64().unwrap_or(-1),
                args: get(key::EVENT_ARGS),
            },
            _ => Message::Other(v),
        }
    }

    /// Field order matches HX Edit's own - transaction id first - so encoded
    /// messages are byte-identical to captured ones.
    pub fn to_value(&self) -> Value {
        let m = match self {
            Message::Request { txn, opcode, args } => vec![
                (Key::Int(key::TXN), Value::Int(*txn)),
                (Key::Int(key::OPCODE), Value::Int(*opcode)),
                (Key::Int(key::ARGS), args.clone()),
            ],
            Message::Response {
                txn,
                status,
                result,
            } => vec![
                (Key::Int(key::TXN), Value::Int(*txn)),
                (Key::Int(key::STATUS), Value::Int(*status)),
                (Key::Int(key::RESULT), result.clone()),
            ],
            Message::Notification { event, args } => vec![
                (Key::Int(key::EVENT), Value::Int(*event)),
                (Key::Int(key::EVENT_ARGS), args.clone()),
            ],
            Message::Other(v) => return v.clone(),
        };
        Value::Map(m)
    }

    pub fn encode(&self) -> Vec<u8> {
        Encoder::encode(&self.to_value())
    }
}

/// Read a preset index from either a bare number or a front-panel label.
///
/// The tools print `03B`, so they should accept it. Requiring the reader to
/// convert back to 7 by hand is a small cruelty.
pub fn parse_slot(text: &str) -> Option<i64> {
    if let Ok(index) = text.trim().parse::<i64>() {
        return (index >= 0).then_some(index);
    }
    let text = text.trim();
    let (bank, slot) = text.split_at(text.len().checked_sub(1)?);
    let letter = slot.chars().next()?.to_ascii_uppercase();
    let position = "ABC".find(letter)? as i64;
    let bank: i64 = bank.parse().ok()?;
    (bank >= 1)
        .then_some(bank)
        .and_then(|bank| bank.checked_sub(1))
        .and_then(|bank| bank.checked_mul(3))
        .and_then(|index| index.checked_add(position))
}

/// Render a preset index the way the hardware labels it: `03B` for index 7.
pub fn slot_label(index: i64) -> String {
    if index < 0 {
        return index.to_string();
    }
    format!(
        "{:02}{}",
        index / 3 + 1,
        b"ABC"[(index % 3) as usize] as char
    )
}

// ------------------------------------------------------------ stream framing ---

/// One message in a channel's byte stream, with the 8-byte prefix that
/// delimits it.
#[derive(Debug, Clone)]
pub struct StreamMessage {
    pub flags: u16,
    pub service: u16,
    pub body: Value,
}

/// Reassembles a channel's byte stream and yields whole messages.
///
/// The stream is chunked arbitrarily across USB transfers, so a message can
/// straddle any number of them. Bytes are buffered until a complete
/// length-prefixed message is present.
#[derive(Default)]
pub struct StreamReader {
    buf: Vec<u8>,
    overflowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    TooLarge(usize),
    BufferOverflow,
    InvalidBody(String),
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(size) => write!(f, "stream message declares {size} bytes"),
            Self::BufferOverflow => write!(f, "stream buffer exceeded its safety limit"),
            Self::InvalidBody(error) => write!(f, "stream message is not MessagePack: {error}"),
        }
    }
}

impl std::error::Error for StreamError {}

const MAX_STREAM_MESSAGE: usize = 16 * 1024 * 1024;

impl StreamReader {
    fn reset_with(&mut self, error: StreamError) -> Result<Vec<StreamMessage>, StreamError> {
        self.buf.clear();
        self.overflowed = false;
        Err(error)
    }
}

impl StreamReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the bytes that followed a channel header.
    pub fn push(&mut self, bytes: &[u8]) {
        if self
            .buf
            .len()
            .checked_add(bytes.len())
            .is_none_or(|size| size > MAX_STREAM_MESSAGE + 8)
        {
            self.buf.clear();
            self.overflowed = true;
        } else if !self.overflowed {
            self.buf.extend_from_slice(bytes);
        }
    }

    /// Whole messages that have arrived, consuming them from the buffer.
    pub fn take_messages(&mut self) -> Result<Vec<StreamMessage>, StreamError> {
        if self.overflowed {
            return self.reset_with(StreamError::BufferOverflow);
        }
        let mut out = Vec::new();
        let mut pos = 0usize;
        while let Some(header_end) = pos.checked_add(8).filter(|end| *end <= self.buf.len()) {
            // Originator tag: 1 from the host, 0 from the device. Kept for
            // symmetry but never gated on.
            let flags = u16::from_le_bytes([self.buf[pos], self.buf[pos + 1]]);
            // Nominally the service id, but the device sends uninitialised
            // memory here on some replies - the same reply arrives with
            // different values across captures. Only the length can be
            // trusted, so this is carried, not checked.
            let service = u16::from_le_bytes([self.buf[pos + 2], self.buf[pos + 3]]);
            let len = u32::from_le_bytes([
                self.buf[pos + 4],
                self.buf[pos + 5],
                self.buf[pos + 6],
                self.buf[pos + 7],
            ]) as usize;
            if len > MAX_STREAM_MESSAGE {
                return self.reset_with(StreamError::TooLarge(len));
            }
            let Some(body_end) = header_end.checked_add(len) else {
                return self.reset_with(StreamError::TooLarge(len));
            };
            if self.buf.len() < body_end {
                break; // wait for more bytes
            }
            let body = &self.buf[header_end..body_end];
            let decoded = {
                let mut decoder = Decoder::new(body);
                match decoder.value() {
                    // Real replies carry an opaque 23-byte tail inside the
                    // declared body on roughly every other fresh session. The
                    // first MessagePack value is the payload; the remaining
                    // bytes are not a second value and are not canonical.
                    Ok(value) => Ok(value),
                    Err(error) => Err(StreamError::InvalidBody(error.to_string())),
                }
            };
            match decoded {
                Ok(body) => out.push(StreamMessage {
                    flags,
                    service,
                    body,
                }),
                Err(error) => return self.reset_with(error),
            }
            pos = body_end;
        }
        self.buf.drain(..pos);
        Ok(out)
    }
}

/// What can drive a parameter, in the order HX Edit lists the sources.
///
/// The wire carries the ordinal, so the order is the protocol rather than a
/// presentation choice: 1 is EXP Pedal 1 and 9 is Snapshots on every device
/// seen. `None` is the absence of an assignment and has no ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Expression(u8),
    Footswitch(u8),
    MidiCc,
    Snapshots,
}

impl Source {
    /// The ordinal for "nothing controls this". Removing an assignment is not
    /// a separate opcode: it is opcode 37 with `{74: 0, 71: 0}`.
    pub const NONE: i64 = 0;

    /// The ordinal the device expects under `key::ASSIGN_FLAGS`.
    pub fn ordinal(self) -> i64 {
        match self {
            // Two expression pedals, then five footswitches, then the rest.
            Source::Expression(n) => n.clamp(1, 2) as i64,
            Source::Footswitch(n) => 2 + n.clamp(1, 5) as i64,
            Source::MidiCc => 8,
            Source::Snapshots => 9,
        }
    }

    pub fn from_ordinal(n: i64) -> Option<Source> {
        match n {
            1..=2 => Some(Source::Expression(n as u8)),
            3..=7 => Some(Source::Footswitch((n - 2) as u8)),
            8 => Some(Source::MidiCc),
            9 => Some(Source::Snapshots),
            _ => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            Source::Expression(n) => format!("Expression Pedal {n}"),
            Source::Footswitch(n) => format!("Footswitch {n}"),
            Source::MidiCc => "MIDI CC".into(),
            Source::Snapshots => "Snapshots".into(),
        }
    }

    /// The same name with the space taken out, for a tag on a block where
    /// there is room for four characters and no more.
    pub fn short(self) -> String {
        match self {
            Source::Expression(n) => format!("EXP{n}"),
            Source::Footswitch(n) => format!("FS{n}"),
            Source::MidiCc => "MIDI".into(),
            Source::Snapshots => "SNAP".into(),
        }
    }

    /// Whether this source can drive a block's on/off.
    ///
    /// Bypass is a switch, so a pedal that sweeps cannot drive it, and
    /// snapshots carry a block's state themselves rather than through an
    /// assignment. HX Edit lists pedals here and then steps over them, which is
    /// worse than not offering them.
    pub fn switches(self) -> bool {
        matches!(self, Source::Footswitch(_) | Source::MidiCc)
    }

    /// Every source, for offering a choice.
    pub fn all() -> Vec<Source> {
        (1..=9).filter_map(Source::from_ordinal).collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sources_round_trip_through_their_ordinals() {
        use super::Source;
        for n in 1..=9 {
            let source = Source::from_ordinal(n).expect("a source");
            assert_eq!(source.ordinal(), n, "{source:?} does not round trip");
        }
        // The ordering is the protocol's, captured from HX Edit.
        assert_eq!(Source::from_ordinal(1), Some(Source::Expression(1)));
        assert_eq!(Source::from_ordinal(3), Some(Source::Footswitch(1)));
        assert_eq!(Source::from_ordinal(8), Some(Source::MidiCc));
        assert_eq!(Source::all().len(), 9);
    }

    use super::*;

    /// A real select-preset request captured from HX Edit.
    const SELECT_PRESET: &[u8] = &[
        0x83, 0x66, 0xcd, 0x03, 0xf1, 0x64, 0x14, 0x65, 0x82, 0x6b, 0x00, 0x6c, 0x0c,
    ];

    #[test]
    fn recognises_a_request() {
        let v = Decoder::new(SELECT_PRESET).value().unwrap();
        match Message::from_value(v) {
            Message::Request { txn, opcode, args } => {
                assert_eq!((txn, opcode), (1009, op::SELECT_PRESET));
                assert_eq!(args.get(key::PRESET_INDEX).unwrap().as_i64(), Some(12));
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn a_request_round_trips_byte_for_byte() {
        // Encoding must reproduce HX Edit's own field order, or captured
        // traffic and generated traffic stop being comparable.
        let request = Message::Request {
            txn: 1009,
            opcode: op::SELECT_PRESET,
            args: crate::msgmap! {
                key::SETLIST => Value::Int(0),
                key::PRESET_INDEX => Value::Int(12),
            },
        };
        assert_eq!(request.encode(), SELECT_PRESET);

        let back = Message::from_value(Decoder::new(&request.encode()).value().unwrap());
        assert!(matches!(
            back,
            Message::Request {
                txn: 1009,
                opcode: 20,
                ..
            }
        ));
    }

    #[test]
    fn stream_reader_waits_for_a_whole_message() {
        // {102: 1000} preceded by its 8-byte stream prefix.
        let body = Encoder::encode(&crate::msgmap! { key::TXN => Value::Int(1000) });
        let mut framed = vec![0x00, 0x00, 0x06, 0x00];
        framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
        framed.extend_from_slice(&body);

        let mut r = StreamReader::new();
        // Split mid-message: nothing should come out until it is complete.
        r.push(&framed[..6]);
        assert!(r.take_messages().unwrap().is_empty());
        r.push(&framed[6..]);
        let msgs = r.take_messages().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].service, 6);
        assert_eq!(msgs[0].body.get(key::TXN).unwrap().as_i64(), Some(1000));
    }

    #[test]
    fn stream_reader_rejects_unbounded_messages_and_recovers_from_bad_bodies() {
        let mut oversized = StreamReader::new();
        let mut header = vec![0; 8];
        header[4..8].copy_from_slice(&((MAX_STREAM_MESSAGE as u32) + 1).to_le_bytes());
        oversized.push(&header);
        assert!(matches!(
            oversized.take_messages(),
            Err(StreamError::TooLarge(_))
        ));

        let mut trailing = StreamReader::new();
        let body = [0xc0, 0xc0]; // two nil values in a one-value message
        let mut framed = vec![0; 4];
        framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
        framed.extend_from_slice(&body);
        trailing.push(&framed);
        assert_eq!(trailing.take_messages().unwrap().len(), 1);

        let mut malformed = StreamReader::new();
        malformed.push(&[0, 0, 0, 0, 1, 0, 0, 0, 0xc1]);
        assert!(matches!(
            malformed.take_messages(),
            Err(StreamError::InvalidBody(_))
        ));

        // An error clears the poisoned state instead of retaining it and
        // growing forever; the next well-formed message is usable.
        malformed.push(&[0, 0, 0, 0, 1, 0, 0, 0, 0xc0]);
        assert_eq!(malformed.take_messages().unwrap().len(), 1);
    }

    #[test]
    fn slot_labels_parse_back_to_their_index() {
        for index in [0, 7, 12, 125] {
            assert_eq!(parse_slot(&slot_label(index)), Some(index), "index {index}");
        }
        assert_eq!(parse_slot("03b"), Some(7));
        assert_eq!(parse_slot("7"), Some(7));
        assert_eq!(parse_slot(" 03B "), Some(7));
        assert_eq!(parse_slot("00A"), None);
        assert_eq!(parse_slot("03D"), None);
        assert_eq!(parse_slot("nonsense"), None);
        assert_eq!(parse_slot(""), None);
        assert_eq!(parse_slot("9223372036854775807A"), None);
    }

    #[test]
    fn slot_labels_match_the_front_panel() {
        // {107: 0, 108: 7} was captured as "CT-Sad", which the device shows as 03B.
        assert_eq!(slot_label(0), "01A");
        assert_eq!(slot_label(7), "03B");
        assert_eq!(slot_label(125), "42C");
        assert_eq!(slot_label(-1), "-1");
    }
}
