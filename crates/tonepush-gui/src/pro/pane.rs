//! The StompStation PRO's pane (docs/design/redesign-2026-10-01, screens 09
//! and 21): the chosen block's head and its face, in the same components as
//! an HX block's.
//!
//! The face is built from the pedal's schema, not from a list kept here: the
//! block's on/off as the footswitch it is, the model it plays from one of the
//! pedal's libraries as a wide cell with Change, and every other control as a
//! knob, a switch or a menu, in balanced rows of the largest knobs that fit.
//! On a large window the pedal's protection and what the library knows of
//! the preset sit beside the block.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::*;
use crate::pane::{card_head, centred, face_height, fit_face, knob_sizes, CELL_TEXT, ROW_GAP};
use crate::shell;
use crate::theme::{Icon, Tier};

/// What the library knows of the loaded preset, for the large pane's card.
#[derive(Clone, Debug, Default)]
pub(crate) struct LibraryNote {
    /// The tone's name and version, when the library holds one by this name.
    pub(crate) kept: Option<(String, u32)>,
    /// Song, artist and the rest, in order; empty ones are left out.
    pub(crate) rows: Vec<(&'static str, String)>,
    /// Whether the library's copy is the pedal's.
    pub(crate) sync: theme::Sync,
}

/// A model cell's width: three knobs' worth, as the design draws a capture.
const MODEL_CELL: f32 = 214.0;

/// The block's on/off.
struct Switch {
    path: NodePath,
    description: NodeDescription,
    current: Value,
    on: bool,
    /// Its off and on values, in that order.
    values: (Value, Value),
}

/// A model the block plays from one of the pedal's libraries.
struct ModelCell {
    path: NodePath,
    description: NodeDescription,
    current: Value,
    library: Library,
    /// Every installed choice, by slot.
    choices: Vec<(usize, String)>,
}

/// One control of a face.
enum Control {
    Knob {
        path: NodePath,
        description: NodeDescription,
        current: Value,
    },
    Toggle {
        path: NodePath,
        description: NodeDescription,
        current: Value,
        on: bool,
        values: (Value, Value),
    },
    Menu {
        path: NodePath,
        description: NodeDescription,
        current: Value,
        choices: Vec<Value>,
    },
    Reading {
        label: String,
        text: String,
    },
}

/// A block's controls, gathered before drawing.
#[derive(Default)]
struct Face {
    switch: Option<Switch>,
    models: Vec<ModelCell>,
    controls: Vec<Control>,
}

/// An edit a face asked for: the node, its value before, and the new one.
struct Asked {
    path: NodePath,
    description: NodeDescription,
    before: Value,
    value: Value,
}

/// What a library's slot is called in a sentence.
fn library_noun(library: Library) -> &'static str {
    match library {
        Library::Amps => "NAM amp",
        Library::Drives => "NAM drive",
        Library::Irs => "Impulse response",
        Library::Presets => "Preset",
    }
}

/// A cell's model caption.
fn model_caption(library: Library) -> &'static str {
    match library {
        Library::Irs => "IMPULSE RESPONSE",
        _ => "MODEL",
    }
}

/// A typed reading as a number in the node's range: the first number in the
/// text, so "120 ms" and "120" both read.
fn typed_number(text: &str, description: &NodeDescription) -> Option<f64> {
    let start = text.find(|c: char| c.is_ascii_digit() || c == '-' || c == '.')?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.'))
        .unwrap_or(rest.len());
    let number: f64 = rest[..end].parse().ok()?;
    let min = description.min.unwrap_or(f64::MIN);
    let max = description.max.unwrap_or(f64::MAX);
    Some(number.clamp(min, max))
}

/// A float node's value moved to its step, as the pedal stores it.
fn stepped(description: &NodeDescription, number: f64) -> f64 {
    description
        .step
        .filter(|step| *step > 0.0)
        .map(|step| {
            let base = description.min.unwrap_or(0.0);
            base + ((number - base) / step).round() * step
        })
        .unwrap_or(number)
}

