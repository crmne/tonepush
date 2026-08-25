//! The operations an editor performs: presets, blocks, parameters, snapshots
//! and impulse responses.
//!
//! Split from the transport deliberately. Everything here is a thin call onto
//! `Session::request` - the interesting code is the channel protocol next door,
//! and mixing the two made one file read as a protocol engine with a service
//! catalogue stapled on.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use hx_proto::msgpack::Value;
use hx_proto::rpc::Message;
use hx_proto::{rpc, ChannelId, DeviceProfile, Preset};

use crate::{checksum, Error, Result, Session};

fn non_negative(value: i64, name: &str) -> Result<()> {
    if value < 0 {
        return Err(Error::Protocol(format!("{name} cannot be negative")));
    }
    Ok(())
}

fn block_param(block: i64, param: i64) -> Result<()> {
    non_negative(block, "block")?;
    non_negative(param, "parameter index")
}

fn finite_value(value: &Value, name: &str) -> Result<()> {
    let finite = match value {
        Value::F32(number) => number.is_finite(),
        Value::F64(number) => number.is_finite(),
        _ => true,
    };
    if !finite {
        return Err(Error::Protocol(format!("{name} must be a finite number")));
    }
    Ok(())
}

fn switch_index(profile: &DeviceProfile, switch: u8) -> Result<i64> {
    if !(1..=profile.switches).contains(&switch) {
        return Err(Error::Protocol(format!(
            "{} has footswitches 1 to {}; {switch} is not one",
            profile.name, profile.switches
        )));
    }
    Ok(i64::from(switch - 1))
}

/// What controls a parameter, and over what part of its travel.
///
/// The ends are normalised: 0.0 is the parameter's own minimum and 1.0 its
/// maximum, so an expression pedal set to sweep the middle third reads
/// `min: 0.33, max: 0.66` whatever units the parameter is shown in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assignment {
    pub source: rpc::Source,
}

/// A footswitch, as the pedal describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Switch {
    /// One-based, the way it is printed on the pedal.
    pub switch: u8,
    /// Momentary holds while your foot is down; latching toggles.
    pub momentary: bool,
    /// A name typed for it, if one has been.
    pub label: Option<String>,
    /// A colour chosen for it. `None` is Auto Color, where the switch takes
    /// the colour of whatever it controls.
    pub colour: Option<i64>,
    /// What it controls. Usually one thing, sometimes several.
    pub carries: Vec<Carried>,
}

impl Switch {
    /// What to call this switch: the name typed for it, or what it controls,
    /// or nothing at all.
    pub fn describes(&self) -> Option<&str> {
        if let Some(label) = self.label.as_deref() {
            return Some(label);
        }
        self.carries.first().map(|c| c.name.as_str())
    }

    /// The colour to light it: the one chosen, or the one its block wears.
    pub fn lit(&self) -> Option<i64> {
        self.colour
            .or_else(|| self.carries.first().and_then(|c| c.colour))
    }
}

/// One thing a footswitch controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Carried {
    /// The block, as the device numbers them.
    pub block: i64,
    /// Its name, which the device sends along rather than making us look it up.
    pub name: String,
    /// The colour the device gives it, `0xRRGGBB`. This is the block's own
    /// category colour, which is what makes an Auto Color switch match the
    /// block it toggles.
    pub colour: Option<i64>,
    pub enabled: bool,
}

