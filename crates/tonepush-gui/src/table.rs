//! The library's table, and the only one in the app.
//!
//! Tones from the library and from TonePush's catalog are drawn by this: they
//! differ in their columns and in nothing else, which is the point. They are
//! one library with two views of itself, and two tables that behaved
//! differently would say otherwise.
//!
//! Rows are virtualised, so a library of a few thousand tones lays out only the
//! twenty you can see, and the first columns are sticky, so scrolling sideways
//! never takes the name away from the row it belongs to. That is what
//! `egui_extras::TableBuilder` could not do: it builds every row every frame.
//!
//! The cells are prepared as plain values before the table draws. Reading them
//! costs a few short strings per row and it buys the whole thing being a value:
//! sorting, selection and editing all happen on data rather than inside a
//! drawing closure, where borrowing the app twice is a fight.
//!
//! It is drawn as the 2026-10-01 redesign draws a table: 36-point rows with a
//! hairline between them and no stripes, a 32-point header of captions, the
//! selection a neutral fill, and the sorted column named in the ink of the
//! rows rather than in amber, which is kept for the next action.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::shell;
use crate::theme::{self, Icon};

/// How far a cell's contents sit from its column's left edge.
const PADDING: f32 = 8.0;

/// How tall one row is. Public because a caller that has to bound the table's
/// height has to know what a row costs.
pub const ROW_HEIGHT: f32 = 36.0;

/// How tall the header is.
pub const HEADER_HEIGHT: f32 = 32.0;

/// What a cell holds.
pub enum Cell {
    /// Ordinary text.
    Text(String),
    /// Text in the second voice: derived, not typed, and not editable.
    Dim(String),
    /// A tone's name, in the row's strongest weight, with a quieter tag after
    /// it for a tone from another pedal family ("PRO").
    Name { text: String, tag: Option<String> },
    /// Two things in one column, the second quieter: "Static Bloom · June
    /// Arcade". Sorts on the first.
    Pair { text: String, aside: String },
    /// Read-only text whose sort key is not its presentation (for example a
    /// comma-formatted download count or a shortened ISO timestamp).
    Value {
        text: String,
        key: String,
        dim: bool,
    },
    /// A chain as a strip of category colours; sorts on `key`.
    Chain {
        chain: Vec<shell::Mini>,
        key: String,
    },
    /// A rating out of five as stars. Clicking a star rates, when `editable`.
    Stars { rating: u8, editable: bool },
    /// Where a tone is: one icon per place, each its own button. The last
    /// value says whether this particular view offers an action there. Sync
    /// can still be unknown while sending is useful (for example before the
    /// first pedal backup has finished), so actionability cannot be inferred
    /// from the sync state alone.
    Places(Vec<(theme::Icon, theme::Sync, &'static str, bool)>),
}

impl Cell {
    /// What this cell sorts on. A dot sorts by its state, so clicking that
    /// header gathers everything that needs doing.
    fn key(&self) -> SortKey {
        match self {
            Cell::Text(t) | Cell::Dim(t) => SortKey::Text(t.to_lowercase()),
            Cell::Name { text, .. } | Cell::Pair { text, .. } => SortKey::Text(text.to_lowercase()),
            Cell::Value { key, .. } | Cell::Chain { key, .. } => SortKey::Text(key.clone()),
            Cell::Stars { rating, .. } => SortKey::Text(rating.to_string()),
            // Sorted so everything with something to do gathers at the top.
            Cell::Places(places) => SortKey::Text(
                places
                    .iter()
                    .map(|(_, state, _, _)| match state {
                        theme::Sync::Working => '0',
                        theme::Sync::Differs => '1',
                        theme::Sync::Absent => '2',
                        theme::Sync::Same => '3',
                        theme::Sync::Unknown => '4',
                    })
                    .collect(),
            ),
        }
    }
}

/// What a cell sorts on: its words, or its state written as a rank.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Text(String),
}

/// A column: what it is called, how wide, and whether its cells can be typed
/// into.
pub struct Column {
    /// Owned rather than static: a header can depend on what is in the column.
    pub title: String,
    pub width: f32,
    pub editable: bool,
    /// Fills whatever is left. At most one column should say yes.
    pub fills: bool,
}

impl Column {
    pub fn new(title: impl Into<String>, width: f32) -> Column {
        Column {
            title: title.into(),
            width,
            editable: false,
            fills: false,
        }
    }

