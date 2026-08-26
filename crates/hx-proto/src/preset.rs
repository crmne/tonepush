//! The preset document: what is actually in the signal chain.
//!
//! A preset arrives as a blob whose contents are themselves MessagePack - the
//! magic string `l6-helix`, a table of section offsets, and the tone. The tone
//! is a fixed array of slots, most of them empty on a small device.
//!
//! Slots are keyed by integers throughout, so the constants here are doing the
//! work a schema would do elsewhere. They were read off captured presets and
//! cross-checked against the model catalog: a slot holding model 101 carries
//! exactly three values, and model 101 is Scream 808, which has exactly three
//! parameters.

use crate::msgpack::{Decoder, Encoder, Value};

/// A parsed preset.
pub struct Preset {
    /// Section offset table: a directory of the tone's top-level sections,
    /// decoded in [`computed_sections`](Self::computed_sections). Verified
    /// against the tone on parse and retained for byte-exact round trips.
    pub sections: Vec<u8>,
    /// Header width the section table arrived with, so it goes back unchanged.
    sections_width: u8,
    pub slots: Vec<Slot>,
    /// The whole tone map, for fields this type does not model yet.
    pub tone: Value,
}

/// One thing a controller drives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assignment {
    /// The block, as the device numbers them.
    pub block: i64,
    /// What drives it. The document indexes controllers by the same ordinals
    /// the wire uses, so this is the wire's own type rather than a second
    /// vocabulary for the same nine things.
    pub source: crate::rpc::Source,
    pub target: Target,
    /// The ends of the controller's travel, **in the parameter's own units**.
    /// A pitch block's read 7 and 12 because those are semitones; a wah's read
    /// 0 and 1 because that is a wah's range. They are not percentages.
    pub min: f32,
    pub max: f32,
    /// Which MIDI CC drives it, whenever MIDI is what drives it - a bypass and
    /// a parameter alike. `None` under any other source, which has no number
    /// to give.
    pub cc: Option<i64>,
}

/// What an assignment moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// The block's on/off.
    Bypass,
    /// One of its parameters, by index.
    Param(i64),
}

/// Where each part of the signal path lives in the slot array.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// Each independent signal path, in order. Most devices have one; Helix and
    /// Helix LT have two, so a preset can hold four lanes once both split.
    pub paths: Vec<Path>,
}

impl Lane {
    /// Positions in this lane with nothing in them, in order - where a new
    /// block can go.
    pub fn free(&self, preset: &Preset) -> Vec<usize> {
        self.span
            .clone()
            .filter(|p| preset.slots.get(*p).is_some_and(|s| s.model.is_none()))
            .collect()
    }
}

impl Path {
    /// Where one branch's row starts in the slot array.
    ///
    /// The upper row is the whole main line, from just after the input to just
    /// before the output: a split and a join sit *inside* that run rather than
    /// dividing it, which is why the blocks after a join keep counting on from
    /// the ones before it. The lower row is the lower lane's own span, which
    /// begins right after the split.
    fn row_base(&self, branch: usize) -> Option<usize> {
        if branch > 0 {
            return self
                .lanes
                .iter()
                .find(|lane| lane.branch == branch)
                .map(|lane| lane.span.start)
                // A branch with nothing on it is drawn as no lane at all, and
                // that is exactly the chain a file gets loaded onto: cleared.
                // The branch still begins in the slot after the split.
                .or_else(|| self.split.map(|split| split + 1));
        }
        self.input
            .map(|input| input + 1)
            .or_else(|| self.head.first().copied())
            .or_else(|| self.lanes.first().map(|lane| lane.span.start))
    }
}

impl Layout {
    /// Every lane across every path, which is what a renderer draws row by row.
    pub fn lanes(&self) -> impl Iterator<Item = &Lane> {
        self.paths.iter().flat_map(|p| p.lanes.iter())
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The slot a `.hlx` block sits in, from the branch and row index HX Edit
    /// records as `@path` and `@position`.
    ///
    /// HX Edit numbers cells along the drawn row rather than device slots, and
    /// the two are only the same on a chain with no split. Checked against a
    /// factory preset exported both ways: HX Edit puts the amp at 0 and the
    /// plate reverb at 5 where the device holds them in slots 1 and 6, and the
    /// four blocks on the lower branch at 1 to 4 where the device holds them in
    /// 12 to 15.
    ///
    /// A junction's own number means "before this cell", which is the same
    /// arithmetic: the split of that preset reads 1 and attaches before slot 2.
    pub fn slot_of(&self, path: usize, branch: usize, position: usize) -> Option<usize> {
        self.paths
            .get(path)?
            .row_base(branch)?
            .checked_add(position)
    }

    /// The other direction: which branch a slot is on, and its place in that
    /// row, for writing a `.hlx` HX Edit can read.
    pub fn position_of(&self, path: usize, slot: usize) -> Option<(usize, usize)> {
        let path = self.paths.get(path)?;
        // The lower branch first: its span is a stretch of the same slot array,
        // and a slot inside it is not on the main line.
        for lane in path.lanes.iter().filter(|lane| lane.branch > 0) {
            if lane.span.contains(&slot) {
                return Some((lane.branch, slot - lane.span.start));
            }
        }
        let base = path.row_base(0)?;
        (slot >= base).then(|| (0, slot - base))
    }
}

/// One signal path: an input, whatever the signal passes through, an output.
///
/// A split does not divide the whole path - it divides a *stretch* of it. The
/// blocks before the split and after the join are common to both branches, and
/// the split records where it attaches, so a preset reads:
///
/// ```text
/// input → head… → ⑂ → lanes… → ⑃ → tail… → output
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Path {
    pub input: Option<usize>,
    pub output: Option<usize>,
    /// The slot holding the split, when the path divides. It is not a block in
    /// the line - it is where the wiring parts - so it is kept out of the lanes.
    pub split: Option<usize>,
    pub join: Option<usize>,
    /// Blocks before the split, carrying the undivided signal.
    pub head: Vec<usize>,
    /// The branches, when the path splits. Empty when it runs straight through,
    /// in which case every block is in `head`.
    pub lanes: Vec<Lane>,
    /// Blocks after the join, carrying the recombined signal.
    pub tail: Vec<usize>,
}

/// A single row of blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lane {
    /// Which branch of the split this is: 0 for the upper, 1 for the lower.
    pub branch: usize,
    /// Positions of the occupied blocks, in signal order. Empty slots are not
    /// listed; use [`Lane::free`] to find somewhere to put a new block.
    pub blocks: Vec<usize>,
    /// Every slot position this lane covers, occupied or not.
    pub span: std::ops::Range<usize>,
}

/// One position in the signal chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub kind: Kind,
    /// Model number, resolvable through `hx-catalog`'s symbol table.
    pub model: Option<u32>,
    /// A second model sharing the slot. This is how Amp+Cab works: the amp is
    /// the primary and the cab rides along with its own parameters. `None` on
    /// the great majority of blocks.
    pub paired: Option<u32>,
    pub enabled: bool,
    /// Parameter values in the order the device indexes them, which is the
    /// order `Helix.sym` lists for this model.
    pub values: Vec<f32>,
    /// The paired model's values, in its own parameter order.
    pub paired_values: Vec<f32>,
    /// Key 9: the slot's engine class.
    ///
    /// It was first misread as a branch index - the branch is implied by slot
    /// position, which is what [`Preset::layout`] uses - and then as a function
    /// of the model alone, which it is not: an amp carries 17 on its own and 18
    /// with a cab riding along, so 30 of 217 models seen carry two values.
    /// Keyed by the model's *category* and whether it is paired, nothing is
    /// ambiguous across 240 captured presets. `Catalog::type_tag` derives it,
    /// and `hx-catalog/tests/type_tags.rs` pins the derivation against every
    /// fixture.
    ///
    /// Editing a preset still carries it through byte-exact - the device
    /// maintains it on model changes, so there is nothing to gain by
    /// recomputing. Deriving it is for *building* a document from a symbolic
    /// tone, where there is nothing to carry through.
    pub type_tag: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Input,
    Output,
    Split,
    Join,
    /// An effect, amp or cab.
    Block,
    /// A looper, which the device keeps in a slot of its own rather than among
    /// the effects - it carries its model inline, the way a junction does.
    Looper,
    Empty,
    /// A slot kind we have not identified; carried through rather than dropped.
    Unknown(i64),
}

impl Kind {
    fn from_wire(k: i64) -> Kind {
        match k {
            0 => Kind::Input,
            1 => Kind::Output,
            2 => Kind::Split,
            3 => Kind::Join,
            6 => Kind::Block,
            7 => Kind::Looper,
            8 => Kind::Empty,
            other => Kind::Unknown(other),
        }
    }
}

/// Field keys inside the tone.
mod key {
    /// Metadata: DSP name, firmware, build string.
    pub const META: i64 = 7;
    /// The signal path.
    pub const PATH: i64 = 0;
    /// Slot array within the path.
    pub const SLOTS: i64 = 22;

    /// Slot kind.
    pub const KIND: i64 = 19;
    /// Slot body.
    pub const BODY: i64 = 20;

    /// Model reference on an effect slot.
    pub const MODEL_REF: i64 = 24;
    /// Model number within a model reference.
    pub const MODEL: i64 = 25;
    /// Second model in the same slot, or -1 when the slot holds only one.
    pub const PAIRED_MODEL: i64 = 26;
    /// The paired model's parameter values.
    pub const PAIRED_VALUES: i64 = 12;
    /// Where an input slot takes its signal from. Sits beside the value array
    /// rather than in it, which is why it never showed up as a parameter.
    pub const INPUT_FROM: i64 = 5;
    /// Where an output slot sends its signal.
    pub const OUTPUT_TO: i64 = 6;
    /// Model number on a split or join, which carries it directly.
    pub const INLINE_MODEL: i64 = 8;
    /// Whether the block is switched on.
    pub const ENABLED: i64 = 10;
    /// The slot's engine class. See `Slot::type_tag`.
    pub const TYPE_TAG: i64 = 9;
    /// Parameter values on an effect slot.
    pub const VALUES: i64 = 11;
    /// Parameter values on input, output, split and join slots.
    pub const IO_VALUES: i64 = 7;

    /// Split and join keep their model and values one level down.
    pub const SPLIT_BODY: i64 = 15;
    pub const JOIN_BODY: i64 = 17;
    /// Inside a split or join body: the slot index it attaches before. A split
    /// carrying 2 divides the signal just before slot 2; a join carrying 6
    /// recombines it just before slot 6.
    pub const ATTACH: i64 = 13;

    /// Inside a value array: the count, then the values.
    pub const ARRAY_VALUES: i64 = 4;

    pub const FIRMWARE: i64 = 35;
    pub const BUILD: i64 = 37;

    /// Preset-wide settings.
    pub const SETTINGS: i64 = 5;
    /// Tempo in BPM, within the settings.
    pub const TEMPO: i64 = 16;

    /// The model number a junction holds, which is what kind of split it is:
    /// a Y, an A/B or a crossover.
    pub const JUNCTION_MODEL: i64 = 8;

    /// On a split or a join: whether a snapshot can switch it. HX Edit lists
    /// the junction among a snapshot's blocks only when this is set.
    pub const JUNCTION_SNAPSHOTS: i64 = 18;

    /// Everything the controllers drive, as an array indexed by the source's
    /// ordinal: entry 1 is Expression Pedal 1's list, entry 3 Footswitch 1's.
    /// Confirmed by assigning one parameter and diffing the document.
    pub const ASSIGNMENTS: i64 = 4;
    /// Inside one of those entries: the assignment itself, at key 1. Key 0 is
    /// its place in the controller's table, not the block.
    pub const ASSIGNED_WHAT: i64 = 1;
    /// And inside the assignment, the block it acts on.
    pub const ASSIGNED_ON: i64 = 5;
    /// Inside the assignment, and it means two things depending on the shape
    /// around it - exactly as key 71 does on the wire. **[confirmed]**
    ///
    /// On a *parameter* assignment it is 4, saying only that a controller is
    /// there. On a *bypass* it is the MIDI CC number, and reading it as a kind
    /// is what made a MIDI bypass vanish: the parser took anything but 0 for a
    /// parameter, then found no parameter target and dropped the assignment, so
    /// a bypass on CC 77 was invisible while one on CC 0 was not.
    ///
    /// Which shape it is comes from [`ASSIGNED_BYPASS`] against
    /// [`ASSIGNED_TARGET`], not from here.
    pub const ASSIGNED_KIND: i64 = 1;
    /// A bypass assignment points at the bypass with this, the document's copy
    /// of the wire's key 95. Its presence is what tells a bypass from a
    /// parameter.
    pub const ASSIGNED_BYPASS: i64 = 9;
    /// The ends of the controller's travel, in the parameter's own units. The
    /// document is the only place these are right; opcode 36 reports the
    /// defaults forever.
    pub const ASSIGNED_MIN: i64 = 2;
    pub const ASSIGNED_MAX: i64 = 3;
    /// What a parameter assignment points at; the index lives inside it.
    pub const ASSIGNED_TARGET: i64 = 6;
    /// The parameter's index, inside that.
    ///
    /// **Key 29, not 28.** The request that *makes* an assignment carries the
    /// index at 28 and a commit flag at 29; the document swaps them, holding
    /// the path at 28 and the index at 29. Reading 28 made every assignment in
    /// every preset report parameter 0, so a footswitch put on a Bright switch
    /// was drawn as owning the Drive knob. Confirmed by dumping key 4 of a
    /// preset whose only assignment is on parameter 1: `6: {28: 0, 29: 1}`.
    pub const ASSIGNED_PARAM: i64 = 29;

