//! Reading and writing a Line 6 HX Edit backup bundle (`.hxb`) with no device.
//!
//! An `.hxb` is an `AF6L` container. Its layout, read off four real HX Stomp
//! backups:
//!
//! ```text
//! [0:4]   "AF6L"
//! [4:8]   version (u32, = 1)
//! [8:16]  offset of the block table (u64)
//! [16:24] block count (u64)
//! [24:..] the blocks, packed back to back, each stored as-is
//! [table] one 36-byte entry per block:
//!         tag(4) offset(u64) stored_len(u64) flags(u32) raw_len(u64) reserved(u32)
//! ```
//!
//! Blocks are tagged: `IDXH` (an index carrying the device id and the backup's
//! Unix timestamp), `GLOB` (global settings as JSON), and the setlist payload
//! `SL00` (schema `L6Setlist`, its `data.presets` array holding all 126 slots in
//! front-panel order). Compressed blocks (flags == 1) are a single zlib stream;
//! block integrity rides on zlib's own adler32, so the container carries no
//! checksum of its own - the field that looked like one is the timestamp.
//!
//! Each preset in the setlist is `{ meta, device, tone, device_version }`, and a
//! `.hlx` file is exactly `{ "data": { meta, tone } }` - so a preset lifts out of
//! a backup into a portable tone file with a plain reshape, no device and no
//! lossy round-trip involved.

use std::io::Read;

use serde_json::{json, Value};

use crate::Error;

// ------------------------------------------------------------- presets view ---

/// One preset recovered from a backup.
pub struct BackupPreset {
    /// Zero-based slot: 0 is 01A, 1 is 01B, 3 is 02A (three presets to a bank).
    pub index: usize,
    /// The preset's name. Truly empty slots read as `""`; a never-edited one as
    /// `"New Preset"`.
    pub name: String,
    /// A ready-to-write `.hlx` document: `{ "data": { "meta", "tone" } }`.
    pub hlx: Value,
    /// Whether the slot holds no tone worth keeping - no `meta`, an empty name,
    /// or the factory default `"New Preset"`.
    pub empty: bool,
}

impl BackupPreset {
    /// The front-panel label for this slot, like `03B` - the pedal's own three
    /// presets to a bank, so it matches what the hardware shows.
    pub fn label(&self) -> String {
        hx_proto::rpc::slot_label(self.index as i64)
    }

    /// The `.hlx` document as pretty JSON with a trailing newline, matching how
    /// the rest of the workspace writes JSON to disk.
    pub fn to_hlx_string(&self) -> String {
        serde_json::to_string_pretty(&self.hlx).unwrap_or_default() + "\n"
    }
}

/// A whole backup's presets, in front-panel order.
pub struct Backup {
    /// The setlist's own name, if the bundle carries one.
    pub name: String,
    /// All 126 slots, empty ones included so indices line up with the pedal.
    pub presets: Vec<BackupPreset>,
}

impl Backup {
    /// The slots worth keeping - occupied, user-named presets.
    pub fn occupied(&self) -> impl Iterator<Item = &BackupPreset> {
        self.presets.iter().filter(|p| !p.empty)
    }
}

/// Read an `.hxb` bundle into its presets, touching no hardware.
pub fn read_backup(bytes: &[u8]) -> Result<Backup, Error> {
    let container = Container::parse(bytes)?;
    if container.version != 1 {
        return Err(Error::Backup(format!(
            "unsupported AF6L backup version {}",
            container.version
        )));
    }
    let mut setlists = container
        .blocks
        .iter()
        .filter(|block| is_setlist_tag(&block.tag));
    let block = setlists
        .next()
        .ok_or_else(|| Error::Backup("no setlist block found in this .hxb".into()))?;
    if setlists.next().is_some() {
        return Err(Error::Backup(
            "more than one setlist block found in this .hxb".into(),
        ));
    }
    let raw = block.decompress()?;
    let setlist: Value = serde_json::from_slice(&raw)
        .map_err(|error| Error::Backup(format!("the backup's setlist is not JSON: {error}")))?;

    presets_from(&setlist)
}

fn is_setlist_tag(tag: &[u8; 4]) -> bool {
    (tag[0..2] == *b"SL" && tag[2..].iter().all(u8::is_ascii_digit))
        || (tag[2..] == *b"LS" && tag[..2].iter().all(u8::is_ascii_digit))
}