    pub fn editable(mut self) -> Column {
        self.editable = true;
        self
    }

    pub fn fills(mut self) -> Column {
        self.fills = true;
        self
    }
}

/// Everything the table needs to draw, prepared.
#[derive(Default)]
pub struct Grid {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    /// Which rows are picked out, by row index into `rows`.
    pub chosen: Vec<bool>,
    /// The one row whose details the inspector is showing.
    pub selected: Option<usize>,
    /// Which column orders the rows, and which way. Only used for the arrow;
    /// the caller has already sorted.
    pub sort: (usize, bool),
    /// How many columns stay put when the table is scrolled sideways.
    pub sticky: usize,
    /// The cell being typed into, and what has been typed so far.
    pub editing: Option<(usize, usize)>,
    pub draft: String,
    /// What a right-click offers, in order. Data rather than a closure, so the
    /// menu can be drawn inside the cell where egui wants it without the table
    /// having to borrow the app that owns the actions.
    pub menu: Vec<String>,
    /// Optional columns offered by right-clicking any column header. The
    /// caller-provided key is returned unchanged when visibility is toggled.
    pub column_choices: Vec<(usize, String, bool)>,
    /// Said before the optional cue icon when there are no rows. The headers
    /// still show, because an empty table should still say what it would hold.
    pub nothing_yet: &'static str,
    /// The exact interface glyph an empty-state instruction refers to.
    pub nothing_icon: Option<theme::Icon>,
    /// The rest of the sentence after [`Self::nothing_icon`], allowing the
    /// actual glyph to sit where its name would otherwise have to be.
    pub nothing_after_icon: &'static str,
    /// How tall a row is. Zero means [`ROW_HEIGHT`].
    pub row_height: f32,
}

impl Grid {
    /// Put the rows in their displayed order and return where each one came
    /// from. Selection and editing follow their rows automatically.
    ///
    /// A cell may have to build a lowercase or formatted key. Caching those
    /// keys means one allocation per row instead of one per sort comparison,
    /// which matters when the library holds thousands of tones.
    pub fn sort_rows(&mut self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        let Some(columns) = self.rows.first().map(Vec::len).filter(|&len| len > 0) else {
            return order;
        };
        let sorting = self.sort.0.min(columns - 1);
        order.sort_by_cached_key(|&row| self.rows[row][sorting].key());
        if !self.sort.1 {
            order.reverse();
        }

        self.rows = reorder(std::mem::take(&mut self.rows), &order);
        if self.chosen.len() == order.len() {
            self.chosen = order.iter().map(|&row| self.chosen[row]).collect();
        }
        self.selected = self
            .selected
            .and_then(|selected| order.iter().position(|&row| row == selected));
        self.editing = self.editing.and_then(|(editing, column)| {
            order
                .iter()
                .position(|&row| row == editing)
                .map(|row| (row, column))
        });
        order
    }
}

/// Put values back in an order worked out without cloning them.
fn reorder<T>(values: Vec<T>, order: &[usize]) -> Vec<T> {
    let mut held: Vec<Option<T>> = values.into_iter().map(Some).collect();
    order
        .iter()
        .filter_map(|&index| held.get_mut(index).and_then(Option::take))
        .collect()
}

/// What a person did to the table this frame.
#[derive(Default)]
pub struct Did {
    pub clicked: Option<(usize, bool, bool)>,
    pub double_clicked: Option<usize>,
    /// Which row's place icon was pressed, and which of them.
    pub place: Option<(usize, usize)>,
    /// A row rated with its stars, and the rating: clicking the rating a row
    /// already has takes it away.
    pub rated: Option<(usize, u8)>,
    /// Start typing in this cell.
    pub edit: Option<(usize, usize)>,
    /// Finish typing: keep it, or throw it away.
    pub committed: bool,
    pub cancelled: bool,
    pub sort: Option<usize>,
    /// A header context-menu choice: caller key and new visibility.
    pub column_visibility: Option<(usize, bool)>,
    /// A right-click, and which item of the menu it ended on.
    pub context: Option<usize>,
    pub chose: Option<(usize, usize)>,
    /// Furthest row the virtual table actually painted this frame. Callers
    /// with paged backing data use this to prefetch shortly before the reader
    /// reaches the rows they have not loaded yet.
    pub last_visible: Option<usize>,
}

/// Draw it, and answer with what happened.
pub fn show(ui: &mut Ui, id: &str, grid: &mut Grid) -> Did {
    if grid.rows.is_empty() && !grid.nothing_yet.is_empty() {
        // Headers first, then the sentence under them: an empty table that
        // shows nothing at all looks broken, and one that shows only a sentence
        // does not say what it is for.
        let column_visibility = draw_headers(ui, grid);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(PADDING + 8.0);
            theme::label(
                ui,
                grid.nothing_yet,
                theme::regular(theme::BODY),
                theme::muted(),
            );
            if let Some(icon) = grid.nothing_icon {
                theme::place_enabled(ui, icon, theme::Sync::Absent, false);
                if !grid.nothing_after_icon.is_empty() {
                    theme::label(
                        ui,
                        grid.nothing_after_icon,
                        theme::regular(theme::BODY),
                        theme::muted(),
                    );
                }
            }
        });
        return Did {
            column_visibility,
            ..Default::default()
        };
    }

    let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
    let available = ui.available_width();
    // Every column is widened by its own padding below, so the filling column
    // has to give that room back for every one of them - including its own.
    // Counting only the fixed widths made the columns add up to wider than the
    // table and put a scrollbar under a table that fitted.
    let fixed: f32 = grid
        .columns
        .iter()
        .filter(|c| !c.fills)
        .map(|c| c.width)
        .sum::<f32>()
        + PADDING * grid.columns.len() as f32;
    let columns: Vec<egui_table::Column> = grid
        .columns
        .iter()
        .map(|c| {
            // The margin is for the scrollbar the table draws down its right,
            // which takes width the columns cannot have.
            let width = if c.fills {
                (available - fixed - 12.0).max(90.0)
            } else {
                c.width
            };
            // The width asked for is a floor, not a suggestion. egui_table
            // sizes a column to its widest cell on the first frame, and a cell
            // that truncates rather than wraps reports almost no width at all,
            // so left to itself every column collapses to a few pixels and the
            // headers read "Nar Cha Ger". A floor costs the ability to drag a
            // column narrower than its default, which nobody has ever wanted.
            let width = width + PADDING;
            egui_table::Column::new(width)
                .range(width..=drag_ceiling(width))
                .resizable(!c.fills)
        })
        .collect();

    let mut delegate = Delegate {
        grid,
        did: Did::default(),
        // Taken from the table's own name, so it is the same id next frame.
        // See `Delegate::id`.
        id: ui.id().with(id),
        ctrl,
        shift,
    };
    // The lines between columns show only where a column is being resized:
    // the design's table has none at rest.
    ui.scope(|ui| {
        ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::NONE;
        egui_table::Table::new()
            .id_salt(id)
            .num_rows(delegate.grid.rows.len() as u64)
            .columns(columns)
            .num_sticky_cols(delegate.grid.sticky)
            .headers([egui_table::HeaderRow::new(HEADER_HEIGHT)])
            .show(ui, &mut delegate);
    });
    delegate.did
}

