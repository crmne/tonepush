//! The local library: tones kept on this machine, outliving any preset slot on
//! any pedal.
//!
//! A tone is stored under the hash of its own bytes, in `library/objects`, and
//! everything else points at it by that hash. That one decision settles a
//! handful of problems at once:
//!
//! - **A setlist is frozen by construction.** Editing a tone writes different
//!   bytes, so it writes a different hash; the setlist still names the old one
//!   and still plays exactly what it played. Nothing has to remember a rule.
//! - **Freezing costs nothing.** A setlist that plays a tone the library also
//!   holds is two names for one file, not two copies of it.
//! - **Renaming is free and safe.** The name is a label in the index, so
//!   changing it cannot orphan a setlist the way a file rename would.
//! - **Duplicates cannot happen.** Identical bytes are one object, exactly.
//! - **Deleting is honest.** Taking a tone out of the library forgets it from
//!   the index; the object survives as long as a setlist plays it, and goes to
//!   the trash when nothing does. No `.setlists` folder holding aside what the
//!   library pretends to have deleted.
//!
//! An object is named after its tone, because a folder you cannot read is a
//! folder you cannot trust: `CT-Blackend 8b2446f5.hxpreset`. The short hash on
//! the end is only there to tell two tones of the same name apart. The **full**
//! hash is still the identity, and it is never taken from the file name: the
//! map from hash to file is built by reading the objects and hashing them, so
//! it is always recoverable and cannot be wrong. Rename a file by hand and the
//! library still finds it; edit its bytes and it becomes what its bytes say it
//! is, which is the only honest answer.
//!
//! Objects keep their extension, so every one is an ordinary `.hxpreset` any
//! other program can open, and each is written with a `.hlx` beside it: the
//! device's own bytes for a restore that is exact, Line 6's portable form for
//! everything else that might want to read it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use sha2::{Digest, Sha256};

/// Replace one file only after all of its new contents are safely written.
///
/// Object bytes and the JSON files that point at them are the library's source
/// of truth. Writing either in place would turn a full disk, process kill, or
/// power loss into a valid path containing only part of its document.
pub(crate) fn atomic_write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut file = AtomicWriteFile::open(path)?;
    file.write_all(bytes.as_ref())?;
    file.commit()
}

/// Where kept tones live: `~/.local/share/tonepush/library`, beside the
/// extracted resources. `None` only when there is no home to write into.
pub fn dir() -> Option<PathBuf> {
    hx_catalog::home::library()
}

/// The object store: one file per distinct tone, named after its hash.
fn objects_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("objects"))
}

/// The extensions a tone can arrive as. `.hxpreset` is the device's own
/// document and what everything captured from a pedal is; `.hlx` is Line 6's
/// portable form, welcome here because somebody may well open one and keep it.
const KINDS: [&str; 2] = ["hxpreset", "hlx"];

/// A tone's identity: the SHA-256 of its document bytes, in hex.
///
/// Cryptographic rather than fast, because a collision here would silently
/// serve one person's tone in place of another's. The cost is microseconds on
/// files this size.
pub fn hash_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Enough of a hash to tell tones apart by eye, for a tooltip or a log line.
pub fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// A tone's name, made safe to use as a file name.
///
/// Kept as close to what the pedal shows as a file name can be: a preset name
/// can hold a colon and a slash and a file name cannot, so those become
/// underscores and everything else survives. The name is not the identity, so
/// nothing breaks when two tones sanitise to the same string.
fn readable(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_+&()'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().to_owned();
    // Some names are all punctuation, and some file systems will not take a
    // leading dot. The short hash is a name of last resort.
    if cleaned.is_empty() || cleaned.starts_with('.') {
        String::new()
    } else {
        cleaned.chars().take(80).collect()
    }
}

/// What an object is called on disk: the tone's name, then enough hash to be
/// unique.
fn file_name(name: &str, hash: &str, ext: &str) -> String {
    let readable = readable(name);
    let short = short(hash);
    if readable.is_empty() {
        format!("{short}.{ext}")
    } else {
        format!("{readable} {short}.{ext}")
    }
}

/// Where every object is, by hash.
///
/// Held in memory because the file name no longer answers the question. It is
/// built by reading each object and hashing it, which is the only source of
/// truth there is and is also cheap: a few thousand tones is a few megabytes.
/// Anything that writes to the store keeps this in step rather than dropping
/// it, so the common case never rescans.
/// Which library it was built from is remembered with it: this is one map for
/// the whole process, and a test pointing the library somewhere else would
/// otherwise be answered from the last one's.
static PLACES: std::sync::Mutex<Option<(PathBuf, BTreeMap<String, PathBuf>)>> =
    std::sync::Mutex::new(None);

/// Read every object and remember where it is.
fn rescan() -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let Some(dir) = objects_dir() else {
        return found;
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return found;
    };
    for path in read.flatten().map(|e| e.path()) {
        let is_tone = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| KINDS.iter().any(|k| e.eq_ignore_ascii_case(k)));
        if !is_tone {
            continue;
        }
        // An `.hlx` sitting beside an `.hxpreset` of the same name is that
        // object's portable copy, not an object of its own. It hashes to
        // something different, because it is a different file, and counting it
        // as a tone would make the store look like it held twice what it does.
        if path.extension().is_some_and(|e| e == "hlx") && path.with_extension("hxpreset").is_file()
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        found.insert(hash_of(&bytes), path);
    }
    found
}

/// Where the object with this hash is, if the store holds it.
pub fn object_path(hash: &str) -> Option<PathBuf> {
    let dir = objects_dir()?;
    let mut places = PLACES.lock().unwrap_or_else(|e| e.into_inner());
    if places.as_ref().is_none_or(|(known, _)| *known != dir) {
        *places = Some((dir, rescan()));
    }
    let known = &mut places.as_mut()?.1;
    match known.get(hash) {
        // Remembered, and still where it was. A file somebody moved or deleted
        // behind our back sends us back to the directory rather than reporting
        // a tone that is not there.
        Some(path) if path.is_file() => Some(path.clone()),
        Some(_) => {
            *known = rescan();
            known.get(hash).cloned()
        }
        None => None,
    }
}

/// Forget where everything is, so the next question re-reads the directory.
fn forget_places() {
    *PLACES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Remember one object's place without re-reading the whole directory.
fn remember_place(hash: &str, path: &Path) {
    let mut places = PLACES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((known, places)) = places.as_mut() {
        if objects_dir().as_ref() == Some(known) {
            places.insert(hash.to_owned(), path.to_owned());
        }
    }
}

/// Whether the store holds these bytes already.
pub fn holds(hash: &str) -> bool {
    object_path(hash).is_some()
}

/// What kind of document an object is: the extension it was stored under.
pub fn kind(hash: &str) -> Option<String> {
    object_path(hash)?
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_owned)
}

/// Read a stored tone back.
pub fn read(hash: &str) -> Option<Vec<u8>> {
    std::fs::read(object_path(hash)?).ok()
}

/// Put bytes in the store and answer with their hash.
///
/// Writing the same tone twice is free and idempotent: the second write finds
/// the object already there and does nothing. Storing is deliberately separate
/// from indexing, so a tone whose name still has to be settled can be safely on
/// disk while the question is asked. An object nothing ends up pointing at is
/// swept up by [`collect_garbage`].
pub fn store(name: &str, bytes: &[u8], ext: &str) -> Result<String, String> {
    let dir = objects_dir().ok_or("no home directory to keep tones in")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create the library: {e}"))?;
    let hash = hash_of(bytes);
    if holds(&hash) {
        return Ok(hash);
    }
    let target = dir.join(file_name(name, &hash, ext));
    atomic_write(&target, bytes).map_err(|e| format!("could not keep the tone: {e}"))?;
    remember_place(&hash, &target);
    Ok(hash)
}

/// Call the object on disk what the tone is called now.
///
/// Renaming a tone is a label change and moves nothing that matters, but a
/// folder full of names that went stale a year ago is exactly the folder this
/// was meant not to be.
fn rename_object(hash: &str, name: &str) {
    let Some(old) = object_path(hash) else { return };
    let Some(dir) = old.parent().map(Path::to_path_buf) else {
        return;
    };
    let ext = old
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("hxpreset")
        .to_owned();
    let new = dir.join(file_name(name, hash, &ext));
    if new == old {
        return;
    }
    if std::fs::rename(&old, &new).is_ok() {
        remember_place(hash, &new);
        // The portable copy travels with it, or the pair stops looking like a
        // pair the moment a tone is renamed.
        let companion = old.with_extension("hlx");
        if companion.is_file() {
            let _ = std::fs::rename(&companion, new.with_extension("hlx"));
        }
    }
}