/// Lift the presets out of a `{data: {meta, presets}}` document, which is what a
/// backup's setlist block and a `.hls` file both hold.
fn presets_from(setlist: &Value) -> Result<Backup, Error> {
    let data = &setlist["data"];
    let name = data
        .get("meta")
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("HX Stomp backup")
        .to_owned();
    let raw = data
        .get("presets")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Backup("the setlist holds no presets".into()))?;

    let presets = raw
        .iter()
        .enumerate()
        .map(|(index, preset)| -> Result<BackupPreset, Error> {
            let preset = preset
                .as_object()
                .ok_or_else(|| Error::Backup(format!("preset slot {index} is not an object")))?;
            let meta = preset.get("meta");
            let name = match meta {
                None => String::new(),
                Some(meta) => meta
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::Backup(format!("preset slot {index} has invalid metadata"))
                    })?
                    .to_owned(),
            };
            let tone = preset.get("tone").cloned().unwrap_or(Value::Null);
            // A slot is worth keeping only if it names a tone someone made.
            let empty = meta.is_none() || name.is_empty() || name == "New Preset";
            if !empty && !tone.is_object() {
                return Err(Error::Backup(format!(
                    "named preset slot {index} holds no tone"
                )));
            }
            let hlx = json!({
                "data": {
                    "meta": meta.cloned().unwrap_or_else(|| json!({ "name": name })),
                    "tone": tone,
                }
            });
            Ok(BackupPreset {
                index,
                name,
                hlx,
                empty,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Backup { name, presets })
}

// --------------------------------------------------------- the raw container ---

const MAGIC: &[u8; 4] = b"AF6L";
const HEADER_LEN: usize = 24;
const ENTRY_LEN: usize = 36;

/// One block of an `.hxb`, kept byte-for-byte so a parsed backup re-encodes
/// identically.
pub struct Block {
    /// Four-character type tag: `IDXH`, `GLOB`, `SL00`, and so on.
    pub tag: [u8; 4],
    /// Whether [`stored`](Self::stored) is a zlib stream (the table's flag == 1).
    pub compressed: bool,
    /// The uncompressed length the table records.
    pub raw_len: u64,
    /// The block's bytes exactly as they sit in the file, compressed if it is.
    pub stored: Vec<u8>,
}

impl Block {
    /// The block's tag as text, for matching and display.
    pub fn tag_str(&self) -> String {
        String::from_utf8_lossy(&self.tag).into_owned()
    }

    /// The block's content, inflating it if it was stored compressed.
    pub fn decompress(&self) -> Result<Vec<u8>, Error> {
        if !self.compressed {
            if self.raw_len != self.stored.len() as u64 {
                return Err(Error::Backup(format!(
                    "a backup block is {} bytes where its table says {}",
                    self.stored.len(),
                    self.raw_len
                )));
            }
            return Ok(self.stored.clone());
        }
        const MAX_BLOCK_LEN: u64 = 64 * 1024 * 1024;
        if self.raw_len > MAX_BLOCK_LEN {
            return Err(Error::Backup(format!(
                "a backup block declares {} decompressed bytes, above the {MAX_BLOCK_LEN}-byte safety limit",
                self.raw_len
            )));
        }
        let out = inflate_exact(&self.stored, self.raw_len, "a backup block")?;
        if out.len() as u64 != self.raw_len {
            return Err(Error::Backup(format!(
                "a backup block inflated to {} bytes where its table says {}",
                out.len(),
                self.raw_len
            )));
        }
        Ok(out)
    }
}

fn inflate_exact(compressed: &[u8], limit: u64, what: &str) -> Result<Vec<u8>, Error> {
    let mut decoder = flate2::bufread::ZlibDecoder::new(compressed);
    let mut out = Vec::new();
    decoder
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|error| Error::Backup(format!("{what} would not inflate: {error}")))?;
    if decoder.total_in() != compressed.len() as u64 {
        return Err(Error::Backup(format!(
            "{what} has trailing data after its zlib stream"
        )));
    }
    Ok(out)
}

/// The raw `AF6L` container beneath [`read_backup`]: its header and blocks kept
/// faithfully, so an `.hxb` can be taken apart and put back together byte for
/// byte - the ground a backup *writer* stands on.
pub struct Container {
    /// Container format version (1 on every HX Stomp backup seen).
    pub version: u32,
    /// The blocks, in file order. The first is `IDXH`, a 24-byte index that
    /// carries the device id and the backup's timestamp; it is preserved like
    /// any other block, so a byte-exact round trip needs nothing special.
    pub blocks: Vec<Block>,
}

