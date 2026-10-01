//! Installing StompStation PRO firmware from TonePush (docs/design/
//! redesign-2026-10-01, screens 12 to 17), with the checks `tonepush pro
//! firmware-update` makes and in the order Sonulab's own procedure takes.
//!
//! Five steps, always in view: Back up, Update Mode, Write, Restart, Check.
//! The image is checked by `voidx_client::firmware::Image` before anything
//! else; the pedal is backed up, and the backup checked, before it is let
//! go; it is started in Update Mode by hand, and TonePush only reads who it
//! is then (never its libraries); every batch of the image must be confirmed
//! with the exact number of bytes sent; and TonePush never restarts the
//! pedal. Once it has the whole image the pedal is left on for five minutes
//! to finish writing, then switched off and on by hand, and TonePush checks
//! the version it comes back with and backs it up again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use voidx_client::firmware;

use super::*;
use crate::shell;
use crate::theme::{Icon, Mood, StepState};

/// How long the pedal is left on after it has the whole image, as the
/// command line says: it is still writing it.
pub(crate) const SETTLE: Duration = Duration::from_secs(5 * 60);
/// How long the pedal stays off before it is switched on again.
pub(crate) const OFF_WAIT: Duration = Duration::from_secs(10);
/// How often the worker looks for the pedal while it waits for it.
pub(crate) const POLL: Duration = Duration::from_millis(750);
/// How old a backup an update may stand on, as the command line allows.
const RECENT: u64 = 24 * 60 * 60;
/// Where Sonulab publishes the firmware and its help.
const SONULAB: &str = "https://sonulab.com/stompstationpro/";

/// A firmware file, once checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageFacts {
    pub(crate) file: String,
    pub(crate) version: String,
    pub(crate) sha256: String,
    /// An official Sonulab release TonePush lists by its SHA-256.
    pub(crate) official: bool,
    pub(crate) size: usize,
}

/// The backup an update stands on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackupFacts {
    pub(crate) path: PathBuf,
    pub(crate) captured: u64,
    /// The firmware it was taken on, which it restores onto.
    pub(crate) version: String,
    pub(crate) presets: usize,
    pub(crate) amps: usize,
    pub(crate) drives: usize,
    pub(crate) irs: usize,
}

/// What the worker saw while it waited for the pedal in Update Mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    /// No pedal at all: it is off, as it should be for a moment.
    Nothing,
    /// The pedal, started normally.
    Normal,
    /// More than one device that could be the pedal in Update Mode.
    Several,
    /// A device in its place that says it is something else.
    Other(String),
    /// After the update, the pedal back in Update Mode.
    UpdateModeAgain,
}

/// What the worker says about an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FirmwareEvt {
    Image(ImageFacts),
    BackedUp(BackupFacts),
    /// The pedal was let go, to be started in Update Mode.
    Waiting,
    Seen(Seen),
    /// The pedal is connected in Update Mode, and only its identity read.
    UpdateMode,
    Started {
        batch: usize,
    },
    Sent {
        bytes: usize,
        total: usize,
    },
    /// The pedal confirmed the whole image.
    Written,
    Failed {
        why: String,
        confirmed: usize,
        total: usize,
    },
    /// The pedal went away after the update: it was switched off.
    Off,
    /// It came back, started normally.
    On,
}

/// Where an update stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Checking the firmware file.
    Loading,
    BackingUp,
    /// Backed up and checked: Continue lets the pedal go.
    BackedUp,
    /// Waiting for the pedal in Update Mode.
    AwaitingUpdateMode,
    /// The pedal is in Update Mode; writing waits for the confirmation.
    Ready,
    Writing,
    /// The pedal has the whole image and is writing it: it stays on.
    Settling,
    /// Switched off; it waits ten seconds and is switched on again.
    SwitchedOff,
    /// Back, being read.
    Checking,
    Done,
    Failed,
}

/// What went wrong, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    /// The step it stopped in.
    pub(crate) during: Stage,
    pub(crate) why: String,
    /// Bytes of the image the pedal confirmed, of all of them.
    pub(crate) confirmed: usize,
    pub(crate) total: usize,
}

impl Failure {
    /// Whether the pedal had every byte: it may be writing, so it is left on
    /// before anything else.
    pub(crate) fn whole(&self) -> bool {
        self.total > 0 && self.confirmed >= self.total
    }
}

/// What the panel sends the worker next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    Nothing,
    /// Back the pedal up, in its normal mode.
    BackUp,
    /// The pedal is already in Update Mode: find a recent backup of it.
    FindBackup,
    /// Let the pedal go and wait for it in Update Mode.
    Await,
    /// Write the image of this version.
    Write(String),
    /// Back the updated pedal up.
    BackUpAgain,
}

/// One update, from choosing the file to checking the result.
#[derive(Clone, Debug)]
pub(crate) struct Flow {
    pub(crate) image: Option<ImageFacts>,
    /// The firmware it replaces, as the pedal or its backup said.
    pub(crate) from: Option<String>,
    pub(crate) backup: Option<BackupFacts>,
    pub(crate) stage: Stage,
    pub(crate) seen: Seen,
    /// Bytes confirmed, of the image's.
    pub(crate) sent: (usize, usize),
    pub(crate) write_started: Option<Instant>,
    pub(crate) written_at: Option<Instant>,
    pub(crate) off_at: Option<(Instant, SystemTime)>,
    /// Switched off before it had been left on long enough.
    pub(crate) early: bool,
    pub(crate) failure: Option<Failure>,
    /// The pedal was in Update Mode when the update began, so there was
    /// nothing to back up from it.
    pub(crate) started_in_update_mode: bool,
    /// The confirmation is open.
    pub(crate) asking: bool,
}

impl Flow {
    pub(crate) fn new(from: Option<String>, in_update_mode: bool) -> Flow {
        Flow {
            image: None,
            from,
            backup: None,
            stage: Stage::Loading,
            seen: Seen::Nothing,
            sent: (0, 0),
            write_started: None,
            written_at: None,
            off_at: None,
            early: false,
            failure: None,
            started_in_update_mode: in_update_mode,
            asking: false,
        }
    }

    fn fail(&mut self, why: String, confirmed: usize, total: usize) {
        self.failure = Some(Failure {
            during: self.stage,
            why,
            confirmed,
            total,
        });
        self.stage = Stage::Failed;
        self.asking = false;
    }

    /// What the worker said, and what to send it next.
    pub(crate) fn on(&mut self, event: &FirmwareEvt, now: Instant, clock: SystemTime) -> Next {
        match (self.stage, event) {
            (Stage::Loading, FirmwareEvt::Image(image)) => {
                self.sent = (0, image.size);
                self.image = Some(image.clone());
                if self.started_in_update_mode {
                    Next::FindBackup
                } else {
                    self.stage = Stage::BackingUp;
                    Next::BackUp
                }
            }
            (Stage::Loading | Stage::BackingUp, FirmwareEvt::BackedUp(backup)) => {
                self.from.get_or_insert_with(|| backup.version.clone());
                self.backup = Some(backup.clone());
                if self.started_in_update_mode {
                    self.stage = Stage::Ready;
                    self.asking = true;
                } else {
                    self.stage = Stage::BackedUp;
                }
                Next::Nothing
            }
            (Stage::AwaitingUpdateMode, FirmwareEvt::Waiting) => {
                self.seen = Seen::Nothing;
                Next::Nothing
            }
            (Stage::AwaitingUpdateMode, FirmwareEvt::Seen(seen)) => {
                self.seen = seen.clone();
                Next::Nothing
            }
            (Stage::AwaitingUpdateMode, FirmwareEvt::UpdateMode) => {
                self.stage = Stage::Ready;
                self.asking = true;
                Next::Nothing
            }
            (Stage::Writing, FirmwareEvt::Started { .. }) => Next::Nothing,
            (Stage::Writing, FirmwareEvt::Sent { bytes, total }) => {
                self.sent = (*bytes, *total);
                Next::Nothing
            }
            (Stage::Writing, FirmwareEvt::Written) => {
                self.sent.0 = self.sent.1;
                self.stage = Stage::Settling;
                self.written_at = Some(now);
                Next::Nothing
            }
            (Stage::Settling, FirmwareEvt::Off) => {
                self.early = self
                    .written_at
                    .is_some_and(|written| now.duration_since(written) < SETTLE);
                self.off_at = Some((now, clock));
                self.stage = Stage::SwitchedOff;
                Next::Nothing
            }
            (Stage::Settling | Stage::SwitchedOff, FirmwareEvt::Seen(seen)) => {
                self.seen = seen.clone();
                Next::Nothing
            }
            (Stage::SwitchedOff, FirmwareEvt::On) => {
                self.stage = Stage::Checking;
                Next::Nothing
            }
            (
                stage,
                FirmwareEvt::Failed {
                    why,
                    confirmed,
                    total,
                },
            ) if stage != Stage::Done && stage != Stage::Failed => {
                self.fail(why.clone(), *confirmed, *total);
                Next::Nothing
            }
            _ => Next::Nothing,
        }
    }

    /// The pedal is connected again, in its normal mode, on `version`.
    pub(crate) fn connected(&mut self, version: &str) -> Next {
        if !matches!(
            self.stage,
            Stage::Checking | Stage::SwitchedOff | Stage::Settling
        ) {
            return Next::Nothing;
        }
        let Some(image) = &self.image else {
            return Next::Nothing;
        };
        if version == image.version {
            self.stage = Stage::Done;
            Next::BackUpAgain
        } else {
            let why = format!(
                "The pedal started with firmware {version}, not {}.",
                image.version
            );
            self.stage = Stage::Checking;
            self.fail(why, self.sent.0, self.sent.1);
            Next::Nothing
        }
    }

