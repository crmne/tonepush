//! The window with no pedal (docs/design/redesign-2026-10-01, screen 18):
//! one calm page instead of an empty chain and an empty pane. It says to plug
//! the pedal in, shows both families TonePush looks for and whether it is
//! still looking, what is already fine on this computer, what to do first,
//! and Line 6's model data as a step to take rather than a window in the way.
//! The library stays one click away.

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
                            "Plug the pedal in, then look again.",
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
