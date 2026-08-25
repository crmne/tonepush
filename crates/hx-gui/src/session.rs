//! The device, on its own thread.
//!
//! Talking to the hardware blocks - a preset read is a dozen round trips - so
//! the session lives on a worker and the UI speaks to it through channels. The
//! worker owns it outright: the protocol is a strictly ordered stream, and two
//! callers would interleave transfers and desynchronise it.

use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, OnceLock,
};
use std::time::{Duration, Instant};

/// One slot on its way to the pedal: where it goes, and the preset that goes
/// there - its name and its document - or nothing, to empty the slot.
pub type SlotWrite = (i64, Option<(String, Vec<u8>)>);

/// What the UI asks for.
pub enum Cmd {
    Connect,
    Disconnect,
    /// Read the whole pedal into a bundle directory.
    BackUp(std::path::PathBuf),
    /// Write a bundle back onto the pedal.
    RestoreAll(std::path::PathBuf),
    Rename {
        index: i64,
        name: String,
    },
    MoveBlock {
        from: usize,
        to: usize,
    },
    /// Move a block into the gap just before `before`, shifting the blocks
    /// between to close ranks - what dropping it there means.
    MoveBlockBefore {
        from: usize,
        before: usize,
    },
    ListIrs,
    SetTempo(f32),
    RenameSnapshot {
        index: usize,
        name: String,
    },
    ListSetlists,
    /// Put a block's bypass under MIDI, or take it back off. No CC number:
    /// the pedal picks one, and no captured message sets it.
    AssignMidi {
        block: i64,
        on: bool,
        /// Which CC drives it. Sent on every change, because the number rides
        /// the assignment itself for a bypass - there is no separate message
        /// to change it with.
        cc: i64,
    },
    /// Which CC drives an assigned *parameter*, which is its own opcode rather
    /// than part of the assignment.
    SetAssignCc {
        block: i64,
        param: i64,
        cc: i64,
    },
    /// Put a block's bypass under a footswitch, or take it off one.
    AssignBypassFootswitch {
        block: i64,
        switch: u8,
        on: bool,
    },
    /// Put a parameter under a controller, or `None` to take it off one.
    AssignParameter {
        block: i64,
        param: i64,
        source: Option<hx_proto::rpc::Source>,
    },
    /// Move one end of a controller's travel, normalised to 0.0-1.0.
    SetAssignRange {
        block: i64,
        param: i64,
        value: f32,
        high_end: bool,
    },
    /// What every footswitch is set to. Cheap: one round trip per switch, and
    /// a pedal has a handful.
    ReadSwitches,
    /// Change the footswitch itself rather than what it carries.
    EditSwitch {
        /// One-based, the way it is printed on the pedal.
        switch: u8,
        edit: SwitchEdit,
    },
    LoadIr {
        slot: i64,
        file: std::path::PathBuf,
    },
    ClearIr(i64),
    /// Read the device's favourite blocks.
    ListFavourites,
    /// Keep the block at this position as a favourite.
    SaveFavourite {
        block: i64,
        index: i64,
        name: String,
    },
    /// Forget a favourite.
    ClearFavourite(i64),
    /// Read an impulse response off the device and write it out as a WAV.
    SaveIr {
        slot: i64,
        file: std::path::PathBuf,
    },
    /// Rename an impulse response slot, leaving its samples alone.
    RenameIr {
        slot: i64,
        name: String,
    },
    SelectPreset(i64),
    SelectSetlist(i64),
    /// Load a preset document into a chosen preset's edit buffer: put the
    /// device there first, then write the bytes. Save is the user's call.
    LoadDocument {
        dest: i64,
        bytes: Vec<u8>,
    },
    /// Load a symbolic tone into a chosen preset: clear the chain, then build
    /// it back block by block. Clearing first is what makes room - probed on
    /// hardware; a model set into a cleared slot is an ordinary edit.
    LoadSteps {
        dest: i64,
        name: String,
        blocks: Vec<ApplyBlock>,
    },
    /// Temporarily replace the loaded edit buffer with a cloud Tone. The
    /// worker keeps the exact document and history it displaced until the
    /// audition is either ended or deliberately kept.
    AuditionDocument {
        key: i64,
        name: String,
        bytes: Vec<u8>,
    },
    /// The symbolic `.hlx` counterpart to [`Self::AuditionDocument`].
    AuditionSteps {
        key: i64,
        name: String,
        blocks: Vec<ApplyBlock>,
    },
    /// Put back the edit buffer that was playing before the audition began.
    EndAudition,
    /// Leave the audition in the edit buffer as an ordinary unsaved edit.
    /// Save remains the person's explicit choice.
    KeepAudition,
    SelectBlock(i64),
    SetParam {
        block: i64,
        index: i64,
        value: f32,
        switch: bool,
    },
    SetEnabled {
        block: i64,
        enabled: bool,
    },
    SetModel {
        block: i64,
        model: u32,
        /// The cab that rides along, for an Amp+Cab. `None` for everything else.
        paired: Option<u32>,
    },
    SelectSnapshot(i64),
    ClearBlock(i64),
    /// Point an input or output somewhere else - opcode 42, the operation
    /// HX Edit's own routing clicks send.
    SetRouting {
        block: i64,
        to: i64,
    },
    /// Commit the edit buffer to the loaded preset.
    SavePreset,
    /// Copy one block over another slot.
    CopyBlock {
        from: usize,
        to: usize,
    },
    /// Copy a snapshot's settings over another, keeping its name.
    CopySnapshot {
        from: usize,
        to: usize,
    },
    /// Re-attach a split or join: the fork or merge moves to sit just before
    /// `before` in the main line.
    MoveJunction {
        junction: usize,
        before: usize,
    },
    /// Put the preset back as it was before the last document edit.
    Undo,
    /// Add a block at a position, sliding whatever is there along.
    InsertBlock {
        at: usize,
        model: u32,
        /// The cab that rides along, for an Amp+Cab. `None` for everything else.
        paired: Option<u32>,
    },
    /// Put back what the last undo took away.
    Redo,
    /// Flip one of the device's global settings.
    SetSetting {
        id: i64,
        on: bool,
    },
    /// Read every global setting this program knows the name of.
    ReadSettings,
    /// Write one global setting, in whatever shape the device holds it.
    WriteSetting {
        id: i64,
        value: f32,
    },
    /// Read the loaded preset and hand back its bytes, for the clipboard or a
    /// file. The document is copied verbatim rather than rebuilt from what the
    /// UI shows, because a preset carries more than the UI models.
    CopyPreset,
    /// Write a whole preset document over the loaded one.
    PastePreset(Vec<u8>),
    /// Empty a slot back to the factory blank, the way HX Edit's restore blanks
    /// the slots a backup holds nothing for. This one writes flash.
    ClearPreset(i64),
    /// Read every preset in the setlist and hand the documents back, so the
    /// library can keep the whole pedal as a setlist.
    CaptureSetlist,
    /// Write a setlist back onto the pedal: for each slot, the name and the
    /// document, or nothing to empty it. Every one of these is a flash write.
    PushSetlist(Vec<SlotWrite>),
}

/// One change to a footswitch's own settings.
///
/// The three things a footswitch is, apart from what it drives: what it is
/// called, what colour it lights, and whether it holds or toggles.
#[derive(Clone)]
pub enum SwitchEdit {
    /// A name to write under it, or nothing to go back to naming what it
    /// carries.
    Label(Option<String>),
    /// A colour by its place in HX Edit's list, or nothing for Auto Color.
    Colour(Option<i64>),
    /// Momentary holds while your foot is down; latching toggles.
    Momentary(bool),
}

