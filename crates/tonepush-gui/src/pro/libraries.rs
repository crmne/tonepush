//! The pedal's NAM and IR libraries as pages (docs/design/redesign-2026-10-01,
//! screens 10 and 11): one table of a library's slots, and an inspector for
//! the one chosen with what its file says about itself and the presets that
//! play it.
//!
//! What a file says about itself, and which presets play it, come from the
//! checked backup that guards the pedal: it holds every file and preset
//! byte for byte, so nothing is read off the pedal to show them. Presets
//! find a model by its name, so a model a preset plays is not renamed or
//! removed (the worker asks the pedal itself again before either).

use std::collections::BTreeMap;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::*;
use crate::shell;
use crate::theme::Icon;

/// What a checked backup says about one library file.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SlotFacts {
    /// A NAM capture's gear, as its maker wrote it.
    pub(crate) gear: Option<String>,
    pub(crate) modeled_by: Option<String>,
    /// A capture's network: WaveNet's Standard, Lite, Feather or Nano, or
    /// the architecture's own name.
    pub(crate) size: Option<String>,
    pub(crate) input_dbu: Option<f64>,
    /// An impulse response's length, in milliseconds.
    pub(crate) length_ms: Option<f64>,
    /// An impulse response's shape, a few hundred points of it.
    pub(crate) wave: Vec<f32>,
    /// The file's size, as it would be exported.
    pub(crate) bytes: usize,
}

/// A preset that plays a library file: its slot, its name, and the block
/// that names the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct User {
    pub(crate) preset: usize,
    pub(crate) name: String,
    pub(crate) block: String,
}

/// Everything the checked backup says about the pedal's libraries, by file
/// name: presets find a file by its name, so the name is what follows it
/// when slots move.
#[derive(Clone, Debug, Default)]
pub(crate) struct LibraryFacts {
    pub(crate) files: BTreeMap<(Library, String), SlotFacts>,
    pub(crate) users: BTreeMap<(Library, String), Vec<User>>,
}

impl LibraryFacts {
    pub(crate) fn file(&self, library: Library, name: &str) -> Option<&SlotFacts> {
        self.files.get(&(library, name.to_owned()))
    }

    pub(crate) fn users(&self, library: Library, name: &str) -> &[User] {
        self.users
            .get(&(library, name.to_owned()))
            .map_or(&[], Vec::as_slice)
    }
}

/// What a `.nam` document says about itself.
pub(crate) fn nam_facts(document: &[u8]) -> SlotFacts {
    let mut facts = SlotFacts {
        bytes: document.len(),
        ..SlotFacts::default()
    };
    let Ok(json) = serde_json::from_slice::<Value>(document) else {
        return facts;
    };
    let metadata = json.get("metadata");
    let text = |key: &str| {
        metadata
            .and_then(|metadata| metadata.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    facts.gear = match (text("gear_make"), text("gear_model")) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    };
    facts.modeled_by = text("modeled_by");
    facts.input_dbu = metadata
        .and_then(|metadata| metadata.get("input_level_dbu"))
        .and_then(Value::as_f64);
    let architecture = json.get("architecture").and_then(Value::as_str);
    let channels = json
        .get("config")
        .and_then(|config| config.get("layers"))
        .and_then(|layers| layers.get(0))
        .and_then(|layer| layer.get("channels"))
        .and_then(Value::as_u64);
    facts.size = match (architecture, channels) {
        (Some("WaveNet"), Some(16)) => Some("Standard".to_owned()),
        (Some("WaveNet"), Some(12)) => Some("Lite".to_owned()),
        (Some("WaveNet"), Some(8)) => Some("Feather".to_owned()),
        (Some("WaveNet"), Some(4)) => Some("Nano".to_owned()),
        (Some(architecture), _) => Some(architecture.to_owned()),
        (None, _) => None,
    };
    facts
}

/// What an impulse response's slot holds: its length without the padding
/// the slot adds, and its shape.
pub(crate) fn ir_facts(blob: &[u8]) -> SlotFacts {
    let samples: Vec<f32> = blob
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    let frames = samples
        .iter()
        .rposition(|sample| *sample != 0.0)
        .map_or(0, |last| last + 1);
    let rate = voidx_client::ir::SAMPLE_RATE as f64;
    SlotFacts {
        length_ms: (frames > 0).then(|| frames as f64 * 1000.0 / rate),
        wave: thinned(&samples[..frames], 240),
        bytes: frames * 4,
        ..SlotFacts::default()
    }
}

/// `samples` in `points` buckets, each the sample of greatest magnitude in
/// it, so the shape keeps its peaks.
fn thinned(samples: &[f32], points: usize) -> Vec<f32> {
    if samples.is_empty() || points == 0 {
        return Vec::new();
    }
    let size = samples.len().div_ceil(points).max(1);
    samples
        .chunks(size)
        .map(|bucket| {
            bucket.iter().copied().fold(0.0_f32, |kept, sample| {
                if sample.abs() > kept.abs() {
                    sample
                } else {
                    kept
                }
            })
        })
        .collect()
}

/// The stereo pairs among a library's names: a slot named "<name> L" with
/// the next named "<name> R", as TonePush imports a stereo WAV. Each pair is
/// its left slot and its name.
pub(crate) fn stereo_pairs(names: &[Option<String>]) -> Vec<(usize, String)> {
    names
        .windows(2)
        .enumerate()
        .filter_map(|(slot, pair)| {
            let left = pair[0].as_deref()?.strip_suffix(" L")?;
            let right = pair[1].as_deref()?.strip_suffix(" R")?;
            (left == right).then(|| (slot, left.to_owned()))
        })
        .collect()
}

/// One row of a library's table.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Slot(usize),
    /// A stereo pair: its left slot, the right after it.
    Pair(usize, String),
    /// The first free slot, where an import goes.
    Free(usize),
}