impl Container {
    /// Parse an `.hxb`'s container structure, keeping every block's bytes.
    pub fn parse(bytes: &[u8]) -> Result<Container, Error> {
        if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
            return Err(Error::Backup("not an AF6L backup bundle".into()));
        }
        let version = u32le(bytes, 4);
        let table_off = usize::try_from(u64le(bytes, 8))
            .map_err(|_| Error::Backup("backup block table offset is too large".into()))?;
        let count = usize::try_from(u64le(bytes, 16))
            .map_err(|_| Error::Backup("backup block count is too large".into()))?;

        let table_end = count
            .checked_mul(ENTRY_LEN)
            .and_then(|len| table_off.checked_add(len));
        if table_off < HEADER_LEN || table_end != Some(bytes.len()) {
            return Err(Error::Backup(
                "backup block table is outside the canonical file layout".into(),
            ));
        }
        let mut blocks = Vec::with_capacity(count);
        let mut next_block = HEADER_LEN;
        for i in 0..count {
            let e = table_off + i * ENTRY_LEN;
            let tag = [bytes[e], bytes[e + 1], bytes[e + 2], bytes[e + 3]];
            let off = usize::try_from(u64le(bytes, e + 4))
                .map_err(|_| Error::Backup("backup block offset is too large".into()))?;
            let stored_len = usize::try_from(u64le(bytes, e + 12))
                .map_err(|_| Error::Backup("backup block length is too large".into()))?;
            let flags = u32le(bytes, e + 20);
            let raw_len = u64le(bytes, e + 24);
            let reserved = u32le(bytes, e + 32);
            if flags > 1 || reserved != 0 {
                return Err(Error::Backup(
                    "a backup block has unsupported table flags".into(),
                ));
            }
            let Some(end) = off.checked_add(stored_len) else {
                return Err(Error::Backup("a backup block runs past the file".into()));
            };
            if off != next_block || end > table_off {
                return Err(Error::Backup(
                    "backup blocks overlap, have gaps, or run into the table".into(),
                ));
            }
            next_block = end;
            blocks.push(Block {
                tag,
                compressed: flags == 1,
                raw_len,
                stored: bytes[off..end].to_vec(),
            });
        }
        if next_block != table_off {
            return Err(Error::Backup(
                "backup block data does not end at its table".into(),
            ));
        }
        Ok(Container { version, blocks })
    }

    /// Serialise back to an `.hxb`. For a container straight from [`parse`] with
    /// its blocks untouched, this reproduces the input byte for byte.
    ///
    /// [`parse`]: Self::parse
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // table offset, filled below
        out.extend_from_slice(&(self.blocks.len() as u64).to_le_bytes());

        // Blocks pack back to back straight after the header.
        let mut entries = Vec::with_capacity(self.blocks.len());
        for b in &self.blocks {
            let off = out.len() as u64;
            out.extend_from_slice(&b.stored);
            entries.push((b.tag, off, b.stored.len() as u64, b.compressed, b.raw_len));
        }
        let table_off = out.len() as u64;
        for (tag, off, stored_len, compressed, raw_len) in entries {
            out.extend_from_slice(&tag);
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&stored_len.to_le_bytes());
            out.extend_from_slice(&(compressed as u32).to_le_bytes());
            out.extend_from_slice(&raw_len.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        }
        out[8..16].copy_from_slice(&table_off.to_le_bytes());
        out
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Compress a block the way the bundle stores its JSON.
    fn deflate(v: &Value) -> (Vec<u8>, u64) {
        let raw = v.to_string().into_bytes();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        z.write_all(&raw).unwrap();
        (z.finish().unwrap(), raw.len() as u64)
    }

    /// Build a real `.hxb`-shaped container: an `IDXH` index block, then a
    /// `GLOB` globals block, then the `SL00` setlist - the shape a backup takes.
    fn bundle(setlist: &Value) -> Vec<u8> {
        let (glob, glob_raw) = deflate(&json!({ "System": { "tempo": 120 } }));
        let (sl, sl_raw) = deflate(setlist);
        Container {
            version: 1,
            blocks: vec![
                // IDXH: 24 bytes on a real bundle (device id + timestamp); its
                // exact contents do not matter to the reader, only that it round
                // trips.
                Block {
                    tag: *b"IDXH",
                    compressed: false,
                    raw_len: 24,
                    stored: vec![0u8; 24],
                },
                Block {
                    tag: *b"GLOB",
                    compressed: true,
                    raw_len: glob_raw,
                    stored: glob,
                },
                Block {
                    tag: *b"SL00",
                    compressed: true,
                    raw_len: sl_raw,
                    stored: sl,
                },
            ],
        }
        .encode()
    }

    #[test]
    fn container_round_trips_byte_for_byte() {
        let setlist = json!({ "data": { "meta": { "name": "S" }, "presets": [] } });
        let bytes = bundle(&setlist);
        let again = Container::parse(&bytes).expect("parses").encode();
        assert_eq!(bytes, again, "parse then encode must reproduce the bytes");
    }

    #[test]
    fn lifts_presets_out_in_slot_order() {
        let setlist = json!({
            "schema": "L6Setlist",
            "data": {
                "meta": { "name": "My Setlist" },
                "presets": [
                    { "meta": { "name": "CT-Blackend" }, "tone": { "dsp0": { "block0": {} } } },
                    { "meta": { "name": "New Preset" }, "tone": {} },
                    { "tone": {} },
                    { "meta": { "name": "CT-Day CLN" }, "tone": { "dsp0": {} } },
                ]
            }
        });
        let backup = read_backup(&bundle(&setlist)).expect("reads");
        assert_eq!(backup.name, "My Setlist");
        assert_eq!(backup.presets.len(), 4);
        assert_eq!(backup.presets[0].label(), "01A");
        assert_eq!(backup.presets[1].label(), "01B");
        // Three presets to a bank, so slot 3 is 02A, not 01D.
        assert_eq!(backup.presets[3].label(), "02A");

        // Only the named, user-made presets count as occupied.
        let kept: Vec<_> = backup.occupied().map(|p| p.name.as_str()).collect();
        assert_eq!(kept, ["CT-Blackend", "CT-Day CLN"]);

        // The first lifts out as a valid `.hlx`.
        let hlx = &backup.presets[0].hlx;
        assert_eq!(
            hlx.pointer("/data/meta/name").and_then(Value::as_str),
            Some("CT-Blackend")
        );
        assert!(hlx.pointer("/data/tone/dsp0").is_some());
    }

    #[test]
    fn refuses_bytes_that_are_not_a_bundle() {
        assert!(Container::parse(b"not an hxb").is_err());
        assert!(read_backup(&bundle(&json!({ "data": { "nope": 1 } }))).is_err());
    }

    #[test]
    fn corrupt_setlist_blocks_and_preset_records_are_reported() {
        let broken = Container {
            version: 1,
            blocks: vec![Block {
                tag: *b"SL00",
                compressed: true,
                raw_len: 20,
                stored: b"not zlib".to_vec(),
            }],
        }
        .encode();
        let error = read_backup(&broken)
            .err()
            .expect("corrupt block")
            .to_string();
        assert!(error.contains("inflate"), "{error}");

        let named_without_tone = json!({
            "data": { "presets": [{ "meta": { "name": "Looks occupied" } }] }
        });
        let error = read_backup(&bundle(&named_without_tone))
            .err()
            .expect("missing tone")
            .to_string();
        assert!(error.contains("holds no tone"), "{error}");

        let mut future = bundle(&json!({ "data": { "presets": [] } }));
        future[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(read_backup(&future)
            .err()
            .expect("future version")
            .to_string()
            .contains("unsupported"));
    }

    #[test]
    fn ambiguous_duplicate_setlist_blocks_are_rejected() {
        let setlist = json!({ "data": { "presets": [] } });
        let (stored, raw_len) = deflate(&setlist);
        let duplicate = Container {
            version: 1,
            blocks: [*b"SL00", *b"01LS"]
                .into_iter()
                .map(|tag| Block {
                    tag,
                    compressed: true,
                    raw_len,
                    stored: stored.clone(),
                })
                .collect(),
        }
        .encode();

        let error = read_backup(&duplicate)
            .err()
            .expect("duplicate setlist")
            .to_string();
        assert!(error.contains("more than one setlist"), "{error}");
    }

    #[test]
    fn malformed_table_arithmetic_is_an_error_not_a_panic() {
        let mut header = vec![0; HEADER_LEN];
        header[0..4].copy_from_slice(MAGIC);
        header[8..16].copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
        header[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Container::parse(&header).is_err());

        let mut table = vec![0; HEADER_LEN + ENTRY_LEN];
        table[0..4].copy_from_slice(MAGIC);
        table[8..16].copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
        table[16..24].copy_from_slice(&1u64.to_le_bytes());
        table[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(b"TEST");
        table[HEADER_LEN + 4..HEADER_LEN + 12].copy_from_slice(&u64::MAX.to_le_bytes());
        table[HEADER_LEN + 12..HEADER_LEN + 20].copy_from_slice(&2u64.to_le_bytes());
        assert!(Container::parse(&table).is_err());

        table[HEADER_LEN + 4..HEADER_LEN + 20].fill(0);
        table[HEADER_LEN + 20..HEADER_LEN + 24].copy_from_slice(&2u32.to_le_bytes());
        assert!(Container::parse(&table).is_err(), "unknown flags");
    }

    #[test]
    fn overlapping_or_noncanonical_block_ranges_are_rejected() {
        let canonical = Container {
            version: 1,
            blocks: vec![
                Block {
                    tag: *b"ONE!",
                    compressed: false,
                    raw_len: 2,
                    stored: b"12".to_vec(),
                },
                Block {
                    tag: *b"TWO!",
                    compressed: false,
                    raw_len: 2,
                    stored: b"34".to_vec(),
                },
            ],
        }
        .encode();
        assert!(Container::parse(&canonical).is_ok());

        let table = canonical.len() - 2 * ENTRY_LEN;
        let mut overlap = canonical.clone();
        overlap[table + ENTRY_LEN + 4..table + ENTRY_LEN + 12]
            .copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
        assert!(Container::parse(&overlap).is_err());

        let mut trailing = canonical;
        trailing.push(0);
        assert!(Container::parse(&trailing).is_err());
    }

    #[test]
    fn a_blocks_declared_size_is_checked_after_inflation() {
        let (stored, raw_len) = deflate(&json!({ "tone": "clean" }));
        let compressed = Block {
            tag: *b"TEST",
            compressed: true,
            raw_len: raw_len + 1,
            stored,
        };
        assert!(compressed.decompress().is_err());

        let plain = Block {
            tag: *b"TEST",
            compressed: false,
            raw_len: 2,
            stored: vec![0],
        };
        assert!(plain.decompress().is_err());
    }

    #[test]
    fn trailing_data_after_a_compressed_block_is_rejected() {
        let (mut stored, raw_len) = deflate(&json!({ "tone": "clean" }));
        stored.extend_from_slice(b"hidden trailing bytes");
        let block = Block {
            tag: *b"TEST",
            compressed: true,
            raw_len,
            stored,
        };

        let error = block.decompress().unwrap_err().to_string();
        assert!(error.contains("trailing data"), "{error}");
    }

    /// Byte-exact round trip against real HX Stomp backups, when they are on
    /// this machine. Ignored by default - it needs Carmine's backup folder - and
    /// run with `--ignored` to prove the writer against genuine `.hxb` files.
    #[test]
    #[ignore = "needs real .hxb backups on disk"]
    fn round_trips_real_backups() {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .expect("home folder");
        let dir =
            std::path::Path::new(&home).join("Nextcloud/Documents/Line 6/Tones/Helix/Backups");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("backup folder") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("hxb") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let round = Container::parse(&bytes).expect("parses").encode();
            assert_eq!(
                bytes,
                round,
                "{} did not round-trip byte-for-byte",
                path.display()
            );
            // And the presets still lift out.
            let backup = read_backup(&bytes).expect("reads presets");
            assert!(backup.presets.len() >= 100, "expected a full setlist");
            checked += 1;
        }
        assert!(checked > 0, "no .hxb files found to check");
        eprintln!("round-tripped {checked} real backups byte-for-byte");
    }
}

