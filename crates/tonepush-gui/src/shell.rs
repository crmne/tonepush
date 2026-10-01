//! The frame every page sits in (docs/design/redesign-2026-10-01, "The
//! frame"): the sidebar with the pedal and its presets, the deck with the
//! loaded preset, and the switch between the Edit, Library and Pedal pages.
//!
//! The status bar and the floating device, preferences and EQ windows are
//! gone. The pedal's name, connection and firmware are the sidebar's device
//! card; what was behind them is the Pedal page; work under way and problems
//! are the deck's state line; the version and the update offer are in the
//! settings under the sidebar's foot.
//!
//! The pieces here that know nothing of either protocol (preset rows, the
//! deck's parts, the mini deck, page heads) are shared with the StompStation
//! PRO panel, so the two families read as one editor.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontId, Galley, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::theme::{self, Icon, Mood, Tier};
use crate::{session, App, Cmd, Connection};

/// The three pages: the loaded preset, the library, the pedal itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Edit,
    Library,
    Pedal,
}

impl Page {
    pub(crate) const ALL: [Page; 3] = [Page::Edit, Page::Library, Page::Pedal];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Page::Edit => "Edit",
            Page::Library => "Library",
            Page::Pedal => "Pedal",
        }
    }
}

/// The sidebar's width at each size.
pub(crate) fn sidebar_width(tier: Tier) -> f32 {
    tier.pick(216.0, 232.0, 272.0)
}

/// The deck's height at each size.
pub(crate) fn deck_height(tier: Tier) -> f32 {
    tier.pick(56.0, 64.0, 72.0)
}

/// The one-line deck over the Library and Pedal pages.
pub(crate) const MINI_DECK_HEIGHT: f32 = 52.0;
/// The sidebar's foot.
pub(crate) const FOOT_HEIGHT: f32 = 44.0;
/// One preset row.
pub(crate) const ROW_HEIGHT: f32 = 26.0;
/// A page head with a pedal's well, title and the line under it.
pub(crate) const PAGE_HEAD: f32 = 78.0;
/// The same, with a row of tabs under it.
pub(crate) const PAGE_HEAD_WITH_TABS: f32 = PAGE_HEAD + 37.0;
/// The gap above the first preset of a bank.
pub(crate) const BANK_GAP: f32 = 6.0;

// ---------------------------------------------------------------------------
// Text

/// One line of text, laid out.
pub(crate) fn galley(
    ui: &Ui,
    text: impl Into<String>,
    font: FontId,
    colour: Color32,
) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.into(), font, colour)
}

/// One line of text cut to `width` with an ellipsis.
pub(crate) fn elided(
    ui: &Ui,
    text: impl Into<String>,
    font: FontId,
    colour: Color32,
    width: f32,
) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, colour);
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// Paint a laid-out line with its left edge at `left`, centred on `y`.
pub(crate) fn paint_line(ui: &Ui, galley: Arc<Galley>, left: f32, y: f32) {
    let top = y - galley.size().y / 2.0;
    ui.painter()
        .galley(Pos2::new(left, top), galley, Color32::PLACEHOLDER);
}

/// A title, tightened the way the design sets large type (-0.015 em).
pub(crate) fn title_galley(
    ui: &Ui,
    text: &str,
    size: f32,
    colour: Color32,
    width: f32,
) -> Arc<Galley> {
    let mut job = theme::paint::spaced(text, theme::semibold(size), colour, -0.015);
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

// ---------------------------------------------------------------------------
// Sidebar pieces

/// What the sidebar shows of one preset slot.
#[derive(Clone, Debug, Default)]
pub(crate) struct PresetRow {
    pub index: usize,
    /// As the pedal labels it: 01A.
    pub slot: String,
    pub name: String,
    /// Nothing in it: an HX "New Preset", a free PRO slot.
    pub empty: bool,
    /// The preset loaded on the pedal.
    pub selected: bool,
    /// Loaded, and edited since it was saved.
    pub edited: bool,
    pub favourite: bool,
    /// Whether the library holds it: the same, under its name but different,
    /// or not at all.
    pub library: theme::Sync,
    /// The first slot of a bank after the first: a gap above it.
    pub bank_start: bool,
}

/// How the preset rows are drawn this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RowMode {
    #[default]
    Normal,
    /// A tone is on its way to the pedal: every row is a destination.
    Sending,
}

/// Whether an HX slot holds nothing: a blank name, or the factory default.
pub(crate) fn hx_slot_is_empty(name: &str) -> bool {
    let name = name.trim();
    name.is_empty() || name == "New Preset"
}

/// One row of the preset list, 26 points: slot label, name, and the marks at
/// the right (the edited dot, the favourite star, a check when the library
/// holds it unchanged, a compare mark when it differs). A selected row is a
/// neutral fill; amber is for the next action, so in send mode the free slots
/// are amber and the row under the pointer is the drop target.
pub(crate) fn preset_row(ui: &mut Ui, row: &PresetRow, mode: RowMode) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let sending = mode == RowMode::Sending;
    let hovered = response.hovered();
    let corner = CornerRadius::same(7);
    if sending && hovered {
        ui.painter().rect_filled(rect, corner, theme::accent_soft());
        theme::paint::dashed_rect(
            ui.painter(),
            rect.shrink(0.75),
            6.5,
            Stroke::new(1.5, theme::accent_line()),
            4.0,
            3.0,
        );
    } else if row.selected && !sending {
        ui.painter().rect_filled(rect, corner, theme::hover());
    } else if hovered {
        ui.painter().rect_filled(rect, corner, theme::raised());
    }

    let y = rect.center().y;
    let slot_ink = if row.selected && !sending {
        theme::text_soft()
    } else {
        theme::muted()
    };
    let slot = galley(ui, row.slot.clone(), theme::medium(theme::LABEL), slot_ink);
    paint_line(ui, slot, rect.left() + 8.0, y);

    // The marks, from the right edge in.
    let mut right = rect.right() - 8.0;
    if sending {
        if hovered {
            let hint = if row.empty { "Put it here" } else { "Replace" };
            let hint = galley(ui, hint, theme::regular(11.0), theme::muted());
            right -= hint.size().x;
            paint_line(ui, hint, right, y);
            right -= 6.0;
        }
    } else {
        match row.library {
            theme::Sync::Same => {
                theme::paint_icon(
                    ui,
                    Icon::Check,
                    Pos2::new(right - 6.5, y),
                    13.0,
                    theme::faint(),
                );
                right -= 13.0 + 4.0;
            }
            theme::Sync::Differs => {
                theme::paint_icon(
                    ui,
                    Icon::GitCompare,
                    Pos2::new(right - 6.5, y),
                    13.0,
                    theme::hot(),
                );
                right -= 13.0 + 4.0;
            }
            _ => {}
        }
        if row.favourite {
            theme::paint_icon(
                ui,
                Icon::Star,
                Pos2::new(right - 6.0, y),
                12.0,
                theme::accent(),
            );
            right -= 12.0 + 4.0;
        }
        if row.edited {
            let centre = Pos2::new(right - 3.5, y);
            ui.painter().circle_filled(centre, 6.5, theme::hot_soft());
            ui.painter().circle_filled(centre, 3.5, theme::hot());
            right -= 7.0 + 4.0;
        }
        right -= 4.0;
    }

    let (name, font, ink) = if sending && row.empty {
        (
            "Empty".to_owned(),
            theme::medium(theme::BODY),
            theme::accent(),
        )
    } else if row.empty {
        let name = if row.name.trim().is_empty() {
            "Empty".to_owned()
        } else {
            row.name.clone()
        };
        (name, theme::regular(theme::BODY), theme::faint())
    } else if row.selected && !sending {
        (row.name.clone(), theme::medium(theme::BODY), theme::text())
    } else if sending && hovered {
        (row.name.clone(), theme::regular(theme::BODY), theme::text())
    } else {
        (
            row.name.clone(),
            theme::regular(theme::BODY),
            theme::text_soft(),
        )
    };
    let left = rect.left() + 8.0 + 27.0 + 9.0;
    let name = elided(ui, name, font, ink, right - left);
    paint_line(ui, name, left, y);
    response
}

/// The preset list under the header: banks apart, scrolled, fading out at the
/// bottom while there is more. `row` draws and answers each row; a list with
/// nothing to show says `empty` instead.
pub(crate) fn preset_list(
    ui: &mut Ui,
    id: &str,
    empty: &str,
    rows: &[PresetRow],
    mut row: impl FnMut(&mut Ui, &PresetRow),
) {
    if rows.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width() - 12.0, 60.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(empty)
                                .font(theme::regular(12.5))
                                .color(theme::muted()),
                        )
                        .wrap(),
                    );
                },
            );
        });
        return;
    }
    let output = egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 2,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for preset in rows {
                        if preset.bank_start {
                            ui.add_space(BANK_GAP);
                        }
                        row(ui, preset);
                    }
                });
        });
    // More below: fade the last rows into the sidebar, as the design does.
    let shown = output.inner_rect;
    let more = output.state.offset.y + shown.height() < output.content_size.y - 1.0;
    if more {
        let fade = Rect::from_min_max(Pos2::new(shown.left(), shown.bottom() - 28.0), shown.max);
        theme::paint::gradient_rect(
            ui.painter(),
            fade,
            0.0,
            &[(0.0, theme::alpha(theme::bg(), 0.0)), (1.0, theme::bg())],
        );
    }
}

