//! The Library page's views (docs/design/redesign-2026-10-01, screens 05, 06
//! and 07): tones and the Cloud as a table beside an inspector, and setlists
//! as cards beside the chosen one, laid out bank by bank against what is on
//! the pedal now.

use std::collections::BTreeMap;

use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::shell;
use crate::theme::{self, Icon, Mood, Tier};
use crate::{library, App, CloudAction, LibraryView};

// ---------------------------------------------------------------------------
// Comparing a setlist with the pedal

/// How one slot of a setlist compares with what the pedal holds there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotState {
    /// The same preset on both.
    Same,
    /// The setlist holds another preset, or another version of this one.
    Replaced,
    /// The setlist holds a preset where the pedal's slot is empty.
    Fills,
    /// The setlist leaves the slot empty and the pedal has a preset there.
    Clears,
    /// Empty on both.
    Empty,
}

impl SlotState {
    /// Whether putting the setlist on the pedal changes this slot.
    pub(crate) fn differs(self) -> bool {
        matches!(
            self,
            SlotState::Replaced | SlotState::Fills | SlotState::Clears
        )
    }
}

/// What the pedal holds, slot by slot, as its backup says: the preset's hash
/// where there is one, and every slot's name, empty for an empty slot.
pub(crate) struct PedalSlots {
    pub hashes: BTreeMap<i64, String>,
    pub names: Vec<String>,
}

/// Compare a setlist's slots with the pedal's, slot by slot.
pub(crate) fn compare_slots(slots: &[library::Slot], pedal: &PedalSlots) -> Vec<SlotState> {
    let count = slots.len().max(pedal.names.len());
    (0..count)
        .map(|index| {
            let wanted = slots.get(index).filter(|slot| !slot.is_empty());
            let held = pedal
                .names
                .get(index)
                .is_some_and(|name| !shell::hx_slot_is_empty(name));
            match wanted {
                None if held => SlotState::Clears,
                None => SlotState::Empty,
                Some(_) if !held => SlotState::Fills,
                Some(slot) if pedal.hashes.get(&(index as i64)) == Some(&slot.hash) => {
                    SlotState::Same
                }
                Some(_) => SlotState::Replaced,
            }
        })
        .collect()
}

/// The counts a comparison comes to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Differences {
    pub replaced: usize,
    pub fills: usize,
    pub clears: usize,
    pub same: usize,
    pub empty: usize,
}

impl Differences {
    pub(crate) fn of(states: &[SlotState]) -> Differences {
        let mut counts = Differences::default();
        for state in states {
            match state {
                SlotState::Same => counts.same += 1,
                SlotState::Replaced => counts.replaced += 1,
                SlotState::Fills => counts.fills += 1,
                SlotState::Clears => counts.clears += 1,
                SlotState::Empty => counts.empty += 1,
            }
        }
        counts
    }

    pub(crate) fn differing(self) -> usize {
        self.replaced + self.fills + self.clears
    }
}

/// "6 slots differ from the pedal. Four hold other presets, two are empty in
/// the setlist. The other 120 match."
pub(crate) fn difference_words(counts: Differences) -> (String, String) {
    let differing = counts.differing();
    if differing == 0 {
        return (
            "Matches the pedal.".to_owned(),
            format!(
                "Every one of its {} slots holds what the pedal holds now.",
                counts.same + counts.empty
            ),
        );
    }
    let title = if differing == 1 {
        "1 slot differs from the pedal.".to_owned()
    } else {
        format!("{differing} slots differ from the pedal.")
    };
    let mut parts = Vec::new();
    let count = |n: usize, one: &str, many: &str| {
        let word = match n {
            1 => "one".to_owned(),
            2 => "two".to_owned(),
            3 => "three".to_owned(),
            4 => "four".to_owned(),
            5 => "five".to_owned(),
            6 => "six".to_owned(),
            n => n.to_string(),
        };
        format!("{word} {}", if n == 1 { one } else { many })
    };
    if counts.replaced > 0 {
        parts.push(count(
            counts.replaced,
            "holds another preset",
            "hold other presets",
        ));
    }
    if counts.fills > 0 {
        parts.push(count(
            counts.fills,
            "fills an empty slot",
            "fill empty slots",
        ));
    }
    if counts.clears > 0 {
        parts.push(count(
            counts.clears,
            "is empty in the setlist",
            "are empty in the setlist",
        ));
    }
    let mut body = parts.join(", ");
    if let Some(first) = body.get(..1) {
        body = first.to_uppercase() + &body[1..];
    }
    let rest = counts.same + counts.empty;
    if rest > 0 {
        body.push_str(&format!(". The other {rest} match."));
    } else {
        body.push('.');
    }
    (title, body)
}

// ---------------------------------------------------------------------------
// Small pieces

/// A caption between the sections of an inspector, with the hairline above.
pub(crate) fn section(ui: &mut Ui, caption: &str) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::hover());
    ui.painter().hline(
        (rect.left() - 16.0)..=(rect.right() + 16.0),
        rect.top() + 0.5,
        Stroke::new(1.0, theme::line()),
    );
    let galley = ui.painter().layout_job(theme::paint::spaced(
        &caption.to_uppercase(),
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let height = galley.size().y;
    ui.painter().galley(
        Pos2::new(rect.left(), rect.bottom() - 6.0 - height),
        galley,
        Color32::PLACEHOLDER,
    );
}

/// The key of an inspector row, in its column.
fn key(ui: &mut Ui, text: &str) {
    let (place, _) = ui.allocate_exact_size(Vec2::new(100.0, 24.0), Sense::hover());
    let galley = shell::galley(ui, text, theme::regular(theme::SECONDARY), theme::muted());
    shell::paint_line(ui, galley, place.left(), place.center().y);
}

/// A value typed in place: plain text until hovered or focused, then a field.
fn field(ui: &mut Ui, id: &str, value: &mut String, hint: &str, font: egui::FontId) -> Response {
    let width = ui.available_width();
    let response = ui.add(
        egui::TextEdit::singleline(value)
            .id_salt(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::symmetric(0, 3))
            .desired_width(width)
            .font(font)
            .text_color(theme::text())
            .hint_text(
                egui::RichText::new(hint)
                    .color(theme::faint())
                    .font(theme::regular(theme::SECONDARY)),
            ),
    );
    let rect = response.rect;
    if response.has_focus() {
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0, theme::accent_line()),
        );
    } else if response.hovered() {
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0, theme::line_strong()),
        );
    }
    response
}

/// A value typed in place, as wide as what it holds: for words that sit in
/// a sentence, such as a setlist's venue and date.
fn fitted_field(
    ui: &mut Ui,
    id: &str,
    value: &mut String,
    hint: &str,
    font: egui::FontId,
) -> Response {
    let shown = if value.is_empty() {
        hint
    } else {
        value.as_str()
    };
    let width = shell::galley(ui, shown.to_owned(), font.clone(), theme::text())
        .size()
        .x
        + 4.0;
    ui.allocate_ui(Vec2::new(width.max(40.0), 22.0), |ui| {
        field(ui, id, value, hint, font)
    })
    .inner
}

/// Give a field the keyboard with all it holds selected, so typing
/// replaces it: a rename.
fn select_all(ctx: &egui::Context, field: &Response, value: &str) {
    field.request_focus();
    let mut state = egui::TextEdit::load_state(ctx, field.id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(value.chars().count()),
        )));
    state.store(ctx, field.id);
}

/// An inspector row typed in place.
fn field_row(ui: &mut Ui, label: &str, value: &mut String, hint: &str) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        key(ui, label);
        field(ui, label, value, hint, theme::regular(theme::SECONDARY))
    })
    .inner
}

/// An inspector row that is one of a few words, chosen from a menu.
fn choice_row(ui: &mut Ui, label: &str, value: &mut String, options: &[&str]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        key(ui, label);
        let shown = if value.is_empty() {
            "Not set".to_owned()
        } else {
            shell::sentence_case(value)
        };
        let galley = shell::galley(
            ui,
            shown,
            theme::regular(theme::SECONDARY),
            if value.is_empty() {
                theme::faint()
            } else {
                theme::text()
            },
        );
        let width = galley.size().x + 20.0;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click());
        if response.hovered() {
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 2.0,
                Stroke::new(1.0, theme::line_strong()),
            );
        }
        shell::paint_line(ui, galley, rect.left(), rect.center().y);
        theme::paint_icon(
            ui,
            Icon::ChevronDown,
            Pos2::new(rect.right() - 7.0, rect.center().y),
            12.0,
            theme::muted(),
        );
        egui::Popup::menu(&response).gap(4.0).show(|ui| {
            theme::menu_width(ui, 180.0);
            if theme::menu_item(ui, value.is_empty().then_some(Icon::Check), "Not set", None)
                .clicked()
            {
                value.clear();
            }
            for option in options {
                let on = value.as_str() == *option;
                if theme::menu_item(
                    ui,
                    on.then_some(Icon::Check),
                    &shell::sentence_case(option),
                    None,
                )
                .clicked()
                {
                    *value = (*option).to_owned();
                }
            }
        });
    });
}

/// Five stars to rate with; returns the rating clicked. Clicking the rating
/// already given takes it away.
fn stars(ui: &mut Ui, rating: u8) -> Option<u8> {
    let mut chosen = None;
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(5.0 * 16.0, 24.0), Sense::click());
    let under = pointer
        .filter(|p| rect.contains(*p))
        .map(|p| (((p.x - rect.left()) / 16.0).floor() as u8 + 1).min(5));
    let shown = under.unwrap_or(rating);
    for star in 1..=5u8 {
        let centre = Pos2::new(
            rect.left() + 8.0 + f32::from(star - 1) * 16.0,
            rect.center().y,
        );
        let filled = star <= shown;
        theme::paint_icon(
            ui,
            if filled { Icon::StarOn } else { Icon::Star },
            centre,
            14.0,
            if filled {
                theme::accent()
            } else {
                theme::faint()
            },
        );
    }
    if response.clicked() {
        if let Some(star) = under {
            chosen = Some(if star == rating { 0 } else { star });
        }
    }
    chosen
}

/// A tone's chain as wells: each block's drawing on its category's tint.
fn chain_wells(ui: &mut Ui, chain: &[shell::Mini]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for item in chain {
            let lanes: Vec<(Color32, bool)> = match item {
                shell::Mini::Block { colour, on } => vec![(*colour, *on)],
                shell::Mini::Stack(lanes) => lanes.clone(),
            };
            let height = lanes.len() as f32 * 28.0 + lanes.len().saturating_sub(1) as f32 * 3.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, height), Sense::hover());
            for (index, (colour, on)) in lanes.iter().enumerate() {
                let well = Rect::from_min_size(
                    Pos2::new(rect.left(), rect.top() + index as f32 * 31.0),
                    Vec2::splat(28.0),
                );
                if *on {
                    ui.painter().rect(
                        well,
                        CornerRadius::same(7),
                        theme::mix(theme::panel(), *colour, 0.16),
                        Stroke::new(1.0, theme::alpha(*colour, 0.55)),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    theme::paint::dashed_rect(
                        ui.painter(),
                        well.shrink(0.5),
                        6.5,
                        Stroke::new(1.0, theme::faint()),
                        3.0,
                        2.5,
                    );
                }
                if let Some(drawing) = category_of_colour(*colour).and_then(theme::category_icon) {
                    drawing.paint(
                        ui,
                        Rect::from_center_size(well.center(), Vec2::splat(16.0)),
                        if *on { *colour } else { theme::faint() },
                    );
                }
            }
        }
    });
}

