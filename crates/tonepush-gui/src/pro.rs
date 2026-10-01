//! Native StompStation PRO panel and its strictly ordered VoidX worker.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::RichText;
use serde_json::Value;
use voidx_client::backup::{self, ArmedRollback};
use voidx_client::{BlobList, Device, Identity, SerialLink, UploadStep, WriteSafety};
use voidx_proto::{NodeDescription, NodeKind, NodePath, Preset};

use crate::{config, theme, LibraryLookup};

mod board;
mod firmware;
mod frame;
mod libraries;
mod pane;
mod settings;
pub(crate) use frame::{slot_label, Picked};
use libraries::LibraryFacts;
pub(crate) use pane::LibraryNote;

/// Keep PRO favourites apart from the first HX setlist in the existing local
/// preferences file. The UI is shared; only the device-address key differs.
const FAVORITES_SETLIST: i64 = -1;

/// The firmware has input controls under global settings, not under
/// `root\app`. This sentinel lets the input endpoint participate in the same
/// selection model as preset processors without inventing a device path.
const INPUT_GROUP: &str = "@input";

const LIBRARIES: [Library; 4] = [
    Library::Presets,
    Library::Irs,
    Library::Amps,
    Library::Drives,
];

type PresetSlotWrite = (usize, Option<(String, Vec<u8>)>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Library {
    Presets,
    Irs,
    Amps,
    Drives,
}

impl Library {
    fn path(self) -> &'static str {
        match self {
            Self::Presets => "root\\presets",
            Self::Irs => "root\\ir_list",
            Self::Amps => "root\\nam_amp",
            Self::Drives => "root\\nam_drive",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Presets => "Presets",
            Self::Irs => "Impulse responses",
            Self::Amps => "NAM amps",
            Self::Drives => "NAM drives",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Presets => "vxpreset",
            Self::Irs => "wav",
            Self::Amps | Self::Drives => "nam",
        }
    }
}

#[derive(Clone)]
struct LibraryState {
    library: Library,
    info: BlobList,
}

#[derive(Clone)]
struct Snapshot {
    identity: Identity,
    transport: String,
    active_preset: Option<String>,
    libraries: Vec<LibraryState>,
    app: Vec<(NodePath, NodeDescription)>,
    settings: Vec<(NodePath, NodeDescription)>,
}

enum Cmd {
    Connect,
    Disconnect,
    Undo,
    Redo,
    SelectPreset(usize),
    ReadPreset {
        index: usize,
        target: ReadTarget,
    },
    CaptureSetlist,
    SetNode {
        path: NodePath,
        description: Box<NodeDescription>,
        before: Value,
        value: Value,
        persistent: bool,
    },
    UseRollback(PathBuf),
    SavePreset(String),
    Rename {
        library: Library,
        index: usize,
        name: String,
    },
    Move {
        library: Library,
        from: usize,
        to: usize,
    },
    Clear {
        library: Library,
        index: usize,
    },
    Import {
        library: Library,
        index: usize,
        name: String,
        file: PathBuf,
    },
    ImportBytes {
        index: usize,
        name: String,
        bytes: Vec<u8>,
    },
    PushSetlist(Vec<PresetSlotWrite>),
    Audition {
        key: i64,
        name: String,
        bytes: Vec<u8>,
    },
    EndAudition,
    KeepAudition,
    Export {
        library: Library,
        index: usize,
        file: PathBuf,
    },
    ImportStereo {
        left: usize,
        right: usize,
        name: String,
        file: PathBuf,
    },
    ExportStereo {
        left: usize,
        right: usize,
        file: PathBuf,
    },
    Backup(PathBuf),
    /// Back the pedal up where TonePush keeps its own backups, which unlocks
    /// saving once it matches.
    BackupHere,
    Restore(PathBuf),
    /// Check a firmware file and keep it for an update.
    FirmwareLoad(PathBuf),
    /// Back the pedal up for an update.
    FirmwareBackup,
    /// The pedal is in Update Mode already: find a recent backup of it.
    FirmwareFindBackup,
    /// Let the pedal go and wait for it in Update Mode.
    FirmwareAwait,
    /// Write the firmware file, which must be this version.
    FirmwareWrite {
        expect: String,
    },
    /// Stop an update that is not writing, and look for the pedal.
    FirmwareCancel,
}

enum ReadTarget {
    Library { replace: bool },
    Clipboard,
    File(PathBuf),
}

enum Evt {
    Connected(Snapshot),
    /// A Connect has been handled, whatever came of it. The connect page
    /// watches USB only while no look is in flight.
    Looked,
    Snapshot {
        snapshot: Snapshot,
        baseline: bool,
    },
    NodeValue {
        path: String,
        value: Value,
    },
    PresetRead {
        index: usize,
        name: String,
        bytes: Vec<u8>,
        target: ReadTarget,
    },
    PresetIndexed {
        index: usize,
        hash: Option<String>,
    },
    ForgetPresetHashes,
    SetlistRead(Vec<(String, Option<Vec<u8>>)>),
    Auditioning(Option<i64>),
    Guarded {
        path: PathBuf,
        preset_hashes: BTreeMap<usize, String>,
        facts: LibraryFacts,
    },
    History {
        undo: usize,
        redo: usize,
    },
    Busy(bool),
    Progress(String),
    Working {
        what: String,
        progress: f32,
    },
    Success(String),
    Failed(String),
    /// The pedal answered, on firmware TonePush has not been verified
    /// against: it is browsed, exported and backed up, and nothing else.
    ReadOnly(String),
    /// A pedal found in Update Mode: only its identity was read.
    UpdateMode(Identity),
    /// How a firmware update is going.
    Firmware(firmware::FirmwareEvt),
    Disconnected,
}

/// The tabs of the PRO's page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Backups,
    Library(Library),
    Settings,
    Firmware,
}

struct Confirmation {
    question: String,
    /// The confirming button's label, naming what it does.
    action: &'static str,
    command: Cmd,
}

pub(crate) struct Panel {
    tx: Sender<Cmd>,
    rx: Receiver<Evt>,
    /// Whether a Connect is with the worker and not yet answered.
    connecting: bool,
    active: bool,
    online: bool,
    busy: bool,
    dirty: bool,
    status: String,
    snapshot: Option<Snapshot>,
    rollback: Option<PathBuf>,
    /// When the backup the rollback guard holds was taken, for the deck and
    /// the sidebar's foot.
    rollback_time: Option<SystemTime>,
    /// Whether the worker's last word was a failure, so the deck says it in
    /// the voice of a problem rather than of a note.
    failed: bool,
    /// Why this pedal is read only, when it is.
    read_only: Option<String>,
    tab: Tab,
    selected_group: String,
    search: String,
    drafts: BTreeMap<String, Value>,
    saved_drafts: BTreeMap<String, Value>,
    undo_depth: usize,
    redo_depth: usize,
    tempo_draft: Option<String>,
    taps: Vec<std::time::Instant>,
    names: BTreeMap<(Library, usize), String>,
    library_searches: BTreeMap<Library, String>,
    target_slots: BTreeMap<Library, usize>,
    slot_renaming: Option<(Library, usize)>,
    save_name: String,
    show_favorites_only: bool,
    renaming_preset: Option<(usize, String)>,
    renaming_header: Option<String>,
    clipboard: Option<(String, Vec<u8>)>,
    preset_hashes: BTreeMap<usize, String>,
    library_documents: Vec<(String, Vec<u8>, bool)>,
    captured_setlists: Vec<Vec<(String, Option<Vec<u8>>)>>,
    /// The sidebar asked to keep the whole pedal as a setlist.
    capture_asked: bool,
    audition_events: Vec<Option<i64>>,
    preview: Option<(String, Snapshot)>,
    confirmation: Option<Confirmation>,
    /// A whole-device operation and its completion fraction. Kept separate
    /// from `busy`: a progress meter communicates work without turning live
    /// sound controls into a loading screen.
    working: Option<(String, f32)>,
    /// A reading being typed: its node, and the text so far.
    typing: Option<(String, String)>,
    /// What the checked backup says about the libraries' files.
    facts: LibraryFacts,
    /// Sent after the confirmation's own command once it is confirmed: the
    /// second slot of a stereo pair being removed.
    after_confirmation: Option<Cmd>,
    /// A firmware update under way.
    firmware: Option<firmware::Flow>,
    /// The pedal is connected in Update Mode: its identity is all TonePush
    /// read, and the update is all it can do.
    update_mode: bool,
    /// The Pedal page's Firmware tab was asked for from another page.
    page_request: bool,
}

