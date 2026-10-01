//! The components every page is built from, as the design draws them: buttons,
//! icon buttons, segmented controls, chips, tags, LEDs, captions, cards,
//! banners, progress bars, steppers, dialogs, menu rows, tabs, knobs and the
//! footswitch a block's on/off is drawn as.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Galley, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::paint;
use super::*;

fn layout(ui: &Ui, text: &str, font: egui::FontId, colour: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, colour)
}

fn centred_galley(ui: &Ui, galley: Arc<Galley>, left: f32, centre_y: f32) {
    let pos = Pos2::new(left, centre_y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, Color32::PLACEHOLDER);
}

// ---------------------------------------------------------------------------
// Buttons

/// What a button is for, which is what it looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The amber gradient: the one next action on a surface.
    Primary,
    /// Raised, outlined: everything else a person might press.
    Secondary,
    /// Text only, until hovered.
    Ghost,
    /// Filled red: a destructive confirmation.
    Danger,
    /// Outlined red: a destructive action that will still ask.
    DangerLine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// 26 points.
    Small,
    /// 30 points.
    Medium,
    /// 36 points.
    Large,
}

impl Size {
    fn height(self) -> f32 {
        match self {
            Size::Small => 26.0,
            Size::Medium => 30.0,
            Size::Large => 36.0,
        }
    }

    fn padding(self) -> f32 {
        match self {
            Size::Small => 9.0,
            Size::Medium => 12.0,
            Size::Large => 16.0,
        }
    }

    fn font(self) -> f32 {
        match self {
            Size::Small => 12.0,
            Size::Medium => 13.0,
            Size::Large => 13.5,
        }
    }

    fn icon(self) -> f32 {
        match self {
            Size::Small => 14.0,
            _ => 15.0,
        }
    }
}

/// A button: label, optional icon and keyboard hint, in one of the tones.
#[must_use]
pub struct Button<'a> {
    label: &'a str,
    icon: Option<Icon>,
    trailing: Option<Icon>,
    hint: Option<&'a str>,
    tone: Tone,
    size: Size,
    enabled: bool,
    min_width: f32,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            icon: None,
            trailing: None,
            hint: None,
            tone: Tone::Secondary,
            size: Size::Medium,
            enabled: true,
            min_width: 0.0,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn primary(self) -> Self {
        self.tone(Tone::Primary)
    }

    pub fn ghost(self) -> Self {
        self.tone(Tone::Ghost)
    }

    pub fn danger(self) -> Self {
        self.tone(Tone::Danger)
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn small(self) -> Self {
        self.size(Size::Small)
    }

    pub fn large(self) -> Self {
        self.size(Size::Large)
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// An icon after the label: a dropdown's chevron.
    pub fn trailing(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }

    /// The shortcut, in a quieter voice after the label.
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let Button {
            label,
            icon,
            trailing,
            hint,
            tone,
            size,
            enabled,
            min_width,
        } = self;
        let weight = if tone == Tone::Ghost {
            Weight::Medium
        } else {
            Weight::SemiBold
        };
        let (ink, glyph_ink) = if !enabled {
            (faint(), faint())
        } else {
            match tone {
                Tone::Primary => (accent_ink(), accent_ink()),
                Tone::Secondary => (text(), text_soft()),
                Tone::Ghost => (text_soft(), text_soft()),
                Tone::Danger => (Color32::WHITE, Color32::WHITE),
                Tone::DangerLine => (danger(), danger()),
            }
        };
        let text_galley =
            (!label.is_empty()).then(|| layout(ui, label, font(size.font(), weight), ink));
        let hint_galley = hint.map(|hint| layout(ui, hint, medium(11.0), alpha(ink, 0.7)));
        let gap = if size == Size::Small { 6.0 } else { 7.0 };
        let icon_size = size.icon();
        let mut width = size.padding() * 2.0;
        let mut parts = 0;
        if icon.is_some() {
            width += icon_size;
            parts += 1;
        }
        if let Some(galley) = &text_galley {
            width += galley.size().x;
            parts += 1;
        }
        if let Some(galley) = &hint_galley {
            width += galley.size().x;
            parts += 1;
        }
        if trailing.is_some() {
            width += 12.0;
            parts += 1;
        }
        width += gap * (parts.max(1) - 1) as f32;
        if label.is_empty() && icon.is_some() && trailing.is_none() {
            // An icon on its own is a square.
            width = size.height();
        }
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width.max(min_width), size.height()), sense);
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let radius = f32::from(RADIUS_CONTROL);
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let pressed = enabled && response.is_pointer_button_down_on();
        match (tone, enabled) {
            (Tone::Primary, true) => {
                paint::glow(
                    painter,
                    rect.shrink2(Vec2::new(10.0, 8.0)),
                    radius,
                    Vec2::new(0.0, 8.0),
                    20.0,
                    0.0,
                    rgba_of(0xd8a83b, if hovered { 0.75 } else { 0.55 }),
                );
                let (top, bottom) = if pressed {
                    (rgb_of(0xd8a83b), rgb_of(0xd8a83b))
                } else if hovered {
                    (
                        accent_bright(),
                        mix(rgb_of(0xd8a83b), accent_bright(), 0.35),
                    )
                } else {
                    (accent_bright(), rgb_of(0xd8a83b))
                };
                paint::gradient_rect(painter, rect, radius, &[(0.0, top), (1.0, bottom)]);
                painter.line_segment(
                    [
                        Pos2::new(rect.left() + radius, rect.top() + 1.0),
                        Pos2::new(rect.right() - radius, rect.top() + 1.0),
                    ],
                    Stroke::new(1.0, Color32::from_white_alpha(64)),
                );
                painter.rect_stroke(
                    rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    Stroke::new(1.0, accent()),
                    egui::StrokeKind::Inside,
                );
            }
            (Tone::Primary, false) => {
                painter.rect(
                    rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    raised(),
                    Stroke::new(1.0, line()),
                    egui::StrokeKind::Inside,
                );
            }
            (_, false) => {
                painter.rect_stroke(
                    rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    Stroke::new(1.0, line()),
                    egui::StrokeKind::Inside,
                );
            }
            (Tone::Secondary, true) => {
                let fill = if pressed {
                    pressed_fill()
                } else if hovered {
                    hover()
                } else {
                    raised()
                };
                painter.rect(
                    rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    fill,
                    Stroke::new(1.0, line_strong()),
                    egui::StrokeKind::Inside,
                );
            }
            (Tone::Ghost, true) => {
                if hovered || pressed {
                    painter.rect_filled(
                        rect,
                        CornerRadius::same(RADIUS_CONTROL),
                        if pressed { hover() } else { raised() },
                    );
                }
            }
            (Tone::Danger, true) => {
                let fill = if hovered {
                    mix(danger(), Color32::WHITE, 0.1)
                } else {
                    danger()
                };
                painter.rect(
                    rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    fill,
                    Stroke::new(1.0, danger()),
                    egui::StrokeKind::Inside,
                );
            }
            (Tone::DangerLine, true) => {
                if hovered {
                    painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), danger_soft());
                }
                painter.rect_stroke(
                    rect,
                    CornerRadius::same(RADIUS_CONTROL),
                    Stroke::new(1.0, alpha(danger(), 0.55)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        // The contents, centred as one group.
        let content = width - size.padding() * 2.0;
        let mut x = rect.center().x - content / 2.0;
        if label.is_empty() && icon.is_some() && trailing.is_none() {
            x = rect.center().x - icon_size / 2.0;
        }
        let y = rect.center().y;
        if let Some(icon) = icon {
            paint_icon(
                ui,
                icon,
                Pos2::new(x + icon_size / 2.0, y),
                icon_size,
                glyph_ink,
            );
            x += icon_size + gap;
        }
        if let Some(galley) = text_galley {
            let width = galley.size().x;
            centred_galley(ui, galley, x, y);
            x += width + gap;
        }
        if let Some(galley) = hint_galley {
            let width = galley.size().x;
            centred_galley(ui, galley, x - gap + 4.0, y);
            x += width + gap;
        }
        if let Some(icon) = trailing {
            paint_icon(ui, icon, Pos2::new(x + 6.0, y), 12.0, glyph_ink);
        }
        response
    }
}

