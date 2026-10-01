//! The Library and Pedal pages (docs/design/redesign-2026-10-01, "One job per
//! surface"). The library used to be a strip docked under the editor, and the
//! pedal three floating windows opened from its name and two icons in the
//! status bar; each is now a page of its own, with the loaded preset kept in
//! the one-line deck above it.

use egui::{CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::shell;
use crate::theme::{self, Icon, Mood, Tier};
use crate::{session, table, App, Cmd, Connection, LibraryView, RichText};

/// The tabs of an HX pedal's page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PedalTab {
    #[default]
    Backups,
    Irs,
    Favourites,
    Eq,
    Settings,
    Activity,
}

impl PedalTab {
    const ALL: [PedalTab; 6] = [
        PedalTab::Backups,
        PedalTab::Irs,
        PedalTab::Favourites,
        PedalTab::Eq,
        PedalTab::Settings,
        PedalTab::Activity,
    ];

    fn label(self) -> &'static str {
        match self {
            PedalTab::Backups => "Backups",
            PedalTab::Irs => "Impulse responses",
            PedalTab::Favourites => "Favorite blocks",
            PedalTab::Eq => "Global EQ",
            PedalTab::Settings => "Settings",
            PedalTab::Activity => "Activity",
        }
    }
}

/// A small menu button: the choice, and a chevron.
fn choice_button(ui: &mut Ui, label: &str) -> egui::Response {
    theme::Button::new(label)
        .small()
        .trailing(Icon::ChevronDown)
        .show(ui)
}

/// A row of a device list: 34 points, with a hairline under it.
fn list_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::line_soft()),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        add,
    );
}

