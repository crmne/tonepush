//! Local transport for running HX commands in the editor's USB worker.

use crate::{parse_batch, run_on, BatchStep, Cli, Cmd, OUTPUT};
use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::{fs::MetadataExt, net::UnixStream};
use std::path::{Path, PathBuf};

pub const ENV: &str = "TONEPUSH_GUI_SOCKET";

#[derive(Serialize, Deserialize)]
enum Input {
    Command(Vec<OsString>),
    Batch(String),
}

#[derive(Serialize, Deserialize)]
struct Message {
    /// The caller's working directory. As an `OsString`, like the arguments,
    /// so a directory whose name is not UTF-8 still crosses the socket.
    cwd: OsString,
    input: Input,
}

#[derive(Serialize, Deserialize)]
pub struct Reply {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<String>,
}

impl Reply {
    pub fn ok() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            error: Some(error.into()),
        }
    }

    pub fn write(&self, stream: &mut UnixStream) -> Result<()> {
        serde_json::to_writer(&mut *stream, self)?;
        stream.write_all(b"\n")?;
        Ok(())
    }
}

fn request(socket: &OsStr, input: Input) -> Result<()> {
    let path = Path::new(socket);
    private_parent(path)?;
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("connecting to TonePush GUI at {}", path.display()))?;
    serde_json::to_writer(
        &mut stream,
        &Message {
            cwd: std::env::current_dir()?.into_os_string(),
            input,
        },
    )?;
    stream.shutdown(Shutdown::Write)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut received = false;
    while reader.read_line(&mut line)? != 0 {
        received = true;
        let reply: Reply = serde_json::from_str(&line)?;
        std::io::stdout().write_all(&reply.stdout)?;
        std::io::stderr().write_all(&reply.stderr)?;
        if let Some(error) = reply.error {
            bail!("{error}");
        }
        line.clear();
    }
    if !received {
        bail!("TonePush closed without replying");
    }
    Ok(())
}

/// The directory is the access boundary, including during socket creation.
pub fn private_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|_| path.is_absolute())
        .context("use an absolute socket path")?;
    let metadata = std::fs::metadata(parent)?;
    // SAFETY: geteuid has no arguments and always returns the effective UID.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        bail!(
            "TonePush socket requires a private directory (chmod 700): {}",
            parent.display()
        );
    }
    Ok(())
}

/// Refuse a connection from any account but the one running the editor.
///
/// The private directory keeps other accounts from reaching the socket at
/// all. This asks the kernel who actually connected, so a directory that was
/// swapped or loosened after it was checked still lets nobody else in.
pub fn same_user(stream: &UnixStream) -> Result<()> {
    // SAFETY: geteuid has no arguments and always returns the effective UID.
    let own = unsafe { libc::geteuid() };
    if peer_uid(stream)? != own {
        bail!("TonePush only takes commands from the account running it");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> Result<libc::uid_t> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for writes of the sizes passed, and
    // the descriptor is open for as long as `stream` is borrowed.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error()).context("identifying the TonePush client");
    }
    Ok(cred.uid)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn peer_uid(stream: &UnixStream) -> Result<libc::uid_t> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: both pointers are valid for one write each, and the descriptor
    // is open for as long as `stream` is borrowed.
    let status = unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) };
    if status != 0 {
        return Err(std::io::Error::last_os_error()).context("identifying the TonePush client");
    }
    Ok(uid)
}

pub fn forward(socket: &OsStr, args: Vec<OsString>) -> Result<()> {
    request(socket, Input::Command(args))
}

pub fn forward_batch(socket: &OsStr, text: &str) -> Result<()> {
    request(socket, Input::Batch(text.into()))
}

pub struct Request {
    cwd: PathBuf,
    steps: Vec<BatchStep>,
    batch: bool,
}

/// What a request did to the pedal, for the editor to catch up on.
#[derive(Default)]
pub struct Changes {
    /// The edit buffer was written.
    pub buffer: bool,
    /// Another preset was loaded, or the loaded one loaded again.
    pub selected: bool,
    /// Another snapshot was recalled.
    pub snapshot: bool,
    /// The preset list may have changed in ways `saved` does not name.
    pub presets: bool,
    /// Every preset was written over, as a restore of everything does.
    pub restored: bool,
    /// Slots saved to, as `(setlist, index)`.
    pub saved: Vec<(i64, i64)>,
    /// A preset's name changed and nothing else about it.
    pub renamed: bool,
    pub irs: bool,
    pub settings: bool,
    pub dirty: Option<bool>,
    /// The pedal announced something during a pause. The announcement was
    /// read here, where the editor's own poll could not see it, so the
    /// editor has to look at the pedal again.
    pub heard: bool,
    pub lost: bool,
}

