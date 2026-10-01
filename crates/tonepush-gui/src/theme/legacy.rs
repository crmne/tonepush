//! The editor's earlier widgets, drawn in the redesign's tokens until the
//! pages that use them are redrawn: the chain tiles, junctions and markers,
//! the model shelf's chips and tiles, the place marks, and the old knob.

use egui::{Color32, CornerRadius, Response, Sense, Stroke, Ui, Vec2};

use super::*;

/// A centered question that must be answered before work continues.
pub fn modal(title: &'static str) -> egui::Window<'static> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
}

/// A colour written as the three bytes it is: the global EQ's band colours.
pub fn rgb((r, g, b): (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The consistent break between major sections in a tool window.
pub fn section_break(ui: &mut Ui) {
    ui.add_space(18.0);
    ui.separator();
    ui.add_space(10.0);
}

/// An animated busy indicator paced independently of the graphics driver.
///
/// egui's stock spinner requests an immediate frame. That normally relies on
/// vsync to cap the loop, but EGL drivers are allowed to ignore the requested
/// swap interval; on those machines one visible spinner consumes a whole CPU
/// core. Thirty frames per second is smooth for this small indicator and keeps
/// the rest of the interface responsive without busy-rendering.
pub fn spinner(ui: &mut Ui) -> Response {
    let size = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator));

    if ui.is_rect_visible(rect) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        let radius = size / 2.0 - 2.0;
        let start = ui.input(|input| input.time) * std::f64::consts::TAU;
        let sweep = 240_f64.to_radians();
        let points = (0..16)
            .map(|i| {
                let angle = start + sweep * f64::from(i) / 15.0;
                let (sin, cos) = angle.sin_cos();
                rect.center() + radius * egui::vec2(cos as f32, sin as f32)
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(2.0, accent())));
    }

    response
}

/// One block in the chain, tinted with its category's own colour.
///
/// The colours are HX Edit's, read from its catalog rather than invented, so a
/// chain here reads the same as a chain there: amber distortion, yellow EQ and
/// dynamics, red amps, green delay. A bypassed block is drawn dim and its name
/// bracketed, which is also what HX Edit does.
pub fn block_button_tinted(
    ui: &mut Ui,
    name: &str,
    category: Option<&str>,
    artwork: Option<&Art>,
    selected: bool,
    enabled: bool,
    accent: Color32,
) -> Response {
    let size = Vec2::new(BLOCK_WIDTH, BLOCK_HEIGHT);
    // Draggable as well as clickable: a chain is an order, and dragging is how
    // people reorder things.
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());

    if ui.is_rect_visible(rect) {
        // The category colour carries the meaning; the fill only has to keep
        // it legible. A bypassed block loses its colour, which is the whole
        // point of bypassing it.
        let tint = if enabled { accent } else { muted() };
        let fill = if selected {
            if enabled {
                accent.gamma_multiply(0.23)
            } else {
                // Selection must not make a bypassed block look engaged. A
                // brighter neutral surface says “being edited” while its dim
                // artwork and category edge continue to say “off”.
                hover()
            }
        } else if !enabled {
            tile_off()
        } else if response.hovered() {
            hover()
        } else {
            panel()
        };
        let border = if response.dragged() {
            Stroke::new(2.0_f32, super::accent())
        } else {
            Stroke::new(
                1.5_f32,
                tint.gamma_multiply(if selected || response.hovered() {
                    1.0
                } else {
                    0.78
                }),
            )
        };

        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(6), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            border,
            egui::StrokeKind::Inside,
        );

        let text_colour = if !enabled {
            muted()
        } else if category.is_some() {
            accent
        } else {
            text()
        };
        if let Some(art) = artwork {
            let box_ = egui::Rect::from_center_size(
                rect.center() - Vec2::new(0.0, 15.0),
                Vec2::new(76.0, 58.0),
            );
            let tint = if enabled { accent } else { faint() };
            art.paint(ui, box_, tint);
        }

        // Model names routinely exceed the tile, so they are truncated with an
        // ellipsis; the full name is on the hover tooltip.
        //
        // The category goes underneath rather than into the name. An Amp+Cab
        // block holds two models and saying so in the name - "Cali Rectifire +
        // Cab" - only pushed the name itself off the tile. HX Edit puts the
        // category here for the same reason.
        let name_y = if category.is_some() { 25.0 } else { 13.0 };
        ui.painter().text(
            rect.center_bottom() - Vec2::new(0.0, name_y),
            egui::Align2::CENTER_CENTER,
            elide(name, 15),
            egui::FontId::proportional(11.0),
            text_colour,
        );
        if let Some(category) = category {
            ui.painter().text(
                rect.center_bottom() - Vec2::new(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                category_short(category),
                egui::FontId::proportional(9.5),
                if enabled {
                    accent
                } else {
                    muted().gamma_multiply(0.6)
                },
            );
        }
    }

    response.on_hover_text(name)
}