// -------------------------------------------------- the editor's own files ---

/// Read a `.hls` setlist file - HX Edit's "export setlist".
///
/// The wrapper is plain JSON; the presets are base64 of a zlib stream, and the
/// JSON inside is the same `{meta, presets}` the `.hxb` carries in its `SL00`
/// block. So a setlist file and a backup lift apart exactly the same way, and
/// this returns the same [`Backup`].
///
/// The wrapper states the decompressed size and a CRC32 of it; both are checked,
/// because a truncated download that still parses is the failure worth catching.
pub fn read_setlist_file(bytes: &[u8]) -> Result<Backup, Error> {
    let wrapper: Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Backup(format!("not a readable .hls: {e}")))?;
    let encoded = wrapper
        .get("encoded_data")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Backup("the setlist file carries no presets".into()))?;

    let compressed = base64(encoded)
        .ok_or_else(|| Error::Backup("the setlist file's payload is not base64".into()))?;
    const MAX_SETLIST_LEN: u64 = 64 * 1024 * 1024;
    let declared = wrapper
        .pointer("/compression/decompressed_size")
        .and_then(Value::as_u64);
    let limit = declared.unwrap_or(MAX_SETLIST_LEN);
    if limit > MAX_SETLIST_LEN {
        return Err(Error::Backup(format!(
            "the setlist file declares {limit} decompressed bytes, above the {MAX_SETLIST_LEN}-byte safety limit"
        )));
    }
    let raw = inflate_exact(&compressed, limit, "the setlist file")?;
    if raw.len() as u64 > limit {
        return Err(Error::Backup(
            "the setlist file expands beyond its safe declared size".into(),
        ));
    }

    if let Some(expected) = declared {
        if expected != raw.len() as u64 {
            return Err(Error::Backup(format!(
                "the setlist file is {} bytes where it says {expected}; it is truncated",
                raw.len()
            )));
        }
    }
    if let Some(expected) = wrapper
        .pointer("/compression/crc32")
        .and_then(Value::as_u64)
    {
        let expected = u32::try_from(expected)
            .map_err(|_| Error::Backup("the setlist file's checksum is out of range".into()))?;
        let got = crc32(&raw);
        if expected != got {
            return Err(Error::Backup(
                "the setlist file's checksum does not match its contents".into(),
            ));
        }
    }

    let setlist: Value = serde_json::from_slice(&raw)
        .map_err(|e| Error::Backup(format!("the setlist file's presets are not JSON: {e}")))?;
    // A `.hxb` wraps this in `data`; a `.hls` does not. Reuse the one reader.
    presets_from(&json!({ "data": setlist }))
}