    /// The person moves on from the backup to Update Mode.
    pub(crate) fn go_to_update_mode(&mut self) -> Next {
        if self.stage != Stage::BackedUp {
            return Next::Nothing;
        }
        self.stage = Stage::AwaitingUpdateMode;
        self.seen = Seen::Nothing;
        Next::Await
    }

    /// The person confirmed: write the image.
    pub(crate) fn confirm(&mut self, now: Instant) -> Next {
        let (Stage::Ready, Some(image)) = (self.stage, &self.image) else {
            return Next::Nothing;
        };
        if self.backup.is_none() {
            return Next::Nothing;
        }
        self.asking = false;
        self.stage = Stage::Writing;
        self.sent = (0, image.size);
        self.write_started = Some(now);
        Next::Write(image.version.clone())
    }

    /// After a failed write, or a backup that could not be found: start the
    /// pedal in Update Mode again, or back it up again.
    pub(crate) fn try_again(&mut self) -> Next {
        if self.stage != Stage::Failed {
            return Next::Nothing;
        }
        let during = self.failure.as_ref().map(|failure| failure.during);
        self.failure = None;
        match during {
            Some(Stage::Loading) | None => {
                self.stage = Stage::Loading;
                Next::Nothing
            }
            Some(Stage::BackingUp) => {
                self.stage = Stage::BackingUp;
                Next::BackUp
            }
            _ => {
                self.stage = Stage::AwaitingUpdateMode;
                self.seen = Seen::Nothing;
                Next::Await
            }
        }
    }

    /// How much longer the pedal is left on.
    pub(crate) fn settle_left(&self, now: Instant) -> Duration {
        self.written_at.map_or(SETTLE, |written| {
            SETTLE.saturating_sub(now.duration_since(written))
        })
    }

    /// How much longer the pedal stays off.
    pub(crate) fn off_left(&self, now: Instant) -> Duration {
        self.off_at.map_or(OFF_WAIT, |(off, _)| {
            OFF_WAIT.saturating_sub(now.duration_since(off))
        })
    }

    /// The five steps' states, for the stepper.
    pub(crate) fn steps(&self) -> [StepState; 5] {
        use StepState::{Done, Failed, Now, Waiting};
        let at = match self.stage {
            Stage::Loading | Stage::BackingUp => 0,
            Stage::BackedUp | Stage::AwaitingUpdateMode | Stage::Ready => 1,
            Stage::Writing => 2,
            Stage::Settling | Stage::SwitchedOff => 3,
            Stage::Checking => 4,
            Stage::Done => 5,
            Stage::Failed => match self.failure.as_ref().map(|failure| failure.during) {
                Some(Stage::Loading | Stage::BackingUp) | None => 0,
                Some(Stage::BackedUp | Stage::AwaitingUpdateMode | Stage::Ready) => 1,
                Some(Stage::Writing) => 2,
                Some(Stage::Settling | Stage::SwitchedOff) => 3,
                Some(_) => 4,
            },
        };
        let mut steps = [Waiting; 5];
        for (index, step) in steps.iter_mut().enumerate() {
            *step = if index < at {
                Done
            } else if index == at {
                if self.stage == Stage::Failed {
                    Failed
                } else if self.stage == Stage::BackedUp && index == 1 {
                    Waiting
                } else {
                    Now
                }
            } else {
                Waiting
            };
        }
        if self.stage == Stage::BackedUp {
            steps[0] = Done;
            steps[1] = Now;
        }
        steps
    }

    /// Everything about this update, for a support request.
    pub(crate) fn details(&self, device: &str) -> String {
        let mut lines = vec![format!("{device} firmware update")];
        if let Some(image) = &self.image {
            lines.push(format!(
                "Firmware file: {} ({} bytes, sha256 {}, {})",
                image.file,
                image.size,
                image.sha256,
                if image.official {
                    "an official Sonulab release"
                } else {
                    "version read from its name"
                }
            ));
            lines.push(format!("Firmware: {}", image.version));
        }
        if let Some(from) = &self.from {
            lines.push(format!("From firmware: {from}"));
        }
        if let Some(backup) = &self.backup {
            lines.push(format!(
                "Backup: {} (firmware {}, taken {})",
                backup.path.display(),
                backup.version,
                stamp(backup.captured)
            ));
        }
        lines.push(format!("Step: {:?}", self.stage));
        if self.sent.1 > 0 {
            lines.push(format!(
                "Confirmed by the pedal: {} of {} bytes",
                self.sent.0, self.sent.1
            ));
        }
        if let Some(failure) = &self.failure {
            lines.push(format!(
                "Stopped during {:?}: {} ({} of {} bytes confirmed)",
                failure.during, failure.why, failure.confirmed, failure.total
            ));
        }
        lines.join("\n")
    }
}