/// What the worker reports.
pub enum Evt {
    Connected {
        device: String,
        presets: u16,
    },
    Disconnected,
    Presets(Vec<String>),
    Loaded {
        index: i64,
        name: String,
        firmware: String,
        tempo: Option<f32>,
        snapshots: Vec<String>,
        chain: Vec<Block>,
        layout: hx_proto::preset::Layout,
        /// Everything a controller drives in this preset, every block at once.
        /// It comes out of the document the reload already read, so it costs
        /// nothing on the wire and is right about the travel, which opcode 36
        /// is not.
        assignments: Vec<hx_proto::preset::Assignment>,
        /// Whether the edit buffer differs from the stored preset. The worker
        /// owns this: a reload follows most edits, and a reload that reset the
        /// flag made Save go grey with changes still unsaved.
        dirty: bool,
    },
    /// The edit buffer has been committed to the preset.
    Saved,
    /// Which cloud Tone is temporarily in the edit buffer, or `None` once the
    /// original has been restored or the audition has been kept.
    Auditioning(Option<i64>),
    /// What every footswitch is set to, in order.
    Switches(Vec<hx_usb::Switch>),
    /// A backup or restore is running, and how far along it is (0.0 to 1.0).
    Working {
        what: String,
        progress: f32,
    },
    /// A backup finished, with where it went and what it holds.
    BackedUp {
        dir: std::path::PathBuf,
        presets: usize,
        settings: usize,
        irs: usize,
    },
    /// Whether the worker is in the middle of a device conversation. Edits
    /// take real round trips - a document write near a second - and a window
    /// that does nothing for a second looks broken.
    Busy(bool),
    /// How many steps can be undone and redone.
    History {
        undo: usize,
        redo: usize,
    },
    /// Global settings worth showing, read when a session opens.
    Settings {
        global_eq: bool,
    },
    /// Every named global setting and its current value, as a number: a switch
    /// reads 0 or 1, a choice its index, a number itself.
    SettingValues(Vec<(i64, f32)>),
    /// The loaded preset's bytes, in answer to `Cmd::CopyPreset`.
    Copied {
        name: String,
        blob: Vec<u8>,
    },
    Irs(Vec<(i64, String)>),
    /// The device's favourite blocks, as (index, name).
    Favourites(Vec<(i64, String)>),
    Setlists(Vec<String>),
    /// Every preset in the setlist, as (name, document bytes). An empty slot
    /// comes back as `None` so the setlist can record that it is empty rather
    /// than silently shortening.
    CapturedSetlist(Vec<(String, Option<Vec<u8>>)>),
    Activity(String),
    Failed(String),
}

/// One slot as the UI needs it: enough to draw without holding the preset.
#[derive(Clone)]
pub struct Block {
    pub position: i64,
    /// Where an input or output is routed. `None` on everything else.
    pub routing: Option<i64>,
    /// What the slot is: an effect, or the path's input, output, split or join.
    pub kind: hx_proto::preset::Kind,
    pub model: u32,
    pub enabled: bool,
    pub values: Vec<f32>,
    /// The cab riding along with an amp, if any.
    pub paired: Option<u32>,
    pub paired_values: Vec<f32>,
}

/// One block of a symbolic tone, resolved and ready to apply: the model
/// number, whether it is engaged, and its parameter values by index.
#[derive(Debug, Clone)]
pub struct ApplyBlock {
    pub model: u32,
    pub enabled: bool,
    /// `(parameter index, native value, is a switch)`.
    pub params: Vec<(i64, f32, bool)>,
}

/// Put a model in a slot, with or without a cab riding along.
///
/// One place so inserting and swapping cannot drift apart on which of the two
/// device calls they make.
fn place(
    device: &mut hx_usb::Session,
    block: i64,
    model: u32,
    paired: Option<u32>,
) -> hx_usb::Result<()> {
    match paired {
        Some(cab) => device.set_model_pair(block, model, cab),
        None => device.set_model(block, model),
    }
}

/// The drawable blocks of a preset document: everything the signal passes
/// through, in slot order. Shared by the worker's own view and the preview of
/// a preset file, so a file is drawn by the same rules as the loaded chain.
pub fn chain_of(preset: &hx_proto::Preset) -> Vec<Block> {
    preset
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind != hx_proto::preset::Kind::Empty)
        .map(|(position, slot)| Block {
            position: position as i64,
            routing: preset.routing(position),
            kind: slot.kind,
            model: slot.model.unwrap_or_default(),
            enabled: slot.enabled,
            values: slot.values.clone(),
            paired: slot.paired,
            paired_values: slot.paired_values.clone(),
        })
        .collect()
}

/// Connect the device worker's event channel to eframe's event loop.
///
/// The worker exists before eframe creates its context, so binding happens in
/// the app-creation callback. Once bound, a device event wakes a sleeping UI
/// immediately instead of waiting for a polling repaint.
#[derive(Clone, Default)]
pub struct RepaintSignal(Arc<OnceLock<egui::Context>>);

impl RepaintSignal {
    pub fn bind(&self, ctx: &egui::Context) {
        let _ = self.0.set(ctx.clone());
    }

    fn request(&self) {
        if let Some(ctx) = self.0.get() {
            ctx.request_repaint();
        }
    }
}

#[derive(Clone)]
struct Events {
    tx: Sender<Evt>,
    repaint: RepaintSignal,
}

impl Events {
    fn send(&self, evt: Evt) {
        if self.tx.send(evt).is_ok() {
            self.repaint.request();
        }
    }
}

pub fn spawn() -> (Sender<Cmd>, Receiver<Evt>) {
    let (commands, events, _) = spawn_repainting();
    (commands, events)
}

pub fn spawn_repainting() -> (Sender<Cmd>, Receiver<Evt>, RepaintSignal) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (evt_tx, evt_rx) = mpsc::channel();
    let repaint = RepaintSignal::default();
    let worker_repaint = repaint.clone();
    std::thread::spawn(move || {
        Worker {
            cmds: cmd_rx,
            events: Events {
                tx: evt_tx,
                repaint: worker_repaint,
            },
            device: None,
            setlist: 0,
            history: Vec::new(),
            future: Vec::new(),
            snapshot_taken: false,
            dirty: false,
            shown: (-1, String::new()),
            stumbles: 0,
            audition: None,
        }
        .run()
    });
    (cmd_tx, evt_rx, repaint)
}

struct Worker {
    cmds: Receiver<Cmd>,
    events: Events,
    device: Option<hx_usb::Session>,
    /// Which setlist preset selections apply to.
    setlist: i64,
    /// Preset documents as they were before each document-level edit, newest
    /// last. Bounded - an undo history, not an archive.
    history: Vec<Vec<u8>>,
    /// States undone and therefore redoable, cleared by any fresh edit.
    future: Vec<Vec<u8>>,
    /// Whether the current burst of edits has already been snapshotted.
    ///
    /// Turning a knob is one edit per pixel of drag; a history entry each
    /// would be useless. One entry is taken for the first change after a load
    /// or a save, so undo steps back to the last known-good state - which is
    /// what "undo" means to someone who has just been turning knobs.
    snapshot_taken: bool,
    /// Whether the edit buffer differs from the stored preset. Kept here
    /// rather than in the UI because only the worker knows which reloads are
    /// fresh presets and which are edits taking effect.
    dirty: bool,
    /// The loaded preset's slot and name, as last read from the device - so a
    /// view built from a document we hold does not need a round trip to say
    /// which preset it is.
    shown: (i64, String),
    /// Keepalives missed in a row. The device goes quiet while it commits a
    /// document write, and dropping the session on the first missed beat is
    /// how a drag came to end in a disconnect.
    stumbles: u32,
    /// Everything an audition temporarily displaces. Histories are moved here
    /// rather than copied: while a cloud Tone is sounding it is not an edit
    /// somebody can accidentally fold into their existing undo stack.
    audition: Option<Audition>,
}

struct Audition {
    key: i64,
    original: Vec<u8>,
    dirty: bool,
    history: Vec<Vec<u8>>,
    future: Vec<Vec<u8>>,
    snapshot_taken: bool,
}

