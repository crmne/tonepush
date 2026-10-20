//! Right-click menus and the keys beside them, in one order on every
//! surface: hear it, put it on the pedal, keep it; then its name and
//! versions; then out of this computer; delete last, alone, in red. The same
//! action has the same words, icon and key everywhere, and an item a tone
//! cannot use says why on its right rather than disappearing
//! (docs/design/library-workflow-2026-10-02, 17 "Right-click menus").

use egui::Pos2;

use crate::audition::Arrows;
use crate::cloud;
use crate::devices::{self, Marker, Refusal, Verdict};
use crate::library;
use crate::theme::{self, Icon, Tier};
use crate::{App, CloudAction, LibColumn, LibraryView};

/// What a row menu is for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuFor {
    /// Library tones, by row: one, or several chosen.
    Tones(Vec<usize>),
    /// A TonePush tone, by its row in the Cloud.
    Cloud(usize),
    /// A setlist, by its place in the library's list.
    Setlist(usize),
    /// One slot of a setlist.
    SetlistSlot { setlist: usize, slot: usize },
    /// One of your tones on TonePush, by its id there.
    Mine(i64),
    /// One of your setlists on TonePush, by its id there.
    MineSetlist(i64),
}

/// What a menu asked for, carried out once it has closed.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Play(usize),
    PlayVersion(usize, String, u32),
    Put(Vec<usize>),
    Rename(usize),
    Publish(Vec<usize>),
    Open(String),
    Export(Vec<usize>),
    ShowInFolder(usize),
    Delete(Vec<usize>),
    CloudPlay(usize),
    CloudVersion(usize, u32),
    CloudPut(usize),
    CloudKeep(usize),
    CopyLink(String),
    SetlistPut(usize),
    SetlistCapture(usize),
    SetlistRename(usize),
    SetlistShow(usize),
    SetlistExport(usize),
    SetlistDelete(usize),
    SlotPlay(usize, usize),
    SlotSend(usize, usize),
    SlotElsewhere(usize, usize),
    SlotShow(usize, usize),
    MinePlay(i64),
    MinePut(i64),
    MineShow(i64),
    MineRename(i64),
    PublishHash(String),
    MakeCurrent(String, u32),
    ToneVisibility(i64, cloud::Visibility),
    DeleteYours(crate::account::Yours),
    SetlistVisibility(i64, cloud::Visibility),
    PublishSetlist(usize),
}

fn menu_id() -> egui::Id {
    egui::Id::new("tonepush-row-menu")
}

/// Why a tone cannot play, in the few words a menu row has room for at its
/// right: "for PRO 2.x", "needs firmware 2.x".
pub(crate) fn short_refusal(refusal: &Refusal, tone: &Marker) -> String {
    match refusal {
        Refusal::Family | Refusal::Model => match tone.family {
            devices::Family::Pro => format!("for PRO {}", tone.model),
            devices::Family::Hx => format!("for {}", tone.device_name()),
        },
        Refusal::NeedsRouting => "needs firmware 2.x".to_owned(),
        Refusal::Firmware { made, .. } => format!("made on {made}"),
    }
}

impl App {
    /// Open a row's menu at a point: the pointer for a right-click, the
    /// row's start for Shift F10.
    pub(crate) fn open_row_menu(&mut self, ctx: &egui::Context, what: MenuFor, at: Pos2) {
        self.row_menu = Some((what, at));
        egui::Popup::open_id(ctx, menu_id());
    }

    /// The open row menu, over everything; what it asks is done once it
    /// closes.
    pub(crate) fn row_menu_window(&mut self, ctx: &egui::Context) {
        let Some((what, at)) = self.row_menu.clone() else {
            return;
        };
        if !egui::Popup::is_id_open(ctx, menu_id()) {
            self.row_menu = None;
            return;
        }
        let mut act = None;
        let shown = egui::Popup::new(
            menu_id(),
            ctx.clone(),
            egui::PopupAnchor::Position(at),
            egui::LayerId::new(egui::Order::Foreground, menu_id()),
        )
        .kind(egui::PopupKind::Menu)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .style(egui::containers::menu::menu_style)
        .open_memory(None)
        .show(|ui| {
            theme::menu_width(ui, 292.0);
            act = match &what {
                MenuFor::Tones(rows) if rows.len() > 1 => self.several_menu(ui, rows),
                MenuFor::Tones(rows) => rows.first().and_then(|&row| self.tone_menu(ui, row)),
                MenuFor::Cloud(row) => self.cloud_menu(ui, *row),
                MenuFor::Setlist(index) => self.setlist_menu(ui, *index),
                MenuFor::SetlistSlot { setlist, slot } => {
                    self.setlist_slot_menu(ui, *setlist, *slot)
                }
                MenuFor::Mine(tone_id) => self.mine_menu(ui, *tone_id),
                MenuFor::MineSetlist(id) => self.mine_setlist_menu(ui, *id),
            };
        });
        if shown.is_none() {
            self.row_menu = None;
        }
        if let Some(act) = act {
            egui::Popup::close_id(ctx, menu_id());
            self.row_menu = None;
            self.carry_out(act, ctx);
        }
    }

    /// Why the pedal connected cannot play a tone of this marker: the few
    /// words for a menu row and the sentence for its hover. No pedal is a
    /// reason too, here.
    fn menu_refusal(
        &self,
        marker: Option<&Marker>,
        firmware: &str,
        pro: bool,
    ) -> Option<(String, String)> {
        let Some(pedal) = self.pedal() else {
            return Some((
                "no pedal".to_owned(),
                "Connect a pedal to play it".to_owned(),
            ));
        };
        let full = self.refusal_hint(marker, firmware, pro)?;
        let short = match marker {
            Some(marker) => match devices::verdict(&pedal, marker, firmware) {
                Verdict::Refused(refusal) => short_refusal(&refusal, marker),
                _ => "another pedal's tone".to_owned(),
            },
            None => "another pedal's tone".to_owned(),
        };
        Some((short, full))
    }

