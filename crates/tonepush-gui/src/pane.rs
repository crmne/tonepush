//! The pane under the board (docs/design/redesign-2026-10-01, "The pane",
//! "Faceplate", and screens 01, 03 and 04): the selected block, or a lens.
//!
//! Knobs and the floor are always there. The footswitch editor and the
//! snapshot matrix open from their own anchors (the lens switch, a floor
//! chip, a row of the controls card) and replace the block until it is chosen
//! again; on a large window they sit beside it instead. The model browser is
//! a lens of its own, in `browser.rs`.

use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};
use hx_catalog::Kind;
use hx_proto::preset::{Assignment, Target};
use hx_proto::rpc::Source;

use crate::shell;
use crate::theme::{self, Icon, Mood, Tier};
use crate::{session, App, AssignAction, AssignMenu, BypassAction, BypassView, Cmd, Connection};

/// What the pane shows under the board.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Lens {
    #[default]
    Block,
    Footswitches,
    Snapshots,
}

impl Lens {
    fn label(self) -> &'static str {
        match self {
            Lens::Block => "Block",
            Lens::Footswitches => "Footswitches",
            Lens::Snapshots => "Snapshots",
        }
    }
}

/// What a tag says for a source, as the pedal prints it.
pub(crate) fn source_tag(source: Source) -> String {
    match source {
        Source::Footswitch(n) => format!("FS{n}"),
        Source::Expression(n) => format!("EXP {n}"),
        Source::MidiCc => "MIDI".to_owned(),
        Source::Snapshots => "Snapshots".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Fitting a face

/// Under a knob: the reading, the name and a row of tags.
const CELL_TEXT: f32 = 3.0 + 17.0 + 15.0 + 4.0 + 18.0;
/// Between rows of a face.
const ROW_GAP: f32 = 8.0;
/// The rule and the cab's name between an amp's controls and its cab's.
const GROUP_GAP: f32 = 30.0;

/// The knob sizes a face tries at each size of window, largest first.
pub(crate) fn knob_sizes(tier: Tier) -> &'static [f32] {
    match tier {
        Tier::S => &[60.0, 52.0, 44.0],
        Tier::M => &[68.0, 60.0, 52.0, 44.0],
        Tier::L => &[76.0],
    }
}

/// How many cells each row of a face holds: all of them on one row when they
/// fit, otherwise rows as even as they can be, never one straggler.
pub(crate) fn balanced_rows(count: usize, cell: f32, width: f32) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    if count as f32 * cell + 20.0 <= width {
        return vec![count];
    }
    let per = ((width - 37.0) / cell).floor().max(1.0) as usize;
    let rows = count.div_ceil(per);
    let each = count.div_ceil(rows);
    let mut out = Vec::with_capacity(rows);
    let mut left = count;
    while left > 0 {
        let n = each.min(left);
        out.push(n);
        left -= n;
    }
    out
}

/// The largest knob whose balanced rows fit the room, and each group's rows
/// at that size. A face's groups are a model's controls and, on an Amp+Cab,
/// its cab's. With no height to fit, the first size is taken.
pub(crate) fn fit_face(
    counts: &[usize],
    lead: bool,
    width: f32,
    height: Option<f32>,
    sizes: &[f32],
) -> (f32, Vec<Vec<usize>>) {
    let mut chosen = (sizes.last().copied().unwrap_or(44.0), Vec::new());
    for &size in sizes {
        let cell = size + 26.0;
        let lead_width = if lead { cell + 17.0 } else { 0.0 };
        let rows: Vec<Vec<usize>> = counts
            .iter()
            .map(|count| balanced_rows(*count, cell, width - lead_width))
            .collect();
        let needed = face_height(size, &rows);
        chosen = (size, rows);
        if height.is_none_or(|room| needed <= room) {
            break;
        }
    }
    chosen
}

/// How tall a face is with `rows` of knobs `size` across, padding included.
fn face_height(size: f32, rows: &[Vec<usize>]) -> f32 {
    let lines: usize = rows.iter().map(Vec::len).sum();
    let groups = rows.iter().filter(|group| !group.is_empty()).count();
    lines as f32 * (size + 66.0) + 24.0 + groups.saturating_sub(1) as f32 * GROUP_GAP
}

// ---------------------------------------------------------------------------
// Tags

/// A tag owned for a frame: what drives a control.
#[derive(Clone)]
struct TagSpec {
    text: String,
    mark: Option<theme::Mark>,
}

impl TagSpec {
    fn of(source: Source, led: Option<Color32>) -> TagSpec {
        match source {
            Source::Snapshots => TagSpec {
                text: String::new(),
                mark: Some(theme::Mark::Icon(Icon::Camera)),
            },
            Source::Footswitch(n) => TagSpec {
                text: format!("FS{n}"),
                mark: Some(theme::Mark::Led(led)),
            },
            other => TagSpec {
                text: source_tag(other),
                mark: None,
            },
        }
    }

    fn tag(&self) -> theme::Tag<'_> {
        theme::Tag {
            text: &self.text,
            mark: self.mark,
            inset: true,
            off: false,
        }
    }
}

/// A row of tags centred in `rect`, each answering a click.
fn tag_row(
    ui: &mut Ui,
    rect: Rect,
    tags: &[TagSpec],
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> Vec<Response> {
    let sizes: Vec<Vec2> = tags.iter().map(|tag| tag.tag().size(ui)).collect();
    let total: f32 =
        sizes.iter().map(|size| size.x).sum::<f32>() + 3.0 * tags.len().saturating_sub(1) as f32;
    let mut x = rect.center().x - total / 2.0;
    let id = ui.id().with(("tags", salt));
    let mut responses = Vec::with_capacity(tags.len());
    for (index, (tag, size)) in tags.iter().zip(sizes).enumerate() {
        let place = Rect::from_min_size(Pos2::new(x, rect.center().y - size.y / 2.0), size);
        tag.tag().paint(ui, place);
        responses.push(ui.interact(place, id.with(index), Sense::click()));
        x += size.x + 3.0;
    }
    responses
}

// ---------------------------------------------------------------------------
// One face

/// One control of a face, gathered before drawing.
struct Control {
    index: usize,
    param: hx_catalog::Param,
    value: f32,
    reading: String,
    choices: Option<Vec<String>>,
    menu: AssignMenu,
    tags: Vec<TagSpec>,
}

/// The controls of one model on a face.
struct Group {
    /// The cab's name over its controls, on an Amp+Cab.
    title: Option<String>,
    paired: bool,
    controls: Vec<Control>,
}

/// What a face was asked this frame.
#[derive(Default)]
struct FaceAsked {
    /// A control turned: its index, value and kind, and whether it is the
    /// cab's.
    edit: Option<(usize, f32, Kind, bool)>,
    assign: Option<(i64, AssignAction)>,
    bypass: Option<BypassAction>,
    /// A value being typed, or `Some(None)` when typing ended.
    draft: Option<Option<(i64, i64, String)>>,
}

/// How a face is fitted to its room.
pub(crate) struct Fit<'a> {
    pub width: f32,
    pub height: Option<f32>,
    pub sizes: &'a [f32],
    /// Drawn at its natural width rather than the whole `width`.
    pub natural: bool,
    pub fill: Color32,
}

/// The cab's controls are typed into under their own key, so a value typed
/// into the cab's first knob is not shown in the amp's as well.
fn draft_key(index: usize, paired: bool) -> i64 {
    index as i64 + if paired { 10_000 } else { 0 }
}

fn assign_popup(block: i64, index: usize, paired: bool) -> egui::Id {
    if paired {
        egui::Id::new(("assign-cab", block, index))
    } else {
        crate::popup_id(block, index)
    }
}

impl App {
    /// The colour a block's on/off lights: the footswitch carrying it, or the
    /// block's own.
    fn bypass_light(&self, block: &session::Block) -> Color32 {
        self.assignment(block.position, Target::Bypass)
            .and_then(|a| match a.source {
                Source::Footswitch(n) => self.switch_led(n),
                _ => None,
            })
            .unwrap_or_else(|| self.block_colour(block))
    }

    /// The tags under a control: what drives it.
    fn tags_for(&self, under: Option<Source>) -> Vec<TagSpec> {
        under
            .map(|source| {
                let led = match source {
                    Source::Footswitch(n) => self.switch_led(n),
                    _ => None,
                };
                vec![TagSpec::of(source, led)]
            })
            .unwrap_or_default()
    }

    /// The face's groups: the model's controls and, on an Amp+Cab, the cab's.
    fn face_groups(&self, block: &session::Block) -> Vec<Group> {
        let Some(catalog) = self.catalog.as_ref() else {
            return Vec::new();
        };
        let mut models = vec![(self.slot_model(block).cloned(), block.values.clone(), false)];
        if let Some(cab) = block.paired.and_then(|m| catalog.model_number(m)) {
            models.push((Some(cab.clone()), block.paired_values.clone(), true));
        }
        models
            .into_iter()
            .filter_map(|(model, values, paired)| {
                let model = model?;
                let params: Vec<hx_catalog::Param> = catalog
                    .ordered_params(&model)
                    .into_iter()
                    .cloned()
                    .collect();
                let controls = values
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| {
                        let param = params.get(index)?.clone();
                        // The cab's controls answer to the same indices as
                        // the amp's, as they always have here.
                        let menu = self.assign_view(
                            block.position,
                            Target::Param(index as i64),
                            param.name.clone(),
                        );
                        Some(Control {
                            index,
                            reading: catalog.format(&param, *value),
                            choices: catalog.choices(&param).map(<[String]>::to_vec),
                            tags: self.tags_for(menu.under),
                            menu,
                            param,
                            value: *value,
                        })
                    })
                    .collect();
                Some(Group {
                    title: paired.then(|| model.name.clone()),
                    paired,
                    controls,
                })
            })
            .collect()
    }

    /// The face: the block's controls on a card, the on/off switch owning
    /// the first column, the rest in balanced rows of the largest knobs that
    /// fit. Returns how wide it was drawn.
    pub(crate) fn face(&mut self, ui: &mut Ui, block: &session::Block, fit: &Fit) -> f32 {
        let position = block.position;
        if self.catalog.is_none() {
            theme::label(
                ui,
                "The knobs need HX Edit's data for their names and ranges.",
                theme::regular(theme::SECONDARY),
                theme::muted(),
            );
            return 0.0;
        }
        let effect = self.is_effect(block);
        let colour = self.block_colour(block);
        let groups = self.face_groups(block);
        let counts: Vec<usize> = groups.iter().map(|group| group.controls.len()).collect();
        let (knob, rows) = fit_face(&counts, effect, fit.width, fit.height, fit.sizes);
        let cell = knob + 26.0;
        let lead_width = if effect { cell + 17.0 } else { 0.0 };
        let widest = rows.iter().flatten().copied().max().unwrap_or(0);
        let natural = 22.0 + lead_width + widest as f32 * cell;
        let outer = if fit.natural {
            natural.min(fit.width)
        } else {
            fit.width
        };
        let body_height = face_height(knob, &rows) - 24.0 - ROW_GAP;

        let bypass: Option<BypassView> = effect.then(|| self.bypass_view(position));
        let light = self.bypass_light(block);
        let bypass_tags = bypass
            .as_ref()
            .map(|view| self.tags_for(view.menu.under))
            .unwrap_or_default();
        let draft = self.param_draft.clone();
        let mut asked = FaceAsked::default();

        let catalog = self.catalog.take();
        egui::Frame::new()
            .fill(fit.fill)
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
                    if let Some(view) = &bypass {
                        bypass_cell(ui, view, (knob, cell), light, &bypass_tags, &mut asked);
                        let (line, _) =
                            ui.allocate_exact_size(Vec2::new(17.0, body_height), Sense::hover());
                        ui.painter().vline(
                            line.center().x,
                            (line.top() + 4.0)..=(line.bottom() - 24.0).max(line.top() + 4.0),
                            Stroke::new(1.0, theme::line()),
                        );
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(0.0, ROW_GAP);
                        let mut first = true;
                        for (group, group_rows) in groups.iter().zip(&rows) {
                            if group.controls.is_empty() {
                                continue;
                            }
                            if !first {
                                group_heading(ui, group.title.as_deref(), widest as f32 * cell);
                            }
                            first = false;
                            let mut start = 0;
                            for count in group_rows {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                                    for control in &group.controls[start..start + count] {
                                        control_cell(
                                            ui,
                                            catalog.as_ref(),
                                            (position, group.paired),
                                            control,
                                            (knob, cell),
                                            colour,
                                            draft.as_ref(),
                                            &mut asked,
                                        );
                                    }
                                });
                                start += count;
                            }
                        }
                    });
                });
            });
        self.catalog = catalog;

        if let Some(update) = asked.draft {
            self.param_draft = update;
        }
        if let Some(action) = asked.bypass {
            self.bypass_action(position, action);
        }
        if let Some((param, chose)) = asked.assign {
            self.assign_action(position, Target::Param(param), chose);
        }
        if let Some((index, value, kind, paired)) = asked.edit {
            self.set_param(position, index, value, kind, paired);
        }
        outer
    }

    /// Turn one control: kept in our copy at once, and sent to the pedal.
    /// The cab's controls are addressed on the same block by the same index;
    /// only which half they belong to differs, and the pedal tells them apart.
    fn set_param(&mut self, position: i64, index: usize, value: f32, kind: Kind, paired: bool) {
        if let Some(slot) = self.chain.iter_mut().find(|b| b.position == position) {
            let values = if paired {
                &mut slot.paired_values
            } else {
                &mut slot.values
            };
            if let Some(held) = values.get_mut(index) {
                *held = value;
            }
        }
        self.edit(Cmd::SetParam {
            block: position,
            index: index as i64,
            value,
            kind,
        });
    }
}