impl Worker {
    fn run(mut self) {
        let mut last_poll = Instant::now();
        loop {
            match self.cmds.recv_timeout(Duration::from_millis(120)) {
                Ok(mut cmd) => {
                    // Someone riding the preset list queues a select per
                    // click, and only the last one is where they meant to
                    // land. Collapsing the run spares the device a switch
                    // per click; a dozen switches stacked up is precisely
                    // the load that wedges it.
                    let mut follow_up = None;
                    if matches!(cmd, Cmd::SelectPreset(_)) {
                        while let Ok(next) = self.cmds.try_recv() {
                            if matches!(next, Cmd::SelectPreset(_)) {
                                cmd = next;
                            } else {
                                follow_up = Some(next);
                                break;
                            }
                        }
                    }
                    // Bracket the work so the UI can say the device is being
                    // spoken to; it only shows the state when it lasts.
                    self.send(Evt::Busy(true));
                    self.handle(cmd);
                    if let Some(next) = follow_up {
                        self.handle(next);
                    }
                    self.send(Evt::Busy(false));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }

            // The device drops idle sessions, and it pushes front-panel
            // activity between our requests, so keep it fed and drain what it
            // sent.
            if last_poll.elapsed() > Duration::from_millis(700) {
                last_poll = Instant::now();
                self.poll();
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        // Any ordinary device action means the person has moved on from the
        // cloud browser. Restore first, then perform the requested action on
        // the real preset. ReadSwitches is the one automatic follow-up to a
        // presented document and must not end an audition by itself.
        if self.audition.is_some()
            && !matches!(
                &cmd,
                Cmd::AuditionDocument { .. }
                    | Cmd::AuditionSteps { .. }
                    | Cmd::EndAudition
                    | Cmd::KeepAudition
                    | Cmd::ReadSwitches
            )
        {
            self.end_audition();
        }
        match cmd {
            Cmd::Connect => self.connect(),
            Cmd::Disconnect => {
                self.device = None;
                self.send(Evt::Disconnected);
            }
            Cmd::SelectPreset(index) => {
                let setlist = self.setlist;
                if self.run_on_device(|d| d.select_preset(setlist, index)) {
                    // The history belongs to the preset it was recorded on.
                    // Kept across a switch, undo would write the previous
                    // preset's document over the new one.
                    self.forget_history();
                    self.dirty = false;
                    self.reload();
                }
            }
            Cmd::SelectSetlist(index) => {
                self.setlist = index;
                if let Some(names) = self.try_on_device(|d| d.presets(index)) {
                    self.send(Evt::Presets(names));
                }
            }
            Cmd::LoadDocument { dest, bytes } => {
                if self.go_to(dest) {
                    self.paste(&bytes);
                }
            }
            Cmd::LoadSteps { dest, name, blocks } => self.load_steps(dest, &name, &blocks),
            Cmd::AuditionDocument { key, name, bytes } => {
                self.audition_document(key, &name, &bytes)
            }
            Cmd::AuditionSteps { key, name, blocks } => self.audition_steps(key, &name, &blocks),
            Cmd::EndAudition => self.end_audition(),
            Cmd::KeepAudition => self.keep_audition(),
            Cmd::SelectBlock(block) => {
                self.run_on_device(|d| d.select_block(block));
            }
            Cmd::SetParam {
                block,
                index,
                value,
                switch,
            } => {
                use hx_proto::msgpack::Value;
                self.snapshot();
                let wire = if switch {
                    Value::Bool(value >= 0.5)
                } else {
                    Value::F32(value)
                };
                if self.run_on_device(|d| d.set_param(block, index, wire)) {
                    self.dirty = true;
                }
            }
            Cmd::SetEnabled { block, enabled } => {
                self.snapshot();
                if self.run_on_device(|d| d.set_enabled(block, enabled)) {
                    self.dirty = true;
                }
            }
            Cmd::SetRouting { block, to } => {
                self.snapshot();
                if self.run_on_device(|d| d.set_routing(block, to)) {
                    self.dirty = true;
                    self.reload();
                }
            }
            Cmd::CopyBlock { from, to } => {
                self.edit_document(|p| {
                    let block = p.copy_slot(from).ok_or("no such block")?;
                    p.paste_slot(to, &block)
                        .then_some(())
                        .ok_or("that slot cannot hold a block")
                });
            }
            Cmd::CopySnapshot { from, to } => {
                self.edit_document(|p| {
                    let snap = p.copy_snapshot(from).ok_or("no such snapshot")?;
                    p.paste_snapshot(to, &snap)
                        .then_some(())
                        .ok_or("could not write that snapshot")
                });
            }
            Cmd::Undo => self.step_history(true),
            Cmd::Redo => self.step_history(false),
            Cmd::InsertBlock { at, model, paired } => {
                use hx_proto::preset::Kind;

                // Two device operations, and no more: adjust the document if
                // the slot needs freeing or a branch needs attaching, then set
                // the model. Reloading in between lands a third round trip
                // while the device is still committing the write, which is
                // enough to jam it.
                let Some(mut preset) = self.read_settled() else {
                    return;
                };
                let original = preset.encode();

                // The gap before an endpoint or a junction means "at the end
                // of the lane that finishes here" - you cannot put a pedal on
                // the output itself, which is what asking for that slot did.
                let holds_blocks = preset
                    .slots
                    .get(at)
                    .is_some_and(|s| matches!(s.kind, Kind::Block | Kind::Empty));
                let Some(bounds) =
                    preset.lane_bounds(if holds_blocks { at } else { at.max(1) - 1 })
                else {
                    return self.send(Evt::Failed("nothing can go there".into()));
                };

                let free_in_lane = |p: &hx_proto::Preset| {
                    bounds
                        .clone()
                        .find(|i| p.slots.get(*i).is_some_and(|s| s.model.is_none()))
                };
                let target = if holds_blocks {
                    at
                } else {
                    match free_in_lane(&preset) {
                        Some(slot) => slot,
                        None => {
                            return self.send(Evt::Failed(
                                "this row is full - remove a block first".into(),
                            ))
                        }
                    }
                };

                // The first block on an empty branch should parallel the whole
                // line: fork just after the input, merge just before the
                // output. The device initialises the attach points itself when
                // the model lands - to zero, which parallels nothing - so ours
                // have to be written *after* the model, not before. Verified
                // against the hardware; later inserts leave them alone.
                let layout = preset.layout();
                let claim = layout
                    .paths
                    .iter()
                    .find(|p| {
                        p.split
                            .zip(p.join)
                            .is_some_and(|(s, j)| (s + 1..j).contains(&target))
                    })
                    .filter(|p| {
                        (p.split.unwrap() + 1..p.join.unwrap())
                            .all(|s| preset.slots.get(s).is_none_or(|s| s.model.is_none()))
                    })
                    .and_then(|p| Some((p.split?, p.join?, p.input? + 1, p.output?)));

                if preset.slots.get(target).is_some_and(|s| s.model.is_some()) {
                    if !preset.make_room(target, bounds) {
                        return self.send(Evt::Failed(
                            "this row is full - remove a block first".into(),
                        ));
                    }
                    if !self.run_on_device(|d| d.write_preset(&preset)) {
                        return;
                    }
                }

                if self.run_on_device(|d| place(d, target as i64, model, paired)) {
                    if let Some((split, join, fork_at, merge_at)) = claim {
                        self.run_on_device(|d| {
                            let mut p = d.read_preset()?;
                            if p.set_attach(split, fork_at) && p.set_attach(join, merge_at) {
                                d.write_preset(&p)?;
                            }
                            Ok(())
                        });
                    }
                    self.dirty = true;
                    self.history.push(original);
                    if self.history.len() > 32 {
                        self.history.remove(0);
                    }
                    self.future.clear();
                    self.report_history();
                    self.reload();
                }
            }
            Cmd::AssignBypassFootswitch { block, switch, on } => {
                self.snapshot();
                let ok = if on {
                    self.run_on_device(|d| d.assign_bypass_footswitch(block, switch))
                } else {
                    self.run_on_device(|d| d.unassign_bypass_footswitch(block, switch))
                };
                if ok {
                    self.dirty = true;
                    let verb = if on { "assigned to" } else { "taken off" };
                    self.send(Evt::Activity(format!("bypass {verb} footswitch {switch}")));
                    // Say so by showing it. The editor draws the switches from
                    // what the pedal reports rather than from what it just
                    // asked for, so a write that did not take does not leave a
                    // button looking as though it did.
                    if let Some(switches) = self.try_on_device(|d| d.switches()) {
                        self.send(Evt::Switches(switches));
                    }
                    // And by re-reading the document, which is where the rest
                    // of the editor gets its assignments from now.
                    self.reload();
                }
            }
            Cmd::AssignParameter {
                block,
                param,
                source,
            } => {
                self.snapshot();
                if self.run_on_device(|d| d.assign_parameter(block, param, source)) {
                    self.dirty = true;
                    self.send(Evt::Activity(match source {
                        Some(source) => format!("assigned to {}", source.label()),
                        None => "assignment removed".to_owned(),
                    }));
                    // A footswitch assignment changes what the switch carries,
                    // and the document says the rest.
                    if let Some(switches) = self.try_on_device(|d| d.switches()) {
                        self.send(Evt::Switches(switches));
                    }
                    self.reload();
                }
            }
            Cmd::SetAssignRange {
                block,
                param,
                value,
                high_end,
            } => {
                // Dragging an end streams a write per intermediate value, the
                // way the device's own editor does; no undo step per pixel.
                if self.run_on_device(|d| d.set_assign_range(block, param, value, high_end)) {
                    self.dirty = true;
                }
            }
            Cmd::ReadSwitches => {
                if let Some(switches) = self.try_on_device(|d| d.switches()) {
                    self.send(Evt::Switches(switches));
                }
            }
            Cmd::EditSwitch { switch, edit } => {
                self.snapshot();
                let ok = match &edit {
                    SwitchEdit::Label(label) => {
                        self.run_on_device(|d| d.set_switch_label(switch, label.as_deref()))
                    }
                    SwitchEdit::Colour(colour) => {
                        self.run_on_device(|d| d.set_switch_colour(switch, *colour))
                    }
                    SwitchEdit::Momentary(momentary) => {
                        self.run_on_device(|d| d.set_switch_momentary(switch, *momentary))
                    }
                };
                if ok {
                    self.dirty = true;
                    // Read it back rather than assume: what the pedal says the
                    // switch is now is the only thing worth drawing.
                    if let Some(switches) = self.try_on_device(|d| d.switches()) {
                        self.send(Evt::Switches(switches));
                    }
                }
            }
            Cmd::SetSetting { id, on } => {
                if self.run_on_device(|d| d.set_object(id, hx_proto::msgpack::Value::Bool(on))) {
                    self.send(Evt::Activity(format!("setting {id} is now {on}")));
                }
            }
            Cmd::ReadSettings => {
                let mut values = Vec::new();
                for setting in hx_proto::settings::SETTINGS {
                    if let Some(v) = self.try_on_device(|d| d.object(setting.id)) {
                        if let Some(number) = as_number(&v) {
                            values.push((setting.id, number));
                        }
                    }
                }
                self.send(Evt::SettingValues(values));
            }
            Cmd::WriteSetting { id, value } => {
                // The device refuses a value of the wrong type, so it goes back
                // shaped like whatever it currently holds.
                let Some(current) = self.try_on_device(|d| d.object(id)) else {
                    return;
                };
                let shaped = match current {
                    hx_proto::msgpack::Value::Bool(_) => {
                        hx_proto::msgpack::Value::Bool(value >= 0.5)
                    }
                    hx_proto::msgpack::Value::F32(_) => hx_proto::msgpack::Value::F32(value),
                    hx_proto::msgpack::Value::F64(_) => hx_proto::msgpack::Value::F64(value as f64),
                    _ => hx_proto::msgpack::Value::Int(value.round() as i64),
                };
                if self.run_on_device(|d| d.set_object(id, shaped)) {
                    let name = hx_proto::settings::setting(id)
                        .map(|s| s.name)
                        .unwrap_or("setting");
                    self.send(Evt::Activity(format!("{name} is now {value}")));
                }
            }
            Cmd::BackUp(dir) => self.back_up(&dir),
            Cmd::RestoreAll(dir) => self.restore_all(&dir),
            Cmd::SavePreset => {
                let Some((setlist, index, name)) =
                    self.device.as_mut().and_then(|d| d.preset_info().ok())
                else {
                    return self.send(Evt::Failed("no preset loaded".into()));
                };
                if self.run_on_device(|d| d.save_preset(setlist, index, &name)) {
                    self.dirty = false;
                    self.send(Evt::Activity(format!("saved {name}")));
                    // The saved state is the new baseline to undo back to.
                    self.snapshot_taken = false;
                    // The automatic backup follows the save, so the copy on
                    // disk is never older than the last thing you did. One
                    // preset is milliseconds, which is why this can be silent.
                    // It goes before the news of the save rather than after,
                    // because the editor reads that bundle to know what the
                    // pedal is holding, and a stale answer would show up as a
                    // dot saying the opposite of the truth.
                    self.back_up_one(index);
                    self.send(Evt::Saved);
                }
            }
            Cmd::CopyPreset => {
                if let Some(blob) = self.preset_bytes() {
                    let name = self.preset_name();
                    self.send(Evt::Copied { name, blob });
                }
            }
            Cmd::PastePreset(blob) => self.paste(&blob),
            Cmd::CaptureSetlist => self.capture_setlist(),
            Cmd::PushSetlist(slots) => self.push_setlist(slots),
            Cmd::ClearPreset(index) => {
                let setlist = self.setlist;
                if self.run_on_device(|d| d.clear_preset_at(setlist, index)) {
                    // Same care as a rename: `clear_preset_at` paces its own
                    // flash commit, so the list read lands on a settled device.
                    // Unlike a rename this does change the document, so the
                    // loaded preset is read back too - but only if it is the one
                    // that was emptied.
                    if let Some(names) = self.try_on_device(|d| d.presets(setlist)) {
                        self.send(Evt::Presets(names));
                    }
                    if self.shown.0 == index {
                        self.reload();
                    }
                    self.send(Evt::Activity(format!(
                        "emptied {}",
                        hx_proto::rpc::slot_label(index)
                    )));
                }
            }
            Cmd::ClearBlock(block) => {
                // Removing a pedal deserves its own undo step, not a shared
                // one with whatever knob was last turned.
                let recorded = self.record_history();
                if self.run_on_device(|d| d.clear_block(block)) {
                    self.dirty = true;
                    self.reload();
                } else if recorded {
                    self.history.pop();
                    self.report_history();
                }
            }
            Cmd::SelectSnapshot(index) => {
                if self.run_on_device(|d| d.select_snapshot(index)) {
                    self.reload();
                }
            }
            Cmd::Rename { index, name } => {
                let setlist = self.setlist;
                if self.run_on_device(|d| d.rename_preset(setlist, index, &name)) {
                    // rename_preset paces the flash commit itself, so this list
                    // read lands on a settled device. Crucially there is no
                    // reload: a rename changes only a slot's label, never the
                    // loaded document, and reloading here raced the commit - a
                    // burst of renames stacking read-backs onto in-flight writes
                    // is exactly what once wedged the pedal into a factory reset.
                    if let Some(names) = self.try_on_device(|d| d.presets(setlist)) {
                        self.send(Evt::Presets(names));
                    }
                    if self.shown.0 == index {
                        self.shown.1 = name.clone();
                    }
                }
            }
            Cmd::MoveBlock { from, to } => {
                self.edit_document(|p| {
                    p.move_slot(from, to)
                        .then_some(())
                        .ok_or("that block cannot move there")
                });
            }
            Cmd::MoveBlockBefore { from, before } => {
                self.edit_document(|p| {
                    // A block dropped onto an empty branch claims the whole
                    // line, the same as adding one there - and because this is
                    // a document move rather than a model change, the claim
                    // rides in the same write with nothing to reset it.
                    let layout = p.layout();
                    if let Some(path) = layout.paths.iter().find(|path| {
                        path.split
                            .zip(path.join)
                            .is_some_and(|(s, j)| (s + 1..j).contains(&before))
                    }) {
                        let (split, join) = (path.split.unwrap(), path.join.unwrap());
                        let vacant = (split + 1..join)
                            .all(|s| p.slots.get(s).is_none_or(|x| x.model.is_none()));
                        if vacant {
                            if let (Some(input), Some(output)) = (path.input, path.output) {
                                p.set_attach(split, input + 1);
                                p.set_attach(join, output);
                            }
                        }
                    }
                    p.insert_slot(from, before)
                        .then_some(())
                        .ok_or("there is no room for it there")
                });
            }
            Cmd::MoveJunction { junction, before } => {
                self.edit_document(|p| {
                    p.set_attach(junction, before)
                        .then_some(())
                        .ok_or("only a fork or merge can be moved")
                });
            }
            Cmd::SetTempo(bpm) => {
                self.snapshot();
                if self.run_on_device(|d| d.set_tempo(bpm)) {
                    self.dirty = true;
                    self.reload();
                }
            }
            Cmd::RenameSnapshot { index, name } => {
                self.snapshot();
                if self.run_on_device(|d| d.rename_snapshot(index, &name)) {
                    self.dirty = true;
                    self.reload();
                }
            }
            Cmd::AssignMidi { block, on, cc } => {
                self.snapshot();
                if self.run_on_device(|d| d.assign_bypass_midi(block, on.then_some(cc))) {
                    self.dirty = true;
                    self.reload();
                }
            }
            Cmd::SetAssignCc { block, param, cc } => {
                self.snapshot();
                if self.run_on_device(|d| d.set_assign_cc(block, param, cc)) {
                    self.dirty = true;
                    self.reload();
                }
            }
            Cmd::ListSetlists => {
                if let Some(names) = self.try_on_device(|d| d.setlists()) {
                    self.send(Evt::Setlists(names));
                }
            }
            Cmd::ListIrs => {
                if let Some(slots) = self.try_on_device(|d| d.irs()) {
                    self.send(Evt::Irs(slots));
                }
            }
            Cmd::LoadIr { slot, file } => {
                let loaded = self.try_on_device(|d| {
                    let wav = crate::wav::read(&file)?;
                    let samples = hx_usb::ir::prepare(&wav.samples, wav.sample_rate)?;
                    let name = file
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("impulse")
                        .chars()
                        .take(20)
                        .collect::<String>();
                    d.upload_ir(slot, &name, &samples)
                });
                if loaded.is_some() {
                    self.handle(Cmd::ListIrs);
                }
            }
            Cmd::ListFavourites => {
                if let Some(list) = self.try_on_device(|d| d.favourites()) {
                    self.send(Evt::Favourites(list));
                }
            }
            Cmd::SaveFavourite { block, index, name } => {
                if self.run_on_device(|d| d.save_favourite(block, index, &name)) {
                    self.send(Evt::Activity(format!("kept {name}")));
                    self.handle(Cmd::ListFavourites);
                }
            }
            Cmd::ClearFavourite(index) => {
                if self.run_on_device(|d| d.clear_favourite(index)) {
                    self.handle(Cmd::ListFavourites);
                }
            }
            Cmd::SaveIr { slot, file } => match self.try_on_device(|d| d.read_ir(slot)) {
                Some(Some((name, samples))) => match crate::wav::write(&file, &samples, 48_000) {
                    Ok(()) => {
                        self.send(Evt::Activity(format!("saved {name} to {}", file.display())))
                    }
                    Err(e) => self.send(Evt::Failed(e.to_string())),
                },
                Some(None) => self.send(Evt::Failed("that slot is empty".into())),
                None => {}
            },
            Cmd::RenameIr { slot, name } => {
                if self.run_on_device(|d| d.rename_ir(slot, &name)) {
                    self.handle(Cmd::ListIrs);
                }
            }
            Cmd::ClearIr(slot) => {
                if self.run_on_device(|d| d.clear_ir(slot)) {
                    self.handle(Cmd::ListIrs);
                }
            }
            Cmd::SetModel {
                block,
                model,
                paired,
            } => {
                self.snapshot();
                if self.run_on_device(|d| place(d, block, model, paired)) {
                    self.dirty = true;
                    self.reload();
                }
            }
        }
    }

    fn connect(&mut self) {
        let found = match hx_usb::list() {
            Ok(devices) => devices.into_iter().next(),
            Err(e) => return self.send(Evt::Failed(e.to_string())),
        };
        let Some(found) = found else {
            return self.send(Evt::Failed(
                "No HX device found - check the USB cable.".into(),
            ));
        };
        // Retry once: the device ignores a fresh session's opening handshake
        // on roughly every other attempt. The CLI does the same.
        let opened = found.open().or_else(|_| found.open());
        match opened {
            Ok(session) => {
                let profile = session.profile;
                self.device = Some(session);
                // A fresh session starts with a clean slate: whatever history
                // was recorded belongs to whatever was connected before.
                self.forget_history();
                self.dirty = false;
                self.send(Evt::Connected {
                    device: profile.name.to_owned(),
                    presets: profile.presets,
                });
                // Load the chain first: it is the view the user is looking at.
                // The name list is a nicety and rides on the control channel,
                // which is the flakier of the two.
                self.reload();
                if let Some(hx_proto::msgpack::Value::Bool(on)) =
                    self.try_optional_on_device(|device| device.object(203))
                {
                    self.send(Evt::Settings { global_eq: on });
                }
                if self.device.is_none() {
                    return;
                }
                // The name list rides on the flakier control channel, so give
                // it the same second chance the session itself gets.
                let names = self
                    .try_on_device(|d| d.presets(0))
                    .or_else(|| self.try_on_device(|d| d.presets(0)));
                if let Some(names) = names {
                    self.send(Evt::Presets(names));
                }
            }
            Err(e) => self.send(Evt::Failed(e.to_string())),
        }
    }

    /// Remember the preset as it stands, unless this burst already did.
    fn snapshot(&mut self) {
        if self.snapshot_taken {
            return;
        }
        if self.record_history() {
            self.snapshot_taken = true;
        }
    }

    /// Push the preset as it stands onto the undo stack, unconditionally.
    /// Returns whether there was a preset to record.
    fn record_history(&mut self) -> bool {
        let Some(document) = self.try_on_device(|device| device.read_preset()) else {
            return false;
        };
        self.history.push(document.encode());
        if self.history.len() > 32 {
            self.history.remove(0);
        }
        // A fresh edit is a new branch; anything undone is unreachable now.
        self.future.clear();
        self.report_history();
        true
    }

    /// Drop the whole history, for when what it records no longer exists.
    fn forget_history(&mut self) {
        self.history.clear();
        self.future.clear();
        self.snapshot_taken = false;
        self.report_history();
    }

    fn report_history(&self) {
        self.events.send(Evt::History {
            undo: self.history.len(),
            redo: self.future.len(),
        });
    }

    /// Move one step back or forward through the document history.
    fn step_history(&mut self, back: bool) {
        let document = if back {
            self.history.last()
        } else {
            self.future.last()
        }
        .cloned();
        let Some(document) = document else {
            let what = if back { "undo" } else { "redo" };
            return self.send(Evt::Activity(format!("nothing to {what}")));
        };
        let Some(preset) = hx_proto::Preset::parse(&document) else {
            if back {
                self.history.pop();
            } else {
                self.future.pop();
            }
            self.report_history();
            return self.send(Evt::Failed("the history is corrupt".into()));
        };
        // Keep the current state on the other stack so the step is reversible.
        let Some(current) = self.try_on_device(|device| device.read_preset()) else {
            return;
        };
        if self.run_on_device(|d| d.write_preset(&preset)) {
            if back {
                self.history.pop();
                self.future.push(current.encode());
            } else {
                self.future.pop();
                self.history.push(current.encode());
            }
            // The buffer now differs from the stored preset - almost always,
            // and "save available after undo" errs on the side of not losing
            // the state someone deliberately stepped to.
            self.dirty = true;
            self.send(Evt::Activity(if back { "undone" } else { "redone" }.into()));
            self.report_history();
            self.present(&preset);
        }
    }

    /// Apply a change to the whole preset document, keeping an undo step.
    ///
    /// Document edits are all-or-nothing: the device takes the new document or
    /// the preset is lost, so the original is kept first and only a successful
    /// modification is sent.
    fn edit_document<F>(&mut self, change: F)
    where
        F: FnOnce(&mut hx_proto::Preset) -> Result<(), &'static str>,
    {
        // Settled: this read may land while the previous edit's write is
        // still committing, and a quick second drag deserves patience too.
        let Some(mut preset) = self.read_settled() else {
            return;
        };
        let original = preset.encode();
        if let Err(why) = change(&mut preset) {
            return self.send(Evt::Failed(why.into()));
        }
        if self.run_on_device(|d| d.write_preset(&preset)) {
            self.dirty = true;
            // Bounded: this is an undo history, not an archive.
            self.history.push(original);
            if self.history.len() > 32 {
                self.history.remove(0);
            }
            // A fresh edit is a new branch; anything undone is unreachable now.
            self.future.clear();
            self.report_history();
            // What was written is what there is - no read-back to race.
            self.present(&preset);
        }
    }

    /// The loaded preset's name, or an empty string if the device will not say.
    fn preset_name(&mut self) -> String {
        self.device
            .as_mut()
            .and_then(|d| d.preset_info().ok())
            .map(|(_, _, name)| name)
            .unwrap_or_default()
    }

    /// The loaded preset exactly as the device holds it.
    fn preset_bytes(&mut self) -> Option<Vec<u8>> {
        let device = self.device.as_mut()?;
        match device.read_preset() {
            Ok(preset) => Some(preset.encode()),
            Err(e) => {
                self.send(Evt::Failed(format!("reading the preset: {e}")));
                None
            }
        }
    }

    /// Put the device on `dest` so a load lands there, not over the open
    /// preset. A no-op when it is already the one loaded.
    fn go_to(&mut self, dest: i64) -> bool {
        let current = self
            .device
            .as_mut()
            .and_then(|d| d.preset_info().ok())
            .map(|(_, index, _)| index);
        if current == Some(dest) {
            return true;
        }
        let setlist = self.setlist;
        if !self.run_on_device(|d| d.select_preset(setlist, dest)) {
            return false;
        }
        // The history belongs to the preset it was recorded on.
        self.forget_history();
        self.dirty = false;
        true
    }

    /// Load a symbolic tone: clear the chain, then build it back block by
    /// block. The recipe is hardware-proven: clearing first is what makes
    /// room, and a model set into a cleared slot is an ordinary edit. One
    /// history step covers the whole load, so undo puts the chain back.
    fn load_steps(&mut self, dest: i64, name: &str, blocks: &[ApplyBlock]) {
        if !self.go_to(dest) {
            return;
        }
        let recorded = self.record_history();
        match self.apply_steps(blocks) {
            Ok(()) => {
                self.dirty = true;
                self.send(Evt::Activity(format!("loaded {name}; Save keeps it")));
                self.reload();
            }
            Err(why) => {
                self.send(Evt::Failed(why));
                if recorded {
                    // Put the chain back the way it was.
                    self.step_history(true);
                }
            }
        }
    }

    /// Capture the loaded edit buffer and its undo state once, before the
    /// first cloud Tone replaces it. Moving between audition rows keeps this
    /// same baseline so Done always returns to the sound heard before browsing.
    fn begin_audition(&mut self) -> bool {
        if self.audition.is_some() {
            return true;
        }
        let Some(original) = self.preset_bytes() else {
            return false;
        };
        self.audition = Some(Audition {
            key: -1,
            original,
            dirty: self.dirty,
            history: std::mem::take(&mut self.history),
            future: std::mem::take(&mut self.future),
            snapshot_taken: self.snapshot_taken,
        });
        self.snapshot_taken = false;
        self.report_history();
        true
    }

    fn audition_document(&mut self, key: i64, name: &str, bytes: &[u8]) {
        let Some(preset) = hx_proto::Preset::parse(bytes) else {
            return self.send(Evt::Failed(format!("{name} is not a readable preset")));
        };
        if !self.begin_audition() {
            return;
        }
        if self.run_on_device(|device| device.write_preset(&preset)) {
            self.dirty = false;
            if let Some(audition) = self.audition.as_mut() {
                audition.key = key;
            }
            self.present(&preset);
            self.send(Evt::Auditioning(Some(key)));
            self.send(Evt::Activity(format!("auditioning {name}")));
        } else {
            self.send(Evt::Auditioning(self.audition.as_ref().map(|a| a.key)));
        }
    }

    fn audition_steps(&mut self, key: i64, name: &str, blocks: &[ApplyBlock]) {
        if !self.begin_audition() {
            return;
        }
        if let Err(why) = self.apply_steps(blocks) {
            self.send(Evt::Failed(why));
            self.end_audition();
            return;
        }
        let Some(preset) = self.read_settled() else {
            self.end_audition();
            return;
        };
        self.dirty = false;
        if let Some(audition) = self.audition.as_mut() {
            audition.key = key;
        }
        self.present(&preset);
        self.send(Evt::Auditioning(Some(key)));
        self.send(Evt::Activity(format!("auditioning {name}")));
    }

    /// Restore the captured document byte-for-byte and reinstate the undo
    /// stacks exactly as they were before discovery began.
    fn end_audition(&mut self) {
        let Some(audition) = self.audition.take() else {
            return;
        };
        let Some(original) = hx_proto::Preset::parse(&audition.original) else {
            self.audition = Some(audition);
            return self.send(Evt::Failed(
                "the preset saved before audition is unreadable".into(),
            ));
        };
        if !self.run_on_device(|device| device.write_preset(&original)) {
            self.audition = Some(audition);
            return;
        }
        self.dirty = audition.dirty;
        self.history = audition.history;
        self.future = audition.future;
        let snapshot_taken = audition.snapshot_taken;
        self.report_history();
        self.present(&original);
        self.snapshot_taken = snapshot_taken;
        self.send(Evt::Auditioning(None));
        self.send(Evt::Activity(
            "restored the sound from before the audition".into(),
        ));
    }

    /// Turn the temporary sound into a normal edit. Its displaced preset is
    /// the next undo step, so even this deliberate handoff remains reversible.
    fn keep_audition(&mut self) {
        let Some(audition) = self.audition.take() else {
            return;
        };
        let Some(current) = self.read_settled() else {
            self.audition = Some(audition);
            return;
        };
        self.history = audition.history;
        self.future.clear();
        self.history.push(audition.original);
        if self.history.len() > 32 {
            self.history.remove(0);
        }
        self.snapshot_taken = false;
        self.dirty = true;
        self.report_history();
        self.present(&current);
        self.send(Evt::Auditioning(None));
        self.send(Evt::Activity(
            "kept the audition in the edit buffer; Save writes it".into(),
        ));
    }

    /// Clear every block, then set each of the tone's blocks into the run of
    /// slots after the endpoints, with its parameters and bypass state.
    fn apply_steps(&mut self, blocks: &[ApplyBlock]) -> Result<(), String> {
        use hx_proto::msgpack::Value;

        let preset = self.read_settled().ok_or("the device stopped answering")?;
        for (position, slot) in preset.slots.iter().enumerate() {
            if slot.kind == hx_proto::preset::Kind::Block && slot.model.is_some() {
                let p = position as i64;
                if !self.run_on_device(|d| d.clear_block(p)) {
                    return Err(format!("could not clear block {position}"));
                }
            }
        }

        // The clears are writes the device commits at its own pace, and a
        // request that lands mid-commit is refused. Reading the preset back
        // is the barrier that proves it is ready for more.
        let _ = self.read_settled();

        // The block run follows the input and ends at the output - probed on
        // hardware: an HX Stomp carries its input at slot 0, blocks across
        // slots 1 to 8, and the output after them.
        let layout = preset.layout();
        let path = layout
            .paths
            .first()
            .ok_or("this preset has no signal path")?;
        let base = path.input.map(|input| input + 1).unwrap_or(1);
        let ceiling = path.output.unwrap_or(preset.slots.len());

        for (i, block) in blocks.iter().enumerate() {
            let position = (base + i) as i64;
            if base + i >= ceiling {
                return Err(format!(
                    "this tone has {} blocks; this device fits {i}",
                    blocks.len()
                ));
            }
            // A refusal here is usually pacing, not a verdict - the device is
            // still committing the previous edit. Ask quietly, settle, and
            // only a second refusal counts.
            let model = block.model;
            let placed = self.quietly(|d| d.set_model(position, model)) || {
                let _ = self.read_settled();
                self.run_on_device(|d| d.set_model(position, model))
            };
            if !placed {
                return Err(format!(
                    "this tone has {} blocks; this device took {i}",
                    blocks.len()
                ));
            }
            for (index, value, switch) in &block.params {
                let wire = if *switch {
                    Value::Bool(*value >= 0.5)
                } else {
                    Value::F32(*value)
                };
                // A parameter the device declines is a detail; the block is
                // already right, so keep going rather than tearing down.
                let (index, wire) = (*index, wire.clone());
                if !self.quietly(|d| d.set_param(position, index, wire.clone())) {
                    let _ = self.read_settled();
                    let _ = self.quietly(|d| d.set_param(position, index, wire));
                }
            }
            if !block.enabled && !self.quietly(|d| d.set_enabled(position, false)) {
                let _ = self.read_settled();
                let _ = self.quietly(|d| d.set_enabled(position, false));
            }
        }
        Ok(())
    }

    /// Ask the device without reporting a refusal: for requests that will be
    /// retried, where the first no is pacing rather than an answer.
    fn quietly<T>(&mut self, f: impl FnOnce(&mut hx_usb::Session) -> hx_usb::Result<T>) -> bool {
        self.device.as_mut().is_some_and(|d| f(d).is_ok())
    }

    /// Write a whole preset document over the loaded one.
    ///
    /// The bytes are parsed first. A malformed document is accepted by the
    /// device and then reads back as an empty preset, so refusing early is the
    /// difference between "that file is not a preset" and a wiped slot.
    /// Read the whole setlist off the pedal, document by document.
    ///
    /// This is the read half of a backup without writing a bundle: the same
    /// `read_preset_at` that made reading all 126 take under two seconds
    /// instead of two minutes, because it never loads a preset to read it.
    /// Nothing here writes to the device.
    fn capture_setlist(&mut self) {
        let setlist = self.setlist;
        let Some(names) = self.try_on_device(|d| d.presets(setlist)) else {
            return;
        };
        let total = names.len();
        let mut slots = Vec::with_capacity(total);
        for (index, name) in names.into_iter().enumerate() {
            self.send(Evt::Working {
                what: "reading the setlist".into(),
                progress: index as f32 / total.max(1) as f32,
            });
            // `Ok(None)` is a genuinely empty slot. An outer `None` means the
            // request itself failed; flattening the two would turn an
            // unreadable occupied slot into an empty one in the export.
            let Some(preset) = self.try_on_device(|d| d.read_preset_at(setlist, index as i64))
            else {
                self.send(Evt::Working {
                    what: String::new(),
                    progress: 1.0,
                });
                return;
            };
            let bytes = preset.map(|preset| preset.encode());
            slots.push((name, bytes));
        }
        self.send(Evt::Working {
            what: String::new(),
            progress: 1.0,
        });
        self.send(Evt::CapturedSetlist(slots));
    }

    /// Write a whole setlist onto the pedal.
    ///
    /// Every slot is a flash write, and unpaced flash writes are what once
    /// corrupted a setlist past a power cycle - so this goes through
    /// `write_preset_at` and `clear_preset_at`, which pace their own commits,
    /// one slot at a time and never in a hurry.
    fn push_setlist(&mut self, slots: Vec<SlotWrite>) {
        let setlist = self.setlist;
        let total = slots.len();
        let mut written = 0usize;
        for (step, (index, bytes)) in slots.into_iter().enumerate() {
            self.send(Evt::Working {
                what: "writing the setlist".into(),
                progress: step as f32 / total.max(1) as f32,
            });
            let ok = match bytes {
                Some((name, bytes)) => match hx_proto::Preset::parse(&bytes) {
                    Some(preset) => {
                        self.run_on_device(|d| d.write_preset_at(setlist, index, &name, &preset))
                    }
                    None => {
                        self.send(Evt::Failed(format!(
                            "{} is not a preset document",
                            hx_proto::rpc::slot_label(index)
                        )));
                        false
                    }
                },
                None => self.run_on_device(|d| d.clear_preset_at(setlist, index)),
            };
            if !ok {
                // The device has stopped answering; carrying on would be 100
                // more failures and a longer wait for the same news.
                break;
            }
            written += 1;
        }
        self.send(Evt::Working {
            what: String::new(),
            progress: 1.0,
        });
        if let Some(names) = self.try_on_device(|d| d.presets(setlist)) {
            self.send(Evt::Presets(names));
        }
        self.reload();
        self.send(Evt::Activity(format!(
            "wrote {written} presets to the pedal"
        )));
        // The bundle now describes a pedal that no longer exists. Re-reading it
        // whole costs a couple of seconds against the minutes of flash writes
        // that just happened, and the snapshot it rotates aside is the pedal as
        // it was before the setlist landed, which is worth having.
        self.refresh_automatic();
    }

    /// Bring the automatic backup back in step with the pedal, if there is one.
    fn refresh_automatic(&mut self) {
        let Some(dir) = automatic_dir() else { return };
        if hx_usb::backup::exists(&dir) {
            self.back_up(&dir);
        }
    }

    fn paste(&mut self, blob: &[u8]) {
        let Some(preset) = hx_proto::Preset::parse(blob) else {
            self.send(Evt::Failed("that is not a preset file".into()));
            return;
        };
        // A paste replaces the whole document; it is exactly the kind of edit
        // someone reaches for undo after.
        let recorded = self.record_history();
        if self.run_on_device(|d| d.write_preset(&preset)) {
            self.dirty = true;
            self.present(&preset);
        } else if recorded {
            self.history.pop();
            self.report_history();
        }
    }

    /// Read the preset back from the device and show it.
    /// Read the whole pedal into a bundle directory.
    fn back_up(&mut self, dir: &std::path::Path) {
        let stamp = now();
        // Put the copy that is there aside before overwriting it. Corruption is
        // noticed later than it happens, and a single bundle that every
        // connection refreshes is always the pedal as it is now - which is no
        // use at all when what you need is the pedal as it was on Tuesday.
        if Some(dir) == automatic_dir().as_deref() {
            match hx_usb::backup::snapshot(dir, &datestamp(), KEEP_SNAPSHOTS) {
                Ok(Some(_)) => {}
                Ok(None) => {}
                Err(e) => self.send(Evt::Activity(format!("could not keep a snapshot: {e}"))),
            }
        }
        let events = self.events.clone();
        let outcome = self.try_on_device(|d| {
            hx_usb::backup::capture(d, dir, stamp, |step| {
                if let Some(evt) = working(&step) {
                    events.send(evt);
                }
            })
        });
        if let Some(manifest) = outcome {
            self.send(Evt::BackedUp {
                dir: dir.to_owned(),
                presets: manifest.presets.iter().filter(|n| !n.is_empty()).count(),
                settings: manifest.globals,
                irs: manifest.irs.len(),
            });
        }
    }

    /// Write a bundle back onto the pedal, then show what is there now.
    fn restore_all(&mut self, dir: &std::path::Path) {
        let events = self.events.clone();
        let done = self.run_on_device(|d| {
            hx_usb::backup::restore(dir, d, hx_usb::backup::Parts::default(), |step| {
                if let Some(evt) = working(&step) {
                    events.send(evt);
                }
            })
        });
        if done {
            self.send(Evt::Activity("restored from backup".into()));
            self.refresh_automatic();
            let setlist = self.setlist;
            if let Some(names) = self.try_on_device(|d| d.presets(setlist)) {
                self.send(Evt::Presets(names));
            }
            self.reload();
        }
    }

    /// Keep the automatic backup current after a save.
    ///
    /// Silent on purpose: it costs milliseconds and nobody asked for it, so it
    /// should not interrupt. A missing backup directory simply means automatic
    /// backups are not set up yet, which is not an error worth reporting.
    fn back_up_one(&mut self, index: i64) {
        let Some(dir) = automatic_dir() else { return };
        if !hx_usb::backup::exists(&dir) {
            return;
        }
        let _ = self.try_on_device(|d| hx_usb::backup::capture_one(d, &dir, index));
    }

    fn reload(&mut self) {
        let Some(preset) = self.read_settled() else {
            return;
        };
        let Some((_, index, name)) = self.try_on_device(|device| device.preset_info()) else {
            return;
        };
        self.shown = (index, name);
        self.present(&preset);
    }

    /// Read the preset, giving the device time to settle first if it must.
    ///
    /// A document write takes the device a moment to commit, and a read that
    /// lands inside that moment fails. That is a busy device, not a dead one:
    /// only when it stays unreachable is the session dropped.
    fn read_settled(&mut self) -> Option<hx_proto::Preset> {
        let device = self.device.as_mut()?;
        let mut last = None;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            match device.read_preset() {
                Ok(preset) => return Some(preset),
                Err(e) => last = Some(e),
            }
        }
        if let Some(e) = last {
            self.send(Evt::Failed(e.to_string()));
        }
        self.device = None;
        self.send(Evt::Disconnected);
        None
    }

    /// Show a preset document the worker already holds.
    ///
    /// Every edit used to be followed by a read-back, and the device commits
    /// writes slowly enough that the read could return the *old* document -
    /// a drag that "didn't take" - or fail outright and drop the session.
    /// For our own writes the written bytes are the truth, so the view is
    /// built from them and the wire stays quiet.
    fn present(&mut self, preset: &hx_proto::Preset) {
        let firmware = preset.firmware().unwrap_or_default();
        // Everything the signal passes through, not just the effects: HX Edit
        // draws the input, output and any split/join, and a chain without them
        // reads as though it starts nowhere.
        let chain = chain_of(preset);
        self.snapshot_taken = false;
        self.send(Evt::Loaded {
            index: self.shown.0,
            name: self.shown.1.clone(),
            firmware,
            tempo: preset.tempo(),
            snapshots: preset.snapshots(),
            chain,
            layout: preset.layout(),
            assignments: preset.assignments(),
            dirty: self.dirty,
        });
    }

    fn poll(&mut self) {
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let polled = device.poll_notifications();
        if let Ok(events) = &polled {
            for (event, args) in events {
                self.events
                    .send(Evt::Activity(format!("event {event}: {args:?}")));
            }
        }
        // The device goes quiet while committing a write; one missed beat is
        // patience, not a dead link.
        match polled.map(|_| ()).and_then(|_| device.keepalive()) {
            Ok(()) => self.stumbles = 0,
            Err(e) => {
                self.stumbles += 1;
                if self.stumbles >= 3 {
                    self.send(Evt::Failed(e.to_string()));
                    self.device = None;
                    self.send(Evt::Disconnected);
                }
            }
        }
    }

    /// Run something on the device, reporting failure. Returns whether it worked.
    fn run_on_device(
        &mut self,
        f: impl FnOnce(&mut hx_usb::Session) -> hx_usb::Result<()>,
    ) -> bool {
        self.try_on_device(f).is_some()
    }

    fn try_on_device<T>(
        &mut self,
        f: impl FnOnce(&mut hx_usb::Session) -> hx_usb::Result<T>,
    ) -> Option<T> {
        let result = {
            let device = self.device.as_mut()?;
            f(device)
        };
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                let lost = e.loses_session();
                self.events.send(Evt::Failed(e.to_string()));
                if lost {
                    // Never let the next queued click write onto an unknown
                    // transaction/sequence state. Releasing the interface is
                    // the only safe response; the UI can offer a fresh Connect
                    // after the pedal itself is healthy again.
                    self.device = None;
                    self.stumbles = 0;
                    self.events.send(Evt::Disconnected);
                }
                None
            }
        }
    }

