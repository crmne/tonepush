//! Firmware 2.x's chain on the board (docs/design/redesign-2026-10-01,
//! screen 09; docs/_reference/stompstation-router.md): the pedal's router
//! drawn as the pedal reports it, its positions between the input and
//! output jacks.
//!
//! A position holds a block, drawn as the same tile as an HX block's, or
//! nothing, drawn as a narrow dashed slot whose "+" opens the block picker.
//! Fixed positions wear a lock. Positions joined in parallel are one bank,
//! stacked as lanes between a split and a sum, the way an HX branch is
//! drawn. A connector is chosen on the wire between two positions, or
//! between two lanes.
//!
//! Every edit is a whole new chain computed from the one the pedal last
//! reported, written once by the worker; the board changes when the pedal
//! has said what it kept.

use std::ops::Range;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use voidx_proto::router::{EditError, Fixed, Link, Router, RouterNode, PATH};

use super::*;
use crate::board::{self, Geometry, TileLook, Wire, JACK};
use crate::shell;
use crate::theme::{Icon, Tier};

/// The block picker, open on a position: what a block chosen in it goes
/// into.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Picking {
    pub(crate) position: usize,
    /// The block a choice replaces, when the position holds one.
    pub(crate) replacing: Option<String>,
    /// The category the rail shows; `None` is every block.
    pub(crate) category: Option<&'static str>,
}

/// A block the chain can hold: a child of `root\app` whose item type is
/// `block` and whose first control is its on/off.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Placeable {
    pub(crate) group: String,
    pub(crate) path: String,
    /// The category it is drawn in, from its style's `cat:`.
    pub(crate) category: &'static str,
}

/// The 2.x chain as the board draws it: the router as the pedal last
/// reported it, and the blocks the chain can hold.
#[derive(Clone, Debug)]
pub(crate) struct Chain {
    pub(crate) node: RouterNode,
    /// The router's own description, which an undo writes it back through.
    pub(crate) description: NodeDescription,
    pub(crate) blocks: Vec<Placeable>,
    /// Whether an empty branch of a bank passes the dry signal, as on
    /// 2.2.6, rather than nothing, as on 2.0.10.
    pub(crate) dry_branches: bool,
}

impl Chain {
    /// The chain a snapshot holds, when its firmware has a router.
    pub(crate) fn of(snapshot: &Snapshot) -> Option<Chain> {
        let (_, description) = snapshot.app.iter().find(|(path, description)| {
            path.as_str() == PATH && description.kind == Some(NodeKind::Router)
        })?;
        let node = RouterNode::from_description(description).ok()?;
        Some(Chain {
            node,
            description: description.clone(),
            blocks: placeable(snapshot),
            dry_branches: dry_branches(&snapshot.identity.version),
        })
    }

    pub(crate) fn router(&self) -> &Router {
        &self.node.value
    }

    pub(crate) fn fixed(&self) -> &Fixed {
        &self.node.fixed
    }

    pub(crate) fn positions(&self) -> usize {
        self.node.value.positions()
    }

    /// The block a position holds, when it is one the pedal has.
    pub(crate) fn held(&self, position: usize) -> Option<&Placeable> {
        let path = self.node.value.block(position)?;
        self.blocks.iter().find(|block| block.path == path)
    }

    /// What a position holds when it names no block the pedal has: the
    /// pedal stores it and passes nothing there.
    pub(crate) fn stray(&self, position: usize) -> Option<&str> {
        self.node
            .value
            .block(position)
            .filter(|_| self.held(position).is_none())
    }

    /// A block by its group, when the chain can hold it.
    pub(crate) fn find(&self, group: &str) -> Option<&Placeable> {
        self.blocks.iter().find(|block| block.group == group)
    }

    /// Where a block is, when the chain holds it.
    pub(crate) fn position_of(&self, group: &str) -> Option<usize> {
        self.node.value.position_of(&self.find(group)?.path)
    }

    /// The blocks in the chain, in the order the signal meets them.
    pub(crate) fn groups(&self) -> Vec<String> {
        (0..self.positions())
            .filter_map(|position| self.held(position).map(|block| block.group.clone()))
            .collect()
    }

    /// The blocks the chain can hold that it does not, in the pedal's
    /// order.
    pub(crate) fn available(&self) -> Vec<&Placeable> {
        self.blocks
            .iter()
            .filter(|block| !self.node.value.contains(&block.path))
            .collect()
    }

    /// The chain with a block put in a position: placed there when it is
    /// empty, in place of what it holds otherwise, a string that names no
    /// block included.
    pub(crate) fn put(&self, position: usize, block: &Placeable) -> Result<Router, EditError> {
        let router = self.router();
        if router.block(position).is_some() {
            router.replace(self.fixed(), position, &block.path)
        } else {
            router.place(self.fixed(), &block.path, position)
        }
    }

    /// What a position is called in a sentence: its block's name, or the
    /// position itself.
    pub(crate) fn label(&self, position: usize) -> String {
        match self.held(position) {
            Some(block) => super::board::tile_name(&block.group),
            None => format!("position {}", position + 1),
        }
    }
}

/// Whether an empty branch of a parallel bank passes the dry signal. 2.0.10
/// passes nothing there and 2.2.6 the dry input; firmware between or after
/// them is taken to work as 2.2.6 does.
pub(crate) fn dry_branches(version: &str) -> bool {
    !version.starts_with("2.0.")
}

/// The blocks a snapshot's chain can hold, in the pedal's order.
pub(crate) fn placeable(snapshot: &Snapshot) -> Vec<Placeable> {
    let mut blocks = Vec::new();
    for (index, (path, description)) in snapshot.app.iter().enumerate() {
        let Some(group) = path.as_str().strip_prefix("root\\app\\") else {
            continue;
        };
        if group.contains('\\') || description.item_type.as_deref() != Some("block") {
            continue;
        }
        let prefix = format!("{}\\", path.as_str());
        let first = snapshot.app[index + 1..].iter().find_map(|(child, _)| {
            child
                .as_str()
                .strip_prefix(&prefix)
                .filter(|leaf| !leaf.contains('\\'))
        });
        if first != Some("on_off") {
            continue;
        }
        blocks.push(Placeable {
            group: group.to_owned(),
            path: path.to_string(),
            category: style_category(description).unwrap_or_else(|| group_category(group)),
        });
    }
    blocks
}

/// A block's category from its style's `cat:` (firmware 2.x), in the names
/// the shared palette and drawings use. `None` without a style; `""` for a
/// category the palette has no colour for, such as `Misc`.
fn style_category(description: &NodeDescription) -> Option<&'static str> {
    let style = description.extra.get("style")?.as_str()?;
    let cat = style
        .split(';')
        .find_map(|part| part.trim().strip_prefix("cat:"))?;
    Some(match cat.trim() {
        "Gate" | "Comp" | "Dynamics" => "Dynamics",
        "Pitch" => "Pitch/Synth",
        "Expr" | "Wah" => "Wah",
        "Mod" => "Modulation",
        "Drive" => "Distortion",
        "Amp" => "Amp",
        "IR" | "Cab" => "IR",
        "EQ" => "EQ",
        "Filter" => "Filter",
        "Delay" => "Delay",
        "Reverb" => "Reverb",
        _ => "",
    })
}

/// A block's category: its style's when the firmware gives one, else the
/// one 1.5.12's blocks are known by.
pub(crate) fn block_category(snapshot: &Snapshot, group: &str) -> &'static str {
    snapshot
        .app
        .iter()
        .find(|(path, _)| {
            path.as_str()
                .strip_prefix("root\\app\\")
                .is_some_and(|name| name == group)
        })
        .and_then(|(_, description)| style_category(description))
        .unwrap_or_else(|| group_category(group))
}

/// A category's drawing, or a plain waveform for a block in none.
pub(crate) fn block_drawing(category: &str) -> Option<theme::Art> {
    theme::category_icon(category)
        .or_else(|| Some(theme::Art::whole(Icon::AudioWaveform.uri().to_owned())))
}

/// What a 2.x block's tile says under its name when it has no model or
/// mode to show.
fn caption_2x(group: &str) -> &'static str {
    match group {
        "gate" => "Noise gate",
        "exp" => "Expression",
        "flanger" | "chorus" => "Vintage",
        "eq" | "eq2" => "Parametric",
        _ => "",
    }
}