/// One block kept as a favourite, read from a `.fav` file.
pub struct Favourite {
    pub name: String,
    /// The block, and the cab riding with it if it is an amp: `slot0` and
    /// `slot1` in the file, in the same shape a `.hlx` writes a block.
    pub slots: Vec<Value>,
}

/// Read a `.fav` favourite file - HX Edit's "export favourite".
///
/// Plain JSON, uncompressed: a name and the block itself, an amp bringing its
/// cab along as a second slot.
pub fn read_favourite_file(bytes: &[u8]) -> Result<Favourite, Error> {
    let file: Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Backup(format!("not a readable .fav: {e}")))?;
    let name = file
        .pointer("/data/meta/name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::Backup("the favourite file has no readable name".into()))?
        .to_owned();
    let slots = file
        .pointer("/data/favorite")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Backup("the favourite file holds no block".into()))?;

    // slot0, slot1, … in order, so an amp keeps its cab behind it.
    let mut numbered = Vec::with_capacity(slots.len());
    for (key, value) in slots {
        let number = key
            .strip_prefix("slot")
            .and_then(|number| number.parse::<u32>().ok())
            .ok_or_else(|| Error::Backup(format!("invalid favourite slot key {key:?}")))?;
        if value
            .get("@model")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(Error::Backup(format!(
                "favourite slot {number} has no model"
            )));
        }
        numbered.push((number, value.clone()));
    }
    numbered.sort_by_key(|(n, _)| *n);
    if numbered.is_empty()
        || numbered
            .iter()
            .enumerate()
            .any(|(expected, (found, _))| usize::try_from(*found) != Ok(expected))
    {
        return Err(Error::Backup(
            "the favourite file's slots are empty or not contiguous from slot0".into(),
        ));
    }

    Ok(Favourite {
        name,
        slots: numbered.into_iter().map(|(_, v)| v).collect(),
    })
}