impl Panel {
    /// What the pane shows for a group: a block's controls, or the input's
    /// or the output's. `live` reads the values being edited; a preview
    /// reads the preset's own.
    fn face_of(&self, snapshot: &Snapshot, group: &str, live: bool) -> Face {
        let persistent = group == INPUT_GROUP;
        let source = if persistent {
            &snapshot.settings
        } else {
            &snapshot.app
        };
        let mut face = Face::default();
        for (path, description) in source {
            let in_group = if persistent {
                path.as_str().starts_with("root\\settings\\input\\")
            } else {
                app_group(path.as_str()).as_deref() == Some(group)
            };
            if !in_group {
                continue;
            }
            let value = if live {
                self.drafts
                    .get(path.as_str())
                    .cloned()
                    .or_else(|| description.value.clone())
            } else {
                description.value.clone()
            };
            let Some(current) = value else {
                continue;
            };
            if !editable_node(path.as_str(), description, &current) {
                continue;
            }
            let leaf = path.as_str().rsplit('\\').next().unwrap_or_default();
            if !persistent && face.switch.is_none() && matches!(leaf, "on_off" | "enable" | "on") {
                let values = toggle_choices(description)
                    .or_else(|| current.is_boolean().then(|| (false.into(), true.into())));
                if let Some(values) = values {
                    face.switch = Some(Switch {
                        on: current == values.1,
                        path: path.clone(),
                        description: description.clone(),
                        current,
                        values,
                    });
                    continue;
                }
            }
            if matches!(description.kind, Some(NodeKind::PropertyList)) {
                let library = description.reference.as_deref().and_then(|reference| {
                    snapshot
                        .libraries
                        .iter()
                        .find(|state| state.info.path.as_str() == reference)
                });
                if let Some(state) = library.filter(|state| state.library != Library::Presets) {
                    face.models.push(ModelCell {
                        path: path.clone(),
                        description: description.clone(),
                        current,
                        library: state.library,
                        choices: state
                            .info
                            .names
                            .iter()
                            .enumerate()
                            .filter_map(|(slot, name)| Some((slot, name.clone()?)))
                            .collect(),
                    });
                    continue;
                }
            }
            face.controls.push(match description.kind {
                Some(NodeKind::Float) => Control::Knob {
                    path: path.clone(),
                    description: description.clone(),
                    current,
                },
                Some(NodeKind::Enum | NodeKind::Array) if toggle_choices(description).is_some() => {
                    let values = toggle_choices(description).expect("guarded above");
                    Control::Toggle {
                        on: current == values.1,
                        path: path.clone(),
                        description: description.clone(),
                        current,
                        values,
                    }
                }
                Some(NodeKind::Item) if current.is_boolean() => Control::Toggle {
                    on: current.as_bool().unwrap_or_default(),
                    path: path.clone(),
                    description: description.clone(),
                    current,
                    values: (false.into(), true.into()),
                },
                Some(NodeKind::Enum | NodeKind::Array | NodeKind::PropertyList) => {
                    let choices = selector_choices(snapshot, description);
                    Control::Menu {
                        path: path.clone(),
                        description: description.clone(),
                        current,
                        choices,
                    }
                }
                _ => Control::Reading {
                    label: node_label(path, description),
                    text: value_text(&current),
                },
            });
        }
        // A block's mode says what the rest of its controls mean: it leads.
        if let Some(index) = face.controls.iter().position(|control| {
            matches!(control, Control::Menu { description, .. }
                if description.desc.as_deref().is_some_and(|desc| desc.eq_ignore_ascii_case("mode")))
        }) {
            let mode = face.controls.remove(index);
            face.controls.insert(0, mode);
        }
        face
    }

    /// Send one edit, kept in the drafts at once.
    fn send_edit(&mut self, asked: Asked, persistent: bool) {
        if asked.description.validate_value(&asked.value).is_err() {
            return;
        }
        self.drafts
            .insert(asked.path.to_string(), asked.value.clone());
        self.recompute_dirty();
        let _ = self.tx.send(Cmd::SetNode {
            path: asked.path,
            description: Box::new(asked.description),
            before: asked.before,
            value: asked.value,
            persistent,
        });
    }