/// The darker fill a secondary button takes while pressed.
fn pressed_fill() -> Color32 {
    palette().pressed
}

fn rgb_of(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn rgba_of(value: u32, a: f32) -> Color32 {
    alpha(rgb_of(value), a)
}

/// A frameless square button with an icon in it: muted at rest, raised under
/// the pointer, filled when it is a toggle that is on.
#[must_use]
pub struct IconButton {
    icon: Icon,
    side: f32,
    glyph: f32,
    on: bool,
    enabled: bool,
    tint: Option<Color32>,
}

impl IconButton {
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            side: 28.0,
            glyph: 16.0,
            on: false,
            enabled: true,
            tint: None,
        }
    }

    /// The 24-point form, for lists and section headers.
    pub fn small(mut self) -> Self {
        self.side = 24.0;
        self.glyph = 14.0;
        self
    }

    pub fn side(mut self, side: f32, glyph: f32) -> Self {
        self.side = side;
        self.glyph = glyph;
        self
    }

    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A colour of its own at rest: a favourite's amber star.
    pub fn tint(mut self, tint: Color32) -> Self {
        self.tint = Some(tint);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(self.side), sense);
        if ui.is_rect_visible(rect) {
            let hovered = self.enabled && response.hovered();
            let radius = if self.side < 26.0 { 6 } else { 7 };
            if self.on {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(radius), hover());
            } else if hovered {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(radius), raised());
            }
            let ink = if !self.enabled {
                alpha(faint(), 0.6)
            } else if let Some(tint) = self.tint {
                tint
            } else if hovered || self.on {
                text()
            } else {
                muted()
            };
            paint_icon(ui, self.icon, rect.center(), self.glyph, ink);
        }
        response
    }
}

/// The ordinary 28-point icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool) -> Response {
    IconButton::new(icon).enabled(enabled).show(ui)
}

// ---------------------------------------------------------------------------
// Segmented controls

/// One segment: a label, and a quieter number before it ("1 Verse") or a
/// count after it ("Tones 248").
pub struct Segment<'a> {
    pub label: &'a str,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub icon: Option<Icon>,
}