/// One entry of a switch's assignment list.
fn carried(item: &Value) -> Option<Carried> {
    let target = item.get(rpc::key::SWITCH_TARGET)?;
    Some(Carried {
        block: target.get(rpc::key::BLOCK).and_then(|v| v.as_i64())?,
        name: target
            .get(rpc::key::NAME)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        colour: item.get(rpc::key::LED_COLOUR).and_then(|v| v.as_i64()),
        enabled: item
            .get(rpc::key::ENABLED)
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

impl Session {
    /// Every preset in a setlist, in order.
    pub fn presets(&mut self, setlist: i64) -> Result<Vec<String>> {
        let result = self.request(
            ChannelId::CONTROL,
            rpc::op::LIST_PRESETS,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::ARGS => Value::Int(2),
            },
        )?;
        let Value::Array(entries) = result else {
            return Err(Error::Protocol("preset list was not an array".into()));
        };
        // Each entry is a single-key map from preset index to its details, so
        // the name is one level in regardless of what the index is.
        entries
            .into_iter()
            .enumerate()
            .map(|(position, entry)| match entry {
                Value::Map(fields) => fields
                    .first()
                    .and_then(|(_, v)| v.get(rpc::key::NAME))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        Error::Protocol(format!(
                            "preset list entry {position} has no readable name"
                        ))
                    }),
                _ => Err(Error::Protocol(format!(
                    "preset list entry {position} is not a map"
                ))),
            })
            .collect()
    }

    /// Set one parameter on one block.
    ///
    /// The value is in the parameter's own units; `hx-catalog` knows the range
    /// and how to display it.
    pub fn set_param(&mut self, block: i64, index: i64, value: Value) -> Result<()> {
        block_param(block, index)?;
        finite_value(&value, "parameter value")?;
        self.command(
            ChannelId::DATA,
            rpc::op::SET_PARAM,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::COMMIT => Value::Bool(true),
                rpc::key::PATH => Value::Int(0),
                rpc::key::PARAM_INDEX => Value::Int(index),
                rpc::key::VALUE => value,
            },
        )
    }

    /// Impulse response slots, as `(slot, name)`.
    ///
    /// Also the way to tell that an upload has finished: the device reports the
    /// new name here once it has written it.
    pub fn irs(&mut self) -> Result<Vec<(i64, String)>> {
        let result = self.request(
            ChannelId::CONTROL,
            rpc::op::LIST_IRS,
            hx_proto::msgmap! { rpc::key::ARGS => Value::Int(2) },
        )?;
        let entries = match result {
            Value::Array(entries) => entries,
            // The captured device response for no occupied slots is nil rather
            // than an empty array.
            Value::Nil => return Ok(Vec::new()),
            _ => {
                return Err(Error::Protocol(
                    "the device returned an invalid impulse response list".into(),
                ))
            }
        };
        let mut seen = BTreeSet::new();
        entries
            .into_iter()
            .enumerate()
            .map(|(position, entry)| {
                let slot = entry
                    .get(rpc::key::IR_SLOT)
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        Error::Protocol(format!("IR list entry {position} has no slot"))
                    })?;
                let name = entry
                    .get(rpc::key::NAME)
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::Protocol(format!("IR list entry {position} has no name"))
                    })?;
                if slot < 0 || !seen.insert(slot) {
                    return Err(Error::Protocol(format!(
                        "IR list entry {position} has an invalid or duplicate slot {slot}"
                    )));
                }
                Ok((slot, name.to_owned()))
            })
            .collect()
    }

    /// Read an impulse response back off the device.
    ///
    /// The other half of [`upload_ir`](Self::upload_ir), and the piece a backup
    /// needs: without it an IR that only ever existed on the pedal could not be
    /// saved anywhere. Two opcodes, the way the editor's own export does it -
    /// op12 for the descriptor and op11 for the samples.
    ///
    /// What comes back is what the device stores: 48 kHz mono `f32`, as many
    /// samples as the upload declared. Rate conversion is a host-side concern;
    /// [`crate::ir::prepare`] performs it before an ordinary WAV is uploaded.
    /// `None` is an empty slot.
    pub fn read_ir(&mut self, slot: i64) -> Result<Option<(String, Vec<f32>)>> {
        self.bootstrap()?;
        let descriptor = self.request(
            ChannelId::CONTROL,
            rpc::op::IR_DESCRIPTOR,
            hx_proto::msgmap! { rpc::key::IR_SLOT => Value::Int(slot) },
        )?;
        let Some(name) = descriptor.get(rpc::key::NAME).and_then(Value::as_str) else {
            return Ok(None);
        };
        let name = name.to_owned();

        let samples = self.request(
            ChannelId::CONTROL,
            rpc::op::IR_SAMPLES,
            hx_proto::msgmap! {
                rpc::key::IR_SLOT => Value::Int(slot),
                rpc::key::ARGS => Value::Int(2),
            },
        )?;
        let Some(bytes) = samples.as_raw() else {
            return Err(Error::Protocol(
                "the device returned invalid impulse response samples".into(),
            ));
        };
        Ok(Some((name, decode_ir_samples(bytes)?)))
    }

    /// Rename an impulse response slot, leaving its samples alone.
    pub fn rename_ir(&mut self, slot: i64, name: &str) -> Result<()> {
        self.bootstrap()?;
        self.command(
            ChannelId::CONTROL,
            rpc::op::RENAME_IR,
            hx_proto::msgmap! {
                rpc::key::IR_SLOT => Value::Int(slot),
                rpc::key::NAME => Value::Str(name.to_owned()),
            },
        )?;
        self.settle_flash();
        Ok(())
    }

    /// The device's favourite blocks, as `(index, name)`.
    ///
    /// A favourite is a block kept with its settings so it can be dropped into
    /// any preset - the editor's own shelf, living on the pedal rather than in
    /// this program. Distinct from TonePush's favourite *presets*, which are
    /// a local file.
    pub fn favourites(&mut self) -> Result<Vec<(i64, String)>> {
        let result = self.request(ChannelId::DATA, rpc::op::LIST_FAVOURITES, Value::Nil)?;
        let Value::Array(entries) = result else {
            return Ok(Vec::new());
        };
        Ok(entries
            .iter()
            .filter_map(|e| {
                Some((
                    e.get(rpc::key::OBJECT_ID)?.as_i64()?,
                    e.get(rpc::key::NAME)?.as_str()?.to_owned(),
                ))
            })
            .collect())
    }

    /// Keep a block as a favourite, under a name.
    ///
    /// `block` is a position in the loaded chain; `index` is which favourite
    /// slot to put it in. This is the one message the editor sends when you
    /// choose "save as favourite", and it reads the block itself - there is
    /// nothing to send but where it is and what to call it.
    pub fn save_favourite(&mut self, block: i64, index: i64, name: &str) -> Result<()> {
        self.bootstrap()?;
        self.command(
            ChannelId::CONTROL,
            rpc::op::SAVE_FAVOURITE,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::OBJECT_ID => Value::Int(index),
                rpc::key::FAVOURITE_FLAG => Value::Bool(true),
                rpc::key::NAME => Value::Str(name.to_owned()),
            },
        )?;
        self.settle_flash();
        Ok(())
    }

    /// Rename a favourite.
    pub fn rename_favourite(&mut self, index: i64, name: &str) -> Result<()> {
        self.command(
            ChannelId::DATA,
            rpc::op::RENAME_FAVOURITE,
            hx_proto::msgmap! {
                rpc::key::OBJECT_ID => Value::Int(index),
                rpc::key::NAME => Value::Str(name.to_owned()),
            },
        )?;
        self.settle_flash();
        Ok(())
    }

    /// Forget a favourite.
    pub fn clear_favourite(&mut self, index: i64) -> Result<()> {
        self.command(
            ChannelId::DATA,
            rpc::op::CLEAR_FAVOURITE,
            hx_proto::msgmap! { rpc::key::OBJECT_ID => Value::Int(index) },
        )?;
        self.settle_flash();
        Ok(())
    }

    /// Bring the control channel up to the state HX Edit leaves it in.
    ///
    /// Opening the service is not enough: HX Edit follows it with a fixed
    /// sequence - end, setlists, presets, ready, list IRs - and the device
    /// will not service an IR upload without it. Cheap enough to do before any
    /// control-channel work.
    fn bootstrap(&mut self) -> Result<()> {
        let c = ChannelId::CONTROL;
        self.command(c, rpc::op::END, hx_proto::msgmap! {})?;
        self.request(c, rpc::op::LIST_SETLISTS, Value::Nil)?;
        self.request(
            c,
            rpc::op::LIST_PRESETS,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(0),
                rpc::key::ARGS => Value::Int(2),
            },
        )?;
        self.request(c, rpc::op::READY, Value::Nil)?;
        self.request(
            c,
            rpc::op::LIST_IRS,
            hx_proto::msgmap! { rpc::key::ARGS => Value::Int(2) },
        )?;
        self.command(c, rpc::op::BEGIN, hx_proto::msgmap! {})
    }

    /// Send an impulse response to a slot.
    ///
    /// Samples are mono `f32`. The IR is one RPC message on the control
    /// channel - 4 KB of samples for a 1024-sample file - which the transport
    /// then splits across frames like any other large message.
    ///
    /// The checksum field is reproduced from a capture and its algorithm is
    /// unknown; if the device rejects an upload, that is the first thing to
    /// suspect.
    pub fn upload_ir(&mut self, slot: i64, name: &str, samples: &[f32]) -> Result<()> {
        non_negative(slot, "impulse response slot")?;
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Protocol(
                "an impulse response contains a non-finite sample".into(),
            ));
        }
        // The descriptor declares the stored length as 256 × 2^code samples
        // (key 115; key 114 is a multiplier the editor always sends as 1).
        // The device zero-pads shorter data to the declared length - but data
        // *longer* than declared wedges its transfer state machine hard enough
        // to need the 9V adapter pulled, so the length is checked here rather
        // than discovered there.
        let code = match samples.len() {
            0 => return Err(Error::Protocol("an empty impulse response".into())),
            1..=1024 => 2,
            1025..=2048 => 3,
            n => {
                return Err(Error::Protocol(format!(
                    "{n} samples will not fit; the device stores at most 2048"
                )))
            }
        };

        self.bootstrap()?;
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.command(
            ChannelId::CONTROL,
            rpc::op::UPLOAD_IR,
            hx_proto::msgmap! {
                rpc::key::IR_SLOT => Value::Int(slot),
                rpc::key::IR_CHECKSUM => Value::UInt(checksum(&bytes)),
                rpc::key::NAME => Value::Str(name.to_owned()),
                rpc::key::IR_FORMAT_A => Value::Int(1),
                rpc::key::IR_FORMAT_B => Value::Int(code),
                123 => Value::Bool(false),
                124 => Value::Bool(false),
                125 => Value::Int(0),
                rpc::key::IR_SAMPLES => Value::Bin(bytes, 2),
            },
        )?;

        // Opcode 9 answers "accepted", not "done": the device writes the IR to
        // flash afterwards and shows "transferring data" while it does. HX Edit
        // closes the operation with an end marker and then re-reads the slot
        // list, and doing the same is what takes the device out of that state -
        // simply waiting does not.
        self.command(ChannelId::CONTROL, rpc::op::END, hx_proto::msgmap! {})?;

        // Poll the slot list until our name shows up. That is the only honest
        // signal that the write finished rather than merely being accepted, and
        // it is what the device's "transferring data" display is tracking -
        // returning before it appears is what used to leave the unit stuck.
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self
                .irs()?
                .iter()
                .any(|(s, n)| *s == slot && n.trim() == name.trim())
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        Err(Error::Protocol(format!(
            "the device accepted the impulse response but slot {} still does not show {name:?}; \
             it may still be writing",
            slot + 1
        )))
    }

    /// Empty an impulse response slot.
    pub fn clear_ir(&mut self, slot: i64) -> Result<()> {
        non_negative(slot, "impulse response slot")?;
        self.bootstrap()?;
        self.command(
            ChannelId::CONTROL,
            rpc::op::CLEAR_IR,
            hx_proto::msgmap! { rpc::key::IR_SLOT => Value::Int(slot) },
        )?;
        self.command(ChannelId::CONTROL, rpc::op::END, hx_proto::msgmap! {})?;
        self.irs()?;
        Ok(())
    }

    /// Empty a slot, removing whatever block is in it.
    ///
    /// The device wants the editing cursor moved to the block first. Sending
    /// the clear alone is answered as though it succeeded and changes nothing,
    /// which is a quietly misleading combination - HX Edit always selects then
    /// clears, and so do we.
    pub fn clear_block(&mut self, block: i64) -> Result<()> {
        non_negative(block, "block")?;
        self.select_block(block)?;
        self.command(
            ChannelId::DATA,
            rpc::op::CLEAR_BLOCK,
            hx_proto::msgmap! { rpc::key::BLOCK => Value::Int(block) },
        )
    }

    /// Switch to a snapshot, by zero-based index.
    pub fn select_snapshot(&mut self, index: i64) -> Result<()> {
        non_negative(index, "snapshot index")?;
        self.command(
            ChannelId::DATA,
            rpc::op::SELECT_SNAPSHOT,
            hx_proto::msgmap! { rpc::key::SNAPSHOT => Value::Int(index) },
        )
    }

    /// Write a whole preset document back to the device, and do not return
    /// until it has demonstrably landed.
    ///
    /// Several operations - reordering blocks, switching snapshots, pasting a
    /// preset - have no dedicated opcode. This is how HX Edit performs its
    /// undo, and it is the general mechanism for anything the opcode table does
    /// not cover.
    ///
    /// The commit is verified by reading the chain back, because acceptance is
    /// not landing: the completion notification is sometimes missed, and the
    /// device answers other questions happily while the commit is still in
    /// flight. A session that closes in that window - any one-shot CLI
    /// command - leaves the device holding a half-committed document, and it
    /// resolves that by wiping the edit buffer. Verified against the
    /// hardware; the read-back loop is what makes a write safe to be the
    /// last thing a process does.
    pub fn write_preset(&mut self, preset: &Preset) -> Result<()> {
        // An empty branch must go out the way the device itself would keep
        // it - attach points zeroed - or the document contradicts itself in a
        // way the device settles by wiping the edit buffer. Normalised here,
        // at the one place every document leaves through.
        let mut settled = Preset::parse(&preset.encode())
            .ok_or_else(|| Error::Protocol("the document to write does not re-parse".into()))?;
        settled.settle_branches();
        let preset = &settled;

        // Deferred: the device accepts the document and then commits it. The
        // next operation must wait for the completion notification, or the
        // commits pile up and the device eventually stops taking writes.
        self.command_deferred(
            ChannelId::DATA,
            rpc::op::WRITE_PRESET,
            hx_proto::msgmap! { rpc::key::DOCUMENT => Value::Bin(preset.encode(), 2) },
        )?;

        // What the chain should look like once the document lands. Kinds and
        // models rather than bytes, in case the device re-serialises.
        let fingerprint =
            |p: &Preset| -> Vec<_> { p.slots.iter().map(|s| (s.kind, s.model)).collect() };
        let want = fingerprint(preset);
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Ok(back) = self.read_preset() {
                if fingerprint(&back) == want {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Protocol(
                    "the device accepted a document and never showed it back".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Give a flash write time to commit before the next one is sent.
    ///
    /// Rename and save are flash writes: the device replies at once but commits
    /// a moment later, and firing the next flash write into that window stacks
    /// racing commits until the transfer state machine jams. That is not a
    /// theoretical edge - a burst of renames once corrupted a whole setlist past
    /// what a power cycle could clear, and only a factory reset recovered it. A
    /// pause here paces flash writes the way HX Edit's own gaps do. Captured
    /// commits take about 300 ms; this is generous over that. Verifying by
    /// reading the result back, the way [`write_preset`](Self::write_preset)
    /// does, is stronger still and is the next step once the command transcript
    /// can be re-captured against hardware.
    pub(crate) fn settle_flash(&self) {
        std::thread::sleep(Duration::from_millis(750));
    }

    /// Commit the edit buffer to a preset slot.
    ///
    /// Everything else in this API edits the device's *edit buffer*: change a
    /// parameter and the device sounds different immediately, but reload the
    /// preset and the change is gone. This is the operation that makes an edit
    /// permanent, and it is HX Edit's File > Save Preset.
    pub fn save_preset(&mut self, setlist: i64, index: i64, name: &str) -> Result<()> {
        self.command(
            ChannelId::DATA,
            rpc::op::SAVE_PRESET,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::PRESET_INDEX => Value::Int(index),
                rpc::key::NAME => Value::Str(name.to_owned()),
            },
        )?;
        // A save landing back-to-back with the next slot's save - as a restore
        // writes preset after preset - must not stack its commit onto this one.
        self.settle_flash();
        Ok(())
    }

    /// Read one device object - a global setting, by numeric id.
    pub fn object(&mut self, id: i64) -> Result<Value> {
        let v = self.request(
            ChannelId::DATA,
            rpc::op::FETCH_OBJECT,
            hx_proto::msgmap! { rpc::key::OBJECT_ID => Value::Int(id) },
        )?;
        v.get(rpc::key::VALUE)
            .cloned()
            .ok_or_else(|| Error::Protocol(format!("object {id} answered without a value")))
    }

    /// Write one device object.
    ///
    /// Global settings live in a flat numbered namespace rather than a
    /// structured document: 147 of the first 160 ids answer on an HX Stomp.
    /// The value's type has to match what the device holds - sending a float
    /// where it wants a boolean is refused with error -3.
    pub fn set_object(&mut self, id: i64, value: Value) -> Result<()> {
        non_negative(id, "object id")?;
        finite_value(&value, "object value")?;
        self.command(
            ChannelId::DATA,
            rpc::op::SET_OBJECT,
            hx_proto::msgmap! {
                rpc::key::OBJECT_ID => Value::Int(id),
                rpc::key::VALUE => value,
            },
        )
    }

    /// Whether the tempo is currently driven by external MIDI clock.
    ///
    /// HX Edit replaces its BPM readout with "[External]" when this is true,
    /// which is how the flag was identified: patching the reply to true in
    /// flight changed that display, and sending real MIDI beat clock to the
    /// device flips it for as long as the clock runs.
    pub fn tempo_is_external(&mut self) -> Result<bool> {
        let v = self.request(
            ChannelId::DATA,
            rpc::op::TEMPO_IS_EXTERNAL,
            hx_proto::msgmap! {},
        )?;
        Ok(v.get(rpc::key::IN_EFFECT)
            .and_then(|e| match e {
                Value::Bool(b) => Some(*b),
                _ => None,
            })
            .unwrap_or(false))
    }

    /// Point an input or output somewhere else - opcode 42, `{98: slot, 51:
    /// destination}`. The destination indexes the same menu the preset stores
    /// under the slot's routing key; changing it through a document write is
    /// ignored, and this opcode, captured from HX Edit's own routing clicks,
    /// is the way that works.
    pub fn set_routing(&mut self, block: i64, to: i64) -> Result<()> {
        non_negative(block, "block")?;
        non_negative(to, "routing destination")?;
        self.command(
            ChannelId::DATA,
            rpc::op::SET_ROUTING,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::ROUTING => Value::Int(to),
            },
        )
    }

    /// Rename a preset.
    ///
    /// This is a flash write, and an unpaced one is what corrupted a setlist
    /// into needing a factory reset: the GUI fired a rename and immediately read
    /// the list and the document back, and a burst of them stacked commits until
    /// the device jammed. So the write is paced - see [`settle_flash`] - and the
    /// caller must not follow it with a document reload; a rename changes a
    /// slot's label, never its tone.
    ///
    /// [`settle_flash`]: Self::settle_flash
    pub fn rename_preset(&mut self, setlist: i64, index: i64, name: &str) -> Result<()> {
        self.command(
            ChannelId::DATA,
            rpc::op::RENAME_PRESET,
            hx_proto::msgmap! {
                rpc::key::SETLIST => Value::Int(setlist),
                rpc::key::PRESET_INDEX => Value::Int(index),
                rpc::key::NAME => Value::Str(name.to_owned()),
            },
        )?;
        self.settle_flash();
        Ok(())
    }

    /// Change what a block is.
    ///
    /// Like clearing, this needs the editing cursor on the block first - see
    /// [`Session::clear_block`] for why that matters.
    pub fn set_model(&mut self, block: i64, model: u32) -> Result<()> {
        self.set_model_ref(block, model, None)
    }

    /// Make a block an Amp+Cab: an amp with its cab riding along in the same
    /// slot, each keeping its own parameters.
    ///
    /// The pairing is the amp's, not a free choice - `amp.models` gives every
    /// amp a `cablink`, and `hx_catalog::Catalog::paired_cab` resolves it.
    pub fn set_model_pair(&mut self, block: i64, amp: u32, cab: u32) -> Result<()> {
        self.set_model_ref(block, amp, Some(cab))
    }

    fn set_model_ref(&mut self, block: i64, model: u32, paired: Option<u32>) -> Result<()> {
        self.select_block(block)?;
        self.command(
            ChannelId::DATA,
            rpc::op::SET_MODEL,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::MODEL_REF => hx_proto::msgmap! {
                    rpc::key::PAIRED => Value::Bool(paired.is_some()),
                    rpc::key::MODEL => Value::Int(model as i64),
                    rpc::key::PAIRED_MODEL => Value::Int(paired.map_or(-1, |p| p as i64)),
                },
            },
        )
    }

    /// Move the editing cursor, which the device's own screen follows.
    pub fn select_block(&mut self, block: i64) -> Result<()> {
        non_negative(block, "block")?;
        self.command(
            ChannelId::DATA,
            rpc::op::SELECT_BLOCK,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::PATH => Value::Int(0),
            },
        )
    }

    /// Put a block's bypass under MIDI, or take it back off.
    ///
    /// Put a block's bypass under a MIDI CC, and say which one.
    ///
    /// HX Edit's assign page gives MIDI its own row rather than a place in the
    /// source list, and this is that row. `None` takes the row off.
    ///
    /// **Key 71 is the CC number here**, which took two captures to see. It
    /// reads as a constant 4 in every capture that only ever switches the row
    /// on, because 4 is the CC the pedal picks by default - so it was written
    /// down as an on switch. `mac-cc-capture.log` sets the row to 42, then 43,
    /// 44, 45 and 46, and sends exactly those under key 71. Taking the row off
    /// sends 0.
    ///
    /// A *parameter* works the other way round: its assignment carries the on
    /// switch at key 71 and its CC number goes through
    /// [`Session::set_assign_cc`].
    pub fn assign_bypass_midi(&mut self, block: i64, cc: Option<i64>) -> Result<()> {
        /// What is being controlled: this block's bypass.
        const BYPASS_TARGET: i64 = 5;
        /// Constant across every captured assignment.
        const SCOPE: i64 = 300;

        non_negative(block, "block")?;
        if cc.is_some_and(|cc| !(0..=127).contains(&cc)) {
            return Err(Error::Protocol("a MIDI CC is 0 to 127".into()));
        }
        self.command(
            ChannelId::DATA,
            rpc::op::ASSIGN_CONTROLLER,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::ASSIGN_TARGET => Value::Int(BYPASS_TARGET),
                rpc::key::ASSIGN_SCOPE => Value::Int(SCOPE),
                rpc::key::ASSIGN_FLAGS => Value::Int(rpc::Source::NONE),
                rpc::key::ASSIGN_KIND => Value::Int(cc.unwrap_or(0)),
            },
        )
    }

    /// Which MIDI CC drives a parameter that is already assigned to one.
    ///
    /// Two messages, not one: [`Session::assign_parameter`] with
    /// [`rpc::Source::MIDI`] says a controller exists, and this says which CC.
    /// HX Edit sends one of these per intermediate value while the field is
    /// spun, so a single call is the same thing arriving in one step.
    ///
    /// The bypass row is the other way round - see
    /// [`Session::assign_bypass_midi`], where the number rides the assignment
    /// itself.
    pub fn set_assign_cc(&mut self, block: i64, param: i64, cc: i64) -> Result<()> {
        block_param(block, param)?;
        if !(0..=127).contains(&cc) {
            return Err(Error::Protocol("a MIDI CC is 0 to 127".into()));
        }
        self.command(
            ChannelId::DATA,
            rpc::op::SET_ASSIGN_CC,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::COMMIT => Value::Bool(true),
                rpc::key::PATH => Value::Int(0),
                rpc::key::PARAM_INDEX => Value::Int(param),
                rpc::key::CC => Value::Int(cc),
            },
        )
    }

    /// Put a parameter under a controller - an expression pedal, a footswitch,
    /// a MIDI CC.
    ///
    /// Captured from HX Edit's Bypass/Controller Assign page by scrolling its
    /// source menu, which is how those custom-drawn dropdowns can be driven at
    /// all: they ignore synthetic clicks. The ordinal under key 74 is the
    /// source, keys 72 and 73 the ends of its travel.
    /// `None` takes the assignment off.
    ///
    /// Key 71 is not the constant it looks like: it is the assignment's on
    /// switch, `4` when one is made and `0` when it is removed, and removing
    /// sends `{74: 0, 71: 0}` through this same opcode rather than one of its
    /// own.
    pub fn assign_parameter(
        &mut self,
        block: i64,
        param: i64,
        source: Option<rpc::Source>,
    ) -> Result<()> {
        block_param(block, param)?;
        let (flags, kind) = match source {
            Some(source) => (source.ordinal(), 4),
            None => (rpc::Source::NONE, 0),
        };
        self.command(
            ChannelId::DATA,
            rpc::op::ASSIGN_CONTROLLER,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::PATH => Value::Int(0),
                rpc::key::PARAM_INDEX => Value::Int(param),
                rpc::key::COMMIT => Value::Bool(true),
                rpc::key::ASSIGN_FLAGS => Value::Int(flags),
                rpc::key::ASSIGN_KIND => Value::Int(kind),
                rpc::key::ASSIGN_EXTRA => Value::Bool(false),
            },
        )
    }

    /// One end of a controller's travel, normalised to 0.0–1.0.
    ///
    /// Min and Max are their own opcodes; keys 72 and 73 are what a read
    /// returns, not what a write takes.
    pub fn set_assign_range(
        &mut self,
        block: i64,
        param: i64,
        value: f32,
        high_end: bool,
    ) -> Result<()> {
        block_param(block, param)?;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Error::Protocol(
                "an assignment endpoint must be between 0 and 1".into(),
            ));
        }
        let op = if high_end {
            rpc::op::ASSIGN_MAX_OP
        } else {
            rpc::op::ASSIGN_MIN_OP
        };
        self.command(
            ChannelId::DATA,
            op,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::PATH => Value::Int(0),
                rpc::key::PARAM_INDEX => Value::Int(param),
                rpc::key::COMMIT => Value::Bool(true),
                rpc::key::VALUE => Value::F32(value),
            },
        )
    }

    /// What controls a parameter now, and over what travel.
    ///
    /// `None` for a parameter nothing controls - which the device reports as
    /// source ordinal 0, the same "None" the assign page offers.
    pub fn read_assignment_raw(&mut self, block: i64, param: i64) -> Result<Value> {
        block_param(block, param)?;
        self.request(
            ChannelId::DATA,
            rpc::op::READ_ASSIGNMENT,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::COMMIT => Value::Bool(true),
                rpc::key::PATH => Value::Int(0),
                rpc::key::PARAM_INDEX => Value::Int(param),
            },
        )
    }

    pub fn read_assignment(&mut self, block: i64, param: i64) -> Result<Option<Assignment>> {
        block_param(block, param)?;
        let reply = self.request(
            ChannelId::DATA,
            rpc::op::READ_ASSIGNMENT,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::COMMIT => Value::Bool(true),
                rpc::key::PATH => Value::Int(0),
                rpc::key::PARAM_INDEX => Value::Int(param),
            },
        )?;
        // The reply does not use the key the request did: opcode 37 takes the
        // source at 74, opcode 36 answers with it at 0. Reading 74 here meant
        // every parameter came back unassigned, which is why an assignment that
        // the pedal had certainly made showed nothing at all in the editor.
        //
        // A parameter nothing controls answers `nil`, not a map with a zero in
        // it, so `get` finding nothing is the same answer as ordinal 0.
        let ordinal = reply
            .get(rpc::key::ASSIGN_SOURCE)
            .and_then(|v| v.as_i64())
            .unwrap_or(rpc::Source::NONE);
        // Ordinal 0 is None, and `from_ordinal` says so by answering nothing.
        let Some(source) = rpc::Source::from_ordinal(ordinal) else {
            return Ok(None);
        };
        Ok(Some(Assignment { source }))
    }

    /// Make a footswitch toggle a block in and out.
    ///
    /// Bypass is a switch, so only a footswitch or a MIDI CC can drive it -
    /// HX Edit lists expression pedals for it but steps over them.
    pub fn assign_bypass_footswitch(&mut self, block: i64, switch: u8) -> Result<()> {
        non_negative(block, "block")?;
        let switch = switch_index(&self.profile, switch)?;
        self.command(
            ChannelId::DATA,
            rpc::op::ASSIGN_FOOTSWITCH,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::SWITCH => Value::Int(switch),
            },
        )
    }

    /// What a footswitch is set to.
    ///
    /// Opcode 33 counts from **one** on the way in and answers with a
    /// **zero-based** index in key 102, which is the opposite of opcodes 56 and
    /// 57. That was an open question in the protocol notes and is now settled by
    /// asking the pedal: `read_switch(1)` comes back `{102: 0}`, and so on up.
    /// The one-based number goes in here so a caller says "footswitch 3" and
    /// means it.
    pub fn read_switch(&mut self, switch: u8) -> Result<Switch> {
        switch_index(&self.profile, switch)?;
        let reply = self.request(
            ChannelId::DATA,
            rpc::op::FOOTSWITCH_CONFIG,
            hx_proto::msgmap! {
                rpc::key::SWITCH => Value::Int(i64::from(switch)),
            },
        )?;
        let carries = match reply.get(rpc::key::SWITCH_ASSIGNED) {
            Some(Value::Array(items)) => items.iter().filter_map(carried).collect(),
            _ => Vec::new(),
        };
        Ok(Switch {
            switch,
            momentary: reply
                .get(rpc::key::MOMENTARY)
                .and_then(Value::as_bool)
                .unwrap_or(false),
            label: reply
                .get(rpc::key::NAME)
                .and_then(Value::as_str)
                .map(str::to_owned),
            colour: reply.get(rpc::key::LED_COLOUR).and_then(|v| v.as_i64()),
            carries,
        })
    }

    /// Every footswitch a device has, in order.
    pub fn switches(&mut self) -> Result<Vec<Switch>> {
        (1..=self.profile.switches)
            .map(|switch| self.read_switch(switch))
            .collect()
    }

    /// Latching, which toggles, or momentary, which holds while your foot is
    /// down.
    pub fn set_switch_momentary(&mut self, switch: u8, momentary: bool) -> Result<()> {
        let switch = switch_index(&self.profile, switch)?;
        self.command(
            ChannelId::DATA,
            rpc::op::SWITCH_TYPE,
            hx_proto::msgmap! {
                rpc::key::SWITCH => Value::Int(switch),
                rpc::key::MOMENTARY => Value::Bool(momentary),
            },
        )
    }

    /// Write a name under a footswitch, or clear it back to whatever it
    /// carries. Clearing is its own opcode rather than an empty string.
    pub fn set_switch_label(&mut self, switch: u8, label: Option<&str>) -> Result<()> {
        let switch = Value::Int(switch_index(&self.profile, switch)?);
        match label {
            Some(label) => self.command(
                ChannelId::DATA,
                rpc::op::SWITCH_LABEL,
                hx_proto::msgmap! {
                    rpc::key::SWITCH => switch,
                    rpc::key::NAME => Value::Str(label.to_owned()),
                },
            ),
            None => self.command(
                ChannelId::DATA,
                rpc::op::SWITCH_LABEL_CLEAR,
                hx_proto::msgmap! { rpc::key::SWITCH => switch },
            ),
        }
    }

    /// Light a footswitch a chosen colour, or `None` for Auto Color, where it
    /// takes the colour of whatever it controls.
    ///
    /// The colour is an index into HX Edit's `footswitchLED` list, not an RGB
    /// value: the capture sets White by sending `66: 1`. Auto Color is index 0
    /// of that list and has an opcode to itself.
    pub fn set_switch_colour(&mut self, switch: u8, colour: Option<i64>) -> Result<()> {
        let switch = Value::Int(switch_index(&self.profile, switch)?);
        if colour.is_some_and(|colour| colour < 1) {
            return Err(Error::Protocol(
                "a chosen footswitch colour must be 1 or greater; use None for Auto Color".into(),
            ));
        }
        match colour {
            Some(colour) => self.command(
                ChannelId::DATA,
                rpc::op::SWITCH_COLOUR,
                hx_proto::msgmap! {
                    rpc::key::SWITCH => switch,
                    rpc::key::LED_COLOUR => Value::Int(colour),
                },
            ),
            None => self.command(
                ChannelId::DATA,
                rpc::op::SWITCH_COLOUR_AUTO,
                hx_proto::msgmap! { rpc::key::SWITCH => switch },
            ),
        }
    }

    /// Take a block's bypass off a footswitch again.
    pub fn unassign_bypass_footswitch(&mut self, block: i64, switch: u8) -> Result<()> {
        non_negative(block, "block")?;
        let switch = switch_index(&self.profile, switch)?;
        self.command(
            ChannelId::DATA,
            rpc::op::UNASSIGN_FOOTSWITCH,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::SWITCH => Value::Int(switch),
            },
        )
    }

    /// Change the tempo of the loaded preset.
    ///
    /// No opcode carries tempo on its own, so this edits the preset document
    /// and writes it back.
    pub fn set_tempo(&mut self, bpm: f32) -> Result<()> {
        if !bpm.is_finite() || !(40.0..=240.0).contains(&bpm) {
            return Err(Error::Protocol(
                "tempo must be between 40 and 240 BPM".into(),
            ));
        }
        let mut preset = self.read_preset()?;
        if !preset.set_tempo(bpm) {
            return Err(Error::Protocol("this preset has no tempo field".into()));
        }
        self.write_preset(&preset)
    }

    /// Rename a snapshot, by zero-based index.
    pub fn rename_snapshot(&mut self, index: usize, name: &str) -> Result<()> {
        let mut preset = self.read_preset()?;
        if !preset.set_snapshot_name(index, name) {
            return Err(Error::Protocol(format!("no snapshot {}", index + 1)));
        }
        self.write_preset(&preset)
    }

    /// Setlist names.
    pub fn setlists(&mut self) -> Result<Vec<String>> {
        let result = self.request(ChannelId::CONTROL, rpc::op::LIST_SETLISTS, Value::Nil)?;
        let Value::Array(entries) = result else {
            return Err(Error::Protocol("setlist list was not an array".into()));
        };
        // A setlist entry is a single-pair map from index to name - `{0:
        // 'PRESETS'}` - rather than the keyed record the preset list uses.
        entries
            .into_iter()
            .enumerate()
            .map(|(position, entry)| match entry {
                Value::Map(fields) => fields
                    .first()
                    .and_then(|(_, v)| v.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        Error::Protocol(format!("setlist entry {position} has no readable name"))
                    }),
                Value::Str(name) => Ok(name),
                _ => Err(Error::Protocol(format!(
                    "setlist entry {position} has no readable name"
                ))),
            })
            .collect()
    }

    /// Switch a block in or out of the signal path.
    pub fn set_enabled(&mut self, block: i64, enabled: bool) -> Result<()> {
        non_negative(block, "block")?;
        self.command(
            ChannelId::DATA,
            rpc::op::SET_ENABLED,
            hx_proto::msgmap! {
                rpc::key::BLOCK => Value::Int(block),
                rpc::key::ENABLED => Value::Bool(enabled),
            },
        )
    }

    /// Setlist, index and name of the preset currently loaded.
    ///
    /// The name is not part of the preset document itself, so it comes from
    /// this separate metadata call rather than from `read_preset`.
    pub fn preset_info(&mut self) -> Result<(i64, i64, String)> {
        let v = self.request(ChannelId::DATA, rpc::op::PRESET_INFO, Value::Nil)?;
        let setlist = v
            .get(rpc::key::SETLIST)
            .and_then(Value::as_i64)
            .filter(|value| *value >= 0)
            .ok_or_else(|| Error::Protocol("preset info has no valid setlist".into()))?;
        let index = v
            .get(rpc::key::PRESET_INDEX)
            .and_then(Value::as_i64)
            .filter(|value| *value >= 0)
            .ok_or_else(|| Error::Protocol("preset info has no valid slot".into()))?;
        let name = v
            .get(rpc::key::NAME)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Protocol("preset info has no name".into()))?;
        Ok((setlist, index, name.to_owned()))
    }

    /// Fetch an object by id.
    pub fn fetch(&mut self, id: i64) -> Result<Value> {
        self.request(
            ChannelId::DATA,
            rpc::op::FETCH_OBJECT,
            hx_proto::msgmap! { rpc::key::OBJECT_ID => Value::Int(id) },
        )
    }

    /// Any notifications the device has pushed since the last call.
    pub fn poll_notifications(&mut self) -> Vec<(i64, Value)> {
        let _ = self.read_once(Duration::from_millis(20));
        let mut out = Vec::new();
        for ch in self.channels.values_mut() {
            for sm in ch.reader.take_messages() {
                if let Message::Notification { event, args } = Message::from_value(sm.body) {
                    out.push((event, args));
                }
            }
        }
        out
    }
}

