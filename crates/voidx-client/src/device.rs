use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use voidx_proto::blob::{chunk_count, decode_hex};
use voidx_proto::{router, Command, Frame, NodeDescription, NodePath, NodeTree, Router};

use crate::session::Session;
use crate::{values_equivalent, Error, Link, Notification, Result};

/// Firmware release lines (major.minor) on which every persistent write
/// TonePush makes was checked on a pedal, by reading each result back:
/// saving, renaming, moving, clearing, preset, IR, stereo IR and NAM
/// uploads, global settings and restore. Checked on 1.5.12, 2.0.10 and 2.2.6
/// (paced, after the computer had used the pedal's USB audio); patch releases
/// within a line are treated alike.
pub const VERIFIED_FIRMWARE: &[&str] = &["1.5", "2.0", "2.2"];

/// 2.0's NAM player caches models by slot and is not told when slots are
/// swapped, so a preset can keep playing the model that used to be there
/// until the pedal restarts. 2.2 fixed it.
const NAM_MOVES_STALE: &[&str] = &["2.0"];

/// The release line of a firmware version: `2.0.10` to `2.0`. `None` for
/// anything that is not at least `major.minor` in digits, such as
/// "Update Mode".
pub fn release_line(version: &str) -> Option<&str> {
    let mut parts = version.splitn(3, '.');
    let major = parts
        .next()
        .filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))?;
    let minor = parts
        .next()
        .filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))?;
    if let Some(patch) = parts.next() {
        if patch.is_empty() || !patch.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(&version[..major.len() + 1 + minor.len()])
}
const NAME_CHUNK_BYTES: usize = 128;
const READ_BATCH_CHUNKS: usize = 32;
/// Chunk data asked for in one batch. Firmware 2.2.6 moved presets to
/// 1024-byte chunks and sometimes takes seconds to answer a large batch, so
/// a batch is also bounded by bytes, not only by count.
const READ_BATCH_BYTES: usize = 16 * 1024;
/// Firmware 2.2.6 sometimes pauses for more than half a minute before
/// answering an ordinary 16 KiB read (34 s measured while reading NAM
/// models), then carries on. 1.5.12 and 2.0.10 answered within a second.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub license: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteSafety {
    /// Identity exactly matches the firmware used to establish write behavior.
    Verified,
    /// Reads and backups remain available, but mutations cannot be enabled.
    ReadOnly { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStep {
    Name,
    Data { chunk: usize, chunks: usize },
    Commit,
    Verify { chunk: usize, chunks: usize },
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlobList {
    pub path: NodePath,
    pub description: Option<String>,
    pub size: usize,
    pub count: usize,
    pub chunk_size: usize,
    pub group: Option<usize>,
    pub gzip: bool,
    pub movable: bool,
    pub item_type: Option<String>,
    pub names: Vec<Option<String>>,
}

impl BlobList {
    pub fn chunks_per_slot(&self) -> usize {
        chunk_count(self.size, self.chunk_size).expect("BlobList rejects a zero chunk size")
    }

    pub fn occupied(&self) -> impl Iterator<Item = (usize, &str)> {
        self.names
            .iter()
            .enumerate()
            .filter_map(|(index, name)| name.as_deref().map(|name| (index, name)))
    }
}

pub struct Device<L> {
    session: Session<L>,
    identity: Identity,
    write_safety: WriteSafety,
    writes_enabled: bool,
    live_edits_enabled: bool,
    interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl<L: Link> Device<L> {
    pub fn connect(link: L) -> Result<Self> {
        Self::connect_with_timeout(link, REPLY_TIMEOUT)
    }

    pub fn connect_with_timeout(link: L, timeout: Duration) -> Result<Self> {
        let mut session = Session::new(link, timeout);
        let identity = Identity {
            name: read_string(&mut session, "root\\sys\\_name")?,
            version: read_string(&mut session, "root\\sys\\_ver")?,
            architecture: read_string(&mut session, "root\\sys\\_arch")?,
            license: read_string(&mut session, "root\\sys\\_license")?,
        };
        let write_safety = assess_identity(&identity);
        if needs_pacing(&identity.version) {
            session.pace(PACE_GAP);
        }
        Ok(Self {
            session,
            identity,
            write_safety,
            writes_enabled: false,
            live_edits_enabled: false,
            interrupt: None,
        })
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn transport(&self) -> &str {
        self.session.description()
    }

    pub fn write_safety(&self) -> &WriteSafety {
        &self.write_safety
    }

    /// Require an explicit opt-in even on verified firmware. Mutating methods
    /// remain unavailable after this fails.
    pub fn enable_writes(&mut self) -> Result<()> {
        match &self.write_safety {
            WriteSafety::Verified => {
                self.writes_enabled = true;
                Ok(())
            }
            WriteSafety::ReadOnly { reason } => Err(Error::WriteRefused(reason.clone())),
        }
    }

    pub fn writes_enabled(&self) -> bool {
        self.writes_enabled
    }

    /// Allow edits to the live state under `root\app` (the loaded preset,
    /// its parameters and blocks) on a PRO whose firmware is not verified for
    /// persistent writes. Nothing reaches the pedal's memory this way: saving
    /// a preset, slots, libraries and global settings still need
    /// [`Device::enable_writes`]. Refused in update mode, which has no live
    /// state.
    pub fn enable_live_edits(&mut self) -> Result<()> {
        if self.identity.name != "StompStation PRO"
            || self.identity.version == voidx_proto::update::UPDATE_MODE_VERSION
        {
            return Err(Error::WriteRefused(format!(
                "{} {} has no live state to edit",
                self.identity.name, self.identity.version
            )));
        }
        self.live_edits_enabled = true;
        Ok(())
    }

    pub fn read(&mut self, path: NodePath) -> Result<Frame> {
        self.session.request(Command::Read(path))
    }

    pub fn read_value(&mut self, path: NodePath) -> Result<Value> {
        let frame = self.read(path.clone())?;
        response_value(&frame, &path.to_string())
    }

    pub fn browse(&mut self, path: NodePath) -> Result<NodeTree> {
        let frame = self.session.request(Command::Browse(path))?;
        Ok(NodeTree::from_frame(&frame)?)
    }

    pub fn list_info(&mut self, path: NodePath) -> Result<BlobList> {
        let tree = self.browse(path.clone())?;
        let node = tree.get(&path).ok_or_else(|| Error::InvalidResponse {
            subject: path.to_string(),
            detail: "browse response omitted the requested list node".into(),
        })?;
        let size = required(node.size, &path, "size")?;
        let count = required(node.count, &path, "count")?;
        let chunk_size = required(node.chunk, &path, "chunk")?;
        if chunk_size == 0 {
            return Err(Error::InvalidResponse {
                subject: path.to_string(),
                detail: "chunk size was zero".into(),
            });
        }

        let value = self.read_value(path.clone())?;
        let values = value.as_array().ok_or_else(|| Error::InvalidResponse {
            subject: path.to_string(),
            detail: "list value was not an array".into(),
        })?;
        if values.len() != count {
            return Err(Error::InvalidResponse {
                subject: path.to_string(),
                detail: format!(
                    "metadata says {count} slots but the name table has {}",
                    values.len()
                ),
            });
        }
        let names = values
            .iter()
            .enumerate()
            .map(|(index, value)| match value {
                Value::String(name) if name.is_empty() => Ok(None),
                Value::String(name) => Ok(Some(name.clone())),
                Value::Null => Ok(None),
                _ => Err(Error::InvalidResponse {
                    subject: path.to_string(),
                    detail: format!("slot {index} name was not a string or null"),
                }),
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(BlobList {
            path,
            description: node.desc.clone(),
            size,
            count,
            chunk_size,
            group: node.group,
            gzip: node.gzip.unwrap_or(false),
            movable: node.movable.unwrap_or(false),
            item_type: node.item_type.clone(),
            names,
        })
    }

    /// Read one occupied list slot in small protocol batches. Every response
    /// still has to echo the requested slot and chunk before its fixed-size,
    /// padded device bytes are accepted.
    /// Let a flag stop the slot reads that follow: while it is raised, every
    /// blob read returns [`Error::Cancelled`] before it sends anything. For a
    /// long check or backup someone may want to leave; `None` takes it away.
    pub fn set_interrupt(&mut self, flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) {
        self.interrupt = flag;
    }

    fn interrupted(&self) -> Result<()> {
        match &self.interrupt {
            Some(flag) if flag.load(std::sync::atomic::Ordering::Relaxed) => Err(Error::Cancelled),
            _ => Ok(()),
        }
    }

    pub fn read_blob(&mut self, list: &BlobList, index: usize) -> Result<Vec<u8>> {
        self.read_blob_with_progress(list, index, |_, _| {})
    }

    pub fn read_blob_with_progress(
        &mut self,
        list: &BlobList,
        index: usize,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Vec<u8>> {
        if index >= list.count {
            return Err(Error::SlotOutOfRange {
                path: list.path.clone(),
                index,
                count: list.count,
            });
        }
        if list.names[index].is_none() {
            return Err(Error::EmptySlot {
                path: list.path.clone(),
                index,
            });
        }

        let chunks = list.chunks_per_slot();
        let mut blob = Vec::with_capacity(list.size);
        progress(0, chunks);
        let batch = self.batch_chunks(list.chunk_size);
        for first in (1..=chunks).step_by(batch) {
            let last = (first + batch - 1).min(chunks);
            let requested = (first..=last).collect::<Vec<_>>();
            for (chunk, bytes) in requested
                .iter()
                .copied()
                .zip(self.read_blob_chunks(&list.path, index, &requested)?)
            {
                let expected = if chunk == chunks {
                    list.size - list.chunk_size * (chunks - 1)
                } else {
                    list.chunk_size
                };
                if bytes.len() != expected {
                    return Err(Error::InvalidResponse {
                        subject: format!("dread {}", list.path),
                        detail: format!(
                            "slot {index} chunk {chunk} contained {} bytes; expected {expected}",
                            bytes.len()
                        ),
                    });
                }
                blob.extend_from_slice(&bytes);
                progress(chunk, chunks);
            }
        }
        Ok(blob)
    }

    pub fn read_blob_chunk(
        &mut self,
        path: &NodePath,
        index: usize,
        chunk: usize,
    ) -> Result<Vec<u8>> {
        self.interrupted()?;
        let command = Command::data_read(path.clone(), index, chunk)?;
        let subject = command.response_subject();
        let frame = self.session.request(command)?;
        let record = frame
            .records()
            .iter()
            .find(|record| record.subject() == subject)
            .ok_or_else(|| Error::InvalidResponse {
                subject: subject.clone(),
                detail: "matching response record was absent".into(),
            })?;
        let (actual_chunk, bytes) = decode_data_read_record(record, &subject, index)?;
        if actual_chunk != chunk {
            return Err(Error::InvalidResponse {
                subject,
                detail: format!("chunk echoed {actual_chunk}, expected value {chunk}"),
            });
        }
        Ok(bytes)
    }

    pub(crate) fn read_blob_chunks(
        &mut self,
        path: &NodePath,
        index: usize,
        chunks: &[usize],
    ) -> Result<Vec<Vec<u8>>> {
        self.interrupted()?;
        let commands = chunks
            .iter()
            .map(|chunk| Command::data_read(path.clone(), index, *chunk))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let subject = format!("dread {path}");
        let frame = self.session.request_many(&commands)?;
        let mut decoded = BTreeMap::new();
        for record in frame
            .records()
            .iter()
            .filter(|record| record.subject() == subject)
        {
            let (chunk, bytes) = decode_data_read_record(record, &subject, index)?;
            if !chunks.contains(&chunk) {
                return Err(Error::InvalidResponse {
                    subject,
                    detail: format!("batch returned unrequested chunk {chunk}"),
                });
            }
            if decoded.insert(chunk, bytes).is_some() {
                return Err(Error::InvalidResponse {
                    subject,
                    detail: format!("batch returned chunk {chunk} more than once"),
                });
            }
        }
        chunks
            .iter()
            .map(|chunk| {
                decoded.remove(chunk).ok_or_else(|| Error::InvalidResponse {
                    subject: subject.clone(),
                    detail: format!("batch omitted chunk {chunk}"),
                })
            })
            .collect()
    }

    /// Set a live node after validating against its browsed description.
    pub fn write_node(
        &mut self,
        path: NodePath,
        description: &NodeDescription,
        value: Value,
    ) -> Result<()> {
        description.validate_value(&value)?;
        self.write_value(path, value)
    }

    /// Set a live node and require the acknowledgement to echo the value.
    fn write_value(&mut self, path: NodePath, value: Value) -> Result<()> {
        if !(self.live_edits_enabled && is_live(&path)) {
            self.require_writes()?;
        }
        let frame = self.session.request(Command::Write {
            path: path.clone(),
            value: value.clone(),
        })?;
        let echoed = response_value(&frame, path.as_str())?;
        if !values_equivalent(&value, &echoed) {
            return Err(Error::InvalidResponse {
                subject: path.to_string(),
                detail: format!("write acknowledgement echoed {echoed}, expected {value}"),
            });
        }
        Ok(())
    }

    /// The chain on firmware 2.x, as the pedal holds it now.
    pub fn read_router(&mut self) -> Result<Router> {
        let path = NodePath::new(router::PATH)?;
        let value = self.read_value(path.clone())?;
        Router::from_value(&value).map_err(|error| Error::InvalidResponse {
            subject: path.to_string(),
            detail: error.to_string(),
        })
    }

    /// Write a whole chain once and read back what the pedal made of it.
    ///
    /// The pedal answers a router write by echoing the request, whether it
    /// applied it, adjusted it (a fixed block kept in its place, a block
    /// written twice kept once) or ignored it, so the echo proves only that
    /// the write arrived: the chain read afterwards is the result. Like any
    /// edit under `root\app` it changes the live preset only, so live edits
    /// are enough.
    pub fn write_router(&mut self, chain: &Router) -> Result<Router> {
        self.write_value(NodePath::new(router::PATH)?, chain.to_value())?;
        self.read_router()
    }

    /// Persist the current live state under the supplied preset name.
    pub fn save_preset(&mut self, name: &str) -> Result<()> {
        validate_slot_name(name)?;
        self.require_writes()?;
        let path = NodePath::new("root\\app\\preset")?;
        let value = Value::String(name.to_owned());
        let frame = self.session.request(Command::Save {
            path: path.clone(),
            value: value.clone(),
        })?;
        if response_value(&frame, path.as_str())? != value {
            return Err(Error::InvalidResponse {
                subject: path.to_string(),
                detail: "save acknowledgement did not echo the preset name".into(),
            });
        }
        Ok(())
    }

    /// Select a preset slot by its current name. This changes live sound but
    /// does not write flash.
    pub fn select_preset(&mut self, index: usize) -> Result<()> {
        let list = self.list_info(NodePath::new("root\\presets")?)?;
        validate_slot(&list, index)?;
        let name = list.names[index].clone().ok_or_else(|| Error::EmptySlot {
            path: list.path.clone(),
            index,
        })?;
        let path = NodePath::new("root\\app\\preset")?;
        let tree = self.browse(path.clone())?;
        let description = tree.get(&path).ok_or_else(|| Error::InvalidResponse {
            subject: path.to_string(),
            detail: "preset selector browse omitted its own node".into(),
        })?;
        self.write_node(path, description, Value::String(name))
    }

    /// Rename an occupied slot without rewriting its content.
    ///
    /// NAM and IR references in presets are name-based; callers must scan and
    /// update those references before renaming a model used by any preset.
    pub fn rename_slot(&mut self, list: &BlobList, index: usize, name: &str) -> Result<()> {
        validate_slot(list, index)?;
        validate_slot_name(name)?;
        self.require_writes()?;
        let expected = name.to_owned();
        let name_bytes = padded_name(name)?;
        self.data_write_acked(&list.path, index, -1, name_bytes, -1)?;
        let refreshed = self.list_info(list.path.clone())?;
        if refreshed.names[index].as_deref() != Some(expected.as_str()) {
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: format!("slot {index} name did not become {expected:?}"),
            });
        }
        Ok(())
    }

    /// Clear a slot's name-table entry. Firmware treats the all-zero commit as
    /// deletion; the content becomes inaccessible.
    pub fn clear_slot(&mut self, list: &BlobList, index: usize) -> Result<()> {
        validate_slot(list, index)?;
        self.require_writes()?;
        self.data_write_acked(&list.path, index, -1, vec![0; NAME_CHUNK_BYTES], -1)?;
        let refreshed = self.list_info(list.path.clone())?;
        if refreshed.names[index].is_some() {
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: format!("slot {index} still has a name after clear"),
            });
        }
        Ok(())
    }

    /// Upload one exact fixed-size device blob using name → data → name commit,
    /// verify every next-chunk acknowledgement, then read back every byte.
    pub fn write_blob(
        &mut self,
        list: &BlobList,
        index: usize,
        name: &str,
        blob: &[u8],
        mut progress: impl FnMut(UploadStep),
    ) -> Result<()> {
        validate_slot(list, index)?;
        validate_slot_name(name)?;
        if blob.len() != list.size {
            return Err(Error::BlobSize {
                path: list.path.clone(),
                actual: blob.len(),
                expected: list.size,
            });
        }
        self.require_writes()?;
        // The pedal drops an upload whose name another slot already has,
        // while acknowledging every chunk as usual.
        let current = self.list_info(list.path.clone())?;
        if let Some(other) = current
            .names
            .iter()
            .enumerate()
            .position(|(slot, existing)| slot != index && existing.as_deref() == Some(name))
        {
            return Err(Error::WriteRefused(format!(
                "slot {} of {} is already named {name:?}; the pedal ignores an upload under a name another slot uses",
                other + 1,
                list.path
            )));
        }

        // Name chunks are always 128 bytes on 1.5.12, independently of the
        // list's data chunk size (NAM data chunks are 1024 bytes).
        let name_bytes = padded_name(name)?;
        progress(UploadStep::Name);
        self.data_write_acked(&list.path, index, 0, name_bytes.clone(), 1)?;
        let chunks = list.chunks_per_slot();
        for chunk in 1..=chunks {
            progress(UploadStep::Data { chunk, chunks });
            let start = (chunk - 1) * list.chunk_size;
            let mut data = blob[start..(start + list.chunk_size).min(blob.len())].to_vec();
            data.resize(list.chunk_size, 0);
            let next = if chunk == chunks {
                -1
            } else {
                i32::try_from(chunk + 1).map_err(|_| Error::InvalidResponse {
                    subject: list.path.to_string(),
                    detail: "chunk count does not fit firmware integer range".into(),
                })?
            };
            let sent = i32::try_from(chunk).map_err(|_| Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: "chunk number does not fit firmware integer range".into(),
            })?;
            self.data_write_acked(&list.path, index, sent, data, next)?;
        }
        progress(UploadStep::Commit);
        self.data_write_acked(&list.path, index, -1, name_bytes, -1)?;

        let refreshed = self.list_info(list.path.clone())?;
        if refreshed.names[index].as_deref() != Some(name) {
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: format!("slot {index} name did not become {name:?} after commit"),
            });
        }
        let readback = self.read_blob_with_progress(&refreshed, index, |chunk, chunks| {
            progress(UploadStep::Verify { chunk, chunks });
        })?;
        // The pedal stores a NAM model uncompressed and gzips it again when it
        // is read, with its own header (OS byte 3, Unix), so a gzip slot is
        // checked by what it decompresses to.
        let same = if list.gzip {
            let wrote = gunzip(blob);
            wrote.is_some() && wrote == gunzip(&readback)
        } else {
            readback == blob
        };
        if !same {
            let first = readback
                .iter()
                .zip(blob)
                .position(|(actual, expected)| actual != expected)
                .unwrap_or(readback.len().min(blob.len()));
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: format!(
                    "slot {} read back differently from what was written, from byte {first}",
                    index + 1
                ),
            });
        }
        progress(UploadStep::Done);
        Ok(())
    }

    /// Atomically exchange two name+content slots. A fresh name-table read must
    /// confirm the permutation before this reports success.
    pub fn swap_slots(&mut self, list: &BlobList, first: usize, second: usize) -> Result<()> {
        validate_slot(list, first)?;
        validate_slot(list, second)?;
        if !list.movable {
            return Err(Error::WriteRefused(format!(
                "{} does not advertise move support",
                list.path
            )));
        }
        self.require_writes()?;
        if list.gzip
            && release_line(&self.identity.version)
                .is_some_and(|line| NAM_MOVES_STALE.contains(&line))
        {
            return Err(Error::WriteRefused(format!(
                "firmware {} keeps playing the old model after NAM slots move until it restarts; update to 2.2.6 to reorder models",
                self.identity.version
            )));
        }
        let before = self.list_info(list.path.clone())?;
        let before_first = before.names[first].clone();
        let before_second = before.names[second].clone();
        let command = Command::DataSwap {
            path: list.path.clone(),
            first,
            second,
        };
        let subject = command.response_subject();
        let frame = self.session.request(command)?;
        let body = response_object(&frame, &subject)?;
        expect_unsigned(body.get("index"), first, &subject, "index")?;
        expect_unsigned(body.get("index2"), second, &subject, "index2")?;
        let refreshed = self.list_info(list.path.clone())?;
        if refreshed.names[first] != before_second || refreshed.names[second] != before_first {
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: "name table did not reflect the requested swap".into(),
            });
        }
        Ok(())
    }

    /// Move one slot while shifting the intervening range. The firmware only
    /// exposes atomic swap, so a move is a checked sequence of adjacent swaps.
    /// If one fails on an aligned session, completed swaps are reversed.
    pub fn move_slot(&mut self, list: &BlobList, from: usize, to: usize) -> Result<()> {
        validate_slot(list, from)?;
        validate_slot(list, to)?;
        if from == to {
            return Ok(());
        }
        let before = self.list_info(list.path.clone())?;
        let pairs = if from < to {
            (from..to)
                .map(|index| (index, index + 1))
                .collect::<Vec<_>>()
        } else {
            ((to + 1)..=from)
                .rev()
                .map(|index| (index, index - 1))
                .collect::<Vec<_>>()
        };
        let mut completed = Vec::new();
        let mut names = before.names.clone();
        for &(first, second) in &pairs {
            if let Err(error) = self.swap_slots(list, first, second) {
                // The swap can fail after the pedal applied it (a bad echo or
                // read-back). Undo it too when the name table shows it took.
                let mut swapped = names.clone();
                swapped.swap(first, second);
                if self
                    .list_info(list.path.clone())
                    .is_ok_and(|now| now.names == swapped && swapped != names)
                {
                    completed.push((first, second));
                }
                let rollback = completed
                    .iter()
                    .rev()
                    .try_for_each(|&(first, second)| self.swap_slots(list, first, second))
                    .and_then(|()| {
                        let now = self.list_info(list.path.clone())?;
                        if now.names == before.names {
                            Ok(())
                        } else {
                            Err(Error::InvalidResponse {
                                subject: list.path.to_string(),
                                detail: "name table differs from before the move".into(),
                            })
                        }
                    });
                return match rollback {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(Error::WriteRefused(format!(
                        "move failed: {error}; reversing completed swaps also failed: {rollback}"
                    ))),
                };
            }
            names.swap(first, second);
            completed.push((first, second));
        }

        let mut expected = before.names;
        let moved = expected.remove(from);
        expected.insert(to, moved);
        let after = self.list_info(list.path.clone())?;
        if after.names != expected {
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: "name table does not match the requested move".into(),
            });
        }
        Ok(())
    }

    fn data_write_acked(
        &mut self,
        path: &NodePath,
        index: usize,
        chunk: i32,
        value: Vec<u8>,
        expected_next: i32,
    ) -> Result<()> {
        let command = Command::data_write(path.clone(), index, chunk, value)?;
        let subject = command.response_subject();
        let frame = self.session.request(command)?;
        let body = response_object(&frame, &subject)?;
        // Firmware 1.5.12 reports `index:-1` for NAM-list acknowledgements.
        // The exact response subject, strict one-request sequencing, and the
        // required next-chunk value identify those ACKs. Other numeric indices
        // still have to echo the requested slot.
        if let Some(echoed) = body.get("index").and_then(Value::as_i64) {
            if echoed >= 0 {
                expect_unsigned(body.get("index"), index, &subject, "index")?;
            } else if echoed != -1 {
                return Err(Error::InvalidResponse {
                    subject,
                    detail: format!("write acknowledgement used invalid index {echoed}"),
                });
            }
        }
        let acknowledged = body.get("chunk").and_then(Value::as_i64);
        if acknowledged != Some(i64::from(expected_next)) {
            return Err(Error::InvalidResponse {
                subject,
                detail: format!(
                    "write of chunk {chunk} acknowledged {acknowledged:?}; expected next chunk {expected_next}"
                ),
            });
        }
        Ok(())
    }

    fn require_writes(&self) -> Result<()> {
        if self.writes_enabled {
            Ok(())
        } else {
            Err(Error::WriteRefused(
                "call enable_writes after showing the user the exact operation".into(),
            ))
        }
    }

    pub fn drain_notifications(&mut self) -> Vec<Notification> {
        self.session.drain_notifications()
    }

    pub fn disconnect(self) -> L {
        self.session.into_link()
    }
}