impl<'a> Segment<'a> {
    pub fn new(label: &'a str) -> Self {
        Segment {
            label,
            prefix: None,
            suffix: None,
            icon: None,
        }
    }

    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A segment's prefix, label and suffix, laid out.
type Laid = (Option<Arc<Galley>>, Arc<Galley>, Option<Arc<Galley>>);

/// What a segmented control was asked this frame.
#[derive(Default)]
pub struct Segmented {
    pub clicked: Option<usize>,
    pub secondary: Option<usize>,
    pub rects: Vec<Rect>,
    pub response: Option<Response>,
}

/// A row of segments on a sunken track; the chosen one raised. `fill` makes
/// every segment the same width across the room it is given.
pub fn segmented(
    ui: &mut Ui,
    id: &str,
    segments: &[Segment<'_>],
    selected: Option<usize>,
    fill: bool,
) -> Segmented {
    const HEIGHT: f32 = 30.0;
    const PAD: f32 = 2.0;
    const GAP: f32 = 2.0;
    let id = ui.id().with(id);
    let font = medium(12.5);
    let laid: Vec<Laid> = segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let on = selected == Some(index);
            let quiet = if on { muted() } else { faint() };
            (
                segment
                    .prefix
                    .as_deref()
                    .map(|prefix| layout(ui, prefix, semibold(11.5), quiet)),
                layout(
                    ui,
                    segment.label,
                    font.clone(),
                    if on { text() } else { muted() },
                ),
                segment
                    .suffix
                    .as_deref()
                    .map(|suffix| layout(ui, suffix, semibold(12.0), quiet)),
            )
        })
        .collect();
    let natural: Vec<f32> = laid
        .iter()
        .zip(segments)
        .map(|((prefix, label, suffix), segment)| {
            let mut width = 20.0 + label.size().x;
            if let Some(prefix) = prefix {
                width += prefix.size().x + 7.0;
            }
            if let Some(suffix) = suffix {
                width += suffix.size().x + 6.0;
            }
            if segment.icon.is_some() {
                width += 14.0 + 6.0;
            }
            width
        })
        .collect();
    let total = if fill {
        ui.available_width()
    } else {
        natural.iter().sum::<f32>() + GAP * (segments.len().saturating_sub(1)) as f32 + PAD * 2.0
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(total, HEIGHT), Sense::hover());
    let mut out = Segmented {
        response: Some(response),
        ..Default::default()
    };
    if !ui.is_rect_visible(rect) {
        return out;
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(9),
        bg_deep(),
        Stroke::new(1.0, line()),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(PAD);
    let fair = if segments.is_empty() {
        0.0
    } else {
        (inner.width() - GAP * (segments.len() - 1) as f32) / segments.len() as f32
    };
    let mut x = inner.left();
    for (index, ((prefix, label, suffix), segment)) in laid.into_iter().zip(segments).enumerate() {
        let width = if fill { fair } else { natural[index] };
        let cell = Rect::from_min_size(Pos2::new(x, inner.top()), Vec2::new(width, inner.height()));
        x += width + GAP;
        let hit = ui.interact(cell, id.with(index), Sense::click());
        let on = selected == Some(index);
        if on {
            if is_dark() {
                ui.painter().rect_filled(
                    cell.translate(Vec2::new(0.0, 1.0)),
                    CornerRadius::same(7),
                    Color32::from_black_alpha(70),
                );
                ui.painter()
                    .rect_filled(cell, CornerRadius::same(7), hover());
                ui.painter().line_segment(
                    [
                        Pos2::new(cell.left() + 7.0, cell.top() + 0.5),
                        Pos2::new(cell.right() - 7.0, cell.top() + 0.5),
                    ],
                    Stroke::new(1.0, Color32::from_white_alpha(10)),
                );
            } else {
                paint::glow(
                    ui.painter(),
                    cell,
                    7.0,
                    Vec2::new(0.0, 1.0),
                    2.0,
                    0.0,
                    Color32::from_black_alpha(30),
                );
                ui.painter()
                    .rect_filled(cell, CornerRadius::same(7), panel());
            }
        } else if hit.hovered() {
            ui.painter()
                .rect_filled(cell, CornerRadius::same(7), alpha(hover(), 0.6));
        }
        let mut content = label.size().x;
        if let Some(prefix) = &prefix {
            content += prefix.size().x + 7.0;
        }
        if let Some(suffix) = &suffix {
            content += suffix.size().x + 6.0;
        }
        if segment.icon.is_some() {
            content += 20.0;
        }
        let mut cursor = cell.center().x - content / 2.0;
        let y = cell.center().y;
        if let Some(icon) = segment.icon {
            paint_icon(
                ui,
                icon,
                Pos2::new(cursor + 7.0, y),
                14.0,
                if on { text() } else { muted() },
            );
            cursor += 20.0;
        }
        if let Some(prefix) = prefix {
            let width = prefix.size().x;
            centred_galley(ui, prefix, cursor, y);
            cursor += width + 7.0;
        }
        let width = label.size().x;
        centred_galley(ui, label, cursor, y);
        cursor += width + 6.0;
        if let Some(suffix) = suffix {
            centred_galley(ui, suffix, cursor, y);
        }
        if hit.clicked() {
            out.clicked = Some(index);
        }
        if hit.secondary_clicked() {
            out.secondary = Some(index);
        }
        out.rects.push(cell);
    }
    out
}

// ---------------------------------------------------------------------------
// Chips, tags, LEDs and captions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Neutral,
    Ok,
    Hot,
    Accent,
    Info,
    Danger,
    Ghost,
}