/// The pedal as the device card shows it.
pub(crate) struct DeviceCard {
    pub name: String,
    pub status: String,
    /// Connected and answering: the dot is green.
    pub online: bool,
}

/// The pedal: a drawing of one in a well, its name, how it is connected, and
/// a chevron that opens what can be done to the connection.
pub(crate) fn device_card(ui: &mut Ui, card: &DeviceCard) -> Response {
    ui.add_space(10.0);
    let full = ui.available_width();
    let (outer, _) = ui.allocate_exact_size(Vec2::new(full, 52.0), Sense::hover());
    ui.add_space(8.0);
    let rect = Rect::from_min_max(
        Pos2::new(outer.left() + 10.0, outer.top()),
        Pos2::new(outer.right() - 10.0, outer.bottom()),
    );
    let response = ui.interact(rect, ui.id().with("device-card"), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        if response.hovered() || open {
            theme::raised()
        } else {
            theme::panel()
        },
        Stroke::new(1.0, theme::line()),
        egui::StrokeKind::Inside,
    );
    let well = Rect::from_min_size(
        Pos2::new(rect.left() + 9.0, rect.center().y - 17.0),
        Vec2::splat(34.0),
    );
    ui.painter().rect(
        well,
        CornerRadius::same(9),
        theme::raised(),
        Stroke::new(1.0, theme::line_strong()),
        egui::StrokeKind::Inside,
    );
    theme::paint_icon(ui, Icon::Pedal, well.center(), 18.0, theme::text_soft());
    let left = well.right() + 10.0;
    let room = rect.right() - 30.0 - left;
    let name = elided(
        ui,
        card.name.clone(),
        theme::semibold(13.5),
        theme::text(),
        room,
    );
    paint_line(ui, name, left, rect.center().y - 8.0);
    let dot = Pos2::new(left + 3.0, rect.center().y + 8.5);
    ui.painter().circle_filled(
        dot,
        3.0,
        if card.online {
            theme::ok()
        } else {
            theme::faint()
        },
    );
    let status = elided(
        ui,
        card.status.clone(),
        theme::regular(11.5),
        theme::muted(),
        room - 12.0,
    );
    paint_line(ui, status, left + 12.0, rect.center().y + 8.5);
    theme::paint_icon(
        ui,
        Icon::ChevronsUpDown,
        Pos2::new(rect.right() - 17.0, rect.center().y),
        14.0,
        theme::muted(),
    );
    response
}

/// Edit, Library and Pedal: which page the window shows. Returns the page
/// asked for.
pub(crate) fn page_switch(ui: &mut Ui, page: Page) -> Option<Page> {
    let mut asked = None;
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let width = ui.available_width() - 10.0;
        ui.allocate_ui_with_layout(
            Vec2::new(width, 30.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let segments: Vec<theme::Segment> = Page::ALL
                    .iter()
                    .map(|page| theme::Segment::new(page.label()))
                    .collect();
                let chosen = Page::ALL.iter().position(|p| *p == page);
                if let Some(index) = theme::segmented(ui, "pages", &segments, chosen, true).clicked
                {
                    asked = Some(Page::ALL[index]);
                }
            },
        );
    });
    ui.add_space(6.0);
    asked
}

/// The amber card above the list while a tone waits for a slot. Returns
/// whether Cancel was pressed.
pub(crate) fn sending_card(ui: &mut Ui, name: &str) -> bool {
    let mut cancel = false;
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let width = ui.available_width() - 10.0;
        egui::Frame::new()
            .fill(theme::accent_soft())
            .stroke(Stroke::new(1.0, theme::accent_line()))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(10, 9))
            .show(ui, |ui| {
                ui.set_width(width - 22.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
                    let (icon, _) = ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::hover());
                    theme::paint_icon(ui, Icon::Download, icon.center(), 14.0, theme::text());
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Choose a slot for {name}."))
                                    .font(theme::semibold(12.0))
                                    .color(theme::text()),
                            )
                            .wrap(),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "Free slots are amber; a used one says what it replaces. \
                                     Esc cancels.",
                                )
                                .font(theme::regular(12.0))
                                .color(theme::text_soft()),
                            )
                            .wrap(),
                        );
                        ui.add_space(4.0);
                        cancel = theme::Button::new("Cancel")
                            .ghost()
                            .small()
                            .show(ui)
                            .clicked();
                    });
                });
            });
    });
    ui.add_space(4.0);
    cancel
}

/// What the presets header was asked.
#[derive(Default)]
pub(crate) struct HeaderAsked {
    pub favourites: bool,
    pub capture: bool,
}

