//! Local transport for running HX commands in the editor's USB worker.

use crate::{BatchStep, Cli, Cmd, OUTPUT, parse_batch, run_on};
use anyhow::{Context, Result, bail};
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
    cwd: PathBuf,
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
            cwd: std::env::current_dir()?,
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

#[derive(Default)]
pub struct Changes {
    pub buffer: bool,
    pub presets: bool,
    pub irs: bool,
    pub settings: bool,
    pub switched: bool,
    pub dirty: Option<bool>,
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
        if !cwd.is_absolute() {
            bail!("invalid command working directory");
        }
        let (steps, batch) = match input {
            Input::Command(args) => {
                let cmd =
                    Cli::try_parse_from(std::iter::once(OsString::from("tonepush")).chain(args))?
                        .cmd;
                (
                    vec![BatchStep::Run {
                        line: 1,
                        text: String::new(),
                        cmd: Box::new(cmd),
                    }],
                    false,
                )
            }
            Input::Batch(text) => (parse_batch(&text)?, true),
        };
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
                BatchStep::Sleep(pause) => std::thread::sleep(pause),
                BatchStep::Run {
                    line,
                    text,
                    mut cmd,
                } => {
                    commands += 1;
                    resolve_paths(&mut cmd, &self.cwd);
                    let before = session.channel_stats();
                    let result = run_on(session, &cmd);
                    changes.record(&cmd, result.is_ok(), before != session.channel_stats());
                    if let Err(err) = result {
                        failures += 1;
                        let detail = format!("{err:#}");
                        changes.lost = err.downcast_ref::<hx_usb::Error>().is_some_and(|e| {
                            e.loses_session()
                                && !(matches!(e, hx_usb::Error::Protocol(_))
                                    && before == session.channel_stats())
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
        if self.batch && failures != 0 {
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
                self.switched = true;
                if succeeded {
                    self.dirty = Some(false);
                }
            }
            Cmd::Save { .. } => {
                self.presets = true;
                if succeeded {
                    self.dirty = Some(false);
                }
            }
            Cmd::Rename { .. } => self.presets = true,
            Cmd::IrLoad { .. } | Cmd::IrClear { .. } => self.irs = true,
            Cmd::SetSetting { .. } => self.settings = true,
            Cmd::RestoreAll { .. } => {
                self.buffer = true;
                self.presets = true;
                self.irs = true;
                self.settings = true;
            }
            Cmd::Snapshot { .. } => self.switched = true,
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
        | Cmd::Import { file, .. } => absolute(file),
        Cmd::BackupAll { directory, .. }
        | Cmd::BackUp { directory }
        | Cmd::RestoreAll { directory, .. } => absolute(directory),
        Cmd::BundleToPresets { file, output } => {
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

    #[test]
    fn socket_preserves_argument_boundaries_and_resolves_paths_at_the_client() {
        let (mut client, server) = UnixStream::pair().unwrap();
        serde_json::to_writer(
            &mut client,
            &Message {
                cwd: "/client".into(),
                input: Input::Command(
                    ["ir-load", "104", "model with spaces.wav"]
                        .into_iter()
                        .map(OsString::from)
                        .collect(),
                ),
            },
        )
        .unwrap();
        client.shutdown(Shutdown::Write).unwrap();

        let request = Request::read(&server).unwrap();
        let [BatchStep::Run { cmd, .. }] = request.steps.as_slice() else {
            panic!("expected exactly one command");
        };
        let mut cmd = *cmd.clone();
        resolve_paths(&mut cmd, &request.cwd);
        let Cmd::IrLoad { slot, file } = cmd else {
            panic!("expected ir-load");
        };
        assert_eq!(slot, 104);
        assert_eq!(file, Path::new("/client/model with spaces.wav"));
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
