//! The StompStation PRO in the shared frame: its presets in the sidebar, its
//! preset in the deck, its protection in the foot, and its page.
//!
//! The PRO's rule is stricter than the HX's: nothing is written to its memory
//! until TonePush holds a checked backup of this very pedal (the rollback
//! guard). The frame says so where the question is asked: the deck's state
//! line, the sidebar's foot, the Save button's reason, and the menu rows that
//! wait for it.

use egui::{Pos2, Rect, Sense, Ui, Vec2};

use super::*;
use crate::shell::{self, DeviceCard, Foot, Mini, MiniAsked, PresetRow, RowMode, State};
use crate::theme::{Icon, Mood, Tier};

/// What the PRO's preset list asked of the app.
pub(crate) enum Picked {
    /// The slot a tone being sent should go to.
    Slot(usize),
    /// Stop sending.
    Cancel,
}

/// A preset slot as the pedal's home screen shows it: three to a bank, 01A
/// to 20C.
pub(crate) fn slot_label(index: usize) -> String {
    format!(
        "{:02}{}",
        index / 3 + 1,
        char::from(b'A' + (index % 3) as u8)
    )
}

/// The tabs of the PRO's page.
const TABS: [Tab; 6] = [
    Tab::Backups,
    Tab::Library(Library::Amps),
    Tab::Library(Library::Drives),
    Tab::Library(Library::Irs),
    Tab::Settings,
    Tab::Firmware,
];

