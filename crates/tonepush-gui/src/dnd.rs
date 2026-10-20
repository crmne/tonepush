//! Drag and drop. Every drag is a shortcut: what a drop does is what a menu
//! item or a key does too, and both ask [`App::drop_outcome`] what that is,
//! so the words on the ghost, the row under the pointer and the tabs cannot
//! drift from the act. A drop that writes the pedal's memory goes through
//! the put question, which asks when it replaces something.
//!
//! Sources set an egui payload when their drag starts; places that take a
//! drop register their rects while they are drawn; and at the end of the
//! frame the target under the pointer is found, the ghost painted beside
//! the pointer, and a release carries the drop out.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

use crate::library;
use crate::shell;
use crate::theme::{self, Icon};
use crate::{App, CloudAction, LibraryView};

/// What is being dragged.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Dragged {
    /// Library tones, in the order chosen: hash and name.
    Tones(Vec<(String, String)>),
    /// A TonePush tone, by its row in the Cloud.
    Cloud(usize),
    /// A preset on the pedal, by slot.
    Preset(i64),
    /// What plays on the pedal now: the deck's name.
    Playing,
    /// A setlist, by its place in the library's list.
    Setlist(usize),
    /// One slot of a setlist.
    SetlistSlot { setlist: usize, slot: usize },
    /// The whole pedal: the Presets heading.
    Pedal,
    /// Tone files dragged in from outside the window.
    Files(Vec<std::path::PathBuf>),
}

/// Whether a file is a tone TonePush reads, by its extension.
fn tone_file(path: &std::path::Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "hxpreset" => Some("hxpreset"),
        "hlx" => Some("hlx"),
        "vxpreset" => Some("vxpreset"),
        _ => None,
    }
}

/// Where something can be dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A preset in the sidebar.
    Slot(i64),
    /// The presets as a whole, for a setlist.
    Presets,
    /// The board: what is dropped there plays.
    Board,
    /// One of the library pane's tabs, folded or not.
    Tab(LibraryView),
    /// A setlist's slot, on its page.
    SetlistSlot { setlist: usize, slot: usize },
    /// A setlist's card.
    Setlist(usize),
}

/// What a drop would do, in the ghost's words, or why it would not.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Outcome {
    pub words: String,
    pub icon: Icon,
    /// It would not: the words say why.
    pub refused: bool,
}

impl Outcome {
    fn does(icon: Icon, words: impl Into<String>) -> Option<Outcome> {
        Some(Outcome {
            words: words.into(),
            icon,
            refused: false,
        })
    }

    fn refuses(words: impl Into<String>) -> Option<Outcome> {
        Some(Outcome {
            words: words.into(),
            icon: Icon::Ban,
            refused: true,
        })
    }
}

/// The drop state the app keeps between frames.
#[derive(Default)]
pub(crate) struct Drops {
    /// The places that take a drop this frame, in the order drawn.
    zones: Vec<(Rect, Target)>,
    /// The target under the pointer last frame, for the places to mark it.
    pub hover: Option<Target>,
}

