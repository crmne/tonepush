//! The HX's backups on this computer: the automatic copy TonePush keeps
//! current, the dated copies it set aside before refreshing it, and the
//! copies saved to a file, each with why it was taken; the one chosen is
//! compared with the pedal as it is now, and can be put back whole.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::{CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use hx_usb::backup::Manifest;

use crate::theme::{self, Icon, Mood, Tier};
use crate::{shell, App, Cmd, Connection};

/// The note a set-aside copy carries of why it was set aside. A copy
/// without one restores all the same; it only cannot say why it exists.
pub(crate) const WHY_FILE: &str = "tonepush-why.json";

/// Why the automatic backup was set aside before it was refreshed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "why", rename_all = "lowercase")]
pub(crate) enum Why {
    /// The pedal connected: the copy is the pedal as last seen before.
    Connected,
    /// A setlist from the library was written over the pedal.
    Setlist { name: String },
    /// Presets were sent to these slots.
    Slots { slots: Vec<usize> },
    /// A backup was put back on the pedal.
    Restore,
}

/// Write down why a set-aside copy exists, inside it, so the note goes when
/// the copy is pruned.
pub(crate) fn note_why(bundle: &Path, why: &Why) -> std::io::Result<()> {
    let json = serde_json::to_vec(why).map_err(std::io::Error::other)?;
    std::fs::write(bundle.join(WHY_FILE), json)
}

fn why_of(bundle: &Path) -> Option<Why> {
    serde_json::from_slice(&std::fs::read(bundle.join(WHY_FILE)).ok()?).ok()
}

/// Where a copy came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The automatic backup, kept current after every save; the slot saved
    /// last since it was read, if any was.
    Current { saved: Option<usize> },
    /// Set aside before the automatic backup was refreshed, and why, when
    /// TonePush wrote it down.
    SetAside(Option<Why>),
    /// Saved to a file the person chose.
    File,
}

/// One copy of the pedal on this computer.
#[derive(Clone, Debug)]
pub(crate) struct BackupCopy {
    pub(crate) path: PathBuf,
    pub(crate) origin: Origin,
    /// When it was taken, in seconds since the epoch: for the automatic
    /// backup, when it was last kept current.
    pub(crate) when: u64,
    pub(crate) manifest: Manifest,
    /// Whether every preset, impulse response and the settings the copy
    /// names are there to be put back.
    pub(crate) complete: bool,
}

impl BackupCopy {
    /// The presets it holds, across every setlist.
    pub(crate) fn presets(&self) -> usize {
        self.manifest
            .setlist_presets()
            .flatten()
            .filter(|name| !shell::hx_slot_is_empty(name))
            .count()
    }

    /// "42 presets · 4 IRs".
    pub(crate) fn holds(&self) -> String {
        let presets = self.presets();
        let irs = self.manifest.irs.len();
        format!(
            "{presets} {} · {irs} {}",
            if presets == 1 { "preset" } else { "presets" },
            if irs == 1 { "IR" } else { "IRs" }
        )
    }
}

/// What a copy holds, slot by slot: every setlist's names, and the hash of
/// each slot's document by (setlist, slot).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    pub(crate) names: Vec<Vec<String>>,
    pub(crate) hashes: BTreeMap<(usize, usize), String>,
}

/// Read what a copy holds. Every slot is hashed, which for a whole pedal is
/// a few hundred small files: done once, when the copy is chosen.
pub(crate) fn held(dir: &Path, manifest: &Manifest) -> Held {
    let names: Vec<Vec<String>> = manifest.setlist_presets().map(<[String]>::to_vec).collect();
    let mut hashes = BTreeMap::new();
    for setlist in 0..names.len() {
        for (slot, path) in slot_files(dir, setlist) {
            if let Ok(bytes) = std::fs::read(&path) {
                hashes.insert((setlist, slot), crate::library::hash_of(&bytes));
            }
        }
    }
    Held { names, hashes }
}

/// A setlist's preset files in a bundle by slot, read the way `hx-usb`
/// writes them: `presets` for the first, `presets-1` on, each file named
/// after its slot number.
fn slot_files(dir: &Path, setlist: usize) -> BTreeMap<usize, PathBuf> {
    let folder = match setlist {
        0 => dir.join("presets"),
        n => dir.join(format!("presets-{n}")),
    };
    let Ok(read) = std::fs::read_dir(folder) else {
        return BTreeMap::new();
    };
    read.flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "hxpreset"))
        .filter_map(|path| {
            let slot = path
                .file_name()?
                .to_str()?
                .split_once(' ')?
                .0
                .parse()
                .ok()?;
            Some((slot, path))
        })
        .collect()
}