/// A 2.x block's tile caption: what it plays, or what kind it is.
pub(crate) fn caption(snapshot: &Snapshot, group: &str) -> String {
    block_model_name(snapshot, group).unwrap_or_else(|| caption_2x(group).to_owned())
}

/// The 2.x chain's measurements: the PRO's, with the design's shorter
/// wires (6, 6 and 12 points) so fourteen positions fit beside the sidebar,
/// and the width of an empty position.
pub(crate) fn geometry(tier: Tier) -> (Geometry, f32) {
    let mut g = Geometry::pro(tier);
    g.wire = tier.pick(6.0, 6.0, 12.0);
    (g, tier.pick(16.0, 16.0, 24.0))
}

/// What laying the chain out needs to know of a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Holds {
    /// A block, on or off.
    Block { on: bool },
    /// Nothing, or a string that names no block.
    Nothing,
}

/// Where a position is drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spot {
    /// The tile, or an empty position's slot.
    pub(crate) rect: Rect,
    /// Its lane in its bank; 0 is the main line.
    pub(crate) lane: usize,
    /// Whether it is drawn as an empty position's slot.
    pub(crate) slot: bool,
}

/// A dot where a bank splits or is summed, or both where two banks meet.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Junction {
    pub(crate) at: Pos2,
    /// The bank summed here.
    pub(crate) sums: Option<Range<usize>>,
    /// The bank split here.
    pub(crate) splits: Option<Range<usize>>,
}

/// Where the connector before a position is chosen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Toggle {
    /// The position the connector comes before.
    pub(crate) position: usize,
    pub(crate) link: Link,
    /// Where its chip is drawn.
    pub(crate) centre: Pos2,
    /// The area that shows the chip under the pointer and takes its click.
    pub(crate) zone: Rect,
}

/// Where everything on the 2.x board sits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Laid {
    pub(crate) input: Pos2,
    pub(crate) output: Pos2,
    /// Every position, in order.
    pub(crate) spots: Vec<Spot>,
    /// Every wire, and whether signal runs on it.
    pub(crate) wires: Vec<(Wire, bool)>,
    pub(crate) junctions: Vec<Junction>,
    /// The connector before every position but the first.
    pub(crate) toggles: Vec<Toggle>,
    /// The tiles' width.
    pub(crate) tile: f32,
    /// How many lanes the deepest bank stacks.
    pub(crate) lanes: usize,
    /// The content's width, padding included.
    pub(crate) width: f32,
    /// Whether even the narrowest tiles do not fit, so the board scrolls.
    pub(crate) overflow: bool,
}

/// One stage as it was placed: its left and right edges, and where a bank
/// splits and is summed.
struct Placed {
    left: f32,
    right: f32,
    split: Option<Pos2>,
    sum: Option<Pos2>,
}

/// How tall the board is for a chain whose deepest bank stacks `lanes`.
pub(crate) fn board_height(g: &Geometry, lanes: usize) -> f32 {
    let lanes = lanes.max(1) as f32;
    g.top + lanes * g.tile_height + (lanes - 1.0) * g.lane_gap + g.bottom
}

/// Lay the chain out from `left`, `top` across `width`: the input jack,
/// every stage, the output jack, with tiles as wide as fill the width
/// between their limits, centred when they stop short of it. A bank's lanes
/// stack under the main line; an empty position is `slot` wide unless a
/// tile shares its bank.
pub(crate) fn lay_out(
    router: &Router,
    holds: &[Holds],
    dry_branches: bool,
    (left, top, width): (f32, f32, f32),
    g: &Geometry,
    slot: f32,
) -> Laid {
    let stages = router.stages();
    let block = |position: usize| matches!(holds.get(position), Some(Holds::Block { .. }));
    let bank = |index: usize| stages.get(index).is_some_and(|stage| stage.len() > 1);
    // The gap after a stage (`None` for the input), up to the next stage or
    // the output: a bank's split or sum takes a junction's width, and two
    // banks meeting share one dot with room on both sides of it.
    let gap_after = |index: Option<usize>| {
        let before = index.is_some_and(bank);
        let after = bank(index.map_or(0, |index| index + 1));
        match (before, after) {
            (true, true) => g.junction + 8.0,
            (true, false) | (false, true) => g.junction,
            (false, false) => g.wire,
        }
    };
    let widest = |stage: &Range<usize>| stage.clone().any(block);

    let mut fixed = 2.0 * g.endpoint + gap_after(None);
    let mut units = 0.0;
    for (index, stage) in stages.iter().enumerate() {
        fixed += gap_after(Some(index));
        if widest(stage) {
            units += 1.0;
        } else {
            fixed += slot;
        }
    }
    let available = width - 2.0 * g.pad;
    let (tile, overflow) = board::tile_width(available, fixed, units, g);
    let natural = fixed + units * tile;
    let start = left + g.pad + ((available - natural) / 2.0).max(0.0);
    let main = top + g.tile_height / 2.0;
    let pitch = g.lane_pitch();

    let input = Pos2::new(start + g.endpoint / 2.0, main);
    let mut laid = Laid {
        input,
        output: input,
        spots: vec![
            Spot {
                rect: Rect::NOTHING,
                lane: 0,
                slot: true,
            };
            router.positions()
        ],
        wires: Vec::new(),
        junctions: Vec::new(),
        toggles: Vec::new(),
        tile,
        lanes: stages.iter().map(|stage| stage.len()).max().unwrap_or(1),
        width: natural + 2.0 * g.pad,
        overflow,
    };
    let slot_rect = |x: f32, lane_top: f32, width: f32| {
        Rect::from_min_size(
            Pos2::new(x, lane_top + g.tile_height * 0.2),
            Vec2::new(width, g.tile_height * 0.6),
        )
    };
    // Where the signal leaves what came before, and the right edge of it.
    let mut out = Pos2::new(input.x + JACK, main);
    let mut right = start + g.endpoint;
    let mut placed: Vec<Placed> = Vec::with_capacity(stages.len());
    for (index, stage) in stages.iter().enumerate() {
        let stage_left = right + gap_after(index.checked_sub(1));
        let stage_width = if widest(stage) { tile } else { slot };
        let stage_right = stage_left + stage_width;
        if stage.len() == 1 {
            let position = stage.start;
            laid.wires
                .push((Wire::Straight([out, Pos2::new(stage_left, main)]), true));
            laid.spots[position] = Spot {
                rect: if block(position) {
                    Rect::from_min_size(Pos2::new(stage_left, top), Vec2::new(tile, g.tile_height))
                } else {
                    slot_rect(stage_left, top, slot)
                },
                lane: 0,
                slot: !block(position),
            };
            out = Pos2::new(stage_right, main);
            placed.push(Placed {
                left: stage_left,
                right: stage_right,
                split: None,
                sum: None,
            });
            right = stage_right;
            continue;
        }

        // A bank: split, lanes, sum. Meeting another bank, the split is the
        // dot that bank is summed at.
        let split = match laid.junctions.last_mut() {
            Some(shared) if index > 0 && bank(index - 1) => {
                shared.splits = Some(stage.clone());
                shared.at
            }
            _ => {
                let split = Pos2::new(right + 5.0, main);
                laid.wires.push((Wire::Straight([out, split]), true));
                laid.junctions.push(Junction {
                    at: split,
                    sums: None,
                    splits: Some(stage.clone()),
                });
                split
            }
        };
        let next_gap = gap_after(Some(index));
        let sum = if bank(index + 1) {
            Pos2::new(stage_right + next_gap / 2.0, main)
        } else {
            Pos2::new(stage_right + next_gap - 5.0, main)
        };
        let contributes = stage
            .clone()
            .any(|position| holds.get(position) == Some(&Holds::Block { on: true }));
        for (lane, position) in stage.clone().enumerate() {
            let y = main + lane as f32 * pitch;
            let lane_top = top + lane as f32 * pitch;
            let item = if block(position) { tile } else { slot };
            let item_left = stage_left + (stage_width - item) / 2.0;
            let item_right = item_left + item;
            // An empty branch passes nothing on 2.0.10, unless no branch
            // passes anything, when the dry input does.
            let live = block(position) || dry_branches || !contributes;
            let lead_in = (stage_left - split.x) * 0.6;
            laid.wires.push((
                if lane == 0 {
                    Wire::Straight([split, Pos2::new(stage_left, y)])
                } else {
                    Wire::Curve([
                        split,
                        Pos2::new(split.x + lead_in, split.y),
                        Pos2::new(stage_left - lead_in, y),
                        Pos2::new(stage_left, y),
                    ])
                },
                live,
            ));
            if item_left > stage_left {
                laid.wires.push((
                    Wire::Straight([Pos2::new(stage_left, y), Pos2::new(item_left, y)]),
                    live,
                ));
            }
            if item_right < stage_right {
                laid.wires.push((
                    Wire::Straight([Pos2::new(item_right, y), Pos2::new(stage_right, y)]),
                    live,
                ));
            }
            let lead_out = (sum.x - stage_right) * 0.6;
            laid.wires.push((
                if lane == 0 {
                    Wire::Straight([Pos2::new(stage_right, y), sum])
                } else {
                    Wire::Curve([
                        Pos2::new(stage_right, y),
                        Pos2::new(stage_right + lead_out, y),
                        Pos2::new(sum.x - lead_out, sum.y),
                        sum,
                    ])
                },
                live,
            ));
            laid.spots[position] = Spot {
                rect: if block(position) {
                    Rect::from_min_size(
                        Pos2::new(item_left, lane_top),
                        Vec2::new(tile, g.tile_height),
                    )
                } else {
                    slot_rect(item_left, lane_top, slot)
                },
                lane,
                slot: !block(position),
            };
        }
        laid.junctions.push(Junction {
            at: sum,
            sums: Some(stage.clone()),
            splits: None,
        });
        out = sum;
        placed.push(Placed {
            left: stage_left,
            right: stage_right,
            split: Some(split),
            sum: Some(sum),
        });
        right = stage_right;
    }
    let output_left = right + gap_after(Some(stages.len().saturating_sub(1)));
    laid.output = Pos2::new(output_left + g.endpoint / 2.0, main);
    laid.wires.push((
        Wire::Straight([out, Pos2::new(laid.output.x - JACK, main)]),
        true,
    ));

    // The connectors: between two lanes of a bank, or on the line between
    // two stages, at the dot of a bank they meet.
    let stage_of = |position: usize| {
        stages
            .iter()
            .position(|stage| stage.contains(&position))
            .unwrap_or(0)
    };
    for position in 1..router.positions() {
        let link = router.link(position).unwrap_or(Link::Series);
        let (before, after) = (stage_of(position - 1), stage_of(position));
        let (centre, zone) = if before == after {
            let stage = &placed[after];
            let lane = position - stages[after].start;
            let centre = Pos2::new(
                (stage.left + stage.right) / 2.0,
                main + (lane as f32 - 0.5) * pitch,
            );
            let width = ((stage.right - stage.left) * 0.5).max(18.0);
            (
                centre,
                Rect::from_center_size(centre, Vec2::new(width, g.lane_gap + 12.0)),
            )
        } else if let Some(sum) = placed[before].sum {
            (sum, Rect::from_center_size(sum, Vec2::new(20.0, 26.0)))
        } else if let Some(split) = placed[after].split {
            (split, Rect::from_center_size(split, Vec2::new(20.0, 26.0)))
        } else {
            let (from, to) = (placed[before].right, placed[after].left);
            let centre = Pos2::new((from + to) / 2.0, main);
            (
                centre,
                Rect::from_center_size(centre, Vec2::new(to - from + 8.0, 24.0)),
            )
        };
        laid.toggles.push(Toggle {
            position,
            link,
            centre,
            zone,
        });
    }
    laid
}