impl Request {
    pub fn read(stream: &UnixStream) -> Result<Self> {
        const LIMIT: u64 = 1024 * 1024;
        let mut data = Vec::new();
        stream.take(LIMIT + 1).read_to_end(&mut data)?;
        if data.len() as u64 > LIMIT {
            bail!("TonePush request is too large");
        }
        let Message { cwd, input } = serde_json::from_slice(&data)?;
        match input {
            Input::Command(args) => Self::command(cwd.into(), args),
            Input::Batch(text) => Self::batch(cwd.into(), &text),
        }
    }

    /// One command, as its arguments after the program's name, with relative
    /// paths in it read from `cwd`.
    pub fn command(cwd: PathBuf, args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let cmd = Cli::try_parse_from(std::iter::once(OsString::from("tonepush")).chain(args))?.cmd;
        Self::new(
            cwd,
            vec![BatchStep::Run {
                line: 1,
                text: String::new(),
                cmd: Box::new(cmd),
            }],
            false,
        )
    }

    /// A batch file's text.
    pub fn batch(cwd: PathBuf, text: &str) -> Result<Self> {
        Self::new(cwd, parse_batch(text)?, true)
    }

    fn new(cwd: PathBuf, steps: Vec<BatchStep>, batch: bool) -> Result<Self> {
        if !cwd.is_absolute() {
            bail!("invalid command working directory");
        }
        Ok(Self { cwd, steps, batch })
    }

    pub fn watch(&self) -> bool {
        matches!(self.steps.as_slice(), [BatchStep::Run { cmd, .. }] if matches!(**cmd, Cmd::Watch))
    }

    pub fn run(self, session: &mut hx_usb::Session) -> (Reply, Changes) {
        let mut changes = Changes::default();
        OUTPUT.with(|output| output.replace(Some(crate::CommandOutput::default())));
        let mut failures = 0;
        let mut commands = 0;
        let mut error = None;
        for step in self.steps {
            match step {
                BatchStep::Sleep(pause) => match rest(session, pause) {
                    Ok(heard) => changes.heard |= heard,
                    Err(err) => {
                        changes.lost = true;
                        error = Some(format!("{err:#}"));
                        break;
                    }
                },
                BatchStep::Run {
                    line,
                    text,
                    mut cmd,
                } => {
                    commands += 1;
                    resolve_paths(&mut cmd, &self.cwd);
                    let before = session.channel_stats();
                    // Asked first: afterwards the loaded preset may be the
                    // one just saved to, which says nothing of where from.
                    let (saving, result) = match save_slot(session, &cmd) {
                        Ok(slot) => (slot, run_on(session, &cmd)),
                        Err(err) => (None, Err(err)),
                    };
                    let sent = before != session.channel_stats();
                    changes.record(&cmd, result.is_ok(), sent);
                    match result {
                        Ok(()) => changes.saved.extend(saving),
                        Err(err) => {
                            failures += 1;
                            let detail = format!("{err:#}");
                            // A protocol error with nothing sent is this
                            // program refusing its own arguments.
                            changes.lost = err.downcast_ref::<hx_usb::Error>().is_some_and(|e| {
                                e.loses_session()
                                    && (sent || !matches!(e, hx_usb::Error::Protocol(_)))
                            });
                            if self.batch {
                                eprintln!("line {line}: {text}: {detail}");
                            }
                            error = Some(detail);
                            if changes.lost {
                                break;
                            }
                        }
                    }
                }
            }
        }
        if self.batch && failures != 0 && !changes.lost {
            error = Some(format!(
                "{failures} of {commands} attempted commands failed"
            ));
        }
        let output = OUTPUT.with(|output| output.take().unwrap_or_default());
        (
            Reply {
                stdout: output.stdout,
                stderr: output.stderr,
                error,
            },
            changes,
        )
    }
}