/// A UTC time in seconds since the epoch as a reader would write it.
fn stamp(seconds: u64) -> String {
    jiff::Timestamp::from_second(i64::try_from(seconds).unwrap_or_default())
        .map(|time| time.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

/// "5.3 MB".
fn megabytes(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// "4:12".
fn minutes(left: Duration) -> String {
    let seconds = left.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What a backup holds and when it was taken, after checking it whole.
pub(crate) fn backup_facts(path: &Path) -> WorkResult<BackupFacts> {
    let bundle = backup::open_verified(path)?;
    let manifest = bundle.manifest();
    let count = |library: Library| {
        manifest
            .lists
            .iter()
            .find(|list| list.path == library.path())
            .map_or(0, |list| {
                list.slots.iter().filter(|slot| slot.blob.is_some()).count()
            })
    };
    Ok(BackupFacts {
        path: path.to_owned(),
        captured: manifest.captured_unix_seconds,
        version: manifest.identity.version.clone(),
        presets: count(Library::Presets),
        amps: count(Library::Amps),
        drives: count(Library::Drives),
        irs: count(Library::Irs),
    })
}

/// A backup an update can stand on, as the command line requires it:
/// complete and checked, of a StompStation PRO, and taken in the last day.
fn require_recent_backup(path: &Path) -> WorkResult<BackupFacts> {
    let facts = backup_facts(path)?;
    let bundle = backup::open_verified(path)?;
    let name = &bundle.manifest().identity.name;
    if name != "StompStation PRO" {
        return Err(WorkError::Other(format!(
            "{} is a backup of a {name}",
            path.display()
        )));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WorkError::Other(error.to_string()))?
        .as_secs();
    if now.saturating_sub(facts.captured) > RECENT {
        return Err(WorkError::Other(format!(
            "{} is more than a day old; back the pedal up again before updating it",
            path.display()
        )));
    }
    Ok(facts)
}

/// What the worker waits for between commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Awaiting {
    /// The pedal, started in Update Mode.
    UpdateMode,
    /// The pedal switched off after the update (`off` once it was), then on.
    Restart { off: bool },
}

impl Worker {
    fn firmware_event(&self, event: FirmwareEvt) {
        self.send(Evt::Firmware(event));
    }

    /// Run one step of an update; a step that fails says so to the update,
    /// as well as to the deck.
    pub(super) fn firmware_step(
        &mut self,
        step: impl FnOnce(&mut Self) -> WorkResult<()>,
    ) -> WorkResult<()> {
        let result = step(self);
        if let Err(error) = &result {
            self.firmware_event(FirmwareEvt::Failed {
                why: error.to_string(),
                confirmed: 0,
                total: 0,
            });
        }
        result
    }

    /// Check a firmware file and keep it for the update.
    pub(super) fn firmware_load(&mut self, path: &Path) -> WorkResult<()> {
        let image = firmware::Image::load(path)?;
        let facts = ImageFacts {
            file: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            version: image.version.clone(),
            sha256: image.sha256.clone(),
            official: image.official,
            size: image.len(),
        };
        self.firmware_image = Some(image);
        self.firmware_event(FirmwareEvt::Image(facts));
        Ok(())
    }

    /// Back the pedal up for the update, and check the backup whole.
    pub(super) fn firmware_backup(&mut self) -> WorkResult<()> {
        let path = self.back_up_automatically()?;
        let facts = require_recent_backup(&path)?;
        self.firmware_backup = Some(path);
        self.firmware_event(FirmwareEvt::BackedUp(facts));
        Ok(())
    }

    /// The pedal is already in Update Mode, where its libraries cannot be
    /// read: the update stands on the newest checked backup of a
    /// StompStation PRO taken in the last day, if there is one.
    pub(super) fn firmware_find_backup(&mut self) -> WorkResult<()> {
        let found = backup_candidates()
            .unwrap_or_default()
            .into_iter()
            .find_map(|path| require_recent_backup(&path).ok().map(|facts| (path, facts)));
        let Some((path, facts)) = found else {
            return Err(WorkError::Other(
                "There is no backup of a StompStation PRO from the last day. Start the pedal \
                 normally, without holding UPD, so TonePush can back it up first."
                    .into(),
            ));
        };
        self.firmware_backup = Some(path);
        self.firmware_event(FirmwareEvt::BackedUp(facts));
        Ok(())
    }

    /// Let the pedal go, and wait for it in Update Mode.
    pub(super) fn firmware_await(&mut self) -> WorkResult<()> {
        if !self.update_mode {
            self.restore_audition()?;
        }
        self.device = None;
        self.update_mode = false;
        self.rollback = None;
        self.rollback_path = None;
        self.forget_history();
        self.awaiting = Some(Awaiting::UpdateMode);
        self.last_seen = None;
        self.refused_port = None;
        self.firmware_event(FirmwareEvt::Waiting);
        Ok(())
    }

    /// Send the image to the pedal in Update Mode. The backup is checked
    /// again first, and the version must be the one the person confirmed.
    pub(super) fn firmware_write(&mut self, expect: &str) -> WorkResult<()> {
        let image = self
            .firmware_image
            .clone()
            .ok_or_else(|| WorkError::Other("Choose the firmware file again".into()))?;
        if image.version != expect {
            return Err(WorkError::Other(format!(
                "The firmware file is {}, not {expect}",
                image.version
            )));
        }
        let backup = self
            .firmware_backup
            .clone()
            .ok_or_else(|| WorkError::Other("Back the pedal up before updating it".into()))?;
        require_recent_backup(&backup)?;
        if !self.update_mode {
            return Err(WorkError::Other("The pedal is not in Update Mode".into()));
        }
        let device = self
            .device
            .take()
            .ok_or_else(|| WorkError::Other("The pedal is not connected".into()))?;
        // The transfer ends this session: the pedal is restarted by hand.
        self.update_mode = false;
        let events = self.events.clone();
        let ctx = self.ctx.clone();
        let total = image.len();
        let mut confirmed = 0;
        let result = firmware::flash(device, &image, |step| {
            let event = match step {
                firmware::Step::Started { batch } => FirmwareEvt::Started { batch },
                firmware::Step::Sent { bytes, total } => {
                    confirmed = bytes;
                    FirmwareEvt::Sent { bytes, total }
                }
            };
            if events.send(Evt::Firmware(event)).is_ok() {
                ctx.request_repaint();
            }
        });
        match result {
            Ok(_link) => {
                self.awaiting = Some(Awaiting::Restart { off: false });
                self.last_seen = None;
                self.firmware_event(FirmwareEvt::Written);
            }
            Err(error) => self.firmware_event(FirmwareEvt::Failed {
                why: error.to_string(),
                confirmed,
                total,
            }),
        }
        Ok(())
    }

    /// Stop waiting, let a pedal in Update Mode go, and look for the pedal
    /// as usual.
    pub(super) fn firmware_cancel(&mut self) -> WorkResult<()> {
        self.awaiting = None;
        self.firmware_image = None;
        self.firmware_backup = None;
        if self.update_mode {
            self.device = None;
            self.update_mode = false;
        }
        self.connect()
    }

    /// A pedal found in Update Mode when TonePush looks for it: exactly one
    /// candidate, which must say it is a StompStation PRO in Update Mode.
    /// Only its identity is read.
    pub(super) fn connect_update_mode(&mut self) -> WorkResult<()> {
        let candidates = voidx_client::list_update_mode()?;
        let [candidate] = candidates.as_slice() else {
            return Err(WorkError::Other("No StompStation PRO found".into()));
        };
        let device = Device::connect(candidate.open()?)?;
        let identity = device.identity().clone();
        if identity.name != "StompStation PRO" || !firmware::is_update_mode(&identity) {
            return Err(WorkError::Other(format!(
                "{} is {} {}, not a StompStation PRO in Update Mode",
                candidate.port_name, identity.name, identity.version
            )));
        }
        self.device = Some(device);
        self.update_mode = true;
        self.rollback = None;
        self.rollback_path = None;
        self.forget_history();
        self.send(Evt::UpdateMode(identity));
        Ok(())
    }

    fn seen(&mut self, seen: Seen) {
        if self.last_seen.as_ref() != Some(&seen) {
            self.last_seen = Some(seen.clone());
            self.firmware_event(FirmwareEvt::Seen(seen));
        }
    }

    /// Look for the pedal the update is waiting for.
    pub(super) fn poll(&mut self) {
        let Some(awaiting) = self.awaiting else {
            return;
        };
        let normal = voidx_client::list().map_or(0, |found| found.len());
        let update = voidx_client::list_update_mode().unwrap_or_default();
        match awaiting {
            Awaiting::UpdateMode => match update.as_slice() {
                [candidate] => {
                    if self.refused_port.as_deref() == Some(candidate.port_name.as_str()) {
                        return;
                    }
                    let connected = candidate
                        .open()
                        .map_err(WorkError::from)
                        .and_then(|link| Ok(Device::connect(link)?));
                    // A pedal still starting does not answer yet: the next look
                    // will tell.
                    if let Ok(device) = connected {
                        let identity = device.identity().clone();
                        if identity.name == "StompStation PRO"
                            && firmware::is_update_mode(&identity)
                        {
                            self.device = Some(device);
                            self.update_mode = true;
                            self.awaiting = None;
                            self.firmware_event(FirmwareEvt::UpdateMode);
                        } else {
                            // Nothing more goes to a device that is not the
                            // pedal.
                            self.refused_port = Some(candidate.port_name.clone());
                            self.seen(Seen::Other(format!(
                                "{} {}",
                                identity.name, identity.version
                            )));
                        }
                    }
                }
                [] if normal > 0 => self.seen(Seen::Normal),
                [] => self.seen(Seen::Nothing),
                _ => self.seen(Seen::Several),
            },
            Awaiting::Restart { off: false } => {
                if update.is_empty() && normal == 0 {
                    self.awaiting = Some(Awaiting::Restart { off: true });
                    self.firmware_event(FirmwareEvt::Off);
                }
            }
            Awaiting::Restart { off: true } => {
                if normal > 0 {
                    self.awaiting = None;
                    self.firmware_event(FirmwareEvt::On);
                    if let Err(error) = self.connect() {
                        self.send(Evt::Failed(error.to_string()));
                    }
                } else if !update.is_empty() {
                    self.seen(Seen::UpdateModeAgain);
                }
            }
        }
    }
}

impl Panel {
    /// Choose a firmware file and begin.
    pub(super) fn firmware_choose(&mut self) {
        let Some(file) = rfd::FileDialog::new()
            .set_title("Choose Sonulab's firmware: the .zip, or the .upd inside it")
            .add_filter("StompStation PRO firmware", &["zip", "upd"])
            .pick_file()
        else {
            return;
        };
        let from = self
            .snapshot
            .as_ref()
            .filter(|snapshot| !firmware::is_update_mode(&snapshot.identity))
            .map(|snapshot| snapshot.identity.version.clone());
        self.firmware = Some(Flow::new(from, self.update_mode));
        let _ = self.tx.send(Cmd::FirmwareLoad(file));
    }

    /// Send what an update needs next.
    pub(super) fn firmware_next(&mut self, next: Next) {
        let command = match next {
            Next::Nothing => return,
            Next::BackUp => Cmd::FirmwareBackup,
            Next::FindBackup => Cmd::FirmwareFindBackup,
            Next::Await => Cmd::FirmwareAwait,
            Next::Write(version) => Cmd::FirmwareWrite { expect: version },
            Next::BackUpAgain => Cmd::BackupHere,
        };
        let _ = self.tx.send(command);
    }

    /// Stop an update that has not begun writing.
    fn firmware_cancel(&mut self) {
        self.firmware = None;
        let _ = self.tx.send(Cmd::FirmwareCancel);
    }

    /// The Firmware tab.
    pub(super) fn firmware_page(&mut self, ui: &mut Ui) {
        // One column, no wider than the design's, in the middle of the page.
        let room = ui.available_width();
        let width = room.min(880.0);
        ui.horizontal_top(|ui| {
            ui.add_space(((room - width) / 2.0).max(0.0));
            ui.allocate_ui_with_layout(
                Vec2::new(width, 10.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 14.0;
                    match self.firmware.clone() {
                        None => self.firmware_start(ui),
                        Some(flow) => {
                            theme::card()
                                .inner_margin(egui::Margin::symmetric(18, 14))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    let states = flow.steps();
                                    theme::stepper(
                                        ui,
                                        &[
                                            ("Back up", states[0]),
                                            ("Update Mode", states[1]),
                                            ("Write", states[2]),
                                            ("Restart", states[3]),
                                            ("Check", states[4]),
                                        ],
                                    );
                                });
                            self.firmware_stage(ui, &flow);
                        }
                    }
                },
            );
        });
    }

    /// No update under way: what the tab offers.
    fn firmware_start(&mut self, ui: &mut Ui) {
        let version = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.identity.version.clone());
        let mut choose = false;
        let (title, body) = if self.update_mode {
            (
                "The pedal is in Update Mode".to_owned(),
                "Choose Sonulab's firmware to install it. Update Mode cannot read the pedal's \
                 presets or models, so the update stands on a checked backup of it from the last \
                 day. To use the pedal as it is, switch it off and on without holding UPD."
                    .to_owned(),
            )
        } else {
            (
                format!(
                    "Firmware {}",
                    version.clone().unwrap_or_else(|| "unknown".to_owned())
                ),
                "TonePush installs Sonulab's firmware from the .zip on the StompStation PRO page, \
                 or the .upd inside it, and checks the file first. It backs the pedal up, then \
                 walks you through Sonulab's update steps and checks the result."
                    .to_owned(),
            )
        };
        let ready = (self.online || self.update_mode) && !self.busy && self.working.is_none();
        theme::banner(ui, Mood::Info, Icon::PackageCheck, &title, &body, |ui| {
            choose = theme::Button::new("Choose a firmware file…")
                .small()
                .primary()
                .icon(Icon::FolderOpen)
                .enabled(ready)
                .show(ui)
                .on_disabled_hover_text("Connect the pedal first")
                .clicked();
            if theme::Button::new("Sonulab's StompStation PRO page")
                .small()
                .ghost()
                .icon(Icon::ExternalLink)
                .show(ui)
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(SONULAB));
            }
        });
        if choose {
            self.firmware_choose();
        }
    }

    /// The card for the stage an update is at.
    fn firmware_stage(&mut self, ui: &mut Ui, flow: &Flow) {
        let now = Instant::now();
        match flow.stage {
            Stage::Loading => self.stage_card(ui, |_, ui| {
                ui.horizontal(|ui| {
                    let (spot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    shell::spin(ui, spot.center(), 5.5);
                    theme::label(
                        ui,
                        "Checking the firmware file",
                        theme::regular(theme::BODY),
                        theme::text_soft(),
                    );
                });
            }),
            Stage::BackingUp | Stage::BackedUp => self.backup_stage(ui, flow),
            Stage::AwaitingUpdateMode | Stage::Ready => self.update_mode_stage(ui, flow),
            Stage::Writing => self.writing_stage(ui, flow, now),
            Stage::Settling | Stage::SwitchedOff | Stage::Checking => {
                ui.ctx().request_repaint_after(Duration::from_millis(500));
                self.restart_stage(ui, flow, now);
            }
            Stage::Done => self.done_stage(ui, flow),
            Stage::Failed => self.failed_stage(ui, flow),
        }
        if flow.asking {
            self.firmware_confirm(ui.ctx(), flow);
        }
    }

    /// A stage's card: its body, and a footer when it has one.
    fn stage_card(&mut self, ui: &mut Ui, body: impl FnOnce(&mut Self, &mut Ui)) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 20))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    body(self, ui);
                });
        });
    }

    /// The card's footer: a note at the left, actions at the right.
    fn stage_footer(ui: &mut Ui, note: &str, actions: impl FnOnce(&mut Ui)) {
        let rect = ui.max_rect();
        ui.painter().hline(
            rect.x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, theme::line()),
        );
        egui::Frame::new()
            .fill(theme::mix(theme::panel(), theme::bg(), 0.55))
            .inner_margin(egui::Margin::symmetric(20, 12))
            .corner_radius(CornerRadius {
                nw: 0,
                ne: 0,
                sw: theme::RADIUS_CARD,
                se: theme::RADIUS_CARD,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.set_min_height(30.0);
                    theme::label(ui, note, theme::regular(12.5), theme::text_soft());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        actions(ui);
                    });
                });
            });
    }

    /// Back up (12): what happens, and the backup being taken.
    fn backup_stage(&mut self, ui: &mut Ui, flow: &Flow) {
        let image = flow.image.clone();
        let version = image
            .as_ref()
            .map_or_else(String::new, |image| image.version.clone());
        let mut proceed = false;
        let mut cancel = false;
        let working = self.working.clone();
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 20))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let width = ui.available_width();
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 24.0;
                        let left = (width - 24.0) * 0.56;
                        ui.allocate_ui_with_layout(
                            Vec2::new(left, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(left);
                                let same = flow.from.as_deref() == Some(version.as_str());
                                theme::label(
                                    ui,
                                    &if same {
                                        format!("Install firmware {version} again")
                                    } else {
                                        format!("Update to firmware {version}")
                                    },
                                    theme::semibold(19.0),
                                    theme::text(),
                                );
                                ui.add_space(4.0);
                                wrapped(
                                    ui,
                                    &format!(
                                        "{}TonePush backs up everything on the pedal first, then \
                                         walks you through Sonulab's update steps and checks the \
                                         result.",
                                        flow.from
                                            .as_ref()
                                            .map(|from| format!("From {from}. "))
                                            .unwrap_or_default()
                                    ),
                                    theme::text_soft(),
                                );
                                ui.add_space(14.0);
                                caption(ui, "What happens");
                                ui.add_space(8.0);
                                let steps = [
                                    ("Back up the pedal", "each slot read back and checked"),
                                    ("Start it in Update Mode", "you unplug it and hold UPD"),
                                    ("Write the firmware", "about a minute over USB"),
                                    ("Restart the pedal", "after five minutes left on"),
                                    ("Check", "TonePush backs it up again"),
                                ];
                                for (index, (what, aside)) in steps.into_iter().enumerate() {
                                    numbered(
                                        ui,
                                        index + 1,
                                        if index == 0 && flow.stage == Stage::BackingUp {
                                            Badge::Now
                                        } else if index == 0 {
                                            Badge::Done
                                        } else {
                                            Badge::Waiting
                                        },
                                        what,
                                        aside,
                                    );
                                }
                            },
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(width - left - 24.0, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width - left - 24.0);
                                backup_box(ui, flow, working.as_ref());
                            },
                        );
                    });
                });
            Self::stage_footer(
                ui,
                "Use the USB cable and keep VoidX Control closed.",
                |ui| {
                    proceed = theme::Button::new("Continue")
                        .primary()
                        .enabled(flow.stage == Stage::BackedUp)
                        .show(ui)
                        .on_disabled_hover_text("The backup comes first")
                        .clicked();
                    cancel = theme::Button::new("Cancel").show(ui).clicked();
                },
            );
        });
        if proceed {
            if let Some(flow) = self.firmware.as_mut() {
                let next = flow.go_to_update_mode();
                self.firmware_next(next);
            }
        }
        if cancel {
            self.firmware_cancel();
        }
    }

    /// Update Mode (13): how to start the pedal in it, and what TonePush sees.
    fn update_mode_stage(&mut self, ui: &mut Ui, flow: &Flow) {
        let found = flow.stage == Stage::Ready;
        let mut cancel = false;
        let mut ask = false;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 20))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    beside_drawing(ui, |ui, art| pedal_drawing(ui, art, true, if found { "Update Mode" } else { "SONULAB" }), |ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            theme::label(
                                ui,
                                "Start the pedal in Update Mode",
                                theme::semibold(19.0),
                                theme::text(),
                            );
                            if let Some(backup) = &flow.backup {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let (spot, _) =
                                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    theme::paint_icon(ui, Icon::ShieldCheck, spot.center(), 14.0, theme::ok());
                                    wrapped(
                                        ui,
                                        &format!(
                                            "Backup checked: {} presets, {} NAM amps, {} drives, {} IRs \
                                             and the settings",
                                            backup.presets, backup.amps, backup.drives, backup.irs
                                        ),
                                        theme::ok(),
                                    );
                                });
                            }
                            ui.add_space(8.0);
                            let off = !matches!(flow.seen, Seen::Normal) || found;
                            numbered(
                                ui,
                                1,
                                if off { Badge::Done } else { Badge::Now },
                                "Unplug the pedal's power, leave the USB cable in, and wait ten \
                                 seconds.",
                                "",
                            );
                            numbered(
                                ui,
                                2,
                                if found { Badge::Done } else { Badge::Now },
                                "Plug the power back in. Once the Sonulab logo shows, press and \
                                 hold UPD on the back for a few seconds.",
                                "",
                            );
                            numbered(
                                ui,
                                3,
                                if found { Badge::Done } else { Badge::Waiting },
                                "Let go when the screen says Update Mode.",
                                "",
                            );
                            ui.add_space(4.0);
                            wrapped(
                                ui,
                                "Hold UPD only once the logo shows. Held while the power comes on, \
                                 it starts a USB boot mode instead, with nothing on the screen: \
                                 unplug the power and start again.",
                                theme::muted(),
                            );
                            ui.add_space(10.0);
                            status_box(ui, flow, found);
                        });
                });
            Self::stage_footer(ui, "TonePush moves on as soon as it sees Update Mode.", |ui| {
                if found {
                    ask = theme::Button::new(&format!(
                        "Update to {}",
                        flow.image
                            .as_ref()
                            .map_or_else(String::new, |image| image.version.clone())
                    ))
                    .primary()
                    .show(ui)
                    .clicked();
                }
                cancel = theme::Button::new("Cancel update").show(ui).clicked();
            });
        });
        if ask {
            if let Some(flow) = self.firmware.as_mut() {
                flow.asking = true;
            }
        }
        if cancel {
            self.firmware_cancel();
        }
    }

    /// The confirmation before writing (14).
    fn firmware_confirm(&mut self, ctx: &egui::Context, flow: &Flow) {
        let (Some(image), Some(backup)) = (&flow.image, &flow.backup) else {
            return;
        };
        let device = self.device_name().to_owned();
        let device = if device.is_empty() {
            "StompStation PRO".to_owned()
        } else {
            device
        };
        let when = shell::clock(time_of(backup.captured))
            .map(|(when, _)| when)
            .unwrap_or_default();
        let rows = [
            (
                Icon::PackageCheck,
                theme::muted(),
                format!("Firmware {}", image.version),
                format!("{}, about a minute", megabytes(image.size)),
            ),
            (
                Icon::ShieldCheck,
                theme::ok(),
                format!("Backup from {when}"),
                format!("restores onto firmware {}", backup.version),
            ),
            (
                Icon::Plug,
                theme::hot(),
                "Keep the pedal on and connected".to_owned(),
                "until TonePush says so".to_owned(),
            ),
        ];
        let explanation = match &flow.from {
            Some(from) if *from == image.version => format!(
                "This writes firmware {from} again. Your presets, NAM models and IRs are in the \
                 backup if anything goes missing."
            ),
            Some(from) => format!(
                "This replaces firmware {from}. Your presets, NAM models and IRs are in the \
                 backup if anything goes missing."
            ),
            None => "Your presets, NAM models and IRs are in the backup if anything goes missing."
                .to_owned(),
        };
        let mut decided = None;
        let (_, close) = theme::dialog(ctx, "pro-firmware-confirm", 520.0, |ui| {
            theme::dialog_header(
                ui,
                &format!("Update the {device} to {}?", image.version),
                Some(&explanation),
            );
            theme::dialog_body(ui, |ui| facts_rows(ui, &rows));
            theme::dialog_footer(ui, "The pedal makes no sound while it updates.", |ui| {
                if theme::Button::new(&format!("Update to {}", image.version))
                    .primary()
                    .show(ui)
                    .clicked()
                {
                    decided = Some(true);
                }
                if theme::Button::new("Not now").show(ui).clicked() {
                    decided = Some(false);
                }
            });
        });
        if close && decided.is_none() {
            decided = Some(false);
        }
        match decided {
            Some(true) => {
                if let Some(flow) = self.firmware.as_mut() {
                    let next = flow.confirm(Instant::now());
                    self.firmware_next(next);
                }
            }
            Some(false) => {
                if let Some(flow) = self.firmware.as_mut() {
                    flow.asking = false;
                }
            }
            None => {}
        }
    }

    /// Write (15): how much the pedal has confirmed.
    fn writing_stage(&mut self, ui: &mut Ui, flow: &Flow, now: Instant) {
        let version = flow
            .image
            .as_ref()
            .map_or_else(String::new, |image| image.version.clone());
        let (confirmed, total) = flow.sent;
        let fraction = if total > 0 {
            confirmed as f32 / total as f32
        } else {
            0.0
        };
        let left = flow.write_started.and_then(|started| {
            let elapsed = now.duration_since(started).as_secs_f64();
            (confirmed > 0 && elapsed > 2.0).then(|| {
                let rate = confirmed as f64 / elapsed;
                Duration::from_secs_f64(((total - confirmed) as f64 / rate).max(0.0))
            })
        });
        let last = confirmed > 0 && confirmed + 4096 >= total;
        let mut copy = false;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(24, 20))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            theme::label(
                                ui,
                                &format!("Writing firmware {version}"),
                                theme::semibold(19.0),
                                theme::text(),
                            );
                            let mut words =
                                format!("{} of {} sent", megabytes(confirmed), megabytes(total));
                            if last {
                                words.push_str(" · the pedal is writing it");
                            } else if let Some(left) = left {
                                words.push_str(&format!(
                                    " · about {} left",
                                    if left.as_secs() >= 60 {
                                        format!("{} min", left.as_secs().div_ceil(60))
                                    } else {
                                        format!("{} s", left.as_secs().max(1))
                                    }
                                ));
                            }
                            theme::label(ui, &words, theme::regular(12.5), theme::muted());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            theme::label(
                                ui,
                                &format!("{:.0}%", fraction * 100.0),
                                theme::semibold(30.0),
                                theme::text(),
                            );
                        });
                    });
                    ui.add_space(14.0);
                    theme::progress(ui, fraction, 8.0, None);
                    ui.add_space(16.0);
                    ui.horizontal_top(|ui| {
                        let width = ui.available_width();
                        ui.allocate_ui_with_layout(
                            Vec2::new(width * 0.5, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                check_row(
                                    ui,
                                    if last { Badge::Done } else { Badge::Now },
                                    "Sending the firmware, each batch confirmed",
                                    &megabytes(confirmed),
                                );
                                check_row(
                                    ui,
                                    if last { Badge::Now } else { Badge::Waiting },
                                    "The pedal writes it into its memory",
                                    "",
                                );
                            },
                        );
                        ui.add_space(16.0);
                        notice(
                            ui,
                            Icon::PlugZap,
                            "Keep the pedal on and connected",
                            "If the transfer stops early, the pedal keeps the firmware it has: it \
                             writes nothing until it has the whole file.",
                        );
                    });
                });
            Self::stage_footer(
                ui,
                "Once started, the transfer runs to the end. Everything else in TonePush keeps \
                 working.",
                |ui| {
                    copy = theme::Button::new("Copy details")
                        .small()
                        .ghost()
                        .icon(Icon::Copy)
                        .show(ui)
                        .clicked();
                },
            );
        });
        if copy {
            ui.ctx().copy_text(flow.details(self.device_name()));
        }
    }

    /// Restart (16): leave it on, switch it off, wait, switch it on.
    fn restart_stage(&mut self, ui: &mut Ui, flow: &Flow, now: Instant) {
        let version = flow
            .image
            .as_ref()
            .map_or_else(String::new, |image| image.version.clone());
        let settle = flow.settle_left(now);
        let off_left = flow.off_left(now);
        let backup_time = flow
            .backup
            .as_ref()
            .and_then(|backup| shell::clock(time_of(backup.captured)))
            .map(|(when, _)| when)
            .unwrap_or_default();
        let from = flow.from.clone().unwrap_or_default();
        self.stage_card(ui, |_, ui| {
            beside_drawing(
                ui,
                |ui, art| {
                    pedal_drawing(
                        ui,
                        art,
                        false,
                        match flow.stage {
                            Stage::SwitchedOff => "",
                            Stage::Checking => "SONULAB",
                            _ => "Update Mode",
                        },
                    )
                },
                |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    theme::Chip::new("The pedal has the whole file")
                        .mood(Mood::Ok)
                        .icon(Icon::CircleCheck)
                        .height(22.0)
                        .show(ui);
                    ui.add_space(4.0);
                    let (title, body) = match flow.stage {
                        Stage::Settling if !settle.is_zero() => (
                            "Leave the pedal on while it writes".to_owned(),
                            format!(
                                "It is writing firmware {version} into its memory. Leave it \
                                 switched on; TonePush says when to restart it."
                            ),
                        ),
                        Stage::Settling => (
                            "Now unplug the pedal's power, wait ten seconds, and plug it back in"
                                .to_owned(),
                            "Without holding UPD this time. The new firmware starts the first \
                             time the pedal starts, which can take a little longer than usual."
                                .to_owned(),
                        ),
                        Stage::SwitchedOff if !off_left.is_zero() => (
                            "Wait ten seconds, then plug the power back in".to_owned(),
                            "Without holding UPD. The new firmware starts the first time the \
                             pedal starts, which can take a little longer than usual."
                                .to_owned(),
                        ),
                        Stage::SwitchedOff => (
                            "Now plug the power back in".to_owned(),
                            "Without holding UPD. The new firmware starts the first time the \
                             pedal starts, which can take a little longer than usual."
                                .to_owned(),
                        ),
                        _ => (
                            "Reading the pedal".to_owned(),
                            format!("Checking that it reports firmware {version}."),
                        ),
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&title)
                                .font(theme::semibold(19.0))
                                .color(theme::text()),
                        )
                        .wrap(),
                    );
                    wrapped(ui, &body, theme::text_soft());
                    ui.add_space(10.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 18.0;
                        let (ring, _) = ui.allocate_exact_size(Vec2::splat(54.0), Sense::hover());
                        let (left, whole, words) = match flow.stage {
                            Stage::Settling => (settle, SETTLE, minutes(settle)),
                            Stage::SwitchedOff => {
                                (off_left, OFF_WAIT, off_left.as_secs().to_string())
                            }
                            _ => (Duration::ZERO, OFF_WAIT, String::new()),
                        };
                        countdown(ui, ring, left, whole, &words, flow.stage == Stage::Checking);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let settled = settle.is_zero() || flow.stage != Stage::Settling;
                            check_row(
                                ui,
                                if settled { Badge::Done } else { Badge::Timer },
                                &if settled {
                                    "Left on while it wrote".to_owned()
                                } else {
                                    format!("Leave it on: {} more", minutes(settle))
                                },
                                "",
                            );
                            let off = flow.off_at.map(|(_, clock)| {
                                shell::clock(clock)
                                    .map(|(when, _)| when)
                                    .unwrap_or_default()
                            });
                            check_row(
                                ui,
                                match (flow.stage, settled) {
                                    (Stage::Settling, true) => Badge::Now,
                                    (Stage::Settling, false) => Badge::Waiting,
                                    _ => Badge::Done,
                                },
                                &match off {
                                    Some(when) => format!("Switched off at {when}"),
                                    None => "Unplug the power".to_owned(),
                                },
                                "",
                            );
                            check_row(
                                ui,
                                match flow.stage {
                                    Stage::SwitchedOff if off_left.is_zero() => Badge::Now,
                                    Stage::SwitchedOff => Badge::Timer,
                                    Stage::Checking => Badge::Done,
                                    _ => Badge::Waiting,
                                },
                                &match flow.stage {
                                    Stage::SwitchedOff if !off_left.is_zero() => {
                                        format!("Wait {} more seconds", off_left.as_secs().max(1))
                                    }
                                    _ => "Plug it back in".to_owned(),
                                },
                                "",
                            );
                        });
                    });
                    if flow.early {
                        ui.add_space(8.0);
                        notice(
                            ui,
                            Icon::TriangleAlert,
                            "It was switched off early",
                            "If it does not start normally, start it in Update Mode again and \
                             write the firmware again: holding UPD always starts the updater.",
                        );
                    }
                    if flow.seen == Seen::UpdateModeAgain {
                        ui.add_space(8.0);
                        notice(
                            ui,
                            Icon::Info,
                            "It started in Update Mode",
                            "Unplug the power again, wait ten seconds, and plug it back in \
                             without touching UPD.",
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (spot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        theme::paint_icon(ui, Icon::Info, spot.center(), 14.0, theme::muted());
                        wrapped(
                            ui,
                            &format!(
                                "TonePush reconnects by itself, checks that the pedal reports \
                                 {version}, and backs it up again: the {backup_time} backup \
                                 describes {from}, so saving waits for the new one."
                            ),
                            theme::muted(),
                        );
                    });
                },
            );
        });
    }

    /// Check: the pedal came back on the new firmware.
    fn done_stage(&mut self, ui: &mut Ui, flow: &Flow) {
        let version = flow
            .image
            .as_ref()
            .map_or_else(String::new, |image| image.version.clone());
        let protected = self.rollback.is_some();
        let mut close = false;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 20))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    theme::banner(
                        ui,
                        Mood::Ok,
                        Icon::CircleCheck,
                        &format!("Firmware {version} is running"),
                        &if protected {
                            "The pedal reported the new firmware and TonePush backed it up again."
                                .to_owned()
                        } else if self.working.is_some() {
                            "The pedal reported the new firmware. TonePush is backing it up again."
                                .to_owned()
                        } else {
                            "The pedal reported the new firmware.".to_owned()
                        },
                        |_| {},
                    );
                });
            Self::stage_footer(ui, "", |ui| {
                close = theme::Button::new("Close").show(ui).clicked();
            });
        });
        if close {
            self.firmware = None;
        }
    }

    /// Failed (17): where it stopped, that nothing here was lost, and how to
    /// finish.
    fn failed_stage(&mut self, ui: &mut Ui, flow: &Flow) {
        let Some(failure) = flow.failure.clone() else {
            return;
        };
        let version = flow
            .image
            .as_ref()
            .map_or_else(String::new, |image| image.version.clone());
        let percent = (failure.confirmed * 100)
            .checked_div(failure.total)
            .unwrap_or_default();
        let (title, steps): (String, Vec<String>) = match failure.during {
            Stage::Loading if flow.image.is_some() => (
                "There is no recent backup of the pedal".to_owned(),
                vec![
                    "Switch the pedal off and on without holding UPD, so it starts normally."
                        .to_owned(),
                    "Choose the firmware again: TonePush backs the pedal up first.".to_owned(),
                ],
            ),
            Stage::Loading => (
                "That is not a firmware file TonePush can install".to_owned(),
                vec![
                    "Choose Sonulab's .zip from the StompStation PRO page, or the .upd inside it."
                        .to_owned(),
                ],
            ),
            Stage::BackingUp => (
                "The backup did not finish".to_owned(),
                vec![
                    "Nothing was sent to the pedal.".to_owned(),
                    "Check the USB cable, then choose Try again.".to_owned(),
                ],
            ),
            Stage::Writing if failure.whole() => (
                "The pedal had the whole file, then stopped answering".to_owned(),
                vec![
                    "Leave the pedal on for five minutes: it may still be writing.".to_owned(),
                    "Then unplug its power, wait ten seconds, and plug it back in without \
                     holding UPD."
                        .to_owned(),
                    "If it does not start normally, start it in Update Mode and choose Try again."
                        .to_owned(),
                ],
            ),
            Stage::Writing => (
                format!("The update stopped at {percent}%"),
                vec![
                    "Leave the USB cable where it is.".to_owned(),
                    "Unplug the pedal's power, wait ten seconds, and plug it back in. Once the \
                     Sonulab logo shows, hold UPD until the screen says Update Mode."
                        .to_owned(),
                    "Choose Try again. TonePush still has the firmware file and your backup."
                        .to_owned(),
                ],
            ),
            Stage::Checking => (
                "The pedal came back on other firmware".to_owned(),
                vec![
                    "Start it in Update Mode and choose Try again to write the firmware again."
                        .to_owned(),
                ],
            ),
            _ => (
                "The update stopped".to_owned(),
                vec!["Start the pedal in Update Mode and choose Try again.".to_owned()],
            ),
        };
        let detail = match failure.during {
            Stage::Writing if failure.whole() => {
                "The pedal had every byte of the file, then stopped answering. Nothing on this \
                 computer was lost."
                    .to_owned()
            }
            Stage::Writing => "The pedal stopped confirming the firmware it was sent. It writes \
                               nothing until it has the whole file, so it still has the firmware \
                               it had. Nothing on this computer was lost."
                .to_owned(),
            Stage::Checking => format!("{} Nothing on this computer was lost.", failure.why),
            _ => "Nothing on this computer was lost.".to_owned(),
        };
        let saw = (failure.during != Stage::Checking).then(|| failure.why.clone());
        let retry = !matches!(failure.during, Stage::Loading);
        let mut again = false;
        let mut close = false;
        let mut copy = false;
        let mut support = false;
        let mut folder = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 20))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let width = ui.available_width();
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 24.0;
                        let left = (width - 24.0) * 0.6;
                        ui.allocate_ui_with_layout(
                            Vec2::new(left, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(left);
                                theme::banner(
                                    ui,
                                    Mood::Danger,
                                    Icon::TriangleAlert,
                                    &title,
                                    &detail,
                                    |_| {},
                                );
                                if let Some(saw) = &saw {
                                    ui.add_space(6.0);
                                    wrapped(
                                        ui,
                                        &format!("What TonePush saw: {saw}"),
                                        theme::muted(),
                                    );
                                }
                                ui.add_space(14.0);
                                caption(ui, "To finish the update");
                                ui.add_space(8.0);
                                for (index, step) in steps.iter().enumerate() {
                                    numbered(ui, index + 1, Badge::Waiting, step, "");
                                }
                                ui.add_space(8.0);
                                wrapped(
                                    ui,
                                    &format!(
                                        "If the pedal does not start in Update Mode, contact \
                                         Sonulab support and mention firmware {version}. Your \
                                         backup stays on this computer either way."
                                    ),
                                    theme::muted(),
                                );
                            },
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(width - left - 24.0, 10.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width - left - 24.0);
                                ui.spacing_mut().item_spacing.y = 12.0;
                                if let Some(image) = &flow.image {
                                    fact_card(
                                        ui,
                                        "Firmware file",
                                        &image.file,
                                        &format!(
                                            "{} · checked before writing",
                                            megabytes(image.size)
                                        ),
                                    );
                                }
                                if let Some(backup) = &flow.backup {
                                    let when = shell::when_words("taken", time_of(backup.captured));
                                    if fact_card(
                                        ui,
                                        "Your backup",
                                        &format!("StompStation PRO, {when}"),
                                        &format!("Checked · restores onto {}", backup.version),
                                    ) {
                                        folder = Some(backup.path.clone());
                                    }
                                }
                            },
                        );
                    });
                });
            Self::stage_footer(ui, "", |ui| {
                if retry {
                    again = theme::Button::new("Try again")
                        .primary()
                        .icon(Icon::RotateCw)
                        .show(ui)
                        .clicked();
                }
                close = theme::Button::new("Close").show(ui).clicked();
                support = theme::Button::new("Sonulab's PRO page")
                    .small()
                    .ghost()
                    .icon(Icon::ExternalLink)
                    .show(ui)
                    .clicked();
                copy = theme::Button::new("Copy details for support")
                    .small()
                    .ghost()
                    .icon(Icon::Copy)
                    .show(ui)
                    .clicked();
            });
        });
        if copy {
            ui.ctx().copy_text(flow.details(self.device_name()));
        }
        if support {
            ui.ctx().open_url(egui::OpenUrl::new_tab(SONULAB));
        }
        if let Some(path) = folder {
            open_folder(&path);
        }
        if again {
            if let Some(flow) = self.firmware.as_mut() {
                let next = flow.try_again();
                self.firmware_next(next);
            }
        }
        if close {
            self.firmware_cancel();
        }
    }
}

