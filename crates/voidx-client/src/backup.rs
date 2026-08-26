//! Durable, byte-exact StompStation PRO backup bundles.
//!
//! A bundle is a directory. Blobs are written into a private sibling first,
//! hashed, re-read at their boundary chunks, and only published once a manifest
//! describing the complete capture is durable. Restore code can only obtain a
//! [`VerifiedBundle`] after every file, size, hash, preset, and path validates.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use voidx_proto::{NodeDescription, NodePath, Preset};

use crate::{BlobList, Device, Error, Identity, Link, Result, UploadStep};

pub const FORMAT_VERSION: u32 = 1;
pub const LIBRARY_PATHS: [&str; 4] = [
    "root\\presets",
    "root\\ir_list",
    "root\\nam_amp",
    "root\\nam_drive",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Slash-separated path relative to the bundle root.
    pub path: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotRecord {
    pub index: usize,
    /// `None` means the slot was empty and must not have a blob file.
    pub name: Option<String>,
    pub blob: Option<FileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRecord {
    pub path: String,
    pub description: Option<String>,
    pub size: usize,
    pub count: usize,
    pub chunk_size: usize,
    pub group: Option<usize>,
    pub gzip: bool,
    pub movable: bool,
    pub item_type: Option<String>,
    pub slots: Vec<SlotRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub captured_unix_seconds: u64,
    pub identity: Identity,
    pub schema: FileRecord,
    pub lists: Vec<ListRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub path: String,
    pub description: NodeDescription,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub preset_index: usize,
    pub preset_name: String,
    pub node_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Schema,
    List {
        path: String,
        occupied: usize,
    },
    Blob {
        path: String,
        index: usize,
        name: String,
        chunk: usize,
        chunks: usize,
    },
    Reused {
        path: String,
        index: usize,
        name: String,
        chunks: usize,
    },
    Verifying {
        path: String,
        index: usize,
    },
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreStep {
    Preflight,
    Writing {
        path: String,
        index: usize,
        name: String,
        upload: UploadStep,
    },
    Clearing {
        path: String,
        index: usize,
    },
    Setting {
        path: String,
    },
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RestoreReport {
    pub written: usize,
    pub cleared: usize,
    pub unchanged: usize,
    pub settings_written: usize,
    pub settings_unchanged: usize,
}

/// A complete bundle held in verified memory. Restore does not trust files a
/// second time after preflight, avoiding a check/use gap.
pub struct VerifiedBundle {
    manifest: Manifest,
    schema: Vec<NodeSnapshot>,
    blobs: BTreeMap<(String, usize), Vec<u8>>,
}

/// A rollback bundle that was proven against a connected device before an
/// editing session's first persistent write. Callers must discard it if that
/// device session is lost; unlike [`restore`], [`restore_armed`] deliberately
/// permits the current state to differ because those are the edits it exists
/// to undo.
pub struct ArmedRollback {
    bundle: VerifiedBundle,
    identity: Identity,
}

impl ArmedRollback {
    pub fn manifest(&self) -> &Manifest {
        self.bundle.manifest()
    }

    pub fn references_to(&self, list_path: &str, name: &str) -> Result<Vec<Reference>> {
        self.bundle.references_to(list_path, name)
    }
}

impl VerifiedBundle {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn schema(&self) -> &[NodeSnapshot] {
        &self.schema
    }

    pub fn blob(&self, path: &str, index: usize) -> Option<&[u8]> {
        self.blobs.get(&(path.to_owned(), index)).map(Vec::as_slice)
    }

    pub fn list(&self, path: &str) -> Option<&ListRecord> {
        self.manifest.lists.iter().find(|list| list.path == path)
    }

    /// Find presets whose property-list nodes refer to a named model/IR. This
    /// lets rename UI refuse to orphan references before a flash write.
    pub fn references_to(&self, list_path: &str, name: &str) -> Result<Vec<Reference>> {
        let parameter_paths = self
            .schema
            .iter()
            .filter(|node| node.description.reference.as_deref() == Some(list_path))
            .map(|node| node.path.as_str())
            .collect::<BTreeSet<_>>();
        if parameter_paths.is_empty() {
            return Ok(Vec::new());
        }
        let Some(presets) = self.list("root\\presets") else {
            return Err(Error::Backup("bundle has no preset library".into()));
        };
        let mut references = Vec::new();
        for slot in &presets.slots {
            let Some(preset_name) = &slot.name else {
                continue;
            };
            let Some(bytes) = self.blob("root\\presets", slot.index) else {
                continue;
            };
            let preset = Preset::parse(bytes).map_err(|error| {
                Error::Backup(format!("preset {} no longer parses: {error}", slot.index))
            })?;
            for record in preset.records() {
                if parameter_paths.contains(record.subject())
                    && record
                        .value()
                        .get("value")
                        .and_then(serde_json::Value::as_str)
                        == Some(name)
                {
                    references.push(Reference {
                        preset_index: slot.index,
                        preset_name: preset_name.clone(),
                        node_path: record.subject().to_owned(),
                    });
                }
            }
        }
        Ok(references)
    }

    /// Confirm that raw slot dimensions and firmware identity match before a
    /// caller enables writes. This method performs reads only.
    pub fn preflight<L: Link>(&self, device: &mut Device<L>) -> Result<Vec<BlobList>> {
        if device.identity() != &self.manifest.identity {
            return Err(Error::Backup(format!(
                "backup identity {:?} does not exactly match connected identity {:?}",
                self.manifest.identity,
                device.identity()
            )));
        }
        let mut lists = Vec::with_capacity(self.manifest.lists.len());
        for saved in &self.manifest.lists {
            let current = device.list_info(NodePath::new(saved.path.clone())?)?;
            let compatible = current.size == saved.size
                && current.count == saved.count
                && current.chunk_size == saved.chunk_size
                && current.item_type == saved.item_type;
            if !compatible {
                return Err(Error::Backup(format!(
                    "{} dimensions changed: backup is count={}, size={}, chunk={}, type={:?}; device is count={}, size={}, chunk={}, type={:?}",
                    saved.path,
                    saved.count,
                    saved.size,
                    saved.chunk_size,
                    saved.item_type,
                    current.count,
                    current.size,
                    current.chunk_size,
                    current.item_type,
                )));
            }
            lists.push(current);
        }
        Ok(lists)
    }

    /// Prove this bundle describes the connected device's current logical
    /// state by matching every name and both boundary chunks of occupied slots.
    /// Intended as the mandatory guard before an isolated mutation.
    pub fn verify_current_state<L: Link>(&self, device: &mut Device<L>) -> Result<Vec<BlobList>> {
        let lists = self.preflight(device)?;
        for current in &lists {
            let saved = self
                .list(current.path.as_str())
                .expect("preflight matched every manifest list");
            let names = saved
                .slots
                .iter()
                .map(|slot| slot.name.clone())
                .collect::<Vec<_>>();
            if current.names != names {
                return Err(Error::Backup(format!(
                    "connected {} name table differs from rollback bundle; capture a fresh rollback",
                    current.path
                )));
            }
            verify_rollback_boundaries(device, current, self)?;
        }
        verify_settings_current(self, device)?;
        Ok(lists)
    }

    /// Consume this verified bundle after proving that it is the connected
    /// device's current state. The returned capability is suitable for a
    /// sequence of guarded edits followed by [`restore_armed`].
    pub fn arm<L: Link>(self, device: &mut Device<L>) -> Result<ArmedRollback> {
        self.verify_current_state(device)?;
        Ok(ArmedRollback {
            identity: device.identity().clone(),
            bundle: self,
        })
    }
}

/// Restore a verified bundle, using a second verified bundle as durable proof
/// of the device's starting state and rollback source.
///
/// Both bundles and every connected list are preflighted before the first
/// write. The rollback bundle's name tables must exactly match the connected
/// device. A caller must still explicitly enable writes on [`Device`].
pub fn restore<L: Link>(
    source: &VerifiedBundle,
    rollback: &VerifiedBundle,
    device: &mut Device<L>,
    mut progress: impl FnMut(RestoreStep),
) -> Result<RestoreReport> {
    progress(RestoreStep::Preflight);
    if !device.writes_enabled() {
        return Err(Error::WriteRefused(
            "restore requires an explicit enable_writes call after user confirmation".into(),
        ));
    }
    let current_lists = source.preflight(device)?;
    if source.manifest.identity != rollback.manifest.identity {
        return Err(Error::Backup(
            "source and rollback bundles describe different device identities".into(),
        ));
    }

    // Establish that the rollback bundle is genuinely the state we are about
    // to replace, rather than merely another valid backup of this model.
    rollback.verify_current_state(device)?;
    restore_preflighted(source, rollback, device, current_lists, false, progress)
}

/// Restore after one or more writes made in the same uninterrupted session in
/// which `rollback` was armed. The rollback proof is intentionally not
/// repeated: the current state is expected to contain the edits being undone.
pub fn restore_armed<L: Link>(
    source: &VerifiedBundle,
    rollback: &ArmedRollback,
    device: &mut Device<L>,
    mut progress: impl FnMut(RestoreStep),
) -> Result<RestoreReport> {
    progress(RestoreStep::Preflight);
    if !device.writes_enabled() {
        return Err(Error::WriteRefused(
            "restore requires an explicitly enabled write session".into(),
        ));
    }
    if device.identity() != &rollback.identity {
        return Err(Error::Backup(
            "armed rollback belongs to a different device session identity".into(),
        ));
    }
    let current_lists = source.preflight(device)?;
    if source.manifest.identity != rollback.bundle.manifest.identity {
        return Err(Error::Backup(
            "source and armed rollback bundles describe different device identities".into(),
        ));
    }
    rollback.bundle.preflight(device)?;
    restore_preflighted(
        source,
        &rollback.bundle,
        device,
        current_lists,
        true,
        progress,
    )
}

fn restore_preflighted<L: Link>(
    source: &VerifiedBundle,
    rollback: &VerifiedBundle,
    device: &mut Device<L>,
    current_lists: Vec<BlobList>,
    force_current: bool,
    mut progress: impl FnMut(RestoreStep),
) -> Result<RestoreReport> {
    let settings = prepare_settings_restore(source, rollback, device, force_current)?;
    let mut report = RestoreReport::default();
    for current in current_lists {
        let wanted = source
            .list(current.path.as_str())
            .expect("source preflight matched every manifest list");
        let previous = rollback
            .list(current.path.as_str())
            .expect("rollback preflight matched every manifest list");
        for index in 0..current.count {
            let target = &wanted.slots[index];
            let before = &previous.slots[index];
            let same = if force_current {
                target.name.is_none() && current.names[index].is_none()
            } else {
                target.name == before.name
                    && match (&target.blob, &before.blob) {
                        (None, None) => true,
                        (Some(_), Some(_)) => {
                            source.blob(&wanted.path, index) == rollback.blob(&previous.path, index)
                        }
                        _ => false,
                    }
            };
            if same {
                report.unchanged += 1;
                continue;
            }
            match (&target.name, source.blob(&wanted.path, index)) {
                (Some(name), Some(blob)) => {
                    device.write_blob(&current, index, name, blob, |upload| {
                        progress(RestoreStep::Writing {
                            path: current.path.to_string(),
                            index,
                            name: name.clone(),
                            upload,
                        });
                    })?;
                    report.written += 1;
                }
                (None, None) => {
                    progress(RestoreStep::Clearing {
                        path: current.path.to_string(),
                        index,
                    });
                    device.clear_slot(&current, index)?;
                    report.cleared += 1;
                }
                _ => unreachable!("VerifiedBundle enforces name/blob pairs"),
            }
        }
        let final_list = device.list_info(current.path.clone())?;
        let wanted_names = wanted
            .slots
            .iter()
            .map(|slot| slot.name.clone())
            .collect::<Vec<_>>();
        if final_list.names != wanted_names {
            return Err(Error::InvalidResponse {
                subject: current.path.to_string(),
                detail: "final name table does not match restore source".into(),
            });
        }
    }
    for setting in settings {
        if !setting.changed {
            report.settings_unchanged += 1;
            continue;
        }
        progress(RestoreStep::Setting {
            path: setting.path.to_string(),
        });
        device.write_node(
            setting.path.clone(),
            &setting.description,
            setting.value.clone(),
        )?;
        let readback = device.read_value(setting.path.clone())?;
        if !crate::values_equivalent(&setting.value, &readback) {
            return Err(Error::InvalidResponse {
                subject: setting.path.to_string(),
                detail: format!(
                    "setting readback was {readback}, expected {}",
                    setting.value
                ),
            });
        }
        report.settings_written += 1;
    }
    progress(RestoreStep::Done);
    Ok(report)
}

struct SettingRestore {
    path: NodePath,
    description: NodeDescription,
    value: serde_json::Value,
    changed: bool,
}

fn restorable_settings(bundle: &VerifiedBundle) -> BTreeMap<&str, &serde_json::Value> {
    bundle
        .schema
        .iter()
        .filter(|node| node.path.starts_with("root\\settings\\"))
        .filter_map(|node| {
            let value = node
                .description
                .value
                .as_ref()
                .filter(|value| !value.is_null())?;
            node.description
                .validate_value(value)
                .is_ok()
                .then_some((node.path.as_str(), value))
        })
        .collect()
}

fn verify_settings_current<L: Link>(
    rollback: &VerifiedBundle,
    device: &mut Device<L>,
) -> Result<()> {
    let expected = restorable_settings(rollback);
    if expected.is_empty() {
        return Ok(());
    }
    let live = device.browse(NodePath::new("root\\settings")?)?;
    for (path, value) in expected {
        let path = NodePath::new(path)?;
        let actual = live
            .get(&path)
            .and_then(|description| description.value.as_ref())
            .ok_or_else(|| {
                Error::Backup(format!(
                    "connected setting {path} is missing; rollback cannot prove current settings"
                ))
            })?;
        if !crate::values_equivalent(value, actual) {
            return Err(Error::Backup(format!(
                "connected setting {path} differs from rollback bundle; capture a fresh rollback"
            )));
        }
    }
    Ok(())
}

fn prepare_settings_restore<L: Link>(
    source: &VerifiedBundle,
    rollback: &VerifiedBundle,
    device: &mut Device<L>,
    force_current: bool,
) -> Result<Vec<SettingRestore>> {
    let wanted = restorable_settings(source);
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let previous = restorable_settings(rollback);
    let live = device.browse(NodePath::new("root\\settings")?)?;
    let mut plan = Vec::with_capacity(wanted.len());
    for (path, value) in wanted {
        let before = previous.get(path).ok_or_else(|| {
            Error::Backup(format!(
                "rollback bundle has no safe value for source setting {path}"
            ))
        })?;
        let path = NodePath::new(path)?;
        let description = live.get(&path).cloned().ok_or_else(|| {
            Error::Backup(format!("connected device has no source setting {path}"))
        })?;
        description.validate_value(value).map_err(|error| {
            Error::Backup(format!(
                "source value for {path} is incompatible with the live schema: {error}"
            ))
        })?;
        plan.push(SettingRestore {
            path,
            changed: if force_current {
                description.value.as_ref() != Some(value)
            } else {
                value != *before
            },
            description,
            value: value.clone(),
        });
    }
    Ok(plan)
}

fn verify_rollback_boundaries<L: Link>(
    device: &mut Device<L>,
    current: &BlobList,
    rollback: &VerifiedBundle,
) -> Result<()> {
    for (index, name) in current.occupied() {
        let saved = rollback.blob(current.path.as_str(), index).ok_or_else(|| {
            Error::Backup(format!(
                "rollback bundle has no bytes for {} slot {index} ({name:?})",
                current.path
            ))
        })?;
        if !boundary_chunks_match(device, current, index, saved)? {
            return Err(Error::Backup(format!(
                "{} slot {index} no longer matches rollback at its boundary chunks",
                current.path
            )));
        }
    }
    Ok(())
}

/// Read every occupied slot without changing the active preset or any device
/// value, then atomically publish a complete directory bundle.
pub fn capture<L: Link>(
    device: &mut Device<L>,
    target: &Path,
    captured_unix_seconds: u64,
    progress: impl FnMut(Step),
) -> Result<Manifest> {
    capture_reusing(device, target, captured_unix_seconds, None, progress)
}

/// Capture a complete bundle while reusing bytes from a verified bundle when
/// its identity, list shape, slot name and boundary chunks still match.
/// A fresh schema is always captured, so a settings-only change avoids
/// retransferring every megabyte-sized NAM model.
pub fn capture_reusing<L: Link>(
    device: &mut Device<L>,
    target: &Path,
    captured_unix_seconds: u64,
    reuse: Option<&VerifiedBundle>,
    mut progress: impl FnMut(Step),
) -> Result<Manifest> {
    recover_bundle(target)?;
    let staging = StagingBundle::new(target)?;
    let reuse = reuse.filter(|bundle| bundle.manifest.identity == *device.identity());

    progress(Step::Schema);
    let root = device.browse(NodePath::new("root")?)?;
    let schema = root
        .nodes()
        .iter()
        .map(|(path, description)| NodeSnapshot {
            path: path.to_string(),
            description: description.clone(),
        })
        .collect::<Vec<_>>();
    let schema_bytes = pretty_json(&schema)?;
    let schema_record = write_record(&staging.path, "schema.json", &schema_bytes)?;

    let mut lists = Vec::with_capacity(LIBRARY_PATHS.len());
    for path in LIBRARY_PATHS {
        let info = device.list_info(NodePath::new(path)?)?;
        progress(Step::List {
            path: path.to_owned(),
            occupied: info.occupied().count(),
        });
        let folder = list_folder(&info.path);
        private_dir(&staging.path.join(&folder))?;
        let mut slots = Vec::with_capacity(info.count);
        for index in 0..info.count {
            let Some(name) = info.names[index].clone() else {
                slots.push(SlotRecord {
                    index,
                    name: None,
                    blob: None,
                });
                continue;
            };
            let reusable = reuse
                .and_then(|bundle| bundle.list(path))
                .filter(|saved| list_shapes_match(saved, &info))
                .filter(|saved| saved.slots[index].name.as_deref() == Some(name.as_str()))
                .and_then(|_| reuse.and_then(|bundle| bundle.blob(path, index)))
                .filter(|blob| blob.len() == info.size);
            let blob = if let Some(blob) = reusable {
                if boundary_chunks_match(device, &info, index, blob)? {
                    progress(Step::Reused {
                        path: path.to_owned(),
                        index,
                        name: name.clone(),
                        chunks: info.chunks_per_slot(),
                    });
                    blob.to_vec()
                } else {
                    read_and_verify_blob(device, &info, path, index, &name, &mut progress)?
                }
            } else {
                read_and_verify_blob(device, &info, path, index, &name, &mut progress)?
            };
            let relative = format!("{folder}/{index:03}.blob");
            let record = write_record(&staging.path, &relative, &blob)?;
            slots.push(SlotRecord {
                index,
                name: Some(name),
                blob: Some(record),
            });
        }

        // A rename/save during capture could pair the wrong name table with the
        // bytes just read. Refuse the whole capture rather than bless a tear.
        let after = device.list_info(info.path.clone())?;
        if info.names != after.names {
            return Err(Error::Backup(format!(
                "{} changed while it was being captured; no backup was published",
                info.path
            )));
        }
        lists.push(ListRecord {
            path: info.path.to_string(),
            description: info.description,
            size: info.size,
            count: info.count,
            chunk_size: info.chunk_size,
            group: info.group,
            gzip: info.gzip,
            movable: info.movable,
            item_type: info.item_type,
            slots,
        });
    }

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        captured_unix_seconds,
        identity: device.identity().clone(),
        schema: schema_record,
        lists,
    };
    // The manifest is the completeness marker and is always written last.
    atomic_write(staging.path.join("manifest.json"), &pretty_json(&manifest)?)
        .map_err(|error| backup_io("writing the backup manifest", error))?;
    staging.commit(target)?;
    progress(Step::Done);
    Ok(manifest)
}

fn list_shapes_match(saved: &ListRecord, current: &BlobList) -> bool {
    saved.size == current.size
        && saved.count == current.count
        && saved.chunk_size == current.chunk_size
        && saved.item_type == current.item_type
        && saved.slots.len() == current.count
}

fn read_and_verify_blob<L: Link>(
    device: &mut Device<L>,
    info: &BlobList,
    path: &str,
    index: usize,
    name: &str,
    progress: &mut impl FnMut(Step),
) -> Result<Vec<u8>> {
    let blob = device.read_blob_with_progress(info, index, |chunk, chunks| {
        progress(Step::Blob {
            path: path.to_owned(),
            index,
            name: name.to_owned(),
            chunk,
            chunks,
        });
    })?;
    progress(Step::Verifying {
        path: path.to_owned(),
        index,
    });
    verify_boundary_chunks(device, info, index, &blob)?;
    Ok(blob)
}

pub fn exists(target: &Path) -> bool {
    recover_bundle(target).is_ok() && target.join("manifest.json").is_file()
}

/// Validate and load every byte needed by restore before any device write.
pub fn open_verified(target: &Path) -> Result<VerifiedBundle> {
    recover_bundle(target)?;
    let manifest_path = target.join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|error| backup_io("reading the backup manifest", error))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        Error::Backup(format!(
            "{} is not valid JSON: {error}",
            manifest_path.display()
        ))
    })?;
    validate_manifest_shape(&manifest)?;

    let schema_bytes = read_and_verify(target, &manifest.schema)?;
    let schema: Vec<NodeSnapshot> = serde_json::from_slice(&schema_bytes).map_err(|error| {
        Error::Backup(format!("the schema snapshot is not valid JSON: {error}"))
    })?;
    let mut schema_paths = BTreeSet::new();
    for node in &schema {
        NodePath::new(node.path.clone())?;
        if !schema_paths.insert(&node.path) {
            return Err(Error::Backup(format!(
                "schema contains duplicate node {}",
                node.path
            )));
        }
    }

    let mut blobs = BTreeMap::new();
    for list in &manifest.lists {
        for slot in &list.slots {
            match (&slot.name, &slot.blob) {
                (None, None) => {}
                (Some(_), Some(file)) => {
                    let bytes = read_and_verify(target, file)?;
                    if bytes.len() != list.size {
                        return Err(Error::Backup(format!(
                            "{} slot {} has {} bytes; expected {}",
                            list.path,
                            slot.index,
                            bytes.len(),
                            list.size
                        )));
                    }
                    if list.item_type.as_deref() == Some("pst_pst") {
                        Preset::parse(&bytes).map_err(|error| {
                            Error::Backup(format!(
                                "{} slot {} is not a valid preset: {error}",
                                list.path, slot.index
                            ))
                        })?;
                    }
                    blobs.insert((list.path.clone(), slot.index), bytes);
                }
                _ => {
                    return Err(Error::Backup(format!(
                        "{} slot {} must have both a name and blob, or neither",
                        list.path, slot.index
                    )))
                }
            }
        }
    }
    Ok(VerifiedBundle {
        manifest,
        schema,
        blobs,
    })
}

