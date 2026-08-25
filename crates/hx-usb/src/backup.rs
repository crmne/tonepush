//! A complete backup of a pedal, and putting one back.
//!
//! What a backup has to hold is everything the pedal would lose if it were
//! wiped: every preset, every global setting, every impulse response, and the
//! setlists they live in. That is what is captured here, and the reason it is
//! worth building rather than leaning on HX Edit's `.hxb` is that a `.hxb`
//! stores presets as HX Edit's own symbolic JSON - a conversion, and a
//! conversion is a thing that can be wrong. These are the pedal's own bytes.
//!
//! A bundle is a **directory**, not an archive:
//!
//! ```text
//! 2026-08-09 HX Stomp.hxbundle/
//!   manifest.json          what this is, when, and from which pedal
//!   presets/000 CT-Blackend.hxpreset      byte for byte as the device holds it
//!   presets/001 CT-Day CLN.hxpreset
//!   globals.json           every setting the device answers for, id to value
//!   irs/01 Fredman.f32     48 kHz mono f32 samples, as stored
//! ```
//!
//! Being a directory is the point. A half-written archive is a lost backup,
//! whereas a half-written directory has lost only the file it was writing; the
//! presets are ordinary files a person can read, copy, or hand back one at a
//! time without this program; and an incremental backup can rewrite one preset
//! rather than the whole thing. The manifest is JSON for the same reason.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use hx_proto::msgpack::Value;
use hx_proto::Preset;

use crate::{Error, Result, Session};

/// What a bundle says about itself.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Manifest {
    /// Bundle format version, so a future reader knows what it is looking at.
    pub version: u32,
    /// The pedal this came off, e.g. "HX Stomp".
    pub device: String,
    /// Firmware it was running, so a restore onto different firmware is at
    /// least an informed decision.
    pub firmware: String,
    /// When it was taken, as seconds since the epoch. Written by the caller,
    /// because this crate does not otherwise need a clock.
    pub captured: u64,
    /// Setlist names, in order.
    pub setlists: Vec<String>,
    /// Every slot's name, in order, empty string for an empty slot. This is the
    /// index: it says what the bundle should contain before you open it.
    pub presets: Vec<String>,
    /// Impulse response slot numbers to names.
    pub irs: BTreeMap<String, String>,
    /// How many device settings were captured.
    pub globals: usize,
}

/// How far along a capture or a restore is, for a progress bar or a log line.
pub enum Step<'a> {
    Presets {
        done: usize,
        total: usize,
        name: &'a str,
    },
    Globals,
    Irs {
        done: usize,
        total: usize,
    },
    Done,
}

/// Which parts of a bundle to put back.
///
/// Restoring everything is the common case; the parts exist because HX Edit's
/// own restore dialog offers them, and because putting back only the globals
/// after fiddling with the pedal's menus is genuinely useful.
#[derive(Clone, Copy, Debug)]
pub struct Parts {
    pub presets: bool,
    pub globals: bool,
    pub irs: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Parts {
            presets: true,
            globals: true,
            irs: true,
        }
    }
}