fn decode_data_read_record(
    record: &voidx_proto::Record,
    subject: &str,
    index: usize,
) -> Result<(usize, Vec<u8>)> {
    let body = record
        .value()
        .as_object()
        .ok_or_else(|| Error::InvalidResponse {
            subject: subject.to_owned(),
            detail: "response body was not an object".into(),
        })?;
    expect_unsigned(body.get("index"), index, subject, "index")?;
    let chunk = body
        .get("chunk")
        .and_then(Value::as_u64)
        .and_then(|chunk| usize::try_from(chunk).ok())
        .ok_or_else(|| Error::InvalidResponse {
            subject: subject.to_owned(),
            detail: "response had no unsigned chunk number".into(),
        })?;
    let hex = body
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidResponse {
            subject: subject.to_owned(),
            detail: "response had no string value".into(),
        })?;
    Ok((chunk, decode_hex(hex)?))
}

/// The loaded preset's live tree, which the pedal keeps only until another
/// preset loads unless it is saved.
fn is_live(path: &NodePath) -> bool {
    path.as_str().starts_with("root\\app\\")
}

/// The whole content of a gzip stream, or `None` when it does not decompress.
fn gunzip(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut content = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut content)
        .ok()?;
    Some(content)
}

pub(crate) fn read_batch_chunks(chunk_size: usize) -> usize {
    (READ_BATCH_BYTES / chunk_size.max(1)).clamp(1, READ_BATCH_CHUNKS)
}

