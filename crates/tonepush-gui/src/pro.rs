//! Native StompStation PRO panel and its strictly ordered VoidX worker.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::RichText;
use serde_json::Value;
use voidx_client::backup::{self, ArmedRollback};
use voidx_client::{BlobList, Device, Identity, SerialLink, UploadStep};
use voidx_proto::{NodeDescription, NodeKind, NodePath, Preset};

use crate::{config, processor, theme, LibraryLookup};

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
    Restore(PathBuf),
}

enum ReadTarget {
    Library { replace: bool },
    Clipboard,
    File(PathBuf),
}

enum Evt {
    Connected(Snapshot),
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
    Guarded(PathBuf),
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
    Disconnected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Library(Library),
    Settings,
    Backup,
}

struct Confirmation {
    question: String,
    command: Cmd,
}

pub(crate) struct Panel {
    tx: Sender<Cmd>,
    rx: Receiver<Evt>,
    active: bool,
    online: bool,
    busy: bool,
    dirty: bool,
    status: String,
    snapshot: Option<Snapshot>,
    rollback: Option<PathBuf>,
    tab: Tab,
    selected_group: String,
    show_device: bool,
    search: String,
    shelf_search: String,
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
    audition_events: Vec<Option<i64>>,
    preview: Option<(String, Snapshot)>,
    confirmation: Option<Confirmation>,
    /// A whole-device operation and its completion fraction. Kept separate
    /// from `busy`: a progress meter communicates work without turning live
    /// sound controls into a loading screen.
    working: Option<(String, f32)>,
}