/// Whether everything the manifest names is in the bundle: a file for every
/// named preset in every setlist, one per impulse response, and the settings.
fn complete(dir: &Path, manifest: &Manifest) -> bool {
    let presets = manifest.holds_every_setlist()
        && manifest
            .setlist_presets()
            .enumerate()
            .all(|(setlist, names)| {
                let files = slot_files(dir, setlist);
                names
                    .iter()
                    .enumerate()
                    .all(|(slot, name)| name.is_empty() || files.contains_key(&slot))
            });
    let irs = std::fs::read_dir(dir.join("irs")).map_or(0, |read| {
        read.flatten()
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "f32"))
            .count()
    });
    presets && irs >= manifest.irs.len() && dir.join("globals.json").is_file()
}

fn seconds(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// The time a set-aside copy's name records, `2026-10-04 180200` in UTC,
/// as seconds since the epoch.
pub(crate) fn seconds_of_stamp(stamp: &str) -> Option<u64> {
    let time = jiff::civil::DateTime::strptime("%Y-%m-%d %H%M%S", stamp).ok()?;
    let zoned = time.to_zoned(jiff::tz::TimeZone::UTC).ok()?;
    u64::try_from(zoned.timestamp().as_second()).ok()
}

/// The automatic backup, kept current: when it was last written to, and
/// the slot saved last if that was after it was read.
fn current(dir: &Path, manifest: Manifest) -> BackupCopy {
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(seconds)
    };
    let mut saved = None;
    for setlist in 0..manifest.setlist_presets().count() {
        for (slot, path) in slot_files(dir, setlist) {
            if let Some(when) = modified(&path) {
                // A capture writes every file at once; one written well
                // after it was kept current by a save.
                if when > manifest.captured + 5 && saved.is_none_or(|(_, last)| when > last) {
                    saved = Some((slot, when));
                }
            }
        }
    }
    let when = modified(&dir.join("manifest.json"))
        .unwrap_or(manifest.captured)
        .max(manifest.captured);
    BackupCopy {
        path: dir.to_owned(),
        origin: Origin::Current {
            saved: saved.map(|(slot, _)| slot),
        },
        when,
        complete: complete(dir, &manifest),
        manifest,
    }
}

/// Every copy of the pedal on this computer: the automatic backup first,
/// then the rest newest first. `files` are the backups saved to a file;
/// those since moved or deleted are left out.
pub(crate) fn copies(automatic: &Path, files: &[PathBuf]) -> Vec<BackupCopy> {
    let mut found = Vec::new();
    let stem = automatic
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("automatic")
        .to_owned();
    if let Some(history) = automatic.parent().map(|parent| parent.join("history")) {
        for path in std::fs::read_dir(history)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && path.extension().is_some_and(|e| e == "hxbundle"))
        {
            let Ok(manifest) = hx_usb::backup::open(&path) else {
                continue;
            };
            let when = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(&format!("{stem} ")))
                .and_then(seconds_of_stamp)
                .unwrap_or(manifest.captured);
            found.push(BackupCopy {
                origin: Origin::SetAside(why_of(&path)),
                when,
                complete: complete(&path, &manifest),
                manifest,
                path,
            });
        }
    }
    for path in files {
        if let Ok(manifest) = hx_usb::backup::open(path) {
            found.push(BackupCopy {
                path: path.clone(),
                origin: Origin::File,
                when: manifest.captured,
                complete: complete(path, &manifest),
                manifest,
            });
        }
    }
    found.sort_by_key(|copy| std::cmp::Reverse(copy.when));
    if hx_usb::backup::exists(automatic) {
        if let Ok(manifest) = hx_usb::backup::open(automatic) {
            found.insert(0, current(automatic, manifest));
        }
    }
    found
}

/// How a slot differs between a copy and the pedal now, said from now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// The same preset, changed since.
    Edited,
    /// Another preset holds the slot now.
    Replaced { was: String },
    /// The slot was empty in the copy.
    Added,
    /// The slot is empty now; this is what the copy held.
    Removed,
}

/// One slot that differs, with the name of the preset it concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Difference {
    pub(crate) setlist: usize,
    pub(crate) slot: usize,
    pub(crate) name: String,
    pub(crate) change: Change,
}

impl Difference {
    /// "edited since", "was Night Verb".
    pub(crate) fn words(&self) -> String {
        match &self.change {
            Change::Edited => "edited since".to_owned(),
            Change::Replaced { was } => format!("was {was}"),
            Change::Added => "added since".to_owned(),
            Change::Removed => "removed since".to_owned(),
        }
    }
}

