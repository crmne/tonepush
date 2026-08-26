//! The library's table, and the only one in the app.
//!
//! Both halves of the library are drawn by this: tones and setlists differ in
//! their columns and in nothing else, which is the point. They are one library
//! with two views of itself, and two tables that behaved differently would say
//! otherwise.
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

use egui::{RichText, Ui};

use crate::theme;

/// How far a cell's contents sit from its column's left edge.
const PADDING: f32 = 8.0;

/// How tall one row is, and the header with it. Public because a caller that
/// has to bound the table's height has to know what a row costs.
pub const ROW_HEIGHT: f32 = 22.0;

/// The width a table of these columns wants before it starts overlapping its
/// own headers: every column, the padding each one adds, and the scrollbar down
/// the right.
///
/// Public because a panel holding a table is the thing that gets dragged, and
/// it cannot know what it is holding unless the table says so. The setlist rail
/// had no floor and could be squeezed to a hundred pixels, where the headers
/// sat on top of each other and the buttons below wrapped a word to a line.
pub fn width_wanted(columns: &[Column]) -> f32 {
    columns.iter().map(|c| c.width).sum::<f32>() + PADDING * columns.len() as f32 + SCROLLBAR
}

/// What the table keeps clear down its right for the scrollbar.
const SCROLLBAR: f32 = 12.0;