/// The category a colour stands for, by matching the palette: the strip
/// carries colours, and the wells want the drawing too.
fn category_of_colour(colour: Color32) -> Option<&'static str> {
    [
        "Distortion",
        "Dynamics",
        "EQ",
        "Modulation",
        "Delay",
        "Reverb",
        "Pitch/Synth",
        "Filter",
        "Wah",
        "Amp",
        "Preamp",
        "Cab",
        "IR",
        "Volume/Pan",
        "Send/Return",
        "Looper",
    ]
    .into_iter()
    .find(|name| theme::category_colour(name) == colour)
}

/// A tone's device marker, in the same two words as its row: solid when the
/// pedal connected plays it, dashed when it does not. Nothing when not even
/// the family is known.
pub(crate) fn marker(ui: &mut Ui, marker: Option<&crate::devices::Marker>, plays: bool) {
    if let Some(marker) = marker {
        theme::Marker::new(marker.family.label(), &marker.model)
            .solid(plays)
            .show(ui)
            .on_hover_text(format!("For {}", marker.with_article()));
    }
}

/// The voices of a where-row's words.
#[derive(Clone, Copy)]
pub(crate) enum Words {
    Soft,
    Bold,
    Hot,
    Faint,
    /// What plays on the pedal now, in amber.
    Accent,
}

/// A where-row's words as one line, each part in its voice.
pub(crate) fn where_job(parts: &[(&str, Words)], width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (text, voice) in parts {
        let (font, colour) = match voice {
            Words::Soft => (theme::regular(12.5), theme::text_soft()),
            Words::Bold => (theme::semibold(12.5), theme::text()),
            Words::Hot => (theme::regular(12.5), theme::hot()),
            Words::Faint => (theme::regular(12.5), theme::faint()),
            Words::Accent => (theme::medium(12.5), theme::accent()),
        };
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font,
                color: colour,
                ..Default::default()
            },
        );
    }
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(20.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

/// One version in a list: its number and date, what it is at the right,
/// and a click anywhere on it that plays it. The one playing says so, or
/// that it is on its way. Answers whether it was clicked.
pub(crate) fn version_row(
    ui: &mut Ui,
    number: &str,
    date: &str,
    playing: Option<bool>,
    right: impl FnOnce(&mut Ui),
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    if playing.is_some() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), theme::accent_soft());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), theme::raised());
    }
    let y = rect.center().y;
    let number = shell::galley(ui, number, theme::semibold(theme::SECONDARY), theme::text());
    let number_width = number.size().x;
    shell::paint_line(ui, number, rect.left() + 6.0, y);
    let date = shell::galley(ui, date, theme::regular(theme::SECONDARY), theme::muted());
    shell::paint_line(ui, date, rect.left() + 6.0 + number_width + 8.0, y);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(6.0, 0.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    match playing {
        Some(true) => {
            theme::Chip::new("Loading")
                .spinner()
                .mood(Mood::Accent)
                .outline(theme::accent_line())
                .show(&mut child);
        }
        Some(false) => {
            theme::Chip::new("Playing")
                .icon(Icon::Volume)
                .mood(Mood::Accent)
                .outline(theme::accent_line())
                .show(&mut child);
        }
        None => {
            right(&mut child);
            if response.hovered() {
                theme::label(
                    &mut child,
                    "click to hear it",
                    theme::regular(12.0),
                    theme::muted(),
                );
            }
        }
    }
    response.clicked()
}

/// One line of where a tone is (the design's inspector): an icon in its own
/// column, the words, and the next action there as a small button at the
/// right. Returns whether the action was pressed.
pub(crate) fn where_row(
    ui: &mut Ui,
    icon: Icon,
    ink: Color32,
    parts: &[(&str, Words)],
    action: Option<&str>,
) -> bool {
    where_line(ui, icon, ink, parts, action).0
}

/// The same, with the reason on the row's hover and no action.
pub(crate) fn where_row_hover(
    ui: &mut Ui,
    icon: Icon,
    ink: Color32,
    parts: &[(&str, Words)],
    hover: &str,
) {
    let (_, row) = where_line(ui, icon, ink, parts, None);
    if !hover.is_empty() {
        row.on_hover_text(hover);
    }
}

fn where_line(
    ui: &mut Ui,
    icon: Icon,
    ink: Color32,
    parts: &[(&str, Words)],
    action: Option<&str>,
) -> (bool, Response) {
    let width = ui.available_width();
    let (rect, row) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::hover());
    let mut pressed = false;
    let mut right = rect.right();
    if let Some(action) = action {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .id_salt(("where-action", action))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        pressed = theme::Button::new(action)
            .small()
            .show(&mut child)
            .clicked();
        right = child.min_rect().left() - 8.0;
    }
    theme::paint_icon(
        ui,
        icon,
        Pos2::new(rect.left() + 8.0, rect.center().y),
        14.0,
        ink,
    );
    let left = rect.left() + 16.0 + 9.0;
    let galley = ui.painter().layout_job(where_job(parts, right - left));
    shell::paint_line(ui, galley, left, rect.center().y);
    (pressed, row)
}

/// A hairline across the inspector, its padding included.
pub(crate) fn rule(ui: &mut Ui) {
    let width = ui.available_width();
    let (place, _) = ui.allocate_exact_size(Vec2::new(width, 9.0), Sense::hover());
    ui.painter().hline(
        (place.left() - 16.0)..=(place.right() + 16.0),
        place.center().y,
        Stroke::new(1.0, theme::line_soft()),
    );
}

/// A tag as a removable chip.
fn tag_chip(ui: &mut Ui, tag: &str) -> bool {
    let galley = shell::galley(ui, tag, theme::medium(12.0), theme::text_soft());
    let width = galley.size().x + 10.0 + 16.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click());
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::RADIUS_CHIP),
        theme::raised(),
        Stroke::new(1.0, theme::line()),
        egui::StrokeKind::Inside,
    );
    shell::paint_line(ui, galley, rect.left() + 7.0, rect.center().y);
    theme::paint_icon(
        ui,
        Icon::Close,
        Pos2::new(rect.right() - 9.0, rect.center().y),
        10.0,
        if response.hovered() {
            theme::text()
        } else {
            theme::faint()
        },
    );
    response.on_hover_text(format!("Take {tag} off")).clicked()
}

// ---------------------------------------------------------------------------
// The views

impl App {
    /// The inspector's width at each size of window.
    fn inspector_width(tier: Tier) -> f32 {
        tier.pick(280.0, 300.0, 400.0)
    }

