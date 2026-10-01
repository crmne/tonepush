//! The floor (docs/design/redesign-2026-10-01, "Floor"): what the pedal has
//! under your feet, along the bottom of the Edit page. Each footswitch with
//! its LED ring and what it carries, each expression pedal, and how many
//! controls MIDI reaches. A chip opens its source in the Footswitches lens.

use egui::{CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use hx_proto::rpc::Source;

use crate::pane::{paint_source, source_tag, SourceLook};
use crate::shell;
use crate::theme::{self, Icon, Mood, Tier};
use crate::App;

/// The floor's height.
pub(crate) const FLOOR_HEIGHT: f32 = 64.0;

/// One chip on the floor, gathered before drawing.
struct FloorChip {
    source: Source,
    look: SourceLook,
    /// "FS1 · Toggles", "EXP 1 · auto on".
    first: String,
    /// What it carries: "Minotaur", "Lead · 2 controls".
    second: String,
    empty: bool,
}

/// How wide a chip is at its natural size.
fn chip_width(ui: &Ui, chip: &FloorChip) -> f32 {
    let first = shell::galley(
        ui,
        chip.first.clone(),
        theme::semibold(10.5),
        theme::muted(),
    );
    let second = shell::galley(
        ui,
        chip.second.clone(),
        theme::semibold(12.5),
        theme::text(),
    );
    10.0 + 26.0 + 10.0 + first.size().x.max(second.size().x) + 12.0
}

impl App {
    /// The chips the floor shows at this size: every footswitch and pedal,
    /// and on a small window only those that carry something.
    fn floor_chips(&self, tier: Tier) -> Vec<FloorChip> {
        let compact = tier == Tier::S;
        self.listed_sources()
            .into_iter()
            .filter(|source| *source != Source::MidiCc)
            .filter_map(|source| {
                let carried = self.carried_by(source);
                let empty = carried.is_empty();
                if empty && compact && !matches!(source, Source::Footswitch(_)) {
                    return None;
                }
                let first = match (source, self.source_mode(source, &carried)) {
                    (Source::Expression(n), Some(_)) => format!("EXP {n} · auto on"),
                    (_, Some(mode)) => format!("{} · {mode}", source_tag(source)),
                    (_, None) => source_tag(source),
                };
                let blocks: std::collections::BTreeSet<i64> =
                    carried.iter().map(|a| a.block).collect();
                let second = if empty {
                    "Nothing assigned".to_owned()
                } else {
                    let name = self.source_name(source, &carried);
                    match (source, carried.len()) {
                        // A pedal says what it sweeps; its auto-engage is
                        // already in the first line.
                        (Source::Expression(_), _) if !compact => {
                            let swept: Vec<_> = carried
                                .iter()
                                .filter(|a| !Self::auto_engage(a.source, a.target))
                                .collect();
                            match swept.as_slice() {
                                [one] => {
                                    format!("{name} {}", self.target_name(one.block, one.target))
                                }
                                [] => name,
                                more => format!("{name} · {} controls", more.len()),
                            }
                        }
                        (_, n) if n > 1 && !compact => {
                            if blocks.len() > 1 || self.is_labelled(source) {
                                format!("{name} · {n} controls")
                            } else {
                                name
                            }
                        }
                        _ => name,
                    }
                };
                Some(FloorChip {
                    source,
                    look: self.source_look(source, &carried),
                    first,
                    second,
                    empty,
                })
            })
            .collect()
    }

    /// Whether a footswitch has a name typed for it.
    fn is_labelled(&self, source: Source) -> bool {
        let Source::Footswitch(n) = source else {
            return false;
        };
        self.switches
            .iter()
            .find(|s| s.switch == n)
            .and_then(|s| s.label.as_ref())
            .is_some_and(|label| !label.trim().is_empty())
    }

    /// The floor along the bottom of the Edit page.
    pub(crate) fn floor(&mut self, root: &mut Ui, tier: Tier) {
        let chips = self.floor_chips(tier);
        // How many CC numbers reach the preset: one CC driving two things is
        // still one CC on the controller.
        let midi = self.carried_by(Source::MidiCc);
        let numbers: std::collections::BTreeSet<i64> = midi.iter().map(|a| self.cc_of(a)).collect();
        let mut open = None;
        egui::Panel::bottom("floor")
            .exact_size(FLOOR_HEIGHT)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.top() + 0.5,
                    Stroke::new(1.0, theme::line()),
                );
                let y = rect.center().y;
                let mut right = rect.right() - 16.0;
                if !midi.is_empty() && tier != Tier::S {
                    let words = format!("MIDI · {} CC", numbers.len());
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(Rect::from_min_max(
                                Pos2::new(rect.left(), rect.top()),
                                Pos2::new(right, rect.bottom()),
                            ))
                            .id_salt("floor-midi")
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    if theme::Chip::new(&words)
                        .mood(Mood::Ghost)
                        .icon(Icon::Cable)
                        .height(22.0)
                        .show(&mut child)
                        .on_hover_text("Edit what MIDI controls")
                        .clicked()
                    {
                        open = Some(Source::MidiCc);
                    }
                    right = child.min_rect().left() - 12.0;
                }
                let left = rect.left() + 20.0;
                let gap = 8.0;
                let room = (right - left - gap * chips.len().saturating_sub(1) as f32).max(0.0);
                let natural: Vec<f32> = chips.iter().map(|chip| chip_width(ui, chip)).collect();
                let total: f32 = natural.iter().sum();
                // Too wide: every chip gives up the same share of its text.
                let scale = if total > room && total > 0.0 {
                    room / total
                } else {
                    1.0
                };
                let mut x = left;
                for (chip, width) in chips.iter().zip(natural) {
                    let width = (width * scale).max(46.0);
                    let place = Rect::from_min_size(Pos2::new(x, y - 22.0), Vec2::new(width, 44.0));
                    if floor_chip(ui, place, chip).clicked() {
                        open = Some(chip.source);
                    }
                    x += width + gap;
                }
            });
        if let Some(source) = open {
            self.focus_source(source);
        }
    }
}