/// The table's rows: every occupied slot, pairs as one, and the first free
/// slot, narrowed to what matches the search.
fn rows(state: &LibraryState, needle: &str) -> Vec<Row> {
    let names = &state.info.names;
    let pairs: BTreeMap<usize, String> = if state.library == Library::Irs {
        stereo_pairs(names).into_iter().collect()
    } else {
        BTreeMap::new()
    };
    let mut rows = Vec::new();
    let mut slot = 0;
    while slot < state.info.count {
        if let Some(name) = pairs.get(&slot) {
            if needle.is_empty() || name.to_ascii_lowercase().contains(needle) {
                rows.push(Row::Pair(slot, name.clone()));
            }
            slot += 2;
            continue;
        }
        if let Some(Some(name)) = names.get(slot) {
            if slot_matches_search(slot, Some(name), needle) {
                rows.push(Row::Slot(slot));
            }
        }
        slot += 1;
    }
    if needle.is_empty() {
        if let Some(free) =
            (0..state.info.count).find(|slot| names.get(*slot).is_none_or(Option::is_none))
        {
            rows.push(Row::Free(free));
        }
    }
    rows
}

/// "1.9 MB", "380 kB".
fn size_words(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} kB", (bytes as f64 / 1000.0).round().max(1.0))
    }
}

/// "6 presets", "1 preset", "none".
fn users_words(count: usize) -> String {
    match count {
        0 => "none".to_owned(),
        1 => "1 preset".to_owned(),
        n => format!("{n} presets"),
    }
}