/// Quiet kept after each reply on firmware that needs pacing.
const PACE_GAP: Duration = Duration::from_millis(50);
/// Chunk data asked for in one paced request.
const PACED_BATCH_BYTES: usize = 8 * 1024;

/// Firmware 1.x and 2.0 answer back-to-back requests at full speed. 2.2
/// stalls once the computer has used the pedal's USB audio unless requests
/// stay small and the link goes quiet between replies: on a pedal at 2.2.6,
/// 8 KiB requests with 50 ms after each reply read 400 batches without a
/// stall, where 16 KiB back to back stalled on 14 of 40. Anything newer than
/// 2.0 is paced until shown otherwise.
fn needs_pacing(version: &str) -> bool {
    !matches!(release_line(version), Some(line) if line.starts_with("1.") || line == "2.0")
        && version != voidx_proto::update::UPDATE_MODE_VERSION
}

impl<L: Link> Device<L> {
    /// Chunks per read request for a list with `chunk_size`-byte chunks.
    pub(crate) fn batch_chunks(&self, chunk_size: usize) -> usize {
        if self.session.paced() {
            (PACED_BATCH_BYTES / chunk_size.max(1)).clamp(1, READ_BATCH_CHUNKS)
        } else {
            read_batch_chunks(chunk_size)
        }
    }
}