impl Mood {
    /// Ink and fill.
    fn colours(self) -> (Color32, Color32, Option<Color32>) {
        match self {
            Mood::Neutral => (text_soft(), raised(), Some(line())),
            Mood::Ghost => (text_soft(), Color32::TRANSPARENT, Some(line())),
            Mood::Ok => (ok(), ok_soft(), None),
            Mood::Hot => (hot(), hot_soft(), None),
            Mood::Accent => (accent(), accent_soft(), None),
            Mood::Info => (info(), info_soft(), None),
            Mood::Danger => (danger(), danger_soft(), None),
        }
    }

    /// The solid colour of the mood.
    #[must_use]
    pub fn ink(self) -> Color32 {
        self.colours().0
    }

    /// The tinted well behind the mood's icon.
    #[must_use]
    pub fn soft(self) -> Color32 {
        match self {
            Mood::Neutral | Mood::Ghost => raised(),
            other => other.colours().1,
        }
    }
}

/// A small label in a mood: "Edited", "Matches the pedal", "6 slots differ".
#[must_use]
pub struct Chip<'a> {
    text: &'a str,
    mood: Mood,
    icon: Option<Icon>,
    dot: bool,
    height: f32,
}

impl<'a> Chip<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            mood: Mood::Neutral,
            icon: None,
            dot: false,
            height: 20.0,
        }
    }

    pub fn mood(mut self, mood: Mood) -> Self {
        self.mood = mood;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A dot before the text, in the chip's ink.
    pub fn dot(mut self) -> Self {
        self.dot = true;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let (ink, fill, stroke) = self.mood.colours();
        let galley = layout(ui, self.text, semibold(11.0), ink);
        let mut width = 14.0 + galley.size().x;
        if self.icon.is_some() {
            width += 12.0 + 5.0;
        }
        if self.dot {
            width += 6.0 + 5.0;
        }
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, self.height), Sense::click());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(RADIUS_CHIP),
                fill,
                stroke.map_or(Stroke::NONE, |colour| Stroke::new(1.0, colour)),
                egui::StrokeKind::Inside,
            );
            let mut x = rect.left() + 7.0;
            let y = rect.center().y;
            if self.dot {
                painter.circle_filled(Pos2::new(x + 3.0, y), 3.0, ink);
                x += 11.0;
            }
            if let Some(icon) = self.icon {
                paint_icon(ui, icon, Pos2::new(x + 6.0, y), 12.0, ink);
                x += 17.0;
            }
            centred_galley(ui, galley, x, y);
        }
        response
    }
}

/// What a tag carries before its text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    /// A footswitch's LED, in the colour it lights; `None` when it is dark.
    Led(Option<Color32>),
    Icon(Icon),
}

/// A tag: what drives a control ("FS1" with its LED, "EXP 1", a camera for
/// snapshots), 18 points, hung on a tile's edge or set in a row.
#[derive(Clone, Copy, Debug)]
pub struct Tag<'a> {
    pub text: &'a str,
    pub mark: Option<Mark>,
    /// The quieter form set inside cards and rows.
    pub inset: bool,
    /// The dashed OFF tag of a bypassed tile.
    pub off: bool,
}

impl<'a> Tag<'a> {
    pub fn new(text: &'a str) -> Self {
        Tag {
            text,
            mark: None,
            inset: false,
            off: false,
        }
    }

    pub fn led(text: &'a str, colour: Option<Color32>) -> Self {
        Tag {
            mark: Some(Mark::Led(colour)),
            ..Tag::new(text)
        }
    }

    pub fn icon(icon: Icon) -> Self {
        Tag {
            mark: Some(Mark::Icon(icon)),
            ..Tag::new("")
        }
    }

    pub fn inset(mut self) -> Self {
        self.inset = true;
        self
    }

    pub fn off() -> Self {
        Tag {
            off: true,
            ..Tag::new("OFF")
        }
    }

    fn galley(&self, ui: &Ui) -> Option<Arc<Galley>> {
        if self.text.is_empty() {
            return None;
        }
        let (font, em, ink) = if self.off {
            (bold(9.5), 0.06, muted())
        } else {
            (bold(10.0), 0.02, text_soft())
        };
        Some(
            ui.painter()
                .layout_job(paint::spaced(self.text, font, ink, em)),
        )
    }

    /// How much room it takes.
    pub fn size(&self, ui: &Ui) -> Vec2 {
        let mut width = 10.0;
        let galley = self.galley(ui);
        if let Some(galley) = &galley {
            width += galley.size().x;
        }
        match self.mark {
            Some(Mark::Led(_)) => width += 6.0 + if galley.is_some() { 4.0 } else { 0.0 },
            Some(Mark::Icon(_)) => width += 11.0 + if galley.is_some() { 4.0 } else { 0.0 },
            None => {}
        }
        Vec2::new(width.max(18.0), 18.0)
    }