impl Panel {
    fn presets(&self) -> Option<&LibraryState> {
        self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .libraries
                .iter()
                .find(|state| state.library == Library::Presets)
        })
    }

    /// Why a write to the pedal's memory cannot happen now, if it cannot.
    pub(crate) fn write_refusal(&self) -> Option<String> {
        if !self.online {
            return Some("Connect the pedal first".to_owned());
        }
        if let Some(reason) = &self.read_only {
            return Some(format!("Read only: {reason}"));
        }
        if self.rollback.is_none() {
            return Some("Waits for a checked backup of this pedal".to_owned());
        }
        None
    }

    /// The rows of the PRO preset list.
    fn rows(&self, lookup: &LibraryLookup, config: &config::Config) -> Vec<PresetRow> {
        let Some(presets) = self.presets() else {
            return Vec::new();
        };
        let active = self.snapshot.as_ref().and_then(active_preset_index);
        let mut rows: Vec<PresetRow> = presets
            .info
            .names
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                !self.show_favorites_only || config.is_favorite(FAVORITES_SETLIST, *index as i64)
            })
            .map(|(index, name)| {
                let selected = active == Some(index);
                PresetRow {
                    index,
                    slot: slot_label(index),
                    name: name.clone().unwrap_or_default(),
                    empty: name.is_none(),
                    selected,
                    edited: selected && self.dirty,
                    favourite: config.is_favorite(FAVORITES_SETLIST, index as i64),
                    library: if name.is_some() {
                        self.slot_sync(index, lookup)
                    } else {
                        theme::Sync::Unknown
                    },
                    bank_start: index % 3 == 0,
                }
            })
            .collect();
        for (position, row) in rows.iter_mut().enumerate() {
            if position == 0 || self.show_favorites_only {
                row.bank_start = false;
            }
        }
        rows
    }

    /// The sidebar's presets for a PRO: the header, the picking card while a
    /// tone is on its way, and the list with each preset's menu.
    pub(crate) fn sidebar(
        &mut self,
        ui: &mut Ui,
        lookup: &LibraryLookup,
        config: &mut config::Config,
        sending: Option<&str>,
    ) -> Option<Picked> {
        let mut picked = None;
        if let Some(name) = sending {
            if shell::sending_card(ui, name) {
                picked = Some(Picked::Cancel);
            }
        }
        let rows = self.rows(lookup, config);
        let (aside, hint) = match self.presets() {
            None => (String::new(), String::new()),
            Some(presets) if sending.is_some() => {
                let free = presets
                    .info
                    .names
                    .iter()
                    .filter(|name| name.is_none())
                    .count();
                (
                    format!("{free} free"),
                    "Slots that hold nothing yet".to_owned(),
                )
            }
            Some(presets) => {
                let kept = (0..presets.info.count)
                    .filter(|&index| {
                        presets.info.names.get(index).is_some_and(Option::is_some)
                            && matches!(
                                self.slot_sync(index, lookup),
                                theme::Sync::Same | theme::Sync::Differs
                            )
                    })
                    .count();
                (
                    format!("{kept} of {}", presets.info.count),
                    format!(
                        "{kept} of the pedal's {} presets are in your library",
                        presets.info.count
                    ),
                )
            }
        };
        let asked = shell::presets_header(ui, &aside, &hint, self.show_favorites_only, self.online);
        if asked.favourites {
            self.show_favorites_only = !self.show_favorites_only;
        }
        // Asked through the app, which keeps every capture of the pedal
        // in one place.
        if asked.capture {
            self.capture_asked = true;
        }

        let mode = if sending.is_some() {
            RowMode::Sending
        } else {
            RowMode::Normal
        };
        let empty = if self.show_favorites_only && self.presets().is_some() {
            "No favourites yet. Add one from a preset's menu."
        } else {
            "The pedal's presets appear here, in its own banks, as soon as it connects."
        };
        let guarded = self.rollback.is_some() && self.read_only.is_none();
        let refusal = self.write_refusal().unwrap_or_default();
        let count = self.presets().map_or(0, |presets| presets.info.count);
        let clipboard = self.clipboard.as_ref().map(|(name, _)| name.clone());
        let mut select = None;
        let mut toggle = None;
        let mut read = None;
        let mut rename = None;
        let mut rename_start = None;
        let mut renaming = self.renaming_preset.clone();
        let mut command = None;
        let mut confirm = None;
        let mut copy = None;
        let mut paste = None;
        let mut export = None;
        let mut import = None;
        shell::preset_list(ui, "pro-presets", empty, &rows, |ui, row| {
            let index = row.index;
            if mode == RowMode::Normal {
                if let Some((editing, draft)) = renaming.as_mut() {
                    if *editing == index {
                        let field = ui.add_sized(
                            Vec2::new(ui.available_width(), shell::ROW_HEIGHT),
                            egui::TextEdit::singleline(draft).hint_text("preset name"),
                        );
                        if !field.has_focus() && !field.lost_focus() {
                            field.request_focus();
                        }
                        if field.lost_focus() {
                            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                                rename = Some((index, draft.clone()));
                            }
                            rename_start = Some(None);
                        }
                        return;
                    }
                }
            }
            let response = shell::preset_row(ui, row, mode);
            if mode == RowMode::Sending {
                if response.clicked() {
                    picked = Some(Picked::Slot(index));
                }
                return;
            }
            // An empty slot has nothing to load and nothing to act on.
            if row.empty {
                return;
            }
            if response.clicked() {
                select = Some(index);
            }
            response.context_menu(|ui| {
                theme::menu_width(ui, 256.0);
                theme::menu_header(
                    ui,
                    &format!("{} {}", row.slot, row.name),
                    Some("on the pedal"),
                );
                let guarded_item = |ui: &mut Ui, icon: Icon, text: &str| {
                    if guarded {
                        theme::menu_item(ui, Some(icon), text, None).clicked()
                    } else {
                        theme::menu_disabled(ui, Some(icon), text, Some("after a backup"))
                            .on_hover_text(&refusal);
                        false
                    }
                };
                if guarded_item(ui, Icon::TextCursorInput, "Rename") {
                    rename_start = Some(Some((index, row.name.clone())));
                }
                if theme::menu_item(ui, Some(Icon::Copy), "Copy", None).clicked() {
                    copy = Some(index);
                }
                match &clipboard {
                    Some(copied) => {
                        if guarded_item(ui, Icon::Paste, &format!("Paste {copied} here")) {
                            paste = Some(index);
                        }
                    }
                    None => {
                        theme::menu_disabled(
                            ui,
                            Some(Icon::Paste),
                            "Paste",
                            Some("nothing copied"),
                        );
                    }
                }
                theme::menu_separator(ui);
                match row.library {
                    theme::Sync::Same => {
                        theme::menu_disabled(ui, Some(Icon::Computer), "In your library", None);
                    }
                    theme::Sync::Differs => {
                        if theme::menu_item(
                            ui,
                            Some(Icon::Computer),
                            "Update in library",
                            Some("differs"),
                        )
                        .clicked()
                        {
                            read = Some((index, true));
                        }
                    }
                    _ => {
                        if theme::menu_item(ui, Some(Icon::Computer), "Keep in library", None)
                            .clicked()
                        {
                            read = Some((index, false));
                        }
                    }
                }
                if theme::menu_item(ui, Some(Icon::Download), "Save to file…", None).clicked() {
                    export = Some((index, row.name.clone()));
                }
                if guarded_item(ui, Icon::Upload, "Load from file…") {
                    import = Some(index);
                }
                if index > 0 && guarded_item(ui, Icon::ChevronUp, "Move up") {
                    command = Some(Cmd::Move {
                        library: Library::Presets,
                        from: index,
                        to: index - 1,
                    });
                }
                if index + 1 < count && guarded_item(ui, Icon::ChevronDown, "Move down") {
                    command = Some(Cmd::Move {
                        library: Library::Presets,
                        from: index,
                        to: index + 1,
                    });
                }
                let favourite = if row.favourite {
                    "Remove from favourites"
                } else {
                    "Add to favourites"
                };
                if theme::menu_item(ui, Some(Icon::Star), favourite, None).clicked() {
                    toggle = Some(index);
                }
                theme::menu_separator(ui);
                if guarded {
                    if theme::menu_danger(ui, Icon::Remove, "Empty this slot…").clicked() {
                        confirm = Some(Confirmation {
                            question: format!("Empty {} {}?", row.slot, row.name),
                            action: "Empty the slot",
                            command: Cmd::Clear {
                                library: Library::Presets,
                                index,
                            },
                        });
                    }
                } else {
                    theme::menu_disabled(
                        ui,
                        Some(Icon::Remove),
                        "Empty this slot…",
                        Some("after a backup"),
                    )
                    .on_hover_text(&refusal);
                }
            });
        });
        match rename_start {
            Some(started) => self.renaming_preset = started,
            None => self.renaming_preset = renaming,
        }
        if let Some((index, name)) = rename {
            let _ = self.tx.send(Cmd::Rename {
                library: Library::Presets,
                index,
                name,
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
        if let Some(index) = copy {
            let _ = self.tx.send(Cmd::ReadPreset {
                index,
                target: ReadTarget::Clipboard,
            });
        }
        if let Some(index) = paste {
            if let Some((name, bytes)) = self.clipboard.clone() {
                let _ = self.tx.send(Cmd::ImportBytes { index, name, bytes });
            }
        }
        if let Some((index, name)) = export {
            let stem = sanitise(if name.is_empty() { "preset" } else { &name });
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(format!("{stem}.vxpreset"))
                .add_filter("StompStation PRO preset", &["vxpreset"])
                .save_file()
            {
                let _ = self.tx.send(Cmd::ReadPreset {
                    index,
                    target: ReadTarget::File(path),
                });
            }
        }
        if let Some(index) = import {
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("StompStation PRO preset", &["vxpreset"])
                .pick_file()
            {
                let name = file
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("Preset")
                    .to_owned();
                confirm = Some(Confirmation {
                    question: format!("Replace {} with {name}?", slot_label(index)),
                    action: "Write to the pedal",
                    command: Cmd::Import {
                        library: Library::Presets,
                        index,
                        name,
                        file,
                    },
                });
            }
        }
        if let Some(command) = command {
            let _ = self.tx.send(command);
        }
        if confirm.is_some() {
            self.confirmation = confirm;
        }
        if let Some(index) = select {
            self.select_preset(index);
        }
        // Escape gets out of picking, as the card says.
        if sending.is_some() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            picked = Some(Picked::Cancel);
        }
        picked
    }

    /// The device card's words for the PRO.
    pub(crate) fn device_card(&self) -> DeviceCard {
        let name = match self.device_name() {
            "" => "StompStation PRO".to_owned(),
            name => name.to_owned(),
        };
        let updating = self.firmware.as_ref().map(|flow| flow.stage);
        let status = if self.update_mode {
            "Update Mode".to_owned()
        } else if let Some(stage) = updating.filter(|_| !self.online) {
            use super::firmware::Stage;
            match stage {
                Stage::AwaitingUpdateMode | Stage::SwitchedOff => "Switched off".to_owned(),
                Stage::Settling => "Writing its firmware".to_owned(),
                Stage::Failed => "Not answering".to_owned(),
                _ => "Updating its firmware".to_owned(),
            }
        } else if self.online {
            match self.firmware() {
                "" => "Connected".to_owned(),
                version => format!("Connected · {version}"),
            }
        } else {
            "Not connected".to_owned()
        };
        DeviceCard {
            name,
            status,
            online: self.online || self.update_mode,
        }
    }

    /// The sidebar's foot: whether the pedal is protected.
    pub(crate) fn foot(&self) -> Foot {
        // While an update has the pedal, the backup it stands on.
        if let Some(backup) = self
            .firmware
            .as_ref()
            .and_then(|flow| flow.backup.as_ref())
            .filter(|_| !self.online)
        {
            return Foot {
                icon: Some(Icon::ShieldCheck),
                text: shell::when_words(
                    "Backed up",
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(backup.captured),
                ),
                mood: Mood::Ok,
                hint: format!(
                    "The update stands on {}, a checked backup of the pedal on firmware {}",
                    backup.path.display(),
                    backup.version
                ),
            };
        }
        if let Some(reason) = &self.read_only {
            return Foot {
                icon: Some(Icon::Lock),
                text: "Read only".to_owned(),
                mood: Mood::Hot,
                hint: reason.clone(),
            };
        }
        match &self.rollback {
            Some(path) => Foot {
                icon: Some(Icon::ShieldCheck),
                text: match self.rollback_time.and_then(shell::clock) {
                    Some((when, _)) => format!("Protected · backup {when}"),
                    None => "Protected".to_owned(),
                },
                mood: Mood::Ok,
                hint: format!(
                    "TonePush can put back anything it writes, from {}",
                    path.display()
                ),
            },
            None => Foot {
                icon: Some(Icon::ShieldAlert),
                text: "Not protected yet".to_owned(),
                mood: Mood::Hot,
                hint: "Saving, renaming and importing wait for a checked backup of this pedal"
                    .to_owned(),
            },
        }
    }

    /// Whether Save can write now, and why not.
    fn save_state(&self) -> (bool, String) {
        if let Some(refusal) = self.write_refusal() {
            return (false, format!("Saving: {}", refusal.to_lowercase()));
        }
        if self.save_name.trim().is_empty() {
            return (false, "The preset needs a name".to_owned());
        }
        (self.dirty, "Nothing to save".to_owned())
    }

    /// The deck's second line for the PRO.
    fn state(&self, tier: Tier) -> Vec<State> {
        let mut parts = Vec::new();
        if !self.online {
            parts.push(State::Problem(if self.status.is_empty() {
                "StompStation PRO disconnected".to_owned()
            } else {
                self.status.clone()
            }));
            return parts;
        }
        if let Some((what, progress)) = &self.working {
            parts.push(State::Busy(format!("{what} · {:.0}%", progress * 100.0)));
        } else if self.busy {
            parts.push(State::Busy(if self.status.is_empty() {
                "Talking to the pedal".to_owned()
            } else {
                self.status.clone()
            }));
        } else if self.failed && !self.status.is_empty() {
            parts.push(State::Problem(self.status.clone()));
        }
        if self.dirty {
            parts.push(State::Unsaved("Changes not saved".to_owned()));
        }
        if let Some(version) = self.read_only.as_ref().map(|_| self.firmware()) {
            parts.push(State::Warning(
                Icon::Lock,
                format!("Read only on firmware {version}"),
            ));
        } else if self.rollback.is_some() {
            parts.push(State::Good(
                Icon::ShieldCheck,
                match self.rollback_time.and_then(shell::clock) {
                    Some((when, true)) => format!("Protected by the {when} backup"),
                    Some((when, false)) => format!("Protected by the backup of {when}"),
                    None => "Protected by a checked backup".to_owned(),
                },
            ));
        } else {
            parts.push(State::Warning(
                Icon::ShieldAlert,
                "Saving waits for a backup of this pedal".to_owned(),
            ));
        }
        let quiet = !self.busy && !self.failed && self.working.is_none();
        if quiet && !self.status.is_empty() && tier != Tier::S {
            parts.push(State::Note(self.status.clone()));
        }
        parts
    }

    /// The tempo node, when this firmware has one, and its value now.
    fn tempo_node(&self) -> Option<(NodePath, NodeDescription, f32)> {
        let snapshot = self.snapshot.as_ref()?;
        let (path, description) = snapshot.app.iter().find(|(path, description)| {
            path.as_str().ends_with("\\tempo_bpm")
                && matches!(description.kind, Some(NodeKind::Float))
        })?;
        let tempo = self.drafts.get(path.as_str()).and_then(Value::as_f64)? as f32;
        Some((path.clone(), description.clone(), tempo))
    }

    /// Set the tempo, live: a tempo is part of the edit buffer.
    ///
    /// The field takes whatever parses and a tap can come out anywhere, so a
    /// tempo outside the range the pedal advertised for the node (or no
    /// number at all) is answered here instead of being sent. The reading
    /// changes when the pedal says it has the value: shown at once, a refused
    /// tempo stayed on screen as though it had been set.
    fn set_tempo(&mut self, path: NodePath, description: NodeDescription, bpm: f32) {
        let value = Value::from(f64::from(bpm));
        if !bpm.is_finite() || description.validate_value(&value).is_err() {
            self.failed = true;
            self.status = tempo_refusal(&description);
            return;
        }
        let before = self
            .drafts
            .get(path.as_str())
            .cloned()
            .unwrap_or(Value::Null);
        if self.failed && self.status == tempo_refusal(&description) {
            self.failed = false;
            self.status.clear();
        }
        let _ = self.tx.send(Cmd::SetNode {
            path,
            description: Box::new(description),
            before,
            value,
            persistent: false,
        });
    }

    /// The deck for a PRO preset. Returns whether the hidden sidebar was
    /// asked back.
    pub(crate) fn deck(&mut self, ui: &mut Ui, tier: Tier, sidebar_hidden: bool) -> bool {
        let snapshot = self.snapshot.clone();
        let mut show_sidebar = false;
        let full = ui.max_rect();
        let active = snapshot.as_ref().and_then(active_preset_index);
        let (can_save, refused) = self.save_state();
        let right = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(full.with_max_x(full.right() - 16.0))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    let Some(snapshot) = &snapshot else {
                        return;
                    };
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let asked = shell::deck_actions(
                        ui,
                        &shell::DeckActions {
                            tier,
                            live: self.online,
                            undo: self.undo_depth,
                            redo: self.redo_depth,
                            dirty: self.dirty,
                            can_save,
                            save_refused: &refused,
                            unlock: self.unlock_offer(),
                        },
                    );
                    if asked.save {
                        let _ = self.tx.send(Cmd::SavePreset(self.save_name.trim().into()));
                    }
                    if asked.unlock {
                        self.back_up_here();
                    }
                    if asked.undo {
                        let _ = self.tx.send(Cmd::Undo);
                    }
                    if asked.redo {
                        let _ = self.tx.send(Cmd::Redo);
                    }
                    if asked.discard {
                        self.discard_changes(snapshot);
                    }
                    if let Some((path, description, bpm)) = self.tempo_node() {
                        ui.add_space(8.0);
                        shell::rule(ui);
                        ui.add_space(8.0);
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let set = shell::tempo(ui, bpm, &mut self.tempo_draft, &mut self.taps);
                        if let Some(bpm) = set {
                            self.set_tempo(path, description, bpm);
                        }
                    }
                },
            )
            .response
            .rect;
        let left = full.left() + if sidebar_hidden { 12.0 } else { 20.0 };
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(left, full.top()),
                    Pos2::new(right.left() - 14.0, full.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if sidebar_hidden && shell::sidebar_toggle(ui) {
                    show_sidebar = true;
                }
                if let Some(index) = active {
                    shell::slot_chip(ui, &slot_label(index));
                }
                let width = ui.available_width();
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 44.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let size = tier.pick(theme::PRESET_NAME, theme::PRESET_NAME, 22.0);
                        let name = snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.active_preset.clone())
                            .unwrap_or_else(|| "StompStation PRO".to_owned());
                        let can_rename = active.is_some() && self.write_refusal().is_none();
                        let refusal = self
                            .write_refusal()
                            .map(|why| format!("Renaming: {}", why.to_lowercase()))
                            .unwrap_or_default();
                        if let Some(typed) = shell::deck_title(
                            ui,
                            &name,
                            size,
                            width,
                            &mut self.renaming_header,
                            can_rename,
                            &refusal,
                        ) {
                            if let Some(index) = active {
                                let _ = self.tx.send(Cmd::Rename {
                                    library: Library::Presets,
                                    index,
                                    name: typed,
                                });
                            }
                        }
                        let parts = self.state(tier);
                        shell::state_line(ui, &parts, width);
                    },
                );
            },
        );
        show_sidebar
    }

    /// The loaded PRO preset's chain as category colours.
    fn mini_chain(&self) -> Vec<Mini> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        app_groups(snapshot)
            .iter()
            .map(|group| Mini::Block {
                colour: theme::category_colour(group_category(group)),
                on: block_enabled(snapshot, group),
            })
            .collect()
    }

    /// The mini deck for a PRO preset.
    pub(crate) fn mini_deck(&mut self, ui: &mut Ui, sidebar_hidden: bool) -> MiniAsked {
        let paused = self.update_mode || (self.firmware.is_some() && !self.online);
        let snapshot = self.snapshot.clone().filter(|_| !paused);
        let Some(snapshot) = snapshot else {
            let words = if paused {
                "Editing is paused while the firmware updates.".to_owned()
            } else {
                self.status.clone()
            };
            let mut show_sidebar = false;
            let full = ui.max_rect();
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(
                        full.with_min_x(full.left() + if sidebar_hidden { 12.0 } else { 20.0 }),
                    )
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    if sidebar_hidden {
                        show_sidebar = shell::sidebar_toggle(ui);
                    }
                    theme::label(ui, &words, theme::regular(theme::BODY), theme::muted());
                },
            );
            return MiniAsked {
                show_sidebar,
                ..MiniAsked::default()
            };
        };
        let slot = active_preset_index(&snapshot).map_or_else(|| "--".to_owned(), slot_label);
        let name = snapshot
            .active_preset
            .clone()
            .unwrap_or_else(|| "StompStation PRO".to_owned());
        let chain = self.mini_chain();
        let (can_save, refused) = self.save_state();
        let asked = shell::mini_deck(
            ui,
            &shell::MiniDeck {
                slot: &slot,
                name: &name,
                dirty: self.dirty,
                chain: &chain,
                snapshot: None,
                can_save,
                save_refused: &refused,
                sidebar_hidden,
            },
        );
        if asked.save {
            let _ = self.tx.send(Cmd::SavePreset(self.save_name.trim().into()));
        }
        asked
    }

    /// The PRO's page: its name and firmware, then backups, the NAM and IR
    /// libraries and its settings, one tab each.
    pub(crate) fn pedal_page(&mut self, root: &mut Ui, tier: Tier) {
        let snapshot = self.snapshot.clone();
        if (self.update_mode || self.firmware.is_some()) && snapshot.is_none() {
            return self.update_mode_page(root);
        }
        egui::Panel::top("pro-pedal-head")
            .exact_size(if snapshot.is_some() {
                shell::PAGE_HEAD_WITH_TABS
            } else {
                shell::PAGE_HEAD
            })
            .resizable(false)
            .show_separator_line(snapshot.is_none())
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let Some(snapshot) = &snapshot else {
                    shell::page_head(
                        ui,
                        Some(Icon::Pedal),
                        "StompStation PRO",
                        &self.status,
                        |_| {},
                    );
                    return;
                };
                let presets = self
                    .presets()
                    .map_or(0, |presets| presets.info.occupied().count());
                let count = self.presets().map_or(0, |presets| presets.info.count);
                let subtitle = format!(
                    "Firmware {} · {presets} of {count} preset slots used · {}",
                    snapshot.identity.version, snapshot.transport
                );
                let mut let_go = false;
                let mut reconnect = false;
                let online = self.online;
                let file = self.firmware.as_ref().and_then(|flow| flow.image.clone());
                shell::page_head(
                    ui,
                    Some(Icon::Pedal),
                    &snapshot.identity.name,
                    &subtitle,
                    |ui| {
                        if let Some(image) = &file {
                            theme::Chip::new(&format!(
                                "{} · firmware {}",
                                image.file, image.version
                            ))
                            .icon(Icon::PackageCheck)
                            .height(24.0)
                            .show(ui);
                        } else if online {
                            let_go = theme::Button::new("Let the pedal go")
                                .ghost()
                                .small()
                                .icon(Icon::Power)
                                .show(ui)
                                .on_hover_text("Disconnect, so another editor can use the pedal")
                                .clicked();
                        } else {
                            reconnect = theme::Button::new("Look for it again")
                                .small()
                                .icon(Icon::Usb)
                                .show(ui)
                                .clicked();
                        }
                    },
                );
                if let_go {
                    let _ = self.tx.send(Cmd::Disconnect);
                }
                if reconnect {
                    let _ = self.tx.send(Cmd::Connect);
                }
                let tabs: Vec<(&str, Option<String>)> = TABS
                    .iter()
                    .map(|tab| match tab {
                        Tab::Backups => ("Backups", None),
                        Tab::Library(library) => (
                            library.title(),
                            snapshot
                                .libraries
                                .iter()
                                .find(|state| state.library == *library)
                                .map(|state| state.info.occupied().count().to_string()),
                        ),
                        Tab::Settings => ("Settings", None),
                        Tab::Firmware => ("Firmware", None),
                    })
                    .collect();
                let selected = TABS
                    .iter()
                    .position(|tab| *tab == self.tab)
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    if let Some(index) = theme::tabs(ui, &tabs, selected) {
                        self.tab = TABS[index];
                    }
                });
            });
        if let (Some(snapshot), Tab::Library(library)) = (&snapshot, self.tab) {
            egui::Panel::right("pro-library-inspector")
                .resizable(false)
                .exact_size(tier.pick(300.0, 340.0, 380.0))
                .frame(egui::Frame::new().fill(theme::bg()))
                .show(root, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("pro-library-inspector", library.path()))
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.library_inspector(ui, snapshot, library));
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::bg())
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 16,
                        top: 16,
                        bottom: 12,
                    }),
            )
            .show(root, |ui| {
                let Some(snapshot) = &snapshot else {
                    ui.set_max_width(640.0);
                    let mut look = false;
                    theme::banner(
                        ui,
                        Mood::Info,
                        Icon::Usb,
                        "No StompStation PRO connected",
                        "Its backups, NAM and IR libraries and settings appear here once it \
                         connects. Quit VoidX Control first: only one editor can use the pedal \
                         at a time.",
                        |ui| {
                            look = theme::Button::new("Look for a pedal")
                                .small()
                                .icon(Icon::Usb)
                                .show(ui)
                                .clicked();
                        },
                    );
                    if look {
                        let _ = self.tx.send(Cmd::Connect);
                    }
                    return;
                };
                match self.tab {
                    Tab::Backups => {
                        egui::ScrollArea::vertical()
                            .id_salt("pro-backups")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width().min(980.0));
                                self.backups(ui, snapshot);
                            });
                    }
                    Tab::Library(library) => {
                        egui::ScrollArea::vertical()
                            .id_salt(("pro-library", library.path()))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add_enabled_ui(self.online && !self.busy, |ui| {
                                    self.library_page(ui, snapshot, library);
                                });
                            });
                    }
                    Tab::Settings => {
                        egui::ScrollArea::vertical()
                            .id_salt("pro-settings")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width().min(980.0));
                                self.settings_page(ui, snapshot);
                            });
                    }
                    Tab::Firmware => {
                        egui::ScrollArea::vertical()
                            .id_salt("pro-firmware")
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.firmware_page(ui));
                    }
                }
            });
    }

    /// The Pedal page while the pedal is in Update Mode and TonePush has not
    /// read anything else of it: its name, and the update.
    fn update_mode_page(&mut self, root: &mut Ui) {
        egui::Panel::top("pro-pedal-head")
            .exact_size(shell::PAGE_HEAD)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let name = match self.device_name() {
                    "" => "StompStation PRO".to_owned(),
                    name => name.to_owned(),
                };
                let file = self.firmware.as_ref().and_then(|flow| flow.image.clone());
                shell::page_head(ui, Some(Icon::Pedal), &name, "In Update Mode", |ui| {
                    if let Some(image) = &file {
                        theme::Chip::new(&format!("{} · firmware {}", image.file, image.version))
                            .icon(Icon::PackageCheck)
                            .height(24.0)
                            .show(ui);
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::bg())
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 16,
                        top: 16,
                        bottom: 12,
                    }),
            )
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("pro-firmware")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.firmware_page(ui));
            });
    }

    /// The strip over the Edit page while an update has the pedal.
    pub(crate) fn paused_strip(&mut self, root: &mut Ui) {
        let mut show = false;
        egui::Panel::top("pro-paused")
            .exact_size(48.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::mix(theme::bg(), theme::info(), 0.08)))
            .show(root, |ui| {
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    egui::Stroke::new(1.0, theme::alpha(theme::info(), 0.3)),
                );
                let inner = Rect::from_min_max(
                    Pos2::new(rect.left() + 20.0, rect.top()),
                    Pos2::new(rect.right() - 16.0, rect.bottom()),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(inner)
                        .id_salt("pro-paused-actions")
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                show = theme::Button::new("Show the update")
                    .ghost()
                    .small()
                    .show(&mut child)
                    .clicked();
                theme::paint_icon(
                    ui,
                    Icon::PackageCheck,
                    Pos2::new(inner.left() + 8.0, inner.center().y),
                    16.0,
                    theme::info(),
                );
                let words = if self.update_mode {
                    "The pedal is in Update Mode: editing waits until it starts normally."
                } else {
                    "Editing is paused while the firmware updates."
                };
                let galley = shell::galley(ui, words, theme::regular(12.5), theme::text());
                shell::paint_line(ui, galley, inner.left() + 28.0, inner.center().y);
            });
        if show {
            self.tab = Tab::Firmware;
            self.page_request = true;
        }
    }

    /// Backups: whether the pedal is protected, and a whole-pedal backup or
    /// restore through a file.
    fn backups(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let available = self.online && !self.busy;
        let (mood, icon, title, body) = if let Some(reason) = &self.read_only {
            (
                Mood::Info,
                Icon::Lock,
                format!("Read only on firmware {}", snapshot.identity.version),
                format!(
                    "TonePush browses, exports and backs up this pedal, and changes nothing: \
                     {reason}."
                ),
            )
        } else if self.rollback.is_some() {
            (
                Mood::Ok,
                Icon::ShieldCheck,
                "This StompStation PRO is protected".to_owned(),
                format!(
                    "TonePush checked {} against the pedal: every preset, NAM model, impulse \
                     response and setting. Saving, renaming and importing are on, and the \
                     backup can put back anything they write.",
                    match self.rollback_time.and_then(shell::clock) {
                        Some((when, true)) => format!("the backup taken at {when}"),
                        Some((when, false)) => format!("the backup taken on {when}"),
                        None => "a backup".to_owned(),
                    }
                ),
            )
        } else {
            (
                Mood::Hot,
                Icon::ShieldAlert,
                "Not protected yet".to_owned(),
                "Edits play on the pedal now. Saving, renaming and importing write its memory, \
                 so they wait until TonePush holds a checked backup of this pedal."
                    .to_owned(),
            )
        };
        let guarded = self.rollback.is_some() && self.read_only.is_none();
        let mut backup = false;
        let mut restore = false;
        let mut existing = false;
        let mut here = false;
        let unlock = self.unlock_offer();
        theme::banner(ui, mood, icon, &title, &body, |ui| {
            if let Some(ready) = unlock {
                here = theme::Button::new("Back up to unlock saving")
                    .small()
                    .primary()
                    .icon(Icon::Download)
                    .enabled(ready)
                    .show(ui)
                    .on_hover_text("Read the whole pedal into a checked backup on this computer")
                    .clicked();
            }
            backup = theme::Button::new("Back up to a file…")
                .small()
                .icon(Icon::Download)
                .enabled(available)
                .show(ui)
                .on_hover_text("Save every preset, model, impulse response and setting")
                .clicked();
            restore = theme::Button::new("Restore from a file…")
                .small()
                .icon(Icon::History)
                .enabled(available && guarded)
                .show(ui)
                .on_hover_text("Replace the pedal with a complete backup")
                .on_disabled_hover_text("Restoring waits for a checked backup of this pedal")
                .clicked();
            if self.rollback.is_none() && self.read_only.is_none() {
                existing = theme::Button::new("Use an existing backup…")
                    .ghost()
                    .small()
                    .enabled(available)
                    .show(ui)
                    .on_hover_text("Check a backup taken earlier against the pedal")
                    .clicked();
            }
        });
        if here {
            self.back_up_here();
        }
        if backup {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Where to put the backup")
                .set_file_name(format!("{}.vxbundle", sanitise(&snapshot.identity.name)))
                .save_file()
            {
                let _ = self.tx.send(Cmd::Backup(path));
            }
        }
        if restore {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.confirmation = Some(Confirmation {
                    action: "Restore the pedal",
                    question: format!(
                        "Restore every library and safe setting from {}?",
                        path.display()
                    ),
                    command: Cmd::Restore(path),
                });
            }
        }
        if existing {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                let _ = self.tx.send(Cmd::UseRollback(path));
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let (spot, _) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), Sense::hover());
            theme::paint_icon(ui, Icon::Info, spot.center(), 14.0, theme::muted());
            theme::label(
                ui,
                "TonePush takes a backup on its own when a pedal it has backed up before \
                 connects and no backup matches it any more.",
                theme::regular(12.0),
                theme::muted(),
            );
        });
    }
}