fn assess_identity(identity: &Identity) -> WriteSafety {
    let expected = format!(
        "StompStation PRO / firmware {}.x / CM4 / sspro",
        VERIFIED_FIRMWARE.join(".x or ")
    );
    if identity.name == "StompStation PRO"
        && release_line(&identity.version).is_some_and(|line| VERIFIED_FIRMWARE.contains(&line))
        && identity.architecture == "CM4"
        && identity.license == "sspro"
    {
        WriteSafety::Verified
    } else {
        WriteSafety::ReadOnly {
            reason: format!(
                "connected identity is {} / firmware {} / {} / {}; writes are verified only for {expected}",
                identity.name, identity.version, identity.architecture, identity.license
            ),
        }
    }
}

fn read_string<L: Link>(session: &mut Session<L>, path: &str) -> Result<String> {
    let path = NodePath::new(path)?;
    let frame = session.request(Command::Read(path.clone()))?;
    response_value(&frame, path.as_str())?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidResponse {
            subject: path.to_string(),
            detail: "identity value was not a string".into(),
        })
}

fn response_value(frame: &Frame, subject: &str) -> Result<Value> {
    frame
        .records()
        .iter()
        .find(|record| record.subject() == subject)
        .and_then(|record| record.value().get("value"))
        .cloned()
        .ok_or_else(|| Error::InvalidResponse {
            subject: subject.to_owned(),
            detail: "response had no value field".into(),
        })
}

