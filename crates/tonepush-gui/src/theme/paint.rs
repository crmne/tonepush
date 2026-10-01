//! The drawing the components share: everything the design asks of the
//! painter that egui has no single call for. Flat fills, strokes and rounded
//! rectangles are egui's own; gradients are vertex-coloured meshes, dashed
//! outlines are flattened paths, and letter-spaced captions are layout jobs.

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// One stop of a vertical gradient: where it is, from 0 at the top to 1 at
/// the bottom, and its colour.
pub type Stop = (f32, Color32);

/// The colour of a vertical gradient at `t`, from 0 at the top to 1.
fn colour_at(stops: &[Stop], t: f32) -> Color32 {
    let Some(first) = stops.first() else {
        return Color32::TRANSPARENT;
    };
    if t <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            let span = (t1 - t0).max(f32::EPSILON);
            return lerp_premultiplied(c0, c1, (t - t0) / span);
        }
    }
    stops.last().map_or(Color32::TRANSPARENT, |stop| stop.1)
}

/// Interpolate two colours as the GPU blends them: premultiplied, in sRGB,
/// which is how CSS interpolates `color-mix` and gradients towards
/// transparent.
fn lerp_premultiplied(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// How far in from the left and right a rounded rectangle's edge is at `y`.
fn inset(rect: Rect, radius: f32, y: f32) -> f32 {
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let from_top = y - rect.top();
    let from_bottom = rect.bottom() - y;
    let corner = if from_top < radius {
        radius - from_top
    } else if from_bottom < radius {
        radius - from_bottom
    } else {
        return 0.0;
    };
    radius - (radius * radius - corner * corner).max(0.0).sqrt()
}

/// Fill a rounded rectangle with a vertical gradient.
///
/// Built as horizontal strips, one between each pair of rows where either
/// the outline turns or a stop falls, so a stop part of the way down (a
/// tile's wash fading out at 72 %) lands exactly where it is asked to. The
/// edge has no feathering; everything filled this way also gets an outline
/// on top, which is what keeps it from looking jagged.
pub fn gradient_rect(painter: &Painter, rect: Rect, radius: f32, stops: &[Stop]) {
    if rect.height() <= 0.0 || rect.width() <= 0.0 || stops.is_empty() {
        return;
    }
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut rows: Vec<f32> = Vec::new();
    // The corners, finely enough that the edge reads as an arc.
    let steps = (radius / 1.5).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        rows.push(rect.top() + radius * t);
        rows.push(rect.bottom() - radius * t);
    }
    for &(t, _) in stops {
        rows.push(rect.top() + rect.height() * t.clamp(0.0, 1.0));
    }
    rows.push(rect.top());
    rows.push(rect.bottom());
    rows.retain(|y| (rect.top()..=rect.bottom()).contains(y));
    rows.sort_by(f32::total_cmp);
    rows.dedup_by(|a, b| (*a - *b).abs() < 0.01);

    let mut mesh = Mesh::default();
    for y in &rows {
        let edge = inset(rect, radius, *y);
        let colour = colour_at(stops, (*y - rect.top()) / rect.height());
        mesh.vertices.push(Vertex {
            pos: Pos2::new(rect.left() + edge, *y),
            uv: WHITE_UV,
            color: colour,
        });
        mesh.vertices.push(Vertex {
            pos: Pos2::new(rect.right() - edge, *y),
            uv: WHITE_UV,
            color: colour,
        });
    }
    for row in 0..rows.len().saturating_sub(1) {
        let i = (row * 2) as u32;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
    painter.add(Shape::mesh(mesh));
}

/// Fill a circle with a vertical gradient (a knob's face).
pub fn gradient_circle(
    painter: &Painter,
    centre: Pos2,
    radius: f32,
    top: Color32,
    bottom: Color32,
) {
    let rect = Rect::from_center_size(centre, Vec2::splat(radius * 2.0));
    gradient_rect(painter, rect, radius, &[(0.0, top), (1.0, bottom)]);
}

/// Fill a circle with a radial gradient whose centre sits `lift` of the
/// radius above the middle, the way light falls on a stomp button.
pub fn radial_circle(
    painter: &Painter,
    centre: Pos2,
    radius: f32,
    inner: Color32,
    outer: Color32,
    lift: f32,
) {
    let segments = 40;
    let focus = centre - Vec2::new(0.0, radius * lift);
    let reach = radius * (1.0 + lift);
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: focus,
        uv: WHITE_UV,
        color: inner,
    });
    for i in 0..=segments {
        let angle = std::f32::consts::TAU * i as f32 / segments as f32;
        let pos = centre + Vec2::angled(angle) * radius;
        let t = ((pos - focus).length() / reach).clamp(0.0, 1.0);
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color: lerp_premultiplied(inner, outer, t),
        });
    }
    for i in 1..=segments as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(Shape::mesh(mesh));
}

/// The outline of a rounded rectangle as points, clockwise from the top left
/// corner's end, closed.
pub fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut points = Vec::new();
    let corners = [
        (rect.right_top() + Vec2::new(-radius, radius), -90.0_f32),
        (rect.right_bottom() + Vec2::new(-radius, -radius), 0.0),
        (rect.left_bottom() + Vec2::new(radius, -radius), 90.0),
        (rect.left_top() + Vec2::new(radius, radius), 180.0),
    ];
    let steps = 8;
    for (centre, start) in corners {
        for i in 0..=steps {
            let angle = (start + 90.0 * i as f32 / steps as f32).to_radians();
            points.push(centre + Vec2::angled(angle) * radius);
        }
    }
    if let Some(first) = points.first().copied() {
        points.push(first);
    }
    points
}

