//! Crash reports, only when the person turns them on.
//!
//! Off until someone says yes: the app asks once, on its first run, and
//! Settings has a switch. The choice is `crash_reports` in `config.json`;
//! `TONEPUSH_CRASH_REPORTS=1` or `0` overrides it. What's sent is a Rust
//! panic only, to TonePush's project on our own error tracker, Bugsink at
//! `errors.plenty.is`, which speaks the Sentry protocol:
//!
//! - where in the source code it panicked (file, line and column);
//! - the panic's message only when it was written in the source code, or
//!   the start of one of Rust's own messages ("index out of bounds"), never
//!   text put into it while running, which could hold a file, tone or
//!   preset name or a token;
//! - the release (`tonepush@0.9.1`), whether it is a development build, and
//!   the operating system's name, kernel version (on Linux) and
//!   architecture.
//!
//! Never the user or host name, the command line, environment variables,
//! the config file, file names, tones, presets, backups or the account's
//! token. Paths in a report have the home folder replaced with `~`. At most
//! five reports of one panic and thirty in all are sent an hour, counted
//! across restarts (`crash-reports.json` beside `config.json`), and thirty
//! a session.
//!
//! The panic hook re-reads the setting when a panic happens, so a change in
//! Settings applies at once. It sends from a thread of its own and waits a
//! few seconds for it, so a panic that ends the process still gets its
//! report out.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;

/// The `tonepush-desktop` project on our Bugsink server. A DSN only lets a
/// client submit events, so it ships inside the program like any other
/// client's.
const DEFAULT_DSN: &str = "https://faf40925c6714fd29a0f0415620edd2c@errors.plenty.is/9";

/// `TONEPUSH_CRASH_REPORTS_DSN` at build time replaces the address, and an
/// empty value builds a TonePush that can't send crash reports at all.
const COMPILED_DSN: Option<&str> = match option_env!("TONEPUSH_CRASH_REPORTS_DSN") {
    Some(dsn) if dsn.is_empty() => None,
    Some(dsn) => Some(dsn),
    None => Some(DEFAULT_DSN),
};

/// `1` or `0` (also `true`/`false`, `on`/`off`, `yes`/`no`) over the file.
pub const ENV: &str = "TONEPUSH_CRASH_REPORTS";

/// How long a panicking thread waits for its report to go out.
const SEND_WAIT: Duration = Duration::from_secs(5);
const SAME_PANIC_PER_HOUR: usize = 5;
const REPORTS_PER_HOUR: usize = 30;
const REPORTS_PER_SESSION: u32 = 30;
const HOUR: u64 = 3600;
/// The longest message a report keeps.
const MESSAGE_CHARS: usize = 200;
const WITHHELD: &str = "panic (its message was formed at run time, so it is not sent)";
/// The thread that sends; a panic on it is never reported.
const THREAD: &str = "tonepush-crash-report";

/// The person's choice, and where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Consent {
    /// `None` until they've been asked.
    pub enabled: Option<bool>,
    /// Set by `TONEPUSH_CRASH_REPORTS`, which the switch can't change.
    pub from_env: bool,
}

impl Consent {
    /// From the environment, else the config.
    pub(crate) fn read(config: Option<&Config>) -> Self {
        Self::resolve(
            std::env::var(ENV).ok().as_deref(),
            config.and_then(|c| c.crash_reports),
        )
    }

    fn resolve(env: Option<&str>, config: Option<bool>) -> Self {
        match env.and_then(switch) {
            Some(on) => Self {
                enabled: Some(on),
                from_env: true,
            },
            None => Self {
                enabled: config,
                from_env: false,
            },
        }
    }

    pub fn sends(self) -> bool {
        self.enabled == Some(true)
    }

    /// Nobody has said yes or no yet: the app asks.
    pub fn undecided(self) -> bool {
        self.enabled.is_none() && available()
    }
}