fn response_object<'a>(
    frame: &'a Frame,
    subject: &str,
) -> Result<&'a serde_json::Map<String, Value>> {
    frame
        .records()
        .iter()
        .find(|record| record.subject() == subject)
        .and_then(|record| record.value().as_object())
        .ok_or_else(|| Error::InvalidResponse {
            subject: subject.to_owned(),
            detail: "response had no object body".into(),
        })
}

fn validate_slot(list: &BlobList, index: usize) -> Result<()> {
    if index < list.count {
        Ok(())
    } else {
        Err(Error::SlotOutOfRange {
            path: list.path.clone(),
            index,
            count: list.count,
        })
    }
}

fn validate_slot_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || name
            .chars()
            .any(|character| character.is_control() || character == '\0')
    {
        Err(Error::InvalidSlotName(name.to_owned()))
    } else {
        Ok(())
    }
}

fn padded_name(name: &str) -> Result<Vec<u8>> {
    validate_slot_name(name)?;
    let mut bytes = name.as_bytes().to_vec();
    bytes.resize(NAME_CHUNK_BYTES, 0);
    Ok(bytes)
}

fn required<T: Copy>(value: Option<T>, path: &NodePath, field: &'static str) -> Result<T> {
    value.ok_or_else(|| Error::MissingMetadata {
        path: path.clone(),
        field,
    })
}

