//! The model browser (docs/design/redesign-2026-10-01, screen 02): a lens of
//! the pane, opened by Change model, a block's name or a gap's "+".
//!
//! Swapping is an audition. The edit buffer is live, so a click plays the
//! model on the pedal at once; the pedal's worker keeps the preset as it was
//! before the first try, so "Put back" restores it exactly and "Keep" makes
//! every try one undo step. Anything else done meanwhile keeps what is
//! playing, as the shelf this replaces always did. Adding a block is one
//! click: the model goes into the gap and the browser closes.

use std::collections::HashMap;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use hx_catalog::{Catalog, Kind, Model};

use crate::shell;
use crate::theme::{self, Icon, Mood, Tier};
use crate::{App, Cmd};

/// How many models Recent remembers.
const RECENT: usize = 12;

/// What the browser is choosing a model for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Target {
    /// Trying models in a block's place.
    Swap {
        position: i64,
        /// What the block held when the browser opened: its name and id, and
        /// whether a cab rode along.
        original: String,
        original_id: Option<String>,
        paired: bool,
    },
    /// Choosing a block for a gap: the slot it goes into.
    Insert { at: usize },
}

/// A place in the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rail {
    Recent,
    Category(u32),
}

/// A model's face on a card: its knobs at their defaults, its switches, and
/// the names of its controls.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Face {
    pub knobs: Vec<f32>,
    pub switches: Vec<bool>,
    pub names: String,
}

/// The open browser.
pub(crate) struct Browser {
    pub(crate) target: Target,
    search: String,
    rail: Rail,
    /// The shelf chosen within the category, by name.
    shelf: Option<String>,
    /// Rows rather than cards.
    list: bool,
    /// The model playing in the block's place after a try: its id and name.
    pub(crate) playing: Option<(String, String)>,
    /// Ask for the keyboard on the next frame.
    focus: bool,
    /// Every model that can be chosen, for search: id and lowercased name,
    /// in HX Edit's order.
    index: Vec<(String, String)>,
    faces: HashMap<String, Face>,
}

/// Which shelf of a category opens: the one chosen if it is still one of
/// this category's, else the one holding `holding`, else the first.
pub(crate) fn open_shelf(
    shelves: &[(&str, Vec<&Model>)],
    chosen: Option<&str>,
    holding: Option<&str>,
) -> usize {
    chosen
        .and_then(|want| shelves.iter().position(|(name, _)| *name == want))
        .or_else(|| {
            let held = holding?;
            shelves
                .iter()
                .position(|(_, models)| models.iter().any(|m| m.id == held))
        })
        .unwrap_or(0)
}

/// How a card's knobs are laid out: up to six in one row, or two rows of
/// three, with five shown and a count when there are more than six.
pub(crate) fn card_rows(knobs: usize) -> (usize, Vec<usize>) {
    let shown = knobs.min(if knobs > 6 { 5 } else { 6 });
    let rows = if shown > 3 && shown != 4 {
        let first = shown.div_ceil(2);
        vec![first, shown - first]
    } else if shown == 0 {
        Vec::new()
    } else {
        vec![shown]
    };
    (shown, rows)
}

/// A model's face, from its controls in the order the pedal indexes them.
pub(crate) fn face_of(catalog: &Catalog, model: &Model) -> Face {
    let params = catalog.ordered_params(model);
    let mut face = Face::default();
    let fraction = |param: &hx_catalog::Param| {
        let span = param.max - param.min;
        if span.abs() > f32::EPSILON {
            ((param.default - param.min) / span).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    for param in &params {
        match param.kind {
            Kind::Switch => face.switches.push(param.default >= 0.5),
            Kind::Enum if catalog.choices(param).is_some() => {}
            Kind::Text => {}
            _ => face.knobs.push(fraction(param)),
        }
    }
    face.names = params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    face
}

/// Paint a card's face: its knobs at their defaults on the category's wash.
fn paint_face(ui: &Ui, rect: Rect, face: &Face, colour: Color32) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), theme::bg_deep());
    theme::paint::gradient_rect(
        painter,
        rect,
        8.0,
        &[
            (0.0, theme::alpha(colour, 0.20)),
            (1.0, theme::alpha(colour, 0.05)),
        ],
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0, theme::mix(theme::line(), colour, 0.3)),
        egui::StrokeKind::Inside,
    );
    let (shown, rows) = card_rows(face.knobs.len());
    let switches = face.switches.len().min(3);
    let mut height = rows.len() as f32 * 22.0 + rows.len().saturating_sub(1) as f32 * 3.0;
    if switches > 0 {
        height += if rows.is_empty() { 10.0 } else { 13.0 };
    }
    let mut y = rect.center().y - height / 2.0;
    let mut knobs = face.knobs.iter().take(shown);
    for count in &rows {
        let width = *count as f32 * 22.0 + (*count as f32 - 1.0) * 7.0;
        let mut x = rect.center().x - width / 2.0;
        for _ in 0..*count {
            if let Some(fraction) = knobs.next() {
                theme::paint_dial(
                    painter,
                    Rect::from_min_size(Pos2::new(x, y), Vec2::splat(22.0)),
                    *fraction,
                    None,
                    colour,
                    false,
                );
            }
            x += 29.0;
        }
        y += 25.0;
    }
    if switches > 0 {
        let width = switches as f32 * 16.0 + (switches as f32 - 1.0) * 7.0;
        let mut x = rect.center().x - width / 2.0;
        for on in face.switches.iter().take(switches) {
            let pill = Rect::from_min_size(Pos2::new(x, y), Vec2::new(16.0, 10.0));
            if *on {
                painter.rect_filled(pill, CornerRadius::same(5), colour);
            } else {
                painter.rect(
                    pill,
                    CornerRadius::same(5),
                    theme::raised(),
                    Stroke::new(1.0, theme::line_strong()),
                    egui::StrokeKind::Inside,
                );
            }
            x += 23.0;
        }
    }
    if face.knobs.len() > shown {
        let more = shell::galley(
            ui,
            format!("+{}", face.knobs.len() - shown),
            theme::semibold(10.0),
            theme::muted(),
        );
        let size = more.size();
        painter.galley(
            Pos2::new(rect.right() - 6.0 - size.x, rect.bottom() - 4.0 - size.y),
            more,
            Color32::PLACEHOLDER,
        );
    }
}