impl Panel {
    /// The slot chosen in a library's table.
    fn chosen_slot(&self, library: Library) -> usize {
        self.target_slots
            .get(&library)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1)
    }

    /// A library's page: its count and search, then its table.
    pub(super) fn library_page(&mut self, ui: &mut Ui, snapshot: &Snapshot, library: Library) {
        let Some(state) = snapshot
            .libraries
            .iter()
            .find(|state| state.library == library)
            .cloned()
        else {
            return;
        };
        let used = state.info.occupied().count();
        let unused = state
            .info
            .names
            .iter()
            .flatten()
            .filter(|name| self.facts.users(library, name).is_empty())
            .count();
        let pairs = if library == Library::Irs {
            stereo_pairs(&state.info.names).len()
        } else {
            0
        };
        let writable = self.rollback.is_some() && self.read_only.is_none() && self.online;
        let mut import = false;
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            let mut words = egui::text::LayoutJob::default();
            words.append(
                &format!("{used} of {} slots", state.info.count),
                0.0,
                egui::TextFormat::simple(theme::semibold(13.0), theme::text()),
            );
            let aside = if pairs > 0 {
                format!(
                    "{} stereo {}",
                    pairs,
                    if pairs == 1 { "pair" } else { "pairs" }
                )
            } else if self.rollback.is_some() {
                format!("{unused} not used by any preset")
            } else {
                String::new()
            };
            if !aside.is_empty() {
                words.append(
                    &format!("· {aside}"),
                    6.0,
                    egui::TextFormat::simple(theme::regular(13.0), theme::muted()),
                );
            }
            ui.label(words);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                import = theme::Button::new(if library == Library::Irs {
                    "Import WAV files…"
                } else {
                    "Import .nam files…"
                })
                .small()
                .icon(Icon::Upload)
                .enabled(writable)
                .show(ui)
                .on_hover_text("Into the first free slot")
                .on_disabled_hover_text(
                    self.write_refusal()
                        .unwrap_or_else(|| "The pedal is busy".to_owned()),
                )
                .clicked();
                let search = self.library_searches.entry(library).or_default();
                theme::search_field(
                    ui,
                    &format!("library-search-{}", library.path()),
                    search,
                    match library {
                        Library::Irs => "Search IRs",
                        Library::Amps => "Search NAM amps",
                        Library::Drives => "Search NAM drives",
                        Library::Presets => "Search presets",
                    },
                    200.0,
                );
            });
        });
        ui.add_space(8.0);
        if import {
            let free = (0..state.info.count)
                .find(|slot| state.info.names.get(*slot).is_none_or(Option::is_none));
            match free {
                Some(slot) => self.import_into_slot(&state, slot),
                None => self.status = format!("Every {} slot is in use", library.title()),
            }
        }
        let needle = self
            .library_searches
            .get(&library)
            .map_or("", String::as_str)
            .trim()
            .to_ascii_lowercase();
        self.library_table(ui, &state, &needle, writable);
        if library == Library::Irs {
            ui.add_space(14.0);
            drop_note(ui);
        }
    }

    /// A library's slots as a table.
    fn library_table(&mut self, ui: &mut Ui, state: &LibraryState, needle: &str, writable: bool) {
        let library = state.library;
        let rows = rows(state, needle);
        let chosen = self.chosen_slot(library);
        let mut picked = None;
        let mut import = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let width = ui.available_width();
            let middle = if library == Library::Irs {
                ("Channels", "Length", 150.0, 90.0)
            } else {
                ("Captured gear", "WaveNet", 190.0, 96.0)
            };
            let used_width = 104.0;
            let slot_width = 52.0;
            let narrow = width < 560.0;
            let (third, fourth) = if narrow {
                (0.0, 0.0)
            } else {
                (middle.2, middle.3)
            };
            let name_width = (width - 24.0 - slot_width - third - fourth - used_width).max(120.0);
            let columns = [slot_width, name_width, third, fourth, used_width];
            let (head, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
            let mut x = head.left() + 20.0;
            for (title, column) in ["Slot", "Name", middle.0, middle.1, "Used by"]
                .into_iter()
                .zip(columns)
            {
                if column > 0.0 {
                    let galley =
                        shell::galley(ui, title, theme::semibold(theme::LABEL), theme::muted());
                    shell::paint_line(ui, galley, x, head.center().y);
                }
                x += column;
            }
            ui.painter().hline(
                head.x_range(),
                head.bottom() - 0.5,
                Stroke::new(1.0, theme::line()),
            );
            if rows.is_empty() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 44.0), Sense::hover());
                let galley = shell::galley(
                    ui,
                    "No slots match this search.",
                    theme::regular(theme::BODY),
                    theme::muted(),
                );
                shell::paint_line(ui, galley, rect.left() + 20.0, rect.center().y);
                return;
            }
            let last = rows.len() - 1;
            for (index, row) in rows.iter().enumerate() {
                let (height, first) = match row {
                    Row::Pair(left, _) => (68.0, *left),
                    Row::Slot(slot) | Row::Free(slot) => (34.0, *slot),
                };
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
                let selected = match row {
                    Row::Pair(left, _) => chosen == *left || chosen == left + 1,
                    Row::Slot(slot) => chosen == *slot,
                    Row::Free(_) => false,
                };
                let painter = ui.painter();
                if selected {
                    painter.rect_filled(rect, CornerRadius::ZERO, theme::hover());
                } else if response.hovered() {
                    painter.rect_filled(
                        rect,
                        CornerRadius::ZERO,
                        theme::alpha(theme::hover(), 0.5),
                    );
                }
                if index < last {
                    painter.hline(
                        rect.x_range(),
                        rect.bottom() - 0.5,
                        Stroke::new(1.0, theme::line_soft()),
                    );
                }
                self.library_row(ui, state, row, rect, columns);
                match row {
                    Row::Free(slot) => {
                        let response = response.on_hover_text(if writable {
                            "Import a file into this slot"
                        } else {
                            "Importing waits for a checked backup of this pedal"
                        });
                        if response.clicked() && writable {
                            import = Some(*slot);
                        }
                    }
                    _ => {
                        if response.clicked() {
                            picked = Some(first);
                        }
                    }
                }
            }
        });
        if let Some(slot) = picked {
            if self
                .slot_renaming
                .is_some_and(|(_, renaming)| renaming != slot)
            {
                self.slot_renaming = None;
            }
            self.target_slots.insert(library, slot + 1);
        }
        if let Some(slot) = import {
            self.import_into_slot(state, slot);
        }
    }

    /// One row's cells.
    fn library_row(&self, ui: &Ui, state: &LibraryState, row: &Row, rect: Rect, columns: [f32; 5]) {
        let library = state.library;
        let ink = theme::category_colour(match library {
            Library::Amps => "Amp",
            Library::Drives => "Distortion",
            _ => "IR",
        });
        let icon = theme::category_icon(match library {
            Library::Amps => "Amp",
            Library::Drives => "Distortion",
            _ => "IR",
        });
        let line = |ui: &Ui, slot: usize, y: f32, name: &str, quiet: bool, side: Option<&str>| {
            let mut x = rect.left() + 20.0;
            let number = shell::galley(
                ui,
                format!("{:02}", slot + 1),
                theme::regular(theme::BODY),
                theme::muted(),
            );
            shell::paint_line(ui, number, x, y);
            x += columns[0];
            if let Some(icon) = &icon {
                icon.paint(
                    ui,
                    Rect::from_center_size(Pos2::new(x + 7.0, y), Vec2::splat(14.0)),
                    if quiet { theme::alpha(ink, 0.6) } else { ink },
                );
            }
            let galley = shell::elided(
                ui,
                name,
                if quiet {
                    theme::regular(theme::BODY)
                } else {
                    theme::semibold(theme::BODY)
                },
                if quiet { theme::muted() } else { theme::text() },
                columns[1] - 32.0,
            );
            shell::paint_line(ui, galley, x + 22.0, y);
            x += columns[1];
            if columns[2] > 0.0 {
                if let Some(side) = side {
                    let galley = shell::galley(ui, side, theme::semibold(11.0), ink);
                    let pill = Rect::from_min_size(
                        Pos2::new(x, y - 10.0),
                        Vec2::new(galley.size().x + 14.0, 20.0),
                    );
                    ui.painter().rect_filled(
                        pill,
                        CornerRadius::same(theme::RADIUS_CHIP),
                        theme::alpha(ink, 0.16),
                    );
                    shell::paint_line(ui, galley, pill.left() + 7.0, y);
                }
            }
            x
        };
        match row {
            Row::Free(slot) => {
                let y = rect.center().y;
                let number = shell::galley(
                    ui,
                    format!("{:02}", slot + 1),
                    theme::regular(theme::BODY),
                    theme::faint(),
                );
                shell::paint_line(ui, number, rect.left() + 20.0, y);
                let x = rect.left() + 20.0 + columns[0];
                theme::paint_icon(ui, Icon::Plus, Pos2::new(x + 7.0, y), 13.0, theme::faint());
                let galley = shell::galley(
                    ui,
                    if library == Library::Irs {
                        "Empty · drop a WAV file here"
                    } else {
                        "Empty · import a .nam file here"
                    },
                    theme::regular(theme::BODY),
                    theme::faint(),
                );
                shell::paint_line(ui, galley, x + 22.0, y);
            }
            Row::Slot(slot) => {
                let y = rect.center().y;
                let name = state
                    .info
                    .names
                    .get(*slot)
                    .cloned()
                    .flatten()
                    .unwrap_or_default();
                let x = line(ui, *slot, y, &name, false, None);
                self.facts_cells(ui, library, &name, x, y, columns, false);
            }
            Row::Pair(left, name) => {
                let top = rect.top() + 17.0;
                let bottom = rect.top() + 51.0;
                let left_name = state
                    .info
                    .names
                    .get(*left)
                    .cloned()
                    .flatten()
                    .unwrap_or_default();
                // The bracket that joins the two slots.
                let bracket_x = rect.left() + 20.0 + 26.0;
                let stroke = Stroke::new(1.5, theme::alpha(ink, 0.7));
                ui.painter().line_segment(
                    [Pos2::new(bracket_x, top), Pos2::new(bracket_x + 5.0, top)],
                    stroke,
                );
                ui.painter().line_segment(
                    [Pos2::new(bracket_x, top), Pos2::new(bracket_x, bottom)],
                    stroke,
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(bracket_x, bottom),
                        Pos2::new(bracket_x + 5.0, bottom),
                    ],
                    stroke,
                );
                let x = line(ui, *left, top, name, false, Some("Left"));
                if columns[2] > 0.0 {
                    let aside =
                        shell::galley(ui, "stereo pair", theme::regular(12.0), theme::muted());
                    shell::paint_line(ui, aside, x - columns[2] + 52.0, top);
                }
                self.facts_cells(ui, library, &left_name, x, top, columns, true);
                let _ = line(ui, left + 1, bottom, name, true, Some("Right"));
            }
        }
    }

    /// The facts columns of a row, from `x`.
    #[allow(clippy::too_many_arguments)]
    fn facts_cells(
        &self,
        ui: &Ui,
        library: Library,
        name: &str,
        x: f32,
        y: f32,
        columns: [f32; 5],
        paired: bool,
    ) {
        let facts = self.facts.file(library, name);
        let mut x = x;
        if columns[2] > 0.0 {
            if library == Library::Irs {
                if !paired {
                    let galley =
                        shell::galley(ui, "Mono", theme::regular(theme::BODY), theme::text_soft());
                    shell::paint_line(ui, galley, x, y);
                }
            } else if let Some(gear) = facts.and_then(|facts| facts.gear.clone()) {
                let galley = shell::elided(
                    ui,
                    gear,
                    theme::regular(theme::BODY),
                    theme::text_soft(),
                    columns[2] - 12.0,
                );
                shell::paint_line(ui, galley, x, y);
            }
            x += columns[2];
            let fourth = if library == Library::Irs {
                facts
                    .and_then(|facts| facts.length_ms)
                    .map(|ms| format!("{ms:.0} ms"))
            } else {
                facts.and_then(|facts| facts.size.clone())
            };
            if let Some(text) = fourth {
                let galley = shell::galley(ui, text, theme::regular(theme::BODY), theme::muted());
                shell::paint_line(ui, galley, x, y);
            }
            x += columns[3];
        }
        if self.rollback.is_some() {
            let count = self.facts.users(library, name).len();
            let galley = shell::galley(
                ui,
                users_words(count),
                theme::regular(theme::BODY),
                if count == 0 {
                    theme::faint()
                } else {
                    theme::text_soft()
                },
            );
            shell::paint_line(ui, galley, x, y);
        }
    }

    /// The inspector beside a library's table, for the slot chosen.
    pub(super) fn library_inspector(&mut self, ui: &mut Ui, snapshot: &Snapshot, library: Library) {
        let Some(state) = snapshot
            .libraries
            .iter()
            .find(|state| state.library == library)
            .cloned()
        else {
            return;
        };
        let slot = self.chosen_slot(library);
        let Some(name) = state.info.names.get(slot).cloned().flatten() else {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                theme::label(
                    ui,
                    "Choose a slot to see what it holds.",
                    theme::regular(theme::SECONDARY),
                    theme::muted(),
                );
            });
            return;
        };
        let pair = (library == Library::Irs)
            .then(|| {
                stereo_pairs(&state.info.names)
                    .into_iter()
                    .find(|(left, _)| *left == slot || left + 1 == slot)
            })
            .flatten();
        let colour = theme::category_colour(match library {
            Library::Amps => "Amp",
            Library::Drives => "Distortion",
            _ => "IR",
        });
        let title = pair.as_ref().map_or(name.clone(), |(_, base)| base.clone());
        let facts = self.facts.file(library, &name).cloned();
        let users: Vec<User> = match &pair {
            Some((left, _)) => {
                let mut users: Vec<User> = state.info.names[*left..=left + 1]
                    .iter()
                    .flatten()
                    .flat_map(|name| self.facts.users(library, name).to_vec())
                    .collect();
                users.sort_by_key(|user| user.preset);
                users.dedup_by(|a, b| a.preset == b.preset);
                users
            }
            None => self.facts.users(library, &name).to_vec(),
        };
        let width = ui.available_width();
        ui.spacing_mut().item_spacing = Vec2::ZERO;

        // The head, with the slot's menu at its right.
        let writable =
            self.rollback.is_some() && self.read_only.is_none() && self.online && !self.busy;
        let mut replace = false;
        let mut up = false;
        let mut down = false;
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 16,
                bottom: 12,
            })
            .show(ui, |ui| {
                ui.set_width(width - 32.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (well, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                    theme::paint::gradient_rect(
                        ui.painter(),
                        well,
                        10.0,
                        &[
                            (0.0, theme::alpha(colour, 0.24)),
                            (1.0, theme::alpha(colour, 0.08)),
                        ],
                    );
                    ui.painter().rect_stroke(
                        well,
                        CornerRadius::same(10),
                        Stroke::new(1.0, theme::alpha(colour, 0.4)),
                        egui::StrokeKind::Inside,
                    );
                    if let Some(art) = theme::category_icon(match library {
                        Library::Amps => "Amp",
                        Library::Drives => "Distortion",
                        _ => "IR",
                    }) {
                        art.paint(
                            ui,
                            Rect::from_center_size(well.center(), Vec2::splat(20.0)),
                            colour,
                        );
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        theme::label_truncated(
                            ui,
                            &title,
                            theme::semibold(theme::TITLE),
                            theme::text(),
                        );
                        let mut parts = vec![match &pair {
                            Some((left, _)) => {
                                format!("Stereo pair · slots {} and {}", left + 1, left + 2)
                            }
                            None => format!("{} · slot {}", library_noun_of(library), slot + 1),
                        }];
                        if let Some(facts) = &facts {
                            if library == Library::Irs {
                                if let Some(ms) = facts.length_ms {
                                    parts.push(format!("{ms:.0} ms"));
                                }
                            } else if facts.bytes > 0 {
                                parts.push(size_words(facts.bytes));
                            }
                        }
                        theme::label_truncated(
                            ui,
                            &parts.join(" · "),
                            theme::regular(12.0),
                            theme::muted(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let more = theme::IconButton::new(Icon::Ellipsis)
                            .small()
                            .show(ui)
                            .on_hover_text("More");
                        egui::Popup::menu(&more)
                            .align(egui::RectAlign::BOTTOM_END)
                            .gap(4.0)
                            .show(|ui| {
                                theme::menu_width(ui, 220.0);
                                let item = |ui: &mut Ui, icon: Icon, text: &str, enabled: bool| {
                                    if enabled {
                                        theme::menu_item(ui, Some(icon), text, None).clicked()
                                    } else {
                                        theme::menu_disabled(ui, Some(icon), text, None);
                                        false
                                    }
                                };
                                replace = item(ui, Icon::Upload, "Replace with a file…", writable);
                                if state.info.movable {
                                    up = item(
                                        ui,
                                        Icon::ChevronUp,
                                        "Move up a slot",
                                        writable && slot > 0,
                                    );
                                    down = item(
                                        ui,
                                        Icon::ChevronDown,
                                        "Move down a slot",
                                        writable && slot + 1 < state.info.count,
                                    );
                                }
                            });
                    });
                });
            });
        divider(ui, theme::line());

        // What the file says.
        let section = |ui: &mut Ui, contents: &mut dyn FnMut(&mut Ui)| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(16, 14))
                .show(ui, |ui| {
                    ui.set_width(width - 32.0);
                    contents(ui);
                });
            divider(ui, theme::line_soft());
        };
        section(ui, &mut |ui| {
            if library == Library::Irs {
                let channels: Vec<(String, usize)> = match &pair {
                    Some((left, _)) => vec![
                        (format!("Left · slot {}", left + 1), *left),
                        (format!("Right · slot {}", left + 2), left + 1),
                    ],
                    None => vec![(format!("Slot {}", slot + 1), slot)],
                };
                for (index, (caption, channel)) in channels.into_iter().enumerate() {
                    if index > 0 {
                        ui.add_space(14.0);
                    }
                    caption_line(ui, &caption);
                    ui.add_space(8.0);
                    let wave = state
                        .info
                        .names
                        .get(channel)
                        .cloned()
                        .flatten()
                        .and_then(|name| {
                            self.facts
                                .file(library, &name)
                                .map(|facts| facts.wave.clone())
                        })
                        .unwrap_or_default();
                    waveform(ui, &wave, colour);
                }
                if facts.is_none() {
                    ui.add_space(8.0);
                    quiet(ui, "The shape shows once a checked backup holds this file.");
                }
                return;
            }
            caption_line(ui, "From the model file");
            ui.add_space(9.0);
            match &facts {
                Some(facts) => {
                    let rows: Vec<(&str, String)> = [
                        ("Gear", facts.gear.clone()),
                        ("Captured by", facts.modeled_by.clone()),
                        (
                            "Model",
                            facts.size.as_ref().map(|size| {
                                if ["Standard", "Lite", "Feather", "Nano"].contains(&size.as_str())
                                {
                                    format!("WaveNet {}", size.to_lowercase())
                                } else {
                                    size.clone()
                                }
                            }),
                        ),
                        (
                            "Input level",
                            facts.input_dbu.map(|level| format!("{level:.1} dBu")),
                        ),
                    ]
                    .into_iter()
                    .filter_map(|(key, value)| Some((key, value?)))
                    .collect();
                    if rows.is_empty() {
                        quiet(ui, "The file says nothing about itself.");
                    }
                    for (key, value) in rows {
                        fact_row(ui, key, &value);
                    }
                }
                None => quiet(
                    ui,
                    "TonePush reads this from a checked backup of the pedal; the next one will \
                     include this file.",
                ),
            }
        });

        // The presets that play it.
        section(ui, &mut |ui| {
            if self.rollback.is_none() {
                caption_line(ui, "Used by");
                ui.add_space(9.0);
                quiet(
                    ui,
                    "Which presets play it shows once TonePush holds a checked backup.",
                );
                return;
            }
            caption_line(
                ui,
                &match users.len() {
                    0 => "Used by no preset".to_owned(),
                    1 => "Used by 1 preset".to_owned(),
                    n => format!("Used by {n} presets"),
                },
            );
            ui.add_space(6.0);
            for user in &users {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
                let y = rect.center().y;
                let label = shell::galley(
                    ui,
                    slot_label(user.preset),
                    theme::medium(theme::SECONDARY),
                    theme::muted(),
                );
                shell::paint_line(ui, label, rect.left(), y);
                let block =
                    shell::galley(ui, user.block.clone(), theme::regular(12.0), theme::muted());
                let block_width = block.size().x;
                shell::paint_line(ui, block, rect.right() - block_width, y);
                let name = shell::elided(
                    ui,
                    user.name.clone(),
                    theme::regular(theme::SECONDARY),
                    theme::text(),
                    rect.width() - 38.0 - block_width - 10.0,
                );
                shell::paint_line(ui, name, rect.left() + 38.0, y);
            }
        });

        // What can be done with it.
        let used = !users.is_empty();
        let mut export = false;
        let mut export_side = None;
        let mut rename = false;
        let mut remove = false;
        let renaming = self.slot_renaming == Some((library, slot));
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(width - 32.0);
                if renaming {
                    let mut draft = self
                        .names
                        .get(&(library, slot))
                        .cloned()
                        .unwrap_or_else(|| name.clone());
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut draft)
                            .desired_width(width - 32.0)
                            .hint_text("Name"),
                    );
                    if !field.has_focus() && !field.lost_focus() {
                        field.request_focus();
                    }
                    self.names.insert((library, slot), draft.clone());
                    if field.lost_focus() {
                        if ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && !draft.trim().is_empty()
                            && draft.trim() != name
                        {
                            let _ = self.tx.send(Cmd::Rename {
                                library,
                                index: slot,
                                name: draft.trim().to_owned(),
                            });
                        }
                        self.slot_renaming = None;
                    }
                    ui.add_space(6.0);
                    quiet(ui, "Enter renames it on the pedal; Esc keeps the name.");
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    let export_label = match (library, &pair) {
                        (Library::Irs, Some(_)) => "Export stereo WAV",
                        (Library::Irs, None) => "Export WAV",
                        _ => "Export .nam",
                    };
                    export = theme::Button::new(export_label)
                        .small()
                        .icon(Icon::Download)
                        .enabled(self.online && !self.busy)
                        .show(ui)
                        .clicked();
                    if pair.is_some() {
                        let side = theme::Button::new("One side")
                            .small()
                            .ghost()
                            .trailing(Icon::ChevronDown)
                            .enabled(self.online && !self.busy)
                            .show(ui);
                        egui::Popup::menu(&side).gap(4.0).show(|ui| {
                            theme::menu_width(ui, 200.0);
                            if let Some((left, _)) = &pair {
                                if theme::menu_item(ui, None, "Left as a mono WAV", None).clicked() {
                                    export_side = Some(*left);
                                }
                                if theme::menu_item(ui, None, "Right as a mono WAV", None).clicked() {
                                    export_side = Some(left + 1);
                                }
                            }
                        });
                    }
                    let why = if used {
                        format!(
                            "{} {} play it",
                            users.len(),
                            if users.len() == 1 { "preset" } else { "presets" }
                        )
                    } else {
                        self.write_refusal().unwrap_or_else(|| "The pedal is busy".to_owned())
                    };
                    rename = theme::Button::new("Rename")
                        .small()
                        .ghost()
                        .icon(Icon::TextCursorInput)
                        .enabled(writable && !used && pair.is_none())
                        .show(ui)
                        .on_disabled_hover_text(&why)
                        .clicked();
                    remove = theme::Button::new("Remove")
                        .small()
                        .ghost()
                        .icon(Icon::Remove)
                        .enabled(writable && !used)
                        .show(ui)
                        .on_disabled_hover_text(&why)
                        .clicked();
                });
                if used {
                    ui.add_space(10.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (spot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        theme::paint_icon(ui, Icon::Info, spot.center(), 14.0, theme::muted());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "Presets find this {} by its name, so TonePush won't rename or \
                                     remove it while {} {} it.",
                                    if library == Library::Irs { "impulse response" } else { "model" },
                                    users_words(users.len()),
                                    if users.len() == 1 { "uses" } else { "use" }
                                ))
                                .font(theme::regular(12.0))
                                .color(theme::muted()),
                            )
                            .wrap(),
                        );
                    });
                }
            });
        if export {
            match &pair {
                Some((left, base)) => {
                    if let Some(file) = rfd::FileDialog::new()
                        .set_file_name(format!("{}.wav", sanitise(base)))
                        .save_file()
                    {
                        let _ = self.tx.send(Cmd::ExportStereo {
                            left: *left,
                            right: left + 1,
                            file,
                        });
                    }
                }
                None => self.export_slot(library, slot, &name),
            }
        }
        if let Some(side) = export_side {
            let side_name = state
                .info
                .names
                .get(side)
                .cloned()
                .flatten()
                .unwrap_or_default();
            self.export_slot(library, side, &side_name);
        }
        if rename {
            self.names.insert((library, slot), name.clone());
            self.slot_renaming = Some((library, slot));
        }
        if remove {
            let slots: Vec<usize> = match &pair {
                Some((left, _)) => vec![*left, left + 1],
                None => vec![slot],
            };
            let what = match &pair {
                Some(_) => format!(
                    "the stereo pair {title} from slots {} and {}",
                    slots[0] + 1,
                    slots[1] + 1
                ),
                None => format!("{name} from {} slot {}", library.title(), slot + 1),
            };
            // A pair goes one slot at a time, the right after the left.
            self.confirmation = Some(Confirmation {
                action: "Remove from the pedal",
                question: format!("Remove {what}?"),
                command: Cmd::Clear {
                    library,
                    index: slots[0],
                },
            });
            if let Some(right) = slots.get(1) {
                self.after_confirmation = Some(Cmd::Clear {
                    library,
                    index: *right,
                });
            }
        }
        if replace {
            self.import_into_slot(&state, slot);
        }
        if up {
            let _ = self.tx.send(Cmd::Move {
                library,
                from: slot,
                to: slot - 1,
            });
            self.target_slots.insert(library, slot);
        }
        if down {
            let _ = self.tx.send(Cmd::Move {
                library,
                from: slot,
                to: slot + 1,
            });
            self.target_slots.insert(library, slot + 2);
        }
    }

    /// Write one slot's file out.
    fn export_slot(&mut self, library: Library, index: usize, name: &str) {
        let stem = sanitise(if name.is_empty() { "slot" } else { name });
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
}