/// PRESETS, how many slots the library holds (or, while sending, how many are
/// free), the favourites filter, and keeping the whole pedal as a setlist.
pub(crate) fn presets_header(
    ui: &mut Ui,
    aside: &str,
    aside_hint: &str,
    favourites_only: bool,
    can_capture: bool,
) -> HeaderAsked {
    let mut asked = HeaderAsked::default();
    ui.add_space(6.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
    // Everything sits on the row's lower edge, 6 points up.
    let y = rect.bottom() - 6.0 - 7.0;
    let caption = ui.painter().layout_job(theme::paint::spaced(
        "PRESETS",
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let caption_width = caption.size().x;
    paint_line(ui, caption, rect.left() + 16.0, y);

    let mut right = rect.right() - 10.0;
    let button = |ui: &mut Ui, right: f32, id: &str, icon: Icon, on: bool, enabled: bool| {
        let place = Rect::from_center_size(Pos2::new(right - 11.0, y + 1.0), Vec2::splat(22.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(place).id_salt(id));
        let mut button = theme::IconButton::new(icon)
            .side(22.0, 14.0)
            .on(on)
            .enabled(enabled);
        if on {
            button = button.tint(theme::accent());
        }
        button.show(&mut child)
    };
    asked.capture = button(ui, right, "capture", Icon::Computer, false, can_capture)
        .on_hover_text("Keep every preset on the pedal, in order, as a setlist")
        .on_disabled_hover_text("Connect the pedal to keep its presets as a setlist")
        .clicked();
    right -= 22.0 + 2.0;
    asked.favourites = button(
        ui,
        right,
        "favourites",
        if favourites_only {
            Icon::StarOn
        } else {
            Icon::Star
        },
        favourites_only,
        true,
    )
    .on_hover_text(if favourites_only {
        "Show every preset"
    } else {
        "Show favourites only"
    })
    .clicked();
    right -= 22.0 + 6.0;
    if !aside.is_empty() {
        let aside_galley = galley(ui, aside, theme::regular(11.5), theme::muted());
        let room = right - (rect.left() + 16.0 + caption_width + 8.0);
        if aside_galley.size().x <= room {
            let left = right - aside_galley.size().x;
            let hit = Rect::from_min_size(
                Pos2::new(left, y - 8.0),
                Vec2::new(aside_galley.size().x, 16.0),
            );
            paint_line(ui, aside_galley, left, y);
            if !aside_hint.is_empty() {
                ui.interact(hit, ui.id().with("presets-aside"), Sense::hover())
                    .on_hover_text(aside_hint);
            }
        }
    }
    asked
}

/// What the sidebar's foot says about the pedal's safety, or about TonePush
/// when no pedal is there.
pub(crate) struct Foot {
    /// `None` draws TonePush's own mark.
    pub icon: Option<Icon>,
    pub text: String,
    pub mood: Mood,
    pub hint: String,
}

/// What the foot was asked.
pub(crate) struct FootAsked {
    pub gear: Response,
    pub offer: bool,
}

/// The sidebar's foot: protection state on the left, an update offer when
/// there is one, and settings on the right.
pub(crate) fn sidebar_foot(ui: &mut Ui, foot: &Foot, update: Option<&str>) -> FootAsked {
    let rect = ui.max_rect();
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, theme::line()),
    );
    let y = rect.center().y;
    let left = rect.left() + 14.0;
    match foot.icon {
        Some(icon) => theme::paint_icon(ui, icon, Pos2::new(left + 7.0, y), 14.0, foot.mood.ink()),
        None => theme::paint_mark(
            ui,
            Rect::from_center_size(Pos2::new(left + 7.0, y), Vec2::splat(16.0)),
        ),
    }
    let gear_place =
        Rect::from_center_size(Pos2::new(rect.right() - 8.0 - 12.0, y), Vec2::splat(24.0));
    let gear = ui
        .scope_builder(
            egui::UiBuilder::new().max_rect(gear_place).id_salt("gear"),
            |ui| theme::IconButton::new(Icon::Gear).small().show(ui),
        )
        .inner
        .on_hover_text("Settings");
    let mut right = gear_place.left() - 6.0;
    let mut offer = false;
    if let Some(version) = update {
        let text = format!("{version} is out");
        let width = galley(ui, text.clone(), theme::semibold(11.0), theme::accent())
            .size()
            .x
            + 14.0;
        let place = Rect::from_min_size(Pos2::new(right - width, y - 10.0), Vec2::new(width, 20.0));
        offer = ui
            .scope_builder(
                egui::UiBuilder::new().max_rect(place).id_salt("update"),
                |ui| theme::Chip::new(&text).mood(Mood::Accent).show(ui),
            )
            .inner
            .on_hover_text("A new TonePush is out: open settings to update")
            .clicked();
        right = place.left() - 6.0;
    }
    let text_left = left + 14.0 + 9.0;
    let text = elided(
        ui,
        foot.text.clone(),
        theme::regular(12.0),
        theme::text_soft(),
        right - text_left,
    );
    let hit = Rect::from_min_size(
        Pos2::new(text_left, y - 9.0),
        Vec2::new(text.size().x, 18.0),
    );
    paint_line(ui, text, text_left, y);
    if !foot.hint.is_empty() {
        ui.interact(hit, ui.id().with("foot-text"), Sense::hover())
            .on_hover_text(&foot.hint);
    }
    FootAsked { gear, offer }
}

// ---------------------------------------------------------------------------
// Deck pieces

/// The slot chip at the start of a deck: 01B.
pub(crate) fn slot_chip(ui: &mut Ui, slot: &str) -> Response {
    let label = galley(ui, slot, theme::semibold(12.5), theme::text_soft());
    let size = Vec2::new((label.size().x + 14.0).max(38.0), 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        theme::raised(),
        Stroke::new(1.0, theme::line_strong()),
        egui::StrokeKind::Inside,
    );
    let width = label.size().x;
    paint_line(ui, label, rect.center().x - width / 2.0, rect.center().y);
    response
}

/// One part of the deck's state line, in its own voice.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    /// Edited and not saved, with the hot dot.
    Unsaved(String),
    /// Protected, verified: the green voice.
    Good(Icon, String),
    /// A warning in the hot voice: saving waits for a backup.
    Warning(Icon, String),
    /// Something that went wrong.
    Problem(String),
    /// A plain fact, muted.
    Note(String),
    /// Work in hand: a spinner and what it is.
    Busy(String),
}

/// What leads a part of the state line.
enum Lead {
    Dot(Color32),
    Icon(Icon),
    Spinner,
}

/// The deck's second line: the state in words, parts apart by a dot, cut to
/// `width`.
pub(crate) fn state_line(ui: &mut Ui, parts: &[State], width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 16.0), Sense::hover());
    let y = rect.center().y;
    let mut x = rect.left();
    for (index, part) in parts.iter().enumerate() {
        if x >= rect.right() - 12.0 {
            break;
        }
        if index > 0 {
            let dot = galley(ui, "·", theme::regular(12.0), theme::faint());
            let width = dot.size().x;
            paint_line(ui, dot, x + 6.0, y);
            x += width + 12.0;
        }
        let (lead, text, ink, medium) = match part {
            State::Unsaved(text) => (Some(Lead::Dot(theme::hot())), text, theme::hot(), true),
            State::Good(icon, text) => (Some(Lead::Icon(*icon)), text, theme::ok(), false),
            State::Warning(icon, text) => (Some(Lead::Icon(*icon)), text, theme::hot(), true),
            State::Problem(text) => (
                Some(Lead::Icon(Icon::CircleAlert)),
                text,
                theme::danger(),
                false,
            ),
            State::Note(text) => (None, text, theme::muted(), false),
            State::Busy(text) => (Some(Lead::Spinner), text, theme::muted(), false),
        };
        match lead {
            Some(Lead::Dot(colour)) => {
                ui.painter()
                    .circle_filled(Pos2::new(x + 3.0, y), 3.0, colour);
                x += 6.0 + 6.0;
            }
            Some(Lead::Icon(icon)) => {
                theme::paint_icon(ui, icon, Pos2::new(x + 6.5, y), 13.0, ink);
                x += 13.0 + 5.0;
            }
            Some(Lead::Spinner) => {
                spin(ui, Pos2::new(x + 6.0, y), 4.5);
                x += 12.0 + 6.0;
            }
            None => {}
        }
        let font = if medium {
            theme::medium(12.0)
        } else {
            theme::regular(12.0)
        };
        let words = elided(ui, text.clone(), font, ink, rect.right() - x);
        let width = words.size().x;
        paint_line(ui, words, x, y);
        x += width;
    }
    response
}

/// A small amber spinner. Repaints are paced, not continuous.
pub(crate) fn spin(ui: &Ui, centre: Pos2, radius: f32) {
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let turn = std::f32::consts::TAU;
    let start = (ui.input(|input| input.time) as f32 * turn) % turn;
    ui.painter().circle_stroke(
        centre,
        radius,
        Stroke::new(1.6, theme::alpha(theme::accent(), 0.2)),
    );
    theme::paint::arc(
        ui.painter(),
        centre,
        radius,
        start,
        std::f32::consts::PI * 1.2,
        Stroke::new(1.6, theme::accent()),
    );
}

/// A thin vertical rule between groups in a row of controls.
pub(crate) fn rule(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 20.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0, theme::line()),
    );
}

/// The deck's title: the preset's name, click to rename. Returns a new name
/// when one was typed and confirmed.
pub(crate) fn deck_title(
    ui: &mut Ui,
    name: &str,
    size: f32,
    width: f32,
    renaming: &mut Option<String>,
    can_rename: bool,
    rename_refused: &str,
) -> Option<String> {
    if let Some(draft) = renaming.as_mut() {
        let field = ui.add(
            egui::TextEdit::singleline(draft)
                .desired_width(width.min(320.0))
                .font(theme::semibold(size - 3.0)),
        );
        if !field.has_focus() && !field.lost_focus() {
            field.request_focus();
        }
        if field.lost_focus() {
            let commit = ui.input(|input| input.key_pressed(egui::Key::Enter));
            let typed = draft.trim().to_owned();
            *renaming = None;
            if commit && !typed.is_empty() {
                return Some(typed);
            }
        }
        return None;
    }
    let title = title_galley(ui, name, size, theme::text(), width);
    let sense = if can_rename {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(title.size().x, size + 4.0), sense);
    paint_line(ui, title, rect.left(), rect.center().y);
    let response = if can_rename {
        response.on_hover_text("Click the name to rename the preset")
    } else if rename_refused.is_empty() {
        response
    } else {
        response.on_hover_text(rename_refused)
    };
    if can_rename && response.clicked() {
        *renaming = Some(name.to_owned());
    }
    None
}

/// The tempo: the reading in tabular figures with BPM after it, typed into
/// on a click, and Tap after it. Laid out right to left, as the deck's end is:
/// Tap first, at the right. Returns a tempo to set.
pub(crate) fn tempo(
    ui: &mut Ui,
    bpm: f32,
    draft: &mut Option<String>,
    taps: &mut Vec<std::time::Instant>,
) -> Option<f32> {
    let mut changed = None;
    if theme::Button::new("Tap")
        .small()
        .show(ui)
        .on_hover_text("Tap in time to set the tempo")
        .clicked()
    {
        if let Some(bpm) = tap(taps, std::time::Instant::now()) {
            changed = Some(bpm);
        }
    }
    match draft {
        Some(text) => {
            let edit = ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(56.0)
                    .font(theme::semibold(14.0)),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            if edit.lost_focus() {
                if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    changed = text.trim().parse().ok();
                }
                *draft = None;
            }
        }
        None => {
            let value = title_galley(ui, &format!("{bpm:.1}"), 16.0, theme::text(), 200.0);
            let unit = ui.painter().layout_job(theme::paint::spaced(
                "BPM",
                theme::semibold(11.0),
                theme::muted(),
                0.04,
            ));
            let size = Vec2::new(value.size().x + 5.0 + unit.size().x, 26.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            // On one baseline, as the design sets them: each line is placed
            // by its own row's baseline.
            let baseline = rect.center().y + 6.0;
            let value_width = value.size().x;
            let value_top = baseline - baseline_of(&value);
            let unit_top = baseline - baseline_of(&unit);
            ui.painter().galley(
                Pos2::new(rect.left(), value_top),
                value,
                Color32::PLACEHOLDER,
            );
            ui.painter().galley(
                Pos2::new(rect.left() + value_width + 5.0, unit_top),
                unit,
                Color32::PLACEHOLDER,
            );
            if response.on_hover_text("Click to type a tempo").clicked() {
                *draft = Some(format!("{bpm:.1}"));
            }
        }
    }
    changed
}

/// How far below a laid-out line's top its baseline sits.
pub(crate) fn baseline_of(galley: &Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
        .unwrap_or(galley.size().y * 0.8)
}

