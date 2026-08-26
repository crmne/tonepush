use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use voidx_proto::blob::{chunk_count, decode_hex};
use voidx_proto::{Command, Frame, NodeDescription, NodePath, NodeTree};

use crate::session::Session;
use crate::{values_equivalent, Error, Link, Notification, Result};

pub const VERIFIED_FIRMWARE: &str = "1.5.12";
const NAME_CHUNK_BYTES: usize = 128;
const READ_BATCH_CHUNKS: usize = 32;

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
}

impl<L: Link> Device<L> {
    pub fn connect(link: L) -> Result<Self> {
        Self::connect_with_timeout(link, Duration::from_secs(5))
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
        Ok(Self {
            session,
            identity,
            write_safety,
            writes_enabled: false,
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
        for first in (1..=chunks).step_by(READ_BATCH_CHUNKS) {
            let last = (first + READ_BATCH_CHUNKS - 1).min(chunks);
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
        self.require_writes()?;
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
        if readback != blob {
            let first = readback
                .iter()
                .zip(blob)
                .position(|(actual, expected)| actual != expected)
                .unwrap_or(readback.len().min(blob.len()));
            return Err(Error::InvalidResponse {
                subject: list.path.to_string(),
                detail: format!("slot {index} readback first differed at byte {first}"),
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
        for &(first, second) in &pairs {
            if let Err(error) = self.swap_slots(list, first, second) {
                let rollback = completed
                    .iter()
                    .rev()
                    .try_for_each(|&(first, second)| self.swap_slots(list, first, second));
                return match rollback {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(Error::WriteRefused(format!(
                        "move failed: {error}; reversing completed swaps also failed: {rollback}"
                    ))),
                };
            }
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

fn assess_identity(identity: &Identity) -> WriteSafety {
    let expected = format!("StompStation PRO / firmware {VERIFIED_FIRMWARE} / CM4 / sspro");
    if identity.name == "StompStation PRO"
        && identity.version == VERIFIED_FIRMWARE
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
            input: Cursor::new(identity_frames(VERIFIED_FIRMWARE)),
            output: vec![],
        };
        let mut device = Device::connect(link).unwrap();
        assert_eq!(device.write_safety(), &WriteSafety::Verified);
        assert!(!device.writes_enabled());
        device.enable_writes().unwrap();
        assert!(device.writes_enabled());

        let link = Scripted {
            input: Cursor::new(identity_frames("1.5.13")),
            output: vec![],
        };
        let mut future = Device::connect(link).unwrap();
        assert!(matches!(
            future.write_safety(),
            WriteSafety::ReadOnly { .. }
        ));
        assert!(future.enable_writes().is_err());
    }

    #[test]
    fn chunk_response_must_echo_its_coordinates() {
        let mut frames = identity_frames(VERIFIED_FIRMWARE);
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
        let mut frames = identity_frames(VERIFIED_FIRMWARE);
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
