//! The window with no pedal (docs/design/redesign-2026-10-01, screen 18):
//! one calm page instead of an empty chain and an empty pane. It says to plug
//! the pedal in, shows both families TonePush looks for and whether it is
//! still looking, what is already fine on this computer, what to do first,
//! and Line 6's model data as a step to take rather than a window in the way.
//! The library stays one click away.

use std::collections::BTreeSet;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::shell;
use crate::theme::{self, Icon, Tier};
use crate::{App, Connection};

/// Where HX Edit is downloaded from.
const HX_EDIT: &str = "https://line6.com/software/";
/// TonePush's guide.
const GUIDE: &str = "https://docs.tonepush.rocks";

/// Where the access rule for USB pedals is installed: by `install.sh`, and by
/// the packages.
#[cfg(target_os = "linux")]
const UDEV_RULES: [&str; 3] = [
    "/etc/udev/rules.d/70-line6-hx.rules",
    "/usr/lib/udev/rules.d/70-line6-hx.rules",
    "/lib/udev/rules.d/70-line6-hx.rules",
];

/// Whether this computer lets TonePush open a USB pedal without asking:
/// `None` where nothing needs setting up.
fn usb_access() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        Some(
            UDEV_RULES
                .iter()
                .any(|path| std::path::Path::new(path).is_file()),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// How often the connect page looks on USB for a pedal.
pub(crate) const WATCH_EVERY: Duration = Duration::from_secs(2);

/// How long a pedal that is listed but did not connect waits before it is
/// tried again: another editor may hold it, and a try every two seconds
/// would fill the activity log while it does.
pub(crate) const RETRY_AFTER: Duration = Duration::from_secs(10);

/// What one look at USB listed, without opening anything: HX pedals by
/// serial (or model), and serial ports that are a StompStation PRO, or may
/// be one in Update Mode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) hx: BTreeSet<String>,
    pub(crate) pro: BTreeSet<String>,
    pub(crate) update: BTreeSet<String>,
}

impl Listed {
    /// Enumerate only: the USB device list and the serial ports' names and
    /// IDs. No device is opened or claimed, so a pedal another program holds
    /// is not disturbed.
    fn scan() -> Self {
        Self {
            hx: hx_usb::list()
                .map(|found| {
                    found
                        .into_iter()
                        .map(|found| {
                            found
                                .serial
                                .unwrap_or_else(|| found.profile.name.to_owned())
                        })
                        .collect()
                })
                .unwrap_or_default(),
            pro: ports(voidx_client::list()),
            update: ports(voidx_client::list_update_mode()),
        }
    }
}

fn ports(found: voidx_client::Result<Vec<voidx_client::Found>>) -> BTreeSet<String> {
    found
        .map(|found| found.into_iter().map(|found| found.port_name).collect())
        .unwrap_or_default()
}

/// One family's side of the watch.
#[derive(Debug, Default)]
struct Watched {
    /// What was last tried, and when.
    tried: Option<(BTreeSet<String>, Instant)>,
    /// Pedals let go of on purpose: left alone until they leave the list
    /// (unplugged) or somebody asks to look again.
    released: BTreeSet<String>,
    /// Set by a let go: the next look's pedals are the ones let go of.
    release_next: bool,
}

impl Watched {
    /// Whether to connect, given what is listed now.
    fn decide(&mut self, listed: &BTreeSet<String>, free: bool, now: Instant) -> bool {
        if std::mem::take(&mut self.release_next) {
            self.released = listed.clone();
        }
        self.released.retain(|key| listed.contains(key));
        let wanted: BTreeSet<String> = listed.difference(&self.released).cloned().collect();
        if wanted.is_empty() {
            self.tried = None;
            return false;
        }
        if !free {
            return false;
        }
        if let Some((tried, at)) = &self.tried {
            if *tried == wanted && now.duration_since(*at) < RETRY_AFTER {
                return false;
            }
        }
        self.tried = Some((wanted, now));
        true
    }
}

/// The connect page's watch on USB. While no pedal is connected and no
/// connect is in flight, it lists what is plugged in every two seconds, on a
/// thread of its own, and asks a family to connect only once a pedal of
/// that family is listed.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    due: Option<Instant>,
    scanning: Option<Receiver<Listed>>,
    hx: Watched,
    pro: Watched,
    /// Update Mode ports already asked who they are. Any Raspberry Pi gadget
    /// can look like one, so each is asked once, not every two seconds.
    probed: BTreeSet<String>,
}