/// Write the portable form of a tone beside the object.
///
/// Both formats, always. The `.hxpreset` is the device's own document and is
/// what goes back on a pedal byte for byte; the `.hlx` is Line 6's own symbolic
/// JSON, which is what HX Edit, CustomTone and anything else can read. Keeping
/// only the first would make the library a place tones can get into and not out
/// of; keeping only the second would lose the snapshots and the routing.
pub fn attach_portable(hash: &str, hlx: &str) -> Result<(), String> {
    let Some(path) = object_path(hash) else {
        return Err("that tone is not in the store".to_owned());
    };
    if path.extension().is_some_and(|e| e == "hlx") {
        // It arrived as one; there is nothing to write beside it.
        return Ok(());
    }
    atomic_write(path.with_extension("hlx"), hlx)
        .map_err(|e| format!("could not write the portable copy: {e}"))
}

/// Whether a tone has its portable copy written yet.
pub fn has_portable(hash: &str) -> bool {
    object_path(hash).is_some_and(|p| {
        p.extension().is_some_and(|e| e == "hlx") || p.with_extension("hlx").is_file()
    })
}

/// Where a tone's portable copy is: the `.hlx`, whether that is the object
/// itself or the file written beside it.
pub fn portable_path(hash: &str) -> Option<PathBuf> {
    let path = object_path(hash)?;
    if path.extension().is_some_and(|e| e == "hlx") {
        return Some(path);
    }
    let beside = path.with_extension("hlx");
    beside.is_file().then_some(beside)
}

/// The hash of a tone's *portable* copy, which is a different number from the
/// tone's own identity and answers a different question.
///
/// A tone is identified here by the bytes the device gave us. TonePush stores
/// the uploaded Tone artifact, which is the `.hlx`. So "is this tone on the
/// site" is asked of the portable copy's hash and never of the object's, and
/// the two must not be confused: every tone in the library has both, and they
/// never match.
pub fn portable_hash(hash: &str) -> Option<String> {
    Some(hash_of(&std::fs::read(portable_path(hash)?).ok()?))
}

/// A local Tone's editable metadata, including the Song it realizes. Song facts
/// (`song`, `artist`, tags, genres and `description`) are kept apart from Tone
/// facts (`name`, `tone_description`, part, guitar and playback character) when
/// they cross the cloud boundary. The chain itself is derived from the preset,
/// never stored here, so it cannot drift from the bytes.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone, PartialEq)]
pub struct Meta {
    /// The tone's name as the pedal shows it - "DIR:USDoubleNrm", colon and
    /// all. This is the label, not the identity: two tones may not share it,
    /// and changing it moves nothing on disk.
    #[serde(default)]
    pub name: String,
    /// The Song's description: the musical idea shared by all its Tones.
    pub description: String,
    /// What is specific to this device-native Tone.
    #[serde(default)]
    pub tone_description: String,
    pub tags: Vec<String>,
    pub character: String, // clean / drive / hi-gain / fuzz / other
    pub genres: Vec<String>,
    pub artist: String,
    pub song: String,
    pub part: String, // rhythm / lead / clean / ...
    pub guitar: String,
    pub pickup_type: String,        // single-coil / humbucker / P90
    pub pickup_electronics: String, // passive / active
    pub tuning: String,
    pub gain: String, // a 1-10 feel, kept free-form for now
    /// When this tone first became part of this computer's library.
    #[serde(default)]
    pub added_at: String,
    /// The most recent preset or metadata change made in this library.
    #[serde(default)]
    pub modified_at: String,
    /// This person's private 1–5 star rating. Zero means unrated.
    #[serde(default)]
    pub rating: u8,
}

/// The library index: which objects the library claims, and what it calls them.
///
/// The objects are the tones; this says which of them a person considers to be
/// *in* their library, and everything they have typed about each. Lose it and
/// no tone is lost, only its name and its notes.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Index {
    /// The on-disk layout this was written by. Present so a future reader knows
    /// what it is looking at; the migration itself keys off what is on disk.
    #[serde(default)]
    version: u32,
    /// One entry per tone the library holds, by hash.
    #[serde(default)]
    tones: BTreeMap<String, Meta>,
    /// Stable local tone identities. Each entry orders the immutable content
    /// hashes that have answered to one tone name and points at the revision
    /// currently shown by the library.
    #[serde(default)]
    series: BTreeMap<String, ToneSeries>,
    /// What libraries before the object store wrote: metadata by file name.
    /// Read once, by the migration, and never written again.
    #[serde(default)]
    entries: BTreeMap<String, Meta>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct ToneSeries {
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    current: String,
}

/// The layout this version writes.
const VERSION: u32 = 3;

fn now() -> String {
    jiff::Timestamp::now().to_string()
}

fn object_modified_at(hash: &str) -> Option<String> {
    let modified = std::fs::metadata(object_path(hash)?)
        .ok()?
        .modified()
        .ok()?;
    jiff::Timestamp::try_from(modified)
        .ok()
        .map(|timestamp| timestamp.to_string())
}

/// Give libraries written before history fields existed an honest, stable
/// approximation: the object's own modification time. It is persisted on the
/// first read, so merely opening a later version never makes an old tone look
/// newly added again.
fn fill_history(hash: &str, meta: &mut Meta) -> bool {
    if !meta.added_at.is_empty() && !meta.modified_at.is_empty() && meta.rating <= 5 {
        return false;
    }
    let fallback = object_modified_at(hash).unwrap_or_else(now);
    if meta.added_at.is_empty() {
        meta.added_at = fallback.clone();
    }
    if meta.modified_at.is_empty() {
        meta.modified_at = fallback;
    }
    meta.rating = meta.rating.min(5);
    true
}

fn index_path() -> Option<PathBuf> {
    dir().map(|d| d.join("index.json"))
}

/// The index as it is on disk, or why it could not be read.
///
/// The difference between "there is no index yet" and "the index is there and
/// unreadable" matters exactly once, and it matters a lot: [`collect_garbage`]
/// decides what nothing points at, and an unreadable index looks identical to
/// an empty one. Reading it as empty would sweep the whole library.
fn load_index() -> Result<Index, String> {
    let Some(path) = index_path() else {
        return Ok(Index::default());
    };
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
        Err(e) => Err(format!("could not read the library index: {e}")),
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("the library index will not parse: {e}")),
    }
}

fn normalise_series(index: &mut Index) -> bool {
    let mut changed = false;

    // Remove interrupted or hand-edited references to objects the index no
    // longer knows, and give each pre-v3 tone a one-revision identity.
    for series in index.series.values_mut() {
        let before = series.versions.len();
        series
            .versions
            .retain(|hash| index.tones.contains_key(hash));
        series.versions.dedup();
        changed |= series.versions.len() != before;
        if !series.versions.contains(&series.current) {
            series.current = series.versions.last().cloned().unwrap_or_default();
            changed = true;
        }
    }
    index.series.retain(|_, series| !series.versions.is_empty());

    let claimed: BTreeSet<String> = index
        .series
        .values()
        .flat_map(|series| series.versions.iter().cloned())
        .collect();
    for hash in index.tones.keys().filter(|hash| !claimed.contains(*hash)) {
        index.series.insert(
            hash.clone(),
            ToneSeries {
                versions: vec![hash.clone()],
                current: hash.clone(),
            },
        );
        changed = true;
    }
    if index.version != VERSION {
        index.version = VERSION;
        changed = true;
    }
    changed
}

fn read_index() -> Index {
    // A broken index must stay broken until it can be recovered by the person
    // who owns it. Treating a read failure as an empty index is useful for the
    // UI (it can still open), but writing that default back would erase the
    // only copy of the metadata and make a later garbage-collection pass
    // believe none of the objects are referenced.
    let Ok(mut index) = load_index() else {
        return Index::default();
    };
    if normalise_series(&mut index) {
        let _ = write_index(&index);
    }
    index
}

/// Read the index for an operation that will change it.
///
/// Mutations may not use the UI's empty fallback: overwriting an unreadable
/// index with one new entry would silently discard every tone that was already
/// recorded there.
fn index_for_update() -> Result<Index, String> {
    let mut index = load_index()?;
    if normalise_series(&mut index) {
        write_index(&index)?;
    }
    Ok(index)
}