impl Panel {
    pub(crate) fn new(ctx: egui::Context) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        if cfg!(test) {
            // A test builds an App, and an App builds this panel: nothing a
            // test asks of it may reach a StompStation PRO that happens to
            // be plugged into the machine running the tests. The commands
            // are taken and dropped, and the events stay open, so the panel
            // behaves as one whose pedal never answers.
            std::thread::spawn(move || {
                let _events = events;
                let _ctx = ctx;
                for _ in commands {}
            });
        } else {
            std::thread::spawn(move || Worker::new(commands, events, ctx).run());
        }
        let mut panel = Self {
            tx,
            rx,
            connecting: false,
            active: false,
            online: false,
            busy: false,
            dirty: false,
            status: "Looking for a StompStation PRO…".into(),
            snapshot: None,
            rollback: None,
            rollback_time: None,
            failed: false,
            read_only: None,
            tab: Tab::Backups,
            selected_group: String::new(),
            search: String::new(),
            drafts: BTreeMap::new(),
            saved_drafts: BTreeMap::new(),
            undo_depth: 0,
            redo_depth: 0,
            tempo_draft: None,
            taps: Vec::new(),
            names: BTreeMap::new(),
            library_searches: BTreeMap::new(),
            target_slots: LIBRARIES.into_iter().map(|library| (library, 1)).collect(),
            slot_renaming: None,
            save_name: String::new(),
            show_favorites_only: false,
            renaming_preset: None,
            renaming_header: None,
            clipboard: None,
            preset_hashes: BTreeMap::new(),
            library_documents: Vec::new(),
            captured_setlists: Vec::new(),
            capture_asked: false,
            audition_events: Vec::new(),
            preview: None,
            confirmation: None,
            working: None,
            typing: None,
            facts: LibraryFacts::default(),
            after_confirmation: None,
            firmware: None,
            update_mode: false,
            page_request: false,
        };
        // A test builds an App, and an App builds this panel: it must never
        // reach for a StompStation PRO that happens to be plugged into the
        // machine running the tests.
        if !cfg!(test) {
            panel.ask_connect();
        }
        panel
    }

    pub(crate) fn claims_ui(&self) -> bool {
        self.active
    }

    pub(crate) fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }

    pub(crate) fn drain(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(Evt::Looked) => self.connecting = false,
                Ok(Evt::Connected(snapshot)) => {
                    self.active = true;
                    self.online = true;
                    self.failed = false;
                    self.read_only = None;
                    self.update_mode = false;
                    self.status.clear();
                    self.preset_hashes.clear();
                    let version = snapshot.identity.version.clone();
                    self.install_snapshot(snapshot, true);
                    // Back after an update: check what it runs.
                    if let Some(flow) = self.firmware.as_mut() {
                        let next = flow.connected(&version);
                        self.firmware_next(next);
                    }
                }
                Ok(Evt::UpdateMode(identity)) => {
                    self.active = true;
                    self.online = false;
                    self.failed = false;
                    self.update_mode = true;
                    self.rollback = None;
                    self.rollback_time = None;
                    self.status = format!("{} in Update Mode", identity.name);
                    self.tab = Tab::Firmware;
                }
                Ok(Evt::Firmware(event)) => {
                    match &event {
                        firmware::FirmwareEvt::Waiting => {
                            // The session ends while the pedal restarts; what
                            // it held stays on screen.
                            self.online = false;
                            self.rollback = None;
                            self.rollback_time = None;
                            self.preset_hashes.clear();
                            self.facts = LibraryFacts::default();
                            self.status = "Editing is paused while the firmware updates".into();
                        }
                        firmware::FirmwareEvt::UpdateMode => self.update_mode = true,
                        firmware::FirmwareEvt::Written | firmware::FirmwareEvt::Failed { .. } => {
                            self.update_mode = false;
                        }
                        _ => {}
                    }
                    if let Some(flow) = self.firmware.as_mut() {
                        let next = flow.on(&event, Instant::now(), SystemTime::now());
                        self.firmware_next(next);
                    }
                }
                Ok(Evt::Snapshot { snapshot, baseline }) => {
                    self.install_snapshot(snapshot, baseline);
                }
                Ok(Evt::NodeValue { path, value }) => {
                    self.drafts.insert(path.clone(), value.clone());
                    if let Some(snapshot) = &mut self.snapshot {
                        for (node, description) in
                            snapshot.app.iter_mut().chain(snapshot.settings.iter_mut())
                        {
                            if node.as_str() == path {
                                description.value = Some(value.clone());
                            }
                        }
                    }
                    self.recompute_dirty();
                }
                Ok(Evt::PresetRead {
                    index,
                    name,
                    bytes,
                    target,
                }) => {
                    self.preset_hashes.insert(index, slot_hash(&bytes));
                    match target {
                        ReadTarget::Library { replace } => {
                            self.library_documents.push((name, bytes, replace));
                        }
                        ReadTarget::Clipboard => {
                            self.status = format!("Copied {name}");
                            self.clipboard = Some((name, bytes));
                        }
                        ReadTarget::File(path) => {
                            self.status = match crate::library::atomic_write(&path, bytes) {
                                Ok(()) => format!("Exported {}", path.display()),
                                Err(error) => {
                                    format!("Could not write {}: {error}", path.display())
                                }
                            };
                        }
                    }
                }
                Ok(Evt::PresetIndexed { index, hash }) => {
                    if let Some(hash) = hash {
                        self.preset_hashes.insert(index, hash);
                    } else {
                        self.preset_hashes.remove(&index);
                    }
                }
                Ok(Evt::ForgetPresetHashes) => self.preset_hashes.clear(),
                Ok(Evt::SetlistRead(slots)) => {
                    for (index, (name, bytes)) in slots.iter().enumerate() {
                        if let Some(bytes) = bytes {
                            self.preset_hashes.insert(index, slot_hash(bytes));
                        } else if name.is_empty() {
                            self.preset_hashes.remove(&index);
                        }
                    }
                    self.captured_setlists.push(slots);
                }
                Ok(Evt::Auditioning(key)) => self.audition_events.push(key),
                Ok(Evt::Guarded {
                    path,
                    preset_hashes,
                    facts,
                }) => {
                    self.facts = facts;
                    // The bundle is written once and published whole, so the
                    // folder's time is when the backup was taken.
                    self.rollback_time = std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok();
                    self.rollback = Some(path);
                    self.preset_hashes = preset_hashes;
                    self.working = None;
                    self.status.clear();
                }
                Ok(Evt::History { undo, redo }) => {
                    self.undo_depth = undo;
                    self.redo_depth = redo;
                }
                Ok(Evt::Busy(busy)) => self.busy = busy,
                Ok(Evt::Progress(line)) => {
                    self.failed = false;
                    self.status = line;
                }
                Ok(Evt::Working { what, progress }) => {
                    self.working = Some((what, progress.clamp(0.0, 1.0)));
                }
                Ok(Evt::Success(line)) => {
                    self.working = None;
                    self.failed = false;
                    self.status = line;
                }
                Ok(Evt::Failed(line)) => {
                    self.working = None;
                    self.failed = true;
                    self.status = line;
                }
                Ok(Evt::ReadOnly(reason)) => self.read_only = Some(reason),
                Ok(Evt::Disconnected) => {
                    if self.active {
                        self.online = false;
                        self.status = "StompStation PRO disconnected".into();
                    }
                    self.rollback = None;
                    self.rollback_time = None;
                    self.read_only = None;
                    self.preset_hashes.clear();
                    self.working = None;
                    self.typing = None;
                    self.facts = LibraryFacts::default();
                    self.update_mode = false;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.online = false;
                    self.failed = true;
                    self.status = "StompStation PRO worker stopped".into();
                    break;
                }
            }
        }
    }

    fn install_snapshot(&mut self, snapshot: Snapshot, baseline: bool) {
        self.drafts.clear();
        for (path, description) in snapshot.app.iter().chain(&snapshot.settings) {
            if let Some(value) = &description.value {
                self.drafts.insert(path.to_string(), value.clone());
            }
        }
        if baseline {
            self.saved_drafts = self
                .drafts
                .iter()
                .filter(|(path, _)| path.starts_with("root\\app\\"))
                .map(|(path, value)| (path.clone(), value.clone()))
                .collect();
        }
        self.recompute_dirty();
        self.names.clear();
        for library in &snapshot.libraries {
            for (index, name) in library.info.names.iter().enumerate() {
                if let Some(name) = name {
                    self.names.insert((library.library, index), name.to_owned());
                }
            }
        }
        self.save_name = snapshot.active_preset.clone().unwrap_or_default();
        let groups = app_groups(&snapshot);
        if self.selected_group != INPUT_GROUP
            && self.selected_group != "output"
            && !groups.contains(&self.selected_group)
        {
            self.selected_group = groups.first().cloned().unwrap_or_default();
        }
        self.snapshot = Some(snapshot);
    }

    fn recompute_dirty(&mut self) {
        self.dirty = drafts_differ(&self.saved_drafts, &self.drafts);
    }

    /// Whether a firmware update has the pedal, or it is in Update Mode.
    pub(crate) fn updating(&self) -> bool {
        self.update_mode || self.firmware.is_some()
    }

    pub(crate) fn is_online(&self) -> bool {
        self.online
    }

    pub(crate) fn device_name(&self) -> &str {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.identity.name.as_str())
            .unwrap_or("")
    }

    pub(crate) fn firmware(&self) -> &str {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.identity.version.as_str())
            .unwrap_or("")
    }

    pub(crate) fn take_library_documents(&mut self) -> Vec<(String, Vec<u8>, bool)> {
        std::mem::take(&mut self.library_documents)
    }

    pub(crate) fn take_captured_setlists(&mut self) -> Vec<Vec<(String, Option<Vec<u8>>)>> {
        std::mem::take(&mut self.captured_setlists)
    }

    /// Whether the sidebar asked to keep the whole pedal, once.
    pub(crate) fn take_capture_asked(&mut self) -> bool {
        std::mem::take(&mut self.capture_asked)
    }

    /// Whether the firmware update asked for the Pedal page, once.
    pub(crate) fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.page_request)
    }

    pub(crate) fn reconnect(&mut self) {
        if !self.online && !self.updating() {
            self.failed = false;
            self.status = "Looking for a StompStation PRO…".into();
        }
        self.ask_connect();
    }

    /// Ask the worker to look for the pedal, and remember that it is.
    fn ask_connect(&mut self) {
        self.connecting = true;
        let _ = self.tx.send(Cmd::Connect);
    }

    /// Whether the connect page may watch USB for a StompStation PRO: none
    /// is connected, no look is in flight, and no firmware update has it.
    pub(crate) fn free_to_look(&self) -> bool {
        !self.online && !self.connecting && !self.updating()
    }

    /// Whether TonePush is still looking for a StompStation PRO.
    pub(crate) fn looking(&self) -> bool {
        !self.online && !self.failed && !self.updating()
    }

    pub(crate) fn send_tone(&self, index: usize, name: String, bytes: Vec<u8>) {
        let _ = self.tx.send(Cmd::ImportBytes { index, name, bytes });
    }

    pub(crate) fn push_setlist(&self, slots: Vec<PresetSlotWrite>) {
        let _ = self.tx.send(Cmd::PushSetlist(slots));
    }

    pub(crate) fn preset_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .libraries
                    .iter()
                    .find(|state| state.library == Library::Presets)
            })
            .map_or(0, |presets| presets.info.count)
    }

    pub(crate) fn audition(&self, key: i64, name: String, bytes: Vec<u8>) {
        let _ = self.tx.send(Cmd::Audition { key, name, bytes });
    }

    pub(crate) fn end_audition(&self) {
        let _ = self.tx.send(Cmd::EndAudition);
    }

    pub(crate) fn keep_audition(&self) {
        let _ = self.tx.send(Cmd::KeepAudition);
    }

    pub(crate) fn take_audition_events(&mut self) -> Vec<Option<i64>> {
        std::mem::take(&mut self.audition_events)
    }

    pub(crate) fn preview(&mut self, name: String, bytes: &[u8]) -> Result<(), String> {
        let mut snapshot = self
            .snapshot
            .clone()
            .ok_or_else(|| "connect a StompStation PRO to inspect its preset schema".to_owned())?;
        let preset = Preset::parse(bytes).map_err(|error| error.to_string())?;
        for record in preset.records() {
            let Some((_, description)) = snapshot
                .app
                .iter_mut()
                .find(|(path, _)| path.as_str() == record.subject())
            else {
                continue;
            };
            if let Some(value) = record.value().get("value") {
                description.value = Some(value.clone());
            }
        }
        snapshot.active_preset = Some(name.clone());
        self.preview = Some((name, snapshot));
        Ok(())
    }

    /// What the PRO holds, slot by slot: the hash of each preset read off
    /// it, and every slot's name, empty for an empty slot.
    pub(crate) fn pedal_slots(&self) -> Option<(BTreeMap<i64, String>, Vec<String>)> {
        let names = self
            .snapshot
            .as_ref()?
            .libraries
            .iter()
            .find(|state| state.library == Library::Presets)?
            .info
            .names
            .iter()
            .map(|name| name.clone().unwrap_or_default())
            .collect();
        let hashes = self
            .preset_hashes
            .iter()
            .map(|(slot, hash)| (*slot as i64, hash.clone()))
            .collect();
        Some((hashes, names))
    }

    /// Read every preset on the PRO into the library as a setlist.
    pub(crate) fn capture_setlist(&self) {
        let _ = self.tx.send(Cmd::CaptureSetlist);
    }

    pub(crate) fn tone_sync(&self, hash: &str, name: &str) -> theme::Sync {
        if self.preset_hashes.values().any(|known| known == hash) {
            return theme::Sync::Same;
        }
        let named_slot = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .libraries
                    .iter()
                    .find(|state| state.library == Library::Presets)
            })
            .and_then(|presets| {
                presets.info.names.iter().position(|known| {
                    known
                        .as_deref()
                        .is_some_and(|known| known.eq_ignore_ascii_case(name))
                })
            });
        match named_slot {
            Some(index) if self.preset_hashes.contains_key(&index) => theme::Sync::Differs,
            Some(_) => theme::Sync::Unknown,
            None => theme::Sync::Absent,
        }
    }

    /// The loaded preset's name, and whether the library holds it as it is.
    pub(crate) fn loaded(&self, lookup: &LibraryLookup) -> Option<(String, theme::Sync)> {
        let snapshot = self.snapshot.as_ref()?;
        let index = active_preset_index(snapshot)?;
        let name = snapshot.active_preset.clone()?;
        Some((name, self.slot_sync(index, lookup)))
    }

    fn slot_sync(&self, index: usize, lookup: &LibraryLookup) -> theme::Sync {
        if let Some(hash) = self.preset_hashes.get(&index) {
            let name = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .libraries
                        .iter()
                        .find(|state| state.library == Library::Presets)
                })
                .and_then(|presets| presets.info.names.get(index))
                .and_then(Option::as_deref)
                .unwrap_or_default();
            return lookup.sync(hash, name);
        }
        let name = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .libraries
                    .iter()
                    .find(|state| state.library == Library::Presets)
            })
            .and_then(|presets| presets.info.names.get(index))
            .and_then(Option::as_deref)
            .unwrap_or_default();
        if name.is_empty() || lookup.names.contains(&name.trim().to_ascii_lowercase()) {
            theme::Sync::Unknown
        } else {
            theme::Sync::Absent
        }
    }

    /// Load another preset, asking first when that would throw away edits:
    /// the pedal rebuilds its live tree from the stored preset, and the
    /// worker forgets the undo history with it.
    fn select_preset(&mut self, index: usize) {
        if !self.dirty {
            let _ = self.tx.send(Cmd::SelectPreset(index));
            return;
        }
        let name = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| preset_name(snapshot, index))
            .unwrap_or_default();
        self.confirmation = Some(Confirmation {
            question: format!(
                "This preset has changes that are not saved. Load {} {name} and lose them?",
                slot_label(index)
            ),
            action: "Discard and load",
            command: Cmd::SelectPreset(index),
        });
    }

    /// Selecting the already active slot makes the pedal rebuild its live app
    /// tree from the stored preset. That is the protocol's native discard.
    fn discard_changes(&self, snapshot: &Snapshot) {
        if self.online && self.dirty {
            if let Some(index) = active_preset_index(snapshot) {
                let _ = self.tx.send(Cmd::SelectPreset(index));
            }
        }
    }

    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|memory| memory.focused().is_some()) || ctx.any_popup_open() {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        const REDO: KeyboardShortcut =
            KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
        const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let pressed =
            |shortcut: &KeyboardShortcut| ctx.input_mut(|input| input.consume_shortcut(shortcut));
        if pressed(&REDO) {
            if self.online && self.redo_depth > 0 {
                let _ = self.tx.send(Cmd::Redo);
            }
        } else if pressed(&UNDO) && self.online && self.undo_depth > 0 {
            let _ = self.tx.send(Cmd::Undo);
        }
        if pressed(&SAVE)
            && self.dirty
            && self.rollback.is_some()
            && !self.save_name.trim().is_empty()
        {
            let _ = self.tx.send(Cmd::SavePreset(self.save_name.trim().into()));
        }

        if self.online && self.confirmation.is_none() {
            let direction = ctx.input_mut(|input| {
                if input.modifiers != Modifiers::NONE {
                    return 0;
                }
                if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    -1
                } else if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    1
                } else {
                    0
                }
            });
            if let Some(index) = adjacent_occupied_preset(self.snapshot.as_ref(), direction) {
                self.select_preset(index);
            }
        }
    }

    /// The Edit page for a PRO: the strip that says saving waits, when it
    /// does, the board, and the pane.
    pub(crate) fn body(&mut self, root: &mut egui::Ui, tier: theme::Tier, note: &LibraryNote) {
        if self.update_mode || (self.firmware.is_some() && !self.online) {
            self.paused_strip(root);
        }
        let Some(snapshot) = self.snapshot.clone() else {
            let status = self.status.clone();
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::bg()))
                .show(root, |ui| {
                    let rect = ui.max_rect();
                    crate::pane::centred(
                        ui,
                        status,
                        theme::regular(theme::BODY),
                        theme::muted(),
                        egui::pos2(rect.center().x, rect.top() + 72.0),
                        rect.width() - 40.0,
                    );
                });
            return;
        };
        if self.unlock_offer().is_some() {
            self.unlock_strip(root);
        }
        self.board(root, &snapshot, tier);
        self.pane(root, &snapshot, tier, note);
    }

    /// Whether saving waits for a backup the deck can take, and whether it
    /// can be taken now.
    pub(crate) fn unlock_offer(&self) -> Option<bool> {
        (self.online
            && self.snapshot.is_some()
            && self.rollback.is_none()
            && self.read_only.is_none())
        .then_some(!self.busy && self.working.is_none())
    }

    /// Back the pedal up where TonePush keeps its own backups.
    pub(crate) fn back_up_here(&self) {
        let _ = self.tx.send(Cmd::BackupHere);
    }

    /// The questions and previews that float over whichever page shows.
    pub(crate) fn windows(&mut self, ctx: &egui::Context) {
        self.confirmation_window(ctx);
        self.preview_window(ctx);
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some((name, snapshot)) = self.preview.clone() else {
            return;
        };
        let tier = theme::Tier::now(ctx);
        let mut open = true;
        egui::Window::new(name)
            .open(&mut open)
            .default_width(900.0)
            .default_height(430.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("StompStation PRO preset · preview").color(theme::muted()));
                ui.add_space(6.0);
                let g = crate::board::Geometry::pro(tier);
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), g.top + g.tile_height + g.bottom),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(theme::RADIUS_CARD),
                    theme::bg_deep(),
                );
                let mut board = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                let _ = self.draw_board(&mut board, &snapshot, &g, tier, false);
                ui.add_space(4.0);
                self.preview_pane(ui, &snapshot, tier);
            });
        if !open {
            self.preview = None;
        }
    }

    fn import_into_slot(&mut self, state: &LibraryState, index: usize) {
        let library = state.library;
        let label = if library == Library::Irs {
            "WAV impulse response"
        } else {
            "NAM model"
        };
        let Some(file) = rfd::FileDialog::new()
            .add_filter(label, &[library.extension()])
            .pick_file()
        else {
            return;
        };
        self.import_file(state, index, file);
    }

    /// Import a file into a slot, asking first when that replaces something.
    fn import_file(&mut self, state: &LibraryState, index: usize, file: PathBuf) {
        let library = state.library;
        let name = file
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Imported")
            .to_owned();

        let (command, targets) = if library == Library::Irs {
            let read = std::fs::read(&file)
                .map_err(|error| format!("Could not read {}: {error}", file.display()))
                .and_then(|bytes| {
                    let wav = voidx_client::ir::Wav::parse(&bytes).map_err(|e| e.to_string())?;
                    let channels = wav
                        .device_blobs(state.info.size)
                        .map_err(|e| e.to_string())?
                        .len();
                    Ok(channels)
                });
            match read.and_then(|channels| ir_import_targets(channels, index, state.info.count)) {
                Ok(targets) if targets.len() == 1 => (
                    Cmd::Import {
                        library,
                        index,
                        name,
                        file,
                    },
                    targets,
                ),
                Ok(targets) => (
                    Cmd::ImportStereo {
                        left: targets[0],
                        right: targets[1],
                        name,
                        file,
                    },
                    targets,
                ),
                Err(error) => {
                    self.status = error;
                    return;
                }
            }
        } else {
            (
                Cmd::Import {
                    library,
                    index,
                    name,
                    file,
                },
                vec![index],
            )
        };

        let occupied = targets
            .iter()
            .filter(|&&slot| state.info.names.get(slot).is_some_and(Option::is_some))
            .count();
        if occupied > 0 {
            let slots = targets
                .iter()
                .map(|slot| (slot + 1).to_string())
                .collect::<Vec<_>>()
                .join(" and ");
            self.confirmation = Some(Confirmation {
                action: "Write to pedal",
                question: format!(
                    "Replace {occupied} occupied {} slot(s) at {slots}? The automatic backup remains available.",
                    library.title()
                ),
                command,
            });
        } else {
            let _ = self.tx.send(command);
        }
    }

    fn confirmation_window(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let question = confirmation.question.clone();
        let action = confirmation.action;
        let mut confirm = false;
        let mut cancel = false;
        let note = if matches!(confirmation.command, Cmd::SelectPreset(_)) {
            "The edits play on the pedal until another preset loads."
        } else {
            "This writes the pedal's memory. The automatic backup can put it back."
        };
        let (_, close) = theme::dialog(ctx, "pro-confirm", 460.0, |ui| {
            theme::dialog_header(ui, &question, None);
            theme::dialog_footer(ui, note, |ui| {
                confirm = theme::Button::new(action).danger().show(ui).clicked();
                cancel = theme::Button::new("Cancel").show(ui).clicked();
            });
        });
        cancel |= close && !confirm;
        if confirm {
            if let Some(confirmation) = self.confirmation.take() {
                let _ = self.tx.send(confirmation.command);
            }
            if let Some(next) = self.after_confirmation.take() {
                let _ = self.tx.send(next);
            }
        } else if cancel {
            self.confirmation = None;
            self.after_confirmation = None;
        }
    }
}