    /// The colour and category of what the pane shows.
    fn pane_category(group: &str) -> &'static str {
        match group {
            INPUT_GROUP => "Input",
            "output" => "Output",
            group => group_category(group),
        }
    }

    /// The name the pane gives a group.
    fn pane_title(group: &str) -> String {
        match group {
            INPUT_GROUP => "Input".to_owned(),
            "output" => "Output".to_owned(),
            group => super::board::tile_name(group),
        }
    }

    /// What the block is, after its name: its category in its colour, then
    /// what it plays and whether it is off.
    fn pane_meta(snapshot: &Snapshot, group: &str, face: &Face) -> Vec<(String, bool)> {
        match group {
            INPUT_GROUP => {
                return vec![
                    ("Input".to_owned(), true),
                    ("kept in the pedal's settings".to_owned(), false),
                ]
            }
            "output" => {
                return vec![
                    ("Output".to_owned(), true),
                    ("the preset's level and tempo".to_owned(), false),
                ]
            }
            _ => {}
        }
        let mut parts = vec![(group_category(group).to_owned(), true)];
        for model in &face.models {
            let slot = model
                .choices
                .iter()
                .find(|(_, name)| Value::String(name.clone()) == model.current)
                .map(|(slot, _)| format!(" in slot {}", slot + 1))
                .unwrap_or_default();
            parts.push((format!("{}{slot}", library_noun(model.library)), false));
        }
        if face.models.is_empty() {
            if let Some(mode) = block_selector(snapshot, group)
                .and_then(|(_, description)| description.value)
                .map(|value| value_text(&value))
            {
                parts.push((mode, false));
            }
        }
        if face.switch.as_ref().is_some_and(|switch| !switch.on) {
            parts.push(("off".to_owned(), false));
        }
        parts
    }

    /// The pane under the board: the chosen block's head and face, and on a
    /// large window the pedal's protection and the library's card beside it.
    pub(super) fn pane(
        &mut self,
        root: &mut Ui,
        snapshot: &Snapshot,
        tier: Tier,
        note: &LibraryNote,
    ) {
        let mut keep = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                let pane = ui.max_rect();
                let group = self.selected_group.clone();
                let large = tier == Tier::L;
                let side = if large { 420.0 } else { 0.0 };
                egui::ScrollArea::vertical()
                    .id_salt("pro-pane")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let room = pane.width() - if large { side + 16.0 } else { 0.0 };
                            ui.allocate_ui_with_layout(
                                Vec2::new(room, 10.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(room);
                                    self.block_column(ui, snapshot, &group, tier, pane.height());
                                },
                            );
                            if large {
                                ui.add_space(16.0);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(side - 16.0, 10.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(side - 16.0);
                                        ui.spacing_mut().item_spacing.y = 16.0;
                                        ui.add_space(16.0);
                                        self.protection_card(ui, snapshot);
                                        keep = self.library_card(ui, note);
                                    },
                                );
                            }
                        });
                        ui.add_space(12.0);
                    });
            });
        if let Some(replace) = keep {
            if let Some(index) = active_preset_index(snapshot) {
                let _ = self.tx.send(Cmd::ReadPreset {
                    index,
                    target: ReadTarget::Library { replace },
                });
            }
        }
    }

    /// A preset being looked at: the chosen block's head and face from the
    /// preset's own values, drawn but not answered.
    pub(super) fn preview_pane(&mut self, ui: &mut Ui, snapshot: &Snapshot, tier: Tier) {
        let group = self.selected_group.clone();
        let face = self.face_of(snapshot, &group, false);
        let colour = theme::category_colour(Self::pane_category(&group));
        self.block_head(ui, snapshot, &group, &face, colour);
        ui.add_enabled_ui(false, |ui| {
            ui.horizontal_top(|ui| {
                ui.add_space(20.0);
                let width = ui.available_width() - 16.0;
                let _ = self.face(ui, &face, colour, width, None, knob_sizes(tier));
            });
        });
    }

    /// The head and the face of what the pane shows.
    fn block_column(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        group: &str,
        tier: Tier,
        height: f32,
    ) {
        let face = self.face_of(snapshot, group, true);
        let category = Self::pane_category(group);
        let colour = theme::category_colour(category);
        self.block_head(ui, snapshot, group, &face, colour);
        let persistent = group == INPUT_GROUP;
        let guarded = !persistent || self.rollback.is_some();
        if persistent && !guarded {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                theme::label(
                    ui,
                    "These are the pedal's own settings, so they wait for a checked backup of it.",
                    theme::regular(theme::SECONDARY),
                    theme::muted(),
                );
            });
            ui.add_space(8.0);
        }
        let enabled = self.online && !self.busy && guarded && self.read_only.is_none();
        ui.horizontal_top(|ui| {
            ui.add_space(20.0);
            let width = ui.available_width() - 16.0;
            ui.add_enabled_ui(enabled, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 10.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if face.switch.is_none()
                            && face.models.is_empty()
                            && face.controls.is_empty()
                        {
                            theme::label(
                                ui,
                                "Nothing here to set.",
                                theme::regular(theme::SECONDARY),
                                theme::muted(),
                            );
                            return;
                        }
                        let asked = self.face(
                            ui,
                            &face,
                            colour,
                            width,
                            Some(height - 64.0 - 16.0),
                            knob_sizes(tier),
                        );
                        if let Some(asked) = asked {
                            self.send_edit(asked, persistent);
                        }
                    },
                );
            });
        });
    }

    /// The chosen block's head: its drawing in a well, its name, and what it
    /// is.
    fn block_head(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        group: &str,
        face: &Face,
        colour: Color32,
    ) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 64.0), Sense::hover());
        let inner = Rect::from_min_max(
            Pos2::new(rect.left() + 20.0, rect.top() + 14.0),
            Pos2::new(rect.right() - 16.0, rect.bottom() - 10.0),
        );
        let y = inner.center().y;
        let well = Rect::from_min_size(Pos2::new(inner.left(), y - 20.0), Vec2::splat(40.0));
        theme::paint::gradient_rect(
            ui.painter(),
            well,
            11.0,
            &[
                (0.0, theme::alpha(colour, 0.24)),
                (1.0, theme::alpha(colour, 0.08)),
            ],
        );
        ui.painter().rect_stroke(
            well,
            CornerRadius::same(11),
            Stroke::new(1.0, theme::alpha(colour, 0.4)),
            egui::StrokeKind::Inside,
        );
        if let Some(art) = theme::category_icon(Self::pane_category(group)) {
            art.paint(
                ui,
                Rect::from_center_size(well.center(), Vec2::splat(22.0)),
                colour,
            );
        }
        let x = well.right() + 12.0;
        let heading = shell::title_galley(
            ui,
            &Self::pane_title(group),
            theme::BLOCK_NAME,
            theme::text(),
            (inner.right() - x).max(120.0),
        );
        shell::paint_line(ui, heading, x, y - 9.0);
        let mut meta_x = x;
        for (index, (part, strong)) in Self::pane_meta(snapshot, group, face)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                let dot = shell::galley(ui, "·", theme::regular(12.0), theme::muted());
                shell::paint_line(ui, dot, meta_x, y + 11.0);
                meta_x += 10.0;
            }
            let galley = shell::galley(
                ui,
                part,
                if strong {
                    theme::semibold(12.0)
                } else {
                    theme::regular(12.0)
                },
                if strong { colour } else { theme::muted() },
            );
            let w = galley.size().x;
            shell::paint_line(ui, galley, meta_x, y + 11.0);
            meta_x += w + 6.0;
        }
    }

    /// The face: the on/off column, the models it plays, and its controls
    /// in balanced rows. Returns an edit when one was made.
    fn face(
        &mut self,
        ui: &mut Ui,
        face: &Face,
        colour: Color32,
        width: f32,
        height: Option<f32>,
        sizes: &[f32],
    ) -> Option<Asked> {
        let lead = face.switch.is_some();
        let models = if face.models.is_empty() {
            0.0
        } else {
            MODEL_CELL + 17.0
        };
        let (knob, rows) = fit_face(&[face.controls.len()], lead, width - models, height, sizes);
        let rows = rows.into_iter().next().unwrap_or_default();
        let cell = knob + 26.0;
        let lead_width = if lead { cell + 17.0 } else { 0.0 };
        let widest = rows.iter().copied().max().unwrap_or(0);
        let natural = 22.0 + lead_width + models + widest as f32 * cell;
        let outer = natural.min(width).max(160.0);
        let knob_rows = face_height(knob, std::slice::from_ref(&rows)) - 24.0 - ROW_GAP;
        let model_rows = face.models.len() as f32 * (knob + CELL_TEXT)
            + face.models.len().saturating_sub(1) as f32 * ROW_GAP;
        let body_height = knob_rows.max(model_rows).max(knob + CELL_TEXT);
        let mut asked = None;
        egui::Frame::new()
            .fill(theme::panel())
            .stroke(Stroke::new(1.0, theme::line()))
            .corner_radius(CornerRadius::same(theme::RADIUS_CARD))
            .inner_margin(egui::Margin {
                left: 10,
                right: 10,
                top: 14,
                bottom: 10,
            })
            .show(ui, |ui| {
                ui.set_width(outer - 22.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let divider = |ui: &mut Ui| {
                        let (line, _) =
                            ui.allocate_exact_size(Vec2::new(17.0, body_height), Sense::hover());
                        ui.painter().vline(
                            line.center().x,
                            (line.top() + 4.0)..=(line.bottom() - 24.0).max(line.top() + 4.0),
                            Stroke::new(1.0, theme::line()),
                        );
                    };
                    if let Some(switch) = &face.switch {
                        if let Some(edit) = switch_cell(ui, switch, (knob, cell), colour) {
                            asked = Some(edit);
                        }
                        divider(ui);
                    }
                    if !face.models.is_empty() {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(0.0, ROW_GAP);
                            for model in &face.models {
                                if let Some(edit) = model_cell(ui, model, knob + CELL_TEXT) {
                                    asked = Some(edit);
                                }
                            }
                        });
                        if !face.controls.is_empty() {
                            divider(ui);
                        }
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(0.0, ROW_GAP);
                        let mut start = 0;
                        for count in &rows {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                for control in &face.controls[start..start + count] {
                                    if let Some(edit) =
                                        self.control_cell(ui, control, (knob, cell), colour)
                                    {
                                        asked = Some(edit);
                                    }
                                }
                            });
                            start += count;
                        }
                    });
                });
            });
        asked
    }

    /// One control: its knob, switch or menu, the reading under it (a
    /// knob's can be typed), and its name.
    fn control_cell(
        &mut self,
        ui: &mut Ui,
        control: &Control,
        (knob, cell): (f32, f32),
        colour: Color32,
    ) -> Option<Asked> {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(cell, knob + CELL_TEXT), Sense::hover());
        let face = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.top() + knob / 2.0),
            Vec2::splat(knob),
        );
        let reading = Rect::from_min_size(
            Pos2::new(rect.left(), face.bottom() + 3.0),
            Vec2::new(cell, 17.0),
        );
        let label = Rect::from_min_size(reading.left_bottom(), Vec2::new(cell, 15.0));
        let mut asked = None;
        let name = match control {
            Control::Knob {
                path,
                description,
                current,
            } => {
                let min = description.min.unwrap_or(0.0) as f32;
                let max = description.max.unwrap_or(100.0) as f32;
                let mut number = current.as_f64().unwrap_or_default() as f32;
                let default = description.default.as_ref().and_then(Value::as_f64);
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("pro-knob", path.as_str()))
                        .max_rect(face),
                );
                let hit = theme::dial(
                    &mut child,
                    &mut number,
                    min..=max,
                    knob,
                    colour,
                    default.map(|default| default as f32),
                )
                .on_hover_text(
                    "Drag to turn, Shift-drag for fine moves. Double-click for the default",
                );
                let changed = if hit.double_clicked() {
                    default
                } else if hit.changed() {
                    Some(stepped(description, f64::from(number)))
                } else {
                    None
                };
                if let Some(value) = changed.and_then(json_number) {
                    asked = Some(Asked {
                        path: path.clone(),
                        description: description.clone(),
                        before: current.clone(),
                        value,
                    });
                }
                // The reading, which a click turns into a field.
                let typing = self
                    .typing
                    .as_ref()
                    .filter(|(typed, _)| typed == path.as_str())
                    .map(|(_, text)| text.clone());
                match typing {
                    Some(mut text) => {
                        let width = (cell - 8.0).min(80.0);
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt(("pro-typing", path.as_str()))
                                .max_rect(Rect::from_center_size(
                                    reading.center(),
                                    Vec2::new(width, 20.0),
                                )),
                        );
                        let field = child.add(
                            egui::TextEdit::singleline(&mut text)
                                .desired_width(width)
                                .horizontal_align(egui::Align::Center)
                                .margin(egui::Margin::symmetric(4, 1))
                                .font(theme::medium(theme::BODY)),
                        );
                        if !field.has_focus() && !field.lost_focus() {
                            field.request_focus();
                        }
                        if field.lost_focus() {
                            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                                if let Some(value) = typed_number(&text, description)
                                    .map(|number| stepped(description, number))
                                    .and_then(json_number)
                                {
                                    asked = Some(Asked {
                                        path: path.clone(),
                                        description: description.clone(),
                                        before: current.clone(),
                                        value,
                                    });
                                }
                            }
                            self.typing = None;
                        } else {
                            self.typing = Some((path.to_string(), text));
                        }
                    }
                    None => {
                        let shown = format_node_value(description, &Value::from(f64::from(number)));
                        centred(
                            ui,
                            shown.clone(),
                            theme::medium(theme::BODY),
                            theme::text(),
                            reading.center(),
                            cell,
                        );
                        let response = ui
                            .interact(
                                reading,
                                ui.id().with(("pro-reading", path.as_str())),
                                Sense::click(),
                            )
                            .on_hover_text("Click to type a value");
                        if response.clicked() {
                            self.typing = Some((path.to_string(), shown));
                        }
                    }
                }
                node_label(path, description)
            }
            Control::Toggle {
                path,
                description,
                current,
                on,
                values,
            } => {
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("pro-toggle", path.as_str()))
                        .max_rect(Rect::from_center_size(face.center(), Vec2::new(34.0, 20.0))),
                );
                let mut lit = *on;
                if theme::toggle(&mut child, &mut lit, colour).changed() {
                    asked = Some(Asked {
                        path: path.clone(),
                        description: description.clone(),
                        before: current.clone(),
                        value: if lit {
                            values.1.clone()
                        } else {
                            values.0.clone()
                        },
                    });
                }
                centred(
                    ui,
                    shell::sentence_case(&value_text(current).to_lowercase()),
                    theme::medium(theme::BODY),
                    theme::text(),
                    reading.center(),
                    cell,
                );
                node_label(path, description)
            }
            Control::Menu {
                path,
                description,
                current,
                choices,
            } => {
                let width = (cell - 6.0).min(104.0);
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("pro-menu", path.as_str()))
                        .max_rect(Rect::from_center_size(
                            face.center(),
                            Vec2::new(width, 26.0),
                        ))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let shown = value_text(current);
                let button = theme::Button::new(&shown)
                    .small()
                    .trailing(Icon::ChevronDown)
                    .min_width(width)
                    .show(&mut child)
                    .on_hover_text(format!("{}: choose", node_label(path, description)));
                egui::Popup::menu(&button).gap(4.0).show(|ui| {
                    theme::menu_width(ui, 220.0);
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for choice in choices {
                                let chosen = choice == current;
                                if theme::menu_item(
                                    ui,
                                    chosen.then_some(Icon::Check),
                                    &value_text(choice),
                                    None,
                                )
                                .clicked()
                                    && !chosen
                                {
                                    asked = Some(Asked {
                                        path: path.clone(),
                                        description: description.clone(),
                                        before: current.clone(),
                                        value: choice.clone(),
                                    });
                                }
                            }
                        });
                });
                node_label(path, description)
            }
            Control::Reading { label, text } => {
                centred(
                    ui,
                    text.clone(),
                    theme::medium(theme::BODY),
                    theme::text(),
                    face.center(),
                    cell,
                );
                label.clone()
            }
        };
        centred(
            ui,
            name,
            theme::regular(theme::LABEL),
            theme::muted(),
            label.center(),
            cell - 4.0,
        );
        asked
    }

    /// The pedal's protection, as a card: how it stands, the backup that
    /// guards it, and what that backup holds.
    fn protection_card(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let protected = self.rollback.is_some();
        let (icon, title) = if self.read_only.is_some() {
            (Icon::Lock, "Read only")
        } else if protected {
            (Icon::ShieldCheck, "Protected")
        } else {
            (Icon::ShieldAlert, "Not protected yet")
        };
        let ready = self.online && !self.busy && self.working.is_none();
        let mut back_up = false;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            card_head(ui, Some(icon), title, "", |ui| {
                if self.read_only.is_none() {
                    back_up = theme::Button::new(if protected {
                        "Back up now"
                    } else {
                        "Back up to unlock saving"
                    })
                    .small()
                    .icon(Icon::Download)
                    .enabled(ready)
                    .show(ui)
                    .on_hover_text("Read the whole pedal into a checked backup on this computer")
                    .clicked();
                }
            });
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 14,
                    bottom: 16,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 9.0;
                    let when = match self.rollback_time.and_then(shell::clock) {
                        Some((when, true)) => format!("The backup from today at {when}"),
                        Some((when, false)) => format!("The backup of {when}"),
                        None => "A checked backup".to_owned(),
                    };
                    let words = if let Some(reason) = &self.read_only {
                        format!(
                            "TonePush browses, exports and backs up this pedal, and changes \
                             nothing: {reason}."
                        )
                    } else if protected {
                        format!(
                            "{when} matches this pedal on firmware {}, so saving, renaming and \
                             importing are on. If a write goes wrong, TonePush puts the pedal \
                             back from it.",
                            snapshot.identity.version
                        )
                    } else {
                        "Edits play on the pedal now. Saving, renaming and importing write its \
                         memory, so they wait until TonePush holds a checked backup of this \
                         pedal."
                            .to_owned()
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(words)
                                .font(theme::regular(12.5))
                                .color(theme::text_soft()),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                    for (key, value) in library_counts(snapshot) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (place, _) =
                                ui.allocate_exact_size(Vec2::new(96.0, 18.0), Sense::hover());
                            let k = shell::galley(ui, key, theme::regular(12.5), theme::muted());
                            shell::paint_line(ui, k, place.left(), place.center().y);
                            theme::label_truncated(ui, &value, theme::regular(12.5), theme::text());
                        });
                    }
                });
        });
        if back_up {
            self.back_up_here();
        }
    }

    /// What the library knows of the loaded preset, as a card. Says
    /// whether keeping it was asked for, and whether as a new version.
    fn library_card(&mut self, ui: &mut Ui, note: &LibraryNote) -> Option<bool> {
        let mut keep = None;
        let saved = !self.dirty;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let aside = note
                .kept
                .as_ref()
                .map(|(name, version)| format!("{name} · v{version}"))
                .unwrap_or_else(|| "not kept yet".to_owned());
            card_head(ui, Some(Icon::Library), "In your library", &aside, |ui| {
                let label = match (&note.kept, note.sync) {
                    (Some((_, version)), theme::Sync::Differs) => {
                        Some((format!("Keep as v{}", version + 1), true))
                    }
                    (None, _) => Some(("Keep in library".to_owned(), false)),
                    _ => None,
                };
                if let Some((label, replace)) = label {
                    if theme::Button::new(&label)
                        .small()
                        .icon(Icon::Computer)
                        .enabled(saved && self.online)
                        .show(ui)
                        .on_hover_text("Keep the saved preset in your library")
                        .on_disabled_hover_text(
                            "Save the preset first: the library keeps what is saved",
                        )
                        .clicked()
                    {
                        keep = Some(replace);
                    }
                }
            });
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 14,
                    bottom: 16,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if note.kept.is_none() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "Keep this preset to give it a song, a part and tags, and \
                                     to find it again later.",
                                )
                                .font(theme::regular(12.5))
                                .color(theme::muted()),
                            )
                            .wrap(),
                        );
                        return;
                    }
                    ui.spacing_mut().item_spacing.y = 9.0;
                    for (key, value) in &note.rows {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (place, _) =
                                ui.allocate_exact_size(Vec2::new(96.0, 18.0), Sense::hover());
                            let k = shell::galley(ui, *key, theme::regular(12.5), theme::muted());
                            shell::paint_line(ui, k, place.left(), place.center().y);
                            theme::label_truncated(ui, value, theme::regular(12.5), theme::text());
                        });
                    }
                });
        });
        keep
    }

    /// Edits play live but saving waits: the strip under the deck that says
    /// so, with the way to use a backup taken earlier.
    pub(crate) fn unlock_strip(&mut self, root: &mut Ui) {
        let mut existing = false;
        egui::Panel::top("pro-unlock")
            .exact_size(52.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::mix(theme::bg(), theme::hot(), 0.08)))
            .show(root, |ui| {
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    Stroke::new(1.0, theme::alpha(theme::hot(), 0.3)),
                );
                let inner = Rect::from_min_max(
                    Pos2::new(rect.left() + 20.0, rect.top()),
                    Pos2::new(rect.right() - 16.0, rect.bottom()),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(inner)
                        .id_salt("pro-unlock-actions")
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                existing = theme::Button::new("Use an existing backup…")
                    .ghost()
                    .small()
                    .enabled(self.online && !self.busy && self.working.is_none())
                    .show(&mut child)
                    .on_hover_text("Check a backup taken earlier against the pedal")
                    .clicked();
                let limit = child.min_rect().left() - 16.0;
                theme::paint_icon(
                    ui,
                    Icon::ShieldAlert,
                    Pos2::new(inner.left() + 8.0, inner.center().y),
                    16.0,
                    theme::hot(),
                );
                let left = inner.left() + 28.0;
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    "Edits play on the pedal now.",
                    0.0,
                    egui::TextFormat::simple(theme::semibold(12.5), theme::text()),
                );
                job.append(
                    "Saving, renaming and importing write its memory, so they wait until \
                     TonePush holds a checked backup of this pedal.",
                    5.0,
                    egui::TextFormat::simple(theme::regular(12.5), theme::text_soft()),
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: (limit - left).max(80.0),
                    max_rows: 2,
                    break_anywhere: false,
                    overflow_character: Some('…'),
                };
                let galley = ui.painter().layout_job(job);
                shell::paint_line(ui, galley, left, inner.center().y);
            });
        if existing {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose a backup of this pedal")
                .pick_folder()
            {
                let _ = self.tx.send(Cmd::UseRollback(path));
            }
        }
    }
}