/// Count a tap, and the tempo the taps so far make, once there are two.
/// Taps more than two seconds apart start again; the last five count.
pub(crate) fn tap(taps: &mut Vec<std::time::Instant>, now: std::time::Instant) -> Option<f32> {
    if taps
        .last()
        .is_some_and(|previous| now.duration_since(*previous).as_secs_f32() > 2.0)
    {
        taps.clear();
    }
    taps.push(now);
    if taps.len() > 5 {
        taps.remove(0);
    }
    if taps.len() < 2 {
        return None;
    }
    let span = taps
        .last()
        .expect("two taps checked")
        .duration_since(taps[0])
        .as_secs_f32();
    let bpm = 60.0 * (taps.len() - 1) as f32 / span;
    (20.0..=999.0).contains(&bpm).then_some(bpm)
}

/// What the deck's actions were asked.
#[derive(Default)]
pub(crate) struct DeckAsked {
    pub undo: bool,
    pub redo: bool,
    pub discard: bool,
    pub save: bool,
    /// The backup that unlocks saving, asked for in Save's place.
    pub unlock: bool,
}

/// What the end of a deck needs to know.
pub(crate) struct DeckActions<'a> {
    pub tier: Tier,
    /// Talking to the pedal: undo, redo and discard are possible.
    pub live: bool,
    pub undo: usize,
    pub redo: usize,
    pub dirty: bool,
    pub can_save: bool,
    /// Why Save is unavailable, when it is.
    pub save_refused: &'a str,
    /// Saving waits for a backup this deck can take: the backup takes
    /// Save's place, enabled when nothing else is talking to the pedal.
    pub unlock: Option<bool>,
}

/// Discard, undo, redo and Save, the way every deck ends. Laid out right to
/// left, so call it inside a right-to-left layout: Save lands at the edge.
/// Save is the amber primary only when there is something to save.
pub(crate) fn deck_actions(ui: &mut Ui, actions: &DeckActions) -> DeckAsked {
    let mut asked = DeckAsked::default();
    let shortcut = |modifiers, key| {
        ui.ctx()
            .format_shortcut(&egui::KeyboardShortcut::new(modifiers, key))
    };
    let save_key = shortcut(egui::Modifiers::COMMAND, egui::Key::S);
    let undo_key = shortcut(egui::Modifiers::COMMAND, egui::Key::Z);
    let redo_key = shortcut(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    if let Some(ready) = actions.unlock {
        asked.unlock = theme::Button::new("Back up to unlock saving")
            .primary()
            .icon(Icon::Download)
            .enabled(ready)
            .show(ui)
            .on_hover_text(
                "Read the whole pedal into a checked backup on this computer; saving, \
                 renaming and importing are on once it matches",
            )
            .on_disabled_hover_text("The pedal is busy")
            .clicked();
    } else if actions.dirty {
        let mut save = theme::Button::new("Save")
            .primary()
            .enabled(actions.can_save);
        if actions.tier != Tier::S {
            save = save.hint(&save_key);
        }
        asked.save = save
            .show(ui)
            .on_hover_text(format!("Write these changes into the preset ({save_key})"))
            .on_disabled_hover_text(actions.save_refused)
            .clicked();
    } else {
        theme::Button::new("Saved")
            .icon(Icon::Check)
            .enabled(false)
            .show(ui)
            .on_disabled_hover_text("Nothing to save");
    }
    asked.redo = theme::IconButton::new(Icon::Redo)
        .enabled(actions.live && actions.redo > 0)
        .show(ui)
        .on_hover_text(format!("Redo ({redo_key})"))
        .on_disabled_hover_text("Nothing to redo")
        .clicked();
    asked.undo = theme::IconButton::new(Icon::Undo)
        .enabled(actions.live && actions.undo > 0)
        .show(ui)
        .on_hover_text(format!("Undo ({undo_key})"))
        .on_disabled_hover_text("Nothing to undo")
        .clicked();
    if actions.dirty {
        asked.discard = theme::IconButton::new(Icon::RotateCcw)
            .enabled(actions.live)
            .show(ui)
            .on_hover_text("Discard the changes: load the stored preset again")
            .clicked();
    }
    asked
}

/// One block of the mini chain: its category colour, or a dashed square when
/// it is off; a parallel stretch stacks its lanes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mini {
    Block { colour: Color32, on: bool },
    Stack(Vec<(Color32, bool)>),
}

/// A preset's chain as a strip of category colours.
pub(crate) fn mini_chain(ui: &mut Ui, chain: &[Mini]) -> Response {
    mini_chain_sized(ui, chain, 14.0, 7.0)
}

/// The same strip at another size: `square` points a block, `wire` between.
pub(crate) fn mini_chain_sized(ui: &mut Ui, chain: &[Mini], square: f32, wire: f32) -> Response {
    const LANE_GAP: f32 = 3.0;
    let width = chain.len() as f32 * square + chain.len().saturating_sub(1) as f32 * wire;
    let tallest = chain
        .iter()
        .map(|item| match item {
            Mini::Block { .. } => 1,
            Mini::Stack(lanes) => lanes.len().max(1),
        })
        .max()
        .unwrap_or(1) as f32;
    let height = tallest * square + (tallest - 1.0) * LANE_GAP;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let mid = rect.center().y;
    let block = |x: f32, y: f32, colour: Color32, on: bool| {
        let r = Rect::from_min_size(Pos2::new(x, y - square / 2.0), Vec2::splat(square));
        if on {
            painter.rect(
                r,
                CornerRadius::same(4),
                theme::mix(theme::panel(), colour, 0.3),
                Stroke::new(1.5, colour),
                egui::StrokeKind::Inside,
            );
        } else {
            theme::paint::dashed_rect(
                painter,
                r.shrink(0.75),
                3.25,
                Stroke::new(1.5, theme::faint()),
                2.5,
                2.0,
            );
        }
    };
    let mut x = rect.left();
    for (index, item) in chain.iter().enumerate() {
        if index > 0 {
            painter.hline((x - wire)..=x, mid, Stroke::new(2.0, theme::wire()));
        }
        match item {
            Mini::Block { colour, on } => block(x, mid, *colour, *on),
            Mini::Stack(lanes) => {
                let count = lanes.len() as f32;
                let total = count * square + (count - 1.0) * LANE_GAP;
                let mut y = mid - total / 2.0 + square / 2.0;
                for (colour, on) in lanes {
                    block(x, y, *colour, *on);
                    y += square + LANE_GAP;
                }
            }
        }
        x += square + wire;
    }
    response
}

/// A chain as a strip of category colours, in the order the signal meets
/// the blocks, a parallel stretch stacked. `colour` says what a block is drawn
/// in, or `None` to leave it out (the endpoints and junctions).
pub(crate) fn minis(
    chain: &[crate::session::Block],
    layout: &hx_proto::preset::Layout,
    colour: impl Fn(&crate::session::Block) -> Option<Color32>,
) -> Vec<Mini> {
    let block = |slot: usize| -> Option<(Color32, bool)> {
        let block = chain.iter().find(|b| b.position == slot as i64)?;
        colour(block).map(|colour| (colour, block.enabled))
    };
    let mut strip = Vec::new();
    for path in &layout.paths {
        for slot in &path.head {
            if let Some((colour, on)) = block(*slot) {
                strip.push(Mini::Block { colour, on });
            }
        }
        let longest = path
            .lanes
            .iter()
            .map(|lane| lane.blocks.len())
            .max()
            .unwrap_or(0);
        for column in 0..longest {
            let lanes: Vec<(Color32, bool)> = path
                .lanes
                .iter()
                .filter_map(|lane| lane.blocks.get(column))
                .filter_map(|slot| block(*slot))
                .collect();
            match lanes.len() {
                0 => {}
                1 => strip.push(Mini::Block {
                    colour: lanes[0].0,
                    on: lanes[0].1,
                }),
                _ => strip.push(Mini::Stack(lanes)),
            }
        }
        for slot in &path.tail {
            if let Some((colour, on)) = block(*slot) {
                strip.push(Mini::Block { colour, on });
            }
        }
    }
    strip
}

/// What the mini deck shows of the loaded preset.
pub(crate) struct MiniDeck<'a> {
    pub slot: &'a str,
    pub name: &'a str,
    pub dirty: bool,
    pub chain: &'a [Mini],
    pub snapshot: Option<&'a str>,
    pub can_save: bool,
    pub save_refused: &'a str,
    /// The sidebar is hidden: lead with the button that brings it back.
    pub sidebar_hidden: bool,
}

/// What the mini deck was asked.
#[derive(Default)]
pub(crate) struct MiniAsked {
    pub save: bool,
    pub edit: bool,
    pub show_sidebar: bool,
}