/// How wide a column may be dragged, given the width it starts at.
///
/// 800 is the sensible stopping point for dragging a column of text wider, and
/// it used to be written as a flat maximum. But a column that fills takes
/// whatever the window leaves it, and on a wide enough window that is more than
/// 800 - which made the range `1007.75..=800.0`, and egui clamps into that
/// range, and a clamp whose min exceeds its max panics. Opening a setlist on a
/// large display was enough to do it.
///
/// A ceiling below the floor is not a ceiling. The width a column already needs
/// is the least it may be allowed.
fn drag_ceiling(width: f32) -> f32 {
    width.max(800.0)
}

/// How tall this table's rows are: what it asked for, or the design's row.
fn row_height(grid: &Grid) -> f32 {
    if grid.row_height > 0.0 {
        grid.row_height
    } else {
        ROW_HEIGHT
    }
}

/// A header's title, with the sort arrow after it when it orders the rows.
fn paint_header(ui: &Ui, rect: Rect, title: &str, sorting: Option<bool>, hovered: bool) {
    if hovered {
        ui.painter().rect_filled(
            rect.shrink2(Vec2::new(2.0, 4.0)),
            CornerRadius::same(6),
            theme::alpha(theme::hover(), 0.6),
        );
    }
    if title.is_empty() {
        return;
    }
    let galley = shell::galley(
        ui,
        title,
        theme::semibold(12.0),
        if sorting.is_some() {
            theme::text_soft()
        } else {
            theme::muted()
        },
    );
    let width = galley.size().x;
    let x = rect.left() + PADDING;
    shell::paint_line(ui, galley, x, rect.center().y);
    if let Some(ascending) = sorting {
        theme::paint_icon(
            ui,
            if ascending {
                Icon::ChevronUp
            } else {
                Icon::ChevronDown
            },
            Pos2::new(x + width + 9.0, rect.center().y),
            12.0,
            theme::text_soft(),
        );
    }
}