/// "14C", "14C and 15A", "14C, 15A and 15B".
fn listed(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl App {
    /// What is being dragged, if anything.
    pub(crate) fn dragged(&self, ctx: &egui::Context) -> Option<Dragged> {
        if let Some(payload) = egui::DragAndDrop::payload::<Dragged>(ctx) {
            return Some((*payload).clone());
        }
        // Files from outside: only tones are this module's to place.
        let files: Vec<std::path::PathBuf> = ctx.input(|input| {
            input
                .raw
                .hovered_files
                .iter()
                .filter_map(|file| file.path.clone())
                .filter(|path| tone_file(path).is_some())
                .collect()
        });
        (!files.is_empty()).then_some(Dragged::Files(files))
    }

    /// Start a drag.
    pub(crate) fn start_drag(&mut self, ctx: &egui::Context, dragged: Dragged) {
        egui::DragAndDrop::set_payload(ctx, dragged);
    }

    /// A place that takes a drop, drawn this frame.
    pub(crate) fn drop_zone(&mut self, rect: Rect, target: Target) {
        self.drops.zones.push((rect, target));
    }

    /// Whether what is being dragged would go to a slot, so the presets
    /// read as destinations.
    pub(crate) fn dropping_on_slots(&self, ctx: &egui::Context) -> bool {
        let Some(dragged) = self.dragged(ctx) else {
            return false;
        };
        if matches!(
            dragged,
            Dragged::Setlist(_) | Dragged::Pedal | Dragged::Playing
        ) {
            return false;
        }
        // Another slot than the loaded one, for a preset dragged from it.
        let slot = match dragged {
            Dragged::Preset(from) => (from + 1) % self.slot_count().max(1),
            _ => self.loaded_slot_any().unwrap_or(0),
        };
        matches!(
            self.drop_outcome(&dragged, Target::Slot(slot), false),
            Some(Outcome { refused: false, .. })
        )
    }

    /// How many slots the pedal has.
    fn slot_count(&self) -> i64 {
        if self.pro_active() {
            self.pro.preset_count() as i64
        } else {
            self.hx_total() as i64
        }
    }

    /// Any slot, for asking whether a drag goes to slots at all.
    fn loaded_slot_any(&self) -> Option<i64> {
        if self.pro_active() {
            self.pro.loaded_slot().map(|slot| slot as i64)
        } else {
            self.loaded_in_view()
        }
    }

    /// The tones a library drag carries, from a row: the chosen ones with it.
    pub(crate) fn dragged_tones(&self, row: usize) -> Dragged {
        Dragged::Tones(
            self.put_rows(row)
                .into_iter()
                .filter_map(|row| self.lib_entries.get(row))
                .map(|entry| (entry.hash.clone(), entry.name.clone()))
                .collect(),
        )
    }

    /// What a drop of `dragged` on `target` would do, or why not; nothing
    /// when the target does not take that kind of drop at all. `moving` is
    /// the modifier that turns a copy between presets into a move.
    pub(crate) fn drop_outcome(
        &self,
        dragged: &Dragged,
        target: Target,
        moving: bool,
    ) -> Option<Outcome> {
        match (dragged, target) {
            (Dragged::Tones(tones), Target::Slot(slot)) => self.tones_to_slot(tones, slot),
            (Dragged::Tones(tones), Target::Board) => {
                let (hash, name) = tones.first()?;
                self.plays_here(hash, name)
            }
            (Dragged::Tones(tones), Target::Tab(LibraryView::Cloud)) => {
                if tones.len() > 1 {
                    Outcome::refuses("One tone at a time goes to TonePush")
                } else if self.config.token.is_none() {
                    Outcome::refuses("Sign in to TonePush to publish")
                } else {
                    Outcome::does(Icon::CloudUpload, "Publish on TonePush")
                }
            }
            (Dragged::Tones(tones), Target::SetlistSlot { setlist, slot }) => {
                let (_, list) = self.lib_setlists.get(setlist)?;
                let first = self.active_slot_label(slot as i64);
                let words = if tones.len() == 1 {
                    format!("Put it in {}'s {first}", list.name)
                } else {
                    format!("Fill {} slots of {} from {first}", tones.len(), list.name)
                };
                Outcome::does(Icon::ArrowDownToLine, words)
            }
            (Dragged::Cloud(row), Target::Slot(slot)) => {
                let entry = self.cloud_entries.get(*row)?;
                if let Some(why) = self.cloud_audition_blocker(&entry.discovered) {
                    return Outcome::refuses(why);
                }
                let name = entry.discovered.tone.summary.name.clone();
                let mut outcome = self.tones_to_slot(&[(String::new(), name)], slot)?;
                if !outcome.refused {
                    outcome.words = format!("Keep it, then {}", lowercase_first(&outcome.words));
                }
                Some(outcome)
            }
            (Dragged::Cloud(_), Target::Tab(LibraryView::Tones)) => {
                Outcome::does(Icon::CloudDownload, "Keep in Tones")
            }
            (Dragged::Cloud(row), Target::Board) => {
                let entry = self.cloud_entries.get(*row)?;
                match self.cloud_audition_blocker(&entry.discovered) {
                    Some(why) => Outcome::refuses(why),
                    None => Outcome::does(Icon::Volume, "Play it on the pedal"),
                }
            }
            (Dragged::Preset(from), Target::Tab(LibraryView::Tones)) => {
                if self.pro_active() {
                    Outcome::refuses("Keep a StompStation PRO preset from its menu")
                } else {
                    let _ = from;
                    Outcome::does(Icon::Computer, "Keep in Tones")
                }
            }
            (Dragged::Preset(from), Target::Slot(slot)) => {
                if *from == slot {
                    return None;
                }
                if let Some(why) = self.put_refusal() {
                    return Outcome::refuses(why);
                }
                if self.pro_active() {
                    return Outcome::refuses(
                        "Copy and paste a StompStation PRO preset from its menu",
                    );
                }
                if self.slot_document(*from).is_none() {
                    return Outcome::refuses(format!(
                        "{} has changes not saved: save it first",
                        self.active_slot_label(*from)
                    ));
                }
                let verb = if moving { "Move" } else { "Copy" };
                let label = self.active_slot_label(slot);
                let held = self.held_in(slot);
                Outcome::does(
                    if moving {
                        Icon::ArrowDownToLine
                    } else {
                        Icon::Copy
                    },
                    match held {
                        Some(old) => format!("{verb} over {label} {old}"),
                        None => format!("{verb} to {label}"),
                    },
                )
            }
            (Dragged::Preset(_), Target::Tab(LibraryView::Cloud)) => {
                if self.pro_active() {
                    Outcome::refuses("Keep a StompStation PRO preset from its menu first")
                } else if self.config.token.is_none() {
                    Outcome::refuses("Sign in to TonePush to publish")
                } else {
                    Outcome::does(Icon::CloudUpload, "Keep it, then publish it")
                }
            }
            (Dragged::Playing, Target::Tab(LibraryView::Tones)) => {
                if self.hearing() {
                    Outcome::refuses("Keep or put back the audition first")
                } else if self.pro_active() {
                    Outcome::refuses("Keep a StompStation PRO preset from its menu")
                } else {
                    Outcome::does(Icon::Computer, "Keep it as it sounds now")
                }
            }
            (Dragged::Setlist(index), Target::Presets | Target::Slot(_)) => {
                let (_, setlist) = self.lib_setlists.get(*index)?;
                if !self.pedal_online() {
                    return Outcome::refuses("Connect the pedal first");
                }
                if !self.setlist_compatible(setlist) {
                    return Outcome::refuses("This setlist is for another pedal");
                }
                Outcome::does(Icon::Download, format!("Put {} on the pedal", setlist.name))
            }
            (
                Dragged::SetlistSlot {
                    setlist,
                    slot: from,
                },
                Target::Slot(slot),
            ) => {
                let (_, list) = self.lib_setlists.get(*setlist)?;
                let tone = list.slots.get(*from)?;
                self.tones_to_slot(&[(tone.hash.clone(), tone.name.clone())], slot)
            }
            (Dragged::SetlistSlot { setlist, slot }, Target::Tab(LibraryView::Tones)) => {
                let (_, list) = self.lib_setlists.get(*setlist)?;
                let tone = list.slots.get(*slot)?;
                if self.lib_entries.iter().any(|entry| entry.hash == tone.hash) {
                    Outcome::refuses(format!("{} is in your library already", tone.name))
                } else {
                    Outcome::does(Icon::Computer, "Keep in Tones")
                }
            }
            (Dragged::Files(files), Target::Tab(LibraryView::Tones)) => Outcome::does(
                Icon::Computer,
                if files.len() == 1 {
                    "Import into Tones".to_owned()
                } else {
                    format!("Import {} tones", files.len())
                },
            ),
            (Dragged::Files(files), Target::Slot(slot)) => {
                if files.len() > 1 {
                    return Outcome::refuses("One file at a time goes to a slot");
                }
                if let Some(why) = self.put_refusal() {
                    return Outcome::refuses(why);
                }
                let label = self.active_slot_label(slot);
                match self.held_in(slot) {
                    Some(old) => Outcome::does(Icon::Replace, format!("Replace {label} {old}")),
                    None => Outcome::does(Icon::ArrowDownToLine, format!("Put it in {label}")),
                }
            }
            (Dragged::Pedal, Target::Tab(LibraryView::Setlists)) => {
                if self.pedal_online() {
                    Outcome::does(Icon::ListMusic, "Keep every slot as a new setlist")
                } else {
                    Outcome::refuses("Connect the pedal first")
                }
            }
            (Dragged::Pedal, Target::Setlist(index)) => {
                let (_, setlist) = self.lib_setlists.get(index)?;
                if self.pedal_online() {
                    Outcome::does(
                        Icon::ListMusic,
                        format!(
                            "Capture the pedal as {} v{}",
                            setlist.name,
                            setlist.revision() + 1
                        ),
                    )
                } else {
                    Outcome::refuses("Connect the pedal first")
                }
            }
            _ => None,
        }
    }

    /// What a slot holds now, when it holds something.
    fn held_in(&self, slot: i64) -> Option<String> {
        let names = if self.pro_active() {
            self.pro.pedal_slots().map(|(_, names)| names)?
        } else {
            self.presets.clone()
        };
        let name = names.get(usize::try_from(slot).ok()?)?.trim().to_owned();
        (!name.is_empty() && !shell::hx_slot_is_empty(&name)).then_some(name)
    }

    /// Library tones going to slots from `slot`: what they replace or fill.
    fn tones_to_slot(&self, tones: &[(String, String)], slot: i64) -> Option<Outcome> {
        if let Some(why) = self.put_refusal() {
            return Outcome::refuses(why);
        }
        for (hash, _) in tones {
            if hash.is_empty() {
                continue;
            }
            if let Some(entry) = self.lib_entries.iter().find(|entry| &entry.hash == hash) {
                if let Some(why) =
                    self.refusal_hint(entry.marker.as_ref(), &entry.firmware, entry.pro)
                {
                    return Outcome::refuses(why);
                }
            }
        }
        let label = self.active_slot_label(slot);
        if tones.len() > 1 {
            let labels: Vec<String> = (0..tones.len() as i64)
                .map(|offset| self.active_slot_label(slot + offset))
                .collect();
            return Outcome::does(Icon::ArrowDownToLine, format!("Fill {}", listed(&labels)));
        }
        match self.held_in(slot) {
            Some(old) => Outcome::does(Icon::Replace, format!("Replace {label} {old}")),
            None => Outcome::does(Icon::ArrowDownToLine, format!("Put it in {label}")),
        }
    }

    /// Whether a library tone dropped on the board plays.
    fn plays_here(&self, hash: &str, name: &str) -> Option<Outcome> {
        let entry = self
            .lib_entries
            .iter()
            .find(|entry| entry.hash == hash)
            .cloned()?;
        match self.cannot_play(&entry) {
            Some(strip) => Outcome::refuses(
                strip
                    .words
                    .iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>(),
            ),
            None => Outcome::does(Icon::Volume, format!("Play {name} on the pedal")),
        }
    }

    /// Carry a drop out: the same acts as the menus and keys.
    pub(crate) fn drop_on(
        &mut self,
        dragged: Dragged,
        target: Target,
        moving: bool,
        ctx: &egui::Context,
    ) {
        match (dragged, target) {
            (Dragged::Tones(tones), Target::Slot(slot)) => self.put_to(slot, tones),
            (Dragged::Tones(tones), Target::Board) => {
                if let Some(row) = tones
                    .first()
                    .and_then(|(hash, _)| self.lib_entries.iter().position(|e| &e.hash == hash))
                {
                    self.choose_tone(row);
                    self.audition_library(row);
                }
            }
            (Dragged::Tones(tones), Target::Tab(LibraryView::Cloud)) => {
                if let Some(row) = tones
                    .first()
                    .and_then(|(hash, _)| self.lib_entries.iter().position(|e| &e.hash == hash))
                {
                    self.publish_or_open(row, ctx);
                }
            }
            (Dragged::Tones(tones), Target::SetlistSlot { setlist, slot }) => {
                self.compose_setlist(setlist, slot, &tones);
            }
            (Dragged::Cloud(row), Target::Slot(slot)) => {
                self.start_cloud_action(row, CloudAction::PutIn(slot), ctx);
            }
            (Dragged::Cloud(row), Target::Tab(LibraryView::Tones)) => {
                self.start_cloud_action(row, CloudAction::Computer, ctx);
            }
            (Dragged::Cloud(row), Target::Board) => {
                self.cloud_selected = Some(row);
                self.start_cloud_action(row, CloudAction::Audition, ctx);
            }
            (Dragged::Preset(from), Target::Tab(LibraryView::Tones)) => {
                self.row_action(from, crate::RowAction::Keep);
            }
            (Dragged::Preset(from), Target::Slot(slot)) => {
                if let Some((name, bytes)) = self.slot_document(from) {
                    self.put_bytes(slot, name, bytes, moving.then_some(from));
                }
            }
            (Dragged::Preset(from), Target::Tab(LibraryView::Cloud)) => {
                if let Some((name, bytes)) = self.slot_document(from) {
                    let origin = self.origin();
                    self.keep_tone(&name, "hxpreset", &bytes, origin);
                    let hash = library::hash_of(&bytes);
                    if let Some(row) = self.lib_entries.iter().position(|e| e.hash == hash) {
                        self.publish_or_open(row, ctx);
                    }
                }
            }
            (Dragged::Playing, Target::Tab(LibraryView::Tones)) => self.keep_edit_as_version(),
            (Dragged::Setlist(index), Target::Presets | Target::Slot(_)) => {
                self.confirm_push = Some(index);
            }
            (
                Dragged::SetlistSlot {
                    setlist,
                    slot: from,
                },
                Target::Slot(slot),
            ) => {
                if let Some(tone) = self
                    .lib_setlists
                    .get(setlist)
                    .and_then(|(_, list)| list.slots.get(from))
                    .cloned()
                {
                    self.put_to(slot, vec![(tone.hash, tone.name)]);
                }
            }
            (Dragged::SetlistSlot { setlist, slot }, Target::Tab(LibraryView::Tones)) => {
                if let Some(tone) = self
                    .lib_setlists
                    .get(setlist)
                    .and_then(|(_, list)| list.slots.get(slot))
                    .cloned()
                {
                    if let Some(bytes) = library::read(&tone.hash) {
                        let kind = library::kind(&tone.hash).unwrap_or_else(|| "hxpreset".into());
                        self.keep_tone(&tone.name, &kind, &bytes, None);
                    }
                }
            }
            (Dragged::Pedal, Target::Tab(LibraryView::Setlists)) => {
                self.capture_pedal(None);
            }
            (Dragged::Pedal, Target::Setlist(index)) => {
                if let Some(name) = self.lib_setlists.get(index).map(|(_, s)| s.name.clone()) {
                    self.capture_pedal(Some(name));
                }
            }
            _ => {}
        }
    }

    /// Tone files dropped from outside, where the pointer last was: into the
    /// library on the Tones tab, into a slot on a preset. Answers whether it
    /// placed them; elsewhere a file opens its preview, as it always has.
    pub(crate) fn dropped_tone_files(&mut self, ctx: &egui::Context) -> bool {
        let files: Vec<std::path::PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .filter(|path| tone_file(path).is_some())
                .collect()
        });
        let target = self.drops.hover;
        if files.is_empty() {
            return false;
        }
        match target {
            Some(Target::Tab(LibraryView::Tones)) => {
                for path in &files {
                    let (Some(kind), Ok(bytes)) = (tone_file(path), std::fs::read(path)) else {
                        self.problem(format!("could not read {}", path.display()));
                        continue;
                    };
                    let name = path
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Untitled".to_owned());
                    self.keep_tone(&name, kind, &bytes, None);
                }
                self.drops.hover = None;
                true
            }
            Some(Target::Slot(slot)) if files.len() == 1 => {
                let path = &files[0];
                let Ok(bytes) = std::fs::read(path) else {
                    self.problem(format!("could not read {}", path.display()));
                    return true;
                };
                let name = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Untitled".to_owned());
                if tone_file(path) == Some("hlx") && !self.pro_active() {
                    // Built on the pedal, not written: its preview opens aimed
                    // at the slot.
                    self.open_tone_file_for(path, slot);
                } else {
                    self.put_bytes(slot, name, bytes, None);
                }
                self.drops.hover = None;
                true
            }
            _ => false,
        }
    }

    /// Compose a setlist by dropping tones into its slots: a new version,
    /// each tone in a slot from the one dropped on, in order. The version
    /// before stays as it was: a setlist is a record.
    pub(crate) fn compose_setlist(
        &mut self,
        index: usize,
        slot: usize,
        tones: &[(String, String)],
    ) {
        let Some((_, setlist)) = self.lib_setlists.get(index).cloned() else {
            return;
        };
        let mut next = setlist.clone();
        for (offset, (hash, name)) in tones.iter().enumerate() {
            let at = slot + offset;
            if at >= next.slots.len() {
                break;
            }
            next.slots[at] = library::Slot {
                hash: hash.clone(),
                name: name.clone(),
                file: String::new(),
            };
        }
        match library::save_setlist_version(&next) {
            Ok((path, saved)) => {
                self.note(format!(
                    "{} v{}: {} put in {}",
                    saved.name,
                    saved.revision(),
                    tones
                        .iter()
                        .map(|(_, name)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.active_slot_label(slot as i64)
                ));
                self.refresh_library();
                if let Some(index) = self.lib_setlists.iter().position(|(p, _)| *p == path) {
                    self.select_setlist_entry(index);
                }
            }
            Err(why) => self.problem(why),
        }
    }

    /// The places drawn elsewhere that take drops: the presets, each preset,
    /// and the board. Registered before the frame settles, the presets list
    /// first so a preset under the pointer wins over the list around it.
    pub(crate) fn register_drop_zones(&mut self, ctx: &egui::Context) {
        if shell::take_title_drag(ctx) && self.pedal_online() {
            self.start_drag(ctx, Dragged::Playing);
        }
        let Some(dragged) = self.dragged(ctx) else {
            return;
        };
        if let Some(rect) = self.presets_rect {
            self.drop_zone(rect, Target::Presets);
            // A setlist goes to the presets as a whole: the list says so.
            if let Some(outcome) = self
                .drop_outcome(&dragged, Target::Presets, false)
                .filter(|outcome| !outcome.refused)
            {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("presets-drop"),
                ));
                let over = matches!(self.drops.hover, Some(Target::Presets | Target::Slot(_)));
                let inner = rect.shrink(6.0);
                painter.rect_filled(
                    inner,
                    CornerRadius::same(10),
                    theme::alpha(theme::accent(), if over { 0.08 } else { 0.04 }),
                );
                theme::paint::dashed_rect(
                    &painter,
                    inner,
                    9.5,
                    Stroke::new(1.5, theme::accent_line()),
                    5.0,
                    4.0,
                );
                // The words on a pill of their own, over the rows.
                let words =
                    painter.layout_no_wrap(outcome.words, theme::semibold(12.0), theme::accent());
                let pill = Rect::from_center_size(
                    Pos2::new(inner.center().x, inner.top() + 20.0),
                    Vec2::new((words.size().x + 20.0).min(inner.width() - 8.0), 24.0),
                );
                painter.rect(
                    pill,
                    CornerRadius::same(12),
                    theme::mix(theme::panel(), theme::accent(), 0.12),
                    Stroke::new(1.0, theme::accent_line()),
                    egui::StrokeKind::Inside,
                );
                let clipped = painter.with_clip_rect(pill.shrink2(Vec2::new(8.0, 0.0)));
                clipped.galley(
                    Pos2::new(
                        (pill.center().x - words.size().x / 2.0).max(pill.left() + 10.0),
                        pill.center().y - words.size().y / 2.0,
                    ),
                    words,
                    Color32::PLACEHOLDER,
                );
            }
        }
        let rows: Vec<(i64, Rect)> = if self.pro_active() {
            self.pro
                .row_rects()
                .into_iter()
                .map(|(index, rect)| (index as i64, rect))
                .collect()
        } else {
            self.row_rects
                .iter()
                .map(|(slot, rect)| (*slot, *rect))
                .collect()
        };
        for (slot, rect) in rows {
            self.drop_zone(rect, Target::Slot(slot));
        }
        let board = if self.pro_active() {
            self.pro.board_rect
        } else {
            self.board_rect
        };
        if let Some(rect) = board {
            self.drop_zone(rect, Target::Board);
            self.paint_board_drop(ctx, rect);
        }
    }

    /// While something that plays is dragged, the board says it takes it:
    /// a faint dashed edge, and what a drop there does.
    fn paint_board_drop(&self, ctx: &egui::Context, rect: Rect) {
        let Some(dragged) = self.dragged(ctx) else {
            return;
        };
        if !self
            .drop_outcome(&dragged, Target::Board, false)
            .is_some_and(|outcome| !outcome.refused)
        {
            return;
        }
        let over = self.drops.hover == Some(Target::Board);
        egui::Area::new(egui::Id::new("board-drop"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .interactable(false)
            .show(ctx, |ui| {
                ui.allocate_exact_size(rect.size(), Sense::hover());
                let inner = rect.shrink(6.0);
                if over {
                    ui.painter().rect_filled(
                        inner,
                        CornerRadius::same(12),
                        theme::alpha(theme::accent(), 0.06),
                    );
                }
                theme::paint::dashed_rect(
                    ui.painter(),
                    inner,
                    11.5,
                    Stroke::new(
                        1.5,
                        if over {
                            theme::accent_line()
                        } else {
                            theme::alpha(theme::accent_line(), 0.5)
                        },
                    ),
                    5.0,
                    4.0,
                );
                // At the top, in the board's header line, as the sheet draws
                // it.
                let words = shell::galley(
                    ui,
                    "Drop on the board to hear it",
                    theme::medium(12.0),
                    theme::accent(),
                );
                let width = 13.0 + 7.0 + words.size().x;
                let left = inner.center().x - width / 2.0;
                let y = rect.top() + 20.0;
                theme::paint_icon(
                    ui,
                    Icon::Volume,
                    Pos2::new(left + 6.5, y),
                    13.0,
                    theme::accent(),
                );
                shell::paint_line(ui, words, left + 20.0, y);
            });
    }

    /// End of the frame: the target under the pointer, the ghost beside it,
    /// and a release carried out.
    pub(crate) fn settle_drop(&mut self, ctx: &egui::Context) {
        let zones = std::mem::take(&mut self.drops.zones);
        let Some(dragged) = self.dragged(ctx) else {
            self.drops.hover = None;
            return;
        };
        let Some(pointer) = ctx.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let target = zones
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pointer))
            .map(|(_, target)| *target);
        self.drops.hover = target;
        let moving = ctx.input(|input| input.modifiers.shift);
        let outcome = target.and_then(|target| self.drop_outcome(&dragged, target, moving));
        if matches!(dragged, Dragged::Files(_)) {
            self.paint_ghost(ctx, &dragged, pointer, outcome.as_ref());
            return;
        }
        if ctx.input(|input| input.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ctx);
            self.drops.hover = None;
            if let (Some(target), Some(Outcome { refused: false, .. })) = (target, &outcome) {
                self.drop_on(dragged, target, moving, ctx);
            }
            return;
        }
        self.paint_ghost(ctx, &dragged, pointer, outcome.as_ref());
    }

    /// What is dragged, beside the pointer: its marker, name and chain, and
    /// what the drop under the pointer would do, or why not.
    fn paint_ghost(
        &self,
        ctx: &egui::Context,
        dragged: &Dragged,
        pointer: Pos2,
        outcome: Option<&Outcome>,
    ) {
        let (marker, name, chain, count) = self.drag_label(dragged);
        egui::Area::new(egui::Id::new("drop-ghost"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pointer + Vec2::new(14.0, 16.0))
            .interactable(false)
            .show(ctx, |ui| {
                let frame = egui::Frame::new()
                    .fill(theme::panel())
                    .stroke(Stroke::new(1.0, theme::line_strong()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(90),
                    });
                let shown = frame.show(ui, |ui| {
                    ui.set_min_width(230.0);
                    ui.set_max_width(420.0);
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    ui.horizontal(|ui| {
                        crate::library_view::marker(ui, marker.as_ref(), true);
                        theme::label(ui, &name, theme::semibold(13.0), theme::text());
                        if !chain.is_empty() {
                            shell::mini_chain(ui, &chain);
                        }
                    });
                    if let Some(outcome) = outcome {
                        let (spot, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 1.0),
                            Sense::hover(),
                        );
                        ui.painter().hline(
                            spot.x_range(),
                            spot.center().y,
                            Stroke::new(1.0, theme::line()),
                        );
                        ui.horizontal(|ui| {
                            let ink = if outcome.refused {
                                theme::muted()
                            } else {
                                theme::accent()
                            };
                            let (icon, _) =
                                ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                            theme::paint_icon(ui, outcome.icon, icon.center(), 13.0, ink);
                            theme::label(
                                ui,
                                &outcome.words,
                                if outcome.refused {
                                    theme::regular(12.5)
                                } else {
                                    theme::semibold(12.5)
                                },
                                ink,
                            );
                        });
                    }
                });
                // Several at once: a stack, and how many.
                if count > 1 {
                    let rect = shown.response.rect;
                    let badge = Pos2::new(rect.right() - 2.0, rect.top() + 2.0);
                    ui.painter().circle_filled(badge, 10.0, theme::accent());
                    let galley = shell::galley(
                        ui,
                        count.to_string(),
                        theme::semibold(11.0),
                        theme::accent_ink(),
                    );
                    let width = galley.size().x;
                    shell::paint_line(ui, galley, badge.x - width / 2.0, badge.y);
                }
            });
    }

    /// The ghost's first line: a marker, a name, a chain, and how many.
    fn drag_label(
        &self,
        dragged: &Dragged,
    ) -> (
        Option<crate::devices::Marker>,
        String,
        Vec<shell::Mini>,
        usize,
    ) {
        match dragged {
            Dragged::Tones(tones) => {
                let first = tones.first().and_then(|(hash, _)| {
                    self.lib_entries.iter().find(|entry| &entry.hash == hash)
                });
                let name = match tones.as_slice() {
                    [(_, name)] => name.clone(),
                    [(_, name), rest @ ..] => format!("{name} and {} more", rest.len()),
                    [] => String::new(),
                };
                (
                    first.and_then(|entry| entry.marker.clone()),
                    name,
                    if tones.len() == 1 {
                        first.map(|entry| entry.chain.clone()).unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    tones.len(),
                )
            }
            Dragged::Cloud(row) => self.cloud_entries.get(*row).map_or_else(
                || (None, String::new(), Vec::new(), 1),
                |entry| {
                    (
                        entry.row.marker.clone(),
                        entry.discovered.tone.summary.name.clone(),
                        entry.row.chain.clone(),
                        1,
                    )
                },
            ),
            Dragged::Preset(slot) => (
                None,
                format!(
                    "{} {}",
                    self.active_slot_label(*slot),
                    self.held_in(*slot).unwrap_or_default()
                ),
                Vec::new(),
                1,
            ),
            Dragged::Playing => (
                None,
                if self.pro_active() {
                    self.pro.loaded_name()
                } else {
                    self.preset_name.clone()
                },
                Vec::new(),
                1,
            ),
            Dragged::Setlist(index) => (
                None,
                self.lib_setlists
                    .get(*index)
                    .map(|(_, setlist)| setlist.name.clone())
                    .unwrap_or_default(),
                Vec::new(),
                1,
            ),
            Dragged::SetlistSlot { setlist, slot } => (
                None,
                self.lib_setlists
                    .get(*setlist)
                    .and_then(|(_, list)| list.slots.get(*slot))
                    .map(|tone| tone.name.clone())
                    .unwrap_or_default(),
                Vec::new(),
                1,
            ),
            Dragged::Pedal => (
                None,
                format!("Every preset on the {}", self.device.trim()),
                Vec::new(),
                1,
            ),
            Dragged::Files(files) => (
                None,
                match files.as_slice() {
                    [one] => one
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    several => format!("{} files", several.len()),
                },
                Vec::new(),
                files.len(),
            ),
        }
    }
}