/// The pedal's libraries in a line each: how many of their slots are used.
fn library_counts(snapshot: &Snapshot) -> Vec<(&'static str, String)> {
    snapshot
        .libraries
        .iter()
        .map(|state| {
            let used = state.info.occupied().count();
            let key = match state.library {
                Library::Presets => "Presets",
                Library::Amps => "NAM amps",
                Library::Drives => "NAM drives",
                Library::Irs => "IRs",
            };
            let mut value = format!("{used} of {}", state.info.count);
            if state.library == Library::Irs {
                let pairs = super::libraries::stereo_pairs(&state.info.names).len();
                if pairs > 0 {
                    value.push_str(&format!(
                        ", {} stereo {}",
                        pairs,
                        if pairs == 1 { "pair" } else { "pairs" }
                    ));
                }
            }
            (key, value)
        })
        .collect()
}

/// The block's on/off as the footswitch it is: a click turns it.
fn switch_cell(
    ui: &mut Ui,
    switch: &Switch,
    (knob, cell): (f32, f32),
    colour: Color32,
) -> Option<Asked> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(cell, knob + CELL_TEXT), Sense::hover());
    let face = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.top() + knob / 2.0),
        Vec2::splat(knob),
    );
    let response = ui
        .interact(
            face,
            ui.id().with(("pro-switch", switch.path.as_str())),
            Sense::click(),
        )
        .on_hover_text(if switch.on {
            "On. Click to turn it off"
        } else {
            "Off. Click to turn it on"
        });
    theme::paint_footswitch(ui.painter(), face, switch.on, colour, response.hovered());
    let reading = Rect::from_min_size(
        Pos2::new(rect.left(), face.bottom() + 3.0),
        Vec2::new(cell, 17.0),
    );
    centred(
        ui,
        if switch.on { "On" } else { "Off" },
        theme::medium(theme::BODY),
        if switch.on {
            theme::text()
        } else {
            theme::muted()
        },
        reading.center(),
        cell,
    );
    centred(
        ui,
        "On/Off",
        theme::regular(theme::LABEL),
        theme::muted(),
        Pos2::new(rect.center().x, reading.bottom() + 7.5),
        cell - 4.0,
    );
    response.clicked().then(|| Asked {
        path: switch.path.clone(),
        description: switch.description.clone(),
        before: switch.current.clone(),
        value: if switch.on {
            switch.values.0.clone()
        } else {
            switch.values.1.clone()
        },
    })
}