/// Decode standard base64, ignoring the whitespace a pretty-printed file wraps
/// its payload in.
fn base64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let compact: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !compact.len().is_multiple_of(4) {
        return None;
    }
    let value = |byte| {
        ALPHABET
            .iter()
            .position(|candidate| *candidate == byte)
            .map(|n| n as u8)
    };
    let mut out = Vec::with_capacity(compact.len() / 4 * 3);
    let chunks = compact.len() / 4;
    for (index, chunk) in compact.chunks_exact(4).enumerate() {
        let (a, b) = (value(chunk[0])?, value(chunk[1])?);
        let last = index + 1 == chunks;
        match (chunk[2], chunk[3]) {
            (b'=', b'=') if last && b & 0x0f == 0 => {
                out.push((a << 2) | (b >> 4));
            }
            (c, b'=') if last => {
                let c = value(c)?;
                if c & 0x03 != 0 {
                    return None;
                }
                out.push((a << 2) | (b >> 4));
                out.push((b << 4) | (c >> 2));
            }
            (c, d) => {
                let (c, d) = (value(c)?, value(d)?);
                out.push((a << 2) | (b >> 4));
                out.push((b << 4) | (c >> 2));
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}

/// CRC32, the ordinary one, to check what a `.hls` says about its own payload.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod editor_file_tests {
    use super::*;

    fn capture(name: &str) -> Option<Vec<u8>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../captures/library-exports")
            .join(name);
        std::fs::read(path).ok()
    }

    /// A `.hls` setlist file lifts apart the same way a backup does.
    #[test]
    fn a_setlist_file_reads_as_a_backup() {
        let Some(bytes) = capture("HX Stomp.hls") else {
            return;
        };
        let setlist = read_setlist_file(&bytes).expect("reads");
        assert_eq!(setlist.name, "HX Stomp");
        assert_eq!(setlist.presets.len(), 126, "a full setlist");
        assert!(
            setlist.occupied().count() > 50,
            "and most of them hold a tone"
        );

        // The first slot lifts out as a `.hlx` with a tone in it.
        let first = &setlist.presets[0];
        assert_eq!(first.label(), "01A");
        assert!(first.hlx.pointer("/data/tone/dsp0").is_some());
    }

    /// The wrapper states its own payload's size and checksum, and a file that
    /// disagrees with itself is refused rather than half-read.
    #[test]
    fn a_truncated_setlist_file_is_refused() {
        let Some(bytes) = capture("HX Stomp.hls") else {
            return;
        };
        let text = String::from_utf8(bytes).expect("json");
        // Drop a chunk out of the middle of the payload.
        let cut = text.replacen("eNrs", "eNr", 1);
        assert!(read_setlist_file(cut.as_bytes()).is_err());
    }

    /// A `.fav` holds one block, and an amp brings its cab.
    #[test]
    fn a_favourite_file_reads_its_block() {
        let Some(bytes) = capture("favtest.fav") else {
            return;
        };
        let favourite = read_favourite_file(&bytes).expect("reads");
        assert_eq!(favourite.name, "favtest");
        assert_eq!(favourite.slots.len(), 2, "the amp and its cab");
        assert_eq!(
            favourite.slots[0]["@model"].as_str().unwrap(),
            "HD2_AmpUSDeluxeNrm"
        );
        assert!(favourite.slots[1]["@model"]
            .as_str()
            .unwrap()
            .contains("Cab"));
    }

    #[test]
    fn malformed_favourite_slots_are_not_silently_dropped_or_reordered() {
        let favourite = |slots| {
            serde_json::to_vec(&json!({
                "data": {
                    "meta": { "name": "test" },
                    "favorite": slots,
                }
            }))
            .unwrap()
        };

        let misspelled = favourite(json!({
            "slot0": { "@model": "HD2_DistScream808" },
            "slto1": { "@model": "HD2_Cab1x12USDeluxe" },
        }));
        assert!(read_favourite_file(&misspelled).is_err());

        let gap = favourite(json!({
            "slot1": { "@model": "HD2_Cab1x12USDeluxe" },
        }));
        assert!(read_favourite_file(&gap).is_err());

        let missing_model = favourite(json!({ "slot0": {} }));
        assert!(read_favourite_file(&missing_model).is_err());
    }

    #[test]
    fn base64_decodes_what_the_editor_writes() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        // Whitespace is ignored, the way a pretty-printed payload carries it.
        assert_eq!(base64("aGVs\n bG8=").unwrap(), b"hello");
        for malformed in ["a", "a===", "aGV=sbG8", "aGVsbG8=garbage", "ab=="] {
            assert!(base64(malformed).is_none(), "accepted {malformed:?}");
        }
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926, "the standard check value");
    }
}