/// "Replace 05B Chime Clean" as it reads after "Keep it, then".
fn lowercase_first(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Marker;
    use crate::session::{Cmd, Evt};
    use crate::{Connection, LibEntry};
    use std::sync::mpsc;

    fn app() -> (App, mpsc::Sender<Evt>, mpsc::Receiver<Cmd>) {
        let (to_device, cmds) = mpsc::channel();
        let (events, from_device) = mpsc::channel();
        let app = App::new(&egui::Context::default(), to_device, from_device);
        let _ = cmds.try_iter().count();
        (app, events, cmds)
    }

    /// An HX Stomp whose 05B holds Chime Clean and whose 05C on is empty,
    /// to 06A.
    fn online(app: &mut App) {
        app.connection = Connection::Online;
        app.device = "HX Stomp".into();
        app.firmware = "3.80".into();
        app.library_connected_device = "HX Stomp".into();
        app.preset_index = 1;
        app.preset_name = "Plexi Crunch".into();
        app.presets = (0..16)
            .map(|index| match index {
                13 => "Chime Clean".to_owned(),
                14 | 15 => "New Preset".to_owned(),
                index => format!("Preset {index}"),
            })
            .collect();
    }

    fn tone(name: &str, marker: Marker) -> LibEntry {
        let kind = if marker.family == crate::devices::Family::Pro {
            "vxpreset"
        } else {
            "hxpreset"
        };
        let hash = library::store(name, name.as_bytes(), kind).expect("the scratch library");
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
            pro: kind == "vxpreset",
            marker: Some(marker),
            firmware: "3.80".to_owned(),
        }
    }

    fn tones(app: &App, rows: &[usize]) -> Dragged {
        Dragged::Tones(
            rows.iter()
                .map(|&row| {
                    (
                        app.lib_entries[row].hash.clone(),
                        app.lib_entries[row].name.clone(),
                    )
                })
                .collect(),
        )
    }

    /// The ghost and the row under the pointer say what a drop of tones on
    /// a preset does, the same words the question then asks about.
    #[test]
    fn a_drop_on_a_preset_says_what_it_replaces_or_fills() {
        let _scratch = library::tests::Scratch::new("dnd-words");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        app.lib_entries = vec![
            tone("Slapback Twang", Marker::hx("Stomp")),
            tone("Doom Fuzz", Marker::hx("Stomp")),
            tone("Velvet Drive", Marker::pro("2.x")),
        ];
        let one = tones(&app, &[0]);
        let words = |outcome: Option<Outcome>| outcome.map(|o| (o.words, o.refused));
        assert_eq!(
            words(app.drop_outcome(&one, Target::Slot(13), false)),
            Some(("Replace 05B Chime Clean".to_owned(), false))
        );
        assert_eq!(
            words(app.drop_outcome(&one, Target::Slot(14), false)),
            Some(("Put it in 05C".to_owned(), false))
        );
        assert_eq!(
            words(app.drop_outcome(&tones(&app, &[0, 1]), Target::Slot(13), false)),
            Some(("Fill 05B and 05C".to_owned(), false))
        );
        let refused = app
            .drop_outcome(&tones(&app, &[2]), Target::Slot(13), false)
            .expect("a reason");
        assert!(refused.refused);
        assert_eq!(
            refused.words,
            "The HX Stomp cannot play a StompStation PRO tone"
        );
        assert!(
            app.drop_outcome(&one, Target::Tab(LibraryView::Tones), false)
                .is_none(),
            "a library tone is in Tones already"
        );
    }

    /// A drop on an empty slot writes it; one on a used slot asks first.
    #[test]
    fn a_drop_writes_an_empty_slot_and_asks_before_replacing() {
        let _scratch = library::tests::Scratch::new("dnd-drop");
        let (mut app, _events, cmds) = app();
        online(&mut app);
        app.lib_entries = vec![tone("Slapback Twang", Marker::hx("Stomp"))];
        let ctx = egui::Context::default();
        app.drop_on(tones(&app, &[0]), Target::Slot(14), false, &ctx);
        assert!(app.put_question.is_none());
        assert!(matches!(cmds.try_recv(), Ok(Cmd::PushSetlist(_))));

        app.drop_on(tones(&app, &[0]), Target::Slot(13), false, &ctx);
        assert!(app.put_question.is_some());
        assert!(cmds.try_recv().is_err());
    }

    /// Tones dropped into a setlist's slots compose its next version; the
    /// version before stays as it was.
    #[test]
    fn tones_dropped_into_a_setlist_compose_its_next_version() {
        let _scratch = library::tests::Scratch::new("dnd-compose");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        app.lib_entries = vec![tone("Dream Pop", Marker::hx("Stomp"))];
        let mut setlist = library::Setlist {
            name: "Album release show".to_owned(),
            slots: vec![library::Slot::default(); 4],
            ..Default::default()
        };
        setlist.slots[0] = library::Slot {
            hash: app.lib_entries[0].hash.clone(),
            name: "Dream Pop".to_owned(),
            file: String::new(),
        };
        let (path, first) = library::save_setlist_version(&setlist).unwrap();
        app.lib_setlists = library::setlists();
        let index = app
            .lib_setlists
            .iter()
            .position(|(known, _)| *known == path)
            .expect("the setlist");
        let ctx = egui::Context::default();
        let dragged = tones(&app, &[0]);
        assert_eq!(
            app.drop_outcome(
                &dragged,
                Target::SetlistSlot {
                    setlist: index,
                    slot: 2
                },
                false
            )
            .map(|outcome| outcome.words),
            Some("Put it in Album release show's 01C".to_owned())
        );
        app.drop_on(
            dragged,
            Target::SetlistSlot {
                setlist: index,
                slot: 2,
            },
            false,
            &ctx,
        );

        let versions: Vec<library::Setlist> = library::setlists()
            .into_iter()
            .map(|(_, setlist)| setlist)
            .filter(|setlist| setlist.name == "Album release show")
            .collect();
        assert_eq!(versions.len(), 2, "a new version beside the old");
        let next = versions.iter().find(|s| s.revision() == 2).expect("v2");
        assert_eq!(next.slots[2].name, "Dream Pop");
        let old = versions.iter().find(|s| s.revision() == 1).expect("v1");
        assert!(old.slots[2].is_empty(), "the record before is unchanged");
        assert_eq!(first.revision(), 1);
    }

    /// The pedal dropped on a setlist captures it as that setlist's next
    /// version; a setlist dropped on the presets asks to put it on the
    /// pedal.
    #[test]
    fn the_pedal_and_setlists_go_where_they_are_dropped() {
        let _scratch = library::tests::Scratch::new("dnd-setlists");
        let (mut app, _events, cmds) = app();
        online(&mut app);
        let setlist = library::Setlist {
            name: "Rehearsal".to_owned(),
            slots: vec![library::Slot::default(); 16],
            ..Default::default()
        };
        library::save_setlist_version(&setlist).unwrap();
        app.lib_setlists = library::setlists();
        let ctx = egui::Context::default();
        assert_eq!(
            app.drop_outcome(&Dragged::Pedal, Target::Setlist(0), false)
                .map(|outcome| outcome.words),
            Some("Capture the pedal as Rehearsal v2".to_owned())
        );
        app.drop_on(Dragged::Pedal, Target::Setlist(0), false, &ctx);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::CaptureSetlist)));
        assert_eq!(app.capture_as.as_deref(), Some("Rehearsal"));

        app.drop_on(Dragged::Setlist(0), Target::Presets, false, &ctx);
        assert_eq!(app.confirm_push, Some(0));
    }
}