fn time_of(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Open a folder in the system's file manager.
fn open_folder(path: &Path) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(path).spawn();
}

/// The confirmation's rows, in the outcomes' frame: an icon in its ink,
/// what it is, and a quieter word on it.
fn facts_rows(ui: &mut Ui, rows: &[(Icon, Color32, String, String)]) {
    egui::Frame::new()
        .fill(theme::bg())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            for (icon, ink, strong, quiet) in rows {
                ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    ui.add_space(14.0);
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (spot, _) = ui.allocate_exact_size(Vec2::new(22.0, 20.0), Sense::hover());
                    theme::paint_icon(ui, *icon, spot.center(), 15.0, *ink);
                    theme::label(ui, strong, theme::medium(theme::BODY), theme::text());
                    theme::label(ui, quiet, theme::regular(theme::BODY), theme::muted());
                });
            }
        });
}

/// A drawing of the pedal at the left with words beside it, or the words
/// alone when there is no room for both.
fn beside_drawing(ui: &mut Ui, draw: impl FnOnce(&Ui, Rect), words: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    if width < 700.0 {
        ui.vertical(words);
        return;
    }
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 34.0;
        let (art, _) = ui.allocate_exact_size(Vec2::new(320.0, 250.0), Sense::hover());
        draw(ui, art);
        let rest = width - 320.0 - 34.0;
        ui.allocate_ui_with_layout(
            Vec2::new(rest, 10.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(rest);
                words(ui);
            },
        );
    });
}