fn write_index(index: &Index) -> Result<(), String> {
    let dir = dir().ok_or("no library to save into")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create the library: {e}"))?;
    let json = serde_json::to_vec_pretty(index).map_err(|e| e.to_string())?;
    atomic_write(dir.join("index.json"), json).map_err(|e| e.to_string())
}

/// One tone in the library: what it is, and what it is called.
#[derive(Clone, PartialEq)]
pub struct Entry {
    pub hash: String,
    pub meta: Meta,
    /// Stable identity shared by all revisions of this tone.
    pub series: String,
    /// One-based position of this immutable revision.
    pub version: u32,
    /// How many revisions the tone currently has.
    pub versions: u32,
}

impl Entry {
    /// What to show. A tone whose name was never recorded falls back to its
    /// hash, which is at least unique and at least true.
    pub fn name(&self) -> &str {
        if self.meta.name.is_empty() {
            short(&self.hash)
        } else {
            &self.meta.name
        }
    }
}

/// Every tone the library holds, by name.
pub fn entries() -> Vec<Entry> {
    let mut index = read_index();
    let changed = index.tones.iter_mut().fold(false, |changed, (hash, meta)| {
        fill_history(hash, meta) || changed
    });
    if changed {
        index.version = VERSION;
        let _ = write_index(&index);
    }
    let mut found = Vec::with_capacity(index.series.len());
    for (series_id, series) in &index.series {
        let Some(meta) = index.tones.get(&series.current).cloned() else {
            continue;
        };
        let version = series
            .versions
            .iter()
            .position(|hash| hash == &series.current)
            .map_or(1, |position| position as u32 + 1);
        found.push(Entry {
            hash: series.current.clone(),
            meta,
            series: series_id.clone(),
            version,
            versions: series.versions.len() as u32,
        });
    }
    found.sort_by_key(|e| e.name().to_lowercase());
    found
}

/// Every immutable revision belonging to the same local tone as `hash`.
pub fn versions_of(hash: &str) -> Vec<Entry> {
    let index = read_index();
    let Some((series_id, series)) = index
        .series
        .iter()
        .find(|(_, series)| series.versions.iter().any(|version| version == hash))
    else {
        return Vec::new();
    };
    series
        .versions
        .iter()
        .enumerate()
        .filter_map(|(position, hash)| {
            Some(Entry {
                hash: hash.clone(),
                meta: index.tones.get(hash)?.clone(),
                series: series_id.clone(),
                version: position as u32 + 1,
                versions: series.versions.len() as u32,
            })
        })
        .collect()
}

/// Make an existing revision the one shown by the library. No history is
/// removed; saving another edit continues after the highest revision number.
pub fn make_current(hash: &str) -> Result<(), String> {
    let mut index = index_for_update()?;
    let Some(series) = index
        .series
        .values_mut()
        .find(|series| series.versions.iter().any(|version| version == hash))
    else {
        return Err("that tone revision is no longer in the library".to_owned());
    };
    series.current = hash.to_owned();
    write_index(&index)
}

/// Metadata for one tone.
pub fn meta_of(hash: &str) -> Option<Meta> {
    read_index().tones.get(hash).cloned()
}

/// Every indexed tone's metadata, read in one pass.
///
/// Screens that need to answer the same question for a whole setlist should
/// take one snapshot instead of opening and decoding the index once per row.
pub fn metadata() -> BTreeMap<String, Meta> {
    read_index().tones
}

/// The tone already using this name, if any is.
pub fn named(name: &str) -> Option<Entry> {
    entries()
        .into_iter()
        .find(|entry| entry.meta.name.eq_ignore_ascii_case(name.trim()))
}

/// What keeping a tone ran into. The bytes are in the store either way; what
/// differs is whether the library can go ahead and claim them.
#[derive(Debug, PartialEq)]
pub enum Keeping {
    /// Claimed under the name asked for. Nothing else to decide.
    Kept,
    /// These exact bytes were already in the library, under this name. Keeping
    /// a tone twice is not an error and does not make a second copy.
    Already(String),
    /// A different tone already answers to this name. The bytes are stored and
    /// waiting; the caller asks whether to override or save under another name.
    NameTaken { holder: String },
}

/// Put a tone in the library under a name.
///
/// Names are unique, which is what stops a library filling with six things
/// called "Blackened" that a person then has to open one at a time. A name
/// already in use is not resolved here: the answer belongs to whoever can ask.
pub fn keep(name: &str, ext: &str, bytes: &[u8]) -> Result<(String, Keeping), String> {
    let hash = store(name, bytes, ext)?;
    let mut index = index_for_update()?;
    index.version = VERSION;
    if let Some(known) = index.tones.get(&hash).map(|meta| meta.name.clone()) {
        if let Some(series) = index
            .series
            .values_mut()
            .find(|series| series.versions.iter().any(|version| version == &hash))
        {
            if series.current != hash {
                series.current = hash.clone();
                write_index(&index)?;
            }
        }
        return Ok((hash, Keeping::Already(known)));
    }
    let wanted = name.trim();
    if let Some((_, meta)) = index
        .tones
        .iter()
        .find(|(_, m)| m.name.eq_ignore_ascii_case(wanted))
    {
        return Ok((
            hash,
            Keeping::NameTaken {
                holder: meta.name.clone(),
            },
        ));
    }
    index.tones.insert(
        hash.clone(),
        Meta {
            name: wanted.to_owned(),
            added_at: now(),
            modified_at: now(),
            ..Default::default()
        },
    );
    index.series.insert(
        hash.clone(),
        ToneSeries {
            versions: vec![hash.clone()],
            current: hash.clone(),
        },
    );
    write_index(&index)?;
    Ok((hash, Keeping::Kept))
}

/// Keep a tone, taking the next free name if the one asked for is in use.
///
/// For capturing a whole pedal, where 126 questions is not an answer. Nothing
/// is duplicated by this: identical bytes are still one object under one name,
/// and a number is only ever added when two genuinely different tones want to
/// be called the same thing.
pub fn keep_beside(name: &str, ext: &str, bytes: &[u8]) -> Result<(String, Keeping), String> {
    let (hash, how) = keep(name, ext, bytes)?;
    if !matches!(how, Keeping::NameTaken { .. }) {
        return Ok((hash, how));
    }
    let free = (2..)
        .map(|n| format!("{} {n}", name.trim()))
        .find(|candidate| name_is_free(candidate, &hash))
        .unwrap_or_else(|| short(&hash).to_owned());
    adopt(&hash, &free)?;
    Ok((hash, Keeping::Kept))
}

/// Claim a stored object under a name, keeping any metadata already recorded
/// for it. This is what *Save as* does once a free name has been typed.
pub fn adopt(hash: &str, name: &str) -> Result<(), String> {
    let mut index = index_for_update()?;
    index.version = VERSION;
    let mut meta = index.tones.get(hash).cloned().unwrap_or_default();
    meta.name = name.trim().to_owned();
    fill_history(hash, &mut meta);
    meta.modified_at = now();
    index.tones.insert(hash.to_owned(), meta);
    if !index
        .series
        .values()
        .any(|series| series.versions.iter().any(|version| version == hash))
    {
        index.series.insert(
            hash.to_owned(),
            ToneSeries {
                versions: vec![hash.to_owned()],
                current: hash.to_owned(),
            },
        );
    }
    write_index(&index)?;
    rename_object(hash, name);
    Ok(())
}

/// Add different bytes as the next revision of one tone.
///
/// The old revision remains indexed and can be restored later. Its notes come
/// across to the new revision because this is the same musical tone, edited.
pub fn override_with(old: &str, hash: &str, name: &str) -> Result<(), String> {
    let mut index = index_for_update()?;
    index.version = VERSION;
    let mut meta = index.tones.get(old).cloned().unwrap_or_default();
    meta.name = name.trim().to_owned();
    fill_history(old, &mut meta);
    meta.modified_at = now();
    index.tones.insert(hash.to_owned(), meta);
    let series_id = index
        .series
        .iter()
        .find(|(_, series)| series.versions.iter().any(|version| version == old))
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| old.to_owned());
    let series = index.series.entry(series_id).or_insert_with(|| ToneSeries {
        versions: vec![old.to_owned()],
        current: old.to_owned(),
    });
    if !series.versions.iter().any(|version| version == hash) {
        series.versions.push(hash.to_owned());
    }
    series.current = hash.to_owned();
    write_index(&index)?;
    rename_object(hash, name);
    Ok(())
}