/// Which families to connect.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub(crate) hx: bool,
    pub(crate) pro: bool,
}

impl Watch {
    /// What to connect, given what a look listed and which families are
    /// free to try.
    pub(crate) fn decide(
        &mut self,
        listed: &Listed,
        hx_free: bool,
        pro_free: bool,
        now: Instant,
    ) -> Wanted {
        let hx = self.hx.decide(&listed.hx, hx_free, now);
        let mut pro = self.pro.decide(&listed.pro, pro_free, now);
        self.probed.retain(|port| listed.update.contains(port));
        // The worker takes an Update Mode port only when it is the only one.
        if pro_free && !pro && listed.pro.is_empty() && listed.update.len() == 1 {
            let port = listed.update.iter().next().expect("one port");
            if self.probed.insert(port.clone()) {
                pro = true;
            }
        }
        Wanted { hx, pro }
    }

    /// The HX pedal was let go of: do not take it back by itself.
    pub(crate) fn release_hx(&mut self) {
        self.hx.release_next = true;
    }

    /// The StompStation PRO was let go of: do not take it back by itself.
    pub(crate) fn release_pro(&mut self) {
        self.pro.release_next = true;
    }

    /// Somebody asked to look: forget what was let go of and tried.
    pub(crate) fn forget(&mut self) {
        *self = Self {
            due: self.due,
            scanning: self.scanning.take(),
            ..Self::default()
        };
    }
}

/// How a family's card says TonePush is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Search {
    Looking,
    NotFound,
}

impl App {
    /// Whether the Edit page is the connect page: no pedal of either family.
    pub(crate) fn shows_connect(&self) -> bool {
        !matches!(self.connection, Connection::Online) && !self.pro_active()
    }

    /// Whether the connect page may look on USB now: it shows, and neither
    /// family is connected or has a connect in flight.
    fn may_watch(&self) -> bool {
        self.page == shell::Page::Edit
            && self.shows_connect()
            && self.connection == Connection::Offline
            && self.pro.free_to_look()
    }