/// Everything a checked backup says about the pedal's library files: what
/// each says about itself, and which presets play it.
pub(crate) fn facts_of(bundle: &voidx_client::backup::VerifiedBundle) -> LibraryFacts {
    let mut facts = LibraryFacts::default();
    for library in [Library::Amps, Library::Drives, Library::Irs] {
        let Some(list) = bundle.list(library.path()) else {
            continue;
        };
        for slot in &list.slots {
            let Some(name) = &slot.name else {
                continue;
            };
            if let Some(blob) = bundle.blob(library.path(), slot.index) {
                let file = match library {
                    Library::Irs => Some(ir_facts(blob)),
                    _ => voidx_client::nam::decode(blob, list.size)
                        .ok()
                        .map(|document| nam_facts(&document)),
                };
                if let Some(file) = file {
                    facts.files.insert((library, name.clone()), file);
                }
            }
            let users: Vec<User> = bundle
                .references_to(library.path(), name)
                .unwrap_or_default()
                .into_iter()
                .map(|reference| User {
                    preset: reference.preset_index,
                    name: reference.preset_name,
                    block: app_group(&reference.node_path)
                        .map(|group| super::board::tile_name(&group))
                        .unwrap_or_default(),
                })
                .collect();
            if !users.is_empty() {
                facts.users.insert((library, name.clone()), users);
            }
        }
    }
    facts
}