/// Read the whole pedal into a bundle directory.
///
/// Fast, because it reads each slot where it lies rather than loading it: a
/// full HX Stomp takes a couple of seconds, and the preset the player is on
/// never changes. Nothing here writes to the device.
pub fn capture(
    session: &mut Session,
    dir: &Path,
    captured: u64,
    mut progress: impl FnMut(Step),
) -> Result<Manifest> {
    let (device, firmware) = identify(session)?;
    let setlists = session.setlists()?;
    let mut names = session.presets(0)?;

    std::fs::create_dir_all(dir.join("presets")).map_err(io("creating the bundle"))?;

    // Presets, byte for byte. An empty slot is recorded as an empty name and
    // no file, which is what tells a restore to blank it rather than skip it.
    let total = names.len();
    let mut preset_files = BTreeSet::new();
    for (index, listed_name) in names.iter_mut().enumerate() {
        let name = listed_name.clone();
        progress(Step::Presets {
            done: index,
            total,
            name: &name,
        });
        if let Some(preset) = session.read_preset_at(0, index as i64)? {
            let file = preset_file(index, &name);
            let path = dir.join("presets").join(&file);
            std::fs::write(&path, preset.encode()).map_err(io("writing a preset"))?;
            preset_files.insert(OsString::from(file));
        } else {
            // The document is the authority on whether the slot is occupied;
            // list labels on some firmware use a default name for empty slots.
            listed_name.clear();
        }
    }

    // Every setting the device answers for. Ids it does not know are simply not
    // in the file; a device that gains settings later just captures more.
    progress(Step::Globals);
    let mut globals = BTreeMap::new();
    for id in 0..GLOBAL_IDS {
        let value = match session.object(id) {
            Ok(value) => value,
            // Unsupported ids are expected in this deliberately broad sweep.
            Err(Error::Device(_)) => continue,
            // Silence or malformed protocol is not evidence that a setting is
            // absent; carrying on would bless a partial capture as complete.
            Err(error) => return Err(error),
        };
        if let Some(json) = to_json(&value) {
            globals.insert(id.to_string(), json);
        }
    }
    std::fs::write(
        dir.join("globals.json"),
        serde_json::to_vec_pretty(&globals).map_err(json_err)?,
    )
    .map_err(io("writing the settings"))?;

    // Impulse responses, samples and all - the pedal is the only place an IR
    // that was uploaded once and never kept still exists.
    // A failed list is not an empty pedal. Recording it as one would make a
    // later restore erase every IR that the incomplete backup happened not to
    // mention.
    let slots = session.irs()?;
    let mut irs = BTreeMap::new();
    if !slots.is_empty() {
        std::fs::create_dir_all(dir.join("irs")).map_err(io("creating the IR folder"))?;
    }
    for (done, (slot, _)) in slots.iter().enumerate() {
        progress(Step::Irs {
            done,
            total: slots.len(),
        });
        let (name, samples) = session.read_ir(*slot)?.ok_or_else(|| {
            Error::Protocol(format!(
                "impulse response slot {slot} disappeared during the backup"
            ))
        })?;
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in &samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let path = dir
            .join("irs")
            .join(format!("{slot:02} {}.f32", sanitise(&name)));
        std::fs::write(&path, bytes).map_err(io("writing an impulse response"))?;
        irs.insert(slot.to_string(), name);
    }

    let manifest = Manifest {
        version: 1,
        device,
        firmware,
        captured,
        setlists,
        presets: names,
        irs,
        globals: globals.len(),
    };
    remove_stale_preset_files(&dir.join("presets"), &preset_files)?;
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(json_err)?,
    )
    .map_err(io("writing the manifest"))?;
    progress(Step::Done);
    Ok(manifest)
}