/// A narrow card for a processor whose chain position is fixed.
///
/// The category image owns the top of the card while the model wraps above its
/// type underneath. Keeping those regions separate gives both the artwork and
/// the horizontal text room to breathe without making a fixed chain wider.
pub fn fixed_block_button_tinted(
    ui: &mut Ui,
    name: &str,
    category: &str,
    artwork: Option<&Art>,
    selected: bool,
    enabled: bool,
    accent: Color32,
) -> Response {
    const WIDTH: f32 = 76.0;
    const HEIGHT: f32 = 88.0;
    let size = Vec2::new(WIDTH, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response.on_hover_text(name);
    }

    let tint = if enabled { accent } else { muted() };
    let fill = if selected {
        if enabled {
            accent.gamma_multiply(0.23)
        } else {
            hover()
        }
    } else if !enabled {
        tile_off()
    } else if response.hovered() {
        hover()
    } else {
        panel()
    };
    let border = Stroke::new(
        if selected { 2.0 } else { 1.5 },
        tint.gamma_multiply(if selected || response.hovered() {
            1.0
        } else {
            0.78
        }),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(6),
        border,
        egui::StrokeKind::Inside,
    );

    let text_colour = if enabled { accent } else { muted() };
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 18.0),
        Vec2::splat(22.0),
    );
    if let Some(artwork) = artwork {
        artwork.paint(ui, icon, if enabled { accent } else { faint() });
    } else {
        painter.circle_filled(icon.center(), 4.0, tint);
    }

    let (first, second) = compact_name_lines(name, 12);
    for (line, y) in [
        (first.as_str(), rect.top() + 44.0),
        (second.as_str(), rect.top() + 57.0),
    ] {
        painter.text(
            egui::pos2(rect.center().x, y),
            egui::Align2::CENTER_CENTER,
            line,
            egui::FontId::proportional(9.5),
            text_colour,
        );
    }
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 10.0),
        egui::Align2::CENTER_CENTER,
        category_short(category),
        egui::FontId::proportional(8.0),
        if enabled {
            tint.gamma_multiply(0.82)
        } else {
            muted().gamma_multiply(0.6)
        },
    );

    response.on_hover_text(format!("{name} · {category}"))
}

fn compact_name_lines(text: &str, width: usize) -> (String, String) {
    let text = text.trim();
    if text.chars().count() <= width {
        return (text.to_owned(), String::new());
    }
    let mut split = text
        .char_indices()
        .take_while(|(index, _)| *index <= width)
        .filter_map(|(index, character)| character.is_whitespace().then_some(index))
        .last()
        .unwrap_or_else(|| {
            text.char_indices()
                .nth(width)
                .map_or(text.len(), |(index, _)| index)
        });
    if split == 0 {
        split = text
            .char_indices()
            .nth(width)
            .map_or(text.len(), |(index, _)| index);
    }
    let first = text[..split].trim().to_owned();
    let rest = text[split..].trim();
    (first, elide(rest, width + 1))
}

/// A small filled circle. Painted rather than typed, because the bundled font
/// has no glyph for one and an empty box is worse than no indicator at all.
pub fn status_dot(ui: &mut Ui, colour: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(rect.center(), 5.0, colour);
    }
    response
}

/// Whether the same tone is in the other place, and whether it is the same.
///
/// One vocabulary, used beside a preset on the pedal (about the library) and
/// beside a tone in the library (about the pedal), so a person learns it once.
/// The same three words will do for the web later, which is the reason to
/// settle it now rather than invent it twice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    /// Not over there at all.
    Absent,
    /// Over there, and the same bytes.
    Same,
    /// Over there under this name, and different.
    Differs,
    /// On its way there now.
    Working,
    /// Not knowable yet, because the pedal has not been read.
    Unknown,
}

/// The same, in a colour of its own - what a favourite star needs, since being
/// on is a state rather than a hover.
pub fn tinted_icon_button(
    ui: &mut Ui,
    icon: Icon,
    enabled: bool,
    tint: Option<Color32>,
) -> Response {
    sized_icon_button(ui, icon, enabled, tint, 24.0)
}

/// A smaller one, for a list row - where a 24-pixel target next to 13-pixel
/// text is the loudest thing on the line.
pub fn small_icon_button(ui: &mut Ui, icon: Icon, tint: Option<Color32>) -> Response {
    sized_icon_button(ui, icon, true, tint, 16.0)
}

fn sized_icon_button(
    ui: &mut Ui,
    icon: Icon,
    enabled: bool,
    tint: Option<Color32>,
    side: f32,
) -> Response {
    let mut button = IconButton::new(icon)
        .side(side, side - 6.0)
        .enabled(enabled);
    if let Some(tint) = tint {
        button = button.tint(tint);
    }
    button.show(ui)
}

