//! The board: the loaded preset's chain on the darker dotted surface
//! (docs/design/redesign-2026-10-01, "Block tile" and "Lanes, wires and
//! junctions").
//!
//! The geometry is worked out first, as plain numbers, and drawn after: tile
//! widths fill the board between a minimum and a maximum, wires are fixed,
//! a fork or merge takes a junction's width, and lanes stack under the main
//! line. The main line never changes height: input, output and everything the
//! undivided signal passes through sit on one row, and a parallel branch hangs
//! below the stretch it parallels, the way HX Edit draws it. Splits and joins
//! are not tiles in the line: they are the wiring forking and merging, a dot
//! you can click for their own parameters and drag along the line.
//!
//! Every gap is where a block can be added or dropped, keyed by the slot a
//! block put there would go before, as the drag and drop that ends here has
//! always worked.

use std::ops::Range;

use egui::epaint::CubicBezierShape;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use hx_proto::preset::{Kind, Path};

use crate::shell;
use crate::theme::{self, Icon, Tier};
use crate::{session, App, Cmd, Connection};

/// The jack an endpoint is drawn as: its radius, and where its wire starts.
pub(crate) const JACK: f32 = 15.0;

/// The room a divided stretch with nothing on any lane still takes, so it can
/// be clicked and dropped into.
fn empty_stretch(g: &Geometry) -> f32 {
    g.wire * 2.0 + 24.0
}

/// The board's measurements at one size, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    /// Room on either side of the chain.
    pub pad: f32,
    /// From the board's top to the first lane's tiles; the header row sits in
    /// it.
    pub top: f32,
    pub bottom: f32,
    /// What an input or output takes along the line.
    pub endpoint: f32,
    /// A wire between two tiles.
    pub wire: f32,
    /// A fork or a merge.
    pub junction: f32,
    pub tile_height: f32,
    /// Between lanes.
    pub lane_gap: f32,
    pub min_tile: f32,
    pub max_tile: f32,
}

impl Geometry {
    /// An HX chain at one of the three sizes.
    pub(crate) fn hx(tier: Tier) -> Geometry {
        Geometry {
            pad: 22.0,
            top: tier.pick(34.0, 40.0, 46.0),
            bottom: tier.pick(18.0, 22.0, 26.0),
            endpoint: 34.0,
            wire: tier.pick(18.0, 24.0, 30.0),
            junction: tier.pick(26.0, 30.0, 34.0),
            tile_height: tier.pick(84.0, 92.0, 128.0),
            lane_gap: tier.pick(10.0, 10.0, 14.0),
            min_tile: tier.pick(72.0, 92.0, 136.0),
            max_tile: tier.pick(104.0, 124.0, 176.0),
        }
    }

    /// A StompStation PRO's chain: narrower tiles and shorter wires, so the
    /// pedal's whole chain fits beside the sidebar.
    pub(crate) fn pro(tier: Tier) -> Geometry {
        Geometry {
            pad: 16.0,
            top: tier.pick(34.0, 40.0, 46.0),
            bottom: tier.pick(18.0, 22.0, 26.0),
            endpoint: 34.0,
            wire: tier.pick(6.0, 8.0, 14.0),
            junction: tier.pick(20.0, 22.0, 30.0),
            tile_height: tier.pick(84.0, 92.0, 128.0),
            lane_gap: tier.pick(10.0, 10.0, 14.0),
            min_tile: tier.pick(64.0, 68.0, 96.0),
            max_tile: tier.pick(88.0, 112.0, 150.0),
        }
    }

    /// From one lane's top to the next.
    pub(crate) fn lane_pitch(&self) -> f32 {
        self.tile_height + self.lane_gap
    }
}

/// One thing along a path, in the order the signal meets it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Piece {
    Input(usize),
    Output(usize),
    Block(usize),
    /// A divided stretch: the fork, its lanes, and the merge.
    Fork(Fork),
}

/// A divided stretch of a path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fork {
    pub split: Option<usize>,
    pub join: Option<usize>,
    pub lanes: Vec<LanePlan>,
}

/// One lane of a divided stretch: its blocks, and the slots it may hold.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LanePlan {
    pub blocks: Vec<usize>,
    pub span: Range<usize>,
}

/// A path as the board meets it.
pub(crate) fn pieces(path: &Path) -> Vec<Piece> {
    let mut pieces = Vec::new();
    if let Some(input) = path.input {
        pieces.push(Piece::Input(input));
    }
    pieces.extend(path.head.iter().copied().map(Piece::Block));
    if !path.lanes.is_empty() {
        pieces.push(Piece::Fork(Fork {
            split: path.split,
            join: path.join,
            lanes: path
                .lanes
                .iter()
                .map(|lane| LanePlan {
                    blocks: lane.blocks.clone(),
                    span: lane.span.clone(),
                })
                .collect(),
        }));
    }
    pieces.extend(path.tail.iter().copied().map(Piece::Block));
    if let Some(output) = path.output {
        pieces.push(Piece::Output(output));
    }
    pieces
}

/// What a run of pieces takes along the line: the fixed width, and how many
/// tiles wide it is.
pub(crate) fn measure(pieces: &[Piece], g: &Geometry) -> (f32, f32) {
    let mut fixed = 0.0;
    let mut units = 0.0;
    for (index, piece) in pieces.iter().enumerate() {
        if index > 0 {
            fixed += if matches!(piece, Piece::Fork(_)) {
                g.junction
            } else {
                g.wire
            };
        }
        match piece {
            Piece::Input(_) => fixed += g.endpoint / 2.0 + JACK,
            Piece::Output(_) => fixed += g.endpoint,
            Piece::Block(_) => units += 1.0,
            Piece::Fork(fork) => {
                let longest = fork
                    .lanes
                    .iter()
                    .map(|lane| lane.blocks.len())
                    .max()
                    .unwrap_or(0);
                if longest == 0 {
                    fixed += empty_stretch(g);
                } else {
                    units += longest as f32;
                    fixed += (longest - 1) as f32 * g.wire;
                }
                // The merge.
                fixed += g.junction;
            }
        }
    }
    (fixed, units)
}

/// How wide a tile is when `available` points hold the run: as wide as fills
/// it, between the minimum and the maximum; and whether even the minimum does
/// not fit, so the board scrolls.
pub(crate) fn tile_width(available: f32, fixed: f32, units: f32, g: &Geometry) -> (f32, bool) {
    if units <= 0.0 {
        return (g.max_tile, false);
    }
    let fitting = (available - fixed) / units;
    if fitting < g.min_tile {
        (g.min_tile, true)
    } else {
        (fitting.min(g.max_tile).floor(), false)
    }
}

/// A wire, straight along a lane or curving between lanes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Wire {
    Straight([Pos2; 2]),
    Curve([Pos2; 4]),
}

/// Where everything on one path landed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Placed {
    /// Each tile by slot, its rectangle, and which lane it is in.
    pub tiles: Vec<(usize, Rect, usize)>,
    /// Each endpoint by slot, the jack's centre, and whether it is the input.
    pub endpoints: Vec<(usize, Pos2, bool)>,
    /// Each fork and merge by slot, its dot, and whether it is the fork.
    pub junctions: Vec<(Option<usize>, Pos2, bool)>,
    /// Each gap by the slot a block dropped in it goes before: the area that
    /// answers, and where its "+" sits on the wire.
    pub gaps: Vec<(usize, Rect, Pos2)>,
    /// Every wire, and whether the signal on it is stereo.
    pub wires: Vec<(Wire, bool)>,
    /// How many lanes the path stacks.
    pub lanes: usize,
    /// From the origin to the last thing drawn.
    pub width: f32,
}

/// Whether the signal leaving a slot is stereo: `Some` when the model says,
/// `None` when it passes on what it was given.
pub(crate) type StereoOf<'a> = &'a dyn Fn(usize) -> Option<bool>;