/// Read a bundle's manifest, to show what it holds before putting it back.
pub fn open(dir: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(dir.join("manifest.json")).map_err(io("reading the manifest"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(json_err)?;
    if manifest.version != 1 {
        return Err(Error::Protocol(format!(
            "unsupported backup version {}",
            manifest.version
        )));
    }
    Ok(manifest)
}

/// Write a bundle back onto the pedal.
///
/// Every write here is a flash write, and flash writes have to be paced or the
/// device stacks their commits until its transfer state machine jams - which is
/// not theoretical, it once cost a whole setlist. The pacing lives in the
/// commands themselves, so a restore is a plain loop.
pub fn restore(
    dir: &Path,
    session: &mut Session,
    parts: Parts,
    mut progress: impl FnMut(Step),
) -> Result<()> {
    let manifest = open(dir)?;
    validate_device(&manifest, session.profile.name)?;
    if parts.presets && manifest.presets.len() != usize::from(session.profile.presets) {
        return Err(Error::Protocol(format!(
            "the backup has {} preset slots, but {} has {}",
            manifest.presets.len(),
            session.profile.name,
            session.profile.presets
        )));
    }

    // Preflight the complete local side before the first flash write. A full
    // restore must not get halfway through its presets and only then discover
    // that globals.json is broken or an IR file is missing.
    let presets = parts
        .presets
        .then(|| restore_presets(dir, &manifest))
        .transpose()?;
    let globals = parts.globals.then(|| restore_globals(dir)).transpose()?;
    let irs = parts.irs.then(|| restore_irs(dir, &manifest)).transpose()?;

    if let Some(presets) = presets {
        let total = presets.len();
        for (index, name, preset) in presets {
            progress(Step::Presets {
                done: index,
                total,
                name: &name,
            });
            match preset {
                Some(preset) => session.write_preset_at(0, index as i64, &name, &preset)?,
                None => session.clear_preset_at(0, index as i64)?,
            }
        }
    }

    if let Some(globals) = globals {
        progress(Step::Globals);
        for (id, want) in &globals {
            // The device refuses a value of the wrong type, so each one goes
            // back shaped like what the device currently holds.
            let current = match session.object(*id) {
                Ok(current) => current,
                // A firmware that does not have an older setting says so with
                // a complete refusal. Transport/protocol errors instead mean
                // the session is no longer safe to keep using.
                Err(Error::Device(_)) => continue,
                Err(error) => return Err(error),
            };
            if let Some(value) = from_json(want, &current) {
                match session.set_object(*id, value) {
                    Ok(()) | Err(Error::Device(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }

    if let Some(irs) = irs {
        // The manifest lists occupied slots; occupied device slots absent from
        // that list were empty in the backup and are cleared afterwards.
        let total = irs.len();
        for (done, (slot, (name, samples))) in irs.iter().enumerate() {
            progress(Step::Irs { done, total });
            session.upload_ir(*slot, name, samples)?;
        }
        for (slot, _) in session.irs()? {
            if !irs.contains_key(&slot) {
                session.clear_ir(slot)?;
            }
        }
    }

    progress(Step::Done);
    Ok(())
}

fn validate_device(manifest: &Manifest, device: &str) -> Result<()> {
    if manifest.device != device {
        return Err(Error::Protocol(format!(
            "this is a {} backup, but the connected device is {}",
            manifest.device, device
        )));
    }
    Ok(())
}

type RestorePreset = (usize, String, Option<Preset>);

fn restore_presets(dir: &Path, manifest: &Manifest) -> Result<Vec<RestorePreset>> {
    manifest
        .presets
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let path = dir.join("presets").join(preset_file(index, name));
            let preset = match std::fs::read(&path) {
                Ok(bytes) => Some(Preset::parse(&bytes).ok_or_else(|| {
                    Error::Protocol(format!("{} is not a preset document", path.display()))
                })?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && name.is_empty() => {
                    None
                }
                Err(error) => {
                    return Err(Error::Protocol(format!(
                        "reading saved preset {}: {error}",
                        path.display()
                    )))
                }
            };
            Ok((index, name.clone(), preset))
        })
        .collect()
}

type RestoreGlobals = Vec<(i64, serde_json::Value)>;

fn restore_globals(dir: &Path) -> Result<RestoreGlobals> {
    let path = dir.join("globals.json");
    let bytes = std::fs::read(&path).map_err(io("reading the settings"))?;
    let saved: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&bytes).map_err(json_err)?;
    let mut ids = BTreeSet::new();
    saved
        .into_iter()
        .map(|(text, value)| {
            let id: i64 = text.parse().map_err(|_| {
                Error::Protocol(format!("the backup has an invalid setting id {text:?}"))
            })?;
            if id < 0 || !ids.insert(id) {
                return Err(Error::Protocol(format!(
                    "the backup has an invalid or duplicate setting id {id}"
                )));
            }
            Ok((id, value))
        })
        .collect()
}

type RestoreIrs = BTreeMap<i64, (String, Vec<f32>)>;

fn restore_irs(dir: &Path, manifest: &Manifest) -> Result<RestoreIrs> {
    let mut restored = BTreeMap::new();
    for (slot, name) in &manifest.irs {
        let slot: i64 = slot
            .parse()
            .map_err(|_| Error::Protocol(format!("the backup has an invalid IR slot {slot:?}")))?;
        if slot < 0 || restored.contains_key(&slot) {
            return Err(Error::Protocol(format!(
                "the backup has an invalid or duplicate IR slot {slot}"
            )));
        }
        let path = dir
            .join("irs")
            .join(format!("{slot:02} {}.f32", sanitise(name)));
        let bytes = std::fs::read(&path).map_err(|error| {
            Error::Protocol(format!("reading saved IR {}: {error}", path.display()))
        })?;
        if !bytes.len().is_multiple_of(4) {
            return Err(Error::Protocol(format!(
                "{} ends partway through an f32 sample",
                path.display()
            )));
        }
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        if samples.is_empty() || samples.len() > 2048 || samples.iter().any(|s| !s.is_finite()) {
            return Err(Error::Protocol(format!(
                "{} does not contain 1 to 2048 finite IR samples",
                path.display()
            )));
        }
        restored.insert(slot, (name.clone(), samples));
    }
    Ok(restored)
}

/// Back up a single preset into an existing bundle, replacing what was there.
///
/// This is what makes automatic backups bearable. A full capture is seconds,
/// which is too long to do after every save; one preset is milliseconds, so a
/// bundle can be kept current as you work without ever interrupting.
pub fn capture_one(session: &mut Session, dir: &Path, index: i64) -> Result<()> {
    let mut manifest = open(dir)?;
    let names = session.presets(0)?;
    let slot = usize::try_from(index)
        .map_err(|_| Error::Protocol("preset index cannot be negative".into()))?;
    let mut name = names
        .get(slot)
        .cloned()
        .ok_or_else(|| Error::Protocol(format!("there is no preset slot {index}")))?;
    if slot >= manifest.presets.len() {
        return Err(Error::Protocol(
            "the backup manifest has fewer preset slots than the device".into(),
        ));
    }

    // Read first, so a failed device request leaves the existing backup whole.
    // Write the replacement before removing renamed duplicates for the same
    // reason: at every failure point at least one copy remains.
    let preset = session.read_preset_at(0, index)?;
    let keep = if let Some(preset) = preset {
        let file = preset_file(slot, &name);
        std::fs::write(dir.join("presets").join(&file), preset.encode())
            .map_err(io("writing a preset"))?;
        Some(OsString::from(file))
    } else {
        name.clear();
        None
    };
    remove_other_slot_files(&dir.join("presets"), slot, keep.as_deref())?;

    manifest.presets[slot] = name;
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(json_err)?,
    )
    .map_err(io("writing the manifest"))
}

fn preset_slot(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    let (slot, _) = name.split_once(' ')?;
    slot.parse().ok()
}

fn remove_stale_preset_files(dir: &Path, wanted: &BTreeSet<OsString>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(io("reading the preset backup"))? {
        let entry = entry.map_err(io("reading the preset backup"))?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "hxpreset")
            && !wanted.contains(&entry.file_name())
        {
            std::fs::remove_file(path).map_err(io("removing a stale preset backup"))?;
        }
    }
    Ok(())
}

fn remove_other_slot_files(dir: &Path, slot: usize, keep: Option<&OsStr>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(io("reading the preset backup"))? {
        let entry = entry.map_err(io("reading the preset backup"))?;
        let path = entry.path();
        let file_name = entry.file_name();
        if path
            .extension()
            .is_some_and(|extension| extension == "hxpreset")
            && preset_slot(&path) == Some(slot)
            && keep != Some(file_name.as_os_str())
        {
            std::fs::remove_file(path).map_err(io("removing a renamed preset backup"))?;
        }
    }
    Ok(())
}

/// Every preset a bundle holds, by slot number.
///
/// Read by listing rather than by building the names from the manifest: the
/// file name carries the preset's name, which changes under a rename, and the
/// slot number in front of it does not. A bundle is also the cheapest place to
/// learn what a pedal is holding without asking the pedal, which is what the
/// editor needs to say whether a preset is in the library.
pub fn slot_files(dir: &Path) -> BTreeMap<usize, PathBuf> {
    let Ok(read) = std::fs::read_dir(dir.join("presets")) else {
        return BTreeMap::new();
    };
    read.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "hxpreset"))
        .filter_map(|p| Some((preset_slot(&p)?, p)))
        .collect()
}

/// A bundle's contents, ready to be written out in some other format: what it
/// says about itself, every slot's name and the document behind it, and the
/// settings.
pub type Exportable = (Manifest, Vec<(String, Option<Vec<u8>>)>, serde_json::Value);

/// A bundle's contents, ready to be written out in some other format.
///
/// The manifest, every slot's name paired with the document bytes behind it,
/// and the settings. What it deliberately does not do is interpret any of it:
/// turning a document into HX Edit's symbolic JSON needs the model catalog, and
/// this crate talks to devices. The caller that has a catalog does that half.
pub fn for_export(dir: &Path) -> Result<Exportable> {
    let manifest = open(dir)?;
    let presets: Result<Vec<_>> = manifest
        .presets
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let path = dir.join("presets").join(preset_file(index, name));
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => {
                    if Preset::parse(&bytes).is_none() {
                        return Err(Error::Protocol(format!(
                            "{} is not a preset document",
                            path.display()
                        )));
                    }
                    Some(bytes)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && name.is_empty() => {
                    None
                }
                Err(error) => {
                    return Err(Error::Protocol(format!(
                        "reading saved preset {}: {error}",
                        path.display()
                    )))
                }
            };
            Ok((name.clone(), bytes))
        })
        .collect();
    let presets = presets?;
    let globals_path = dir.join("globals.json");
    let globals = serde_json::from_slice(
        &std::fs::read(&globals_path).map_err(io("reading the saved settings"))?,
    )
    .map_err(json_err)?;
    Ok((manifest, presets, globals))
}

