//! Putting tones in the pedal's slots: one, or several in a run from the
//! slot chosen. A write to an empty slot just happens; one that replaces
//! something asks once, beside the slot for one and in the middle of the
//! window for several, naming what it replaces and what keeps it. On a
//! StompStation PRO without a checked backup the question offers to take
//! one first; on firmware TonePush has not verified, nothing is written.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::library;
use crate::session::Cmd;
use crate::shell;
use crate::theme::{self, Icon};
use crate::App;

/// How long the deck says what was written, in seconds.
const WROTE_FOR: f32 = 8.0;

/// What is on its way to the pedal's slots: the presets are its
/// destinations until one is chosen.
#[derive(Clone, Debug)]
pub(crate) struct Sending {
    /// The tones, in the order chosen: hash and name.
    pub tones: Vec<(String, String)>,
}

impl Sending {
    #[cfg(test)]
    pub(crate) fn one(hash: String, name: String) -> Sending {
        Sending {
            tones: vec![(hash, name)],
        }
    }

    /// What the picking card calls it: the tone's name, or "3 tones".
    pub(crate) fn words(&self) -> String {
        match self.tones.as_slice() {
            [(_, name)] => name.clone(),
            tones => format!("{} tones", tones.len()),
        }
    }

    /// Whether this tone is among those being sent.
    pub(crate) fn holds(&self, hash: &str) -> bool {
        self.tones.iter().any(|(held, _)| held == hash)
    }
}

/// One slot a put writes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Write {
    pub slot: i64,
    pub label: String,
    pub hash: String,
    pub name: String,
    /// What the slot holds now, when it holds something.
    pub replaces: Option<String>,
    /// The preset itself, for one copied from another slot: it is not a
    /// library tone to be read by its hash.
    pub bytes: Option<Vec<u8>>,
}

/// The question a put asks before it replaces anything.
#[derive(Clone, Debug)]
pub(crate) struct Asking {
    pub writes: Vec<Write>,
    /// The slot whose row one slot's question hangs beside.
    pub beside: Option<i64>,
    /// A StompStation PRO with no checked backup: the put takes one first.
    pub back_up_first: bool,
    /// A move: the slot it came from, emptied once it is written.
    pub clears: Option<i64>,
}

/// How the question was answered.
enum Answer {
    Put,
    Cancel,
}