impl Changes {
    fn record(&mut self, cmd: &Cmd, succeeded: bool, sent: bool) {
        if !succeeded && !sent {
            return;
        }
        match cmd {
            Cmd::Select { .. } => {
                self.selected = true;
                if succeeded {
                    self.dirty = Some(false);
                }
            }
            // A save that failed after it was sent may have landed all the
            // same. Which slot is not known then, so the editor reads again.
            Cmd::Save { .. } => {
                if succeeded {
                    self.dirty = Some(false);
                } else {
                    self.presets = true;
                }
            }
            Cmd::Rename { .. } => self.renamed = true,
            Cmd::IrLoad { .. } | Cmd::IrClear { .. } => self.irs = true,
            Cmd::SetSetting { .. } => self.settings = true,
            Cmd::RestoreAll { .. } => {
                self.buffer = true;
                self.selected = true;
                self.presets = true;
                self.restored = true;
                self.irs = true;
                self.settings = true;
            }
            Cmd::Snapshot { .. } => self.snapshot = true,
            Cmd::Set { .. }
            | Cmd::Enable { .. }
            | Cmd::Model { .. }
            | Cmd::Clear { .. }
            | Cmd::Assign { .. }
            | Cmd::Tempo { .. }
            | Cmd::SnapshotName { .. }
            | Cmd::Route { .. }
            | Cmd::CopyBlock { .. }
            | Cmd::CopySnapshot { .. }
            | Cmd::Move { .. }
            | Cmd::Import { dry_run: false, .. }
            | Cmd::Restore { .. }
            | Cmd::AssignParam { .. }
            | Cmd::AssignCc { .. }
            | Cmd::AssignRange { .. }
            | Cmd::SwitchAssign { .. }
            | Cmd::SwitchSet { .. } => {
                self.buffer = true;
                self.dirty = Some(true);
            }
            _ => {}
        }
    }
}

/// Wait out a batch's `sleep` without letting the session go quiet.
///
/// The editor's worker is the only thing that keeps the pedal's channels
/// alive and reads what the pedal announces, and it is busy here for as
/// long as the batch runs. A pause longer than a keep-alive's interval is
/// taken in pieces, and between them the announcements are read and
/// acknowledged and a keep-alive is sent, the way `watch` paces its own. A
/// pedal left with nowhere to put its announcements stops taking writes, and
/// one whose events go unacknowledged stops sending them (PROTOCOL.md). A
/// shorter pause is the plain sleep it always was.
///
/// Returns whether the pedal announced anything, which the editor then has
/// to catch up on, since what was read here never reaches its own poll.
fn rest(session: &mut hx_usb::Session, pause: std::time::Duration) -> hx_usb::Result<bool> {
    rest_every(session, pause, KEEPALIVE_EVERY)
}

fn rest_every(
    session: &mut hx_usb::Session,
    pause: std::time::Duration,
    every: std::time::Duration,
) -> hx_usb::Result<bool> {
    let mut left = pause;
    let mut heard = false;
    while left > every {
        std::thread::sleep(every);
        left -= every;
        heard |= !session.poll_notifications()?.is_empty();
        session.keepalive()?;
    }
    std::thread::sleep(left);
    Ok(heard)
}

const KEEPALIVE_EVERY: std::time::Duration = std::time::Duration::from_millis(700);

/// The slot a `save` is about to write, or `None` for any other command.
fn save_slot(session: &mut hx_usb::Session, cmd: &Cmd) -> Result<Option<(i64, i64)>> {
    let Cmd::Save { index, setlist, .. } = cmd else {
        return Ok(None);
    };
    let (loaded_setlist, loaded, _) = session.preset_info()?;
    let index = index
        .as_deref()
        .map(|text| crate::slot(&session.profile, text))
        .transpose()?;
    Ok(Some(crate::save_target(
        (loaded_setlist, loaded),
        *setlist,
        index,
    )))
}

fn resolve_paths(cmd: &mut Cmd, cwd: &Path) {
    let absolute = |path: &mut PathBuf| {
        if path.is_relative() {
            *path = cwd.join(&*path);
        }
    };
    match cmd {
        Cmd::IrLoad { file, .. }
        | Cmd::Export { file }
        | Cmd::Backup { file }
        | Cmd::Restore { file }
        | Cmd::Import { file, .. }
        | Cmd::Inspect { file }
        | Cmd::IrInfo { file }
        | Cmd::Decode { log: file } => absolute(file),
        Cmd::BackupAll { directory, .. }
        | Cmd::BackUp { directory }
        | Cmd::RestoreAll { directory, .. } => absolute(directory),
        Cmd::BundleToPresets { file, output }
        | Cmd::ExtractBackup { file, output }
        | Cmd::ExportHlx {
            input: file,
            output,
        }
        | Cmd::ExportHxb {
            bundle: file,
            output,
        } => {
            absolute(file);
            absolute(output);
        }
        _ => {}
    }
}