    /// Paint it at `rect` without allocating: tags hang outside their tile.
    pub fn paint(&self, ui: &Ui, rect: Rect) {
        let painter = ui.painter();
        if self.off {
            painter.rect_filled(rect, CornerRadius::same(RADIUS_CHIP), tile_off());
            paint::dashed_rect(
                painter,
                rect.shrink(0.5),
                f32::from(RADIUS_CHIP),
                Stroke::new(1.0, faint()),
                2.0,
                2.0,
            );
        } else {
            let (fill, stroke) = if self.inset {
                (raised(), line())
            } else {
                (panel(), line_strong())
            };
            painter.rect(
                rect,
                CornerRadius::same(RADIUS_CHIP),
                fill,
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
        }
        let galley = self.galley(ui);
        let mut content = galley.as_ref().map_or(0.0, |galley| galley.size().x);
        match self.mark {
            Some(Mark::Led(_)) => content += 6.0 + if galley.is_some() { 4.0 } else { 0.0 },
            Some(Mark::Icon(_)) => content += 11.0 + if galley.is_some() { 4.0 } else { 0.0 },
            None => {}
        }
        let mut x = rect.center().x - content / 2.0;
        let y = rect.center().y;
        match self.mark {
            Some(Mark::Led(colour)) => {
                paint_led(painter, Pos2::new(x + 3.0, y), 3.0, colour);
                x += 10.0;
            }
            Some(Mark::Icon(icon)) => {
                paint_icon(ui, icon, Pos2::new(x + 5.5, y), 11.0, text_soft());
                x += 15.0;
            }
            None => {}
        }
        if let Some(galley) = galley {
            centred_galley(ui, galley, x, y);
        }
    }

    /// Set it in the row being laid out.
    pub fn show(&self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size(ui), Sense::click());
        if ui.is_rect_visible(rect) {
            self.paint(ui, rect);
        }
        response
    }
}

/// A footswitch LED: lit in its colour with a glow, or a dark ring.
pub fn paint_led(painter: &egui::Painter, centre: Pos2, radius: f32, colour: Option<Color32>) {
    match colour {
        Some(colour) => {
            painter.circle_filled(centre, radius + 4.0, alpha(colour, 0.12));
            painter.circle_filled(centre, radius + 2.0, alpha(colour, 0.22));
            painter.circle_filled(centre, radius, colour);
        }
        None => {
            painter.circle_stroke(centre, radius - 0.5, Stroke::new(1.5, faint()));
        }
    }
}

/// A section caption: capitals, letter-spaced, in the muted ink.
pub fn caption(ui: &mut Ui, text: &str) -> Response {
    let job = paint::spaced(&text.to_uppercase(), semibold(CAPTION), muted(), 0.07);
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    response
}

/// Text in one font and colour, allocated where the layout puts it.
pub fn label(ui: &mut Ui, text: &str, font: egui::FontId, colour: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(colour)).selectable(false))
}

/// The same, truncated with an ellipsis to the room it is given.
pub fn label_truncated(ui: &mut Ui, text: &str, font: egui::FontId, colour: Color32) -> Response {
    ui.add(
        egui::Label::new(egui::RichText::new(text).font(font).color(colour))
            .selectable(false)
            .truncate(),
    )
}

// ---------------------------------------------------------------------------
// Surfaces

/// A card: the panel fill, a hairline edge, 14-point corners.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(panel())
        .stroke(Stroke::new(1.0, line()))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
}

/// A banner: a card with a tinted icon well, a title, a line of explanation
/// and room for actions under it.
pub fn banner(
    ui: &mut Ui,
    mood: Mood,
    icon: Icon,
    title: &str,
    body: &str,
    actions: impl FnOnce(&mut Ui),
) -> Response {
    card()
        .inner_margin(egui::Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (well, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                ui.painter()
                    .rect_filled(well, CornerRadius::same(10), mood.soft());
                paint_icon(ui, icon, well.center(), 18.0, mood.ink());
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    label(ui, title, semibold(14.5), text());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(body)
                                .font(regular(12.5))
                                .color(text_soft()),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        actions(ui);
                    });
                });
            });
        })
        .response
}

/// A progress bar, 6 points (8 for firmware), in amber unless a mood says.
pub fn progress(ui: &mut Ui, fraction: f32, height: f32, mood: Option<Mood>) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    if ui.is_rect_visible(rect) {
        let radius = (height / 2.0).round() as u8;
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius), raised());
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * fraction, height));
            match mood {
                Some(mood) => {
                    ui.painter()
                        .rect_filled(fill, CornerRadius::same(radius), mood.ink());
                }
                None => {
                    // Left to right, darker amber to bright.
                    let mut mesh = egui::epaint::Mesh::default();
                    let left = rgb_of(0xd8a83b);
                    let right = accent_bright();
                    mesh.colored_vertex(fill.left_top(), left);
                    mesh.colored_vertex(fill.right_top(), right);
                    mesh.colored_vertex(fill.right_bottom(), right);
                    mesh.colored_vertex(fill.left_bottom(), left);
                    mesh.add_triangle(0, 1, 2);
                    mesh.add_triangle(0, 2, 3);
                    ui.painter().add(egui::Shape::mesh(mesh));
                }
            }
        }
    }
    response
}

/// Where a step of a sequence stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Done,
    Now,
    Failed,
    Waiting,
}