// ----------------------------------------------------------- writing one ---

/// What goes into a backup bundle.
pub struct NewBackup<'a> {
    /// The setlist's name, e.g. "PRESETS".
    pub setlist: &'a str,
    /// Every slot in front-panel order: the preset's name and its tone as the
    /// symbolic JSON [`to_hlx`](crate::to_hlx) writes, or `None` for an empty
    /// slot.
    pub presets: &'a [(String, Option<Value>)],
    /// The device's global settings, as HX Edit's `GLOB` block holds them.
    pub globals: Value,
    /// The device id, e.g. `0x00210006` for an HX Stomp.
    pub device: u32,
    /// The firmware version word, as the device reports it.
    pub device_version: u32,
    /// When the backup was taken, seconds since the epoch.
    pub captured: u32,
}

/// Build an HX Edit `.hxb` backup bundle.
///
/// The container is exact - it round-trips four real backups byte for byte -
/// and the presets are written by the same converter that agrees with HX Edit's
/// own output on all 94 presets it was checked against.
///
/// One block is deliberately absent. A real backup carries `SDMU`, an archive of
/// 980 model descriptors that is HX Edit's own catalog cache rather than
/// anything about this pedal's presets, and inventing one would be inventing
/// data. **Whether HX Edit accepts a bundle without it is untested** - it needs
/// a machine with HX Edit on it to find out. TonePush's own restore does not
/// go through this format: it uses the pedal's own bytes, which cannot lose
/// anything a conversion might.
pub fn write_backup(new: &NewBackup) -> Vec<u8> {
    let presets: Vec<Value> = new
        .presets
        .iter()
        .map(|(name, tone)| match tone {
            Some(tone) => json!({
                "meta": { "name": name },
                "device": new.device,
                "device_version": new.device_version,
                "tone": tone,
            }),
            // An empty slot carries no meta at all, which is how a backup says
            // there is nothing there.
            None => json!({ "device": new.device, "device_version": new.device_version }),
        })
        .collect();

    let setlist = json!({
        "version": 2,
        "schema": "L6Setlist",
        "meta": { "name": new.setlist },
        "data": { "meta": { "name": new.setlist }, "presets": presets },
    });

    // IDXH: device id, firmware, then the timestamp sixteen bytes in.
    let mut index = Vec::with_capacity(24);
    index.extend_from_slice(&new.device.to_le_bytes());
    index.extend_from_slice(&new.device_version.to_le_bytes());
    index.extend_from_slice(&[0u8; 8]);
    index.extend_from_slice(&new.captured.to_le_bytes());
    index.extend_from_slice(&[0u8; 4]);

    let mut name = new.setlist.as_bytes().to_vec();
    name.push(0);

    Container {
        version: 1,
        blocks: vec![
            Block {
                tag: *b"IDXH",
                compressed: false,
                raw_len: 24,
                stored: index,
            },
            deflated(*b"BOLG", &new.globals),
            Block {
                tag: *b"MNLS",
                compressed: false,
                raw_len: name.len() as u64,
                stored: name,
            },
            deflated(*b"00LS", &setlist),
        ],
    }
    .encode()
}