/// What a card's badge says.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Badge {
    /// What the block held before the first try.
    WasHere,
    /// What the block holds, before anything has been tried.
    InTheBlock,
    /// Playing on the pedal now.
    Playing,
}

impl Badge {
    fn paint(self, ui: &Ui, card: Rect) {
        let (text, accent) = match self {
            Badge::WasHere => ("Was here", false),
            Badge::InTheBlock => ("In the block", false),
            Badge::Playing => ("On the pedal", true),
        };
        let galley = shell::galley(
            ui,
            text,
            theme::semibold(10.5),
            if accent {
                theme::accent()
            } else {
                theme::text_soft()
            },
        );
        let icon = if accent { 12.0 + 4.0 } else { 0.0 };
        let rect = Rect::from_min_size(
            Pos2::new(card.left() + 10.0, card.top() - 9.0),
            Vec2::new(galley.size().x + 14.0 + icon, 18.0),
        );
        let (fill, stroke) = if accent {
            (
                theme::mix(theme::panel(), theme::accent(), 0.18),
                theme::accent_line(),
            )
        } else {
            (theme::panel(), theme::line_strong())
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(theme::RADIUS_CHIP),
            fill,
            Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + 7.0;
        if accent {
            theme::paint_icon(
                ui,
                Icon::Volume,
                Pos2::new(x + 6.0, rect.center().y),
                12.0,
                theme::accent(),
            );
            x += icon;
        }
        shell::paint_line(ui, galley, x, rect.center().y);
    }
}

/// What the browser was asked this frame.
enum Asked {
    Pick(String),
    Close,
    PutBack,
    Keep,
}

impl App {
    /// Open the browser on the selected block, to try other models in its
    /// place.
    pub(crate) fn open_browser_swap(&mut self) {
        let Some(block) = self.chain.get(self.selected).cloned() else {
            return;
        };
        if !self.is_effect(&block) {
            return;
        }
        if matches!(&self.browser, Some(b) if b.target_slot() == Some(block.position)) {
            return;
        }
        self.close_browser(true);
        let original_id = self
            .catalog
            .as_ref()
            .and_then(|c| c.model_number(block.model))
            .map(|m| m.id.clone());
        let rail = match (&self.catalog, block.paired, &original_id) {
            (_, Some(_), _) => Rail::Category(hx_catalog::Category::AMP_CAB),
            (Some(catalog), None, Some(id)) => catalog
                .category_of(id)
                .map_or(self.default_rail(), Rail::Category),
            _ => self.default_rail(),
        };
        self.browser = Some(Browser {
            target: Target::Swap {
                position: block.position,
                original: self.slot_label(&block),
                original_id,
                paired: block.paired.is_some(),
            },
            rail,
            ..self.blank_browser()
        });
    }

    /// Open the browser on a gap, to add a block there.
    pub(crate) fn open_browser_insert(&mut self, at: usize) {
        if self.browser_insert_at() == Some(at) {
            return;
        }
        self.close_browser(true);
        let rail = self.default_rail();
        self.browser = Some(Browser {
            target: Target::Insert { at },
            rail,
            ..self.blank_browser()
        });
    }