pub fn event(line: String) -> Reply {
    Reply {
        stdout: format!("{line}\n").into_bytes(),
        stderr: Vec::new(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tests::pedal::{Cable, Pedal};
    use std::os::unix::ffi::OsStringExt;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn session(pedal: &Arc<Mutex<Pedal>>) -> hx_usb::Session {
        hx_usb::Session::replaying(Box::new(Cable(pedal.clone())), hx_proto::HX_STOMP).unwrap()
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Arguments keep their boundaries across the socket, and so does a
    /// working directory whose name is not UTF-8.
    #[test]
    fn the_socket_carries_arguments_and_the_working_directory_whole() {
        let cwd = OsString::from_vec(b"/client/caf\xe9".to_vec());
        let (mut client, server) = UnixStream::pair().unwrap();
        serde_json::to_writer(
            &mut client,
            &Message {
                cwd: cwd.clone(),
                input: Input::Command(args(&["ir-load", "104", "model with spaces.wav"])),
            },
        )
        .unwrap();
        client.shutdown(Shutdown::Write).unwrap();

        let request = Request::read(&server).unwrap();
        assert_eq!(request.cwd, Path::new(&cwd));
        let [BatchStep::Run { cmd, .. }] = request.steps.as_slice() else {
            panic!("expected exactly one command");
        };
        let Cmd::IrLoad { slot, file } = &**cmd else {
            panic!("expected ir-load");
        };
        assert_eq!(*slot, 104);
        assert_eq!(file, Path::new("model with spaces.wav"));
    }

    /// The editor runs in its own directory. A relative file a command names
    /// is the caller's, wherever in a request it appears.
    #[test]
    fn a_forwarded_command_reads_its_files_from_the_callers_directory() {
        let pedal = Arc::new(Mutex::new(Pedal::default()));
        let cwd = PathBuf::from("/no-such-tonepush-client");
        let single = Request::command(cwd.clone(), args(&["ir-load", "104", "cab.wav"])).unwrap();
        let (reply, _) = single.run(&mut session(&pedal));
        let error = reply.error.expect("the file does not exist");
        assert!(
            error.contains("/no-such-tonepush-client/cab.wav"),
            "{error}"
        );

        let batch = Request::batch(cwd, "ir-load 104 cab.wav\n").unwrap();
        let (reply, _) = batch.run(&mut session(&pedal));
        let stderr = text(&reply.stderr);
        assert!(
            stderr.contains("/no-such-tonepush-client/cab.wav"),
            "{stderr}"
        );

        // The commands that need no pedal can sit in a batch too.
        let resolved = |line: &str| {
            let mut cmd = Cli::try_parse_from(std::iter::once("tonepush").chain(line.split(' ')))
                .unwrap()
                .cmd;
            resolve_paths(&mut cmd, Path::new("/c"));
            cmd
        };
        assert!(matches!(
            resolved("inspect a.hlx"),
            Cmd::Inspect { file } if file == Path::new("/c/a.hlx")
        ));
        assert!(matches!(
            resolved("ir-info a.wav"),
            Cmd::IrInfo { file } if file == Path::new("/c/a.wav")
        ));
        assert!(matches!(
            resolved("export-hlx a.json /abs/b.hlx"),
            Cmd::ExportHlx { input, output }
                if input == Path::new("/c/a.json") && output == Path::new("/abs/b.hlx")
        ));
    }

    /// What a failing line of a batch said goes back to whoever ran it, not
    /// to the editor's own standard error.
    #[test]
    fn a_forwarded_batch_reports_each_failure_in_its_reply() {
        let pedal = Arc::new(Mutex::new(Pedal::default()));
        let batch = Request::batch("/c".into(), "enable 3 on\nselect nowhere\n").unwrap();
        let (reply, changes) = batch.run(&mut session(&pedal));
        let stderr = text(&reply.stderr);
        assert!(stderr.contains("line 2: select nowhere:"), "{stderr}");
        assert_eq!(
            reply.error.as_deref(),
            Some("1 of 2 attempted commands failed")
        );
        assert!(changes.buffer && !changes.lost);
    }

    /// A save names the slot it wrote, which is what the editor's backup has
    /// to copy again. A rename asks for the list and nothing else.
    #[test]
    fn a_save_names_its_slot_and_a_rename_asks_for_no_reload() {
        let pedal = Arc::new(Mutex::new(Pedal::default()));
        let saves = "save\nsave 3 --setlist 2\n";
        let (reply, changes) = Request::batch("/c".into(), saves)
            .unwrap()
            .run(&mut session(&pedal));
        assert_eq!(reply.error, None, "{}", text(&reply.stderr));
        assert_eq!(changes.saved, [(0, 0), (2, 3)]);
        assert_eq!(changes.dirty, Some(false));
        assert!(!changes.presets && !changes.buffer);

        let (reply, changes) = Request::command("/c".into(), args(&["rename", "3", "Lead"]))
            .unwrap()
            .run(&mut session(&pedal));
        assert_eq!(reply.error, None);
        assert!(changes.renamed);
        assert!(!changes.presets && !changes.selected && !changes.buffer);
        assert!(changes.saved.is_empty());
    }

    /// A long `sleep` in a batch holds the editor's worker, which is what
    /// keeps the pedal's channels alive. A short one is left as it was.
    #[test]
    fn a_long_pause_keeps_the_session_alive() {
        struct Counted(Cable, Arc<Mutex<usize>>);
        impl hx_usb::Wire for Counted {
            fn send(&mut self, bytes: &[u8]) -> hx_usb::Result<()> {
                *self.1.lock().unwrap() += 1;
                self.0.send(bytes)
            }
            fn recv(&mut self, timeout: Duration) -> hx_usb::Result<Vec<u8>> {
                self.0.recv(timeout)
            }
        }
        let sent = Arc::new(Mutex::new(0));
        let pedal = Arc::new(Mutex::new(Pedal::default()));
        let mut session = hx_usb::Session::replaying(
            Box::new(Counted(Cable(pedal.clone()), sent.clone())),
            hx_proto::HX_STOMP,
        )
        .unwrap();
        let frames = || *sent.lock().unwrap();

        let before = frames();
        rest_every(
            &mut session,
            Duration::from_millis(4),
            Duration::from_millis(5),
        )
        .unwrap();
        assert_eq!(frames(), before, "a short pause sends nothing");

        rest_every(
            &mut session,
            Duration::from_millis(11),
            Duration::from_millis(5),
        )
        .unwrap();
        let channels = hx_proto::ChannelId::ALL.len();
        assert_eq!(frames() - before, 2 * channels, "two keep-alives a channel");
    }

    /// What the pedal announces during a long pause is read and
    /// acknowledged, not left on the wire until the batch ends, and the
    /// editor is told there is something to catch up on.
    #[test]
    fn a_long_pause_reads_what_the_pedal_announces() {
        let pedal = Arc::new(Mutex::new(Pedal::default()));
        let mut session =
            hx_usb::Session::replaying(Box::new(Cable(pedal.clone())), hx_proto::HX_STOMP).unwrap();
        let rest = |session: &mut hx_usb::Session| {
            rest_every(session, Duration::from_millis(11), Duration::from_millis(5)).unwrap()
        };

        assert!(
            !rest(&mut session),
            "a quiet pedal leaves nothing to follow"
        );

        pedal
            .lock()
            .unwrap()
            .notify(49, hx_proto::msgpack::Value::Nil);
        assert!(rest(&mut session), "the announcement was read here");
        assert!(
            session.poll_notifications().unwrap().is_empty(),
            "and is not waiting on the wire"
        );
    }

    #[test]
    fn only_the_account_running_the_editor_is_served() {
        let (client, server) = UnixStream::pair().unwrap();
        same_user(&server).unwrap();
        same_user(&client).unwrap();
        // SAFETY: geteuid has no arguments and always returns the effective UID.
        assert_eq!(peer_uid(&server).unwrap(), unsafe { libc::geteuid() });
    }

    #[test]
    fn failed_save_does_not_hide_unsaved_changes() {
        let mut changes = Changes::default();
        changes.record(
            &Cli::parse_from(["tonepush", "tempo", "111"]).cmd,
            true,
            true,
        );
        let save = Cli::parse_from(["tonepush", "save"]).cmd;
        changes.record(&save, false, true);
        assert_eq!(changes.dirty, Some(true));
        changes.record(&save, true, true);
        assert_eq!(changes.dirty, Some(false));
    }
}