/// A block holding zlib-compressed JSON, the way the bundle stores one.
fn deflated(tag: [u8; 4], value: &Value) -> Block {
    use std::io::Write;
    let raw = serde_json::to_vec(value).unwrap_or_default();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&raw);
    Block {
        tag,
        compressed: true,
        raw_len: raw.len() as u64,
        stored: z.finish().unwrap_or_default(),
    }
}

#[cfg(test)]
mod writing_tests {
    use super::*;

    /// What is written reads back as the same presets, through the same reader
    /// that reads HX Edit's own backups.
    #[test]
    fn a_written_backup_reads_back() {
        let presets = vec![
            (
                "CT-Blackend".to_owned(),
                Some(json!({ "dsp0": { "block0": { "@model": "HD2_DistScream808" } } })),
            ),
            ("New Preset".to_owned(), None),
            ("Soundgarden".to_owned(), Some(json!({ "dsp0": {} }))),
        ];
        let bytes = write_backup(&NewBackup {
            setlist: "PRESETS",
            presets: &presets,
            globals: json!({ "System": { "tempo": 120 } }),
            device: 0x0021_0006,
            device_version: 0x0380_0000,
            captured: 1_786_308_507,
        });

        let back = read_backup(&bytes).expect("reads back");
        assert_eq!(back.name, "PRESETS");
        assert_eq!(back.presets.len(), 3);
        assert_eq!(
            back.occupied().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["CT-Blackend", "Soundgarden"],
            "the empty slot stays empty and the named ones survive"
        );
        assert_eq!(
            back.presets[0]
                .hlx
                .pointer("/data/tone/dsp0/block0/@model")
                .and_then(Value::as_str),
            Some("HD2_DistScream808")
        );

        // And the container itself is the shape a real backup has.
        let container = Container::parse(&bytes).expect("parses");
        let tags: Vec<String> = container.blocks.iter().map(|b| b.tag_str()).collect();
        assert_eq!(tags, ["IDXH", "BOLG", "MNLS", "00LS"]);
        assert_eq!(container.blocks[0].stored.len(), 24, "the index block");
        assert_eq!(&container.blocks[2].stored, b"PRESETS\0");
    }
}