    fn blank_browser(&self) -> Browser {
        let index = self
            .catalog
            .as_ref()
            .map(|catalog| {
                let mut seen = std::collections::HashSet::new();
                catalog
                    .categories()
                    .iter()
                    .filter(|category| category.is_effect() && !category.paired)
                    .flat_map(|category| catalog.models_in(category.id))
                    .filter(|model| seen.insert(model.id.clone()))
                    .map(|model| (model.id.clone(), model.name.to_lowercase()))
                    .collect()
            })
            .unwrap_or_default();
        Browser {
            target: Target::Insert { at: 0 },
            search: String::new(),
            rail: Rail::Recent,
            shelf: None,
            list: false,
            playing: None,
            focus: true,
            index,
            faces: HashMap::new(),
        }
    }

    /// Where the browser opens when nothing says otherwise: the category last
    /// looked at, or the first.
    fn default_rail(&self) -> Rail {
        if let Some(category) = self.browsed_category {
            return Rail::Category(category);
        }
        self.catalog
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .categories()
                    .iter()
                    .find(|category| {
                        category.is_effect() && !catalog.models_in(category.id).is_empty()
                    })
                    .map(|category| Rail::Category(category.id))
            })
            .unwrap_or(Rail::Recent)
    }

    /// The gap the browser is adding a block to.
    pub(crate) fn browser_insert_at(&self) -> Option<usize> {
        match self.browser.as_ref()?.target {
            Target::Insert { at } => Some(at),
            Target::Swap { .. } => None,
        }
    }

    /// The block the browser is trying models in.
    pub(crate) fn browser_swap_slot(&self) -> Option<i64> {
        self.browser.as_ref()?.target_slot()
    }

    /// Close the browser, keeping what is playing or putting the block back.
    pub(crate) fn close_browser(&mut self, keep: bool) {
        if let Some(browser) = self.browser.take() {
            if browser.playing.is_some() {
                self.send(if keep { Cmd::KeepTry } else { Cmd::PutBack });
            }
        }
        self.trial_kept.set(false);
    }

    /// Remember a model for Recent.
    fn remember_model(&mut self, id: &str) {
        let recent = &mut self.config.recent_models;
        recent.retain(|known| known != id);
        recent.insert(0, id.to_owned());
        recent.truncate(RECENT);
        self.config.save();
    }

    /// The browser in the pane: its head, the rail and the models, and the
    /// audition bar while a model is being tried.
    pub(crate) fn browser_lens(&mut self, ui: &mut Ui, tier: Tier) {
        // Something else done since the last frame kept what was playing.
        if self.trial_kept.replace(false) {
            if let Some(browser) = self.browser.as_mut() {
                browser.playing = None;
            }
            self.browser = None;
            return;
        }
        let Some(catalog) = self.catalog.as_ref() else {
            let rect = ui.max_rect();
            let words = shell::galley(
                ui,
                "Choosing a model needs HX Edit's data. Add it on the first-run screen.",
                theme::regular(theme::BODY),
                theme::muted(),
            );
            let width = words.size().x;
            shell::paint_line(ui, words, rect.center().x - width / 2.0, rect.top() + 72.0);
            return;
        };
        let recent: Vec<String> = self.config.recent_models.clone();
        let Some(browser) = self.browser.as_mut() else {
            return;
        };
        let full = ui.max_rect();
        let trying = browser.playing.clone();
        let bar = trying.is_some() && matches!(browser.target, Target::Swap { .. });
        let head = Rect::from_min_size(full.min, Vec2::new(full.width(), 60.0));
        let foot =
            bar.then(|| Rect::from_min_max(Pos2::new(full.left(), full.bottom() - 52.0), full.max));
        let body = Rect::from_min_max(
            Pos2::new(full.left(), head.bottom()),
            Pos2::new(full.right(), foot.map_or(full.bottom(), |f| f.top())),
        );
        ui.allocate_rect(full, Sense::hover());

        // What the rail lists: Recent, then HX Edit's categories with counts.
        let categories: Vec<(u32, String, usize)> = catalog
            .categories()
            .iter()
            .filter(|category| category.is_effect())
            .map(|category| {
                (
                    category.id,
                    category.name.clone(),
                    catalog.models_in(category.id).len(),
                )
            })
            .filter(|(_, _, count)| *count > 0)
            .collect();
        let recent_models: Vec<&Model> = recent.iter().filter_map(|id| catalog.model(id)).collect();
        let searching = !browser.search.trim().is_empty();
        let showing = browser.rail;
        let holding = match &browser.target {
            Target::Swap { original_id, .. } => original_id.clone(),
            Target::Insert { .. } => None,
        };
        let shelves: Vec<(&str, Vec<&Model>)> = match (searching, showing) {
            (false, Rail::Category(id)) => catalog
                .category(id)
                .map(|category| {
                    category
                        .subcategories
                        .iter()
                        .map(|sub| {
                            (
                                sub.name.as_str(),
                                sub.models
                                    .iter()
                                    .filter_map(|id| catalog.model(id))
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .filter(|(_, models)| !models.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let shelved = shelves.len() > 1;
        let open = open_shelf(&shelves, browser.shelf.as_deref(), holding.as_deref());
        let models: Vec<&Model> = if searching {
            let needle = browser.search.trim().to_lowercase();
            browser
                .index
                .iter()
                .filter(|(_, name)| name.contains(&needle))
                .filter_map(|(id, _)| catalog.model(id))
                .collect()
        } else if shelved {
            shelves[open].1.clone()
        } else {
            match showing {
                Rail::Recent => recent_models.clone(),
                Rail::Category(id) => catalog.models_in(id),
            }
        };
        let pairing = !searching
            && matches!(showing, Rail::Category(id) if catalog.category(id).is_some_and(|c| c.paired));
        for model in &models {
            if !browser.faces.contains_key(&model.id) {
                browser
                    .faces
                    .insert(model.id.clone(), face_of(catalog, model));
            }
        }
        let colour_of = |model: &Model| {
            catalog
                .category_of(&model.id)
                .and_then(|id| catalog.category(id))
                .map(|category| theme::category_colour(&category.name))
                .unwrap_or_else(theme::accent)
        };

        let mut asked: Option<Asked> = None;

        // The head.
        let (title, subtitle, well_colour, well_drawing) = match &browser.target {
            Target::Swap {
                original,
                original_id,
                ..
            } => {
                let model = original_id.as_deref().and_then(|id| catalog.model(id));
                let category = model
                    .and_then(|m| catalog.category_of(&m.id))
                    .and_then(|id| catalog.category(id))
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                (
                    format!("Swap {original} for"),
                    "Edits are live: click a model to hear it on the pedal".to_owned(),
                    theme::category_colour(&category),
                    theme::category_icon(&category),
                )
            }
            Target::Insert { .. } => (
                "Add a block".to_owned(),
                "Click a model to put it in the marked gap".to_owned(),
                theme::accent(),
                None,
            ),
        };
        let mut head_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(head.left() + 20.0, head.top()),
                    Pos2::new(head.right() - 16.0, head.bottom()),
                ))
                .id_salt("browser-head")
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        head_ui.spacing_mut().item_spacing.x = 12.0;
        let (well, _) = head_ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        theme::paint::gradient_rect(
            head_ui.painter(),
            well,
            11.0,
            &[
                (0.0, theme::alpha(well_colour, 0.24)),
                (1.0, theme::alpha(well_colour, 0.08)),
            ],
        );
        head_ui.painter().rect_stroke(
            well,
            CornerRadius::same(11),
            Stroke::new(1.0, theme::alpha(well_colour, 0.4)),
            egui::StrokeKind::Inside,
        );
        match well_drawing {
            Some(drawing) => drawing.paint(
                &head_ui,
                Rect::from_center_size(well.center(), Vec2::splat(22.0)),
                well_colour,
            ),
            None => theme::paint_icon(&head_ui, Icon::Plus, well.center(), 20.0, well_colour),
        }
        let title_galley = shell::galley(
            &head_ui,
            title,
            theme::semibold(theme::TITLE),
            theme::text(),
        );
        let subtitle_galley =
            shell::galley(&head_ui, subtitle, theme::regular(12.0), theme::muted());
        let words_width = title_galley.size().x.max(subtitle_galley.size().x);
        let (words, _) = head_ui.allocate_exact_size(Vec2::new(words_width, 36.0), Sense::hover());
        shell::paint_line(&head_ui, title_galley, words.left(), words.top() + 9.5);
        shell::paint_line(&head_ui, subtitle_galley, words.left(), words.top() + 27.0);
        head_ui.add_space(8.0);
        let hint = format!("Search {} models", browser.index.len());
        let search_width = if tier == Tier::S { 180.0 } else { 240.0 };
        let field = theme::search_field(
            &mut head_ui,
            "browser-search",
            &mut browser.search,
            &hint,
            search_width,
        );
        if browser.focus {
            field.request_focus();
            browser.focus = false;
        }
        let search_focused = field.has_focus();
        // Enter in the search field finishes the search, nothing more.
        let search_left = field.lost_focus();
        if field.changed() {
            browser.shelf = None;
        }
        // The right of the head, laid out from the right edge.
        let mut right_ui = head_ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    Pos2::new(head_ui.cursor().left(), head.top()),
                    Pos2::new(head.right() - 16.0, head.bottom()),
                ))
                .id_salt("browser-head-right")
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right_ui.spacing_mut().item_spacing.x = 8.0;
        let close_hint = match (&browser.target, &trying) {
            (Target::Swap { original, .. }, Some(_)) => {
                format!("Close and put {original} back (Esc)")
            }
            _ => "Close (Esc)".to_owned(),
        };
        if theme::IconButton::new(Icon::Close)
            .show(&mut right_ui)
            .on_hover_text(close_hint)
            .clicked()
        {
            asked = Some(if trying.is_some() {
                Asked::PutBack
            } else {
                Asked::Close
            });
        }
        let (sep, _) = right_ui.allocate_exact_size(Vec2::new(1.0, 20.0), Sense::hover());
        right_ui
            .painter()
            .rect_filled(sep, CornerRadius::ZERO, theme::line());
        if theme::IconButton::new(Icon::List)
            .on(browser.list)
            .show(&mut right_ui)
            .on_hover_text("Show as a list")
            .clicked()
        {
            browser.list = true;
        }
        if theme::IconButton::new(Icon::LayoutGrid)
            .on(!browser.list)
            .show(&mut right_ui)
            .on_hover_text("Show as cards")
            .clicked()
        {
            browser.list = false;
        }
        if shelved && tier != Tier::S {
            right_ui.add_space(4.0);
            let counts: Vec<String> = shelves
                .iter()
                .map(|(_, models)| models.len().to_string())
                .collect();
            let segments: Vec<theme::Segment> = shelves
                .iter()
                .zip(&counts)
                .map(|((name, _), count)| theme::Segment::new(name).suffix(count.clone()))
                .collect();
            if let Some(index) = theme::segmented(
                &mut right_ui,
                "browser-shelves",
                &segments,
                Some(open),
                false,
            )
            .clicked
            {
                browser.shelf = Some(shelves[index].0.to_owned());
            }
        }
        ui.painter().hline(
            head.x_range(),
            head.bottom() - 0.5,
            Stroke::new(1.0, theme::line()),
        );

        // The rail.
        let rail_width = if tier == Tier::S { 168.0 } else { 188.0 };
        let rail = Rect::from_min_max(body.min, Pos2::new(body.left() + rail_width, body.bottom()));
        ui.painter().vline(
            rail.right() - 0.5,
            rail.y_range(),
            Stroke::new(1.0, theme::line()),
        );
        let mut rail_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rail.shrink2(Vec2::new(8.0, 8.0)))
                .id_salt("browser-rail")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        rail_ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
        egui::ScrollArea::vertical()
            .id_salt("browser-rail-scroll")
            .auto_shrink([false, false])
            .show(&mut rail_ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut chose = None;
                if rail_row(
                    ui,
                    RailRow::Recent,
                    "Recent",
                    recent_models.len(),
                    !searching && showing == Rail::Recent,
                )
                .clicked()
                {
                    chose = Some(Rail::Recent);
                }
                let (sep, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 13.0), Sense::hover());
                ui.painter().hline(
                    (sep.left() + 8.0)..=(sep.right() - 8.0),
                    sep.center().y,
                    Stroke::new(1.0, theme::line()),
                );
                for (id, name, count) in &categories {
                    let on = !searching && showing == Rail::Category(*id);
                    if rail_row(ui, RailRow::Category(name), name, *count, on).clicked() {
                        chose = Some(Rail::Category(*id));
                    }
                }
                if let Some(rail) = chose {
                    browser.rail = rail;
                    browser.shelf = None;
                    browser.search.clear();
                }
            });

        // The models.
        let cards = Rect::from_min_max(Pos2::new(rail.right(), body.top()), body.max);
        let mut cards_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(cards)
                .id_salt("browser-cards")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let original_id = holding.clone();
        let playing_id = trying.as_ref().map(|(id, _)| id.clone());
        let badge_of = |model: &Model| -> Option<Badge> {
            if playing_id.as_deref() == Some(model.id.as_str()) {
                Some(Badge::Playing)
            } else if original_id.as_deref() == Some(model.id.as_str()) {
                Some(if playing_id.is_some() {
                    Badge::WasHere
                } else {
                    Badge::InTheBlock
                })
            } else {
                None
            }
        };
        cards_ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
        egui::ScrollArea::vertical()
            .id_salt(("browser-models", format!("{showing:?}"), open, searching))
            .auto_shrink([false, false])
            .show(&mut cards_ui, |ui| {
                let inner = ui.max_rect();
                // On a small window the head has no room for the shelves, so
                // they lead the models instead.
                if shelved && tier == Tier::S && !searching {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        let counts: Vec<String> = shelves
                            .iter()
                            .map(|(_, models)| models.len().to_string())
                            .collect();
                        let segments: Vec<theme::Segment> = shelves
                            .iter()
                            .zip(&counts)
                            .map(|((name, _), count)| {
                                theme::Segment::new(name).suffix(count.clone())
                            })
                            .collect();
                        if let Some(index) = theme::segmented(
                            ui,
                            "browser-shelves-small",
                            &segments,
                            Some(open),
                            false,
                        )
                        .clicked
                        {
                            browser.shelf = Some(shelves[index].0.to_owned());
                        }
                    });
                }
                if models.is_empty() {
                    let text = if searching {
                        format!("Nothing matches \u{201c}{}\u{201d}", browser.search.trim())
                    } else if showing == Rail::Recent {
                        "Models you choose appear here".to_owned()
                    } else {
                        "Nothing here".to_owned()
                    };
                    let words =
                        shell::galley(ui, text, theme::regular(theme::BODY), theme::muted());
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
                if browser.list {
                    ui.add_space(8.0);
                    for model in &models {
                        let colour = colour_of(model);
                        let face = browser.faces.get(&model.id).cloned().unwrap_or_default();
                        if model_row(ui, catalog, model, &face, colour, badge_of(model)).clicked() {
                            asked = Some(Asked::Pick(model.id.clone()));
                        }
                    }
                    ui.add_space(8.0);
                    return;
                }
                let gap = 12.0;
                let width = cards.width() - 32.0;
                let columns = (((width + gap) / (156.0 + gap)).floor() as usize).max(3);
                let card = ((width - gap * (columns as f32 - 1.0)) / columns as f32).floor();
                ui.add_space(14.0);
                for chunk in models.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.spacing_mut().item_spacing.x = gap;
                        for model in chunk {
                            let colour = colour_of(model);
                            let face = browser.faces.get(&model.id).cloned().unwrap_or_default();
                            if model_card(ui, model, &face, colour, card, badge_of(model)).clicked()
                            {
                                asked = Some(Asked::Pick(model.id.clone()));
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            });

        // The audition bar.
        if let (Some(foot), Some((_, name)), Target::Swap { original, .. }) =
            (foot, &trying, &browser.target)
        {
            ui.painter().rect_filled(
                foot,
                CornerRadius::ZERO,
                theme::mix(theme::bg(), theme::accent(), 0.06),
            );
            ui.painter().hline(
                foot.x_range(),
                foot.top() + 0.5,
                Stroke::new(1.0, theme::line()),
            );
            let mut bar_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        Pos2::new(foot.left() + 20.0, foot.top()),
                        Pos2::new(foot.right() - 16.0, foot.bottom()),
                    ))
                    .id_salt("audition-bar")
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            bar_ui.spacing_mut().item_spacing.x = 12.0;
            let keep_label = format!("Keep {name}");
            let back_label = format!("Put {original} back");
            if theme::Button::new(&keep_label)
                .primary()
                .small()
                .hint("Enter")
                .show(&mut bar_ui)
                .on_hover_text("Keep it as an edit. One undo puts the block back")
                .clicked()
            {
                asked = Some(Asked::Keep);
            }
            if theme::Button::new(&back_label)
                .small()
                .hint("Esc")
                .show(&mut bar_ui)
                .on_hover_text("Put the block back exactly as it was")
                .clicked()
            {
                asked = Some(Asked::PutBack);
            }
            let room = bar_ui.min_rect().left() - 12.0 - (foot.left() + 20.0);
            let mut words_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(
                        Pos2::new(foot.left() + 20.0, foot.top()),
                        Pos2::new(foot.left() + 20.0 + room.max(0.0), foot.bottom()),
                    ))
                    .id_salt("audition-words")
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            words_ui.spacing_mut().item_spacing.x = 12.0;
            theme::Chip::new("Live")
                .mood(Mood::Accent)
                .icon(Icon::Volume)
                .show(&mut words_ui);
            let mut job = egui::text::LayoutJob::default();
            job.append(
                name,
                0.0,
                egui::TextFormat {
                    font_id: theme::semibold(12.5),
                    color: theme::text(),
                    ..Default::default()
                },
            );
            job.append(
                &format!(
                    " is playing on the pedal in {original}'s place. Its knobs start at their defaults."
                ),
                0.0,
                egui::TextFormat {
                    font_id: theme::regular(12.5),
                    color: theme::text_soft(),
                    ..Default::default()
                },
            );
            job.wrap = egui::text::TextWrapping {
                max_width: words_ui.available_width().max(40.0),
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let galley = words_ui.painter().layout_job(job);
            let (place, _) = words_ui.allocate_exact_size(galley.size(), Sense::hover());
            words_ui
                .painter()
                .galley(place.min, galley, Color32::PLACEHOLDER);
        }

        // Keys: Escape puts back or closes, Enter keeps. Not while another
        // field has the keyboard: its Enter and Escape are its own.
        let elsewhere = !search_focused && ui.memory(|memory| memory.focused().is_some());
        let (escape, enter) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Escape),
                input.key_pressed(egui::Key::Enter),
            )
        });
        let (escape, enter) = (escape && !elsewhere, enter && !elsewhere);
        if asked.is_none() && escape {
            asked = Some(if trying.is_some() {
                Asked::PutBack
            } else {
                Asked::Close
            });
        }
        if asked.is_none() && enter && trying.is_some() && !search_focused && !search_left {
            asked = Some(Asked::Keep);
        }
        let last_category = match showing {
            Rail::Category(id) if !searching => Some(id),
            _ => None,
        };
        let pick = match &asked {
            Some(Asked::Pick(id)) => catalog.model(id).and_then(|model| {
                let number = crate::number_of(catalog, &model.id)?;
                // In a paired category the cab comes with the amp; if the
                // firmware does not know that cab the amp goes in alone.
                let paired = pairing
                    .then(|| catalog.paired_cab(model))
                    .flatten()
                    .and_then(|cab| crate::number_of(catalog, &cab.id));
                Some((model.id.clone(), model.name.clone(), number, paired))
            }),
            _ => None,
        };
        if last_category.is_some() {
            self.browsed_category = last_category;
        }
        match asked {
            Some(Asked::Pick(_)) => {
                if let Some(pick) = pick {
                    let id = pick.0.clone();
                    if self.pick_model(pick) {
                        self.remember_model(&id);
                    }
                }
            }
            Some(Asked::Close) => self.close_browser(true),
            Some(Asked::Keep) => self.close_browser(true),
            Some(Asked::PutBack) => self.close_browser(false),
            None => {}
        }
    }

    /// Try a model in the block's place, or put one in the gap. Returns
    /// whether a model was played or placed, for Recent.
    pub(crate) fn pick_model(
        &mut self,
        (id, name, model, paired): (String, String, u32, Option<u32>),
    ) -> bool {
        let Some(browser) = self.browser.as_mut() else {
            return false;
        };
        match browser.target.clone() {
            Target::Swap {
                position,
                original_id,
                paired: was_paired,
                ..
            } => {
                if browser
                    .playing
                    .as_ref()
                    .is_some_and(|(playing, _)| *playing == id)
                {
                    return false;
                }
                // Choosing what was there is putting it back, exactly: a
                // fresh copy of the model would lose its settings.
                if original_id.as_deref() == Some(id.as_str()) && was_paired == paired.is_some() {
                    if browser.playing.take().is_some() {
                        self.send(Cmd::PutBack);
                    }
                    return false;
                }
                browser.playing = Some((id.clone(), name));
                self.dirty = true;
                self.send(Cmd::TryModel {
                    block: position,
                    model,
                    paired,
                });
                true
            }
            Target::Insert { at } => {
                self.browser = None;
                self.pending_select = Some(at as i64);
                self.edit(Cmd::InsertBlock { at, model, paired });
                true
            }
        }
    }
}

