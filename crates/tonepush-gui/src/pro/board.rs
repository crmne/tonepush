//! The StompStation PRO's chain on the board (docs/design/redesign-2026-10-01,
//! screen 09): the pedal's blocks drawn as the same tiles as an HX's, between
//! its input and output jacks.
//!
//! Firmware 2.x describes its chain in a router, which `routing` draws and
//! edits. Firmware 1.5.12 has none: one chain of twelve blocks, each always
//! in its place, which this module draws in the order the schema lists them,
//! with no free positions and no locks: every block can be turned off, none
//! can be moved. That chain is read from the schema rather than written down
//! here, so a firmware that lists other blocks gets them as ordinary tiles,
//! named from the schema.

use egui::{Pos2, Rect, Sense, Ui, Vec2};

use super::*;
use crate::board::{self, Geometry, TileLook, Wire, JACK};
use crate::shell;
use crate::theme::{Icon, Tier};

/// One block of the chain as its tile shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProTile {
    pub(crate) group: String,
    pub(crate) name: String,
    /// What it holds: its model, capture or mode, or what kind it is.
    pub(crate) caption: String,
    pub(crate) category: &'static str,
    pub(crate) on: bool,
}

/// The chain's blocks, in the order the pedal lists them.
pub(crate) fn chain_tiles(snapshot: &Snapshot) -> Vec<ProTile> {
    app_groups(snapshot)
        .into_iter()
        .map(|group| ProTile {
            name: tile_name(&group),
            caption: block_model_name(snapshot, &group)
                .unwrap_or_else(|| tile_caption(&group).to_owned()),
            category: group_category(&group),
            on: block_enabled(snapshot, &group),
            group,
        })
        .collect()
}

/// A block's name as short as the pedal's own screen keeps it.
pub(crate) fn tile_name(group: &str) -> String {
    match group {
        "gate" => "Gate",
        "exp" => "Wah",
        "comp" => "Comp",
        "mod" => "Mod",
        "ir" => "IR",
        // Firmware 2.x's blocks: the second of a kind is numbered, as the
        // pedal lists two delays, two reverbs and two EQs.
        "flanger" => "Flanger",
        "chorus" => "Chorus",
        "eq2" => "EQ 2",
        "crossover" => "Crossover",
        "delay2" => "Delay 2",
        "reverb2" => "Reverb 2",
        "pickup" => "Pickup Sim",
        "pitch_time" => "Pitch Time",
        "dyna_comp" => "Dynamic Comp",
        "parametric_eq" => "Parametric EQ",
        "dtchr" => "Detune Chorus",
        "rotary" => "Rotary",
        _ => return friendly_group(group),
    }
    .to_owned()
}

/// What a tile says under its name when its block has no model or mode.
fn tile_caption(group: &str) -> &'static str {
    match group {
        "gate" => "Noise gate",
        "exp" => "Expression",
        _ => "",
    }
}

/// Whether the firmware keeps every block in one fixed order, as 1.5.12
/// does.
pub(crate) fn fixed_order(snapshot: &Snapshot) -> bool {
    snapshot.identity.version.starts_with("1.")
}

/// Where everything on the board sits.
#[derive(Debug)]
pub(crate) struct Laid {
    pub(crate) input: Pos2,
    pub(crate) output: Pos2,
    pub(crate) tiles: Vec<Rect>,
    /// The tiles' width.
    pub(crate) tile: f32,
    /// The content's width, padding included.
    pub(crate) width: f32,
    /// Whether even the narrowest tiles do not fit, so the board scrolls.
    pub(crate) overflow: bool,
}

/// The input jack, `count` tiles and the output jack along one line, the
/// tiles as wide as fills `width` between their limits, centred when they
/// stop short of it.
pub(crate) fn lay_out(count: usize, left: f32, top: f32, width: f32, g: &Geometry) -> Laid {
    let available = width - 2.0 * g.pad;
    let fixed = 2.0 * g.endpoint + (count as f32 + 1.0) * g.wire;
    let (tile, overflow) = board::tile_width(available, fixed, count as f32, g);
    let natural = fixed + count as f32 * tile;
    let start = left + g.pad + ((available - natural) / 2.0).max(0.0);
    let main = top + g.tile_height / 2.0;
    let mut x = start + g.endpoint + g.wire;
    let mut tiles = Vec::with_capacity(count);
    for _ in 0..count {
        tiles.push(Rect::from_min_size(
            Pos2::new(x, top),
            Vec2::new(tile, g.tile_height),
        ));
        x += tile + g.wire;
    }
    Laid {
        input: Pos2::new(start + g.endpoint / 2.0, main),
        output: Pos2::new(x + g.endpoint / 2.0, main),
        tiles,
        tile,
        width: natural + 2.0 * g.pad,
        overflow,
    }
}