/// Lay a path out from `origin`, the left edge of its first lane's tiles.
pub(crate) fn place(
    pieces: &[Piece],
    g: &Geometry,
    tile: f32,
    origin: Pos2,
    stereo_of: StereoOf,
    merges_stereo: &dyn Fn(Option<usize>, &[bool]) -> bool,
) -> Placed {
    let mut placed = Placed {
        lanes: 1,
        ..Placed::default()
    };
    let main = origin.y + g.tile_height / 2.0;
    let pitch = g.lane_pitch();
    let gap_rect = |from: f32, to: f32, y: f32| {
        Rect::from_min_max(
            Pos2::new(from, y - g.tile_height / 2.0),
            Pos2::new(to.max(from + 1.0), y + g.tile_height / 2.0),
        )
    };
    let mut x = origin.x;
    let mut right: Option<f32> = None;
    let mut stereo = false;
    for piece in pieces {
        let fork = matches!(piece, Piece::Fork(_));
        if let Some(before) = right {
            x = before + if fork { g.junction } else { g.wire };
            if !fork {
                let (key, end) = match piece {
                    Piece::Block(slot) => (Some(*slot), x),
                    Piece::Output(slot) => (Some(*slot), x + g.endpoint / 2.0 - JACK),
                    _ => (None, x),
                };
                placed.wires.push((
                    Wire::Straight([Pos2::new(before, main), Pos2::new(end, main)]),
                    stereo,
                ));
                if let Some(key) = key {
                    placed.gaps.push((
                        key,
                        gap_rect(before, x, main),
                        Pos2::new((before + x) / 2.0, main),
                    ));
                }
            }
        }
        match piece {
            Piece::Input(slot) => {
                let centre = Pos2::new(x + g.endpoint / 2.0, main);
                placed.endpoints.push((*slot, centre, true));
                right = Some(centre.x + JACK);
            }
            Piece::Output(slot) => {
                let centre = Pos2::new(x + g.endpoint / 2.0, main);
                placed.endpoints.push((*slot, centre, false));
                right = Some(x + g.endpoint);
            }
            Piece::Block(slot) => {
                let rect =
                    Rect::from_min_size(Pos2::new(x, origin.y), Vec2::new(tile, g.tile_height));
                placed.tiles.push((*slot, rect, 0));
                stereo = stereo_of(*slot).unwrap_or(stereo);
                right = Some(x + tile);
            }
            Piece::Fork(fork) => {
                let before = right.unwrap_or(x - g.junction);
                let fork_x = before + 5.0;
                placed.wires.push((
                    Wire::Straight([Pos2::new(before, main), Pos2::new(fork_x, main)]),
                    stereo,
                ));
                placed
                    .junctions
                    .push((fork.split, Pos2::new(fork_x, main), true));
                let longest = fork
                    .lanes
                    .iter()
                    .map(|lane| lane.blocks.len())
                    .max()
                    .unwrap_or(0);
                let stretch = if longest == 0 {
                    empty_stretch(g)
                } else {
                    longest as f32 * tile + (longest - 1) as f32 * g.wire
                };
                let end = x + stretch;
                let merge_x = end + g.junction - 5.0;
                let mut outs = Vec::with_capacity(fork.lanes.len());
                for (k, lane) in fork.lanes.iter().enumerate() {
                    let y = main + k as f32 * pitch;
                    let lane_top = origin.y + k as f32 * pitch;
                    let lead = (x - fork_x) * 0.6;
                    placed.wires.push((
                        if k == 0 {
                            Wire::Straight([Pos2::new(fork_x, main), Pos2::new(x, main)])
                        } else {
                            Wire::Curve([
                                Pos2::new(fork_x, main),
                                Pos2::new(fork_x + lead, main),
                                Pos2::new(x - lead, y),
                                Pos2::new(x, y),
                            ])
                        },
                        stereo,
                    ));
                    let mut at = x;
                    let mut signal = stereo;
                    if lane.blocks.is_empty() && !lane.span.is_empty() {
                        placed.gaps.push((
                            lane.span.start,
                            gap_rect(fork_x + 4.0, end, y),
                            Pos2::new((x + end) / 2.0, y),
                        ));
                    }
                    for (j, slot) in lane.blocks.iter().enumerate() {
                        if j == 0 {
                            placed.gaps.push((
                                *slot,
                                gap_rect(fork_x + 4.0, x, y),
                                Pos2::new((fork_x + x) / 2.0 + 2.0, y),
                            ));
                        } else {
                            placed.wires.push((
                                Wire::Straight([Pos2::new(at, y), Pos2::new(at + g.wire, y)]),
                                signal,
                            ));
                            placed.gaps.push((
                                *slot,
                                gap_rect(at, at + g.wire, y),
                                Pos2::new(at + g.wire / 2.0, y),
                            ));
                            at += g.wire;
                        }
                        placed.tiles.push((
                            *slot,
                            Rect::from_min_size(
                                Pos2::new(at, lane_top),
                                Vec2::new(tile, g.tile_height),
                            ),
                            k,
                        ));
                        signal = stereo_of(*slot).unwrap_or(signal);
                        at += tile;
                    }
                    if at < end {
                        placed.wires.push((
                            Wire::Straight([Pos2::new(at, y), Pos2::new(end, y)]),
                            signal,
                        ));
                    }
                    if let Some(last) = lane.blocks.last() {
                        if lane.blocks.len() < lane.span.len() {
                            placed.gaps.push((
                                last + 1,
                                gap_rect(at, merge_x - 4.0, y),
                                Pos2::new((at.max(end - g.wire) + merge_x) / 2.0, y),
                            ));
                        }
                    }
                    outs.push(signal);
                }
                for (k, out) in outs.iter().enumerate() {
                    let y = main + k as f32 * pitch;
                    let lead = (merge_x - end) * 0.6;
                    placed.wires.push((
                        if k == 0 {
                            Wire::Straight([Pos2::new(end, main), Pos2::new(merge_x, main)])
                        } else {
                            Wire::Curve([
                                Pos2::new(end, y),
                                Pos2::new(end + lead, y),
                                Pos2::new(merge_x - lead, main),
                                Pos2::new(merge_x, main),
                            ])
                        },
                        *out,
                    ));
                }
                stereo = merges_stereo(fork.join, &outs);
                placed
                    .junctions
                    .push((fork.join, Pos2::new(merge_x, main), false));
                let after = end + g.junction;
                placed.wires.push((
                    Wire::Straight([Pos2::new(merge_x, main), Pos2::new(after, main)]),
                    stereo,
                ));
                placed.lanes = placed.lanes.max(fork.lanes.len());
                right = Some(after);
            }
        }
    }
    placed.width = right.map_or(0.0, |right| right - origin.x);
    placed
}

/// How a tile is drawn this frame.
pub(crate) struct TileLook<'a> {
    pub name: &'a str,
    /// The category's short name, in capitals under the name.
    pub caption: &'a str,
    pub colour: Color32,
    pub drawing: Option<theme::Art>,
    pub on: bool,
    pub selected: bool,
    pub hovered: bool,
    /// Ringed in a colour: what the focused footswitch reaches.
    pub highlight: Option<Color32>,
    /// Outlined in dashed amber: the slot the model browser is trying models in.
    pub trying: bool,
    pub compact: bool,
    pub large: bool,
    /// The camera, when snapshots set something on it.
    pub snapshot: bool,
    /// What drives it: a footswitch with the colour it lights, a pedal, MIDI.
    pub drivers: &'a [(String, Option<Color32>)],
    /// The caption is a model's name in the quiet ink, as a StompStation
    /// PRO's tiles say what they hold, rather than the category in capitals.
    pub plain_caption: bool,
    /// A lock on the left of the top edge: the block cannot move.
    pub locked: bool,
}