/// A section caption, spaced capitals.
fn caption(ui: &mut Ui, text: &str) {
    let galley = ui.painter().layout_job(theme::paint::spaced(
        &text.to_uppercase(),
        theme::semibold(theme::CAPTION),
        theme::muted(),
        0.07,
    ));
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
}

/// Text that wraps.
fn wrapped(ui: &mut Ui, text: &str, colour: Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(12.5))
                .color(colour),
        )
        .wrap(),
    );
}

/// How a numbered or ticked row stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Badge {
    Done,
    Now,
    Waiting,
    Timer,
}

/// A numbered step: its badge, what it is and a quieter aside.
fn numbered(ui: &mut Ui, number: usize, badge: Badge, what: &str, aside: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (spot, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        let centre = spot.center();
        let painter = ui.painter();
        match badge {
            Badge::Done => {
                painter.circle_filled(centre, 11.0, theme::ok_soft());
                theme::paint_icon(ui, Icon::Check, centre, 12.0, theme::ok());
            }
            Badge::Now => {
                painter.circle_filled(centre, 11.0, theme::accent());
                let galley = shell::galley(
                    ui,
                    number.to_string(),
                    theme::bold(11.5),
                    theme::accent_ink(),
                );
                let size = galley.size();
                painter.galley(centre - size / 2.0, galley, Color32::PLACEHOLDER);
            }
            Badge::Waiting | Badge::Timer => {
                painter.circle_stroke(centre, 10.5, Stroke::new(1.2, theme::line_strong()));
                let galley = shell::galley(
                    ui,
                    number.to_string(),
                    theme::semibold(11.5),
                    theme::muted(),
                );
                let size = galley.size();
                painter.galley(centre - size / 2.0, galley, Color32::PLACEHOLDER);
            }
        }
        let mut job = egui::text::LayoutJob::default();
        job.append(
            what,
            0.0,
            egui::TextFormat::simple(
                if badge == Badge::Now {
                    theme::semibold(13.0)
                } else {
                    theme::regular(13.0)
                },
                if badge == Badge::Waiting {
                    theme::text_soft()
                } else {
                    theme::text()
                },
            ),
        );
        if !aside.is_empty() {
            job.append(
                aside,
                10.0,
                egui::TextFormat::simple(theme::regular(12.5), theme::muted()),
            );
        }
        job.wrap.max_width = ui.available_width();
        let galley = ui.painter().layout_job(job);
        let (rect, _) =
            ui.allocate_exact_size(galley.size().max(Vec2::new(0.0, 22.0)), Sense::hover());
        ui.painter().galley(
            Pos2::new(rect.left(), rect.top() + (22.0 - 18.0) / 2.0),
            galley,
            Color32::PLACEHOLDER,
        );
    });
    ui.add_space(6.0);
}