/// The gap a dragged block would land in, filled so there is no mistaking
/// it: a bar the height of the blocks either side, in the accent.
pub fn insert_marker(ui: &Ui, rect: egui::Rect) {
    let bar = egui::Rect::from_center_size(rect.center(), Vec2::new(5.0, BLOCK_HEIGHT * 0.9));
    ui.painter()
        .rect_filled(bar, CornerRadius::same(3), accent());
}

/// The dragged block, riding along under the pointer so the hand knows what
/// it is holding. A plain tile - name and category colour - floating above
/// everything on its own layer.
///
/// Centred on the pointer, deliberately: the tile is what the eye aims with,
/// and an offset tile meant a drop that looked right landed one gap over.
pub fn drag_ghost(ctx: &egui::Context, at: egui::Pos2, name: &str, colour: Color32) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("drag-ghost"),
    ));
    let rect = egui::Rect::from_center_size(at, Vec2::new(BLOCK_WIDTH * 0.8, BLOCK_HEIGHT * 0.55));
    painter.rect_filled(rect, CornerRadius::same(5), raised());
    painter.rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(2.0_f32, colour),
        egui::StrokeKind::Middle,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        elide(name, 13),
        egui::FontId::proportional(11.0),
        text(),
    );
}

/// The category's colour, as a bar beside the name it belongs to.
pub fn category_swatch(ui: &mut Ui, colour: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(6.0, 22.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(3), colour);
    }
    response
}

/// The colour a footswitch lights, drawn as the light rather than as a chip.
///
/// A bar does for a category, which is a label. This stands for an LED under
/// your foot, so it is round and it glows - and beside a single value a bar
/// reads as a divider between two controls rather than as a colour.
pub fn led_dot(ui: &mut Ui, colour: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(14.0, 22.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.circle_filled(rect.center(), 4.0, colour);
        painter.circle_filled(rect.center(), 6.5, colour.gamma_multiply(0.22));
    }
    response
}

/// A category, as a chip in that category's own colour, with HX Edit's own
/// glyph for it where one is installed.
///
/// The icons are monochrome silhouettes, so they are tinted to match the text
/// rather than drawn as they come - which also means the chip still reads when
/// it is filled and the text goes black.
pub fn category_chip(
    ui: &mut Ui,
    name: &str,
    icon: Option<&Art>,
    colour: Color32,
    on: bool,
) -> Response {
    const ICON: f32 = 14.0;
    const GAP: f32 = 5.0;

    let ink = if on { Color32::BLACK } else { colour };
    let galley =
        ui.painter()
            .layout_no_wrap(name.to_owned(), egui::FontId::proportional(12.0), ink);
    let art_width = icon.map_or(0.0, |_| ICON + GAP);
    let size = Vec2::new(galley.size().x + art_width + 16.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(11), colour);
    } else {
        painter.rect_stroke(
            rect,
            CornerRadius::same(11),
            Stroke::new(
                1.0_f32,
                colour.gamma_multiply(if response.hovered() { 1.0 } else { 0.5 }),
            ),
            egui::StrokeKind::Middle,
        );
    }
    // Icon and label as one group, centred together.
    let left = rect.center().x - (galley.size().x + art_width) / 2.0;
    if let Some(icon) = icon {
        icon.paint(
            ui,
            egui::Rect::from_min_size(
                egui::pos2(left, rect.center().y - ICON / 2.0),
                Vec2::splat(ICON),
            ),
            ink,
        );
    }
    painter.galley(
        egui::pos2(left + art_width, rect.center().y - galley.size().y / 2.0),
        galley,
        text(),
    );
    response
}

/// A category in the model browser's navigation rail.
///
/// Unlike a chip, this consumes a predictable single row. That lets the
/// category vocabulary sit beside the models instead of pushing them several
/// rows down whenever the browser is narrow.
pub fn category_rail_row(
    ui: &mut Ui,
    name: &str,
    icon: Option<&Art>,
    colour: Color32,
    on: bool,
) -> Response {
    const HEIGHT: f32 = 28.0;
    const ICON: f32 = 14.0;
    let size = Vec2::new(ui.available_width(), HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let painter = ui.painter();
    let fill = if on {
        colour.gamma_multiply(0.23)
    } else if response.hovered() {
        hover()
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(4), fill);

    let mut left = rect.left() + 6.0;
    if let Some(icon) = icon {
        icon.paint(
            ui,
            egui::Rect::from_min_size(
                egui::pos2(left, rect.center().y - ICON / 2.0),
                Vec2::splat(ICON),
            ),
            colour,
        );
        left += ICON + 6.0;
    }
    painter.text(
        egui::pos2(left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        elide(name, 15),
        egui::FontId::proportional(11.0),
        if on { text() } else { muted() },
    );
    response
}

/// A subcategory, as a smaller pill under the category it belongs to.
///
/// Deliberately quieter than [`category_chip`]: no icon, no colour of its own,
/// and shorter. Mono / Stereo / Legacy is a second question you only ask after
/// the first one, and a pill that shouted as loud as Distortion would make the
/// row above it look like a sibling rather than a parent.
pub fn shelf_pill(ui: &mut Ui, name: &str, on: bool) -> Response {
    let ink = if on { bg() } else { muted() };
    let galley =
        ui.painter()
            .layout_no_wrap(name.to_owned(), egui::FontId::proportional(11.0), ink);
    let size = Vec2::new(galley.size().x + 14.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(9), text());
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(9), raised());
    }
    painter.galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        ink,
    );
    response
}

