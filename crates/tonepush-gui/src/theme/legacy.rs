//! What is kept from the editor's earlier widgets, drawn in the redesign's
//! tokens: the EQ's band colours, the paced spinner, the place marks and
//! their vocabulary, and the tile that rides under the pointer in a drag.

use egui::{Color32, CornerRadius, Response, Sense, Stroke, Ui, Vec2};

use super::*;

/// A colour written as the three bytes it is: the global EQ's band colours.
pub fn rgb((r, g, b): (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(r, g, b)
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

/// Whether the same tone is in the other place, and whether it is the same.
///
/// One vocabulary, used beside a preset on the pedal (about the library) and
/// beside a tone in the library (about the pedal), so a person learns it once.
/// The same three words will do for the web later, which is the reason to
/// settle it now rather than invent it twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    #[default]
    Unknown,
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

/// Signal-path geometry. Fixed rather than derived so the two lanes of a split
/// line up column for column, which is the whole point of drawing them stacked.
pub const BLOCK_WIDTH: f32 = 96.0;
pub const BLOCK_HEIGHT: f32 = 108.0;
fn elide(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max - 1).collect::<String>() + "…"
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