    /// Look on USB every two seconds while the connect page shows, so a
    /// pedal plugged in later, or freed by another program, is found
    /// without asking. Called every frame, painted or not.
    pub(crate) fn watch_usb(&mut self, ctx: &egui::Context) {
        if let Some(scanning) = &self.watch.scanning {
            match scanning.try_recv() {
                Ok(listed) => {
                    self.watch.scanning = None;
                    self.on_listed(&listed, Instant::now());
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.watch.scanning = None,
            }
        }
        // Tests, and the screenshots they draw, never look at real USB.
        if cfg!(test) || !self.may_watch() {
            self.watch.due = None;
            return;
        }
        let now = Instant::now();
        match self.watch.due {
            Some(due) if now >= due => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(Listed::scan());
                });
                self.watch.scanning = Some(rx);
                self.watch.due = Some(now + WATCH_EVERY);
            }
            Some(_) => {}
            None => self.watch.due = Some(now + WATCH_EVERY),
        }
        ctx.request_repaint_after(WATCH_EVERY);
    }

    /// Connect the families a look found a pedal of, if the page still
    /// waits for one.
    pub(crate) fn on_listed(&mut self, listed: &Listed, now: Instant) {
        let watching = self.may_watch();
        let wanted = self.watch.decide(listed, watching, watching, now);
        if wanted.hx {
            self.connection = Connection::Connecting;
            self.send(crate::Cmd::Connect);
        }
        if wanted.pro {
            self.pro.reconnect();
        }
    }

    /// The connect page, in place of the deck, the board and the pane.
    pub(crate) fn connect_page(&mut self, root: &mut Ui, tier: Tier) {
        let compact = tier == Tier::S;
        let hx = if self.connection == Connection::Connecting {
            Search::Looking
        } else {
            Search::NotFound
        };
        let pro = if self.pro.looking() {
            Search::Looking
        } else {
            Search::NotFound
        };
        let mut look = false;
        let mut open_library = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::bg()))
            .show(root, |ui| {
                let room = ui.max_rect();
                let width = (room.width() - 64.0).min(720.0);
                let left = room.left() + (room.width() - width) / 2.0;
                let column = Rect::from_min_max(
                    Pos2::new(left, room.top() + if compact { 26.0 } else { 44.0 }),
                    Pos2::new(left + width, room.bottom() - 20.0),
                );
                let mut ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(column)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                let ui = &mut ui;
                ui.spacing_mut().item_spacing.y = 0.0;

                // The mark and the name.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let (mark, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                    theme::paint_mark(ui, mark);
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        "Tone",
                        0.0,
                        egui::TextFormat::simple(theme::bold(17.0), theme::text()),
                    );
                    job.append(
                        "Push",
                        0.0,
                        egui::TextFormat::simple(theme::bold(17.0), theme::accent()),
                    );
                    ui.label(job);
                });
                ui.add_space(if compact { 14.0 } else { 22.0 });
                let title = shell::title_galley(
                    ui,
                    "Plug in your pedal",
                    if compact { 26.0 } else { 30.0 },
                    theme::text(),
                    width,
                );
                let (rect, _) = ui.allocate_exact_size(title.size(), Sense::hover());
                ui.painter().galley(rect.min, title, Color32::PLACEHOLDER);
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.set_max_width(600.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "TonePush finds it on USB by itself. Edits play on the pedal as \
                                 you make them, and nothing is written to its memory until you \
                                 save.",
                            )
                            .font(theme::regular(14.0))
                            .color(theme::text_soft()),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(if compact { 16.0 } else { 22.0 });

                // The two families.
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let card = (width - 14.0) / 2.0;
                    family(
                        ui,
                        card,
                        if compact { 150.0 } else { 180.0 },
                        Family::Hx,
                        hx,
                    );
                    family(
                        ui,
                        card,
                        if compact { 150.0 } else { 180.0 },
                        Family::Pro,
                        pro,
                    );
                });
                if hx == Search::NotFound && pro == Search::NotFound {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        look = theme::Button::new("Look again")
                            .small()
                            .icon(Icon::Usb)
                            .show(ui)
                            .on_hover_text("Look on USB for an HX pedal and a StompStation PRO")
                            .clicked();
                        ui.add_space(10.0);
                        let line = shell::galley(
                            ui,
                            "TonePush keeps checking USB while this page is open.",
                            theme::regular(12.5),
                            theme::muted(),
                        );
                        let (spot, _) = ui.allocate_exact_size(line.size(), Sense::hover());
                        ui.painter().galley(
                            Pos2::new(spot.left(), spot.center().y - line.size().y / 2.0),
                            line,
                            Color32::PLACEHOLDER,
                        );
                    });
                }
                ui.add_space(if compact { 12.0 } else { 18.0 });

                // What is fine, what to do, and the step for HX pedals.
                if let Some(access) = usb_access() {
                    if access {
                        check(
                            ui,
                            Icon::CircleCheck,
                            theme::ok(),
                            "This computer can talk to USB pedals",
                            "The access rule for Line 6 and Sonulab pedals is installed.",
                        );
                    } else {
                        check(
                            ui,
                            Icon::CircleAlert,
                            theme::hot(),
                            "USB pedals need an access rule on this computer",
                            "install.sh installs it, and the guide shows how by hand. Replug the \
                             pedal afterwards.",
                        );
                    }
                }
                check(
                    ui,
                    Icon::Info,
                    theme::info(),
                    "Quit HX Edit and VoidX Control first",
                    "Only one editor can use a pedal at a time.",
                );
                self.model_data_step(ui);

                // The library, and where to read more, at the foot.
                let bottom = column.bottom() - 22.0;
                let cursor = ui.cursor().top();
                if bottom > cursor + 12.0 {
                    ui.add_space(bottom - cursor - 12.0);
                } else {
                    ui.add_space(14.0);
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (spot, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    theme::paint_icon(ui, Icon::Library, spot.center(), 14.0, theme::text_soft());
                    let tones = self.lib_entries.len();
                    theme::label(
                        ui,
                        &match tones {
                            0 => "Your library is empty".to_owned(),
                            1 => "Your library has 1 tone".to_owned(),
                            n => format!("Your library has {n} tones"),
                        },
                        theme::regular(12.5),
                        theme::text_soft(),
                    );
                    ui.add_space(4.0);
                    open_library = link(ui, "Open it", theme::accent());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 18.0;
                        if link(ui, "What's new", theme::muted()) {
                            ui.ctx()
                                .open_url(egui::OpenUrl::new_tab(crate::update::RELEASES));
                        }
                        if link(ui, "Guide", theme::muted()) {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(GUIDE));
                        }
                    });
                });
                ui.add_space(6.0);
                theme::label(
                    ui,
                    "Free and open source, MIT licensed. Not affiliated with Yamaha Guitar Group.",
                    theme::regular(11.0),
                    theme::faint(),
                );
            });
        if look {
            self.look_for_pedal();
        }
        if open_library {
            self.go_to(shell::Page::Library);
        }
    }

    /// Line 6's model data, as a step: done, being copied, or the two ways
    /// to get it.
    pub(crate) fn model_data_step(&mut self, ui: &mut Ui) {
        if self.catalog.is_some() {
            check(
                ui,
                Icon::CircleCheck,
                theme::ok(),
                "Line 6 model names and pictures",
                "HX Edit's data is in place.",
            );
            return;
        }
        let mut find = false;
        let mut download = false;
        let extracting = self.extracting.is_some();
        let status = self.onboarding_status.clone();
        row(ui, |ui| {
            let (spot, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            if extracting {
                shell::spin(ui, spot.center(), 6.0);
            } else {
                theme::paint_icon(ui, Icon::CircleDashed, spot.center(), 16.0, theme::muted());
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                theme::label(
                    ui,
                    "Line 6 model names and pictures",
                    theme::medium(13.0),
                    theme::text(),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "HX pedals need HX Edit's data once; TonePush copies it from HX \
                             Edit's installer, and it never leaves this computer. The \
                             StompStation PRO needs nothing.",
                        )
                        .font(theme::regular(12.0))
                        .color(theme::muted()),
                    )
                    .wrap(),
                );
                if let Some(status) = &status {
                    theme::label(ui, status, theme::regular(12.0), theme::accent());
                }
                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    find = theme::Button::new("Find the installer")
                        .small()
                        .icon(Icon::Search)
                        .enabled(!extracting)
                        .show(ui)
                        .on_hover_text(
                            "Look in Downloads for HX Edit's installer (the Mac .dmg or the \
                             Windows .exe), or choose it",
                        )
                        .clicked();
                    download = theme::Button::new("Download HX Edit")
                        .small()
                        .ghost()
                        .icon(Icon::ExternalLink)
                        .show(ui)
                        .on_hover_text("Free from line6.com; a Line 6 account is required")
                        .clicked();
                });
            });
        });
        if download {
            ui.ctx().open_url(egui::OpenUrl::new_tab(HX_EDIT));
        }
        if find {
            match hx_catalog::extract::installer_in_downloads() {
                Some(installer) => self.extract_resources(installer),
                None => {
                    if let Some(installer) = rfd::FileDialog::new()
                        .set_title("Choose HX Edit's installer")
                        .add_filter("HX Edit installer", &["dmg", "exe"])
                        .pick_file()
                    {
                        self.extract_resources(installer);
                    }
                }
            }
        }
    }
}