    /// The details beside a table: the chosen tone, or with nothing chosen
    /// the loaded preset's. Scrolled, not grown: its fields must not decide
    /// the pane's height.
    fn inspector_contents(&mut self, ui: &mut Ui, cloud: bool) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("tone-inspector-scroll")
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 16,
                        top: 12,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if cloud {
                            self.cloud_tone_inspector(ui);
                        } else {
                            self.tone_inspector(ui);
                        }
                    });
            });
    }

    /// Tones and the Cloud as a table beside the chosen tone's details. On
    /// the smallest window the details open over the table from their button
    /// in the pane's header.
    pub(crate) fn library_body(&mut self, root: &mut Ui, tier: Tier) {
        let cloud = self.lib_showing == LibraryView::Cloud;
        if !cloud {
            self.follow_loaded_tone();
        }
        let body = root.max_rect();
        if tier != Tier::S {
            egui::Panel::right("tone-inspector")
                .resizable(true)
                .default_size(Self::inspector_width(tier))
                .size_range(260.0..=520.0)
                // The panel's own separator is the one line on its left.
                .frame(egui::Frame::new().fill(theme::bg()))
                .show(root, |ui| {
                    // As wide as the panel was made, whatever is in it: a
                    // side panel otherwise grows to the widest thing it holds.
                    let rect = ui.max_rect();
                    let mut inner = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    inner.set_clip_rect(rect.intersect(ui.clip_rect()));
                    self.inspector_contents(&mut inner, cloud);
                    ui.advance_cursor_after_rect(rect);
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                if cloud {
                    if let Some(why) = self.cloud_error.clone() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            theme::label(
                                ui,
                                &why,
                                theme::regular(theme::SECONDARY),
                                theme::danger(),
                            );
                            if theme::Button::new("Try again")
                                .ghost()
                                .small()
                                .show(ui)
                                .clicked()
                            {
                                self.refresh_cloud();
                            }
                        });
                        ui.add_space(4.0);
                    }
                    self.cloud_table(ui);
                } else {
                    self.library_table(ui);
                }
            });
        if tier == Tier::S && self.pane.details {
            let width = Self::inspector_width(tier);
            let rect = Rect::from_min_max(Pos2::new(body.right() - width, body.top()), body.max);
            egui::Area::new(egui::Id::new("tone-inspector-over"))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(root.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(theme::bg())
                        .stroke(Stroke::new(1.0, theme::line_strong()))
                        .shadow(ui.style().visuals.popup_shadow)
                        .show(ui, |ui| {
                            ui.set_min_size(rect.size());
                            ui.set_max_size(rect.size());
                            self.inspector_contents(ui, cloud);
                        });
                });
        }
    }

    /// With nothing chosen, the details are the loaded preset's tone's, the
    /// whole of them: chosen for it, and moved when another preset loads.
    fn follow_loaded_tone(&mut self) {
        if self.lib_selected.is_some() && !self.lib_follows {
            return;
        }
        let loaded = self.loaded_tone();
        if loaded == self.lib_selected {
            return;
        }
        match loaded {
            Some(index) => self.select_lib_entry(index),
            None => self.lib_selected = None,
        }
        self.lib_follows = true;
    }

    /// The tags to filter tones by, and the order of the Cloud's, as one
    /// menu beside the search. Called inside a right-to-left layout.
    pub(crate) fn filter_button(&mut self, ui: &mut Ui) {
        let cloud = self.lib_showing == LibraryView::Cloud;
        let tags: Vec<String> = if cloud {
            self.cloud_tags.clone()
        } else if self.library_device_filter.is_none() {
            self.library_lookup.tags.clone()
        } else {
            let mut tags = self
                .lib_entries
                .iter()
                .filter(|entry| self.tone_in_library_scope(entry))
                .flat_map(|entry| entry.meta.tags.iter().cloned())
                .collect::<Vec<_>>();
            tags.sort();
            tags.dedup();
            tags
        };
        let filter = if cloud {
            self.cloud_tag_filter.clone()
        } else {
            self.lib_tag_filter.clone()
        };
        if tags.is_empty() && filter.is_none() && !cloud {
            return;
        }
        let label = match &filter {
            Some(tag) => format!("# {tag}"),
            None if cloud => self.cloud_order.label().to_owned(),
            None => "All tags".to_owned(),
        };
        let button = crate::pages::choice_button(ui, &label).on_hover_text(if cloud {
            "Order TonePush's tones, or show only those with a tag"
        } else {
            "Show only the tones with a tag"
        });
        let mut chose_tag: Option<Option<String>> = None;
        let mut chose_order = None;
        egui::Popup::menu(&button)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .show(|ui| {
                theme::menu_width(ui, 220.0);
                if cloud {
                    theme::menu_header(ui, "Order", None);
                    for order in crate::cloud::DiscoveryOrder::ALL {
                        let on = order == self.cloud_order;
                        if theme::menu_item(ui, on.then_some(Icon::Check), order.label(), None)
                            .clicked()
                        {
                            chose_order = Some(order);
                        }
                    }
                    theme::menu_separator(ui);
                }
                theme::menu_header(ui, "Tags", None);
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if theme::menu_item(
                            ui,
                            filter.is_none().then_some(Icon::Check),
                            "All tones",
                            None,
                        )
                        .clicked()
                        {
                            chose_tag = Some(None);
                        }
                        for tag in &tags {
                            let on = filter.as_deref() == Some(tag.as_str());
                            if theme::menu_item(
                                ui,
                                on.then_some(Icon::Check),
                                &format!("# {tag}"),
                                None,
                            )
                            .clicked()
                            {
                                chose_tag = Some(Some(tag.clone()));
                            }
                        }
                    });
            });
        if let Some(tag) = chose_tag {
            if cloud {
                self.cloud_tag_filter = tag;
            } else {
                self.lib_tag_filter = tag;
            }
        }
        if let Some(order) = chose_order {
            if order != self.cloud_order {
                self.cloud_order = order;
                self.cloud_sort = (
                    match order {
                        crate::cloud::DiscoveryOrder::Popular => crate::LibColumn::Downloads,
                        crate::cloud::DiscoveryOrder::Newest => crate::LibColumn::Added,
                        crate::cloud::DiscoveryOrder::Updated => crate::LibColumn::Modified,
                        crate::cloud::DiscoveryOrder::Rating => crate::LibColumn::Rating,
                    },
                    false,
                );
                self.refresh_cloud();
            }
        }
    }

    /// The table's columns, as a menu on an icon. Called inside a
    /// right-to-left layout.
    pub(crate) fn columns_button(&mut self, ui: &mut Ui) {
        let button = theme::IconButton::new(Icon::List)
            .show(ui)
            .on_hover_text("Choose the table's columns");
        let choices = self.column_choices();
        let mut changed = None;
        egui::Popup::menu(&button)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .show(|ui| {
                if let Some(choice) = crate::table::column_menu(ui, &choices) {
                    changed = Some(choice);
                }
            });
        self.apply_column_visibility(changed);
    }

    /// The library tone that is the loaded preset: the one whose bytes the
    /// pedal's slot holds, or else the one of its name.
    pub(crate) fn loaded_tone(&self) -> Option<usize> {
        if !self.pedal_online() {
            return None;
        }
        if self.pro_active() {
            let (name, _) = self.pro.loaded(&self.library_lookup)?;
            return self.lib_entries.iter().position(|entry| {
                entry.pro && entry.name.trim().eq_ignore_ascii_case(name.trim())
            });
        }
        let held = self.mirror.get(&self.loaded_in_view()?);
        held.and_then(|hash| {
            self.lib_entries
                .iter()
                .position(|entry| &entry.hash == hash)
        })
        .or_else(|| {
            self.lib_entries.iter().position(|entry| {
                !entry.pro
                    && entry
                        .name
                        .trim()
                        .eq_ignore_ascii_case(self.preset_name.trim())
            })
        })
    }

    /// The slot on the pedal holding a tone's very bytes, when the pedal
    /// says.
    pub(crate) fn slot_holding(&self, hash: &str) -> Option<i64> {
        if self.pro_active() {
            let (hashes, _) = self.pro.pedal_slots()?;
            return hashes
                .iter()
                .find(|(_, held)| held.as_str() == hash)
                .map(|(slot, _)| *slot);
        }
        self.mirror
            .iter()
            .find(|(_, held)| held.as_str() == hash)
            .map(|(slot, _)| *slot)
    }

    /// The line under a tone's name: its marker, then its version, its
    /// character and when it was kept.
    fn marker_line(&self, ui: &mut Ui, entry: &crate::LibEntry, version: String) {
        let plays = self.fit(entry, false).plays;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            marker(ui, entry.marker.as_ref(), plays);
            let mut words = vec![version];
            if !entry.meta.character.trim().is_empty() {
                words.push(shell::sentence_case(entry.meta.character.trim()));
            }
            if !entry.added_at.is_empty() {
                words.push(format!("kept {}", crate::day_month(&entry.added_at)));
            }
            theme::label_truncated(ui, &words.join(" · "), theme::regular(12.0), theme::muted());
        });
    }

    /// Where a library tone is, a line for each place with what can be done
    /// there: the pedal (loaded, in a slot, not there yet, or not a pedal it
    /// plays), this computer, and TonePush.
    fn tone_where(&mut self, ui: &mut Ui, index: usize) {
        let entry = self.lib_entries[index].clone();
        let device = match self.device.trim() {
            "" => "pedal".to_owned(),
            device => device.to_owned(),
        };
        let loaded = self.loaded_tone() == Some(index);
        let dirty = if self.pro_active() {
            self.pro.is_dirty()
        } else {
            self.dirty
        };
        let loaded_slot = if self.pro_active() {
            self.pro.loaded_slot().map(|slot| slot as i64)
        } else {
            self.loaded_in_view()
        };
        // The pedal.
        let sending = self.sending.as_ref().is_some_and(|s| s.holds(&entry.hash));
        let heard = self.hears(&crate::audition::Source::Library(entry.hash.clone()));
        let set_aside = self.hearing.set_aside.clone().unwrap_or_default();
        if heard && !sending {
            let slot = set_aside.slot.clone();
            if self.heard_loading() {
                where_row(
                    ui,
                    Icon::LoaderCircle,
                    theme::accent(),
                    &[
                        ("On its way to ", Words::Soft),
                        (slot.as_str(), Words::Bold),
                    ],
                    None,
                );
            } else {
                let playing = format!("Playing in {slot}");
                let mut words = vec![(playing.as_str(), Words::Accent)];
                let place = format!(", in place of {}", set_aside.name);
                if !set_aside.name.is_empty() {
                    words.push((place.as_str(), Words::Soft));
                }
                where_row(ui, Icon::Volume, theme::accent(), &words, None);
            }
            // Already on the pedal somewhere else too: a way to it.
            if let Some(held) = self
                .slot_holding(&entry.hash)
                .filter(|held| Some(*held) != loaded_slot)
            {
                let label = self.active_slot_label(held);
                let go = format!("Go to {label}");
                if where_row(
                    ui,
                    Icon::Pedal,
                    theme::text_soft(),
                    &[
                        ("Already in ", Words::Soft),
                        (&label, Words::Bold),
                        (" on the pedal", Words::Soft),
                    ],
                    Some(&go),
                ) {
                    self.request_preset(held);
                }
            }
        } else if sending {
            if where_row(
                ui,
                Icon::Download,
                theme::accent(),
                &[("Choose a slot in the presets on the left", Words::Soft)],
                Some("Cancel"),
            ) {
                self.sending = None;
            }
        } else if !self.pedal_online() {
            let plays = entry.marker.as_ref().map_or_else(
                || "Plays on its own pedal".to_owned(),
                |marker| format!("Plays on {}", marker.with_article()),
            );
            where_row(
                ui,
                Icon::Pedal,
                theme::muted(),
                &[(&plays, Words::Soft)],
                None,
            );
        } else if let Some(why) =
            self.refusal_hint(entry.marker.as_ref(), &entry.firmware, entry.pro)
        {
            let plays = entry.marker.as_ref().map_or(why.clone(), |marker| {
                let mut words = format!("Plays on {}", marker.with_article());
                if marker.family == crate::devices::Family::Pro && !marker.model.is_empty() {
                    words = format!("{words} on {}", marker.model);
                }
                words
            });
            where_row_hover(
                ui,
                Icon::Ban,
                theme::muted(),
                &[(&plays, Words::Soft)],
                &why,
            );
        } else if loaded && self.hearing.heard.is_some() {
            // Set aside while another tone plays in its slot.
            let slot = set_aside.slot.clone();
            let playing = self
                .hearing
                .heard
                .as_ref()
                .map(|heard| heard.name.clone())
                .unwrap_or_default();
            let mut words = vec![
                ("Set aside from ", Words::Soft),
                (slot.as_str(), Words::Bold),
                (" while ", Words::Soft),
                (playing.as_str(), Words::Bold),
                (" plays", Words::Soft),
            ];
            if set_aside.dirty {
                words.push((" · ", Words::Faint));
                words.push(("changes kept", Words::Hot));
            }
            where_row(ui, Icon::Pedal, theme::text_soft(), &words, None);
        } else if loaded {
            let slot = loaded_slot
                .map(|slot| self.active_slot_label(slot))
                .unwrap_or_default();
            let mut words = vec![("Loaded in ", Words::Soft), (slot.as_str(), Words::Bold)];
            if dirty {
                words.push((" · ", Words::Faint));
                words.push(("changes not saved", Words::Hot));
            }
            where_row(ui, Icon::Pedal, theme::text_soft(), &words, None);
        } else if let Some(slot) = self.slot_holding(&entry.hash) {
            let label = self.active_slot_label(slot);
            let go = format!("Go to {label}");
            if where_row(
                ui,
                Icon::Pedal,
                theme::text_soft(),
                &[
                    (&format!("On the {device} at "), Words::Soft),
                    (&label, Words::Bold),
                ],
                Some(&go),
            ) {
                self.request_preset(slot);
            }
        } else {
            let words = match self.tone_sync(&entry.hash, &entry.name) {
                theme::Sync::Differs => format!("The {device} holds another version"),
                _ => format!("Not on the {device} yet"),
            };
            if where_row(
                ui,
                Icon::Pedal,
                theme::muted(),
                &[(&words, Words::Soft)],
                Some("Put in a slot…"),
            ) {
                self.start_sending(index);
            }
        }
        // This computer.
        let version = format!("v{}", entry.version);
        let keep_as = format!("Keep as v{}", entry.versions + 1);
        let offer_keep = loaded && dirty && !self.pro_active() && !self.hearing();
        if where_row(
            ui,
            Icon::Computer,
            theme::text_soft(),
            &if offer_keep {
                vec![
                    ("In your library as ", Words::Soft),
                    (&version, Words::Bold),
                ]
            } else {
                vec![("In your library · ", Words::Soft), (&version, Words::Bold)]
            },
            offer_keep.then_some(keep_as.as_str()),
        ) {
            self.keep_edit_as_version();
        }
        // TonePush.
        let publishing = self
            .publishing
            .as_ref()
            .is_some_and(|p| p.hash == entry.hash);
        // Published from here: which version TonePush gives, and whether
        // the library has moved on since.
        let on = (!publishing)
            .then(|| self.on_tonepush(&entry.hash))
            .flatten();
        if let Some((current, file)) = on
            .as_ref()
            .and_then(|on| Some((on.versions()?.0, on.details.as_ref()?.file_sha256.clone()?)))
        {
            let portable = self
                .portable_hashes
                .get(&entry.hash)
                .cloned()
                .or_else(|| library::portable_hash(&entry.hash));
            let version = format!("v{current}");
            if portable.as_deref() == Some(file.as_str()) {
                let open = where_row(
                    ui,
                    Icon::Cloud,
                    theme::text_soft(),
                    &[("On TonePush as ", Words::Soft), (&version, Words::Bold)],
                    Some("Open"),
                );
                if open {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab(crate::cloud::tone_url(&file)));
                }
            } else {
                let next = format!(
                    "Publish v{}…",
                    on.as_ref()
                        .and_then(|on| on.versions())
                        .map_or(current + 1, |(_, n)| n + 1)
                );
                let publish = where_row(
                    ui,
                    Icon::CloudUpload,
                    theme::hot(),
                    &[("TonePush has ", Words::Soft), (&version, Words::Hot)],
                    Some(&next),
                );
                if publish {
                    self.ask_to_publish(vec![entry.hash.clone()]);
                }
            }
            return;
        }
        match (publishing, self.cloud_sync(&entry.hash)) {
            (true, _) => {
                where_row(
                    ui,
                    Icon::Cloud,
                    theme::accent(),
                    &[("Publishing on TonePush…", Words::Soft)],
                    None,
                );
            }
            (false, theme::Sync::Same) => {
                let open = where_row(
                    ui,
                    Icon::Cloud,
                    theme::text_soft(),
                    &[("Published on TonePush", Words::Soft)],
                    Some("Open"),
                );
                if open {
                    self.publish_or_open(index, ui.ctx());
                }
            }
            (false, theme::Sync::Absent | theme::Sync::Differs) => {
                let publish = where_row(
                    ui,
                    Icon::Cloud,
                    theme::muted(),
                    &[("Not on TonePush", Words::Soft)],
                    Some("Publish…"),
                );
                if publish {
                    self.publish_or_open(index, ui.ctx());
                }
            }
            _ => {}
        }
    }

    /// The selected tone: its name, chain and whereabouts, its song and tone
    /// details typed in place, its versions, and publishing and export.
    fn tone_inspector(&mut self, ui: &mut Ui) {
        // Several chosen: what they are, and putting them in slots.
        if self.lib_chosen.len() > 1 {
            let names: Vec<String> = self
                .lib_order
                .iter()
                .filter_map(|&row| self.lib_entries.get(row))
                .filter(|entry| self.lib_chosen.contains(&entry.hash))
                .map(|entry| entry.name.clone())
                .collect();
            let first = self.lib_order.iter().copied().find(|&row| {
                self.lib_entries
                    .get(row)
                    .is_some_and(|entry| self.lib_chosen.contains(&entry.hash))
            });
            ui.spacing_mut().item_spacing.y = 4.0;
            let title = format!("{} tones chosen", self.lib_chosen.len());
            let galley = shell::title_galley(ui, &title, 15.0, theme::text(), ui.available_width());
            let (place, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            shell::paint_line(ui, galley, place.left(), place.center().y);
            theme::label_truncated(ui, &names.join(", "), theme::regular(12.0), theme::muted());
            ui.add_space(8.0);
            rule(ui);
            let refusal = self.put_refusal();
            let words = refusal.clone().map_or_else(
                || "In order, one slot each".to_owned(),
                |why| format!("Putting them in slots {why}"),
            );
            if where_row(
                ui,
                Icon::Pedal,
                theme::muted(),
                &[(&words, Words::Soft)],
                refusal.is_none().then_some("Put in slots…"),
            ) {
                if let Some(first) = first {
                    self.start_sending(first);
                }
            }
            return;
        }
        let Some(i) = self.lib_selected.filter(|i| *i < self.lib_entries.len()) else {
            // The loaded preset, not kept yet: the one thing to do with it.
            if let Some(index) = self
                .loaded_in_view()
                .filter(|_| self.pedal_online() && !self.pro_active())
            {
                let slot = self.active_slot_label(index);
                let name = self.preset_name.clone();
                ui.spacing_mut().item_spacing.y = 4.0;
                let title =
                    shell::title_galley(ui, &name, 15.0, theme::text(), ui.available_width());
                let (place, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
                shell::paint_line(ui, title, place.left(), place.center().y);
                ui.add_space(8.0);
                rule(ui);
                let words = format!("Loaded in {slot}, not in your library yet");
                if where_row(
                    ui,
                    Icon::Computer,
                    theme::muted(),
                    &[(&words, Words::Soft)],
                    Some("Keep in library"),
                ) {
                    self.row_action(index, crate::RowAction::Keep);
                }
                return;
            }
            ui.add_space(24.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Select a tone to see where it is and type in its song and tone.",
                    )
                    .font(theme::regular(theme::SECONDARY))
                    .color(theme::muted()),
                )
                .wrap(),
            );
            return;
        };
        let hash = self.lib_entries[i].hash.clone();
        ui.spacing_mut().item_spacing.y = 4.0;

        // The name is editable here. It is a label rather than an identity,
        // so changing it moves nothing and breaks no setlist; what it must
        // stay is unique.
        let free = library::name_is_free(&self.lib_draft.name, &hash);
        let name = field(
            ui,
            "tone-name",
            &mut self.lib_draft.name,
            "Name this tone",
            theme::semibold(theme::BLOCK_NAME),
        );
        if !free {
            theme::label(
                ui,
                "Another tone has that name",
                theme::regular(12.0),
                theme::hot(),
            );
        }
        if name.lost_focus() && !free {
            self.lib_draft.name = self.lib_entries[i].meta.name.clone();
        }
        let entry = self.lib_entries[i].clone();
        let version = if entry.versions > 1 {
            format!("v{} of {}", entry.version, entry.versions)
        } else {
            "v1".to_owned()
        };
        self.marker_line(ui, &entry, version);
        if !entry.chain.is_empty() && theme::Tier::now(ui.ctx()) == Tier::L {
            ui.add_space(8.0);
            chain_wells(ui, &entry.chain);
        }
        ui.add_space(8.0);
        rule(ui);
        // Where it is: the pedal, this computer, TonePush, each with what
        // can be done about it.
        self.tone_where(ui, i);
        ui.add_space(4.0);

        // What it is.
        section(ui, "Song and tone");
        field_row(ui, "Song", &mut self.lib_draft.song, "Original");
        field_row(ui, "Artist", &mut self.lib_draft.artist, "Nobody yet");
        field_row(ui, "Part", &mut self.lib_draft.part, "Rhythm, lead…");
        choice_row(
            ui,
            "Character",
            &mut self.lib_draft.character,
            &["clean", "drive", "hi-gain", "fuzz", "other"],
        );
        field_row(ui, "Genres", &mut self.lib_genres_buf, "Rock, blues…");
        field_row(ui, "Guitar", &mut self.lib_draft.guitar, "Telecaster…");
        choice_row(
            ui,
            "Pickups",
            &mut self.lib_draft.pickup_type,
            &["single-coil", "humbucker", "P90"],
        );
        choice_row(
            ui,
            "Electronics",
            &mut self.lib_draft.pickup_electronics,
            &["passive", "active"],
        );
        field_row(ui, "Tuning", &mut self.lib_draft.tuning, "E standard…");
        field_row(ui, "Gain", &mut self.lib_draft.gain, "1 to 10");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            key(ui, "Rating");
            if let Some(rating) = stars(ui, self.lib_draft.rating) {
                self.lib_draft.rating = rating;
            }
        });
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            key(ui, "Tags");
            ui.vertical(|ui| {
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
                    for (index, tag) in self.lib_draft.tags.iter().enumerate() {
                        if tag_chip(ui, tag) {
                            remove = Some(index);
                        }
                    }
                });
                if let Some(index) = remove {
                    self.lib_draft.tags.remove(index);
                }
                let added = field(
                    ui,
                    "add-tag",
                    &mut self.lib_tag_add,
                    "Add a tag",
                    theme::regular(theme::SECONDARY),
                );
                let submit =
                    added.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if submit && !self.lib_tag_add.trim().is_empty() {
                    let tag = self.lib_tag_add.trim().to_owned();
                    if !self.lib_draft.tags.contains(&tag) {
                        self.lib_draft.tags.push(tag);
                    }
                    self.lib_tag_add.clear();
                }
            });
        });
        self.lib_draft.genres = self
            .lib_genres_buf
            .split(',')
            .map(|genre| genre.trim().to_owned())
            .filter(|genre| !genre.is_empty())
            .collect();
        ui.add_space(10.0);
        section(ui, "Notes");
        for (label, value) in [
            ("The song", &mut self.lib_draft.description),
            ("This tone", &mut self.lib_draft.tone_description),
        ] {
            theme::label(ui, label, theme::regular(12.0), theme::muted());
            ui.add(
                egui::TextEdit::multiline(value)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .font(theme::regular(theme::SECONDARY))
                    .hint_text(
                        egui::RichText::new("Anything worth remembering").color(theme::faint()),
                    ),
            );
            ui.add_space(4.0);
        }

        // Persist as the fields change; one tone's metadata, the rest of the
        // index untouched. A name that is not free is the one thing not
        // written: it would put two tones under one name for as long as it
        // took to finish typing the rest of it.
        if self.lib_draft != self.lib_entries[i].meta && free {
            let saved = match library::save_meta(&hash, &self.lib_draft) {
                Ok(saved) => saved,
                Err(error) => {
                    self.note(error);
                    return;
                }
            };
            self.lib_draft = saved.clone();
            self.lib_entries[i].meta = saved;
            self.lib_entries[i].added_at = self.lib_entries[i].meta.added_at.clone();
            self.lib_entries[i].modified_at = self.lib_entries[i].meta.modified_at.clone();
            self.lib_entries[i].rating = (self.lib_entries[i].meta.rating > 0)
                .then_some(f32::from(self.lib_entries[i].meta.rating));
            // The table reads this rather than the metadata, so a rename shows
            // in the row as it is typed.
            if !self.lib_draft.name.is_empty() {
                self.lib_entries[i].name = self.lib_draft.name.clone();
            }
            self.library_lookup.reindex(&self.lib_entries);
        }

        // Its versions, when it has more than one.
        let history = library::versions_of(&hash);
        if history.len() > 1 {
            ui.add_space(10.0);
            section(ui, "Versions");
            let mut restore = None;
            let mut play = None;
            let loading = self.heard_loading();
            for version in history.iter().rev() {
                let playing = self.hears(&crate::audition::Source::Library(version.hash.clone()));
                let current = version.hash == hash;
                let clicked = version_row(
                    ui,
                    &format!("v{}", version.version),
                    &crate::day_month(&version.meta.modified_at),
                    playing.then_some(loading),
                    |ui| {
                        if current {
                            theme::Chip::new("Current").mood(Mood::Ok).show(ui);
                        } else if theme::Button::new("Make current")
                            .ghost()
                            .small()
                            .show(ui)
                            .clicked()
                        {
                            restore = Some(version.hash.clone());
                        }
                    },
                );
                if clicked {
                    play = Some((version.hash.clone(), version.version, current));
                }
            }
            if let Some((version, number, current)) = play {
                if current {
                    self.audition_library(i);
                } else {
                    self.audition_version(i, version, number);
                }
            }
            if let Some(version) = restore {
                match library::make_current(&version) {
                    Ok(()) => {
                        self.refresh_library();
                        if let Some(row) = self
                            .lib_entries
                            .iter()
                            .position(|entry| entry.hash == version)
                        {
                            self.select_lib_entry(row);
                        }
                        self.note("restored an earlier tone version".to_owned());
                    }
                    Err(why) => self.note(why),
                }
                return;
            }
        }

        // Out of this computer: publishing and exporting.
        ui.add_space(14.0);
        let width = ui.available_width();
        let (rule, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
        ui.painter().hline(
            (rule.left() - 16.0)..=(rule.right() + 16.0),
            rule.center().y,
            Stroke::new(1.0, theme::line()),
        );
        ui.add_space(12.0);
        let published = self.cloud_sync(&hash) == theme::Sync::Same;
        let publishing = self.publishing.as_ref().is_some_and(|p| p.hash == hash);
        let mut publish = false;
        let mut export = false;
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
            publish = theme::Button::new(if published {
                "Open on TonePush"
            } else if publishing {
                "Publishing…"
            } else {
                "Publish"
            })
            .small()
            .icon(Icon::Cloud)
            .enabled(!publishing)
            .show(ui)
            .on_hover_text(if published {
                "This tone is on TonePush: open it there"
            } else {
                "Publish its Song and this Tone on TonePush"
            })
            .clicked();
            export = theme::Button::new("Export for the web")
                .ghost()
                .small()
                .icon(Icon::FileDown)
                .show(ui)
                .on_hover_text(
                    "Write this tone's publishable preset with its details alongside, ready \
                     to upload",
                )
                .clicked();
        });
        ui.add_space(8.0);
        // Removal drops the file into the library's trash: recoverable, so
        // no confirmation ceremony.
        if theme::Button::new("Remove from library")
            .ghost()
            .small()
            .icon(Icon::Remove)
            .show(ui)
            .on_hover_text("It goes to the library's trash, and any setlist playing it keeps it")
            .clicked()
        {
            remove = true;
        }
        if publish {
            self.publish_or_open(i, ui.ctx());
        }
        if export {
            self.export_for_the_web(i);
        }
        if remove {
            let name = self.lib_entries[i].name.clone();
            match library::forget(&hash) {
                Ok(()) => self.note(format!("removed {name}")),
                Err(why) => self.note(why),
            }
            self.lib_selected = None;
            self.refresh_library();
        }
    }

    /// Publish a tone, after the sheet asks, or open it on TonePush when it
    /// is there already.
    pub(crate) fn publish_or_open(&mut self, entry: usize, ctx: &egui::Context) {
        let Some(hash) = self.lib_entries.get(entry).map(|e| e.hash.clone()) else {
            return;
        };
        let known = self.portable_hashes.get(&hash).and_then(|portable| {
            self.cloud_files
                .as_ref()
                .filter(|files| files.contains(portable))
                .map(|_| portable.clone())
        });
        match known {
            Some(portable) => {
                ctx.open_url(egui::OpenUrl::new_tab(crate::cloud::tone_url(&portable)));
            }
            None => self.ask_to_publish(vec![hash]),
        }
    }

    /// A tone from TonePush: what it is, auditioning it on the pedal,
    /// keeping it, and its versions.
    fn cloud_tone_inspector(&mut self, ui: &mut Ui) {
        let Some(index) = self
            .cloud_selected
            .filter(|selected| *selected < self.cloud_entries.len())
        else {
            ui.add_space(24.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Select a tone to audition it on the pedal or keep it in your library.",
                    )
                    .font(theme::regular(theme::SECONDARY))
                    .color(theme::muted()),
                )
                .wrap(),
            );
            return;
        };
        let entry = self.cloud_entries[index].discovered.clone();
        let row = self.cloud_entries[index].row.clone();
        let summary = &entry.tone.summary;
        ui.spacing_mut().item_spacing.y = 4.0;
        let title = shell::title_galley(
            ui,
            &summary.name,
            theme::BLOCK_NAME,
            theme::text(),
            ui.available_width(),
        );
        let (place, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
        shell::paint_line(ui, title, place.left(), place.center().y);
        let mut words = vec![format!(
            "v{} of {}",
            summary.version_number.unwrap_or(1).max(1),
            summary.versions_count.max(1)
        )];
        if let Some(creator) = summary.creator.as_ref().filter(|c| !c.is_empty()) {
            words.push(format!("by {creator}"));
        }
        if !row.meta.character.trim().is_empty() {
            words.push(shell::sentence_case(row.meta.character.trim()));
        }
        let plays = self.fit(&row, false).plays;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            if row.marker.is_some() {
                marker(ui, row.marker.as_ref(), plays);
            } else {
                theme::label(
                    ui,
                    &summary.device.name,
                    theme::regular(12.0),
                    theme::muted(),
                );
            }
            theme::label_truncated(ui, &words.join(" · "), theme::regular(12.0), theme::muted());
        });
        if !row.chain.is_empty() && theme::Tier::now(ui.ctx()) == Tier::L {
            ui.add_space(8.0);
            chain_wells(ui, &row.chain);
        }
        ui.add_space(8.0);
        rule(ui);

        // Where it is: the pedal, this computer, TonePush.
        let mut action = None;
        let heard = self.hears(&crate::audition::Source::TonePush(summary.id));
        let set_aside = self.hearing.set_aside.clone().unwrap_or_default();
        if heard && self.heard_loading() {
            where_row(
                ui,
                Icon::LoaderCircle,
                theme::accent(),
                &[
                    ("On its way to ", Words::Soft),
                    (set_aside.slot.as_str(), Words::Bold),
                ],
                None,
            );
        } else if heard {
            let playing = format!("Playing in {}", set_aside.slot);
            let place = format!(", in place of {}", set_aside.name);
            let mut words = vec![(playing.as_str(), Words::Accent)];
            if !set_aside.name.is_empty() {
                words.push((place.as_str(), Words::Soft));
            }
            where_row(ui, Icon::Volume, theme::accent(), &words, None);
        } else if let Some(why) = self.cloud_audition_blocker(&entry) {
            where_row_hover(ui, Icon::Ban, theme::muted(), &[(&why, Words::Soft)], &why);
        } else if where_row(
            ui,
            Icon::Pedal,
            theme::muted(),
            &[("Not playing", Words::Soft)],
            Some("Play"),
        ) {
            action = Some(2);
        }
        let wanted = entry.tone.file_sha256.clone();
        let local = self.local_cloud_entry(wanted.as_deref());
        match local.and_then(|row| self.lib_entries.get(row).map(|entry| entry.version)) {
            Some(version) => {
                let version = format!("v{version}");
                if where_row(
                    ui,
                    Icon::Computer,
                    theme::text_soft(),
                    &[("In your library · ", Words::Soft), (&version, Words::Bold)],
                    Some("Show"),
                ) {
                    action = Some(3);
                }
            }
            None => {
                if where_row(
                    ui,
                    Icon::Computer,
                    theme::muted(),
                    &[("Not in your library", Words::Soft)],
                    Some("Keep"),
                ) {
                    action = Some(4);
                }
            }
        }
        let downloads = summary.downloads_count.unwrap_or(summary.installs_count);
        let mut counted = format!("{} downloads", crate::format_count(downloads));
        if downloads == 1 {
            counted = "1 download".to_owned();
        }
        let updated = crate::day_month(&summary.updated_at);
        if !updated.is_empty() {
            counted = format!("{counted} · updated {updated}");
        }
        if where_row(
            ui,
            Icon::Cloud,
            theme::text_soft(),
            &[(&counted, Words::Soft)],
            wanted.as_ref().map(|_| "Open"),
        ) {
            if let Some(hash) = &wanted {
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab(crate::cloud::tone_url(hash)));
            }
        }
        ui.add_space(10.0);
        section(ui, "Song and tone");
        let meta = &row.meta;
        let downloads = crate::format_count(summary.installs_count);
        let rating = summary
            .rating
            .map(|rating| format!("{rating:.1} of 5"))
            .unwrap_or_default();
        for (label, value) in [
            ("Song", meta.song.clone()),
            ("Artist", meta.artist.clone()),
            ("Part", meta.part.clone()),
            ("Character", shell::sentence_case(&meta.character)),
            ("Genres", meta.genres.join(", ")),
            ("Guitar", meta.guitar.clone()),
            ("Pickups", meta.pickup_type.clone()),
            ("Electronics", meta.pickup_electronics.clone()),
            ("Tuning", meta.tuning.clone()),
            ("Downloads", downloads),
            ("Rating", rating),
        ] {
            if value.trim().is_empty() {
                continue;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                key(ui, label);
                theme::label_truncated(ui, &value, theme::regular(theme::SECONDARY), theme::text());
            });
        }
        if !meta.tags.is_empty() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                key(ui, "Tags");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
                    for tag in &meta.tags {
                        theme::Chip::new(tag).show(ui);
                    }
                });
            });
        }
        for (label, text) in [
            ("The song", meta.description.clone()),
            ("This tone", meta.tone_description.clone()),
        ] {
            if text.trim().is_empty() {
                continue;
            }
            ui.add_space(6.0);
            theme::label(ui, label, theme::regular(12.0), theme::muted());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .font(theme::regular(theme::SECONDARY))
                        .color(theme::text_soft()),
                )
                .wrap(),
            );
        }
        if entry.tone.versions.len() > 1 {
            ui.add_space(10.0);
            section(ui, "Versions");
            let mut chosen_version = None;
            let loading = self.heard_loading();
            for historical in entry.tone.versions.iter().rev() {
                let key = if historical.current {
                    summary.id
                } else {
                    Self::cloud_version_entry(&entry, historical)
                        .tone
                        .summary
                        .id
                };
                let playing = self.hears(&crate::audition::Source::TonePush(key));
                let clicked = version_row(
                    ui,
                    &format!("v{}", historical.number),
                    &crate::day_month(&historical.created_at),
                    playing.then_some(loading),
                    |ui| {
                        if !historical.current
                            && theme::Button::new("Keep")
                                .ghost()
                                .small()
                                .show(ui)
                                .on_hover_text("Keep this version in your library")
                                .clicked()
                        {
                            chosen_version = Some((historical.clone(), CloudAction::Computer));
                        }
                        if historical.current && !playing {
                            theme::Chip::new("Current").mood(Mood::Ok).show(ui);
                        }
                    },
                );
                if clicked && !playing {
                    chosen_version = Some((historical.clone(), CloudAction::Audition));
                }
            }
            if let Some((version, kind)) = chosen_version {
                if version.current {
                    self.start_cloud_action(index, kind, ui.ctx());
                } else {
                    let historical = Self::cloud_version_entry(&entry, &version);
                    self.start_cloud_entry_action(historical, kind, ui.ctx());
                }
            }
        }
        match action {
            Some(2) => self.start_cloud_action(index, CloudAction::Audition, ui.ctx()),
            Some(3) => {
                if let Some(row) = local {
                    self.show_library_view(LibraryView::Tones);
                    self.lib_tag_filter = None;
                    self.select_lib_entry(row);
                }
            }
            Some(4) => self.start_cloud_action(index, CloudAction::Computer, ui.ctx()),
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Setlists

    /// What the connected pedal holds, slot by slot, as its backup says.
    /// `None` when there is no pedal, or nothing read from it yet.
    pub(crate) fn pedal_slots(&self) -> Option<PedalSlots> {
        if !self.pedal_online() {
            return None;
        }
        if self.pro_active() {
            let (hashes, names) = self.pro.pedal_slots()?;
            return Some(PedalSlots { hashes, names });
        }
        if self.mirror.is_empty() || self.presets.is_empty() {
            return None;
        }
        Some(PedalSlots {
            hashes: self.mirror.clone(),
            names: self.presets.clone(),
        })
    }

    /// How a setlist compares with the pedal, when that can be said.
    pub(crate) fn setlist_states(&self, setlist: &library::Setlist) -> Option<Vec<SlotState>> {
        if !self.setlist_compatible(setlist) {
            return None;
        }
        let pedal = self.pedal_slots()?;
        Some(compare_slots(&setlist.slots, &pedal))
    }

    /// The setlists, as cards down the left, and the chosen one beside them.
    pub(crate) fn setlists_view(&mut self, root: &mut Ui, tier: Tier) {
        egui::Panel::left("lib-setlists")
            .resizable(false)
            .exact_size(tier.pick(270.0, 300.0, 340.0))
            .frame(
                // The panel's own separator is the one line on its right.
                egui::Frame::new().fill(theme::bg()),
            )
            .show(root, |ui| self.setlist_cards(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| self.setlist_detail(ui, tier));
    }

    /// The setlists as cards: name, how many presets, where and when, and how
    /// each compares with the pedal.
    fn setlist_cards(&mut self, ui: &mut Ui) {
        let needle = self.setlist_search.trim().to_ascii_lowercase();
        let mut rows: Vec<usize> = self
            .lib_setlists
            .iter()
            .enumerate()
            .filter(|(_, (_, setlist))| self.setlist_in_library_scope(setlist))
            .filter(|(_, (_, setlist))| {
                needle.is_empty()
                    || setlist.name.to_ascii_lowercase().contains(&needle)
                    || setlist.venue.to_ascii_lowercase().contains(&needle)
                    || setlist.date.to_ascii_lowercase().contains(&needle)
                    || setlist.description.to_ascii_lowercase().contains(&needle)
                    || setlist
                        .slots
                        .iter()
                        .any(|slot| slot.name.to_ascii_lowercase().contains(&needle))
            })
            .map(|(index, _)| index)
            .collect();
        // Newest first: the setlist played last is the one wanted next.
        rows.sort_by(|a, b| {
            let (sa, sb) = (&self.lib_setlists[*a].1, &self.lib_setlists[*b].1);
            sb.modified_at
                .cmp(&sa.modified_at)
                .then_with(|| sa.name.to_lowercase().cmp(&sb.name.to_lowercase()))
        });
        let live = self.pedal_online();
        let mut picked = None;
        let mut menu = None;
        let mut capture = false;
        let mut dragged = None;
        self.menu_anchor = None;
        egui::ScrollArea::vertical()
            .id_salt("setlist-cards")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if rows.is_empty() {
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                theme::label(
                                    ui,
                                    if needle.is_empty() {
                                        "No setlists yet"
                                    } else {
                                        "No setlist matches"
                                    },
                                    theme::regular(theme::SECONDARY),
                                    theme::muted(),
                                );
                            });
                            ui.add_space(8.0);
                        }
                        for &index in &rows {
                            let setlist = self.lib_setlists[index].1.clone();
                            let states = self.setlist_states(&setlist);
                            let selected = self.lib_setlist == Some(index);
                            let response =
                                self.setlist_card(ui, &setlist, states.as_deref(), selected);
                            if response.drag_started() {
                                dragged = Some(index);
                            }
                            if self.dragged(ui.ctx()).is_some() {
                                self.drop_zone(response.rect, crate::dnd::Target::Setlist(index));
                                if self.drops.hover == Some(crate::dnd::Target::Setlist(index)) {
                                    theme::paint::dashed_rect(
                                        ui.painter(),
                                        response.rect.shrink(0.75),
                                        9.5,
                                        Stroke::new(1.5, theme::accent_line()),
                                        4.0,
                                        3.0,
                                    );
                                }
                            }
                            if response.clicked() {
                                picked = Some(index);
                            }
                            if response.secondary_clicked() {
                                let at = ui
                                    .ctx()
                                    .input(|input| input.pointer.interact_pos())
                                    .unwrap_or_else(|| response.rect.left_bottom());
                                menu = Some((index, at));
                            }
                            if selected {
                                self.menu_anchor =
                                    Some(response.rect.left_bottom() + Vec2::new(36.0, 2.0));
                            }
                        }
                        ui.add_space(6.0);
                        let width = ui.available_width();
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(width, 38.0),
                            if live { Sense::click() } else { Sense::hover() },
                        );
                        if response.hovered() && live {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(10),
                                theme::alpha(theme::hover(), 0.5),
                            );
                        }
                        theme::paint::dashed_rect(
                            ui.painter(),
                            rect.shrink(0.5),
                            9.5,
                            Stroke::new(1.0, theme::line_strong()),
                            4.0,
                            3.0,
                        );
                        theme::paint_icon(
                            ui,
                            Icon::Computer,
                            Pos2::new(rect.left() + 14.0 + 7.0, rect.center().y),
                            14.0,
                            if live {
                                theme::text_soft()
                            } else {
                                theme::faint()
                            },
                        );
                        let words = shell::galley(
                            ui,
                            "Keep the whole pedal as a setlist",
                            theme::regular(theme::SECONDARY),
                            if live {
                                theme::text_soft()
                            } else {
                                theme::faint()
                            },
                        );
                        shell::paint_line(ui, words, rect.left() + 14.0 + 24.0, rect.center().y);
                        if response
                            .on_hover_text(if live {
                                "Read every preset on the pedal into your library, as a setlist"
                            } else {
                                "Connect a pedal to keep what is on it"
                            })
                            .clicked()
                        {
                            capture = true;
                        }
                    });
            });
        if let Some(index) = dragged {
            self.start_drag(ui.ctx(), crate::dnd::Dragged::Setlist(index));
        }
        if let Some(index) = picked {
            self.select_setlist_entry(index);
            self.hearing.arrows = crate::audition::Arrows::Setlists;
        }
        // A right-click chooses the setlist and opens its menu.
        if let Some((index, at)) = menu {
            self.select_setlist_entry(index);
            self.hearing.arrows = crate::audition::Arrows::Setlists;
            self.open_row_menu(ui.ctx(), crate::menus::MenuFor::Setlist(index), at);
        }
        if capture {
            self.capture_pedal(None);
        }
    }

    /// One setlist as a card.
    fn setlist_card(
        &self,
        ui: &mut Ui,
        setlist: &library::Setlist,
        states: Option<&[SlotState]>,
        selected: bool,
    ) -> Response {
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, 84.0), Sense::click_and_drag());
        if selected {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(10), theme::hover());
        } else if response.hovered() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(10),
                theme::alpha(theme::hover(), 0.5),
            );
        }
        let inner = rect.shrink2(Vec2::new(10.0, 9.0));
        // Which pedal it is for, beside its name, as every row says.
        let mut marker_width = 0.0;
        if let Some(found) = self.setlist_marker(setlist) {
            let plays = self.setlist_compatible(setlist) || !self.pedal_online();
            let chip = theme::Marker::new(found.family.label(), &found.model).solid(plays);
            let size = chip.size(ui);
            chip.paint(
                ui,
                Rect::from_min_size(
                    Pos2::new(inner.right() - size.x, inner.top() + 9.0 - size.y / 2.0),
                    size,
                ),
            );
            marker_width = size.x + 10.0;
        }
        let name = shell::elided(
            ui,
            setlist.name.clone(),
            theme::semibold(14.0),
            theme::text(),
            inner.width() - marker_width,
        );
        shell::paint_line(ui, name, inner.left(), inner.top() + 9.0);
        let count = match setlist.filled() {
            1 => "1 preset".to_owned(),
            count => format!("{count} presets"),
        };
        let place: Vec<&str> = [setlist.venue.as_str(), setlist.date.as_str()]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .chain(std::iter::once(count.as_str()))
            .collect();
        let where_when = shell::elided(
            ui,
            place.join(" · "),
            theme::regular(12.0),
            theme::muted(),
            inner.width(),
        );
        shell::paint_line(ui, where_when, inner.left(), inner.top() + 30.0);
        let version = shell::galley(
            ui,
            format!("v{}", setlist.revision()),
            theme::regular(11.5),
            theme::faint(),
        );
        let version_width = version.size().x;
        shell::paint_line(
            ui,
            version,
            inner.right() - version_width,
            inner.bottom() - 9.0,
        );
        // How it compares, in one chip.
        let chip = if !self.setlist_compatible(setlist) {
            Some((
                if self.pro_active() {
                    "For HX pedals".to_owned()
                } else {
                    "For StompStation PRO".to_owned()
                },
                Mood::Neutral,
                None,
            ))
        } else {
            states.map(|states| {
                let differing = Differences::of(states).differing();
                if differing == 0 {
                    ("Matches the pedal".to_owned(), Mood::Ok, Some(Icon::Check))
                } else if differing == 1 {
                    (
                        "1 slot differs".to_owned(),
                        Mood::Hot,
                        Some(Icon::GitCompare),
                    )
                } else {
                    (
                        format!("{differing} slots differ"),
                        Mood::Hot,
                        Some(Icon::GitCompare),
                    )
                }
            })
        };
        if let Some((words, mood, icon)) = chip {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        Pos2::new(inner.left(), inner.bottom() - 20.0),
                        Pos2::new(inner.right() - version_width - 8.0, inner.bottom()),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let mut chip = theme::Chip::new(&words).mood(mood);
            if let Some(icon) = icon {
                chip = chip.icon(icon);
            }
            chip.show(&mut child);
        }
        response
    }

    /// The chosen setlist: its name, place and date typed in place, what
    /// putting it on the pedal would change, and its slots bank by bank.
    fn setlist_detail(&mut self, ui: &mut Ui, tier: Tier) {
        let Some(i) = self.lib_setlist.filter(|i| *i < self.lib_setlists.len()) else {
            let rect = ui.max_rect();
            let words = shell::galley(
                ui,
                "Choose a setlist to see it bank by bank against the pedal.",
                theme::regular(theme::BODY),
                theme::muted(),
            );
            let width = words.size().x;
            shell::paint_line(ui, words, rect.center().x - width / 2.0, rect.top() + 80.0);
            return;
        };
        let setlist = self.lib_setlists[i].1.clone();
        let live = self.pedal_online() && self.setlist_compatible(&setlist);
        let states = self.setlist_states(&setlist);
        let device = if self.device.trim().is_empty() {
            "the pedal".to_owned()
        } else {
            self.device.trim().to_owned()
        };
        egui::ScrollArea::vertical()
            .id_salt("setlist-detail")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 20,
                        top: 14,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 6.0;
                        self.setlist_head(ui, &setlist, live, &device);
                        ui.add_space(8.0);
                        if let Some(states) = &states {
                            let shown = self.shown_slots(&setlist, Some(states));
                            self.setlist_banner(ui, states, shown);
                            ui.add_space(8.0);
                        } else if !self.setlist_compatible(&setlist) {
                            theme::banner(
                                ui,
                                Mood::Info,
                                Icon::Info,
                                "For another pedal",
                                "Its tones are for another family of pedal, so it is not \
                                 compared with this one.",
                                |_| {},
                            );
                            ui.add_space(8.0);
                        }
                        self.bank_grid(ui, &setlist, states.as_deref(), tier);
                        ui.add_space(14.0);
                        theme::caption(ui, "Notes");
                        let notes = ui.add(
                            egui::TextEdit::multiline(&mut self.lib_setlist_draft.description)
                                .desired_width(f32::INFINITY)
                                .desired_rows(2)
                                .font(theme::regular(theme::SECONDARY))
                                .hint_text(
                                    egui::RichText::new("Anything worth remembering about it")
                                        .color(theme::faint()),
                                ),
                        );
                        if notes.changed() {
                            self.save_setlist_draft();
                        }
                    });
            });
    }

    /// The setlist's head: name, venue and date typed in place, its version,
    /// and capturing the pedal as its next version or putting it on.
    fn setlist_head(&mut self, ui: &mut Ui, setlist: &library::Setlist, live: bool, device: &str) {
        let width = ui.available_width();
        // The actions take about 420 points; below room for them and a
        // readable name beside them, they go under the name.
        let stacked = width < 660.0;
        let (put, capture, remove) = if stacked {
            self.setlist_words(ui, setlist, width);
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(
                Vec2::new(width, 30.0),
                egui::Layout::right_to_left(egui::Align::Min),
                |ui| self.setlist_actions(ui, setlist, live, device),
            )
            .inner
        } else {
            ui.horizontal_top(|ui| {
                self.setlist_words(ui, setlist, width - 420.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    self.setlist_actions(ui, setlist, live, device)
                })
                .inner
            })
            .inner
        };
        if put {
            self.confirm_push = self.lib_setlist;
        }
        if capture {
            self.capture_pedal(Some(setlist.name.clone()));
        }
        if remove {
            if let Some((path, _)) = self
                .lib_setlist
                .and_then(|i| self.lib_setlists.get(i))
                .cloned()
            {
                match library::remove_setlist(&path) {
                    Ok(()) => {
                        self.lib_setlist = None;
                        self.refresh_library();
                    }
                    Err(why) => self.note(why),
                }
            }
        }
    }

    /// The setlist's name, and its venue, date and version, edited in place.
    fn setlist_words(&mut self, ui: &mut Ui, setlist: &library::Setlist, width: f32) {
        let width = width.max(200.0);
        ui.allocate_ui_with_layout(
            Vec2::new(width, 50.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let found = self.setlist_marker(setlist);
                let plays = self.setlist_compatible(setlist) || !self.pedal_online();
                let named = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let named = fitted_field(
                            ui,
                            "setlist-name",
                            &mut self.lib_setlist_draft.name,
                            "Name this setlist",
                            theme::semibold(theme::PRESET_NAME),
                        );
                        if std::mem::take(&mut self.pane.rename_setlist) {
                            select_all(ui.ctx(), &named, &self.lib_setlist_draft.name);
                        }
                        marker(ui, found.as_ref(), plays);
                        named
                    })
                    .inner;
                // A setlist with no name has no file to live in.
                if named.lost_focus() {
                    if self.lib_setlist_draft.name.trim().is_empty() {
                        self.lib_setlist_draft.name = setlist.name.clone();
                    } else if self.lib_setlist_draft.name != setlist.name {
                        self.save_setlist_draft();
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let venue = fitted_field(
                        ui,
                        "setlist-venue",
                        &mut self.lib_setlist_draft.venue,
                        "Venue",
                        theme::regular(12.5),
                    );
                    if venue.lost_focus() && self.lib_setlist_draft.venue != setlist.venue {
                        self.save_setlist_draft();
                    }
                    theme::label(ui, "·", theme::regular(12.5), theme::faint());
                    let date = fitted_field(
                        ui,
                        "setlist-date",
                        &mut self.lib_setlist_draft.date,
                        "Date",
                        theme::regular(12.5),
                    );
                    if date.lost_focus() && self.lib_setlist_draft.date != setlist.date {
                        self.save_setlist_draft();
                    }
                    theme::label(ui, "·", theme::regular(12.5), theme::faint());
                    theme::label(
                        ui,
                        &format!(
                            "v{}, captured {}",
                            setlist.revision(),
                            crate::day_month(&setlist.modified_at)
                        ),
                        theme::regular(12.5),
                        theme::muted(),
                    );
                });
            },
        );
    }

    /// Capture, Put on and the version's menu, laid out right to left; says
    /// which was asked for.
    fn setlist_actions(
        &mut self,
        ui: &mut Ui,
        setlist: &library::Setlist,
        live: bool,
        device: &str,
    ) -> (bool, bool, bool) {
        ui.spacing_mut().item_spacing.x = 10.0;
        let next = setlist.revision() + 1;
        let mut remove = false;
        let more = theme::IconButton::new(Icon::Ellipsis)
            .show(ui)
            .on_hover_text("More");
        egui::Popup::menu(&more)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .show(|ui| {
                theme::menu_width(ui, 220.0);
                theme::menu_header(ui, &setlist.name, Some(&format!("v{}", setlist.revision())));
                remove = theme::menu_danger(ui, Icon::Remove, "Remove this version").clicked();
            });
        let put = theme::Button::new(&format!("Put on {device}…"))
            .primary()
            .icon(Icon::Download)
            .enabled(live)
            .show(ui)
            .on_hover_text(if live {
                "Write the setlist over what is on the pedal, after a question"
            } else {
                "Connect a pedal of its family to put it on"
            })
            .clicked();
        let capture = theme::Button::new(&format!("Capture the pedal as v{next}"))
            .icon(Icon::Computer)
            .enabled(self.pedal_online())
            .show(ui)
            .on_hover_text("Keep what is on the pedal now as this setlist's next version")
            .clicked();
        (put, capture, remove)
    }

    /// What putting the setlist on the pedal would change, as a banner with
    /// the filter between every slot and only those that differ.
    fn setlist_banner(&mut self, ui: &mut Ui, states: &[SlotState], shown: usize) {
        let counts = Differences::of(states);
        let (title, body) = difference_words(counts);
        // Where slots differ, what a click on one does.
        let body = if counts.differing() > 0 {
            format!("{body} A click plays the setlist's version; the pedal's stays where it is.")
        } else {
            body
        };
        let total = shown;
        let differing = counts.differing();
        let mood = if differing == 0 { Mood::Ok } else { Mood::Hot };
        let icon = if differing == 0 {
            Icon::CircleCheck
        } else {
            Icon::GitCompare
        };
        let mut only = self.setlist_only_differing;
        egui::Frame::new()
            .fill(theme::panel())
            .stroke(Stroke::new(1.0, theme::line()))
            .corner_radius(CornerRadius::same(theme::RADIUS_CARD))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (well, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    ui.painter()
                        .rect_filled(well, CornerRadius::same(8), mood.soft());
                    theme::paint_icon(ui, icon, well.center(), 16.0, mood.ink());
                    let room = (ui.available_width() - 210.0).max(160.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(room, 36.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(room);
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                &format!("{title} "),
                                0.0,
                                egui::TextFormat {
                                    font_id: theme::semibold(theme::SECONDARY),
                                    color: theme::text(),
                                    ..Default::default()
                                },
                            );
                            job.append(
                                &body,
                                0.0,
                                egui::TextFormat {
                                    font_id: theme::regular(theme::SECONDARY),
                                    color: theme::text_soft(),
                                    ..Default::default()
                                },
                            );
                            job.wrap.max_width = room;
                            ui.label(job);
                        },
                    );
                    if differing > 0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let all = format!("All {}", total);
                            let some = format!("{differing} that differ");
                            let segments = [theme::Segment::new(&all), theme::Segment::new(&some)];
                            if let Some(index) = theme::segmented(
                                ui,
                                "setlist-filter",
                                &segments,
                                Some(usize::from(only)),
                                false,
                            )
                            .clicked
                            {
                                only = index == 1;
                            }
                        });
                    }
                });
            });
        self.setlist_only_differing = only && differing > 0;
    }

    /// The setlist's slots, bank by bank: what each holds, the slots that
    /// differ from the pedal marked, and the ones it would empty named.
    /// Rows of 30 point cells, 5 apart.
    fn bank_grid(
        &mut self,
        ui: &mut Ui,
        setlist: &library::Setlist,
        states: Option<&[SlotState]>,
        tier: Tier,
    ) {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            self.bank_rows(ui, setlist, states, tier);
        });
    }

    fn bank_rows(
        &mut self,
        ui: &mut Ui,
        setlist: &library::Setlist,
        states: Option<&[SlotState]>,
        tier: Tier,
    ) {
        let per_bank = if self.pro_active() {
            3
        } else {
            self.presets_per_bank()
        };
        let total = self.shown_slots(setlist, states);
        let banks = total.div_ceil(per_bank.max(1));
        let width = ui.available_width();
        let label_width = 36.0;
        let gap = 6.0;
        let cell =
            ((width - label_width - gap * (per_bank as f32 - 1.0)) / per_bank as f32).floor();
        let only = self.setlist_only_differing;
        let mut play = None;
        let mut menu = None;
        let mut drag_slot = None;
        let mut drop_zones = Vec::new();
        let dragging = self.dragged(ui.ctx()).is_some();
        let setlist_index = self
            .lib_setlists
            .iter()
            .position(|(_, known)| {
                known.series == setlist.series && known.version == setlist.version
            })
            .or(self.lib_setlist)
            .unwrap_or(0);
        let pedal = self.pedal_slots();
        for bank in 0..banks {
            let slots: Vec<usize> = (bank * per_bank..((bank + 1) * per_bank).min(total)).collect();
            let differing = slots.iter().any(|slot| {
                states
                    .and_then(|s| s.get(*slot))
                    .is_some_and(|state| state.differs())
            });
            if only && !differing {
                continue;
            }
            let (row, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
            let label = shell::galley(
                ui,
                format!("{:02}", bank + 1),
                theme::semibold(12.0),
                theme::muted(),
            );
            shell::paint_line(ui, label, row.left(), row.center().y);
            for (column, slot) in slots.iter().enumerate() {
                let rect = Rect::from_min_size(
                    Pos2::new(
                        row.left() + label_width + column as f32 * (cell + gap),
                        row.top(),
                    ),
                    Vec2::new(cell, 30.0),
                );
                let entry = setlist.slots.get(*slot).filter(|s| !s.is_empty());
                let state = states.and_then(|s| s.get(*slot)).copied();
                let response = ui.interact(
                    rect,
                    ui.id().with(("bank-slot", *slot)),
                    if entry.is_some() {
                        Sense::click_and_drag()
                    } else {
                        Sense::hover()
                    },
                );
                let target = crate::dnd::Target::SetlistSlot {
                    setlist: setlist_index,
                    slot: *slot,
                };
                if dragging {
                    drop_zones.push((rect, target));
                }
                if entry.is_some() && response.drag_started() {
                    drag_slot = Some(*slot);
                }
                let dropping_here = dragging && self.drops.hover == Some(target);
                let hot = state.is_some_and(SlotState::differs);
                // The slot playing on the pedal: amber, with a speaker.
                let heard = entry.is_some_and(|entry| {
                    self.hears(&crate::audition::Source::Setlist {
                        hash: entry.hash.clone(),
                        setlist: setlist.name.clone(),
                        slot: *slot as i64,
                    })
                });
                let fill = if heard || dropping_here {
                    theme::accent_soft()
                } else if hot {
                    theme::mix(theme::panel(), theme::hot(), 0.07)
                } else if response.hovered() && entry.is_some() {
                    theme::raised()
                } else {
                    theme::panel()
                };
                if entry.is_none() && state != Some(SlotState::Empty) && state.is_some() {
                    theme::paint::dashed_rect(
                        ui.painter(),
                        rect.shrink(0.5),
                        7.5,
                        Stroke::new(1.0, theme::alpha(theme::hot(), 0.8)),
                        4.0,
                        3.0,
                    );
                } else {
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(8),
                        fill,
                        Stroke::new(
                            1.0,
                            if heard || dropping_here {
                                theme::accent_line()
                            } else if hot {
                                theme::alpha(theme::hot(), 0.75)
                            } else {
                                theme::line()
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                }
                let letter = (b'A' + column as u8) as char;
                let letter = shell::galley(
                    ui,
                    letter.to_string(),
                    theme::semibold(11.0),
                    theme::faint(),
                );
                shell::paint_line(ui, letter, rect.left() + 10.0, rect.center().y);
                let mut right = rect.right() - 8.0;
                if heard {
                    theme::paint_icon(
                        ui,
                        Icon::Volume,
                        Pos2::new(right - 6.0, rect.center().y),
                        12.0,
                        theme::accent(),
                    );
                    right -= 18.0;
                } else if hot {
                    theme::paint_icon(
                        ui,
                        Icon::GitCompare,
                        Pos2::new(right - 6.0, rect.center().y),
                        12.0,
                        theme::hot(),
                    );
                    right -= 18.0;
                }
                // A tone the library no longer holds cannot be written, and
                // the slot says so rather than looking ready.
                let missing = entry.is_some_and(|entry| {
                    !self
                        .library_lookup
                        .setlist_tones
                        .get(&entry.hash)
                        .is_some_and(|tone| tone.held)
                });
                let (text, colour) = match (entry, state) {
                    (Some(entry), _) if missing => {
                        (format!("{} · missing", entry.name), theme::faint())
                    }
                    (Some(entry), _) => (entry.name.clone(), theme::text()),
                    (None, Some(SlotState::Clears)) => {
                        let held = pedal
                            .as_ref()
                            .and_then(|pedal| pedal.names.get(*slot))
                            .cloned()
                            .unwrap_or_default();
                        (format!("Empty · clears {held}"), theme::muted())
                    }
                    (None, _) => ("Empty".to_owned(), theme::faint()),
                };
                let name = shell::elided(
                    ui,
                    text,
                    theme::regular(theme::BODY),
                    colour,
                    (right - rect.left() - 26.0).max(10.0),
                );
                shell::paint_line(ui, name, rect.left() + 26.0, rect.center().y);
                if let Some(entry) = entry {
                    let label = self.active_slot_label(*slot as i64);
                    let hover = match state {
                        Some(SlotState::Same) => format!("{label} holds this already"),
                        Some(SlotState::Replaced) => format!("{label} holds something else now"),
                        Some(SlotState::Fills) => format!("{label} is empty now"),
                        _ => label.clone(),
                    };
                    let response = response.on_hover_text(format!(
                        "{hover}. A click plays the setlist's version; its menu sends it to {label}"
                    ));
                    if response.clicked() {
                        play = Some((*slot, entry.clone()));
                    }
                    if response.secondary_clicked() {
                        let at = ui
                            .ctx()
                            .input(|input| input.pointer.interact_pos())
                            .unwrap_or_else(|| rect.left_bottom());
                        menu = Some((*slot, at));
                    }
                }
            }
            ui.add_space(gap - ui.spacing().item_spacing.y);
        }
        let _ = tier;
        for (rect, target) in drop_zones {
            self.drop_zone(rect, target);
        }
        if let Some(slot) = drag_slot {
            self.start_drag(
                ui.ctx(),
                crate::dnd::Dragged::SetlistSlot {
                    setlist: setlist_index,
                    slot,
                },
            );
        }
        if let Some((slot, entry)) = play {
            self.hearing.arrows = crate::audition::Arrows::Presets;
            self.audition_setlist_slot(&setlist.name, slot as i64, &entry);
        }
        if let Some((slot, at)) = menu {
            self.open_row_menu(
                ui.ctx(),
                crate::menus::MenuFor::SetlistSlot {
                    setlist: setlist_index,
                    slot,
                },
                at,
            );
        }
    }

    /// What putting a setlist on the pedal changes, as the confirmation's
    /// rows: the presets replaced and the slots filled or emptied, by slot,
    /// and the rest that are written as they are.
    pub(crate) fn push_outcomes(&self, states: &[SlotState]) -> Vec<theme::Outcome> {
        let pedal = self.pedal_slots();
        let label = |slot: usize| self.active_slot_label(slot as i64);
        let list = |names: Vec<String>| {
            if names.len() > 5 {
                format!("{}, and {} more", names[..4].join(", "), names.len() - 4)
            } else {
                names.join(", ")
            }
        };
        let slots = |wanted: SlotState| -> Vec<usize> {
            states
                .iter()
                .enumerate()
                .filter(|(_, state)| **state == wanted)
                .map(|(slot, _)| slot)
                .collect()
        };
        let counts = Differences::of(states);
        let mut rows = Vec::new();
        let replaced = slots(SlotState::Replaced);
        if !replaced.is_empty() {
            rows.push(theme::Outcome {
                icon: Icon::RefreshCw,
                count: replaced.len().to_string(),
                sentence: format!(
                    "{}: {}",
                    if replaced.len() == 1 {
                        "preset replaced"
                    } else {
                        "presets replaced"
                    },
                    list(replaced.into_iter().map(label).collect())
                ),
                quiet: false,
            });
        }
        let filled = slots(SlotState::Fills);
        if !filled.is_empty() {
            rows.push(theme::Outcome {
                icon: Icon::Download,
                count: filled.len().to_string(),
                sentence: format!(
                    "{}: {}",
                    if filled.len() == 1 {
                        "empty slot filled"
                    } else {
                        "empty slots filled"
                    },
                    list(filled.into_iter().map(label).collect())
                ),
                quiet: false,
            });
        }
        let cleared = slots(SlotState::Clears);
        if !cleared.is_empty() {
            let named = cleared
                .into_iter()
                .map(|slot| {
                    let held = pedal
                        .as_ref()
                        .and_then(|pedal| pedal.names.get(slot))
                        .cloned()
                        .unwrap_or_default();
                    format!("{} {held}", label(slot)).trim().to_owned()
                })
                .collect::<Vec<_>>();
            rows.push(theme::Outcome {
                icon: Icon::Remove,
                count: named.len().to_string(),
                sentence: format!(
                    "{}: {}",
                    if named.len() == 1 {
                        "slot emptied"
                    } else {
                        "slots emptied"
                    },
                    list(named)
                ),
                quiet: false,
            });
        }
        rows.push(theme::Outcome {
            icon: Icon::Check,
            count: counts.same.to_string(),
            sentence: "already match, and are written again as they are".to_owned(),
            quiet: true,
        });
        rows.push(theme::Outcome {
            icon: Icon::CircleDashed,
            count: counts.empty.to_string(),
            sentence: "empty slots stay empty".to_owned(),
            quiet: true,
        });
        rows
    }

    /// How many slots the bank grid shows: every bank up to the last slot
    /// either the setlist or the pedal fills, so a setlist of 42 on a pedal of
    /// 126 slots is 14 banks, not 42.
    fn shown_slots(&self, setlist: &library::Setlist, states: Option<&[SlotState]>) -> usize {
        let per_bank = if self.pro_active() {
            3
        } else {
            self.presets_per_bank()
        }
        .max(1);
        let last = (0..setlist
            .slots
            .len()
            .max(states.map_or(0, <[SlotState]>::len)))
            .rev()
            .find(|slot| {
                setlist.slots.get(*slot).is_some_and(|s| !s.is_empty())
                    || states
                        .and_then(|states| states.get(*slot))
                        .is_some_and(|state| *state != SlotState::Empty)
            });
        last.map_or(0, |last| (last / per_bank + 1) * per_bank)
    }

    /// Read every preset on the pedal into the library as a setlist; with a
    /// name, as that setlist's next version. Says whether it was asked for.
    pub(crate) fn capture_pedal(&mut self, name: Option<String>) -> bool {
        if !self.pedal_online() {
            self.note("no pedal to keep".to_owned());
            return false;
        }
        // A capture asked for now is kept and named as usual, and a write
        // that was waiting for an earlier one is not made: which of the two
        // arrives first cannot be told apart.
        if let Some(waiting) = self.push_after_capture.take() {
            self.note(format!(
                "“{}” was not written: the pedal was captured again before it was kept",
                waiting.name
            ));
        }
        self.capture_as = name;
        if self.pro_active() {
            self.pro.capture_setlist();
        } else {
            self.send(crate::Cmd::CaptureSetlist);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With nothing chosen the details are the loaded preset's tone, and they
    /// move with it; a tone chosen by a click stays chosen when another
    /// preset loads.
    #[test]
    fn the_details_follow_the_loaded_preset_until_a_tone_is_chosen() {
        let (to_device, _nowhere) = std::sync::mpsc::channel();
        let (_silent, from_device) = std::sync::mpsc::channel();
        let mut app = App::new(&egui::Context::default(), to_device, from_device);
        let tone = |name: &str| crate::LibEntry {
            hash: format!("{name}-hash"),
            series: format!("{name}-series"),
            name: name.to_owned(),
            line: String::new(),
            meta: library::Meta {
                name: name.to_owned(),
                ..Default::default()
            },
            added_at: String::new(),
            modified_at: String::new(),
            downloads: None,
            rating: None,
            version: 1,
            versions: 1,
            chain: Vec::new(),
            pro: false,
            marker: Some(crate::devices::Marker::hx("Stomp")),
            firmware: "3.80".to_owned(),
        };
        app.lib_entries = vec![
            tone("Glass Clean"),
            tone("Plexi Crunch"),
            tone("Brown Lead"),
        ];
        app.connection = crate::Connection::Online;
        app.device = "HX Stomp".into();
        let load = |app: &mut App, index: i64, name: &str| {
            app.preset_index = index;
            app.preset_name = name.to_owned();
            app.follow_loaded_tone();
        };

        load(&mut app, 1, "Plexi Crunch");
        assert_eq!(app.lib_selected, Some(1));
        assert_eq!(
            app.lib_draft.name, "Plexi Crunch",
            "its details are drafted"
        );
        load(&mut app, 2, "Brown Lead");
        assert_eq!(app.lib_selected, Some(2), "they move with the preset");

        app.select_lib_entry(0);
        load(&mut app, 1, "Plexi Crunch");
        assert_eq!(app.lib_selected, Some(0), "a chosen tone stays chosen");
    }

    fn slot(hash: &str, name: &str) -> library::Slot {
        library::Slot {
            hash: hash.to_owned(),
            name: name.to_owned(),
            file: String::new(),
        }
    }

    /// Slot by slot: the same preset, another one, one the pedal lacks, one
    /// the setlist leaves empty, and empty on both.
    #[test]
    fn a_setlist_is_compared_with_the_pedal_slot_by_slot() {
        let setlist = vec![
            slot("a", "Glass Clean"),
            slot("b", "Plexi Crunch"),
            slot("c", "Brown Lead"),
            library::Slot::default(),
            library::Slot::default(),
        ];
        let pedal = PedalSlots {
            hashes: BTreeMap::from([
                (0, "a".to_owned()),
                (1, "x".to_owned()),
                (3, "y".to_owned()),
            ]),
            names: vec![
                "Glass Clean".into(),
                "Plexi Crunch".into(),
                "New Preset".into(),
                "Night Verb".into(),
                String::new(),
            ],
        };
        let states = compare_slots(&setlist, &pedal);
        assert_eq!(
            states,
            vec![
                SlotState::Same,
                SlotState::Replaced,
                SlotState::Fills,
                SlotState::Clears,
                SlotState::Empty,
            ]
        );
        let counts = Differences::of(&states);
        assert_eq!(counts.differing(), 3);
        assert_eq!((counts.same, counts.empty), (1, 1));
    }

    /// The banner says what differs in words, and when nothing does.
    #[test]
    fn the_differences_are_said_in_words() {
        let (title, body) = difference_words(Differences {
            replaced: 4,
            clears: 2,
            same: 36,
            empty: 84,
            ..Default::default()
        });
        assert_eq!(title, "6 slots differ from the pedal.");
        assert_eq!(
            body,
            "Four hold other presets, two are empty in the setlist. The other 120 match."
        );
        let (title, _) = difference_words(Differences {
            same: 3,
            ..Default::default()
        });
        assert_eq!(title, "Matches the pedal.");
    }
}