    /// Probe an optional device capability quietly when the device explicitly
    /// refuses it, while still treating transport loss as session loss.
    fn try_optional_on_device<T>(
        &mut self,
        f: impl FnOnce(&mut hx_usb::Session) -> hx_usb::Result<T>,
    ) -> Option<T> {
        let result = {
            let device = self.device.as_mut()?;
            f(device)
        };
        match result {
            Ok(value) => Some(value),
            Err(hx_usb::Error::Device(_)) => None,
            Err(error) => {
                self.events.send(Evt::Failed(error.to_string()));
                self.device = None;
                self.stumbles = 0;
                self.events.send(Evt::Disconnected);
                None
            }
        }
    }

    fn send(&self, evt: Evt) {
        self.events.send(evt);
    }
}

/// Seconds since the epoch, for stamping a bundle.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How many dated copies of the pedal to keep behind the current one.
///
/// A whole pedal is a few megabytes, so this is tens of megabytes at worst -
/// against the alternative, which is having exactly one copy and it being the
/// broken one.
const KEEP_SNAPSHOTS: usize = 10;

/// The current date and time, as a name that sorts by time.
///
/// Most significant first and no separators that a filesystem would object to,
/// so ordering the snapshots by name orders them by age without trusting any
/// filesystem's idea of when a directory was written.
fn datestamp() -> String {
    stamp_of(now())
}