/// One model in the browser, as a picture with its name under it.
///
/// A grid of thumbnails the way Logic's Pedalboard shows its shelf: with a few
/// hundred models to choose from, the picture is what you actually recognise.
pub fn model_tile(
    ui: &mut Ui,
    name: &str,
    artwork: Option<&Art>,
    selected: bool,
    accent: Color32,
    size: Vec2,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(
        rect,
        CornerRadius::same(6),
        if selected {
            accent.gamma_multiply(0.23)
        } else if response.hovered() {
            hover()
        } else {
            panel()
        },
    );
    if selected {
        painter.rect_stroke(
            rect.shrink(0.75),
            CornerRadius::same(6),
            Stroke::new(2.0_f32, accent),
            egui::StrokeKind::Inside,
        );
    } else {
        let outline = if response.hovered() {
            line_strong()
        } else {
            line()
        };
        painter.rect_stroke(
            rect.shrink(0.5),
            CornerRadius::same(6),
            Stroke::new(1.0_f32, outline),
            egui::StrokeKind::Inside,
        );
    }

    let art = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 8.0, rect.top() + 6.0),
        Vec2::new(size.x - 16.0, (size.y - 32.0).max(64.0)),
    );
    match artwork {
        Some(a) => a.paint(ui, art, Color32::WHITE),
        None => {
            painter.rect_filled(art, CornerRadius::same(3), raised());
        }
    }

    // A browser model has one caption, not the category row carried by a
    // signal-chain block. Keep it to one line and leave the full name on hover.
    painter.text(
        egui::pos2(rect.center().x, art.bottom() + 4.0),
        egui::Align2::CENTER_TOP,
        elide(name, 16),
        egui::FontId::proportional(12.0),
        if selected { accent } else { text() },
    );

    response.on_hover_text(name)
}

/// Signal-path geometry. Fixed rather than derived so the two lanes of a split
/// line up column for column, which is the whole point of drawing them stacked.
pub const BLOCK_WIDTH: f32 = 96.0;
pub const BLOCK_HEIGHT: f32 = 108.0;
pub const WIRE_WIDTH: f32 = 22.0;
pub const JUNCTION_WIDTH: f32 = 34.0;
/// Height of one lane including the gap under it.
pub const LANE_HEIGHT: f32 = BLOCK_HEIGHT + 10.0;
/// One block and the wire that follows it.
pub const COLUMN: f32 = BLOCK_WIDTH + WIRE_WIDTH;

/// The pedal, drawn as large as it goes without inventing detail.
///
/// HX Edit's artwork is 128 to 256 pixels square. Asking for more than that
/// stretches it, and a stretched pedal looks worse than a small sharp one, so
/// this never scales past 1:1 - it only shrinks to fit `max`.
pub fn pedal_image(ui: &mut Ui, art: &Art, max: f32) -> Response {
    let image = egui::Image::new(&art.uri).maintain_aspect_ratio(true);
    let mut natural = image
        .load_and_calc_size(ui, Vec2::splat(f32::INFINITY))
        .unwrap_or(Vec2::splat(max));
    // One frame of a strip is as tall as it is wide; the file is the whole
    // strip, so its height is the frame count times that.
    if let Some((_, total)) = art.frame {
        if total > 0 {
            natural.y /= total as f32;
        }
    }

    let scale = (max / natural.x).min(max / natural.y).min(1.0);
    let size = natural * scale;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        art.paint(ui, rect, Color32::WHITE);
    }
    response
}

fn elide(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max - 1).collect::<String>() + "…"
}