/// The 52-point deck the Library and Pedal pages keep the loaded preset in:
/// slot, name, Edited, its chain in colours, the snapshot, Save and Edit.
pub(crate) fn mini_deck(ui: &mut Ui, deck: &MiniDeck) -> MiniAsked {
    let mut asked = MiniAsked::default();
    let full = ui.max_rect();
    let right = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(full.with_max_x(full.right() - 16.0))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                asked.edit = theme::Button::new("Edit")
                    .ghost()
                    .small()
                    .icon(Icon::ArrowUpRight)
                    .show(ui)
                    .on_hover_text("Edit the loaded preset")
                    .clicked();
                if deck.dirty {
                    asked.save = theme::Button::new("Save")
                        .primary()
                        .small()
                        .enabled(deck.can_save)
                        .show(ui)
                        .on_hover_text("Write these changes into the preset")
                        .on_disabled_hover_text(deck.save_refused)
                        .clicked();
                }
            },
        )
        .response
        .rect;
    let left_rect = Rect::from_min_max(
        Pos2::new(
            full.left() + if deck.sidebar_hidden { 12.0 } else { 20.0 },
            full.top(),
        ),
        Pos2::new(right.left() - 12.0, full.bottom()),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(left_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if deck.sidebar_hidden {
                asked.show_sidebar = sidebar_toggle(ui);
            }
            slot_chip(ui, deck.slot);
            let name = elided(
                ui,
                deck.name,
                theme::semibold(14.0),
                theme::text(),
                (ui.available_width() * 0.4).max(80.0),
            );
            let (rect, _) = ui.allocate_exact_size(Vec2::new(name.size().x, 20.0), Sense::hover());
            paint_line(ui, name, rect.left(), rect.center().y);
            if deck.dirty {
                theme::Chip::new("Edited").mood(Mood::Hot).dot().show(ui);
            }
            if !deck.chain.is_empty() && ui.available_width() > 160.0 {
                ui.add_space(-6.0);
                mini_chain(ui, deck.chain);
            }
            if let Some(snapshot) = deck.snapshot {
                if ui.available_width() > 90.0 {
                    ui.add_space(-8.0);
                    theme::Chip::new(snapshot)
                        .mood(Mood::Ghost)
                        .icon(Icon::History)
                        .show(ui)
                        .on_hover_text("The snapshot on the pedal");
                }
            }
        },
    );
    asked
}

/// The button that brings the hidden sidebar back.
pub(crate) fn sidebar_toggle(ui: &mut Ui) -> bool {
    let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND,
        egui::Key::B,
    ));
    theme::IconButton::new(Icon::PanelLeft)
        .show(ui)
        .on_hover_text(format!("Show the sidebar ({shortcut})"))
        .clicked()
}