/// "14C", "14C and 15A", "14C, 15A and 15B".
pub(crate) fn listed(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl App {
    /// Why nothing can be put in a slot now, in a few words.
    pub(crate) fn put_refusal(&self) -> Option<String> {
        if !self.pedal_online() {
            return Some("no pedal is connected".to_owned());
        }
        if self.pro_active() {
            if self.pro.updating() {
                return Some("waits for the firmware update".to_owned());
            }
            if let Some(firmware) = self.pro.read_only_firmware() {
                return Some(format!("read only on {firmware}"));
            }
        }
        None
    }

    /// Whether a put waits for a checked backup the pedal can take first.
    pub(crate) fn put_needs_backup(&self) -> bool {
        self.pro_active() && !self.pro.guarded()
    }

    /// The rows a put from this row takes: every chosen one, in the order
    /// the table draws them, when the row is one of several chosen; else
    /// the row alone.
    pub(crate) fn put_rows(&self, row: usize) -> Vec<usize> {
        let Some(entry) = self.lib_entries.get(row) else {
            return Vec::new();
        };
        if self.lib_chosen.len() > 1 && self.lib_chosen.contains(&entry.hash) {
            let mut rows: Vec<usize> = self
                .lib_order
                .iter()
                .copied()
                .filter(|&row| {
                    self.lib_entries
                        .get(row)
                        .is_some_and(|entry| self.lib_chosen.contains(&entry.hash))
                })
                .collect();
            if rows.is_empty() {
                rows.push(row);
            }
            return rows;
        }
        vec![row]
    }

    /// Start putting library tones in slots: the presets become their
    /// destinations. A tone this pedal cannot play is left out, and says
    /// why.
    pub(crate) fn start_putting(&mut self, rows: &[usize]) {
        if let Some(why) = self.put_refusal() {
            return self.problem(format!("Nothing can be put in a slot: {why}"));
        }
        let mut tones = Vec::new();
        let mut left_out = Vec::new();
        for &row in rows {
            let Some(entry) = self.lib_entries.get(row) else {
                continue;
            };
            let refusal = self.refusal_of(entry).or_else(|| {
                (!self.tone_kind_compatible(&entry.hash))
                    .then(|| format!("{} is for another kind of pedal.", entry.name))
            });
            match refusal {
                Some(why) => left_out.push(why),
                None => tones.push((entry.hash.clone(), entry.name.clone())),
            }
        }
        if let Some(why) = left_out.first() {
            if tones.is_empty() {
                return self.problem(why.clone());
            }
            self.note(format!("left out of the put: {}", left_out.join(" ")));
        }
        if !tones.is_empty() {
            self.sending = Some(Sending { tones });
        }
    }

    /// Every slot's name on the pedal connected, empty for an empty slot.
    fn slot_names(&self) -> Vec<String> {
        if self.pro_active() {
            return self
                .pro
                .pedal_slots()
                .map(|(_, names)| names)
                .unwrap_or_default();
        }
        (0..self.hx_total())
            .map(|index| {
                let name = self.presets.get(index).cloned().unwrap_or_default();
                if shell::hx_slot_is_empty(&name) {
                    String::new()
                } else {
                    name
                }
            })
            .collect()
    }

    /// The slots a put from `first` writes, one tone each in order, and
    /// what each holds now. A run that would go past the last slot stops
    /// there.
    pub(crate) fn plan_put(&self, first: i64, tones: &[(String, String)]) -> Vec<Write> {
        let names = self.slot_names();
        tones
            .iter()
            .enumerate()
            .filter_map(|(offset, (hash, name))| {
                let slot = first + offset as i64;
                let held = names.get(usize::try_from(slot).ok()?)?;
                Some(Write {
                    slot,
                    label: self.active_slot_label(slot),
                    hash: hash.clone(),
                    name: name.clone(),
                    replaces: (!held.trim().is_empty()).then(|| held.trim().to_owned()),
                    bytes: None,
                })
            })
            .collect()
    }

    /// The slot the pedal has loaded, as an index.
    fn loaded_slot_index(&self) -> Option<i64> {
        if self.pro_active() {
            self.pro.loaded_slot().map(|slot| slot as i64)
        } else {
            self.loaded_in_view()
        }
    }

    /// A slot was chosen for what is being sent: write it, or ask first
    /// when that replaces something.
    pub(crate) fn finish_sending(&mut self, slot: i64) {
        let Some(sending) = self.sending.take() else {
            return;
        };
        self.put_to(slot, sending.tones);
    }

    /// Put tones in a run of slots from `slot`: at once into empty ones,
    /// after the question when anything would be replaced.
    pub(crate) fn put_to(&mut self, slot: i64, tones: Vec<(String, String)>) {
        if let Some(why) = self.put_refusal() {
            return self.problem(format!("Nothing can be put in a slot: {why}"));
        }
        let sending = Sending { tones };
        let writes = self.plan_put(slot, &sending.tones);
        if writes.len() < sending.tones.len() {
            self.note(format!(
                "only {} of {} fit from {}: the pedal ends there",
                writes.len(),
                sending.tones.len(),
                self.active_slot_label(slot)
            ));
        }
        if writes.is_empty() {
            return;
        }
        let back_up_first = self.put_needs_backup();
        let loaded = self.loaded_slot_index();
        let replaces = writes
            .iter()
            .any(|write| write.replaces.is_some() || Some(write.slot) == loaded);
        if replaces || back_up_first {
            self.put_question = Some(Asking {
                writes,
                beside: (sending.tones.len() == 1).then_some(slot),
                back_up_first,
                clears: None,
            });
        } else {
            self.write_puts(writes);
        }
    }

    /// Put a preset's own bytes in a slot, copied from another: asking when
    /// it replaces something, and for a move, emptying the slot it came
    /// from once it is written.
    pub(crate) fn put_bytes(
        &mut self,
        slot: i64,
        name: String,
        bytes: Vec<u8>,
        clears: Option<i64>,
    ) {
        if let Some(why) = self.put_refusal() {
            return self.problem(format!("Nothing can be put in a slot: {why}"));
        }
        let names = self.slot_names();
        let Some(held) = usize::try_from(slot)
            .ok()
            .and_then(|index| names.get(index))
        else {
            return;
        };
        let write = Write {
            slot,
            label: self.active_slot_label(slot),
            hash: library::hash_of(&bytes),
            name,
            replaces: (!held.trim().is_empty()).then(|| held.trim().to_owned()),
            bytes: Some(bytes),
        };
        let back_up_first = self.put_needs_backup();
        let loaded = self.loaded_slot_index();
        if write.replaces.is_some() || clears.is_some() || back_up_first || Some(slot) == loaded {
            self.put_question = Some(Asking {
                writes: vec![write],
                beside: Some(slot),
                back_up_first,
                clears,
            });
        } else {
            self.write_puts(vec![write]);
        }
    }

    /// Write what was agreed: what an audition set aside comes back first,
    /// changes and all, and the writes go to the pedal's memory without
    /// touching the loaded buffer.
    pub(crate) fn write_puts(&mut self, writes: Vec<Write>) {
        self.write_puts_clearing(writes, None);
    }

    /// The same, emptying a slot after: the one a move came from.
    pub(crate) fn write_puts_clearing(&mut self, writes: Vec<Write>, clears: Option<i64>) {
        self.put_back();
        let mut items = Vec::new();
        for write in &writes {
            let Some(bytes) = write.bytes.clone().or_else(|| library::read(&write.hash)) else {
                self.problem(format!("{} is missing from the library", write.name));
                continue;
            };
            if !self.pro_active() && library::kind(&write.hash).as_deref() == Some("hlx") {
                // A portable tone is built on the pedal, not written: its
                // preview opens aimed at the slot, and its Load builds it.
                self.preview_hlx(&write.name, bytes);
                if let Some(preview) = self.preview.as_mut() {
                    preview.dest = write.slot;
                }
                continue;
            }
            items.push((write.slot, write.name.clone(), bytes));
        }
        if items.is_empty() {
            return;
        }
        let words = match items.as_slice() {
            [(slot, name, _)] => format!("Wrote {name} to {}", self.active_slot_label(*slot)),
            several => format!(
                "Wrote {} tones to {}",
                several.len(),
                listed(
                    &several
                        .iter()
                        .map(|(slot, ..)| self.active_slot_label(*slot))
                        .collect::<Vec<_>>()
                )
            ),
        };
        self.note(words.to_lowercase());
        if self.pro_active() {
            for (slot, name, bytes) in items {
                self.pro.send_tone(slot as usize, name, bytes);
            }
        } else {
            let mut writes: Vec<crate::session::SlotWrite> = items
                .into_iter()
                .map(|(slot, name, bytes)| (slot, Some((name, bytes))))
                .collect();
            if let Some(slot) = clears {
                writes.push((slot, None));
            }
            self.send(Cmd::PushSetlist(writes));
        }
        self.wrote = Some((words, std::time::Instant::now()));
    }

    /// Where a preset's row is in the sidebar this frame, if it shows.
    pub(crate) fn preset_row_rect(&self, slot: i64) -> Option<Rect> {
        if self.pro_active() {
            self.pro.row_rect(usize::try_from(slot).ok()?)
        } else {
            self.row_rects.get(&slot).copied()
        }
    }

    /// The slots the open question would write, for their rows to say so.
    pub(crate) fn put_targets(&self) -> Vec<i64> {
        self.put_question
            .as_ref()
            .map(|asking| asking.writes.iter().map(|write| write.slot).collect())
            .unwrap_or_default()
    }

    /// What the deck says once a put is written: "Wrote Slapback Twang to
    /// 05B", for a few seconds.
    pub(crate) fn wrote_note(&self) -> Option<String> {
        self.wrote
            .as_ref()
            .filter(|(_, at)| at.elapsed().as_secs_f32() < WROTE_FOR)
            .map(|(words, _)| words.clone())
    }

    /// A put waiting for the backup it asked for: written once the pedal is
    /// guarded, forgotten if it goes.
    pub(crate) fn settle_put_after_backup(&mut self) {
        if self.put_after_backup.is_none() {
            return;
        }
        if !self.pro_active() || !self.pro.is_online() {
            self.put_after_backup = None;
            return;
        }
        if self.pro.guarded() {
            if let Some(writes) = self.put_after_backup.take() {
                self.write_puts(writes);
            }
        }
    }

    /// When the backup that keeps what a write replaces was taken: "14:02".
    fn backup_time(&self) -> Option<String> {
        let time = if self.pro_active() {
            self.pro.backup_time()?
        } else {
            let captured = self.automatic_backup.as_ref()?.captured;
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(captured)
        };
        shell::clock(time).map(|(words, _)| words)
    }

    /// Whether the library holds what a slot has now.
    fn slot_kept(&self, slot: i64) -> bool {
        if self.pro_active() {
            let Some((hashes, names)) = self.pro.pedal_slots() else {
                return false;
            };
            let name = names
                .get(usize::try_from(slot).unwrap_or(usize::MAX))
                .cloned()
                .unwrap_or_default();
            return hashes
                .get(&slot)
                .is_some_and(|hash| self.library_lookup.sync(hash, &name) == theme::Sync::Same);
        }
        self.slot_sync(slot) == theme::Sync::Same
    }

    /// What becomes of the loaded preset, as the question says it.
    fn put_leaves(&self, writes: &[Write]) -> Option<(Icon, String, bool)> {
        if let Some(set_aside) = &self.hearing.set_aside {
            let changes = if set_aside.dirty {
                ", with its changes"
            } else {
                ""
            };
            return Some((
                Icon::Pedal,
                format!("{} comes back{changes}", set_aside.name),
                false,
            ));
        }
        let loaded = self.loaded_slot_index()?;
        let (name, dirty) = if self.pro_active() {
            (self.pro.loaded_name(), self.pro.is_dirty())
        } else {
            (self.preset_name.trim().to_owned(), self.dirty)
        };
        if name.is_empty() {
            return None;
        }
        if writes.iter().any(|write| write.slot == loaded) {
            return Some(if dirty {
                (
                    Icon::CircleAlert,
                    format!("{name}'s unsaved changes are lost"),
                    true,
                )
            } else {
                (
                    Icon::Pedal,
                    format!("{name} is replaced where it plays"),
                    false,
                )
            });
        }
        Some((
            Icon::Pedal,
            if dirty {
                format!("{name} keeps playing, with its changes")
            } else {
                format!("{name} keeps playing")
            },
            false,
        ))
    }

    /// The question a put asks: beside the slot for one write, centred for
    /// several. Enter puts, Esc and Cancel take it back.
    pub(crate) fn put_question_window(&mut self, ctx: &egui::Context) {
        let Some(asking) = self.put_question.clone() else {
            return;
        };
        let backup = self.backup_time();
        let leaves = self.put_leaves(&asking.writes);
        let enter = ctx.input(|input| input.key_pressed(egui::Key::Enter));
        let frame = egui::Frame::new()
            .fill(theme::panel())
            .stroke(Stroke::new(1.0, theme::line_strong()))
            .corner_radius(CornerRadius::same(theme::RADIUS_DIALOG))
            .shadow(egui::epaint::Shadow {
                offset: [0, 18],
                blur: 48,
                spread: 0,
                color: Color32::from_black_alpha(110),
            });
        let id = egui::Id::new("put-question");
        // Beside the slot's row while it shows; in the middle otherwise.
        let beside = asking.beside.and_then(|slot| self.preset_row_rect(slot));
        let (answer, close) = match beside {
            Some(beside) => {
                let area = egui::Area::new(id)
                    .kind(egui::UiKind::Modal)
                    .sense(Sense::hover())
                    .order(egui::Order::Foreground)
                    .interactable(true)
                    // Its title level with the row, pushed up where the
                    // window ends.
                    .pivot(Align2::LEFT_TOP)
                    .fixed_pos(Pos2::new(beside.right() + 14.0, beside.center().y - 24.0))
                    .constrain(true);
                let shown = egui::Modal::new(id)
                    .area(area)
                    .backdrop_color(Color32::TRANSPARENT)
                    .frame(frame)
                    .show(ctx, |ui| {
                        ui.set_width(340.0);
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let answer = self.one_put(ui, &asking, backup.as_deref(), leaves.as_ref());
                        // The arrow on the question's edge, at the slot.
                        let edge = ui.min_rect();
                        let y = beside
                            .center()
                            .y
                            .clamp(edge.top() + 22.0, edge.bottom() - 22.0);
                        let tip = Pos2::new(edge.left() - 8.0, y);
                        let painter = ui.painter().clone().with_clip_rect(ui.ctx().content_rect());
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                Pos2::new(edge.left() + 1.0, y - 8.0),
                                tip,
                                Pos2::new(edge.left() + 1.0, y + 8.0),
                            ],
                            theme::panel(),
                            Stroke::NONE,
                        ));
                        painter.line_segment(
                            [Pos2::new(edge.left(), y - 8.0), tip],
                            Stroke::new(1.0, theme::line_strong()),
                        );
                        painter.line_segment(
                            [tip, Pos2::new(edge.left(), y + 8.0)],
                            Stroke::new(1.0, theme::line_strong()),
                        );
                        answer
                    });
                let close = shown.should_close();
                (shown.inner, close)
            }
            None => theme::dialog(ctx, "put-question-several", 500.0, |ui| {
                self.several_puts(ui, &asking, backup.as_deref(), leaves.as_ref())
            }),
        };
        let answer = match (answer, close, enter) {
            (Some(answer), ..) => Some(answer),
            (None, true, _) => Some(Answer::Cancel),
            (None, false, true) => Some(Answer::Put),
            _ => None,
        };
        match answer {
            Some(Answer::Put) => {
                self.put_question = None;
                if asking.back_up_first {
                    self.put_after_backup = Some(asking.writes);
                    self.pro.back_up_here();
                } else {
                    self.write_puts_clearing(asking.writes, asking.clears);
                }
            }
            Some(Answer::Cancel) => self.put_question = None,
            None => {}
        }
    }

    /// One slot's question, as sheet 09 draws it.
    fn one_put(
        &self,
        ui: &mut Ui,
        asking: &Asking,
        backup: Option<&str>,
        leaves: Option<&(Icon, String, bool)>,
    ) -> Option<Answer> {
        let write = asking.writes.first()?;
        let explanation = if asking.back_up_first {
            "This writes the pedal's memory, which waits for a checked backup of it: \
             TonePush takes one first, in about 40 seconds."
        } else {
            "This writes the pedal's memory, so it asks once."
        };
        theme::dialog_header(
            ui,
            &format!("Put {} in {}?", write.name, write.label),
            Some(explanation),
        );
        theme::dialog_body(ui, |ui| {
            fact_box(ui, |ui| {
                match &write.replaces {
                    Some(old) => {
                        fact(
                            ui,
                            Icon::Replace,
                            theme::hot(),
                            &[
                                ("Replaces ", false),
                                (old.as_str(), true),
                                (&format!(" in {}", write.label), false),
                            ],
                        );
                        let kept = self.slot_kept(write.slot);
                        let words = match (kept, backup) {
                            (true, Some(time)) => {
                                format!("{old} stays in your library and the {time} backup")
                            }
                            (false, Some(time)) => format!("{old} stays in the {time} backup"),
                            (true, None) => format!("{old} stays in your library"),
                            (false, None) => format!("{old} is not kept anywhere else"),
                        };
                        let safe = kept || backup.is_some();
                        fact(
                            ui,
                            if safe {
                                Icon::ShieldCheck
                            } else {
                                Icon::ShieldAlert
                            },
                            if safe { theme::ok() } else { theme::hot() },
                            &[(words.as_str(), false)],
                        );
                    }
                    None => fact(
                        ui,
                        Icon::CircleDashed,
                        theme::muted(),
                        &[(write.label.as_str(), true), (" is empty", false)],
                    ),
                }
                if let Some(from) = asking.clears {
                    let label = self.active_slot_label(from);
                    fact(
                        ui,
                        Icon::ArrowDownToLine,
                        theme::muted(),
                        &[
                            ("A move: ", false),
                            (label.as_str(), true),
                            (" is left empty once it is written", false),
                        ],
                    );
                }
                if let Some((icon, words, hot)) = leaves {
                    fact(
                        ui,
                        *icon,
                        if *hot { theme::hot() } else { theme::muted() },
                        &[(words.as_str(), false)],
                    );
                }
            });
        });
        let mut answer = None;
        theme::dialog_footer(ui, "", |ui| {
            let put = if asking.back_up_first {
                theme::Button::new(&format!("Back up, then put it in {}", write.label))
                    .primary()
                    .show(ui)
            } else if write.replaces.is_some() {
                theme::Button::new(&format!("Replace {}", write.label))
                    .danger()
                    .hint("Enter")
                    .show(ui)
            } else {
                theme::Button::new(&format!("Put it in {}", write.label))
                    .primary()
                    .hint("Enter")
                    .show(ui)
            };
            if put.clicked() {
                answer = Some(Answer::Put);
            }
            if theme::Button::new("Cancel").show(ui).clicked() {
                answer = Some(Answer::Cancel);
            }
        });
        answer
    }

    /// Several slots' question, as sheet 12 draws it.
    fn several_puts(
        &self,
        ui: &mut Ui,
        asking: &Asking,
        backup: Option<&str>,
        leaves: Option<&(Icon, String, bool)>,
    ) -> Option<Answer> {
        let labels: Vec<String> = asking.writes.iter().map(|w| w.label.clone()).collect();
        theme::dialog_header(
            ui,
            &format!("Put {} tones in {}?", asking.writes.len(), listed(&labels)),
            Some(if asking.back_up_first {
                "They go in the order you chose them, one slot each, once TonePush has taken \
                 a checked backup of the pedal, in about 40 seconds."
            } else {
                "They go in the order you chose them, one slot each, from the slot you chose."
            }),
        );
        let replaced: Vec<String> = asking
            .writes
            .iter()
            .filter_map(|write| write.replaces.clone())
            .collect();
        theme::dialog_body(ui, |ui| {
            fact_box(ui, |ui| {
                for write in &asking.writes {
                    ui.horizontal(|ui| {
                        ui.set_min_height(34.0);
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.add_space(14.0);
                        let (spot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        let (icon, ink) = if write.replaces.is_some() {
                            (Icon::Replace, theme::hot())
                        } else {
                            (Icon::CircleDashed, theme::muted())
                        };
                        theme::paint_icon(ui, icon, spot.center(), 14.0, ink);
                        shell::slot_chip(ui, &write.label);
                        let (name_spot, _) =
                            ui.allocate_exact_size(Vec2::new(120.0, 18.0), Sense::hover());
                        let name = shell::elided(
                            ui,
                            write.name.clone(),
                            theme::semibold(12.5),
                            theme::text(),
                            name_spot.width(),
                        );
                        shell::paint_line(ui, name, name_spot.left(), name_spot.center().y);
                        match &write.replaces {
                            Some(old) => {
                                paint_runs(ui, &[("replaces ", false), (old.as_str(), true)])
                            }
                            None => paint_runs(ui, &[("an empty slot", false)]),
                        }
                    });
                }
            });
            ui.add_space(4.0);
            let mut note = String::new();
            if !replaced.is_empty() {
                let what = listed(&replaced);
                let verb = if replaced.len() == 1 { "stays" } else { "stay" };
                note = match backup {
                    Some(time) => format!("{what} {verb} in the {time} backup."),
                    None => format!("{what} {verb} only in your library, if it holds them."),
                };
            }
            if let Some((_, words, _)) = leaves {
                if !note.is_empty() {
                    note.push(' ');
                }
                note.push_str(words);
                note.push('.');
            }
            if !note.is_empty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (spot, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    theme::paint_icon(ui, Icon::ShieldCheck, spot.center(), 13.0, theme::muted());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(note)
                                .font(theme::regular(12.5))
                                .color(theme::text_soft()),
                        )
                        .wrap(),
                    );
                });
            }
        });
        let count = asking.writes.len();
        let mut answer = None;
        theme::dialog_footer(
            ui,
            &format!("{} writes to the pedal's memory.", words_for(count)),
            |ui| {
                let label = format!("Write {count} slots");
                let put = if asking.back_up_first {
                    theme::Button::new("Back up, then write them")
                        .primary()
                        .show(ui)
                } else if replaced.is_empty() {
                    theme::Button::new(&label).primary().show(ui)
                } else {
                    theme::Button::new(&label).danger().show(ui)
                };
                if put.clicked() {
                    answer = Some(Answer::Put);
                }
                if theme::Button::new("Cancel").show(ui).clicked() {
                    answer = Some(Answer::Cancel);
                }
            },
        );
        answer
    }
}