/// What the pointer asked of the 2.x board this frame.
enum Asked {
    /// Show a block's controls, or the input's or the output's.
    Select(String),
    /// Open the block picker on a position.
    Pick(usize),
    /// A new chain, and the block to select once the pedal holds it.
    Route(Result<Router, EditError>, Option<String>),
}

impl Panel {
    /// Whether the chain can be edited now: the pedal online and not busy,
    /// and no other chain on its way to it.
    pub(super) fn can_route(&self) -> bool {
        self.online
            && !self.busy
            && !self.routing
            && self.working.is_none()
            && !self.update_mode
            && self.firmware.is_none()
    }

    /// Send a new chain to the pedal, once. The board keeps the chain it
    /// has until the pedal says what it kept.
    pub(super) fn route(
        &mut self,
        chain: &Chain,
        next: Result<Router, EditError>,
        select: Option<String>,
    ) {
        match next {
            Ok(next) if next != *chain.router() => {
                self.routing = true;
                self.select_routed = select;
                let _ = self.tx.send(Cmd::SetRouter {
                    description: Box::new(chain.description.clone()),
                    before: chain.router().clone(),
                    chain: next,
                });
            }
            Ok(_) | Err(EditError::Unchanged) => {}
            Err(error) => {
                self.failed = true;
                self.status = format!("The chain was not changed: {error}");
            }
        }
    }