/// A category in the compact vocabulary used on the signal-chain cards.
///
/// The card already carries the full model name, so the second line should be
/// the quick answer to “what kind of block is this?” rather than another long
/// label competing for the same width.
fn category_short(category: &str) -> String {
    match category {
        "Distortion" => "DIST".to_owned(),
        "Dynamics" => "DYN".to_owned(),
        "Modulation" => "MOD".to_owned(),
        "Pitch/Synth" => "PITCH/SYNTH".to_owned(),
        "Amp+Cab" => "AMP+CAB".to_owned(),
        "Volume/Pan" => "VOLUME/PAN".to_owned(),
        "Send/Return" => "SEND/RETURN".to_owned(),
        known @ ("EQ" | "Delay" | "Reverb" | "Filter" | "Wah" | "Amp" | "Preamp" | "Cab" | "IR"
        | "Looper" | "Input" | "Output" | "Split" | "Merge") => known.to_uppercase(),
        other => other.to_uppercase(),
    }
}

/// How much room a knob takes, across and down. Public because anything
/// centring a knob against something else has to know what it costs.
pub const KNOB: f32 = 64.0;

/// A rotary knob, the way a pedal has them.
///
/// Sliders are fine for a mixer but wrong for a stompbox: the whole point of the
/// artwork is that the thing looks like the pedal you already know, and a pedal
/// has knobs. Dragging vertically turns it, which is what every audio
/// application does and what the hand expects.
pub fn knob(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> Response {
    let size = Vec2::splat(KNOB);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());

    let (min, max) = (*range.start(), *range.end());
    let span = max - min;
    let drag_value = response.id.with("continuous knob value");
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(drag_value, *value));
    }
    if response.dragged() && span.abs() > f32::EPSILON {
        // Ableton Live and Cubase both use Shift-drag for finer control. Keep
        // the unrounded value in egui's temporary state so stepped parameters
        // can accumulate that fine motion instead of losing every sub-step
        // when their caller rounds the displayed/device value.
        let fine = ui.input(|input| input.modifiers.shift);
        let delta = super::widgets::knob_drag_delta(response.drag_delta().y, span, fine);
        let before = *value;
        let continuous = ui.data_mut(|data| {
            let continuous = data.get_temp::<f32>(drag_value).unwrap_or(*value);
            let continuous = (continuous + delta).clamp(min.min(max), max.max(min));
            data.insert_temp(drag_value, continuous);
            continuous
        });
        *value = continuous;
        if (*value - before).abs() > f32::EPSILON {
            response.mark_changed();
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove_temp::<f32>(drag_value));
    }

    if ui.is_rect_visible(rect) {
        let centre = rect.center();
        let face = rect.width() * 0.365;
        let track = rect.width() * 0.455;
        let fraction = if span.abs() > f32::EPSILON {
            ((*value - min) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Knobs sweep 270°, leaving a gap at the bottom so the pointer position
        // is unambiguous.
        let start = std::f32::consts::PI * 0.75;
        let sweep = std::f32::consts::PI * 1.5;
        let angle = start + sweep * fraction;

        let painter = ui.painter();

        // The complete sweep stays visible behind the travelled value. This
        // makes a knob readable before looking at its number, while the layered
        // face gives it the depth of the concept without pretending to be a
        // photograph of hardware.
        paint_arc(
            painter,
            centre,
            track,
            start,
            sweep,
            1.0,
            Stroke::new(3.0_f32, line_strong()),
        );
        paint_arc(
            painter,
            centre,
            track,
            start,
            sweep,
            fraction,
            Stroke::new(3.2_f32, accent()),
        );

        painter.circle_filled(
            centre + Vec2::new(0.0, 2.0),
            face + 1.0,
            Color32::from_black_alpha(120),
        );
        painter.circle_filled(centre, face, palette().knob_bottom);
        painter.circle_stroke(
            centre,
            face,
            Stroke::new(
                1.4_f32,
                if response.hovered() {
                    muted()
                } else {
                    palette().knob_edge
                },
            ),
        );
        let direction = Vec2::new(angle.cos(), angle.sin());
        let pointer = [centre + direction * 2.0, centre + direction * (face * 0.76)];
        painter.line_segment(pointer, Stroke::new(2.5_f32, text()));
    }

    response
}

fn paint_arc(
    painter: &egui::Painter,
    centre: egui::Pos2,
    radius: f32,
    start: f32,
    sweep: f32,
    fraction: f32,
    stroke: Stroke,
) {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= f32::EPSILON {
        return;
    }
    let steps = (36.0 * fraction).ceil().max(2.0) as usize;
    let points: Vec<egui::Pos2> = (0..=steps)
        .map(|i| {
            let t = fraction * i as f32 / steps as f32;
            let angle = start + sweep * t;
            centre + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points.clone(), stroke));
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        painter.circle_filled(*first, stroke.width * 0.5, stroke.color);
        painter.circle_filled(*last, stroke.width * 0.5, stroke.color);
    }
}

