//! The block picker (docs/design/redesign-2026-10-01, screen 02, as the
//! StompStation PRO's 2.x chain uses it): a lens of the pane, opened by an
//! empty position's "+" or by Replace on a block. It is the model browser's
//! head, rail and cards, listing the blocks the chain can hold and does not,
//! by category. A click puts the block in the position, in one write.

use egui::{CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::routing::{self, Chain, Placeable};
use super::*;
use crate::browser::{self, Face, RailRow};
use crate::shell;
use crate::theme::{Icon, Tier};

/// The categories of these blocks, each once, in the order the pedal lists
/// its blocks.
fn categories(blocks: &[&Placeable]) -> Vec<&'static str> {
    let mut categories = Vec::new();
    for block in blocks {
        if !categories.contains(&block.category) {
            categories.push(block.category);
        }
    }
    categories
}

/// What a category is called on the rail.
fn category_name(category: &str) -> &str {
    if category.is_empty() {
        "Other"
    } else {
        category
    }
}

/// A block's face on its card: its knobs and switches where the pedal has
/// them now, and the names of its controls.
fn block_face(snapshot: &Snapshot, group: &str) -> Face {
    let mut face = Face::default();
    let mut names = Vec::new();
    for (path, description) in &snapshot.app {
        if app_group(path.as_str()).as_deref() != Some(group) {
            continue;
        }
        let Some(value) = description.value.as_ref() else {
            continue;
        };
        let leaf = path.as_str().rsplit('\\').next().unwrap_or_default();
        if !editable_node(path.as_str(), description, value)
            || matches!(leaf, "on_off" | "enable" | "on")
        {
            continue;
        }
        match description.kind {
            Some(NodeKind::Float) => {
                let (min, max) = (
                    description.min.unwrap_or(0.0),
                    description.max.unwrap_or(1.0),
                );
                let number = value.as_f64().unwrap_or(min);
                let fraction = if max > min {
                    ((number - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                face.knobs.push(fraction as f32);
            }
            Some(NodeKind::Enum | NodeKind::Array) => {
                if let Some((_, on)) = toggle_choices(description) {
                    face.switches.push(*value == on);
                }
            }
            _ => {}
        }
        names.push(node_label(path, description));
    }
    face.names = names.join(" · ");
    face
}

/// What the picker was asked this frame.
enum Asked {
    Close,
    Category(Option<&'static str>),
    Pick(String),
}

impl Panel {
    /// The picker in the pane: its head, the rail of categories, and a card
    /// for every block the chain can hold and does not.
    pub(super) fn picker(&mut self, ui: &mut Ui, snapshot: &Snapshot, chain: &Chain, tier: Tier) {
        let Some(picking) = self.picking.clone() else {
            return;
        };
        if picking.position >= chain.positions() || chain.fixed().position(picking.position) {
            self.picking = None;
            return;
        }
        let full = ui.max_rect();
        ui.allocate_rect(full, Sense::hover());
        let head = Rect::from_min_size(full.min, Vec2::new(full.width(), 60.0));
        let body = Rect::from_min_max(Pos2::new(full.left(), head.bottom()), full.max);
        let position = picking.position;
        let replacing = picking
            .replacing
            .as_deref()
            .and_then(|group| chain.find(group));
        let available = chain.available();
        let mut asked = None;

        // The head: the well, what is being chosen, and Close.
        let (title, subtitle, colour, drawing) = match replacing {
            Some(block) => {
                let name = super::board::tile_name(&block.group);
                (
                    format!("Replace {name}"),
                    format!(
                        "Click a block to put it in position {}. {name} leaves the chain",
                        position + 1
                    ),
                    theme::category_colour(block.category),
                    routing::block_drawing(block.category),
                )
            }
            None => {
                let bank = chain.router().bank(position);
                let with: Vec<String> = bank
                    .filter(|other| *other != position)
                    .map(|other| chain.label(other))
                    .collect();
                let place = match with.as_slice() {
                    [] => format!("Position {}", position + 1),
                    [one] => format!("Position {}, in parallel with {one}", position + 1),
                    [rest @ .., last] => format!(
                        "Position {}, in parallel with {} and {last}",
                        position + 1,
                        rest.join(", ")
                    ),
                };
                (
                    "Add a block".to_owned(),
                    format!("{place} · click a block to put it there"),
                    theme::accent(),
                    None,
                )
            }
        };
        let mut head_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(head.left() + 20.0, head.top()),
                    Pos2::new(head.right() - 16.0, head.bottom()),
                ))
                .id_salt("pro-picker-head")
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        head_ui.spacing_mut().item_spacing.x = 12.0;
        let (well, _) = head_ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        theme::paint::gradient_rect(
            head_ui.painter(),
            well,
            11.0,
            &[
                (0.0, theme::alpha(colour, 0.24)),
                (1.0, theme::alpha(colour, 0.08)),
            ],
        );
        head_ui.painter().rect_stroke(
            well,
            CornerRadius::same(11),
            Stroke::new(1.0, theme::alpha(colour, 0.4)),
            egui::StrokeKind::Inside,
        );
        match drawing {
            Some(art) => art.paint(
                &head_ui,
                Rect::from_center_size(well.center(), Vec2::splat(22.0)),
                colour,
            ),
            None => theme::paint_icon(&head_ui, Icon::Plus, well.center(), 20.0, colour),
        }
        let words_width = (head.width() - 20.0 - 16.0 - 40.0 - 12.0 - 48.0).max(80.0);
        let title_galley = shell::elided(
            &head_ui,
            title,
            theme::semibold(theme::TITLE),
            theme::text(),
            words_width,
        );
        let subtitle_galley = shell::elided(
            &head_ui,
            subtitle,
            theme::regular(12.0),
            theme::muted(),
            words_width,
        );
        let width = title_galley.size().x.max(subtitle_galley.size().x);
        let (words, _) = head_ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
        shell::paint_line(&head_ui, title_galley, words.left(), words.top() + 9.5);
        shell::paint_line(&head_ui, subtitle_galley, words.left(), words.top() + 27.0);
        let mut right_ui = head_ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(head_ui.cursor().left(), head.top()),
                    Pos2::new(head.right() - 16.0, head.bottom()),
                ))
                .id_salt("pro-picker-head-right")
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if theme::IconButton::new(Icon::Close)
            .show(&mut right_ui)
            .on_hover_text("Close (Esc)")
            .clicked()
        {
            asked = Some(Asked::Close);
        }
        ui.painter().hline(
            head.x_range(),
            head.bottom() - 0.5,
            Stroke::new(1.0, theme::line()),
        );

        // The rail: every block, then each category, with how many each
        // offers.
        let rail_width = if tier == Tier::S { 168.0 } else { 188.0 };
        let rail = Rect::from_min_max(body.min, Pos2::new(body.left() + rail_width, body.bottom()));
        ui.painter().vline(
            rail.right() - 0.5,
            rail.y_range(),
            Stroke::new(1.0, theme::line()),
        );
        let categories = categories(&available);
        let showing = picking
            .category
            .filter(|category| categories.contains(category));
        let mut rail_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rail.shrink2(Vec2::new(8.0, 8.0)))
                .id_salt("pro-picker-rail")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        rail_ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
        egui::ScrollArea::vertical()
            .id_salt("pro-picker-rail-scroll")
            .auto_shrink([false, false])
            .show(&mut rail_ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if browser::rail_row(
                    ui,
                    RailRow::Icon(Icon::LayoutGrid),
                    "Every block",
                    available.len(),
                    showing.is_none(),
                )
                .clicked()
                {
                    asked = Some(Asked::Category(None));
                }
                let (separator, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 13.0), Sense::hover());
                ui.painter().hline(
                    (separator.left() + 8.0)..=(separator.right() - 8.0),
                    separator.center().y,
                    Stroke::new(1.0, theme::line()),
                );
                for category in &categories {
                    let count = available
                        .iter()
                        .filter(|block| block.category == *category)
                        .count();
                    let row = if category.is_empty() {
                        RailRow::Icon(Icon::AudioWaveform)
                    } else {
                        RailRow::Category(category)
                    };
                    if browser::rail_row(
                        ui,
                        row,
                        category_name(category),
                        count,
                        showing == Some(*category),
                    )
                    .clicked()
                    {
                        asked = Some(Asked::Category(Some(category)));
                    }
                }
            });

        // The blocks, as cards.
        let shown: Vec<&Placeable> = available
            .iter()
            .copied()
            .filter(|block| showing.is_none_or(|category| block.category == category))
            .collect();
        let cards = Rect::from_min_max(Pos2::new(rail.right(), body.top()), body.max);
        let mut cards_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(cards)
                .id_salt("pro-picker-cards")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        cards_ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
        let ready = self.can_route();
        egui::ScrollArea::vertical()
            .id_salt(("pro-picker-blocks", showing))
            .auto_shrink([false, false])
            .show(&mut cards_ui, |ui| {
                let inner = ui.max_rect();
                if shown.is_empty() {
                    let words = shell::galley(
                        ui,
                        "Every block this pedal has is in the chain",
                        theme::regular(theme::BODY),
                        theme::muted(),
                    );
                    let width = words.size().x;
                    ui.allocate_exact_size(Vec2::new(inner.width(), 120.0), Sense::hover());
                    shell::paint_line(
                        ui,
                        words,
                        inner.center().x - width / 2.0,
                        inner.top() + 60.0,
                    );
                    return;
                }
                let gap = 12.0;
                let width = cards.width() - 32.0;
                let columns = (((width + gap) / (156.0 + gap)).floor() as usize).max(3);
                let card = ((width - gap * (columns as f32 - 1.0)) / columns as f32).floor();
                ui.add_space(14.0);
                for row in shown.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.spacing_mut().item_spacing.x = gap;
                        for block in row {
                            let name = super::board::tile_name(&block.group);
                            let face = block_face(snapshot, &block.group);
                            let colour = theme::category_colour(block.category);
                            let response =
                                browser::model_card(ui, &name, &face, colour, card, None, None)
                                    .on_hover_text(format!(
                                        "Put {name} in position {}",
                                        position + 1
                                    ));
                            if ready && response.clicked() {
                                asked = Some(Asked::Pick(block.group.clone()));
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            });

        // Escape closes it, unless a field has the keyboard.
        if asked.is_none()
            && ui.memory(|memory| memory.focused().is_none())
            && ui.input(|input| input.key_pressed(egui::Key::Escape))
        {
            asked = Some(Asked::Close);
        }
        match asked {
            Some(Asked::Close) => self.picking = None,
            Some(Asked::Category(category)) => {
                if let Some(picking) = self.picking.as_mut() {
                    picking.category = category;
                }
            }
            Some(Asked::Pick(group)) => {
                let Some(block) = chain.find(&group) else {
                    return;
                };
                let next = chain.put(position, block);
                self.picking = None;
                self.route(chain, next, Some(group));
            }
            None => {}
        }
    }
}