/// One tile: the category's wash and border, its drawing, name and caption,
/// and the tags hanging on its top edge like tape on a pedal.
pub(crate) fn paint_tile(ui: &Ui, rect: Rect, look: &TileLook) {
    let painter = ui.painter();
    let radius = f32::from(theme::RADIUS_TILE);
    let corner = CornerRadius::same(theme::RADIUS_TILE);
    let cat = look.colour;
    if look.on {
        // The glow under the tile, in its own colour.
        theme::paint::glow(
            painter,
            rect.shrink2(Vec2::new(10.0, 6.0)),
            radius,
            Vec2::new(0.0, if look.selected { 10.0 } else { 8.0 }),
            if look.selected { 22.0 } else { 16.0 },
            0.0,
            theme::alpha(cat, if look.selected { 0.55 } else { 0.28 }),
        );
        if look.selected {
            painter.rect_stroke(
                rect.expand(1.5),
                CornerRadius::same(theme::RADIUS_TILE + 2),
                Stroke::new(3.0, theme::alpha(cat, 0.24)),
                egui::StrokeKind::Outside,
            );
        }
        painter.rect_filled(rect, corner, theme::panel());
        let (top, until, bottom) = if look.selected {
            (0.26, 0.8, 0.06)
        } else {
            (0.15, 0.72, 0.0)
        };
        theme::paint::gradient_rect(
            painter,
            rect,
            radius,
            &[
                (0.0, theme::alpha(cat, top)),
                (until, theme::alpha(cat, bottom)),
                (1.0, theme::alpha(cat, bottom)),
            ],
        );
        if look.hovered && !look.selected {
            painter.rect_filled(rect, corner, theme::alpha(theme::text(), 0.03));
        }
        painter.rect_stroke(
            rect,
            corner,
            Stroke::new(
                1.5,
                if look.selected {
                    cat
                } else {
                    theme::alpha(cat, if look.hovered { 0.7 } else { 0.48 })
                },
            ),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.rect_filled(rect, corner, theme::tile_off());
        theme::paint::dashed_rect(
            painter,
            rect.shrink(0.75),
            radius - 0.75,
            Stroke::new(
                1.5,
                if look.selected {
                    theme::text_soft()
                } else {
                    theme::faint()
                },
            ),
            5.0,
            3.5,
        );
        if look.selected {
            painter.rect_stroke(
                rect.expand(1.5),
                CornerRadius::same(theme::RADIUS_TILE + 2),
                Stroke::new(3.0, theme::alpha(theme::text_soft(), 0.14)),
                egui::StrokeKind::Outside,
            );
        }
    }
    if let Some(ring) = look.highlight {
        painter.rect_stroke(
            rect.expand(1.5),
            CornerRadius::same(theme::RADIUS_TILE + 2),
            Stroke::new(3.0, ring),
            egui::StrokeKind::Outside,
        );
    }
    if look.trying {
        painter.rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(theme::RADIUS_TILE + 2),
            Stroke::new(4.0, theme::accent_soft()),
            egui::StrokeKind::Outside,
        );
        theme::paint::dashed_rect(
            painter,
            rect.shrink(0.75),
            radius - 0.75,
            Stroke::new(1.5, theme::accent()),
            5.0,
            3.5,
        );
    }

    // The drawing at the top, the name and caption at the bottom.
    let (pad_top, pad_bottom, glyph, name_size, line, caption_size) = if look.large {
        (14.0, 11.0, 30.0, 14.0, 17.0, 10.5)
    } else if look.compact {
        (12.0, 8.0, 20.0, 11.5, 14.0, 9.5)
    } else {
        (10.0, 8.0, 24.0, theme::SECONDARY, 15.0, theme::CAPTION_TINY)
    };
    let ink_drawing = if look.on {
        cat
    } else {
        theme::alpha(theme::faint(), 0.8)
    };
    if let Some(drawing) = &look.drawing {
        let area = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.top() + pad_top + glyph / 2.0),
            Vec2::splat(glyph),
        );
        drawing.paint(ui, area, ink_drawing);
    }
    let mut bottom = rect.bottom() - pad_bottom;
    if !look.compact && !look.caption.is_empty() {
        let caption = if look.plain_caption {
            shell::elided(
                ui,
                look.caption,
                theme::regular(if look.large { 12.0 } else { 11.0 }),
                if look.on {
                    theme::muted()
                } else {
                    theme::faint()
                },
                rect.width() - 10.0,
            )
        } else {
            ui.painter().layout_job(theme::paint::spaced(
                &look.caption.to_uppercase(),
                theme::bold(caption_size),
                if look.on { cat } else { theme::faint() },
                0.08,
            ))
        };
        let width = caption.size().x;
        let height = caption.size().y;
        painter.galley(
            Pos2::new(rect.center().x - width / 2.0, bottom - height),
            caption,
            Color32::PLACEHOLDER,
        );
        bottom -= height + 1.0;
    }
    let mut job = theme::paint::spaced(
        look.name,
        theme::semibold(name_size),
        if look.on {
            theme::text()
        } else {
            theme::muted()
        },
        -0.005,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: rect.width() - if look.compact { 8.0 } else { 12.0 },
        max_rows: 2,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job.halign = egui::Align::Center;
    for section in &mut job.sections {
        section.format.line_height = Some(line);
    }
    let name = ui.painter().layout_job(job);
    let height = name.size().y;
    painter.galley(
        Pos2::new(rect.center().x, bottom - height - 1.0),
        name,
        Color32::PLACEHOLDER,
    );

    // The tags on the top edge, half outside the tile.
    let tag_y = rect.top() - 9.0;
    let mut right = rect.right() - 8.0;
    let mut tags: Vec<theme::Tag> = Vec::new();
    if !look.on {
        tags.push(theme::Tag::off());
    }
    if !(look.compact && !look.on) {
        for (text, led) in look.drivers {
            let tag = if text.starts_with("FS") {
                if look.compact {
                    theme::Tag::led("", *led)
                } else {
                    theme::Tag::led(text, *led)
                }
            } else {
                theme::Tag::new(text)
            };
            tags.push(tag);
        }
    }
    for tag in tags.iter().rev() {
        let size = tag.size(ui);
        let place = Rect::from_min_size(Pos2::new(right - size.x, tag_y), size);
        tag.paint(ui, place);
        right = place.left() - 3.0;
    }
    let left_mark = if look.snapshot {
        Some(Icon::Camera)
    } else if look.locked {
        Some(Icon::Lock)
    } else {
        None
    };
    if let Some(mark) = left_mark {
        let tag = theme::Tag::icon(mark);
        let size = tag.size(ui);
        tag.paint(
            ui,
            Rect::from_min_size(Pos2::new(rect.left() + 8.0, tag_y), size),
        );
    }
}

/// An endpoint as a round jack: its drawing, IN or OUT under it and, when
/// there is one, the short name of where it is routed.
pub(crate) fn paint_jack(
    ui: &Ui,
    centre: Pos2,
    input: bool,
    selected: bool,
    hovered: bool,
    sub: Option<&str>,
) {
    if selected {
        ui.painter()
            .circle_filled(centre, JACK + 4.0, theme::alpha(theme::text(), 0.1));
    }
    ui.painter().circle(
        centre,
        JACK,
        if hovered {
            theme::raised()
        } else {
            theme::panel()
        },
        Stroke::new(
            1.5,
            if selected {
                theme::text_soft()
            } else {
                theme::line_strong()
            },
        ),
    );
    if let Some(drawing) = theme::category_icon(if input { "Input" } else { "Output" }) {
        drawing.paint(
            ui,
            Rect::from_center_size(centre, Vec2::splat(15.0)),
            theme::text_soft(),
        );
    }
    let label = ui.painter().layout_job(theme::paint::spaced(
        if input { "IN" } else { "OUT" },
        theme::bold(theme::CAPTION_TINY),
        theme::muted(),
        0.08,
    ));
    let width = label.size().x;
    let label_top = centre.y + JACK + 5.0;
    let label_height = label.size().y;
    ui.painter().galley(
        Pos2::new(centre.x - width / 2.0, label_top),
        label,
        Color32::PLACEHOLDER,
    );
    if let Some(sub) = sub {
        let sub = shell::galley(ui, sub, theme::regular(10.5), theme::faint());
        let width = sub.size().x;
        ui.painter().galley(
            Pos2::new(centre.x - width / 2.0, label_top + label_height - 1.0),
            sub,
            Color32::PLACEHOLDER,
        );
    }
}