impl Panel {
    /// Files dropped on the window while a PRO is the pedal: a WAV goes to
    /// the impulse responses' first free slot (two, side by side, for a
    /// stereo one), and a `.nam` to the NAM library open on the Pedal page.
    pub(crate) fn dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        for file in dropped {
            let extension = file
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let library = match extension.as_str() {
                "wav" => Library::Irs,
                "nam" => match self.tab {
                    Tab::Library(library @ (Library::Amps | Library::Drives)) => library,
                    _ => {
                        self.status = "Open NAM amps or NAM drives on the Pedal page, then drop \
                                       the .nam file there"
                            .to_owned();
                        continue;
                    }
                },
                _ => continue,
            };
            if let Some(why) = self.write_refusal() {
                self.status = format!("Importing: {}", why.to_lowercase());
                continue;
            }
            let Some(state) = snapshot
                .libraries
                .iter()
                .find(|state| state.library == library)
            else {
                continue;
            };
            match (0..state.info.count)
                .find(|slot| state.info.names.get(*slot).is_none_or(Option::is_none))
            {
                Some(slot) => self.import_file(state, slot, file),
                None => self.status = format!("Every {} slot is in use", library.title()),
            }
        }
    }
}

/// What a library's file is called in a sentence.
fn library_noun_of(library: Library) -> &'static str {
    match library {
        Library::Amps => "NAM amp",
        Library::Drives => "NAM drive",
        Library::Irs => "Impulse response",
        Library::Presets => "Preset",
    }
}