impl Panel {
    pub(crate) fn new(ctx: egui::Context) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        std::thread::spawn(move || Worker::new(commands, events, ctx).run());
        let panel = Self {
            tx,
            rx,
            active: false,
            online: false,
            busy: false,
            dirty: false,
            status: "Looking for a StompStation PRO…".into(),
            snapshot: None,
            rollback: None,
            tab: Tab::Settings,
            selected_group: String::new(),
            show_device: false,
            search: String::new(),
            shelf_search: String::new(),
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
            audition_events: Vec::new(),
            preview: None,
            confirmation: None,
            working: None,
        };
        let _ = panel.tx.send(Cmd::Connect);
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
                Ok(Evt::Connected(snapshot)) => {
                    self.active = true;
                    self.online = true;
                    self.status.clear();
                    self.preset_hashes.clear();
                    self.install_snapshot(snapshot, true);
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
                    self.preset_hashes
                        .insert(index, crate::library::hash_of(&bytes));
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
                            self.preset_hashes
                                .insert(index, crate::library::hash_of(bytes));
                        } else if name.is_empty() {
                            self.preset_hashes.remove(&index);
                        }
                    }
                    self.captured_setlists.push(slots);
                }
                Ok(Evt::Auditioning(key)) => self.audition_events.push(key),
                Ok(Evt::Guarded(path)) => {
                    self.rollback = Some(path);
                    self.working = None;
                    self.status.clear();
                }
                Ok(Evt::History { undo, redo }) => {
                    self.undo_depth = undo;
                    self.redo_depth = redo;
                }
                Ok(Evt::Busy(busy)) => self.busy = busy,
                Ok(Evt::Progress(line)) => self.status = line,
                Ok(Evt::Working { what, progress }) => {
                    self.working = Some((what, progress.clamp(0.0, 1.0)));
                }
                Ok(Evt::Success(line)) => {
                    self.working = None;
                    self.status = line;
                }
                Ok(Evt::Failed(line)) => {
                    self.working = None;
                    self.status = line;
                }
                Ok(Evt::Disconnected) => {
                    if self.active {
                        self.online = false;
                        self.status = "StompStation PRO disconnected".into();
                    }
                    self.rollback = None;
                    self.preset_hashes.clear();
                    self.working = None;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.online = false;
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

    pub(crate) fn capture_setlist(&self) {
        let _ = self.tx.send(Cmd::CaptureSetlist);
    }

    pub(crate) fn reconnect(&self) {
        let _ = self.tx.send(Cmd::Connect);
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

    pub(crate) fn top_bar(&mut self, root: &mut egui::Ui) {
        let snapshot = self.snapshot.clone();
        processor::top_bar(root, "pro_top", |ui| {
            ui.add_space(8.0);
            if let Some(snapshot) = &snapshot {
                let title = snapshot
                    .active_preset
                    .as_deref()
                    .unwrap_or("StompStation PRO");
                let slot = active_preset_index(snapshot)
                    .map(|index| format!("{:02}", index + 1))
                    .unwrap_or_else(|| "--".into());
                if let Some(name) = processor::preset_title(
                    ui,
                    &slot,
                    title,
                    self.dirty,
                    self.rollback.is_some(),
                    &mut self.renaming_header,
                ) {
                    if let Some(index) = active_preset_index(snapshot) {
                        let _ = self.tx.send(Cmd::Rename {
                            library: Library::Presets,
                            index,
                            name,
                        });
                    }
                }
                ui.add_space(12.0);
                let save_enabled =
                    self.dirty && self.rollback.is_some() && !self.save_name.trim().is_empty();
                let save_disabled = if self.rollback.is_none() {
                    "Save becomes available after a current rollback is verified"
                } else {
                    "Save - no changes to save"
                };
                let actions = processor::preset_tools(
                    ui,
                    self.online,
                    self.undo_depth,
                    self.redo_depth,
                    save_enabled,
                    save_disabled,
                );
                if actions.undo {
                    let _ = self.tx.send(Cmd::Undo);
                }
                if actions.redo {
                    let _ = self.tx.send(Cmd::Redo);
                }
                if actions.save {
                    let _ = self.tx.send(Cmd::SavePreset(self.save_name.trim().into()));
                }
            }
            if self.busy {
                theme::spinner(ui);
            }
            if let Some(snapshot) = &snapshot {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    self.tempo_control(ui, snapshot);
                });
            }
        });
    }

    fn tempo_control(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let Some((path, description)) = snapshot.app.iter().find(|(path, description)| {
            path.as_str().ends_with("\\tempo_bpm")
                && matches!(description.kind, Some(NodeKind::Float))
        }) else {
            return;
        };
        let Some(tempo) = self
            .drafts
            .get(path.as_str())
            .and_then(Value::as_f64)
            .map(|value| value as f32)
        else {
            return;
        };
        if let Some(bpm) =
            processor::tempo_control(ui, tempo, &mut self.tempo_draft, &mut self.taps)
        {
            let value = Value::from(f64::from(bpm));
            if description.validate_value(&value).is_ok() {
                let before = self
                    .drafts
                    .get(path.as_str())
                    .cloned()
                    .unwrap_or(Value::Null);
                self.drafts.insert(path.to_string(), value.clone());
                self.recompute_dirty();
                let _ = self.tx.send(Cmd::SetNode {
                    path: path.clone(),
                    description: Box::new(description.clone()),
                    before,
                    value,
                    persistent: false,
                });
            }
        }
    }

    pub(crate) fn shortcuts(&self, ctx: &egui::Context) {
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
                let _ = self.tx.send(Cmd::SelectPreset(index));
            }
        }
    }

    pub(crate) fn status_bar(&mut self, root: &mut egui::Ui) {
        let snapshot = self.snapshot.clone();
        processor::status_bar(root, "pro_status", |ui| {
            ui.add_space(8.0);
            theme::status_dot(
                ui,
                if self.online {
                    egui::Color32::from_rgb(0x4c, 0xc0, 0x60)
                } else {
                    theme::DIM
                },
            );
            let device_name = snapshot
                .as_ref()
                .map(|snapshot| snapshot.identity.name.as_str())
                .unwrap_or("No device");
            if processor::device_button(
                ui,
                self.online,
                RichText::new(device_name).strong(),
                "device settings, backup and restore",
            )
            .clicked()
            {
                self.tab = Tab::Settings;
                self.show_device = !self.show_device;
            }
            if let Some(snapshot) = &snapshot {
                ui.label(
                    RichText::new(format!("firmware {}", snapshot.identity.version))
                        .color(theme::DIM),
                );
            }
            if self.online {
                if ui.small_button("Disconnect").clicked() {
                    let _ = self.tx.send(Cmd::Disconnect);
                }
            } else if ui.small_button("Reconnect").clicked() {
                let _ = self.tx.send(Cmd::Connect);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let guard = if self.rollback.is_some() {
                    "Automatic backup ready"
                } else {
                    "Automatic backup required for Save"
                };
                let guard = ui.label(RichText::new(guard).small().color(theme::DIM));
                if let Some(path) = &self.rollback {
                    guard.on_hover_text(format!(
                        "TonePush can restore persistent changes from {}",
                        path.display()
                    ));
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(&self.status).small().color(theme::DIM));
                }
                if let Some((what, progress)) = &self.working {
                    ui.separator();
                    ui.add(
                        egui::ProgressBar::new(*progress)
                            .desired_width(150.0)
                            .text(RichText::new(what).small()),
                    );
                }
            });
        });
    }

    pub(crate) fn body(&mut self, root: &mut egui::Ui) {
        let snapshot = self.snapshot.clone();
        if let Some(snapshot) = &snapshot {
            let max_height = (root.available_height() - 96.0).max(126.0);
            processor::chain_panel(
                root,
                "pro_chain",
                106.0_f32.min(max_height),
                106.0..=max_height,
                |ui| self.signal_chain(ui, snapshot),
            );
            // The picker belongs to the editor below the chain. Creating this
            // dock after the top panel means it cannot steal chain width or
            // paint over the rightmost controls.
            if block_selector(snapshot, &self.selected_group).is_some() {
                egui::Panel::right("pro_shelf")
                    .min_size(320.0)
                    .max_size(420.0)
                    .default_size(360.0)
                    .resizable(true)
                    .show(root, |ui| self.model_shelf(ui, snapshot));
            }
        }

        processor::editor(root, |ui| match &snapshot {
            Some(snapshot) => {
                let guarded = self.selected_group != INPUT_GROUP || self.rollback.is_some();
                ui.add_enabled_ui(self.online && !self.busy && guarded, |ui| {
                    self.nodes_ui(ui, snapshot, false, Some(self.selected_group.clone()))
                });
            }
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(&self.status).color(theme::DIM));
                });
            }
        });
    }

    pub(crate) fn windows(&mut self, ctx: &egui::Context) {
        let snapshot = self.snapshot.clone();
        if let Some(snapshot) = &snapshot {
            self.device_window(ctx, snapshot);
        }
        self.confirmation_window(ctx);
        self.preview_window(ctx);
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some((name, snapshot)) = self.preview.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new(name)
            .open(&mut open)
            .default_width(900.0)
            .default_height(430.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("StompStation PRO preset · preview").color(theme::DIM));
                ui.separator();
                ui.add_enabled_ui(false, |ui| {
                    let height = 150.0_f32.min(ui.available_height() * 0.42);
                    ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                        self.signal_chain(ui, &snapshot);
                    });
                    ui.separator();
                    self.pedal_nodes_ui(ui, &snapshot, &self.selected_group.clone());
                });
            });
        if !open {
            self.preview = None;
        }
    }

    pub(crate) fn preset_list(
        &mut self,
        root: &mut egui::Ui,
        lookup: &LibraryLookup,
        config: &mut config::Config,
        sending: Option<&str>,
    ) -> Option<usize> {
        let snapshot = self.snapshot.clone();
        let mut picked = None;
        let mut cancel_send = false;
        processor::preset_panel(root, "pro_presets", |ui| {
            let Some(snapshot) = &snapshot else { return };
            let Some(presets) = snapshot
                .libraries
                .iter()
                .find(|state| state.library == Library::Presets)
            else {
                return;
            };
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("SETLIST").small().color(theme::DIM))
                    .on_hover_text("Use ↑/↓ to move through presets when no field is active");
                let (mark, colour) = if self.show_favorites_only {
                    (theme::Icon::StarOn, theme::ACCENT)
                } else {
                    (theme::Icon::Star, theme::DIM)
                };
                if theme::small_icon_button(ui, mark, Some(colour))
                    .on_hover_text("Show favourites only")
                    .clicked()
                {
                    self.show_favorites_only = !self.show_favorites_only;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::place_enabled(
                        ui,
                        theme::Icon::Computer,
                        theme::Sync::Absent,
                        self.online,
                    )
                    .on_hover_text("keep every preset on the pedal, in order, as a setlist")
                    .clicked()
                    {
                        let _ = self.tx.send(Cmd::CaptureSetlist);
                    }
                });
            });
            if let Some(name) = sending {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Choose a slot for {name}"))
                            .small()
                            .color(theme::ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Cancel").clicked() {
                            cancel_send = true;
                        }
                    });
                });
            }
            ui.separator();
            egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut toggle = None;
                let mut read = None;
                let mut rename = None;
                for (index, name) in presets.info.names.iter().enumerate() {
                    let favorite = config.is_favorite(FAVORITES_SETLIST, index as i64);
                    if self.show_favorites_only && !favorite {
                        continue;
                    }
                    let label = name.as_deref().unwrap_or("Empty");
                    let selected = name.as_deref() == snapshot.active_preset.as_deref();
                    let label = format!("{:>2}  {label}", index + 1);
                    let text = if selected {
                        RichText::new(&label).color(theme::ACCENT).strong()
                    } else if name.is_some() {
                        RichText::new(&label)
                    } else {
                        RichText::new(&label).color(theme::DIM)
                    };
                    ui.horizontal(|ui| {
                        ui.set_min_height(20.0);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let (star, colour) = if favorite {
                            (theme::Icon::StarOn, theme::ACCENT)
                        } else {
                            (theme::Icon::Star, theme::DIM)
                        };
                        if theme::small_icon_button(ui, star, Some(colour))
                            .on_hover_text(if favorite { "Remove favourite" } else { "Favourite" })
                            .clicked()
                        {
                            toggle = Some(index);
                        }
                        if name.is_some() {
                            let state = self.slot_sync(index, lookup);
                            let keep = theme::place_enabled(
                                ui,
                                theme::Icon::Computer,
                                state,
                                self.online && !matches!(state, theme::Sync::Working),
                            );
                            let keep = match state {
                                theme::Sync::Absent => keep.on_hover_text("Not in your library. Keep it"),
                                theme::Sync::Same => keep.on_hover_text("In your library"),
                                theme::Sync::Differs => keep.on_hover_text(
                                    "In your library under this name, but different. Update it from the pedal",
                                ),
                                theme::Sync::Working => keep.on_hover_text("Saving…"),
                                theme::Sync::Unknown => keep.on_hover_text("Check and keep in your library"),
                            };
                            if keep.clicked() && state != theme::Sync::Same {
                                read = Some((index, matches!(state, theme::Sync::Differs)));
                            }
                        } else {
                            ui.add_space(16.0);
                        }
                        if sending.is_some() {
                            let empty = name.is_none();
                            let target_text = if empty {
                                RichText::new(format!("{:>2}  empty", index + 1)).color(theme::ACCENT)
                            } else {
                                RichText::new(&label).color(theme::DIM)
                            };
                            let target = ui.add(
                                egui::Button::new(()).left_text(target_text).frame(false)
                                    .min_size(egui::vec2(ui.available_width(), 18.0)),
                            );
                            let target = if empty { target.on_hover_text("Put it here") } else {
                                target.on_hover_text(format!("Replace {}", name.as_deref().unwrap_or_default()))
                            };
                            if target.clicked() {
                                picked = Some(index);
                            }
                        } else if matches!(&self.renaming_preset, Some((slot, _)) if *slot == index) {
                            if let Some((_, draft)) = self.renaming_preset.as_mut() {
                                let field = ui.add(
                                    egui::TextEdit::singleline(draft)
                                        .desired_width(180.0)
                                        .hint_text("preset name"),
                                );
                                if !field.has_focus() && !field.lost_focus() {
                                    field.request_focus();
                                }
                                if field.lost_focus() {
                                    if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                                        rename = Some((index, draft.clone()));
                                    }
                                    self.renaming_preset = None;
                                }
                            }
                        } else {
                            let row = ui.add_enabled_ui(name.is_some(), |ui| ui.selectable_label(selected, text)).inner;
                            if row.clicked() {
                                let _ = self.tx.send(Cmd::SelectPreset(index));
                            }
                            row.context_menu(|ui| {
                                if ui
                                    .add_enabled(
                                        self.rollback.is_some(),
                                        egui::Button::new("Rename"),
                                    )
                                    .clicked()
                                {
                                    self.renaming_preset = Some((index, name.clone().unwrap_or_default()));
                                    ui.close();
                                }
                                if ui.button("Copy").clicked() {
                                    let _ = self.tx.send(Cmd::ReadPreset { index, target: ReadTarget::Clipboard });
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        self.rollback.is_some() && self.clipboard.is_some(),
                                        egui::Button::new("Paste"),
                                    )
                                    .clicked()
                                {
                                    if let Some((name, bytes)) = self.clipboard.clone() {
                                        let _ = self.tx.send(Cmd::ImportBytes { index, name, bytes });
                                    }
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button("Save to file…").clicked() {
                                    let stem = sanitise(name.as_deref().unwrap_or("preset"));
                                    if let Some(path) = rfd::FileDialog::new()
                                        .set_file_name(format!("{stem}.vxpreset"))
                                        .add_filter("StompStation PRO preset", &["vxpreset"])
                                        .save_file()
                                    {
                                        let _ = self.tx.send(Cmd::ReadPreset { index, target: ReadTarget::File(path) });
                                    }
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        self.rollback.is_some(),
                                        egui::Button::new("Load from file…"),
                                    )
                                    .clicked()
                                {
                                    if let Some(file) = rfd::FileDialog::new()
                                        .add_filter("StompStation PRO preset", &["vxpreset"])
                                        .pick_file()
                                    {
                                        let import_name = file.file_stem().and_then(|stem| stem.to_str())
                                            .unwrap_or("Preset").to_owned();
                                        self.confirmation = Some(Confirmation {
                                            question: format!("Replace slot {} with {import_name}?", index + 1),
                                            command: Cmd::Import { library: Library::Presets, index, name: import_name, file },
                                        });
                                    }
                                    ui.close();
                                }
                                if ui.button("Keep in library").clicked() {
                                    read = Some((index, false));
                                    ui.close();
                                }
                                ui.separator();
                                if ui
                                    .add_enabled(
                                        self.rollback.is_some() && index > 0,
                                        egui::Button::new("Move up"),
                                    )
                                    .clicked()
                                {
                                    let _ = self.tx.send(Cmd::Move {
                                        library: Library::Presets,
                                        from: index,
                                        to: index - 1,
                                    });
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        self.rollback.is_some()
                                            && index + 1 < presets.info.count,
                                        egui::Button::new("Move down"),
                                    )
                                    .clicked()
                                {
                                    let _ = self.tx.send(Cmd::Move {
                                        library: Library::Presets,
                                        from: index,
                                        to: index + 1,
                                    });
                                    ui.close();
                                }
                                ui.separator();
                                if ui
                                    .add_enabled(
                                        self.rollback.is_some(),
                                        egui::Button::new("Remove"),
                                    )
                                    .clicked()
                                {
                                    self.confirmation = Some(Confirmation {
                                        question: format!("Empty slot {} back to a blank preset?", index + 1),
                                        command: Cmd::Clear { library: Library::Presets, index },
                                    });
                                    ui.close();
                                }
                            });
                        }
                    });
                }
                if let Some(index) = toggle {
                    config.toggle_favorite(FAVORITES_SETLIST, index as i64);
                }
                if let Some((index, replace)) = read {
                    let _ = self.tx.send(Cmd::ReadPreset {
                        index,
                        target: ReadTarget::Library { replace },
                    });
                }
                if let Some((index, name)) = rename {
                    let _ = self.tx.send(Cmd::Rename {
                        library: Library::Presets,
                        index,
                        name,
                    });
                }
            });
        });
        if cancel_send {
            return Some(usize::MAX);
        }
        picked
    }

    fn signal_chain(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        egui::ScrollArea::horizontal()
            .id_salt("pro-fixed-chain-v2")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Each card already owns its four-pixel wire. egui's
                    // ordinary widget gap on top of that was nearly three
                    // extra card widths across this fixed fourteen-block run.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if processor::fixed_endpoint(ui, "Input", self.selected_group == INPUT_GROUP)
                        .on_hover_text("input and pickup settings")
                        .clicked()
                    {
                        self.selected_group = INPUT_GROUP.into();
                        self.search.clear();
                    }
                    processor::fixed_connector(ui);
                    let groups = app_groups(snapshot);
                    for group in &groups {
                        let title = friendly_group(group);
                        let name = block_model_name(snapshot, group).unwrap_or(title);
                        let category = group_category(group);
                        if processor::fixed_signal_block(
                            ui,
                            &name,
                            category,
                            self.selected_group == *group,
                            block_enabled(snapshot, group),
                        )
                        .on_hover_text(format!("root\\app\\{group}"))
                        .clicked()
                        {
                            self.selected_group = group.clone();
                            self.search.clear();
                        }
                        processor::fixed_connector(ui);
                    }
                    if processor::fixed_endpoint(ui, "Output", self.selected_group == "output")
                        .on_hover_text("master and preset output settings")
                        .clicked()
                    {
                        self.selected_group = "output".into();
                        self.search.clear();
                    }
                });
            });
    }

    fn model_shelf(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let Some((path, description)) = block_selector(snapshot, &self.selected_group) else {
            return;
        };
        let current = self
            .drafts
            .get(path.as_str())
            .cloned()
            .or_else(|| description.value.clone())
            .unwrap_or(Value::Null);
        let choices = selector_choices(snapshot, &description);
        ui.add_space(8.0);
        ui.heading(friendly_group(&self.selected_group));
        ui.label(RichText::new("MODELS").small().color(theme::DIM));
        ui.add(
            egui::TextEdit::singleline(&mut self.shelf_search)
                .hint_text("Search")
                .desired_width(f32::INFINITY),
        );
        let needle = self.shelf_search.trim().to_ascii_lowercase();
        let referenced_library = description.reference.as_deref().and_then(|reference| {
            snapshot
                .libraries
                .iter()
                .find(|state| state.info.path.as_str() == reference)
        });
        if let Some(state) = referenced_library {
            if matches!(
                state.library,
                Library::Irs | Library::Amps | Library::Drives
            ) {
                self.slot_library_shelf(ui, state, &path, &description, &current, &needle);
                return;
            }
        }

        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for choice in choices {
                    let name = value_text(&choice);
                    if !needle.is_empty() && !name.to_ascii_lowercase().contains(&needle) {
                        continue;
                    }
                    if ui
                        .selectable_label(current == choice, &name)
                        .on_hover_text(format!("choose {name}"))
                        .clicked()
                    {
                        let _ = self.tx.send(Cmd::SetNode {
                            path: path.clone(),
                            description: Box::new(description.clone()),
                            before: current.clone(),
                            value: choice.clone(),
                            persistent: false,
                        });
                        self.drafts.insert(path.to_string(), choice);
                        self.recompute_dirty();
                    }
                }
            });
    }

    /// Installed IR/NAM choices are physical device slots, so show that fact
    /// directly instead of putting a slot-number form above a name-only list.
    fn slot_library_shelf(
        &mut self,
        ui: &mut egui::Ui,
        state: &LibraryState,
        path: &NodePath,
        description: &NodeDescription,
        current: &Value,
        needle: &str,
    ) {
        let library = state.library;
        let current_index = current.as_str().and_then(|selected| {
            state
                .info
                .names
                .iter()
                .position(|name| name.as_deref() == Some(selected))
        });
        if let Some(index) = current_index {
            self.target_slots.insert(library, index + 1);
        }
        ui.add_space(5.0);
        self.slot_count_header(ui, state);
        if let Some(index) = current_index {
            self.slot_tools_ui(ui, state, index);
        } else {
            ui.label(
                RichText::new("Choose an installed model below to manage its slot.")
                    .small()
                    .color(theme::DIM),
            );
        }
        ui.separator();
        self.slot_rows_ui(ui, state, needle, Some((path, description, current)));
    }

    fn device_window(&mut self, ctx: &egui::Context, snapshot: &Snapshot) {
        if !self.show_device {
            return;
        }
        let mut open = true;
        egui::Window::new("Device")
            .open(&mut open)
            .default_width(760.0)
            .default_height(560.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{}  ·  firmware {}  ·  {}",
                        snapshot.identity.name, snapshot.identity.version, snapshot.transport
                    ))
                    .color(theme::DIM),
                );
                ui.horizontal_wrapped(|ui| {
                    tab_button(ui, &mut self.tab, Tab::Library(Library::Irs), "IRs");
                    tab_button(ui, &mut self.tab, Tab::Library(Library::Amps), "NAM amps");
                    tab_button(
                        ui,
                        &mut self.tab,
                        Tab::Library(Library::Drives),
                        "NAM drives",
                    );
                    tab_button(ui, &mut self.tab, Tab::Settings, "Settings");
                    tab_button(ui, &mut self.tab, Tab::Backup, "Backup & restore");
                });
                ui.separator();
                ui.add_enabled_ui(self.online && !self.busy, |ui| match self.tab {
                    Tab::Library(library) => self.library_ui(ui, snapshot, library),
                    Tab::Settings => self.nodes_ui(ui, snapshot, true, None),
                    Tab::Backup => self.backup_ui(ui, snapshot),
                });
            });
        self.show_device = open;
    }

    fn nodes_ui(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        settings: bool,
        group: Option<String>,
    ) {
        if settings {
            return self.settings_ui(ui, snapshot);
        }
        if let Some(group) = group.as_deref() {
            self.pedal_nodes_ui(ui, snapshot, group)
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.horizontal(|ui| {
            ui.heading("Settings");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Filter parameters")
                        .desired_width(220.0),
                );
            });
        });
        if self.rollback.is_none() {
            ui.label(
                RichText::new(
                    "Load and verify a current rollback bundle in Backup & restore before changing global settings.",
                )
                .color(theme::DIM),
            );
        }
        ui.separator();
        let needle = self.search.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (section, heading) in [
                    ("tuner", "Tuner"),
                    ("input", "Input"),
                    ("misc", "General"),
                    ("ctrl", "Controller"),
                ] {
                    let nodes = snapshot
                        .settings
                        .iter()
                        .filter(|(path, description)| {
                            settings_group(path.as_str()) == Some(section)
                                && description.value.as_ref().is_some_and(|value| {
                                    editable_node(path.as_str(), description, value)
                                })
                                && node_matches_search(path.as_str(), description, &needle)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if nodes.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(heading).strong());
                    ui.separator();
                    for (path, description) in nodes {
                        let label = node_label(&path, &description);
                        let path_text = path.to_string();
                        processor::parameter_row(
                            ui,
                            self.rollback.is_some(),
                            &label,
                            &path_text,
                            |ui| self.node_control(ui, snapshot, path, description, true),
                        );
                    }
                }
            });
    }

    fn pedal_nodes_ui(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, group: &str) {
        let title = block_model_name(snapshot, group).unwrap_or_else(|| friendly_group(group));
        let category = group_category(group);
        let persistent = group == INPUT_GROUP;
        ui.horizontal(|ui| {
            ui.heading(&title);
            ui.label(RichText::new(category).color(theme::DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Filter parameters")
                        .desired_width(220.0),
                );
            });
        });
        if persistent && self.rollback.is_none() {
            ui.label(
                RichText::new("Input settings unlock after a rollback backup verifies.")
                    .small()
                    .color(theme::DIM),
            );
        }
        ui.separator();

        let needle = self.search.trim().to_ascii_lowercase();
        let source = if persistent {
            &snapshot.settings
        } else {
            &snapshot.app
        };
        let selector_path = block_selector(snapshot, group).map(|(path, _)| path);
        let nodes = source
            .iter()
            .filter(|(path, description)| {
                let in_group = if persistent {
                    path.as_str().starts_with("root\\settings\\input\\")
                } else {
                    app_group(path.as_str()).as_deref() == Some(group)
                };
                in_group
                    && selector_path.as_ref() != Some(path)
                    && description
                        .value
                        .as_ref()
                        .is_some_and(|value| editable_node(path.as_str(), description, value))
                    && node_matches_search(path.as_str(), description, &needle)
            })
            .cloned()
            .collect::<Vec<_>>();
        let (columns, indent) = processor::control_grid(ui, nodes.len());
        egui::ScrollArea::vertical()
            .id_salt("pro-pedal")
            .show(ui, |ui| {
                if let Some(art) = theme::category_icon(category) {
                    ui.vertical_centered(|ui| {
                        theme::pedal_image(ui, &art, 150.0);
                        ui.label(RichText::new(&title).heading());
                    });
                    ui.add_space(10.0);
                }
                for row in nodes.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        ui.add_space(indent);
                        for (path, description) in row {
                            let Some(current) = self.drafts.get(path.as_str()).cloned() else {
                                continue;
                            };
                            let path = path.clone();
                            let description = description.clone();
                            ui.allocate_ui(processor::CONTROL_CELL, |ui| {
                                ui.vertical_centered(|ui| {
                                    let mut changed = None;
                                    match description.kind.as_ref() {
                                        Some(NodeKind::Float) => {
                                            let min = description.min.unwrap_or(0.0) as f32;
                                            let max = description.max.unwrap_or(100.0) as f32;
                                            let mut number =
                                                current.as_f64().unwrap_or_default() as f32;
                                            let hit = theme::knob(ui, &mut number, min..=max);
                                            if hit.double_clicked() {
                                                if let Some(default) = description
                                                    .default
                                                    .as_ref()
                                                    .and_then(Value::as_f64)
                                                {
                                                    number = default as f32;
                                                    changed = json_number(number as f64);
                                                }
                                            } else if hit.changed() {
                                                let rounded = description
                                                    .step
                                                    .filter(|step| *step > 0.0)
                                                    .map(|step| {
                                                        let base = description.min.unwrap_or(0.0);
                                                        base + ((number as f64 - base) / step)
                                                            .round()
                                                            * step
                                                    })
                                                    .unwrap_or(number as f64);
                                                changed = json_number(rounded);
                                            }
                                            ui.label(
                                                RichText::new(format_node_value(
                                                    &description,
                                                    &Value::from(number as f64),
                                                ))
                                                .monospace()
                                                .color(theme::ACCENT),
                                            );
                                        }
                                        Some(NodeKind::Enum | NodeKind::Array)
                                            if toggle_choices(&description).is_some() =>
                                        {
                                            let (off, on) = toggle_choices(&description)
                                                .expect("guarded above");
                                            let mut enabled = current == on;
                                            if ui.add(theme::switch(&mut enabled)).changed() {
                                                changed = Some(if enabled { on } else { off });
                                            }
                                            ui.label(
                                                RichText::new(value_text(&current))
                                                    .monospace()
                                                    .color(theme::ACCENT),
                                            );
                                        }
                                        Some(
                                            NodeKind::Enum
                                            | NodeKind::Array
                                            | NodeKind::PropertyList,
                                        ) => {
                                            ui.add_space(11.0);
                                            let mut choices = description
                                                .options
                                                .clone()
                                                .or(description.items.clone())
                                                .unwrap_or_default();
                                            if let Some(reference) = &description.reference {
                                                if let Some(library) =
                                                    snapshot.libraries.iter().find(|library| {
                                                        library.info.path.as_str() == reference
                                                    })
                                                {
                                                    choices = library
                                                        .info
                                                        .names
                                                        .iter()
                                                        .filter_map(|name| {
                                                            name.clone().map(Value::String)
                                                        })
                                                        .collect();
                                                }
                                            }
                                            egui::ComboBox::from_id_salt((
                                                "pro-pedal-param",
                                                path.as_str(),
                                            ))
                                            .width(processor::CONTROL_CELL.x)
                                            .selected_text(
                                                RichText::new(value_text(&current))
                                                    .color(theme::ACCENT),
                                            )
                                            .show_ui(
                                                ui,
                                                |ui| {
                                                    for choice in choices {
                                                        if ui
                                                            .selectable_label(
                                                                choice == current,
                                                                value_text(&choice),
                                                            )
                                                            .clicked()
                                                        {
                                                            changed = Some(choice);
                                                        }
                                                    }
                                                },
                                            );
                                            ui.add_space(11.0);
                                        }
                                        Some(NodeKind::Item) if current.is_boolean() => {
                                            let mut enabled = current.as_bool().unwrap_or_default();
                                            if ui.add(theme::switch(&mut enabled)).changed() {
                                                changed = Some(Value::Bool(enabled));
                                            }
                                            ui.label(
                                                RichText::new(if enabled { "On" } else { "Off" })
                                                    .monospace()
                                                    .color(theme::ACCENT),
                                            );
                                        }
                                        _ => {
                                            ui.add_space(24.0);
                                            ui.label(
                                                RichText::new(value_text(&current))
                                                    .monospace()
                                                    .color(theme::ACCENT),
                                            );
                                            ui.add_space(24.0);
                                        }
                                    }
                                    let label = description.desc.as_deref().unwrap_or_else(|| {
                                        path.as_str().rsplit('\\').next().unwrap_or(path.as_str())
                                    });
                                    ui.label(RichText::new(label).small());
                                    if let Some(value) = changed {
                                        if description.validate_value(&value).is_ok() {
                                            self.drafts.insert(path.to_string(), value.clone());
                                            self.recompute_dirty();
                                            let _ = self.tx.send(Cmd::SetNode {
                                                path: path.clone(),
                                                description: Box::new(description.clone()),
                                                before: current.clone(),
                                                value,
                                                persistent,
                                            });
                                        }
                                    }
                                });
                            });
                        }
                    });
                }
            });
    }

    fn node_control(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        path: NodePath,
        description: NodeDescription,
        persistent: bool,
    ) {
        let Some(current) = self.drafts.get(path.as_str()).cloned() else {
            return;
        };
        let mut changed = None;
        match description.kind.as_ref() {
            Some(NodeKind::Float) => {
                let mut number = current.as_f64().unwrap_or_default();
                let response = match (description.min, description.max) {
                    (Some(min), Some(max)) => ui.add(
                        egui::Slider::new(&mut number, min..=max)
                            .step_by(description.step.unwrap_or(0.0).max(0.0))
                            .suffix(
                                description
                                    .unit
                                    .as_deref()
                                    .map(|unit| format!(" {unit}"))
                                    .unwrap_or_default(),
                            ),
                    ),
                    _ => ui.add(egui::DragValue::new(&mut number)),
                };
                if response.changed() {
                    changed = serde_json::Number::from_f64(number).map(Value::Number);
                }
            }
            Some(NodeKind::Enum | NodeKind::Array | NodeKind::PropertyList) => {
                let mut choices = description
                    .options
                    .clone()
                    .or(description.items.clone())
                    .unwrap_or_default();
                if let Some(reference) = &description.reference {
                    if let Some(library) = snapshot
                        .libraries
                        .iter()
                        .find(|library| library.info.path.as_str() == reference)
                    {
                        choices = library
                            .info
                            .names
                            .iter()
                            .filter_map(|name| name.clone().map(Value::String))
                            .collect();
                    }
                }
                egui::ComboBox::from_id_salt(("pro-node", path.as_str()))
                    .selected_text(value_text(&current))
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for choice in choices {
                            if ui
                                .selectable_label(choice == current, value_text(&choice))
                                .clicked()
                            {
                                changed = Some(choice);
                            }
                        }
                    });
            }
            Some(NodeKind::Item) if current.is_boolean() => {
                let mut value = current.as_bool().unwrap_or_default();
                if ui.checkbox(&mut value, "").changed() {
                    changed = Some(Value::Bool(value));
                }
            }
            Some(NodeKind::Item) if current.is_string() => {
                let mut value = current.as_str().unwrap_or_default().to_owned();
                let response = ui.add(egui::TextEdit::singleline(&mut value).desired_width(210.0));
                if response.lost_focus() && response.changed() {
                    changed = Some(Value::String(value));
                }
            }
            _ => {
                ui.label(
                    RichText::new(value_text(&current))
                        .monospace()
                        .color(theme::DIM),
                );
            }
        }
        if let Some(value) = changed {
            if description.validate_value(&value).is_ok() {
                self.drafts.insert(path.to_string(), value.clone());
                self.recompute_dirty();
                let _ = self.tx.send(Cmd::SetNode {
                    path,
                    description: Box::new(description),
                    before: current,
                    value,
                    persistent,
                });
            }
        }
    }

    fn library_ui(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, library: Library) {
        let Some(state) = snapshot
            .libraries
            .iter()
            .find(|state| state.library == library)
        else {
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(library.title());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search = self.library_searches.entry(library).or_default();
                ui.add(
                    egui::TextEdit::singleline(search)
                        .hint_text("Search slots")
                        .desired_width(220.0),
                );
            });
        });
        if library == Library::Presets {
            ui.horizontal(|ui| {
                ui.label("Save live state as");
                ui.add(egui::TextEdit::singleline(&mut self.save_name).desired_width(230.0));
                if ui
                    .add_enabled(
                        self.rollback.is_some() && !self.save_name.trim().is_empty(),
                        egui::Button::new("Save preset"),
                    )
                    .on_disabled_hover_text("load a rollback bundle first")
                    .clicked()
                {
                    let _ = self.tx.send(Cmd::SavePreset(self.save_name.trim().into()));
                }
            });
        }
        self.slot_count_header(ui, state);
        let selected = self
            .target_slots
            .entry(library)
            .or_insert(1)
            .saturating_sub(1)
            .min(state.info.count.saturating_sub(1));
        self.slot_tools_ui(ui, state, selected);
        ui.separator();
        let needle = self
            .library_searches
            .get(&library)
            .map_or("", String::as_str)
            .trim()
            .to_ascii_lowercase();
        self.slot_rows_ui(ui, state, &needle, None);
    }

    fn slot_count_header(&self, ui: &mut egui::Ui, state: &LibraryState) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("SLOTS").small().color(theme::DIM));
            let occupied = state.info.occupied().count();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "{occupied} installed · {} empty",
                        state.info.count.saturating_sub(occupied)
                    ))
                    .small()
                    .color(theme::DIM),
                );
            });
        });
    }

    fn slot_tools_ui(&mut self, ui: &mut egui::Ui, state: &LibraryState, index: usize) {
        let library = state.library;
        let occupied = state.info.names.get(index).and_then(Option::as_deref);
        ui.add_space(3.0);
        ui.label(
            RichText::new(format!("SLOT {}", index + 1))
                .small()
                .color(theme::DIM),
        );
        let Some(original) = occupied else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Empty and ready for an import").color(theme::DIM));
                if ui
                    .add_enabled(self.rollback.is_some(), egui::Button::new("Import here…"))
                    .on_disabled_hover_text("automatic backup must be ready first")
                    .clicked()
                {
                    self.import_into_slot(state, index);
                }
            });
            return;
        };

        let key = (library, index);
        let editing_name = self.slot_renaming == Some(key);
        let mut draft = self
            .names
            .get(&key)
            .cloned()
            .unwrap_or_else(|| original.to_owned());
        if editing_name {
            if ui
                .add(egui::TextEdit::singleline(&mut draft).hint_text("Slot name"))
                .changed()
            {
                self.names.insert(key, draft.clone());
            }
        } else {
            ui.label(RichText::new(original).strong());
        }

        let mut begin_rename = false;
        let mut save_rename = false;
        let mut cancel_rename = false;
        let mut replace = false;
        let mut export = false;
        let mut up = false;
        let mut down = false;
        let mut remove = false;
        let mut export_pair = false;
        ui.horizontal_wrapped(|ui| {
            if editing_name {
                save_rename = ui
                    .add_enabled(
                        self.rollback.is_some() && !draft.trim().is_empty() && draft != original,
                        egui::Button::new("Save name"),
                    )
                    .clicked();
                cancel_rename = ui.button("Cancel").clicked();
            } else {
                begin_rename = ui
                    .add_enabled(self.rollback.is_some(), egui::Button::new("Rename"))
                    .clicked();
                replace = ui
                    .add_enabled(self.rollback.is_some(), egui::Button::new("Replace…"))
                    .clicked();
                export = ui.button("Export…").clicked();
                if library == Library::Irs
                    && state.info.names.get(index + 1).is_some_and(Option::is_some)
                {
                    export_pair = ui
                        .button("Export pair…")
                        .on_hover_text(format!(
                            "export slots {} and {} as a stereo WAV",
                            index + 1,
                            index + 2
                        ))
                        .clicked();
                }
                if state.info.movable {
                    up = ui
                        .add_enabled(self.rollback.is_some() && index > 0, egui::Button::new("↑"))
                        .on_hover_text("move this slot up")
                        .clicked();
                    down = ui
                        .add_enabled(
                            self.rollback.is_some() && index + 1 < state.info.count,
                            egui::Button::new("↓"),
                        )
                        .on_hover_text("move this slot down")
                        .clicked();
                }
                remove = ui
                    .add_enabled(self.rollback.is_some(), egui::Button::new("Remove"))
                    .clicked();
            }
        });

        if begin_rename {
            self.names.insert(key, original.to_owned());
            self.slot_renaming = Some(key);
        }
        if save_rename {
            let _ = self.tx.send(Cmd::Rename {
                library,
                index,
                name: draft.trim().into(),
            });
            self.slot_renaming = None;
        }
        if cancel_rename {
            self.names.insert(key, original.to_owned());
            self.slot_renaming = None;
        }
        if replace {
            self.import_into_slot(state, index);
        }
        if export {
            let stem = sanitise(if draft.is_empty() { "slot" } else { &draft });
            if let Some(file) = rfd::FileDialog::new()
                .set_file_name(format!("{stem}.{}", library.extension()))
                .save_file()
            {
                let _ = self.tx.send(Cmd::Export {
                    library,
                    index,
                    file,
                });
            }
        }
        if export_pair {
            let stem = sanitise(if draft.is_empty() {
                "stereo-ir"
            } else {
                &draft
            });
            if let Some(file) = rfd::FileDialog::new()
                .set_file_name(format!("{stem}.wav"))
                .save_file()
            {
                let _ = self.tx.send(Cmd::ExportStereo {
                    left: index,
                    right: index + 1,
                    file,
                });
            }
        }
        if up {
            let _ = self.tx.send(Cmd::Move {
                library,
                from: index,
                to: index - 1,
            });
        }
        if down {
            let _ = self.tx.send(Cmd::Move {
                library,
                from: index,
                to: index + 1,
            });
        }
        if remove {
            self.confirmation = Some(Confirmation {
                question: format!(
                    "Remove {original} from {} slot {}?",
                    library.title(),
                    index + 1
                ),
                command: Cmd::Clear { library, index },
            });
        }
    }

    fn slot_rows_ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &LibraryState,
        needle: &str,
        selector: Option<(&NodePath, &NodeDescription, &Value)>,
    ) {
        let library = state.library;
        let managed = self.target_slots.get(&library).copied().unwrap_or(1);
        let mut chose = None;
        let mut import = None;
        let mut shown = 0usize;
        egui::ScrollArea::vertical()
            .id_salt(("pro-slot-list", library.path()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in 0..state.info.count {
                    let name = state.info.names.get(index).and_then(Option::as_deref);
                    if !slot_matches_search(index, name, needle) {
                        continue;
                    }
                    shown += 1;
                    ui.horizontal(|ui| {
                        ui.set_min_height(24.0);
                        ui.label(
                            RichText::new(format!("{:>2}", index + 1))
                                .monospace()
                                .color(theme::DIM),
                        );
                        let selected = selector
                            .and_then(|(_, _, current)| current.as_str())
                            .zip(name)
                            .is_some_and(|(current, name)| current == name)
                            || (selector.is_none() && managed == index + 1);
                        let text = match name {
                            Some(name) => RichText::new(name),
                            None => RichText::new("Empty slot").italics().color(theme::DIM),
                        };
                        if ui.selectable_label(selected, text).clicked() {
                            chose = Some(index);
                        }
                        if name.is_none()
                            && ui
                                .add_enabled(
                                    self.rollback.is_some(),
                                    egui::Button::new("Import…").min_size(egui::vec2(68.0, 20.0)),
                                )
                                .on_disabled_hover_text("automatic backup must be ready first")
                                .clicked()
                        {
                            import = Some(index);
                        }
                    });
                }
            });
        if shown == 0 {
            ui.label(RichText::new("No slots match this search.").color(theme::DIM));
        }
        if let Some(index) = chose {
            if self.slot_renaming != Some((library, index)) {
                self.slot_renaming = None;
            }
            self.target_slots.insert(library, index + 1);
            if let (Some((path, description, current)), Some(name)) = (
                selector,
                state.info.names.get(index).and_then(Option::as_deref),
            ) {
                let value = Value::String(name.to_owned());
                if description.validate_value(&value).is_ok() && value != *current {
                    let _ = self.tx.send(Cmd::SetNode {
                        path: path.clone(),
                        description: Box::new(description.clone()),
                        before: current.clone(),
                        value: value.clone(),
                        persistent: false,
                    });
                    self.drafts.insert(path.to_string(), value);
                    self.recompute_dirty();
                }
            }
        }
        if let Some(index) = import {
            self.import_into_slot(state, index);
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

    fn backup_ui(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.heading("Backup & restore");
        ui.label(
            RichText::new(
                "Bundles contain exact preset, stereo IR, NAM amp/drive bytes and safe device settings. They are private on disk and verified before use.",
            )
            .color(theme::DIM),
        );
        ui.add_space(12.0);
        if ui.button("Capture complete backup…").clicked() {
            if let Some(parent) = rfd::FileDialog::new().pick_folder() {
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|time| time.as_secs())
                    .unwrap_or_default();
                let name = format!(
                    "stompstation-pro-{}-{stamp}.vxbundle",
                    snapshot.identity.version
                );
                let _ = self.tx.send(Cmd::Backup(parent.join(name)));
            }
        }
        ui.add_space(8.0);
        if ui.button("Load current rollback bundle…").clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let _ = self.tx.send(Cmd::UseRollback(path));
            }
        }
        if let Some(path) = &self.rollback {
            ui.label(RichText::new(path.display().to_string()).monospace());
        } else {
            ui.label(
                RichText::new(
                    "Live editing remains available. Save and other persistent controls unlock after a bundle is verified against the pedal's current names, boundary chunks, settings and firmware.",
                )
                .color(theme::DIM),
            );
        }
        ui.add_space(16.0);
        if ui
            .add_enabled(
                self.rollback.is_some(),
                egui::Button::new("Restore a bundle…"),
            )
            .on_disabled_hover_text("load a current rollback bundle first")
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.confirmation = Some(Confirmation {
                    question: format!(
                        "Restore every library and safe setting from {}?",
                        path.display()
                    ),
                    command: Cmd::Restore(path),
                });
            }
        }
    }

    fn confirmation_window(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let question = confirmation.question.clone();
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Confirm device write")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(question);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    confirm = ui.button("Write to pedal").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if confirm {
            if let Some(confirmation) = self.confirmation.take() {
                let _ = self.tx.send(confirmation.command);
            }
        } else if cancel {
            self.confirmation = None;
        }
    }
}