/// The rule and the cab's name between an amp's controls and its cab's.
fn group_heading(ui: &mut Ui, title: Option<&str>, width: f32) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width.max(80.0), GROUP_GAP - ROW_GAP),
        Sense::hover(),
    );
    ui.painter().hline(
        (rect.left() + 8.0)..=(rect.right() - 8.0),
        rect.top() + 2.0,
        Stroke::new(1.0, theme::line()),
    );
    if let Some(title) = title {
        let caption = ui.painter().layout_job(theme::paint::spaced(
            &format!("CAB · {}", title.to_uppercase()),
            theme::semibold(theme::CAPTION),
            theme::muted(),
            0.07,
        ));
        let height = caption.size().y;
        ui.painter().galley(
            Pos2::new(rect.left() + 10.0, rect.bottom() - height),
            caption,
            Color32::PLACEHOLDER,
        );
    }
}

/// Paint one line of text centred on `centre`, no wider than `width`.
fn centred(
    ui: &Ui,
    text: impl Into<String>,
    font: egui::FontId,
    colour: Color32,
    centre: Pos2,
    width: f32,
) {
    let galley = shell::elided(ui, text, font, colour, width);
    let size = galley.size();
    ui.painter().galley(
        Pos2::new(centre.x - size.x / 2.0, centre.y - size.y / 2.0),
        galley,
        Color32::PLACEHOLDER,
    );
}

/// The block's on/off, drawn as the footswitch it is: the pedal's LED ring
/// around a stomp button, On or Off under it, and what drives it.
fn bypass_cell(
    ui: &mut Ui,
    view: &BypassView,
    (knob, cell): (f32, f32),
    light: Color32,
    tags: &[TagSpec],
    asked: &mut FaceAsked,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(cell, knob + CELL_TEXT), Sense::hover());
    let id = ui.id().with(("bypass", view.position));
    let switch = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.top() + knob / 2.0),
        Vec2::splat(knob),
    );
    let pressed = ui.interact(switch, id, Sense::click());
    theme::paint_footswitch(ui.painter(), switch, view.enabled, light, pressed.hovered());
    let pressed = pressed.on_hover_text(if view.enabled {
        "On. Click to turn it off, right-click to give it a control"
    } else {
        "Off. Click to turn it on, right-click to give it a control"
    });
    if pressed.clicked() {
        asked.bypass = Some(BypassAction::Toggle(!view.enabled));
    }
    let reading = Rect::from_min_size(
        Pos2::new(rect.left(), switch.bottom() + 3.0),
        Vec2::new(cell, 17.0),
    );
    centred(
        ui,
        if view.enabled { "On" } else { "Off" },
        theme::medium(theme::BODY),
        if view.enabled {
            theme::text()
        } else {
            theme::muted()
        },
        reading.center(),
        cell,
    );
    let label = Rect::from_min_size(reading.left_bottom(), Vec2::new(cell, 15.0));
    centred(
        ui,
        "On/Off",
        theme::regular(theme::LABEL),
        if view.menu.under.is_some() {
            theme::text_soft()
        } else {
            theme::muted()
        },
        label.center(),
        cell - 4.0,
    );
    let name = ui
        .interact(label, id.with("name"), Sense::click())
        .on_hover_text(match (view.menu.under, view.auto_engage) {
            (Some(source), true) => format!(
                "{} turns this on by itself when it moves. Click to change",
                source.label()
            ),
            (Some(source), false) => format!("{} switches this. Click to change", source.label()),
            (None, _) => "Click to put this under a footswitch or a CC".to_owned(),
        });
    let popup = crate::bypass_popup_id(view.position);
    if name.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    let strip = Rect::from_min_size(
        Pos2::new(rect.left(), label.bottom() + 4.0),
        Vec2::new(cell, 18.0),
    );
    let mut parts = vec![pressed, name.clone()];
    for tagged in tag_row(ui, strip, tags, ("bypass", view.position)) {
        if tagged.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
        }
        parts.push(tagged);
    }
    egui::Popup::from_response(&name)
        .id(popup)
        .open_memory(None)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            if let Some(chose) = assign_menu(ui, &view.menu) {
                asked.bypass = Some(BypassAction::Assign(chose));
            }
        });
    for part in &parts {
        part.context_menu(|ui| {
            if let Some(chose) = assign_menu(ui, &view.menu) {
                asked.bypass = Some(BypassAction::Assign(chose));
            }
        });
    }
}

/// One control: its knob, switch or menu, the reading under it (click to
/// type one), its name (click to give it a control) and what drives it.
#[allow(clippy::too_many_arguments)]
fn control_cell(
    ui: &mut Ui,
    catalog: Option<&hx_catalog::Catalog>,
    (position, paired): (i64, bool),
    control: &Control,
    (knob, cell): (f32, f32),
    colour: Color32,
    draft: Option<&(i64, i64, String)>,
    asked: &mut FaceAsked,
) {
    let param = &control.param;
    let key = draft_key(control.index, paired);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(cell, knob + CELL_TEXT), Sense::hover());
    let id = ui.id().with(("control", position, key));
    let face = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.top() + knob / 2.0),
        Vec2::splat(knob),
    );
    let mut current = control.value;
    let mut changed = false;
    let mut parts: Vec<Response> = Vec::new();
    let menu_like = param.kind == Kind::Enum && control.choices.is_some();
    match (param.kind, &control.choices) {
        (Kind::Switch, _) => {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("switch", key))
                    .max_rect(Rect::from_center_size(face.center(), Vec2::new(34.0, 20.0))),
            );
            let mut on = current >= 0.5;
            let hit = theme::toggle(&mut child, &mut on, colour);
            if hit.changed() {
                current = if on { 1.0 } else { 0.0 };
                changed = true;
            }
            parts.push(hit);
        }
        (Kind::Enum, Some(choices)) => {
            let width = (cell - 6.0).min(96.0);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("menu", key))
                    .max_rect(Rect::from_center_size(
                        face.center(),
                        Vec2::new(width, 26.0),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let button = theme::Button::new(&control.reading)
                .small()
                .trailing(Icon::ChevronDown)
                .min_width(width)
                .show(&mut child)
                .on_hover_text(format!("{}: choose", param.name));
            let chosen = current.round().max(0.0) as usize;
            egui::Popup::menu(&button).gap(4.0).show(|ui| {
                theme::menu_width(ui, 220.0);
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (n, label) in choices.iter().enumerate() {
                            let on = n == chosen;
                            if theme::menu_item(ui, on.then_some(Icon::Check), label, None)
                                .clicked()
                                && !on
                            {
                                current = n as f32;
                                changed = true;
                            }
                        }
                    });
            });
            parts.push(button);
        }
        _ => {
            let mut child =
                ui.new_child(egui::UiBuilder::new().id_salt(("knob", key)).max_rect(face));
            let mut value = current;
            let hit = theme::dial(
                &mut child,
                &mut value,
                param.min..=param.max,
                knob,
                colour,
                Some(param.default),
            );
            if hit.double_clicked() {
                current = param.default;
                changed = true;
            } else if hit.changed() {
                current = value;
                changed = true;
            }
            parts.push(hit.on_hover_text(
                "Drag to turn, Shift-drag for fine moves. Double-click for the default, \
                 right-click to give it a control",
            ));
        }
    }

    // The reading, which a click turns into a field for typing one.
    let reading = Rect::from_min_size(
        Pos2::new(rect.left(), face.bottom() + 3.0),
        Vec2::new(cell, 17.0),
    );
    let shown = |current: f32, changed: bool| match (changed, catalog) {
        (true, Some(catalog)) => catalog.format(param, current),
        (true, None) => format!("{current:.2}"),
        (false, _) => control.reading.clone(),
    };
    match draft.filter(|(block, index, _)| *block == position && *index == key) {
        Some((_, _, text)) => {
            let mut text = text.clone();
            let width = (cell - 8.0).min(72.0);
            let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("typing", key)).max_rect(
                Rect::from_center_size(reading.center(), Vec2::new(width, 20.0)),
            ));
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
                    if let Some(typed) =
                        catalog.and_then(|catalog| catalog.parse(param, text.trim()))
                    {
                        current = typed.clamp(param.min, param.max);
                        changed = true;
                    }
                }
                asked.draft = Some(None);
            } else {
                asked.draft = Some(Some((position, key, text)));
            }
        }
        None if !menu_like => {
            centred(
                ui,
                shown(current, changed),
                theme::medium(theme::BODY),
                theme::text(),
                reading.center(),
                cell,
            );
            let typable = param.kind == Kind::Continuous;
            let response = ui.interact(
                reading,
                id.with("reading"),
                if typable {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            if typable && response.clicked() {
                asked.draft = Some(Some((position, key, shown(current, changed))));
            }
            parts.push(if typable {
                response.on_hover_text("Click to type a value")
            } else {
                response
            });
        }
        None => {}
    }

    // The name, which opens the control's assignment menu.
    let label = Rect::from_min_size(reading.left_bottom(), Vec2::new(cell, 15.0));
    centred(
        ui,
        param.name.clone(),
        theme::regular(theme::LABEL),
        if control.menu.under.is_some() {
            theme::text_soft()
        } else {
            theme::muted()
        },
        label.center(),
        cell - 4.0,
    );
    let popup = assign_popup(position, control.index, paired);
    let name = ui
        .interact(label, id.with("name"), Sense::click())
        .on_hover_text(match control.menu.under {
            Some(source) => format!(
                "{} controls {}. Click to change",
                source.label(),
                param.name
            ),
            None => format!("{}. Click to put it under a pedal or a switch", param.name),
        });
    if name.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    let strip = Rect::from_min_size(
        Pos2::new(rect.left(), label.bottom() + 4.0),
        Vec2::new(cell, 18.0),
    );
    for tagged in tag_row(ui, strip, &control.tags, ("control", position, key)) {
        if tagged.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
        }
        parts.push(tagged);
    }
    let index = control.index as i64;
    egui::Popup::from_response(&name)
        .id(popup)
        .open_memory(None)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            if let Some(chose) = assign_menu(ui, &control.menu) {
                asked.assign = Some((index, chose));
            }
        });
    parts.push(name);
    for part in &parts {
        part.context_menu(|ui| {
            if let Some(chose) = assign_menu(ui, &control.menu) {
                asked.assign = Some((index, chose));
            }
        });
    }
    if changed {
        // Integer parameters drawn as knobs move in whole native steps.
        if param.kind == Kind::Enum {
            current = current.round();
        }
        asked.edit = Some((control.index, current, param.kind, paired));
    }
}

/// The one assignment menu, for a knob and for a block's on/off alike: what
/// drives it, and what each source already carries.
pub(crate) fn assign_menu(ui: &mut Ui, menu: &AssignMenu) -> Option<AssignAction> {
    theme::menu_width(ui, 260.0);
    theme::menu_header(ui, &format!("Control {} with", menu.name), None);
    let mut action = None;
    let none = menu.under.is_none();
    if theme::menu_item(ui, none.then_some(Icon::Check), "Nothing", None).clicked() && !none {
        action = Some(AssignAction::To(None));
    }
    for (source, carries) in &menu.sources {
        let on = menu.under == Some(*source);
        // What a source is busy with already, so a footswitch is not quietly
        // given a second job the night you find out about it.
        let hint = carries
            .first()
            .filter(|_| !on)
            .map(|what| format!("has {what}"));
        if theme::menu_item(
            ui,
            on.then_some(Icon::Check),
            &source.label(),
            hint.as_deref(),
        )
        .clicked()
        {
            action = Some(AssignAction::To((!on).then_some(*source)));
        }
    }
    action
}

// ---------------------------------------------------------------------------
// Heads and cards

/// A head over a lens: its title, a line under it, and controls at the
/// right, laid out right to left.
fn lens_head(ui: &mut Ui, title: &str, subtitle: &str, right: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 64.0), Sense::hover());
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 20.0, rect.top() + 14.0),
        Pos2::new(rect.right() - 16.0, rect.bottom() - 10.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .id_salt(("lens-head", title))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 12.0;
    right(&mut child);
    let room = (child.min_rect().left() - 16.0 - inner.left()).max(60.0);
    let heading = shell::title_galley(ui, title, theme::BLOCK_NAME, theme::text(), room);
    shell::paint_line(ui, heading, inner.left(), inner.top() + 10.5);
    let line = shell::elided(
        ui,
        subtitle,
        theme::regular(theme::SECONDARY),
        theme::muted(),
        room,
    );
    shell::paint_line(ui, line, inner.left(), inner.top() + 29.0);
}

/// A card's head: an icon, the title, a quieter aside, and room at the
/// right laid out right to left.
fn card_head(
    ui: &mut Ui,
    icon: Option<Icon>,
    title: &str,
    aside: &str,
    right: impl FnOnce(&mut Ui),
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::line()),
    );
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 16.0, rect.top()),
        Pos2::new(rect.right() - 14.0, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .id_salt(("card-head", title))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    right(&mut child);
    let limit = child.min_rect().left() - 10.0;
    let mut x = inner.left();
    let y = inner.center().y;
    if let Some(icon) = icon {
        theme::paint_icon(ui, icon, Pos2::new(x + 7.0, y), 14.0, theme::text_soft());
        x += 24.0;
    }
    let heading = shell::galley(ui, title, theme::semibold(13.5), theme::text());
    let w = heading.size().x;
    shell::paint_line(ui, heading, x, y);
    x += w + 10.0;
    if !aside.is_empty() && limit > x + 24.0 {
        let aside = shell::elided(ui, aside, theme::regular(12.0), theme::muted(), limit - x);
        shell::paint_line(ui, aside, x, y);
    }
}