/// The device, firmware, and timestamp fields an HX Edit backup needs.
///
/// These are kept in a TonePush manifest as readable text and a wider
/// timestamp. Converting them explicitly avoids silently labelling every
/// exported bundle as an HX Stomp running whichever firmware the writer was
/// compiled against.
pub fn hxb_metadata(manifest: &Manifest) -> Result<(u32, u32, u32)> {
    let profile = hx_proto::PROFILES
        .iter()
        .find(|profile| profile.name == manifest.device)
        .ok_or_else(|| {
            Error::Protocol(format!("{} is not a supported HX device", manifest.device))
        })?;
    let (major, minor) = manifest.firmware.split_once('.').ok_or_else(|| {
        Error::Protocol(format!("{} is not a firmware version", manifest.firmware))
    })?;
    if major.is_empty()
        || major.len() > 2
        || minor.is_empty()
        || minor.len() > 2
        || !major.bytes().all(|byte| byte.is_ascii_digit())
        || !minor.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::Protocol(format!(
            "{} is not a firmware version",
            manifest.firmware
        )));
    }
    let major = u8::from_str_radix(major, 16)
        .map_err(|_| Error::Protocol(format!("{} is not a firmware version", manifest.firmware)))?;
    let minor = u8::from_str_radix(minor, 16)
        .map_err(|_| Error::Protocol(format!("{} is not a firmware version", manifest.firmware)))?;
    let firmware = (u32::from(major) << 24) | (u32::from(minor) << 16);
    let captured = u32::try_from(manifest.captured).map_err(|_| {
        Error::Protocol(format!(
            "backup timestamp {} does not fit the HX Edit format",
            manifest.captured
        ))
    })?;
    Ok((profile.device_id, firmware, captured))
}