    /// The 2.x chain drawn into `ui`: with `live`, its positions, wires and
    /// jacks answer the pointer. Returns where the notch under the chosen
    /// one goes.
    pub(super) fn draw_route(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        chain: &Chain,
        tier: Tier,
        live: bool,
    ) -> Option<f32> {
        let (g, slot) = geometry(tier);
        let rect = ui.max_rect();
        theme::paint::dots(ui.painter(), rect, theme::dot());
        let holds: Vec<Holds> = (0..chain.positions())
            .map(|position| match chain.held(position) {
                Some(block) => Holds::Block {
                    on: block_enabled(snapshot, &block.group),
                },
                None => Holds::Nothing,
            })
            .collect();
        let editable = live && self.can_route();
        if !editable {
            self.route_drag = None;
        }
        self.route_header(ui, chain, rect);

        let measured = lay_out(
            chain.router(),
            &holds,
            chain.dry_branches,
            (0.0, 0.0, rect.width()),
            &g,
            slot,
        );
        let content_width = measured.width.max(rect.width());
        // Deeper than the room the board was given: its lanes scroll.
        let content_height = board_height(&g, measured.lanes).max(rect.height());
        let compact = measured.tile < 66.0;
        let large = tier == Tier::L;
        let fixed = chain.fixed().clone();
        let mut asked: Option<Asked> = None;
        let mut notch = None;
        // Only sideways, as every board scrolls, unless a bank is deeper than
        // the room: a wheel then still scrolls the chain along.
        let output = egui::ScrollArea::new([true, content_height > rect.height() + 0.5])
            .id_salt("pro-route-scroll")
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            })
            .show(ui, |ui| {
                let content = Rect::from_min_size(
                    Pos2::new(ui.max_rect().left(), ui.max_rect().top()),
                    Vec2::new(content_width, content_height),
                );
                ui.allocate_rect(content, Sense::hover());
                let laid = lay_out(
                    chain.router(),
                    &holds,
                    chain.dry_branches,
                    (content.left(), content.top() + g.top, content_width),
                    &g,
                    slot,
                );
                // What is chosen comes into view when the choice changes: on
                // a window too narrow for the chain, the notch would point at
                // nothing.
                let focus = match &self.picking {
                    Some(picking) => laid
                        .spots
                        .get(picking.position)
                        .map(|spot| (format!("@{}", picking.position), spot.rect)),
                    None => chain
                        .position_of(&self.selected_group)
                        .map(|position| (self.selected_group.clone(), laid.spots[position].rect)),
                };
                // One already in view stays where the pointer left it.
                if let Some((key, rect)) = focus.filter(|_| live) {
                    if self.route_focus.as_deref() != Some(key.as_str()) {
                        if !ui.clip_rect().contains_rect(rect) {
                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                        }
                        self.route_focus = Some(key);
                    }
                }

                // The wires under everything, then the dots on them.
                for (wire, carries) in &laid.wires {
                    if *carries {
                        board::paint_wire(ui, *wire, false, theme::wire());
                    } else {
                        paint_silent_wire(ui, *wire);
                    }
                }
                for junction in &laid.junctions {
                    ui.painter().circle(
                        junction.at,
                        4.5,
                        theme::bg_deep(),
                        Stroke::new(2.0, theme::wire()),
                    );
                }

                // The jacks.
                for (centre, input, group) in [
                    (laid.input, true, INPUT_GROUP),
                    (laid.output, false, "output"),
                ] {
                    let hit = Rect::from_center_size(centre, Vec2::splat(2.0 * JACK + 6.0));
                    let sense = if live { Sense::click() } else { Sense::hover() };
                    let response = ui
                        .interact(hit, ui.id().with(("pro-route-jack", input)), sense)
                        .on_hover_text(if input {
                            "The input: its level and the pickup it expects"
                        } else {
                            "The output: the preset's level and tempo"
                        });
                    let selected = live && self.picking.is_none() && self.selected_group == group;
                    if selected {
                        notch = Some(centre.x);
                    }
                    board::paint_jack(ui, centre, input, selected, response.hovered(), None);
                    if response.clicked() {
                        asked = Some(Asked::Select(group.to_owned()));
                    }
                }

                // The positions.
                for (position, (spot, held)) in laid.spots.iter().zip(&holds).enumerate() {
                    let pinned = fixed.position(position);
                    let Some(block) = chain.held(position) else {
                        let picked = live
                            && self
                                .picking
                                .as_ref()
                                .is_some_and(|picking| picking.position == position);
                        if picked {
                            notch = Some(spot.rect.center().x);
                        }
                        let stray = chain.stray(position);
                        let sense = if editable && !pinned {
                            Sense::click()
                        } else {
                            Sense::hover()
                        };
                        let response = ui.interact(
                            spot.rect.expand2(Vec2::new(3.0, 0.0)),
                            ui.id().with(("pro-route-slot", position)),
                            sense,
                        );
                        let hovered =
                            editable && !pinned && response.hovered() && self.route_drag.is_none();
                        paint_slot(ui, spot.rect, picked, hovered, stray.is_some());
                        if live
                            && response
                                .on_hover_text(slot_words(chain, position, stray))
                                .clicked()
                        {
                            asked = Some(Asked::Pick(position));
                        }
                        continue;
                    };
                    let group = block.group.clone();
                    let name = super::board::tile_name(&group);
                    let caption = caption(snapshot, &group);
                    let on = matches!(held, Holds::Block { on: true });
                    let sense = if !live {
                        Sense::hover()
                    } else if editable && !pinned {
                        Sense::click_and_drag()
                    } else {
                        Sense::click()
                    };
                    let response =
                        ui.interact(spot.rect, ui.id().with(("pro-route-tile", position)), sense);
                    let replacing = live
                        && self
                            .picking
                            .as_ref()
                            .is_some_and(|picking| picking.position == position);
                    let selected = live && self.picking.is_none() && self.selected_group == group;
                    if selected || replacing {
                        notch = Some(spot.rect.center().x);
                    }
                    let carried = self.route_drag == Some(position);
                    board::paint_tile(
                        ui,
                        spot.rect,
                        &TileLook {
                            name: &name,
                            caption: &caption,
                            colour: theme::category_colour(block.category),
                            drawing: block_drawing(block.category),
                            on,
                            selected,
                            hovered: response.hovered() && live && !carried,
                            highlight: None,
                            trying: replacing,
                            compact,
                            large,
                            snapshot: false,
                            drivers: &[],
                            plain_caption: true,
                            locked: pinned,
                            share: None,
                        },
                    );
                    if carried {
                        ui.painter().rect_filled(
                            spot.rect.expand(2.0),
                            CornerRadius::same(theme::RADIUS_TILE + 2),
                            theme::alpha(theme::bg_deep(), 0.6),
                        );
                    }
                    if !live {
                        continue;
                    }
                    let response = response
                        .on_hover_text(tile_words(&name, &caption, on, position, pinned, editable));
                    if editable && !pinned && response.drag_started() {
                        self.route_drag = Some(position);
                    }
                    if response.clicked() {
                        asked = Some(Asked::Select(group.clone()));
                    }
                    if editable {
                        if let Some(menu) = tile_menu(&response, chain, position, &name, pinned) {
                            asked = Some(menu);
                        }
                    }
                }

                // The connectors, under the pointer.
                if editable && self.route_drag.is_none() {
                    for toggle in &laid.toggles {
                        if fixed.link(toggle.position) {
                            continue;
                        }
                        let response = ui.interact(
                            toggle.zone,
                            ui.id().with(("pro-route-link", toggle.position)),
                            Sense::click(),
                        );
                        if !response.hovered() {
                            continue;
                        }
                        paint_link_chip(ui, toggle.centre, toggle.link);
                        if response
                            .on_hover_text(link_words(chain, toggle.position, toggle.link))
                            .clicked()
                        {
                            let flipped = match toggle.link {
                                Link::Series => Link::Parallel,
                                Link::Parallel => Link::Series,
                            };
                            asked = Some(Asked::Route(
                                chain.router().connect(&fixed, toggle.position, flipped),
                                None,
                            ));
                        }
                    }
                }
                if editable {
                    if let Some(dropped) = self.route_drag_frame(ui, chain, &laid) {
                        asked = Some(dropped);
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
        if live {
            if let Some(menu) = chain_menu(ui, chain, rect, editable) {
                asked = Some(menu);
            }
        }
        match asked {
            Some(Asked::Select(group)) => {
                if group != self.selected_group {
                    self.typing = None;
                }
                self.selected_group = group;
                self.search.clear();
                self.picking = None;
            }
            Some(Asked::Pick(position)) => {
                self.typing = None;
                self.picking = Some(Picking {
                    position,
                    replacing: chain.held(position).map(|block| block.group.clone()),
                    category: None,
                });
            }
            Some(Asked::Route(next, select)) => self.route(chain, next, select),
            None => {}
        }
        notch.map(|x| x - scrolled)
    }

    /// The header: how many positions hold a block, how many are fixed, and
    /// what runs in parallel, or what the board is doing.
    fn route_header(&self, ui: &Ui, chain: &Chain, rect: Rect) {
        let y = rect.top() + 20.0;
        let mut x = rect.left() + 20.0;
        let held = (0..chain.positions())
            .filter(|position| chain.held(*position).is_some())
            .count();
        let fixed = (0..chain.positions())
            .filter(|position| chain.fixed().position(*position))
            .count();
        let paint = |text: String, strong: bool, x: &mut f32| {
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
            shell::paint_line(ui, galley, *x, y);
            *x += width;
        };
        let dot = |x: &mut f32| {
            *x += 6.0;
            let dot = shell::galley(ui, "·", theme::regular(11.5), theme::faint());
            shell::paint_line(ui, dot, *x, y);
            *x += 12.0;
        };
        paint(held.to_string(), true, &mut x);
        paint(
            format!(" of {} positions", chain.positions()),
            false,
            &mut x,
        );
        if fixed > 0 {
            dot(&mut x);
            paint(format!("{fixed} fixed"), false, &mut x);
            theme::paint_icon(ui, Icon::Lock, Pos2::new(x + 9.0, y), 11.0, theme::muted());
            x += 16.0;
        }
        if let Some(note) = self.route_note(chain) {
            dot(&mut x);
            paint(note, false, &mut x);
        }
        if let Some(shown) = &self.hearing {
            shell::paint_hearing_note(ui, x, y, &shown.board_note());
            shell::paint_hearing_rule(ui, rect);
        }
    }

    /// What the header says after the counts: what the board is doing, or
    /// what runs in parallel.
    fn route_note(&self, chain: &Chain) -> Option<String> {
        if let Some(from) = self.route_drag {
            return Some(sentence(&format!(
                "Drop {} on another position to swap them",
                chain.label(from)
            )));
        }
        if let Some(picking) = &self.picking {
            return Some(match &picking.replacing {
                Some(group) => format!(
                    "Choosing a block for {}'s place",
                    super::board::tile_name(group)
                ),
                None => format!("Choosing a block for position {}", picking.position + 1),
            });
        }
        let banks: Vec<Range<usize>> = chain
            .router()
            .stages()
            .into_iter()
            .filter(|stage| stage.len() > 1)
            .collect();
        match banks.as_slice() {
            [] => None,
            [bank] => Some(sentence(&format!(
                "{} run in parallel",
                names(chain, bank.clone())
            ))),
            banks => Some(format!("{} parallel banks", banks.len())),
        }
    }

    /// Follow a block being dragged along the chain: the position under the
    /// pointer lights up when the block can go there, the free positions
    /// show faintly that they can take it, and a fixed one refuses it.
    /// Letting go on a position swaps the two; Escape lets go of it.
    fn route_drag_frame(&mut self, ui: &Ui, chain: &Chain, laid: &Laid) -> Option<Asked> {
        let from = self.route_drag?;
        let Some(block) = chain.held(from).filter(|_| !chain.fixed().position(from)) else {
            self.route_drag = None;
            return None;
        };
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.route_drag = None;
            return None;
        }
        let pointer = ui.input(|input| input.pointer.interact_pos())?;
        let targets: Vec<usize> = (0..chain.positions())
            .filter(|position| *position != from && !chain.fixed().position(*position))
            .collect();
        let hot = targets
            .iter()
            .copied()
            .find(|position| laid.spots[*position].rect.expand(6.0).contains(pointer));
        let refused = (0..chain.positions()).any(|position| {
            position != from
                && chain.fixed().position(position)
                && laid.spots[position].rect.contains(pointer)
        });
        ui.ctx().set_cursor_icon(if refused {
            egui::CursorIcon::NotAllowed
        } else {
            egui::CursorIcon::Grabbing
        });
        for position in &targets {
            let spot = &laid.spots[*position];
            let radius = if spot.slot {
                9.0
            } else {
                f32::from(theme::RADIUS_TILE)
            };
            let outline = spot.rect.expand(3.0);
            if hot == Some(*position) {
                ui.painter().rect_filled(
                    outline,
                    CornerRadius::same((radius + 3.0) as u8),
                    theme::alpha(theme::accent(), 0.12),
                );
                theme::paint::dashed_rect(
                    ui.painter(),
                    outline,
                    radius + 3.0,
                    Stroke::new(2.0, theme::accent()),
                    5.0,
                    3.5,
                );
            } else if spot.slot {
                theme::paint::dashed_rect(
                    ui.painter(),
                    outline,
                    radius + 3.0,
                    Stroke::new(1.5, theme::alpha(theme::accent(), 0.45)),
                    4.0,
                    4.0,
                );
            }
        }
        theme::drag_ghost(
            ui.ctx(),
            pointer,
            &super::board::tile_name(&block.group),
            theme::category_colour(block.category),
        );
        if ui.input(|input| input.pointer.any_released()) {
            self.route_drag = None;
            let to = hot?;
            return Some(Asked::Route(
                chain.router().swap(chain.fixed(), from, to),
                Some(block.group.clone()),
            ));
        }
        None
    }
}

/// The chain's own menu at the right of the header: the default chain, or
/// a cleared one. Returns the edit it asked for.
fn chain_menu(ui: &mut Ui, chain: &Chain, rect: Rect, editable: bool) -> Option<Asked> {
    let place = Rect::from_center_size(
        Pos2::new(rect.right() - 16.0 - 13.0, rect.top() + 20.0),
        Vec2::splat(26.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(place)
            .id_salt("pro-route-menu")
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let button = theme::IconButton::new(Icon::Ellipsis)
        .small()
        .enabled(editable)
        .show(&mut child)
        .on_hover_text("The whole chain: load the default one, or clear it");
    let mut asked = None;
    egui::Popup::menu(&button).gap(4.0).show(|ui| {
        theme::menu_width(ui, 250.0);
        theme::menu_header(ui, "The chain", None);
        match chain.node.defaulted() {
            Some(default) if default != *chain.router() => {
                if theme::menu_item(ui, Some(Icon::RotateCcw), "Load the default chain", None)
                    .on_hover_text("The chain every preset saved before firmware 2.x plays")
                    .clicked()
                {
                    asked = Some(Asked::Route(Ok(default), None));
                }
            }
            _ => {
                theme::menu_disabled(
                    ui,
                    Some(Icon::RotateCcw),
                    "Load the default chain",
                    Some("loaded"),
                );
            }
        }
        let cleared = chain.router().cleared(chain.fixed());
        if cleared == *chain.router() {
            theme::menu_disabled(ui, Some(Icon::Remove), "Clear the chain", Some("clear"));
        } else if theme::menu_danger(ui, Icon::Remove, "Clear the chain")
            .on_hover_text("Empty every position that is not fixed, and run them all in series")
            .clicked()
        {
            asked = Some(Asked::Route(Ok(cleared), None));
        }
    });
    asked
}

/// A tile's menu: replace the block, run it in series or parallel with
/// its neighbours, or take it out of the chain. Returns what was chosen.
fn tile_menu(
    response: &egui::Response,
    chain: &Chain,
    position: usize,
    name: &str,
    pinned: bool,
) -> Option<Asked> {
    let fixed = chain.fixed();
    let mut asked = None;
    response.context_menu(|ui| {
        theme::menu_width(ui, 260.0);
        theme::menu_header(ui, name, Some(&format!("Position {}", position + 1)));
        if pinned {
            theme::menu_disabled(ui, Some(Icon::LayoutGrid), "Replace…", Some("fixed"));
        } else if theme::menu_item(ui, Some(Icon::LayoutGrid), "Replace…", None).clicked() {
            asked = Some(Asked::Pick(position));
        }
        // The connectors either side: (the position each comes before, the
        // neighbour it joins).
        let neighbours: Vec<(usize, usize)> = [
            position.checked_sub(1).map(|before| (position, before)),
            (position + 1 < chain.positions()).then_some((position + 1, position + 1)),
        ]
        .into_iter()
        .flatten()
        .filter(|(link, _)| !fixed.link(*link))
        .collect();
        if !neighbours.is_empty() {
            theme::menu_separator(ui);
        }
        for (link, other) in neighbours {
            let (icon, label, wanted) = match chain.router().link(link) {
                Some(Link::Parallel) => (
                    Icon::ArrowRight,
                    format!("Run in series with {}", chain.label(other)),
                    Link::Series,
                ),
                _ => (
                    Icon::Layers,
                    format!("Run in parallel with {}", chain.label(other)),
                    Link::Parallel,
                ),
            };
            if theme::menu_item(ui, Some(icon), &label, None).clicked() {
                asked = Some(Asked::Route(
                    chain.router().connect(fixed, link, wanted),
                    None,
                ));
            }
        }
        theme::menu_separator(ui);
        if pinned {
            theme::menu_disabled(
                ui,
                Some(Icon::Remove),
                "Remove from the chain",
                Some(&format!("fixed in position {}", position + 1)),
            );
            return;
        }
        if theme::menu_danger(ui, Icon::Remove, "Remove from the chain")
            .on_hover_text(REMOVED)
            .clicked()
        {
            asked = Some(Asked::Route(chain.router().remove(fixed, position), None));
        }
        if chain.dry_branches
            && chain.router().bank(position).len() > 1
            && theme::menu_danger(ui, Icon::Remove, "Remove, and run the rest in series")
                .on_hover_text(DRY_BRANCH)
                .clicked()
        {
            asked = Some(Asked::Route(
                chain.router().remove_in_series(fixed, position),
                None,
            ));
        }
    });
    asked
}

/// What happens to a block taken out of the chain.
pub(crate) const REMOVED: &str = "It goes silent. Its settings stay until another preset \
     loads, and are not saved with the preset";

/// Why removing a branch offers to run the rest of its bank in series.
pub(crate) const DRY_BRANCH: &str = "On this firmware an empty branch passes the dry \
     signal, which a bank with a branch taken out would mix in";

/// The positions of a range in a sentence: "Delay and Reverb", "Mod,
/// Delay and position 14".
fn names(chain: &Chain, range: Range<usize>) -> String {
    let labels: Vec<String> = range.map(|position| chain.label(position)).collect();
    match labels.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Text made to start a sentence.
fn sentence(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// What a tile says under the pointer.
fn tile_words(
    name: &str,
    caption: &str,
    on: bool,
    position: usize,
    pinned: bool,
    editable: bool,
) -> String {
    let what = match (caption.is_empty(), on) {
        (true, true) => name.to_owned(),
        (true, false) => format!("{name}, off"),
        (false, true) => format!("{name}: {caption}"),
        (false, false) => format!("{name}: {caption}, off"),
    };
    let place = if pinned {
        format!(
            "Fixed in position {}: it cannot move or leave the chain",
            position + 1
        )
    } else if editable {
        format!(
            "Position {}. Drag it onto another position to swap them; right-click for more",
            position + 1
        )
    } else {
        format!("Position {}", position + 1)
    };
    format!("{what}\n{place}")
}

/// What an empty position says under the pointer.
fn slot_words(chain: &Chain, position: usize, stray: Option<&str>) -> String {
    let mut words = match stray {
        Some(stray) => format!(
            "Position {} holds {stray}, which names no block this pedal has, so nothing \
             plays there. Click to put a block in it",
            position + 1
        ),
        None => format!(
            "Position {} is empty. Click to put a block in it",
            position + 1
        ),
    };
    if chain.router().bank(position).len() > 1 {
        words.push_str(if chain.dry_branches {
            "\nIn a parallel bank, an empty branch passes the dry signal on this firmware"
        } else {
            "\nIn a parallel bank, an empty branch passes nothing on this firmware"
        });
    }
    words
}

/// What a connector's chip says under the pointer.
fn link_words(chain: &Chain, position: usize, link: Link) -> String {
    let bank = chain.router().bank(position);
    match link {
        Link::Series => sentence(&format!(
            "Run {} in parallel with {}",
            names(chain, bank),
            names(chain, chain.router().bank(position - 1))
        )),
        Link::Parallel => sentence(&format!(
            "Run {} after {}, in series",
            names(chain, position..bank.end),
            names(chain, bank.start..position)
        )),
    }
}

/// An empty position: a narrow dashed slot with a "+", amber when the
/// picker is open on it or the pointer is over it, and a warning instead
/// when it holds a string that names no block.
fn paint_slot(ui: &Ui, rect: Rect, picked: bool, hovered: bool, stray: bool) {
    let painter = ui.painter();
    let (fill, line, ink) = if picked {
        (theme::accent_soft(), theme::accent(), theme::accent())
    } else if hovered {
        (
            theme::alpha(theme::accent_soft(), 0.7),
            theme::accent_line(),
            theme::accent(),
        )
    } else {
        (
            theme::alpha(theme::bg(), 0.4),
            theme::line_strong(),
            theme::faint(),
        )
    };
    painter.rect_filled(rect, CornerRadius::same(9), fill);
    theme::paint::dashed_rect(
        painter,
        rect.shrink(0.75),
        8.25,
        Stroke::new(1.5, line),
        4.0,
        3.0,
    );
    if stray && !picked {
        theme::paint_icon(ui, Icon::TriangleAlert, rect.center(), 12.0, theme::hot());
    } else {
        theme::paint_icon(ui, Icon::Plus, rect.center(), 13.0, ink);
    }
}

/// A wire no signal runs on: an empty branch that passes nothing.
fn paint_silent_wire(ui: &Ui, wire: Wire) {
    let points = match wire {
        Wire::Straight([from, to]) => vec![from, to],
        Wire::Curve(points) => egui::epaint::CubicBezierShape::from_points_stroke(
            points,
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        )
        .flatten(Some(0.5)),
    };
    ui.painter().extend(egui::Shape::dashed_line(
        &points,
        Stroke::new(1.5, theme::alpha(theme::wire(), 0.7)),
        3.0,
        3.0,
    ));
}

/// The chip that chooses a connector, drawn under the pointer as the
/// board's "+" is: a fork offers parallel, an arrow, one after the other,
/// series.
fn paint_link_chip(ui: &Ui, centre: Pos2, link: Link) {
    let painter = ui.painter();
    painter.circle_filled(centre, 12.0, theme::accent_soft());
    painter.circle_filled(centre, 9.0, theme::accent());
    let ink = Stroke::new(1.6, theme::accent_ink());
    let at = |x: f32, y: f32| centre + Vec2::new(x, y);
    match link {
        Link::Series => {
            painter.line_segment([at(-5.0, 0.0), at(-1.0, 0.0)], ink);
            painter.line_segment([at(-1.0, 0.0), at(4.5, -3.5)], ink);
            painter.line_segment([at(-1.0, 0.0), at(4.5, 3.5)], ink);
        }
        Link::Parallel => {
            painter.line_segment([at(-5.0, 0.0), at(4.5, 0.0)], ink);
            painter.line_segment([at(1.5, -3.0), at(4.5, 0.0)], ink);
            painter.line_segment([at(1.5, 3.0), at(4.5, 0.0)], ink);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::super::demo::DemoChain;
    use super::*;

    fn row(value: serde_json::Value) -> Router {
        Router::from_value(&value).expect("a router row")
    }

    /// What each position of the demo's default chain holds: twelve blocks
    /// and two empty positions.
    fn default_holds() -> Vec<Holds> {
        (0..14)
            .map(|position| {
                if matches!(position, 9 | 13) {
                    Holds::Nothing
                } else {
                    Holds::Block { on: true }
                }
            })
            .collect()
    }

    fn default_chain() -> Router {
        row(DemoChain::Default.value())
    }

    /// The fourteen positions fit beside the sidebar at the reference size,
    /// in series, with the empty ones as narrow slots on the line.
    #[test]
    fn the_default_chain_fits_beside_the_sidebar() {
        let (g, slot) = geometry(Tier::M);
        let laid = lay_out(
            &default_chain(),
            &default_holds(),
            false,
            (0.0, 0.0, 1048.0),
            &g,
            slot,
        );
        assert!(!laid.overflow);
        assert!(laid.tile >= g.min_tile, "{}", laid.tile);
        assert!(laid.width <= 1048.0, "{}", laid.width);
        assert_eq!(laid.lanes, 1);
        assert!(laid.junctions.is_empty());
        assert!(laid.wires.iter().all(|(_, carries)| *carries));
        for position in [9, 13] {
            let spot = &laid.spots[position];
            assert!(spot.slot);
            assert_eq!(spot.rect.width(), slot);
            assert!((spot.rect.height() - g.tile_height * 0.6).abs() < 0.01);
            assert!((spot.rect.center().y - g.tile_height / 2.0).abs() < 0.01);
        }
        for pair in laid.spots.windows(2) {
            assert!((pair[1].rect.left() - pair[0].rect.right() - g.wire).abs() < 0.01);
        }
        assert!(laid.input.x + JACK < laid.spots[0].rect.left());
        assert!(laid.spots[13].rect.right() < laid.output.x - JACK);
        // A connector before every position but the first, on its gap.
        assert_eq!(laid.toggles.len(), 13);
        let toggle = &laid.toggles[8];
        assert_eq!(toggle.position, 9);
        assert_eq!(toggle.link, Link::Series);
        assert!(toggle.centre.x > laid.spots[8].rect.right());
        assert!(toggle.centre.x < laid.spots[9].rect.left());
    }

    /// A bank stacks its lanes under the main line, between a split after
    /// what comes before it and a sum before what comes after.
    #[test]
    fn a_bank_stacks_its_lanes_between_a_split_and_a_sum() {
        let (g, slot) = geometry(Tier::M);
        let parallel = row(DemoChain::Parallel.value());
        let laid = lay_out(
            &parallel,
            &default_holds(),
            false,
            (0.0, 0.0, 1048.0),
            &g,
            slot,
        );
        assert_eq!(laid.lanes, 2);
        let (delay, reverb) = (&laid.spots[11], &laid.spots[12]);
        assert_eq!((delay.lane, reverb.lane), (0, 1));
        assert_eq!(delay.rect.left(), reverb.rect.left());
        assert!((reverb.rect.top() - delay.rect.top() - g.lane_pitch()).abs() < 0.01);
        let [split, sum] = laid.junctions.as_slice() else {
            panic!("one split and one sum: {:?}", laid.junctions)
        };
        assert_eq!(split.splits, Some(11..13));
        assert_eq!(sum.sums, Some(11..13));
        assert!(laid.spots[10].rect.right() < split.at.x && split.at.x < delay.rect.left());
        assert!(delay.rect.right() < sum.at.x && sum.at.x < laid.spots[13].rect.left());
        // The connectors: at the split, between the lanes, at the sum.
        let toggle = |position: usize| &laid.toggles[position - 1];
        assert_eq!(toggle(11).centre, split.at);
        assert_eq!(toggle(12).link, Link::Parallel);
        assert!(toggle(12).centre.y > delay.rect.bottom() - 1.0);
        assert!(toggle(12).centre.y < reverb.rect.top() + 1.0);
        assert_eq!(toggle(13).centre, sum.at);
        // Drawn as wide as measured.
        assert!(laid.width <= 1048.0 && !laid.overflow);
    }

    /// Two banks that meet share the dot where one is summed and the next
    /// split.
    #[test]
    fn two_banks_that_meet_share_one_dot() {
        let (g, slot) = geometry(Tier::M);
        let banks = row(serde_json::json!([[
            "root\\app\\gate",
            "p",
            "root\\app\\comp",
            "s",
            "root\\app\\delay",
            "p",
            "root\\app\\reverb"
        ]]));
        let holds = vec![Holds::Block { on: true }; 4];
        let laid = lay_out(&banks, &holds, false, (0.0, 0.0, 1048.0), &g, slot);
        let [first, shared, last] = laid.junctions.as_slice() else {
            panic!("three dots: {:?}", laid.junctions)
        };
        assert_eq!(first.splits, Some(0..2));
        assert_eq!(
            (shared.sums.clone(), shared.splits.clone()),
            (Some(0..2), Some(2..4))
        );
        assert_eq!(last.sums, Some(2..4));
        assert!(laid.spots[0].rect.right() < shared.at.x);
        assert!(shared.at.x < laid.spots[2].rect.left());
        assert_eq!(
            laid.toggles[1].centre, shared.at,
            "the connector between them"
        );
    }

    /// On 2.0.10 an empty branch passes nothing, and its wires are drawn so,
    /// unless no branch passes anything, when the dry signal does; on 2.2.6
    /// it passes the dry signal.
    #[test]
    fn an_empty_branch_is_silent_only_where_it_passes_nothing() {
        let (g, slot) = geometry(Tier::M);
        let bank = row(serde_json::json!([["root\\app\\delay", "p", ""]]));
        let silent = |holds: &[Holds], dry: bool| {
            lay_out(&bank, holds, dry, (0.0, 0.0, 600.0), &g, slot)
                .wires
                .iter()
                .filter(|(_, carries)| !carries)
                .count()
        };
        let on = [Holds::Block { on: true }, Holds::Nothing];
        assert!(silent(&on, false) > 0, "2.0.10");
        assert_eq!(silent(&on, true), 0, "2.2.6");
        let off = [Holds::Block { on: false }, Holds::Nothing];
        assert_eq!(
            silent(&off, false),
            0,
            "nothing contributes: the dry signal passes"
        );
        // The empty branch is a slot, centred in its tile-wide lane.
        let laid = lay_out(&bank, &on, false, (0.0, 0.0, 600.0), &g, slot);
        assert!(laid.spots[1].slot);
        assert!((laid.spots[1].rect.center().x - laid.spots[0].rect.center().x).abs() < 0.01);
    }

    /// A bank with nothing in it is as narrow as one empty position.
    #[test]
    fn a_bank_of_empty_positions_is_as_narrow_as_one() {
        let (g, slot) = geometry(Tier::S);
        let bank = row(serde_json::json!([["", "p", "", "s", "root\\app\\amp"]]));
        let holds = [Holds::Nothing, Holds::Nothing, Holds::Block { on: true }];
        let laid = lay_out(&bank, &holds, true, (0.0, 0.0, 800.0), &g, slot);
        assert_eq!(laid.spots[0].rect.width(), slot);
        assert_eq!(laid.spots[0].rect.left(), laid.spots[1].rect.left());
        assert_eq!(laid.spots[1].lane, 1);
        assert!(!laid.spots[2].slot);
    }

    #[test]
    fn the_board_grows_with_its_deepest_bank() {
        let (g, _) = geometry(Tier::M);
        assert_eq!(board_height(&g, 1), g.top + g.tile_height + g.bottom);
        assert_eq!(
            board_height(&g, 3),
            g.top + 3.0 * g.tile_height + 2.0 * g.lane_gap + g.bottom
        );
        assert_eq!(board_height(&g, 0), board_height(&g, 1));
    }

    /// The blocks a chain can hold are the schema's blocks whose first
    /// control is their on/off, drawn in the category their style names.
    #[test]
    fn the_chain_holds_blocks_with_an_on_off_first() {
        let mut snapshot = super::super::demo::snapshot_2x(&DemoChain::Default);
        // A block node that is never connected, as 2.2.6's pcm42 is: its
        // first child is not an on/off.
        let node = |path: &str, value: serde_json::Value| {
            (
                NodePath::new(path).unwrap(),
                serde_json::from_value::<NodeDescription>(value).unwrap(),
            )
        };
        snapshot.app.push(node(
            "root\\app\\pcm42",
            serde_json::json!({"type": "item", "desc": "PCM42", "value": "", "item_type": "block", "style": "cat:Delay;"}),
        ));
        snapshot.app.push(node(
            "root\\app\\pcm42\\time",
            serde_json::json!({"type": "float", "desc": "Time", "value": 1.0, "min": 0.0, "max": 2.0}),
        ));
        snapshot.app.push(node(
            "root\\app\\pcm42\\on_off",
            serde_json::json!({"type": "enum", "desc": "Enable", "value": "ON", "options": ["ON", "OFF"]}),
        ));
        let chain = Chain::of(&snapshot).expect("2.0.10 has a router");
        let groups: Vec<&str> = chain
            .blocks
            .iter()
            .map(|block| block.group.as_str())
            .collect();
        assert_eq!(groups.len(), 19);
        assert!(!groups.contains(&"pcm42"));
        assert!(!groups.contains(&"output"), "the master is not a block");
        assert_eq!(chain.find("chorus").unwrap().category, "Modulation");
        assert_eq!(chain.find("gate").unwrap().category, "Dynamics");
        assert_eq!(chain.find("exp").unwrap().category, "Wah");
        assert_eq!(chain.find("crossover").unwrap().category, "Filter");
        assert_eq!(
            chain.find("pickup").unwrap().category,
            "",
            "Misc has no colour"
        );
        assert_eq!(block_category(&snapshot, "chorus"), "Modulation");
        assert_eq!(block_category(&snapshot, "amp"), "Amp");
        assert!(!chain.dry_branches, "2.0.10");
        // In the chain, in order; and what is left for the picker.
        assert_eq!(chain.groups().len(), 12);
        assert_eq!(chain.groups()[0], "gate");
        assert_eq!(chain.position_of("delay"), Some(11));
        assert_eq!(chain.position_of("chorus"), None);
        let available: Vec<&str> = chain
            .available()
            .iter()
            .map(|block| block.group.as_str())
            .collect();
        assert_eq!(
            available,
            [
                "flanger",
                "chorus",
                "eq2",
                "crossover",
                "delay2",
                "reverb2",
                "pickup"
            ]
        );
        assert_eq!(chain.label(9), "position 10");
        assert_eq!(chain.label(10), "Mod");
    }

    /// 1.5.12 has no router, and keeps its fixed board.
    #[test]
    fn a_pedal_without_a_router_has_no_chain_to_edit() {
        let mut panel = Panel::new(egui::Context::default());
        panel.show_demo();
        assert!(Chain::of(panel.snapshot.as_ref().unwrap()).is_none());
    }

    #[test]
    fn an_empty_branch_passes_the_dry_signal_after_2_0() {
        assert!(!dry_branches("2.0.10"));
        assert!(dry_branches("2.2.6"));
        assert!(dry_branches("2.3.0"));
    }

    /// A block chosen in the picker goes into an empty position, or takes
    /// the place of what a position holds, a string naming no block too.
    #[test]
    fn a_chosen_block_is_placed_or_replaces() {
        let snapshot = super::super::demo::snapshot_2x(&DemoChain::Default);
        let chain = Chain::of(&snapshot).unwrap();
        let chorus = chain.find("chorus").unwrap();
        let placed = chain.put(9, chorus).unwrap();
        assert_eq!(placed.block(9), Some("root\\app\\chorus"));
        let replaced = chain.put(10, chorus).unwrap();
        assert_eq!(replaced.block(10), Some("root\\app\\chorus"));
        assert!(!replaced.contains("root\\app\\mod"));
        assert_eq!(chain.put(6, chorus), Err(EditError::FixedPosition(6)));

        // Position 10 holding a path the pedal stores but has no block for.
        let mut written = chain.router().to_value();
        written[0][18] = serde_json::json!("root\\app\\pcm42");
        let mut odd = chain.clone();
        odd.node.value = chain.router().apply(chain.fixed(), &written);
        assert_eq!(odd.stray(9), Some("root\\app\\pcm42"));
        assert!(odd.held(9).is_none());
        assert_eq!(
            odd.put(9, chorus).unwrap().block(9),
            Some("root\\app\\chorus")
        );
    }

    /// A panel on the demo's 2.0.10 pedal, cut off from its worker: what it
    /// sends, and a way to answer as the worker would.
    fn panel(chain: DemoChain) -> (Panel, mpsc::Receiver<Cmd>, mpsc::Sender<Evt>) {
        let mut panel = Panel::new(egui::Context::default());
        panel.show_demo_router(chain);
        let (tx, commands) = mpsc::channel();
        panel.tx = tx;
        let (events, rx) = mpsc::channel();
        panel.rx = rx;
        (panel, commands, events)
    }

    fn chain_of(panel: &Panel) -> Chain {
        Chain::of(panel.snapshot.as_ref().unwrap()).unwrap()
    }

    /// An edit is one write, computed from the chain the pedal last
    /// reported. The board keeps that chain, and takes no other edit, until
    /// the pedal says what it kept; then it shows that and selects the
    /// block the edit put in.
    #[test]
    fn an_edit_is_sent_once_and_the_board_shows_what_the_pedal_kept() {
        let (mut panel, commands, events) = panel(DemoChain::Default);
        let chain = chain_of(&panel);
        let chorus = chain.find("chorus").unwrap().clone();
        panel.route(&chain, chain.put(9, &chorus), Some("chorus".into()));

        let sent: Vec<Cmd> = commands.try_iter().collect();
        let [Cmd::SetRouter {
            before,
            chain: next,
            description,
        }] = sent.as_slice()
        else {
            panic!("one chain write")
        };
        assert_eq!(before, chain.router());
        assert_eq!(next.block(9), Some("root\\app\\chorus"));
        assert_eq!(description.kind, Some(NodeKind::Router));
        assert!(panel.routing);
        assert!(!panel.can_route());
        assert_eq!(chain_of(&panel).router(), chain.router(), "not yet");

        events.send(Evt::Routed(Some(next.to_value()))).unwrap();
        panel.drain();
        assert!(!panel.routing);
        assert!(panel.can_route());
        assert_eq!(chain_of(&panel).router(), next);
        assert_eq!(panel.selected_group, "chorus");
    }

    /// A write that failed leaves the chain as the pedal last reported it,
    /// and lets the board be edited again.
    #[test]
    fn a_failed_write_changes_nothing_and_frees_the_board() {
        let (mut panel, commands, events) = panel(DemoChain::Default);
        let chain = chain_of(&panel);
        panel.route(&chain, chain.router().remove(chain.fixed(), 10), None);
        assert_eq!(commands.try_iter().count(), 1);
        events.send(Evt::Routed(None)).unwrap();
        panel.drain();
        assert!(!panel.routing);
        assert_eq!(chain_of(&panel).router(), chain.router());
        assert_eq!(panel.selected_group, "amp");
    }

    /// An edit that changes nothing, or that cannot be made, is not sent.
    #[test]
    fn an_edit_that_changes_nothing_is_not_sent() {
        let (mut panel, commands, _events) = panel(DemoChain::Default);
        let chain = chain_of(&panel);
        panel.route(&chain, Ok(chain.router().clone()), None);
        panel.route(&chain, chain.router().remove(chain.fixed(), 9), None);
        assert_eq!(commands.try_iter().count(), 0);
        assert!(!panel.routing);
        assert!(panel.failed, "the empty position says why");
    }

    /// A changed chain is an unsaved edit, until it is put back the way the
    /// preset has it.
    #[test]
    fn a_changed_chain_is_unsaved_until_it_is_put_back() {
        let (mut panel, _commands, events) = panel(DemoChain::Default);
        panel.saved_drafts = panel
            .drafts
            .iter()
            .filter(|(path, _)| path.starts_with("root\\app\\"))
            .map(|(path, value)| (path.clone(), value.clone()))
            .collect();
        panel.recompute_dirty();
        assert!(!panel.dirty);
        let chain = chain_of(&panel);
        let parallel = chain
            .router()
            .connect(chain.fixed(), 12, Link::Parallel)
            .unwrap();
        events.send(Evt::Routed(Some(parallel.to_value()))).unwrap();
        panel.drain();
        assert!(panel.dirty);
        // Undo answers with the value it wrote back.
        events
            .send(Evt::NodeValue {
                path: PATH.to_owned(),
                value: chain.router().to_value(),
            })
            .unwrap();
        panel.drain();
        assert!(!panel.dirty);
    }

    /// When the selected block leaves the chain, the one nearest where it
    /// was is selected.
    #[test]
    fn the_selection_moves_to_the_nearest_block_left() {
        let (mut panel, _commands, events) = panel(DemoChain::Default);
        panel.selected_group = "eq".into();
        let chain = chain_of(&panel);
        let without = chain.router().remove(chain.fixed(), 8).unwrap();
        events.send(Evt::Routed(Some(without.to_value()))).unwrap();
        panel.drain();
        assert_eq!(
            panel.selected_group, "mod",
            "position 10 holds the next block"
        );
    }

    /// Another preset, or the same one reloaded, closes the picker: the
    /// position it was open on belonged to the chain that was there.
    #[test]
    fn a_preset_loading_closes_the_picker() {
        let (mut panel, _commands, _events) = panel(DemoChain::Picking);
        assert!(panel.picking.is_some());
        let snapshot = panel.snapshot.clone().unwrap();
        panel.install_snapshot(snapshot.clone(), false);
        assert!(panel.picking.is_some(), "a refresh keeps it");
        panel.install_snapshot(snapshot, true);
        assert!(panel.picking.is_none());
    }

    /// A chain write the worker cannot make still answers, so the board does
    /// not wait for ever, and adds nothing to undo.
    #[test]
    fn a_chain_write_without_a_pedal_frees_the_board() {
        let (_commands, receiver) = mpsc::channel();
        let (events, received) = mpsc::channel();
        let mut worker = Worker::new(receiver, events, egui::Context::default());
        let snapshot = super::super::demo::snapshot_2x(&DemoChain::Default);
        let chain = Chain::of(&snapshot).unwrap();
        let result = worker.handle(Cmd::SetRouter {
            description: Box::new(chain.description.clone()),
            before: chain.router().clone(),
            chain: chain
                .router()
                .connect(chain.fixed(), 12, Link::Parallel)
                .unwrap(),
        });
        assert!(result.is_err());
        assert!(received
            .try_iter()
            .any(|event| matches!(event, Evt::Routed(None))));
        assert!(worker.history.is_empty());
    }

    /// A library preset without a router record previews the default chain,
    /// which is what the pedal loads for it; one with a record, its own.
    #[test]
    fn a_preview_shows_the_chain_the_preset_would_load() {
        let (mut panel, _commands, _events) = panel(DemoChain::Parallel);
        panel
            .preview("Old".into(), b"root\\app\\amp\\gain:{\"value\":5.0}\r\n")
            .unwrap();
        let (_, preview) = panel.preview.clone().unwrap();
        assert_eq!(
            Chain::of(&preview).unwrap().router(),
            &row(DemoChain::Default.value())
        );
        let record = format!(
            "root\\app\\router:{{\"value\":{}}}\r\n",
            DemoChain::ThreeWay.value()
        );
        panel.preview("New".into(), record.as_bytes()).unwrap();
        let (_, preview) = panel.preview.clone().unwrap();
        assert_eq!(
            Chain::of(&preview).unwrap().router(),
            &row(DemoChain::ThreeWay.value())
        );
        assert_eq!(
            chain_of(&panel).router(),
            &row(DemoChain::Parallel.value()),
            "the pedal's own chain is untouched"
        );
    }

    /// The one-line deck draws the chain the router holds: no empty
    /// positions, a bank stacked, no block the chain does not hold.
    #[test]
    fn the_one_line_deck_draws_the_chain_the_router_holds() {
        let (panel, _commands, _events) = panel(DemoChain::ThreeWay);
        let minis = panel.mini_chain();
        assert_eq!(
            minis.len(),
            8,
            "gate, comp, drive, amp, ir, the bank, delay, reverb"
        );
        let crate::shell::Mini::Stack(lanes) = &minis[5] else {
            panic!("the bank is stacked")
        };
        assert_eq!(lanes.len(), 3);
        assert_eq!(
            lanes.iter().map(|(_, on)| *on).collect::<Vec<_>>(),
            [true, false, true],
            "the flanger is off"
        );
    }

    #[test]
    fn positions_and_banks_read_as_sentences() {
        let snapshot = super::super::demo::snapshot_2x(&DemoChain::ThreeWay);
        let chain = Chain::of(&snapshot).unwrap();
        assert_eq!(names(&chain, 8..11), "Chorus, Flanger and Mod");
        assert_eq!(names(&chain, 12..14), "Reverb and position 14");
        assert_eq!(
            link_words(&chain, 10, Link::Parallel),
            "Run Mod after Chorus and Flanger, in series"
        );
        assert_eq!(
            link_words(&chain, 3, Link::Series),
            "Run position 4 in parallel with Drive"
        );
        assert!(slot_words(&chain, 3, None).starts_with("Position 4 is empty"));
    }
}