/// A dashed outline around a rounded rectangle: a bypassed tile, a free
/// slot, a drop target.
pub fn dashed_rect(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    stroke: Stroke,
    dash: f32,
    gap: f32,
) {
    let points = rounded_outline(rect, radius);
    painter.extend(Shape::dashed_line(&points, stroke, dash, gap));
}

/// A dashed circle.
pub fn dashed_circle(
    painter: &Painter,
    centre: Pos2,
    radius: f32,
    stroke: Stroke,
    dash: f32,
    gap: f32,
) {
    let segments = ((radius * std::f32::consts::TAU) / 1.5).ceil().max(12.0) as usize;
    let points: Vec<Pos2> = (0..=segments)
        .map(|i| centre + Vec2::angled(std::f32::consts::TAU * i as f32 / segments as f32) * radius)
        .collect();
    painter.extend(Shape::dashed_line(&points, stroke, dash, gap));
}

/// An arc from `start` sweeping `sweep` radians, with round caps.
pub fn arc(painter: &Painter, centre: Pos2, radius: f32, start: f32, sweep: f32, stroke: Stroke) {
    if sweep.abs() <= f32::EPSILON {
        return;
    }
    let steps = ((radius * sweep.abs()) / 2.0).ceil().clamp(4.0, 96.0) as usize;
    let points: Vec<Pos2> = (0..=steps)
        .map(|i| centre + Vec2::angled(start + sweep * i as f32 / steps as f32) * radius)
        .collect();
    let (first, last) = (points[0], points[steps]);
    painter.add(Shape::line(points, stroke));
    painter.circle_filled(first, stroke.width / 2.0, stroke.color);
    painter.circle_filled(last, stroke.width / 2.0, stroke.color);
}

/// A line with round caps.
pub fn rounded_line(painter: &Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    painter.line_segment([from, to], stroke);
    painter.circle_filled(from, stroke.width / 2.0, stroke.color);
    painter.circle_filled(to, stroke.width / 2.0, stroke.color);
}

/// The board's dots: one point on a 16-point grid, offset by half a cell.
pub fn dots(painter: &Painter, rect: Rect, colour: Color32) {
    const SPACING: f32 = 16.0;
    let first_x = (rect.left() / SPACING).floor() * SPACING + SPACING / 2.0;
    let first_y = (rect.top() / SPACING).floor() * SPACING + SPACING / 2.0;
    let mut mesh = Mesh::default();
    let mut y = first_y;
    while y < rect.bottom() {
        let mut x = first_x;
        while x < rect.right() {
            if rect.contains(Pos2::new(x, y)) {
                mesh.add_colored_rect(
                    Rect::from_center_size(Pos2::new(x, y), Vec2::splat(1.5)),
                    colour,
                );
            }
            x += SPACING;
        }
        y += SPACING;
    }
    painter.add(Shape::mesh(mesh));
}

/// A soft glow around a rectangle, the one shadow the design allows: under
/// floating layers, the selected tile and the primary button.
pub fn glow(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    offset: Vec2,
    blur: f32,
    spread: f32,
    colour: Color32,
) {
    let shadow = egui::epaint::Shadow {
        offset: [offset.x.round() as i8, offset.y.round() as i8],
        blur: blur.round().clamp(0.0, 255.0) as u8,
        spread: spread.round().clamp(0.0, 255.0) as u8,
        color: colour,
    };
    painter.add(shadow.as_shape(rect, egui::CornerRadius::same(radius.round() as u8)));
}

/// Text with letter spacing, which captions in capitals carry (`+0.07em`).
#[must_use]
pub fn spaced(text: &str, font: egui::FontId, colour: Color32, em: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let spacing = font.size * em;
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: colour,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    job.wrap.max_rows = 1;
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gradient_stop_part_way_down_holds_its_colour() {
        let stops = [(0.0, Color32::WHITE), (0.72, Color32::TRANSPARENT)];
        assert_eq!(colour_at(&stops, 0.0), Color32::WHITE);
        assert_eq!(colour_at(&stops, 0.72), Color32::TRANSPARENT);
        assert_eq!(
            colour_at(&stops, 0.9),
            Color32::TRANSPARENT,
            "past the last stop"
        );
        let half = colour_at(&stops, 0.36);
        assert!((126..=129).contains(&half.a()), "half way {half:?}");
    }

    #[test]
    fn a_rounded_rect_narrows_only_in_its_corners() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 40.0));
        assert_eq!(inset(rect, 12.0, 20.0), 0.0);
        assert!((inset(rect, 12.0, 0.0) - 12.0).abs() < 0.01);
        assert!(inset(rect, 12.0, 6.0) > 0.0 && inset(rect, 12.0, 6.0) < 12.0);
    }

    #[test]
    fn the_outline_closes_on_itself() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 30.0));
        let points = rounded_outline(rect, 8.0);
        assert_eq!(points.first(), points.last());
        assert!(points.iter().all(|p| rect.expand(0.01).contains(*p)));
    }
}