fn switch(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// This build has somewhere to send reports to.
pub fn available() -> bool {
    COMPILED_DSN.and_then(Dsn::parse).is_some()
}

static INSTALLED: OnceLock<()> = OnceLock::new();

/// Report this process's panics, when the person allows it. Call it once,
/// early; later calls do nothing. Starts no thread.
pub fn install() {
    let Some(dsn) = COMPILED_DSN.and_then(Dsn::parse) else {
        return;
    };
    let Some(config_file) = hx_catalog::home::config() else {
        return;
    };
    if INSTALLED.set(()).is_err() {
        return;
    }
    let reporter = Arc::new(Reporter {
        dsn,
        config_file,
        session: Mutex::new(0),
    });
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        reporter.panicked(info);
    }));
}

thread_local! {
    static REPORTING: Cell<bool> = const { Cell::new(false) };
}

struct Reporter {
    dsn: Dsn,
    /// `config.json`, read at each panic; the limits are kept beside it.
    config_file: PathBuf,
    /// Reports sent this session.
    session: Mutex<u32>,
}

impl Reporter {
    fn panicked(&self, info: &std::panic::PanicHookInfo<'_>) {
        if std::thread::current().name() == Some(THREAD) || REPORTING.with(Cell::get) {
            return;
        }
        REPORTING.with(|reporting| reporting.set(true));
        let crash = Crash::from_panic(info);
        if let Some(outcome) = self.report(&crash, SystemTime::now()) {
            eprintln!("{outcome}");
        }
        REPORTING.with(|reporting| reporting.set(false));
    }

    /// Send `crash` if the person allows it and the limits do: `None` when
    /// nothing was tried, else a sentence saying how it went.
    fn report(&self, crash: &Crash, now: SystemTime) -> Option<String> {
        let config = Config::read(&self.config_file);
        if !Consent::read(config.as_ref()).sends() || !self.admit(crash, now) {
            return None;
        }
        let body = envelope(crash, now);
        let (dsn, sent_body) = (self.dsn.clone(), body.clone());
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(THREAD.into())
            .spawn(move || {
                let _ = tx.send(post(&dsn, sent_body));
            });
        let result = match spawned {
            Ok(_) => rx
                .recv_timeout(SEND_WAIT)
                .unwrap_or_else(|_| Err("timed out".to_owned())),
            // No thread to spare: send from this one.
            Err(_) => post(&self.dsn, body),
        };
        Some(match result {
            Ok(()) => "Sent a crash report to the TonePush team.".into(),
            Err(why) => format!("Couldn't send the crash report: {why}"),
        })
    }

    /// Within the limits: five of one panic and thirty in all an hour,
    /// across restarts, and thirty a session. Counts what it lets through.
    fn admit(&self, crash: &Crash, now: SystemTime) -> bool {
        let Ok(mut session) = self.session.lock() else {
            return false;
        };
        if *session >= REPORTS_PER_SESSION {
            return false;
        }
        let file = log_file(&self.config_file);
        let at = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let mut log = SentLog::read(&file);
        log.sent
            .retain(|s| at.saturating_sub(s.at) < HOUR && s.at <= at);
        let key = crash.key();
        if log.sent.len() >= REPORTS_PER_HOUR
            || log.sent.iter().filter(|s| s.key == key).count() >= SAME_PANIC_PER_HOUR
        {
            return false;
        }
        log.sent.push(Sent { key, at });
        // A log that can't be written still counts in this session.
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(&log) {
            let _ = std::fs::write(&file, bytes);
        }
        *session += 1;
        true
    }
}

/// Where the reports sent in the last hour are counted: beside the config.
fn log_file(config_file: &Path) -> PathBuf {
    config_file.with_file_name("crash-reports.json")
}

/// The reports sent in the last hour, kept between runs so a program that
/// panics at every start can't send one each time.
#[derive(Default, Serialize, Deserialize)]
struct SentLog {
    sent: Vec<Sent>,
}

#[derive(Serialize, Deserialize)]
struct Sent {
    key: String,
    at: u64,
}

impl SentLog {
    fn read(file: &Path) -> Self {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

/// What a report says about one panic.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Crash {
    message: String,
    file: String,
    line: u32,
    column: u32,
}

impl Crash {
    fn from_panic(info: &std::panic::PanicHookInfo<'_>) -> Self {
        let payload = info.payload();
        let message = if let Some(text) = payload.downcast_ref::<&'static str>() {
            public_message(text, false)
        } else if let Some(text) = payload.downcast_ref::<String>() {
            public_message(text, true)
        } else {
            WITHHELD.to_string()
        };
        let (file, line, column) = info.location().map_or((String::new(), 0, 0), |l| {
            (scrub_home(l.file()).into_owned(), l.line(), l.column())
        });
        Self {
            message,
            file,
            line,
            column,
        }
    }