/// A footswitch-style toggle, for the parameters a pedal exposes as a switch.
///
/// Sized to sit in the same grid cell as a knob so a row of controls lines up
/// whatever mix of the two a model happens to have.
/// Where a tone is, and whether it is the same there.
///
/// One icon per place, always in the same order, so a row says at a glance
/// which of the three have it. The state is in the ink, not the shape: an
/// outline means it is not there, solid means it is there and identical, amber
/// means it is there and different.
///
/// This replaced a single dot that meant "the other place", which could not
/// work: beside a preset the other place was the library, in the library it was
/// the pedal, and the same amber dot therefore meant two opposite things
/// depending on where you were standing.
pub fn place(ui: &mut Ui, icon: Icon, state: Sync) -> Response {
    place_enabled(
        ui,
        icon,
        state,
        !matches!(state, Sync::Unknown | Sync::Working),
    )
}

/// Draw a place whose actionability is decided by its caller.
///
/// Most place marks derive this from their sync state, but a Push column is
/// still useful before presence is known. Keeping the two facts separate also
/// means a visible action never silently ignores a click.
pub fn place_enabled(ui: &mut Ui, icon: Icon, state: Sync, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(18.0, 16.0), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hot = response.hovered() && enabled;
    let tint = match state {
        Sync::Differs => accent(),
        Sync::Working => {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
            let pulse = ui.input(|input| (input.time * 5.0).sin() as f32 * 0.18 + 0.82);
            accent().gamma_multiply(pulse)
        }
        Sync::Same => text(),
        Sync::Absent => {
            if hot {
                text()
            } else {
                wire()
            }
        }
        Sync::Unknown => {
            if hot {
                text()
            } else {
                wire()
            }
        }
    };
    if let Some((_, uri, _)) = UI_ICONS.iter().find(|(i, _, _)| *i == icon) {
        let art = Art::whole((*uri).to_owned());
        let inside = egui::Rect::from_center_size(rect.center(), Vec2::splat(14.0));
        art.paint(ui, inside, tint);
    }
    // A tone that is there but different gets a mark rather than only a change
    // of colour, so it survives being looked at quickly and being looked at by
    // somebody who does not see amber.
    if state == Sync::Differs {
        ui.painter()
            .circle_filled(rect.right_top() + Vec2::new(-2.0, 3.0), 2.5, accent());
    }
    response
}

/// How far a tag's text sits from its edges.
const TAG_PAD: Vec2 = Vec2::new(4.0, 1.0);

/// A small tag in a block's corner, saying what reaches it.
///
/// In the corner rather than in the block's face, because the face is the
/// model's artwork and its name, and those are what a person is reading when
/// they scan a chain. This is for the second look.
pub fn block_tag(ui: &Ui, block: egui::Rect, text: &str, colour: Color32) {
    let galley = tag_text(ui, text);
    let size = galley.size() + TAG_PAD * 2.0;
    let rect = egui::Rect::from_min_size(
        egui::Pos2::new(block.right() - size.x - 3.0, block.top() + 3.0),
        size,
    );
    paint_tag(ui, rect, galley, colour);
}

/// The same tag, in the run of a line rather than in a corner.
///
/// FS1 is written one way wherever it appears - on the block in the chain,
/// beside the on/off switch, in the assignments table - so that seeing it in
/// three places is seeing one thing three times, and not three spellings of it.
/// It carries the block's colour for the same reason.
pub fn tag(ui: &mut Ui, text: &str, colour: Color32) -> Response {
    let galley = tag_text(ui, text);
    let (rect, response) = ui.allocate_exact_size(galley.size() + TAG_PAD * 2.0, Sense::click());
    if ui.is_rect_visible(rect) {
        paint_tag(ui, rect, galley, colour);
    }
    response
}

fn tag_text(ui: &Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    ui.painter()
        .layout_no_wrap(text.to_owned(), egui::FontId::proportional(9.0), bg())
}

fn paint_tag(ui: &Ui, rect: egui::Rect, galley: std::sync::Arc<egui::Galley>, colour: Color32) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), colour);
    painter.galley(rect.min + TAG_PAD, galley, bg());
}

