//! Is there a newer TonePush than the one running, and can this copy become it?
//!
//! The check is a courtesy, not a feature: it asks GitHub once a day, off the
//! UI thread, and if the answer does not arrive - no network, rate limited,
//! GitHub down - nothing is said. An editor that nags about its own version
//! while you are trying to hear a preset has its priorities wrong.
//!
//! When a newer release exists, fastframe-update decides whether this copy may
//! replace itself. A copy that a package manager owns (Homebrew, the AUR, a
//! .deb or .rpm) never does: the status bar says which tool updates it. A copy
//! that may downloads only when asked, checks the release's signature against
//! the key compiled in here, and restarts only when asked again.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::RichText;
pub use fastframe_update::{DownloadState, Receipt, Release, Unsupported};
use fastframe_update::{
    MacConfig, PackageManager, Request, Response, Transport, UpdateConfig, Updater, CHECK_INTERVAL,
};

use crate::theme;

/// The version this binary was built as.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where a person goes to get a newer one.
pub const RELEASES: &str = "https://github.com/crmne/tonepush/releases/latest";

/// TonePush's releases, as the updater needs to know them.
///
/// The slug names the release assets (`tonepush-v0.7.0-macos-universal.dmg`),
/// the Homebrew cask, the portable marker (`tonepush-portable.txt`), the
/// macOS bundle's executable and the `tonepush 0.7.0` that `--version`
/// answers. The bundle identifier is the one in `packaging/macos/Info.plist`.
pub const CONFIG: UpdateConfig = UpdateConfig {
    // The Linux and Windows archives carry the `tonepush` command-line tool
    // beside the editor. The editor is the one this updater installs, and only
    // a running `tonepush-gui` next to the marker replaces itself.
    portable_executable: Some("tonepush-gui"),
    macos: MacConfig {
        bundle_ids: &["rocks.tonepush.editor"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    // Releases sign checksums.txt with the matching private key; a release
    // without a valid signature is never downloaded.
    publisher_key: Some(include_str!("../../../assets/update-public-key.hex")),
    ..UpdateConfig::new("crmne/tonepush", "TonePush", "tonepush", VERSION)
};

/// What `--version` prints. The previous release's update helper runs a
/// downloaded TonePush with `--version` and accepts it only on this answer.
pub fn version_line() -> String {
    format!("{} {VERSION}", CONFIG.slug)
}

/// The updater, over the same HTTP client the rest of TonePush uses.
pub fn updater() -> Updater {
    Updater::new(CONFIG, UreqTransport::new())
}

/// fastframe-update's [`Transport`] over ureq.
///
/// It must not follow redirects: the updater follows them itself, and only
/// to GitHub's release hosts. Statuses are answers, not errors, for the same
/// reason.
struct UreqTransport(ureq::Agent);

impl UreqTransport {
    fn new() -> Self {
        Self(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_connect(Some(Duration::from_secs(15)))
                // Long enough for the whole disk image on a slow line.
                .timeout_global(Some(Duration::from_secs(15 * 60)))
                .build()
                .new_agent(),
        )
    }
}

impl Transport for UreqTransport {
    fn get(&self, request: &Request<'_>) -> anyhow::Result<Response> {
        let response = self
            .0
            .get(request.url)
            .header("Accept", request.accept)
            .header("User-Agent", request.user_agent)
            .call()?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Response {
            status,
            location,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

/// How a person updates a copy that cannot update itself, in TonePush's words.
pub fn advice(reason: &Unsupported) -> String {
    match reason {
        Unsupported::Homebrew => "Installed with Homebrew: update it with brew upgrade.".into(),
        Unsupported::SystemPackage(PackageManager::Pacman) => {
            "Installed from the AUR: update it with your AUR helper or pacman.".into()
        }
        Unsupported::SystemPackage(PackageManager::Apt) => {
            "Installed from a .deb package: update it with apt or your software center.".into()
        }
        Unsupported::SystemPackage(PackageManager::Dnf) => {
            "Installed from an .rpm package: update it with dnf or your software center.".into()
        }
        Unsupported::SystemDirectory => {
            "Installed system-wide: update it through the package manager that installed it.".into()
        }
        Unsupported::Flatpak => "Installed with Flatpak: update it with flatpak update.".into(),
        Unsupported::Snap => "Installed with Snap: update it with snap refresh.".into(),
        Unsupported::Nix => "Installed with Nix: update it through Nix.".into(),
        Unsupported::Cargo => "Built with cargo install: update it the same way.".into(),
        Unsupported::MoveToApplications => {
            "Move TonePush to Applications, then open it again to update it here.".into()
        }
        // A copy built from source, or unpacked without its marker: nothing
        // next to it says which files belong to it, so it is not replaced.
        _ => "This copy cannot replace itself. Download the new version from the release page."
            .into(),
    }
}

/// Everything the updater tells the interface, from its threads.
enum Message {
    Checked(Option<Release>),
    Support(Result<(), Unsupported>),
    Progress { received: u64, total: u64 },
    Downloaded(Result<fastframe_update::Prepared, String>),
    HandedOff(Result<(), String>),
}

/// The update state the status bar shows and drives.
pub struct Updates {
    /// When the next check is due; `None` checks on the next frame.
    next_check: Option<Instant>,
    checking: bool,
    release: Option<Release>,
    /// Whether this copy may replace itself, once asked.
    support: Option<Result<(), Unsupported>>,
    download: DownloadState,
    /// Handed to the new version: this launch's own arguments.
    relaunch: Vec<String>,
    /// The helper's proof that this launch is an update it installed.
    receipt: Option<Receipt>,
    /// The helper's message when it had to put the previous version back.
    problem: Option<String>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}

impl Default for Updates {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            next_check: None,
            checking: false,
            release: None,
            support: None,
            download: DownloadState::Idle,
            relaunch: Vec::new(),
            receipt: None,
            problem: None,
            tx,
            rx,
        }
    }
}

impl Updates {
    /// What this launch brought from the update helper: the receipt of an
    /// update it installed, or the message of one it rolled back.
    pub fn launched(
        &mut self,
        receipt: Option<Receipt>,
        error: Option<String>,
        arguments: Vec<String>,
    ) {
        self.receipt = receipt;
        self.problem = error;
        self.relaunch = arguments;
    }

    /// The rolled-back update's message, once.
    pub fn take_problem(&mut self) -> Option<String> {
        self.problem.take()
    }

    /// Tells the helper the new version is up, so it keeps it. Called after
    /// the first frame; until then the helper stands ready to roll back.
    pub fn acknowledge(&mut self) {
        if let Some(receipt) = self.receipt.take() {
            std::thread::spawn(move || {
                if let Err(error) = receipt.acknowledge() {
                    eprintln!("could not confirm the update started: {error:#}");
                }
            });
        }
    }

    /// Starts the daily check when it is due and takes whatever the threads
    /// have answered.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if !self.checking && self.next_check.is_none_or(|due| now >= due) {
            self.checking = true;
            self.next_check = Some(now + CHECK_INTERVAL);
            self.spawn(ctx, |_| {
                // Every failure is the same failure: say nothing.
                Message::Checked(updater().check().ok().flatten())
            });
        }
        while let Ok(message) = self.rx.try_recv() {
            self.receive(ctx, message);
        }
    }

    fn receive(&mut self, ctx: &egui::Context, message: Message) {
        match message {
            Message::Checked(release) => {
                self.checking = false;
                // A download already under way or waiting for a restart
                // finishes first; the next day's check offers anything newer.
                let busy = !matches!(
                    self.download,
                    DownloadState::Idle | DownloadState::Failed(_)
                );
                if let Some(release) = release.filter(|_| !busy) {
                    if self.release.as_ref() != Some(&release) {
                        self.release = Some(release);
                        self.download = DownloadState::Idle;
                        self.support = None;
                        self.spawn(ctx, |_| {
                            Message::Support(updater().installation().map(|_| ()))
                        });
                    }
                }
            }
            Message::Support(support) => self.support = Some(support),
            Message::Progress { received, total } => {
                if matches!(self.download, DownloadState::Downloading { .. }) {
                    self.download = DownloadState::Downloading { received, total };
                }
            }
            Message::Downloaded(result) => {
                self.download = match result {
                    Ok(prepared) => DownloadState::Ready(Box::new(prepared)),
                    Err(error) => DownloadState::Failed(error),
                };
            }
            Message::HandedOff(Ok(())) => {
                // The helper waits for this process to exit. Closing the
                // window goes the normal way out, so the pedal is let go
                // cleanly before the new version reconnects to it.
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Message::HandedOff(Err(error)) => self.download = DownloadState::Failed(error),
        }
    }

    fn spawn(
        &self,
        ctx: &egui::Context,
        work: impl FnOnce(&Sender<Message>) -> Message + Send + 'static,
    ) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let message = work(&tx);
            let _ = tx.send(message);
            ctx.request_repaint();
        });
    }

    fn download(&mut self, ctx: &egui::Context) {
        let Some(release) = self.release.clone() else {
            return;
        };
        if !matches!(self.support, Some(Ok(())))
            || !matches!(
                self.download,
                DownloadState::Idle | DownloadState::Failed(_)
            )
        {
            return;
        }
        self.download = DownloadState::Downloading {
            received: 0,
            total: 0,
        };
        let progress_ctx = ctx.clone();
        self.spawn(ctx, move |tx| {
            let tx = tx.clone();
            let result = updater().download(&release, move |received, total| {
                let _ = tx.send(Message::Progress { received, total });
                progress_ctx.request_repaint();
            });
            Message::Downloaded(result.map_err(|error| format!("{error:#}")))
        });
    }

    fn restart(&mut self, ctx: &egui::Context) {
        if !matches!(self.download, DownloadState::Ready(_)) {
            return;
        }
        let DownloadState::Ready(prepared) =
            std::mem::replace(&mut self.download, DownloadState::Installing)
        else {
            return;
        };
        let arguments = self.relaunch.clone();
        self.spawn(ctx, move |_| {
            Message::HandedOff(
                updater()
                    .handoff(*prepared, arguments)
                    .map_err(|error| format!("{error:#}")),
            )
        });
    }

    /// The version in the status bar's corner, and whatever an update needs
    /// next to it. Laid out right to left: the version sits furthest right,
    /// and the update's one action to its left.
    pub fn status_ui(&mut self, ui: &mut egui::Ui) {
        let Some(release) = self.release.clone() else {
            let label = RichText::new(format!("TonePush {VERSION}"))
                .small()
                .color(theme::DIM);
            if ui
                .add(egui::Label::new(label).sense(egui::Sense::click()))
                .on_hover_text(format!("TonePush {VERSION} · click for the releases page"))
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(RELEASES));
            }
            return;
        };

        let hover = match &self.support {
            Some(Err(reason)) => format!("{} Click to open the release.", advice(reason)),
            _ => format!(
                "TonePush {} is out. Click for what changed.",
                release.version
            ),
        };
        let label = RichText::new(format!(
            "TonePush {VERSION} · {} available",
            release.version
        ))
        .small()
        .color(theme::ACCENT);
        if ui
            .add(egui::Label::new(label).sense(egui::Sense::click()))
            .on_hover_text(hover)
            .clicked()
        {
            ui.ctx()
                .open_url(egui::OpenUrl::new_tab(release_page(&release)));
        }

        if !matches!(self.support, Some(Ok(()))) {
            return;
        }
        let ctx = ui.ctx().clone();
        match &self.download {
            DownloadState::Idle => {
                if ui
                    .small_button("Update")
                    .on_hover_text(format!(
                        "Download TonePush {} and check its signature. Nothing is \
                         installed until you restart.",
                        release.version
                    ))
                    .clicked()
                {
                    self.download(&ctx);
                }
            }
            DownloadState::Downloading { received, total } => {
                let progress = if *total > 0 {
                    *received as f32 / *total as f32
                } else {
                    0.0
                };
                crate::processor::operation_progress(ui, "Downloading update", progress);
            }
            DownloadState::Ready(_) => {
                if ui
                    .small_button(RichText::new("Restart to update").color(theme::ACCENT))
                    .on_hover_text(format!(
                        "TonePush closes, lets the pedal go and opens {}. If the new \
                         version does not start, this one comes back.",
                        release.version
                    ))
                    .clicked()
                {
                    self.restart(&ctx);
                }
            }
            DownloadState::Installing => {
                theme::spinner(ui);
                ui.label(RichText::new("Restarting…").small().color(theme::DIM));
            }
            DownloadState::Failed(error) => {
                if ui
                    .small_button("Retry update")
                    .on_hover_text(format!("The update did not finish: {error}"))
                    .clicked()
                {
                    self.download(&ctx);
                }
            }
        }
    }
}