    /// The menu of one library tone.
    fn tone_menu(&self, ui: &mut egui::Ui, row: usize) -> Option<Act> {
        let entry = self.lib_entries.get(row)?;
        let mut act = None;
        let aside = match &entry.marker {
            Some(marker) => format!("{} · library", marker.device_name()),
            None => "library".to_owned(),
        };
        theme::menu_header(ui, &entry.name, Some(&aside));
        let refused = self.menu_refusal(entry.marker.as_ref(), &entry.firmware, entry.pro);
        if keyed(
            ui,
            Icon::Volume,
            &format!("Play on {}", self.device_words()),
            &["Space"],
            refused.as_ref(),
        ) {
            act = Some(Act::Play(row));
        }
        let put_refused = refused.clone().or_else(|| self.put_menu_refusal());
        if keyed(
            ui,
            Icon::ArrowDownToLine,
            "Put in a slot…",
            &["Ctrl", "Enter"],
            put_refused.as_ref(),
        ) {
            act = Some(Act::Put(vec![row]));
        }
        theme::menu_separator(ui);
        if keyed(ui, Icon::TextCursorInput, "Rename", &["F2"], None) {
            act = Some(Act::Rename(row));
        }
        let history = library::versions_of(&entry.hash);
        if history.len() > 1 {
            theme::menu_submenu(ui, Icon::History, "Versions", |ui| {
                theme::menu_width(ui, 220.0);
                for version in history.iter().rev() {
                    let current = version.hash == entry.hash;
                    let words = format!(
                        "v{}  ·  {}",
                        version.version,
                        crate::day_month(&version.meta.modified_at)
                    );
                    if theme::menu_item(
                        ui,
                        Some(Icon::Volume),
                        &words,
                        current.then_some("this one"),
                    )
                    .clicked()
                    {
                        act = Some(if current {
                            Act::Play(row)
                        } else {
                            Act::PlayVersion(row, version.hash.clone(), version.version)
                        });
                    }
                }
            });
        } else {
            theme::menu_disabled(ui, Some(Icon::History), "Versions", Some("only this one"));
        }
        theme::menu_separator(ui);
        if theme::menu_item(ui, Some(Icon::CloudUpload), "Publish on TonePush…", None).clicked() {
            act = Some(Act::Publish(vec![row]));
        }
        match self.published_url(&entry.hash) {
            Some(url) => {
                if theme::menu_item(ui, Some(Icon::ExternalLink), "Open on tonepush.rocks", None)
                    .clicked()
                {
                    act = Some(Act::Open(url));
                }
            }
            None => {
                theme::menu_disabled(
                    ui,
                    Some(Icon::ExternalLink),
                    "Open on tonepush.rocks",
                    Some("not published"),
                );
            }
        }
        if theme::menu_item(ui, Some(Icon::FileDown), "Export…", None).clicked() {
            act = Some(Act::Export(vec![row]));
        }
        if theme::menu_item(ui, Some(Icon::FolderOpen), "Show in folder", None).clicked() {
            act = Some(Act::ShowInFolder(row));
        }
        theme::menu_separator(ui);
        if danger_keyed(ui, "Delete…", &["Del"]) {
            act = Some(Act::Delete(vec![row]));
        }
        act
    }

    /// The menu of several library tones, chosen with Ctrl or Shift.
    fn several_menu(&self, ui: &mut egui::Ui, rows: &[usize]) -> Option<Act> {
        let mut act = None;
        let count = rows.len();
        theme::menu_header(ui, &format!("{count} tones"), Some("in your library"));
        let refused = self.put_menu_refusal();
        if keyed(
            ui,
            Icon::ArrowDownToLine,
            "Put in slots…",
            &["Ctrl", "Enter"],
            refused.as_ref(),
        ) {
            act = Some(Act::Put(rows.to_vec()));
        }
        theme::menu_separator(ui);
        if theme::menu_item(
            ui,
            Some(Icon::CloudUpload),
            &format!("Publish {count} on TonePush…"),
            None,
        )
        .clicked()
        {
            act = Some(Act::Publish(rows.to_vec()));
        }
        if theme::menu_item(ui, Some(Icon::FileDown), &format!("Export {count}…"), None).clicked()
        {
            act = Some(Act::Export(rows.to_vec()));
        }
        theme::menu_separator(ui);
        if danger_keyed(ui, &format!("Delete {count} tones…"), &["Del"]) {
            act = Some(Act::Delete(rows.to_vec()));
        }
        act
    }

    /// The menu of someone's tone on TonePush: nothing to rename or delete.
    fn cloud_menu(&self, ui: &mut egui::Ui, row: usize) -> Option<Act> {
        let entry = self.cloud_entries.get(row)?;
        let tone = &entry.discovered.tone;
        let mut act = None;
        let by = tone
            .summary
            .creator
            .as_deref()
            .map(str::trim)
            .filter(|creator| !creator.is_empty())
            .map(|creator| format!("by {creator}"));
        theme::menu_header(ui, &tone.summary.name, by.as_deref());
        let refused = self.cloud_menu_refusal(&entry.discovered);
        if keyed(
            ui,
            Icon::Volume,
            &format!("Play on {}", self.device_words()),
            &["Space"],
            refused.as_ref(),
        ) {
            act = Some(Act::CloudPlay(row));
        }
        let put_refused = refused.clone().or_else(|| self.put_menu_refusal());
        if keyed(
            ui,
            Icon::ArrowDownToLine,
            "Put in a slot…",
            &["Ctrl", "Enter"],
            put_refused.as_ref(),
        ) {
            act = Some(Act::CloudPut(row));
        }
        if keyed(
            ui,
            Icon::CloudDownload,
            "Keep in your library",
            &["Ctrl", "D"],
            None,
        ) {
            act = Some(Act::CloudKeep(row));
        }
        theme::menu_separator(ui);
        if tone.versions.len() > 1 {
            theme::menu_submenu(ui, Icon::History, "Versions", |ui| {
                theme::menu_width(ui, 220.0);
                for version in tone.versions.iter().rev() {
                    let words = format!(
                        "v{}  ·  {}",
                        version.number,
                        crate::day_month(&version.created_at)
                    );
                    if theme::menu_item(
                        ui,
                        Some(Icon::Volume),
                        &words,
                        version.current.then_some("current"),
                    )
                    .clicked()
                    {
                        act = Some(if version.current {
                            Act::CloudPlay(row)
                        } else {
                            Act::CloudVersion(row, version.number)
                        });
                    }
                }
            });
        } else {
            // The feed may count versions it does not list; their page does.
            let hint = if tone.summary.versions_count > 1 {
                "on its page"
            } else {
                "only this one"
            };
            theme::menu_disabled(ui, Some(Icon::History), "Versions", Some(hint));
        }
        if let Some(url) = tone.file_sha256.as_deref().map(crate::cloud::tone_url) {
            if theme::menu_item(ui, Some(Icon::ExternalLink), "Open on tonepush.rocks", None)
                .clicked()
            {
                act = Some(Act::Open(url.clone()));
            }
            if theme::menu_item(ui, Some(Icon::Link), "Copy link", None).clicked() {
                act = Some(Act::CopyLink(url));
            }
        }
        act
    }