/// The two families TonePush looks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Hx,
    Pro,
}

/// A family's card: its drawing, its name, what it covers, and whether
/// TonePush is looking for one.
fn family(ui: &mut Ui, width: f32, art: f32, which: Family, search: Search) {
    let (name, models) = match which {
        Family::Hx => ("Line 6 HX", "HX Stomp, HX Stomp XL, HX Effects, Helix"),
        Family::Pro => ("Sonulab StompStation PRO", "Firmware 1.5 and 2.x"),
    };
    theme::card()
        .inner_margin(egui::Margin {
            left: 18,
            right: 18,
            top: 18,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(width - 36.0);
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(art, art * 0.62), Sense::hover());
                match which {
                    Family::Hx => hx_drawing(ui, rect),
                    Family::Pro => pro_drawing(ui, rect),
                }
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    theme::label(ui, name, theme::semibold(14.0), theme::text());
                    theme::label(ui, models, theme::regular(12.0), theme::muted());
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let words = match search {
                        Search::Looking => "Looking on USB",
                        Search::NotFound => "Not found",
                    };
                    let galley = shell::galley(ui, words, theme::regular(12.0), theme::text_soft());
                    let total = 14.0 + 7.0 + galley.size().x;
                    ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                    let (spot, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    match search {
                        Search::Looking => shell::spin(ui, spot.center(), 5.0),
                        Search::NotFound => theme::paint_icon(
                            ui,
                            Icon::CircleDashed,
                            spot.center(),
                            13.0,
                            theme::muted(),
                        ),
                    }
                    ui.add_space(7.0);
                    let (text, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                    ui.painter().galley(text.min, galley, Color32::PLACEHOLDER);
                });
            });
        });
}