/// The board's lower edge, with the notch under the selected tile cut into
/// it, pointing at the pane.
pub(crate) fn board_edge(root: &Ui, rect: Rect, notch: Option<f32>) {
    let painter = root.painter();
    let y = rect.bottom() - 0.5;
    let line = Stroke::new(1.0, theme::line());
    match notch.filter(|x| *x > rect.left() + 16.0 && *x < rect.right() - 16.0) {
        Some(x) => {
            painter.hline(rect.left()..=(x - 11.0), y, line);
            painter.hline((x + 11.0)..=rect.right(), y, line);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(x - 11.0, rect.bottom()),
                    Pos2::new(x, rect.bottom() - 11.0),
                    Pos2::new(x + 11.0, rect.bottom()),
                ],
                theme::bg(),
                Stroke::NONE,
            ));
            painter.line_segment([Pos2::new(x - 11.0, y), Pos2::new(x, y - 10.5)], line);
            painter.line_segment([Pos2::new(x, y - 10.5), Pos2::new(x + 11.0, y)], line);
        }
        None => {
            painter.hline(rect.x_range(), y, line);
        }
    }
}

/// A wire as the signal on it is: one line for mono, two for stereo.
pub(crate) fn paint_wire(ui: &Ui, wire: Wire, stereo: bool, colour: Color32) {
    let painter = ui.painter();
    let offsets: &[f32] = if stereo { &[-1.8, 1.8] } else { &[0.0] };
    let width = if stereo { 1.5 } else { 2.0 };
    for offset in offsets {
        let shift = Vec2::new(0.0, *offset);
        match wire {
            Wire::Straight([a, b]) => {
                painter.line_segment([a + shift, b + shift], Stroke::new(width, colour));
            }
            Wire::Curve(points) => {
                painter.add(CubicBezierShape::from_points_stroke(
                    points.map(|point| point + shift),
                    false,
                    Color32::TRANSPARENT,
                    Stroke::new(width, colour),
                ));
            }
        }
    }
}

/// The legend at the right of the board's header.
fn paint_legend(ui: &Ui, right: f32, y: f32) {
    let mut x = right;
    for (word, stereo) in [("stereo", true), ("mono", false)] {
        let text = shell::galley(ui, word, theme::regular(11.5), theme::muted());
        x -= text.size().x;
        shell::paint_line(ui, text, x, y);
        x -= 6.0 + 18.0;
        let wire = Wire::Straight([Pos2::new(x, y), Pos2::new(x + 18.0, y)]);
        paint_wire(ui, wire, stereo, theme::wire());
        x -= 14.0;
    }
}

/// The amber "+" a gap offers under the pointer.
fn paint_plus(ui: &Ui, centre: Pos2) {
    let painter = ui.painter();
    painter.circle_filled(centre, 13.0, theme::accent_soft());
    painter.circle_filled(centre, 9.0, theme::accent());
    theme::paint_icon(ui, Icon::Plus, centre, 12.0, theme::accent_ink());
}

/// A gap a dragged block or junction can land in.
fn paint_landing(ui: &Ui, centre: Pos2, hot: bool) {
    if hot {
        paint_plus(ui, centre);
    } else {
        ui.painter()
            .circle_filled(centre, 3.5, theme::alpha(theme::accent(), 0.55));
    }
}

/// Everything the board needs to know about one block to draw its tile.
struct TileFacts {
    name: String,
    caption: String,
    colour: Color32,
    drawing: Option<theme::Art>,
    on: bool,
    snapshot: bool,
    drivers: Vec<(String, Option<Color32>)>,
    hover: String,
    effect: bool,
}

impl App {
    /// The facts a tile shows, gathered before drawing.
    fn tile_facts(&self, block: &session::Block) -> TileFacts {
        let category = self.block_category(block).unwrap_or_default();
        let colour = self.block_colour(block);
        let mut snapshot = false;
        let mut drivers: Vec<(String, Option<Color32>)> = Vec::new();
        let mut hover = Vec::new();
        if let Some(marks) = self.control_marks().get(&block.position) {
            for (source, what) in marks {
                hover.push(match what.as_str() {
                    "Auto-engage" => format!(
                        "{} engages this on its own when you move it",
                        source.label()
                    ),
                    what => format!("{} controls {what}", source.label()),
                });
                match source {
                    hx_proto::rpc::Source::Snapshots => snapshot = true,
                    hx_proto::rpc::Source::Footswitch(n) => {
                        let text = format!("FS{n}");
                        if !drivers.iter().any(|(known, _)| *known == text) {
                            drivers.push((text, self.switch_led(*n)));
                        }
                    }
                    other => {
                        let text = crate::pane::source_tag(*other);
                        if !drivers.iter().any(|(known, _)| *known == text) {
                            drivers.push((text, None));
                        }
                    }
                }
            }
        }
        TileFacts {
            name: self.slot_label(block),
            caption: theme::Category::named(&category).short().to_owned(),
            colour,
            drawing: theme::category_icon(&category),
            on: block.enabled,
            snapshot,
            drivers,
            hover: hover.join("\n"),
            effect: self.is_effect(block),
        }
    }

    /// The colour footswitch `n` lights, as the pedal reports it.
    pub(crate) fn switch_led(&self, n: u8) -> Option<Color32> {
        self.switches
            .iter()
            .find(|switch| switch.switch == n)
            .and_then(|switch| switch.lit().map(|lit| self.led_colour(Some(lit))))
    }

    /// Whether the signal leaving a slot is stereo, as its model says.
    fn slot_stereo(&self, slot: usize) -> Option<bool> {
        let block = self.chain.iter().find(|b| b.position == slot as i64)?;
        if block.kind != Kind::Block || block.model == 0 {
            return None;
        }
        let symbol = self.catalog.as_ref()?.symbol(block.model)?;
        Some(symbol.symbol.ends_with("Stereo"))
    }

    /// Whether a merge sends on stereo: any lane arriving stereo, or the
    /// lanes panned apart.
    fn merge_stereo(&self, join: Option<usize>, lanes: &[bool]) -> bool {
        if lanes.iter().any(|stereo| *stereo) {
            return true;
        }
        let Some(block) =
            join.and_then(|slot| self.chain.iter().find(|b| b.position == slot as i64))
        else {
            return false;
        };
        let Some(model) = self.slot_model(block) else {
            return false;
        };
        let Some(catalog) = self.catalog.as_ref() else {
            return false;
        };
        let pans: Vec<f32> = catalog
            .ordered_params(model)
            .iter()
            .zip(&block.values)
            .filter(|(param, _)| param.name.contains("Pan"))
            .map(|(_, value)| *value)
            .collect();
        pans.windows(2).any(|pair| (pair[0] - pair[1]).abs() > 0.01)
    }

    /// The name an endpoint's routing goes by: Multi, Main L/R.
    pub(crate) fn routing_name(&self, block: &session::Block) -> Option<String> {
        let routing = block.routing?;
        let model = self.slot_model(block)?;
        let catalog = self.catalog.as_ref()?;
        let param = model
            .params
            .iter()
            .find(|p| p.id == "@input" || p.id == "@output")?;
        catalog
            .choices(param)?
            .get(routing.max(0) as usize)
            .cloned()
    }

    /// How tall the board is for the loaded chain at this size.
    fn board_height(&self, g: &Geometry) -> f32 {
        if self.chain.is_empty() {
            return g.top + g.tile_height + g.bottom;
        }
        let mut height = g.top;
        for (index, path) in self.layout.paths.iter().enumerate() {
            if index > 0 {
                height += 22.0;
            }
            let lanes = path.lanes.len().max(1) as f32;
            height += lanes * g.tile_height + (lanes - 1.0) * g.lane_gap;
            if self.can_offer_branch(path) {
                height += g.lane_gap + ghost_height(g);
            }
        }
        height + g.bottom
    }