/// How many object ids to sweep when capturing settings. 147 of the first 160
/// answer on an HX Stomp; the Global EQ reaches past 200, so this covers the
/// range with room for a device that knows more.
const GLOBAL_IDS: i64 = 256;

/// `000 CT-Blackend.hxpreset` - the slot number sorts, the name is for whoever
/// opens the folder looking for one tone.
fn preset_file(index: usize, name: &str) -> String {
    format!("{index:03} {}.hxpreset", sanitise(name))
}

/// Make a preset or IR name safe to use as a filename.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().to_owned();
    if cleaned.is_empty() {
        "untitled".to_owned()
    } else {
        cleaned
    }
}

fn identify(session: &mut Session) -> Result<(String, String)> {
    let firmware = session.read_preset()?.firmware().unwrap_or_default();
    Ok((session.profile.name.to_owned(), firmware))
}

/// A device setting as JSON, so the file is readable and editable.
fn to_json(value: &Value) -> Option<serde_json::Value> {
    Some(match value {
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) | Value::WideInt(i, _) => serde_json::json!(i),
        Value::UInt(u) | Value::Wide(u, _) => serde_json::json!(u),
        Value::F32(f) => serde_json::json!(f),
        Value::F64(f) => serde_json::json!(f),
        Value::Str(s) => serde_json::Value::String(s.clone()),
        _ => return None,
    })
}