/// What differs between a copy (`then`) and the pedal now, slot by slot,
/// and how many slots are the same. A slot is the same when both are empty,
/// or both hold the same document under the same name.
pub(crate) fn differences(then: &Held, now: &Held) -> (Vec<Difference>, usize) {
    let mut found = Vec::new();
    let mut same = 0;
    let setlists = then.names.len().max(now.names.len());
    for setlist in 0..setlists {
        let empty = Vec::new();
        let was = then.names.get(setlist).unwrap_or(&empty);
        let is = now.names.get(setlist).unwrap_or(&empty);
        for slot in 0..was.len().max(is.len()) {
            let name = |names: &Vec<String>| {
                names
                    .get(slot)
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !shell::hx_slot_is_empty(name))
            };
            let change = match (name(was), name(is)) {
                (None, None) => None,
                (Some(old), None) => Some((old, Change::Removed)),
                (None, Some(new)) => Some((new, Change::Added)),
                (Some(old), Some(new)) if old != new => Some((new, Change::Replaced { was: old })),
                (Some(_), Some(new)) => {
                    let key = (setlist, slot);
                    (then.hashes.get(&key) != now.hashes.get(&key)).then_some((new, Change::Edited))
                }
            };
            match change {
                Some((name, change)) => found.push(Difference {
                    setlist,
                    slot,
                    name,
                    change,
                }),
                None => same += 1,
            }
        }
    }
    (found, same)
}

/// "Today, 14:02", "Yesterday, 21:11", "28 Sep, 10:02", with the year when
/// it is not this one.
fn when_words_at(then: &jiff::Zoned, now: &jiff::Zoned) -> String {
    let time = then.strftime("%H:%M");
    let (day, today) = (then.date(), now.date());
    if day == today {
        format!("Today, {time}")
    } else if day.tomorrow().ok() == Some(today) {
        format!("Yesterday, {time}")
    } else if day.year() == today.year() {
        format!("{}, {time}", then.strftime("%-d %b"))
    } else {
        format!("{}, {time}", then.strftime("%-d %b %Y"))
    }
}

/// [`when_words_at`] for a time in seconds since the epoch, in the
/// computer's own time zone.
pub(crate) fn when_words(when: u64) -> String {
    let Ok(stamp) = jiff::Timestamp::from_second(i64::try_from(when).unwrap_or(i64::MAX)) else {
        return String::new();
    };
    let zone = jiff::tz::TimeZone::system();
    when_words_at(
        &stamp.to_zoned(zone.clone()),
        &jiff::Timestamp::now().to_zoned(zone),
    )
}

/// Open a folder in the system's file manager.
fn show_in_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map(drop)
}

/// A whole-pedal restore waiting for its confirmation.
#[derive(Clone, Debug)]
pub(crate) struct Restoring {
    pub(crate) path: PathBuf,
    pub(crate) manifest: Manifest,
    /// "the copy from 28 Sep, 18:02", or the file's name.
    pub(crate) what: String,
    /// How many presets differ from the pedal now, when that is known.
    pub(crate) differ: Option<usize>,
}

/// The When, Why, Holds and Checked columns' widths for a card `width`
/// wide; Holds goes first when there is no room for it.
fn columns(width: f32) -> [f32; 4] {
    let inner = width - 24.0;
    let when = 128.0;
    let checked = 104.0;
    let holds = if inner - when - checked >= 200.0 + 150.0 {
        150.0
    } else {
        0.0
    };
    [
        when,
        (inner - when - holds - checked).max(120.0),
        holds,
        checked,
    ]
}

const HISTORY_ROW: f32 = 44.0;
const HISTORY_HEAD: f32 = 32.0;

impl App {
    /// The inspector's width beside the history.
    fn backup_inspector_width(tier: Tier) -> f32 {
        tier.pick(300.0, 340.0, 380.0)
    }

    /// Read the copies on disk if they have not been read since the last
    /// backup, keeping the chosen one chosen; the newest set-aside copy is
    /// chosen at first, since the current one is the pedal itself.
    pub(crate) fn read_backups(&mut self) {
        if self.backup_shelf.is_some() {
            return;
        }
        let copies = session_automatic()
            .map(|dir| copies(&dir, &self.config.backup_files))
            .unwrap_or_default();
        let chosen = self
            .backup_chosen
            .as_ref()
            .filter(|path| copies.iter().any(|copy| &copy.path == *path))
            .cloned()
            .or_else(|| {
                copies
                    .iter()
                    .find(|copy| !matches!(copy.origin, Origin::Current { .. }))
                    .or(copies.first())
                    .map(|copy| copy.path.clone())
            });
        self.backup_chosen = chosen;
        self.backup_held.clear();
        self.backup_shelf = Some(copies);
    }

    /// Forget the copies read, so the next look reads them again.
    pub(crate) fn forget_backups(&mut self) {
        self.backup_shelf = None;
    }

    fn chosen_backup(&self) -> Option<&BackupCopy> {
        let chosen = self.backup_chosen.as_ref()?;
        self.backup_shelf
            .as_ref()?
            .iter()
            .find(|copy| &copy.path == chosen)
    }

    /// What a copy holds, read once.
    fn held_by(&mut self, copy: &BackupCopy) -> Held {
        if let Some(held) = self.backup_held.get(&copy.path) {
            return held.clone();
        }
        let read = held(&copy.path, &copy.manifest);
        self.backup_held.insert(copy.path.clone(), read.clone());
        read
    }