/// How tall the board is for a snapshot's chain: one row of tiles, or as
/// many lanes as a 2.x chain's deepest parallel bank stacks.
pub(crate) fn board_height(snapshot: &Snapshot, tier: Tier) -> f32 {
    match routing::Chain::of(snapshot) {
        Some(chain) => {
            let lanes = chain
                .router()
                .stages()
                .iter()
                .map(|stage| stage.len())
                .max()
                .unwrap_or(1);
            routing::board_height(&routing::geometry(tier).0, lanes)
        }
        None => {
            let g = Geometry::pro(tier);
            g.top + g.tile_height + g.bottom
        }
    }
}

impl Panel {
    /// The board under the deck: the chain on its dotted surface, a click
    /// on a tile or a jack choosing what the pane shows.
    pub(super) fn board(&mut self, root: &mut Ui, snapshot: &Snapshot, tier: Tier) {
        let g = Geometry::pro(tier);
        // A bank deep enough to crowd the pane out scrolls instead.
        let room = (root.available_height() * 0.6).max(g.top + g.tile_height + g.bottom);
        let height = board_height(snapshot, tier).min(room);
        let mut notch = None;
        let panel = egui::Panel::top("pro-board")
            .exact_size(height)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::bg_deep()))
            .show(root, |ui| {
                notch = self.draw_board(ui, snapshot, &g, tier, true);
            });
        self.board_rect = Some(panel.response.rect);
        board::board_edge(root, panel.response.rect, notch);
    }

    /// The chain drawn into `ui`: with `live`, its tiles and jacks choose
    /// what the pane shows, and a 2.x chain's positions can be edited.
    /// Returns where the notch under the chosen one goes.
    pub(super) fn draw_board(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        g: &Geometry,
        tier: Tier,
        live: bool,
    ) -> Option<f32> {
        if let Some(chain) = routing::Chain::of(snapshot) {
            return self.draw_route(ui, snapshot, &chain, tier, live);
        }
        let rect = ui.max_rect();
        theme::paint::dots(ui.painter(), rect, theme::dot());
        let tiles = chain_tiles(snapshot);

        // The header: how many blocks, and that their order is the pedal's.
        let header_y = rect.top() + 20.0;
        let mut x = rect.left() + 20.0;
        let mut parts = vec![
            (tiles.len().to_string(), true, false),
            (
                if tiles.len() == 1 {
                    " block"
                } else {
                    " blocks"
                }
                .to_owned(),
                false,
                false,
            ),
        ];
        if fixed_order(snapshot) {
            parts.push(("in the pedal's fixed order".to_owned(), false, true));
        }
        for (text, strong, dot) in parts {
            if dot {
                x += 6.0;
                let dot = shell::galley(ui, "·", theme::regular(11.5), theme::faint());
                shell::paint_line(ui, dot, x, header_y);
                x += 12.0;
            }
            let galley = shell::galley(
                ui,
                text,
                if strong {
                    theme::semibold(11.5)
                } else {
                    theme::regular(11.5)
                },
                if strong {
                    theme::text_soft()
                } else {
                    theme::muted()
                },
            );
            let width = galley.size().x;
            shell::paint_line(ui, galley, x, header_y);
            x += width;
        }
        if let Some(shown) = &self.hearing {
            shell::paint_hearing_note(ui, x, header_y, &shown.board_note());
            shell::paint_hearing_rule(ui, rect);
        }

        let measured = lay_out(tiles.len(), 0.0, 0.0, rect.width(), g);
        let content_width = measured.width.max(rect.width());
        let compact = measured.tile < 66.0;
        let large = tier == Tier::L;
        let sense = if live { Sense::click() } else { Sense::hover() };
        let mut pick = None;
        let mut notch = None;
        let output = egui::ScrollArea::horizontal()
            .id_salt("pro-board-scroll")
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
                let laid = lay_out(
                    tiles.len(),
                    content.left(),
                    content.top() + g.top,
                    content_width,
                    g,
                );
                // The wire, under everything it passes through.
                board::paint_wire(
                    ui,
                    Wire::Straight([
                        Pos2::new(laid.input.x + JACK, laid.input.y),
                        Pos2::new(laid.output.x - JACK, laid.output.y),
                    ]),
                    false,
                    theme::wire(),
                );
                for (centre, input, group) in [
                    (laid.input, true, INPUT_GROUP),
                    (laid.output, false, "output"),
                ] {
                    let hit = Rect::from_center_size(centre, Vec2::splat(2.0 * JACK + 6.0));
                    let response = ui
                        .interact(hit, ui.id().with(("pro-jack", input)), sense)
                        .on_hover_text(if input {
                            "The input: its level and the pickup it expects"
                        } else {
                            "The output: the preset's level and tempo"
                        });
                    let selected = live && self.selected_group == group;
                    if selected {
                        notch = Some(centre.x);
                    }
                    board::paint_jack(ui, centre, input, selected, response.hovered(), None);
                    if response.clicked() {
                        pick = Some(group.to_owned());
                    }
                }
                for (tile, place) in tiles.iter().zip(&laid.tiles) {
                    let response =
                        ui.interact(*place, ui.id().with(("pro-tile", &tile.group)), sense);
                    let selected = live && self.selected_group == tile.group;
                    if selected {
                        notch = Some(place.center().x);
                    }
                    board::paint_tile(
                        ui,
                        *place,
                        &TileLook {
                            name: &tile.name,
                            caption: &tile.caption,
                            colour: theme::category_colour(tile.category),
                            drawing: theme::category_icon(tile.category),
                            on: tile.on,
                            selected,
                            hovered: response.hovered() && live,
                            highlight: None,
                            trying: false,
                            compact,
                            large,
                            snapshot: false,
                            drivers: &[],
                            plain_caption: true,
                            locked: false,
                            share: None,
                        },
                    );
                    let hover = match (tile.caption.is_empty(), tile.on) {
                        (true, true) => tile.name.clone(),
                        (true, false) => format!("{}, off", tile.name),
                        (false, true) => format!("{}: {}", tile.name, tile.caption),
                        (false, false) => format!("{}: {}, off", tile.name, tile.caption),
                    };
                    if response.on_hover_text(hover).clicked() {
                        pick = Some(tile.group.clone());
                    }
                }
            });
        // Wider than the window: the right edge fades, and says so.
        let scrolled = output.state.offset.x;
        let shown = output.inner_rect;
        if measured.overflow && scrolled + shown.width() < output.content_size.x - 1.0 {
            let fade = Rect::from_min_max(Pos2::new(shown.right() - 64.0, shown.top()), shown.max);
            board::horizontal_fade(ui, fade);
            theme::paint_icon(
                ui,
                Icon::ChevronRight,
                Pos2::new(shown.right() - 18.0, shown.center().y),
                16.0,
                theme::muted(),
            );
        }
        if let Some(group) = pick {
            if group != self.selected_group {
                self.typing = None;
            }
            self.selected_group = group;
            self.search.clear();
        }
        notch.map(|x| x - scrolled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pedal's twelve blocks fill the board beside the sidebar at the
    /// reference size, and scroll rather than squeeze on a narrow one.
    #[test]
    fn the_whole_chain_fits_the_reference_board() {
        let g = Geometry::pro(Tier::M);
        let laid = lay_out(12, 0.0, 0.0, 1048.0, &g);
        assert!(!laid.overflow);
        assert!(laid.tile >= 68.0 && laid.tile <= 112.0, "{}", laid.tile);
        assert!(laid.input.x + JACK < laid.tiles[0].left());
        assert!(laid.tiles[11].right() < laid.output.x - JACK);
        assert!(laid.output.x + g.endpoint / 2.0 <= 1048.0 - g.pad + 0.5);
        for pair in laid.tiles.windows(2) {
            assert_eq!(pair[1].left() - pair[0].right(), g.wire);
        }

        let narrow = lay_out(12, 0.0, 0.0, 808.0, &Geometry::pro(Tier::S));
        assert!(narrow.overflow);
        assert_eq!(narrow.tile, 64.0);
        assert!(narrow.width > 808.0);
    }

    /// Few blocks sit in the middle of the board at their widest.
    #[test]
    fn a_short_chain_is_centred() {
        let g = Geometry::pro(Tier::L);
        let laid = lay_out(3, 0.0, 0.0, 2288.0, &g);
        assert_eq!(laid.tile, g.max_tile);
        let left_room = laid.input.x - g.endpoint / 2.0;
        let right_room = 2288.0 - (laid.output.x + g.endpoint / 2.0);
        assert!((left_room - right_room).abs() < 1.0);
    }

    #[test]
    fn tiles_are_named_as_the_pedal_names_its_blocks() {
        assert_eq!(tile_name("gate"), "Gate");
        assert_eq!(tile_name("exp"), "Wah");
        assert_eq!(tile_name("mod_pre"), "Pre Mod");
        assert_eq!(tile_name("amp"), "Amp");
        assert_eq!(tile_name("chorus2"), "Chorus2");
        // Firmware 2.x's own blocks, the second of a kind numbered.
        assert_eq!(tile_name("delay2"), "Delay 2");
        assert_eq!(tile_name("eq2"), "EQ 2");
        assert_eq!(tile_name("dtchr"), "Detune Chorus");
        assert_eq!(tile_name("pickup"), "Pickup Sim");
    }

    /// The board is as tall as a 2.x chain's deepest bank, and 1.5.12's one
    /// row as it was.
    #[test]
    fn the_board_is_as_tall_as_the_chain_it_draws() {
        use super::super::demo::{snapshot_2x, DemoChain};
        let g = Geometry::pro(Tier::M);
        let row = g.top + g.tile_height + g.bottom;
        let mut panel = Panel::new(egui::Context::default());
        panel.show_demo();
        assert_eq!(board_height(panel.snapshot.as_ref().unwrap(), Tier::M), row);
        assert_eq!(
            board_height(&snapshot_2x(&DemoChain::Default), Tier::M),
            row
        );
        assert_eq!(
            board_height(&snapshot_2x(&DemoChain::ThreeWay), Tier::M),
            row + 2.0 * g.lane_pitch()
        );
    }
}