    /// One panic, for the limits: where it happened.
    fn key(&self) -> String {
        format!("{}:{}:{}", self.file, self.line, self.column)
    }
}

/// The prefixes of Rust's own panic messages, which say what went wrong
/// before any value.
const STANDARD_MESSAGES: &[&str] = &[
    "called `",
    "index out of bounds",
    "attempt to ",
    "already borrowed",
    "already mutably borrowed",
    "byte index ",
    "range start index ",
    "range end index ",
    "slice index ",
    "removal index ",
    "insertion index ",
    "assertion `left",
    "assertion failed",
    "capacity overflow",
    "RefCell already",
];

/// A message written in the source code (`formatted` false) is sent as it
/// is; one formed at run time only as far as the start of a standard
/// message, before any value. Anything else is withheld.
fn public_message(text: &str, formatted: bool) -> String {
    let kept = if formatted {
        let end = [": ", "; ", "\n"]
            .iter()
            .filter_map(|cut| text.find(cut))
            .min()
            .unwrap_or(text.len());
        let head = &text[..end];
        if !STANDARD_MESSAGES.iter().any(|p| head.starts_with(p)) {
            return WITHHELD.to_string();
        }
        head
    } else {
        text
    };
    let scrubbed = scrub_home(kept.trim());
    if scrubbed.is_empty() {
        return WITHHELD.to_string();
    }
    scrubbed.chars().take(MESSAGE_CHARS).collect()
}

/// A Sentry envelope holding one event for `crash`.
fn envelope(crash: &Crash, now: SystemTime) -> Vec<u8> {
    let event_id = event_id();
    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let in_app = !crash.file.starts_with('/') && !crash.file.starts_with('~');
    let mut os = json!({ "type": "os", "name": os_name() });
    if let Some(version) = kernel_version() {
        os["kernel_version"] = json!(version);
    }
    let event = json!({
        "event_id": event_id,
        "timestamp": timestamp,
        "platform": "native",
        "level": "fatal",
        "release": release(),
        "environment": environment(),
        "tags": {
            "component": "desktop",
            "os.family": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "contexts": { "os": os },
        "exception": { "values": [{
            "type": "panic",
            "value": crash.message,
            "mechanism": { "type": "panic", "handled": false },
            "stacktrace": { "frames": [{
                "filename": crash.file,
                "lineno": crash.line,
                "colno": crash.column,
                "in_app": in_app,
            }]},
        }]},
        "fingerprint": ["panic", crash.key(), crash.message],
        "sdk": { "name": "tonepush.crash_reports", "version": env!("CARGO_PKG_VERSION") },
    })
    .to_string();
    let mut body = Vec::with_capacity(event.len() + 128);
    body.extend_from_slice(json!({ "event_id": event_id }).to_string().as_bytes());
    body.push(b'\n');
    body.extend_from_slice(
        json!({ "type": "event", "length": event.len(), "content_type": "application/json" })
            .to_string()
            .as_bytes(),
    );
    body.push(b'\n');
    body.extend_from_slice(event.as_bytes());
    body.push(b'\n');
    body
}

/// `tonepush@0.9.1`: the release reports are grouped under.
pub fn release() -> String {
    format!("tonepush@{}", env!("CARGO_PKG_VERSION"))
}

fn environment() -> &'static str {
    if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
    }
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        other => other,
    }
}

/// The kernel's version on Linux, such as `6.17.2-arch1-1`.
fn kernel_version() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let release = release.trim();
    (!release.is_empty() && release.len() < 128).then(|| release.to_string())
}

/// 32 random hex digits.
fn event_id() -> String {
    let random = || {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        hasher.finish()
    };
    format!("{:016x}{:016x}", random(), random())
}

/// Where to send: `https://<key>@<host>/<project>`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Dsn {
    key: String,
    /// `https://host[:port]`.
    origin: String,
    project: String,
}