    /// The board: the chain on its dotted surface, under the deck. Its height
    /// is the chain's; a chain wider than the window scrolls sideways.
    pub(crate) fn signal_chain(&mut self, root: &mut Ui) {
        let tier = Tier::now(root.ctx());
        let g = Geometry::hx(tier);
        let height = self.board_height(&g);
        let mut notch = None;
        let panel = egui::Panel::top("board")
            .exact_size(height)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::bg_deep()))
            .show(root, |ui| {
                notch = self.board(ui, &g, tier);
            });
        board_edge(root, panel.response.rect, notch);
    }

    /// Draw the board into `ui`, returning where the notch goes.
    fn board(&mut self, ui: &mut Ui, g: &Geometry, tier: Tier) -> Option<f32> {
        let rect = ui.max_rect();
        theme::paint::dots(ui.painter(), rect, theme::dot());
        let header_y = rect.top() + 10.0 + 10.0;
        if self.chain.is_empty() {
            let (text, busy) = if self.loading {
                ("Loading from the pedal", true)
            } else if matches!(self.connection, Connection::Online) {
                ("No preset loaded", false)
            } else {
                ("The loaded preset's chain appears here", false)
            };
            let galley = shell::galley(ui, text, theme::regular(theme::BODY), theme::muted());
            let width = galley.size().x;
            let centre = rect.center();
            if busy {
                shell::spin(ui, Pos2::new(centre.x - width / 2.0 - 12.0, centre.y), 5.0);
            }
            shell::paint_line(ui, galley, centre.x - width / 2.0, centre.y);
            return None;
        }

        // The header row: how many blocks, which path, and the legend.
        let blocks = self.chain.iter().filter(|b| self.is_effect(b)).count();
        let header = self.board_header(blocks, tier);
        let mut x = rect.left() + 20.0;
        for (text, strong, dot) in &header {
            if *dot {
                x += 6.0;
                let dot = shell::galley(ui, "·", theme::regular(11.5), theme::faint());
                shell::paint_line(ui, dot, x, header_y);
                x += 12.0;
            }
            let galley = shell::galley(
                ui,
                text.clone(),
                if *strong {
                    theme::semibold(11.5)
                } else {
                    theme::regular(11.5)
                },
                if *strong {
                    theme::text_soft()
                } else {
                    theme::muted()
                },
            );
            let width = galley.size().x;
            shell::paint_line(ui, galley, x, header_y);
            x += width;
        }
        paint_legend(ui, rect.right() - 16.0, header_y);

        // Every path at one tile width, so their tiles line up.
        let paths = self.layout.paths.clone();
        let plans: Vec<Vec<Piece>> = paths.iter().map(pieces).collect();
        let available = rect.width() - 2.0 * g.pad;
        let mut tile = g.max_tile;
        let mut overflow = false;
        for plan in &plans {
            let (fixed, units) = measure(plan, g);
            let (width, over) = tile_width(available, fixed, units, g);
            tile = tile.min(width);
            overflow |= over;
        }
        let compact = tile < 90.0 || tier == Tier::S;
        let large = tier == Tier::L && !compact;
        let natural: f32 = plans
            .iter()
            .map(|plan| {
                let (fixed, units) = measure(plan, g);
                fixed + units * tile
            })
            .fold(0.0, f32::max);
        let content_width = (natural + 2.0 * g.pad).max(rect.width());

        self.gap_rects.clear();
        self.block_rects.clear();
        self.ghost_target = None;
        let mut notch = None;
        let mut pick = None;
        let output = egui::ScrollArea::horizontal()
            .id_salt("board-scroll")
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            })
            .show(ui, |ui| {
                let content = Rect::from_min_size(
                    Pos2::new(ui.max_rect().left(), rect.top()),
                    Vec2::new(content_width, rect.height()),
                );
                ui.allocate_rect(content, Sense::hover());
                let mut top = content.top() + g.top;
                for (index, (path, plan)) in paths.iter().zip(&plans).enumerate() {
                    if paths.len() > 1 {
                        if index > 0 {
                            top += 22.0;
                        }
                        let label = ui.painter().layout_job(theme::paint::spaced(
                            &format!("PATH {}", index + 1),
                            theme::semibold(theme::CAPTION),
                            theme::faint(),
                            0.07,
                        ));
                        ui.painter().galley(
                            Pos2::new(content.left() + 20.0, top - 18.0),
                            label,
                            Color32::PLACEHOLDER,
                        );
                    }
                    let (fixed, units) = measure(plan, g);
                    let width = fixed + units * tile;
                    let left = content.left() + g.pad + ((available - width) / 2.0).max(0.0);
                    let placed = place(
                        plan,
                        g,
                        tile,
                        Pos2::new(left, top),
                        &|slot| self.slot_stereo(slot),
                        &|join, lanes| self.merge_stereo(join, lanes),
                    );
                    let (picked, selected_x) = self.draw_path(ui, path, &placed, g, compact, large);
                    pick = picked.or(pick);
                    notch = selected_x.or(notch);
                    top += placed.lanes as f32 * g.tile_height
                        + (placed.lanes as f32 - 1.0) * g.lane_gap;
                    if self.can_offer_branch(path) {
                        top += g.lane_gap + ghost_height(g);
                    }
                }
                self.block_drag(ui, g);
            });
        // Wider than the window: the right edge fades, and says so.
        let scrolled = output.state.offset.x;
        let shown = output.inner_rect;
        if overflow && scrolled + shown.width() < output.content_size.x - 1.0 {
            let fade = Rect::from_min_max(Pos2::new(shown.right() - 64.0, shown.top()), shown.max);
            horizontal_fade(ui, fade);
            theme::paint_icon(
                ui,
                Icon::ChevronRight,
                Pos2::new(shown.right() - 18.0, shown.center().y),
                16.0,
                theme::muted(),
            );
        }
        if let Some(index) = pick {
            // Purely a local view change. Mirroring the selection onto the
            // device's own screen meant every click was a round trip, and
            // clicking through a chain quickly wedged it. Choosing another
            // block keeps whatever the browser was playing.
            if index != self.selected {
                self.close_browser(true);
            }
            self.selected = index;
        }
        notch.map(|x| x - scrolled)
    }

    /// The board's header: the block count and the path, or what the page is
    /// doing with the chain right now. Each part says whether it is strong
    /// and whether a dot goes before it.
    fn board_header(&self, blocks: usize, tier: Tier) -> Vec<(String, bool, bool)> {
        let mut parts = vec![
            (blocks.to_string(), true, false),
            (
                if blocks == 1 { " block" } else { " blocks" }.to_owned(),
                false,
                false,
            ),
        ];
        let note = if let Some(from) = self.dragging.and_then(|slot| self.index_of(slot)) {
            format!("Drop to move {}", self.slot_label(&self.chain[from]))
        } else if let Some(browser) = &self.browser {
            match &browser.target {
                crate::browser::Target::Swap { original, .. } => {
                    format!("Trying models in {original}'s place")
                }
                crate::browser::Target::Insert { .. } => {
                    "Choosing a block for the marked gap".to_owned()
                }
            }
        } else if let Some(lens) = self
            .lens_header()
            .filter(|_| tier != Tier::L || self.lens == crate::pane::Lens::Footswitches)
        {
            return lens;
        } else {
            match self.layout.paths.len() {
                0 | 1 => "Path 1".to_owned(),
                n => format!("{n} paths"),
            }
        };
        parts.push((note, false, true));
        parts
    }

    /// One path's tiles, endpoints, junctions, wires and gaps, drawn and
    /// answered. Returns the index of a block clicked, and the selected
    /// tile's centre for the notch.
    fn draw_path(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        placed: &Placed,
        g: &Geometry,
        compact: bool,
        large: bool,
    ) -> (Option<usize>, Option<f32>) {
        let mut pick = None;
        let mut notch = None;
        for (wire, stereo) in &placed.wires {
            paint_wire(ui, *wire, *stereo, theme::wire());
        }

        // The gaps: an amber "+" under the pointer, a click to add a block.
        let live = !self.display_only;
        let dragging = self.dragging.is_some() || self.dragging_junction.is_some();
        let inserting = self.browser_insert_at();
        for (before, rect, centre) in &placed.gaps {
            if !live {
                continue;
            }
            self.gap_rects.push((*before, *rect));
            if dragging {
                continue;
            }
            let response = ui
                .interact(
                    *rect,
                    ui.id().with(("gap", *before, rect.top() as i32)),
                    Sense::click(),
                )
                .on_hover_text("Add a block here");
            if inserting == Some(*before) || response.hovered() {
                paint_plus(ui, *centre);
            }
            if response.clicked() {
                self.open_browser_insert(*before);
            }
        }

        // The junctions: a dot on the line, clickable and draggable.
        for (slot, centre, opening) in &placed.junctions {
            let Some(slot) = slot else {
                continue;
            };
            let Some(index) = self.index_of(*slot) else {
                continue;
            };
            let label = self.slot_label(&self.chain[index]);
            let held = self.dragging_junction == Some((*slot, *opening));
            let hit = Rect::from_center_size(*centre, Vec2::splat(18.0));
            let sense = if live {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            };
            let what = if *opening {
                "The signal forks here: drag to move it, click for how it divides"
            } else {
                "The branches rejoin here: drag to move it, click for their levels"
            };
            let response = ui
                .interact(hit, ui.id().with(("junction", *slot)), sense)
                .on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text(format!("{label}\n{what}"));
            let selected = index == self.selected || held;
            let ring = if selected {
                theme::text()
            } else if response.hovered() {
                theme::text_soft()
            } else {
                theme::wire()
            };
            if selected {
                ui.painter()
                    .circle_filled(*centre, 8.0, theme::alpha(theme::text(), 0.12));
            }
            ui.painter()
                .circle(*centre, 4.5, theme::bg_deep(), Stroke::new(2.0, ring));
            if *opening {
                if let Some(tag) = crate::split_tag(&label) {
                    let galley = ui.painter().layout_job(theme::paint::spaced(
                        tag,
                        theme::bold(theme::CAPTION_TINY),
                        theme::muted(),
                        0.06,
                    ));
                    let width = galley.size().x;
                    ui.painter().galley(
                        Pos2::new(centre.x - width / 2.0, centre.y + 8.0),
                        galley,
                        Color32::PLACEHOLDER,
                    );
                }
            }
            if live && response.drag_started() {
                self.dragging_junction = Some((*slot, *opening));
            }
            if response.clicked() {
                pick = Some(index);
            }
        }
        self.junction_drag(ui, path);

        // The endpoints: round jacks with their routing under them.
        for (slot, centre, input) in &placed.endpoints {
            let Some(index) = self.index_of(*slot) else {
                continue;
            };
            let block = self.chain[index].clone();
            let hit = Rect::from_center_size(*centre, Vec2::splat(2.0 * JACK + 6.0));
            let response = ui
                .interact(hit, ui.id().with(("endpoint", *slot)), Sense::click())
                .on_hover_text(if *input {
                    "The input: where the signal comes from"
                } else {
                    "The output: where the signal goes"
                });
            let selected = index == self.selected;
            // The short name under the jack: Multi, not every input it
            // gathers, which the block's own head spells out.
            let short = self
                .routing_name(&block)
                .map(|routing| routing.split(" (").next().unwrap_or_default().to_owned());
            paint_jack(
                ui,
                *centre,
                *input,
                selected,
                response.hovered(),
                short.as_deref(),
            );
            if response.clicked() {
                pick = Some(index);
            }
        }

        // The tiles.
        let focus = self.lens_highlight();
        let trying = self.browser_swap_slot();
        for (slot, rect, _lane) in &placed.tiles {
            let Some(index) = self.index_of(*slot) else {
                continue;
            };
            let block = self.chain[index].clone();
            let facts = self.tile_facts(&block);
            let sense = if live {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            };
            let response = ui.interact(*rect, ui.id().with(("tile", *slot)), sense);
            let selected = index == self.selected && !self.display_only;
            if selected {
                notch = Some(rect.center().x);
            }
            let highlight = focus
                .as_ref()
                .and_then(|(slots, colour)| slots.contains(&block.position).then_some(*colour));
            paint_tile(
                ui,
                *rect,
                &TileLook {
                    name: &facts.name,
                    caption: &facts.caption,
                    colour: facts.colour,
                    drawing: facts.drawing.clone(),
                    on: facts.on,
                    selected,
                    hovered: response.hovered(),
                    highlight,
                    trying: trying == Some(block.position),
                    compact,
                    large,
                    snapshot: facts.snapshot,
                    drivers: &facts.drivers,
                    plain_caption: false,
                    locked: false,
                },
            );
            if !live {
                continue;
            }
            let response = if facts.hover.is_empty() {
                response
            } else {
                response.on_hover_text(&facts.hover)
            };
            self.block_rects.push((*slot, *rect));
            if response.drag_started() {
                self.dragging = Some(*slot);
            }
            if response.clicked() {
                pick = Some(index);
            }
            // What a hand reaching for a block that is not the one being
            // edited wants: the same actions the block's own header offers.
            if facts.effect {
                let copied = self.copied_block;
                let mut action = None;
                response.context_menu(|ui| {
                    theme::menu_width(ui, 220.0);
                    theme::menu_header(ui, &facts.name, Some(&facts.caption));
                    if theme::menu_item(ui, Some(Icon::LayoutGrid), "Change model…", None).clicked()
                    {
                        action = Some(TileAction::Change);
                    }
                    theme::menu_separator(ui);
                    if theme::menu_item(ui, Some(Icon::Copy), "Copy", None).clicked() {
                        action = Some(TileAction::Copy);
                    }
                    if copied {
                        if theme::menu_item(ui, Some(Icon::Paste), "Paste here", None).clicked() {
                            action = Some(TileAction::Paste);
                        }
                    } else {
                        theme::menu_disabled(
                            ui,
                            Some(Icon::Paste),
                            "Paste",
                            Some("nothing copied"),
                        );
                    }
                    theme::menu_separator(ui);
                    if theme::menu_danger(ui, Icon::Remove, "Remove from the chain").clicked() {
                        action = Some(TileAction::Remove);
                    }
                });
                match action {
                    Some(TileAction::Change) => {
                        self.selected = index;
                        self.open_browser_swap();
                    }
                    Some(TileAction::Copy) => self.copy_block(*slot, &block),
                    Some(TileAction::Paste) => self.paste_block(*slot),
                    Some(TileAction::Remove) => self.edit(Cmd::ClearBlock(block.position)),
                    None => {}
                }
            }
        }

        // The offer of a parallel branch, where it would actually run.
        if live && self.can_offer_branch(path) {
            self.ghost_branch(ui, placed, g);
        }
        (pick, notch)
    }

    /// The dashed preview of the branch a click would create: it forks after
    /// the input, runs under the line, and merges before the output, which is
    /// exactly where the real one will go.
    fn ghost_branch(&mut self, ui: &mut Ui, placed: &Placed, g: &Geometry) {
        let Some(path) = self.layout.paths.iter().find(|p| {
            placed
                .endpoints
                .iter()
                .any(|(slot, _, input)| *input && Some(*slot) == p.input)
        }) else {
            return;
        };
        let Some(at) = self.free_on_branch(path) else {
            return;
        };
        let (Some(first), Some(last)) = (
            placed
                .tiles
                .iter()
                .map(|(_, rect, _)| rect.left())
                .reduce(f32::min),
            placed
                .tiles
                .iter()
                .map(|(_, rect, _)| rect.right())
                .reduce(f32::max),
        ) else {
            return;
        };
        let main = placed
            .endpoints
            .first()
            .map_or(first, |(_, centre, _)| centre.y);
        let top = main + g.tile_height / 2.0 + g.lane_gap;
        let rect = Rect::from_min_max(
            Pos2::new(first, top),
            Pos2::new(last, top + ghost_height(g)),
        );
        let response = ui
            .interact(rect, ui.id().with("ghost-branch"), Sense::click())
            .on_hover_text("Add a block on a parallel branch, or drag one down here");
        self.ghost_target = Some((at, rect));
        let hot = response.hovered() || self.dragging.is_some();
        let ink = if hot {
            theme::accent_line()
        } else {
            theme::line_strong()
        };
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(10), theme::accent_soft());
        }
        theme::paint::dashed_rect(ui.painter(), rect, 10.0, Stroke::new(1.5, ink), 5.0, 4.0);
        // Dashed curves from the line down to it and back up.
        let mid = rect.center().y;
        for (from, to) in [
            (Pos2::new(first - g.wire * 0.6, main), Pos2::new(first, mid)),
            (Pos2::new(last, mid), Pos2::new(last + g.wire * 0.6, main)),
        ] {
            let points: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let ease = t * t * (3.0 - 2.0 * t);
                    Pos2::new(
                        from.x + (to.x - from.x) * t,
                        from.y + (to.y - from.y) * ease,
                    )
                })
                .collect();
            ui.painter().extend(egui::Shape::dashed_line(
                &points,
                Stroke::new(1.5, ink),
                4.0,
                3.0,
            ));
        }
        let text = shell::galley(
            ui,
            "Run a block in parallel",
            theme::medium(12.0),
            if hot { theme::accent() } else { theme::muted() },
        );
        let width = text.size().x + 20.0;
        let x = rect.center().x - width / 2.0;
        theme::paint_icon(
            ui,
            Icon::Plus,
            Pos2::new(x + 7.0, mid),
            13.0,
            if hot { theme::accent() } else { theme::muted() },
        );
        shell::paint_line(ui, text, x + 20.0, mid);
        if response.clicked() {
            self.open_browser_insert(at);
        }
    }

    /// Follow a block being dragged along the chain, resolved fresh from the
    /// pointer every frame, never from what was hovered some frames ago, which
    /// is how a release over nothing came to move a block anyway.
    ///
    /// Every gap shows where it lands; dropping into the nearest one slides
    /// the block in there. Dropping onto a block in another lane trades places
    /// with it: the lanes are not contiguous, so between them a move is a
    /// trade. Dropping onto the offered branch runs it in parallel. Escape
    /// lets go.
    fn block_drag(&mut self, ui: &mut Ui, g: &Geometry) {
        if self.display_only {
            return;
        }
        let Some(from) = self.dragging else {
            return;
        };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dragging = None;
            return;
        }
        let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        let ghost = self
            .ghost_target
            .filter(|(_, rect)| rect.expand(6.0).contains(pointer));
        let swap = if ghost.is_some() {
            None
        } else {
            self.block_rects
                .iter()
                .find(|(slot, rect)| {
                    *slot != from && rect.contains(pointer) && !self.same_lane(from, *slot)
                })
                .map(|(slot, rect)| (*slot, *rect))
        };
        let gap = if ghost.is_some() || swap.is_some() {
            None
        } else {
            let candidates = self
                .gap_rects
                .iter()
                // The gaps either side of the block itself go nowhere.
                .filter(|(before, _)| *before != from && *before != from + 1);
            // A gap under the pointer wins outright; otherwise the nearest one
            // within reach: half a lane vertically, so the main line and a
            // branch never compete, and a tile horizontally, so a drop far
            // from any gap means nothing.
            candidates
                .clone()
                .find(|(_, rect)| rect.contains(pointer))
                .or_else(|| {
                    candidates
                        .filter(|(_, rect)| {
                            (pointer.y - rect.center().y).abs() < g.lane_pitch() * 0.5
                                && (pointer.x - rect.center().x).abs() < g.max_tile
                        })
                        .min_by(|a, b| {
                            let da = (a.1.center().x - pointer.x).abs();
                            let db = (b.1.center().x - pointer.x).abs();
                            da.total_cmp(&db)
                        })
                })
                .map(|(before, rect)| (*before, *rect))
        };

        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        for (before, rect) in &self.gap_rects {
            if *before == from || *before == from + 1 {
                continue;
            }
            let hot = gap.is_some_and(|(g, _)| g == *before);
            paint_landing(ui, rect.center(), hot);
        }
        if let Some((_, rect)) = swap {
            theme::paint::dashed_rect(
                ui.painter(),
                rect.expand(3.0),
                f32::from(theme::RADIUS_TILE) + 3.0,
                Stroke::new(2.0, theme::accent()),
                5.0,
                3.5,
            );
        }
        if let Some((_, rect)) = ghost {
            paint_plus(ui, rect.center());
        }
        if let Some(i) = self.index_of(from) {
            let block = &self.chain[i];
            theme::drag_ghost(
                ui.ctx(),
                pointer,
                &self.slot_label(block),
                self.block_colour(block),
            );
        }

        if ui.input(|i| i.pointer.any_released()) {
            if let Some((before, _)) = ghost {
                self.edit(Cmd::MoveBlockBefore { from, before });
            } else if let Some((slot, _)) = swap {
                self.edit(Cmd::MoveBlock { from, to: slot });
            } else if let Some((before, _)) = gap {
                self.edit(Cmd::MoveBlockBefore { from, before });
            }
            self.dragging = None;
        }
    }

    /// Follow a fork or merge being dragged along the line. Every gap it can
    /// land in shows a dot while the drag lasts, the one nearest the pointer
    /// takes the amber "+", and releasing commits the move. A fork can go
    /// anywhere between the input and the merge; a merge, anywhere between the
    /// fork and the output. Escape lets go.
    fn junction_drag(&mut self, ui: &mut Ui, path: &Path) {
        if self.display_only {
            return;
        }
        let Some((slot, opening)) = self.dragging_junction else {
            return;
        };
        let dragged = if opening { path.split } else { path.join };
        if dragged != Some(slot) {
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dragging_junction = None;
            return;
        }
        let Some((lowest, highest, current)) = crate::attach_range(path, opening) else {
            return;
        };
        let candidates: Vec<(usize, Rect)> = self
            .gap_rects
            .iter()
            .filter(|(before, _)| (lowest..=highest).contains(before))
            .copied()
            .collect();
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let nearest = pointer.and_then(|p| {
            candidates
                .iter()
                .min_by(|a, b| {
                    let da = (a.1.center().x - p.x).abs();
                    let db = (b.1.center().x - p.x).abs();
                    da.total_cmp(&db)
                })
                .copied()
        });
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        for (before, rect) in &candidates {
            let hot = nearest.is_some_and(|(n, _)| n == *before);
            paint_landing(ui, rect.center(), hot);
        }
        if ui.input(|i| i.pointer.any_released()) {
            if let Some((before, _)) = nearest {
                if before != current {
                    self.edit(Cmd::MoveJunction {
                        junction: slot,
                        before,
                    });
                }
            }
            self.dragging_junction = None;
        }
    }

    /// A chain drawn for looking only, at its natural width: a previewed tone
    /// file. No gaps, no drags, no branch offer.
    pub(crate) fn board_preview(&mut self, ui: &mut Ui) {
        let tier = Tier::now(ui.ctx());
        let g = Geometry::hx(tier);
        let height = self.board_height(&g);
        let plans: Vec<Vec<Piece>> = self.layout.paths.iter().map(pieces).collect();
        let natural: f32 = plans
            .iter()
            .map(|plan| {
                let (fixed, units) = measure(plan, &g);
                fixed + units * g.min_tile
            })
            .fold(0.0, f32::max);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(natural + 2.0 * g.pad, height), Sense::hover());
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(theme::RADIUS_CARD),
            theme::bg_deep(),
        );
        theme::paint::dots(ui.painter(), rect, theme::dot());
        let paths = self.layout.paths.clone();
        let mut top = rect.top() + g.top;
        for (path, plan) in paths.iter().zip(&plans) {
            let placed = place(
                plan,
                &g,
                g.min_tile,
                Pos2::new(rect.left() + g.pad, top),
                &|slot| self.slot_stereo(slot),
                &|join, lanes| self.merge_stereo(join, lanes),
            );
            let _ = self.draw_path(ui, path, &placed, &g, tier == Tier::S, false);
            top += placed.lanes as f32 * g.lane_pitch() + 22.0;
        }
    }

    /// The natural width of a chain drawn for looking at: a previewed tone.
    pub(crate) fn preview_width(&self, layout: &hx_proto::preset::Layout, tier: Tier) -> f32 {
        let g = Geometry::hx(tier);
        layout
            .paths
            .iter()
            .map(|path| {
                let (fixed, units) = measure(&pieces(path), &g);
                fixed + units * g.min_tile
            })
            .fold(0.0, f32::max)
            + 2.0 * g.pad
    }
}