/// What to say about a tempo the pedal's tempo node does not take, in the
/// words the HX's deck uses, with the range the pedal itself advertised.
fn tempo_refusal(description: &NodeDescription) -> String {
    match (description.min, description.max) {
        (Some(min), Some(max)) => format!("tempo must be between {min} and {max} BPM"),
        (Some(min), None) => format!("tempo must be at least {min} BPM"),
        (None, Some(max)) => format!("tempo must be at most {max} BPM"),
        (None, None) => "tempo must be a number of BPM".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_read_like_the_pedals_home_screen() {
        assert_eq!(slot_label(0), "01A");
        assert_eq!(slot_label(1), "01B");
        assert_eq!(slot_label(2), "01C");
        assert_eq!(slot_label(3), "02A");
        assert_eq!(slot_label(59), "20C");
    }

    /// The rollback guard, said where the question is asked: Save's reason,
    /// the deck's state line and the sidebar's foot all agree.
    #[test]
    fn writes_wait_for_a_checked_backup_and_say_so() {
        let mut panel = Panel::new(egui::Context::default());
        panel.online = true;
        assert_eq!(
            panel.write_refusal().as_deref(),
            Some("Waits for a checked backup of this pedal")
        );
        assert_eq!(panel.foot().text, "Not protected yet");
        assert!(panel
            .state(Tier::M)
            .iter()
            .any(|part| matches!(part, State::Warning(Icon::ShieldAlert, _))));

        panel.rollback = Some(PathBuf::from("stompstation-pro-1.5.12.vxbundle"));
        assert_eq!(panel.write_refusal(), None);
        assert!(panel.foot().text.starts_with("Protected"));
        assert!(panel
            .state(Tier::M)
            .iter()
            .any(|part| matches!(part, State::Good(Icon::ShieldCheck, _))));

        panel.read_only = Some("firmware 2.0.10 is not verified".to_owned());
        assert!(panel
            .write_refusal()
            .is_some_and(|why| why.starts_with("Read only")));
        assert_eq!(panel.foot().text, "Read only");

        panel.online = false;
        assert_eq!(
            panel.write_refusal().as_deref(),
            Some("Connect the pedal first")
        );
    }

    /// Before a backup matches, the deck offers the one that unlocks
    /// saving, ready only while nothing else is talking to the pedal, and
    /// never for a pedal that is read only.
    #[test]
    fn the_backup_that_unlocks_saving_takes_saves_place() {
        let mut panel = Panel::new(egui::Context::default());
        assert_eq!(panel.unlock_offer(), None, "no pedal, nothing to back up");
        panel.online = true;
        panel.snapshot = Some(demo_snapshot());
        assert_eq!(panel.unlock_offer(), Some(true));
        panel.busy = true;
        assert_eq!(panel.unlock_offer(), Some(false));
        panel.busy = false;
        panel.working = Some(("Reading presets".to_owned(), 0.4));
        assert_eq!(panel.unlock_offer(), Some(false));
        panel.working = None;
        panel.rollback = Some(PathBuf::from("stompstation-pro-1.5.12.vxbundle"));
        assert_eq!(panel.unlock_offer(), None, "protected: Save is back");
        panel.rollback = None;
        panel.read_only = Some("firmware 2.0.10 is not verified".to_owned());
        assert_eq!(panel.unlock_offer(), None, "read only: nothing to unlock");
    }

    #[test]
    fn a_read_only_connection_is_remembered_until_the_pedal_goes() {
        let mut panel = Panel::new(egui::Context::default());
        let (events, rx) = mpsc::channel();
        panel.rx = rx;
        events
            .send(Evt::Connected(demo_snapshot()))
            .expect("the panel listens");
        events
            .send(Evt::ReadOnly("not verified".to_owned()))
            .expect("the panel listens");
        panel.drain();
        assert_eq!(panel.read_only.as_deref(), Some("not verified"));
        events.send(Evt::Disconnected).expect("the panel listens");
        panel.drain();
        assert_eq!(panel.read_only, None);
        assert!(!panel.online);
    }

    /// The PRO's tempo node says what it holds. Anything outside that, or no
    /// number at all, is answered in the deck instead of being sent; a tempo
    /// the node takes is sent, and the reading waits for the pedal to say it
    /// has it.
    #[test]
    fn only_a_tempo_the_pro_takes_is_sent() {
        let mut panel = Panel::new(egui::Context::default());
        panel.show_demo();
        let (tx, commands) = mpsc::channel();
        panel.tx = tx;
        let (path, description, bpm) = panel.tempo_node().expect("the demo has a tempo");
        assert_eq!(bpm, 96.0);

        for bpm in [20.0, 999.0, -1.0, f32::NAN, f32::INFINITY] {
            panel.set_tempo(path.clone(), description.clone(), bpm);
        }
        assert!(commands.try_recv().is_err(), "nothing out of range is sent");
        assert_eq!(panel.status, "tempo must be between 40 and 240 BPM");
        assert!(panel.failed);

        panel.set_tempo(path.clone(), description.clone(), 120.0);
        assert!(matches!(
            commands.try_recv(),
            Ok(Cmd::SetNode { value, persistent: false, .. }) if value == 120.0
        ));
        assert_eq!(
            panel.tempo_node().map(|(_, _, bpm)| bpm),
            Some(96.0),
            "the reading waits for the pedal"
        );
        assert!(!panel.failed, "a tempo that is sent clears the refusal");

        let mut open = description.clone();
        open.min = None;
        open.max = None;
        panel.set_tempo(path.clone(), open.clone(), f32::NAN);
        assert!(commands.try_recv().is_err(), "no number is never sent");
        assert_eq!(panel.status, "tempo must be a number of BPM");
        panel.set_tempo(path, open, 300.0);
        assert!(
            commands.try_recv().is_ok(),
            "no range advertised, no range checked"
        );
    }

    fn demo_snapshot() -> Snapshot {
        let mut panel = Panel::new(egui::Context::default());
        panel.show_demo();
        panel.snapshot.expect("the demo has a snapshot")
    }
}