fn app_group(path: &str) -> Option<String> {
    path.strip_prefix("root\\app\\")?
        .split('\\')
        .next()
        .filter(|group| !group.is_empty())
        .map(str::to_owned)
}

fn settings_group(path: &str) -> Option<&str> {
    path.strip_prefix("root\\settings\\")?
        .split('\\')
        .next()
        .filter(|group| !group.is_empty())
}

fn node_matches_search(path: &str, description: &NodeDescription, needle: &str) -> bool {
    needle.is_empty()
        || path.to_ascii_lowercase().contains(needle)
        || description
            .desc
            .as_deref()
            .is_some_and(|desc| desc.to_ascii_lowercase().contains(needle))
}

fn slot_matches_search(index: usize, name: Option<&str>, needle: &str) -> bool {
    needle.is_empty()
        || (index + 1).to_string().contains(needle)
        || name
            .unwrap_or("empty slot")
            .to_ascii_lowercase()
            .contains(needle)
}

fn ir_import_targets(channels: usize, start: usize, count: usize) -> Result<Vec<usize>, String> {
    match channels {
        1 if start < count => Ok(vec![start]),
        2 if start + 1 < count => Ok(vec![start, start + 1]),
        2 => Err(format!(
            "A stereo IR needs two adjacent slots; slot {} is the last slot",
            start + 1
        )),
        channels => Err(format!(
            "PRO IR import does not support {channels} channels"
        )),
    }
}

fn node_label(path: &NodePath, description: &NodeDescription) -> String {
    description.desc.clone().unwrap_or_else(|| {
        path.as_str()
            .rsplit('\\')
            .next()
            .unwrap_or(path.as_str())
            .to_owned()
    })
}

fn preset_name(snapshot: &Snapshot, index: usize) -> Option<String> {
    snapshot
        .libraries
        .iter()
        .find(|state| state.library == Library::Presets)?
        .info
        .names
        .get(index)?
        .clone()
}

fn active_preset_index(snapshot: &Snapshot) -> Option<usize> {
    let active = snapshot.active_preset.as_deref()?;
    snapshot
        .libraries
        .iter()
        .find(|state| state.library == Library::Presets)?
        .info
        .names
        .iter()
        .position(|name| name.as_deref() == Some(active))
}

fn adjacent_occupied_preset(snapshot: Option<&Snapshot>, direction: i32) -> Option<usize> {
    if direction == 0 {
        return None;
    }
    let snapshot = snapshot?;
    let presets = snapshot
        .libraries
        .iter()
        .find(|state| state.library == Library::Presets)?;
    let current = active_preset_index(snapshot)?;
    if direction < 0 {
        (0..current)
            .rev()
            .find(|&index| presets.info.names.get(index).is_some_and(Option::is_some))
    } else {
        ((current + 1)..presets.info.count)
            .find(|&index| presets.info.names.get(index).is_some_and(Option::is_some))
    }
}

fn drafts_differ(saved: &BTreeMap<String, Value>, current: &BTreeMap<String, Value>) -> bool {
    saved
        .iter()
        .any(|(path, saved)| current.get(path).is_none_or(|value| value != saved))
}

fn app_groups(snapshot: &Snapshot) -> Vec<String> {
    let mut groups = Vec::new();
    for (path, description) in &snapshot.app {
        let Some(group) = app_group(path.as_str()) else {
            continue;
        };
        if matches!(group.as_str(), "preset" | "output")
            || !description
                .value
                .as_ref()
                .is_some_and(|value| editable_node(path.as_str(), description, value))
            || groups.contains(&group)
        {
            continue;
        }
        groups.push(group);
    }
    groups
}

/// Whether one advertised node is a real user control.
///
/// Folder/module/link items also have JSON values, which made a generic form
/// render headings such as “Amp” and “Settings” as controls. VoidX reserves
/// underscore-prefixed names for read-only values, so those stay out of every
/// editor and out of the set of candidate writes.
fn editable_node(path: &str, description: &NodeDescription, value: &Value) -> bool {
    !value.is_null()
        && !path.split('\\').any(|segment| segment.starts_with('_'))
        // `ctl1` and `ctl2` are MIDI/controller assignment modules. Their
        // unlinked implementation fields (Source, Address and six identical
        // Min/Max pairs) are not sound parameters and read as corrupt knobs in
        // the normal block editor. They belong in a dedicated assignment UI.
        && !path
            .split('\\')
            .any(|segment| matches!(segment, "ctl1" | "ctl2"))
        && description.item_type.is_none()
        && description.validate_value(value).is_ok()
        && matches!(
            description.kind,
            Some(
                NodeKind::Float
                    | NodeKind::Enum
                    | NodeKind::PropertyList
                    | NodeKind::Array
                    | NodeKind::Item
            )
        )
}

fn friendly_group(group: &str) -> String {
    match group {
        INPUT_GROUP => "Input".into(),
        "gate" => "Noise Gate".into(),
        "pitch" => "Pitch".into(),
        "exp" => "Expression".into(),
        "comp" => "Compressor".into(),
        "mod_pre" => "Pre Mod".into(),
        "drive" => "Drive".into(),
        "amp" => "Amp".into(),
        "ir" => "Cab / IR".into(),
        "eq" => "EQ".into(),
        "mod" => "Modulation".into(),
        "delay" => "Delay".into(),
        "reverb" => "Reverb".into(),
        "output" => "Master".into(),
        other => {
            let mut characters = other.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect(),
                None => String::new(),
            }
        }
    }
}

fn block_selector(snapshot: &Snapshot, group: &str) -> Option<(NodePath, NodeDescription)> {
    snapshot
        .app
        .iter()
        .find(|(path, description)| {
            app_group(path.as_str()).as_deref() == Some(group)
                && (matches!(description.kind, Some(NodeKind::PropertyList))
                    && description.reference.is_some()
                    || matches!(description.kind, Some(NodeKind::Enum))
                        && description
                            .desc
                            .as_deref()
                            .is_some_and(|description| description.eq_ignore_ascii_case("mode")))
        })
        .cloned()
}

fn block_model_name(snapshot: &Snapshot, group: &str) -> Option<String> {
    if let Some((_, description)) = block_selector(snapshot, group) {
        if let Some(value) = description.value.as_ref() {
            return Some(value_text(value));
        }
    }
    None
}

fn selector_choices(snapshot: &Snapshot, description: &NodeDescription) -> Vec<Value> {
    if let Some(reference) = description.reference.as_deref() {
        return snapshot
            .libraries
            .iter()
            .find(|library| library.info.path.as_str() == reference)
            .map(|library| {
                library
                    .info
                    .names
                    .iter()
                    .flatten()
                    .cloned()
                    .map(Value::String)
                    .collect()
            })
            .unwrap_or_default();
    }
    description
        .options
        .as_ref()
        .or(description.items.as_ref())
        .cloned()
        .unwrap_or_default()
}

fn block_enabled(snapshot: &Snapshot, group: &str) -> bool {
    snapshot
        .app
        .iter()
        .find(|(path, description)| {
            app_group(path.as_str()).as_deref() == Some(group)
                && path
                    .as_str()
                    .rsplit('\\')
                    .next()
                    .is_some_and(|name| matches!(name, "on_off" | "enable" | "on"))
                && description.value.is_some()
        })
        .and_then(|(_, description)| description.value.as_ref())
        .is_none_or(|value| match value {
            Value::Bool(on) => *on,
            Value::Number(number) => number.as_f64().is_none_or(|number| number != 0.0),
            Value::String(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "off" | "disabled" | "false" | "0"
            ),
            _ => true,
        })
}

fn group_category(group: &str) -> &'static str {
    match group {
        INPUT_GROUP => "Input",
        "gate" | "comp" => "Dynamics",
        "pitch" => "Pitch/Synth",
        "exp" => "Wah",
        "mod_pre" | "mod" => "Modulation",
        "drive" => "Distortion",
        "amp" => "Amp",
        "ir" => "IR",
        "eq" => "EQ",
        "delay" => "Delay",
        "reverb" => "Reverb",
        "output" => "Output",
        _ => "",
    }
}

/// TonePush Cloud uses the HX catalog's stable numeric category ids for every
/// device. Keeping that wire vocabulary here lets the website draw a PRO tone
/// with the exact same icons, labels, and colours as an HX tone.
fn group_category_id(group: &str) -> Option<u32> {
    match group {
        "drive" => Some(1),
        "gate" | "comp" => Some(2),
        "eq" => Some(3),
        "mod_pre" | "mod" => Some(4),
        "delay" => Some(5),
        "reverb" => Some(6),
        "pitch" => Some(7),
        "exp" => Some(9),
        "amp" => Some(11),
        "ir" => Some(14),
        _ => None,
    }
}

/// A native PRO preset's chain as the strip of category colours the library
/// draws: its blocks in the pedal's fixed order.
pub(crate) fn preset_minis(bytes: &[u8]) -> Vec<crate::shell::Mini> {
    preset_blocks(bytes)
        .iter()
        .map(|block| crate::shell::Mini::Block {
            colour: block
                .get("category")
                .and_then(Value::as_u64)
                .and_then(|id| theme::category_name(id as u32))
                .map_or_else(theme::muted, theme::category_colour),
            on: block
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        })
        .collect()
}

/// A short library-table reading of a native PRO preset.
pub(crate) fn preset_content(bytes: &[u8]) -> Option<String> {
    let preset = Preset::parse(bytes).ok()?;
    let enabled = |group: &str| {
        preset.records().iter().find_map(|record| {
            (record.subject() == format!("root\\app\\{group}\\on_off"))
                .then(|| record.value().get("value"))
                .flatten()
                .map(value_is_on)
        })
    };
    let amp = enabled("amp").unwrap_or(true);
    let cab = enabled("ir").unwrap_or(true);
    Some(
        match (amp, cab) {
            (true, true) => "Full rig",
            (true, false) => "Amp, no cab",
            (false, true) => "Cab and effects",
            (false, false) => "Effects only",
        }
        .to_owned(),
    )
}

/// Device-neutral block metadata for TonePush Cloud. The server receives the
/// same shape for HX and PRO tones even though each adapter discovers its model
/// name differently.
pub(crate) fn preset_blocks(bytes: &[u8]) -> Vec<Value> {
    let Ok(preset) = Preset::parse(bytes) else {
        return Vec::new();
    };
    let mut groups: BTreeMap<String, (bool, Option<String>)> = BTreeMap::new();
    for record in preset.records() {
        let Some(group) = app_group(record.subject()) else {
            continue;
        };
        let leaf = record.subject().rsplit('\\').next().unwrap_or_default();
        let value = record.value().get("value");
        let entry = groups.entry(group).or_insert((true, None));
        if leaf == "on_off" {
            if let Some(value) = value {
                entry.0 = value_is_on(value);
            }
        } else if matches!(leaf, "model" | "mode" | "md" | "ir") {
            if let Some(value) = value {
                entry.1 = Some(value_text(value));
            }
        }
    }
    // A PRO has one fixed serial path. The group name identifies a block on
    // the device, not a separate signal path; publishing it as `path` made the
    // website draw one Input -> Block -> Output board per block. This is the
    // same physical order used by the editor, with Output represented by the
    // shared endpoint rather than a thirteenth block.
    const CHAIN: &[&str] = &[
        "gate", "pitch", "exp", "comp", "mod_pre", "drive", "amp", "ir", "eq", "mod", "delay",
        "reverb",
    ];
    CHAIN
        .iter()
        .filter_map(|group| {
            let (enabled, model) = groups.remove(*group)?;
            let category = group_category_id(group)?;
            serde_json::json!({
                "name": model.unwrap_or_else(|| friendly_group(group)),
                "category": category,
                "enabled": enabled,
                "path": 0,
            })
            .into()
        })
        .collect()
}