impl Browser {
    fn target_slot(&self) -> Option<i64> {
        match self.target {
            Target::Swap { position, .. } => Some(position),
            Target::Insert { .. } => None,
        }
    }
}

/// What a rail row shows before its name.
enum RailRow<'a> {
    Recent,
    Category(&'a str),
}

/// One row of the rail: its drawing, its name and how many it holds.
fn rail_row(ui: &mut Ui, row: RailRow, name: &str, count: usize, on: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 29.0), Sense::click());
    if on {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(7), theme::hover());
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(7),
            theme::alpha(theme::hover(), 0.5),
        );
    }
    let y = rect.center().y;
    let glyph = Pos2::new(rect.left() + 8.0 + 8.0, y);
    match row {
        RailRow::Recent => theme::paint_icon(ui, Icon::History, glyph, 16.0, theme::text_soft()),
        RailRow::Category(category) => {
            if let Some(drawing) = theme::category_icon(category) {
                drawing.paint(
                    ui,
                    Rect::from_center_size(glyph, Vec2::splat(16.0)),
                    theme::category_colour(category),
                );
            }
        }
    }
    let counted = shell::galley(
        ui,
        count.to_string(),
        theme::regular(11.5),
        if on { theme::muted() } else { theme::faint() },
    );
    let count_width = counted.size().x;
    shell::paint_line(ui, counted, rect.right() - 8.0 - count_width, y);
    let label = shell::elided(
        ui,
        name,
        theme::regular(theme::BODY),
        if on {
            theme::text()
        } else {
            theme::text_soft()
        },
        rect.width() - 8.0 - 16.0 - 9.0 - count_width - 16.0,
    );
    shell::paint_line(ui, label, rect.left() + 8.0 + 16.0 + 9.0, y);
    response
}