/// A row of numbered steps joined by lines, across the room it is given.
pub fn stepper(ui: &mut Ui, steps: &[(&str, StepState)]) -> Response {
    let height = 24.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    if !ui.is_rect_visible(rect) || steps.is_empty() {
        return response;
    }
    let labels: Vec<Arc<Galley>> = steps
        .iter()
        .map(|(label, state)| {
            let (font, ink) = match state {
                StepState::Done => (medium(12.5), text_soft()),
                StepState::Now => (semibold(12.5), text()),
                StepState::Failed => (medium(12.5), danger()),
                StepState::Waiting => (medium(12.5), muted()),
            };
            layout(ui, label, font, ink)
        })
        .collect();
    let widths: Vec<f32> = labels
        .iter()
        .map(|galley| 22.0 + 9.0 + galley.size().x)
        .collect();
    let used: f32 = widths.iter().sum();
    let lines = steps.len().saturating_sub(1) as f32;
    let line_length = if lines > 0.0 {
        ((rect.width() - used) / lines - 24.0).max(24.0)
    } else {
        0.0
    };
    let painter = ui.painter();
    let y = rect.center().y;
    let mut x = rect.left();
    for (index, ((_, state), galley)) in steps.iter().zip(labels).enumerate() {
        if index > 0 {
            let done = steps[index - 1].1 == StepState::Done;
            let from = x + 12.0;
            painter.line_segment(
                [Pos2::new(from, y), Pos2::new(from + line_length, y)],
                Stroke::new(
                    1.5,
                    if done {
                        alpha(ok(), 0.55)
                    } else {
                        line_strong()
                    },
                ),
            );
            x = from + line_length + 12.0;
        }
        let centre = Pos2::new(x + 11.0, y);
        match state {
            StepState::Done => {
                painter.circle_filled(centre, 11.0, ok_soft());
                paint_icon(ui, Icon::Check, centre, 12.0, ok());
            }
            StepState::Failed => {
                painter.circle_filled(centre, 11.0, danger_soft());
                paint_icon(ui, Icon::Close, centre, 12.0, danger());
            }
            StepState::Now | StepState::Waiting => {
                let now = *state == StepState::Now;
                if now {
                    painter.circle_filled(centre, 11.0, accent());
                } else {
                    painter.circle_stroke(centre, 10.25, Stroke::new(1.5, line_strong()));
                }
                let number = layout(
                    ui,
                    &(index + 1).to_string(),
                    semibold(11.0),
                    if now { accent_ink() } else { muted() },
                );
                let size = number.size();
                painter.galley(centre - size / 2.0, number, Color32::PLACEHOLDER);
            }
        }
        let width = galley.size().x;
        centred_galley(ui, galley, x + 31.0, y);
        x += 31.0 + width;
    }
    response
}

// ---------------------------------------------------------------------------
// Dialogs

/// A dialog over a dimmed window: radius 16, a title, one line of
/// explanation, a body, and a footer with a note and the actions, the
/// confirming one last. Returns what the contents return, and whether Escape
/// or a click outside asked to close it.
pub fn dialog<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    contents: impl FnOnce(&mut Ui) -> R,
) -> (R, bool) {
    let response = egui::Modal::new(egui::Id::new(id))
        .backdrop_color(scrim())
        .frame(
            egui::Frame::new()
                .fill(panel())
                .stroke(Stroke::new(1.0, line_strong()))
                .corner_radius(CornerRadius::same(RADIUS_DIALOG))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 24],
                    blur: 60,
                    spread: 0,
                    color: palette().shadow,
                }),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            contents(ui)
        });
    let close = response.should_close();
    (response.inner, close)
}

/// A dialog's title and its one line of explanation.
pub fn dialog_header(ui: &mut Ui, title: &str, explanation: Option<&str>) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 24,
            right: 24,
            top: 22,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .font(semibold(18.0))
                        .color(text()),
                )
                .wrap(),
            );
            if let Some(explanation) = explanation {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(explanation)
                            .font(regular(BODY))
                            .color(text_soft()),
                    )
                    .wrap(),
                );
            }
        });
}

/// A dialog's body, padded as the design pads it.
pub fn dialog_body<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 24,
            right: 24,
            top: 14,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            contents(ui)
        })
        .inner
}

/// One outcome of a write, counted: "4 presets replaced: 02B, 05A".
pub struct Outcome {
    pub icon: Icon,
    pub count: String,
    pub sentence: String,
    /// A count that is nothing to worry about, in the quieter ink.
    pub quiet: bool,
}

/// The outcomes of a write, in a framed list.
pub fn outcomes(ui: &mut Ui, rows: &[Outcome]) {
    egui::Frame::new()
        .fill(bg())
        .stroke(Stroke::new(1.0, line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for row in rows {
                ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    ui.add_space(14.0);
                    let (icon, _) = ui.allocate_exact_size(Vec2::new(22.0, 20.0), Sense::hover());
                    paint_icon(ui, row.icon, icon.center(), 15.0, muted());
                    ui.add_space(12.0);
                    let count = layout(
                        ui,
                        &row.count,
                        semibold(14.0),
                        if row.quiet { muted() } else { text() },
                    );
                    let (slot, _) = ui.allocate_exact_size(Vec2::new(34.0, 20.0), Sense::hover());
                    let width = count.size().x;
                    centred_galley(ui, count, slot.right() - width, slot.center().y);
                    ui.add_space(12.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&row.sentence)
                                .font(regular(BODY))
                                .color(text_soft()),
                        )
                        .truncate(),
                    );
                });
            }
        });
}