    /// How the chosen copy differs from the pedal now, as the current copy
    /// holds it; `None` when it is the current copy or there is none.
    fn backup_differences(&mut self, copy: &BackupCopy) -> Option<(Vec<Difference>, usize)> {
        if matches!(copy.origin, Origin::Current { .. }) {
            return None;
        }
        let current = self
            .backup_shelf
            .as_ref()?
            .iter()
            .find(|copy| matches!(copy.origin, Origin::Current { .. }))?
            .clone();
        let then = self.held_by(copy);
        let now = self.held_by(&current);
        Some(differences(&then, &now))
    }

    /// Why a copy was taken, as the history's two lines.
    fn backup_reason(&self, copy: &BackupCopy) -> (String, String) {
        match &copy.origin {
            Origin::Current { saved } => (
                "Kept current".to_owned(),
                match saved {
                    Some(slot) => format!("Refreshed after saving {}", self.backup_slot(0, *slot)),
                    None => shell::when_words("Read", time_of(copy.manifest.captured)),
                },
            ),
            Origin::SetAside(Some(Why::Connected)) => (
                "Pedal connected".to_owned(),
                "As last seen before it connected".to_owned(),
            ),
            Origin::SetAside(Some(Why::Setlist { name })) => (
                "Before a setlist".to_owned(),
                format!("Before {name} was written"),
            ),
            Origin::SetAside(Some(Why::Slots { slots })) => {
                let labels: Vec<String> = slots
                    .iter()
                    .take(3)
                    .map(|slot| self.backup_slot(0, *slot))
                    .collect();
                let more = slots.len().saturating_sub(3);
                let named = if more > 0 {
                    format!("{} and {more} more", labels.join(", "))
                } else {
                    labels.join(", ")
                };
                (
                    if slots.len() == 1 {
                        "Before a preset was sent".to_owned()
                    } else {
                        "Before presets were sent".to_owned()
                    },
                    format!(
                        "Before {named} {} written",
                        if slots.len() == 1 { "was" } else { "were" }
                    ),
                )
            }
            Origin::SetAside(Some(Why::Restore)) => (
                "Before a restore".to_owned(),
                "Before a backup was put back".to_owned(),
            ),
            Origin::SetAside(None) => (
                "An earlier copy".to_owned(),
                "Set aside when the backup was refreshed".to_owned(),
            ),
            Origin::File => (
                "Saved to a file".to_owned(),
                copy.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        }
    }

    /// "02B", or "Setlist 2 02B" on a pedal with more than one setlist.
    fn backup_slot(&self, setlist: usize, slot: usize) -> String {
        let label = self.active_slot_label(slot as i64);
        if setlist == 0 {
            label
        } else {
            let name = self
                .setlists
                .get(setlist)
                .cloned()
                .unwrap_or_else(|| format!("Setlist {}", setlist + 1));
            format!("{name} {label}")
        }
    }

    /// The Backups tab: how the pedal is protected, then every copy on this
    /// computer. The chosen copy's comparison is in the inspector beside it.
    pub(crate) fn backups_tab(&mut self, ui: &mut Ui) {
        self.read_backups();
        self.backup_banner(ui);
        ui.add_space(14.0);
        let copies = self.backup_shelf.clone().unwrap_or_default();
        if copies.is_empty() {
            return;
        }
        let rows: Vec<(String, (String, String))> = copies
            .iter()
            .map(|copy| (when_words(copy.when), self.backup_reason(copy)))
            .collect();
        let mut picked = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let width = ui.available_width();
            let [when, why, holds, checked] = columns(width);
            // The header.
            let (head, _) = ui.allocate_exact_size(Vec2::new(width, HISTORY_HEAD), Sense::hover());
            let mut x = head.left() + 12.0 + 8.0;
            for (title, column) in [
                ("When", when),
                ("Why it was taken", why),
                ("Holds", holds),
                ("Checked", checked),
            ] {
                if column > 0.0 {
                    let galley =
                        shell::galley(ui, title, theme::semibold(theme::LABEL), theme::muted());
                    shell::paint_line(ui, galley, x, head.center().y);
                }
                x += column;
            }
            ui.painter().hline(
                head.x_range(),
                head.bottom() - 0.5,
                Stroke::new(1.0, theme::line()),
            );
            let last = copies.len() - 1;
            for (index, (copy, (time, (title, sub)))) in copies.iter().zip(&rows).enumerate() {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, HISTORY_ROW), Sense::click());
                let chosen = self.backup_chosen.as_ref() == Some(&copy.path);
                let painter = ui.painter();
                if chosen {
                    painter.rect_filled(rect, CornerRadius::ZERO, theme::hover());
                } else if response.hovered() {
                    painter.rect_filled(
                        rect,
                        CornerRadius::ZERO,
                        theme::alpha(theme::hover(), 0.5),
                    );
                }
                if index < last {
                    painter.hline(
                        rect.x_range(),
                        rect.bottom() - 0.5,
                        Stroke::new(1.0, theme::line_soft()),
                    );
                }
                let y = rect.center().y;
                let mut x = rect.left() + 12.0 + 8.0;
                let galley = shell::elided(
                    ui,
                    time,
                    theme::medium(theme::BODY),
                    theme::text(),
                    when - 16.0,
                );
                shell::paint_line(ui, galley, x, y);
                x += when;
                // Why, on two lines when there is a second.
                let title_y = if sub.is_empty() { y } else { y - 8.0 };
                let galley = shell::elided(
                    ui,
                    title,
                    theme::regular(theme::BODY),
                    theme::text(),
                    why - 16.0,
                );
                let title_right = x + galley.size().x;
                shell::paint_line(ui, galley, x, title_y);
                if matches!(copy.origin, Origin::Current { .. }) {
                    let chip = Rect::from_min_size(
                        Pos2::new(title_right + 8.0, title_y - 9.0),
                        Vec2::new(64.0, 18.0),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(chip)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    theme::Chip::new("Current")
                        .mood(Mood::Ok)
                        .height(18.0)
                        .show(&mut child);
                }
                if !sub.is_empty() {
                    let galley = shell::elided(
                        ui,
                        sub,
                        theme::regular(theme::CAPTION + 0.5),
                        theme::muted(),
                        why - 16.0,
                    );
                    shell::paint_line(ui, galley, x, y + 9.0);
                }
                x += why;
                if holds > 0.0 {
                    let galley = shell::elided(
                        ui,
                        copy.holds(),
                        theme::regular(theme::BODY),
                        theme::muted(),
                        holds - 16.0,
                    );
                    shell::paint_line(ui, galley, x, y);
                    x += holds;
                }
                let (icon, words, ink) = if copy.complete {
                    (Icon::Check, "Complete", theme::ok())
                } else {
                    (Icon::CircleAlert, "Incomplete", theme::hot())
                };
                theme::paint_icon(ui, icon, Pos2::new(x + 6.5, y), 13.0, ink);
                let galley = shell::galley(ui, words, theme::regular(theme::BODY), ink);
                shell::paint_line(ui, galley, x + 18.0, y);
                let response = if copy.complete {
                    response
                } else {
                    response.on_hover_text(
                        "Something the copy names is missing from it, so it may not restore",
                    )
                };
                if response.clicked() {
                    picked = Some(copy.path.clone());
                }
            }
        });
        if let Some(path) = picked {
            self.backup_chosen = Some(path);
        }
    }

    /// How the pedal is protected, with a backup to a file and a restore
    /// from one.
    fn backup_banner(&mut self, ui: &mut Ui) {
        let device = self.device.clone();
        let live = matches!(self.connection, Connection::Online);
        let (mood, icon, title, body) = match self.backup_of_this_pedal() {
            Some(manifest) => {
                let slots: usize = manifest.setlist_presets().map(<[String]>::len).sum();
                let presets = if manifest.setlists.len() > 1 {
                    format!("{slots} presets in {} setlists", manifest.setlists.len())
                } else {
                    format!("{slots} presets")
                };
                (
                    Mood::Ok,
                    Icon::ShieldCheck,
                    format!("Everything on this {device} is backed up"),
                    format!(
                        "TonePush reads the whole pedal when it connects and keeps that copy \
                         current after every save: {presets}, {} impulse responses and {} \
                         settings, byte for byte.",
                        manifest.irs.len(),
                        manifest.globals,
                    ),
                )
            }
            None => (
                Mood::Info,
                Icon::Shield,
                "Not backed up yet".to_owned(),
                "TonePush reads the whole pedal as soon as it connects, and keeps that copy \
                 current after every save."
                    .to_owned(),
            ),
        };
        let mut backup = false;
        let mut restore = false;
        theme::banner(ui, mood, icon, &title, &body, |ui| {
            backup = theme::Button::new("Back up to a file…")
                .small()
                .icon(Icon::Download)
                .enabled(live)
                .show(ui)
                .on_hover_text("Save every preset, setting and impulse response to a folder")
                .clicked();
            restore = theme::Button::new("Restore from a file…")
                .small()
                .icon(Icon::History)
                .enabled(live)
                .show(ui)
                .on_hover_text("Replace the pedal with a complete backup")
                .clicked();
        });
        if backup {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Where to put the backup")
                .set_file_name(format!("{}.hxbundle", crate::sanitise(&self.device)))
                .save_file()
            {
                self.note("backing up the pedal".to_owned());
                self.send(Cmd::BackUp(dir));
            }
        }
        if restore {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Choose a backup to restore")
                .pick_folder()
            {
                match hx_usb::backup::open(&dir) {
                    // The pedal refuses another kind's backup; say so before
                    // asking anything.
                    Ok(manifest) if manifest.device.trim() != self.device.trim() => {
                        self.problem(format!(
                            "That backup is from {}, not this {}",
                            if manifest.device.trim().is_empty() {
                                "another pedal".to_owned()
                            } else {
                                format!("a {}", manifest.device.trim())
                            },
                            self.device.trim()
                        ));
                    }
                    Ok(manifest) => {
                        let what = dir
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "the backup".to_owned());
                        self.restoring = Some(Restoring {
                            path: dir,
                            manifest,
                            what,
                            differ: None,
                        });
                    }
                    Err(e) => self.problem(format!("That is not a backup: {e}")),
                }
            }
        }
    }

    /// The chosen copy: when and why, how it differs from the pedal now, and
    /// putting it back whole.
    pub(crate) fn backup_inspector(&mut self, ui: &mut Ui) {
        let Some(copy) = self.chosen_backup().cloned() else {
            return;
        };
        let (_, sub) = self.backup_reason(&copy);
        let current = matches!(copy.origin, Origin::Current { .. });
        let width = ui.available_width();
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        // The head: when, and why in a sentence.
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 16,
                bottom: 12,
            })
            .show(ui, |ui| {
                ui.set_width(width - 32.0);
                theme::label(
                    ui,
                    &when_words(copy.when),
                    theme::semibold(theme::TITLE),
                    theme::text(),
                );
                ui.add_space(3.0);
                let sentence = match &copy.origin {
                    Origin::Current { .. } => "The automatic backup, kept current".to_owned(),
                    Origin::File => format!("Saved to {sub}"),
                    Origin::SetAside(Some(Why::Connected)) => {
                        "Set aside as the pedal connected".to_owned()
                    }
                    Origin::SetAside(_) => format!("Taken {}", lower_first(&sub)),
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(sentence)
                            .font(theme::regular(theme::SECONDARY - 0.5))
                            .color(theme::muted()),
                    )
                    .wrap(),
                );
            });
        divider(ui, theme::line());
        let differences = self.backup_differences(&copy);
        let mut restore = false;
        let mut show = false;
        egui::ScrollArea::vertical()
            .id_salt("backup-inspector")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);
                        caption(ui, "Against the pedal now");
                        ui.add_space(9.0);
                        match &differences {
                            None if current => {
                                compare_line(ui, Icon::Check, "This is the pedal as it is now.", "")
                            }
                            None => compare_line(
                                ui,
                                Icon::CircleDashed,
                                "Nothing to compare with",
                                "until the pedal is backed up.",
                            ),
                            Some((found, same)) if found.is_empty() => compare_line(
                                ui,
                                Icon::Check,
                                "The same as the pedal.",
                                &format!("All {same} slots match."),
                            ),
                            Some((found, same)) => {
                                compare_line(
                                    ui,
                                    Icon::GitCompare,
                                    &format!(
                                        "{} {} differ.",
                                        found.len(),
                                        if found.len() == 1 {
                                            "preset"
                                        } else {
                                            "presets"
                                        }
                                    ),
                                    &format!("{same} are the same."),
                                );
                                ui.add_space(8.0);
                                for difference in found {
                                    let label =
                                        self.backup_slot(difference.setlist, difference.slot);
                                    difference_row(ui, &label, difference);
                                }
                            }
                        }
                    });
                divider(ui, theme::line_soft());
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let live = matches!(self.connection, Connection::Online);
                        let other = copy.manifest.device.trim() != self.device.trim();
                        let hint = if current {
                            "This copy is the pedal as it is now"
                        } else if other {
                            "This copy is from another kind of pedal"
                        } else if !copy.complete {
                            "Something this copy names is missing from it"
                        } else {
                            "Connect the pedal first"
                        };
                        restore = theme::Button::new("Restore the whole pedal…")
                            .icon(Icon::History)
                            .min_width(width - 32.0)
                            .enabled(live && !current && !other && copy.complete)
                            .show(ui)
                            .on_hover_text("Put every preset, impulse response and setting back")
                            .on_disabled_hover_text(hint)
                            .clicked();
                        show = theme::Button::new("Show in folder")
                            .ghost()
                            .icon(Icon::FolderOpen)
                            .min_width(width - 32.0)
                            .show(ui)
                            .clicked();
                    });
            });
        if restore {
            self.restoring = Some(Restoring {
                path: copy.path.clone(),
                manifest: copy.manifest.clone(),
                what: format!("the copy from {}", when_words(copy.when)),
                differ: differences.map(|(found, _)| found.len()),
            });
        }
        if show {
            if let Err(why) = show_in_folder(&copy.path) {
                self.note(format!("could not open {}: {why}", copy.path.display()));
            }
        }
    }

    /// The right of the Backups tab, when there is a copy to show.
    pub(crate) fn backup_inspector_panel(&mut self, root: &mut Ui, tier: Tier) {
        self.read_backups();
        if self.chosen_backup().is_none() {
            return;
        }
        egui::Panel::right("backup-inspector")
            .resizable(false)
            .exact_size(Self::backup_inspector_width(tier))
            .frame(
                // The panel's own separator is the one line on its left.
                egui::Frame::new().fill(theme::bg()),
            )
            .show(root, |ui| self.backup_inspector(ui));
    }

    /// Ask before writing a whole copy over the pedal.
    pub(crate) fn confirm_restore_window(&mut self, ctx: &egui::Context) {
        let Some(restoring) = self.restoring.clone() else {
            return;
        };
        let pedal = if self.device.trim().is_empty() {
            "the pedal".to_owned()
        } else {
            format!("the {}", self.device.trim())
        };
        let manifest = &restoring.manifest;
        let presets: usize = manifest
            .setlist_presets()
            .flatten()
            .filter(|name| !shell::hx_slot_is_empty(name))
            .count();
        let mut rows = vec![theme::Outcome {
            icon: Icon::Download,
            count: presets.to_string(),
            sentence: if presets == 1 {
                "preset written into its slot; the other slots are emptied".to_owned()
            } else {
                "presets written, each into its slot; the other slots are emptied".to_owned()
            },
            quiet: false,
        }];
        if let Some(differ) = restoring.differ {
            rows.push(theme::Outcome {
                icon: Icon::GitCompare,
                count: differ.to_string(),
                sentence: if differ == 1 {
                    "differs from the pedal now".to_owned()
                } else {
                    "differ from the pedal now".to_owned()
                },
                quiet: differ == 0,
            });
        }
        rows.push(theme::Outcome {
            icon: Icon::FileAudio,
            count: manifest.irs.len().to_string(),
            sentence: "impulse responses put back; the other IR slots are emptied".to_owned(),
            quiet: manifest.irs.is_empty(),
        });
        rows.push(theme::Outcome {
            icon: Icon::SlidersHorizontal,
            count: manifest.globals.to_string(),
            sentence: "settings put back".to_owned(),
            quiet: true,
        });
        let mut explanation = format!(
            "TonePush writes {} over everything on {pedal}: presets, impulse responses and \
             settings. Undo cannot reach the pedal's memory; the pedal as it is now stays in \
             its backups.",
            restoring.what
        );
        if !manifest.firmware.is_empty()
            && !self.firmware.is_empty()
            && manifest.firmware != self.firmware
        {
            explanation.push_str(&format!(
                " The copy was taken on firmware {}; the pedal runs {}.",
                manifest.firmware, self.firmware
            ));
        }
        let mut decided = None;
        let (_, close) = theme::dialog(ctx, "confirm-restore", 520.0, |ui| {
            theme::dialog_header(ui, &format!("Restore {pedal}?"), Some(&explanation));
            theme::dialog_body(ui, |ui| theme::outcomes(ui, &rows));
            theme::dialog_footer(ui, "Keep the pedal connected until it is done.", |ui| {
                if theme::Button::new("Restore the whole pedal")
                    .danger()
                    .show(ui)
                    .clicked()
                {
                    decided = Some(true);
                }
                if theme::Button::new("Cancel").show(ui).clicked() {
                    decided = Some(false);
                }
            });
        });
        if close && decided.is_none() {
            decided = Some(false);
        }
        match decided {
            Some(true) => {
                self.restoring = None;
                self.note(format!(
                    "restoring {presets} presets from {}",
                    restoring.path.display()
                ));
                self.send(Cmd::RestoreAll(restoring.path));
            }
            Some(false) => self.restoring = None,
            None => {}
        }
    }

    /// Remember a backup saved to a file, for the history, newest first.
    pub(crate) fn remember_backup_file(&mut self, dir: &Path) {
        if session_automatic().is_some_and(|automatic| automatic == dir) {
            return;
        }
        let files = &mut self.config.backup_files;
        files.retain(|known| known != dir);
        files.insert(0, dir.to_owned());
        files.truncate(KEEP_FILES);
        self.config.save();
    }
}