/// Save one tone's metadata, leaving the rest of the index untouched.
pub fn save_meta(hash: &str, meta: &Meta) -> Result<Meta, String> {
    let mut index = index_for_update()?;
    index.version = VERSION;
    let old = index.tones.get(hash).cloned();
    let mut meta = meta.clone();
    if meta.added_at.is_empty() {
        meta.added_at = old
            .as_ref()
            .map(|old| old.added_at.clone())
            .filter(|date| !date.is_empty())
            .or_else(|| object_modified_at(hash))
            .unwrap_or_else(now);
    }
    meta.modified_at = now();
    meta.rating = meta.rating.min(5);
    let renamed = index
        .tones
        .get(hash)
        .is_none_or(|old| old.name != meta.name);
    index.tones.insert(hash.to_owned(), meta.clone());
    // A tone's name belongs to its stable identity, so history stays
    // recognisable after a rename. Other notes remain revision-specific.
    let siblings: Vec<String> = index
        .series
        .values()
        .find(|series| series.versions.iter().any(|version| version == hash))
        .map(|series| series.versions.clone())
        .unwrap_or_default();
    for sibling in &siblings {
        if sibling != hash {
            if let Some(sibling_meta) = index.tones.get_mut(sibling) {
                sibling_meta.name.clone_from(&meta.name);
            }
        }
    }
    write_index(&index)?;
    if renamed && !meta.name.is_empty() {
        for sibling in siblings.iter().chain(std::iter::once(&hash.to_owned())) {
            rename_object(sibling, &meta.name);
        }
    }
    Ok(meta)
}

/// Whether a name is free, ignoring the tone that already holds it.
pub fn name_is_free(name: &str, except: &str) -> bool {
    let wanted = name.trim();
    let index = read_index();
    let except_series = index.series.iter().find_map(|(id, series)| {
        series
            .versions
            .iter()
            .any(|version| version == except)
            .then_some(id)
    });
    !index.series.iter().any(|(id, series)| {
        Some(id) != except_series
            && index
                .tones
                .get(&series.current)
                .is_some_and(|meta| meta.name.eq_ignore_ascii_case(wanted))
    })
}

/// Take a tone out of the library.
///
/// The object is not deleted here. If a setlist still plays it, it stays where
/// it is and the setlist is untouched, which is the whole reason for the object
/// store; if nothing plays it, [`collect_garbage`] moves it to the trash on the
/// way out. Either way the library no longer claims it.
pub fn forget(hash: &str) -> Result<(), String> {
    let mut index = index_for_update()?;
    index.version = VERSION;
    let series_id = index
        .series
        .iter()
        .find(|(_, series)| series.versions.iter().any(|version| version == hash))
        .map(|(id, _)| id.clone());
    if let Some(series_id) = series_id {
        if let Some(series) = index.series.remove(&series_id) {
            for version in series.versions {
                index.tones.remove(&version);
            }
        }
    } else {
        index.tones.remove(hash);
    }
    write_index(&index)?;
    collect_garbage();
    Ok(())
}

/// Every hash anything points at: the library's own tones, and every slot of
/// every setlist. `None` when that cannot be answered with certainty, which is
/// the only safe answer to give something that deletes.
fn referenced() -> Option<BTreeSet<String>> {
    let mut live: BTreeSet<String> = load_index().ok()?.tones.into_keys().collect();
    for (_, setlist) in setlist_files()? {
        for slot in setlist.slots {
            if !slot.hash.is_empty() {
                live.insert(slot.hash);
            }
        }
    }
    Some(live)
}

/// Move objects nothing points at into the trash, and answer with how many.
///
/// Into `.trash`, not out of existence: a slip of the pointer should cost
/// nothing, and an object is small. Runs after anything that can drop the last
/// reference to a tone.
pub fn collect_garbage() -> usize {
    let (Some(dir), Some(objects)) = (dir(), objects_dir()) else {
        return 0;
    };
    if !objects.is_dir() {
        return 0;
    }
    let Some(live) = referenced() else { return 0 };
    let trash = dir.join(".trash");
    let mut swept = 0;
    // What is on disk, by what it actually is. The file name says the tone's
    // name now, so it is no longer something to work a hash out from.
    for (hash, path) in rescan() {
        if live.contains(&hash) {
            continue;
        }
        if std::fs::create_dir_all(&trash).is_err() {
            return swept;
        }
        let name = path.file_name().unwrap_or_default().to_owned();
        if std::fs::rename(&path, trash.join(&name)).is_ok() {
            swept += 1;
            // The portable copy goes with it. Left behind it would be an
            // orphan that looks like a tone the library has and cannot open.
            let companion = path.with_extension("hlx");
            if companion.is_file() {
                let name = companion.file_name().unwrap_or_default().to_owned();
                let _ = std::fs::rename(&companion, trash.join(&name));
            }
        }
    }
    if swept > 0 {
        forget_places();
    }
    swept
}

impl Meta {
    /// This local row's Song and Tone facts as a two-resource web manifest.
    /// The preset file is the Tone artifact and is exported separately.
    pub fn for_the_web(&self, name: &str) -> serde_json::Value {
        let original = self.song.trim().is_empty();
        let mut song = serde_json::Map::new();
        song.insert(
            "title".into(),
            if original { name } else { self.song.trim() }.into(),
        );
        song.insert(
            "kind".into(),
            if original { "original" } else { "song" }.into(),
        );
        song.insert(
            "artist_name".into(),
            if original || self.artist.trim().is_empty() {
                serde_json::Value::Null
            } else {
                self.artist.trim().into()
            },
        );
        if !self.description.trim().is_empty() {
            song.insert("description".into(), self.description.trim().into());
        }
        song.insert("tags".into(), self.tags.clone().into());
        song.insert("genre_ids".into(), serde_json::Value::Array(Vec::new()));

        let mut tone = serde_json::Map::new();
        tone.insert("name".into(), name.into());
        for (field, value) in [
            ("description", &self.tone_description),
            ("part", &self.part),
            ("tuning", &self.tuning),
            ("guitar_type", &self.guitar),
        ] {
            if !value.trim().is_empty() {
                tone.insert(field.into(), value.trim().into());
            }
        }
        if let Some(key) = pickup_type_key(&self.pickup_type) {
            tone.insert("pickup_type".into(), key.into());
        }
        if let Some(key) = pickup_electronics_key(&self.pickup_electronics) {
            tone.insert("pickup_electronics".into(), key.into());
        }
        if let Some(key) = character_key(&self.character) {
            tone.insert("character".into(), key.into());
        }
        serde_json::json!({ "song": song, "tone": tone })
    }
}

/// The site's `pickup_type` keys, from however the field was typed here.
pub fn pickup_type_key(value: &str) -> Option<&'static str> {
    let folded: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match folded.as_str() {
        "singlecoil" | "single" | "sc" => Some("single_coil"),
        "humbucker" | "hb" | "bucker" => Some("humbucker"),
        "p90" | "p90s" => Some("p90"),
        _ => None,
    }
}

/// The site's `pickup_electronics` keys.
pub fn pickup_electronics_key(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "passive" => Some("passive"),
        "active" => Some("active"),
        _ => None,
    }
}

/// The cloud enum key for the local display spelling.
pub fn character_key(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "clean" => Some("clean"),
        "drive" => Some("drive"),
        "hi-gain" | "hi_gain" => Some("hi_gain"),
        "fuzz" | "fuzzy" => Some("fuzzy"),
        "other" => Some("other"),
        _ => None,
    }
}

/// One slot of a setlist.
///
/// The name rides along with the hash so a setlist can be read, listed and
/// reordered without opening the 126 objects it points at - and so a setlist
/// still says what it played even if the object is lost.
#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone, PartialEq)]
pub struct Slot {
    /// The tone this slot plays, by the hash of its bytes. Empty for an empty
    /// slot.
    #[serde(default)]
    pub hash: String,
    pub name: String,
    /// What setlists written before the object store hold: the library file
    /// this slot played. Read so those setlists can be brought over, and
    /// cleared when they are.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}

impl Slot {
    pub fn new(hash: &str, name: &str) -> Slot {
        Slot {
            hash: hash.to_owned(),
            name: name.to_owned(),
            file: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hash.is_empty()
    }
}

/// A setlist: everything a pedal holds, kept on this machine.
///
/// A pedal has room for one at a time - 126 slots on an HX Stomp - and that is
/// a property of the pedal, not of the music. Here a person keeps as many as
/// they have gigs, and puts any of them back with one button. The slots are in
/// the pedal's own order, because that order *is* the setlist: which preset the
/// footswitch reaches next is the whole point.
///
/// It is frozen, and the object store is what freezes it. A slot names bytes,
/// not a tone that might be edited next week, so a setlist plays in March what
/// it played in March.
#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone, PartialEq)]
pub struct Setlist {
    /// Stable identity shared by saved revisions. Older setlists derive this
    /// from their name until their next explicit version save.
    #[serde(default)]
    pub series: String,
    /// One-based revision. Zero is the on-disk spelling of legacy revision 1.
    #[serde(default)]
    pub version: u32,
    /// When this named setlist first entered the library.
    #[serde(default)]
    pub added_at: String,
    /// When this particular revision was captured.
    #[serde(default)]
    pub modified_at: String,
    pub name: String,
    pub description: String,
    pub venue: String,
    /// Free text on purpose: "summer tour", "2026-03-14" and "second set" are
    /// all things people actually write, and none of them is a date picker.
    pub date: String,
    pub slots: Vec<Slot>,
}

impl Setlist {
    pub fn revision(&self) -> u32 {
        self.version.max(1)
    }