/// One model as a card: its face, its name and the names of its controls.
fn model_card(
    ui: &mut Ui,
    model: &Model,
    face: &Face,
    colour: Color32,
    width: f32,
    badge: Option<Badge>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 130.0), Sense::click());
    let playing = badge == Some(Badge::Playing);
    let was = matches!(badge, Some(Badge::WasHere | Badge::InTheBlock));
    if playing {
        ui.painter().rect_stroke(
            rect.expand(1.5),
            CornerRadius::same(14),
            Stroke::new(3.0, theme::accent_soft()),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::RADIUS_TILE),
        if response.hovered() {
            theme::raised()
        } else {
            theme::panel()
        },
        Stroke::new(
            1.0,
            if playing {
                theme::accent()
            } else if was {
                theme::line_strong()
            } else {
                theme::line()
            },
        ),
        egui::StrokeKind::Inside,
    );
    let face_rect = Rect::from_min_size(
        rect.min + Vec2::new(7.0, 7.0),
        Vec2::new(rect.width() - 14.0, 70.0),
    );
    paint_face(ui, face_rect, face, colour);
    let name = shell::elided(
        ui,
        model.name.clone(),
        theme::semibold(theme::BODY),
        theme::text(),
        rect.width() - 20.0,
    );
    shell::paint_line(
        ui,
        name,
        rect.left() + 10.0,
        face_rect.bottom() + 4.0 + 2.0 + 8.5,
    );
    let controls = shell::elided(
        ui,
        face.names.clone(),
        theme::regular(11.0),
        theme::muted(),
        rect.width() - 20.0,
    );
    shell::paint_line(
        ui,
        controls,
        rect.left() + 10.0,
        face_rect.bottom() + 4.0 + 19.0 + 4.0 + 7.0,
    );
    if let Some(badge) = badge {
        badge.paint(ui, rect);
    }
    response.on_hover_text(match badge {
        Some(Badge::Playing) => format!("{} is on the pedal", model.name),
        Some(Badge::WasHere) => format!("Put {} back", model.name),
        _ => format!("Play {} on the pedal", model.name),
    })
}