/// How many backups saved to files the history remembers.
const KEEP_FILES: usize = 20;

fn session_automatic() -> Option<PathBuf> {
    crate::session::automatic_dir()
}

fn time_of(seconds: u64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
}

/// "Before Album…" to "before Album…".
fn lower_first(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// A line across the inspector.
fn divider(ui: &mut Ui, colour: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, colour));
}

/// An inspector section's caption, spaced capitals in the muted ink.
fn caption(ui: &mut Ui, text: &str) {
    let galley = ui.painter().layout_job(theme::paint::spaced(
        &text.to_uppercase(),
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter()
        .galley(rect.min, galley, egui::Color32::PLACEHOLDER);
}

/// The comparison's sentence: its first part strong, the rest soft.
fn compare_line(ui: &mut Ui, icon: Icon, strong: &str, rest: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (spot, _) = ui.allocate_exact_size(Vec2::new(22.0, 18.0), Sense::hover());
        theme::paint_icon(
            ui,
            icon,
            Pos2::new(spot.left() + 7.0, spot.center().y),
            14.0,
            theme::text_soft(),
        );
        theme::label(ui, strong, theme::semibold(theme::SECONDARY), theme::text());
        if !rest.is_empty() {
            theme::label(
                ui,
                &format!(" {rest}"),
                theme::regular(theme::SECONDARY),
                theme::text_soft(),
            );
        }
    });
}