/// A row with a tick, a spinner, a timer or an empty ring, and a value.
fn check_row(ui: &mut Ui, badge: Badge, what: &str, value: &str) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::hover());
    let centre = Pos2::new(rect.left() + 9.0, rect.center().y);
    match badge {
        Badge::Done => theme::paint_icon(ui, Icon::Check, centre, 14.0, theme::ok()),
        Badge::Now => shell::spin(ui, centre, 5.5),
        Badge::Timer => theme::paint_icon(ui, Icon::Timer, centre, 14.0, theme::accent()),
        Badge::Waiting => {
            ui.painter()
                .circle_stroke(centre, 5.5, Stroke::new(1.2, theme::faint()));
        }
    }
    let galley = shell::galley(
        ui,
        what,
        theme::regular(13.0),
        if badge == Badge::Waiting {
            theme::faint()
        } else {
            theme::text()
        },
    );
    shell::paint_line(ui, galley, rect.left() + 26.0, rect.center().y);
    if !value.is_empty() {
        let galley = shell::galley(ui, value, theme::regular(12.5), theme::muted());
        let w = galley.size().x;
        shell::paint_line(ui, galley, rect.right() - w, rect.center().y);
    }
}

/// A boxed notice: an icon in a well, a title and a sentence.
fn notice(ui: &mut Ui, icon: Icon, title: &str, body: &str) {
    egui::Frame::new()
        .fill(theme::bg())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (well, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                ui.painter()
                    .rect_filled(well, CornerRadius::same(8), theme::hot_soft());
                theme::paint_icon(ui, icon, well.center(), 16.0, theme::hot());
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    theme::label(ui, title, theme::semibold(13.5), theme::text());
                    wrapped(ui, body, theme::text_soft());
                });
            });
        });
}