/// A page's head: an optional icon well, the title and a line under it, and
/// room on the right for the page's own controls (laid out right to left).
pub(crate) fn page_head(
    ui: &mut Ui,
    icon: Option<Icon>,
    title: &str,
    subtitle: &str,
    right: impl FnOnce(&mut Ui),
) {
    let width = ui.available_width();
    let height = if icon.is_some() { PAGE_HEAD } else { 62.0 };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 20.0, rect.top() + 18.0),
        Pos2::new(rect.right() - 16.0, rect.bottom() - 14.0),
    );
    // A child of its own rather than a scope: a scope would move the page's
    // cursor back up to wherever the controls end.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .id_salt("page-head-controls")
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 12.0;
    right(&mut child);
    let controls = child.min_rect();
    let room_right = if controls.width() > 0.0 {
        controls.left() - 12.0
    } else {
        inner.right()
    };
    let mut x = inner.left();
    let y = inner.center().y;
    if let Some(icon) = icon {
        let well = Rect::from_min_size(Pos2::new(x, y - 20.0), Vec2::splat(40.0));
        ui.painter().rect(
            well,
            CornerRadius::same(11),
            theme::raised(),
            Stroke::new(1.0, theme::line_strong()),
            egui::StrokeKind::Inside,
        );
        theme::paint_icon(ui, icon, well.center(), 20.0, theme::text_soft());
        x = well.right() + 12.0;
    }
    let mut job = theme::paint::spaced(
        title,
        theme::semibold(theme::PAGE_TITLE),
        theme::text(),
        -0.02,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (room_right - x).max(40.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let title = ui.painter().layout_job(job);
    if subtitle.is_empty() {
        paint_line(ui, title, x, y);
    } else {
        paint_line(ui, title, x, y - 9.0);
        let sub = elided(
            ui,
            subtitle,
            theme::regular(12.5),
            theme::muted(),
            room_right - x,
        );
        paint_line(ui, sub, x, y + 12.0);
    }
}

/// The settings popover's id, for the gear and the update offer that both
/// open it.
pub(crate) fn settings_popup() -> egui::Id {
    egui::Id::new("tonepush-settings")
}

/// "14:02" when it was today, "4 Oct" before; and whether it was today.
pub(crate) fn clock(time: std::time::SystemTime) -> Option<(String, bool)> {
    let stamp = jiff::Timestamp::try_from(time).ok()?;
    let zone = jiff::tz::TimeZone::system();
    let then = stamp.to_zoned(zone.clone());
    let today = jiff::Timestamp::now().to_zoned(zone).date();
    Some(if then.date() == today {
        (then.strftime("%H:%M").to_string(), true)
    } else {
        (then.strftime("%-d %b").to_string(), false)
    })
}

/// "Backed up at 14:02", or "on 4 Oct" when it was not today.
pub(crate) fn when_words(prefix: &str, time: std::time::SystemTime) -> String {
    match clock(time) {
        Some((when, true)) => format!("{prefix} at {when}"),
        Some((when, false)) => format!("{prefix} on {when}"),
        None => prefix.to_owned(),
    }
}

/// "presets" to "Presets".
pub(crate) fn sentence_case(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// The frame, for the app

impl App {
    /// How many slots one bank holds on the connected HX.
    pub(crate) fn presets_per_bank(&self) -> usize {
        hx_proto::PROFILES
            .iter()
            .find(|profile| profile.name == self.device.trim())
            .map_or(3, |profile| usize::from(profile.presets_per_bank))
            .max(1)
    }

    /// How many presets the HX holds, named or not.
    pub(crate) fn hx_total(&self) -> usize {
        if self.presets.is_empty() {
            self.preset_count as usize
        } else {
            self.presets.len()
        }
    }

    /// The rows of the HX preset list, as the sidebar draws them.
    pub(crate) fn hx_rows(&self) -> Vec<PresetRow> {
        let per_bank = self.presets_per_bank();
        let mut rows: Vec<PresetRow> = (0..self.hx_total())
            .filter(|&index| {
                !self.show_favorites_only || self.config.is_favorite(self.setlist, index as i64)
            })
            .map(|index| {
                let name = self.presets.get(index).cloned().unwrap_or_default();
                let selected = index as i64 == self.preset_index;
                PresetRow {
                    index,
                    slot: self.active_slot_label(index as i64),
                    empty: hx_slot_is_empty(&name),
                    name,
                    selected,
                    edited: selected && self.dirty,
                    favourite: self.config.is_favorite(self.setlist, index as i64),
                    library: self.slot_sync(index as i64),
                    bank_start: index % per_bank == 0,
                }
            })
            .collect();
        // No gap above the first row shown, and none in a filtered list.
        for (position, row) in rows.iter_mut().enumerate() {
            if position == 0 || self.show_favorites_only {
                row.bank_start = false;
            }
        }
        rows
    }

    /// The sidebar, full height on the left: the pedal, the page switch, its
    /// presets and the foot. Ctrl+B hides it.
    pub(crate) fn sidebar(&mut self, root: &mut Ui, tier: Tier) {
        if self.sidebar_hidden {
            return;
        }
        egui::Panel::left("sidebar")
            .exact_size(sidebar_width(tier))
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                let full = ui.max_rect();
                let foot = Rect::from_min_max(
                    Pos2::new(full.left(), full.bottom() - FOOT_HEIGHT),
                    full.max,
                );
                let body = Rect::from_min_max(full.min, Pos2::new(full.right(), foot.top()));
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(body)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                    |ui| {
                        ui.set_clip_rect(body);
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        self.device_card_ui(ui);
                        if let Some(page) = page_switch(ui, self.page) {
                            self.go_to(page);
                        }
                        if self.pro_active() {
                            let sending = self.sending.as_ref().map(|sending| sending.name.clone());
                            let picked = self.pro.sidebar(
                                ui,
                                &self.library_lookup,
                                &mut self.config,
                                sending.as_deref(),
                            );
                            match picked {
                                Some(crate::pro::Picked::Slot(slot)) => {
                                    self.finish_sending(slot as i64);
                                }
                                Some(crate::pro::Picked::Cancel) => self.sending = None,
                                None => {}
                            }
                        } else {
                            self.hx_sidebar(ui);
                        }
                    },
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(foot), |ui| {
                    self.foot_ui(ui);
                });
            });
    }

    /// Show another page. What the Pedal page shows is read as it opens.
    pub(crate) fn go_to(&mut self, page: Page) {
        if page == self.page {
            return;
        }
        // Leaving the Edit page keeps whatever the model browser was playing.
        self.close_browser(true);
        self.page = page;
        if page == Page::Pedal {
            self.read_pedal_page();
        }
    }

    /// Ask the HX for what its page shows: its settings and favourite blocks.
    pub(crate) fn read_pedal_page(&mut self) {
        if matches!(self.connection, Connection::Online) && !self.pro_active() {
            self.send(Cmd::ReadSettings);
            self.send(Cmd::ListFavourites);
        }
    }

    fn device_card_ui(&mut self, ui: &mut Ui) {
        let card = if self.pro_active() {
            self.pro.device_card()
        } else {
            match self.connection {
                Connection::Online => DeviceCard {
                    name: self.device.clone(),
                    status: if self.firmware.is_empty() {
                        "Connected".to_owned()
                    } else {
                        format!("Connected · {}", self.firmware)
                    },
                    online: true,
                },
                Connection::Connecting => DeviceCard {
                    name: "No pedal".to_owned(),
                    status: "Looking on USB…".to_owned(),
                    online: false,
                },
                Connection::Offline => DeviceCard {
                    name: "No pedal".to_owned(),
                    status: "Not connected".to_owned(),
                    online: false,
                },
            }
        };
        let response = device_card(ui, &card);
        let mut pedal_page = false;
        let mut let_go = false;
        let mut look = false;
        egui::Popup::menu(&response)
            .align(egui::RectAlign::BOTTOM_START)
            .gap(4.0)
            .show(|ui| {
                theme::menu_width(ui, 228.0);
                theme::menu_header(
                    ui,
                    &card.name,
                    Some(if card.online {
                        "connected"
                    } else {
                        "not connected"
                    }),
                );
                if card.online {
                    pedal_page =
                        theme::menu_item(ui, Some(Icon::HardDrive), "Open the pedal page", None)
                            .clicked();
                    theme::menu_separator(ui);
                    let_go = theme::menu_item(ui, Some(Icon::Power), "Let the pedal go", None)
                        .on_hover_text("Disconnect, so another editor can use the pedal")
                        .clicked();
                } else {
                    look =
                        theme::menu_item(ui, Some(Icon::Usb), "Look for a pedal", None).clicked();
                }
            });
        if pedal_page {
            self.go_to(Page::Pedal);
        }
        if let_go {
            self.let_go();
        }
        if look {
            self.look_for_pedal();
        }
    }

    /// Let the pedal go, so another editor can have it.
    pub(crate) fn let_go(&mut self) {
        self.status.clear();
        if self.pro_active() {
            self.pro.disconnect();
        } else {
            self.send(Cmd::Disconnect);
        }
    }

    /// Look on USB for either family.
    pub(crate) fn look_for_pedal(&mut self) {
        self.status.clear();
        if !matches!(self.connection, Connection::Online) {
            self.connection = Connection::Connecting;
            self.send(Cmd::Connect);
        }
        self.pro.reconnect();
    }

    fn hx_sidebar(&mut self, ui: &mut Ui) {
        if let Some(name) = self.sending.as_ref().map(|sending| sending.name.clone()) {
            if sending_card(ui, &name) {
                self.sending = None;
            }
        }
        let live = matches!(self.connection, Connection::Online);
        let total = self.hx_total();
        let (aside, hint) = if total == 0 {
            (String::new(), String::new())
        } else if self.sending.is_some() {
            let free = (0..total)
                .filter(|&index| {
                    hx_slot_is_empty(self.presets.get(index).map_or("", String::as_str))
                })
                .count();
            (
                format!("{free} free"),
                "Slots that hold nothing yet".to_owned(),
            )
        } else {
            let kept = (0..total as i64)
                .filter(|&index| {
                    matches!(
                        self.slot_sync(index),
                        theme::Sync::Same | theme::Sync::Differs
                    )
                })
                .count();
            (
                format!("{kept} of {total}"),
                format!("{kept} of the pedal's {total} presets are in your library"),
            )
        };
        let asked = presets_header(ui, &aside, &hint, self.show_favorites_only, live);
        if asked.favourites {
            self.show_favorites_only = !self.show_favorites_only;
        }
        if asked.capture {
            self.capture_pedal(None);
        }
        self.hx_preset_list(ui);
    }

    fn hx_preset_list(&mut self, ui: &mut Ui) {
        let rows = self.hx_rows();
        let mode = if self.sending.is_some() {
            RowMode::Sending
        } else {
            RowMode::Normal
        };
        let empty = if self.show_favorites_only && self.hx_total() > 0 {
            "No favourites yet. Add one from a preset's menu."
        } else {
            "The pedal's presets appear here, in its own banks, as soon as it connects."
        };
        let mut load = None;
        let mut pick = None;
        let mut toggle = None;
        let mut rename_start = None;
        let mut rename_result: Option<Option<(i64, String)>> = None;
        let mut action = None;
        let clipboard = self.clipboard.as_ref().map(|(name, _)| name.clone());
        let mut reveal = self.reveal_preset;
        let mut renaming = self.renaming.clone();
        preset_list(ui, "hx-presets", empty, &rows, |ui, row| {
            let index = row.index as i64;
            if mode == RowMode::Normal {
                if let Some((editing, draft)) = renaming.as_mut() {
                    if *editing == index {
                        let edit = ui.add_sized(
                            Vec2::new(ui.available_width(), ROW_HEIGHT),
                            egui::TextEdit::singleline(draft).hint_text("preset name"),
                        );
                        if !edit.has_focus() && !edit.lost_focus() {
                            edit.request_focus();
                        }
                        if edit.lost_focus() {
                            rename_result = Some(
                                ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    .then(|| (index, draft.clone())),
                            );
                        }
                        return;
                    }
                }
            }
            let response = preset_row(ui, row, mode);
            if row.selected && reveal {
                response.scroll_to_me(Some(egui::Align::Center));
                reveal = false;
            }
            if mode == RowMode::Sending {
                if response.clicked() {
                    pick = Some(index);
                }
                return;
            }
            if response.clicked() {
                load = Some(index);
            }
            response.context_menu(|ui| {
                theme::menu_width(ui, 256.0);
                theme::menu_header(
                    ui,
                    &format!("{} {}", row.slot, row.name),
                    Some("on the pedal"),
                );
                if theme::menu_item(ui, Some(Icon::TextCursorInput), "Rename", None).clicked() {
                    rename_start = Some((index, row.name.clone()));
                }
                if theme::menu_item(ui, Some(Icon::Copy), "Copy", None).clicked() {
                    action = Some((index, crate::RowAction::Copy));
                }
                match &clipboard {
                    Some(copied) => {
                        if theme::menu_item(
                            ui,
                            Some(Icon::Paste),
                            &format!("Paste {copied} here"),
                            None,
                        )
                        .clicked()
                        {
                            action = Some((index, crate::RowAction::Paste));
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
                    theme::Sync::Differs => {
                        if theme::menu_item(
                            ui,
                            Some(Icon::Computer),
                            "Update in library",
                            Some("differs"),
                        )
                        .clicked()
                        {
                            action = Some((index, crate::RowAction::Update));
                        }
                    }
                    theme::Sync::Same => {
                        theme::menu_disabled(ui, Some(Icon::Computer), "In your library", None);
                    }
                    _ => {
                        if theme::menu_item(ui, Some(Icon::Computer), "Keep in library", None)
                            .clicked()
                        {
                            action = Some((index, crate::RowAction::Keep));
                        }
                    }
                }
                if theme::menu_item(ui, Some(Icon::Download), "Save to file…", None).clicked() {
                    action = Some((index, crate::RowAction::Export));
                }
                if theme::menu_item(ui, Some(Icon::Upload), "Load from file…", None).clicked() {
                    action = Some((index, crate::RowAction::Import));
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
                // Last, alone, in red: it empties the slot in flash and undo
                // does not reach it. It asks before it does.
                if theme::menu_danger(ui, Icon::Remove, "Empty this slot…").clicked() {
                    action = Some((index, crate::RowAction::Remove));
                }
            });
        });
        self.reveal_preset = reveal;
        if rename_result.is_none() {
            // The draft as typed this frame.
            self.renaming = renaming;
        }
        if let Some(index) = toggle {
            self.config.toggle_favorite(self.setlist, index);
        }
        if let Some(index) = load {
            // Selecting a preset throws away whatever is in the edit buffer.
            // That is the device's rule, not ours, and it costs a person their
            // unsaved work in silence, so it asks first.
            self.request_preset(index);
        }
        if let Some((index, action)) = action {
            self.row_action(index, action);
        }
        if let Some(started) = rename_start {
            self.renaming = Some(started);
        }
        if let Some(result) = rename_result {
            self.renaming = None;
            if let Some((index, name)) = result {
                self.send(Cmd::Rename { index, name });
            }
        }
        if let Some(slot) = pick {
            self.finish_sending(slot);
        }
        // Escape gets out of picking the way it gets out of everything else.
        if self.sending.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sending = None;
        }
    }

    /// What the foot says: protection with a pedal, TonePush without one.
    fn foot(&self) -> Foot {
        if self.pro_active() && (self.pro.is_online() || self.pro.updating()) {
            return self.pro.foot();
        }
        if !matches!(self.connection, Connection::Online) {
            return Foot {
                icon: None,
                text: format!("TonePush {}", crate::update::VERSION),
                mood: Mood::Neutral,
                hint: String::new(),
            };
        }
        match self.backup_of_this_pedal() {
            Some(manifest) => {
                let time =
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(manifest.captured);
                Foot {
                    icon: Some(Icon::ShieldCheck),
                    text: when_words("Backed up", time),
                    mood: Mood::Ok,
                    hint: "TonePush reads the whole pedal when it connects and keeps that copy \
                           current after every save"
                        .to_owned(),
                }
            }
            None => Foot {
                icon: Some(Icon::Shield),
                text: "Not backed up yet".to_owned(),
                mood: Mood::Neutral,
                hint: "TonePush backs the pedal up as soon as it connects".to_owned(),
            },
        }
    }

    /// The automatic backup, when it is of the pedal connected now.
    pub(crate) fn backup_of_this_pedal(&self) -> Option<&hx_usb::backup::Manifest> {
        self.automatic_backup
            .as_ref()
            .filter(|manifest| manifest.device.trim() == self.device.trim())
    }

    fn foot_ui(&mut self, ui: &mut Ui) {
        let foot = self.foot();
        let update = self.updates.available().map(str::to_owned);
        let asked = sidebar_foot(ui, &foot, update.as_deref());
        if asked.offer {
            egui::Popup::toggle_id(ui.ctx(), settings_popup());
        }
        egui::Popup::from_toggle_button_response(&asked.gear)
            .id(settings_popup())
            .align(egui::RectAlign::TOP_START)
            .gap(6.0)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .frame(
                egui::Frame::new()
                    .fill(theme::panel())
                    .stroke(Stroke::new(1.0, theme::line_strong()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(14))
                    .shadow(ui.style().visuals.popup_shadow),
            )
            .show(|ui| self.settings_ui(ui));
    }

    /// TonePush's own settings: the appearance, and which version this is,
    /// with the update offer beside it.
    fn settings_ui(&mut self, ui: &mut Ui) {
        ui.set_width(272.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        theme::label(ui, "Settings", theme::semibold(theme::TITLE), theme::text());
        ui.add_space(2.0);
        theme::caption(ui, "Appearance");
        let all = theme::Appearance::ALL;
        let segments: Vec<theme::Segment> = all
            .iter()
            .map(|appearance| theme::Segment::new(appearance.label()))
            .collect();
        let chosen = all.iter().position(|a| *a == self.config.appearance);
        if let Some(index) = theme::segmented(ui, "appearance", &segments, chosen, true).clicked {
            self.config.appearance = all[index];
            self.config.save();
            theme::choose(ui.ctx(), self.config.appearance);
        }
        theme::label(
            ui,
            "System follows your desktop's light or dark setting.",
            theme::regular(11.5),
            theme::muted(),
        );
        ui.add_space(6.0);
        theme::caption(ui, "TonePush");
        self.updates.offer_ui(ui);
    }

    /// The deck over the Edit page: the loaded preset, its state in words,
    /// snapshots, tempo, undo, redo and Save.
    pub(crate) fn deck(&mut self, root: &mut Ui, tier: Tier) {
        egui::Panel::top("deck")
            .exact_size(deck_height(tier))
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                if self.pro_active() {
                    let hidden = self.sidebar_hidden;
                    if self.pro.deck(ui, tier, hidden) {
                        self.sidebar_hidden = false;
                    }
                } else {
                    self.hx_deck(ui, tier);
                }
            });
    }

    fn hx_deck(&mut self, ui: &mut Ui, tier: Tier) {
        let full = ui.max_rect();
        let live = matches!(self.connection, Connection::Online);
        let loaded = self.preset_index >= 0;
        // The right-hand group first, from the edge in, so the title knows
        // how much room is left.
        let right = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(full.with_max_x(full.right() - 16.0))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    if !loaded {
                        return;
                    }
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let asked = deck_actions(
                        ui,
                        &DeckActions {
                            tier,
                            live,
                            undo: self.undo_depth,
                            redo: self.redo_depth,
                            dirty: self.dirty,
                            can_save: self.dirty,
                            save_refused: "Nothing to save",
                            unlock: None,
                        },
                    );
                    if asked.save {
                        self.send(Cmd::SavePreset);
                    }
                    if asked.undo {
                        self.send(Cmd::Undo);
                    }
                    if asked.redo {
                        self.send(Cmd::Redo);
                    }
                    if asked.discard {
                        self.discard_changes();
                    }
                    ui.add_space(8.0);
                    rule(ui);
                    ui.add_space(8.0);
                    if let Some(bpm) = self.tempo {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let set = tempo(ui, bpm, &mut self.tempo_draft, &mut self.taps);
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if let Some(bpm) = set {
                            self.set_tempo(bpm);
                        }
                        ui.add_space(8.0);
                        rule(ui);
                        ui.add_space(8.0);
                    }
                    if !self.snapshots.is_empty() {
                        self.snapshot_switch(ui, tier);
                    }
                },
            )
            .response
            .rect;
        let left = full.left() + if self.sidebar_hidden { 12.0 } else { 20.0 };
        let right_edge = if loaded {
            right.left() - 14.0
        } else {
            full.right() - 16.0
        };
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(left, full.top()),
                    Pos2::new(right_edge, full.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if self.sidebar_hidden && sidebar_toggle(ui) {
                    self.sidebar_hidden = false;
                }
                if loaded {
                    slot_chip(ui, &self.active_slot_label(self.preset_index));
                }
                let width = ui.available_width();
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 44.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let size = tier.pick(theme::PRESET_NAME, theme::PRESET_NAME, 22.0);
                        if loaded {
                            if let Some(name) = deck_title(
                                ui,
                                &self.preset_name,
                                size,
                                width,
                                &mut self.renaming_header,
                                true,
                                "",
                            ) {
                                self.send(Cmd::Rename {
                                    index: self.preset_index,
                                    name,
                                });
                            }
                        } else {
                            let title =
                                title_galley(ui, "No preset loaded", size, theme::muted(), width);
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(title.size().x, size + 4.0),
                                Sense::hover(),
                            );
                            paint_line(ui, title, rect.left(), rect.center().y);
                        }
                        let parts = self.hx_state(tier);
                        state_line(ui, &parts, width);
                    },
                );
            },
        );
    }

    /// The deck's second line for an HX preset.
    fn hx_state(&self, tier: Tier) -> Vec<State> {
        let mut parts = Vec::new();
        if self.preset_index < 0 {
            match self.connection {
                Connection::Connecting => {
                    parts.push(State::Busy("Looking for a pedal on USB".to_owned()));
                }
                _ if !self.status.is_empty() => parts.push(State::Problem(self.status.clone())),
                Connection::Online => parts.push(State::Busy("Reading the pedal".to_owned())),
                Connection::Offline => {
                    parts.push(State::Note(
                        "Plug in a pedal to edit its presets".to_owned(),
                    ));
                }
            }
            return parts;
        }
        if let Some((what, progress)) = &self.working {
            parts.push(State::Busy(format!(
                "{} · {:.0}%",
                sentence_case(what),
                progress * 100.0
            )));
        } else if self.loading {
            parts.push(State::Busy("Loading from the pedal".to_owned()));
        } else if self
            .busy_since
            .is_some_and(|since| since.elapsed() > std::time::Duration::from_millis(150))
        {
            parts.push(State::Busy("Writing to the pedal".to_owned()));
        }
        if !self.status.is_empty() {
            parts.push(State::Problem(self.status.clone()));
        }
        if self.dirty {
            parts.push(State::Unsaved("Changes not saved".to_owned()));
        }
        // A small window says one thing: the library's word only when there
        // is nothing more pressing.
        if parts.is_empty() || tier != Tier::S {
            if let Some(note) = self.library_note() {
                parts.push(note);
            }
        }
        parts
    }

    /// Whether the library holds the loaded preset, in the deck's words.
    fn library_note(&self) -> Option<State> {
        let version = |name: &str| {
            self.lib_entries
                .iter()
                .find(|entry| entry.name.trim().eq_ignore_ascii_case(name.trim()))
                .filter(|entry| entry.versions > 1)
                .map(|entry| format!(" as v{}", entry.version))
                .unwrap_or_default()
        };
        match self.slot_sync(self.preset_index) {
            theme::Sync::Same => Some(State::Note(format!(
                "In your library{}",
                version(&self.preset_name)
            ))),
            theme::Sync::Differs => Some(State::Note("Differs from your library".to_owned())),
            _ => None,
        }
    }

    /// The snapshots as a segmented control: the one on the pedal raised;
    /// click to switch, right-click to rename. Called inside the deck's
    /// right-to-left layout.
    fn snapshot_switch(&mut self, ui: &mut Ui, tier: Tier) {
        if let Some((index, draft)) = self.snapshot_draft.as_mut() {
            let index = *index;
            let edit = ui.add(egui::TextEdit::singleline(draft).desired_width(110.0));
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            if edit.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let name = draft.clone();
                    self.send(Cmd::RenameSnapshot { index, name });
                }
                self.snapshot_draft = None;
            }
            return;
        }
        let names = self.snapshots.clone();
        let current = self.current_snapshot;
        let segments: Vec<theme::Segment> = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let label = if tier == Tier::S && index != current {
                    ""
                } else {
                    name.as_str()
                };
                theme::Segment::new(label).prefix((index + 1).to_string())
            })
            .collect();
        // One allocation for the whole control, so it sits at the right
        // place in the deck's right-to-left run.
        let asked = theme::segmented(ui, "snapshots", &segments, Some(current), false);
        if let Some(response) = asked.response {
            response.on_hover_text("Snapshots: click to switch, right-click to rename");
        }
        if let Some(index) = asked.clicked {
            self.current_snapshot = index;
            self.send(Cmd::SelectSnapshot(index as i64));
        }
        if let Some(index) = asked.secondary {
            self.snapshot_draft = Some((index, names[index].clone()));
        }
    }

    /// The loaded HX preset's chain as category colours, for the mini deck.
    pub(crate) fn hx_mini_chain(&self) -> Vec<Mini> {
        minis(&self.chain, &self.layout, |block| {
            self.is_effect(block).then(|| self.block_colour(block))
        })
    }

    /// The 52-point deck over the Library and Pedal pages.
    pub(crate) fn small_deck(&mut self, root: &mut Ui) {
        egui::Panel::top("mini-deck")
            .exact_size(MINI_DECK_HEIGHT)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                let hidden = self.sidebar_hidden;
                let asked = if self.pro_active() {
                    self.pro.mini_deck(ui, hidden)
                } else if self.preset_index < 0 {
                    let full = ui.max_rect();
                    let mut show_sidebar = false;
                    let left = full.left() + if hidden { 12.0 } else { 20.0 };
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(full.with_min_x(left))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            if hidden {
                                show_sidebar = sidebar_toggle(ui);
                            }
                            let text = match self.connection {
                                Connection::Connecting => "Looking for a pedal on USB",
                                Connection::Online => "Reading the pedal",
                                Connection::Offline => "No pedal connected",
                            };
                            theme::label(ui, text, theme::regular(theme::BODY), theme::muted());
                        },
                    );
                    MiniAsked {
                        show_sidebar,
                        ..MiniAsked::default()
                    }
                } else {
                    let chain = self.hx_mini_chain();
                    let snapshot = self.snapshots.get(self.current_snapshot).cloned();
                    let slot = self.active_slot_label(self.preset_index);
                    let live = matches!(self.connection, Connection::Online);
                    let asked = mini_deck(
                        ui,
                        &MiniDeck {
                            slot: &slot,
                            name: &self.preset_name,
                            dirty: self.dirty,
                            chain: &chain,
                            snapshot: snapshot.as_deref(),
                            can_save: live,
                            save_refused: "Connect the pedal to save",
                            sidebar_hidden: hidden,
                        },
                    );
                    if asked.save {
                        self.send(Cmd::SavePreset);
                    }
                    asked
                };
                if asked.edit {
                    self.go_to(Page::Edit);
                }
                if asked.show_sidebar {
                    self.sidebar_hidden = false;
                }
            });
    }

    /// Read the automatic backup's manifest: what the foot and the Pedal
    /// page say about it.
    pub(crate) fn read_automatic_backup(&mut self) {
        self.automatic_backup =
            session::automatic_dir().and_then(|dir| hx_usb::backup::open(&dir).ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tiers_switch_at_1100_and_1800_points() {
        assert_eq!(Tier::of(1024.0), Tier::S);
        assert_eq!(Tier::of(1100.0), Tier::S);
        assert_eq!(Tier::of(1280.0), Tier::M);
        assert_eq!(Tier::of(1799.0), Tier::M);
        assert_eq!(Tier::of(1800.0), Tier::L);
        assert_eq!(Tier::of(2560.0), Tier::L);
        assert_eq!(sidebar_width(Tier::S), 216.0);
        assert_eq!(sidebar_width(Tier::M), 232.0);
        assert_eq!(sidebar_width(Tier::L), 272.0);
        assert_eq!(deck_height(Tier::M), 64.0);
    }

    #[test]
    fn an_hx_slot_holding_the_factory_default_is_empty() {
        assert!(hx_slot_is_empty(""));
        assert!(hx_slot_is_empty("New Preset"));
        assert!(!hx_slot_is_empty("Plexi Crunch"));
    }

    #[test]
    fn taps_two_seconds_apart_start_again() {
        let start = std::time::Instant::now();
        let mut taps = Vec::new();
        assert_eq!(tap(&mut taps, start), None, "one tap is no tempo");
        let bpm = tap(&mut taps, start + std::time::Duration::from_millis(500)).unwrap();
        assert!((bpm - 120.0).abs() < 0.01, "{bpm}");
        assert_eq!(
            tap(&mut taps, start + std::time::Duration::from_secs(5)),
            None,
            "a long pause starts a new count"
        );
        assert_eq!(taps.len(), 1);
    }

    #[test]
    fn work_in_hand_reads_as_a_sentence() {
        assert_eq!(sentence_case("presets"), "Presets");
        assert_eq!(sentence_case(""), "");
    }

    /// An app whose device channel can be read back.
    fn app() -> (App, std::sync::mpsc::Receiver<Cmd>) {
        let (to_device, sent) = std::sync::mpsc::channel();
        let (_silent, from_device) = std::sync::mpsc::channel();
        let app = App::new(&egui::Context::default(), to_device, from_device);
        // What the app asks for on its own as it starts.
        let _ = sent.try_iter().count();
        (app, sent)
    }

    #[test]
    fn opening_the_pedal_page_reads_what_it_shows() {
        let (mut app, sent) = app();
        app.connection = Connection::Online;
        app.go_to(Page::Pedal);
        assert_eq!(app.page, Page::Pedal);
        let asked: Vec<Cmd> = sent.try_iter().collect();
        assert!(asked.iter().any(|cmd| matches!(cmd, Cmd::ReadSettings)));
        assert!(asked.iter().any(|cmd| matches!(cmd, Cmd::ListFavourites)));

        app.go_to(Page::Pedal);
        assert_eq!(
            sent.try_iter().count(),
            0,
            "staying on the page asks nothing again"
        );
    }

    #[test]
    fn without_a_pedal_the_pedal_page_asks_nothing() {
        let (mut app, sent) = app();
        app.connection = Connection::Offline;
        app.go_to(Page::Pedal);
        assert_eq!(sent.try_iter().count(), 0);
    }

    #[test]
    fn letting_the_pedal_go_and_looking_again_reach_the_worker() {
        let (mut app, sent) = app();
        app.connection = Connection::Online;
        app.status = "an old failure".to_owned();
        app.let_go();
        assert!(app.status.is_empty());
        assert!(matches!(sent.try_recv(), Ok(Cmd::Disconnect)));

        app.connection = Connection::Offline;
        app.look_for_pedal();
        assert_eq!(app.connection, Connection::Connecting);
        assert!(matches!(sent.try_recv(), Ok(Cmd::Connect)));
    }

    #[test]
    fn the_foot_says_when_the_pedal_was_backed_up() {
        let (mut app, _sent) = app();
        assert!(app.foot().icon.is_none(), "no pedal: TonePush's own mark");
        app.connection = Connection::Online;
        app.device = "HX Stomp".to_owned();
        assert_eq!(app.foot().text, "Not backed up yet");
        app.automatic_backup = Some(hx_usb::backup::Manifest {
            version: 1,
            device: "HX Stomp".to_owned(),
            firmware: "3.80".to_owned(),
            captured: 0,
            setlists: Vec::new(),
            presets: Vec::new(),
            more_setlists: Vec::new(),
            irs: Default::default(),
            globals: 0,
        });
        assert!(app.foot().text.starts_with("Backed up on "));
        app.device = "HX Stomp XL".to_owned();
        assert_eq!(
            app.foot().text,
            "Not backed up yet",
            "another pedal's backup does not protect this one"
        );
    }

    #[test]
    fn the_rows_keep_the_pedals_banks() {
        let (mut app, _sent) = app();
        app.device = "HX Effects".to_owned();
        app.presets = (0..8).map(|i| format!("Tone {i}")).collect();
        app.presets[2] = "New Preset".to_owned();
        app.preset_index = 1;
        app.dirty = true;
        let rows = app.hx_rows();
        assert_eq!(rows.len(), 8);
        assert!(!rows[0].bank_start, "nothing above the first row");
        assert!(rows[4].bank_start, "four to a bank on an HX Effects");
        assert!(!rows[3].bank_start);
        assert!(rows[1].selected && rows[1].edited);
        assert!(rows[2].empty);
        assert_eq!(rows[4].slot, "02A");
    }
}