/// Turn a setting from the file back into the shape the device holds, because
/// it refuses one of the wrong type - a float where it wants a boolean is
/// error -3, not a coerced write.
fn from_json(want: &serde_json::Value, current: &Value) -> Option<Value> {
    Some(match current {
        Value::Bool(_) => Value::Bool(want.as_bool()?),
        Value::Int(_) | Value::WideInt(..) => Value::Int(want.as_i64()?),
        Value::UInt(_) | Value::Wide(..) => Value::UInt(want.as_u64()?),
        Value::F32(_) => {
            let value = want.as_f64()? as f32;
            Value::F32(value.is_finite().then_some(value)?)
        }
        Value::F64(_) => {
            let value = want.as_f64()?;
            Value::F64(value.is_finite().then_some(value)?)
        }
        Value::Str(_) => Value::Str(want.as_str()?.to_owned()),
        _ => return None,
    })
}

fn io(doing: &'static str) -> impl Fn(std::io::Error) -> Error {
    move |e| Error::Protocol(format!("{doing}: {e}"))
}

fn json_err(e: serde_json::Error) -> Error {
    Error::Protocol(format!("the bundle's JSON is not readable: {e}"))
}

/// Keep a dated copy of a bundle, and drop the oldest once there are more than
/// `keep`.
///
/// The automatic backup is one directory that every connection overwrites, so
/// there has only ever been one copy of the pedal on disk and it is always the
/// pedal as it is *now*. That is the wrong shape for the failure it exists to
/// survive: unpaced flash writes can corrupt a setlist past a power cycle, and
/// noticing takes longer than reconnecting - by which time the only copy is the
/// corrupted one.
///
/// So the current bundle is copied aside under its date before it is refreshed.
/// Snapshots are cheap: a whole pedal is a few megabytes, and `keep` of them is
/// a bounded cost rather than a directory that grows for ever.
pub fn snapshot(dir: &Path, stamp: &str, keep: usize) -> Result<Option<PathBuf>> {
    // Nothing to snapshot before the first backup has been taken.
    if !dir.join("manifest.json").exists() {
        return Ok(None);
    }
    let Some(parent) = dir.parent() else {
        return Ok(None);
    };
    let history = parent.join("history");
    std::fs::create_dir_all(&history).map_err(io("making room for a snapshot"))?;

    let name = dir.file_stem().and_then(|s| s.to_str()).unwrap_or("backup");
    let target = history.join(format!("{name} {stamp}.hxbundle"));
    // A second snapshot in the same second is the same snapshot.
    if !target.exists() {
        copy_tree(dir, &target)?;
    }
    prune(&history, keep)?;
    Ok(Some(target))
}

/// Copy a bundle directory. Bundles are one level deep - files, plus a
/// `presets` and an `irs` directory - so this does not need to recurse further
/// than that, and refusing to is what keeps it from ever walking somewhere
/// surprising.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(io("making a snapshot"))?;
    for entry in std::fs::read_dir(from)
        .map_err(io("reading the bundle"))?
        .flatten()
    {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            std::fs::create_dir_all(&target).map_err(io("making a snapshot"))?;
            for inner in std::fs::read_dir(&source)
                .map_err(io("reading the bundle"))?
                .flatten()
            {
                if inner.path().is_file() {
                    std::fs::copy(inner.path(), target.join(inner.file_name()))
                        .map_err(io("copying a snapshot"))?;
                }
            }
        } else {
            std::fs::copy(&source, &target).map_err(io("copying a snapshot"))?;
        }
    }
    Ok(())
}