/// A card of facts: a caption, a strong line and a quiet one. A backup's
/// card offers its folder; says whether that was asked for.
fn fact_card(ui: &mut Ui, caption_text: &str, strong: &str, quiet: &str) -> bool {
    let mut show = false;
    egui::Frame::new()
        .fill(theme::bg())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            caption(ui, caption_text);
            ui.add_space(4.0);
            theme::label_truncated(ui, strong, theme::semibold(13.0), theme::text());
            theme::label_truncated(ui, quiet, theme::regular(12.0), theme::muted());
            if caption_text == "Your backup" {
                ui.add_space(6.0);
                show = theme::Button::new("Show in folder")
                    .small()
                    .ghost()
                    .icon(Icon::FolderOpen)
                    .show(ui)
                    .clicked();
            }
        });
    show
}

/// The backup's box beside what happens: its progress while it is taken,
/// then what it holds.
fn backup_box(ui: &mut Ui, flow: &Flow, working: Option<&(String, f32)>) {
    egui::Frame::new()
        .fill(theme::bg())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            let done = flow.stage == Stage::BackedUp;
            let fraction = if done {
                1.0
            } else {
                working.map_or(0.0, |(_, progress)| *progress)
            };
            ui.horizontal(|ui| {
                theme::label(
                    ui,
                    if done { "Backed up" } else { "Backing up" },
                    theme::semibold(13.5),
                    theme::text(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::label(
                        ui,
                        &format!("{:.0}%", fraction * 100.0),
                        theme::regular(12.5),
                        theme::muted(),
                    );
                });
            });
            ui.add_space(8.0);
            theme::progress(ui, fraction, 6.0, done.then_some(Mood::Ok));
            ui.add_space(10.0);
            // The libraries in the order a backup reads them.
            let reading = working.map(|(what, _)| what.as_str()).unwrap_or_default();
            let order = [
                Library::Presets,
                Library::Irs,
                Library::Amps,
                Library::Drives,
            ];
            let current = order
                .iter()
                .position(|library| reading.contains(library.path()));
            let counts = flow.backup.as_ref();
            for (index, library) in order.iter().enumerate() {
                let badge = if done || current.is_some_and(|current| index < current) {
                    Badge::Done
                } else if current == Some(index) {
                    Badge::Now
                } else {
                    Badge::Waiting
                };
                let value = counts.map_or_else(String::new, |backup| {
                    match library {
                        Library::Presets => backup.presets,
                        Library::Irs => backup.irs,
                        Library::Amps => backup.amps,
                        Library::Drives => backup.drives,
                    }
                    .to_string()
                });
                check_row(ui, badge, library.title(), &value);
            }
            check_row(
                ui,
                if done { Badge::Done } else { Badge::Waiting },
                "Settings",
                if done { "the safe ones" } else { "" },
            );
            ui.add_space(8.0);
            wrapped(
                ui,
                "Each slot is read back and checked. The bundle stays private on this computer.",
                theme::muted(),
            );
        });
}

/// What the worker sees while it waits for Update Mode.
fn status_box(ui: &mut Ui, flow: &Flow, found: bool) {
    let (icon, ink, fill, title, body): (Option<Icon>, Color32, Color32, String, String) = if found
    {
        (
            Some(Icon::CircleCheck),
            theme::ok(),
            theme::ok_soft(),
            "Found the StompStation PRO in Update Mode".to_owned(),
            "TonePush read only who it is; it writes nothing until you confirm.".to_owned(),
        )
    } else {
        match &flow.seen {
            Seen::Nothing => (
                None,
                theme::text(),
                theme::raised(),
                "Waiting for the pedal in Update Mode".to_owned(),
                "It disconnected when the power went. That is expected.".to_owned(),
            ),
            Seen::Normal => (
                Some(Icon::Info),
                theme::text(),
                theme::raised(),
                "The pedal started normally".to_owned(),
                "Unplug its power again, and hold UPD only once the logo shows.".to_owned(),
            ),
            Seen::Several => (
                Some(Icon::TriangleAlert),
                theme::hot(),
                theme::hot_soft(),
                "More than one device could be the pedal".to_owned(),
                "Unplug the other Raspberry Pi devices so TonePush knows which is the pedal."
                    .to_owned(),
            ),
            Seen::Other(what) => (
                Some(Icon::TriangleAlert),
                theme::hot(),
                theme::hot_soft(),
                "A device that is not the pedal answered".to_owned(),
                format!("It says it is {what}. TonePush sends it nothing else."),
            ),
            Seen::UpdateModeAgain => (
                None,
                theme::text(),
                theme::raised(),
                "Waiting for the pedal in Update Mode".to_owned(),
                String::new(),
            ),
        }
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (spot, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                match icon {
                    Some(icon) => theme::paint_icon(ui, icon, spot.center(), 16.0, ink),
                    None => shell::spin(ui, spot.center(), 6.0),
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    theme::label(ui, &title, theme::semibold(12.5), ink);
                    if !body.is_empty() {
                        theme::label(ui, &body, theme::regular(12.0), theme::muted());
                    }
                });
            });
        });
}

/// A ring that empties as time runs out, with what is left in it.
fn countdown(ui: &Ui, rect: Rect, left: Duration, whole: Duration, words: &str, busy: bool) {
    let centre = rect.center();
    let radius = rect.width() / 2.0 - 3.0;
    let painter = ui.painter();
    painter.circle_stroke(centre, radius, Stroke::new(3.0, theme::line_strong()));
    if busy {
        shell::spin(ui, centre, 8.0);
        return;
    }
    let fraction = if whole.is_zero() {
        0.0
    } else {
        (left.as_secs_f32() / whole.as_secs_f32()).clamp(0.0, 1.0)
    };
    if fraction > 0.0 {
        theme::paint::arc(
            painter,
            centre,
            radius,
            -std::f32::consts::FRAC_PI_2,
            fraction * std::f32::consts::TAU,
            Stroke::new(3.0, theme::accent()),
        );
    } else {
        theme::paint_icon(ui, Icon::Check, centre, 18.0, theme::ok());
        return;
    }
    let galley = shell::galley(ui, words, theme::semibold(14.0), theme::text());
    let size = galley.size();
    painter.galley(centre - size / 2.0, galley, Color32::PLACEHOLDER);
}