/// A dialog's footer: a note on the left and the actions on the right.
///
/// The actions are laid out from the right edge, so `actions` adds the
/// confirming one first and Cancel after it; on screen they read Cancel, then
/// the action named by its outcome, last. The note takes whatever room the
/// actions leave.
pub fn dialog_footer<R>(ui: &mut Ui, note: &str, actions: impl FnOnce(&mut Ui) -> R) -> R {
    let top = ui.cursor().top();
    let full = ui.max_rect();
    ui.painter()
        .hline(full.x_range(), top, Stroke::new(1.0, line()));
    egui::Frame::new()
        .fill(mix(panel(), bg(), 0.55))
        .corner_radius(CornerRadius {
            nw: 0,
            ne: 0,
            sw: RADIUS_DIALOG,
            se: RADIUS_DIALOG,
        })
        .inner_margin(egui::Margin::symmetric(24, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let inner = actions(ui);
                if !note.is_empty() {
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), 30.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(note).font(regular(12.0)).color(muted()),
                                )
                                .wrap(),
                            );
                        },
                    );
                }
                inner
            })
            .inner
        })
        .inner
}

// ---------------------------------------------------------------------------
// Menus

/// The width every menu row fills.
pub fn menu_width(ui: &mut Ui, width: f32) {
    ui.set_min_width(width);
    ui.set_max_width(width);
}

/// A menu's header: what it acts on, and a quieter aside.
pub fn menu_header(ui: &mut Ui, title: &str, aside: Option<&str>) {
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        ui.add_space(10.0);
        label_truncated(ui, title, semibold(12.5), text());
        if let Some(aside) = aside {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                label(ui, aside, regular(12.0), muted());
            });
        }
    });
    menu_separator(ui);
}

/// One 28-point row: an icon, the label, and a right-aligned hint.
pub fn menu_item(ui: &mut Ui, icon: Option<Icon>, text_: &str, hint: Option<&str>) -> Response {
    menu_row(ui, icon, text_, hint, false, true)
}

/// The last row of a menu, in red: the destructive action.
pub fn menu_danger(ui: &mut Ui, icon: Icon, text_: &str) -> Response {
    menu_row(ui, Some(icon), text_, None, true, true)
}

/// A row that cannot be chosen right now, still saying what it would do.
pub fn menu_disabled(ui: &mut Ui, icon: Option<Icon>, text_: &str, hint: Option<&str>) -> Response {
    menu_row(ui, icon, text_, hint, false, false)
}

fn menu_row(
    ui: &mut Ui,
    icon: Option<Icon>,
    text_: &str,
    hint: Option<&str>,
    destructive: bool,
    enabled: bool,
) -> Response {
    let width = ui.available_width().max(160.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), sense);
    if ui.is_rect_visible(rect) {
        if enabled && response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), hover());
        }
        let (ink, glyph) = if !enabled {
            (faint(), faint())
        } else if destructive {
            (danger(), danger())
        } else {
            (text(), text_soft())
        };
        let mut x = rect.left() + 10.0;
        if let Some(icon) = icon {
            paint_icon(ui, icon, Pos2::new(x + 7.5, rect.center().y), 15.0, glyph);
        }
        x += 15.0 + 10.0;
        let galley = layout(ui, text_, regular(BODY), ink);
        centred_galley(ui, galley, x, rect.center().y);
        if let Some(hint) = hint {
            let galley = layout(ui, hint, regular(12.0), muted());
            let width = galley.size().x;
            centred_galley(ui, galley, rect.right() - 10.0 - width, rect.center().y);
        }
    }
    if response.clicked() {
        ui.close();
    }
    response
}

/// A hairline between groups of menu rows.
pub fn menu_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 13.0), Sense::hover());
    ui.painter().hline(
        (rect.left() + 4.0)..=(rect.right() - 4.0),
        rect.center().y,
        Stroke::new(1.0, line()),
    );
}

// ---------------------------------------------------------------------------
// Tabs

/// A row of page tabs with an underline under the chosen one. Returns the one
/// clicked.
pub fn tabs(ui: &mut Ui, tabs: &[(&str, Option<String>)], selected: usize) -> Option<usize> {
    let mut clicked = None;
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 22.0;
        for (index, (name, count)) in tabs.iter().enumerate() {
            let on = index == selected;
            let galley = layout(ui, name, medium(BODY), if on { text() } else { muted() });
            let count = count
                .as_deref()
                .map(|count| layout(ui, count, medium(12.0), faint()));
            let width = galley.size().x + count.as_ref().map_or(0.0, |c| c.size().x + 7.0);
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::click());
            let hovered = response.hovered() && !on;
            let galley = if hovered {
                layout(ui, name, medium(BODY), text_soft())
            } else {
                galley
            };
            let label_width = galley.size().x;
            centred_galley(ui, galley, rect.left(), rect.center().y);
            if let Some(count) = count {
                centred_galley(ui, count, rect.left() + label_width + 7.0, rect.center().y);
            }
            if on {
                let bar = Rect::from_min_max(
                    Pos2::new(rect.left(), rect.bottom() - 1.0),
                    Pos2::new(rect.right(), rect.bottom() + 1.0),
                );
                ui.painter().rect_filled(bar, CornerRadius::same(2), text());
            }
            if response.clicked() {
                clicked = Some(index);
            }
        }
    });
    let bottom = ui.min_rect().bottom().max(top + 36.0);
    let full = ui.max_rect();
    ui.painter()
        .hline(full.x_range(), bottom, Stroke::new(1.0, line()));
    clicked
}

// ---------------------------------------------------------------------------
// Knobs and switches

/// How a knob responds to a vertical drag: 200 points sweeps the range, and
/// Shift makes it ten times finer, as Ableton Live and Cubase do.
#[must_use]
pub fn knob_drag_delta(delta_y: f32, span: f32, fine: bool) -> f32 {
    let scale = if fine { 0.1 } else { 1.0 };
    -delta_y / 200.0 * span * scale
}