fn value_is_on(value: &Value) -> bool {
    match value {
        Value::Bool(on) => *on,
        Value::Number(number) => number.as_f64().is_none_or(|number| number != 0.0),
        Value::String(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "disabled" | "false" | "0"
        ),
        _ => true,
    }
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn json_number(number: f64) -> Option<Value> {
    serde_json::Number::from_f64(number).map(Value::Number)
}

fn format_node_value(description: &NodeDescription, value: &Value) -> String {
    let Some(number) = value.as_f64() else {
        return value_text(value);
    };
    // As many decimals as the node's step has: a step of 0.1 reads 6.0,
    // of 1 reads 6. Without a step, two at most and no trailing zeros.
    let reading = match description.step.filter(|step| *step > 0.0) {
        Some(step) => {
            let decimals = (-step.log10()).ceil().clamp(0.0, 2.0) as usize;
            format!("{number:.decimals$}")
        }
        None => format!("{number:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned(),
    };
    description
        .unit
        .as_deref()
        .map(|unit| format!("{reading} {unit}"))
        .unwrap_or(reading)
}

fn toggle_choices(description: &NodeDescription) -> Option<(Value, Value)> {
    let choices = description
        .options
        .as_ref()
        .or(description.items.as_ref())?;
    if choices.len() != 2 {
        return None;
    }
    let is_off = |value: &Value| match value {
        Value::Bool(value) => !value,
        Value::Number(value) => value.as_i64() == Some(0),
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "disabled" | "false" | "no"
        ),
        _ => false,
    };
    if is_off(&choices[0]) {
        Some((choices[0].clone(), choices[1].clone()))
    } else if is_off(&choices[1]) {
        Some((choices[1].clone(), choices[0].clone()))
    } else {
        None
    }
}

fn sanitise(name: &str) -> String {
    let name = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let name = name.trim();
    if name.is_empty() {
        "slot".into()
    } else {
        name.into()
    }
}

struct Worker {
    commands: Receiver<Cmd>,
    events: Sender<Evt>,
    ctx: egui::Context,
    device: Option<Device<SerialLink>>,
    rollback: Option<ArmedRollback>,
    rollback_path: Option<PathBuf>,
    audition_original: Vec<(NodePath, NodeDescription, Value)>,
    history: Vec<Vec<NodeEdit>>,
    future: Vec<Vec<NodeEdit>>,
    last_edit_at: Option<Instant>,
    /// The firmware an update writes, once checked.
    firmware_image: Option<voidx_client::firmware::Image>,
    /// The backup the update stands on.
    firmware_backup: Option<PathBuf>,
    /// The device is a pedal in Update Mode: nothing but the update goes to
    /// it.
    update_mode: bool,
    /// What the worker looks for between commands while an update waits.
    awaiting: Option<firmware::Awaiting>,
    last_seen: Option<firmware::Seen>,
    /// A device in the pedal's place that said it is something else.
    refused_port: Option<String>,
}

#[derive(Clone)]
struct NodeEdit {
    path: NodePath,
    description: NodeDescription,
    before: Value,
    after: Value,
}

impl Worker {
    fn new(commands: Receiver<Cmd>, events: Sender<Evt>, ctx: egui::Context) -> Self {
        Self {
            commands,
            events,
            ctx,
            device: None,
            rollback: None,
            rollback_path: None,
            audition_original: Vec::new(),
            history: Vec::new(),
            future: Vec::new(),
            last_edit_at: None,
            firmware_image: None,
            firmware_backup: None,
            update_mode: false,
            awaiting: None,
            last_seen: None,
            refused_port: None,
        }
    }

    fn run(mut self) {
        let mut pending = None;
        loop {
            let command = match pending.take() {
                Some(command) => command,
                // While an update waits for the pedal, look for it between
                // commands.
                None if self.awaiting.is_some() => {
                    match self.commands.recv_timeout(firmware::POLL) {
                        Ok(command) => command,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            self.poll();
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match self.commands.recv() {
                    Ok(command) => command,
                    Err(_) => break,
                },
            };
            let (command, next) = self.coalesce_live_edits(command);
            pending = next;
            let looking = matches!(command, Cmd::Connect);
            let show_busy = !matches!(
                &command,
                Cmd::SetNode {
                    persistent: false,
                    ..
                }
            );
            if show_busy {
                self.send(Evt::Busy(true));
            }
            if let Err(error) = self.handle(command) {
                let lost = matches!(&error, WorkError::Device(error) if error.loses_session());
                self.send(Evt::Failed(error.to_string()));
                if lost {
                    self.device = None;
                    self.rollback = None;
                    self.rollback_path = None;
                    // The live values an audition replaced belong to this
                    // session; writing them into the next one would overwrite
                    // whatever that pedal has loaded.
                    self.audition_original.clear();
                    self.send(Evt::Disconnected);
                }
            }
            if show_busy {
                self.send(Evt::Busy(false));
            }
            if looking {
                self.send(Evt::Looked);
            }
        }
    }

    /// A knob can produce several frame-rate changes while one serial request
    /// is in flight. Preserve the first value for Undo and send only the most
    /// recent adjacent value for that node; unrelated commands keep their
    /// exact order.
    fn coalesce_live_edits(&self, mut command: Cmd) -> (Cmd, Option<Cmd>) {
        loop {
            let Ok(next) = self.commands.try_recv() else {
                return (command, None);
            };
            match (&mut command, next) {
                (
                    Cmd::SetNode {
                        path,
                        description,
                        value,
                        persistent: false,
                        ..
                    },
                    Cmd::SetNode {
                        path: next_path,
                        description: next_description,
                        value: next_value,
                        persistent: false,
                        ..
                    },
                ) if *path == next_path => {
                    *description = next_description;
                    *value = next_value;
                }
                (_, next) => return (command, Some(next)),
            }
        }
    }

    fn send(&self, event: Evt) {
        if self.events.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }

    fn handle(&mut self, command: Cmd) -> WorkResult<()> {
        if !matches!(
            &command,
            Cmd::SetNode {
                persistent: false,
                ..
            }
        ) {
            self.last_edit_at = None;
        }
        match command {
            // An update waiting for the pedal, or holding it in Update
            // Mode, owns it: looking for it again would take it away.
            Cmd::Connect if self.awaiting.is_some() || self.update_mode => Ok(()),
            Cmd::Connect => self.connect(),
            Cmd::Disconnect => {
                if !self.update_mode {
                    self.restore_audition()?;
                }
                self.device = None;
                self.update_mode = false;
                self.awaiting = None;
                self.rollback = None;
                self.rollback_path = None;
                self.forget_history();
                self.send(Evt::Disconnected);
                Ok(())
            }
            Cmd::Undo => self.step_history(true),
            Cmd::Redo => self.step_history(false),
            Cmd::SelectPreset(index) => {
                self.restore_audition()?;
                self.send(Evt::Auditioning(None));
                let device = self.device()?;
                device.select_preset(index)?;
                self.forget_history();
                self.refresh(true)
            }
            Cmd::ReadPreset { index, target } => {
                let list = self.list(Library::Presets)?;
                let name = list
                    .names
                    .get(index)
                    .and_then(Option::as_ref)
                    .cloned()
                    .ok_or_else(|| {
                        WorkError::Other(format!("preset slot {} is empty", index + 1))
                    })?;
                let blob = self.device()?.read_blob(&list, index)?;
                let bytes = export_blob(Library::Presets, &blob, list.size)?;
                self.send(Evt::PresetRead {
                    index,
                    name,
                    bytes,
                    target,
                });
                Ok(())
            }
            Cmd::CaptureSetlist => {
                let list = self.list(Library::Presets)?;
                let mut slots = Vec::with_capacity(list.count);
                for index in 0..list.count {
                    let Some(name) = list.names.get(index).and_then(Option::as_ref).cloned() else {
                        slots.push((String::new(), None));
                        continue;
                    };
                    self.send(Evt::Progress(format!(
                        "Reading preset {} of {}",
                        index + 1,
                        list.count
                    )));
                    let blob = self.device()?.read_blob(&list, index)?;
                    let bytes = export_blob(Library::Presets, &blob, list.size)?;
                    slots.push((name, Some(bytes)));
                }
                self.send(Evt::SetlistRead(slots));
                self.send(Evt::Success("Setlist ready to save".into()));
                Ok(())
            }
            Cmd::SetNode {
                path,
                description,
                before,
                value,
                persistent,
            } => {
                if persistent {
                    self.require_guard()?;
                }
                let device = self.device()?;
                if let Err(error) = device.write_node(path.clone(), &description, value.clone()) {
                    self.send(Evt::NodeValue {
                        path: path.to_string(),
                        value: before,
                    });
                    return Err(error.into());
                }
                self.send(Evt::NodeValue {
                    path: path.to_string(),
                    value: value.clone(),
                });
                if !persistent && !voidx_client::values_equivalent(&before, &value) {
                    self.record_edit(NodeEdit {
                        path,
                        description: *description,
                        before,
                        after: value,
                    });
                }
                Ok(())
            }
            Cmd::UseRollback(path) => {
                let bundle = backup::open_verified(&path)?;
                let preset_hashes = preset_hashes_from_bundle(&bundle)?;
                let facts = libraries::facts_of(&bundle);
                let device = self.device()?;
                let rollback = bundle.arm(device)?;
                device.enable_writes()?;
                self.rollback = Some(rollback);
                self.rollback_path = Some(path.clone());
                self.send(Evt::Guarded {
                    path,
                    preset_hashes,
                    facts,
                });
                Ok(())
            }
            Cmd::SavePreset(name) => {
                self.require_guard()?;
                self.device()?.save_preset(&name)?;
                self.refresh(true)?;
                self.send(Evt::ForgetPresetHashes);
                self.send(Evt::Success(format!("Saved preset {name}")));
                Ok(())
            }
            Cmd::Rename {
                library,
                index,
                name,
            } => {
                self.require_guard()?;
                let list = self.list(library)?;
                let device_name = unique_slot_name(&list, index, &name);
                self.refuse_orphan(&list, index, Some(&device_name))?;
                self.device()?.rename_slot(&list, index, &device_name)?;
                self.refresh(false)?;
                let suffix = if device_name != name {
                    format!(" as {device_name} because pedal names must be unique")
                } else {
                    String::new()
                };
                self.send(Evt::Success(format!(
                    "Renamed {} slot {}{suffix}",
                    library.title(),
                    index + 1
                )));
                Ok(())
            }
            Cmd::Move { library, from, to } => {
                self.require_guard()?;
                let list = self.list(library)?;
                self.device()?.move_slot(&list, from, to)?;
                self.refresh(false)?;
                if library == Library::Presets {
                    self.send(Evt::ForgetPresetHashes);
                }
                self.send(Evt::Success(format!(
                    "Moved {} slot {} to {}",
                    library.title(),
                    from + 1,
                    to + 1
                )));
                Ok(())
            }
            Cmd::Clear { library, index } => {
                self.require_guard()?;
                let list = self.list(library)?;
                self.refuse_orphan(&list, index, None)?;
                self.device()?.clear_slot(&list, index)?;
                self.refresh(false)?;
                if library == Library::Presets {
                    self.send(Evt::PresetIndexed { index, hash: None });
                }
                self.send(Evt::Success(format!(
                    "Cleared {} slot {}",
                    library.title(),
                    index + 1
                )));
                Ok(())
            }
            Cmd::Import {
                library,
                index,
                name,
                file,
            } => self.import(library, index, &name, &file),
            Cmd::ImportBytes { index, name, bytes } => {
                self.require_guard()?;
                let list = self.list(Library::Presets)?;
                let device_name = unique_slot_name(&list, index, &name);
                let blob = import_blob(Library::Presets, &bytes, list.size)?;
                let hash = preset_blob_hash(&blob)?;
                let events = self.events.clone();
                self.device()?
                    .write_blob(&list, index, &device_name, &blob, |step| {
                        if let Some(line) = upload_line(step) {
                            let _ = events.send(Evt::Progress(line));
                        }
                    })?;
                self.refresh(false)?;
                self.send(Evt::PresetIndexed {
                    index,
                    hash: Some(hash),
                });
                let message = if device_name == name {
                    format!("Wrote {name} to preset slot {}", index + 1)
                } else {
                    format!("Wrote {name} to preset slot {} as {device_name}", index + 1)
                };
                self.send(Evt::Success(message));
                Ok(())
            }
            Cmd::PushSetlist(slots) => {
                self.require_guard()?;
                let mut list = self.list(Library::Presets)?;
                for (index, tone) in slots {
                    if index >= list.count {
                        return Err(WorkError::Other(format!(
                            "preset slot {} is outside this pedal's {} slots",
                            index + 1,
                            list.count
                        )));
                    }
                    match tone {
                        Some((name, bytes)) => {
                            let device_name = unique_slot_name(&list, index, &name);
                            let blob = import_blob(Library::Presets, &bytes, list.size)?;
                            let hash = preset_blob_hash(&blob)?;
                            let events = self.events.clone();
                            self.device()?.write_blob(
                                &list,
                                index,
                                &device_name,
                                &blob,
                                |step| {
                                    if let Some(line) = upload_line(step) {
                                        let _ = events.send(Evt::Progress(format!(
                                            "Preset {}: {line}",
                                            index + 1
                                        )));
                                    }
                                },
                            )?;
                            list.names[index] = Some(device_name);
                            self.send(Evt::PresetIndexed {
                                index,
                                hash: Some(hash),
                            });
                        }
                        None if list.names.get(index).is_some_and(Option::is_some) => {
                            self.device()?.clear_slot(&list, index)?;
                            list.names[index] = None;
                            self.send(Evt::PresetIndexed { index, hash: None });
                        }
                        None => {}
                    }
                }
                self.refresh(false)?;
                self.send(Evt::Success("Setlist written to the pedal".into()));
                Ok(())
            }
            Cmd::Audition { key, name, bytes } => {
                self.restore_audition()?;
                let preset = Preset::parse(&bytes)?;
                let tree = self.device()?.browse(NodePath::new("root\\app")?)?;
                let mut changes = Vec::new();
                for record in preset.records() {
                    let Ok(path) = NodePath::new(record.subject()) else {
                        continue;
                    };
                    let Some(description) = tree.get(&path).cloned() else {
                        continue;
                    };
                    if !matches!(
                        description.kind,
                        Some(
                            NodeKind::Float
                                | NodeKind::Enum
                                | NodeKind::PropertyList
                                | NodeKind::Array
                        )
                    ) {
                        continue;
                    }
                    let Some(value) = record.value().get("value").cloned() else {
                        continue;
                    };
                    if description.validate_value(&value).is_err() {
                        continue;
                    }
                    let Some(original) = description.value.clone() else {
                        continue;
                    };
                    changes.push((path, description, original, value));
                }
                let mut written: Vec<(NodePath, NodeDescription, Value)> = Vec::new();
                for (path, description, original, value) in changes {
                    if let Err(error) = self.device()?.write_node(path.clone(), &description, value)
                    {
                        for (path, description, original) in written.into_iter().rev() {
                            let _ = self.device()?.write_node(path, &description, original);
                        }
                        return Err(error.into());
                    }
                    written.push((path, description, original));
                }
                self.audition_original = written;
                self.refresh(false)?;
                self.send(Evt::Auditioning(Some(key)));
                self.send(Evt::Success(format!("Auditioning {name}")));
                Ok(())
            }
            Cmd::EndAudition => {
                self.restore_audition()?;
                self.refresh(false)?;
                self.send(Evt::Auditioning(None));
                self.send(Evt::Success("Restored the previous edit buffer".into()));
                Ok(())
            }
            Cmd::KeepAudition => {
                let original = std::mem::take(&mut self.audition_original);
                let mut edits = Vec::with_capacity(original.len());
                for (path, description, before) in original {
                    let after = self.device()?.read_value(path.clone())?;
                    if before != after {
                        edits.push(NodeEdit {
                            path,
                            description,
                            before,
                            after,
                        });
                    }
                }
                self.record_transaction(edits);
                self.send(Evt::Auditioning(None));
                self.send(Evt::Success("Kept the audition in the edit buffer".into()));
                Ok(())
            }
            Cmd::Export {
                library,
                index,
                file,
            } => self.export(library, index, &file),
            Cmd::ImportStereo {
                left,
                right,
                name,
                file,
            } => self.import_stereo(left, right, &name, &file),
            Cmd::ExportStereo { left, right, file } => self.export_stereo(left, right, &file),
            Cmd::Backup(path) => {
                let captured = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| WorkError::Other(error.to_string()))?
                    .as_secs();
                self.capture_backup(&path, captured)?;
                self.send(Evt::Success(format!("Backup complete: {}", path.display())));
                Ok(())
            }
            Cmd::BackupHere => {
                self.back_up_automatically()?;
                self.send(Evt::Success("Backed up · saving is on".into()));
                Ok(())
            }
            Cmd::FirmwareLoad(path) => self.firmware_step(|worker| worker.firmware_load(&path)),
            Cmd::FirmwareBackup => self.firmware_step(Self::firmware_backup),
            Cmd::FirmwareFindBackup => self.firmware_step(Self::firmware_find_backup),
            Cmd::FirmwareAwait => self.firmware_step(Self::firmware_await),
            Cmd::FirmwareWrite { expect } => {
                self.firmware_step(|worker| worker.firmware_write(&expect))
            }
            Cmd::FirmwareCancel => self.firmware_cancel(),
            Cmd::Restore(path) => {
                self.require_guard()?;
                let source = backup::open_verified(&path)?;
                let restore_total = source
                    .manifest()
                    .lists
                    .iter()
                    .map(|list| list.count)
                    .sum::<usize>()
                    + source.schema().len();
                let restore_total = restore_total.max(1) as f32;
                let lists = source.manifest().lists.clone();
                let slot_offset = |path: &str, index: usize| {
                    lists
                        .iter()
                        .take_while(|list| list.path != path)
                        .map(|list| list.count)
                        .sum::<usize>()
                        + index
                };
                let rollback = self.rollback.take().expect("guard checked");
                let events = self.events.clone();
                let mut settings_done = 0usize;
                let slots_total = lists.iter().map(|list| list.count).sum::<usize>();
                let restored = backup::restore_armed(&source, &rollback, self.device()?, |step| {
                    let (line, progress) = match step {
                        backup::RestoreStep::Preflight => ("Restore preflight".into(), 0.0),
                        backup::RestoreStep::Writing {
                            path,
                            index,
                            upload,
                            ..
                        } => {
                            let within = upload_progress(upload);
                            (
                                format!("{path} slot {}: {upload:?}", index + 1),
                                (slot_offset(&path, index) as f32 + within) / restore_total,
                            )
                        }
                        backup::RestoreStep::Clearing { path, index } => (
                            format!("Clearing {path} slot {}", index + 1),
                            (slot_offset(&path, index) + 1) as f32 / restore_total,
                        ),
                        backup::RestoreStep::Setting { path } => {
                            settings_done += 1;
                            (
                                format!("Setting {path}"),
                                (slots_total + settings_done) as f32 / restore_total,
                            )
                        }
                        backup::RestoreStep::Done => ("Restore verified".into(), 1.0),
                    };
                    let _ = events.send(Evt::Working {
                        what: line,
                        progress,
                    });
                });
                self.rollback = Some(rollback);
                restored?;
                self.forget_history();
                self.refresh(true)?;
                self.send(Evt::ForgetPresetHashes);
                self.send(Evt::Success(format!(
                    "Restored {}; original rollback retained",
                    path.display()
                )));
                Ok(())
            }
        }
    }

    fn connect(&mut self) -> WorkResult<()> {
        let Some(found) = voidx_client::list()?.into_iter().next() else {
            // In Update Mode the pedal names itself a Raspberry Pi; it is
            // used only when it says it is a StompStation PRO in Update Mode.
            return self.connect_update_mode();
        };
        self.update_mode = false;
        let mut device = Device::connect(found.open()?)?;
        // Firmware TonePush has not been verified against still opens, read
        // only: browsing, exports and backups work, which is also what a pedal
        // needs before and after a firmware update.
        let read_only = match device.write_safety().clone() {
            WriteSafety::Verified => {
                device.enable_writes()?;
                None
            }
            WriteSafety::ReadOnly { reason } => {
                // Presets can still be picked and played with: only what
                // would reach the pedal's memory stays off.
                device.enable_live_edits()?;
                Some(reason)
            }
        };
        self.audition_original.clear();
        let snapshot = snapshot(&mut device)?;
        let can_refresh_backup = latest_verified_backup(&snapshot.identity).is_some();
        let version = snapshot.identity.version.clone();
        self.device = Some(device);
        self.forget_history();
        self.send(Evt::Connected(snapshot));
        if let Some(reason) = read_only {
            self.rollback = None;
            self.rollback_path = None;
            self.send(Evt::ReadOnly(reason.clone()));
            self.send(Evt::Success(format!(
                "Not verified for saving: {reason}. Presets, knobs, export and Backup work; nothing is written to the pedal's memory"
            )));
            return Ok(());
        }
        // Matching a rollback checks every slot's contents and global settings. It
        // is important, but it should not make a healthy pedal look absent
        // while it runs: publish the live editor first, then unlock writes.
        let guarded = self.device.as_mut().and_then(latest_matching_rollback);
        if let Some((path, rollback, preset_hashes, facts)) = guarded {
            self.rollback = Some(rollback);
            self.rollback_path = Some(path.clone());
            self.send(Evt::Guarded {
                path,
                preset_hashes,
                facts,
            });
        } else {
            self.rollback = None;
            self.rollback_path = None;
            if can_refresh_backup && hx_catalog::home::backups().is_some() {
                self.back_up_automatically()?;
            } else {
                self.send(Evt::Success(
                    "Live editing ready · back up to unlock saving".into(),
                ));
            }
        }
        let _ = version;
        Ok(())
    }

    /// Back the pedal up where TonePush keeps its own backups, under a name
    /// that says it took it, keeping the newest few of those.
    fn back_up_automatically(&mut self) -> WorkResult<PathBuf> {
        let directory = hx_catalog::home::backups().ok_or_else(|| {
            WorkError::Other("There is no folder for backups on this computer".into())
        })?;
        std::fs::create_dir_all(&directory).map_err(|error| {
            WorkError::Other(format!("Could not make {}: {error}", directory.display()))
        })?;
        let version = self.device()?.identity().version.clone();
        let captured = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| WorkError::Other(error.to_string()))?
            .as_secs();
        // To the second: a reconnect later the same day must not replace the
        // backup taken before that day's edits.
        let stamp = jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S").to_string();
        let path = directory.join(format!("{AUTOMATIC_BACKUP}{version}-{stamp}.vxbundle"));
        self.capture_backup(&path, captured)?;
        prune_automatic_backups(&directory, &path);
        Ok(path)
    }

    fn capture_backup(&mut self, path: &Path, captured: u64) -> WorkResult<()> {
        let identity = self.device()?.identity().clone();
        let reuse = latest_verified_backup(&identity).map(|(_, bundle)| bundle);
        let lists = LIBRARIES
            .into_iter()
            .map(|library| self.list(library))
            .collect::<WorkResult<Vec<_>>>()?;
        let chunks_by_path = lists
            .iter()
            .map(|list| (list.path.to_string(), list.chunks_per_slot()))
            .collect::<BTreeMap<_, _>>();
        let chunk_total = lists
            .iter()
            .map(|list| list.occupied().count() * list.chunks_per_slot())
            .sum::<usize>()
            .max(1);
        let events = self.events.clone();
        let target = path.to_owned();
        let mut completed_chunks = 0usize;
        backup::capture_reusing(self.device()?, path, captured, reuse.as_ref(), |step| {
            let (what, progress) = match step {
                backup::Step::Schema => ("Reading schema".into(), 0.01),
                backup::Step::List { path, occupied } => {
                    (format!("{path}: {occupied} occupied"), 0.02)
                }
                backup::Step::Blob {
                    path,
                    index,
                    chunk,
                    chunks,
                    ..
                } if chunk == 0 || chunk == chunks || chunk % 64 == 0 => {
                    let within = chunk as f32 / chunks.max(1) as f32;
                    (
                        format!("{path} slot {}: {chunk}/{chunks}", index + 1),
                        0.02 + 0.96 * (completed_chunks as f32 + within * chunks as f32)
                            / chunk_total as f32,
                    )
                }
                backup::Step::Verifying { path, index } => {
                    completed_chunks += chunks_by_path.get(&path).copied().unwrap_or_default();
                    (
                        format!("Verifying {path} slot {}", index + 1),
                        0.02 + 0.96 * completed_chunks as f32 / chunk_total as f32,
                    )
                }
                backup::Step::Reused {
                    path,
                    index,
                    chunks,
                    ..
                } => {
                    completed_chunks += chunks;
                    (
                        format!("Reusing verified {path} slot {}", index + 1),
                        0.02 + 0.96 * completed_chunks as f32 / chunk_total as f32,
                    )
                }
                backup::Step::Done => (format!("Published {}", target.display()), 1.0),
                _ => return,
            };
            let _ = events.send(Evt::Working { what, progress });
        })?;
        let bundle = backup::open_verified(path)?;
        let preset_hashes = preset_hashes_from_bundle(&bundle)?;
        let facts = libraries::facts_of(&bundle);
        let rollback = bundle.arm(self.device()?)?;
        self.rollback = Some(rollback);
        self.rollback_path = Some(path.to_owned());
        self.send(Evt::Guarded {
            path: path.to_owned(),
            preset_hashes,
            facts,
        });
        Ok(())
    }

    fn refresh(&mut self, baseline: bool) -> WorkResult<()> {
        let snapshot = snapshot(self.device()?)?;
        self.send(Evt::Snapshot { snapshot, baseline });
        Ok(())
    }

    fn record_edit(&mut self, edit: NodeEdit) {
        let now = Instant::now();
        let same_burst = self.future.is_empty()
            && self
                .last_edit_at
                .is_some_and(|last| now.duration_since(last) <= Duration::from_millis(400));
        if let Some(previous) = same_burst
            .then_some(())
            .and_then(|()| self.history.last_mut())
            .and_then(|step| (step.len() == 1 && step[0].path == edit.path).then_some(&mut step[0]))
        {
            previous.after = edit.after;
            previous.description = edit.description;
        } else {
            self.history.push(vec![edit]);
            if self.history.len() > 32 {
                self.history.remove(0);
            }
        }
        self.last_edit_at = Some(now);
        self.future.clear();
        self.report_history();
    }

    fn record_transaction(&mut self, edits: Vec<NodeEdit>) {
        if edits.is_empty() {
            return;
        }
        self.history.push(edits);
        if self.history.len() > 32 {
            self.history.remove(0);
        }
        self.future.clear();
        self.last_edit_at = None;
        self.report_history();
    }

    fn forget_history(&mut self) {
        self.history.clear();
        self.future.clear();
        self.last_edit_at = None;
        self.report_history();
    }

    fn report_history(&self) {
        self.send(Evt::History {
            undo: self.history.len(),
            redo: self.future.len(),
        });
    }

    fn step_history(&mut self, undo: bool) -> WorkResult<()> {
        self.last_edit_at = None;
        let transaction = if undo {
            self.history.pop()
        } else {
            self.future.pop()
        };
        let Some(transaction) = transaction else {
            self.send(Evt::Success(format!(
                "Nothing to {}",
                if undo { "undo" } else { "redo" }
            )));
            return Ok(());
        };

        let order: Vec<usize> = if undo {
            (0..transaction.len()).rev().collect()
        } else {
            (0..transaction.len()).collect()
        };
        let mut applied: Vec<usize> = Vec::new();
        for index in order {
            let edit = &transaction[index];
            let target = if undo { &edit.before } else { &edit.after };
            if let Err(error) = self.write_edit(edit, target.clone()) {
                for applied_index in applied.into_iter().rev() {
                    let applied_edit = &transaction[applied_index];
                    let restore = if undo {
                        applied_edit.after.clone()
                    } else {
                        applied_edit.before.clone()
                    };
                    let _ = self.write_edit(applied_edit, restore);
                }
                if undo {
                    self.history.push(transaction);
                } else {
                    self.future.push(transaction);
                }
                self.report_history();
                return Err(error);
            }
            applied.push(index);
        }

        for edit in &transaction {
            self.send(Evt::NodeValue {
                path: edit.path.to_string(),
                value: if undo {
                    edit.before.clone()
                } else {
                    edit.after.clone()
                },
            });
        }
        if undo {
            self.future.push(transaction);
        } else {
            self.history.push(transaction);
        }
        self.report_history();
        self.send(Evt::Success(if undo {
            "Undone".into()
        } else {
            "Redone".into()
        }));
        Ok(())
    }

    fn write_edit(&mut self, edit: &NodeEdit, value: Value) -> WorkResult<()> {
        let device = self.device()?;
        device.write_node(edit.path.clone(), &edit.description, value)?;
        Ok(())
    }

    fn device(&mut self) -> WorkResult<&mut Device<SerialLink>> {
        // A pedal in Update Mode answers who it is and takes the update;
        // nothing else is asked of it.
        if self.update_mode {
            return Err(WorkError::Other(
                "The pedal is in Update Mode: switch it off and on without holding UPD to use it"
                    .into(),
            ));
        }
        self.device
            .as_mut()
            .ok_or_else(|| WorkError::Other("StompStation PRO is not connected".into()))
    }

    fn restore_audition(&mut self) -> WorkResult<()> {
        let original = std::mem::take(&mut self.audition_original);
        for (path, description, value) in original {
            self.device()?.write_node(path, &description, value)?;
        }
        Ok(())
    }

    fn require_guard(&self) -> WorkResult<()> {
        if self.rollback.is_none() {
            Err(WorkError::Other(
                "Load a rollback bundle that matches the pedal before persistent writes".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn list(&mut self, library: Library) -> WorkResult<BlobList> {
        Ok(self.device()?.list_info(NodePath::new(library.path())?)?)
    }

    fn refuse_orphan(
        &mut self,
        list: &BlobList,
        index: usize,
        replacement: Option<&str>,
    ) -> WorkResult<()> {
        if list.path.as_str() == Library::Presets.path() {
            return Ok(());
        }
        let old = list
            .names
            .get(index)
            .ok_or_else(|| WorkError::Other(format!("slot {} is out of range", index + 1)))?;
        let Some(old) = old else {
            return Ok(());
        };
        if replacement == Some(old) {
            return Ok(());
        }
        // Ask the pedal's current presets, not only the rollback captured at
        // the start of the session. A preset saved after that point may have
        // introduced a new model/IR reference, and renaming from an old mirror
        // would orphan exactly the work the rollback is meant to protect.
        let schema = self.device()?.browse(NodePath::new("root\\app")?)?;
        let parameter_paths = schema
            .nodes()
            .iter()
            .filter(|(_, description)| description.reference.as_deref() == Some(list.path.as_str()))
            .map(|(path, _)| path.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        if parameter_paths.is_empty() {
            return Ok(());
        }
        let presets = self.list(Library::Presets)?;
        let mut first_reference = None;
        let mut count = 0;
        for (preset_index, preset_name) in presets.occupied() {
            self.send(Evt::Progress(format!(
                "Checking preset {} for {old}",
                preset_index + 1
            )));
            let blob = self.device()?.read_blob(&presets, preset_index)?;
            let preset = Preset::parse(&blob)?;
            for record in preset.records() {
                if parameter_paths.contains(record.subject())
                    && record.value().get("value").and_then(Value::as_str) == Some(old)
                {
                    count += 1;
                    first_reference
                        .get_or_insert_with(|| (preset_name, record.subject().to_owned()));
                }
            }
        }
        if count == 0 {
            Ok(())
        } else {
            let (preset_name, node_path) = first_reference.expect("a counted reference exists");
            Err(WorkError::Other(format!(
                "{old:?} is referenced by {} preset node(s), including {} in {}",
                count, node_path, preset_name
            )))
        }
    }

    fn import(
        &mut self,
        library: Library,
        index: usize,
        name: &str,
        file: &Path,
    ) -> WorkResult<()> {
        self.require_guard()?;
        let source = std::fs::read(file).map_err(|error| {
            WorkError::Other(format!("Could not read {}: {error}", file.display()))
        })?;
        let list = self.list(library)?;
        let device_name = unique_slot_name(&list, index, name);
        let blob = import_blob(library, &source, list.size)?;
        self.refuse_orphan(&list, index, Some(&device_name))?;
        let events = self.events.clone();
        self.device()?
            .write_blob(&list, index, &device_name, &blob, |step| {
                if let Some(line) = upload_line(step) {
                    let _ = events.send(Evt::Progress(line));
                }
            })?;
        self.refresh(false)?;
        if library == Library::Presets {
            let bytes = export_blob(library, &blob, list.size)?;
            self.send(Evt::PresetIndexed {
                index,
                hash: Some(slot_hash(&bytes)),
            });
        }
        self.send(Evt::Success(format!(
            "Imported {} into {} slot {}",
            file.display(),
            library.title(),
            index + 1
        )));
        Ok(())
    }

    fn export(&mut self, library: Library, index: usize, file: &Path) -> WorkResult<()> {
        let list = self.list(library)?;
        let blob = self.device()?.read_blob(&list, index)?;
        let bytes = export_blob(library, &blob, list.size)?;
        crate::library::atomic_write(file, bytes).map_err(|error| {
            WorkError::Other(format!("Could not write {}: {error}", file.display()))
        })?;
        self.send(Evt::Success(format!("Exported {}", file.display())));
        Ok(())
    }

    fn import_stereo(
        &mut self,
        left: usize,
        right: usize,
        name: &str,
        file: &Path,
    ) -> WorkResult<()> {
        self.require_guard()?;
        if left == right {
            return Err(WorkError::Other(
                "Stereo IR needs two different destination slots".into(),
            ));
        }
        let source = std::fs::read(file).map_err(|error| {
            WorkError::Other(format!("Could not read {}: {error}", file.display()))
        })?;
        let list = self.list(Library::Irs)?;
        let blobs = voidx_client::ir::Wav::parse(&source)?.device_blobs(list.size)?;
        if blobs.len() != 2 {
            return Err(WorkError::Other(
                "Stereo import needs a two-channel WAV".into(),
            ));
        }
        let left_name = unique_slot_name(&list, left, &format!("{name} L"));
        // The right name must not collide with the left one about to be
        // written, but the orphan checks need the names presets use today.
        let mut planned = list.clone();
        planned.names[left] = Some(left_name.clone());
        let right_name = unique_slot_name(&planned, right, &format!("{name} R"));
        self.refuse_orphan(&list, left, Some(&left_name))?;
        self.refuse_orphan(&list, right, Some(&right_name))?;
        self.device()?
            .write_blob(&list, left, &left_name, &blobs[0], |_| {})?;
        self.device()?
            .write_blob(&list, right, &right_name, &blobs[1], |_| {})?;
        self.refresh(false)?;
        self.send(Evt::Success(format!(
            "Imported stereo IR into slots {} and {}",
            left + 1,
            right + 1
        )));
        Ok(())
    }

    fn export_stereo(&mut self, left: usize, right: usize, file: &Path) -> WorkResult<()> {
        if left == right {
            return Err(WorkError::Other(
                "Stereo IR needs two different source slots".into(),
            ));
        }
        let list = self.list(Library::Irs)?;
        let left = self.device()?.read_blob(&list, left)?;
        let right = self.device()?.read_blob(&list, right)?;
        let bytes = voidx_client::ir::Wav::from_device_blobs(
            &[left.as_slice(), right.as_slice()],
            voidx_client::ir::SAMPLE_RATE,
        )?
        .encode()?;
        crate::library::atomic_write(file, bytes).map_err(|error| {
            WorkError::Other(format!("Could not write {}: {error}", file.display()))
        })?;
        self.send(Evt::Success(format!("Exported {}", file.display())));
        Ok(())
    }
}

/// Reuse the newest complete backup that still proves equal to this pedal.
/// This gives ordinary Save/Send interactions the same immediacy as HX while
/// retaining the PRO rule that a persistent write never starts unguarded.
fn latest_matching_rollback(
    device: &mut Device<SerialLink>,
) -> Option<(
    PathBuf,
    ArmedRollback,
    BTreeMap<usize, String>,
    LibraryFacts,
)> {
    for path in backup_candidates()? {
        let Ok(bundle) = backup::open_verified(&path) else {
            continue;
        };
        // Another pedal's, or another firmware's, never arms: no need to
        // read what its files say first.
        if bundle.manifest().identity != *device.identity() {
            continue;
        }
        let Ok(preset_hashes) = preset_hashes_from_bundle(&bundle) else {
            continue;
        };
        let facts = libraries::facts_of(&bundle);
        if let Ok(rollback) = bundle.arm(device) {
            return Some((path, rollback, preset_hashes, facts));
        }
    }
    None
}

/// The rollback already owns every verified device byte. Convert its padded
/// preset blobs to the same portable bytes the local library stores, so PRO
/// preset rows can use the exact HX three-state comparison without another
/// serial read.
fn preset_hashes_from_bundle(
    bundle: &backup::VerifiedBundle,
) -> WorkResult<BTreeMap<usize, String>> {
    let list = bundle.list(Library::Presets.path()).ok_or_else(|| {
        WorkError::Other("verified backup has no StompStation preset library".into())
    })?;
    let mut hashes = BTreeMap::new();
    for slot in &list.slots {
        if slot.name.is_none() {
            continue;
        }
        let blob = bundle
            .blob(Library::Presets.path(), slot.index)
            .ok_or_else(|| {
                WorkError::Other(format!(
                    "verified backup has no bytes for preset slot {}",
                    slot.index + 1
                ))
            })?;
        hashes.insert(slot.index, preset_blob_hash(blob)?);
    }
    Ok(hashes)
}

/// The hash a slot holding this device blob is known by: that of the bytes
/// an export of it gives, the form the library keeps. Every way a slot's
/// hash is learned (a read, a backup, a write) goes through the same export,
/// so a preset written from the library is "Same" before and after a
/// reconnect alike.
fn preset_blob_hash(blob: &[u8]) -> WorkResult<String> {
    let bytes = export_blob(Library::Presets, blob, blob.len())?;
    Ok(slot_hash(&bytes))
}

/// The hash of a preset as exported, for comparing a slot with the library.
fn slot_hash(exported: &[u8]) -> String {
    slot_hash_in(exported, crate::library::holds)
}

/// Earlier versions of TonePush dropped the CRLF that ends a stored preset, so a
/// preset kept from the pedal by them is in the library without it. When the
/// library holds that copy and not this one, the slot is known by the copy's
/// hash, so what was kept still reads as the same preset.
fn slot_hash_in(exported: &[u8], holds: impl Fn(&str) -> bool) -> String {
    let hash = crate::library::hash_of(exported);
    let body = exported
        .iter()
        .rposition(|byte| !matches!(byte, b'\r' | b'\n'))
        .map_or(0, |last| last + 1);
    if body < exported.len() && !holds(&hash) {
        let legacy = crate::library::hash_of(&exported[..body]);
        if holds(&legacy) {
            return legacy;
        }
    }
    hash
}

fn latest_verified_backup(identity: &Identity) -> Option<(PathBuf, backup::VerifiedBundle)> {
    for path in backup_candidates()? {
        let Ok(bundle) = backup::open_verified(&path) else {
            continue;
        };
        if bundle.manifest().identity == *identity {
            return Some((path, bundle));
        }
    }
    None
}

/// Name prefix of the bundles TonePush captures on its own when no earlier
/// bundle matches the pedal. Only these are ever pruned.
const AUTOMATIC_BACKUP: &str = "stompstation-pro-";
/// Automatic bundles kept, newest first. Each holds every NAM model, so tens
/// of megabytes; ones the user captured by hand are never removed.
const AUTOMATIC_BACKUPS_KEPT: usize = 10;

fn prune_automatic_backups(directory: &Path, keep: &Path) {
    let Some(candidates) = backup_candidates() else {
        return;
    };
    for stale in stale_automatic_backups(candidates, directory, keep) {
        let _ = std::fs::remove_dir_all(stale);
    }
}

/// From bundles ordered newest first, the automatic ones beyond the newest
/// [`AUTOMATIC_BACKUPS_KEPT`] (counting `keep`, the one just captured).
fn stale_automatic_backups(
    candidates: Vec<PathBuf>,
    directory: &Path,
    keep: &Path,
) -> Vec<PathBuf> {
    candidates
        .into_iter()
        .filter(|path| {
            path.parent() == Some(directory)
                && path != keep
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(AUTOMATIC_BACKUP))
        })
        .skip(AUTOMATIC_BACKUPS_KEPT - 1)
        .collect()
}

fn backup_candidates() -> Option<Vec<PathBuf>> {
    let directory = hx_catalog::home::backups()?;
    let mut candidates = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "vxbundle")
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    candidates.reverse();
    Some(candidates)
}

fn snapshot(device: &mut Device<SerialLink>) -> voidx_client::Result<Snapshot> {
    let identity = device.identity().clone();
    let transport = device.transport().to_owned();
    let app = device.browse(NodePath::new("root\\app")?)?;
    let active_preset = app
        .get(&NodePath::new("root\\app\\preset")?)
        .and_then(|description| description.value.as_ref())
        .and_then(Value::as_str)
        .map(str::to_owned);
    let settings = device.browse(NodePath::new("root\\settings")?)?;
    let mut libraries = Vec::with_capacity(LIBRARIES.len());
    for library in LIBRARIES {
        libraries.push(LibraryState {
            library,
            info: device.list_info(NodePath::new(library.path())?)?,
        });
    }
    Ok(Snapshot {
        identity,
        transport,
        active_preset,
        libraries,
        app: app.nodes().to_vec(),
        settings: settings.nodes().to_vec(),
    })
}

/// VoidX resolves preset and model references by their visible names, and the
/// firmware refuses duplicate names even in different slots. TonePush keeps
/// the person's library title intact and chooses the smallest distinct label
/// only for the pedal copy.
fn unique_slot_name(list: &BlobList, target: usize, requested: &str) -> String {
    const MAX_BYTES: usize = 63;

    fn fitted(text: &str, max: usize) -> String {
        let mut end = text.len().min(max);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    }

    let base = if requested.trim().is_empty() {
        "Preset"
    } else {
        requested.trim()
    };
    let collides = |candidate: &str| {
        list.names.iter().enumerate().any(|(index, known)| {
            index != target
                && known
                    .as_deref()
                    .is_some_and(|known| known.eq_ignore_ascii_case(candidate))
        })
    };
    let first = fitted(base, MAX_BYTES);
    if !collides(&first) {
        return first;
    }
    for number in 2..=(list.count + 2) {
        let suffix = format!(" {number}");
        let candidate = format!("{}{}", fitted(base, MAX_BYTES - suffix.len()), suffix);
        if !collides(&candidate) {
            return candidate;
        }
    }
    // There are fewer occupied slots than candidates tried, so this is only a
    // defensive fallback for malformed list metadata.
    fitted(&format!("{base} copy"), MAX_BYTES)
}

fn import_blob(library: Library, source: &[u8], capacity: usize) -> WorkResult<Vec<u8>> {
    match library {
        Library::Presets => Ok(Preset::parse(source)?.encode_padded(capacity)?),
        Library::Irs => {
            let blobs = voidx_client::ir::Wav::parse(source)?.device_blobs(capacity)?;
            if blobs.len() != 1 {
                return Err(WorkError::Other(
                    "Single-slot import needs a mono WAV; use stereo import".into(),
                ));
            }
            Ok(blobs.into_iter().next().expect("one checked channel"))
        }
        Library::Amps | Library::Drives => Ok(voidx_client::nam::encode(source, capacity)?),
    }
}

fn export_blob(library: Library, blob: &[u8], capacity: usize) -> WorkResult<Vec<u8>> {
    match library {
        Library::Presets => Ok(Preset::parse(blob)?.content()),
        Library::Irs => Ok(voidx_client::ir::Wav::from_device_blobs(
            &[blob],
            voidx_client::ir::SAMPLE_RATE,
        )?
        .encode()?),
        Library::Amps | Library::Drives => Ok(voidx_client::nam::decode(blob, capacity)?),
    }
}

fn upload_line(step: UploadStep) -> Option<String> {
    match step {
        UploadStep::Name => Some("Writing name".into()),
        UploadStep::Data { chunk, chunks } if chunk == 1 || chunk == chunks || chunk % 64 == 0 => {
            Some(format!("Uploading {chunk}/{chunks}"))
        }
        UploadStep::Commit => Some("Committing".into()),
        UploadStep::Verify { chunk, chunks }
            if chunk == 1 || chunk == chunks || chunk % 64 == 0 =>
        {
            Some(format!("Verifying {chunk}/{chunks}"))
        }
        UploadStep::Done => Some("Upload verified".into()),
        _ => None,
    }
}

fn upload_progress(step: UploadStep) -> f32 {
    match step {
        UploadStep::Name => 0.02,
        UploadStep::Data { chunk, chunks } => 0.05 + 0.55 * chunk as f32 / chunks.max(1) as f32,
        UploadStep::Commit => 0.65,
        UploadStep::Verify { chunk, chunks } => 0.7 + 0.28 * chunk as f32 / chunks.max(1) as f32,
        UploadStep::Done => 1.0,
    }
}

type WorkResult<T> = Result<T, WorkError>;

enum WorkError {
    Device(voidx_client::Error),
    Protocol(voidx_proto::CommandError),
    Preset(voidx_proto::PresetError),
    Other(String),
}

impl std::fmt::Display for WorkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device(error) => error.fmt(formatter),
            Self::Protocol(error) => error.fmt(formatter),
            Self::Preset(error) => error.fmt(formatter),
            Self::Other(error) => formatter.write_str(error),
        }
    }
}

impl From<voidx_client::Error> for WorkError {
    fn from(error: voidx_client::Error) -> Self {
        Self::Device(error)
    }
}

impl From<voidx_proto::CommandError> for WorkError {
    fn from(error: voidx_proto::CommandError) -> Self {
        Self::Protocol(error)
    }
}

impl From<voidx_proto::PresetError> for WorkError {
    fn from(error: voidx_proto::PresetError) -> Self {
        Self::Preset(error)
    }
}

/// Synthetic StompStation PRO content for the design screenshots: invented
/// preset, capture and impulse response names on a firmware 1.5.12 schema
/// shaped like the pedal's own. Nothing here talks to a pedal.
#[cfg(test)]
pub(crate) mod demo {
    use super::*;

    const PRESETS: [&str; 24] = [
        "Clean Machine",
        "Glass Wall",
        "Crunch Room",
        "Stereo Swirl",
        "Lead Machine",
        "Plexi Jump",
        "Modern Chug",
        "Velvet Drive",
        "Shimmer Lead",
        "Twin Clean",
        "Brit Crunch",
        "Fuzz Wall",
        "Detuned Dream",
        "Rotary Blues",
        "Para Clean",
        "Spring Surf",
        "Slap Echo",
        "Doom Room",
        "Worship Swell",
        "Funk Comp",
        "Edge Lead",
        "Bass Grit",
        "Practice",
        "Studio DI",
    ];
    const AMPS: [&str; 14] = [
        "JCM800 2203 · Crunch",
        "AC30 TB · Top Boost",
        "Twin Reverb · Vibrato ch",
        "Plexi 1959 · Jumped",
        "Recto Modern · Ch3",
        "Bassman 5F6A · Bright",
        "5150 III · Blue",
        "ODS-style · Drive",
        "DC30 · Ch1",
        "BE-100 · BE",
        "Rockerverb · Dirty",
        "Shiva · Ch2",
        "Mark IIC+ · Lead",
        "SLO-100 · OD",
    ];
    const DRIVES: [&str; 9] = [
        "TS808 · Drive 9",
        "Klon-style · Noon",
        "Rat · Filter 3",
        "Blues Driver · Full",
        "Big Muff · Ram's Head",
        "Fuzz Face · Ge",
        "OCD · HP",
        "SD-1 · Edge",
        "Timmy · Bass cut",
    ];
    const IRS: [&str; 11] = [
        "V30 · SM57 cap edge",
        "Greenback · R121",
        "Oxford Room L",
        "Oxford Room R",
        "Alnico Blue · 57 + 121",
        "Jensen C10Q · SM57",
        "Studio Wide L",
        "Studio Wide R",
        "Bass 8x10 · D112",
        "Celestion G12M · 414",
        "Acoustic body · DI blend",
    ];

    fn list(path: &str, count: usize, size: usize, names: &[&str]) -> BlobList {
        BlobList {
            path: NodePath::new(path).expect("a valid library path"),
            description: None,
            size,
            count,
            chunk_size: 128,
            group: None,
            gzip: false,
            movable: true,
            item_type: None,
            names: (0..count)
                .map(|index| names.get(index).map(|name| (*name).to_owned()))
                .collect(),
        }
    }

    fn node(path: &str, description: serde_json::Value) -> (NodePath, NodeDescription) {
        (
            NodePath::new(path).expect("a valid node path"),
            serde_json::from_value(description).expect("a valid node description"),
        )
    }

    fn float(
        path: &str,
        desc: &str,
        value: f64,
        min: f64,
        max: f64,
        unit: Option<&str>,
    ) -> (NodePath, NodeDescription) {
        let mut description = serde_json::json!({
            "type": "float", "desc": desc, "value": value, "min": min, "max": max,
            "def": (min + max) / 2.0, "step": (max - min) / 100.0,
        });
        if let Some(unit) = unit {
            description["unit"] = unit.into();
        }
        node(path, description)
    }

    fn switch(group: &str, on: bool) -> (NodePath, NodeDescription) {
        node(
            &format!("root\\app\\{group}\\on_off"),
            serde_json::json!({
                "type": "enum", "desc": "On/Off", "value": if on { "ON" } else { "OFF" },
                "options": ["OFF", "ON"],
            }),
        )
    }

    fn mode(group: &str, value: &str, options: &[&str]) -> (NodePath, NodeDescription) {
        node(
            &format!("root\\app\\{group}\\md"),
            serde_json::json!({
                "type": "enum", "desc": "Mode", "value": value, "options": options,
            }),
        )
    }

    fn model(group: &str, leaf: &str, value: &str, library: &str) -> (NodePath, NodeDescription) {
        node(
            &format!("root\\app\\{group}\\{leaf}"),
            serde_json::json!({
                "type": "plist", "desc": "Model", "value": value, "ref": library,
            }),
        )
    }

    /// What the demo's backup says about its files: who captured each amp
    /// and drive and what gear, each impulse response's length and shape,
    /// and which presets play what.
    fn facts() -> LibraryFacts {
        use super::libraries::{SlotFacts, User};

        const GEAR: [&str; 14] = [
            "Marshall JCM800 2203",
            "Vox AC30 Top Boost",
            "Fender Twin Reverb",
            "Marshall 1959 SLP",
            "Mesa Dual Rectifier",
            "Fender Bassman 5F6A",
            "EVH 5150 III",
            "Overdrive Special clone",
            "Matchless DC-30",
            "Friedman BE-100",
            "Orange Rockerverb 50",
            "Bogner Shiva",
            "Mesa Mark IIC+",
            "Soldano SLO-100",
        ];
        const SIZES: [&str; 4] = ["Standard", "Standard", "Lite", "Feather"];
        const MAKERS: [&str; 3] = ["J. Rivera", "M. Okafor", "S. Lindqvist"];
        const PEDALS: [&str; 9] = [
            "Ibanez TS808",
            "Klon Centaur clone",
            "ProCo Rat",
            "Boss BD-2",
            "EHX Big Muff",
            "Dunlop Fuzz Face",
            "Fulltone OCD",
            "Boss SD-1",
            "Paul Cochrane Timmy",
        ];
        let mut facts = LibraryFacts::default();
        for (index, name) in AMPS.iter().enumerate() {
            facts.files.insert(
                (Library::Amps, (*name).to_owned()),
                SlotFacts {
                    gear: Some(GEAR[index].to_owned()),
                    modeled_by: Some(MAKERS[index % 3].to_owned()),
                    size: Some(SIZES[index % 4].to_owned()),
                    input_dbu: Some(12.2),
                    bytes: 1_920_000 - index * 41_000,
                    ..SlotFacts::default()
                },
            );
        }
        for (index, name) in DRIVES.iter().enumerate() {
            facts.files.insert(
                (Library::Drives, (*name).to_owned()),
                SlotFacts {
                    gear: Some(PEDALS[index].to_owned()),
                    modeled_by: Some(MAKERS[(index + 1) % 3].to_owned()),
                    size: Some(SIZES[(index + 2) % 4].to_owned()),
                    input_dbu: Some(10.4),
                    bytes: 1_180_000 - index * 23_000,
                    ..SlotFacts::default()
                },
            );
        }
        let lengths = [
            200.0, 200.0, 500.0, 500.0, 200.0, 170.0, 320.0, 320.0, 200.0, 200.0, 400.0,
        ];
        for (index, name) in IRS.iter().enumerate() {
            let ms: f64 = lengths[index];
            // A decaying ring, different on each side of a pair.
            let wave = (0..240)
                .map(|point| {
                    let t = point as f32 / 240.0;
                    let decay = (-t * (6.0 + index as f32)).exp();
                    decay * ((t * 90.0 + index as f32).sin() * 0.8 + (t * 37.0).cos() * 0.2)
                })
                .collect();
            facts.files.insert(
                (Library::Irs, (*name).to_owned()),
                SlotFacts {
                    length_ms: Some(ms),
                    wave,
                    bytes: (ms * 48.0 * 4.0) as usize,
                    ..SlotFacts::default()
                },
            );
        }
        let user = |preset: usize, block: &str| User {
            preset,
            name: PRESETS[preset].to_owned(),
            block: block.to_owned(),
        };
        let uses: [(Library, &str, &[usize], &str); 9] = [
            (Library::Amps, AMPS[0], &[2, 4, 7, 9, 12, 16], "Amp"),
            (Library::Amps, AMPS[1], &[1, 3, 13, 15], "Amp"),
            (Library::Amps, AMPS[2], &[0, 5, 18], "Amp"),
            (Library::Amps, AMPS[3], &[5, 11], "Amp"),
            (Library::Drives, DRIVES[0], &[2, 7, 11, 20], "Drive"),
            (Library::Irs, IRS[0], &[0, 2, 4, 7, 9, 12, 16], "IR"),
            (Library::Irs, IRS[1], &[1, 3, 13, 15], "IR"),
            (Library::Irs, IRS[2], &[3, 15], "IR"),
            (Library::Irs, IRS[3], &[3, 15], "IR"),
        ];
        for (library, name, presets, block) in uses {
            facts.users.insert(
                (library, name.to_owned()),
                presets.iter().map(|preset| user(*preset, block)).collect(),
            );
        }
        facts
    }

    /// 03B Velvet Drive, as a firmware 1.5.12 pedal would describe it.
    fn snapshot() -> Snapshot {
        let app = vec![
            node(
                "root\\app\\preset",
                serde_json::json!({ "type": "item", "desc": "Preset", "value": "Velvet Drive" }),
            ),
            switch("gate", true),
            float(
                "root\\app\\gate\\thr",
                "Threshold",
                -52.0,
                -96.0,
                0.0,
                Some("dB"),
            ),
            float(
                "root\\app\\gate\\rel",
                "Release",
                120.0,
                10.0,
                1000.0,
                Some("ms"),
            ),
            switch("pitch", false),
            mode("pitch", "Octave", &["Octave", "Detune", "Whammy"]),
            float("root\\app\\pitch\\mix", "Mix", 40.0, 0.0, 100.0, Some("%")),
            switch("exp", false),
            float(
                "root\\app\\exp\\pos",
                "Position",
                0.0,
                0.0,
                100.0,
                Some("%"),
            ),
            switch("comp", true),
            mode("comp", "Studio", &["Dyn", "Studio"]),
            float(
                "root\\app\\comp\\thr",
                "Threshold",
                -24.0,
                -60.0,
                0.0,
                Some("dB"),
            ),
            float("root\\app\\comp\\ratio", "Ratio", 4.0, 1.0, 20.0, None),
            float(
                "root\\app\\comp\\level",
                "Level",
                2.0,
                -12.0,
                12.0,
                Some("dB"),
            ),
            switch("mod_pre", false),
            mode("mod_pre", "Phaser", &["Chorus", "Phaser", "Flanger"]),
            float(
                "root\\app\\mod_pre\\rate",
                "Rate",
                30.0,
                0.0,
                100.0,
                Some("%"),
            ),
            switch("drive", true),
            model("drive", "model", DRIVES[0], "root\\nam_drive"),
            float("root\\app\\drive\\gain", "Gain", 5.5, 0.0, 10.0, None),
            float("root\\app\\drive\\level", "Level", 7.0, 0.0, 10.0, None),
            switch("amp", true),
            model("amp", "model", AMPS[0], "root\\nam_amp"),
            float("root\\app\\amp\\gain", "Gain", 6.2, 0.0, 10.0, None),
            float("root\\app\\amp\\bass", "Low", 4.8, 0.0, 10.0, None),
            float("root\\app\\amp\\mid", "Mid", 6.0, 0.0, 10.0, None),
            float("root\\app\\amp\\treble", "Treble", 5.5, 0.0, 10.0, None),
            float(
                "root\\app\\amp\\volume",
                "Volume",
                -1.5,
                -24.0,
                12.0,
                Some("dB"),
            ),
            switch("ir", true),
            model("ir", "ir", IRS[0], "root\\ir_list"),
            float("root\\app\\ir\\mix", "Mix", 100.0, 0.0, 100.0, Some("%")),
            float(
                "root\\app\\ir\\lowcut",
                "Low Cut",
                80.0,
                20.0,
                500.0,
                Some("Hz"),
            ),
            switch("eq", true),
            float("root\\app\\eq\\low", "Low", 1.5, -12.0, 12.0, Some("dB")),
            float("root\\app\\eq\\mid", "Mid", -2.0, -12.0, 12.0, Some("dB")),
            float("root\\app\\eq\\high", "High", 1.0, -12.0, 12.0, Some("dB")),
            switch("mod", true),
            mode(
                "mod",
                "Chorus",
                &["Chorus", "Flanger", "Phaser", "Tremolo", "Rotary"],
            ),
            float("root\\app\\mod\\rate", "Rate", 35.0, 0.0, 100.0, Some("%")),
            float(
                "root\\app\\mod\\depth",
                "Depth",
                55.0,
                0.0,
                100.0,
                Some("%"),
            ),
            switch("delay", true),
            mode("delay", "Tape", &["Digital", "Tape", "Analog", "Ping Pong"]),
            float(
                "root\\app\\delay\\time",
                "Time",
                410.0,
                20.0,
                2000.0,
                Some("ms"),
            ),
            float(
                "root\\app\\delay\\fb",
                "Feedback",
                32.0,
                0.0,
                100.0,
                Some("%"),
            ),
            float("root\\app\\delay\\mix", "Mix", 28.0, 0.0, 100.0, Some("%")),
            switch("reverb", true),
            mode(
                "reverb",
                "Shimmer",
                &["Room", "Hall", "Plate", "Spring", "Shimmer"],
            ),
            float(
                "root\\app\\reverb\\decay",
                "Decay",
                4.2,
                0.1,
                20.0,
                Some("s"),
            ),
            float("root\\app\\reverb\\mix", "Mix", 35.0, 0.0, 100.0, Some("%")),
            float(
                "root\\app\\output\\volume",
                "Volume",
                0.0,
                -80.0,
                10.0,
                Some("dB"),
            ),
            float(
                "root\\app\\output\\tempo_bpm",
                "Tempo",
                96.0,
                40.0,
                240.0,
                Some("BPM"),
            ),
        ];
        let settings = vec![
            float(
                "root\\settings\\input\\gain",
                "Input gain",
                0.0,
                -12.0,
                12.0,
                Some("dB"),
            ),
            node(
                "root\\settings\\input\\pickup",
                serde_json::json!({
                    "type": "enum", "desc": "Pickup", "value": "Humbucker",
                    "options": ["Single coil", "Humbucker", "Active"],
                }),
            ),
            float(
                "root\\settings\\tuner\\ref",
                "Reference",
                440.0,
                425.0,
                455.0,
                Some("Hz"),
            ),
            node(
                "root\\settings\\misc\\bright",
                serde_json::json!({
                    "type": "enum", "desc": "Screen brightness", "value": "High",
                    "options": ["Low", "Medium", "High"],
                }),
            ),
        ];
        Snapshot {
            identity: Identity {
                name: "StompStation PRO".into(),
                version: "1.5.12".into(),
                architecture: "CM4".into(),
                license: "sspro".into(),
            },
            transport: "USB serial".into(),
            active_preset: Some("Velvet Drive".into()),
            libraries: vec![
                LibraryState {
                    library: Library::Presets,
                    info: list("root\\presets", 60, 16_384, &PRESETS),
                },
                LibraryState {
                    library: Library::Irs,
                    info: list("root\\ir_list", 40, 98_304, &IRS),
                },
                LibraryState {
                    library: Library::Amps,
                    info: list("root\\nam_amp", 30, 262_144, &AMPS),
                },
                LibraryState {
                    library: Library::Drives,
                    info: list("root\\nam_drive", 30, 262_144, &DRIVES),
                },
            ],
            app,
            settings,
        }
    }

    impl Panel {
        /// Cut the panel off from its worker, so nothing a test does reaches
        /// a pedal plugged into the machine, and count the Connects it asks
        /// for from then on.
        pub(crate) fn count_connects(&mut self) -> impl FnMut() -> usize {
            let (tx, commands) = mpsc::channel();
            self.tx = tx;
            move || {
                commands
                    .try_iter()
                    .filter(|command| matches!(command, Cmd::Connect))
                    .count()
            }
        }

        /// A look that has been answered with nothing found.
        pub(crate) fn demo_looked(&mut self) {
            self.connecting = false;
            self.failed = true;
        }

        /// Show the synthetic pedal as though it had just connected, guarded
        /// by a backup and with two controls turned since the last save.
        pub(crate) fn show_demo(&mut self) {
            self.active = true;
            self.online = true;
            self.status.clear();
            self.install_snapshot(snapshot(), true);
            self.rollback = Some(PathBuf::from("stompstation-pro-demo.vxbundle"));
            self.rollback_time = jiff::Zoned::now()
                .with()
                .hour(14)
                .minute(2)
                .second(0)
                .build()
                .ok()
                .map(|time| SystemTime::from(time.timestamp()));
            for (path, value) in [
                ("root\\app\\amp\\gain", 6.2),
                ("root\\app\\delay\\mix", 28.0),
            ] {
                self.saved_drafts
                    .insert(path.into(), Value::from(value - 1.0));
            }
            self.recompute_dirty();
            self.selected_group = "amp".into();
            self.undo_depth = 2;
            self.facts = facts();
        }

        /// Which slots hold what the library holds, as a backup would say.
        pub(crate) fn demo_library_marks(&mut self, hashes: BTreeMap<usize, String>) {
            self.preset_hashes = hashes;
        }

        /// Open the pedal's page on a library: the NAM amps on the AC30, or
        /// the impulse responses on the Oxford Room pair.
        pub(crate) fn demo_library(&mut self, irs: bool) {
            if irs {
                self.tab = Tab::Library(Library::Irs);
                self.target_slots.insert(Library::Irs, 3);
            } else {
                self.tab = Tab::Library(Library::Amps);
                self.target_slots.insert(Library::Amps, 2);
            }
        }

        /// A tempo typed that the pedal's tempo node does not take.
        pub(crate) fn demo_tempo_refused(&mut self) {
            if let Some((path, description, _)) = self.tempo_node() {
                self.set_tempo(path, description, 999.0);
            }
        }

        /// No StompStation PRO answered.
        pub(crate) fn demo_not_found(&mut self) {
            self.failed = true;
            self.status = "No StompStation PRO found".to_owned();
        }

        /// Open the pedal's page on its settings.
        pub(crate) fn demo_settings(&mut self) {
            self.tab = Tab::Settings;
        }

        /// A firmware update to 2.2.6 at one of its steps: backing up,
        /// waiting for Update Mode, asking before writing, writing, waiting
        /// for the restart, and stopped.
        pub(crate) fn demo_firmware(&mut self, step: usize) {
            use super::firmware::{BackupFacts, Failure, Flow, ImageFacts, Seen, Stage};
            let now = Instant::now();
            let half_past_two = jiff::Zoned::now()
                .with()
                .hour(14)
                .minute(31)
                .second(0)
                .build()
                .map_or(0, |time| time.timestamp().as_second() as u64);
            let mut flow = Flow::new(Some("1.5.12".to_owned()), false);
            flow.image = Some(ImageFacts {
                file: "s_pro_2_2_6.upd".to_owned(),
                version: "2.2.6".to_owned(),
                sha256: "63d9b8047ae47a0c9a22b6be7c7de6540cc8d2b2c4dabb756a842b14043083c9"
                    .to_owned(),
                official: true,
                size: 5_128_192,
            });
            flow.sent = (0, 5_128_192);
            let backup = BackupFacts {
                path: PathBuf::from("stompstation-pro-1.5.12-20261001-143100.vxbundle"),
                captured: half_past_two,
                version: "1.5.12".to_owned(),
                presets: 24,
                amps: 14,
                drives: 9,
                irs: 11,
            };
            self.tab = Tab::Firmware;
            self.rollback_time = Some(UNIX_EPOCH + Duration::from_secs(half_past_two));
            match step {
                0 => {
                    flow.stage = Stage::BackingUp;
                    self.working = Some(("Reusing verified root\\nam_amp slot 9".to_owned(), 0.58));
                }
                1 => {
                    flow.backup = Some(backup);
                    flow.stage = Stage::AwaitingUpdateMode;
                    flow.seen = Seen::Nothing;
                    self.online = false;
                }
                2 => {
                    flow.backup = Some(backup);
                    flow.stage = Stage::Ready;
                    flow.asking = true;
                    self.online = false;
                    self.update_mode = true;
                }
                3 => {
                    flow.backup = Some(backup);
                    flow.stage = Stage::Writing;
                    flow.sent = (3_178_496, 5_128_192);
                    flow.write_started = now.checked_sub(Duration::from_secs(38));
                    self.online = false;
                    self.update_mode = true;
                }
                4 => {
                    flow.backup = Some(backup);
                    flow.stage = Stage::SwitchedOff;
                    flow.sent = (5_128_192, 5_128_192);
                    flow.written_at = now.checked_sub(Duration::from_secs(6 * 60 + 3));
                    // Switched off at 14:38, three seconds ago as the
                    // countdown has it.
                    let twenty_to_three = jiff::Zoned::now()
                        .with()
                        .hour(14)
                        .minute(38)
                        .second(0)
                        .build()
                        .map_or(SystemTime::now(), |time| SystemTime::from(time.timestamp()));
                    flow.off_at = now
                        .checked_sub(Duration::from_secs(3))
                        .map(|off| (off, twenty_to_three));
                    self.online = false;
                }
                _ => {
                    flow.backup = Some(backup);
                    flow.stage = Stage::Failed;
                    flow.sent = (2_461_696, 5_128_192);
                    flow.failure = Some(Failure {
                        during: Stage::Writing,
                        why: "timed out waiting for \"upd\"; reconnect before sending another \
                              command"
                            .to_owned(),
                        confirmed: 2_461_696,
                        total: 5_128_192,
                    });
                    self.online = false;
                }
            }
            self.firmware = Some(flow);
        }

        /// The pedal before any backup of it matches: edits play, saving
        /// waits.
        pub(crate) fn demo_unprotected(&mut self) {
            self.rollback = None;
            self.rollback_time = None;
            self.facts = LibraryFacts::default();
            self.preset_hashes.clear();
        }

        /// Open the pedal's page on its backups.
        pub(crate) fn demo_page(&mut self) {
            self.tab = Tab::Backups;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset() -> Vec<u8> {
        b"root\\app\\comp\\on_off:{\"value\":\"ON\"}\r\n\
root\\app\\comp\\md:{\"value\":\"Studio\"}\r\n\
root\\app\\amp\\on_off:{\"value\":\"ON\"}\r\n\
root\\app\\amp\\model:{\"value\":\"British Clean\"}\r\n\
root\\app\\ir\\on_off:{\"value\":\"ON\"}\r\n\
root\\app\\ir\\ir:{\"value\":\"British Cab\"}\r\n"
            .to_vec()
    }

    #[test]
    fn native_preset_facts_feed_the_shared_library() {
        assert_eq!(preset_content(&preset()).as_deref(), Some("Full rig"));
        let blocks = preset_blocks(&preset());
        assert_eq!(
            blocks,
            vec![
                serde_json::json!({
                    "name": "Studio", "category": 2, "enabled": true, "path": 0
                }),
                serde_json::json!({
                    "name": "British Clean", "category": 11, "enabled": true, "path": 0
                }),
                serde_json::json!({
                    "name": "British Cab", "category": 14, "enabled": true, "path": 0
                }),
            ],
            "PRO blocks use the shared Cloud schema and physical chain order"
        );
    }

    #[test]
    fn native_preset_does_not_publish_output_as_a_signal_block() {
        let bytes = b"root\\app\\output\\on_off:{\"value\":\"ON\"}\r\n\
root\\app\\output\\model:{\"value\":\"Master\"}\r\n\
root\\app\\delay\\on_off:{\"value\":\"OFF\"}\r\n";
        assert_eq!(
            preset_blocks(bytes),
            vec![serde_json::json!({
                "name": "Delay", "category": 5, "enabled": false, "path": 0
            })]
        );
    }

    #[test]
    fn native_preset_content_distinguishes_amp_and_cab_capabilities() {
        let bytes = b"root\\app\\amp\\on_off:{\"value\":\"ON\"}\r\n\
root\\app\\ir\\on_off:{\"value\":\"OFF\"}\r\n";
        assert_eq!(preset_content(bytes).as_deref(), Some("Amp, no cab"));
    }

    #[test]
    fn dirty_state_is_measured_against_the_saved_app_values() {
        let saved = BTreeMap::from([
            ("root\\app\\amp\\gain".into(), Value::from(42.0)),
            ("root\\app\\amp\\model".into(), Value::from("Clean")),
        ]);
        assert!(!drafts_differ(&saved, &saved));

        let mut edited = saved.clone();
        edited.insert("root\\app\\amp\\gain".into(), Value::from(43.0));
        assert!(drafts_differ(&saved, &edited));
        edited.insert("root\\app\\amp\\gain".into(), Value::from(42.0));
        assert!(!drafts_differ(&saved, &edited), "undo reached the baseline");
    }

    #[test]
    fn rapid_ticks_of_one_control_form_one_bounded_undo_step() {
        let (_commands, receiver) = mpsc::channel();
        let (events, _events) = mpsc::channel();
        let mut worker = Worker::new(receiver, events, egui::Context::default());
        let path = NodePath::new("root\\app\\amp\\gain").unwrap();
        let description: NodeDescription = serde_json::from_value(serde_json::json!({
            "type": "float", "value": 0.0, "min": 0.0, "max": 100.0
        }))
        .unwrap();
        let edit = |before: f64, after: f64| NodeEdit {
            path: path.clone(),
            description: description.clone(),
            before: Value::from(before),
            after: Value::from(after),
        };

        worker.record_edit(edit(0.0, 1.0));
        worker.record_edit(edit(1.0, 2.0));
        assert_eq!(worker.history.len(), 1);
        assert_eq!(worker.history[0][0].before, Value::from(0.0));
        assert_eq!(worker.history[0][0].after, Value::from(2.0));

        worker.last_edit_at = None;
        worker.record_edit(edit(2.0, 3.0));
        assert_eq!(
            worker.history.len(),
            2,
            "save and command boundaries split history"
        );
    }

    #[test]
    fn folder_metadata_and_private_telemetry_are_not_controls() {
        let folder: NodeDescription = serde_json::from_value(serde_json::json!({
            "type": "item", "desc": "Amp", "value": "", "item_type": "hfolder"
        }))
        .unwrap();
        assert!(!editable_node("root\\app\\amp", &folder, &Value::from("")));

        let float: NodeDescription = serde_json::from_value(serde_json::json!({
            "type": "float", "desc": "Gain", "value": 15.8,
            "min": 0.0, "max": 100.0
        }))
        .unwrap();
        assert!(editable_node(
            "root\\app\\drive\\gain",
            &float,
            &Value::from(15.8)
        ));
        assert!(!editable_node(
            "root\\app\\drive\\_meta_in",
            &float,
            &Value::from(0.0)
        ));
        assert!(!editable_node(
            "root\\app\\output\\pst\\ctl1\\lnk1\\min",
            &float,
            &Value::from(0.0)
        ));
    }

    #[test]
    fn duplicate_device_names_receive_the_smallest_available_suffix() {
        let list = BlobList {
            path: NodePath::new("root\\presets").unwrap(),
            description: None,
            size: 16_384,
            count: 4,
            chunk_size: 128,
            group: None,
            gzip: false,
            movable: true,
            item_type: None,
            names: vec![Some("Clean".into()), Some("Clean 2".into()), None, None],
        };
        assert_eq!(unique_slot_name(&list, 2, "Clean"), "Clean 3");
        assert_eq!(unique_slot_name(&list, 0, "Clean"), "Clean");
    }

    #[test]
    fn slot_searches_cover_numbers_names_and_empty_destinations() {
        assert!(slot_matches_search(22, Some("British Lead"), "23"));
        assert!(slot_matches_search(22, Some("British Lead"), "lead"));
        assert!(slot_matches_search(22, None, "empty"));
        assert!(!slot_matches_search(22, None, "lead"));
    }

    #[test]
    fn stereo_ir_import_owns_the_adjacent_slot() {
        assert_eq!(ir_import_targets(1, 9, 60).unwrap(), vec![9]);
        assert_eq!(ir_import_targets(2, 9, 60).unwrap(), vec![9, 10]);
        assert!(ir_import_targets(2, 59, 60)
            .unwrap_err()
            .contains("two adjacent slots"));
        assert!(ir_import_targets(3, 9, 60)
            .unwrap_err()
            .contains("does not support 3 channels"));
    }

    #[test]
    fn padded_device_preset_hashes_match_portable_library_objects() {
        let portable = b"root\\app\\amp\\gain:{\"value\":42.0}";
        let mut device = portable.to_vec();
        device.resize(16_384, 0);
        let Ok(device_hash) = preset_blob_hash(&device) else {
            panic!("valid padded preset should hash")
        };

        assert_eq!(
            device_hash,
            crate::library::hash_of(portable),
            "sync compares the exact portable bytes kept by the local library"
        );
    }

    /// A preset written from the library is hashed as the slot will be read
    /// back from a backup after a reconnect, so "Same" does not flip, and
    /// that is the library object's own hash, with or without the CRLF a
    /// stored preset ends in.
    #[test]
    fn a_written_preset_hashes_as_the_backup_will_read_it() {
        for library in [
            b"root\\app\\amp\\gain:{\"value\":42.0}\r\n".as_slice(),
            b"root\\app\\amp\\gain:{\"value\":42.0}".as_slice(),
        ] {
            let Ok(blob) = import_blob(Library::Presets, library, 16_384) else {
                panic!("a valid preset should import")
            };
            let Ok(written) = preset_blob_hash(&blob) else {
                panic!("a written preset should hash")
            };
            assert_eq!(written, crate::library::hash_of(library));
            let Ok(exported) = export_blob(Library::Presets, &blob, blob.len()) else {
                panic!("a written preset should export")
            };
            assert_eq!(exported, library, "export gives the library's bytes back");
        }
    }

    /// A preset kept before export held on to the stored CRLF is in the
    /// library without it; the slot is known by that copy's hash until the
    /// library holds the exact export too.
    #[test]
    fn a_preset_kept_without_its_crlf_still_reads_as_kept() {
        let exported = b"root\\app\\amp\\gain:{\"value\":42.0}\r\n";
        let legacy = crate::library::hash_of(b"root\\app\\amp\\gain:{\"value\":42.0}");
        let exact = crate::library::hash_of(exported);

        assert_eq!(slot_hash_in(exported, |_| false), exact, "nothing kept");
        assert_eq!(slot_hash_in(exported, |hash| hash == legacy), legacy);
        assert_eq!(
            slot_hash_in(exported, |hash| hash == legacy || hash == exact),
            exact,
            "the exact copy wins once the library holds it"
        );
        assert_eq!(
            slot_hash_in(b"root\\x:{\"value\":1}", |_| true),
            crate::library::hash_of(b"root\\x:{\"value\":1}"),
            "nothing to strip, nothing to look up"
        );
    }

    #[test]
    fn queued_ticks_for_one_node_collapse_to_the_latest_value() {
        let (commands, receiver) = mpsc::channel();
        let (events, _events) = mpsc::channel();
        let worker = Worker::new(receiver, events, egui::Context::default());
        let path = NodePath::new("root\\app\\amp\\gain").unwrap();
        let description: NodeDescription = serde_json::from_value(serde_json::json!({
            "type": "float", "value": 0.0, "min": 0.0, "max": 100.0
        }))
        .unwrap();
        let command = |before: f64, value: f64| Cmd::SetNode {
            path: path.clone(),
            description: Box::new(description.clone()),
            before: Value::from(before),
            value: Value::from(value),
            persistent: false,
        };
        commands.send(command(1.0, 2.0)).unwrap();
        commands.send(command(2.0, 3.0)).unwrap();

        let (coalesced, pending) = worker.coalesce_live_edits(command(0.0, 1.0));
        assert!(pending.is_none());
        let Cmd::SetNode { before, value, .. } = coalesced else {
            panic!("a live edit must remain a live edit")
        };
        assert_eq!(before, Value::from(0.0));
        assert_eq!(value, Value::from(3.0));
    }

    #[test]
    fn only_old_automatic_backups_are_pruned() {
        let directory = Path::new("/backups");
        let keep = directory.join("stompstation-pro-1.5.12-20261001-120000.vxbundle");
        let mut candidates = vec![keep.clone()];
        for hour in (0..12).rev() {
            candidates.push(directory.join(format!(
                "stompstation-pro-1.5.12-20260930-{hour:02}0000.vxbundle"
            )));
        }
        candidates.insert(3, directory.join("before-gig.vxbundle"));
        let stale = stale_automatic_backups(candidates, directory, &keep);
        assert_eq!(
            stale,
            [
                directory.join("stompstation-pro-1.5.12-20260930-020000.vxbundle"),
                directory.join("stompstation-pro-1.5.12-20260930-010000.vxbundle"),
                directory.join("stompstation-pro-1.5.12-20260930-000000.vxbundle"),
            ]
        );
    }

    /// A pedal in Update Mode is asked nothing but the update, and an update
    /// waiting for the pedal is not interrupted by looking for it again.
    #[test]
    fn a_pedal_in_update_mode_is_asked_nothing_but_the_update() {
        let (_commands, receiver) = mpsc::channel();
        let (events, _received) = mpsc::channel();
        let mut worker = Worker::new(receiver, events, egui::Context::default());
        worker.update_mode = true;
        assert!(worker
            .device()
            .is_err_and(|error| error.to_string().contains("Update Mode")));
        assert!(worker.handle(Cmd::Connect).is_ok());
        worker.update_mode = false;
        worker.awaiting = Some(firmware::Awaiting::UpdateMode);
        assert!(worker.handle(Cmd::Connect).is_ok());
        worker.awaiting = None;
        // Nothing is written without a checked file, a backup, and the pedal
        // in Update Mode.
        assert!(worker
            .firmware_write("2.2.6")
            .is_err_and(|error| error.to_string().contains("firmware file")));
    }

    #[test]
    fn switching_presets_with_unsaved_edits_asks_first() {
        let mut panel = Panel::new(egui::Context::default());
        panel.select_preset(2);
        assert!(panel.confirmation.is_none());

        panel.dirty = true;
        panel.select_preset(2);
        let confirmation = panel.confirmation.as_ref().expect("asks before discarding");
        assert_eq!(confirmation.action, "Discard and load");
        assert!(matches!(confirmation.command, Cmd::SelectPreset(2)));
    }
}