/// A footswitch, drawn as one.
///
/// A block's bypass is the thing you press with your foot, so it is drawn as
/// the thing you press with your foot and it sits with the block's other
/// controls, not as a tick box in a header three inches away. The ring above it
/// is the pedal's own LED: lit in the colour the *device* gives it when the
/// block is engaged, dark when it is bypassed, and hollow when no footswitch
/// carries it at all.
pub fn footswitch(ui: &mut Ui, engaged: bool, lit: Option<Color32>, carried: bool) -> Response {
    // The same cell a knob takes, so a row of controls lines up whatever
    // is in it.
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(KNOB), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let colour = lit.unwrap_or(accent());
    let centre = rect.center() + Vec2::new(0.0, 4.0);

    // The LED, above the switch, where it is on the pedal.
    let led = egui::Pos2::new(rect.center().x, rect.top() + 5.0);
    if engaged {
        painter.circle_filled(led, 3.5, colour);
        // A soft halo, so a lit LED reads as lit rather than as a dot.
        painter.circle_filled(led, 6.0, colour.gamma_multiply(0.22));
    } else {
        painter.circle_stroke(led, 3.5, Stroke::new(1.2_f32, colour.gamma_multiply(0.55)));
    }

    // The switch itself: a hex nut and a stomp button, which is what one looks
    // like from above.
    let body = 13.0;
    let ring = if response.hovered() {
        muted()
    } else {
        line_strong()
    };
    painter.circle_filled(centre, body, bg_deep());
    painter.circle_stroke(centre, body, Stroke::new(1.4_f32, ring));
    painter.circle_filled(
        centre,
        body - 4.0,
        if engaged {
            palette().switch_top
        } else {
            raised()
        },
    );
    // Nothing carries it: say so with a dashed feel rather than a full ring.
    if !carried {
        painter.circle_stroke(
            centre,
            body + 3.0,
            Stroke::new(1.0_f32, muted().gamma_multiply(0.35)),
        );
    }
    response
}

pub fn switch(on: &mut bool) -> impl egui::Widget + '_ {
    move |ui: &mut Ui| {
        let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(KNOB), Sense::click());
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        if ui.is_rect_visible(rect) {
            let body = egui::Rect::from_center_size(rect.center(), Vec2::new(30.0, 30.0));
            let painter = ui.painter();
            painter.rect_filled(body, CornerRadius::same(5), raised());
            painter.rect_stroke(
                body,
                CornerRadius::same(5),
                Stroke::new(1.0_f32, line_strong()),
                egui::StrokeKind::Middle,
            );
            painter.circle_filled(
                body.center(),
                7.0,
                if *on { accent() } else { line_strong() },
            );
        }
        response
    }
}

/// Where the signal forks into a parallel branch, or comes back together.
///
/// Drawn as the wiring itself rather than as a box in the line, which is what
/// it is. The main line runs straight through - a branch is an addition below
/// the line, not a detour of it - and a curve drops away to each branch lane.
/// HX Edit draws it the same way, and it is what makes the moment the path
/// divides legible at a glance. It stays clickable, since a split still has a
/// mode and a join still has levels.
///
/// `opening` curves out to the branches; the merge is the same figure
/// mirrored. `below` is how many branch lanes hang under the main line.
/// `tag` is worn under the dot - "A/B", "XO" - for split types that change
/// how the preset behaves; the default Y goes untagged.
pub fn junction(
    ui: &mut Ui,
    below: usize,
    opening: bool,
    selected: bool,
    tag: Option<&str>,
) -> Response {
    let size = Vec2::new(JUNCTION_WIDTH, BLOCK_HEIGHT);
    // Draggable as well as clickable: the attach point is a position on the
    // line, and positions are things you drag.
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let colour = if selected {
        accent()
    } else if response.hovered() {
        text()
    } else {
        wire()
    };
    let stroke = Stroke::new(if selected { 2.0_f32 } else { 1.5_f32 }, colour);
    let painter = ui.painter();
    let cy = rect.center().y;
    painter.hline(rect.x_range(), cy, stroke);

    // One curve per branch, horizontal at both ends so the wiring reads as
    // wiring: it leaves the line level and arrives at the lane level. Painted
    // past the widget's own rect - the lanes below are still this figure's
    // to meet.
    for n in 1..=below {
        let ty = cy + LANE_HEIGHT * n as f32;
        let (from, to) = if opening {
            (egui::pos2(rect.left(), cy), egui::pos2(rect.right(), ty))
        } else {
            (egui::pos2(rect.left(), ty), egui::pos2(rect.right(), cy))
        };
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [
                from,
                egui::pos2(from.x + JUNCTION_WIDTH, from.y),
                egui::pos2(to.x - JUNCTION_WIDTH, to.y),
                to,
            ],
            false,
            Color32::TRANSPARENT,
            stroke,
        ));
    }
    // A dot on the fork, so it reads as something you can click.
    painter.circle_filled(rect.center(), 4.0, colour);
    if let Some(tag) = tag {
        painter.text(
            rect.center() + Vec2::new(0.0, 13.0),
            egui::Align2::CENTER_CENTER,
            tag,
            egui::FontId::proportional(9.0),
            colour,
        );
    }

    response
}

/// How tall the offer of a parallel branch is; see [`ghost_branch`].
pub const GHOST_HEIGHT: f32 = 40.0;