/// A line across the inspector.
fn divider(ui: &mut Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, colour));
}

/// A section's caption, spaced capitals in the muted ink.
fn caption_line(ui: &mut Ui, text: &str) {
    let galley = ui.painter().layout_job(theme::paint::spaced(
        &text.to_uppercase(),
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
}

/// A key and its value.
fn fact_row(ui: &mut Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.set_height(28.0);
        let (place, _) = ui.allocate_exact_size(Vec2::new(108.0, 20.0), Sense::hover());
        let k = shell::galley(ui, key, theme::regular(12.5), theme::muted());
        shell::paint_line(ui, k, place.left(), place.center().y);
        theme::label_truncated(ui, value, theme::regular(12.5), theme::text());
    });
}

/// A quiet sentence.
fn quiet(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(12.0))
                .color(theme::muted()),
        )
        .wrap(),
    );
}

/// An impulse response's shape, as a line.
fn waveform(ui: &mut Ui, wave: &[f32], colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
    let painter = ui.painter();
    let mid = rect.center().y;
    painter.hline(rect.x_range(), mid, Stroke::new(1.0, theme::line()));
    if wave.len() < 2 {
        return;
    }
    let peak = wave
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        .max(f32::EPSILON);
    let step = rect.width() / (wave.len() - 1) as f32;
    let points: Vec<Pos2> = wave
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            Pos2::new(
                rect.left() + index as f32 * step,
                mid - sample / peak * (rect.height() / 2.0 - 2.0),
            )
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.4, colour)));
}