impl Dsn {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (scheme, rest) = text.split_once("://")?;
        if !matches!(scheme, "https" | "http") {
            return None;
        }
        let (key, rest) = rest.split_once('@')?;
        let (host, project) = rest.split_once('/')?;
        let project = project.trim_end_matches('/');
        let valid = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        };
        if !valid(key) || !valid(host) || !valid(project) {
            return None;
        }
        Some(Self {
            key: key.to_string(),
            origin: format!("{scheme}://{host}"),
            project: project.to_string(),
        })
    }

    fn envelope_url(&self) -> String {
        format!("{}/api/{}/envelope/", self.origin, self.project)
    }
}

/// POST the envelope. The answer is a short reason, never the address.
fn post(dsn: &Dsn, body: Vec<u8>) -> Result<(), String> {
    let agent: ureq::Agent = ureq::config::Config::builder()
        .https_only(dsn.origin.starts_with("https:"))
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(SEND_WAIT))
        .user_agent(format!("TonePush/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();
    let response = agent
        .post(&dsn.envelope_url())
        .header("Content-Type", "application/x-sentry-envelope")
        .header(
            "X-Sentry-Auth",
            &format!(
                "Sentry sentry_version=7, sentry_key={}, sentry_client=tonepush.crash_reports/{}",
                dsn.key,
                env!("CARGO_PKG_VERSION")
            ),
        )
        .send(&body[..])
        .map_err(|error| match error {
            ureq::Error::Timeout(_) => "timed out".to_owned(),
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
                "no connection".to_owned()
            }
            _ => "the request failed".to_owned(),
        })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("the server answered {status}"));
    }
    Ok(())
}

// --- Scrubbing ------------------------------------------------------------

fn home() -> Option<&'static str> {
    static HOME: OnceLock<Option<String>> = OnceLock::new();
    HOME.get_or_init(|| {
        hx_catalog::home::home()
            .and_then(|home| home.to_str().map(str::to_owned))
            .filter(|home| home.len() > 3)
    })
    .as_deref()
}

/// The user's home folder in `text`, written as `~`.
fn scrub_home(text: &str) -> Cow<'_, str> {
    scrub_with(text, home())
}

/// Replace `home` and any `/home/<name>`, `/Users/<name>` or
/// `<drive>:\Users\<name>` with `~`.
fn scrub_with<'a>(text: &'a str, home: Option<&str>) -> Cow<'a, str> {
    let mut text = Cow::Borrowed(text);
    if let Some(home) = home {
        let trimmed = home.trim_end_matches(['/', '\\']);
        if !trimmed.is_empty() && find_ignore_ascii_case(&text, trimmed, 0).is_some() {
            text = Cow::Owned(replace_home(&text, trimmed));
        }
    }
    for prefix in [":\\Users\\", ":/Users/", "/home/", "/Users/", "\\Users\\"] {
        if find_ignore_ascii_case(&text, prefix, 0).is_some() {
            text = Cow::Owned(replace_user_folders(&text, prefix));
        }
    }
    text
}

fn find_ignore_ascii_case(text: &str, needle: &str, from: usize) -> Option<usize> {
    let haystack = text.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len())
        .find(|&start| haystack[start..start + needle.len()].eq_ignore_ascii_case(needle))
}

fn replace_home(text: &str, home: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(start) = find_ignore_ascii_case(text, home, from) {
        // A longer folder name that merely starts with the home path, such
        // as /home/al for /home/alice, is not the home folder.
        let end = start + home.len();
        let boundary = text[end..].chars().next().is_none_or(|next| {
            !(next.is_alphanumeric() || next == '_' || next == '-' || next == '.')
        });
        out.push_str(&text[from..start]);
        out.push_str(if boundary { "~" } else { &text[start..end] });
        from = end;
    }
    out.push_str(&text[from..]);
    out
}