/// A row under a hairline.
fn row(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    let top = ui.cursor().top();
    let rect = ui.max_rect();
    ui.painter().hline(
        rect.x_range(),
        top + 0.5,
        Stroke::new(1.0, theme::line_soft()),
    );
    ui.add_space(9.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        contents(ui);
    });
    ui.add_space(9.0);
}

/// A check: its mark, what it is, and a quieter line.
fn check(ui: &mut Ui, icon: Icon, ink: Color32, title: &str, body: &str) {
    row(ui, |ui| {
        let (spot, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        theme::paint_icon(ui, icon, spot.center(), 16.0, ink);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            theme::label(ui, title, theme::medium(13.0), theme::text());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(body)
                        .font(theme::regular(12.0))
                        .color(theme::muted()),
                )
                .wrap(),
            );
        });
    });
}

/// Words that act as a link: says whether they were clicked.
fn link(ui: &mut Ui, text: &str, colour: Color32) -> bool {
    let galley = shell::galley(ui, text, theme::medium(12.5), colour);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    let hovered = response.hovered();
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    if hovered {
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0, theme::alpha(colour, 0.6)),
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// An HX pedal, drawn: a screen, six knobs and three footswitches.
fn hx_drawing(ui: &Ui, rect: Rect) {
    let scale = rect.width() / 210.0;
    let at = |x: f32, y: f32| Pos2::new(rect.left() + x * scale, rect.top() + y * scale);
    let painter = ui.painter();
    let line = Stroke::new(1.0, theme::line_strong());
    painter.rect(
        Rect::from_min_max(at(4.0, 4.0), at(206.0, 126.0)),
        CornerRadius::same((14.0 * scale) as u8),
        theme::panel(),
        line,
        egui::StrokeKind::Inside,
    );
    painter.rect(
        Rect::from_min_max(at(18.0, 16.0), at(80.0, 50.0)),
        CornerRadius::same((4.0 * scale) as u8),
        theme::bg_deep(),
        line,
        egui::StrokeKind::Inside,
    );
    for x in [96.0, 114.0, 132.0, 150.0, 168.0, 186.0] {
        painter.circle(at(x, 33.0), 6.5 * scale, theme::raised(), line);
    }
    for x in [42.0, 105.0, 168.0] {
        painter.circle_stroke(
            at(x, 92.0),
            17.0 * scale,
            Stroke::new(2.0, theme::line_strong()),
        );
        painter.circle(at(x, 92.0), 11.0 * scale, theme::raised(), line);
    }
}

/// A StompStation PRO, drawn: a screen, the encoder, four knobs and three
/// footswitches.
fn pro_drawing(ui: &Ui, rect: Rect) {
    let scale = rect.width() / 210.0;
    let at = |x: f32, y: f32| Pos2::new(rect.left() + x * scale, rect.top() + y * scale);
    let painter = ui.painter();
    let line = Stroke::new(1.0, theme::line_strong());
    painter.rect(
        Rect::from_min_max(at(4.0, 4.0), at(206.0, 126.0)),
        CornerRadius::same((14.0 * scale) as u8),
        theme::panel(),
        line,
        egui::StrokeKind::Inside,
    );
    painter.rect(
        Rect::from_min_max(at(18.0, 16.0), at(102.0, 60.0)),
        CornerRadius::same((4.0 * scale) as u8),
        theme::bg_deep(),
        line,
        egui::StrokeKind::Inside,
    );
    painter.circle(at(134.0, 38.0), 15.0 * scale, theme::raised(), line);
    painter.circle(at(134.0, 38.0), 9.0 * scale, theme::bg_deep(), line);
    for x in [168.0, 188.0] {
        for y in [26.0, 50.0] {
            painter.circle(at(x, y), 6.0 * scale, theme::raised(), line);
        }
    }
    for x in [42.0, 105.0, 168.0] {
        painter.circle_stroke(
            at(x, 98.0),
            15.0 * scale,
            Stroke::new(2.0, theme::line_strong()),
        );
        painter.circle(at(x, 98.0), 10.0 * scale, theme::raised(), line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(hx: &[&str], pro: &[&str], update: &[&str]) -> Listed {
        let set = |keys: &[&str]| keys.iter().map(|key| (*key).to_owned()).collect();
        Listed {
            hx: set(hx),
            pro: set(pro),
            update: set(update),
        }
    }

    const HX: Wanted = Wanted {
        hx: true,
        pro: false,
    };
    const PRO: Wanted = Wanted {
        hx: false,
        pro: true,
    };
    const NONE: Wanted = Wanted {
        hx: false,
        pro: false,
    };

    /// Nothing is connected until a pedal of the family is listed; then it
    /// is, once, and again only after a while if it is still there and still
    /// not connected (another editor had it).
    #[test]
    fn a_family_connects_once_its_pedal_is_listed() {
        let mut watch = Watch::default();
        let start = Instant::now();
        assert_eq!(watch.decide(&Listed::default(), true, true, start), NONE);

        let stomp = listed(&["HX Stomp"], &[], &[]);
        assert_eq!(watch.decide(&stomp, true, true, start), HX);
        let soon = start + WATCH_EVERY;
        assert_eq!(watch.decide(&stomp, true, true, soon), NONE);
        let later = start + RETRY_AFTER;
        assert_eq!(watch.decide(&stomp, true, true, later), HX, "freed since");

        let pro = listed(&[], &["/dev/ttyACM0"], &[]);
        assert_eq!(watch.decide(&pro, true, true, later), PRO);
    }

    /// While a family is connected or a connect is in flight nothing is
    /// asked, and the pedal is asked for as soon as it is free.
    #[test]
    fn nothing_is_asked_while_a_family_is_not_free() {
        let mut watch = Watch::default();
        let now = Instant::now();
        let both = listed(&["HX Stomp"], &["/dev/ttyACM0"], &[]);
        assert_eq!(watch.decide(&both, false, false, now), NONE);
        assert_eq!(
            watch.decide(&both, true, true, now),
            Wanted {
                hx: true,
                pro: true
            }
        );
    }

    /// A pedal let go of is not taken back by the watch: not until it is
    /// unplugged and plugged in again, or somebody looks again.
    #[test]
    fn a_pedal_let_go_of_stays_let_go() {
        let mut watch = Watch::default();
        let start = Instant::now();
        let stomp = listed(&["HX Stomp"], &[], &[]);
        watch.release_hx();
        assert_eq!(watch.decide(&stomp, true, true, start), NONE);
        let later = start + RETRY_AFTER * 3;
        assert_eq!(watch.decide(&stomp, true, true, later), NONE);

        assert_eq!(watch.decide(&Listed::default(), true, true, later), NONE);
        assert_eq!(
            watch.decide(&stomp, true, true, later),
            HX,
            "plugged in again"
        );

        watch.release_hx();
        assert_eq!(watch.decide(&stomp, true, true, later), NONE);
        watch.forget();
        assert_eq!(watch.decide(&stomp, true, true, later), HX, "asked to look");

        let pro = listed(&[], &["/dev/ttyACM0"], &[]);
        watch.release_pro();
        assert_eq!(watch.decide(&pro, true, true, later), NONE);
    }

    /// A port that may be a PRO in Update Mode is asked who it is once: any
    /// Raspberry Pi gadget looks the same until asked.
    #[test]
    fn an_update_mode_port_is_asked_once() {
        let mut watch = Watch::default();
        let start = Instant::now();
        let pi = listed(&[], &[], &["/dev/ttyACM1"]);
        assert_eq!(watch.decide(&pi, true, true, start), PRO);
        let later = start + RETRY_AFTER * 3;
        assert_eq!(watch.decide(&pi, true, true, later), NONE);
        assert_eq!(watch.decide(&Listed::default(), true, true, later), NONE);
        assert_eq!(
            watch.decide(&pi, true, true, later),
            PRO,
            "plugged in again"
        );

        let two = listed(&[], &[], &["/dev/ttyACM1", "/dev/ttyACM2"]);
        let mut watch = Watch::default();
        assert_eq!(watch.decide(&two, true, true, start), NONE, "which one?");
    }
}