/// Where a dropped WAV goes, said under the table.
fn drop_note(ui: &mut Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 68.0), Sense::hover());
    theme::paint::dashed_rect(
        ui.painter(),
        rect.shrink(0.75),
        f32::from(theme::RADIUS_CARD) - 0.75,
        Stroke::new(1.0, theme::line_strong()),
        5.0,
        4.0,
    );
    let colour = theme::category_colour("IR");
    let well = Rect::from_min_size(
        Pos2::new(rect.left() + 16.0, rect.center().y - 18.0),
        Vec2::splat(36.0),
    );
    ui.painter()
        .rect_filled(well, CornerRadius::same(10), theme::alpha(colour, 0.14));
    theme::paint_icon(ui, Icon::FileAudio, well.center(), 18.0, colour);
    let x = well.right() + 14.0;
    let title = shell::galley(
        ui,
        "Drop WAV files here",
        theme::semibold(13.0),
        theme::text(),
    );
    shell::paint_line(ui, title, x, rect.center().y - 9.0);
    let body = shell::elided(
        ui,
        "48 kHz. A mono file takes one slot; a stereo file takes two free slots side by side, \
         left then right.",
        theme::regular(12.0),
        theme::muted(),
        rect.right() - x - 16.0,
    );
    shell::paint_line(ui, body, x, rect.center().y + 10.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_pairs_are_found_by_their_names() {
        let names = vec![
            Some("V30 · SM57".to_owned()),
            Some("Oxford Room L".to_owned()),
            Some("Oxford Room R".to_owned()),
            None,
            Some("Studio L".to_owned()),
            Some("Other R".to_owned()),
        ];
        assert_eq!(stereo_pairs(&names), vec![(1, "Oxford Room".to_owned())]);
    }

    /// A capture says what it models and how big its network is.
    #[test]
    fn a_capture_says_what_it_models() {
        let document = br#"{"version":"0.5.4","architecture":"WaveNet",
            "config":{"layers":[{"channels":12},{"channels":6}]},
            "metadata":{"gear_make":"Vox","gear_model":"AC30 Top Boost","modeled_by":"J. Rivera",
                "input_level_dbu":12.2},"weights":[]}"#;
        let facts = nam_facts(document);
        assert_eq!(facts.gear.as_deref(), Some("Vox AC30 Top Boost"));
        assert_eq!(facts.modeled_by.as_deref(), Some("J. Rivera"));
        assert_eq!(facts.size.as_deref(), Some("Lite"));
        assert_eq!(facts.input_dbu, Some(12.2));
        assert_eq!(nam_facts(b"not json").gear, None);
    }

    /// An impulse response's length leaves out the slot's padding.
    #[test]
    fn an_impulse_response_is_as_long_as_its_samples() {
        let mut blob = Vec::new();
        for index in 0..4_800 {
            let sample = if index % 2 == 0 { 0.5_f32 } else { -0.25 };
            blob.extend_from_slice(&sample.to_le_bytes());
        }
        blob.resize(98_304, 0);
        let facts = ir_facts(&blob);
        assert_eq!(facts.length_ms, Some(100.0));
        assert_eq!(facts.wave.len(), 240);
        assert!(facts.wave.iter().all(|sample| sample.abs() == 0.5));
    }

    #[test]
    fn the_table_shows_occupied_slots_pairs_and_the_first_free_one() {
        let state = LibraryState {
            library: Library::Irs,
            info: BlobList {
                path: NodePath::new("root\\ir_list").unwrap(),
                description: None,
                size: 4,
                count: 6,
                chunk_size: 4,
                group: None,
                gzip: false,
                movable: true,
                item_type: None,
                names: vec![
                    Some("V30".to_owned()),
                    Some("Room L".to_owned()),
                    Some("Room R".to_owned()),
                    None,
                    Some("Hall".to_owned()),
                    None,
                ],
            },
        };
        assert_eq!(
            rows(&state, ""),
            vec![
                Row::Slot(0),
                Row::Pair(1, "Room".to_owned()),
                Row::Slot(4),
                Row::Free(3),
            ]
        );
        assert_eq!(rows(&state, "hall"), vec![Row::Slot(4)]);
    }
}