/// The header row on its own, for when there are no rows to hang it above.
fn draw_headers(ui: &mut Ui, grid: &Grid) -> Option<(usize, bool)> {
    let mut changed = None;
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, column) in grid.columns.iter().enumerate() {
            let width = if column.fills {
                ui.available_width()
            } else {
                column.width + PADDING
            };
            let (rect, hit) =
                ui.allocate_exact_size(Vec2::new(width, HEADER_HEIGHT), Sense::click());
            if ui.is_rect_visible(rect) {
                paint_header(
                    ui,
                    rect,
                    &column.title,
                    (i == grid.sort.0).then_some(grid.sort.1),
                    false,
                );
            }
            if let Some(choice) = column_context_menu(&hit, &grid.column_choices) {
                changed = Some(choice);
            }
        }
    });
    let full = ui.max_rect();
    ui.painter().hline(
        full.x_range(),
        top + HEADER_HEIGHT - 0.5,
        Stroke::new(1.0, theme::line()),
    );
    changed
}

/// The same visibility menu belongs to every header cell. Keeping it here
/// also gives an empty table (whose headers are drawn without `egui_table`) the
/// identical interaction.
fn column_context_menu(
    response: &egui::Response,
    choices: &[(usize, String, bool)],
) -> Option<(usize, bool)> {
    if choices.is_empty() {
        return None;
    }
    let mut changed = None;
    response.context_menu(|ui| {
        if let Some(choice) = column_menu(ui, choices) {
            changed = Some(choice);
        }
    });
    changed
}

/// Which columns to show, as a menu of checked rows. Shared with any button
/// that offers the same choice.
pub fn column_menu(ui: &mut Ui, choices: &[(usize, String, bool)]) -> Option<(usize, bool)> {
    theme::menu_width(ui, 200.0);
    theme::menu_header(ui, "Columns", None);
    let mut changed = None;
    for (key, title, shown) in choices {
        if theme::menu_item(ui, shown.then_some(Icon::Check), title, None).clicked() {
            changed = Some((*key, !*shown));
        }
    }
    changed
}

struct Delegate<'a> {
    grid: &'a mut Grid,
    did: Did,
    /// The table's own id, for anything inside a cell that has to be the *same*
    /// widget from one frame to the next.
    ///
    /// Keyboard focus is held by id, and an id egui makes up for a widget
    /// counts the widgets built before it in that ui. `egui_table` builds a
    /// cell's ui more than once and not always the same way, so a field left to
    /// name itself can come out with a different id and lose the focus it just
    /// asked for - which is a cell drawn as an empty box that ignores
    /// everything typed at it. Named from the table, it is the same field every
    /// time.
    id: egui::Id,
    ctrl: bool,
    shift: bool,
}

/// Five stars at `x`, filled to `rating`; returns the star under the pointer.
fn paint_stars(ui: &Ui, centre_y: f32, x: f32, rating: u8, pointer: Option<Pos2>) -> Option<u8> {
    let mut under = None;
    for star in 1..=5u8 {
        let centre = Pos2::new(x + 6.0 + f32::from(star - 1) * 14.0, centre_y);
        let hit = Rect::from_center_size(centre, Vec2::new(14.0, 18.0));
        if pointer.is_some_and(|p| hit.contains(p)) {
            under = Some(star);
        }
    }
    let shown = under.unwrap_or(rating);
    for star in 1..=5u8 {
        let centre = Pos2::new(x + 6.0 + f32::from(star - 1) * 14.0, centre_y);
        let filled = star <= shown;
        theme::paint_icon(
            ui,
            if filled { Icon::StarOn } else { Icon::Star },
            centre,
            12.0,
            if filled {
                theme::accent()
            } else {
                theme::faint()
            },
        );
    }
    under
}