/// A caption between the sections of a card.
fn section_caption(ui: &mut Ui, text: &str, rule: bool) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, if rule { 34.0 } else { 30.0 }),
        Sense::hover(),
    );
    if rule {
        ui.painter().hline(
            rect.x_range(),
            rect.top() + 4.0,
            Stroke::new(1.0, theme::line()),
        );
    }
    let galley = ui.painter().layout_job(theme::paint::spaced(
        &text.to_uppercase(),
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let height = galley.size().y;
    ui.painter().galley(
        Pos2::new(rect.left() + 14.0, rect.bottom() - 6.0 - height),
        galley,
        Color32::PLACEHOLDER,
    );
}

/// "LEAD" as a person would say it: "Lead".
fn sentence(text: &str) -> String {
    if text.chars().any(char::is_lowercase) {
        return text.to_owned();
    }
    shell::sentence_case(&text.to_lowercase())
}

/// Small numbers in words, as a sentence wants them.
fn number_word(n: usize) -> String {
    match n {
        2 => "Two".to_owned(),
        3 => "Three".to_owned(),
        4 => "Four".to_owned(),
        5 => "Five".to_owned(),
        6 => "Six".to_owned(),
        7 => "Seven".to_owned(),
        8 => "Eight".to_owned(),
        n => n.to_string(),
    }
}

/// An end of a travel being typed: still typing, or done (with what to set,
/// unless it was abandoned).
enum Typing {
    Edit(bool, String),
    Done(Option<(bool, String)>),
}

/// How a source's switch is drawn.
#[derive(Clone, Copy)]
pub(crate) enum SourceLook {
    Switch { led: Option<Color32>, on: bool },
    Pedal { colour: Color32 },
    Midi,
    Empty,
}

/// A source's switch as the pedal shows it: a stomp button in its LED ring
/// `ring` points out, a pedal, or a cable for MIDI. `rect` is the button.
pub(crate) fn paint_source(ui: &Ui, look: SourceLook, rect: Rect, ring: f32) {
    let painter = ui.painter();
    let centre = rect.center();
    let radius = rect.width() / 2.0;
    match look {
        SourceLook::Switch { led, on } => {
            let led = led.unwrap_or_else(theme::led_off);
            if on {
                painter.circle_filled(centre, radius + ring + 3.0, theme::alpha(led, 0.10));
                painter.circle_stroke(
                    centre,
                    radius + ring,
                    Stroke::new(5.0, theme::alpha(led, 0.16)),
                );
                painter.circle_stroke(centre, radius + ring, Stroke::new(2.0, led));
            } else {
                painter.circle_stroke(
                    centre,
                    radius + ring,
                    Stroke::new(2.0, theme::alpha(led, 0.35)),
                );
            }
            let p = theme::palette();
            theme::paint::radial_circle(
                painter,
                centre,
                radius,
                p.switch_top,
                p.switch_bottom,
                0.24,
            );
            painter.circle_stroke(centre, radius, Stroke::new(1.0, p.switch_edge));
        }
        SourceLook::Pedal { colour } => {
            painter.rect(
                rect,
                CornerRadius::same(8),
                theme::raised(),
                Stroke::new(1.0, theme::line_strong()),
                egui::StrokeKind::Inside,
            );
            if let Some(drawing) = theme::category_icon("Wah") {
                drawing.paint(
                    ui,
                    Rect::from_center_size(centre, Vec2::splat((rect.width() * 0.54).round())),
                    colour,
                );
            }
        }
        SourceLook::Midi => {
            painter.rect(
                rect,
                CornerRadius::same(8),
                theme::raised(),
                Stroke::new(1.0, theme::line_strong()),
                egui::StrokeKind::Inside,
            );
            theme::paint_icon(
                ui,
                Icon::Cable,
                centre,
                (rect.width() * 0.54).round(),
                theme::text_soft(),
            );
        }
        SourceLook::Empty => {
            theme::paint::dashed_circle(
                painter,
                centre,
                radius - 0.75,
                Stroke::new(1.5, theme::line_strong()),
                3.0,
                3.0,
            );
        }
    }
}

/// "Copy Chorus to…" and its menu of the other snapshots. Returns the one
/// chosen.
fn copy_snapshot_button(
    ui: &mut Ui,
    name: &str,
    others: &[(usize, String)],
    ghost: bool,
) -> Option<usize> {
    let label = format!("Copy {name} to…");
    let mut button = theme::Button::new(&label).small().icon(Icon::Copy);
    if ghost {
        button = button.ghost();
    }
    let button = button
        .enabled(!others.is_empty())
        .show(ui)
        .on_hover_text("Copy what this snapshot sets over another, keeping its name");
    let mut chosen = None;
    egui::Popup::menu(&button).gap(4.0).show(|ui| {
        theme::menu_width(ui, 220.0);
        theme::menu_header(ui, &format!("Copy {name} over"), None);
        for (index, other) in others {
            if theme::menu_item(ui, None, &format!("{} {other}", index + 1), None).clicked() {
                chosen = Some(*index);
            }
        }
    });
    chosen
}

/// A form row's key, in a fixed column.
fn form_key(ui: &mut Ui, text: &str) {
    let (place, _) = ui.allocate_exact_size(Vec2::new(84.0, 26.0), Sense::hover());
    let galley = shell::galley(ui, text, theme::regular(12.5), theme::muted());
    shell::paint_line(ui, galley, place.left(), place.center().y);
}

// ---------------------------------------------------------------------------
// The pane

/// One row of the controls card: what drives one thing on the block.
struct ControlRow {
    source: Source,
    target: Target,
    name: String,
    /// The two ends, read as the parameter reads.
    travel: Option<(String, String)>,
    /// The right-hand word: Toggles, Holds, Auto on, CC 4.
    aside: String,
    /// What it reads now, for a control the snapshots set.
    now: Option<String>,
}

/// One carried control as a chip or a row says it: the block, what it
/// drives, its travel, and the block's colour and category.
struct Carried {
    block: String,
    what: String,
    range: Option<String>,
    colour: Color32,
    category: String,
}

impl App {
    /// The pane under the board: the model browser when it is open, every
    /// lens at once on a large window, or the chosen lens.
    pub(crate) fn pane(&mut self, root: &mut Ui, tier: Tier) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                if self.browser.is_some() {
                    self.browser_lens(ui, tier);
                    return;
                }
                if self.chain.is_empty() {
                    self.empty_pane(ui);
                    return;
                }
                if tier == Tier::L {
                    self.large_pane(ui);
                    return;
                }
                match self.lens {
                    Lens::Block => self.block_lens(ui, tier),
                    Lens::Footswitches => self.footswitch_lens(ui, tier),
                    Lens::Snapshots => self.snapshot_lens(ui, tier),
                }
            });
    }

    /// Whether the floor shows: on a window that is not large, under the
    /// block, while there is a chain to drive.
    pub(crate) fn shows_floor(&self, tier: Tier) -> bool {
        tier != Tier::L
            && self.browser.is_none()
            && self.lens == Lens::Block
            && !self.chain.is_empty()
    }

    /// Nothing loaded: say what is coming.
    fn empty_pane(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let text = match self.connection {
            Connection::Online if self.loading => "Reading the preset from the pedal",
            Connection::Online => "The selected block's controls appear here",
            Connection::Connecting => "Looking for a pedal on USB",
            Connection::Offline => "Plug in a pedal to edit its presets",
        };
        centred(
            ui,
            text,
            theme::regular(theme::BODY),
            theme::muted(),
            Pos2::new(rect.center().x, rect.top() + 72.0),
            rect.width() - 40.0,
        );
    }

    /// The lens switch at the right of a pane's head.
    fn lens_switch(&mut self, ui: &mut Ui) {
        let lenses: Vec<Lens> = [Lens::Block, Lens::Footswitches, Lens::Snapshots]
            .into_iter()
            .filter(|lens| *lens != Lens::Snapshots || !self.snapshots.is_empty())
            .collect();
        let segments: Vec<theme::Segment> = lenses
            .iter()
            .map(|lens| theme::Segment::new(lens.label()))
            .collect();
        let chosen = lenses.iter().position(|lens| *lens == self.lens);
        if let Some(index) = theme::segmented(ui, "lens", &segments, chosen, false).clicked {
            self.lens = lenses[index];
        }
    }

    /// The selected block's head: its drawing in a well, its name (which
    /// opens the model browser), what kind of block it is, Change model, copy,
    /// paste and remove, and the lens switch.
    fn block_head(&mut self, ui: &mut Ui, block: &session::Block, lens: bool) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 64.0), Sense::hover());
        let inner = Rect::from_min_max(
            Pos2::new(rect.left() + 20.0, rect.top() + 14.0),
            Pos2::new(rect.right() - 16.0, rect.bottom() - 10.0),
        );
        let mut right = inner.right();
        if lens {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .id_salt("block-head-lens")
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            self.lens_switch(&mut child);
            right = child.min_rect().left() - 12.0;
        }
        let effect = self.is_effect(block);
        let colour = self.block_colour(block);
        let category = self
            .block_category(block)
            .unwrap_or_else(|| self.slot_kind_name(block));
        let mut x = inner.left();
        let y = inner.center().y;

        // The well, washed in the block's colour, with its drawing or, where
        // HX Edit's pictures are installed, the model's picture.
        let well = Rect::from_min_size(Pos2::new(x, y - 20.0), Vec2::splat(40.0));
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
        let drawing = match (effect, self.artwork(block)) {
            (true, Some(picture)) => Some((picture, Color32::WHITE, 34.0)),
            _ => self
                .drawing_of(block, &category)
                .map(|art| (art, colour, 22.0)),
        };
        if let Some((art, tint, size)) = drawing {
            art.paint(
                ui,
                Rect::from_center_size(well.center(), Vec2::splat(size)),
                tint,
            );
        }
        x = well.right() + 12.0;

        // The name, which opens the model browser, and what it is.
        let name = self.slot_label(block);
        let meta = self.block_meta(block, &category);
        let heading = shell::title_galley(
            ui,
            &name,
            theme::BLOCK_NAME,
            theme::text(),
            (right - x - 240.0).max(120.0),
        );
        let heading_width = heading.size().x;
        shell::paint_line(ui, heading, x, y - 9.0);
        let chevron = effect.then(|| {
            Rect::from_center_size(
                Pos2::new(x + heading_width + 6.0 + 7.0, y - 8.0),
                Vec2::splat(16.0),
            )
        });
        let name_rect = Rect::from_min_max(
            Pos2::new(x, y - 20.0),
            Pos2::new(chevron.map_or(x + heading_width, |c| c.right()), y + 1.0),
        );
        if let Some(chevron) = chevron {
            let response = ui
                .interact(name_rect, ui.id().with("block-name"), Sense::click())
                .on_hover_text("Choose another model for this block");
            theme::paint_icon(
                ui,
                Icon::ChevronDown,
                chevron.center(),
                14.0,
                if response.hovered() {
                    theme::text()
                } else {
                    theme::muted()
                },
            );
            if response.clicked() {
                self.open_browser_swap();
            }
        }
        let mut meta_x = x;
        for (index, (part, strong)) in meta.iter().enumerate() {
            if index > 0 {
                let dot = shell::galley(ui, "·", theme::regular(12.0), theme::muted());
                shell::paint_line(ui, dot, meta_x, y + 11.0);
                meta_x += 10.0;
            }
            let galley = shell::galley(
                ui,
                part.clone(),
                if *strong {
                    theme::semibold(12.0)
                } else {
                    theme::regular(12.0)
                },
                if *strong { colour } else { theme::muted() },
            );
            let w = galley.size().x;
            shell::paint_line(ui, galley, meta_x, y + 11.0);
            meta_x += w + 6.0;
        }
        x = name_rect.right().max(meta_x - 6.0) + 12.0;
        if !effect {
            return;
        }

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(x + 8.0, inner.top()),
                    Pos2::new(right.max(x + 8.0), inner.bottom()),
                ))
                .id_salt("block-head-actions")
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 12.0;
        if theme::Button::new("Change model")
            .small()
            .icon(Icon::LayoutGrid)
            .show(&mut child)
            .on_hover_text("Try other models in this block's place")
            .clicked()
        {
            self.open_browser_swap();
        }
        if theme::IconButton::new(Icon::Copy)
            .show(&mut child)
            .on_hover_text("Copy this block")
            .clicked()
        {
            self.copy_block(block.position as usize, block);
        }
        if theme::IconButton::new(Icon::Paste)
            .enabled(self.copied_block)
            .show(&mut child)
            .on_hover_text("Paste the copied block here")
            .on_disabled_hover_text("Nothing copied yet")
            .clicked()
        {
            self.paste_block(block.position as usize);
        }
        if theme::IconButton::new(Icon::Remove)
            .show(&mut child)
            .on_hover_text("Take this block out of the chain")
            .clicked()
        {
            self.edit(Cmd::ClearBlock(block.position));
        }
    }

    /// What to call a slot that is not an effect.
    fn slot_kind_name(&self, block: &session::Block) -> String {
        use hx_proto::preset::Kind as Slot;
        match block.kind {
            Slot::Input => "Input",
            Slot::Output => "Output",
            Slot::Split => "Split",
            Slot::Join => "Merge",
            _ => "",
        }
        .to_owned()
    }

    /// The drawing for a block: its category's, or an endpoint's or a
    /// junction's own.
    fn drawing_of(&self, block: &session::Block, category: &str) -> Option<theme::Art> {
        use hx_proto::preset::Kind as Slot;
        match block.kind {
            Slot::Input => theme::category_icon("Input"),
            Slot::Output => theme::category_icon("Output"),
            Slot::Split => theme::category_icon("Split"),
            Slot::Join => theme::category_icon("Merge"),
            _ => theme::category_icon(category),
        }
    }

    /// What a block is, after its name: its category in its colour, its
    /// shelf, and whether it is mono or stereo.
    fn block_meta(&self, block: &session::Block, category: &str) -> Vec<(String, bool)> {
        use hx_proto::preset::Kind as Slot;
        let mut parts = vec![(category.to_owned(), true)];
        match block.kind {
            Slot::Input | Slot::Output => {
                if let Some(routing) = self.routing_name(block) {
                    parts.push((routing, false));
                }
                return parts;
            }
            Slot::Split => {
                parts.push(("how the signal divides".to_owned(), false));
                return parts;
            }
            Slot::Join => {
                parts.push(("how the branches meet".to_owned(), false));
                return parts;
            }
            _ => {}
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return parts;
        };
        let Some(model) = catalog.model_number(block.model) else {
            return parts;
        };
        let shelf = catalog
            .category_of(&model.id)
            .and_then(|id| catalog.category(id))
            .and_then(|category| {
                category
                    .subcategories
                    .iter()
                    .find(|sub| sub.models.contains(&model.id))
                    .map(|sub| sub.name.clone())
            });
        let stereo = catalog
            .symbol(block.model)
            .is_some_and(|symbol| symbol.symbol.ends_with("Stereo"));
        if let Some(shelf) = shelf.filter(|shelf| shelf != "Mono" && shelf != "Stereo") {
            parts.push((shelf, false));
        }
        parts.push((if stereo { "Stereo" } else { "Mono" }.to_owned(), false));
        if let Some(cab) = block.paired.and_then(|m| catalog.model_number(m)) {
            parts.push((cab.name.clone(), false));
        }
        parts
    }

    /// The Block lens: the head, then the face with the controls card beside
    /// it, or the face alone on a small window.
    fn block_lens(&mut self, ui: &mut Ui, tier: Tier) {
        let Some(block) = self.chain.get(self.selected).cloned() else {
            self.empty_pane(ui);
            return;
        };
        let pane = ui.max_rect();
        self.block_head(ui, &block, true);
        let effect = self.is_effect(&block);
        let side = tier == Tier::M && effect;
        egui::ScrollArea::vertical()
            .id_salt("block-lens")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.x = 16.0;
                    let room = pane.width() - 40.0;
                    let column = ui.allocate_ui_with_layout(
                        Vec2::new(if side { room - 16.0 - 330.0 } else { room }, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            self.block_extras(ui, &block);
                            let height = pane.height() - 64.0 - if side { 16.0 } else { 12.0 };
                            self.face(
                                ui,
                                &block,
                                &Fit {
                                    width: ui.available_width(),
                                    height: Some(height),
                                    sizes: knob_sizes(tier),
                                    natural: side,
                                    fill: theme::panel(),
                                },
                            )
                        },
                    );
                    if side {
                        let width = (room - 16.0 - column.inner).max(240.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(width, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width);
                                self.controls_card(ui, &block, "Controls on this block");
                            },
                        );
                    }
                });
                ui.add_space(12.0);
            });
    }

    /// What a junction or an endpoint offers instead of a model to swap: how
    /// a split divides, where an endpoint is routed.
    fn block_extras(&mut self, ui: &mut Ui, block: &session::Block) {
        let position = block.position;
        let retype = self.split_type_chips(ui, block);
        let reroute = self.routing_choice(ui, block);
        if let Some(to) = reroute {
            self.edit(Cmd::SetRouting {
                block: position,
                to,
            });
        }
        if let Some(model) = retype {
            self.edit(Cmd::SetModel {
                block: position,
                model,
                paired: None,
            });
        }
    }

    /// How a split divides, as a segmented choice. Changing it is an ordinary
    /// model change on the split's slot: the branches survive it.
    fn split_type_chips(&self, ui: &mut Ui, block: &session::Block) -> Option<u32> {
        if block.kind != hx_proto::preset::Kind::Split {
            return None;
        }
        let catalog = self.catalog.as_ref()?;
        let current = catalog.model_number(block.model)?.id.clone();
        let family = catalog.models_in(catalog.category_of(&current)?);
        let names: Vec<String> = family
            .iter()
            .map(|model| {
                model
                    .name
                    .strip_prefix("Split ")
                    .unwrap_or(&model.name)
                    .to_owned()
            })
            .collect();
        let segments: Vec<theme::Segment> =
            names.iter().map(|name| theme::Segment::new(name)).collect();
        let chosen = family.iter().position(|model| model.id == current);
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            theme::caption(ui, "Type");
            let asked = theme::segmented(ui, "split-type", &segments, chosen, false);
            for (index, rect) in asked.rects.iter().enumerate() {
                ui.interact(*rect, ui.id().with(("split-hint", index)), Sense::hover())
                    .on_hover_text(crate::split_type_hint(&family[index].name));
            }
            if let Some(index) = asked.clicked.filter(|index| Some(*index) != chosen) {
                picked = crate::number_of(catalog, &family[index].id);
            }
        });
        picked
    }

    /// Where an input or output is routed, as a menu.
    fn routing_choice(&self, ui: &mut Ui, block: &session::Block) -> Option<i64> {
        let current = block.routing?;
        let model = self.slot_model(block)?;
        let catalog = self.catalog.as_ref()?;
        let param = model
            .params
            .iter()
            .find(|p| p.id == "@input" || p.id == "@output")?;
        let choices = catalog.choices(param)?;
        let showing = choices
            .get(current.max(0) as usize)
            .cloned()
            .unwrap_or_else(|| current.to_string());
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            theme::caption(ui, &param.name);
            let button = theme::Button::new(&showing)
                .small()
                .trailing(Icon::ChevronDown)
                .show(ui);
            egui::Popup::menu(&button).gap(4.0).show(|ui| {
                theme::menu_width(ui, 240.0);
                for (index, label) in choices.iter().enumerate() {
                    let on = index as i64 == current;
                    if theme::menu_item(ui, on.then_some(Icon::Check), label, None).clicked() && !on
                    {
                        chosen = Some(index as i64);
                    }
                }
            });
        });
        chosen
    }

    /// The rows of the controls card for a block.
    fn control_rows(&self, position: i64) -> Vec<ControlRow> {
        let block = self.chain.iter().find(|b| b.position == position);
        self.assignments_on(position)
            .into_iter()
            .map(|a| {
                let auto = Self::auto_engage(a.source, a.target);
                let read = |value: f32, param: &hx_catalog::Param| match self.catalog.as_ref() {
                    Some(catalog) => catalog.format(param, value),
                    None => format!("{value:.2}"),
                };
                let travel = self.travel_of(position, a.target);
                let now = match (a.source, a.target, &travel, block) {
                    (Source::Snapshots, Target::Param(index), Some(travel), Some(block)) => block
                        .values
                        .get(index as usize)
                        .map(|value| read(*value, &travel.param)),
                    _ => None,
                };
                let aside = match a.source {
                    Source::Footswitch(n) => if self.switch_holds(n) {
                        "Holds"
                    } else {
                        "Toggles"
                    }
                    .to_owned(),
                    Source::Expression(_) if auto => "Auto on".to_owned(),
                    Source::MidiCc => format!("CC {}", self.cc_of(&a)),
                    _ => String::new(),
                };
                ControlRow {
                    source: a.source,
                    target: a.target,
                    name: if auto {
                        "On/Off".to_owned()
                    } else {
                        self.target_name(position, a.target)
                    },
                    travel: travel
                        .filter(|_| a.source != Source::Snapshots)
                        .map(|travel| (read(a.min, &travel.param), read(a.max, &travel.param))),
                    aside,
                    now,
                }
            })
            .collect()
    }

    /// Whether footswitch `n` holds rather than toggles.
    fn switch_holds(&self, n: u8) -> bool {
        self.switches
            .iter()
            .find(|s| s.switch == n)
            .is_some_and(|s| s.momentary)
    }

    /// The CC an assignment answers to, the one being dragged to if it is.
    pub(crate) fn cc_of(&self, a: &Assignment) -> i64 {
        self.cc_drafts
            .get(&(a.block, a.target))
            .copied()
            .or(a.cc)
            .unwrap_or(crate::DEFAULT_CC)
    }

    /// "Controls on this block": every assignment on it, one row each, and
    /// how to add one. A row opens its source in the lens that edits it.
    fn controls_card(&mut self, ui: &mut Ui, block: &session::Block, title: &str) {
        let position = block.position;
        let rows = self.control_rows(position);
        let snapshot = self.snapshots.get(self.current_snapshot).cloned();
        let mut open = None;
        let mut take_off = None;
        let tags: Vec<TagSpec> = rows
            .iter()
            .map(|row| match row.source {
                Source::Snapshots => TagSpec {
                    text: "Snapshots".to_owned(),
                    mark: Some(theme::Mark::Icon(Icon::Camera)),
                },
                Source::Footswitch(n) => TagSpec::of(row.source, self.switch_led(n)),
                other => TagSpec::of(other, None),
            })
            .collect();
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            card_head(ui, Some(Icon::SlidersHorizontal), title, "", |_| {});
            for (row, tag) in rows.iter().zip(&tags) {
                let width = ui.available_width();
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::click());
                if response.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::ZERO,
                        theme::alpha(theme::hover(), 0.45),
                    );
                }
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    Stroke::new(1.0, theme::line_soft()),
                );
                let y = rect.center().y;
                let size = tag.tag().size(ui);
                let place =
                    Rect::from_min_size(Pos2::new(rect.left() + 16.0, y - size.y / 2.0), size);
                tag.tag().paint(ui, place);
                let mut x = place.right() + 10.0;
                let name =
                    shell::galley(ui, row.name.clone(), theme::semibold(12.5), theme::text());
                let w = name.size().x;
                shell::paint_line(ui, name, x, y);
                x += w + 10.0;
                // The right-hand side first, so the middle knows its room.
                let mut right = rect.right() - 14.0;
                if !row.aside.is_empty() {
                    let aside =
                        shell::galley(ui, row.aside.clone(), theme::regular(12.5), theme::muted());
                    right -= aside.size().x;
                    shell::paint_line(ui, aside, right, y);
                    right -= 12.0;
                }
                if let (Some(now), Some(name)) = (&row.now, &snapshot) {
                    let text = shell::galley(
                        ui,
                        format!("{name} {now}"),
                        theme::medium(11.5),
                        theme::text(),
                    );
                    let chip = Rect::from_min_size(
                        Pos2::new(right - text.size().x - 10.0, y - 9.0),
                        Vec2::new(text.size().x + 10.0, 18.0),
                    );
                    ui.painter().rect(
                        chip,
                        CornerRadius::same(4),
                        theme::hover(),
                        Stroke::new(1.0, theme::line_strong()),
                        egui::StrokeKind::Inside,
                    );
                    shell::paint_line(ui, text, chip.left() + 5.0, y);
                    right = chip.left() - 10.0;
                }
                match (&row.travel, row.source) {
                    (_, Source::Snapshots) if row.now.is_none() => {
                        let words = shell::elided(
                            ui,
                            "follows the snapshots",
                            theme::regular(12.5),
                            theme::muted(),
                            right - x,
                        );
                        shell::paint_line(ui, words, x, y);
                    }
                    (Some((low, high)), _) => {
                        let low =
                            shell::galley(ui, low.clone(), theme::regular(12.5), theme::muted());
                        let lw = low.size().x;
                        shell::paint_line(ui, low, x, y);
                        x += lw + 8.0;
                        theme::paint_icon(
                            ui,
                            Icon::ArrowRight,
                            Pos2::new(x + 6.0, y),
                            12.0,
                            theme::muted(),
                        );
                        x += 20.0;
                        let high =
                            shell::galley(ui, high.clone(), theme::medium(12.5), theme::text());
                        shell::paint_line(ui, high, x, y);
                    }
                    (None, _) if Self::auto_engage(row.source, row.target) => {
                        let words = shell::elided(
                            ui,
                            "turns on when the pedal leaves the heel",
                            theme::regular(12.5),
                            theme::muted(),
                            right - x,
                        );
                        shell::paint_line(ui, words, x, y);
                    }
                    _ => {}
                }
                let response = response.on_hover_text(match row.source {
                    Source::Snapshots => "Open the snapshots",
                    _ => "Open this in the footswitches",
                });
                if response.clicked() {
                    open = Some(row.source);
                }
                response.context_menu(|ui| {
                    theme::menu_width(ui, 230.0);
                    theme::menu_header(ui, &row.name, Some(&source_tag(row.source)));
                    if theme::menu_item(ui, Some(Icon::ArrowRight), "Open", None).clicked() {
                        open = Some(row.source);
                    }
                    theme::menu_separator(ui);
                    if theme::menu_danger(ui, Icon::Close, "Take the control off").clicked() {
                        take_off = Some(row.target);
                    }
                });
            }
            let width = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::hover());
            let y = rect.center().y;
            theme::paint_icon(
                ui,
                Icon::MousePointerClick,
                Pos2::new(rect.left() + 16.0 + 7.0, y),
                14.0,
                theme::muted(),
            );
            let hint = shell::elided(
                ui,
                "Right-click a knob to give it a control",
                theme::regular(12.5),
                theme::muted(),
                width - 60.0,
            );
            shell::paint_line(ui, hint, rect.left() + 16.0 + 24.0, y);
        });
        if let Some(source) = open {
            self.focus_source(source);
        }
        if let Some(target) = take_off {
            self.assign_action(position, target, AssignAction::To(None));
        }
    }

    /// Show a source in the lens that edits it: the snapshots in the matrix,
    /// anything else in the footswitches.
    pub(crate) fn focus_source(&mut self, source: Source) {
        match source {
            Source::Snapshots => self.lens = Lens::Snapshots,
            other => {
                self.focused_source = Some(other);
                self.lens = Lens::Footswitches;
            }
        }
    }

    /// The blocks the focused source reaches, and the colour to ring them in.
    pub(crate) fn lens_highlight(&self) -> Option<(Vec<i64>, Color32)> {
        if self.lens != Lens::Footswitches || self.browser.is_some() {
            return None;
        }
        let source = self.focused_source?;
        let blocks: Vec<i64> = self
            .carried_by(source)
            .into_iter()
            .map(|a| a.block)
            .collect();
        let colour = match source {
            Source::Footswitch(n) => self.switch_led(n).unwrap_or_else(theme::accent),
            _ => theme::accent(),
        };
        Some((blocks, theme::alpha(colour, 0.75)))
    }

    /// The board's header while a lens is about part of the chain: what the
    /// focused footswitch reaches, or which snapshot is on the pedal. Each
    /// part says whether it is strong and whether a dot goes before it.
    pub(crate) fn lens_header(&self) -> Option<Vec<(String, bool, bool)>> {
        match self.lens {
            Lens::Footswitches => {
                let source = self.focused_source?;
                let blocks: std::collections::BTreeSet<i64> = self
                    .carried_by(source)
                    .into_iter()
                    .map(|a| a.block)
                    .collect();
                let tag = source_tag(source);
                Some(match blocks.len() {
                    0 => vec![(format!("Nothing on {tag}"), false, false)],
                    1 => vec![
                        ("1".to_owned(), true, false),
                        (format!(" block on {tag}"), false, false),
                    ],
                    n => vec![
                        (n.to_string(), true, false),
                        (format!(" blocks on {tag}"), false, false),
                    ],
                })
            }
            Lens::Snapshots => {
                let name = self.snapshots.get(self.current_snapshot)?;
                Some(vec![
                    (
                        format!("Snapshot {}", self.current_snapshot + 1),
                        true,
                        false,
                    ),
                    (format!("{name} is on the pedal"), false, true),
                ])
            }
            Lens::Block => None,
        }
    }

    /// Every source the Footswitches lens lists, in the pedal's order.
    pub(crate) fn listed_sources(&self) -> Vec<Source> {
        let mut sources: Vec<Source> = (1..=self.switch_count()).map(Source::Footswitch).collect();
        sources.extend([Source::Expression(1), Source::Expression(2), Source::MidiCc]);
        sources
    }

    /// What a source carries: what it switches on and off first, then what
    /// it turns, each in the order the signal meets the blocks.
    pub(crate) fn carried_by(&self, source: Source) -> Vec<Assignment> {
        let order = self.signal_order();
        let mut carried: Vec<Assignment> = self
            .all_assignments()
            .into_iter()
            .filter(|a| a.source == source)
            .collect();
        carried.sort_by_key(|a| {
            (
                a.target != Target::Bypass,
                order
                    .iter()
                    .position(|slot| *slot as i64 == a.block)
                    .unwrap_or(usize::MAX),
            )
        });
        carried
    }

    /// The slots in the order the signal meets them: each path's input, the
    /// blocks before its split, each lane in turn, the blocks after, and its
    /// output. Anything the layout does not place follows in slot order.
    pub(crate) fn signal_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for path in &self.layout.paths {
            order.extend(path.input);
            order.extend(path.head.iter().copied());
            order.extend(path.split);
            for lane in &path.lanes {
                order.extend(lane.blocks.iter().copied());
            }
            order.extend(path.join);
            order.extend(path.tail.iter().copied());
            order.extend(path.output);
        }
        for block in &self.chain {
            let slot = block.position as usize;
            if !order.contains(&slot) {
                order.push(slot);
            }
        }
        order
    }

    /// A source's first line: "FS2 · green", "EXP 1 · heel to toe".
    fn source_title(&self, source: Source) -> String {
        match source {
            Source::Footswitch(n) => {
                let named = self
                    .switches
                    .iter()
                    .find(|s| s.switch == n)
                    .and_then(|s| s.colour)
                    .and_then(|index| {
                        self.catalog
                            .as_ref()
                            .and_then(|c| c.menu(hx_catalog::FOOTSWITCH_LED))
                            .and_then(|names| names.get(index.max(0) as usize).cloned())
                    });
                match named {
                    Some(name) => format!("FS{n} · {}", name.to_lowercase()),
                    None => format!("FS{n} · auto colour"),
                }
            }
            Source::Expression(n) if self.carried_by(source).is_empty() => format!("EXP {n}"),
            Source::Expression(n) => format!("EXP {n} · heel to toe"),
            Source::MidiCc => "MIDI".to_owned(),
            Source::Snapshots => "Snapshots".to_owned(),
        }
    }

    /// A source's second line: the name under the switch, the CC numbers, or
    /// what it carries.
    pub(crate) fn source_name(&self, source: Source, carried: &[Assignment]) -> String {
        if let Source::Footswitch(n) = source {
            if let Some(label) = self
                .switches
                .iter()
                .find(|s| s.switch == n)
                .and_then(|s| s.label.as_ref())
                .filter(|label| !label.trim().is_empty())
            {
                return sentence(label);
            }
        }
        if source == Source::MidiCc && !carried.is_empty() {
            let numbers: std::collections::BTreeSet<i64> =
                carried.iter().map(|a| self.cc_of(a)).collect();
            return numbers
                .iter()
                .map(|n| format!("CC {n}"))
                .collect::<Vec<_>>()
                .join(", ");
        }
        carried
            .first()
            .and_then(|a| self.chain.iter().find(|b| b.position == a.block))
            .map(|b| self.slot_label(b))
            .unwrap_or_else(|| "Not assigned".to_owned())
    }

    /// How a source acts, for its chip: Toggles, Holds, Auto on, CC.
    pub(crate) fn source_mode(&self, source: Source, carried: &[Assignment]) -> Option<String> {
        match source {
            Source::Footswitch(n) => (!carried.is_empty()).then(|| {
                if self.switch_holds(n) {
                    "Holds"
                } else {
                    "Toggles"
                }
                .to_owned()
            }),
            Source::Expression(_) => carried
                .iter()
                .any(|a| Self::auto_engage(a.source, a.target))
                .then(|| "Auto on".to_owned()),
            Source::MidiCc => (!carried.is_empty()).then(|| "CC".to_owned()),
            Source::Snapshots => None,
        }
    }

    /// How a source's switch is drawn.
    pub(crate) fn source_look(&self, source: Source, carried: &[Assignment]) -> SourceLook {
        if carried.is_empty() {
            return SourceLook::Empty;
        }
        match source {
            Source::Footswitch(n) => SourceLook::Switch {
                led: self.switch_led(n),
                on: self.switch_on(n, carried),
            },
            Source::Expression(_) => SourceLook::Pedal {
                colour: carried
                    .first()
                    .and_then(|a| self.chain.iter().find(|b| b.position == a.block))
                    .map(|b| self.block_colour(b))
                    .unwrap_or_else(|| theme::category_colour("Wah")),
            },
            _ => SourceLook::Midi,
        }
    }

    /// Whether a footswitch's light is up: what it switches is on.
    fn switch_on(&self, n: u8, carried: &[Assignment]) -> bool {
        if let Some(switch) = self.switches.iter().find(|s| s.switch == n) {
            if !switch.carries.is_empty() {
                return switch.carries.iter().any(|c| c.enabled);
            }
        }
        carried.iter().any(|a| {
            self.chain
                .iter()
                .find(|b| b.position == a.block)
                .is_some_and(|b| b.enabled)
        })
    }

    /// What one carried control says in a chip or a row.
    fn carried_words(&self, a: &Assignment) -> Option<Carried> {
        let block = self.chain.iter().find(|b| b.position == a.block)?;
        let what = if Self::auto_engage(a.source, a.target) {
            "On/Off".to_owned()
        } else {
            self.target_name(a.block, a.target)
        };
        let range = self.travel_of(a.block, a.target).map(|travel| {
            let read = |value: f32| match self.catalog.as_ref() {
                Some(catalog) => catalog.format(&travel.param, value),
                None => format!("{value:.2}"),
            };
            format!("{} → {}", read(a.min), read(a.max))
        });
        Some(Carried {
            block: self.slot_label(block),
            what,
            range,
            colour: self.block_colour(block),
            category: self.block_category(block).unwrap_or_default(),
        })
    }

    /// The Footswitches lens: every source as a row, and the focused one's
    /// editor beside them.
    fn footswitch_lens(&mut self, ui: &mut Ui, tier: Tier) {
        let pane = ui.max_rect();
        let device = if self.device.is_empty() {
            "The pedal".to_owned()
        } else {
            self.device.clone()
        };
        let subtitle = format!("{device} · footswitches, expression pedals and MIDI");
        lens_head(ui, "Footswitches", &subtitle, |ui| self.lens_switch(ui));
        let sources = self.listed_sources();
        if self.focused_source.is_none_or(|s| !sources.contains(&s)) {
            self.focused_source = sources.first().copied();
        }
        let room = pane.width() - 40.0;
        let beside = tier != Tier::S;
        let editor_width = if beside {
            386.0f32.min(room * 0.42)
        } else {
            room
        };
        let list_width = if beside {
            room - 16.0 - editor_width
        } else {
            room
        };
        egui::ScrollArea::vertical()
            .id_salt("footswitch-lens")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.allocate_ui_with_layout(
                        Vec2::new(list_width, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(list_width);
                            ui.spacing_mut().item_spacing.y = 6.0;
                            for source in &sources {
                                self.source_row(ui, *source);
                            }
                            if !beside {
                                ui.add_space(10.0);
                                self.source_editor(ui);
                            }
                        },
                    );
                    if beside {
                        ui.allocate_ui_with_layout(
                            Vec2::new(editor_width, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(editor_width);
                                self.source_editor(ui);
                            },
                        );
                    }
                });
                ui.add_space(12.0);
            });
    }

    /// One source as a row: its switch, its name, what it carries and how it
    /// acts. Click to edit it.
    fn source_row(&mut self, ui: &mut Ui, source: Source) {
        let carried = self.carried_by(source);
        let selected = self.focused_source == Some(source);
        let empty = carried.is_empty();
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::click());
        if empty && !selected {
            theme::paint::dashed_rect(
                ui.painter(),
                rect.shrink(0.5),
                10.5,
                Stroke::new(1.0, theme::line_strong()),
                4.0,
                3.0,
            );
        } else {
            if selected {
                ui.painter().rect_stroke(
                    rect.expand(1.5),
                    CornerRadius::same(13),
                    Stroke::new(3.0, theme::accent_soft()),
                    egui::StrokeKind::Outside,
                );
            }
            ui.painter().rect(
                rect,
                CornerRadius::same(11),
                if selected {
                    theme::mix(theme::panel(), theme::accent(), 0.07)
                } else if response.hovered() {
                    theme::raised()
                } else {
                    theme::panel()
                },
                Stroke::new(
                    1.0,
                    if selected {
                        theme::accent_line()
                    } else {
                        theme::line()
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
        let y = rect.center().y;
        let switch = Rect::from_center_size(
            Pos2::new(rect.left() + 10.0 + 5.0 + 12.0, y),
            Vec2::splat(24.0),
        );
        paint_source(ui, self.source_look(source, &carried), switch, 5.0);
        let left = switch.right() + 5.0 + 12.0;
        let first = shell::elided(
            ui,
            self.source_title(source),
            theme::semibold(10.5),
            theme::muted(),
            112.0,
        );
        shell::paint_line(ui, first, left, y - 8.0);
        let second = shell::elided(
            ui,
            self.source_name(source, &carried),
            theme::semibold(13.5),
            if empty { theme::faint() } else { theme::text() },
            112.0,
        );
        shell::paint_line(ui, second, left, y + 7.5);
        let mut x = left + 112.0 + 12.0;
        let mut limit = rect.right() - 12.0;
        if let Some(mode) = self.source_mode(source, &carried) {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        Pos2::new(x, rect.top()),
                        Pos2::new(limit, rect.bottom()),
                    ))
                    .id_salt(("source-mode", source.ordinal()))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            theme::Chip::new(&mode).show(&mut child);
            limit = child.min_rect().left() - 10.0;
        }
        if empty {
            let hint = match source {
                Source::Footswitch(n) => {
                    format!("Right-click a knob or an on/off and choose FS{n}")
                }
                Source::Expression(n) => format!("Right-click a knob and choose EXP {n}"),
                _ => "Right-click a knob and choose MIDI".to_owned(),
            };
            let hint = shell::elided(
                ui,
                hint,
                theme::regular(12.0),
                theme::faint(),
                (limit - x).max(20.0),
            );
            shell::paint_line(ui, hint, x, y);
        }
        let auto_and_more = carried.len() > 1;
        for a in &carried {
            if auto_and_more && Self::auto_engage(a.source, a.target) {
                continue;
            }
            let Some(words) = self.carried_words(a) else {
                continue;
            };
            if x > limit - 60.0 {
                break;
            }
            let name = shell::galley(ui, words.block, theme::medium(12.0), theme::text());
            let what = shell::galley(ui, words.what, theme::regular(12.0), theme::text_soft());
            let range = words
                .range
                .map(|range| shell::galley(ui, range, theme::regular(12.0), theme::text_soft()));
            let natural = 8.0
                + 13.0
                + 6.0
                + name.size().x
                + 5.0
                + what.size().x
                + range.as_ref().map_or(0.0, |r| r.size().x + 5.0)
                + 8.0;
            let chip = Rect::from_min_size(
                Pos2::new(x, y - 12.0),
                Vec2::new(natural.min(limit - x), 24.0),
            );
            ui.painter().rect(
                chip,
                CornerRadius::same(6),
                theme::bg(),
                Stroke::new(1.0, theme::line()),
                egui::StrokeKind::Inside,
            );
            if let Some(drawing) = theme::category_icon(&words.category) {
                drawing.paint(
                    ui,
                    Rect::from_center_size(
                        Pos2::new(chip.left() + 8.0 + 6.5, y),
                        Vec2::splat(13.0),
                    ),
                    words.colour,
                );
            }
            let painter = ui.painter().with_clip_rect(chip.shrink(1.0));
            let mut at = chip.left() + 8.0 + 13.0 + 6.0;
            for galley in [Some(name), Some(what), range].into_iter().flatten() {
                let width = galley.size().x;
                let top = y - galley.size().y / 2.0;
                painter.galley(Pos2::new(at, top), galley, Color32::PLACEHOLDER);
                at += width + 5.0;
            }
            x = chip.right() + 6.0;
        }
        if response.on_hover_text("Edit this").clicked() {
            self.focused_source = Some(source);
        }
    }

    /// The focused source's editor: a footswitch's name, light and press,
    /// and every control it carries with its two ends as that parameter's own
    /// knobs.
    fn source_editor(&mut self, ui: &mut Ui) {
        let Some(source) = self.focused_source else {
            return;
        };
        let carried = self.carried_by(source);
        let lit = match source {
            Source::Footswitch(n) => Some(self.switch_led(n)),
            _ => None,
        };
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let what = match source {
                Source::Footswitch(_) => "what your foot does",
                Source::Expression(_) => "what the pedal sweeps",
                Source::MidiCc => "what a controller sends",
                Source::Snapshots => "",
            };
            // The switch's light before its name, as on the pedal.
            let title = source_tag(source);
            let (head, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::hover());
            ui.painter().hline(
                head.x_range(),
                head.bottom() - 0.5,
                Stroke::new(1.0, theme::line()),
            );
            let mut x = head.left() + 16.0;
            let y = head.center().y;
            if let Some(led) = lit {
                theme::paint_led(ui.painter(), Pos2::new(x + 4.0, y), 4.0, led);
                x += 8.0 + 10.0;
            }
            let heading = shell::galley(ui, title, theme::semibold(13.5), theme::text());
            let w = heading.size().x;
            shell::paint_line(ui, heading, x, y);
            let aside = shell::galley(ui, what, theme::regular(12.0), theme::muted());
            shell::paint_line(ui, aside, x + w + 10.0, y);
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 14,
                    bottom: 14,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 12.0);
                    if let Source::Footswitch(n) = source {
                        self.switch_fields(ui, n);
                    }
                    ui.horizontal_top(|ui| {
                        form_key(ui, "Carries");
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            if carried.is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "Nothing yet. Right-click a knob, or a block's \
                                             on/off, and choose {}.",
                                            source_tag(source)
                                        ))
                                        .font(theme::regular(12.5))
                                        .color(theme::muted()),
                                    )
                                    .wrap(),
                                );
                            }
                            for a in &carried {
                                self.carried_row(ui, a);
                            }
                        });
                    });
                });
        });
    }

    /// A footswitch's own settings: the name on its scribble strip, the
    /// colour it lights, and whether it toggles or holds.
    fn switch_fields(&mut self, ui: &mut Ui, n: u8) {
        let Some(view) = self.switch_view(n) else {
            theme::label(
                ui,
                "The pedal has not said how this switch is set up.",
                theme::regular(12.5),
                theme::muted(),
            );
            return;
        };
        let colours = self
            .catalog
            .as_ref()
            .and_then(|c| c.menu(hx_catalog::FOOTSWITCH_LED))
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        let mut change = None;
        ui.horizontal(|ui| {
            form_key(ui, "Name");
            let mut text = view.label.clone();
            let width = ui.available_width();
            let field = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(width)
                    .margin(egui::Margin::symmetric(10, 6))
                    .font(theme::regular(theme::BODY))
                    .hint_text(view.carries.clone().unwrap_or_default()),
            );
            if !field.has_focus() {
                // What the field is for, at its far end.
                let hint = shell::galley(
                    ui,
                    "on the pedal's screen",
                    theme::regular(11.5),
                    theme::muted(),
                );
                let w = hint.size().x;
                shell::paint_line(
                    ui,
                    hint,
                    field.rect.right() - 10.0 - w,
                    field.rect.center().y,
                );
            }
            if field.changed() {
                change = Some(crate::SwitchChange::Typing(n, text.clone()));
            }
            if field.lost_focus() {
                let typed = text.trim();
                change = Some(crate::SwitchChange::Set {
                    switch: n,
                    edit: session::SwitchEdit::Label((!typed.is_empty()).then(|| typed.to_owned())),
                });
            }
            field.on_hover_text(
                "What the pedal writes under this switch. Left empty, it names what it carries",
            );
        });
        if !colours.is_empty() {
            ui.horizontal_top(|ui| {
                form_key(ui, "Light");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    let chosen = view.colour.unwrap_or(0);
                    for (index, name) in colours.iter().enumerate() {
                        let on = index as i64 == chosen;
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                        let centre = rect.center();
                        if index == 0 {
                            // Auto Color: every colour, since it takes the
                            // colour of what it carries.
                            let wheel = [
                                "Red",
                                "Light Orange",
                                "Yellow",
                                "Green",
                                "Turquoise",
                                "Blue",
                                "Violet",
                                "Pink",
                            ];
                            let step = std::f32::consts::TAU / wheel.len() as f32;
                            for (k, colour) in wheel.iter().enumerate() {
                                theme::paint::arc(
                                    ui.painter(),
                                    centre,
                                    5.5,
                                    k as f32 * step,
                                    step,
                                    Stroke::new(
                                        11.0,
                                        theme::led(colour).unwrap_or_else(theme::muted),
                                    ),
                                );
                            }
                        } else {
                            ui.painter()
                                .circle_filled(centre, 11.0, theme::led_swatch(name));
                        }
                        ui.painter().circle_stroke(
                            centre,
                            11.0,
                            Stroke::new(1.0, Color32::from_black_alpha(60)),
                        );
                        if on {
                            ui.painter().circle_stroke(
                                centre,
                                13.5,
                                Stroke::new(2.0, theme::text()),
                            );
                        }
                        if response.on_hover_text(name).clicked() && !on {
                            change = Some(crate::SwitchChange::Set {
                                switch: n,
                                edit: session::SwitchEdit::Colour(
                                    (index > 0).then_some(index as i64),
                                ),
                            });
                        }
                    }
                });
            });
        }
        ui.horizontal(|ui| {
            form_key(ui, "Press");
            let asked = theme::segmented(
                ui,
                "press",
                &[theme::Segment::new("Toggles"), theme::Segment::new("Holds")],
                Some(usize::from(view.momentary)),
                false,
            );
            if let Some(index) = asked.clicked {
                let momentary = index == 1;
                if momentary != view.momentary {
                    change = Some(crate::SwitchChange::Set {
                        switch: n,
                        edit: session::SwitchEdit::Momentary(momentary),
                    });
                }
            }
        });
        if let Some(change) = change {
            self.switch_action(change);
        }
    }

    /// One carried control in the editor: the block and what it drives, and
    /// for a parameter its two ends as that parameter's own knobs.
    fn carried_row(&mut self, ui: &mut Ui, a: &Assignment) {
        let Some(block) = self.chain.iter().find(|b| b.position == a.block).cloned() else {
            return;
        };
        let colour = self.block_colour(&block);
        let category = self.block_category(&block).unwrap_or_default();
        let what = if Self::auto_engage(a.source, a.target) {
            "turns on when the pedal moves".to_owned()
        } else {
            match a.target {
                Target::Bypass => "turns on and off".to_owned(),
                Target::Param(_) => self.target_name(a.block, a.target),
            }
        };
        let mut remove = false;
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            let (spot, _) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::hover());
            if let Some(drawing) = theme::category_icon(&category) {
                drawing.paint(
                    ui,
                    Rect::from_center_size(spot.center(), Vec2::splat(14.0)),
                    colour,
                );
            }
            theme::label(
                ui,
                &self.slot_label(&block),
                theme::semibold(12.5),
                theme::text(),
            );
            theme::label(ui, &what, theme::regular(12.5), theme::muted());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                remove = theme::IconButton::new(Icon::Close)
                    .small()
                    .show(ui)
                    .on_hover_text("Take this off")
                    .clicked();
            });
        });
        if let Some(travel) = self.travel_of(a.block, a.target) {
            let words = match a.source {
                Source::Footswitch(_) => ("switch off", "switch on"),
                Source::Expression(_) => ("heel", "toe"),
                Source::MidiCc => ("CC at 0", "CC at 127"),
                Source::Snapshots => ("lowest", "highest"),
            };
            let mut moved = None;
            let mut typing = None;
            let draft = self
                .travel_draft
                .clone()
                .filter(|(block, target, ..)| *block == a.block && *target == a.target);
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.spacing_mut().item_spacing.x = 10.0;
                for (high, value, word) in [(false, a.min, words.0), (true, a.max, words.1)] {
                    let mut current = value;
                    let hit =
                        theme::dial(ui, &mut current, travel.range.clone(), 28.0, colour, None)
                            .on_hover_text(App::end_meaning(a.source, high));
                    if hit.changed() {
                        moved = Some((high, current));
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let read = self.catalog.as_ref().map_or_else(
                            || format!("{current:.2}"),
                            |c| c.format(&travel.param, current),
                        );
                        // Click the reading to type an end, as under the knobs.
                        match draft.as_ref().filter(|(.., end, _)| *end == high) {
                            Some((.., text)) => {
                                let mut text = text.clone();
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut text)
                                        .desired_width(60.0)
                                        .margin(egui::Margin::symmetric(4, 1))
                                        .font(theme::semibold(12.5)),
                                );
                                if !field.has_focus() && !field.lost_focus() {
                                    field.request_focus();
                                }
                                typing = Some(if field.lost_focus() {
                                    let enter =
                                        ui.input(|input| input.key_pressed(egui::Key::Enter));
                                    Typing::Done(enter.then_some((high, text)))
                                } else {
                                    Typing::Edit(high, text)
                                });
                            }
                            None => {
                                let reading = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(&read)
                                                .font(theme::semibold(12.5))
                                                .color(theme::text()),
                                        )
                                        .selectable(false)
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text("Click to type it");
                                if reading.clicked() {
                                    typing = Some(Typing::Edit(high, read.clone()));
                                }
                            }
                        }
                        theme::label(ui, word, theme::regular(11.0), theme::muted());
                    });
                    ui.add_space(12.0);
                }
            });
            match typing {
                Some(Typing::Edit(high, text)) => {
                    self.travel_draft = Some((a.block, a.target, high, text));
                }
                Some(Typing::Done(typed)) => {
                    self.travel_draft = None;
                    let value = typed.and_then(|(high, text)| {
                        let value = match self.catalog.as_ref() {
                            Some(catalog) => catalog.parse(&travel.param, text.trim()),
                            None => text.trim().parse().ok(),
                        }?;
                        Some((
                            high,
                            value.clamp(*travel.range.start(), *travel.range.end()),
                        ))
                    });
                    if let Some((high, value)) = value {
                        moved = Some((high, value));
                    }
                }
                None => {}
            }
            if let Some((high, value)) = moved {
                self.move_travel(a.block, a.target, high, value, &travel.range);
            }
        }
        if a.source == Source::MidiCc {
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                theme::label(ui, "CC", theme::regular(12.5), theme::muted());
                let mut cc = self.cc_of(a);
                let field = ui
                    .add(egui::DragValue::new(&mut cc).range(0..=127))
                    .on_hover_text("Which MIDI CC drives this. Drag, or click to type");
                // Every step is kept so the field follows the pointer; only
                // the number it comes to rest on is sent, so a drag does not
                // order a document read per pixel.
                let settled = if field.changed() {
                    self.cc_drafts.insert((a.block, a.target), cc);
                    !field.dragged()
                } else {
                    field.drag_stopped()
                };
                if settled {
                    self.assign_action(a.block, a.target, AssignAction::Cc(cc));
                }
            });
        }
        if remove {
            self.assign_action(a.block, a.target, AssignAction::To(None));
        }
    }

    /// The Snapshots lens: which blocks each snapshot turns on, what follows
    /// them, and the tempo each keeps.
    fn snapshot_lens(&mut self, ui: &mut Ui, tier: Tier) {
        let pane = ui.max_rect();
        let count = self.snapshots.len();
        let subtitle = match count {
            0 => "This preset has no snapshots.".to_owned(),
            1 => "One version of this preset.".to_owned(),
            n => format!(
                "{} versions of this preset. Switching keeps everything else as it is.",
                number_word(n)
            ),
        };
        let current = self.snapshots.get(self.current_snapshot).cloned();
        let others: Vec<(usize, String)> = self
            .snapshots
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.current_snapshot)
            .map(|(index, name)| (index, name.clone()))
            .collect();
        let mut rename = false;
        let mut copy_to = None;
        lens_head(ui, "Snapshots", &subtitle, |ui| {
            self.lens_switch(ui);
            if let Some(name) = &current {
                rename = theme::Button::new("Rename")
                    .small()
                    .icon(Icon::TextCursorInput)
                    .show(ui)
                    .on_hover_text(format!("Rename {name}"))
                    .clicked();
                copy_to = copy_snapshot_button(ui, name, &others, false);
            }
        });
        if rename {
            self.snapshot_draft = current.map(|name| (self.current_snapshot, name));
        }
        if let Some(to) = copy_to {
            self.edit(Cmd::CopySnapshot {
                from: self.current_snapshot,
                to,
            });
        }
        let room = pane.width() - 40.0;
        let beside = tier != Tier::S;
        let left = if beside {
            ((room - 16.0) * 0.567).round()
        } else {
            room
        };
        egui::ScrollArea::vertical()
            .id_salt("snapshot-lens")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.allocate_ui_with_layout(
                        Vec2::new(left, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left);
                            ui.spacing_mut().item_spacing.y = 16.0;
                            theme::card().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                self.snapshot_matrix(ui, 92.0, false);
                            });
                            if !beside {
                                theme::card().show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                                    self.snapshot_values(ui, 68.0);
                                });
                            }
                        },
                    );
                    if beside {
                        let right = room - 16.0 - left;
                        ui.allocate_ui_with_layout(
                            Vec2::new(right, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(right);
                                theme::card().show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                                    self.snapshot_values(ui, 68.0);
                                });
                            },
                        );
                    }
                });
                ui.add_space(12.0);
            });
    }

    /// The matrix's head row: the snapshots' names, the one on the pedal
    /// lit. Returns a snapshot clicked.
    fn snapshot_head(&self, ui: &mut Ui, column: f32, salt: &str) -> Option<usize> {
        let width = ui.available_width();
        let label_width = width - column * self.snapshots.len() as f32;
        let (head, _) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::hover());
        let mut pick = None;
        for (index, name) in self.snapshots.iter().enumerate() {
            let cell = Rect::from_min_size(
                Pos2::new(
                    head.left() + label_width + index as f32 * column,
                    head.top(),
                ),
                Vec2::new(column, 34.0),
            );
            let on = index == self.current_snapshot;
            let response = ui
                .interact(
                    cell,
                    ui.id().with(("snapshot-head", salt, index)),
                    Sense::click(),
                )
                .on_hover_text(format!("Switch to {name}"));
            if on {
                ui.painter()
                    .rect_filled(cell, CornerRadius::ZERO, theme::hover());
            } else if response.hovered() {
                ui.painter().rect_filled(
                    cell,
                    CornerRadius::ZERO,
                    theme::alpha(theme::hover(), 0.5),
                );
            }
            let number = shell::galley(
                ui,
                format!("{}", index + 1),
                theme::semibold(12.0),
                if on { theme::muted() } else { theme::faint() },
            );
            let label = shell::elided(
                ui,
                name.clone(),
                theme::semibold(12.0),
                if on { theme::text() } else { theme::muted() },
                column - 24.0,
            );
            let number_width = number.size().x + 6.0;
            let total = number_width + label.size().x;
            let x = cell.center().x - total / 2.0;
            shell::paint_line(ui, number, x, cell.center().y);
            shell::paint_line(ui, label, x + number_width, cell.center().y);
            if response.clicked() && !on {
                pick = Some(index);
            }
        }
        pick
    }

    /// The highlight down the column of the snapshot on the pedal.
    fn snapshot_column(&self, ui: &Ui, row: Rect, column: f32) {
        let label_width = row.width() - column * self.snapshots.len() as f32;
        let x = row.left() + label_width + self.current_snapshot as f32 * column;
        ui.painter().rect_filled(
            Rect::from_min_max(Pos2::new(x, row.top()), Pos2::new(x + column, row.bottom())),
            CornerRadius::ZERO,
            theme::hover(),
        );
    }

    /// Which blocks each snapshot turns on: a row per block, a column per
    /// snapshot. With `values`, the controls the snapshots set follow.
    fn snapshot_matrix(&mut self, ui: &mut Ui, column: f32, values: bool) {
        if self.snapshots.is_empty() {
            let width = ui.available_width();
            let (row, _) = ui.allocate_exact_size(Vec2::new(width, 44.0), Sense::hover());
            let words = shell::galley(
                ui,
                "This preset has no snapshots.",
                theme::regular(12.5),
                theme::muted(),
            );
            shell::paint_line(ui, words, row.left() + 14.0, row.center().y);
            return;
        }
        let pick = self.snapshot_head(ui, column, "matrix");
        let width = ui.available_width();
        let label_width = width - column * self.snapshots.len() as f32;
        let (caption, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
        self.snapshot_column(ui, caption, column);
        let text = ui.painter().layout_job(theme::paint::spaced(
            "BLOCKS ON AND OFF",
            theme::semibold(theme::CAPTION),
            theme::muted(),
            0.07,
        ));
        let height = text.size().y;
        ui.painter().galley(
            Pos2::new(caption.left() + 14.0, caption.bottom() - 6.0 - height),
            text,
            Color32::PLACEHOLDER,
        );
        let blocks: Vec<session::Block> = self
            .signal_order()
            .into_iter()
            .filter_map(|slot| self.chain.iter().find(|b| b.position == slot as i64))
            .filter(|b| self.is_effect(b))
            .cloned()
            .collect();
        for block in &blocks {
            let (row, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
            self.snapshot_column(ui, row, column);
            let colour = self.block_colour(block);
            let category = self.block_category(block).unwrap_or_default();
            if let Some(drawing) = theme::category_icon(&category) {
                drawing.paint(
                    ui,
                    Rect::from_center_size(
                        Pos2::new(row.left() + 14.0 + 7.5, row.center().y),
                        Vec2::splat(15.0),
                    ),
                    colour,
                );
            }
            let name = shell::elided(
                ui,
                self.slot_label(block),
                theme::regular(12.5),
                theme::text_soft(),
                label_width - 50.0,
            );
            let name_width = name.size().x;
            shell::paint_line(ui, name, row.left() + 38.0, row.center().y);
            let note = self
                .assignments_on(block.position)
                .into_iter()
                .find(|a| Self::auto_engage(a.source, a.target))
                .map(|a| format!("{} engages it", source_tag(a.source)));
            if let Some(note) = note {
                let room = label_width - 38.0 - name_width - 9.0 - 12.0;
                if room > 40.0 {
                    let note = shell::elided(ui, note, theme::regular(12.5), theme::muted(), room);
                    shell::paint_line(
                        ui,
                        note,
                        row.left() + 38.0 + name_width + 9.0,
                        row.center().y,
                    );
                }
            }
            for index in 0..self.snapshots.len() {
                let centre = Pos2::new(
                    row.left() + label_width + (index as f32 + 0.5) * column,
                    row.center().y,
                );
                let on = if index == self.current_snapshot {
                    Some(block.enabled)
                } else {
                    self.snapshot_details.get(index).and_then(|snapshot| {
                        snapshot
                            .enabled
                            .get(block.position as usize)
                            .copied()
                            .flatten()
                    })
                };
                match on {
                    Some(true) => {
                        ui.painter()
                            .circle_filled(centre, 9.0, theme::alpha(colour, 0.22));
                        ui.painter().circle_filled(centre, 6.0, colour);
                    }
                    Some(false) => {
                        ui.painter()
                            .circle_stroke(centre, 5.25, Stroke::new(1.5, theme::faint()));
                    }
                    None => centred(
                        ui,
                        "-",
                        theme::regular(12.0),
                        theme::faint(),
                        centre,
                        column,
                    ),
                }
            }
        }
        if values {
            self.snapshot_value_rows(ui, column, "Values that change");
        }
        ui.add_space(8.0);
        if let Some(index) = pick {
            self.switch_snapshot(index);
        }
    }

    /// Put another snapshot on the pedal.
    fn switch_snapshot(&mut self, index: usize) {
        self.current_snapshot = index;
        self.send(Cmd::SelectSnapshot(index as i64));
    }

    /// The controls the snapshots set, with what each reads in the snapshot
    /// on the pedal. What they read in the others is not decoded yet.
    fn snapshot_value_rows(&mut self, ui: &mut Ui, column: f32, title: &str) {
        let width = ui.available_width();
        let label_width = width - column * self.snapshots.len() as f32;
        let following: Vec<(Carried, Option<String>)> = self
            .carried_by(Source::Snapshots)
            .into_iter()
            .filter_map(|a| {
                let block = self.chain.iter().find(|b| b.position == a.block)?;
                let now = match (a.target, self.travel_of(a.block, a.target)) {
                    (Target::Param(index), Some(travel)) => {
                        block.values.get(index as usize).map(|value| {
                            self.catalog.as_ref().map_or_else(
                                || format!("{value:.2}"),
                                |c| c.format(&travel.param, *value),
                            )
                        })
                    }
                    _ => None,
                };
                Some((self.carried_words(&a)?, now))
            })
            .collect();
        section_caption(ui, title, true);
        if following.is_empty() {
            let (row, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
            let words = shell::elided(
                ui,
                "No knob follows the snapshots",
                theme::regular(12.5),
                theme::muted(),
                width - 28.0,
            );
            shell::paint_line(ui, words, row.left() + 14.0, row.center().y);
            return;
        }
        for (words, now) in following {
            let (row, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
            self.snapshot_column(ui, row, column);
            if let Some(drawing) = theme::category_icon(&words.category) {
                drawing.paint(
                    ui,
                    Rect::from_center_size(
                        Pos2::new(row.left() + 14.0 + 7.5, row.center().y),
                        Vec2::splat(15.0),
                    ),
                    words.colour,
                );
            }
            let name = shell::elided(
                ui,
                words.block,
                theme::regular(12.5),
                theme::text_soft(),
                label_width - 60.0,
            );
            let name_width = name.size().x;
            shell::paint_line(ui, name, row.left() + 38.0, row.center().y);
            let param = shell::elided(
                ui,
                words.what,
                theme::regular(12.5),
                theme::muted(),
                (label_width - 38.0 - name_width - 21.0).max(20.0),
            );
            shell::paint_line(
                ui,
                param,
                row.left() + 38.0 + name_width + 9.0,
                row.center().y,
            );
            for index in 0..self.snapshots.len() {
                let centre = Pos2::new(
                    row.left() + label_width + (index as f32 + 0.5) * column,
                    row.center().y,
                );
                match (&now, index == self.current_snapshot) {
                    (Some(now), true) => centred(
                        ui,
                        now.clone(),
                        theme::medium(12.5),
                        theme::text(),
                        centre,
                        column - 6.0,
                    ),
                    _ => centred(
                        ui,
                        "-",
                        theme::regular(12.5),
                        theme::faint(),
                        centre,
                        column,
                    ),
                }
            }
        }
    }

    /// The right half of the Snapshots lens: what follows the snapshots, the
    /// tempo each keeps, and how a knob is put under them.
    fn snapshot_values(&mut self, ui: &mut Ui, column: f32) {
        if self.snapshots.is_empty() {
            return;
        }
        if let Some(index) = self.snapshot_head(ui, column, "values") {
            self.switch_snapshot(index);
        }
        self.snapshot_value_rows(ui, column, "Values each snapshot sets");
        let width = ui.available_width();
        let label_width = width - column * self.snapshots.len() as f32;
        section_caption(ui, "Tempo", true);
        let own = self
            .snapshot_details
            .iter()
            .any(|snapshot| snapshot.tempo.is_some());
        let (row, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
        self.snapshot_column(ui, row, column);
        theme::paint_icon(
            ui,
            Icon::Timer,
            Pos2::new(row.left() + 14.0 + 7.0, row.center().y),
            14.0,
            theme::text_soft(),
        );
        let words = shell::elided(
            ui,
            if own {
                "Each snapshot's own"
            } else {
                "Follows the preset"
            },
            theme::regular(12.5),
            theme::text_soft(),
            label_width - 50.0,
        );
        shell::paint_line(ui, words, row.left() + 38.0, row.center().y);
        for index in 0..self.snapshots.len() {
            let centre = Pos2::new(
                row.left() + label_width + (index as f32 + 0.5) * column,
                row.center().y,
            );
            let tempo = self
                .snapshot_details
                .get(index)
                .and_then(|snapshot| snapshot.tempo)
                .or(self.tempo);
            match tempo {
                Some(bpm) => centred(
                    ui,
                    format!("{bpm:.1}"),
                    theme::medium(12.5),
                    theme::text(),
                    centre,
                    column,
                ),
                None => centred(
                    ui,
                    "-",
                    theme::regular(12.5),
                    theme::faint(),
                    centre,
                    column,
                ),
            }
        }
        ui.add_space(8.0);
        let (rule, _) = ui.allocate_exact_size(Vec2::new(width, 9.0), Sense::hover());
        ui.painter().hline(
            (rule.left() + 14.0)..=(rule.right() - 14.0),
            rule.center().y,
            Stroke::new(1.0, theme::line()),
        );
        ui.horizontal_top(|ui| {
            ui.add_space(14.0);
            ui.spacing_mut().item_spacing.x = 8.0;
            let (spot, _) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::hover());
            theme::paint_icon(ui, Icon::Info, spot.center(), 14.0, theme::muted());
            let room = (ui.available_width() - 14.0).max(40.0);
            ui.allocate_ui(Vec2::new(room, 0.0), |ui| {
                ui.set_max_width(room);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "A knob marked with the camera changes only the snapshot you are \
                             on. Right-click any knob to let snapshots set it.",
                        )
                        .font(theme::regular(12.0))
                        .color(theme::muted()),
                    )
                    .wrap(),
                );
            });
        });
        ui.add_space(12.0);
    }

    /// On a large window every lens is on screen at once: the block with its
    /// controls, the footswitches as a board, the snapshot matrix, and what
    /// the library knows of the preset.
    fn large_pane(&mut self, ui: &mut Ui) {
        let Some(block) = self.chain.get(self.selected).cloned() else {
            self.empty_pane(ui);
            return;
        };
        let pane = ui.max_rect();
        let right_width = 600.0;
        let left_width = pane.width() - 40.0 - 16.0 - right_width;
        let effect = self.is_effect(&block);
        egui::ScrollArea::vertical()
            .id_salt("large-pane")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(16.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_width, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left_width);
                            ui.spacing_mut().item_spacing.y = 16.0;
                            theme::card().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                self.block_head(ui, &block, false);
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0);
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 10.0;
                                        self.block_extras(ui, &block);
                                        self.face(
                                            ui,
                                            &block,
                                            &Fit {
                                                width: left_width - 40.0,
                                                height: None,
                                                sizes: knob_sizes(Tier::L),
                                                natural: false,
                                                fill: theme::bg(),
                                            },
                                        );
                                    });
                                });
                                ui.add_space(16.0);
                            });
                            self.footswitch_board(ui);
                            if effect {
                                let title = format!("Controls on {}", self.slot_label(&block));
                                self.controls_card(ui, &block, &title);
                            }
                        },
                    );
                    ui.allocate_ui_with_layout(
                        Vec2::new(right_width, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(right_width);
                            ui.spacing_mut().item_spacing.y = 16.0;
                            let current = self.snapshots.get(self.current_snapshot).cloned();
                            let others: Vec<(usize, String)> = self
                                .snapshots
                                .iter()
                                .enumerate()
                                .filter(|(index, _)| *index != self.current_snapshot)
                                .map(|(index, name)| (index, name.clone()))
                                .collect();
                            theme::card().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                let mut copy_to = None;
                                card_head(
                                    ui,
                                    Some(Icon::Camera),
                                    "Snapshots",
                                    "what changes when you switch",
                                    |ui| {
                                        if let Some(name) = &current {
                                            copy_to = copy_snapshot_button(ui, name, &others, true);
                                        }
                                    },
                                );
                                ui.add_space(6.0);
                                self.snapshot_matrix(ui, 104.0, true);
                                if let Some(to) = copy_to {
                                    self.edit(Cmd::CopySnapshot {
                                        from: self.current_snapshot,
                                        to,
                                    });
                                }
                            });
                            self.library_card(ui);
                        },
                    );
                });
                ui.add_space(16.0);
            });
    }

    /// Every footswitch and pedal as a card side by side: the large window's
    /// footswitch board.
    fn footswitch_board(&mut self, ui: &mut Ui) {
        let sources: Vec<Source> = self
            .listed_sources()
            .into_iter()
            .filter(|source| *source != Source::MidiCc)
            .collect();
        let midi = self.carried_by(Source::MidiCc);
        let midi_words = midi.first().and_then(|a| {
            let words = self.carried_words(a)?;
            Some(format!(
                "MIDI CC {} · {} {}",
                self.cc_of(a),
                words.block,
                words.what
            ))
        });
        let device = self.device.clone();
        let mut open_midi = false;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            card_head(ui, None, "Footswitches", &device, |ui| {
                if let Some(words) = &midi_words {
                    open_midi = theme::Chip::new(words)
                        .mood(Mood::Ghost)
                        .icon(Icon::Cable)
                        .show(ui)
                        .on_hover_text("Edit what MIDI controls")
                        .clicked();
                }
            });
            egui::Frame::new()
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    let columns = sources.len().max(1);
                    let gap = 12.0;
                    let width = ((ui.available_width() - gap * (columns as f32 - 1.0))
                        / columns as f32)
                        .floor();
                    // One height for the row: the tallest card's.
                    let height = sources
                        .iter()
                        .map(|source| self.source_card_height(*source))
                        .fold(0.0, f32::max);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for source in &sources {
                            self.source_card(ui, *source, Vec2::new(width, height));
                        }
                    });
                });
        });
        if open_midi {
            self.focus_source(Source::MidiCc);
        }
    }

    /// One source as a card on the footswitch board.
    /// How tall a source's card is: its switch, up to two carried controls
    /// (or the hint for none), and its foot line.
    fn source_card_height(&self, source: Source) -> f32 {
        let carried = self.carried_by(source);
        let auto_and_more = carried.len() > 1;
        let rows = carried
            .iter()
            .filter(|a| !(auto_and_more && Self::auto_engage(a.source, a.target)))
            .count()
            .clamp(1, 2);
        let foot = if self.source_foot(source, &carried).is_some() {
            10.0 + 24.0
        } else {
            0.0
        };
        14.0 + 44.0 + 10.0 + rows as f32 * 22.0 + foot + 12.0
    }

    fn source_card(&mut self, ui: &mut Ui, source: Source, size: Vec2) {
        let carried = self.carried_by(source);
        let empty = carried.is_empty();
        let selected = self.lens == Lens::Footswitches && self.focused_source == Some(source);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        if selected {
            ui.painter().rect_stroke(
                rect.expand(1.5),
                CornerRadius::same(theme::RADIUS_CARD + 2),
                Stroke::new(3.0, theme::accent_soft()),
                egui::StrokeKind::Outside,
            );
        }
        if empty {
            theme::paint::dashed_rect(
                ui.painter(),
                rect.shrink(0.5),
                13.5,
                Stroke::new(1.0, theme::line_strong()),
                4.0,
                3.0,
            );
        } else {
            ui.painter().rect(
                rect,
                CornerRadius::same(theme::RADIUS_CARD),
                if response.hovered() {
                    theme::raised()
                } else {
                    theme::panel()
                },
                Stroke::new(
                    1.0,
                    if selected {
                        theme::accent_line()
                    } else {
                        theme::line()
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
        let switch = Rect::from_min_size(
            Pos2::new(rect.left() + 14.0 + 5.0, rect.top() + 14.0 + 5.0),
            Vec2::splat(34.0),
        );
        paint_source(ui, self.source_look(source, &carried), switch, 6.0);
        let left = switch.right() + 5.0 + 12.0;
        let mut right = rect.right() - 14.0;
        if let Some(mode) = self.source_mode(source, &carried) {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        Pos2::new(left, switch.top()),
                        Pos2::new(right, switch.bottom()),
                    ))
                    .id_salt(("card-mode", source.ordinal()))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            theme::Chip::new(&mode).show(&mut child);
            right = child.min_rect().left() - 8.0;
        }
        let first = shell::elided(
            ui,
            self.source_title(source),
            theme::semibold(11.0),
            theme::muted(),
            right - left,
        );
        shell::paint_line(ui, first, left, switch.center().y - 9.0);
        let second = shell::elided(
            ui,
            self.source_name(source, &carried),
            theme::semibold(15.0),
            if empty { theme::faint() } else { theme::text() },
            right - left,
        );
        shell::paint_line(ui, second, left, switch.center().y + 9.0);
        let mut y = switch.bottom() + 5.0 + 10.0 + 11.0;
        if empty {
            let hint = match source {
                Source::Footswitch(n) => format!("Right-click a knob and choose FS{n}"),
                Source::Expression(n) => format!("Right-click a knob and choose EXP {n}"),
                _ => "Right-click a knob and choose MIDI".to_owned(),
            };
            let hint = shell::elided(
                ui,
                hint,
                theme::regular(12.0),
                theme::faint(),
                rect.width() - 28.0,
            );
            shell::paint_line(ui, hint, rect.left() + 14.0, y);
        }
        let auto_and_more = carried.len() > 1;
        for a in carried
            .iter()
            .filter(|a| !(auto_and_more && Self::auto_engage(a.source, a.target)))
            .take(2)
        {
            let Some(words) = self.carried_words(a) else {
                continue;
            };
            if let Some(drawing) = theme::category_icon(&words.category) {
                drawing.paint(
                    ui,
                    Rect::from_center_size(
                        Pos2::new(rect.left() + 14.0 + 7.0, y),
                        Vec2::splat(14.0),
                    ),
                    words.colour,
                );
            }
            let mut limit = rect.right() - 14.0;
            if let Some(range) = words.range {
                let range = shell::galley(ui, range, theme::medium(12.0), theme::text());
                limit -= range.size().x;
                shell::paint_line(ui, range, limit, y);
                limit -= 8.0;
            }
            let x = rect.left() + 14.0 + 22.0;
            let name = shell::elided(
                ui,
                words.block,
                theme::regular(12.5),
                theme::text_soft(),
                (limit - x).max(20.0),
            );
            let name_width = name.size().x;
            shell::paint_line(ui, name, x, y);
            if limit - x - name_width > 30.0 {
                let what = shell::elided(
                    ui,
                    words.what,
                    theme::regular(12.5),
                    theme::muted(),
                    limit - x - name_width - 8.0,
                );
                shell::paint_line(ui, what, x + name_width + 8.0, y);
            }
            y += 22.0;
        }
        if let Some(foot) = self.source_foot(source, &carried) {
            let line_y = rect.bottom() - 12.0 - 24.0;
            ui.painter().hline(
                (rect.left() + 14.0)..=(rect.right() - 14.0),
                line_y,
                Stroke::new(1.0, theme::line_soft()),
            );
            let foot = shell::elided(
                ui,
                foot,
                theme::regular(11.5),
                theme::muted(),
                rect.width() - 28.0,
            );
            shell::paint_line(ui, foot, rect.left() + 14.0, line_y + 9.0 + 7.5);
        }
        if response.on_hover_text("Edit this").clicked() {
            if selected {
                self.lens = Lens::Block;
            } else {
                self.focus_source(source);
            }
        }
    }

    /// The line at the foot of a footswitch card: what the pedal shows.
    fn source_foot(&self, source: Source, carried: &[Assignment]) -> Option<String> {
        let first = carried
            .first()
            .and_then(|a| self.chain.iter().find(|b| b.position == a.block));
        match source {
            Source::Footswitch(n) => {
                let switch = self.switches.iter().find(|s| s.switch == n)?;
                match switch
                    .label
                    .as_ref()
                    .filter(|label| !label.trim().is_empty())
                {
                    Some(label) => Some(format!("Scribble strip reads {label}")),
                    None => first.map(|block| {
                        format!(
                            "Name left blank: the pedal shows {}",
                            self.slot_label(block)
                        )
                    }),
                }
            }
            Source::Expression(_) => carried
                .iter()
                .find(|a| Self::auto_engage(a.source, a.target))
                .and_then(|a| self.chain.iter().find(|b| b.position == a.block))
                .map(|block| {
                    format!(
                        "Turns {} on when the pedal leaves the heel",
                        self.slot_label(block)
                    )
                }),
            _ => None,
        }
    }

    /// What the library knows of the loaded preset, on a large window.
    fn library_card(&mut self, ui: &mut Ui) {
        let name = self.preset_name.clone();
        let entry = self
            .lib_entries
            .iter()
            .find(|entry| entry.name.trim().eq_ignore_ascii_case(name.trim()))
            .map(|entry| {
                (
                    entry.name.clone(),
                    entry.version,
                    entry.meta.clone(),
                    entry.downloads,
                )
            });
        let sync = self.slot_sync(self.preset_index);
        let mut keep = false;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let aside = entry
                .as_ref()
                .map(|(name, version, ..)| format!("{name} · v{version}"))
                .unwrap_or_else(|| "not kept yet".to_owned());
            card_head(ui, Some(Icon::Library), "In your library", &aside, |ui| {
                let label = match (&entry, sync) {
                    (Some((_, version, ..)), theme::Sync::Differs) => {
                        Some(format!("Keep as v{}", version + 1))
                    }
                    (None, _) => Some("Keep in library".to_owned()),
                    _ => None,
                };
                if let Some(label) = label {
                    keep = theme::Button::new(&label)
                        .small()
                        .icon(Icon::Computer)
                        .show(ui)
                        .on_hover_text("Keep the saved preset in your library")
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
                    let Some((_, _, meta, downloads)) = &entry else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "Keep this preset to give it a song, a part and tags, and \
                                     to find it again for any pedal.",
                                )
                                .font(theme::regular(12.5))
                                .color(theme::muted()),
                            )
                            .wrap(),
                        );
                        return;
                    };
                    let mut rows: Vec<(&str, String)> = vec![
                        ("Song", meta.song.clone()),
                        ("Artist", meta.artist.clone()),
                        ("Part", meta.part.clone()),
                        ("Guitar", meta.guitar.clone()),
                        ("Tags", meta.tags.join(", ")),
                    ];
                    if let Some(count) = downloads {
                        rows.push(("TonePush", format!("Published · {count} downloads")));
                    }
                    ui.spacing_mut().item_spacing.y = 9.0;
                    for (key, value) in rows {
                        if value.trim().is_empty() {
                            continue;
                        }
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
        if keep {
            let action = if entry.is_some() {
                crate::RowAction::Update
            } else {
                crate::RowAction::Keep
            };
            self.row_action(self.preset_index, action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_never_leaves_one_straggler() {
        // Twelve knobs that need two rows get six and six.
        assert_eq!(balanced_rows(12, 86.0, 559.0), vec![6, 6]);
        // Thirteen get seven and six, not twelve and one.
        assert_eq!(balanced_rows(13, 86.0, 1100.0), vec![7, 6]);
        // What fits on one row stays on one.
        assert_eq!(balanced_rows(5, 86.0, 600.0), vec![5]);
        assert!(balanced_rows(0, 86.0, 600.0).is_empty());
    }

    #[test]
    fn the_largest_knob_that_fits_is_chosen() {
        let sizes = knob_sizes(Tier::M);
        // The reference window: twelve knobs and the switch fill two rows of
        // 60 pt knobs in 662 by 296 points, as the design draws them.
        let (knob, rows) = fit_face(&[12], true, 662.0, Some(296.0), sizes);
        assert_eq!(knob, 60.0);
        assert_eq!(rows, vec![vec![6, 6]]);
        // Less height: a smaller knob, so the rows still fit.
        let (smaller, rows) = fit_face(&[12], true, 662.0, Some(270.0), sizes);
        assert_eq!(smaller, 52.0);
        assert!(face_height(smaller, &rows) <= 270.0);
        // No height to fit: the first size, on one row when it fits.
        assert_eq!(
            fit_face(&[3], false, 400.0, None, &[76.0]),
            (76.0, vec![vec![3]])
        );
    }

    #[test]
    fn an_amp_and_its_cab_share_one_knob_size() {
        let (knob, rows) = fit_face(&[12, 4], true, 662.0, Some(500.0), knob_sizes(Tier::M));
        assert_eq!(rows.len(), 2, "a set of rows each");
        assert!(face_height(knob, &rows) <= 500.0);
        assert_eq!(rows[1].iter().sum::<usize>(), 4);
    }

    #[test]
    fn sources_are_tagged_the_way_the_pedal_prints_them() {
        assert_eq!(source_tag(Source::Footswitch(2)), "FS2");
        assert_eq!(source_tag(Source::Expression(1)), "EXP 1");
        assert_eq!(source_tag(Source::MidiCc), "MIDI");
    }

    #[test]
    fn a_shouted_switch_name_reads_as_a_word() {
        assert_eq!(sentence("LEAD"), "Lead");
        assert_eq!(sentence("Solo boost"), "Solo boost");
    }
}