fn tab_button(ui: &mut egui::Ui, tab: &mut Tab, value: Tab, label: &str) {
    if ui.selectable_label(*tab == value, label).clicked() {
        *tab = value;
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
    let reading = if description.step.is_some_and(|step| step >= 1.0) {
        format!("{number:.0}")
    } else {
        format!("{number:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
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
        }
    }

    fn run(mut self) {
        let mut pending = None;
        loop {
            let command = match pending.take() {
                Some(command) => command,
                None => match self.commands.recv() {
                    Ok(command) => command,
                    Err(_) => break,
                },
            };
            let (command, next) = self.coalesce_live_edits(command);
            pending = next;
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
                    self.send(Evt::Disconnected);
                }
            }
            if show_busy {
                self.send(Evt::Busy(false));
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
            Cmd::Connect => self.connect(),
            Cmd::Disconnect => {
                self.restore_audition()?;
                self.device = None;
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
                let device = self.device()?;
                let rollback = bundle.arm(device)?;
                device.enable_writes()?;
                self.rollback = Some(rollback);
                self.rollback_path = Some(path.clone());
                self.send(Evt::Guarded(path));
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
                let hash = crate::library::hash_of(&bytes);
                let blob = import_blob(Library::Presets, &bytes, list.size)?;
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
                            let hash = crate::library::hash_of(&bytes);
                            let blob = import_blob(Library::Presets, &bytes, list.size)?;
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
        let found = voidx_client::list()?
            .into_iter()
            .next()
            .ok_or_else(|| WorkError::Other("No StompStation PRO found".into()))?;
        let mut device = Device::connect(found.open()?)?;
        device.enable_writes()?;
        let snapshot = snapshot(&mut device)?;
        let can_refresh_backup = latest_verified_backup(&snapshot.identity).is_some();
        let version = snapshot.identity.version.clone();
        self.device = Some(device);
        self.forget_history();
        self.send(Evt::Connected(snapshot));
        // Matching a rollback checks flash boundaries and global settings. It
        // is important, but it should not make a healthy pedal look absent
        // while it runs: publish the live editor first, then unlock writes.
        let guarded = self.device.as_mut().and_then(latest_matching_rollback);
        if let Some((path, rollback)) = guarded {
            self.rollback = Some(rollback);
            self.rollback_path = Some(path.clone());
            self.send(Evt::Guarded(path));
        } else {
            self.rollback = None;
            self.rollback_path = None;
            if can_refresh_backup {
                let Some(directory) = hx_catalog::home::backups() else {
                    self.send(Evt::Success(
                        "Live editing ready · choose Backup to unlock persistent controls".into(),
                    ));
                    return Ok(());
                };
                let captured = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| WorkError::Other(error.to_string()))?
                    .as_secs();
                let date = jiff::Timestamp::now().strftime("%Y%m%d").to_string();
                let path = directory.join(format!("stompstation-pro-{version}-{date}.vxbundle"));
                self.capture_backup(&path, captured)?;
            } else {
                self.send(Evt::Success(
                    "Live editing ready · choose Backup to unlock Save and device libraries".into(),
                ));
            }
        }
        Ok(())
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
        let rollback = backup::open_verified(path)?.arm(self.device()?)?;
        self.rollback = Some(rollback);
        self.rollback_path = Some(path.to_owned());
        self.send(Evt::Guarded(path.to_owned()));
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
                hash: Some(crate::library::hash_of(&bytes)),
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
        let mut list = self.list(Library::Irs)?;
        let blobs = voidx_client::ir::Wav::parse(&source)?.device_blobs(list.size)?;
        if blobs.len() != 2 {
            return Err(WorkError::Other(
                "Stereo import needs a two-channel WAV".into(),
            ));
        }
        let left_name = unique_slot_name(&list, left, &format!("{name} L"));
        list.names[left] = Some(left_name.clone());
        let right_name = unique_slot_name(&list, right, &format!("{name} R"));
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
fn latest_matching_rollback(device: &mut Device<SerialLink>) -> Option<(PathBuf, ArmedRollback)> {
    for path in backup_candidates()? {
        let Ok(bundle) = backup::open_verified(&path) else {
            continue;
        };
        if let Ok(rollback) = bundle.arm(device) {
            return Some((path, rollback));
        }
    }
    None
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
}