    /// How many slots actually hold something.
    pub fn filled(&self) -> usize {
        self.slots.iter().filter(|s| !s.is_empty()).count()
    }

    /// Whether this setlist plays a given tone.
    pub fn plays(&self, hash: &str) -> bool {
        self.slots.iter().any(|s| s.hash == hash)
    }
}

/// Where setlists live, one JSON file each - inspectable, and a single one can
/// be sent to somebody without sending the whole library.
fn setlists_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("setlists"))
}

/// A file name for a setlist, derived from its own name.
fn slug(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = cleaned
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "setlist".to_owned()
    } else {
        collapsed
    }
}

fn setlist_series(setlist: &Setlist) -> String {
    if !setlist.series.is_empty() {
        return setlist.series.clone();
    }
    hash_of(setlist.name.trim().to_ascii_lowercase().as_bytes())
}

fn versioned_setlist_path(dir: &Path, setlist: &Setlist) -> PathBuf {
    let series = setlist_series(setlist);
    dir.join(format!(
        "{}--{}--v{:04}.json",
        slug(&setlist.name),
        short(&series),
        setlist.revision()
    ))
}

/// Every setlist in the library, with the file each came from, by name.
pub fn setlists() -> Vec<(PathBuf, Setlist)> {
    let mut found = setlist_files().unwrap_or_default();
    found.sort_by_key(|(_, s)| (s.name.to_lowercase(), s.revision()));
    found
}

/// Read every setlist or answer that the directory cannot safely be used.
///
/// The public list view may show an empty result on an error. Destructive
/// callers use this distinction: an unreadable setlist can still point at the
/// last copy of a tone, so it must never be silently omitted from migration or
/// garbage collection.
fn setlist_files() -> Option<Vec<(PathBuf, Setlist)>> {
    let Some(dir) = setlists_dir() else {
        return Some(Vec::new());
    };
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut found = Vec::new();
    for entry in read {
        let path = entry.ok()?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            continue;
        }
        let bytes = std::fs::read(&path).ok()?;
        let mut setlist: Setlist = serde_json::from_slice(&bytes).ok()?;
        if setlist.added_at.is_empty() || setlist.modified_at.is_empty() {
            let timestamp = std::fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| jiff::Timestamp::try_from(modified).ok())
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_default();
            if setlist.added_at.is_empty() {
                setlist.added_at = timestamp.clone();
            }
            if setlist.modified_at.is_empty() {
                setlist.modified_at = timestamp;
            }
        }
        found.push((path, setlist));
    }
    Some(found)
}

/// Write a setlist out. An existing file for the same name is replaced, which
/// is what saving one means; renaming makes a new file and leaves the old.
pub fn save_setlist(setlist: &Setlist) -> Result<PathBuf, String> {
    let dir = setlists_dir().ok_or("no home directory to keep setlists in")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create the library: {e}"))?;
    let target = if setlist.version == 0 && setlist.series.is_empty() {
        dir.join(format!("{}.json", slug(&setlist.name)))
    } else {
        versioned_setlist_path(&dir, setlist)
    };
    let json = serde_json::to_vec_pretty(setlist).map_err(|e| e.to_string())?;
    atomic_write(&target, json).map_err(|e| format!("could not save the setlist: {e}"))?;
    Ok(target)
}

/// Capture a new immutable revision under an existing setlist name, or create
/// revision 1 when the typed name is genuinely new.
pub fn save_setlist_version(setlist: &Setlist) -> Result<(PathBuf, Setlist), String> {
    let wanted = setlist.name.trim();
    if wanted.is_empty() {
        return Err("a setlist needs a name".to_owned());
    }
    let existing = setlists()
        .into_iter()
        .filter(|(_, known)| known.name.eq_ignore_ascii_case(wanted))
        .map(|(_, known)| known)
        .max_by_key(Setlist::revision);
    let timestamp = now();
    let mut saved = setlist.clone();
    saved.name = wanted.to_owned();
    if let Some(previous) = existing {
        saved.series = setlist_series(&previous);
        saved.version = previous
            .revision()
            .checked_add(1)
            .ok_or("that setlist has no revision numbers left")?;
        saved.added_at = if previous.added_at.is_empty() {
            previous.modified_at.clone()
        } else {
            previous.added_at.clone()
        };
        if saved.description.is_empty() {
            saved.description = previous.description;
        }
        if saved.venue.is_empty() {
            saved.venue = previous.venue;
        }
        if saved.date.is_empty() {
            saved.date = previous.date;
        }
    } else {
        saved.series = hash_of(format!("{}\0{timestamp}", wanted.to_ascii_lowercase()).as_bytes());
        saved.version = 1;
        saved.added_at = timestamp.clone();
    }
    if saved.added_at.is_empty() {
        saved.added_at = timestamp.clone();
    }
    saved.modified_at = timestamp;
    let path = save_setlist(&saved)?;
    Ok((path, saved))
}

/// Update the notes of one saved revision without creating history on every
/// keystroke. The preset snapshot only changes through
/// [`save_setlist_version`].
pub fn update_setlist(path: &Path, setlist: &Setlist) -> Result<(PathBuf, Setlist), String> {
    let mut saved = setlist.clone();
    saved.modified_at = now();
    if saved.added_at.is_empty() {
        saved.added_at = saved.modified_at.clone();
    }
    let target = save_setlist(&saved)?;
    if target != path && path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| format!("could not finish renaming the setlist: {e}"))?;
    }
    Ok((target, saved))
}

/// Take a setlist out of the library. Tones the library holds stay; objects
/// only this setlist played are swept into the trash.
pub fn remove_setlist(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("could not remove the setlist: {e}"))?;
    collect_garbage();
    Ok(())
}

/// What each object should be called: the library's name for it, or the name a
/// setlist gives it when the library no longer holds it.
fn known_names() -> BTreeMap<String, String> {
    let mut names: BTreeMap<String, String> = read_index()
        .tones
        .into_iter()
        .filter(|(_, meta)| !meta.name.is_empty())
        .map(|(hash, meta)| (hash, meta.name))
        .collect();
    for (_, setlist) in setlists() {
        for slot in setlist.slots {
            if !slot.hash.is_empty() && !slot.name.is_empty() {
                names.entry(slot.hash).or_insert(slot.name);
            }
        }
    }
    names
}

/// Call every object what its tone is called, and answer with how many moved.
///
/// For libraries whose objects were written before the file name said anything.
/// Safe to run at every start: an object already named correctly is a string
/// comparison, and one that is not is a rename. Nothing here can lose a tone,
/// because nothing here decides what a tone *is* - that is still the bytes.
pub fn tidy_names() -> usize {
    let names = known_names();
    let mut moved = 0;
    for (hash, path) in rescan() {
        let Some(name) = names.get(&hash) else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("hxpreset");
        let wanted = file_name(name, &hash, ext);
        if path.file_name().and_then(|n| n.to_str()) == Some(wanted.as_str()) {
            continue;
        }
        rename_object(&hash, name);
        moved += 1;
    }
    moved
}

/// Every object with no portable copy beside it, and what to call it.
///
/// The name comes from the library where the library knows it, and from a
/// setlist slot where the tone is one a setlist plays but the library no longer
/// holds. Both are better than a hash, which is what a tone with no name at all
/// falls back to.
pub fn awaiting_portable() -> Vec<(String, String)> {
    let names = known_names();
    rescan()
        .into_keys()
        .filter(|hash| !has_portable(hash))
        .map(|hash| {
            let name = names
                .get(&hash)
                .cloned()
                .unwrap_or_else(|| short(&hash).to_owned());
            (hash, name)
        })
        .collect()
}