    /// Snapshots and footswitch assignments.
    pub const SNAPSHOT_SECTION: i64 = 10;
    pub const SNAPSHOTS: i64 = 10;
    pub const SNAPSHOT_NAME: i64 = 4;
    /// Whether a snapshot holds anything.
    pub const SNAPSHOT_VALID: i64 = 0;
    /// Per-slot state within a snapshot: `[?, enabled]` for each slot, so a
    /// snapshot remembers which blocks were on when it was taken.
    pub const SNAPSHOT_SLOTS: i64 = 3;
    /// The snapshot's own tempo.
    pub const SNAPSHOT_TEMPO: i64 = 5;
    /// Whether the name was typed rather than left as "SNAPSHOT 1".
    pub const SNAPSHOT_NAMED: i64 = 14;
}

/// One snapshot: a remembered set of bypass states, with its own name and tempo.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub tempo: Option<f32>,
    pub valid: bool,
    /// Whether the name was given rather than left at the default.
    pub named: bool,
    /// Whether each slot was on, indexed the same as [`Preset::slots`].
    pub enabled: Vec<Option<bool>>,
}

impl Preset {
    pub const MAGIC: &'static str = "l6-helix";

    /// Parse the blob carried by a read-preset response.
    pub fn parse(blob: &[u8]) -> Option<Preset> {
        let mut values = Decoder::decode_all(blob).ok()?.into_iter();
        if values.next()?.as_str()? != Self::MAGIC {
            return None;
        }
        let (sections, sections_width) = match values.next()? {
            Value::Bin(bytes, width) => (bytes, width),
            _ => return None,
        };
        let tone = values.next()?;
        if !matches!(tone, Value::Map(_)) || values.next().is_some() {
            return None;
        }

        let slot_values = tone.get(key::PATH)?.get(key::SLOTS)?;
        let Value::Array(raw_slots) = slot_values else {
            return None;
        };
        if raw_slots.is_empty()
            || raw_slots.iter().any(|slot| {
                !slot_is_well_formed(slot)
                    || !endpoint_route_is_well_formed(slot)
                    || !junction_is_well_formed(slot)
            })
        {
            return None;
        }
        if !assignments_are_well_formed(&tone) {
            return None;
        }
        if !snapshots_are_well_formed(&tone, raw_slots.len()) {
            return None;
        }
        if !settings_are_well_formed(&tone) {
            return None;
        }
        let slots = collect_slots(slot_values);

        let preset = Preset {
            sections,
            sections_width,
            slots,
            tone,
        };
        (preset.computed_sections().as_deref() == Some(preset.sections.as_slice()))
            .then_some(preset)
    }

    /// Firmware version, from the BCD-packed field: `0x03800000` is 3.80.
    pub fn firmware(&self) -> Option<String> {
        let raw = u32::try_from(self.tone.get(key::META)?.get(key::FIRMWARE)?.as_i64()?).ok()?;
        let major = ((raw >> 24) & 0xff) as u8;
        let minor = ((raw >> 16) & 0xff) as u8;
        let valid_bcd = |byte: u8| byte >> 4 <= 9 && byte & 0x0f <= 9;
        (valid_bcd(major) && valid_bcd(minor)).then(|| format!("{major:x}.{minor:02x}"))
    }

    pub fn build(&self) -> Option<&str> {
        self.tone.get(key::META)?.get(key::BUILD)?.as_str()
    }

    /// Serialise back to the blob the device exchanges.
    ///
    /// The inverse of [`parse`](Self::parse), and what makes editing possible
    /// at all: several operations have no dedicated opcode, and the way to
    /// perform them is to change the document and write the whole thing back.
    pub fn encode(&self) -> Vec<u8> {
        // The section table is a directory of byte offsets into this very
        // document, so any edit that changes a length invalidates it - and the
        // device answers a stale table by reading the preset as empty. It is
        // recomputed rather than carried through; on an unmodified document
        // the result is identical to what the device sent, which the
        // round-trip test asserts byte for byte.
        let sections = self
            .computed_sections()
            .unwrap_or_else(|| self.sections.clone());

        let mut out = Encoder::encode(&Value::Str(Self::MAGIC.to_owned()));
        out.extend(Encoder::encode(&Value::Bin(sections, self.sections_width)));
        out.extend(Encoder::encode(&self.tone));
        out
    }

    /// Exchange two slots, moving a block along the chain.
    ///
    /// Returns false if either position does not exist - or is not a position
    /// blocks live in. The input, output, split and join are fixtures of the
    /// topology: swapping a pedal into one produces a document the device
    /// cannot make sense of.
    pub fn swap_slots(&mut self, a: usize, b: usize) -> bool {
        if !self.holds_blocks(a) || !self.holds_blocks(b) {
            return false;
        }
        let Some(Value::Array(slots)) = self.tone.at_mut(&[key::PATH, key::SLOTS]) else {
            return false;
        };
        if a >= slots.len() || b >= slots.len() {
            return false;
        }
        slots.swap(a, b);
        self.slots.swap(a, b);
        true
    }

    /// Whether a position is one blocks live in, as opposed to a fixture of
    /// the topology.
    fn holds_blocks(&self, position: usize) -> bool {
        matches!(
            self.slots.get(position).map(|s| s.kind),
            Some(Kind::Block) | Some(Kind::Empty)
        )
    }