impl egui_table::TableDelegate for Delegate<'_> {
    fn header_cell_ui(&mut self, ui: &mut Ui, cell: &egui_table::HeaderCellInfo) {
        let Some(column) = self.grid.columns.get(cell.col_range.start) else {
            return;
        };
        // The whole cell sorts, not just the few pixels the word covers. A
        // header is a target the width of its column; anything less is a game
        // of hunt-the-arrow.
        let index = cell.col_range.start;
        let (sorting, ascending) = self.grid.sort;
        let size = ui.available_size();
        let (rect, hit) = ui.allocate_exact_size(size, Sense::click());
        if ui.is_rect_visible(rect) {
            paint_header(
                ui,
                rect,
                &column.title,
                (index == sorting).then_some(ascending),
                hit.hovered(),
            );
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                Stroke::new(1.0, theme::line()),
            );
        }
        let hover = if self.grid.column_choices.is_empty() {
            "Sort by this column"
        } else {
            "Sort by this column. Right-click to choose columns"
        };
        let hit = hit.on_hover_text(hover);
        if let Some(choice) = column_context_menu(&hit, &self.grid.column_choices) {
            self.did.column_visibility = Some(choice);
        }
        if hit.clicked() {
            self.did.sort = Some(index);
        }
    }

    fn cell_ui(&mut self, ui: &mut Ui, cell: &egui_table::CellInfo) {
        let row = cell.row_nr as usize;
        let col = cell.col_nr;
        let visible = !ui.is_sizing_pass() && ui.clip_rect().intersect(ui.max_rect()).is_positive();
        if visible {
            self.did.last_visible = Some(self.did.last_visible.map_or(row, |last| last.max(row)));
        }
        let rect = ui.max_rect();
        let picked =
            self.grid.chosen.get(row).copied().unwrap_or(false) || self.grid.selected == Some(row);
        // The row under the pointer, across every column: egui_table draws
        // cells, and the row is the thing a person sees.
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let hovered_row = pointer.is_some_and(|p| {
            p.y >= rect.top() && p.y < rect.bottom() && ui.clip_rect().x_range().contains(p.x)
        });
        // Selection is a neutral fill: amber is kept for the next action.
        let background = if picked {
            Some(theme::hover())
        } else if hovered_row {
            Some(theme::alpha(theme::hover(), 0.45))
        } else {
            None
        };
        if let Some(fill) = background {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, theme::line_soft()),
        );

        // Every cell is inset from its column's edge. Without it the text sits
        // hard against the line beside it and the table reads as a spreadsheet
        // somebody forgot to finish.
        ui.add_space(PADDING);
        let editing = self.grid.editing == Some((row, col));
        if editing {
            // Only the copy you can see gets a field.
            //
            // `egui_table` builds every cell twice - once among the sticky
            // columns and once among the scrolling ones - and clips whichever
            // copy does not belong down to nothing. Both are real widgets all
            // the same, so a field built in both holds the keyboard twice and
            // takes every keystroke twice: one Z typed arrived as ZZ. The
            // clipped copy is laid out for its width and left inert. Same for a
            // sizing pass, which is measuring rather than showing.
            if !visible {
                ui.label(self.grid.draft.clone());
                return;
            }
            // No cell-wide click target while a cell is a field: the field is
            // the cell, and a second widget over it takes the keyboard focus it
            // is asking for.
            let mut shown = egui::TextEdit::singleline(&mut self.grid.draft)
                .id(self.id.with(("editing", row, col)))
                .desired_width(f32::INFINITY)
                .font(theme::regular(theme::BODY))
                .frame(egui::Frame::NONE)
                .show(ui);
            let field = shown.response;
            if !field.has_focus() && !field.lost_focus() {
                field.request_focus();
                // With everything selected, so the first thing typed replaces
                // what was there. A cell opens on the value it already holds,
                // and typing 35 into one reading "0 %" left "0 %35" - which is
                // not what anybody meant and is not what a rename does anywhere
                // else either.
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(self.grid.draft.chars().count()),
                );
                shown.state.cursor.set_char_range(Some(all));
                shown.state.store(ui.ctx(), field.id);
            }
            ui.painter().hline(
                (rect.left() + PADDING)..=(rect.right() - 4.0),
                rect.bottom() - 6.0,
                Stroke::new(1.0, theme::accent_line()),
            );
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.did.cancelled = true;
            } else if field.lost_focus() {
                self.did.committed = true;
            }
            return;
        }

        // The whole cell answers, not just the words in it. Without this a
        // right-click landed only on the text, which is a sliver of the row,
        // and the row's menu looked broken.
        //
        // Claimed *before* the contents are drawn, which is the whole trick.
        // egui gives a tie to whichever widget was added last, so a target laid
        // over the cell afterwards quietly takes the clicks meant for what is
        // inside it: it is why pressing Push in the library only ever selected
        // the row.
        let whole = ui.interact(rect, ui.id().with(("cell", row, col)), Sense::click());

        let Some(content) = self.grid.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        // Whether something inside the cell answered the click itself, so the
        // cell does not answer it a second time.
        let mut claimed = false;
        let left = rect.left() + PADDING;
        let room = (rect.right() - left - 6.0).max(8.0);
        let y = rect.center().y;
        match content {
            Cell::Places(places) => {
                let places = places.clone();
                // On the row's centre line, like the text beside them.
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_max(
                            Pos2::new(left, rect.top()),
                            rect.right_bottom(),
                        ))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                child.spacing_mut().item_spacing.x = 4.0;
                for (n, (icon, state, hover, enabled)) in places.iter().enumerate() {
                    let hit = theme::place_enabled(&mut child, *icon, *state, *enabled);
                    let hit = if hover.is_empty() {
                        hit
                    } else {
                        hit.on_hover_text(*hover)
                    };
                    if *enabled && hit.clicked() {
                        self.did.place = Some((row, n));
                        claimed = true;
                    }
                }
            }
            Cell::Text(text) => {
                let galley = shell::elided(
                    ui,
                    text.clone(),
                    theme::regular(theme::BODY),
                    theme::text(),
                    room,
                );
                shell::paint_line(ui, galley, left, y);
            }
            Cell::Dim(text) => {
                let galley = shell::elided(
                    ui,
                    text.clone(),
                    theme::regular(theme::BODY),
                    theme::muted(),
                    room,
                );
                shell::paint_line(ui, galley, left, y);
            }
            Cell::Name { text, tag } => {
                let tag = tag.as_ref().map(|tag| {
                    ui.painter().layout_job(theme::paint::spaced(
                        tag,
                        theme::semibold(10.0),
                        theme::faint(),
                        0.04,
                    ))
                });
                let tag_width = tag.as_ref().map_or(0.0, |tag| tag.size().x + 6.0);
                let galley = shell::elided(
                    ui,
                    text.clone(),
                    theme::semibold(theme::BODY),
                    theme::text(),
                    (room - tag_width).max(8.0),
                );
                let width = galley.size().x;
                shell::paint_line(ui, galley, left, y);
                if let Some(tag) = tag {
                    let height = tag.size().y;
                    ui.painter().galley(
                        Pos2::new(left + width + 6.0, y - height / 2.0 + 0.5),
                        tag,
                        Color32::PLACEHOLDER,
                    );
                }
            }
            Cell::Pair { text, aside } => {
                let first = shell::elided(
                    ui,
                    text.clone(),
                    theme::regular(theme::BODY),
                    theme::text_soft(),
                    room,
                );
                let width = first.size().x;
                shell::paint_line(ui, first, left, y);
                if !aside.is_empty() && room - width > 30.0 {
                    let dot = shell::galley(ui, " · ", theme::regular(theme::BODY), theme::faint());
                    let dot_width = dot.size().x;
                    shell::paint_line(ui, dot, left + width, y);
                    let second = shell::elided(
                        ui,
                        aside.clone(),
                        theme::regular(theme::BODY),
                        theme::muted(),
                        room - width - dot_width,
                    );
                    shell::paint_line(ui, second, left + width + dot_width, y);
                }
            }
            Cell::Value { text, dim, .. } => {
                let galley = shell::elided(
                    ui,
                    text.clone(),
                    theme::regular(theme::BODY),
                    if *dim {
                        theme::muted()
                    } else {
                        theme::text_soft()
                    },
                    room,
                );
                shell::paint_line(ui, galley, left, y);
            }
            Cell::Chain { chain, .. } => {
                if visible {
                    let clip = Rect::from_min_max(
                        Pos2::new(left, rect.top()),
                        Pos2::new(rect.right() - 4.0, rect.bottom()),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(clip)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    child.set_clip_rect(clip.intersect(ui.clip_rect()));
                    shell::mini_chain_sized(&mut child, chain, 12.0, 6.0);
                }
            }
            Cell::Stars { rating, editable } => {
                let under =
                    paint_stars(ui, y, left, *rating, if *editable { pointer } else { None });
                if *editable {
                    if let Some(star) = under {
                        if whole.clicked() {
                            // The rating it has already, clicked again, is
                            // taken away: the way to say "not rated".
                            let rating = if star == *rating { 0 } else { star };
                            self.did.rated = Some((row, rating));
                            claimed = true;
                        }
                    }
                }
            }
        }

        let response = if matches!(content, Cell::Stars { editable: true, .. }) {
            whole.on_hover_text("Click a star to rate it; click its rating again to clear it")
        } else {
            whole
        };

        // A click the contents took is not a click on the cell as well:
        // pressing Push sends a tone, and should not also pick the row.
        if response.clicked() && !claimed {
            // A click on a row that is already selected, in a column that can
            // be typed into, starts typing: the same gesture every file
            // manager uses to rename. Nothing is lost, because the click could
            // not have changed the selection anyway.
            let editable = self.grid.columns.get(col).is_some_and(|c| c.editable);
            if editable && self.grid.selected == Some(row) && !self.ctrl && !self.shift {
                self.did.edit = Some((row, col));
            } else {
                self.did.clicked = Some((row, self.ctrl, self.shift));
            }
        }
        if response.double_clicked() {
            self.did.double_clicked = Some(row);
        }
        if response.secondary_clicked() {
            self.did.context = Some(row);
        }
        if !self.grid.menu.is_empty() {
            let items = self.grid.menu.clone();
            response.context_menu(|ui| {
                theme::menu_width(ui, 220.0);
                for (n, item) in items.iter().enumerate() {
                    // What removes something is drawn as the destructive
                    // item, as menus here always draw it.
                    let destructive = item.starts_with("Delete") || item.starts_with("Remove");
                    let chosen = if destructive {
                        theme::menu_danger(ui, Icon::Remove, item).clicked()
                    } else {
                        theme::menu_item(ui, None, item, None).clicked()
                    };
                    if chosen {
                        self.did.chose = Some((row, n));
                    }
                }
            });
        }
    }

    fn default_row_height(&self) -> f32 {
        row_height(self.grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crash this guards against: a column wide enough to pass the ceiling
    /// made a range whose min exceeded its max, and egui panics clamping into
    /// one. A fills column on a wide window is exactly that.
    #[test]
    fn a_column_may_never_be_capped_below_the_width_it_needs() {
        for width in [0.0, 90.0, 198.0, 799.9, 800.0, 800.1, 1007.75, 4000.0] {
            assert!(
                drag_ceiling(width) >= width,
                "ceiling {} is below the width {width} it is meant to cap",
                drag_ceiling(width)
            );
        }
    }

    /// And it still stops an ordinary column from being dragged off the screen.
    #[test]
    fn an_ordinary_column_still_stops_at_the_usual_place() {
        assert_eq!(drag_ceiling(190.0), 800.0);
    }

    #[test]
    fn sorting_keeps_row_state_with_its_row() {
        let mut grid = Grid {
            rows: vec![
                vec![Cell::Text("Zulu".to_owned())],
                vec![Cell::Text("alpha".to_owned())],
                vec![Cell::Text("Mike".to_owned())],
            ],
            chosen: vec![true, false, false],
            selected: Some(2),
            editing: Some((0, 0)),
            sort: (0, true),
            ..Default::default()
        };

        assert_eq!(grid.sort_rows(), vec![1, 2, 0]);
        assert_eq!(grid.rows[0][0].key(), SortKey::Text("alpha".to_owned()));
        assert_eq!(grid.selected, Some(1));
        assert_eq!(grid.editing, Some((2, 0)));
        assert_eq!(grid.chosen, vec![false, false, true]);
    }

    /// Stars and names sort the way they read: by rating, and by name
    /// whatever tag follows it.
    #[test]
    fn stars_and_names_sort_the_way_they_read() {
        let mut grid = Grid {
            rows: vec![
                vec![Cell::Stars {
                    rating: 4,
                    editable: true,
                }],
                vec![Cell::Stars {
                    rating: 1,
                    editable: true,
                }],
                vec![Cell::Stars {
                    rating: 5,
                    editable: true,
                }],
            ],
            sort: (0, false),
            ..Default::default()
        };
        assert_eq!(grid.sort_rows(), vec![2, 0, 1], "highest first, descending");
        let name = Cell::Name {
            text: "Glass Wall".into(),
            tag: Some("PRO".into()),
        };
        assert_eq!(name.key(), SortKey::Text("glass wall".into()));
    }
}