fn validate_manifest_shape(manifest: &Manifest) -> Result<()> {
    if manifest.format_version != FORMAT_VERSION {
        return Err(Error::Backup(format!(
            "unsupported bundle format {}; expected {FORMAT_VERSION}",
            manifest.format_version
        )));
    }
    if manifest.schema.path != "schema.json" {
        return Err(Error::Backup(
            "the schema record must point to schema.json".into(),
        ));
    }
    let mut paths = BTreeSet::new();
    for list in &manifest.lists {
        let path = NodePath::new(list.path.clone())?;
        if !paths.insert(list.path.clone()) {
            return Err(Error::Backup(format!("duplicate list {}", list.path)));
        }
        if list.count != list.slots.len() || list.chunk_size == 0 || list.size == 0 {
            return Err(Error::Backup(format!(
                "{} has inconsistent count, size, or chunk metadata",
                list.path
            )));
        }
        let expected_folder = list_folder(&path);
        for (expected_index, slot) in list.slots.iter().enumerate() {
            if slot.index != expected_index {
                return Err(Error::Backup(format!(
                    "{} slot records are not contiguous from zero",
                    list.path
                )));
            }
            if let Some(file) = &slot.blob {
                let expected = format!("{expected_folder}/{expected_index:03}.blob");
                if file.path != expected {
                    return Err(Error::Backup(format!(
                        "{} slot {} points to unexpected path {:?}",
                        list.path, expected_index, file.path
                    )));
                }
            }
        }
    }
    Ok(())
}