/// A section's caption with something quieter beside it, and room at the
/// right for its actions (laid out right to left).
fn section_head(ui: &mut Ui, caption: &str, aside: &str, actions: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    let used = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                actions(ui);
            },
        )
        .response
        .rect;
    let caption = ui.painter().layout_job(theme::paint::spaced(
        &caption.to_uppercase(),
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let width = caption.size().x;
    shell::paint_line(ui, caption, rect.left(), rect.center().y);
    if !aside.is_empty() {
        let room = used.left().min(rect.right()) - (rect.left() + width + 10.0) - 8.0;
        let aside = shell::elided(ui, aside, theme::regular(12.0), theme::muted(), room);
        shell::paint_line(ui, aside, rect.left() + width + 10.0, rect.center().y);
    }
    ui.add_space(4.0);
}

impl App {
    // -----------------------------------------------------------------------
    // Library

    /// The Library page: its head, then tones, setlists or Cloud.
    pub(crate) fn library_page(&mut self, root: &mut Ui, tier: Tier) {
        egui::Panel::top("library-head")
            .exact_size(56.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| self.library_head(ui, tier));
        self.library_body(root);
    }

    /// "Library", which of its three views, the search, which pedals it is
    /// scoped to and the account it publishes as.
    fn library_head(&mut self, ui: &mut Ui, tier: Tier) {
        let full = ui.max_rect();
        let inner = Rect::from_min_max(
            Pos2::new(full.left() + 20.0, full.top() + 2.0),
            Pos2::new(full.right() - 16.0, full.bottom()),
        );
        let showing = self.lib_showing;
        let right = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .id_salt("library-head-right")
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    self.account_button(ui);
                    if showing == LibraryView::Cloud {
                        if theme::IconButton::new(Icon::RefreshCw)
                            .show(ui)
                            .on_hover_text("Fetch this search and order from TonePush again")
                            .clicked()
                        {
                            self.refresh_cloud();
                        }
                        if self.auditioning.is_some()
                            && theme::Button::new("Done auditioning")
                                .small()
                                .show(ui)
                                .clicked()
                        {
                            self.end_audition();
                        }
                        if self.cloud_searching.is_some() || self.cloud_download.is_some() {
                            let (spot, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            shell::spin(ui, spot.center(), 5.5);
                        }
                    }
                    self.scope_button(ui);
                    let width = tier.pick(170.0, 200.0, 240.0);
                    let tones = self.scoped_tone_count();
                    let (query, hint) = match showing {
                        LibraryView::Tones => {
                            (&mut self.tone_search, format!("Search {tones} tones"))
                        }
                        LibraryView::Setlists => {
                            (&mut self.setlist_search, "Search setlists".to_owned())
                        }
                        LibraryView::Cloud => {
                            (&mut self.library_search, "Search TonePush".to_owned())
                        }
                    };
                    let search = theme::search_field(ui, "library-search", query, &hint, width);
                    if search.changed() && showing == LibraryView::Cloud {
                        self.cloud_search_due =
                            Some(std::time::Instant::now() + std::time::Duration::from_millis(350));
                    }
                    if showing == LibraryView::Cloud
                        && search.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        self.cloud_search_due = Some(std::time::Instant::now());
                    }
                },
            )
            .response
            .rect;
        let left = Rect::from_min_max(inner.min, Pos2::new(right.left() - 12.0, inner.bottom()));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(left)
                .id_salt("library-head-left")
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                let title =
                    shell::title_galley(ui, "Library", theme::PAGE_TITLE, theme::text(), 200.0);
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(title.size().x, 28.0), Sense::hover());
                shell::paint_line(ui, title, rect.left(), rect.center().y);
                let views = [
                    LibraryView::Tones,
                    LibraryView::Setlists,
                    LibraryView::Cloud,
                ];
                let cloud_count = self.cloud_total.unwrap_or(self.cloud_entries.len());
                let segments = [
                    theme::Segment::new("Tones").suffix(self.scoped_tone_count().to_string()),
                    theme::Segment::new("Setlists").suffix(self.scoped_setlist_count().to_string()),
                    if cloud_count > 0 {
                        theme::Segment::new("Cloud")
                            .icon(Icon::Cloud)
                            .suffix(cloud_count.to_string())
                    } else {
                        theme::Segment::new("Cloud").icon(Icon::Cloud)
                    },
                ];
                let chosen = views.iter().position(|view| *view == self.lib_showing);
                if let Some(index) =
                    theme::segmented(ui, "library-views", &segments, chosen, false).clicked
                {
                    self.show_library_view(views[index]);
                }
            },
        );
    }

    /// Which pedals' tones the library shows: all of them, or the one
    /// connected.
    fn scope_button(&mut self, ui: &mut Ui) {
        let selected = self
            .library_device_filter
            .clone()
            .unwrap_or_else(|| "All pedals".to_owned());
        let connected = self.library_connected_device.clone();
        let button = choice_button(ui, &selected)
            .on_hover_text("Show tones for every pedal, or only for the one connected");
        let mut changed = false;
        egui::Popup::menu(&button)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .show(|ui| {
                theme::menu_width(ui, 200.0);
                let all = self.library_device_filter.is_none();
                if theme::menu_item(ui, all.then_some(Icon::Check), "All pedals", None).clicked() {
                    self.library_device_filter = None;
                    changed = true;
                }
                if !connected.is_empty() {
                    let on = self.library_device_filter.as_deref() == Some(connected.as_str());
                    if theme::menu_item(
                        ui,
                        on.then_some(Icon::Check),
                        &connected,
                        Some("connected"),
                    )
                    .clicked()
                    {
                        self.library_device_filter = Some(connected.clone());
                        changed = true;
                    }
                }
            });
        if changed {
            self.lib_selected = None;
            self.lib_setlist = None;
            self.cloud_loaded_device = None;
            self.refresh_cloud();
        }
    }

    /// Signing in to publish, and the account once signed in. Called inside
    /// a right-to-left layout.
    fn account_button(&mut self, ui: &mut Ui) {
        if let Some(signing) = &self.signing_in {
            let code = signing.code.clone();
            let url = signing.url.clone();
            if theme::Button::new("Cancel")
                .ghost()
                .small()
                .show(ui)
                .clicked()
            {
                self.signing_in = None;
            }
            let (spot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            shell::spin(ui, spot.center(), 5.5);
            theme::Chip::new(&format!("Code {code}"))
                .mood(Mood::Accent)
                .show(ui)
                .on_hover_text(format!("Approve it at {url}, then this signs itself in"));
            return;
        }
        match self.config.account.clone() {
            Some(account) => {
                let button = theme::Button::new(&account)
                    .ghost()
                    .small()
                    .trailing(Icon::ChevronDown)
                    .show(ui);
                let mut sign_out = false;
                egui::Popup::menu(&button)
                    .align(egui::RectAlign::BOTTOM_END)
                    .gap(4.0)
                    .show(|ui| {
                        theme::menu_width(ui, 200.0);
                        theme::menu_header(ui, &account, Some("TonePush"));
                        if theme::menu_item(ui, Some(Icon::ExternalLink), "Open TonePush", None)
                            .clicked()
                        {
                            ui.ctx()
                                .open_url(egui::OpenUrl::new_tab(crate::cloud::site()));
                        }
                        theme::menu_separator(ui);
                        sign_out =
                            theme::menu_item(ui, Some(Icon::PowerOff), "Sign out", None).clicked();
                    });
                if sign_out {
                    self.config.sign_out();
                }
            }
            None => {
                if theme::Button::new("Sign in")
                    .ghost()
                    .small()
                    .show(ui)
                    .on_hover_text("Sign in to TonePush to publish Songs and Tones")
                    .clicked()
                {
                    self.start_signing_in(ui.ctx());
                }
            }
        }
    }

    /// Tones and Cloud are a browse rail, rows and an inspector; setlists are
    /// a rail of setlists and the slots of the one chosen.
    fn library_body(&mut self, root: &mut Ui) {
        match self.lib_showing {
            // Local and public Tones are the same screen shape. Keep that
            // geometry in one place; only the contents know who owns the
            // data.
            view @ (LibraryView::Tones | LibraryView::Cloud) => {
                let cloud = view == LibraryView::Cloud;
                egui::Panel::left("tone-tags")
                    .resizable(false)
                    .default_size(150.0)
                    .show(root, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .id_salt("tone-tags-scroll")
                            .show(ui, |ui| {
                                if cloud {
                                    self.cloud_tags_rail(ui);
                                } else {
                                    self.library_tags_rail(ui);
                                }
                            });
                    });
                egui::Panel::right("tone-inspector")
                    .resizable(true)
                    .default_size(320.0)
                    .show(root, |ui| {
                        // Scrolled, not grown: the inspector's fields must not
                        // decide the page's height.
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .id_salt("tone-inspector-scroll")
                            .show(ui, |ui| {
                                if cloud {
                                    self.cloud_inspector(ui);
                                } else {
                                    self.library_inspector(ui);
                                }
                            });
                    });
                egui::CentralPanel::default().show(root, |ui| {
                    if cloud {
                        self.cloud_table(ui);
                    } else {
                        self.library_table(ui);
                    }
                });
            }
            // Setlists on the left, what is in the chosen one on the right,
            // drawn by the same table the tones use.
            LibraryView::Setlists => {
                egui::Panel::left("lib-setlists")
                    .resizable(true)
                    // A floor taken from the table it holds, not guessed.
                    .min_size(table::width_wanted(&crate::setlist_rail_columns()))
                    .max_size(560.0)
                    .default_size(340.0)
                    // Not wrapped in a scroll area: the table does its own
                    // scrolling.
                    .show(root, |ui| self.setlist_rail(ui));
                egui::CentralPanel::default().show(root, |ui| self.setlist_slots(ui));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Pedal

    /// The HX's page: its name, firmware and libraries in the head, and a tab
    /// each for backups, impulse responses, favourite blocks, the global EQ,
    /// settings and the activity log.
    pub(crate) fn pedal_page(&mut self, root: &mut Ui) {
        let online = matches!(self.connection, Connection::Online);
        egui::Panel::top("pedal-head")
            .exact_size(if online {
                shell::PAGE_HEAD_WITH_TABS
            } else {
                shell::PAGE_HEAD
            })
            .resizable(false)
            .show_separator_line(!online)
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                if !online {
                    shell::page_head(ui, Some(Icon::Pedal), "No pedal", "", |_| {});
                    return;
                }
                let subtitle = self.pedal_facts();
                let mut let_go = false;
                shell::page_head(
                    ui,
                    Some(Icon::Pedal),
                    &self.device.clone(),
                    &subtitle,
                    |ui| {
                        let_go = theme::Button::new("Let the pedal go")
                            .ghost()
                            .small()
                            .icon(Icon::Power)
                            .show(ui)
                            .on_hover_text("Disconnect, so another editor can use the pedal")
                            .clicked();
                    },
                );
                if let_go {
                    self.let_go();
                }
                let tabs: Vec<(&str, Option<String>)> = PedalTab::ALL
                    .iter()
                    .map(|tab| {
                        let count = match tab {
                            PedalTab::Irs => Some(self.irs.len().to_string()),
                            PedalTab::Favourites => Some(self.favourites.len().to_string()),
                            _ => None,
                        };
                        (tab.label(), count)
                    })
                    .collect();
                let selected = PedalTab::ALL
                    .iter()
                    .position(|tab| *tab == self.pedal_tab)
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    if let Some(index) = theme::tabs(ui, &tabs, selected) {
                        self.pedal_tab = PedalTab::ALL[index];
                        self.read_pedal_page();
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::bg())
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 16,
                        top: 16,
                        bottom: 12,
                    }),
            )
            .show(root, |ui| {
                if !online {
                    self.no_pedal_here(ui);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt(("pedal-tab", self.pedal_tab as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(980.0));
                        match self.pedal_tab {
                            PedalTab::Backups => self.pedal_backups(ui),
                            PedalTab::Irs => self.pedal_irs(ui),
                            PedalTab::Favourites => self.pedal_favourites(ui),
                            PedalTab::Eq => self.pedal_eq(ui),
                            PedalTab::Settings => {
                                self.settings_list(ui, |group| group != "Global EQ")
                            }
                            PedalTab::Activity => self.pedal_activity(ui),
                        }
                    });
            });
    }

    /// The line under the pedal's name: firmware, presets and IR slots.
    fn pedal_facts(&self) -> String {
        let mut facts = Vec::new();
        if !self.firmware.is_empty() {
            facts.push(format!("Firmware {}", self.firmware));
        }
        let total = self.hx_total();
        if total > 0 {
            facts.push(match self.setlists.len() {
                0 | 1 => format!("{total} presets in one setlist"),
                count => format!("{total} presets in each of {count} setlists"),
            });
        }
        facts.push(format!(
            "{} of {} IR slots used",
            self.irs.len(),
            session::IR_SLOTS
        ));
        facts.join(" · ")
    }

    /// The Pedal page with nothing plugged in.
    fn no_pedal_here(&mut self, ui: &mut Ui) {
        ui.set_max_width(640.0);
        let mut look = false;
        let looking = self.connection == Connection::Connecting;
        theme::banner(
            ui,
            Mood::Info,
            Icon::Usb,
            if looking {
                "Looking for a pedal on USB"
            } else {
                "No pedal connected"
            },
            "Its backups, impulse responses, favourite blocks, global EQ and settings appear \
             here once it connects. Quit any other pedal editor first: only one can use a pedal \
             at a time.",
            |ui| {
                look = theme::Button::new("Look for a pedal")
                    .small()
                    .icon(Icon::Usb)
                    .enabled(!looking)
                    .show(ui)
                    .clicked();
            },
        );
        if look {
            self.look_for_pedal();
        }
    }

    /// Backups: how the pedal is protected, and a whole-pedal backup or
    /// restore through a file.
    ///
    /// These act on the whole pedal, settings and impulse responses included,
    /// which is why they live on its page rather than on a preset.
    fn pedal_backups(&mut self, ui: &mut Ui) {
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
                let time =
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(manifest.captured);
                (
                    Mood::Ok,
                    Icon::ShieldCheck,
                    format!("Everything on this {device} is backed up"),
                    format!(
                        "TonePush reads the whole pedal when it connects and keeps that copy \
                         current after every save: {presets}, {} impulse responses and {} \
                         settings. {}.",
                        manifest.irs.len(),
                        manifest.globals,
                        shell::when_words("Read", time)
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
                    Ok(manifest) => {
                        let kept = manifest.presets.iter().filter(|n| !n.is_empty()).count();
                        self.note(format!("restoring {kept} presets from {}", dir.display()));
                        self.send(Cmd::RestoreAll(dir));
                    }
                    Err(e) => self.problem(format!("That is not a backup: {e}")),
                }
            }
        }
    }

    /// The pedal's impulse responses, renamed in place, saved out as WAVs or
    /// cleared, and new ones imported into the first free slot.
    fn pedal_irs(&mut self, ui: &mut Ui) {
        let free = session::free_ir_slot(&self.irs);
        let mut import = false;
        let used = format!("{} of {} slots used", self.irs.len(), session::IR_SLOTS);
        section_head(ui, "Impulse responses", &used, |ui| {
            import = theme::Button::new("Import IR…")
                .small()
                .icon(Icon::Upload)
                .enabled(free.is_some())
                .show(ui)
                .on_hover_text("Send a WAV to the first free slot")
                .on_disabled_hover_text("Every IR slot is in use")
                .clicked();
        });
        if import {
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("WAV", &["wav"])
                .pick_file()
            {
                self.load_ir(file);
            }
        }
        if self.irs.is_empty() {
            theme::label(
                ui,
                "No impulse responses on the pedal yet. Import a WAV, or drop one on the window.",
                theme::regular(12.5),
                theme::muted(),
            );
            return;
        }
        let irs = self.irs.clone();
        let mut save = None;
        let mut clear = None;
        let mut rename_start = None;
        let mut rename: Option<Option<(i64, String)>> = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (slot, name) in &irs {
                list_row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let number = shell::galley(
                        ui,
                        format!("{}", slot + 1),
                        theme::medium(theme::LABEL),
                        theme::muted(),
                    );
                    let (place, _) = ui.allocate_exact_size(Vec2::new(28.0, 20.0), Sense::hover());
                    let width = number.size().x;
                    shell::paint_line(ui, number, place.right() - width, place.center().y);
                    match &mut self.renaming_ir {
                        Some((editing, draft)) if editing == slot => {
                            let field =
                                ui.add(egui::TextEdit::singleline(draft).desired_width(220.0));
                            field.request_focus();
                            if field.lost_focus() {
                                let done = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                rename = Some(done.then(|| (*slot, draft.clone())));
                            }
                        }
                        _ => {
                            if ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(name)
                                            .font(theme::regular(theme::BODY))
                                            .color(theme::text()),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_text("Click to rename")
                                .clicked()
                            {
                                rename_start = Some((*slot, name.clone()));
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if theme::Button::new("Clear")
                            .ghost()
                            .small()
                            .show(ui)
                            .on_hover_text("Empty this slot on the pedal")
                            .clicked()
                        {
                            clear = Some(*slot);
                        }
                        if theme::Button::new("Save…")
                            .ghost()
                            .small()
                            .show(ui)
                            .on_hover_text("Write this IR out as a WAV")
                            .clicked()
                        {
                            save = Some((*slot, name.clone()));
                        }
                    });
                });
            }
        });
        if let Some(slot) = clear {
            self.send(Cmd::ClearIr(slot));
        }
        if let Some((slot, name)) = save {
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(format!("{}.wav", crate::sanitise(&name)))
                .add_filter("WAV", &["wav"])
                .save_file()
            {
                self.send(Cmd::SaveIr { slot, file: path });
            }
        }
        if let Some(started) = rename_start {
            self.renaming_ir = Some(started);
        }
        if let Some(result) = rename {
            self.renaming_ir = None;
            if let Some((slot, name)) = result {
                self.send(Cmd::RenameIr { slot, name });
            }
        }
    }

    /// The pedal's favourite blocks: the selected block added, others
    /// removed.
    fn pedal_favourites(&mut self, ui: &mut Ui) {
        let favourites = self.favourites.clone();
        let selected = self.selected_block();
        let free = (0..16).find(|i| !favourites.iter().any(|(n, _)| n == i));
        let add_label = selected.as_ref().map_or_else(
            || "Add the selected block".to_owned(),
            |(_, name)| format!("Add {name}"),
        );
        let mut add = false;
        let used = format!("{} of 16", favourites.len());
        section_head(ui, "Favorite blocks", &used, |ui| {
            add = theme::Button::new(&add_label)
                .small()
                .icon(Icon::Star)
                .enabled(selected.is_some() && free.is_some())
                .show(ui)
                .on_hover_text(
                    "Keep the block selected on the Edit page among the pedal's favourites",
                )
                .on_disabled_hover_text(if selected.is_none() {
                    "Select an effect block on the Edit page first"
                } else {
                    "Every favourite slot is in use"
                })
                .clicked();
        });
        if add {
            if let (Some((block, name)), Some(index)) = (selected, free) {
                self.send(Cmd::SaveFavourite { block, index, name });
            }
        }
        if favourites.is_empty() {
            theme::label(
                ui,
                "No favourite blocks yet.",
                theme::regular(12.5),
                theme::muted(),
            );
            return;
        }
        let mut forget = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (index, name) in &favourites {
                list_row(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let number = shell::galley(
                        ui,
                        format!("{}", index + 1),
                        theme::medium(theme::LABEL),
                        theme::muted(),
                    );
                    let (place, _) = ui.allocate_exact_size(Vec2::new(28.0, 20.0), Sense::hover());
                    let width = number.size().x;
                    shell::paint_line(ui, number, place.right() - width, place.center().y);
                    theme::label(ui, name, theme::regular(theme::BODY), theme::text());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::Button::new("Remove")
                            .ghost()
                            .small()
                            .show(ui)
                            .clicked()
                        {
                            forget = Some(*index);
                        }
                    });
                });
            }
        });
        if let Some(index) = forget {
            self.send(Cmd::ClearFavourite(index));
        }
    }

    /// The global EQ, drawn as what it does rather than as eleven numbers.
    ///
    /// Two cuts and three peaking bands, over a log frequency axis. The handles
    /// are the controls: drag a band to move and lift it, scroll on one to
    /// narrow it, drag a cut along the floor. The numbers underneath say
    /// exactly where everything landed, because a curve is for aiming and a
    /// number is for repeating.
    fn pedal_eq(&mut self, ui: &mut Ui) {
        // Not "some settings have arrived" but "the EQ's own have":
        // `eq_curve_now` substitutes sensible numbers for ids it has not
        // seen, and a panel that can be dragged while it shows substitutes
        // would write them over the pedal's real ones.
        if !self.eq_settings_known() {
            ui.horizontal(|ui| {
                let (spot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                shell::spin(ui, spot.center(), 5.5);
                theme::label(
                    ui,
                    "Reading the pedal's EQ",
                    theme::regular(12.5),
                    theme::muted(),
                );
            });
            return;
        }
        // The bypass leads, because a curve you cannot hear is the first
        // thing to check and the last thing anyone remembers.
        let mut on = self
            .settings
            .get(&crate::id::EQ_ON)
            .is_some_and(|v| *v >= 0.5);
        let mut flatten = false;
        section_head(ui, "Global EQ", if on { "On" } else { "Off" }, |ui| {
            flatten = theme::Button::new("Flatten")
                .small()
                .show(ui)
                .on_hover_text("Every band back to no gain, both cuts off")
                .clicked();
            if theme::toggle(ui, &mut on, theme::accent())
                .on_hover_text("Turn the global EQ on or off")
                .changed()
            {
                self.settings.insert(crate::id::EQ_ON, on as u8 as f32);
                self.send(Cmd::WriteSetting {
                    id: crate::id::EQ_ON,
                    value: on as u8 as f32,
                });
            }
        });
        if flatten {
            self.flatten_eq();
        }
        theme::card()
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.eq_curve(ui, on);
                ui.add_space(10.0);
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0, theme::line()),
                );
                ui.add_space(10.0);
                self.eq_controls(ui);
            });
    }

    /// What the worker has been doing, newest last: a diagnostic, kept out
    /// of the way until asked for.
    fn pedal_activity(&mut self, ui: &mut Ui) {
        section_head(
            ui,
            "Activity",
            "What TonePush asked of the pedal, newest last",
            |_| {},
        );
        egui::Frame::new()
            .fill(if theme::is_dark() {
                theme::bg_deep()
            } else {
                theme::panel()
            })
            .stroke(Stroke::new(1.0, theme::line()))
            .corner_radius(CornerRadius::same(theme::RADIUS_CARD))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.log.is_empty() {
                    theme::label(ui, "Nothing yet.", theme::regular(12.0), theme::muted());
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("activity-log")
                    .max_height(ui.available_height().max(240.0))
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        for line in &self.log {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(line)
                                        .font(egui::FontId::monospace(12.0))
                                        .color(theme::text_soft()),
                                )
                                .wrap(),
                            );
                        }
                    });
            });
    }
}