/// A rotary knob, `diameter` points across, drawn as the design draws one: a
/// 270° track from the lower left, the value arc over it in the block's
/// category colour, a tick at the default (the double-click target), and a
/// shaded face with a pointer. Dragging vertically turns it.
pub fn dial(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    diameter: f32,
    colour: Color32,
    default: Option<f32>,
) -> Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
    let (min, max) = (*range.start(), *range.end());
    let span = max - min;
    let continuous = response.id.with("continuous knob value");
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(continuous, *value));
    }
    if response.dragged() && span.abs() > f32::EPSILON {
        // The unrounded value is kept between frames so a stepped parameter
        // can still be moved by Shift-drag's fine steps.
        let fine = ui.input(|input| input.modifiers.shift);
        let delta = knob_drag_delta(response.drag_delta().y, span, fine);
        let before = *value;
        let next = ui.data_mut(|data| {
            let held = data.get_temp::<f32>(continuous).unwrap_or(*value);
            let next = (held + delta).clamp(min.min(max), max.max(min));
            data.insert_temp(continuous, next);
            next
        });
        *value = next;
        if (*value - before).abs() > f32::EPSILON {
            response.mark_changed();
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove_temp::<f32>(continuous));
    }
    if ui.is_rect_visible(rect) {
        let fraction = |v: f32| {
            if span.abs() > f32::EPSILON {
                ((v - min) / span).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        paint_dial(
            ui.painter(),
            rect,
            fraction(*value),
            default.map(fraction),
            colour,
            response.hovered() || response.dragged(),
        );
    }
    response
}

/// Paint a knob's face into `rect` at `fraction` of its travel.
pub fn paint_dial(
    painter: &egui::Painter,
    rect: Rect,
    fraction: f32,
    default: Option<f32>,
    colour: Color32,
    lit: bool,
) {
    const START: f32 = std::f32::consts::PI * 0.75;
    const SWEEP: f32 = std::f32::consts::PI * 1.5;
    let size = rect.width().min(rect.height());
    let centre = rect.center();
    let radius = size / 2.0 - 2.5;
    let track = Stroke::new(if size < 30.0 { 2.0 } else { 3.0 }, line_strong());
    paint::arc(painter, centre, radius, START, SWEEP, track);
    if fraction > 0.002 {
        paint::arc(
            painter,
            centre,
            radius,
            START,
            SWEEP * fraction,
            Stroke::new(track.width, colour),
        );
    }
    if let Some(default) = default {
        let angle = START + SWEEP * default;
        let direction = Vec2::angled(angle);
        paint::rounded_line(
            painter,
            centre + direction * (radius + 3.0),
            centre + direction * (radius + 6.0),
            Stroke::new(1.4, faint()),
        );
    }
    let face = (radius - if size < 30.0 { 5.0 } else { 7.5 }).max(4.0);
    let p = palette();
    painter.circle_filled(centre + Vec2::new(0.0, 1.5), face + 0.5, p.knob_shadow);
    paint::gradient_circle(painter, centre, face, p.knob_top, p.knob_bottom);
    painter.circle_stroke(
        centre,
        face,
        Stroke::new(
            1.0,
            if lit {
                line_strong().lerp_to_gamma(text(), 0.25)
            } else {
                p.knob_edge
            },
        ),
    );
    let direction = Vec2::angled(START + SWEEP * fraction);
    paint::rounded_line(
        painter,
        centre + direction * (face * 0.22),
        centre + direction * (face * 0.8),
        Stroke::new(if size < 30.0 { 1.6 } else { 2.2 }, text()),
    );
}

/// A block's on/off drawn as the footswitch your foot presses: the pedal's
/// LED ring, lit in `led` with a glow when on and dashed when off, around a
/// stomp button. `diameter` is the knob size it sits beside.
pub fn paint_footswitch(
    painter: &egui::Painter,
    rect: Rect,
    on: bool,
    led: Color32,
    hovered: bool,
) {
    let size = rect.width().min(rect.height());
    let centre = rect.center();
    let radius = size / 2.0 - 3.0;
    if on {
        painter.circle_stroke(centre, radius + 1.0, Stroke::new(7.0, alpha(led, 0.22)));
        painter.circle_stroke(centre, radius, Stroke::new(2.6, led));
    } else {
        paint::dashed_circle(painter, centre, radius, Stroke::new(1.6, faint()), 3.0, 3.0);
    }
    painter.circle(
        centre,
        (radius - 6.0).max(2.0),
        bg_deep(),
        Stroke::new(1.0, line_strong()),
    );
    let p = palette();
    let button = (radius - 10.0).max(2.0);
    paint::radial_circle(painter, centre, button, p.switch_top, p.switch_bottom, 0.24);
    painter.circle_stroke(
        centre,
        button,
        Stroke::new(
            1.0,
            if hovered {
                line_strong().lerp_to_gamma(text(), 0.3)
            } else {
                p.switch_edge
            },
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::knob_drag_delta;

    #[test]
    fn shift_drag_is_ten_times_finer_than_an_ordinary_knob_drag() {
        let ordinary = knob_drag_delta(-20.0, 100.0, false);
        let fine = knob_drag_delta(-20.0, 100.0, true);
        assert_eq!(ordinary, 10.0);
        assert_eq!(fine, 1.0);
    }
}