/// A small count in words: "Three".
fn words_for(count: usize) -> String {
    const WORDS: [&str; 10] = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
    ];
    WORDS
        .get(count)
        .map_or_else(|| count.to_string(), |word| (*word).to_owned())
}

/// The framed list the question's facts sit in.
fn fact_box(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::bg())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            contents(ui);
        });
}

/// One fact: an icon in its column, then plain and bold words, wrapped.
fn fact(ui: &mut Ui, icon: Icon, ink: Color32, runs: &[(&str, bool)]) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.add_space(14.0);
        let (spot, _) = ui.allocate_exact_size(Vec2::new(16.0, 34.0), Sense::hover());
        theme::paint_icon(
            ui,
            icon,
            Pos2::new(spot.center().x, spot.top() + 17.0),
            14.0,
            ink,
        );
        ui.vertical(|ui| {
            ui.add_space(8.0);
            let mut job = egui::text::LayoutJob::default();
            for (text, bold) in runs {
                job.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        font_id: if *bold {
                            theme::semibold(12.5)
                        } else {
                            theme::regular(12.5)
                        },
                        color: if *bold {
                            theme::text()
                        } else {
                            theme::text_soft()
                        },
                        ..Default::default()
                    },
                );
            }
            job.wrap.max_width = (ui.available_width() - 14.0).max(40.0);
            let galley = ui.painter().layout_job(job);
            let (place, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
            ui.painter().galley(place.min, galley, Color32::PLACEHOLDER);
            ui.add_space(8.0);
        });
    });
}