/// Bring a library written before the object store across, and answer with how
/// many tones moved.
///
/// The old shape was one file per tone in `library/`, named after the tone,
/// with metadata in `index.json` keyed by that file name, and tones a setlist
/// still played held aside in a `.setlists/` folder after being deleted. All of
/// it maps onto objects without losing anything: the bytes become objects, the
/// metadata follows the hash, the setlists are repointed, and the tones that
/// were only held aside for a setlist stay exactly that - referenced by the
/// setlist, absent from the library.
///
/// Safe to run at every start. It keys off loose files rather than a version
/// number, so a migration interrupted half way finishes next time, and a
/// library already moved is a directory listing and nothing else.
pub fn migrate() -> usize {
    let Some(dir) = dir() else { return 0 };
    let Some(setlists) = setlist_files() else {
        return 0;
    };
    let loose = loose_tones(&dir);
    let retired = loose_tones(&dir.join(RETIRED));
    // A setlist still naming files is the other thing that needs moving, and it
    // can outlast the files themselves: the old library let a tone be deleted
    // straight out from under a setlist that played it. Those are recoverable
    // from the trash, which is exactly why deletion put them there.
    let stale = setlists
        .iter()
        .any(|(_, s)| s.slots.iter().any(|slot| !slot.file.is_empty()));
    if loose.is_empty() && retired.is_empty() && !stale {
        return 0;
    }

    let Ok(old) = index_for_update() else {
        return 0;
    };
    let mut index = Index {
        version: VERSION,
        tones: old.tones,
        series: old.series,
        entries: BTreeMap::new(),
    };
    // File name to hash, for repointing the setlists afterwards. Retired files
    // were referenced as ".setlists/<name>", so they go in under both.
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    let mut count = 0;

    for (path, claimed) in loose
        .into_iter()
        .map(|p| (p, true))
        .chain(retired.into_iter().map(|p| (p, false)))
    {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("hxpreset")
            .to_ascii_lowercase();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("tone");
        let Ok(hash) = store(stem, &bytes, &ext) else {
            continue;
        };
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !claimed {
            moved.insert(format!(".setlists/{file}"), hash.clone());
            // A retired tone can share a file name with one still in the
            // library. The library's is the one a bare name meant, so it only
            // takes that key if nothing else has.
            moved.entry(file.to_owned()).or_insert(hash);
            continue;
        }
        moved.insert(file.to_owned(), hash.clone());
        // The recorded name wins over the file name, which has had the
        // characters a file name cannot hold taken out of it.
        let mut meta = old.entries.get(file).cloned().unwrap_or_default();
        if meta.name.is_empty() {
            meta.name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("tone")
                .to_owned();
        }
        index.tones.entry(hash).or_insert(meta);
        count += 1;
    }

    // Repoint the setlists before the index is written, so an interruption
    // leaves a library that still migrates cleanly next time.
    for (path, mut setlist) in setlists {
        let mut touched = false;
        for slot in setlist.slots.iter_mut() {
            if slot.file.is_empty() {
                continue;
            }
            match moved.get(&slot.file) {
                Some(hash) => slot.hash = hash.clone(),
                // Not in the library any more. Before tones were stored by
                // content, deleting one took it away from the setlists playing
                // it without asking, so the copy in the trash is the setlist's
                // last link to what it played. Recovering it costs a file read
                // and is the difference between a gig that still loads and one
                // that comes back empty.
                None => {
                    if let Some(hash) = recover(&dir, &slot.file) {
                        slot.hash = hash;
                    }
                }
            }
            slot.file.clear();
            touched = true;
        }
        if touched {
            let json = serde_json::to_vec_pretty(&setlist).unwrap_or_default();
            let _ = atomic_write(&path, json);
        }
    }

    if write_index(&index).is_err() {
        return 0;
    }

    // Only now that every byte is in the store and every pointer moved: take
    // away the originals. A crash before this point costs a re-run, not a tone.
    for name in moved.keys() {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let _ = std::fs::remove_dir(dir.join(".setlists"));
    count
}

/// Where a tone went when it left the library but a setlist still played it.
/// Nothing writes here any more; the object store is what does that job now.
const RETIRED: &str = ".setlists";

/// Find a tone a setlist names but the library no longer has, and put it in the
/// object store. The trash first, then the folder retired tones used to go to.
fn recover(dir: &Path, file: &str) -> Option<String> {
    let name = file.rsplit('/').next()?;
    let found = [dir.join(".trash").join(name), dir.join(RETIRED).join(name)]
        .into_iter()
        .find(|p| p.is_file())?;
    let ext = found
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("hxpreset")
        .to_ascii_lowercase();
    let stem = found
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("tone")
        .to_owned();
    store(&stem, &std::fs::read(found).ok()?, &ext).ok()
}

/// Tone files sitting directly in a directory, which after the migration is
/// only ever a library that has not been moved yet.
fn loose_tones(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| KINDS.iter().any(|k| e.eq_ignore_ascii_case(k)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The library is found through one process-wide environment variable, so
    /// two tests pointing it at two directories at once would each see the
    /// other's. They take turns; the guard is held for the whole test.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Scratch {
        dir: PathBuf,
        // Held, not read: dropping it is what lets the next test run.
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn new(name: &str) -> Scratch {
            // A poisoned lock means some earlier test panicked. That test has
            // already failed; there is no reason for this one to as well.
            let guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "tonepush-library-test-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::env::set_var("TONEPUSH_LIBRARY", &dir);
            Scratch { dir, _guard: guard }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            std::env::remove_var("TONEPUSH_LIBRARY");
        }
    }

    fn objects() -> usize {
        std::fs::read_dir(objects_dir().unwrap())
            .map(|r| r.flatten().count())
            .unwrap_or(0)
    }

    #[test]
    fn a_tone_is_kept_once_however_many_times_it_arrives() {
        let _scratch = Scratch::new("keep");
        let (hash, how) = keep("Blackened", "hxpreset", b"one").unwrap();
        assert_eq!(how, Keeping::Kept);
        assert!(holds(&hash));
        assert_eq!(read(&hash).unwrap(), b"one");

        // The same bytes again: the same tone, said so, and no second object.
        let (again, how) = keep("Blackened", "hxpreset", b"one").unwrap();
        assert_eq!(again, hash);
        assert_eq!(how, Keeping::Already("Blackened".into()));
        assert_eq!(entries().len(), 1);
        assert_eq!(objects(), 1);
    }

    #[test]
    fn a_broken_index_is_never_replaced_or_used_to_collect_tones() {
        let scratch = Scratch::new("broken-index");
        let (hash, _) = keep("Blackened", "hxpreset", b"one").unwrap();
        let index = scratch.dir.join("index.json");
        let broken = b"{ this was truncated";
        std::fs::write(&index, broken).unwrap();

        // Read-only callers may show an empty library, but they must leave the
        // evidence in place for recovery and must not turn it into an empty,
        // valid index that makes every object collectible.
        assert!(entries().is_empty());
        assert_eq!(collect_garbage(), 0);
        assert!(holds(&hash));
        assert_eq!(std::fs::read(&index).unwrap(), broken);

        // Anything that would rewrite the index stops instead. Migration is a
        // startup mutation too, and must leave old-layout files untouched.
        assert!(save_meta(&hash, &Meta::default()).is_err());
        assert!(forget(&hash).is_err());
        let legacy = scratch.dir.join("Legacy.hxpreset");
        std::fs::write(&legacy, b"legacy").unwrap();
        assert_eq!(migrate(), 0);
        assert!(legacy.is_file());
        assert_eq!(std::fs::read(&index).unwrap(), broken);
    }

    /// Different bytes under a name already in use are the whole reason for
    /// Override / Save as: the library does not decide this on its own.
    #[test]
    fn a_taken_name_is_reported_rather_than_resolved() {
        let _scratch = Scratch::new("names");
        let (first, _) = keep("Blackened", "hxpreset", b"one").unwrap();
        let (second, how) = keep("blackened", "hxpreset", b"two").unwrap();
        assert_eq!(
            how,
            Keeping::NameTaken {
                holder: "Blackened".into()
            },
            "the name is matched however it is typed"
        );
        assert_ne!(first, second);
        assert_eq!(entries().len(), 1, "nothing was claimed");
        assert_eq!(objects(), 2, "but the bytes are safe while it is asked");

        // Save as: a free name, and now there are two.
        adopt(&second, "Blackened Lead").unwrap();
        assert_eq!(entries().len(), 2);
        assert!(!name_is_free("blackened lead", &first));
        assert!(name_is_free("Blackened", &first), "except itself");
    }

    /// Saving a new version keeps both immutable revisions, carries the notes
    /// forward, and lets an older one become current without deleting either.
    #[test]
    fn overriding_moves_the_name_and_the_notes() {
        let _scratch = Scratch::new("override");
        let (old, _) = keep("Blackened", "hxpreset", b"one").unwrap();
        save_meta(
            &old,
            &Meta {
                name: "Blackened".into(),
                artist: "Metallica".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let (new, _) = keep("Blackened", "hxpreset", b"two").unwrap();

        override_with(&old, &new, "Blackened").unwrap();
        let held = entries();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].hash, new);
        assert_eq!(held[0].meta.artist, "Metallica", "the notes came across");
        assert!(holds(&old), "the old revision remains available");
        let history = versions_of(&new);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].hash, old);
        assert_eq!(history[1].hash, new);

        make_current(&old).unwrap();
        assert_eq!(entries()[0].hash, old, "history can be restored safely");
        assert_eq!(versions_of(&old).len(), 2, "later work was not removed");
    }

    #[test]
    fn a_local_tone_remembers_history_and_a_personal_rating() {
        let _scratch = Scratch::new("history-rating");
        let (hash, _) = keep("Rated Clean", "hxpreset", b"rated").unwrap();
        let first = entries().pop().unwrap().meta;
        assert!(!first.added_at.is_empty());
        assert!(!first.modified_at.is_empty());

        let mut rated = first.clone();
        rated.rating = 4;
        let saved = save_meta(&hash, &rated).unwrap();
        assert_eq!(saved.rating, 4);
        assert_eq!(saved.added_at, first.added_at);
        assert!(!saved.modified_at.is_empty());
        assert_eq!(meta_of(&hash).unwrap().rating, 4);
    }

    /// The point of the object store: a setlist plays the bytes it was built
    /// from, whatever happens to the library afterwards.
    #[test]
    fn a_setlist_holds_its_tones_against_anything_the_library_does() {
        let _scratch = Scratch::new("frozen");
        let (played, _) = keep("Blackened", "hxpreset", b"march").unwrap();
        let setlist = Setlist {
            name: "Gig".into(),
            slots: vec![Slot::new(&played, "Blackened")],
            ..Default::default()
        };
        save_setlist(&setlist).unwrap();

        // Edit the tone: new bytes, new hash, and Override takes the name.
        let (edited, how) = keep("Blackened", "hxpreset", b"august").unwrap();
        assert_eq!(
            how,
            Keeping::NameTaken {
                holder: "Blackened".into()
            }
        );
        override_with(&played, &edited, "Blackened").unwrap();

        assert_eq!(read(&played).unwrap(), b"march", "the gig is untouched");
        assert_eq!(entries().len(), 1);
        assert_eq!(entries()[0].hash, edited);

        // Delete it from the library outright: the setlist still plays.
        forget(&edited).unwrap();
        assert!(entries().is_empty());
        assert!(holds(&played), "still played, so still here");
        assert!(!holds(&edited), "nothing points at it, so it is swept up");

        // And when the setlist goes, so does the last thing holding it.
        let (path, _) = setlists().pop().unwrap();
        remove_setlist(&path).unwrap();
        assert!(!holds(&played));
    }

    #[test]
    fn a_setlist_saves_and_reads_back() {
        let _scratch = Scratch::new("setlist-roundtrip");
        assert!(setlists().is_empty());

        let saved = Setlist {
            name: "Summer Tour".into(),
            description: "the loud one".into(),
            venue: "Paradiso".into(),
            date: "2026-07-02".into(),
            slots: vec![
                Slot::new("aaa", "Blackened"),
                Slot::default(),
                Slot::new("bbb", "DIR:Relief"),
            ],
            ..Default::default()
        };
        save_setlist(&saved).unwrap();

        let read = setlists();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].1.name, saved.name);
        assert_eq!(read[0].1.description, saved.description);
        assert_eq!(read[0].1.venue, saved.venue);
        assert_eq!(read[0].1.date, saved.date);
        assert_eq!(
            read[0].1.slots, saved.slots,
            "a setlist round-trips unchanged"
        );
        assert!(!read[0].1.added_at.is_empty());
        assert!(!read[0].1.modified_at.is_empty());
        assert_eq!(read[0].1.filled(), 2, "the empty slot is not a preset");
        assert_eq!(read[0].1.slots.len(), 3, "but it still takes up a slot");
    }

    /// Saving the same setlist again replaces it rather than piling up copies -
    /// the pedal has one "Summer Tour" and so does the library.
    #[test]
    fn saving_a_setlist_again_replaces_it() {
        let _scratch = Scratch::new("setlist-replace");
        let mut setlist = Setlist {
            name: "Summer Tour".into(),
            ..Default::default()
        };
        save_setlist(&setlist).unwrap();
        setlist.venue = "Melkweg".into();
        save_setlist(&setlist).unwrap();

        let read = setlists();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].1.venue, "Melkweg");
    }

    #[test]
    fn saving_a_setlist_name_again_creates_an_ordered_revision() {
        let _scratch = Scratch::new("setlist-versions");
        let first = Setlist {
            name: "Summer Tour".into(),
            venue: "Paradiso".into(),
            slots: vec![Slot::new("aaa", "Clean")],
            ..Default::default()
        };
        let (_, first) = save_setlist_version(&first).unwrap();
        let second = Setlist {
            name: "summer tour".into(),
            slots: vec![Slot::new("bbb", "Lead")],
            ..Default::default()
        };
        let (_, second) = save_setlist_version(&second).unwrap();

        assert_eq!(first.revision(), 1);
        assert_eq!(second.revision(), 2);
        assert_eq!(second.series, first.series);
        assert_eq!(second.venue, "Paradiso", "details carry forward");
        assert_eq!(setlists().len(), 2);
        assert_eq!(setlists()[0].1.slots[0].hash, "aaa");
        assert_eq!(setlists()[1].1.slots[0].hash, "bbb");
    }

    #[test]
    fn an_exhausted_setlist_revision_is_reported_without_overflowing() {
        let _scratch = Scratch::new("setlist-version-overflow");
        let setlist = Setlist {
            series: "series".into(),
            version: u32::MAX,
            name: "Very Old Tour".into(),
            ..Default::default()
        };
        save_setlist(&setlist).unwrap();
        let error = save_setlist_version(&setlist).unwrap_err();
        assert!(error.contains("revision numbers"), "{error}");
    }

    #[test]
    fn an_unreadable_setlist_stops_legacy_tones_from_being_retired() {
        let scratch = Scratch::new("migration-broken-setlist");
        let legacy = scratch.dir.join("Only Copy.hxpreset");
        std::fs::write(&legacy, b"the only copy").unwrap();
        std::fs::create_dir_all(scratch.dir.join("setlists")).unwrap();
        let broken = scratch.dir.join("setlists/gig.json");
        std::fs::write(&broken, b"{ this setlist was truncated").unwrap();

        assert_eq!(migrate(), 0);
        assert_eq!(std::fs::read(&legacy).unwrap(), b"the only copy");
        assert_eq!(
            std::fs::read(&broken).unwrap(),
            b"{ this setlist was truncated"
        );
    }

    /// A library from before the object store has to come across whole: the
    /// tones, their names, their notes, and the setlists that play them -
    /// including a tone that had been deleted and held aside for a setlist.
    #[test]
    fn an_older_library_moves_into_the_object_store() {
        let scratch = Scratch::new("migrate");
        let dir = &scratch.dir;
        std::fs::write(dir.join("Blackened.hxpreset"), b"one").unwrap();
        std::fs::write(dir.join("DIR_Relief.hxpreset"), b"two").unwrap();
        std::fs::create_dir_all(dir.join(".setlists")).unwrap();
        std::fs::write(dir.join(".setlists/Gone.hxpreset"), b"three").unwrap();
        std::fs::write(
            dir.join("index.json"),
            serde_json::to_vec(&serde_json::json!({
                "entries": {
                    "DIR_Relief.hxpreset": { "name": "DIR:Relief", "artist": "Nobody",
                        "description": "", "tags": [], "character": "", "genres": [],
                        "song": "", "part": "", "guitar": "", "pickup_type": "",
                        "pickup_electronics": "", "tuning": "", "gain": "" }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("setlists")).unwrap();
        std::fs::write(
            dir.join("setlists/gig.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "Gig", "description": "", "venue": "", "date": "",
                "slots": [
                    { "file": "Blackened.hxpreset", "name": "Blackened" },
                    { "file": ".setlists/Gone.hxpreset", "name": "Gone" }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(migrate(), 2, "two tones were in the library proper");

        let held = entries();
        assert_eq!(held.len(), 2);
        let relief = held.iter().find(|e| e.name() == "DIR:Relief").unwrap();
        assert_eq!(relief.meta.artist, "Nobody", "the notes came across");
        assert_eq!(read(&relief.hash).unwrap(), b"two");
        // The file name was a lookalike; the recorded name is the real one.
        assert!(held.iter().any(|e| e.name() == "Blackened"));

        let (_, setlist) = setlists().pop().unwrap();
        assert!(setlist.slots.iter().all(|s| s.file.is_empty()));
        assert_eq!(setlist.slots[0].hash, hash_of(b"one"));
        assert_eq!(
            setlist.slots[1].hash,
            hash_of(b"three"),
            "a tone held aside for a setlist is still played"
        );
        assert!(holds(&hash_of(b"three")));
        assert!(
            !entries().iter().any(|e| e.hash == hash_of(b"three")),
            "and is still not in the library"
        );

        assert!(
            !dir.join("Blackened.hxpreset").exists(),
            "the originals are gone"
        );
        assert!(!dir.join(".setlists").exists());
        assert_eq!(migrate(), 0, "and running it again finds nothing to do");
    }

    /// The library before this one let a tone be deleted out from under a
    /// setlist that played it: the file went to the trash and the setlist kept
    /// pointing at a name with nothing behind it. Moving across is the moment
    /// to put that right, because the bytes are still in the trash and this is
    /// the last time anything will be looking for them by name.
    #[test]
    fn a_setlist_whose_tones_were_deleted_is_recovered_from_the_trash() {
        let scratch = Scratch::new("recover");
        let dir = &scratch.dir;
        std::fs::create_dir_all(dir.join(".trash")).unwrap();
        std::fs::write(dir.join(".trash/CT-Blackend.hxpreset"), b"the gig").unwrap();
        std::fs::create_dir_all(dir.join("setlists")).unwrap();
        std::fs::write(
            dir.join("setlists/gig.json"),
            serde_json::to_vec(&serde_json::json!({
                "name": "Gig", "description": "", "venue": "", "date": "",
                "slots": [{ "file": "CT-Blackend.hxpreset", "name": "CT-Blackend" }]
            }))
            .unwrap(),
        )
        .unwrap();

        // Nothing loose to move, and still work to do.
        assert_eq!(migrate(), 0, "nothing was in the library to claim");

        let (_, setlist) = setlists().pop().unwrap();
        assert_eq!(setlist.slots[0].hash, hash_of(b"the gig"));
        assert!(setlist.slots[0].file.is_empty());
        assert_eq!(read(&setlist.slots[0].hash).unwrap(), b"the gig");
        assert!(
            entries().is_empty(),
            "recovered for the setlist, not put back in the library"
        );
        // And the object survives a sweep, because the setlist points at it.
        assert_eq!(collect_garbage(), 0);
        assert!(holds(&hash_of(b"the gig")));
    }

    /// A folder you cannot read is a folder you cannot trust. The name on disk
    /// is the tone's, and the hash on the end is only there to tell two apart.
    #[test]
    fn an_object_is_called_what_its_tone_is_called() {
        let _scratch = Scratch::new("readable");
        let (hash, _) = keep("DIR:USDoubleNrm", "hxpreset", b"one").unwrap();
        let path = object_path(&hash).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            name,
            format!("DIR_USDoubleNrm {} .hxpreset", short(&hash)).replace(" .", ".")
        );
        assert!(!name.contains(':'), "a colon cannot reach the filesystem");

        // Renaming the tone renames the file, or the folder goes stale.
        adopt(&hash, "Blackened").unwrap();
        let path = object_path(&hash).unwrap();
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Blackened "));
        assert_eq!(
            read(&hash).unwrap(),
            b"one",
            "and it is still the same tone"
        );

        // Two tones of the same name are told apart by the hash, not by a
        // number nobody could interpret.
        let (other, _) = keep("Blackened", "hxpreset", b"two").unwrap();
        adopt(&other, "Blackened Lead").unwrap();
        assert_ne!(object_path(&hash), object_path(&other));

        // And the identity never comes from the name: rename the file by hand
        // and the library still knows what it is.
        let path = object_path(&hash).unwrap();
        let moved = path.with_file_name("something else entirely.hxpreset");
        std::fs::rename(&path, &moved).unwrap();
        forget_places();
        assert_eq!(read(&hash).unwrap(), b"one");
    }

    /// Both formats, always: the device's own bytes so a restore is exact, and
    /// the portable form so the tone can be read by something that is not us.
    #[test]
    fn a_tone_is_kept_in_both_formats() {
        let _scratch = Scratch::new("both-formats");
        let (hash, _) = keep("Blackened", "hxpreset", b"one").unwrap();
        assert!(!has_portable(&hash), "nothing has written one yet");

        attach_portable(&hash, "{\"tone\": true}").unwrap();
        assert!(has_portable(&hash));
        let beside = object_path(&hash).unwrap().with_extension("hlx");
        assert_eq!(
            std::fs::read_to_string(&beside).unwrap(),
            "{\"tone\": true}"
        );

        // The pair is one tone, not two, and stays a pair through a rename.
        assert_eq!(entries().len(), 1);
        adopt(&hash, "Blackened Lead").unwrap();
        assert!(has_portable(&hash));
        assert!(!beside.is_file(), "the portable copy travelled with it");

        // And when the tone goes, both halves go.
        forget(&hash).unwrap();
        assert!(!holds(&hash));
        let objects = std::fs::read_dir(objects_dir().unwrap())
            .map(|r| r.flatten().count())
            .unwrap_or(0);
        assert_eq!(objects, 0, "no orphan left behind");
    }

    /// Song and Tone facts stay separate in the exported manifest, and local
    /// display spellings become the cloud enum keys.
    #[test]
    fn the_web_manifest_separates_song_and_tone_fields() {
        let meta = Meta {
            pickup_type: "Humbucker".into(),
            pickup_electronics: "Passive".into(),
            song: "Blackened".into(),
            artist: "Metallica".into(),
            tags: vec!["thrash".into()],
            character: "hi-gain".into(),
            tone_description: "For HX Stomp".into(),
            ..Default::default()
        };
        let json = meta.for_the_web("Blackened Rhythm");
        assert_eq!(json["song"]["title"], "Blackened");
        assert_eq!(json["song"]["kind"], "song");
        assert_eq!(json["song"]["artist_name"], "Metallica");
        assert_eq!(json["song"]["tags"][0], "thrash");
        assert_eq!(json["tone"]["name"], "Blackened Rhythm");
        assert_eq!(json["tone"]["description"], "For HX Stomp");
        assert_eq!(json["tone"]["pickup_type"], "humbucker");
        assert_eq!(json["tone"]["pickup_electronics"], "passive");
        assert_eq!(json["tone"]["character"], "hi_gain");

        // No catalog title makes an original Song named after its first Tone.
        let bare = Meta {
            pickup_type: "mystery".into(),
            ..Default::default()
        };
        let json = bare.for_the_web("Untitled");
        assert!(
            json["tone"].get("pickup_type").is_none(),
            "an unmappable pickup is left out"
        );
        assert!(
            json["song"].get("description").is_none(),
            "empty fields are left out"
        );
        assert_eq!(json["song"]["kind"], "original");
        assert_eq!(json["song"]["title"], "Untitled");
        assert!(json["song"]["artist_name"].is_null());
    }

    #[test]
    fn a_setlist_name_becomes_a_usable_file_name() {
        assert_eq!(slug("Summer Tour"), "summer-tour");
        assert_eq!(slug("  Two   Words  "), "two-words");
        assert_eq!(slug("Set #1: The/Loud One"), "set-1-the-loud-one");
        assert_eq!(slug(""), "setlist");
        assert_eq!(slug("///"), "setlist");
    }

    #[test]
    fn a_hash_is_the_sha256_of_the_bytes() {
        // The empty string's SHA-256, so a wrong hash function is caught here
        // rather than by a library that will not open next year.
        assert_eq!(
            hash_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(short(&hash_of(b"")), "e3b0c44298fc");
    }
}