/// Drop the oldest snapshots until `keep` remain.
///
/// Ordered by name, which is ordered by date: the stamp is written most
/// significant first precisely so that sorting it sorts by time, with no need
/// to trust a filesystem's idea of when something was written.
fn prune(history: &Path, keep: usize) -> Result<()> {
    let mut bundles: Vec<PathBuf> = std::fs::read_dir(history)
        .map_err(io("reading the snapshots"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.extension().is_some_and(|e| e == "hxbundle"))
        .collect();
    if bundles.len() <= keep {
        return Ok(());
    }
    bundles.sort();
    let doomed = bundles.len() - keep;
    for old in bundles.into_iter().take(doomed) {
        // A snapshot that will not delete is not worth failing a backup over:
        // the backup itself is the thing that matters.
        let _ = std::fs::remove_dir_all(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            version: 1,
            device: "HX Stomp".into(),
            firmware: "3.80".into(),
            captured: 0,
            setlists: vec!["PRESETS".into()],
            presets: Vec::new(),
            irs: BTreeMap::new(),
            globals: 0,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tonepush-snap-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bundle.hxbundle/presets")).unwrap();
        std::fs::write(dir.join("bundle.hxbundle/manifest.json"), b"{}").unwrap();
        std::fs::write(dir.join("bundle.hxbundle/presets/000 One.hxpreset"), b"one").unwrap();
        dir
    }

    #[test]
    fn a_snapshot_copies_the_whole_bundle() {
        let dir = scratch("copies");
        let bundle = dir.join("bundle.hxbundle");
        let made = snapshot(&bundle, "2026-08-10 001500", 5).unwrap().unwrap();

        assert!(made.join("manifest.json").exists());
        assert_eq!(
            std::fs::read(made.join("presets/000 One.hxpreset")).unwrap(),
            b"one",
            "the presets come with it"
        );
        // And the original is untouched.
        assert!(bundle.join("manifest.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The point of the whole thing: an older copy survives a newer one, so a
    /// corruption noticed late still has something to go back to.
    #[test]
    fn older_snapshots_survive_newer_ones_until_the_limit() {
        let dir = scratch("prune");
        let bundle = dir.join("bundle.hxbundle");
        for stamp in [
            "2026-08-01 100000",
            "2026-08-02 100000",
            "2026-08-03 100000",
        ] {
            snapshot(&bundle, stamp, 3).unwrap();
        }
        let history = dir.join("history");
        assert_eq!(std::fs::read_dir(&history).unwrap().count(), 3);

        // A fourth pushes the oldest out, and only the oldest.
        snapshot(&bundle, "2026-08-04 100000", 3).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&history)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), 3);
        assert!(!left[0].contains("2026-08-01"), "the oldest went: {left:?}");
        assert!(
            left[2].contains("2026-08-04"),
            "the newest stayed: {left:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Nothing to copy before the first backup exists, and saying so is not an
    /// error - it is the first run.
    #[test]
    fn there_is_nothing_to_snapshot_before_the_first_backup() {
        let dir = std::env::temp_dir().join(format!("tonepush-snap-{}-empty", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bundle.hxbundle")).unwrap();
        assert!(
            snapshot(&dir.join("bundle.hxbundle"), "2026-08-10 000000", 3)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn preset_files_sort_by_slot_and_keep_the_name() {
        assert_eq!(preset_file(0, "CT-Blackend"), "000 CT-Blackend.hxpreset");
        assert_eq!(preset_file(125, "FX:Solitude"), "125 FX_Solitude.hxpreset");
        // A slot with no name still gets a file name that sorts where it should.
        assert_eq!(preset_file(7, ""), "007 untitled.hxpreset");
        // Slashes and colons cannot reach the filesystem.
        assert!(!preset_file(1, "a/b:c").contains('/'));
    }

    #[test]
    fn refreshed_backups_drop_stale_and_renamed_preset_files() {
        let root = scratch("stale-presets");
        let dir = root.join("bundle.hxbundle/presets");
        std::fs::write(dir.join("000 Old.hxpreset"), b"old").unwrap();
        std::fs::write(dir.join("001 Keep.hxpreset"), b"keep").unwrap();
        let wanted = BTreeSet::from([OsString::from("001 Keep.hxpreset")]);
        remove_stale_preset_files(&dir, &wanted).unwrap();
        assert!(!dir.join("000 One.hxpreset").exists());
        assert!(!dir.join("000 Old.hxpreset").exists());
        assert!(dir.join("001 Keep.hxpreset").exists());

        std::fs::write(dir.join("001 New.hxpreset"), b"new").unwrap();
        remove_other_slot_files(&dir, 1, Some(OsStr::new("001 New.hxpreset"))).unwrap();
        assert!(!dir.join("001 Keep.hxpreset").exists());
        assert!(dir.join("001 New.hxpreset").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_validates_every_local_file_before_device_writes() {
        const PRESET: &[u8] = include_bytes!("../../hx-proto/tests/preset.bin");
        let root = scratch("restore-inputs");
        let bundle = root.join("bundle.hxbundle");
        let mut saved = manifest();
        saved.presets = vec!["Named".into(), String::new()];

        let error = restore_presets(&bundle, &saved)
            .err()
            .expect("missing named preset");
        assert!(error.to_string().contains("Named"));

        std::fs::write(bundle.join("presets/000 Named.hxpreset"), PRESET).unwrap();
        let presets = restore_presets(&bundle, &saved).unwrap();
        assert!(presets[0].2.is_some());
        assert!(
            presets[1].2.is_none(),
            "an empty slot deliberately has no file"
        );

        saved.irs.insert("1".into(), "Cab".into());
        std::fs::create_dir_all(bundle.join("irs")).unwrap();
        let ir = bundle.join("irs/01 Cab.f32");
        std::fs::write(&ir, [0, 1, 2]).unwrap();
        assert!(restore_irs(&bundle, &saved)
            .unwrap_err()
            .to_string()
            .contains("partway"));

        std::fs::write(&ir, 0.5f32.to_le_bytes()).unwrap();
        let irs = restore_irs(&bundle, &saved).unwrap();
        assert_eq!(irs[&1].1, vec![0.5]);

        std::fs::write(bundle.join("globals.json"), b"{\"not-an-id\": true}").unwrap();
        assert!(restore_globals(&bundle)
            .unwrap_err()
            .to_string()
            .contains("invalid setting id"));
        std::fs::write(bundle.join("globals.json"), b"{\"1\": true, \"01\": false}").unwrap();
        assert!(restore_globals(&bundle)
            .unwrap_err()
            .to_string()
            .contains("duplicate setting id"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_bundles_and_wrong_devices_are_rejected() {
        let root = scratch("compatibility");
        let bundle = root.join("bundle.hxbundle");
        let mut saved = manifest();
        saved.version = 2;
        std::fs::write(
            bundle.join("manifest.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        assert!(open(&bundle)
            .unwrap_err()
            .to_string()
            .contains("unsupported backup version 2"));

        saved.version = 1;
        let error = validate_device(&saved, "Helix Floor").unwrap_err();
        assert!(error.to_string().contains("HX Stomp backup"));
        assert!(error.to_string().contains("Helix Floor"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn settings_round_trip_through_json_in_the_shape_the_device_holds() {
        // A float that reads back as a float, a bool as a bool: the device
        // rejects the wrong type, so this is the part that has to be right.
        let cases = [
            (Value::Bool(true), Value::Bool(false)),
            (Value::Int(120), Value::Int(0)),
            (Value::UInt(u64::MAX), Value::UInt(0)),
            (Value::F32(113.1), Value::F32(0.0)),
        ];
        for (value, shape) in cases {
            let json = to_json(&value).expect("serialises");
            let back = from_json(&json, &shape).expect("deserialises");
            assert_eq!(format!("{back:?}"), format!("{value:?}"));
        }

        // A value of the wrong shape is refused rather than coerced.
        assert!(from_json(&serde_json::json!("text"), &Value::Bool(true)).is_none());
        assert!(from_json(&serde_json::json!(1e300), &Value::F32(0.0)).is_none());
    }

    #[test]
    fn export_uses_the_manifest_and_refuses_incomplete_bundles() {
        const PRESET: &[u8] = include_bytes!("../../hx-proto/tests/preset.bin");
        let root = scratch("export");
        let bundle = root.join("bundle.hxbundle");
        let mut saved = manifest();
        saved.device = "Helix Floor".into();
        saved.firmware = "3.71".into();
        saved.captured = 123;
        saved.presets = vec!["Named".into(), String::new()];
        std::fs::write(
            bundle.join("manifest.json"),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        std::fs::write(bundle.join("globals.json"), b"{\"1\":true}").unwrap();
        std::fs::write(bundle.join("presets/000 Named.hxpreset"), PRESET).unwrap();
        // A stale file for the same slot must not win according to filesystem
        // enumeration order.
        std::fs::write(bundle.join("presets/000 Stale.hxpreset"), b"broken").unwrap();

        let (_, presets, globals) = for_export(&bundle).unwrap();
        assert_eq!(presets[0].1.as_deref(), Some(PRESET));
        assert!(presets[1].1.is_none());
        assert_eq!(globals["1"], true);
        assert_eq!(
            hxb_metadata(&saved).unwrap(),
            (0x0021_0001, 0x0371_0000, 123)
        );

        std::fs::remove_file(bundle.join("presets/000 Named.hxpreset")).unwrap();
        assert!(for_export(&bundle)
            .unwrap_err()
            .to_string()
            .contains("000 Named"));
        saved.captured = u64::from(u32::MAX) + 1;
        assert!(hxb_metadata(&saved).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