/// The pedal, drawn: its back with the jacks and UPD (lit when it is the
/// button to press), and its top with the screen, F1 to F4, the encoder and
/// the footswitches A, B and C.
fn pedal_drawing(ui: &Ui, rect: Rect, upd: bool, screen: &str) {
    let painter = ui.painter();
    let at = |x: f32, y: f32| Pos2::new(rect.left() + x, rect.top() + y);
    let line = Stroke::new(1.0, theme::line_strong());
    let soft = theme::raised();
    let label = |x: f32, y: f32, text: &str, colour: Color32, bold: bool| {
        let galley = shell::galley(
            ui,
            text,
            if bold {
                theme::bold(8.5)
            } else {
                theme::semibold(8.5)
            },
            colour,
        );
        let size = galley.size();
        painter.galley(
            Pos2::new(at(x, y).x - size.x / 2.0, at(x, y).y - size.y / 2.0),
            galley,
            Color32::PLACEHOLDER,
        );
    };
    // The back edge.
    painter.rect(
        Rect::from_min_size(at(10.0, 8.0), Vec2::new(300.0, 46.0)),
        CornerRadius::same(10),
        theme::panel(),
        line,
        egui::StrokeKind::Inside,
    );
    for (text, x) in [
        ("IN 1", 40.0),
        ("IN 2", 72.0),
        ("OUT 1", 110.0),
        ("OUT 2", 142.0),
        ("USB", 186.0),
        ("9V", 222.0),
    ] {
        if text == "USB" {
            painter.rect(
                Rect::from_center_size(at(x, 27.0), Vec2::new(18.0, 9.0)),
                CornerRadius::same(4),
                soft,
                line,
                egui::StrokeKind::Inside,
            );
        } else {
            painter.circle(at(x, 27.0), 7.0, soft, line);
        }
        label(x, 44.0, text, theme::muted(), false);
    }
    if upd {
        painter.circle_stroke(
            at(270.0, 27.0),
            12.0,
            Stroke::new(6.0, theme::alpha(theme::accent(), 0.35)),
        );
        painter.circle(
            at(270.0, 27.0),
            6.0,
            theme::accent(),
            Stroke::new(1.0, theme::accent()),
        );
    } else {
        painter.circle(at(270.0, 27.0), 6.0, soft, line);
    }
    label(
        270.0,
        44.0,
        "UPD",
        if upd { theme::accent() } else { theme::muted() },
        true,
    );
    // The top.
    painter.rect(
        Rect::from_min_size(at(10.0, 66.0), Vec2::new(300.0, 176.0)),
        CornerRadius::same(16),
        theme::panel(),
        line,
        egui::StrokeKind::Inside,
    );
    painter.rect(
        Rect::from_min_size(at(30.0, 84.0), Vec2::new(128.0, 62.0)),
        CornerRadius::same(6),
        theme::bg_deep(),
        line,
        egui::StrokeKind::Inside,
    );
    if !screen.is_empty() {
        let galley = shell::galley(ui, screen, theme::semibold(10.0), theme::muted());
        let size = galley.size();
        painter.galley(
            Pos2::new(
                at(94.0, 115.0).x - size.x / 2.0,
                at(94.0, 115.0).y - size.y / 2.0,
            ),
            galley,
            Color32::PLACEHOLDER,
        );
    }
    painter.circle(at(214.0, 112.0), 20.0, soft, line);
    painter.circle(at(214.0, 112.0), 13.0, theme::bg_deep(), line);
    label(214.0, 145.0, "BROWSE", theme::muted(), false);
    for (index, x) in [44.0, 74.0, 104.0, 134.0].into_iter().enumerate() {
        painter.circle(at(x, 160.0), 7.0, soft, line);
        label(x, 175.0, &format!("F{}", index + 1), theme::muted(), false);
    }
    for (index, x) in [70.0, 160.0, 250.0].into_iter().enumerate() {
        painter.circle_stroke(at(x, 212.0), 15.0, Stroke::new(2.0, theme::line_strong()));
        painter.circle(at(x, 212.0), 10.0, soft, line);
        label(x + 24.0, 213.0, &"ABC"[index..=index], theme::muted(), true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> ImageFacts {
        ImageFacts {
            file: "s_pro_2_2_6.zip".to_owned(),
            version: "2.2.6".to_owned(),
            sha256: "63d9".to_owned(),
            official: true,
            size: 5_128_192,
        }
    }

    fn backup() -> BackupFacts {
        BackupFacts {
            path: PathBuf::from("stompstation-pro-1.5.12-20261001-143100.vxbundle"),
            captured: 1_790_000_000,
            version: "1.5.12".to_owned(),
            presets: 24,
            amps: 14,
            drives: 9,
            irs: 11,
        }
    }

    /// The whole road: the file checked, the pedal backed up, let go and
    /// found in Update Mode, the image written only after the confirmation,
    /// left on, switched off and on, and found on the new firmware.
    #[test]
    fn an_update_goes_through_its_five_steps_in_order() {
        let start = Instant::now();
        let clock = SystemTime::now();
        let mut flow = Flow::new(Some("1.5.12".into()), false);
        assert_eq!(
            flow.on(&FirmwareEvt::Image(image()), start, clock),
            Next::BackUp
        );
        assert_eq!(flow.stage, Stage::BackingUp);
        assert_eq!(
            flow.go_to_update_mode(),
            Next::Nothing,
            "not before the backup"
        );
        assert_eq!(
            flow.on(&FirmwareEvt::BackedUp(backup()), start, clock),
            Next::Nothing
        );
        assert_eq!(flow.stage, Stage::BackedUp);
        assert_eq!(
            flow.confirm(start),
            Next::Nothing,
            "nothing is written before Update Mode"
        );
        assert_eq!(flow.go_to_update_mode(), Next::Await);
        flow.on(&FirmwareEvt::Seen(Seen::Normal), start, clock);
        assert_eq!(flow.seen, Seen::Normal);
        flow.on(&FirmwareEvt::UpdateMode, start, clock);
        assert_eq!(flow.stage, Stage::Ready);
        assert!(flow.asking, "the confirmation opens by itself");
        assert_eq!(flow.confirm(start), Next::Write("2.2.6".into()));
        assert_eq!(flow.stage, Stage::Writing);
        flow.on(
            &FirmwareEvt::Sent {
                bytes: 2048,
                total: 5_128_192,
            },
            start,
            clock,
        );
        assert_eq!(flow.sent, (2048, 5_128_192));
        let written = start + Duration::from_secs(60);
        flow.on(&FirmwareEvt::Written, written, clock);
        assert_eq!(flow.stage, Stage::Settling);
        assert_eq!(
            flow.settle_left(written + Duration::from_secs(60)),
            Duration::from_secs(240)
        );
        let off = written + SETTLE + Duration::from_secs(5);
        flow.on(&FirmwareEvt::Off, off, clock);
        assert_eq!(flow.stage, Stage::SwitchedOff);
        assert!(!flow.early);
        assert_eq!(
            flow.off_left(off + Duration::from_secs(3)),
            Duration::from_secs(7)
        );
        flow.on(&FirmwareEvt::On, off + Duration::from_secs(20), clock);
        assert_eq!(flow.stage, Stage::Checking);
        assert_eq!(flow.connected("2.2.6"), Next::BackUpAgain);
        assert_eq!(flow.stage, Stage::Done);
        assert!(flow.steps().iter().all(|step| *step == StepState::Done));
    }

    /// A pedal switched off before five minutes have passed is warned about,
    /// and one that comes back on other firmware fails the check.
    #[test]
    fn switching_off_early_and_the_wrong_version_are_said() {
        let start = Instant::now();
        let clock = SystemTime::now();
        let mut flow = Flow::new(Some("1.5.12".into()), false);
        flow.on(&FirmwareEvt::Image(image()), start, clock);
        flow.on(&FirmwareEvt::BackedUp(backup()), start, clock);
        flow.go_to_update_mode();
        flow.on(&FirmwareEvt::UpdateMode, start, clock);
        flow.confirm(start);
        flow.on(&FirmwareEvt::Written, start, clock);
        flow.on(&FirmwareEvt::Off, start + Duration::from_secs(30), clock);
        assert!(flow.early);
        flow.on(&FirmwareEvt::On, start + Duration::from_secs(60), clock);
        assert_eq!(flow.connected("1.5.12"), Next::Nothing);
        assert_eq!(flow.stage, Stage::Failed);
        assert_eq!(flow.failure.as_ref().unwrap().during, Stage::Checking);
    }

    /// A transfer that stops early says how far it got, keeps the file and
    /// the backup, and tries again from Update Mode; one that stops after
    /// the last byte says the pedal may be writing.
    #[test]
    fn a_failed_write_tries_again_from_update_mode() {
        let start = Instant::now();
        let clock = SystemTime::now();
        let mut flow = Flow::new(Some("1.5.12".into()), false);
        flow.on(&FirmwareEvt::Image(image()), start, clock);
        flow.on(&FirmwareEvt::BackedUp(backup()), start, clock);
        flow.go_to_update_mode();
        flow.on(&FirmwareEvt::UpdateMode, start, clock);
        flow.confirm(start);
        flow.on(
            &FirmwareEvt::Failed {
                why: "timed out waiting for upd".into(),
                confirmed: 2_461_696,
                total: 5_128_192,
            },
            start,
            clock,
        );
        assert_eq!(flow.stage, Stage::Failed);
        let failure = flow.failure.clone().unwrap();
        assert_eq!(failure.during, Stage::Writing);
        assert!(!failure.whole());
        assert_eq!(failure.confirmed * 100 / failure.total, 48);
        assert_eq!(flow.steps()[2], StepState::Failed);
        assert_eq!(flow.try_again(), Next::Await);
        assert_eq!(flow.stage, Stage::AwaitingUpdateMode);
        assert!(flow.image.is_some() && flow.backup.is_some());

        let whole = Failure {
            during: Stage::Writing,
            why: String::new(),
            confirmed: 10,
            total: 10,
        };
        assert!(whole.whole());
    }

    /// A pedal already in Update Mode cannot be backed up, so the update
    /// stands on a recent backup and goes straight to the confirmation.
    #[test]
    fn a_pedal_already_in_update_mode_uses_a_recent_backup() {
        let start = Instant::now();
        let clock = SystemTime::now();
        let mut flow = Flow::new(None, true);
        assert_eq!(
            flow.on(&FirmwareEvt::Image(image()), start, clock),
            Next::FindBackup
        );
        flow.on(&FirmwareEvt::BackedUp(backup()), start, clock);
        assert_eq!(flow.stage, Stage::Ready);
        assert_eq!(flow.from.as_deref(), Some("1.5.12"));
        assert!(flow.asking);
        assert_eq!(flow.confirm(start), Next::Write("2.2.6".into()));
    }

    #[test]
    fn details_for_support_say_where_it_stopped() {
        let mut flow = Flow::new(Some("1.5.12".into()), false);
        flow.image = Some(image());
        flow.backup = Some(backup());
        flow.stage = Stage::Writing;
        flow.fail("the pedal stopped answering".into(), 100, 200);
        let details = flow.details("StompStation PRO");
        assert!(details.contains("Firmware: 2.2.6"));
        assert!(details.contains("From firmware: 1.5.12"));
        assert!(details.contains("100 of 200 bytes confirmed"));
    }
}