fn verify_boundary_chunks<L: Link>(
    device: &mut Device<L>,
    list: &BlobList,
    index: usize,
    blob: &[u8],
) -> Result<()> {
    if boundary_chunks_match(device, list, index, blob)? {
        return Ok(());
    }
    Err(Error::Backup(format!(
        "{} slot {index} changed at a boundary chunk during capture",
        list.path
    )))
}

fn boundary_chunks_match<L: Link>(
    device: &mut Device<L>,
    list: &BlobList,
    index: usize,
    blob: &[u8],
) -> Result<bool> {
    let chunks = list.chunks_per_slot();
    let wanted = if chunks > 1 { vec![1, chunks] } else { vec![1] };
    let mut boundaries = device
        .read_blob_chunks(&list.path, index, &wanted)?
        .into_iter();
    let first = boundaries.next().expect("requested the first boundary");
    if blob.get(..first.len()) != Some(first.as_slice()) {
        return Ok(false);
    }
    if chunks > 1 {
        let last = boundaries.next().expect("requested the final boundary");
        if blob.get(blob.len().saturating_sub(last.len())..) != Some(last.as_slice()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_and_verify(root: &Path, file: &FileRecord) -> Result<Vec<u8>> {
    validate_relative_path(&file.path)?;
    let path = root.join(&file.path);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| backup_io("reading backup file metadata", error))?;
    if !metadata.file_type().is_file() {
        return Err(Error::Backup(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let bytes = std::fs::read(&path).map_err(|error| backup_io("reading a backup file", error))?;
    if bytes.len() != file.bytes {
        return Err(Error::Backup(format!(
            "{} has {} bytes; manifest says {}",
            path.display(),
            bytes.len(),
            file.bytes
        )));
    }
    let actual = sha256(&bytes);
    if actual != file.sha256 {
        return Err(Error::Backup(format!(
            "{} failed SHA-256 verification",
            path.display()
        )));
    }
    Ok(bytes)
}

fn validate_relative_path(path: &str) -> Result<()> {
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || path.contains('\\')
        || parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::Backup(format!(
            "unsafe bundle-relative path {path:?}"
        )));
    }
    Ok(())
}

fn list_folder(path: &NodePath) -> String {
    format!("slots/{}", path.as_str().replace('\\', "-"))
}

fn write_record(root: &Path, relative: &str, bytes: &[u8]) -> Result<FileRecord> {
    validate_relative_path(relative)?;
    atomic_write(root.join(relative), bytes)
        .map_err(|error| backup_io("writing a backup file", error))?;
    Ok(FileRecord {
        path: relative.to_owned(),
        bytes: bytes.len(),
        sha256: sha256(bytes),
    })
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn pretty_json(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map_err(|error| Error::Backup(format!("serializing backup JSON: {error}")))
}

fn atomic_write(path: impl AsRef<Path>, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = atomic_write_file::OpenOptions::new();
    #[cfg(unix)]
    {
        use atomic_write_file::unix::OpenOptionsExt as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).preserve_mode(false);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.commit()
}

fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .map_err(|error| backup_io("creating a private backup directory", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| backup_io("securing a backup directory", error))?;
    }
    Ok(())
}

fn backup_io(operation: &str, error: std::io::Error) -> Error {
    Error::Backup(format!("{operation}: {error}"))
}

struct StagingBundle {
    path: PathBuf,
    committed: bool,
}

impl StagingBundle {
    fn new(target: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let parent = bundle_parent(target);
        std::fs::create_dir_all(parent)
            .map_err(|error| backup_io("creating the bundle parent", error))?;
        let target_name = target
            .file_name()
            .ok_or_else(|| Error::Backup("a bundle needs a directory name".into()))?;
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let mut name = OsString::from(".");
            name.push(target_name);
            name.push(format!(".{}-{id}.incomplete", std::process::id()));
            let path = parent.join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                            .map_err(|error| backup_io("securing the staging bundle", error))?;
                    }
                    return Ok(Self {
                        path,
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(backup_io("creating a bundle staging directory", error)),
            }
        }
        Err(Error::Backup(
            "could not choose a unique bundle staging directory".into(),
        ))
    }

    fn commit(mut self, target: &Path) -> Result<()> {
        let previous = previous_bundle(target)?;
        let had_previous = target.exists();
        if had_previous {
            std::fs::rename(target, &previous)
                .map_err(|error| backup_io("putting the previous bundle aside", error))?;
        }
        if let Err(error) = std::fs::rename(&self.path, target) {
            let rollback_error = had_previous
                .then(|| std::fs::rename(&previous, target))
                .and_then(|result| result.err());
            let rollback = rollback_error
                .map(|error| format!("; rollback also failed: {error}"))
                .unwrap_or_default();
            return Err(Error::Backup(format!(
                "publishing completed bundle: {error}{rollback}"
            )));
        }
        self.committed = true;
        if had_previous {
            let _ = std::fs::remove_dir_all(previous);
        }
        Ok(())
    }
}

impl Drop for StagingBundle {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn bundle_parent(target: &Path) -> &Path {
    target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn previous_bundle(target: &Path) -> Result<PathBuf> {
    let mut name = target
        .file_name()
        .ok_or_else(|| Error::Backup("a bundle needs a directory name".into()))?
        .to_os_string();
    name.push(".previous");
    Ok(bundle_parent(target).join(name))
}

fn recover_bundle(target: &Path) -> Result<()> {
    let previous = previous_bundle(target)?;
    match (target.exists(), previous.exists()) {
        (false, true) => std::fs::rename(previous, target)
            .map_err(|error| backup_io("recovering the previous bundle", error))?,
        (true, true) => std::fs::remove_dir_all(previous)
            .map_err(|error| backup_io("cleaning a previous bundle", error))?,
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tonepush-voidx-backup-{name}-{}-{id}",
            std::process::id()
        ))
    }

    fn fixture(dir: &Path) -> Manifest {
        std::fs::create_dir_all(dir.join("slots/root-presets")).unwrap();
        let schema = pretty_json(&Vec::<NodeSnapshot>::new()).unwrap();
        let schema = write_record(dir, "schema.json", &schema).unwrap();
        let mut preset = b"root\\app\\preset:{\"value\":\"Clean\"}".to_vec();
        preset.resize(64, 0);
        let blob = write_record(dir, "slots/root-presets/000.blob", &preset).unwrap();
        let manifest = Manifest {
            format_version: FORMAT_VERSION,
            captured_unix_seconds: 1,
            identity: Identity {
                name: "StompStation PRO".into(),
                version: "1.5.12".into(),
                architecture: "CM4".into(),
                license: "sspro".into(),
            },
            schema,
            lists: vec![ListRecord {
                path: "root\\presets".into(),
                description: Some("Presets".into()),
                size: 64,
                count: 1,
                chunk_size: 64,
                group: None,
                gzip: false,
                movable: true,
                item_type: Some("pst_pst".into()),
                slots: vec![SlotRecord {
                    index: 0,
                    name: Some("Clean".into()),
                    blob: Some(blob),
                }],
            }],
        };
        atomic_write(dir.join("manifest.json"), &pretty_json(&manifest).unwrap()).unwrap();
        manifest
    }

    #[test]
    fn verified_bundle_owns_checked_preset_bytes() {
        let dir = scratch("valid");
        let manifest = fixture(&dir);
        let bundle = open_verified(&dir).unwrap();
        assert_eq!(bundle.manifest(), &manifest);
        assert_eq!(bundle.blob("root\\presets", 0).unwrap().len(), 64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_changed_blob_never_reaches_restore_preflight() {
        let dir = scratch("corrupt");
        fixture(&dir);
        std::fs::write(dir.join("slots/root-presets/000.blob"), [0_u8; 64]).unwrap();
        assert!(open_verified(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn manifest_paths_cannot_escape_the_bundle() {
        let dir = scratch("traversal");
        let mut manifest = fixture(&dir);
        manifest.lists[0].slots[0].blob.as_mut().unwrap().path = "../outside".into();
        atomic_write(dir.join("manifest.json"), &pretty_json(&manifest).unwrap()).unwrap();
        assert!(open_verified(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