/// The release's own page, or the latest release when GitHub gave none.
fn release_page(release: &Release) -> &str {
    if release.url.starts_with("https://github.com/") {
        &release.url
    } else {
        RELEASES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(CONFIG.repository, "crmne/tonepush");
        assert_eq!(CONFIG.slug, "tonepush");
        assert!(CONFIG.publisher_key.is_some());
    }

    /// The updater installs the file named here from the archive, so it must
    /// be the editor's binary and not the `tonepush` command-line tool.
    #[test]
    fn the_portable_executable_is_the_editor() {
        let manifest = include_str!("../Cargo.toml");
        let name = CONFIG.portable_executable.unwrap();
        assert!(
            manifest.contains(&format!("[[bin]]\nname = \"{name}\"")),
            "{name} is not this crate's binary"
        );
        assert_ne!(name, CONFIG.slug);
    }

    /// The marker the release workflow puts next to the portable editor, in
    /// the exact form fastframe-update reads: `<slug>-portable.txt` holding
    /// `<slug>-portable-v1`.
    #[test]
    fn the_portable_marker_is_the_one_the_updater_reads() {
        let marker = include_str!("../../../packaging/tonepush-portable.txt");
        assert_eq!(marker.trim(), format!("{}-portable-v1", CONFIG.slug));
        // The build job packs the Linux tarballs and the Windows zips from
        // one script, which must copy the marker beside the binaries. The
        // macOS app updates as a bundle and needs none.
        let release = include_str!("../../../.github/workflows/release.yml");
        assert!(
            release.lines().any(|line| line.contains("cp ")
                && line.contains("packaging/tonepush-portable.txt")
                && line.contains("\"dist/$name/\"")),
            "release.yml must put the marker into the portable archives"
        );
    }

    /// The bundle identifier the updater insists on is the one the app is
    /// built with; a mismatch would call every release a foreign bundle.
    #[test]
    fn the_macos_identity_matches_the_bundle() {
        let plist = include_str!("../../../packaging/macos/Info.plist");
        for id in CONFIG.macos.bundle_ids {
            assert!(plist.contains(&format!("<string>{id}</string>")), "{id}");
        }
        assert!(plist.contains(&format!(
            "<key>CFBundleExecutable</key><string>{}</string>",
            CONFIG.slug
        )));
    }

    /// The answer the previous release's helper checks before installing.
    #[test]
    fn the_version_line_is_the_slug_and_version() {
        assert_eq!(
            version_line(),
            format!("tonepush {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn package_managed_copies_are_told_which_tool_updates_them() {
        for (reason, tool) in [
            (Unsupported::Homebrew, "brew"),
            (Unsupported::SystemPackage(PackageManager::Pacman), "AUR"),
            (Unsupported::SystemPackage(PackageManager::Apt), "apt"),
            (Unsupported::SystemPackage(PackageManager::Dnf), "dnf"),
            (Unsupported::NotPortable, "release page"),
        ] {
            let text = advice(&reason);
            assert!(text.contains(tool), "{reason:?}: {text}");
        }
    }

    /// One request against a local server that answers with a redirect.
    fn serve_once(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}/latest", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            stream.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (address, server)
    }

    /// The updater follows redirects itself, only to GitHub's release hosts,
    /// so the transport must hand a redirect back rather than follow it.
    #[test]
    fn the_transport_returns_redirects_instead_of_following_them() {
        let (address, server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: https://example.com/elsewhere\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let response = UreqTransport::new()
            .get(&Request {
                url: &address,
                accept: "application/json",
                user_agent: "TonePush/9.9.9",
            })
            .unwrap();
        assert_eq!(response.status, 302);
        assert_eq!(
            response.location.as_deref(),
            Some("https://example.com/elsewhere")
        );
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("user-agent: tonepush/9.9.9"), "{request}");
        assert!(request.contains("accept: application/json"), "{request}");
    }

    #[test]
    fn the_transport_reports_statuses_as_answers() {
        let (address, server) = serve_once(
            "HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\nConnection: close\r\n\r\ngone",
        );
        let mut response = UreqTransport::new()
            .get(&Request {
                url: &address,
                accept: "*/*",
                user_agent: "TonePush/9.9.9",
            })
            .unwrap();
        assert_eq!(response.status, 404);
        let mut body = String::new();
        response.body.read_to_string(&mut body).unwrap();
        assert_eq!(body, "gone");
        server.join().unwrap();
    }

    fn release() -> Release {
        Release {
            version: "99.0.0".into(),
            url: "https://github.com/crmne/tonepush/releases/tag/v99.0.0".into(),
        }
    }

    /// A found release asks whether this copy can replace itself; nothing is
    /// downloaded until the person clicks Update.
    #[test]
    fn a_new_release_waits_for_the_person_before_downloading() {
        let ctx = egui::Context::default();
        let mut updates = Updates {
            checking: true,
            ..Updates::default()
        };
        updates.receive(&ctx, Message::Checked(Some(release())));
        assert_eq!(updates.release, Some(release()));
        assert!(!updates.checking);
        assert!(matches!(updates.download, DownloadState::Idle));
        updates.receive(&ctx, Message::Support(Err(Unsupported::Homebrew)));
        // A package-managed copy has no Update button to press, and pressing
        // it anyway does nothing.
        updates.download(&ctx);
        assert!(matches!(updates.download, DownloadState::Idle));
    }

    #[test]
    fn a_check_during_a_download_does_not_replace_it() {
        let ctx = egui::Context::default();
        let mut updates = Updates {
            release: Some(release()),
            support: Some(Ok(())),
            download: DownloadState::Downloading {
                received: 1,
                total: 2,
            },
            ..Updates::default()
        };
        let newer = Release {
            version: "100.0.0".into(),
            ..release()
        };
        updates.receive(&ctx, Message::Checked(Some(newer)));
        assert_eq!(updates.release, Some(release()));
        assert!(matches!(
            updates.download,
            DownloadState::Downloading { .. }
        ));
    }

    #[test]
    fn a_failed_handoff_can_be_retried() {
        let ctx = egui::Context::default();
        let mut updates = Updates {
            release: Some(release()),
            support: Some(Ok(())),
            download: DownloadState::Installing,
            ..Updates::default()
        };
        updates.receive(&ctx, Message::HandedOff(Err("no helper".into())));
        assert!(matches!(&updates.download, DownloadState::Failed(e) if e == "no helper"));
        // Restart does nothing without a verified download.
        updates.restart(&ctx);
        assert!(matches!(updates.download, DownloadState::Failed(_)));
    }

    #[test]
    fn a_rolled_back_update_is_reported_once() {
        let mut updates = Updates::default();
        updates.launched(None, Some("restored".into()), vec![]);
        assert_eq!(updates.take_problem().as_deref(), Some("restored"));
        assert_eq!(updates.take_problem(), None);
    }
}