    /// The menu of a setlist: a record of a night, so putting it on the
    /// pedal and capturing the pedal as its next version, never editing it.
    fn setlist_menu(&self, ui: &mut egui::Ui, index: usize) -> Option<Act> {
        let (_, setlist) = self.lib_setlists.get(index)?;
        let mut act = None;
        let aside = match self.setlist_marker(setlist) {
            Some(marker) => format!("{} · {}", marker.device_name(), setlist.filled()),
            None => format!("{} presets", setlist.filled()),
        };
        theme::menu_header(ui, &setlist.name, Some(&aside));
        let live = self.pedal_online();
        let refused = if !live {
            Some((
                "no pedal".to_owned(),
                "Connect a pedal to put it on".to_owned(),
            ))
        } else if !self.setlist_compatible(setlist) {
            Some((
                "another pedal's".to_owned(),
                format!("This setlist is not for the {}", self.device.trim()),
            ))
        } else {
            self.put_menu_refusal()
        };
        if keyed(
            ui,
            Icon::Download,
            &format!("Put on {}…", self.device_words()),
            &[],
            refused.as_ref(),
        ) {
            act = Some(Act::SetlistPut(index));
        }
        let newest = self
            .lib_setlists
            .iter()
            .filter(|(_, known)| known.series == setlist.series)
            .map(|(_, known)| known.revision())
            .max()
            .unwrap_or_else(|| setlist.revision());
        let no_pedal = (!live).then(|| {
            (
                "no pedal".to_owned(),
                "Connect a pedal to capture it".to_owned(),
            )
        });
        if keyed(
            ui,
            Icon::Computer,
            &format!("Capture the pedal as v{}", newest + 1),
            &[],
            no_pedal.as_ref(),
        ) {
            act = Some(Act::SetlistCapture(index));
        }
        theme::menu_separator(ui);
        if keyed(ui, Icon::TextCursorInput, "Rename", &["F2"], None) {
            act = Some(Act::SetlistRename(index));
        }
        let mut versions: Vec<(usize, u32, String)> = self
            .lib_setlists
            .iter()
            .enumerate()
            .filter(|(_, (_, known))| known.series == setlist.series)
            .map(|(at, (_, known))| (at, known.revision(), crate::day_month(&known.modified_at)))
            .collect();
        versions.sort_by_key(|(_, revision, _)| std::cmp::Reverse(*revision));
        if versions.len() > 1 {
            theme::menu_submenu(ui, Icon::History, "Versions", |ui| {
                theme::menu_width(ui, 220.0);
                for (at, revision, date) in &versions {
                    if theme::menu_item(
                        ui,
                        Some(Icon::ListMusic),
                        &format!("v{revision}  ·  {date}"),
                        (*at == index).then_some("this one"),
                    )
                    .clicked()
                    {
                        act = Some(Act::SetlistShow(*at));
                    }
                }
            });
        } else {
            theme::menu_disabled(ui, Some(Icon::History), "Versions", Some("only this one"));
        }
        if theme::menu_item(ui, Some(Icon::FileDown), "Export…", None).clicked() {
            act = Some(Act::SetlistExport(index));
        }
        if self.account.keeps_setlists()
            && theme::menu_item(ui, Some(Icon::CloudUpload), "Publish on TonePush…", None).clicked()
        {
            act = Some(Act::PublishSetlist(index));
        }
        theme::menu_separator(ui);
        if danger_keyed(ui, "Delete…", &["Del"]) {
            act = Some(Act::SetlistDelete(index));
        }
        act
    }

    /// The menu of one slot of a setlist: it plays and goes to the pedal,
    /// and is never edited in place.
    fn setlist_slot_menu(&self, ui: &mut egui::Ui, setlist: usize, slot: usize) -> Option<Act> {
        let (_, list) = self.lib_setlists.get(setlist)?;
        let tone = list.slots.get(slot).filter(|tone| !tone.is_empty())?;
        let mut act = None;
        let label = self.active_slot_label(slot as i64);
        theme::menu_header(ui, &format!("{label} {}", tone.name), Some(&list.name));
        let entry = self.entry_for_hash(&tone.hash, &tone.name);
        let refused = self.menu_refusal(entry.marker.as_ref(), &entry.firmware, entry.pro);
        if keyed(
            ui,
            Icon::Volume,
            &format!("Play on {}", self.device_words()),
            &["Space"],
            refused.as_ref(),
        ) {
            act = Some(Act::SlotPlay(setlist, slot));
        }
        let put_refused = refused.clone().or_else(|| self.put_menu_refusal());
        if keyed(
            ui,
            Icon::ArrowDownToLine,
            &format!("Send to {label}"),
            &[],
            put_refused.as_ref(),
        ) {
            act = Some(Act::SlotSend(setlist, slot));
        }
        if keyed(
            ui,
            Icon::ArrowDownToLine,
            "Put in another slot…",
            &["Ctrl", "Enter"],
            put_refused.as_ref(),
        ) {
            act = Some(Act::SlotElsewhere(setlist, slot));
        }
        theme::menu_separator(ui);
        if self.library_row_of(&tone.hash).is_some() {
            if theme::menu_item(ui, Some(Icon::Computer), "Show in your tones", None).clicked() {
                act = Some(Act::SlotShow(setlist, slot));
            }
        } else {
            theme::menu_disabled(
                ui,
                Some(Icon::Computer),
                "Show in your tones",
                Some("the setlist's own"),
            )
            .on_hover_text("This version of the tone is kept for the setlist only");
        }
        act
    }