/// One model as a row of the list.
fn model_row(
    ui: &mut Ui,
    catalog: &Catalog,
    model: &Model,
    face: &Face,
    colour: Color32,
    badge: Option<Badge>,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::click());
    let row = rect.shrink2(Vec2::new(16.0, 0.0));
    if badge == Some(Badge::Playing) {
        ui.painter().rect_filled(
            row,
            CornerRadius::same(7),
            theme::mix(theme::panel(), theme::accent(), 0.12),
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(row, CornerRadius::same(7), theme::hover());
    }
    ui.painter().hline(
        row.x_range(),
        row.bottom() - 0.5,
        Stroke::new(1.0, theme::line_soft()),
    );
    let y = row.center().y;
    let category = catalog
        .category_of(&model.id)
        .and_then(|id| catalog.category(id))
        .map(|c| c.name.clone())
        .unwrap_or_default();
    if let Some(drawing) = theme::category_icon(&category) {
        drawing.paint(
            ui,
            Rect::from_center_size(Pos2::new(row.left() + 10.0 + 8.0, y), Vec2::splat(16.0)),
            colour,
        );
    }
    let name = shell::elided(
        ui,
        model.name.clone(),
        theme::medium(theme::BODY),
        theme::text(),
        220.0,
    );
    let name_width = name.size().x;
    shell::paint_line(ui, name, row.left() + 40.0, y);
    let mut right = row.right() - 10.0;
    if let Some(badge) = badge {
        let text = match badge {
            Badge::Playing => "On the pedal",
            Badge::WasHere => "Was here",
            Badge::InTheBlock => "In the block",
        };
        let words = shell::galley(
            ui,
            text,
            theme::semibold(11.0),
            if badge == Badge::Playing {
                theme::accent()
            } else {
                theme::muted()
            },
        );
        right -= words.size().x;
        shell::paint_line(ui, words, right, y);
        right -= 16.0;
    }
    let left = row.left() + 40.0 + name_width.max(180.0) + 16.0;
    if right - left > 40.0 {
        let controls = shell::elided(
            ui,
            face.names.clone(),
            theme::regular(12.0),
            theme::muted(),
            right - left,
        );
        shell::paint_line(ui, controls, left, y);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_shows_up_to_six_knobs_in_even_rows() {
        assert_eq!(card_rows(0), (0, vec![]));
        assert_eq!(card_rows(3), (3, vec![3]));
        assert_eq!(card_rows(4), (4, vec![4]));
        assert_eq!(card_rows(5), (5, vec![3, 2]));
        assert_eq!(card_rows(6), (6, vec![3, 3]));
        // More than six: five shown and the rest counted.
        assert_eq!(card_rows(12), (5, vec![3, 2]));
    }

    #[test]
    fn the_shelf_holding_the_block_opens_unless_one_was_chosen() {
        let model = |id: &str| Model {
            id: id.to_owned(),
            name: id.to_owned(),
            category: 0,
            stereo: false,
            load: 0.0,
            image: None,
            cab_link: None,
            params: Vec::new(),
        };
        let (a, b, c) = (model("a"), model("b"), model("c"));
        let shelves = vec![
            ("Mono", vec![&a]),
            ("Stereo", vec![&b]),
            ("Legacy", vec![&c]),
        ];
        assert_eq!(open_shelf(&shelves, None, Some("b")), 1);
        assert_eq!(open_shelf(&shelves, Some("Legacy"), Some("b")), 2);
        // A shelf chosen in another category is not one of these.
        assert_eq!(open_shelf(&shelves, Some("Guitar"), None), 0);
    }
}