/// The date maths, apart from the clock so it can be checked.
///
/// Days from the Unix epoch converted with the civil-from-days algorithm, which
/// is exact and needs no calendar library for the one place this program has
/// ever needed a date.
fn stamp_of(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let time = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}{:02}{:02}",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// Where the automatic backup lives: one bundle, kept current.
pub fn automatic_dir() -> Option<std::path::PathBuf> {
    hx_catalog::home::backups().map(|d| d.join("automatic.hxbundle"))
}

/// Turn a capture or restore step into something to show.
fn working(step: &hx_usb::backup::Step) -> Option<Evt> {
    use hx_usb::backup::Step;
    Some(match step {
        Step::Presets { done, total, .. } => Evt::Working {
            what: "presets".into(),
            progress: *done as f32 / (*total).max(1) as f32,
        },
        Step::Globals => Evt::Working {
            what: "settings".into(),
            progress: 0.9,
        },
        Step::Irs { done, total } => Evt::Working {
            what: "impulse responses".into(),
            progress: 0.9 + 0.1 * (*done as f32 / (*total).max(1) as f32),
        },
        Step::Done => Evt::Working {
            what: String::new(),
            progress: 1.0,
        },
    })
}

/// A device setting as a plain number, whatever shape it arrived in: a switch
/// is 0 or 1, a choice its index, a number itself.
fn as_number(value: &hx_proto::msgpack::Value) -> Option<f32> {
    use hx_proto::msgpack::Value;
    Some(match value {
        Value::Bool(b) => *b as u8 as f32,
        Value::Int(i) | Value::WideInt(i, _) => *i as f32,
        Value::UInt(u) | Value::Wide(u, _) => *u as f32,
        Value::F32(f) => *f,
        Value::F64(f) => *f as f32,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The snapshot names are the only dates this program writes, and they are
    /// what a person reads when choosing which copy of their pedal to go back
    /// to. Civil-from-days is easy to get subtly wrong, so it is pinned at the
    /// points where it usually breaks: an epoch, a leap day, and a century that
    /// is not a leap year.
    #[test]
    fn the_datestamp_is_a_real_date_that_sorts_by_time() {
        assert_eq!(stamp_of(0), "1970-01-01 000000");
        assert_eq!(stamp_of(86_399), "1970-01-01 235959");
        assert_eq!(stamp_of(86_400), "1970-01-02 000000");
        // 2000 was a leap year; 1900 was not, and 2100 will not be.
        assert_eq!(stamp_of(951_782_400), "2000-02-29 000000");
        assert_eq!(stamp_of(4_107_542_400), "2100-03-01 000000");
        // The date this was written.
        assert_eq!(stamp_of(1_786_060_800), "2026-08-07 000000");

        // Sorting the names sorts them by time, which is what prunes the
        // oldest snapshot rather than an arbitrary one.
        let mut names = [stamp_of(1_786_060_800), stamp_of(0), stamp_of(951_782_400)];
        names.sort();
        assert_eq!(names[0], stamp_of(0));
        assert_eq!(names[2], stamp_of(1_786_060_800));
    }
}