/// The offer of a parallel branch, dashed because it does not exist yet:
/// where the line would fork, the lane the blocks would sit on, and where it
/// would merge back - with a `+` where the first block goes.
///
/// This replaced a label reading "parallel branch" floating in the signal
/// path, which looked like a thing in the chain rather than an action.
/// `from_y` is the main line's height, where the fork will leave it.
pub fn ghost_branch(ui: &mut Ui, width: f32, from_y: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, GHOST_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let colour = if response.hovered() {
        accent()
    } else {
        wire().gamma_multiply(0.9)
    };
    let stroke = Stroke::new(1.5_f32, colour);
    let painter = ui.painter();
    let y = rect.bottom() - 12.0;
    let reach = 30.0_f32.min(width * 0.25);

    let dashed_curve = |from: egui::Pos2, to: egui::Pos2| {
        let points = egui::epaint::CubicBezierShape::from_points_stroke(
            [
                from,
                egui::pos2(from.x + reach, from.y),
                egui::pos2(to.x - reach, to.y),
                to,
            ],
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        )
        .flatten(Some(0.5));
        egui::Shape::dashed_line(&points, stroke, 4.0, 4.0)
    };
    // Fork out of the main line, and merge back into it.
    painter.extend(dashed_curve(
        egui::pos2(rect.left(), from_y),
        egui::pos2(rect.left() + reach, y),
    ));
    painter.extend(dashed_curve(
        egui::pos2(rect.right() - reach, y),
        egui::pos2(rect.right(), from_y),
    ));

    // The lane itself, leaving room for the `+` at its middle.
    let centre = egui::pos2(rect.center().x, y);
    painter.extend(egui::Shape::dashed_line(
        &[
            egui::pos2(rect.left() + reach, y),
            egui::pos2(centre.x - 14.0, y),
        ],
        stroke,
        4.0,
        4.0,
    ));
    painter.extend(egui::Shape::dashed_line(
        &[
            egui::pos2(centre.x + 14.0, y),
            egui::pos2(rect.right() - reach, y),
        ],
        stroke,
        4.0,
        4.0,
    ));

    // The `+`, always visible: this is the affordance, not a hover surprise.
    if response.hovered() {
        painter.circle_filled(centre, 8.0, accent());
    } else {
        painter.circle_stroke(centre, 8.0, Stroke::new(1.5_f32, colour));
    }
    let mark = if response.hovered() {
        Color32::BLACK
    } else {
        colour
    };
    painter.line_segment(
        [centre - Vec2::new(4.0, 0.0), centre + Vec2::new(4.0, 0.0)],
        Stroke::new(2.0_f32, mark),
    );
    painter.line_segment(
        [centre - Vec2::new(0.0, 4.0), centre + Vec2::new(0.0, 4.0)],
        Stroke::new(2.0_f32, mark),
    );

    response
}

/// One place a dragged fork or merge can land: a dot on the wire, grown and
/// lit when it is the one the pointer would choose.
pub fn attach_marker(ui: &Ui, at: egui::Pos2, hot: bool) {
    let painter = ui.painter();
    if hot {
        painter.circle_filled(at, 5.0, accent());
    } else {
        painter.circle_filled(at, 3.0, wire());
        painter.circle_stroke(at, 3.0, Stroke::new(1.0_f32, muted()));
    }
}

/// Mark a drop that would trade places with the block under the pointer.
pub fn swap_marker(ui: &Ui, rect: egui::Rect) {
    ui.painter().rect_stroke(
        rect.expand(2.0),
        CornerRadius::same(6),
        Stroke::new(3.0_f32, accent()),
        egui::StrokeKind::Middle,
    );
}

/// A gap in the chain you can add something to.
///
/// Drawn as ordinary wire until the pointer is over it, when it offers a `+`.
/// Adding a block was previously only possible by finding an empty slot and
/// changing its model, which meant knowing the slot topology - this puts the
/// action where the thing goes.
pub fn insert_point(ui: &mut Ui, height: f32) -> Response {
    // Click *and drag*: a click-only widget here never completed its click,
    // while the blocks either side - which sense drags - always did. Sensing
    // the drag makes this widget the one that owns the press.
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(WIRE_WIDTH, height), Sense::click_and_drag());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let y = rect.center().y;
    painter.hline(rect.x_range(), y, Stroke::new(1.5_f32, wire()));

    if response.hovered() {
        let centre = egui::pos2(rect.center().x, y);
        painter.circle_filled(centre, 8.0, accent());
        painter.line_segment(
            [egui::pos2(centre.x - 4.0, y), egui::pos2(centre.x + 4.0, y)],
            Stroke::new(2.0_f32, Color32::BLACK),
        );
        painter.line_segment(
            [egui::pos2(centre.x, y - 4.0), egui::pos2(centre.x, y + 4.0)],
            Stroke::new(2.0_f32, Color32::BLACK),
        );
    }
    response
}

/// Blank wire, for padding a short lane out to the merge point.
pub fn wire_run(ui: &mut Ui, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            Stroke::new(1.5_f32, wire()),
        );
    }
}