    /// The menu of one of your tones on TonePush. Today renaming it and
    /// making another version current publish its file again.
    fn mine_menu(&self, ui: &mut egui::Ui, tone_id: i64) -> Option<Act> {
        let row = self.mine_row(tone_id)?;
        let mut act = None;
        let aside = match row.versions() {
            Some((current, _)) => format!("yours · v{current}"),
            None => "yours".to_owned(),
        };
        theme::menu_header(ui, &row.name(), Some(&aside));
        let discovered = row.discovered();
        let refused = match &discovered {
            Some(tone) => self.cloud_menu_refusal(tone),
            None => Some((
                "asking TonePush".to_owned(),
                "TonePush has not answered for it yet".to_owned(),
            )),
        };
        if keyed(
            ui,
            Icon::Volume,
            &format!("Play on {}", self.device_words()),
            &["Space"],
            refused.as_ref(),
        ) {
            act = Some(Act::MinePlay(tone_id));
        }
        let put_refused = refused.clone().or_else(|| self.put_menu_refusal());
        if keyed(
            ui,
            Icon::ArrowDownToLine,
            "Put in a slot…",
            &["Ctrl", "Enter"],
            put_refused.as_ref(),
        ) {
            act = Some(Act::MinePut(tone_id));
        }
        if row.local.is_some() {
            if theme::menu_item(ui, Some(Icon::Computer), "Show in your tones", None).clicked() {
                act = Some(Act::MineShow(tone_id));
            }
        } else if keyed(
            ui,
            Icon::CloudDownload,
            "Keep in your library",
            &["Ctrl", "D"],
            discovered
                .is_none()
                .then(|| {
                    (
                        "asking TonePush".to_owned(),
                        "TonePush has not answered for it yet".to_owned(),
                    )
                })
                .as_ref(),
        ) {
            act = Some(Act::MineShow(tone_id));
        }
        theme::menu_separator(ui);
        match self.mine_ahead(&row) {
            Some(ahead) => {
                if theme::menu_item(
                    ui,
                    Some(Icon::CloudUpload),
                    &format!("Publish v{} from your library…", ahead.version),
                    None,
                )
                .clicked()
                {
                    act = Some(Act::PublishHash(ahead.hash));
                }
            }
            None => {
                theme::menu_disabled(
                    ui,
                    Some(Icon::CloudUpload),
                    "Publish from your library…",
                    Some(if row.local.is_some() {
                        "the same"
                    } else {
                        "not in this library"
                    }),
                );
            }
        }
        let renamable = self.mine_rename_refusal(&row);
        if keyed(
            ui,
            Icon::TextCursorInput,
            "Rename on TonePush…",
            &["F2"],
            renamable.as_ref(),
        ) {
            act = Some(Act::MineRename(tone_id));
        }
        let others: Vec<cloud::ToneVersion> = discovered
            .as_ref()
            .map(|tone| {
                tone.tone
                    .versions
                    .iter()
                    .filter(|version| !version.current)
                    .rev()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if others.is_empty() {
            theme::menu_disabled(
                ui,
                Some(Icon::History),
                "Make another version current",
                Some("only this one"),
            );
        } else {
            theme::menu_submenu(ui, Icon::History, "Make another version current", |ui| {
                theme::menu_width(ui, 240.0);
                for version in &others {
                    let words = format!(
                        "v{}  ·  {}",
                        version.number,
                        crate::day_month(&version.created_at)
                    );
                    match self.mine_local_version(&row, &version.file_sha256) {
                        Some(hash) => {
                            if theme::menu_item(ui, Some(Icon::History), &words, None).clicked() {
                                act = Some(Act::MakeCurrent(hash, version.number));
                            }
                        }
                        None => {
                            theme::menu_disabled(
                                ui,
                                Some(Icon::History),
                                &words,
                                Some("not in this library"),
                            );
                        }
                    }
                }
            });
        }
        // Who can see it, where the server can change that.
        let other = match self.tone_visibility(tone_id) {
            Some(cloud::Visibility::Everyone) => Some((
                Icon::EyeOff,
                "Hide from everyone",
                cloud::Visibility::OnlyYou,
            )),
            Some(cloud::Visibility::OnlyYou) => {
                Some((Icon::Globe, "Show to everyone", cloud::Visibility::Everyone))
            }
            None => None,
        };
        if let Some((icon, words, visibility)) = other {
            if theme::menu_item(ui, Some(icon), words, None).clicked() {
                act = Some(Act::ToneVisibility(tone_id, visibility));
            }
        }
        theme::menu_separator(ui);
        if let Some(url) = self.mine_url(&row) {
            if theme::menu_item(ui, Some(Icon::ExternalLink), "Open on tonepush.rocks", None)
                .clicked()
            {
                act = Some(Act::Open(url.clone()));
            }
            if theme::menu_item(ui, Some(Icon::Link), "Copy link", None).clicked() {
                act = Some(Act::CopyLink(url));
            }
        }
        if self.account.lists_tones() {
            theme::menu_separator(ui);
            if danger_keyed(ui, "Delete from TonePush…", &["Del"]) {
                act = Some(Act::DeleteYours(crate::account::Yours::Tone(tone_id)));
            }
        }
        act
    }

    /// The menu of one of your setlists on TonePush.
    fn mine_setlist_menu(&self, ui: &mut egui::Ui, id: i64) -> Option<Act> {
        let setlist = self
            .account
            .setlists
            .as_ref()?
            .iter()
            .find(|setlist| setlist.id == id)?
            .clone();
        let mut act = None;
        theme::menu_header(
            ui,
            &setlist.name,
            Some(&format!("yours · {} presets", setlist.slot_count)),
        );
        let local = self.local_setlist_named(&setlist.name);
        let refused = match local {
            None => Some((
                "not in this library".to_owned(),
                "This library has no setlist of that name to put on the pedal".to_owned(),
            )),
            Some(_) if !self.pedal_online() => Some((
                "no pedal".to_owned(),
                "Connect a pedal to put it on".to_owned(),
            )),
            Some(_) => None,
        };
        if keyed(
            ui,
            Icon::Download,
            &format!("Put on {}…", self.device_words()),
            &[],
            refused.as_ref(),
        ) {
            if let Some(index) = local {
                act = Some(Act::SetlistPut(index));
            }
        }
        if let Some(index) = local {
            if theme::menu_item(ui, Some(Icon::ListMusic), "Show in your setlists", None).clicked()
            {
                act = Some(Act::SetlistShow(index));
            }
        }
        theme::menu_separator(ui);
        if setlist.visibility == "only_you" {
            if theme::menu_item(ui, Some(Icon::Globe), "Show to everyone", None).clicked() {
                act = Some(Act::SetlistVisibility(id, cloud::Visibility::Everyone));
            }
        } else if theme::menu_item(ui, Some(Icon::EyeOff), "Hide from everyone", None).clicked() {
            act = Some(Act::SetlistVisibility(id, cloud::Visibility::OnlyYou));
        }
        theme::menu_separator(ui);
        if danger_keyed(ui, "Delete from TonePush…", &["Del"]) {
            act = Some(Act::DeleteYours(crate::account::Yours::Setlist(id)));
        }
        act
    }

    /// Why one of your tones cannot be renamed on TonePush now. Today a
    /// rename publishes its file again, so the library must hold that file.
    pub(crate) fn mine_rename_refusal(
        &self,
        row: &crate::mine::MineRow,
    ) -> Option<(String, String)> {
        if self.account.lists_tones() {
            return None;
        }
        let file = row
            .details
            .as_ref()
            .and_then(|tone| tone.file_sha256.clone());
        match file {
            None => Some((
                "asking TonePush".to_owned(),
                "TonePush has not answered for it yet".to_owned(),
            )),
            Some(file) if self.mine_local_version(row, &file).is_none() => Some((
                "not in this library".to_owned(),
                "Renaming publishes its file again, and this library does not hold it".to_owned(),
            )),
            Some(_) => None,
        }
    }

    /// Rename one of your tones on TonePush: the sheet asks for the name.
    pub(crate) fn rename_on_tonepush(&mut self, tone_id: i64) {
        let Some(row) = self.mine_row(tone_id) else {
            return;
        };
        if self.account.lists_tones() {
            self.publish_ask = Some(crate::publish::Asked::Rename {
                tone_id,
                hash: None,
                draft: row.name(),
            });
            return;
        }
        if let Some((_, why)) = self.mine_rename_refusal(&row) {
            return self.note(why);
        }
        let file = row
            .details
            .as_ref()
            .and_then(|tone| tone.file_sha256.clone())
            .unwrap_or_default();
        if let Some(hash) = self.mine_local_version(&row, &file) {
            self.ask_to_rename_on_tonepush(tone_id, hash, row.name());
        }
    }

    /// Put one of your tones in a slot: from the library when it holds the
    /// file TonePush gives, else kept from TonePush first.
    pub(crate) fn mine_put(&mut self, tone_id: i64, ctx: &egui::Context) {
        let Some(row) = self.mine_row(tone_id) else {
            return;
        };
        let file = row
            .details
            .as_ref()
            .and_then(|tone| tone.file_sha256.clone());
        let local = file
            .and_then(|file| self.mine_local_version(&row, &file))
            .and_then(|hash| self.lib_entries.iter().position(|entry| entry.hash == hash));
        match (local, row.discovered()) {
            (Some(index), _) => self.start_putting(&[index]),
            (None, Some(tone)) => self.start_cloud_entry_action(tone, CloudAction::Put, ctx),
            (None, None) => {}
        }
    }

    /// "the HX Stomp": the pedal connected, for "Play on the HX Stomp".
    pub(crate) fn device_words(&self) -> String {
        match self.device.trim() {
            "" => "the pedal".to_owned(),
            device => format!("the {device}"),
        }
    }

    /// Why nothing can be put in a slot, for a menu row.
    fn put_menu_refusal(&self) -> Option<(String, String)> {
        let why = self.put_refusal()?;
        let short = match why.as_str() {
            "no pedal is connected" => "no pedal".to_owned(),
            other => other.to_owned(),
        };
        Some((short, format!("Nothing can be put in a slot: {why}")))
    }

    /// Why a TonePush tone cannot play here, for a menu row.
    fn cloud_menu_refusal(&self, entry: &crate::cloud::DiscoveredTone) -> Option<(String, String)> {
        let full = self.cloud_audition_blocker(entry)?;
        if !self.pedal_online() {
            return Some(("no pedal".to_owned(), full));
        }
        let marker = Self::cloud_marker(entry);
        let firmware = Self::cloud_firmware(entry);
        let short = match (self.pedal(), marker.as_ref()) {
            (Some(pedal), Some(marker)) => match devices::verdict(&pedal, marker, &firmware) {
                Verdict::Refused(refusal) => short_refusal(&refusal, marker),
                _ => "hosted elsewhere".to_owned(),
            },
            _ => format!("for {}", entry.tone.summary.device.name),
        };
        Some((short, full))
    }

    /// The tone's page on TonePush, once it is published there.
    fn published_url(&self, hash: &str) -> Option<String> {
        let portable = self.portable_hashes.get(hash)?;
        self.cloud_files
            .as_ref()
            .filter(|files| files.contains(portable))
            .map(|_| crate::cloud::tone_url(portable))
    }

    fn carry_out(&mut self, act: Act, ctx: &egui::Context) {
        let tier = Tier::now(ctx);
        match act {
            Act::Play(row) => {
                self.choose_tone(row);
                self.audition_library(row);
            }
            Act::PlayVersion(row, hash, number) => self.audition_version(row, hash, number),
            Act::Put(rows) => self.start_putting(&rows),
            Act::Rename(row) => self.rename_tone(row, tier),
            Act::Publish(rows) => self.publish_rows(&rows),
            Act::Open(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Act::Export(rows) => self.export_rows(&rows),
            Act::ShowInFolder(row) => self.show_tone_in_folder(row),
            Act::Delete(rows) => {
                self.lib_chosen = rows
                    .iter()
                    .filter_map(|&row| self.lib_entries.get(row))
                    .map(|entry| entry.hash.clone())
                    .collect();
                self.ask_to_delete();
            }
            Act::CloudPlay(row) => {
                self.cloud_selected = Some(row);
                self.hearing.arrows = Arrows::Cloud;
                self.start_cloud_action(row, CloudAction::Audition, ctx);
            }
            Act::CloudVersion(row, number) => {
                let Some(entry) = self.cloud_entries.get(row).map(|e| e.discovered.clone()) else {
                    return;
                };
                if let Some(version) = entry
                    .tone
                    .versions
                    .iter()
                    .find(|version| version.number == number)
                {
                    let historical = Self::cloud_version_entry(&entry, version);
                    self.start_cloud_entry_action(historical, CloudAction::Audition, ctx);
                }
            }
            Act::CloudPut(row) => self.start_cloud_action(row, CloudAction::Put, ctx),
            Act::CloudKeep(row) => self.start_cloud_action(row, CloudAction::Computer, ctx),
            Act::CopyLink(url) => {
                ctx.copy_text(url);
                self.note("copied the tone's link".to_owned());
            }
            Act::SetlistPut(index) => {
                self.select_setlist_entry(index);
                self.confirm_push = Some(index);
            }
            Act::SetlistCapture(index) => {
                if let Some(name) = self.lib_setlists.get(index).map(|(_, s)| s.name.clone()) {
                    self.capture_pedal(Some(name));
                }
            }
            Act::SetlistRename(index) => self.rename_setlist(index, tier),
            Act::SetlistShow(index) => {
                self.open_library(LibraryView::Setlists, tier);
                self.select_setlist_entry(index);
            }
            Act::SetlistExport(index) => self.export_setlist(index),
            Act::SetlistDelete(index) => self.confirm_setlist_delete = Some(index),
            Act::SlotPlay(setlist, slot) => {
                if let Some((name, tone)) = self.setlist_slot(setlist, slot) {
                    self.hearing.arrows = Arrows::Presets;
                    self.audition_setlist_slot(&name, slot as i64, &tone);
                }
            }
            Act::SlotSend(setlist, slot) => {
                if let Some((_, tone)) = self.setlist_slot(setlist, slot) {
                    if library::holds(&tone.hash) {
                        self.put_to(slot as i64, vec![(tone.hash, tone.name)]);
                    } else {
                        self.note(format!("{} is missing from the library", tone.name));
                    }
                }
            }
            Act::SlotElsewhere(setlist, slot) => {
                if let Some((_, tone)) = self.setlist_slot(setlist, slot) {
                    match self.put_refusal() {
                        Some(why) => {
                            self.problem(format!("Nothing can be put in a slot: {why}"));
                        }
                        None => {
                            self.sending = Some(crate::put::Sending {
                                tones: vec![(tone.hash, tone.name)],
                            });
                        }
                    }
                }
            }
            Act::MinePlay(tone_id) => {
                self.mine.selected = Some(tone_id);
                self.hearing.arrows = Arrows::Mine;
                self.mine_play(tone_id, ctx);
            }
            Act::MinePut(tone_id) => self.mine_put(tone_id, ctx),
            Act::MineShow(tone_id) => self.mine_show_or_keep(tone_id, ctx),
            Act::MineRename(tone_id) => self.rename_on_tonepush(tone_id),
            Act::PublishHash(hash) => self.ask_to_publish(vec![hash]),
            Act::MakeCurrent(hash, version) => self.ask_to_make_current(hash, version),
            Act::ToneVisibility(id, visibility) => self.set_tone_visibility(id, visibility, ctx),
            Act::SetlistVisibility(id, visibility) => {
                self.set_setlist_visibility(id, visibility, ctx);
            }
            Act::DeleteYours(yours) => self.account.confirm_delete = Some(yours),
            Act::PublishSetlist(index) => {
                self.publish_ask = Some(crate::publish::Asked::Setlist(index));
            }
            Act::SlotShow(setlist, slot) => {
                let row = self
                    .setlist_slot(setlist, slot)
                    .and_then(|(_, tone)| self.library_row_of(&tone.hash));
                if let Some(row) = row {
                    self.library_device_filter = None;
                    self.open_library(LibraryView::Tones, tier);
                    self.choose_tone(row);
                    self.lib_reveal = true;
                }
            }
        }
    }

    /// The library's row for a tone: the tone itself, or the tone an older
    /// version belongs to, whose details list that version.
    pub(crate) fn library_row_of(&self, hash: &str) -> Option<usize> {
        if let Some(row) = self.lib_entries.iter().position(|entry| entry.hash == hash) {
            return Some(row);
        }
        let series = library::versions_of(hash);
        self.lib_entries
            .iter()
            .position(|entry| series.iter().any(|version| version.hash == entry.hash))
    }

    /// A setlist's name and one of its slots.
    fn setlist_slot(&self, setlist: usize, slot: usize) -> Option<(String, library::Slot)> {
        let (_, list) = self.lib_setlists.get(setlist)?;
        let tone = list.slots.get(slot).filter(|tone| !tone.is_empty())?;
        Some((list.name.clone(), tone.clone()))
    }

    /// Rename a tone where it is listed: its name cell becomes a field.
    pub(crate) fn rename_tone(&mut self, row: usize, tier: Tier) {
        let Some(entry) = self.lib_entries.get(row) else {
            return;
        };
        let editing = (entry.hash.clone(), LibColumn::Name, entry.name.clone());
        self.open_library(LibraryView::Tones, tier);
        self.choose_tone(row);
        self.lib_reveal = true;
        self.lib_editing = Some(editing);
    }

    /// Rename a setlist: its name in the page's head takes the keyboard.
    pub(crate) fn rename_setlist(&mut self, index: usize, tier: Tier) {
        self.open_library(LibraryView::Setlists, tier);
        self.select_setlist_entry(index);
        self.hearing.arrows = Arrows::Setlists;
        self.pane.rename_setlist = true;
    }

    /// Show the folder a tone's file is kept in.
    fn show_tone_in_folder(&mut self, row: usize) {
        let Some(dir) = self
            .lib_entries
            .get(row)
            .and_then(|entry| library::object_path(&entry.hash))
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        else {
            return self.note("that tone's file is missing from the library".to_owned());
        };
        if let Err(why) = crate::backups::show_in_folder(&dir) {
            self.problem(format!("could not show the folder: {why}"));
        }
    }

    /// Publish tones, after the sheet asks: one after another, each as the
    /// one before it is answered.
    pub(crate) fn publish_rows(&mut self, rows: &[usize]) {
        let hashes = rows
            .iter()
            .filter_map(|&row| self.lib_entries.get(row))
            .map(|entry| entry.hash.clone())
            .collect();
        self.ask_to_publish(hashes);
    }

    /// Export tones for the web: one asks where, several go into one folder
    /// chosen once.
    pub(crate) fn export_rows(&mut self, rows: &[usize]) {
        if let [row] = rows {
            return self.export_for_the_web(*row);
        }
        let Some(dir) = rfd::FileDialog::new()
            .set_title(format!("Where to put the {} tones", rows.len()))
            .pick_folder()
        else {
            return;
        };
        let done = rows
            .iter()
            .filter(|&&row| self.export_for_the_web_to(row, &dir))
            .count();
        self.note(format!("exported {done} tones to {}", dir.display()));
    }

    /// Export a setlist: a folder named for it, holding each slot's tone as
    /// the file the pedal's own editor loads, named for its slot.
    pub(crate) fn export_setlist(&mut self, index: usize) {
        let Some((_, setlist)) = self.lib_setlists.get(index).cloned() else {
            return;
        };
        let Some(parent) = rfd::FileDialog::new()
            .set_title("Where to put the setlist")
            .pick_folder()
        else {
            return;
        };
        let dir = parent.join(crate::sanitise(&format!(
            "{} v{}",
            setlist.name,
            setlist.revision()
        )));
        match self.write_setlist_files(&setlist, &dir) {
            Ok(count) => self.note(format!(
                "exported {} with {count} tones to {}",
                setlist.name,
                dir.display()
            )),
            Err(why) => self.problem(why),
        }
    }

    /// Write a setlist's tones into a folder, one file a slot. Answers how
    /// many it wrote.
    pub(crate) fn write_setlist_files(
        &self,
        setlist: &library::Setlist,
        dir: &std::path::Path,
    ) -> Result<usize, String> {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("could not make {}: {error}", dir.display()))?;
        let mut written = 0;
        for (slot, tone) in setlist.slots.iter().enumerate() {
            if tone.is_empty() {
                continue;
            }
            let Some(bytes) = library::read(&tone.hash) else {
                continue;
            };
            let kind = library::kind(&tone.hash).unwrap_or_else(|| "hxpreset".to_owned());
            let label = self.active_slot_label(slot as i64);
            let file = dir.join(format!("{label} {}.{kind}", crate::sanitise(&tone.name)));
            library::atomic_write(&file, bytes)
                .map_err(|error| format!("could not write {}: {error}", file.display()))?;
            written += 1;
        }
        Ok(written)
    }

    /// Take one version of a setlist out of the library; the others stay.
    pub(crate) fn remove_setlist_version(&mut self, index: usize) {
        let Some((path, setlist)) = self.lib_setlists.get(index).cloned() else {
            return;
        };
        match library::remove_setlist(&path) {
            Ok(()) => {
                if self.lib_setlist == Some(index) {
                    self.lib_setlist = None;
                }
                self.note(format!("deleted {} v{}", setlist.name, setlist.revision()));
                self.refresh_library();
            }
            Err(why) => self.note(why),
        }
    }

    /// The question before a setlist's version goes.
    pub(crate) fn confirm_setlist_delete_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.confirm_setlist_delete else {
            return;
        };
        let Some((_, setlist)) = self.lib_setlists.get(index).cloned() else {
            self.confirm_setlist_delete = None;
            return;
        };
        let others = self
            .lib_setlists
            .iter()
            .filter(|(_, known)| known.series == setlist.series)
            .count()
            .saturating_sub(1);
        let version = setlist.revision();
        let title = format!("Delete {} v{version}?", setlist.name);
        let body = match others {
            0 => "It is the only version. The tones it plays stay in your library.".to_owned(),
            1 => "The other version stays, and so do the tones this one plays.".to_owned(),
            others => {
                format!("The {others} other versions stay, and so do the tones this one plays.")
            }
        };
        let mut decided = None;
        let (_, close) = theme::dialog(ctx, "confirm-setlist-delete", 440.0, |ui| {
            theme::dialog_header(ui, &title, Some(&body));
            ui.add_space(14.0);
            theme::dialog_footer(ui, "", |ui| {
                if theme::Button::new(&format!("Delete v{version}"))
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
                self.confirm_setlist_delete = None;
                self.remove_setlist_version(index);
            }
            Some(false) => self.confirm_setlist_delete = None,
            None => {}
        }
    }

    /// The keys beside the menus, on the list last clicked: Ctrl Enter
    /// puts in a slot, F2 renames, Del deletes, Ctrl D keeps, Ctrl C and
    /// Ctrl V copy and paste a preset, and Shift F10 opens the row's menu.
    /// Not while a field has the keyboard, a menu is open, a question is
    /// asked or a tone plays (the audition's own keys have them then).
    pub(crate) fn menu_keys(&mut self, ctx: &egui::Context, tier: Tier) {
        if ctx.memory(|memory| memory.focused().is_some())
            || ctx.any_popup_open()
            || self.asking()
            || self.sending.is_some()
            || self.hearing()
        {
            return;
        }
        use egui::{Key, Modifiers};
        let consume = |modifiers: Modifiers, key: Key| {
            ctx.input_mut(|input| input.consume_key(modifiers, key))
        };
        let folded = self.library_folded(tier);
        match self.hearing.arrows {
            Arrows::Tones if self.lib_showing == LibraryView::Tones && !folded => {
                let Some(row) = self
                    .lib_selected
                    .filter(|row| *row < self.lib_entries.len())
                else {
                    return;
                };
                if consume(Modifiers::COMMAND, Key::Enter) {
                    self.start_sending(row);
                } else if consume(Modifiers::NONE, Key::F2) {
                    self.rename_tone(row, tier);
                } else if consume(Modifiers::NONE, Key::Delete) {
                    self.ask_to_delete();
                } else if consume(Modifiers::SHIFT, Key::F10) {
                    let rows = self.put_rows(row);
                    let at = self.keyboard_menu_at(ctx);
                    self.open_row_menu(ctx, MenuFor::Tones(rows), at);
                }
            }
            Arrows::Cloud if self.lib_showing == LibraryView::Cloud && !folded => {
                let Some(row) = self
                    .cloud_selected
                    .filter(|row| *row < self.cloud_entries.len())
                else {
                    return;
                };
                if consume(Modifiers::COMMAND, Key::D) {
                    self.start_cloud_action(row, CloudAction::Computer, ctx);
                } else if consume(Modifiers::COMMAND, Key::Enter) {
                    self.start_cloud_action(row, CloudAction::Put, ctx);
                } else if consume(Modifiers::SHIFT, Key::F10) {
                    let at = self.keyboard_menu_at(ctx);
                    self.open_row_menu(ctx, MenuFor::Cloud(row), at);
                }
            }
            Arrows::Setlists if self.lib_showing == LibraryView::Setlists && !folded => {
                let Some(index) = self
                    .lib_setlist
                    .filter(|index| *index < self.lib_setlists.len())
                else {
                    return;
                };
                if consume(Modifiers::NONE, Key::F2) {
                    self.rename_setlist(index, tier);
                } else if consume(Modifiers::NONE, Key::Delete) {
                    self.confirm_setlist_delete = Some(index);
                } else if consume(Modifiers::SHIFT, Key::F10) {
                    let at = self.keyboard_menu_at(ctx);
                    self.open_row_menu(ctx, MenuFor::Setlist(index), at);
                }
            }
            Arrows::Mine
                if self.lib_showing == LibraryView::Cloud
                    && self.pane.cloud_scope == crate::library_pane::CloudScope::Mine
                    && !folded =>
            {
                // A setlist chosen: its keys are deleting it and its menu.
                if let Some(id) = self.mine.selected_setlist {
                    if consume(Modifiers::NONE, Key::Delete) {
                        self.account.confirm_delete = Some(crate::account::Yours::Setlist(id));
                    } else if consume(Modifiers::SHIFT, Key::F10) {
                        let at = self.keyboard_menu_at(ctx);
                        self.open_row_menu(ctx, MenuFor::MineSetlist(id), at);
                    }
                    return;
                }
                let Some(tone_id) = self.mine.selected else {
                    return;
                };
                if consume(Modifiers::COMMAND, Key::Enter) {
                    self.mine_put(tone_id, ctx);
                } else if consume(Modifiers::NONE, Key::F2) {
                    self.rename_on_tonepush(tone_id);
                } else if consume(Modifiers::COMMAND, Key::D) {
                    if self
                        .mine_row(tone_id)
                        .is_some_and(|row| row.local.is_none())
                    {
                        self.mine_show_or_keep(tone_id, ctx);
                    }
                } else if consume(Modifiers::NONE, Key::Delete) {
                    // Deleting from TonePush needs the server's help.
                    if self.account.lists_tones() {
                        self.account.confirm_delete = Some(crate::account::Yours::Tone(tone_id));
                    }
                } else if consume(Modifiers::SHIFT, Key::F10) {
                    let at = self.keyboard_menu_at(ctx);
                    self.open_row_menu(ctx, MenuFor::Mine(tone_id), at);
                }
            }
            Arrows::Presets => self.preset_keys(ctx),
            _ => {}
        }
    }

    /// Where a menu opened from the keyboard appears: under the start of
    /// the row chosen, or the middle of the window when it is not drawn.
    fn keyboard_menu_at(&self, ctx: &egui::Context) -> Pos2 {
        self.menu_anchor
            .unwrap_or_else(|| ctx.content_rect().center())
    }

    /// The pedal's presets' keys, on the loaded preset: F2 renames, Ctrl C
    /// copies, Ctrl V pastes, Ctrl D keeps it in the library and Shift F10
    /// opens its menu. A StompStation PRO's panel has its own.
    fn preset_keys(&mut self, ctx: &egui::Context) {
        if self.pro_active() {
            self.pro.preset_keys(ctx, &self.library_lookup);
            return;
        }
        if !matches!(self.connection, crate::Connection::Online) {
            return;
        }
        let Some(slot) = self.loaded_in_view() else {
            return;
        };
        use egui::{Key, Modifiers};
        let name = self.presets.get(slot as usize).cloned().unwrap_or_default();
        let (copied, pasted) = clipboard_events(ctx);
        let consume = |modifiers: Modifiers, key: Key| {
            ctx.input_mut(|input| input.consume_key(modifiers, key))
        };
        if copied || consume(Modifiers::COMMAND, Key::C) {
            // The name goes on the system's clipboard too, so Ctrl V reaches
            // the window with something to paste.
            ctx.copy_text(name);
            self.row_action(slot, crate::RowAction::Copy);
        } else if pasted || consume(Modifiers::COMMAND, Key::V) {
            if self.clipboard.is_some() {
                self.row_action(slot, crate::RowAction::Paste);
            }
        } else if consume(Modifiers::NONE, Key::F2) {
            if !crate::shell::hx_slot_is_empty(&name) {
                self.renaming = Some((slot, name));
            }
        } else if consume(Modifiers::COMMAND, Key::D) {
            match self.slot_sync(slot) {
                theme::Sync::Same => self.note(format!("{name} is in your library as it is")),
                theme::Sync::Differs => self.row_action(slot, crate::RowAction::Update),
                _ => self.row_action(slot, crate::RowAction::Keep),
            }
        } else if consume(Modifiers::SHIFT, Key::F10) {
            self.preset_menu_by_key = true;
        }
    }
}

/// Whether Ctrl C or Ctrl V reached the window this frame. The windowing
/// layer turns them into copy and paste rather than keys, and a paste only
/// arrives when the system's clipboard holds text. Taken, so no other
/// widget acts on them too.
pub(crate) fn clipboard_events(ctx: &egui::Context) -> (bool, bool) {
    ctx.input_mut(|input| {
        let mut copied = false;
        let mut pasted = false;
        input.events.retain(|event| match event {
            egui::Event::Copy => {
                copied = true;
                false
            }
            egui::Event::Paste(_) => {
                pasted = true;
                false
            }
            _ => true,
        });
        (copied, pasted)
    })
}

/// A menu row with its keys at the right, or, when it cannot be chosen,
/// the few words that say why and the sentence on hover. Answers whether it
/// was chosen.
fn keyed(
    ui: &mut egui::Ui,
    icon: Icon,
    words: &str,
    keys: &[&str],
    refused: Option<&(String, String)>,
) -> bool {
    match refused {
        Some((short, full)) => {
            theme::menu_disabled(ui, Some(icon), words, Some(short)).on_hover_text(full);
            false
        }
        None => theme::menu_keyed(ui, Some(icon), words, keys, true).clicked(),
    }
}

/// The destructive row, last and in red, with its key.
fn danger_keyed(ui: &mut egui::Ui, words: &str, keys: &[&str]) -> bool {
    let response = theme::menu_danger(ui, Icon::Remove, words);
    if ui.is_rect_visible(response.rect) {
        let width = theme::keycaps_width(ui, keys);
        theme::paint_keycaps(
            ui,
            response.rect.right() - 10.0 - width,
            response.rect.center().y,
            keys,
            true,
        );
    }
    response.clicked()
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

    fn online(app: &mut App) {
        app.connection = Connection::Online;
        app.device = "HX Stomp".into();
        app.firmware = "3.80".into();
        app.library_connected_device = "HX Stomp".into();
        app.preset_index = 1;
        app.preset_name = "Plexi Crunch".into();
        app.presets = (0..16)
            .map(|index| match index {
                1 => "Plexi Crunch".to_owned(),
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

    /// The end of a frame whose output nobody paints.
    fn end(ctx: &egui::Context) {
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
    }

    /// One key press, as a frame's input.
    fn press(ctx: &egui::Context, modifiers: egui::Modifiers, key: egui::Key) {
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
    }

    /// A menu row says in a few words why a tone cannot do something, and
    /// the hover says it in full.
    #[test]
    fn a_refusal_is_said_short_in_the_menu() {
        assert_eq!(
            short_refusal(&Refusal::Family, &Marker::pro("2.x")),
            "for PRO 2.x"
        );
        assert_eq!(
            short_refusal(&Refusal::Model, &Marker::hx("Helix LT")),
            "for Helix LT"
        );
        assert_eq!(
            short_refusal(&Refusal::NeedsRouting, &Marker::pro("2.x")),
            "needs firmware 2.x"
        );
        assert_eq!(
            short_refusal(
                &Refusal::Firmware {
                    made: "3.90".to_owned(),
                    has: "3.80".to_owned()
                },
                &Marker::hx("Stomp"),
            ),
            "made on 3.90"
        );

        let _scratch = library::tests::Scratch::new("menus-refusal");
        let (mut app, _events, _cmds) = app();
        assert_eq!(
            app.menu_refusal(Some(&Marker::hx("Stomp")), "3.80", false)
                .map(|(short, _)| short),
            Some("no pedal".to_owned())
        );
        online(&mut app);
        assert_eq!(
            app.menu_refusal(Some(&Marker::hx("Stomp")), "3.80", false),
            None
        );
        let (short, full) = app
            .menu_refusal(Some(&Marker::pro("2.x")), "2.2", true)
            .expect("a reason");
        assert_eq!(short, "for PRO 2.x");
        assert_eq!(full, "The HX Stomp cannot play a StompStation PRO tone");
    }

    /// The keys act on the list last clicked: on the tones, Ctrl Enter puts
    /// the chosen one in a slot, F2 renames it, Del asks to delete it and
    /// Shift F10 opens its menu.
    #[test]
    fn the_keys_act_on_the_tone_chosen() {
        let _scratch = library::tests::Scratch::new("menus-keys");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        app.lib_entries = vec![
            tone("Slapback Twang", Marker::hx("Stomp")),
            tone("Doom Fuzz", Marker::hx("Stomp")),
        ];
        app.lib_showing = LibraryView::Tones;
        app.hearing.arrows = Arrows::Tones;
        app.lib_selected = Some(1);
        let ctx = egui::Context::default();

        press(&ctx, egui::Modifiers::COMMAND, egui::Key::Enter);
        app.menu_keys(&ctx, Tier::M);
        end(&ctx);
        assert_eq!(
            app.sending.as_ref().map(crate::put::Sending::words),
            Some("Doom Fuzz".to_owned())
        );
        app.sending = None;

        press(&ctx, egui::Modifiers::NONE, egui::Key::F2);
        app.menu_keys(&ctx, Tier::M);
        end(&ctx);
        assert_eq!(
            app.lib_editing
                .as_ref()
                .map(|(_, column, draft)| (*column, draft.clone())),
            Some((LibColumn::Name, "Doom Fuzz".to_owned()))
        );
        app.lib_editing = None;

        press(&ctx, egui::Modifiers::NONE, egui::Key::Delete);
        app.menu_keys(&ctx, Tier::M);
        end(&ctx);
        assert!(app.confirm_delete.is_some(), "Del asks first");
        app.confirm_delete = None;

        press(&ctx, egui::Modifiers::SHIFT, egui::Key::F10);
        app.menu_keys(&ctx, Tier::M);
        end(&ctx);
        assert_eq!(
            app.row_menu.as_ref().map(|(what, _)| what.clone()),
            Some(MenuFor::Tones(vec![1]))
        );
    }

    /// On the pedal's presets, Ctrl C copies the loaded preset and F2
    /// renames it, the same as its menu.
    #[test]
    fn the_keys_act_on_the_loaded_preset() {
        let _scratch = library::tests::Scratch::new("menus-preset-keys");
        let (mut app, _events, cmds) = app();
        online(&mut app);
        app.hearing.arrows = Arrows::Presets;
        let ctx = egui::Context::default();

        press(&ctx, egui::Modifiers::NONE, egui::Key::F2);
        app.menu_keys(&ctx, Tier::M);
        end(&ctx);
        assert_eq!(app.renaming, Some((1, "Plexi Crunch".to_owned())));
        app.renaming = None;

        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        });
        app.menu_keys(&ctx, Tier::M);
        end(&ctx);
        assert!(
            cmds.try_iter().any(|cmd| matches!(cmd, Cmd::CopyPreset)),
            "Ctrl C reads the loaded preset for the clipboard"
        );
    }

    /// A tone's menu does what its row's keys do: Put in a slot starts
    /// picking, Delete asks, Rename opens the name; another pedal's tone is
    /// left out of a put, and says why.
    #[test]
    fn a_menu_choice_does_what_its_key_does() {
        let _scratch = library::tests::Scratch::new("menus-acts");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        app.lib_entries = vec![
            tone("Slapback Twang", Marker::hx("Stomp")),
            tone("Doom Fuzz", Marker::hx("Stomp")),
            tone("Velvet Drive", Marker::pro("2.x")),
        ];
        let ctx = egui::Context::default();
        app.carry_out(Act::Put(vec![0, 1]), &ctx);
        assert_eq!(
            app.sending.as_ref().map(crate::put::Sending::words),
            Some("2 tones".to_owned())
        );
        app.sending = None;
        app.carry_out(Act::Delete(vec![0, 1]), &ctx);
        assert_eq!(
            app.confirm_delete.as_ref().map(|(chosen, _)| chosen.len()),
            Some(2)
        );
        app.confirm_delete = None;
        app.carry_out(Act::Rename(1), &ctx);
        assert_eq!(app.lib_selected, Some(1));
        assert!(app.lib_editing.is_some());
        app.carry_out(Act::Put(vec![2]), &ctx);
        assert!(app.sending.is_none());
    }

    /// A setlist exports as a folder of its tones, each named for its slot;
    /// deleting a version asks, and takes only that version.
    #[test]
    fn a_setlist_exports_as_files_and_deletes_one_version_at_a_time() {
        let _scratch = library::tests::Scratch::new("menus-setlist");
        let (mut app, _events, _cmds) = app();
        online(&mut app);
        let dream = tone("Dream Pop", Marker::hx("Stomp"));
        let mut setlist = library::Setlist {
            name: "Album release show".to_owned(),
            slots: vec![library::Slot::default(); 4],
            ..Default::default()
        };
        setlist.slots[2] = library::Slot {
            hash: dream.hash.clone(),
            name: "Dream Pop".to_owned(),
            file: String::new(),
        };
        let (_, first) = library::save_setlist_version(&setlist).unwrap();
        library::save_setlist_version(&first).unwrap();
        app.lib_setlists = library::setlists();
        assert_eq!(app.lib_setlists.len(), 2);

        let dir =
            std::path::PathBuf::from(std::env::var("TONEPUSH_LIBRARY").unwrap()).join("export");
        let written = app
            .write_setlist_files(&app.lib_setlists[0].1.clone(), &dir)
            .unwrap();
        assert_eq!(written, 1);
        assert!(dir.join("01C Dream Pop.hxpreset").exists());

        let ctx = egui::Context::default();
        app.carry_out(Act::SetlistDelete(0), &ctx);
        assert_eq!(app.confirm_setlist_delete, Some(0), "it asks first");
        app.confirm_setlist_delete = None;
        app.remove_setlist_version(0);
        assert_eq!(library::setlists().len(), 1, "the other version stays");
    }
}