fn expect_unsigned(
    value: Option<&Value>,
    expected: usize,
    subject: &str,
    field: &str,
) -> Result<()> {
    let actual = value.and_then(Value::as_u64);
    if actual == u64::try_from(expected).ok() {
        Ok(())
    } else {
        Err(Error::InvalidResponse {
            subject: subject.to_owned(),
            detail: format!(
                "{field} echoed {value:?} ({actual:?} as unsigned), expected value {expected}"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use super::*;

    struct Scripted {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Scripted {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Link for Scripted {
        fn description(&self) -> &str {
            "fixture"
        }
    }

    fn identity_frames(version: &str) -> Vec<u8> {
        format!(
            "root\\sys\\_name:{{\"value\":\"StompStation PRO\"}}\0\
             root\\sys\\_ver:{{\"value\":\"{version}\"}}\0\
             root\\sys\\_arch:{{\"value\":\"CM4\"}}\0\
             root\\sys\\_license:{{\"value\":\"sspro\"}}\0"
        )
        .into_bytes()
    }

    #[test]
    fn writes_need_both_a_verified_identity_and_explicit_opt_in() {
        let link = Scripted {
            input: Cursor::new(identity_frames("1.5.12")),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        assert_eq!(device.write_safety(), &WriteSafety::Verified);
        assert!(!device.writes_enabled());
        device.enable_writes().unwrap();
        assert!(device.writes_enabled());

        let link = Scripted {
            input: Cursor::new(identity_frames("1.6.0")),
            output: vec![],
        };
        let mut future = Device::connect(link).unwrap();
        assert!(matches!(
            future.write_safety(),
            WriteSafety::ReadOnly { .. }
        ));
        assert!(future.enable_writes().is_err());
    }

    /// A raised interrupt stops blob reads before anything is sent, so the
    /// session stays aligned for whatever comes next.
    #[test]
    fn a_raised_interrupt_stops_slot_reads_before_they_are_sent() {
        let link = Scripted {
            input: Cursor::new(identity_frames("1.5.12")),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        device.set_interrupt(Some(flag.clone()));
        let sent = |device: &Device<Scripted>| device.session.link().output.len();
        let before = sent(&device);
        let path = NodePath::new("root\\presets").unwrap();
        assert!(matches!(
            device.read_blob_chunk(&path, 0, 1),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            device.read_blob_chunks(&path, 0, &[1, 2]),
            Err(Error::Cancelled)
        ));
        assert_eq!(sent(&device), before, "nothing reached the pedal");
        assert!(!Error::Cancelled.loses_session());
    }

    #[test]
    fn unverified_firmware_can_change_only_the_live_preset() {
        let mut frames = identity_frames("2.3.0");
        frames.extend_from_slice(b"root\\app\\amp\\gain:{\"value\":5}\0");
        let link = Scripted {
            input: Cursor::new(frames),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        assert!(device.enable_writes().is_err());
        let gain = NodePath::new("root\\app\\amp\\gain").unwrap();
        assert!(device.write_value(gain.clone(), Value::from(5)).is_err());

        device.enable_live_edits().unwrap();
        device.write_value(gain, Value::from(5)).unwrap();
        let setting = NodePath::new("root\\settings\\brightness").unwrap();
        assert!(matches!(
            device.write_value(setting, Value::from(5)),
            Err(Error::WriteRefused(_))
        ));
        assert!(device.save_preset("Mine").is_err());
    }

    /// A chain is written once and read back, and the result is what the
    /// pedal read back: here it kept the amp in its fixed position and
    /// dropped the copy written elsewhere, while echoing the request as
    /// sent.
    #[test]
    fn a_chain_is_written_once_and_the_pedals_reading_is_the_result() {
        let asked = r#"[["root\\app\\amp","p","root\\app\\amp","s",""]]"#;
        let kept = r#"[["root\\app\\amp","p","","s",""]]"#;
        let mut frames = identity_frames("2.2.6");
        frames.extend_from_slice(format!("root\\app\\router:{{\"value\":{asked}}}\0").as_bytes());
        frames.extend_from_slice(format!("root\\app\\router:{{\"value\":{kept}}}\0").as_bytes());
        let link = Scripted {
            input: Cursor::new(frames),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        device.enable_live_edits().unwrap();
        let chain = Router::from_value(&serde_json::from_str::<Value>(asked).unwrap()).unwrap();
        let read = device.write_router(&chain).unwrap();
        assert_eq!(
            read.to_value(),
            serde_json::from_str::<Value>(kept).unwrap()
        );
        let sent = device.disconnect().output;
        let identity = identity_requests();
        assert_eq!(
            String::from_utf8_lossy(&sent[identity.len()..]),
            format!("write root\\app\\router:{{\"value\":{asked}}}\0read root\\app\\router\0")
        );
    }

    /// A pedal opened read only, without live edits, is sent nothing.
    #[test]
    fn a_chain_is_not_written_without_live_edits() {
        let link = Scripted {
            input: Cursor::new(identity_frames("2.2.6")),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        let chain = Router::from_value(&serde_json::json!([["", "s", ""]])).unwrap();
        assert!(matches!(
            device.write_router(&chain),
            Err(Error::WriteRefused(_))
        ));
        let sent = device.disconnect().output;
        assert!(!String::from_utf8_lossy(&sent).contains("router"));
    }

    /// An answer that is not a row is a broken answer, not an empty chain.
    #[test]
    fn a_chain_that_reads_back_as_anything_else_is_an_error() {
        let mut frames = identity_frames("2.0.10");
        frames.extend_from_slice(b"root\\app\\router:{\"value\":\"idle\"}\0");
        let link = Scripted {
            input: Cursor::new(frames),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        assert!(matches!(
            device.read_router(),
            Err(Error::InvalidResponse { .. })
        ));
    }

    /// What connecting asks for, before anything a test does.
    fn identity_requests() -> Vec<u8> {
        b"read root\\sys\\_name\0read root\\sys\\_ver\0read root\\sys\\_arch\0read root\\sys\\_license\0"
            .to_vec()
    }

    #[test]
    fn nam_models_are_not_reordered_on_2_0_10() {
        let link = Scripted {
            input: Cursor::new(identity_frames("2.0.10")),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        // Refused either way: unverified firmware cannot write at all, and on
        // verified firmware the NAM guard refuses before anything is sent.
        let _ = device.enable_writes();
        let amps = BlobList {
            path: NodePath::new("root\\nam_amp").unwrap(),
            description: None,
            size: 1 << 20,
            count: 60,
            chunk_size: 1024,
            group: None,
            gzip: true,
            movable: true,
            item_type: Some("nam".into()),
            names: vec![None; 60],
        };
        assert!(matches!(
            device.swap_slots(&amps, 0, 1),
            Err(Error::WriteRefused(_))
        ));
        let sent = device.disconnect().output;
        assert!(!String::from_utf8_lossy(&sent).contains("dswap"));
    }

    #[test]
    fn a_gzip_slot_is_compared_by_its_content() {
        use std::io::Write as _;
        let gz = |os: u8| {
            let mut encoder = flate2::GzBuilder::new()
                .operating_system(os)
                .write(Vec::new(), flate2::Compression::default());
            encoder.write_all(b"{\"version\":\"0.5.4\"}").unwrap();
            encoder.finish().unwrap()
        };
        assert_ne!(gz(255), gz(3));
        assert_eq!(gunzip(&gz(255)), gunzip(&gz(3)));
        assert_eq!(gunzip(b"not gzip"), None);
    }

    #[test]
    fn patch_releases_share_their_line() {
        assert_eq!(release_line("1.5.10"), Some("1.5"));
        assert_eq!(release_line("2.0.10"), Some("2.0"));
        assert_eq!(release_line("2.2"), Some("2.2"));
        assert_eq!(release_line("Update Mode"), None);
        assert_eq!(release_line("1.5.x"), None);
        for (version, verified) in [
            ("1.5.10", true),
            ("1.5.12", true),
            ("2.0.8", true),
            ("2.2.6", true),
            ("2.3.0", false),
            ("1.4.2", false),
        ] {
            let identity = Identity {
                name: "StompStation PRO".into(),
                version: version.into(),
                architecture: "CM4".into(),
                license: "sspro".into(),
            };
            assert_eq!(
                assess_identity(&identity) == WriteSafety::Verified,
                verified,
                "{version}"
            );
        }
    }

    #[test]
    fn firmware_from_2_2_is_paced() {
        assert!(!needs_pacing("1.5.12"));
        assert!(!needs_pacing("2.0.10"));
        assert!(needs_pacing("2.2.6"));
        assert!(needs_pacing("3.0.0"));
        assert!(!needs_pacing("Update Mode"));
    }

    #[test]
    fn a_batch_is_bounded_by_bytes_as_well_as_chunks() {
        assert_eq!(read_batch_chunks(128), 32);
        assert_eq!(read_batch_chunks(1024), 16);
        assert_eq!(read_batch_chunks(1 << 20), 1);
    }

    #[test]
    fn chunk_response_must_echo_its_coordinates() {
        let mut frames = identity_frames("1.5.12");
        frames.extend_from_slice(
            b"dread root\\presets:{\"index\":0,\"chunk\":2,\"value\":\"0001\"}\0",
        );
        let link = Scripted {
            input: Cursor::new(frames),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        let path = NodePath::new("root\\presets").unwrap();
        assert!(matches!(
            device.read_blob_chunk(&path, 0, 1),
            Err(Error::InvalidResponse { .. })
        ));
    }

    #[test]
    fn batched_blob_reads_validate_and_restore_chunk_order() {
        let mut frames = identity_frames("1.5.12");
        frames.extend_from_slice(
            b"dread root\\presets:{\"index\":0,\"chunk\":2,\"value\":\"0203\"}\0\
              dread root\\presets:{\"index\":0,\"chunk\":1,\"value\":\"0001\"}\0",
        );
        let link = Scripted {
            input: Cursor::new(frames),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        let list = BlobList {
            path: NodePath::new("root\\presets").unwrap(),
            description: None,
            size: 4,
            count: 1,
            chunk_size: 2,
            group: None,
            gzip: false,
            movable: true,
            item_type: Some("pst_pst".into()),
            names: vec![Some("Clean".into())],
        };
        assert_eq!(device.read_blob(&list, 0).unwrap(), [0, 1, 2, 3]);
        assert!(device.disconnect().output.ends_with(
            b"dread root\\presets:{\"index\":0,\"chunk\":1}\r\ndread root\\presets:{\"index\":0,\"chunk\":2}\0"
        ));
    }
}