    /// Move a block to another position the way a hand would: within a lane
    /// the blocks in between shift along to make room, and across lanes the
    /// two slots trade places.
    ///
    /// The distinction matters because a lane is contiguous in the slot array
    /// while the other branch of a split is not: shifting across the boundary
    /// would rotate the output and the split themselves into the line.
    pub fn move_slot(&mut self, from: usize, to: usize) -> bool {
        if from == to || !self.holds_blocks(from) || !self.holds_blocks(to) {
            return false;
        }
        let same_lane = match (self.lane_bounds(from), self.lane_bounds(to)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        if !same_lane {
            return self.swap_slots(from, to);
        }
        let Some(Value::Array(items)) = self.tone.at_mut(&[key::PATH, key::SLOTS]) else {
            return false;
        };
        if from >= items.len() || to >= items.len() {
            return false;
        }
        if from < to {
            items[from..=to].rotate_left(1);
            self.slots[from..=to].rotate_left(1);
        } else {
            items[to..=from].rotate_right(1);
            self.slots[to..=from].rotate_right(1);
        }
        true
    }

    /// Move a block into the gap just before `before`, the way dropping it
    /// there reads: the block leaves its slot and the others close ranks.
    ///
    /// Within a lane this is [`move_slot`](Self::move_slot) with the index
    /// adjusted - landing in the gap before `before` means landing *at*
    /// `before - 1` when the block comes from the left, because everything
    /// between shifts back one. Into another lane, the gap's own slot is
    /// freed first and the block trades into it.
    pub fn insert_slot(&mut self, from: usize, before: usize) -> bool {
        if !self.holds_blocks(from) {
            return false;
        }
        // The gap before an endpoint or junction means "the end of the lane
        // that finishes here" - the same convention as inserting a new block.
        let anchor = if self.holds_blocks(before) {
            before
        } else {
            before.saturating_sub(1)
        };
        let same_lane = match (self.lane_bounds(from), self.lane_bounds(anchor)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        if same_lane {
            let to = if from < before { before - 1 } else { before };
            // Dropping a block into the gap beside itself is already done.
            return from == to || self.move_slot(from, to);
        }

        // Into another lane: land on the gap's own slot, freeing it if
        // something lives there; at a lane's end, on its first free slot.
        let Some(bounds) = self.lane_bounds(anchor) else {
            return false;
        };
        let target = if self.holds_blocks(before) {
            before
        } else {
            match bounds
                .clone()
                .find(|p| self.slots.get(*p).is_some_and(|s| s.model.is_none()))
            {
                Some(free) => free,
                None => return false,
            }
        };
        if self.slots.get(target).is_some_and(|s| s.model.is_some())
            && !self.make_room(target, bounds)
        {
            return false;
        }
        self.swap_slots(from, target)
    }

    /// Put every empty branch's bookkeeping back the way the device keeps it:
    /// attach points at zero.
    ///
    /// The device stores whatever attach points a document carries - but a
    /// document that says "the fork is at slot 1" over a branch with nothing
    /// on it is a contradiction it settles, eventually, by wiping the whole
    /// edit buffer. Its own operations always zero the attach points when a
    /// branch empties; an edit of ours that empties one has to do the same.
    /// Found the hard way, against the hardware, by a block move that
    /// destroyed the working preset.
    pub fn settle_branches(&mut self) {
        let layout = self.layout();
        for path in &layout.paths {
            let (Some(split), Some(join)) = (path.split, path.join) else {
                continue;
            };
            let vacant = (split + 1..join)
                .all(|s| self.slots.get(s).is_none_or(|slot| slot.model.is_none()));
            if vacant {
                self.set_attach(split, 0);
                self.set_attach(join, 0);
            }
        }
    }

    /// Re-attach a split or join: the fork or merge moves to sit just before
    /// `before` in the main line.
    ///
    /// Written at the width the field arrived at, for the same reason as
    /// [`set_routing`](Self::set_routing): the preset carries a table of byte
    /// offsets into itself, and a narrower integer shifts everything after it.
    pub fn set_attach(&mut self, position: usize, before: usize) -> bool {
        let body_key = match self.slots.get(position).map(|s| s.kind) {
            Some(Kind::Split) => key::SPLIT_BODY,
            Some(Kind::Join) => key::JOIN_BODY,
            _ => return false,
        };
        let Ok(before) = i64::try_from(before) else {
            return false;
        };
        let Some(field) = self
            .slot_body_mut(position)
            .and_then(|b| b.get_mut(body_key))
            .and_then(|b| b.get_mut(key::ATTACH))
        else {
            return false;
        };
        *field = match field {
            Value::Wide(_, w) => Value::Wide(before as u64, *w),
            Value::WideInt(_, w) => Value::WideInt(before, *w),
            _ => Value::Int(before),
        };
        true
    }

    /// Set the tempo, in BPM.
    ///
    /// There is no dedicated opcode for this, so the caller writes the whole
    /// document back afterwards. That is only safe because re-encoding is
    /// byte-exact - see `tests/roundtrip.rs`.
    pub fn set_tempo(&mut self, bpm: f32) -> bool {
        if !bpm.is_finite() || !(40.0..=240.0).contains(&bpm) {
            return false;
        }
        match self.tone.at_mut(&[key::SETTINGS, key::TEMPO]) {
            Some(slot) => {
                *slot = Value::F32(bpm);
                true
            }
            None => false,
        }
    }

    /// Rename a snapshot.
    pub fn set_snapshot_name(&mut self, index: usize, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        let Some(Value::Array(entries)) =
            self.tone.at_mut(&[key::SNAPSHOT_SECTION, key::SNAPSHOTS])
        else {
            return false;
        };
        match entries
            .get_mut(index)
            .and_then(|e| e.get_mut(key::SNAPSHOT_NAME))
        {
            Some(slot) => {
                *slot = Value::Str(name.to_owned());
                true
            }
            None => false,
        }
    }

    /// Tempo in BPM.
    pub fn tempo(&self) -> Option<f32> {
        self.tone.get(key::SETTINGS)?.get(key::TEMPO)?.as_f32()
    }

    /// Everything a controller drives in this preset.
    ///
    /// Read out of the document rather than asked for one parameter at a time,
    /// which is the difference between one read and eighty. Opcode 36 answers
    /// for a single parameter and answers *wrongly* about its travel: it
    /// reports the defaults however the ends have been moved, where the
    /// document is always right. The document also covers every block at once,
    /// which is what the chain needs to say which blocks a pedal reaches.
    ///
    /// The shape: top-level key 4 is an array indexed by source ordinal, so
    /// entry 1 is everything Expression Pedal 1 drives. Each item pairs a block
    /// with the assignment itself.
    pub fn assignments(&self) -> Vec<Assignment> {
        let Some(Value::Array(by_source)) = self.tone.get(key::ASSIGNMENTS) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for (ordinal, entries) in by_source.iter().enumerate() {
            let Value::Array(entries) = entries else {
                continue;
            };
            // Slot 0 is the "nothing" ordinal and holds nothing to draw.
            let Some(source) = crate::rpc::Source::from_ordinal(ordinal as i64) else {
                continue;
            };
            for entry in entries {
                let Some(what) = entry.get(key::ASSIGNED_WHAT) else {
                    continue;
                };
                // The block is *inside* the assignment, at key 5. The entry's
                // own key 0 looks like a block number and is not: it is this
                // assignment's place in the controller's table, which happens
                // to match the block whenever a controller drives one thing.
                // Reading that one instead put the wah's Position on the input.
                let Some(block) = what.get(key::ASSIGNED_ON).and_then(Value::as_i64) else {
                    continue;
                };
                // Which shape this is comes from what it points at, not from
                // key 1. A bypass carries the bypass target at key 9 and a
                // parameter carries an index inside key 6; key 1 is 4 on a
                // parameter and the *CC number* on a bypass, so reading it as
                // a kind dropped every MIDI bypass whose CC was not 0.
                let target = if what.get(key::ASSIGNED_BYPASS).is_some() {
                    Target::Bypass
                } else {
                    match what
                        .get(key::ASSIGNED_TARGET)
                        .and_then(|t| t.get(key::ASSIGNED_PARAM))
                        .and_then(Value::as_i64)
                    {
                        Some(param) => Target::Param(param),
                        // Pointing at nothing we can name. Better left out
                        // than shown as parameter zero.
                        None => continue,
                    }
                };
                // Under MIDI, key 1 is the CC number - for a parameter as much
                // as for a bypass. It reads as the constant 4 on everything
                // else because 4 is the CC the pedal picks for itself, and
                // that made this look like a bypass-only field for a while.
                // Setting one parameter's CC to 42, then 43, then 77 put
                // exactly those here, in the document and in opcode 36's reply.
                let cc = (source == crate::rpc::Source::MidiCc)
                    .then(|| what.get(key::ASSIGNED_KIND).and_then(Value::as_i64))
                    .flatten();
                found.push(Assignment {
                    block,
                    source,
                    target,
                    cc,
                    min: what
                        .get(key::ASSIGNED_MIN)
                        .and_then(Value::as_f32)
                        .unwrap_or(0.0),
                    max: what
                        .get(key::ASSIGNED_MAX)
                        .and_then(Value::as_f32)
                        .unwrap_or(1.0),
                });
            }
        }
        found
    }

    /// Snapshot names, in order.
    pub fn snapshots(&self) -> Vec<String> {
        let Some(Value::Array(entries)) = self
            .tone
            .get(key::SNAPSHOT_SECTION)
            .and_then(|s| s.get(key::SNAPSHOTS))
        else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|e| {
                e.get(key::SNAPSHOT_NAME)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    /// Every snapshot in full, not just its name.
    ///
    /// A snapshot is the preset's other half: the same blocks with a different
    /// set of them switched on, under its own name and tempo. Anything that
    /// claims to save a preset faithfully has to carry these, and reading only
    /// the names - which is all [`snapshots`](Self::snapshots) does - loses
    /// exactly what makes them worth having.
    pub fn snapshot_details(&self) -> Vec<Snapshot> {
        let Some(Value::Array(entries)) = self
            .tone
            .get(key::SNAPSHOT_SECTION)
            .and_then(|s| s.get(key::SNAPSHOTS))
        else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|e| Snapshot {
                name: e
                    .get(key::SNAPSHOT_NAME)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                tempo: e.get(key::SNAPSHOT_TEMPO).and_then(|v| match v {
                    Value::F32(f) => Some(*f),
                    Value::F64(f) => Some(*f as f32),
                    Value::Int(i) => Some(*i as f32),
                    _ => None,
                }),
                valid: matches!(e.get(key::SNAPSHOT_VALID), Some(Value::Bool(true))),
                named: matches!(e.get(key::SNAPSHOT_NAMED), Some(Value::Bool(true))),
                enabled: match e.get(key::SNAPSHOT_SLOTS) {
                    Some(Value::Array(slots)) => slots
                        .iter()
                        .map(|s| match s {
                            // Each slot reads `[?, enabled]`; the first field is
                            // not understood and is not needed to know what was on.
                            Value::Array(pair) => match pair.get(1) {
                                Some(Value::Bool(b)) => Some(*b),
                                _ => None,
                            },
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            })
            .collect()
    }

    /// How the slots are wired together.
    ///
    /// The slot array is a fixed topology, not a running order: the input, the
    /// upper path's blocks, the output, the split, the lower path's blocks and
    /// the join each occupy known positions. Reading it as a flat list puts the
    /// split and join at the end, which is how they came to be drawn in the
    /// wrong place.
    ///
    /// Positions are derived from the slot kinds rather than hard-coded, so a
    /// device with a different number of block slots still works.
    /// Where a split or join attaches: the slot index it sits just before.
    fn attach(&self, position: usize, body_key: i64) -> Option<usize> {
        self.slot_body(position)?
            .get(body_key)?
            .get(key::ATTACH)
            .and_then(Value::as_i64)
            .and_then(|n| usize::try_from(n).ok())
    }

    /// Where the split or join at `position` attaches: the slot it sits just
    /// before on the main line. `None` if that slot is neither.
    pub fn attach_of(&self, position: usize) -> Option<usize> {
        let body_key = match self.slots.get(position).map(|s| s.kind) {
            Some(Kind::Split) => key::SPLIT_BODY,
            Some(Kind::Join) => key::JOIN_BODY,
            _ => return None,
        };
        self.attach(position, body_key)
    }

    /// What kind of junction sits at `position`, as a model number - a Y split
    /// and a crossover are different models, and a document that calls them all
    /// Y describes a chain that splits the wrong way.
    pub fn junction_model(&self, position: usize) -> Option<u32> {
        let body_key = match self.slots.get(position).map(|s| s.kind) {
            Some(Kind::Split) => key::SPLIT_BODY,
            Some(Kind::Join) => key::JOIN_BODY,
            _ => return None,
        };
        self.slot_body(position)?
            .get(body_key)?
            .get(key::JUNCTION_MODEL)
            .and_then(Value::as_i64)
            .and_then(|n| u32::try_from(n).ok())
    }

    /// Whether a snapshot can switch the split or join at `position`.
    ///
    /// A junction is not always under snapshot control, and HX Edit lists it
    /// among a snapshot's blocks only when it is - so anything writing that
    /// document has to ask.
    pub fn junction_switchable(&self, position: usize) -> bool {
        matches!(
            self.slot_body(position)
                .and_then(|b| b.get(key::JUNCTION_SNAPSHOTS)),
            Some(Value::Bool(true))
        )
    }

    pub fn layout(&self) -> Layout {
        let mut paths: Vec<Path> = Vec::new();
        // Blocks are gathered per path first and assigned to head, lanes and
        // tail afterwards, because where they belong depends on the split's
        // attachment point - which is stored on the split, not on them.
        let mut main: Vec<Vec<usize>> = Vec::new();
        let mut lower: Vec<Vec<usize>> = Vec::new();
        let mut past_split = false;

        for (position, slot) in self.slots.iter().enumerate() {
            match slot.kind {
                Kind::Input => {
                    paths.push(Path {
                        input: Some(position),
                        ..Path::default()
                    });
                    main.push(Vec::new());
                    lower.push(Vec::new());
                    past_split = false;
                }
                Kind::Output => {
                    if let Some(path) = paths.last_mut() {
                        path.output = Some(position);
                    }
                }
                // The split appears after the output in the slot array even
                // though the signal reaches it earlier, which is why reading
                // the array as a running order goes wrong.
                Kind::Split => {
                    if let Some(path) = paths.last_mut() {
                        path.split = Some(position);
                    }
                    past_split = true;
                }
                Kind::Join => {
                    if let Some(path) = paths.last_mut() {
                        path.join = Some(position);
                    }
                }
                Kind::Block | Kind::Looper | Kind::Empty | Kind::Unknown(_) => {
                    if slot.model.is_none() {
                        continue; // a free position, not a block
                    }
                    let bucket = if past_split { &mut lower } else { &mut main };
                    if let Some(list) = bucket.last_mut() {
                        list.push(position);
                    }
                }
            }
        }

        for (n, path) in paths.iter_mut().enumerate() {
            let straight = main.get(n).cloned().unwrap_or_default();
            let branch = lower.get(n).cloned().unwrap_or_default();

            // Without a split - or with nothing on the lower branch - the path
            // is one undivided line and an empty second lane would be noise.
            let divides = path.split.is_some() && !branch.is_empty();
            if !divides {
                path.head = straight;
                continue;
            }

            let split_at = path
                .split
                .and_then(|p| self.attach(p, key::SPLIT_BODY))
                .unwrap_or(0);
            let join_at = path
                .join
                .and_then(|p| self.attach(p, key::JOIN_BODY))
                .unwrap_or(usize::MAX);

            path.head = straight.iter().copied().filter(|p| *p < split_at).collect();
            let upper: Vec<usize> = straight
                .iter()
                .copied()
                .filter(|p| *p >= split_at && *p < join_at)
                .collect();
            path.tail = straight.iter().copied().filter(|p| *p >= join_at).collect();

            path.lanes = vec![
                Lane {
                    branch: 0,
                    blocks: upper,
                    span: split_at..join_at.min(self.slots.len()),
                },
                Lane {
                    branch: 1,
                    blocks: branch,
                    span: path.split.map_or(0, |p| p + 1)..path.join.unwrap_or(self.slots.len()),
                },
            ];
        }

        Layout { paths }
    }

    /// Where an endpoint slot is routed: which physical input it listens to,
    /// or which output it feeds.
    ///
    /// HX Edit shows this as the first control on Input and Main L/R, but the
    /// device does not send it among the parameter values - it lives beside
    /// them under its own key, so it has to be read and written separately.
    /// The number indexes the model's `@input`/`@output` menu.
    pub fn routing(&self, position: usize) -> Option<i64> {
        let key = Self::routing_key(self.slots.get(position)?.kind)?;
        self.slot_body(position)?.get(key).and_then(Value::as_i64)
    }

    /// Point an endpoint slot somewhere else. Returns false if the slot is not
    /// an input or output, or has no routing field to change.
    pub fn set_routing(&mut self, position: usize, to: i64) -> bool {
        if to < 0 {
            return false;
        }
        let Some(key) = self
            .slots
            .get(position)
            .map(|s| s.kind)
            .and_then(Self::routing_key)
        else {
            return false;
        };
        let Some(field) = self.slot_body_mut(position).and_then(|b| b.get_mut(key)) else {
            return false;
        };
        // Written at the width it arrived at: a preset carries a table of byte
        // offsets into itself, so a narrower integer shifts everything after it
        // and the device reads the preset back as empty.
        *field = match field {
            Value::Wide(_, w) => Value::Wide(to as u64, *w),
            Value::WideInt(_, w) => Value::WideInt(to, *w),
            _ => Value::Int(to),
        };
        true
    }

    fn routing_key(kind: Kind) -> Option<i64> {
        match kind {
            Kind::Input => Some(key::INPUT_FROM),
            Kind::Output => Some(key::OUTPUT_TO),
            _ => None,
        }
    }

    fn slot_body(&self, position: usize) -> Option<&Value> {
        match self.tone.get(key::PATH)?.get(key::SLOTS)? {
            Value::Array(items) => items.get(position)?.get(key::BODY),
            _ => None,
        }
    }

    fn slot_body_mut(&mut self, position: usize) -> Option<&mut Value> {
        match self.tone.at_mut(&[key::PATH, key::SLOTS])? {
            Value::Array(items) => items.get_mut(position)?.get_mut(key::BODY),
            _ => None,
        }
    }

    /// Recompute the section-offset table from this preset's own encoding.
    ///
    /// The table is a directory of the tone's top-level sections: the offset
    /// of the tone map itself, then the offsets of keys 0, 1, 3, 4, 2, 5, 6,
    /// 7 and 10 in that fixed slot order - each pointing at the key byte -
    /// then the blob's total length twice: twelve little-endian u32s.
    /// Confirmed against two captured presets, and by the test that rebuilds
    /// the fixture's table byte for byte.
    ///
    /// [`encode`](Self::encode) uses this, which is what makes an edit that
    /// changes the document's length - pasting a block, clearing one - safe to
    /// write back at all.
    pub fn computed_sections(&self) -> Option<Vec<u8>> {
        const SLOT_ORDER: [i64; 9] = [0, 1, 3, 4, 2, 5, 6, 7, 10];

        // The prefix the tone will sit behind: magic, then the table itself,
        // whose length is fixed at 9 + 1 + tail entries.
        let magic = Encoder::encode(&Value::Str(Self::MAGIC.to_owned()));
        let table_len = (1 + SLOT_ORDER.len() + 2) * 4;
        let table_hdr =
            Encoder::encode(&Value::Bin(vec![0; table_len], self.sections_width)).len() - table_len;
        let tone_at = magic.len() + table_hdr + table_len;

        let tone = Encoder::encode(&self.tone);
        let total = tone_at + tone.len();

        // Walk the tone's top-level map recording where each key begins.
        let mut key_at = std::collections::HashMap::new();
        let (count, mut at) = match tone.as_slice() {
            [b @ 0x80..=0x8f, ..] => ((b & 0x0f) as usize, 1),
            [0xde, high, low, ..] => (u16::from_be_bytes([*high, *low]) as usize, 3),
            [0xdf, a, b, c, d, ..] => (u32::from_be_bytes([*a, *b, *c, *d]) as usize, 5),
            _ => return None,
        };
        for _ in 0..count {
            let here = at;
            let mut d = Decoder::new(&tone[at..]);
            let key = d.value().ok()?.as_i64()?;
            let after_key = tone.len() - at - d.remaining() + at;
            let mut dv = Decoder::new(&tone[after_key..]);
            dv.value().ok()?;
            at = tone.len() - dv.remaining();
            key_at.insert(key, tone_at + here);
        }

        let mut out = Vec::with_capacity(table_len);
        out.extend_from_slice(&section_word(tone_at)?);
        for key in SLOT_ORDER {
            out.extend_from_slice(&section_word(*key_at.get(&key)?)?);
        }
        out.extend_from_slice(&section_word(total)?);
        out.extend_from_slice(&section_word(total)?);
        Some(out)
    }

    /// Lift a slot out of the document, for pasting elsewhere.
    ///
    /// The whole slot is taken verbatim - model, values, paired cab, the
    /// engine tag, everything - because a block is more than this type models
    /// and rebuilding one from the parts we understand would quietly drop the
    /// rest. HX Edit's block copy is a client-side clipboard too.
    pub fn copy_slot(&self, position: usize) -> Option<Value> {
        match self.tone.get(key::PATH)?.get(key::SLOTS)? {
            Value::Array(items) => items.get(position).cloned(),
            _ => None,
        }
    }

    /// Drop a previously copied slot into `position`.
    ///
    /// Returns false if either side is not an effect/empty position. Inputs,
    /// outputs and junctions are fixtures of the topology, not slots that can
    /// be moved or filled.
    pub fn paste_slot(&mut self, position: usize, slot: &Value) -> bool {
        if !slot_is_well_formed(slot) {
            return false;
        }
        let source_kind = slot
            .get(key::KIND)
            .and_then(Value::as_i64)
            .map(Kind::from_wire);
        if !matches!(source_kind, Some(Kind::Block) | Some(Kind::Empty)) {
            return false;
        }
        let kind = self.slots.get(position).map(|s| s.kind);
        if !matches!(kind, Some(Kind::Block) | Some(Kind::Empty)) {
            return false;
        }
        let Some(Value::Array(items)) = self.tone.at_mut(&[key::PATH, key::SLOTS]) else {
            return false;
        };
        let Some(existing) = items.get_mut(position) else {
            return false;
        };
        *existing = slot.clone();
        self.slots = collect_slots(
            self.tone
                .get(key::PATH)
                .and_then(|p| p.get(key::SLOTS))
                .unwrap_or(&Value::Nil),
        );
        true
    }

    /// Free up `at` by sliding the blocks after it one place along.
    ///
    /// Returns false when the lane is full - there is no empty slot after `at`
    /// to slide into - rather than dropping whatever fell off the end.
    ///
    /// `within` bounds the search so a block never slides out of its lane and
    /// into the other branch of a split, which would silently rewire the
    /// preset.
    pub fn make_room(&mut self, at: usize, within: std::ops::Range<usize>) -> bool {
        if !within.contains(&at) {
            return false;
        }
        let Some(free) = (at..within.end).find(|p| {
            self.slots.get(*p).is_some_and(|s| {
                s.model.is_none() && s.kind != Kind::Input && s.kind != Kind::Output
            })
        }) else {
            return false;
        };
        let Some(Value::Array(items)) = self.tone.at_mut(&[key::PATH, key::SLOTS]) else {
            return false;
        };
        if free >= items.len() {
            return false;
        }
        // Slide everything from `at` up to the free slot along by one, which
        // leaves the empty one sitting at `at`.
        items[at..=free].rotate_right(1);
        self.slots = collect_slots(
            self.tone
                .get(key::PATH)
                .and_then(|p| p.get(key::SLOTS))
                .unwrap_or(&Value::Nil),
        );
        true
    }

    /// The slot range a lane occupies, for bounding [`make_room`].
    pub fn lane_bounds(&self, position: usize) -> Option<std::ops::Range<usize>> {
        let layout = self.layout();
        for path in &layout.paths {
            let upper = path.input.map_or(0, |i| i + 1)..path.output.unwrap_or(self.slots.len());
            if upper.contains(&position) {
                return Some(upper);
            }
            let lower = path.split.map_or(0, |s| s + 1)..path.join.unwrap_or(self.slots.len());
            if path.split.is_some() && lower.contains(&position) {
                return Some(lower);
            }
        }
        None
    }

    /// Lift a snapshot out of the document.
    pub fn copy_snapshot(&self, index: usize) -> Option<Value> {
        match self.tone.get(key::SNAPSHOT_SECTION)?.get(key::SNAPSHOTS)? {
            Value::Array(entries) => entries.get(index).cloned(),
            _ => None,
        }
    }

    /// Overwrite a snapshot, keeping its existing name.
    ///
    /// The name stays because a snapshot's name describes the song section it
    /// is for, and copying settings from one part to another should not
    /// relabel where you are.
    pub fn paste_snapshot(&mut self, index: usize, snapshot: &Value) -> bool {
        if !snapshot_is_well_formed(snapshot, self.slots.len()) {
            return false;
        }
        let name = self.snapshots().get(index).cloned().unwrap_or_default();
        let Some(Value::Array(entries)) =
            self.tone.at_mut(&[key::SNAPSHOT_SECTION, key::SNAPSHOTS])
        else {
            return false;
        };
        let Some(existing) = entries.get_mut(index) else {
            return false;
        };
        *existing = snapshot.clone();
        self.set_snapshot_name(index, &name);
        true
    }

    /// The raw body of one slot, for protocol work.
    pub fn raw_slot(&self, position: usize) -> Option<&Value> {
        self.slot_body(position)
    }

    /// Slots holding an actual effect, with their position in the chain.
    pub fn blocks(&self) -> impl Iterator<Item = (usize, &Slot)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == Kind::Block && s.model.is_some())
    }
}

fn collect_slots(slots: &Value) -> Vec<Slot> {
    let Value::Array(items) = slots else {
        return Vec::new();
    };
    items.iter().map(read_slot).collect()
}

fn section_word(offset: usize) -> Option<[u8; 4]> {
    u32::try_from(offset).ok().map(u32::to_le_bytes)
}

fn assignments_are_well_formed(tone: &Value) -> bool {
    let by_source = match tone.get(key::ASSIGNMENTS) {
        None | Some(Value::Nil) => return true,
        Some(Value::Array(by_source)) => by_source,
        Some(_) => return false,
    };
    by_source.iter().enumerate().all(|(ordinal, entries)| {
        let entries = match entries {
            Value::Nil => return true,
            Value::Array(entries) => entries,
            _ => return false,
        };
        if !entries.is_empty() && crate::rpc::Source::from_ordinal(ordinal as i64).is_none() {
            return false;
        }
        entries.iter().all(|entry| {
            let Some(what) = entry.get(key::ASSIGNED_WHAT) else {
                return false;
            };
            let valid_block = what
                .get(key::ASSIGNED_ON)
                .and_then(Value::as_i64)
                .is_some_and(|block| block >= 0);
            let valid_target = match what.get(key::ASSIGNED_BYPASS) {
                Some(bypass) => bypass.as_i64().is_some_and(|target| target >= 0),
                None => what
                    .get(key::ASSIGNED_TARGET)
                    .and_then(|target| target.get(key::ASSIGNED_PARAM))
                    .and_then(Value::as_i64)
                    .is_some_and(|parameter| parameter >= 0),
            };
            let valid_ends = [key::ASSIGNED_MIN, key::ASSIGNED_MAX]
                .into_iter()
                .all(|field| match what.get(field) {
                    None | Some(Value::Nil) => true,
                    Some(value) => value.as_f32().is_some_and(f32::is_finite),
                });
            let valid_kind = match what.get(key::ASSIGNED_KIND) {
                None | Some(Value::Nil) => ordinal != 8,
                Some(value) => value
                    .as_i64()
                    .is_some_and(|kind| ordinal != 8 || (0..=127).contains(&kind)),
            };
            valid_block && valid_target && valid_ends && valid_kind
        })
    })
}

fn snapshots_are_well_formed(tone: &Value, slot_count: usize) -> bool {
    let section = match tone.get(key::SNAPSHOT_SECTION) {
        None | Some(Value::Nil) => return true,
        Some(section) => section,
    };
    let Some(Value::Array(entries)) = section.get(key::SNAPSHOTS) else {
        return false;
    };
    entries
        .iter()
        .all(|entry| snapshot_is_well_formed(entry, slot_count))
}

fn snapshot_is_well_formed(entry: &Value, slot_count: usize) -> bool {
    if !matches!(entry, Value::Map(_)) {
        return false;
    }
    let valid_name = entry
        .get(key::SNAPSHOT_NAME)
        .and_then(Value::as_str)
        .is_some_and(|name| !name.is_empty());
    let valid_tempo = entry
        .get(key::SNAPSHOT_TEMPO)
        .is_some_and(tempo_is_well_formed);
    let valid_flags = [key::SNAPSHOT_VALID, key::SNAPSHOT_NAMED]
        .into_iter()
        .all(|field| matches!(entry.get(field), Some(Value::Bool(_))));
    let valid_slots = match entry.get(key::SNAPSHOT_SLOTS) {
        Some(Value::Array(slots)) if slots.len() == slot_count => {
            slots.iter().all(|slot| match slot {
                Value::Array(pair) => {
                    pair.len() == 2 && matches!(pair.get(1), Some(Value::Bool(_)))
                }
                _ => false,
            })
        }
        _ => false,
    };
    valid_name && valid_tempo && valid_flags && valid_slots
}

fn tempo_is_well_formed(value: &Value) -> bool {
    value
        .as_f32()
        .is_some_and(|tempo| tempo.is_finite() && (40.0..=240.0).contains(&tempo))
}

fn settings_are_well_formed(tone: &Value) -> bool {
    match tone.get(key::SETTINGS) {
        None | Some(Value::Nil) => true,
        Some(settings @ Value::Map(_)) => settings.get(key::TEMPO).is_none_or(tempo_is_well_formed),
        Some(_) => false,
    }
}

fn endpoint_route_is_well_formed(raw: &Value) -> bool {
    let route_key = match raw
        .get(key::KIND)
        .and_then(Value::as_i64)
        .map(Kind::from_wire)
    {
        Some(Kind::Input) => key::INPUT_FROM,
        Some(Kind::Output) => key::OUTPUT_TO,
        _ => return true,
    };
    match raw.get(key::BODY) {
        None | Some(Value::Nil) => true,
        Some(body @ Value::Map(_)) => body
            .get(route_key)
            .is_none_or(|route| route.as_i64().is_some_and(|route| route >= 0)),
        Some(_) => false,
    }
}

fn junction_is_well_formed(raw: &Value) -> bool {
    let body_key = match raw
        .get(key::KIND)
        .and_then(Value::as_i64)
        .map(Kind::from_wire)
    {
        Some(Kind::Split) => key::SPLIT_BODY,
        Some(Kind::Join) => key::JOIN_BODY,
        _ => return true,
    };
    let Some(body @ Value::Map(_)) = raw.get(key::BODY).and_then(|body| body.get(body_key)) else {
        return false;
    };
    let valid_model = body
        .get(key::JUNCTION_MODEL)
        .and_then(Value::as_i64)
        .and_then(|model| u32::try_from(model).ok())
        .is_some();
    let valid_attach = body
        .get(key::ATTACH)
        .and_then(Value::as_i64)
        .is_some_and(|attach| attach >= 0);
    let valid_snapshot_flag = body
        .get(key::JUNCTION_SNAPSHOTS)
        .is_none_or(|flag| matches!(flag, Value::Bool(_)));
    valid_model && valid_attach && valid_snapshot_flag
}

fn slot_is_well_formed(raw: &Value) -> bool {
    let Some(kind) = raw
        .get(key::KIND)
        .and_then(Value::as_i64)
        .map(Kind::from_wire)
    else {
        return false;
    };
    let Some(body) = raw.get(key::BODY) else {
        return !matches!(kind, Kind::Block | Kind::Looper);
    };
    if !matches!(kind, Kind::Unknown(_)) && !matches!(body, Value::Map(_) | Value::Nil) {
        return false;
    }
    let body = match kind {
        Kind::Split => body.get(key::SPLIT_BODY).unwrap_or(body),
        Kind::Join => body.get(key::JOIN_BODY).unwrap_or(body),
        _ => body,
    };
    if matches!(kind, Kind::Block | Kind::Looper) && model_number(body).is_none() {
        return false;
    }
    if body
        .get(key::ENABLED)
        .is_some_and(|enabled| !matches!(enabled, Value::Bool(_)))
    {
        return false;
    }
    if let Some(paired) = body
        .get(key::MODEL_REF)
        .and_then(|reference| reference.get(key::PAIRED_MODEL))
    {
        let Some(paired) = paired.as_i64() else {
            return false;
        };
        if paired != -1 && u32::try_from(paired).is_err() {
            return false;
        }
    }
    [key::VALUES, key::IO_VALUES, key::PAIRED_VALUES]
        .into_iter()
        .all(|field| body.get(field).is_none_or(value_array_is_well_formed))
}

fn value_array_is_well_formed(value: &Value) -> bool {
    let Some(Value::Array(items)) = value.get(key::ARRAY_VALUES) else {
        return false;
    };
    let count_matches = value.get(2).is_none_or(|count| {
        count.as_i64().and_then(|count| usize::try_from(count).ok()) == Some(items.len())
    });
    // Key 3 is usually the same count, but is deliberately one short for a
    // few model families. It may not claim more values than the array holds.
    let secondary_count_fits = value.get(3).is_none_or(|count| {
        count
            .as_i64()
            .and_then(|count| usize::try_from(count).ok())
            .is_some_and(|count| count <= items.len())
    });
    count_matches
        && secondary_count_fits
        && items.iter().all(|item| match item {
            Value::Bool(_) => true,
            other => other.as_f32().is_some_and(f32::is_finite),
        })
}

fn model_number(body: &Value) -> Option<u32> {
    body.get(key::MODEL_REF)
        .and_then(|reference| reference.get(key::MODEL))
        .or_else(|| body.get(key::INLINE_MODEL))
        .and_then(Value::as_i64)
        .and_then(|number| u32::try_from(number).ok())
}

fn read_slot(raw: &Value) -> Slot {
    let kind = Kind::from_wire(raw.get(key::KIND).and_then(Value::as_i64).unwrap_or(-1));
    let Some(body) = raw.get(key::BODY) else {
        return Slot {
            kind,
            model: None,
            paired: None,
            enabled: false,
            values: Vec::new(),
            paired_values: Vec::new(),
            type_tag: 0,
        };
    };

    // Splits and joins nest their model and values one level deeper than
    // effects do, and inputs and outputs have no model at all.
    let body = match kind {
        Kind::Split => body.get(key::SPLIT_BODY).unwrap_or(body),
        Kind::Join => body.get(key::JOIN_BODY).unwrap_or(body),
        _ => body,
    };

    let reference = body.get(key::MODEL_REF);
    let model = model_number(body);

    // A paired model is written as -1 when absent, so a plain "is it there"
    // check would report every block as having a cab.
    let paired = reference
        .and_then(|r| r.get(key::PAIRED_MODEL))
        .and_then(Value::as_i64)
        .and_then(|n| u32::try_from(n).ok());

    let values = body
        .get(key::VALUES)
        .or_else(|| body.get(key::IO_VALUES))
        .map(read_values)
        .unwrap_or_default();

    Slot {
        kind,
        model,
        paired,
        // Inputs, outputs and empty slots have no switch; treat them as on so
        // callers do not have to special-case them when drawing the chain.
        enabled: body
            .get(key::ENABLED)
            .is_none_or(|v| v == &Value::Bool(true)),
        values,
        paired_values: body
            .get(key::PAIRED_VALUES)
            .map(read_values)
            .unwrap_or_default(),
        type_tag: body.get(key::TYPE_TAG).and_then(Value::as_i64).unwrap_or(0),
    }
}

/// Read a `{2: count, 3: count, 4: [...]}` value array.
///
/// Booleans appear inline among the floats - a switch parameter sits in the
/// same array as the knobs - so they are folded to 0.0 and 1.0 rather than
/// dropped, which keeps positions aligned with the parameter list.
fn read_values(array: &Value) -> Vec<f32> {
    match array.get(key::ARRAY_VALUES) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::Bool(b) => *b as u8 as f32,
                other => other.as_f32().unwrap_or(0.0),
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msgpack::Encoder;

    /// The HX Stomp's factory `DIR:Relief`, whose numbers came out of the same
    /// preset exported twice: once by us from the device's own bytes, once by
    /// HX Edit. The device holds its blocks in slots 1, 6 and 12 to 15; HX Edit
    /// numbers them 0 and 5 on branch 0 and 1 to 4 on branch 1, and puts the
    /// split at 1 and the join at 6.
    fn relief() -> Layout {
        Layout {
            paths: vec![Path {
                input: Some(0),
                output: Some(9),
                split: Some(10),
                join: Some(19),
                head: vec![1],
                lanes: vec![
                    Lane {
                        branch: 0,
                        blocks: vec![6],
                        span: 2..7,
                    },
                    Lane {
                        branch: 1,
                        blocks: vec![12, 13, 14, 15],
                        span: 11..19,
                    },
                ],
                tail: vec![],
            }],
        }
    }

    #[test]
    fn a_row_index_becomes_the_slot_it_names() {
        let layout = relief();
        assert_eq!(
            layout.slot_of(0, 0, 0),
            Some(1),
            "the amp, before the split"
        );
        assert_eq!(
            layout.slot_of(0, 0, 5),
            Some(6),
            "the plate, on the upper branch"
        );
        assert_eq!(
            layout.slot_of(0, 1, 1),
            Some(12),
            "first of the lower branch"
        );
        assert_eq!(
            layout.slot_of(0, 1, 4),
            Some(15),
            "last of the lower branch"
        );
        // A junction's number means "before this cell", which is the same sum.
        assert_eq!(layout.slot_of(0, 0, 1), Some(2), "where the split attaches");
        assert_eq!(layout.slot_of(0, 0, 6), Some(7), "where the join attaches");
        assert_eq!(layout.slot_of(1, 0, 0), None, "there is no second path");
        assert_eq!(
            layout.slot_of(0, 0, usize::MAX),
            None,
            "an imported row index must not overflow"
        );
    }

    #[test]
    fn and_back_again() {
        let layout = relief();
        for (slot, expected) in [(1, (0, 0)), (6, (0, 5)), (12, (1, 1)), (15, (1, 4))] {
            assert_eq!(layout.position_of(0, slot), Some(expected), "slot {slot}");
        }
    }

    /// The parameter index is key 29 of the target map and key 28 is the path,
    /// which is the opposite way round from the request that makes the
    /// assignment. Reading 28 made every assignment on the pedal report
    /// parameter 0, so a footswitch put on a Bright switch was drawn owning the
    /// Drive knob. The shape here is a real one, from `CT-Master Lead`.
    #[test]
    fn the_parameter_index_comes_from_key_29() {
        let assignment = crate::msgmap! {
            key::ASSIGNED_KIND => Value::Int(4),
            key::ASSIGNED_MIN => Value::Int(1),
            key::ASSIGNED_MAX => Value::Int(4),
            key::ASSIGNED_ON => Value::Int(3),
            key::ASSIGNED_TARGET => crate::msgmap! {
                28 => Value::Int(0),
                key::ASSIGNED_PARAM => Value::Int(1),
                41 => Value::Bool(false),
            },
        };
        let preset = Preset {
            sections: Vec::new(),
            sections_width: 0,
            slots: Vec::new(),
            tone: crate::msgmap! {
                key::ASSIGNMENTS => Value::Array(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    // Ordinal 3 is Footswitch 1.
                    Value::Array(vec![crate::msgmap! {
                        0 => Value::Int(0),
                        key::ASSIGNED_WHAT => assignment,
                    }]),
                ]),
            },
        };

        let found = preset.assignments();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].target, Target::Param(1), "key 29, not key 28");
        assert_eq!(found[0].block, 3);
        assert_eq!(found[0].source, crate::rpc::Source::Footswitch(1));
        // In the parameter's own units, not a percentage.
        assert_eq!((found[0].min, found[0].max), (1.0, 4.0));
    }

    /// A MIDI bypass, exactly as the pedal wrote it into the document when
    /// `tonepush assign 3 --cc 77` put block 2's bypass on CC 77:
    ///
    /// ```text
    /// {0: 8, 1: 77, 2: nil, 3: nil, 4: 2, 5: 2, 9: 5, 10: 300, 11: false, 12: 0}
    /// ```
    ///
    /// Key 1 is the CC, not a kind. Reading it as a kind took anything but 0
    /// for a parameter, found no parameter target, and dropped the assignment -
    /// so a bypass on CC 77 was invisible and the same bypass on CC 0 was not.
    #[test]
    fn a_midi_bypass_is_found_whatever_its_cc() {
        let bypass = |cc: i64| {
            crate::msgmap! {
                0 => Value::Int(8),
                key::ASSIGNED_KIND => Value::Int(cc),
                key::ASSIGNED_MIN => Value::Nil,
                key::ASSIGNED_MAX => Value::Nil,
                4 => Value::Int(2),
                key::ASSIGNED_ON => Value::Int(2),
                key::ASSIGNED_BYPASS => Value::Int(5),
                10 => Value::Int(300),
                11 => Value::Bool(false),
                12 => Value::Int(0),
            }
        };
        let preset = |cc: i64| Preset {
            sections: Vec::new(),
            sections_width: 0,
            slots: Vec::new(),
            tone: crate::msgmap! {
                key::ASSIGNMENTS => Value::Array(
                    // Ordinal 8 is MIDI CC.
                    (0..9).map(|o| if o == 8 {
                        Value::Array(vec![crate::msgmap! {
                            0 => Value::Int(1),
                            key::ASSIGNED_WHAT => bypass(cc),
                        }])
                    } else {
                        Value::Nil
                    }).collect::<Vec<_>>()
                ),
            },
        };

        for cc in [0, 4, 42, 77, 127] {
            let found = preset(cc).assignments();
            assert_eq!(found.len(), 1, "a bypass on CC {cc} went missing");
            assert_eq!(found[0].target, Target::Bypass);
            assert_eq!(found[0].block, 2);
            assert_eq!(found[0].source, crate::rpc::Source::MidiCc);
            assert_eq!(found[0].cc, Some(cc), "and it should say which CC");
        }
    }

    /// The other half of the same trap: under anything but MIDI, key 1 really
    /// is the constant 4, and reading it as a CC would report every knob on a
    /// pedal or a switch as being on CC 4.
    #[test]
    fn a_parameter_carries_no_cc_in_that_key() {
        let assignment = crate::msgmap! {
            key::ASSIGNED_KIND => Value::Int(4),
            key::ASSIGNED_MIN => Value::Int(0),
            key::ASSIGNED_MAX => Value::Int(1),
            key::ASSIGNED_ON => Value::Int(1),
            key::ASSIGNED_TARGET => crate::msgmap! {
                28 => Value::Int(0),
                key::ASSIGNED_PARAM => Value::Int(0),
            },
        };
        let preset = Preset {
            sections: Vec::new(),
            sections_width: 0,
            slots: Vec::new(),
            tone: crate::msgmap! {
                key::ASSIGNMENTS => Value::Array(vec![
                    Value::Nil,
                    Value::Array(vec![crate::msgmap! {
                        0 => Value::Int(0),
                        key::ASSIGNED_WHAT => assignment,
                    }]),
                ]),
            },
        };
        let found = preset.assignments();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].target, Target::Param(0));
        assert_eq!(
            found[0].source,
            crate::rpc::Source::Expression(1),
            "the source is what decides whether key 1 is a number or a constant"
        );
        assert_eq!(
            found[0].cc, None,
            "4 there means a controller exists, not CC 4"
        );
    }

    /// And a parameter *under MIDI* does carry its CC in that key, which is the
    /// same trap a third time. The entry below is the document the pedal wrote
    /// when block 5's Feedback was put under MIDI and opcode 64 sent 77; 42 and
    /// 43 before it landed in the same place, and 4 - the pedal's own default -
    /// is what made the field look like a constant.
    #[test]
    fn a_parameter_under_midi_says_which_cc() {
        let under_midi = |cc: i64| Preset {
            sections: Vec::new(),
            sections_width: 0,
            slots: Vec::new(),
            tone: crate::msgmap! {
                key::ASSIGNMENTS => Value::Array(
                    (0..9).map(|o| if o == 8 {
                        Value::Array(vec![crate::msgmap! {
                            0 => Value::Int(0),
                            key::ASSIGNED_WHAT => crate::msgmap! {
                                0 => Value::Int(8),
                                key::ASSIGNED_KIND => Value::Int(cc),
                                key::ASSIGNED_MIN => Value::Int(0),
                                key::ASSIGNED_MAX => Value::Int(1),
                                4 => Value::Int(0),
                                key::ASSIGNED_ON => Value::Int(5),
                                key::ASSIGNED_TARGET => crate::msgmap! {
                                    28 => Value::Int(0),
                                    key::ASSIGNED_PARAM => Value::Int(1),
                                    41 => Value::Bool(false),
                                },
                                7 => Value::Int(0),
                                13 => Value::Bool(false),
                            },
                        }])
                    } else {
                        Value::Nil
                    }).collect::<Vec<_>>()
                ),
            },
        };

        for cc in [4, 42, 43, 77] {
            let found = under_midi(cc).assignments();
            assert_eq!(found.len(), 1, "the assignment on CC {cc} went missing");
            assert_eq!(found[0].target, Target::Param(1));
            assert_eq!(found[0].block, 5);
            assert_eq!(found[0].source, crate::rpc::Source::MidiCc);
            assert_eq!(found[0].cc, Some(cc), "and it should say which CC");
        }
    }

    /// A branch with nothing on it is drawn as no lane at all, which is exactly
    /// the chain a file is loaded onto. The branch still begins after the split.
    #[test]
    fn an_empty_branch_still_knows_where_it_starts() {
        let mut layout = relief();
        layout.paths[0].lanes.clear();
        assert_eq!(layout.slot_of(0, 1, 1), Some(12));
        assert_eq!(layout.slot_of(0, 0, 5), Some(6));
    }

    /// Rebuild the shape of a real captured preset: a Scream 808 in slot 1
    /// with its three parameters, and an empty slot after it.
    fn sample() -> Vec<u8> {
        let slot = crate::msgmap! {
            key::KIND => Value::Int(6),
            key::BODY => crate::msgmap! {
                key::MODEL_REF => crate::msgmap! {
                    key::MODEL => Value::Int(101),
                    key::PAIRED_MODEL => Value::Int(-1),
                },
                key::ENABLED => Value::Bool(true),
                key::VALUES => crate::msgmap! {
                    2 => Value::Int(3),
                    key::ARRAY_VALUES => Value::Array(vec![
                        Value::F32(0.01), Value::F32(0.19), Value::F32(0.98),
                    ]),
                },
            },
        };
        let empty = crate::msgmap! { key::KIND => Value::Int(8), key::BODY => Value::Nil };
        let tone = crate::msgmap! {
            key::META => crate::msgmap! {
                key::FIRMWARE => Value::UInt(0x0380_0000),
                key::BUILD => Value::Str("v3.71-32-g1039661".into()),
            },
            key::PATH => crate::msgmap! {
                key::SLOTS => Value::Array(vec![slot, empty]),
            },
            1 => Value::Nil,
            3 => Value::Nil,
            key::ASSIGNMENTS => Value::Nil,
            2 => Value::Nil,
            key::SETTINGS => Value::Nil,
            6 => Value::Nil,
            key::SNAPSHOT_SECTION => Value::Nil,
        };
        Preset {
            sections: Vec::new(),
            sections_width: 0,
            slots: Vec::new(),
            tone,
        }
        .encode()
    }

    #[test]
    fn reads_blocks_and_their_values() {
        let preset = Preset::parse(&sample()).expect("parses");
        assert_eq!(preset.slots.len(), 2);

        let (position, block) = preset.blocks().next().expect("one block");
        assert_eq!(position, 0);
        assert_eq!(block.model, Some(101));
        assert!(block.enabled);
        assert_eq!(block.values, vec![0.01, 0.19, 0.98]);
    }

    #[test]
    fn empty_slots_hold_no_model() {
        let preset = Preset::parse(&sample()).unwrap();
        assert_eq!(preset.slots[1].kind, Kind::Empty);
        assert_eq!(preset.slots[1].model, None);
        assert_eq!(preset.blocks().count(), 1);
    }

    #[test]
    fn a_paired_model_of_minus_one_means_none() {
        let preset = Preset::parse(&sample()).unwrap();
        assert_eq!(preset.slots[0].paired, None);
    }

    #[test]
    fn negative_or_oversized_identifiers_do_not_wrap() {
        let raw = crate::msgmap! {
            key::KIND => Value::Int(6),
            key::BODY => crate::msgmap! {
                key::MODEL_REF => crate::msgmap! {
                    key::MODEL => Value::Int(-1),
                    key::PAIRED_MODEL => Value::Int(i64::MAX),
                },
            },
        };
        let slot = read_slot(&raw);
        assert_eq!(slot.model, None);
        assert_eq!(slot.paired, None);

        let mut preset = Preset::parse(&sample()).unwrap();
        *preset
            .tone
            .at_mut(&[key::META, key::FIRMWARE])
            .expect("firmware") = Value::Int(-1);
        assert_eq!(preset.firmware(), None);

        *preset
            .tone
            .at_mut(&[key::META, key::FIRMWARE])
            .expect("firmware") = Value::Int(0x1085_0000);
        assert_eq!(preset.firmware().as_deref(), Some("10.85"));

        *preset
            .tone
            .at_mut(&[key::META, key::FIRMWARE])
            .expect("firmware") = Value::Int(0x03fa_0000);
        assert_eq!(preset.firmware(), None);
    }

    #[test]
    fn reads_metadata() {
        let preset = Preset::parse(&sample()).unwrap();
        assert_eq!(preset.firmware().as_deref(), Some("3.80"));
        assert_eq!(preset.build(), Some("v3.71-32-g1039661"));
    }

    #[test]
    fn round_trips_through_the_wire_format() {
        let original = Preset::parse(&sample()).unwrap();
        let again = Preset::parse(&original.encode()).expect("re-parses");

        assert_eq!(again.slots, original.slots);
        assert_eq!(again.sections, original.sections);
        assert_eq!(again.firmware(), original.firmware());
        // Byte-for-byte, which is what makes writing a document back safe.
        assert_eq!(again.encode(), original.encode());
    }

    #[test]
    fn swapping_slots_moves_the_block_and_the_document_together() {
        let mut preset = Preset::parse(&sample()).unwrap();
        assert_eq!(preset.slots[0].model, Some(101));

        assert!(preset.swap_slots(0, 1));
        assert_eq!(preset.slots[1].model, Some(101));
        assert_eq!(preset.slots[0].kind, Kind::Empty);

        // The re-encoded document must agree with the in-memory view.
        let written = Preset::parse(&preset.encode()).unwrap();
        assert_eq!(written.slots, preset.slots);

        assert!(!preset.swap_slots(0, 99));
    }

    /// The endpoints and junctions are fixtures of the topology. Swapping a
    /// pedal into one - which the GUI's drag could once ask for - produced a
    /// document with an amp where the input should be.
    #[test]
    fn a_block_cannot_trade_places_with_a_fixture() {
        let mut preset = shaped(&[IN, BLOCK, OUT, SPLIT, BLOCK, JOIN]);
        assert!(!preset.swap_slots(1, 0), "not with the input");
        assert!(!preset.swap_slots(1, 2), "not with the output");
        assert!(!preset.swap_slots(1, 3), "not with the split");
        assert!(!preset.move_slot(1, 5), "not with the join");
        assert_eq!(preset.slots[0].kind, Kind::Input, "nothing moved");
    }

    /// Within a lane a move shifts the blocks in between along, the way HX
    /// Edit reorders - a swap would trade two pedals when the hand asked to
    /// slide one.
    #[test]
    fn moving_within_a_lane_shifts_the_blocks_between() {
        let mut preset = shaped(&[IN, BLOCK, EMPTY, BLOCK, OUT]);
        assert!(preset.move_slot(1, 3));
        assert_eq!(preset.slots[1].kind, Kind::Empty, "the hole shifted back");
        assert_eq!(preset.slots[2].kind, Kind::Block);
        assert_eq!(preset.slots[3].kind, Kind::Block);
        // The document agrees with the in-memory view after a round trip.
        let again = Preset::parse(&preset.encode()).unwrap();
        assert_eq!(again.slots, preset.slots);
    }

    /// Across lanes the slot array is not contiguous - the other branch lives
    /// past the output and the split - so a move is a trade of positions.
    #[test]
    fn moving_across_lanes_trades_positions() {
        let mut preset = shaped_at(&[IN, BLOCK, OUT, SPLIT, EMPTY, JOIN], 1, 2);
        assert!(preset.move_slot(1, 4));
        assert_eq!(preset.slots[1].kind, Kind::Empty);
        assert_eq!(preset.slots[4].kind, Kind::Block);
        assert_eq!(preset.slots[2].kind, Kind::Output, "the output stayed put");
    }

    /// Dropping into a gap lands the block in that gap, whichever side it
    /// came from: the index shifts by one when the block leaves from the
    /// left, because everything between closes ranks.
    #[test]
    fn dropping_into_a_gap_lands_in_that_gap() {
        // Rightward: block 1 into the gap before 3 lands between 2 and 3.
        let mut preset = shaped(&[IN, BLOCK, EMPTY, BLOCK, OUT]);
        assert!(preset.insert_slot(1, 3));
        assert_eq!(preset.slots[1].kind, Kind::Empty);
        assert_eq!(preset.slots[2].kind, Kind::Block, "landed before slot 3");

        // Leftward: block 3 into the gap before 1 lands at slot 1, and the
        // slots in between shift right to make way.
        let mut preset = shaped(&[IN, EMPTY, BLOCK, BLOCK, OUT]);
        assert!(preset.insert_slot(3, 1));
        assert_eq!(preset.slots[1].kind, Kind::Block, "landed at slot 1");
        assert_eq!(preset.slots[2].kind, Kind::Empty, "the hole shifted along");
        assert_eq!(preset.slots[3].kind, Kind::Block);

        // The gap beside the block itself is where it already is.
        let mut preset = shaped(&[IN, BLOCK, BLOCK, OUT]);
        let untouched = preset.encode();
        assert!(preset.insert_slot(1, 2), "a no-op, not an error");
        assert_eq!(preset.encode(), untouched);
    }

    /// The gap before the output means the end of the line.
    #[test]
    fn dropping_before_the_output_moves_the_block_to_the_end() {
        let mut preset = shaped(&[IN, BLOCK, BLOCK, EMPTY, OUT]);
        assert!(preset.insert_slot(1, 4));
        assert_eq!(preset.slots[1].kind, Kind::Block, "slot 2 shifted back");
        assert_eq!(preset.slots[3].kind, Kind::Block, "the mover took the end");
        assert_eq!(preset.slots[2].kind, Kind::Empty);
    }

    /// A drop into the other lane's gap frees the landing slot first, rather
    /// than trading with whatever happened to sit beyond the gap.
    #[test]
    fn dropping_into_the_other_lane_makes_room() {
        let mut preset = shaped_at(&[IN, BLOCK, OUT, SPLIT, BLOCK, EMPTY, JOIN], 1, 2);
        assert!(preset.insert_slot(1, 4), "main block into the branch's gap");
        assert_eq!(preset.slots[1].kind, Kind::Empty, "it left the main line");
        assert_eq!(preset.slots[4].kind, Kind::Block, "landed in the gap");
        assert_eq!(preset.slots[5].kind, Kind::Block, "the occupant slid along");
    }

    /// An emptied branch goes back to the device's own bookkeeping - attach
    /// points at zero. Leaving them said "the fork is at slot 1" over
    /// nothing, and the device wipes the edit buffer over exactly that.
    #[test]
    fn settling_zeroes_the_attach_points_of_an_empty_branch() {
        let mut preset = shaped_at(&[IN, BLOCK, OUT, SPLIT, EMPTY, JOIN], 1, 2);
        preset.settle_branches();
        let layout = Preset::parse(&preset.encode()).unwrap().layout();
        assert!(layout.paths[0].lanes.is_empty(), "still one line");
        assert_eq!(
            preset.attach(3, key::SPLIT_BODY),
            Some(0),
            "the fork let go"
        );
        assert_eq!(
            preset.attach(5, key::JOIN_BODY),
            Some(0),
            "so did the merge"
        );
    }

    /// A branch with something on it keeps its attach points exactly.
    #[test]
    fn settling_leaves_an_occupied_branch_alone() {
        let mut preset = shaped_at(&[IN, BLOCK, OUT, SPLIT, BLOCK, JOIN], 1, 2);
        preset.settle_branches();
        assert_eq!(preset.attach(3, key::SPLIT_BODY), Some(1));
        assert_eq!(preset.attach(5, key::JOIN_BODY), Some(2));
    }

    /// Re-attaching the split moves the fork, and the layout reads it back.
    #[test]
    fn a_split_can_be_reattached() {
        let mut preset = shaped_at(&[IN, BLOCK, BLOCK, BLOCK, OUT, SPLIT, BLOCK, JOIN], 2, 4);
        assert!(preset.set_attach(5, 1), "the split accepts a new attach");
        assert!(preset.set_attach(7, 3), "so does the join");
        assert!(!preset.set_attach(1, 2), "a block does not");
        let before = preset.attach_of(5);
        assert!(!preset.set_attach(5, usize::MAX));
        assert_eq!(preset.attach_of(5), before, "overflow changes nothing");

        let layout = Preset::parse(&preset.encode()).unwrap().layout();
        let path = &layout.paths[0];
        assert_eq!(path.head, Vec::<usize>::new());
        assert_eq!(path.lanes[0].blocks, vec![1, 2], "the fork moved to slot 1");
        assert_eq!(path.tail, vec![3], "the merge moved to slot 3");
    }

    #[test]
    fn malformed_junction_records_are_rejected() {
        let kinds = [IN, BLOCK, BLOCK, OUT, SPLIT, BLOCK, JOIN];

        let mut bad_attach = shaped_at(&kinds, 1, 2);
        let split = bad_attach.layout().paths[0].split.unwrap();
        *bad_attach
            .slot_body_mut(split)
            .and_then(|body| body.get_mut(key::SPLIT_BODY))
            .and_then(|body| body.get_mut(key::ATTACH))
            .unwrap() = Value::Str("first".into());
        assert!(Preset::parse(&bad_attach.encode()).is_none());

        let mut bad_model = shaped_at(&kinds, 1, 2);
        let split = bad_model.layout().paths[0].split.unwrap();
        *bad_model
            .slot_body_mut(split)
            .and_then(|body| body.get_mut(key::SPLIT_BODY))
            .and_then(|body| body.get_mut(key::JUNCTION_MODEL))
            .unwrap() = Value::Int(-1);
        assert!(Preset::parse(&bad_model.encode()).is_none());

        let mut missing_body = shaped_at(&kinds, 1, 2);
        let split = missing_body.layout().paths[0].split.unwrap();
        *missing_body
            .slot_body_mut(split)
            .and_then(|body| body.get_mut(key::SPLIT_BODY))
            .unwrap() = Value::Nil;
        assert!(Preset::parse(&missing_body.encode()).is_none());
    }

    #[test]
    fn tempo_can_be_changed_and_survives_a_round_trip() {
        let mut preset = Preset::parse(&sample()).unwrap();
        assert!(
            !preset.set_tempo(96.0),
            "the sample has no settings section"
        );

        // With a settings section present it takes, and comes back out again.
        *preset.tone.get_mut(key::SETTINGS).unwrap() =
            crate::msgmap! { key::TEMPO => Value::F32(120.0) };
        assert!(preset.set_tempo(96.0));
        assert_eq!(preset.tempo(), Some(96.0));
        assert_eq!(Preset::parse(&preset.encode()).unwrap().tempo(), Some(96.0));
    }

    #[test]
    fn invalid_tempo_edits_leave_the_preset_unchanged() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let before = preset.tempo().unwrap();

        for invalid in [39.9, 240.1, f32::NAN, f32::INFINITY] {
            assert!(!preset.set_tempo(invalid));
            assert_eq!(preset.tempo(), Some(before));
        }
    }

    #[test]
    fn invalid_preset_tempos_are_rejected() {
        let document = |tempo: Value| {
            let mut preset = Preset::parse(&sample()).unwrap();
            *preset.tone.get_mut(key::SETTINGS).unwrap() = crate::msgmap! { key::TEMPO => tempo };
            preset.encode()
        };
        assert!(Preset::parse(&document(Value::F32(40.0))).is_some());
        assert!(Preset::parse(&document(Value::F32(240.0))).is_some());
        assert!(Preset::parse(&document(Value::F32(39.9))).is_none());
        assert!(Preset::parse(&document(Value::F32(240.1))).is_none());
        assert!(Preset::parse(&document(Value::F32(f32::NAN))).is_none());
        assert!(Preset::parse(&document(Value::Str("fast".into()))).is_none());
    }

    #[test]
    fn snapshots_can_be_renamed() {
        let mut preset = Preset::parse(&sample()).unwrap();
        preset.tone = crate::msgmap! {
            key::SNAPSHOT_SECTION => crate::msgmap! {
                key::SNAPSHOTS => Value::Array(vec![
                    crate::msgmap! { key::SNAPSHOT_NAME => Value::Str("SNAPSHOT 1".into()) },
                ]),
            },
        };
        assert!(preset.set_snapshot_name(0, "Verse"));
        assert_eq!(preset.snapshots(), vec!["Verse".to_string()]);
        assert!(!preset.set_snapshot_name(9, "Nope"));
        assert!(!preset.set_snapshot_name(0, ""));
        assert_eq!(preset.snapshots(), vec!["Verse".to_string()]);
    }

    /// Build a preset from a bare list of slot kinds. Blocks are given a model
    /// so they count as occupied - a slot with no model is a free position.
    fn shaped(kinds: &[i64]) -> Preset {
        shaped_at(kinds, 0, usize::MAX)
    }

    /// As `shaped`, but with the split and join attached at given slots.
    fn shaped_at(kinds: &[i64], split_at: usize, join_at: usize) -> Preset {
        let slot = |k: i64| {
            let mut fields = vec![(crate::msgpack::Key::Int(key::KIND), Value::Int(k))];
            if k == BLOCK {
                fields.push((
                    crate::msgpack::Key::Int(key::BODY),
                    crate::msgmap! {
                        key::MODEL_REF => crate::msgmap! { key::MODEL => Value::Int(101) },
                    },
                ));
            }
            if k == SPLIT {
                fields.push((
                    crate::msgpack::Key::Int(key::BODY),
                    crate::msgmap! {
                        key::SPLIT_BODY => crate::msgmap! {
                            key::INLINE_MODEL => Value::Int(257),
                            key::ATTACH => Value::Int(split_at as i64),
                        },
                    },
                ));
            }
            if k == JOIN {
                fields.push((
                    crate::msgpack::Key::Int(key::BODY),
                    crate::msgmap! {
                        key::JOIN_BODY => crate::msgmap! {
                            key::INLINE_MODEL => Value::Int(151),
                            key::ATTACH => Value::Int(join_at.min(9999) as i64),
                        },
                    },
                ));
            }
            Value::Map(fields)
        };
        let tone = crate::msgmap! {
            key::META => Value::Nil,
            key::PATH => crate::msgmap! {
                key::SLOTS => Value::Array(kinds.iter().map(|k| slot(*k)).collect()),
            },
            1 => Value::Nil,
            3 => Value::Nil,
            key::ASSIGNMENTS => Value::Nil,
            2 => Value::Nil,
            key::SETTINGS => Value::Nil,
            6 => Value::Nil,
            key::SNAPSHOT_SECTION => Value::Nil,
        };
        let blob = Preset {
            sections: Vec::new(),
            sections_width: 0,
            slots: Vec::new(),
            tone,
        }
        .encode();
        Preset::parse(&blob).unwrap()
    }

    /// A real preset captured from an HX Stomp, shared with the round-trip test.
    const FIXTURE: &[u8] = include_bytes!("../tests/preset.bin");

    // Kinds, for readable fixtures.
    const IN: i64 = 0;
    const OUT: i64 = 1;
    const SPLIT: i64 = 2;
    const JOIN: i64 = 3;
    const BLOCK: i64 = 6;
    const EMPTY: i64 = 8;

    /// A split divides a *stretch* of the path, not the whole of it: blocks
    /// before it and after the join carry the undivided signal. Getting this
    /// wrong drew every block as though it were on a branch.
    #[test]
    fn a_split_divides_only_the_stretch_between_its_attach_points() {
        // Slots 1..4 on the main row, split attaching before 2, join before 4.
        let layout = shaped_at(&[IN, BLOCK, BLOCK, BLOCK, OUT, SPLIT, BLOCK, JOIN], 2, 4).layout();

        let path = &layout.paths[0];
        assert_eq!(path.input, Some(0));
        assert_eq!(path.output, Some(4));
        assert_eq!(path.head, vec![1], "slot 1 precedes the split");
        assert_eq!(path.lanes.len(), 2);
        assert_eq!(path.lanes[0].blocks, vec![2, 3], "the upper branch");
        assert_eq!(path.lanes[1].blocks, vec![6], "the lower branch");
        assert_eq!(path.tail, Vec::<usize>::new());
    }

    #[test]
    fn blocks_after_the_join_are_shared_again() {
        // Split before 2, join before 3: one block on each branch, one after.
        let layout = shaped_at(&[IN, BLOCK, BLOCK, BLOCK, OUT, SPLIT, BLOCK, JOIN], 2, 3).layout();

        let path = &layout.paths[0];
        assert_eq!(path.head, vec![1]);
        assert_eq!(path.lanes[0].blocks, vec![2]);
        assert_eq!(path.lanes[1].blocks, vec![6]);
        assert_eq!(path.tail, vec![3], "slot 3 is past the join");
    }

    /// Helix and Helix LT carry two independent signal paths, so a preset that
    /// splits both has four lanes. Anything that assumes two is wrong there.
    #[test]
    fn a_device_with_two_signal_paths_yields_four_lanes() {
        let layout = shaped(&[
            IN, BLOCK, OUT, SPLIT, BLOCK, JOIN, // path 1, split
            IN, BLOCK, OUT, SPLIT, BLOCK, JOIN, // path 2, split
        ])
        .layout();

        assert_eq!(layout.paths.len(), 2);
        assert_eq!(layout.lanes().count(), 4);
        assert_eq!(layout.paths[1].input, Some(6));
        assert_eq!(layout.paths[1].lanes[1].blocks, vec![10]);
    }

    /// The split slot exists in every preset whether or not anything is on the
    /// lower branch, and an empty second row would be noise.
    #[test]
    fn a_split_with_nothing_below_it_draws_as_one_line() {
        let layout = shaped(&[IN, BLOCK, OUT, SPLIT, JOIN]).layout();
        assert!(layout.paths[0].lanes.is_empty());
        assert_eq!(layout.paths[0].head, vec![1]);
        assert_eq!(layout.paths[0].split, Some(3));
    }

    #[test]
    fn a_preset_without_a_split_has_one_line() {
        let layout = Preset::parse(&sample()).unwrap().layout();
        assert!(layout.paths.iter().all(|p| p.lanes.is_empty()));
    }

    /// The real thing. This preset's lower branch is empty, so it draws as one
    /// lane - but its split slot still sits *after* the output in the array,
    /// which is exactly the trap: read as a running order it puts the split and
    /// join on the end of the chain, which is where they were being drawn.
    #[test]
    fn the_captured_preset_keeps_its_split_out_of_the_running_order() {
        let layout = Preset::parse(FIXTURE).unwrap().layout();
        assert_eq!(layout.paths.len(), 1);
        let path = &layout.paths[0];

        assert!(
            path.split.unwrap() > path.output.unwrap(),
            "the split sits after the output in the slot array"
        );
        assert!(path.lanes.is_empty(), "nothing is on the lower branch");
        assert!(
            !path.head.contains(&path.split.unwrap()) && !path.head.contains(&path.join.unwrap()),
            "the split and join must not appear as blocks in the line"
        );
        // And there is room to put something on the lower branch.
        let preset = Preset::parse(FIXTURE).unwrap();
        let lower = Lane {
            branch: 1,
            blocks: Vec::new(),
            span: path.split.unwrap() + 1..path.join.unwrap(),
        };
        assert_eq!(lower.free(&preset).len(), 8);
    }

    /// The routing selector is the first thing HX Edit shows on Input and
    /// Main L/R, and it is not among the parameter values - it sits beside them
    /// under its own key, which is why it was missing from the editor.
    #[test]
    fn endpoint_routing_reads_and_writes() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let input = preset.layout().paths[0].input.unwrap();
        let output = preset.layout().paths[0].output.unwrap();

        assert_eq!(preset.routing(input), Some(1));
        assert_eq!(preset.routing(output), Some(1));
        // A block has no routing field of its own.
        assert_eq!(preset.routing(1), None);
        assert!(!preset.set_routing(1, 3));

        let before = preset.routing(input);
        assert!(!preset.set_routing(input, -1));
        assert_eq!(
            preset.routing(input),
            before,
            "negative routes change nothing"
        );

        assert!(preset.set_routing(output, 6));
        assert_eq!(preset.routing(output), Some(6));

        // And the document still re-encodes to the same length, so the section
        // offsets it carries stay valid.
        let before = Preset::parse(FIXTURE).unwrap().encode().len();
        assert_eq!(preset.encode().len(), before);
    }

    #[test]
    fn malformed_endpoint_routes_are_rejected() {
        let document = |route: Value| {
            let mut preset = Preset::parse(FIXTURE).unwrap();
            let input = preset.layout().paths[0].input.unwrap();
            *preset
                .slot_body_mut(input)
                .and_then(|body| body.get_mut(key::INPUT_FROM))
                .unwrap() = route;
            preset.encode()
        };
        assert!(Preset::parse(&document(Value::Int(0))).is_some());
        assert!(Preset::parse(&document(Value::Int(-1))).is_none());
        assert!(Preset::parse(&document(Value::Str("guitar".into()))).is_none());
    }

    /// The section table is not opaque: it can be rebuilt from the document
    /// itself and must come out byte-identical to what the device sent.
    #[test]
    fn the_section_table_can_be_recomputed() {
        let preset = Preset::parse(FIXTURE).unwrap();
        assert_eq!(
            preset.computed_sections().expect("computable"),
            preset.sections,
            "the recomputed section table does not match the device's own"
        );
    }

    #[test]
    fn section_offsets_do_not_wrap_past_u32() {
        assert_eq!(
            section_word(u32::MAX as usize),
            Some(u32::MAX.to_le_bytes())
        );
        #[cfg(target_pointer_width = "64")]
        assert!(section_word(u32::MAX as usize + 1).is_none());
    }

    #[test]
    fn the_section_table_survives_additional_top_level_fields() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let Value::Map(fields) = &mut preset.tone else {
            panic!("the tone is a map");
        };
        let mut key = 10_000;
        while fields.len() < 16 {
            fields.push((crate::msgpack::Key::Int(key), Value::Nil));
            key += 1;
        }
        assert_eq!(Encoder::encode(&preset.tone)[0], 0xde, "map16");

        let sections = preset.computed_sections().expect("still computable");
        let reparsed = Preset::parse(&preset.encode()).expect("re-parses");
        assert_eq!(reparsed.sections, sections);
    }

    #[test]
    fn a_slot_can_be_copied_and_pasted() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let (from, _) = preset.blocks().next().expect("a block to copy");
        let source_model = preset.slots[from].model;

        // An empty slot in the same row.
        let empty = preset
            .slots
            .iter()
            .position(|s| s.kind == Kind::Empty)
            .expect("an empty slot");

        let block = preset.copy_slot(from).expect("copying");
        assert!(
            preset.paste_slot(empty, &block),
            "pasting into an empty slot"
        );
        assert_eq!(
            preset.slots[empty].model, source_model,
            "the pasted slot does not hold the copied model"
        );
        // The original is untouched - this is a copy, not a move.
        assert_eq!(preset.slots[from].model, source_model);
    }

    /// The endpoints are fixtures of the topology. Pasting a block over the
    /// input would produce a document the device has no way to read.
    #[test]
    fn making_room_slides_blocks_along() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let before: Vec<_> = preset.blocks().map(|(p, s)| (p, s.model)).collect();
        let (first, _) = before[0];
        let bounds = preset.lane_bounds(first).expect("the block is in a lane");

        assert!(preset.make_room(first, bounds));
        assert!(
            preset.slots[first].model.is_none(),
            "the requested slot is now free"
        );
        // Everything that was there is still there, one place along.
        let after: Vec<_> = preset.blocks().map(|(_, s)| s.model).collect();
        assert_eq!(
            after,
            before.iter().map(|(_, m)| *m).collect::<Vec<_>>(),
            "sliding lost or reordered a block"
        );
    }

    /// A full lane has nowhere to slide, and must say so rather than pushing a
    /// block off the end.
    #[test]
    fn making_room_in_a_full_lane_fails() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let bounds = preset.lane_bounds(1).unwrap();
        // Fill every free slot in the lane.
        let block = preset.copy_slot(1).unwrap();
        for p in bounds.clone() {
            if preset.slots[p].model.is_none() {
                preset.paste_slot(p, &block);
            }
        }
        assert!(!preset.make_room(1, bounds));
    }

    #[test]
    fn a_slot_cannot_be_pasted_over_an_endpoint() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let (from, _) = preset.blocks().next().unwrap();
        let block = preset.copy_slot(from).unwrap();
        let input = preset.layout().paths[0].input.unwrap();

        assert!(!preset.paste_slot(input, &block));
        assert_eq!(preset.slots[input].kind, Kind::Input, "the input survived");
    }

    #[test]
    fn a_junction_cannot_be_pasted_over_a_block_slot() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let split = preset
            .slots
            .iter()
            .position(|slot| slot.kind == Kind::Split)
            .expect("a split junction");
        let empty = preset
            .slots
            .iter()
            .position(|slot| slot.kind == Kind::Empty)
            .expect("an empty block slot");
        let junction = preset.copy_slot(split).unwrap();
        let before = preset.copy_slot(empty).unwrap();

        assert!(!preset.paste_slot(empty, &junction));
        assert_eq!(preset.copy_slot(empty), Some(before));
    }

    #[test]
    fn a_snapshot_can_be_copied_and_pasted_keeping_its_name() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        if preset.snapshots().len() < 2 {
            return;
        }
        preset.set_snapshot_name(0, "Verse");
        preset.set_snapshot_name(1, "Chorus");

        let verse = preset.copy_snapshot(0).expect("copying");
        assert!(preset.paste_snapshot(1, &verse));

        // The settings came across; the name did not, because the name says
        // where you are in the song rather than what the settings are.
        assert_eq!(preset.snapshots()[1], "Chorus");
        assert_eq!(preset.snapshots()[0], "Verse");
    }

    #[test]
    fn a_snapshot_with_the_wrong_slot_count_cannot_be_pasted() {
        let mut preset = Preset::parse(FIXTURE).unwrap();
        let mut snapshot = preset.copy_snapshot(0).unwrap();
        let Some(Value::Array(slots)) = snapshot.get_mut(key::SNAPSHOT_SLOTS) else {
            panic!("snapshot slots");
        };
        slots.pop();
        let before = preset.copy_snapshot(1).unwrap();

        assert!(!preset.paste_snapshot(1, &snapshot));
        assert_eq!(preset.copy_snapshot(1), Some(before));
    }

    #[test]
    fn rejects_a_blob_that_is_not_a_preset() {
        assert!(Preset::parse(b"\xa3abc").is_none());
    }

    #[test]
    fn rejects_malformed_controller_tables() {
        let document = |assignments: Value| {
            let mut preset = Preset::parse(&sample()).unwrap();
            *preset.tone.get_mut(key::ASSIGNMENTS).unwrap() = assignments;
            preset.encode()
        };
        let entry = |what| {
            crate::msgmap! {
                0 => Value::Int(0),
                key::ASSIGNED_WHAT => what,
            }
        };
        let parameter = crate::msgmap! {
            key::ASSIGNED_KIND => Value::Int(4),
            key::ASSIGNED_MIN => Value::Int(0),
            key::ASSIGNED_MAX => Value::Int(1),
            key::ASSIGNED_ON => Value::Int(0),
            key::ASSIGNED_TARGET => crate::msgmap! {
                key::ASSIGNED_PARAM => Value::Int(1),
            },
        };
        let under_footswitch = |entry| {
            Value::Array(vec![
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::Array(vec![entry]),
            ])
        };
        assert!(Preset::parse(&document(under_footswitch(entry(parameter)))).is_some());

        let missing_target = crate::msgmap! {
            key::ASSIGNED_KIND => Value::Int(4),
            key::ASSIGNED_ON => Value::Int(0),
        };
        assert!(Preset::parse(&document(under_footswitch(entry(missing_target)))).is_none());
        assert!(Preset::parse(&document(Value::Bool(false))).is_none());

        let midi = crate::msgmap! {
            key::ASSIGNED_KIND => Value::Int(128),
            key::ASSIGNED_ON => Value::Int(0),
            key::ASSIGNED_BYPASS => Value::Int(5),
        };
        let mut by_source = (0..9).map(|_| Value::Nil).collect::<Vec<_>>();
        by_source[8] = Value::Array(vec![entry(midi)]);
        assert!(Preset::parse(&document(Value::Array(by_source))).is_none());
    }

    #[test]
    fn rejects_malformed_snapshot_records() {
        let slot_count = Preset::parse(&sample()).unwrap().slots.len();
        let document = |snapshot: Value| {
            let mut preset = Preset::parse(&sample()).unwrap();
            *preset.tone.get_mut(key::SNAPSHOT_SECTION).unwrap() = crate::msgmap! {
                key::SNAPSHOTS => Value::Array(vec![snapshot]),
            };
            preset.encode()
        };
        let states = Value::Array(
            (0..slot_count)
                .map(|_| Value::Array(vec![Value::Int(0), Value::Bool(true)]))
                .collect(),
        );
        let valid = crate::msgmap! {
            key::SNAPSHOT_VALID => Value::Bool(true),
            key::SNAPSHOT_SLOTS => states,
            key::SNAPSHOT_NAME => Value::Str("Verse".into()),
            key::SNAPSHOT_TEMPO => Value::F32(120.0),
            key::SNAPSHOT_NAMED => Value::Bool(true),
        };
        assert!(Preset::parse(&document(valid.clone())).is_some());

        let mut bad_name = valid.clone();
        *bad_name.get_mut(key::SNAPSHOT_NAME).unwrap() = Value::Nil;
        assert!(Preset::parse(&document(bad_name)).is_none());

        let mut bad_tempo = valid.clone();
        *bad_tempo.get_mut(key::SNAPSHOT_TEMPO).unwrap() = Value::F32(f32::NAN);
        assert!(Preset::parse(&document(bad_tempo)).is_none());

        let mut out_of_range_tempo = valid.clone();
        *out_of_range_tempo.get_mut(key::SNAPSHOT_TEMPO).unwrap() = Value::F32(241.0);
        assert!(Preset::parse(&document(out_of_range_tempo)).is_none());

        let mut short_states = valid;
        *short_states.get_mut(key::SNAPSHOT_SLOTS).unwrap() = Value::Array(Vec::new());
        assert!(Preset::parse(&document(short_states)).is_none());

        let mut trailing_state = crate::msgmap! {
            key::SNAPSHOT_VALID => Value::Bool(true),
            key::SNAPSHOT_SLOTS => Value::Array(
                (0..slot_count)
                    .map(|_| Value::Array(vec![Value::Int(0), Value::Bool(true)]))
                    .collect(),
            ),
            key::SNAPSHOT_NAME => Value::Str("Verse".into()),
            key::SNAPSHOT_TEMPO => Value::F32(120.0),
            key::SNAPSHOT_NAMED => Value::Bool(true),
        };
        let Value::Array(states) = trailing_state.get_mut(key::SNAPSHOT_SLOTS).unwrap() else {
            panic!("snapshot states");
        };
        let Value::Array(first) = &mut states[0] else {
            panic!("snapshot state pair");
        };
        first.push(Value::Nil);
        assert!(Preset::parse(&document(trailing_state)).is_none());
    }

    #[test]
    fn rejects_malformed_preset_envelopes() {
        let preset = Preset::parse(FIXTURE).unwrap();

        let envelope = |table: Value, tone: Value| {
            let mut bytes = Encoder::encode(&Value::Str(Preset::MAGIC.to_owned()));
            bytes.extend(Encoder::encode(&table));
            bytes.extend(Encoder::encode(&tone));
            bytes
        };
        assert!(Preset::parse(&envelope(
            Value::Str("not a section table".to_owned()),
            preset.tone.clone(),
        ))
        .is_none());
        assert!(Preset::parse(&envelope(
            Value::Bin(preset.sections.clone(), preset.sections_width),
            Value::Nil,
        ))
        .is_none());

        let mut corrupt_sections = preset.sections.clone();
        corrupt_sections[0] ^= 1;
        assert!(Preset::parse(&envelope(
            Value::Bin(corrupt_sections, preset.sections_width),
            preset.tone.clone(),
        ))
        .is_none());

        let mut no_slots = Preset::parse(FIXTURE).unwrap();
        *no_slots.tone.get_mut(key::PATH).unwrap() = Value::Nil;
        assert!(Preset::parse(&no_slots.encode()).is_none());

        let mut malformed_slot = Preset::parse(FIXTURE).unwrap();
        *malformed_slot
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .unwrap() = Value::Array(vec![Value::Nil]);
        assert!(Preset::parse(&malformed_slot.encode()).is_none());

        let mut malformed_values = Preset::parse(&sample()).unwrap();
        *malformed_values
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .and_then(|slots| match slots {
                Value::Array(slots) => slots.first_mut(),
                _ => None,
            })
            .and_then(|slot| slot.at_mut(&[key::BODY, key::VALUES, key::ARRAY_VALUES]))
            .unwrap() = Value::Array(vec![Value::Str("not a parameter".into())]);
        assert!(Preset::parse(&malformed_values.encode()).is_none());

        let mut malformed_empty_body = Preset::parse(FIXTURE).unwrap();
        let empty = malformed_empty_body
            .slots
            .iter()
            .position(|slot| slot.kind == Kind::Empty)
            .unwrap();
        let Value::Array(slots) = malformed_empty_body
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .unwrap()
        else {
            panic!("slot array");
        };
        *slots[empty].get_mut(key::BODY).unwrap() = Value::Bool(false);
        assert!(Preset::parse(&malformed_empty_body.encode()).is_none());

        let mut stale_value_count = Preset::parse(&sample()).unwrap();
        *stale_value_count
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .and_then(|slots| match slots {
                Value::Array(slots) => slots.first_mut(),
                _ => None,
            })
            .and_then(|slot| slot.at_mut(&[key::BODY, key::VALUES, 2]))
            .unwrap() = Value::Int(99);
        assert!(Preset::parse(&stale_value_count.encode()).is_none());

        let mut missing_model = Preset::parse(&sample()).unwrap();
        *missing_model
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .and_then(|slots| match slots {
                Value::Array(slots) => slots.first_mut(),
                _ => None,
            })
            .and_then(|slot| slot.at_mut(&[key::BODY, key::MODEL_REF, key::MODEL]))
            .unwrap() = Value::Int(-1);
        assert!(Preset::parse(&missing_model.encode()).is_none());

        let mut missing_block_body = Preset::parse(&sample()).unwrap();
        let Value::Map(fields) = missing_block_body
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .and_then(|slots| match slots {
                Value::Array(slots) => slots.first_mut(),
                _ => None,
            })
            .unwrap()
        else {
            panic!("slot map");
        };
        fields
            .retain(|(field, _)| !matches!(field, crate::msgpack::Key::Int(k) if *k == key::BODY));
        assert!(Preset::parse(&missing_block_body.encode()).is_none());

        let mut malformed_enabled = Preset::parse(&sample()).unwrap();
        *malformed_enabled
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .and_then(|slots| match slots {
                Value::Array(slots) => slots.first_mut(),
                _ => None,
            })
            .and_then(|slot| slot.at_mut(&[key::BODY, key::ENABLED]))
            .unwrap() = Value::Str("yes".into());
        assert!(Preset::parse(&malformed_enabled.encode()).is_none());

        let mut malformed_pair = Preset::parse(&sample()).unwrap();
        *malformed_pair
            .tone
            .at_mut(&[key::PATH, key::SLOTS])
            .and_then(|slots| match slots {
                Value::Array(slots) => slots.first_mut(),
                _ => None,
            })
            .and_then(|slot| slot.at_mut(&[key::BODY, key::MODEL_REF, key::PAIRED_MODEL]))
            .unwrap() = Value::Int(i64::MAX);
        assert!(Preset::parse(&malformed_pair.encode()).is_none());

        let mut trailing = FIXTURE.to_vec();
        trailing.push(0xc0);
        assert!(Preset::parse(&trailing).is_none());
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    /// Snapshots read back with their names, tempos and which blocks were on.
    #[test]
    fn snapshots_carry_more_than_their_names() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut checked = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("hxpreset") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let Some(preset) = Preset::parse(&bytes) else {
                continue;
            };
            let details = preset.snapshot_details();
            if details.is_empty() {
                continue;
            }
            // The names match what the older accessor reports, and each
            // snapshot knows the state of every slot rather than none.
            assert_eq!(
                details.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                preset.snapshots(),
                "{} names disagree",
                path.display()
            );
            assert_eq!(
                details[0].enabled.len(),
                preset.slots.len(),
                "{} should know every slot's state",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 0, "no fixtures with snapshots");
    }
}