/// Plain and bold words on one line.
fn paint_runs(ui: &mut Ui, runs: &[(&str, bool)]) {
    let mut job = egui::text::LayoutJob::default();
    for (text, bold) in runs {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: if *bold {
                    theme::semibold(12.5)
                } else {
                    theme::regular(12.5)
                },
                color: if *bold {
                    theme::text()
                } else {
                    theme::text_soft()
                },
                ..Default::default()
            },
        );
    }
    let galley = ui.painter().layout_job(job);
    let (place, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(place.min, galley, Color32::PLACEHOLDER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Marker;
    use crate::session::Evt;
    use crate::{Connection, LibEntry};
    use std::sync::mpsc;

    fn app() -> (App, mpsc::Sender<Evt>, mpsc::Receiver<Cmd>) {
        let (to_device, cmds) = mpsc::channel();
        let (events, from_device) = mpsc::channel();
        let app = App::new(&egui::Context::default(), to_device, from_device);
        let _ = cmds.try_iter().count();
        (app, events, cmds)
    }

    /// An HX Stomp with 01B Plexi Crunch loaded and edited; 05B holds
    /// Chime Clean, 05C and everything after it is empty, and the pedal
    /// ends at 06A.
    fn online(app: &mut App) {
        app.connection = Connection::Online;
        app.device = "HX Stomp".into();
        app.firmware = "3.80".into();
        app.preset_index = 1;
        app.preset_name = "Plexi Crunch".into();
        app.dirty = true;
        app.presets = [
            "Glass Clean",
            "Plexi Crunch",
            "Brown Lead",
            "Edge of Breakup",
            "Ambient Swell",
            "Slapback Twang",
            "Doom Fuzz",
            "Worship Pad",
            "Funk Rhythm",
            "Velvet Lead",
            "Tape Echo Clean",
            "Octave Fuzz",
            "Shimmer Pad",
            "Chime Clean",
            "New Preset",
            "New Preset",
        ]
        .map(str::to_owned)
        .to_vec();
    }

    fn tone(name: &str) -> LibEntry {
        let hash = library::store(name, name.as_bytes(), "hxpreset").expect("the scratch library");
        LibEntry {
            hash: hash.clone(),
            series: hash,
            name: name.to_owned(),
            line: String::new(),
            meta: library::Meta::default(),
            added_at: String::new(),
            modified_at: String::new(),
            downloads: None,
            rating: None,
            version: 1,
            versions: 1,
            chain: Vec::new(),
            pro: false,
            marker: Some(Marker::hx("Stomp")),
            firmware: "3.80".to_owned(),
        }
    }

    fn written(cmds: &mpsc::Receiver<Cmd>) -> Vec<(i64, String)> {
        cmds.try_iter()
            .filter_map(|cmd| match cmd {
                Cmd::PushSetlist(writes) => Some(writes),
                _ => None,
            })
            .flatten()
            .filter_map(|(slot, tone)| tone.map(|(name, _)| (slot, name)))
            .collect()
    }

    /// Several tones go in a run from the slot chosen, one each, saying
    /// what each slot holds now; the run stops where the pedal ends.
    #[test]
    fn a_put_of_several_runs_from_the_slot_chosen() {
        let _scratch = library::tests::Scratch::new("put-plan");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        let tones: Vec<(String, String)> = ["Brown Lead", "Doom Fuzz", "Dream Pop"]
            .iter()
            .map(|name| (format!("{name}-hash"), (*name).to_owned()))
            .collect();

        let writes = app.plan_put(13, &tones);
        let said: Vec<(&str, &str, Option<&str>)> = writes
            .iter()
            .map(|w| (w.label.as_str(), w.name.as_str(), w.replaces.as_deref()))
            .collect();
        assert_eq!(
            said,
            [
                ("05B", "Brown Lead", Some("Chime Clean")),
                ("05C", "Doom Fuzz", None),
                ("06A", "Dream Pop", None),
            ]
        );
        assert_eq!(app.plan_put(15, &tones).len(), 1, "the pedal ends at 06A");
    }

    /// An empty slot is written at once; a slot that holds something asks
    /// first, and nothing goes until the answer.
    #[test]
    fn only_a_write_that_replaces_something_asks() {
        let _scratch = library::tests::Scratch::new("put-ask");
        let (mut app, _events, cmds) = app();
        online(&mut app);
        app.lib_entries = vec![tone("Slapback Twang")];

        app.start_sending(0);
        app.finish_sending(14);
        assert!(app.put_question.is_none());
        assert_eq!(written(&cmds), [(14, "Slapback Twang".to_owned())]);
        assert_eq!(
            app.wrote_note().as_deref(),
            Some("Wrote Slapback Twang to 05C")
        );

        app.start_sending(0);
        app.finish_sending(13);
        let asking = app.put_question.clone().expect("it asks");
        assert_eq!(asking.writes[0].replaces.as_deref(), Some("Chime Clean"));
        assert!(written(&cmds).is_empty(), "nothing goes before the answer");
        let writes = asking.writes;
        app.put_question = None;
        app.write_puts(writes);
        assert_eq!(written(&cmds), [(13, "Slapback Twang".to_owned())]);
    }

    /// Writing the loaded preset's own slot asks, whatever it holds: what
    /// plays there is replaced.
    #[test]
    fn the_loaded_slot_always_asks() {
        let _scratch = library::tests::Scratch::new("put-loaded");
        let (mut app, _events, cmds) = app();
        online(&mut app);
        app.presets[1] = "New Preset".into();
        app.lib_entries = vec![tone("Slapback Twang")];
        app.start_sending(0);
        app.finish_sending(1);
        assert!(app.put_question.is_some());
        assert!(written(&cmds).is_empty());
    }

    /// A put while a tone is auditioned puts the audition back first, so
    /// the write never lands on the buffer that was set aside.
    #[test]
    fn a_put_during_an_audition_puts_it_back_first() {
        let _scratch = library::tests::Scratch::new("put-audition");
        let (mut app, events, cmds) = app();
        online(&mut app);
        app.lib_entries = vec![tone("Dream Pop"), tone("Slapback Twang")];
        app.audition_library(0);
        let key = app.hearing.heard.as_ref().map(|heard| heard.key).unwrap();
        events.send(Evt::Auditioning(Some(key))).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();

        app.start_sending(1);
        app.finish_sending(14);
        let sent: Vec<Cmd> = cmds.try_iter().collect();
        assert!(matches!(sent.first(), Some(Cmd::EndAudition)));
        assert!(matches!(sent.get(1), Some(Cmd::PushSetlist(_))));
    }

    /// A put from one of several chosen tones takes them all, in the order
    /// the table draws them.
    #[test]
    fn a_put_from_a_chosen_row_takes_every_chosen_one() {
        let _scratch = library::tests::Scratch::new("put-rows");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        app.lib_entries = vec![tone("Brown Lead"), tone("Doom Fuzz"), tone("Dream Pop")];
        app.lib_order = vec![2, 0, 1];
        app.lib_chosen = [0, 2]
            .iter()
            .map(|&row| app.lib_entries[row].hash.clone())
            .collect();
        assert_eq!(app.put_rows(0), [2, 0]);
        assert_eq!(app.put_rows(1), [1], "a row not chosen goes alone");

        app.start_sending(0);
        let sending = app.sending.clone().expect("sending");
        assert_eq!(sending.words(), "2 tones");
    }
}