/// `…/home/alice/x` → `…~/x`; also drops a drive letter or a `/var` before
/// the user folder so the whole home prefix becomes `~`.
fn replace_user_folders(text: &str, prefix: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    let mut search = 0;
    while let Some(found) = find_ignore_ascii_case(text, prefix, search) {
        let name_start = found + prefix.len();
        let windows = prefix.contains('\\') || prefix.starts_with(':');
        let name_end = text[name_start..]
            .find(|c: char| {
                matches!(c, '/' | '\\' | '"' | '\'' | '\0' | '\n')
                    || (!windows && c.is_whitespace())
            })
            .map_or(text.len(), |offset| name_start + offset);
        let mut start = found;
        if prefix.starts_with(':') {
            // Include the drive letter.
            start = found.saturating_sub(1);
        } else if prefix.starts_with('/') && text[..found].ends_with("/var") {
            start = found - 4;
        }
        // `/home/` in the middle of a word (example.com/home/) is a URL
        // path, not a user folder.
        let preceded_by_word = start > 0
            && (bytes[start - 1].is_ascii_alphanumeric()
                || (prefix == "\\Users\\" && bytes[start - 1] == b':'));
        if name_end == name_start || preceded_by_word || start < from {
            search = name_start;
            continue;
        }
        out.push_str(&text[from..start]);
        out.push('~');
        from = name_end;
        search = name_end;
    }
    out.push_str(&text[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    /// A scratch folder of its own for each test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tonepush-crash-reports-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn reporter(dir: &Path, dsn: &str) -> Reporter {
        Reporter {
            dsn: Dsn::parse(dsn).unwrap(),
            config_file: dir.join("config.json"),
            session: Mutex::new(0),
        }
    }

    fn choose(dir: &Path, on: Option<bool>) {
        let config = Config {
            crash_reports: on,
            ..Config::default()
        };
        std::fs::write(
            dir.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
    }

    fn crash(line: u32) -> Crash {
        Crash {
            message: "attempt to subtract with overflow".into(),
            file: "crates/tonepush-gui/src/lib.rs".into(),
            line,
            column: 9,
        }
    }

    #[test]
    fn off_until_someone_says_yes() {
        assert_eq!(Consent::resolve(None, None).enabled, None);
        assert!(!Consent::resolve(None, None).sends());
        assert!(Consent::resolve(None, Some(true)).sends());
        assert!(!Consent::resolve(None, Some(false)).sends());
        let env = Consent::resolve(Some("0"), Some(true));
        assert_eq!(
            env,
            Consent {
                enabled: Some(false),
                from_env: true
            }
        );
        assert!(Consent::resolve(Some("on"), None).sends());
        assert_eq!(
            Consent::resolve(Some("maybe"), Some(false)).enabled,
            Some(false)
        );
        assert!(Consent::resolve(None, None).undecided());
        assert!(!Consent::resolve(None, Some(false)).undecided());
    }

    #[test]
    fn the_choice_is_kept_in_the_config_and_left_out_until_made() {
        let config = Config::default();
        assert!(!serde_json::to_string(&config)
            .unwrap()
            .contains("crash_reports"));
        let config = Config {
            crash_reports: Some(true),
            ..Config::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""crash_reports":true"#), "{json}");
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.crash_reports, Some(true));
        let old: Config = serde_json::from_str(r#"{"favorites":[]}"#).unwrap();
        assert_eq!(old.crash_reports, None);
    }

    #[test]
    fn only_messages_from_the_source_code_are_sent() {
        assert_eq!(
            public_message("the worker is gone", false),
            "the worker is gone"
        );
        assert_eq!(
            public_message("bad file /home/alice/tones.hlx", false),
            "bad file ~/tones.hlx"
        );
        assert_eq!(
            public_message("index out of bounds: the len is 3 but the index is 5", true),
            "index out of bounds"
        );
        assert_eq!(
            public_message(
                "called `Result::unwrap()` on an `Err` value: Os { code: 2, message: \"/home/alice/My Tone.hlx\" }",
                true
            ),
            "called `Result::unwrap()` on an `Err` value"
        );
        assert_eq!(
            public_message("assertion `left == right` failed\n  left: \"token\"", true),
            "assertion `left == right` failed"
        );
        for private in [
            "could not open /Users/bob/Clean Lead.hlx",
            "preset said: Big Muff",
            "token abc123 rejected",
            "expected a setlist: Live 2026",
        ] {
            assert_eq!(public_message(private, true), WITHHELD, "{private}");
        }
        assert_eq!(public_message("", false), WITHHELD);
        assert_eq!(
            public_message(&"x".repeat(500), false).chars().count(),
            MESSAGE_CHARS
        );
    }

    #[test]
    fn home_folders_become_a_tilde() {
        let cases = [
            (
                "/home/alice/.cargo/registry/src/x.rs",
                "~/.cargo/registry/src/x.rs",
            ),
            ("/Users/Bob/src/lib.rs", "~/src/lib.rs"),
            (r"C:\Users\Carol Smith\src\x.rs", r"~\src\x.rs"),
            ("/var/home/frank/x", "~/x"),
            (
                "https://example.com/home/page",
                "https://example.com/home/page",
            ),
            ("src/main.rs", "src/main.rs"),
        ];
        for (input, expected) in cases {
            assert_eq!(scrub_with(input, None), expected, "{input}");
        }
        assert_eq!(
            scrub_with(
                "/srv/people/hank/x and /srv/people/hanko",
                Some("/srv/people/hank")
            ),
            "~/x and /srv/people/hanko"
        );
    }

    fn event_of(envelope: &[u8]) -> serde_json::Value {
        let text = std::str::from_utf8(envelope).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let item: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(item["type"], "event");
        assert_eq!(item["length"], lines[2].len());
        let event: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(header["event_id"], event["event_id"]);
        event
    }

    #[test]
    fn a_report_says_where_and_what_build_and_nothing_personal() {
        let crash = Crash {
            message: "index out of bounds".into(),
            file: scrub_with("/home/alice/.cargo/registry/src/lib.rs", None).into_owned(),
            line: 12,
            column: 5,
        };
        let event = event_of(&envelope(&crash, SystemTime::now()));
        assert_eq!(event["release"], release());
        assert_eq!(event["environment"], environment());
        assert_eq!(event["level"], "fatal");
        assert_eq!(event["tags"]["component"], "desktop");
        assert_eq!(event["contexts"]["os"]["name"], os_name());
        let exception = &event["exception"]["values"][0];
        assert_eq!(exception["value"], "index out of bounds");
        let frame = &exception["stacktrace"]["frames"][0];
        assert_eq!(frame["filename"], "~/.cargo/registry/src/lib.rs");
        assert_eq!(frame["lineno"], 12);
        assert_eq!(frame["in_app"], false);
        assert_eq!(event["event_id"].as_str().unwrap().len(), 32);
        for absent in ["server_name", "user", "request", "extra", "breadcrumbs"] {
            assert!(event.get(absent).is_none(), "{absent}");
        }
        let text = event.to_string();
        assert!(!text.contains("alice"), "{text}");
        let host = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|h| h.trim().to_owned());
        if let Some(host) = host.filter(|h| h.len() > 3) {
            assert!(!text.contains(&host), "the host name is never sent");
        }
    }

    #[test]
    fn limits_hold_across_restarts() {
        let dir = scratch("limits");
        let now = SystemTime::now();
        let first = reporter(&dir, DEFAULT_DSN);
        let admitted = (0..8).filter(|_| first.admit(&crash(1), now)).count();
        assert_eq!(admitted, SAME_PANIC_PER_HOUR);
        let second = reporter(&dir, DEFAULT_DSN);
        assert!(!second.admit(&crash(1), now));
        assert!(second.admit(&crash(2), now));
        assert!(second.admit(&crash(1), now + Duration::from_secs(HOUR)));
        let third = reporter(&dir, DEFAULT_DSN);
        let later = now + Duration::from_secs(3 * HOUR);
        let burst = (0..40)
            .filter(|n| third.admit(&crash(100 + n), later))
            .count();
        assert_eq!(burst, REPORTS_PER_HOUR);
        let fourth = reporter(&dir, DEFAULT_DSN);
        let slow = (0..40_u32)
            .filter(|n| {
                fourth.admit(
                    &crash(1000 + n),
                    now + Duration::from_secs(10 * HOUR + u64::from(*n) * HOUR),
                )
            })
            .count();
        assert_eq!(slow, REPORTS_PER_SESSION as usize);
        assert!(dir.join("crash-reports.json").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_compiled_address_is_our_bugsink_project() {
        let dsn = Dsn::parse(DEFAULT_DSN).expect("the default DSN parses");
        assert_eq!(dsn.origin, "https://errors.plenty.is");
        assert_eq!(dsn.project, "9");
        assert_eq!(
            dsn.envelope_url(),
            "https://errors.plenty.is/api/9/envelope/"
        );
        assert!(available());
        assert!(Dsn::parse("https://errors.plenty.is/9").is_none(), "no key");
        assert!(Dsn::parse("ftp://k@errors.plenty.is/9").is_none());
        assert!(Dsn::parse("https://k@errors.plenty.is/").is_none());
    }

    /// A one-request HTTP server: the request's head and body.
    fn serve_once(status: u16) -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            )
            .unwrap();
            (head, body)
        });
        (format!("http://k3y@{address}/9"), handle)
    }

    #[test]
    fn sends_only_when_allowed_and_says_how_it_went() {
        let dir = scratch("sends");
        let (dsn, server) = serve_once(200);
        let reporter = reporter(&dir, &dsn);
        assert_eq!(reporter.report(&crash(7), SystemTime::now()), None);
        choose(&dir, Some(false));
        assert_eq!(reporter.report(&crash(7), SystemTime::now()), None);
        choose(&dir, Some(true));
        let outcome = reporter.report(&crash(7), SystemTime::now());
        assert_eq!(
            outcome.as_deref(),
            Some("Sent a crash report to the TonePush team.")
        );
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /api/9/envelope/ HTTP/1.1"), "{head}");
        assert!(head.contains("sentry_key=k3y"), "{head}");
        let event = event_of(&body);
        assert_eq!(
            event["exception"]["values"][0]["stacktrace"]["frames"][0]["lineno"],
            7
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refusal_is_reported_without_the_address() {
        let dir = scratch("refusal");
        choose(&dir, Some(true));
        let (dsn, server) = serve_once(429);
        let outcome = reporter(&dir, &dsn).report(&crash(8), SystemTime::now());
        assert_eq!(
            outcome.as_deref(),
            Some("Couldn't send the crash report: the server answered 429")
        );
        server.join().unwrap();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        let outcome = reporter(&dir, &format!("http://k@{address}/9"))
            .report(&crash(9), SystemTime::now())
            .unwrap();
        assert!(
            outcome.starts_with("Couldn't send the crash report: "),
            "{outcome}"
        );
        assert!(!outcome.contains("127.0.0.1"), "{outcome}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The first run asks once; closing the question is a no, which is
    /// kept, and the question goes. The switch's answer is kept the same way.
    #[test]
    fn the_first_run_asks_once_and_keeps_the_answer() {
        use egui_kittest::kittest::Queryable;
        use egui_kittest::Harness;

        // Every save on this test's thread goes to a scratch file, never to
        // the machine's own config or another test's.
        let dir = scratch("question");
        crate::config::tests::SCRATCH
            .with(|scratch| *scratch.borrow_mut() = Some(dir.join("config.json")));

        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 760.0))
            .build_ui_state(
                |ui, app: &mut Option<crate::App>| match app {
                    Some(app) => app.draw(ui),
                    None => {
                        let (to_device, _nowhere) = mpsc::channel();
                        let (_silent, from_device) = mpsc::channel();
                        let mut new = crate::App::new(ui.ctx(), to_device, from_device);
                        new.crash_question = true;
                        *app = Some(new);
                    }
                },
                None,
            );
        harness.run_steps(3);
        harness.get_by_label("Send crash reports?");
        harness.key_press(egui::Key::Escape);
        harness.run_steps(3);
        assert!(harness.query_by_label("Send crash reports?").is_none());
        let app = harness.state_mut().as_mut().unwrap();
        assert!(!app.crash_question);
        assert_eq!(app.config.crash_reports, Some(false));
        let saved = Config::read(&dir.join("config.json")).unwrap();
        assert_eq!(saved.crash_reports, Some(false));

        app.answer_crash_reports(true);
        let saved = Config::read(&dir.join("config.json")).unwrap();
        assert_eq!(saved.crash_reports, Some(true));
        // Answered: a new start does not ask again.
        assert!(!Consent::resolve(None, saved.crash_reports).undecided());
    }
}