/// The offered branch's height.
fn ghost_height(g: &Geometry) -> f32 {
    (g.tile_height * 0.5).round()
}

/// A fade to the board's colour toward the right edge.
pub(crate) fn horizontal_fade(ui: &Ui, rect: Rect) {
    let mut mesh = egui::epaint::Mesh::default();
    let clear = theme::alpha(theme::bg_deep(), 0.0);
    let solid = theme::bg_deep();
    mesh.colored_vertex(rect.left_top(), clear);
    mesh.colored_vertex(
        Pos2::new(rect.left() + rect.width() * 0.8, rect.top()),
        solid,
    );
    mesh.colored_vertex(
        Pos2::new(rect.left() + rect.width() * 0.8, rect.bottom()),
        solid,
    );
    mesh.colored_vertex(rect.left_bottom(), clear);
    mesh.colored_vertex(rect.right_top(), solid);
    mesh.colored_vertex(rect.right_bottom(), solid);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh.add_triangle(1, 4, 5);
    mesh.add_triangle(1, 5, 2);
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// What a tile's menu was asked.
enum TileAction {
    Change,
    Copy,
    Paste,
    Remove,
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_proto::preset::Lane;

    fn stomp() -> Path {
        Path {
            input: Some(0),
            output: Some(9),
            split: Some(10),
            join: Some(19),
            head: vec![1, 2, 3],
            lanes: vec![
                Lane {
                    branch: 0,
                    blocks: vec![4],
                    span: 4..5,
                },
                Lane {
                    branch: 1,
                    blocks: vec![11],
                    span: 11..19,
                },
            ],
            tail: vec![5, 6],
        }
    }

    #[test]
    fn a_path_is_met_in_the_order_the_signal_meets_it() {
        let found = pieces(&stomp());
        assert_eq!(found.first(), Some(&Piece::Input(0)));
        assert_eq!(
            found[1..4],
            [Piece::Block(1), Piece::Block(2), Piece::Block(3)]
        );
        assert!(matches!(&found[4], Piece::Fork(fork) if fork.lanes.len() == 2));
        assert_eq!(
            found[5..],
            [Piece::Block(5), Piece::Block(6), Piece::Output(9)]
        );
    }

    #[test]
    fn tiles_fill_the_board_between_their_limits() {
        let g = Geometry::hx(Tier::M);
        let plan = pieces(&stomp());
        let (fixed, units) = measure(&plan, &g);
        assert_eq!(
            units, 6.0,
            "three before the split, one lane wide, two after"
        );
        // Plenty of room: as wide as the design lets a tile be.
        assert_eq!(tile_width(4000.0, fixed, units, &g), (g.max_tile, false));
        // A little: as wide as fills it.
        let (width, over) = tile_width(fixed + 6.0 * 100.0, fixed, units, &g);
        assert!(!over);
        assert_eq!(width, 100.0);
        // Too little: the minimum, and the board scrolls.
        assert_eq!(
            tile_width(fixed + 6.0 * 50.0, fixed, units, &g),
            (g.min_tile, true)
        );
    }

    #[test]
    fn placing_a_path_takes_exactly_what_measuring_said() {
        let g = Geometry::hx(Tier::M);
        let plan = pieces(&stomp());
        let (fixed, units) = measure(&plan, &g);
        let placed = place(&plan, &g, 110.0, Pos2::ZERO, &|_| None, &|_, _| false);
        assert!((placed.width - (fixed + units * 110.0)).abs() < 0.01);
        assert_eq!(
            placed.tiles.len(),
            7,
            "three before the split, one on each lane, two after"
        );
        assert_eq!(placed.lanes, 2);
        // The branch's cab hangs one lane down, under the main line's.
        let main_cab = placed.tiles.iter().find(|(slot, ..)| *slot == 4).unwrap();
        let branch_cab = placed.tiles.iter().find(|(slot, ..)| *slot == 11).unwrap();
        assert_eq!(main_cab.1.left(), branch_cab.1.left());
        assert_eq!(branch_cab.1.top() - main_cab.1.top(), g.lane_pitch());
        assert_eq!(branch_cab.2, 1);
    }

    #[test]
    fn every_gap_keeps_the_slot_a_drop_goes_before() {
        let g = Geometry::hx(Tier::M);
        let placed = place(
            &pieces(&stomp()),
            &g,
            110.0,
            Pos2::ZERO,
            &|_| None,
            &|_, _| false,
        );
        let keys: Vec<usize> = placed.gaps.iter().map(|(before, ..)| *before).collect();
        // Before each head block, before each lane's first block, after the
        // branch's last (it has room), before each tail block and the output.
        for wanted in [1, 2, 3, 4, 11, 12, 5, 6, 9] {
            assert!(keys.contains(&wanted), "a gap before {wanted}: {keys:?}");
        }
        assert!(!keys.contains(&10), "nothing goes before the split itself");
    }

    #[test]
    fn stereo_follows_the_models_and_the_merge() {
        let g = Geometry::hx(Tier::M);
        // The delay is stereo: everything after it is two lines.
        let stereo = |slot: usize| match slot {
            5 => Some(true),
            1..=4 | 11 => Some(false),
            _ => None,
        };
        let placed = place(
            &pieces(&stomp()),
            &g,
            110.0,
            Pos2::ZERO,
            &stereo,
            &|_, lanes| lanes.iter().any(|s| *s),
        );
        let after_delay = placed
            .tiles
            .iter()
            .find(|(slot, ..)| *slot == 5)
            .map(|(_, rect, _)| rect.right())
            .unwrap();
        let (wire, is_stereo) = placed
            .wires
            .iter()
            .find(|(wire, _)| matches!(wire, Wire::Straight([a, _]) if (a.x - after_delay).abs() < 0.01))
            .unwrap();
        assert!(matches!(wire, Wire::Straight(_)));
        assert!(*is_stereo, "the wire out of a stereo delay is stereo");
        let into_amp = placed
            .wires
            .iter()
            .find(|(wire, _)| matches!(wire, Wire::Straight([_, b]) if (b.x - placed.tiles[2].1.left()).abs() < 0.01))
            .unwrap();
        assert!(!into_amp.1, "the wire into the amp is mono");
    }

    #[test]
    fn a_branch_with_nothing_on_it_still_takes_room() {
        let g = Geometry::hx(Tier::M);
        let path = Path {
            input: Some(0),
            output: Some(9),
            split: Some(10),
            join: Some(19),
            head: vec![],
            lanes: vec![
                Lane {
                    branch: 0,
                    blocks: vec![],
                    span: 1..9,
                },
                Lane {
                    branch: 1,
                    blocks: vec![],
                    span: 11..19,
                },
            ],
            tail: vec![],
        };
        let placed = place(&pieces(&path), &g, 110.0, Pos2::ZERO, &|_| None, &|_, _| {
            false
        });
        let keys: Vec<usize> = placed.gaps.iter().map(|(before, ..)| *before).collect();
        assert!(keys.contains(&1) && keys.contains(&11), "{keys:?}");
        assert!(placed.width > g.endpoint * 2.0 + 2.0 * g.junction);
    }
}