/// What a cell holds.
pub enum Cell {
    /// Ordinary text.
    Text(String),
    /// Text in the second voice: derived, not typed, and not editable.
    Dim(String),
    /// Read-only text whose sort key is not its presentation (for example a
    /// comma-formatted download count or a shortened ISO timestamp).
    Value {
        text: String,
        key: String,
        dim: bool,
    },
    /// Where a tone is: one icon per place, each its own button.
    Places(Vec<(theme::Icon, theme::Sync, &'static str)>),
    /// The same small chip the chain paints on a block: FS1, EXP2, MIDI. The
    /// long name is on hover, because the short one is the one you learn.
    Tag {
        text: String,
        colour: egui::Color32,
        hover: String,
    },
    /// A number, drawn as the pedal draws one. Same widget, same gestures:
    /// drag to turn, click the reading to type it. A row of these needs a
    /// taller row than a row of words, which is what `Grid::row_height` is for.
    Knob {
        value: f32,
        range: std::ops::RangeInclusive<f32>,
        /// The reading, formatted the way that parameter is formatted
        /// everywhere else.
        text: String,
        /// What this end is, in the row's own words. A column header can only
        /// say one thing for every row under it, and what these two ends are
        /// called depends on what moves them.
        hover: String,
    },
    /// A whole number that is an address rather than a quantity: a MIDI CC.
    ///
    /// Drag it or click to type it, like a knob, but written to the device
    /// once at the end rather than streamed as it moves. A knob's value is a
    /// sound you are listening to while you turn it; every number a CC passes
    /// through on the way to 42 is meaningless, and each one costs a document
    /// read.
    Number {
        value: i64,
        range: std::ops::RangeInclusive<i64>,
        hover: &'static str,
    },
}

impl Cell {
    /// What this cell sorts on. A dot sorts by its state, so clicking that
    /// header gathers everything that needs doing.
    fn key(&self) -> String {
        match self {
            Cell::Text(t) | Cell::Dim(t) | Cell::Tag { text: t, .. } => t.to_lowercase(),
            Cell::Value { key, .. } => key.clone(),
            // Padded so 9 sorts before 10, which a plain string does not.
            Cell::Knob { value, .. } => format!("{value:020.6}"),
            Cell::Number { value, .. } => format!("{value:020}"),
            // Sorted so everything with something to do gathers at the top.
            Cell::Places(places) => places
                .iter()
                .map(|(_, state, _)| match state {
                    theme::Sync::Working => '0',
                    theme::Sync::Differs => '1',
                    theme::Sync::Absent => '2',
                    theme::Sync::Same => '3',
                    theme::Sync::Unknown => '4',
                })
                .collect(),
        }
    }
}

/// A column: what it is called, how wide, and whether its cells can be typed
/// into.
pub struct Column {
    /// Owned rather than static: a header can depend on what is in the column.
    /// The two ends of an assignment are a Min and a Max under an expression
    /// pedal and an Off and an On under a footswitch, and they are the same
    /// column.
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
    /// Said instead of an empty grid of rows. The headers still show, because a
    /// table with no rows should still say what it would hold.
    pub nothing_yet: &'static str,
    /// How tall a row is. Zero means [`ROW_HEIGHT`], which is a line of text; a
    /// table with knobs in it needs the room a knob takes.
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
    /// Start typing in this cell.
    pub edit: Option<(usize, usize)>,
    /// Finish typing: keep it, or throw it away.
    pub committed: bool,
    pub cancelled: bool,
    pub sort: Option<usize>,
    /// A right-click, and which item of the menu it ended on.
    pub context: Option<usize>,
    pub chose: Option<(usize, usize)>,
    /// A knob cell was turned: which cell, and what it now reads.
    pub turned: Option<(usize, usize, f32)>,
    /// A number cell was changed: which cell, what it now reads, and whether
    /// the person has finished with it - let go of the drag, or left the field.
    /// Every step is reported so the cell can be redrawn where it has been
    /// dragged to; only a finished one is worth sending anywhere.
    pub numbered: Option<(usize, usize, i64, bool)>,
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
        draw_headers(ui, grid);
        ui.add_space(10.0);
        ui.label(RichText::new(grid.nothing_yet).color(theme::DIM));
        return Did::default();
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
    egui_table::Table::new()
        .id_salt(id)
        .num_rows(delegate.grid.rows.len() as u64)
        .columns(columns)
        .num_sticky_cols(delegate.grid.sticky)
        .headers([egui_table::HeaderRow::new(ROW_HEIGHT)])
        .show(ui, &mut delegate);
    delegate.did
}

/// How tall this table's rows are: what it asked for, or a line of text.
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

fn row_height(grid: &Grid) -> f32 {
    if grid.row_height > 0.0 {
        grid.row_height
    } else {
        ROW_HEIGHT
    }
}

/// The header row on its own, for when there are no rows to hang it above.
fn draw_headers(ui: &mut Ui, grid: &Grid) {
    ui.horizontal(|ui| {
        for (i, column) in grid.columns.iter().enumerate() {
            let width = if column.fills {
                ui.available_width()
            } else {
                column.width
            };
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::hover());
            if ui.is_rect_visible(rect) && !column.title.is_empty() {
                ui.painter().text(
                    rect.left_center() + egui::vec2(PADDING, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &column.title,
                    egui::TextStyle::Body.resolve(ui.style()),
                    if i == grid.sort.0 {
                        theme::ACCENT
                    } else {
                        theme::TEXT
                    },
                );
            }
        }
    });
    ui.separator();
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
        let arrow = match (index == sorting, ascending) {
            (true, true) => " ↑",
            (true, false) => " ↓",
            _ => "",
        };
        let size = ui.available_size();
        let (rect, hit) = ui.allocate_exact_size(size, egui::Sense::click());
        if ui.is_rect_visible(rect) {
            if hit.hovered() {
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(3),
                    egui::Color32::from_rgb(0x25, 0x29, 0x31),
                );
            }
            ui.painter().text(
                rect.left_center() + egui::vec2(PADDING, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{}{arrow}", column.title),
                egui::TextStyle::Body.resolve(ui.style()),
                if index == sorting {
                    theme::ACCENT
                } else {
                    theme::TEXT
                },
            );
        }
        if hit.on_hover_text("sort by this column").clicked() {
            self.did.sort = Some(index);
        }
    }

    fn cell_ui(&mut self, ui: &mut Ui, cell: &egui_table::CellInfo) {
        let row = cell.row_nr as usize;
        let col = cell.col_nr;
        if !ui.is_sizing_pass() && ui.clip_rect().intersect(ui.max_rect()).is_positive() {
            self.did.last_visible = Some(self.did.last_visible.map_or(row, |last| last.max(row)));
        }
        let picked =
            self.grid.chosen.get(row).copied().unwrap_or(false) || self.grid.selected == Some(row);
        // Striping and selection are painted here rather than by the table:
        // egui_table draws cells, and the row is the thing a person sees.
        let background = if picked {
            Some(ui.visuals().selection.bg_fill)
        } else if row % 2 == 1 {
            Some(egui::Color32::from_rgb(0x1e, 0x21, 0x28))
        } else {
            None
        };
        if let Some(fill) = background {
            ui.painter()
                .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, fill);
        }

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
            if ui.is_sizing_pass() || !ui.clip_rect().intersect(ui.max_rect()).is_positive() {
                ui.label(self.grid.draft.clone());
                return;
            }
            // No cell-wide click target while a cell is a field: the field is
            // the cell, and a second widget over it takes the keyboard focus it
            // is asking for.
            let mut shown = egui::TextEdit::singleline(&mut self.grid.draft)
                .id(self.id.with(("editing", row, col)))
                .desired_width(f32::INFINITY)
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
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.did.cancelled = true;
            } else if field.lost_focus() {
                self.did.committed = true;
            }
            return;
        }

        // The whole cell answers, not just the words in it. Without this a
        // right-click landed only on the text, which in a row tall enough to
        // hold a knob is a sliver of it, and the row's menu looked broken.
        //
        // Claimed *before* the contents are drawn, which is the whole trick.
        // egui gives a tie to whichever widget was added last, so a target laid
        // over the cell afterwards quietly takes the clicks meant for what is
        // inside it: it is why pressing Push in the library only ever selected
        // the row, and why a CC could be dragged but never clicked to type.
        let whole = ui.interact(
            ui.max_rect(),
            ui.id().with(("cell", row, col)),
            egui::Sense::click(),
        );

        let Some(content) = self.grid.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        // Whether something inside the cell answered the click itself, so the
        // cell does not answer it a second time.
        let mut claimed = false;
        let response = match content {
            Cell::Places(places) => {
                let places = places.clone();
                // On the row's centre line, like the text beside them.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (n, (icon, state, hover)) in places.iter().enumerate() {
                        let hit = theme::place(ui, *icon, *state);
                        let hit = if hover.is_empty() {
                            hit
                        } else {
                            hit.on_hover_text(*hover)
                        };
                        if !matches!(state, theme::Sync::Unknown | theme::Sync::Working)
                            && hit.clicked()
                        {
                            self.did.place = Some((row, n));
                            claimed = true;
                        }
                    }
                })
                .response
            }
            Cell::Knob {
                value,
                range,
                text,
                hover,
            } => {
                // The pedal's own knob, with the pedal's own gestures: drag to
                // turn, click the reading to type it. A number that behaves one
                // way under the knobs and another way in a table is two things
                // to learn for one job.
                let (mut turned, range) = (*value, range.clone());
                let (text, hover) = (text.clone(), hover.clone());
                let mut moved = None;
                // Exactly as tall as the knob and its reading, so the row's own
                // centre alignment places it: a cell drawn from the top of a
                // row tall enough for a knob floats above the words in the
                // columns either side of it.
                let tall = theme::KNOB
                    + ui.spacing().item_spacing.y
                    + ui.text_style_height(&egui::TextStyle::Monospace);
                // The reading is the cell's click target, exactly as it is
                // under the knobs: the knob takes the drag, the number under it
                // takes the click that starts typing.
                let reading = ui
                    .allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), tall),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            let knob = theme::knob(ui, &mut turned, range)
                                .on_hover_text("drag to turn; Shift-drag for fine adjustment");
                            if knob.changed() {
                                moved = Some(turned);
                            }
                            ui.add(
                                egui::Label::new(
                                    RichText::new(text).monospace().color(theme::ACCENT),
                                )
                                .selectable(false)
                                .sense(egui::Sense::click()),
                            )
                        },
                    )
                    .inner;
                if let Some(turned) = moved {
                    self.did.turned = Some((row, col, turned));
                }
                reading.on_hover_text(format!(
                    "{hover}\ndrag the knob; Shift-drag for fine adjustment\nclick to type it"
                ))
            }
            Cell::Tag {
                text,
                colour,
                hover,
            } => {
                let (text, colour, hover) = (text.clone(), *colour, hover.clone());
                theme::tag(ui, &text, colour).on_hover_text(hover)
            }
            Cell::Number {
                value,
                range,
                hover,
            } => {
                let (mut number, range, hover) = (*value, range.clone(), *hover);
                // The field is the cell. A click here is for the number, never
                // for the row.
                claimed = true;
                let field = ui.add(
                    egui::DragValue::new(&mut number)
                        .speed(0.15)
                        .range(range)
                        .clamp_existing_to_range(true),
                );
                // Every step while it is being dragged, so the cell follows the
                // pointer, and the end of the drag as the one worth sending.
                if field.changed() {
                    self.did.numbered = Some((row, col, number, !field.dragged()));
                } else if field.drag_stopped() {
                    self.did.numbered = Some((row, col, number, true));
                }
                field.on_hover_text(hover)
            }
            Cell::Text(text) | Cell::Dim(text) => {
                let rich = if matches!(content, Cell::Dim(_)) {
                    RichText::new(text).color(theme::DIM)
                } else {
                    RichText::new(text)
                };
                ui.add(
                    // Not selectable. egui makes label text selectable by
                    // default, which puts an I-beam and a highlight on every
                    // cell: it reads as an edit field that refuses to be edited.
                    egui::Label::new(rich)
                        .selectable(false)
                        // Truncated, not wrapped: a wrapped name makes one row
                        // twice the height of its neighbours and the table
                        // ripples.
                        .truncate()
                        .sense(egui::Sense::click()),
                )
            }
            Cell::Value { text, dim, .. } => {
                let rich = if *dim {
                    RichText::new(text).color(theme::DIM)
                } else {
                    RichText::new(text)
                };
                ui.add(
                    egui::Label::new(rich)
                        .selectable(false)
                        .truncate()
                        .sense(egui::Sense::click()),
                )
            }
        };

        let response = response.union(whole);

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
                for (n, item) in items.iter().enumerate() {
                    if ui.button(item).clicked() {
                        self.did.chose = Some((row, n));
                        ui.close();
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

    /// The floor a panel puts under itself has to cover what it is holding.
    #[test]
    fn the_width_wanted_covers_every_column_and_its_padding() {
        let columns = vec![
            Column::new("Setlist", 120.0),
            Column::new("Venue", 90.0),
            Column::new("Date", 80.0),
            Column::new("#", 34.0),
        ];
        let bare: f32 = columns.iter().map(|c| c.width).sum();
        assert!(
            width_wanted(&columns) > bare,
            "a floor that is only the sum of the columns leaves no room for \
             the padding each one adds, nor for the scrollbar"
        );
        assert_eq!(width_wanted(&columns), 324.0 + 32.0 + 12.0);
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
        assert_eq!(grid.rows[0][0].key(), "alpha");
        assert_eq!(grid.selected, Some(1));
        assert_eq!(grid.editing, Some((2, 0)));
        assert_eq!(grid.chosen, vec![false, false, true]);
    }
}