/// One chip on the floor.
fn floor_chip(ui: &mut Ui, rect: Rect, chip: &FloorChip) -> egui::Response {
    let response = ui
        .interact(
            rect,
            ui.id().with(("floor", chip.source.ordinal())),
            Sense::click(),
        )
        .on_hover_text(format!("Edit {}", source_tag(chip.source)));
    if chip.empty {
        theme::paint::dashed_rect(
            ui.painter(),
            rect.shrink(0.5),
            10.5,
            Stroke::new(1.0, theme::line_strong()),
            4.0,
            3.0,
        );
    } else {
        ui.painter().rect(
            rect,
            CornerRadius::same(11),
            if response.hovered() {
                theme::raised()
            } else {
                theme::panel()
            },
            Stroke::new(1.0, theme::line()),
            egui::StrokeKind::Inside,
        );
    }
    let switch = Rect::from_min_size(
        Pos2::new(rect.left() + 10.0, rect.center().y - 13.0),
        Vec2::splat(26.0),
    );
    paint_source(ui, chip.look, switch, 5.0);
    let left = switch.right() + 10.0;
    let room = (rect.right() - 12.0 - left).max(8.0);
    let first = shell::elided(
        ui,
        chip.first.clone(),
        theme::semibold(10.5),
        theme::muted(),
        room,
    );
    shell::paint_line(ui, first, left, rect.center().y - 7.0);
    let second = shell::elided(
        ui,
        chip.second.clone(),
        if chip.empty {
            theme::medium(12.5)
        } else {
            theme::semibold(12.5)
        },
        if chip.empty {
            theme::faint()
        } else {
            theme::text()
        },
        room,
    );
    shell::paint_line(ui, second, left, rect.center().y + 8.0);
    response
}