fn decode_ir_samples(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::Protocol(
            "the device's impulse response ends partway through an f32 sample".into(),
        ));
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    if samples.is_empty()
        || samples.len() > 2048
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err(Error::Protocol(
            "the device returned an invalid impulse response".into(),
        ));
    }
    Ok(samples)
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn non_finite_wire_values_are_rejected() {
        assert!(finite_value(&Value::F32(f32::NAN), "value").is_err());
        assert!(finite_value(&Value::F32(f32::INFINITY), "value").is_err());
        assert!(finite_value(&Value::F64(f64::NEG_INFINITY), "value").is_err());
        assert!(finite_value(&Value::F32(0.5), "value").is_ok());
    }

    #[test]
    fn footswitches_are_checked_against_the_device_profile() {
        assert_eq!(switch_index(&hx_proto::HX_STOMP, 1).unwrap(), 0);
        assert_eq!(switch_index(&hx_proto::HX_STOMP, 5).unwrap(), 4);
        assert!(switch_index(&hx_proto::HX_STOMP, 0).is_err());
        assert!(switch_index(&hx_proto::HX_STOMP, 6).is_err());
    }

    #[test]
    fn protocol_addresses_cannot_be_negative() {
        assert!(block_param(0, 0).is_ok());
        assert!(block_param(-1, 0).is_err());
        assert!(block_param(0, -1).is_err());
    }

    #[test]
    fn malformed_device_ir_samples_are_not_silently_shortened() {
        assert_eq!(decode_ir_samples(&0.5f32.to_le_bytes()).unwrap(), vec![0.5]);
        assert!(decode_ir_samples(&[0, 1, 2]).is_err());
        assert!(decode_ir_samples(&f32::NAN.to_le_bytes()).is_err());
        assert!(decode_ir_samples(&[]).is_err());
    }
}