/// A model the block plays from one of the pedal's libraries, three knobs
/// wide: what it is, its name and slot, and Change.
fn model_cell(ui: &mut Ui, model: &ModelCell, height: f32) -> Option<Asked> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(MODEL_CELL, height), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        theme::bg(),
        Stroke::new(1.0, theme::line()),
        egui::StrokeKind::Inside,
    );
    let slot = model
        .choices
        .iter()
        .find(|(_, name)| Value::String(name.clone()) == model.current)
        .map(|(slot, _)| *slot);
    let left = rect.left() + 12.0;
    let caption = ui.painter().layout_job(theme::paint::spaced(
        model_caption(model.library),
        theme::bold(10.0),
        theme::muted(),
        0.08,
    ));
    let caption_width = caption.size().x;
    shell::paint_line(ui, caption, left, rect.top() + 18.0);
    if let Some(slot) = slot {
        let text = format!("{:02}", slot + 1);
        let tag = theme::Tag::new(&text).inset();
        let size = tag.size(ui);
        tag.paint(
            ui,
            Rect::from_min_size(
                Pos2::new(left + caption_width + 8.0, rect.top() + 18.0 - size.y / 2.0),
                size,
            ),
        );
    }
    let name = shell::elided(
        ui,
        value_text(&model.current),
        theme::semibold(14.0),
        theme::text(),
        MODEL_CELL - 24.0,
    );
    shell::paint_line(ui, name, left, rect.top() + 39.0);
    let sub = match slot {
        Some(slot) => format!("{} slot {}", library_noun(model.library), slot + 1),
        None => format!("Not in the {} library", library_noun(model.library)),
    };
    let sub = shell::elided(
        ui,
        sub,
        theme::regular(11.5),
        theme::muted(),
        MODEL_CELL - 24.0,
    );
    shell::paint_line(ui, sub, left, rect.top() + 57.0);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pro-model", model.path.as_str()))
            .max_rect(Rect::from_min_max(
                Pos2::new(left, rect.bottom() - 40.0),
                Pos2::new(rect.right() - 12.0, rect.bottom() - 10.0),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let button = theme::Button::new("Change")
        .small()
        .icon(Icon::Library)
        .trailing(Icon::ChevronDown)
        .show(&mut child)
        .on_hover_text(format!(
            "Choose another {} the pedal holds",
            library_noun(model.library).to_lowercase()
        ));
    let mut asked = None;
    egui::Popup::menu(&button).gap(4.0).show(|ui| {
        theme::menu_width(ui, 280.0);
        theme::menu_header(
            ui,
            &format!("{}s on the pedal", library_noun(model.library)),
            Some(&model.choices.len().to_string()),
        );
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for (slot, name) in &model.choices {
                    let value = Value::String(name.clone());
                    let chosen = value == model.current;
                    let hint = format!("{:02}", slot + 1);
                    if theme::menu_item(ui, chosen.then_some(Icon::Check), name, Some(&hint))
                        .clicked()
                        && !chosen
                    {
                        asked = Some(Asked {
                            path: model.path.clone(),
                            description: model.description.clone(),
                            before: model.current.clone(),
                            value,
                        });
                    }
                }
            });
    });
    asked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float(min: f64, max: f64, step: f64) -> NodeDescription {
        serde_json::from_value(serde_json::json!({
            "type": "float", "desc": "Time", "value": min, "min": min, "max": max, "step": step,
        }))
        .unwrap()
    }

    /// A reading is typed the way it is shown, unit and all, and lands in
    /// the node's range on its step.
    #[test]
    fn a_typed_reading_lands_on_the_nodes_step() {
        let time = float(20.0, 2000.0, 10.0);
        assert_eq!(typed_number("410 ms", &time), Some(410.0));
        assert_eq!(typed_number("5000", &time), Some(2000.0));
        assert_eq!(typed_number("ms", &time), None);
        assert_eq!(stepped(&time, 414.0), 410.0);
        let gain = float(-24.0, 12.0, 0.5);
        assert_eq!(typed_number("-1.5 dB", &gain), Some(-1.5));
        assert_eq!(stepped(&gain, -1.3), -1.5);
    }
}