/// One slot that differs: its label, the preset, and what changed.
fn difference_row(ui: &mut Ui, label: &str, difference: &Difference) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
    let y = rect.center().y;
    let galley = shell::galley(ui, label, theme::medium(theme::SECONDARY), theme::muted());
    let label_width = galley.size().x.max(28.0);
    shell::paint_line(ui, galley, rect.left(), y);
    let left = rect.left() + label_width + 10.0;
    let room = rect.right() - left;
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &difference.name,
        0.0,
        egui::TextFormat::simple(theme::regular(theme::SECONDARY), theme::text()),
    );
    job.append(
        &difference.words(),
        5.0,
        egui::TextFormat::simple(theme::regular(theme::SECONDARY), theme::muted()),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: room.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    shell::paint_line(ui, galley, left, y);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(names: &[&str], hashes: &[(usize, &str)]) -> Held {
        Held {
            names: vec![names.iter().map(|name| (*name).to_owned()).collect()],
            hashes: hashes
                .iter()
                .map(|(slot, hash)| ((0, *slot), (*hash).to_owned()))
                .collect(),
        }
    }

    /// Slot by slot, said from now: edited, another preset, added, emptied,
    /// and the same, including empty on both sides.
    #[test]
    fn a_copy_is_compared_with_the_pedal_slot_by_slot() {
        let then = held(
            &[
                "Glass",
                "Ambient",
                "Night Verb",
                "",
                "Practice",
                "Tape",
                "New Preset",
            ],
            &[(0, "g"), (1, "a1"), (2, "n"), (4, "p"), (5, "t")],
        );
        let now = held(
            &[
                "Glass",
                "Ambient",
                "Sparkle Verb",
                "Reverse",
                "",
                "Tape",
                "",
            ],
            &[(0, "g"), (1, "a2"), (2, "s"), (3, "r"), (5, "t")],
        );
        let (found, same) = differences(&then, &now);
        assert_eq!(same, 3, "Glass, Tape, and the slot empty on both sides");
        let words: Vec<(usize, String, String)> = found
            .iter()
            .map(|d| (d.slot, d.name.clone(), d.words()))
            .collect();
        assert_eq!(
            words,
            vec![
                (1, "Ambient".to_owned(), "edited since".to_owned()),
                (2, "Sparkle Verb".to_owned(), "was Night Verb".to_owned()),
                (3, "Reverse".to_owned(), "added since".to_owned()),
                (4, "Practice".to_owned(), "removed since".to_owned()),
            ]
        );
    }

    /// The copy names its time in UTC, most significant first, as the
    /// worker writes it.
    #[test]
    fn a_set_aside_copy_says_when_it_was_set_aside() {
        for seconds in [0, 1_759_600_920, 4_102_444_799] {
            assert_eq!(
                seconds_of_stamp(&crate::session::stamp_of(seconds)),
                Some(seconds)
            );
        }
        assert_eq!(seconds_of_stamp("not a time"), None);
    }

    #[test]
    fn times_read_as_today_yesterday_or_a_date() {
        let at = |text: &str| -> jiff::Zoned {
            jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M", text)
                .unwrap()
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap()
        };
        let now = at("2026-10-04 20:00");
        assert_eq!(when_words_at(&at("2026-10-04 14:02"), &now), "Today, 14:02");
        assert_eq!(
            when_words_at(&at("2026-10-03 21:11"), &now),
            "Yesterday, 21:11"
        );
        assert_eq!(
            when_words_at(&at("2026-09-28 10:02"), &now),
            "28 Sep, 10:02"
        );
        assert_eq!(
            when_words_at(&at("2025-12-24 09:30"), &now),
            "24 Dec 2025, 09:30"
        );
    }

    /// Why a copy was set aside travels inside it, and a copy without the
    /// note still reads, saying less.
    #[test]
    fn a_copy_keeps_why_it_was_set_aside() {
        let dir = std::env::temp_dir().join(format!("tonepush-why-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(why_of(&dir), None);
        let why = Why::Setlist {
            name: "Album release show".to_owned(),
        };
        note_why(&dir, &why).unwrap();
        assert_eq!(why_of(&dir), Some(why));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Holds narrows away before the reason does.
    #[test]
    fn the_history_drops_holds_before_squeezing_why() {
        let [_, why, holds, _] = columns(668.0);
        assert_eq!(holds, 150.0);
        assert!(why >= 250.0);
        let [_, why, holds, _] = columns(480.0);
        assert_eq!(holds, 0.0);
        assert!(why >= 200.0);
    }
}
