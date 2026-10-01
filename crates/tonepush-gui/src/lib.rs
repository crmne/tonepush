//! A cross-platform editor for supported guitar processors. HX devices retain
//! their familiar signal-chain editor; the StompStation PRO gets a native,
//! schema-driven VoidX editor and library manager.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use egui::RichText;
use hx_catalog::{Catalog, Kind};

/// Public so the desktop entry point can bring an older library across before
/// the first window opens. Nothing else here needs to be.
mod backups;
mod board;
mod browser;
pub mod cloud;
mod config;
mod eq;
mod floor;
pub mod library;
mod library_view;
mod pages;
mod pane;
mod pro;
#[cfg(test)]
mod screenshots;
mod session;
mod shell;
mod table;
mod theme;
pub mod update;
mod wav;

pub use session::{spawn, spawn_repainting, ApplyBlock, Cmd, Evt};

/// A tone file opened for a look before anything touches the pedal. It is
/// drawn by the same renderer as the loaded chain - one renderer, and the
/// preview simply runs it in display mode - and it carries what Load needs.
struct Preview {
    name: String,
    /// "Full rig, for FRFR or a PA" - what the tone is, in one line.
    line: String,
    chain: Vec<session::Block>,
    layout: hx_proto::preset::Layout,
    skipped: Vec<String>,
    load: LoadKind,
    /// Which preset Load writes into. Defaults to the one open now.
    dest: i64,
    /// The tone exactly as it arrived, so Keep has something to store: the
    /// bytes and the kind of document they are. A path would not do - a
    /// preview can come from the library, where there is no file to point at
    /// that a person would recognise.
    source: (String, Vec<u8>),
}

/// How a previewed tone reaches the pedal: a byte-exact document is written
/// whole; a symbolic tone clears the chain and builds it back block by block.
enum LoadKind {
    Document(Vec<u8>),
    Steps(Vec<session::ApplyBlock>),
}

/// One row in the library browser: which tone it is (the hash of its bytes,
/// which is its identity), what it is called, a one-line reading of the chain
/// derived from the document, and its saved metadata.
#[derive(Clone)]
struct LibEntry {
    hash: String,
    /// Stable local identity shared by every immutable content revision.
    series: String,
    name: String,
    line: String,
    meta: library::Meta,
    added_at: String,
    modified_at: String,
    downloads: Option<u64>,
    rating: Option<f32>,
    version: u32,
    versions: u32,
    /// The chain as a strip of category colours, read once from the document.
    chain: Vec<shell::Mini>,
    /// Whether it is a StompStation PRO tone rather than an HX one.
    pro: bool,
}

/// The small, read-only view of the on-disk library that frame rendering
/// needs. It is rebuilt when the library changes; drawing never opens files.
#[derive(Default)]
struct LibraryLookup {
    /// Indexed tones whose object is still present.
    indexed: std::collections::HashSet<String>,
    /// Current tone names, normalised for the pedal-side name comparison.
    names: std::collections::HashSet<String>,
    tags: Vec<String>,
    /// Facts for every tone named by a setlist, including historical versions.
    setlist_tones: std::collections::HashMap<String, SetlistTone>,
}

struct SetlistTone {
    held: bool,
    meta: library::Meta,
}

impl LibraryLookup {
    /// Keep the browse and sync indexes aligned with the editable rows.
    fn reindex(&mut self, entries: &[LibEntry]) {
        self.names = entries
            .iter()
            .map(|entry| {
                if entry.meta.name.is_empty() {
                    library::short(&entry.hash).to_ascii_lowercase()
                } else {
                    entry.meta.name.trim().to_ascii_lowercase()
                }
            })
            .collect();
        self.tags = entries
            .iter()
            .flat_map(|entry| entry.meta.tags.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        for entry in entries {
            if let Some(tone) = self.setlist_tones.get_mut(&entry.hash) {
                tone.meta = entry.meta.clone();
            }
        }
    }

    fn sync(&self, hash: &str, name: &str) -> theme::Sync {
        if self.indexed.contains(hash) {
            theme::Sync::Same
        } else if !name.is_empty() && self.names.contains(&name.to_ascii_lowercase()) {
            theme::Sync::Differs
        } else {
            theme::Sync::Absent
        }
    }
}

/// A sign-in waiting on somebody, somewhere else.
///
/// The code is on screen so it can be checked against the page, and the URL is
/// kept so it can be said again: the browser that opened may not be the one
/// they are signed in to, and the mail may be read on a phone entirely.
struct Signing {
    code: String,
    url: String,
    answer: std::sync::mpsc::Receiver<cloud::Linked>,
}

/// A library tone on its way to the web, kept with its row identity so that
/// the cloud itself can show the work instead of leaving the click unanswered.
struct PublishingJob {
    hash: String,
    name: String,
    answer: std::sync::mpsc::Receiver<Result<cloud::ToneDetails, cloud::PublishError>>,
}

/// A TonePush query off the UI thread. Keeping its query beside the receiver
/// makes late answers harmless when somebody types a second search quickly.
struct CloudSearchJob {
    query: String,
    order: cloud::DiscoveryOrder,
    /// Product name used for compatibility; empty means no device is known.
    device: String,
    page: u32,
    answer: std::sync::mpsc::Receiver<Result<cloud::DiscoveryPage, String>>,
}

/// A public Tone and the local-table-shaped row derived from it once, when the
/// page arrives. Discovery results are immutable; rebuilding this on every
/// frame only burns strings and JSON walks.
struct CloudEntry {
    discovered: cloud::DiscoveredTone,
    row: LibEntry,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloudAction {
    Audition,
    Computer,
}

/// One cloud artifact in flight and what the click meant to do with it once it
/// arrives. The whole row travels with the job so a new search cannot make an
/// old download act on a different row at the same index.
struct CloudDownloadJob {
    entry: cloud::DiscoveredTone,
    action: CloudAction,
    answer: std::sync::mpsc::Receiver<Result<cloud::ToneDelivery, String>>,
}

/// A tone kept under a name another tone already answers to: the bytes (in the
/// store already, so the question can be asked safely), who holds the name, and
/// the name being typed in case the answer is *Save as*.
struct NameClash {
    hash: String,
    holder: String,
    draft: String,
    /// Published details to attach if this clash came from the Cloud tab.
    cloud_meta: Option<library::Meta>,
    /// Keep a cloud save in the discovery workflow after the question is
    /// answered; ordinary imports continue to land in local Tones.
    return_view: Option<LibraryView>,
}

/// A captured pedal waiting for its setlist identity. Typing filters existing
/// names; choosing one saves another revision, while an unmatched name starts
/// a new setlist.
struct SetlistSave {
    setlist: library::Setlist,
    draft: String,
    failures: usize,
}

/// A tone on its way to the pedal, waiting for a slot to be picked.
///
/// One action rather than two. An "into the first empty slot" button and a
/// "choose a slot" button ask a person to decide something before they can see
/// what they are deciding between; this puts the list itself into a picking
/// state, where every row says what it holds and therefore what it would cost.
struct Sending {
    hash: String,
    name: String,
}

/// The three ways out of that question.
enum Clash {
    Override,
    SaveAs(String),
    Cancel,
}

pub struct App {
    to_device: Sender<Cmd>,
    from_device: Receiver<Evt>,
    pro: pro::Panel,

    catalog: Option<Catalog>,
    connection: Connection,

    device: String,
    firmware: String,
    presets: Vec<String>,
    preset_count: u16,
    preset_index: i64,
    preset_name: String,

    tempo: Option<f32>,
    snapshots: Vec<String>,
    chain: Vec<session::Block>,
    layout: hx_proto::preset::Layout,
    /// Whether a block has been copied, so Paste has something to put down.
    /// The block itself is held by the worker, whole, so it pastes as it was
    /// copied into this preset or another; remembering only the slot pasted
    /// whatever had come to be there.
    copied_block: bool,
    /// A copied preset: its name, and the document verbatim. Held in the app
    /// rather than the system clipboard because it is binary, and because
    /// pasting it into a text field would only produce noise.
    clipboard: Option<(String, Vec<u8>)>,
    /// Where the bytes should go once `Cmd::CopyPreset` answers.
    pending_copy: CopyTarget,
    /// Which page the window shows: the loaded preset, the library, or the
    /// pedal itself.
    page: shell::Page,
    /// Which tab of the HX's page is open.
    pedal_tab: pages::PedalTab,
    /// Whether the sidebar is put away (Ctrl+B), giving the page the width.
    sidebar_hidden: bool,
    /// What the automatic backup holds and when it was read, for the
    /// sidebar's foot and the Pedal page. Read with the mirror, from disk.
    automatic_backup: Option<hx_usb::backup::Manifest>,
    /// When the EQ last wrote to the device. Dragging a band across the curve
    /// is sixty changes a second and the pedal does not want sixty writes; it
    /// wants to be heard moving, so the writes are paced rather than deferred.
    eq_wrote_at: Option<std::time::Instant>,
    /// The device's global EQ switch, as last read.
    global_eq: bool,
    /// How many steps the worker can undo and redo, for enabling the buttons.
    undo_depth: usize,
    redo_depth: usize,
    /// Set while the device is fetching a preset. Loading one takes about a
    /// second, and a window that does not change for a second looks broken.
    loading: bool,
    /// When the worker started its current device conversation, if it is in
    /// one. Edits take real round trips; past a moment, the title bar says
    /// so with a spinner rather than letting the window look stuck.
    busy_since: Option<std::time::Instant>,
    /// A resource extraction running in the background, and the last thing
    /// worth telling the user about one. Extracting reads a whole installer,
    /// which is far too slow for the UI thread.
    extracting: Option<std::sync::mpsc::Receiver<Result<usize, String>>>,
    onboarding_status: Option<String>,
    /// Whether the welcome window is up. It opens on first launch without
    /// the model data and closes itself the moment extraction succeeds. It
    /// does not dismiss: without the model data there are no names, no knob
    /// ranges, and no pictures, so there is nothing honest to dismiss *to*.
    show_onboarding: bool,
    /// When taps were registered, for working out a tapped tempo.
    taps: Vec<std::time::Instant>,
    /// The slot being dragged along the chain.
    dragging: Option<usize>,
    /// A fork or merge being dragged along the main line: its slot, and
    /// whether it is the split.
    dragging_junction: Option<(usize, bool)>,
    /// Where each gap in the chain sits this frame, by the slot it inserts
    /// before - where a dragged block or junction can land. Rebuilt every
    /// frame; resolving a drop from a stale frame moved blocks nobody asked
    /// to move.
    gap_rects: Vec<(usize, egui::Rect)>,
    /// Where each block sits this frame, for dropping one onto another.
    block_rects: Vec<(usize, egui::Rect)>,
    /// The offered branch this frame - the slot a drop on it takes, and
    /// where it was drawn. Dragging a block onto the ghost is how a hand
    /// says "run this one in parallel".
    ghost_target: Option<(usize, egui::Rect)>,
    /// Whether the edit buffer has changes the preset does not.
    ///
    /// The device edits a scratch copy: a changed parameter is audible at once
    /// but vanishes on reload unless it is saved. An editor that does not say
    /// so loses people's work quietly, so this drives a dot in the title.
    dirty: bool,
    selected: usize,
    /// What the pane under the board shows: the block, the footswitches or
    /// the snapshots.
    lens: pane::Lens,
    /// The model browser, while it is open in the pane.
    browser: Option<browser::Browser>,
    /// The category last looked at in the browser, where adding a block
    /// opens it next.
    browsed_category: Option<u32>,
    /// The footswitch, pedal or MIDI the Footswitches lens is editing.
    focused_source: Option<hx_proto::rpc::Source>,
    /// Every snapshot of the loaded preset in full, for the snapshot matrix.
    snapshot_details: Vec<hx_proto::preset::Snapshot>,
    /// A slot to select once the preset next arrives: a block just added.
    pending_select: Option<i64>,
    /// Set when something is sent while a model is being tried: the worker
    /// keeps the try, and the browser closes on its next frame.
    trial_kept: std::cell::Cell<bool>,
    /// Scroll the preset list to the selection on the next frame - set when a
    /// different preset loads, so following along from the pedal's own
    /// front panel keeps the list in view without fighting manual scrolling.
    reveal_preset: bool,

    irs: Vec<(i64, String)>,
    setlists: Vec<String>,
    /// Which setlist the preset list is showing. Only reachable through the
    /// picker, which appears when a device has more than one - an HX Stomp has
    /// a single list, so on that hardware this stays at zero.
    setlist: i64,

    /// Favorites and other settings that persist across runs.
    config: config::Config,
    /// Show only favorited presets in the list.
    show_favorites_only: bool,

    /// A tone file opened for a look, read and shown without touching the device.
    preview: Option<Preview>,
    /// Draw the chain without its editing affordances: no insert gaps, no
    /// ghost branch, no drags. The preview runs the renderer this way.
    display_only: bool,
    /// Which half of the library the strip is showing.
    lib_showing: LibraryView,
    /// `Some(product)` scopes local tones, setlists and Cloud to that pedal.
    /// `None` exposes the whole collection; incompatible rows remain
    /// inspectable but cannot be sent to the connected hardware.
    library_device_filter: Option<String>,
    /// Connection identity observed on the preceding frame. A newly connected
    /// pedal becomes the default scope once, without fighting a later manual
    /// choice of “All pedals”.
    library_connected_device: String,
    /// Each library view searches the collection it is currently showing.
    tone_search: String,
    setlist_search: String,
    /// The Cloud query is also the server-side discovery query.
    library_search: String,
    cloud_search_due: Option<std::time::Instant>,
    cloud_searching: Option<CloudSearchJob>,
    cloud_loaded_query: Option<String>,
    cloud_loaded_order: Option<cloud::DiscoveryOrder>,
    cloud_loaded_device: Option<String>,
    cloud_order: cloud::DiscoveryOrder,
    cloud_entries: Vec<CloudEntry>,
    /// Matching rows reported by the server, including pages not fetched yet.
    cloud_total: Option<usize>,
    /// The one subsequent page to request when the table nears its end.
    cloud_next_page: Option<u32>,
    cloud_error: Option<String>,
    cloud_selected: Option<usize>,
    cloud_tag_filter: Option<String>,
    cloud_tags: Vec<String>,
    cloud_sort: (LibColumn, bool),
    cloud_download: Option<CloudDownloadJob>,
    cloud_artifacts: std::collections::HashMap<i64, Vec<u8>>,
    /// The cloud Tone temporarily sounding through the pedal.
    auditioning: Option<i64>,
    /// The setlists in the library, with the file each came from.
    lib_setlists: Vec<(std::path::PathBuf, library::Setlist)>,
    /// Whether the chosen setlist shows only the banks that differ from the
    /// pedal.
    setlist_only_differing: bool,
    /// The setlist a capture of the pedal becomes the next version of, when
    /// it was asked for from that setlist.
    capture_as: Option<String>,
    /// Which setlist is open, and the draft of its details while it is edited.
    lib_setlist: Option<usize>,
    lib_setlist_draft: library::Setlist,
    /// A just-captured pedal waiting for an existing or new setlist name.
    setlist_save: Option<SetlistSave>,
    /// A setlist waiting on an answer about being written to the pedal. It is
    /// 126 flash writes and it overwrites everything, so it asks first.
    confirm_push: Option<usize>,
    /// The Backups tab's copies of the pedal, read from disk when the tab is
    /// drawn and again after every backup; `None` until then.
    backup_shelf: Option<Vec<backups::BackupCopy>>,
    /// The copy chosen on the Backups tab, by where it lives.
    backup_chosen: Option<std::path::PathBuf>,
    /// What each copy compared holds, read once.
    backup_held: std::collections::HashMap<std::path::PathBuf, backups::Held>,
    /// A whole-pedal restore waiting for its confirmation.
    restoring: Option<backups::Restoring>,
    /// Whether the confirmation keeps the pedal as a setlist before writing.
    keep_before_push: bool,
    /// The setlist written once the capture asked for before it is kept.
    push_after_capture: Option<library::Setlist>,
    lib_entries: Vec<LibEntry>,
    library_lookup: LibraryLookup,
    lib_selected: Option<usize>,
    /// The metadata draft for the selected entry, saved as it is edited.
    lib_draft: library::Meta,
    lib_tag_filter: Option<String>,
    /// Tones picked out in the table, by file name. By file name rather than
    /// row so a sort or a filter does not silently change what is selected.
    lib_chosen: std::collections::BTreeSet<String>,
    /// Where a shift-click measures its range from.
    lib_anchor: Option<usize>,
    /// Which column orders the table, and which way.
    lib_sort: (LibColumn, bool),
    /// Columns turned off. Off rather than on, so a column added later shows
    /// up for everyone rather than only for people who have never touched this.
    lib_hidden: std::collections::BTreeSet<LibColumn>,
    /// The cell being typed into: which tone, which column, and the text so
    /// far. In the app rather than the table, so it survives the frame.
    lib_editing: Option<(String, LibColumn, String)>,
    /// Tones waiting on an answer about being deleted, and the setlists that
    /// play them.
    confirm_delete: Option<(Vec<String>, Vec<String>)>,
    /// A kept tone waiting on an answer about a name already in use.
    name_clash: Option<NameClash>,
    /// A tone from the library waiting for a slot on the pedal.
    sending: Option<Sending>,
    /// What each slot on the pedal is holding, by the hash of its bytes.
    ///
    /// Read out of the automatic backup rather than off the wire: that bundle
    /// is a full copy taken on connect and refreshed one preset at a time after
    /// every save, so it already knows, and asking the pedal for 126 documents
    /// to draw 126 dots would be absurd. What it cannot know is the edit
    /// buffer, which is what the unsaved dot in the title bar is for.
    mirror: std::collections::BTreeMap<i64, String>,
    /// Comma-separated genres and a pending tag, edited in the inspector.
    lib_genres_buf: String,
    lib_tag_add: String,
    /// Editable tempo, so typing does not fight the device's value.
    tempo_draft: Option<String>,
    /// A parameter value being typed, by block position and parameter index,
    /// with the text so far. One at a time, like every other draft here.
    param_draft: Option<(i64, i64, String)>,
    /// Snapshot being renamed, with its draft name.
    snapshot_draft: Option<(usize, String)>,
    /// A footswitch's name being typed, and which switch it belongs to.
    switch_draft: Option<(u8, String)>,
    /// An end of a control's travel being typed: which assignment, whether it
    /// is the high end, and the text so far.
    travel_draft: Option<(i64, hx_proto::preset::Target, bool, String)>,
    /// A sign-in waiting to be approved, somewhere else.
    signing_in: Option<Signing>,
    /// A tone being published, and the answer when the site gives one.
    publishing: Option<PublishingJob>,
    /// A MIDI CC being dragged, by the block and the thing it drives.
    ///
    /// The number is only written to the pedal when the drag stops, so between
    /// picking it up and letting it go there is nowhere else for it to live: a
    /// cell redrawn from the document each frame would sit still under the
    /// pointer. Emptied whenever a preset arrives, which is the document
    /// catching up.
    cc_drafts: std::collections::BTreeMap<(i64, hx_proto::preset::Target), i64>,
    /// Every footswitch and what it carries, as the pedal reports it. Read on
    /// load and after every assignment, so what is on screen is what is on the
    /// pedal rather than what was asked for.
    switches: Vec<hx_usb::Switch>,
    /// Everything a controller drives, for every block in the preset at once.
    ///
    /// Out of the preset document, which arrives with every load and every
    /// reload after an edit, so there is no separate ask and nothing to go
    /// stale between blocks. Opcode 36 used to answer this one parameter at a
    /// time, for one block, and was wrong about the travel besides.
    assignments: Vec<hx_proto::preset::Assignment>,
    /// Editable copy of the preset name, so typing does not fight the device.
    /// The preset being renamed - its slot index and the draft name. Drives the
    /// inline field on both the loaded title and any right-clicked list row.
    renaming: Option<(i64, String)>,
    /// What the worker has been doing. A debugging aid, not something to look
    /// at while playing, so it lives on the Pedal page's Activity tab.
    log: Vec<String>,
    status: String,
    /// A backup or restore in flight: what it is doing, and how far along.
    working: Option<(String, f32)>,
    /// Current value of each named global setting, by object id.
    settings: std::collections::BTreeMap<i64, f32>,
    /// The IR slot being renamed in place, and the name so far.
    renaming_ir: Option<(i64, String)>,
    /// The device's favourite blocks, as (index, name).
    favourites: Vec<(i64, String)>,
    current_snapshot: usize,
    /// Whether closing the window was put off because the worker was in the
    /// middle of talking to the pedal. The window closes itself once it is
    /// not.
    closing: bool,
    /// A switch that would throw away the edit buffer's unsaved changes,
    /// waiting for an answer: a row, an arrow key, a menu action or a
    /// previewed tone's Load asked for it.
    confirm_switch: Option<PendingSwitch>,
    /// Renaming from the header, kept apart from the list's own rename state.
    /// Sharing one made both draw a field for the same preset, and two fields
    /// fighting over the keyboard is neither of them working.
    renaming_header: Option<String>,
    /// A preset the Remove button is waiting on an answer about. Emptying a
    /// slot writes flash and there is no undo for it, so it asks first.
    confirm_clear: Option<i64>,
    /// The daily release check and, when this copy may replace itself, the
    /// download and restart. Quiet until there is something to say.
    updates: update::Updates,
    /// What TonePush already has, by the hash of each published Tone artifact,
    /// and the answer while it is still coming. `None` means the site has not
    /// answered; `Some(empty)` is a real answer saying nothing is published.
    cloud_check: Option<std::sync::mpsc::Receiver<std::collections::BTreeSet<String>>>,
    cloud_files: Option<std::collections::BTreeSet<String>>,
    /// Each library tone's publishable-artifact hash, worked out once. Reading
    /// and hashing a large library is nothing once and too much every frame.
    portable_hashes: std::collections::HashMap<String, String>,
}

/// The global settings the EQ panel drives, by the object id the device holds
/// them under. Named here so the panel reads as an EQ rather than as a handful
/// of magic numbers; `hx_proto::settings` is where they were identified.
mod id {
    pub const EQ_ON: i64 = 203;
    pub const LOW_CUT: i64 = 199;
    pub const LOW_FREQ: i64 = 190;
    pub const LOW_Q: i64 = 191;
    pub const LOW_GAIN: i64 = 192;
    pub const MID_FREQ: i64 = 193;
    pub const MID_Q: i64 = 194;
    pub const MID_GAIN: i64 = 195;
    pub const HIGH_FREQ: i64 = 196;
    pub const HIGH_Q: i64 = 197;
    pub const HIGH_GAIN: i64 = 198;
    pub const HIGH_CUT: i64 = 200;
}

/// Where a copied preset should end up. Reading it is the same round trip
/// either way, so the destination is remembered until the bytes come back.
enum CopyTarget {
    Clipboard,
    File(std::path::PathBuf),
    /// Into the library as the byte-exact native preset document.
    Library,
}

/// A column of the library table, and what it sorts by.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LibColumn {
    /// Whether this tone is on the pedal. Not a column of text, and not one
    /// that can be turned off: it is the thing you act on.
    Sync,
    Name,
    Chain,
    /// The song and its artist in one column, as the design lists a tone.
    SongArtist,
    Character,
    Rating,
    Added,
    Version,
    Artist,
    Song,
    Genre,
    Downloads,
    Modified,
}

impl LibColumn {
    const ALL: [LibColumn; 13] = [
        LibColumn::Sync,
        LibColumn::Name,
        LibColumn::Chain,
        LibColumn::SongArtist,
        LibColumn::Character,
        LibColumn::Rating,
        LibColumn::Downloads,
        LibColumn::Added,
        LibColumn::Version,
        LibColumn::Artist,
        LibColumn::Song,
        LibColumn::Genre,
        LibColumn::Modified,
    ];

    fn title(self) -> &'static str {
        match self {
            LibColumn::Sync => "",
            LibColumn::Name => "Name",
            LibColumn::Chain => "Chain",
            LibColumn::SongArtist => "Song · Artist",
            LibColumn::Character => "Character",
            LibColumn::Rating => "Rating",
            LibColumn::Added => "Added",
            LibColumn::Version => "Version",
            LibColumn::Artist => "Artist",
            LibColumn::Song => "Song",
            LibColumn::Genre => "Genre",
            LibColumn::Downloads => "Downloads",
            LibColumn::Modified => "Modified",
        }
    }

    /// Columns that cannot be turned off. Without a name there is nothing to
    /// read, and without the places there is nothing to press.
    fn always(self) -> bool {
        matches!(self, LibColumn::Sync | LibColumn::Name)
    }

    /// The columns the table starts with: the design's, with the rest on the
    /// header's menu.
    fn hidden_at_first() -> std::collections::BTreeSet<LibColumn> {
        std::collections::BTreeSet::from([
            LibColumn::Version,
            LibColumn::Artist,
            LibColumn::Song,
            LibColumn::Genre,
            LibColumn::Modified,
        ])
    }

    /// Local and Cloud tables share one column definition. Cloud passes false
    /// because published metadata is read-only; local Tones pass true.
    fn column(self, editable: bool) -> table::Column {
        let (title, width, can_edit, fills) = match self {
            LibColumn::Sync => ("", 44.0, false, false),
            LibColumn::Name => ("Name", 150.0, true, false),
            LibColumn::Chain => ("Chain", 112.0, false, false),
            LibColumn::SongArtist => ("Song · Artist", 150.0, false, true),
            LibColumn::Character => ("Character", 76.0, true, false),
            LibColumn::Rating => ("Rating", 76.0, false, false),
            LibColumn::Added => ("Added", 56.0, false, false),
            LibColumn::Version => ("Version", 64.0, false, false),
            LibColumn::Artist => ("Artist", 130.0, true, false),
            LibColumn::Song => ("Song", 150.0, true, false),
            LibColumn::Genre => ("Genre", 130.0, true, false),
            LibColumn::Downloads => ("Downloads", 80.0, false, false),
            LibColumn::Modified => ("Modified", 64.0, false, false),
        };
        let mut column = table::Column::new(title, width);
        if editable && can_edit {
            column = column.editable();
        }
        if fills {
            column = column.fills();
        }
        column
    }

    /// What this column shows for a row as text: what typing into it starts
    /// from, and what a text cell shows.
    fn text(self, entry: &LibEntry) -> String {
        match self {
            LibColumn::Sync | LibColumn::Chain => String::new(),
            LibColumn::Name => entry.name.clone(),
            LibColumn::Version => {
                if entry.versions > 1 {
                    format!("v{} of {}", entry.version, entry.versions)
                } else {
                    "v1".to_owned()
                }
            }
            LibColumn::SongArtist => entry.meta.song.clone(),
            LibColumn::Artist => entry.meta.artist.clone(),
            LibColumn::Song => entry.meta.song.clone(),
            LibColumn::Genre => entry.meta.genres.join(", "),
            LibColumn::Added => day_month(&entry.added_at),
            LibColumn::Modified => day_month(&entry.modified_at),
            LibColumn::Downloads => entry
                .downloads
                .map(format_count)
                .unwrap_or_else(|| "-".to_owned()),
            LibColumn::Rating => entry
                .rating
                .map(|rating| format!("{rating:.1}"))
                .unwrap_or_default(),
            LibColumn::Character => entry.meta.character.clone(),
        }
    }

    fn value_cell(self, entry: &LibEntry, family_tag: Option<&str>, editable: bool) -> table::Cell {
        match self {
            LibColumn::Name => table::Cell::Name {
                text: entry.name.clone(),
                tag: family_tag.map(str::to_owned),
            },
            LibColumn::Chain => table::Cell::Chain {
                chain: entry.chain.clone(),
                key: entry.line.clone(),
            },
            LibColumn::SongArtist => table::Cell::Pair {
                text: if entry.meta.song.trim().is_empty() {
                    "Original".to_owned()
                } else {
                    entry.meta.song.clone()
                },
                aside: entry.meta.artist.clone(),
            },
            LibColumn::Character => table::Cell::Text(shell::sentence_case(&entry.meta.character)),
            LibColumn::Added => table::Cell::Value {
                text: day_month(&entry.added_at),
                key: entry.added_at.clone(),
                dim: entry.added_at.is_empty(),
            },
            LibColumn::Modified => table::Cell::Value {
                text: day_month(&entry.modified_at),
                key: entry.modified_at.clone(),
                dim: entry.modified_at.is_empty(),
            },
            LibColumn::Downloads => table::Cell::Value {
                text: entry
                    .downloads
                    .map(format_count)
                    .unwrap_or_else(|| "-".to_owned()),
                key: format!("{:020}", entry.downloads.unwrap_or(0)),
                dim: entry.downloads.is_none(),
            },
            LibColumn::Rating => table::Cell::Stars {
                rating: entry
                    .rating
                    .map_or(0, |rating| rating.round().clamp(0.0, 5.0) as u8),
                editable,
            },
            LibColumn::Version => table::Cell::Dim(self.text(entry)),
            _ => table::Cell::Text(self.text(entry)),
        }
    }

    fn cell(
        self,
        entry: &LibEntry,
        state: theme::Sync,
        cloud: theme::Sync,
        can_send: bool,
        family_tag: Option<&str>,
    ) -> table::Cell {
        match self {
            // This row is a tone in the library, so the computer is not one of
            // the icons: it shows the pedal, and the cloud once the tone
            // browser has answered for it. The cloud is dropped entirely rather
            // than drawn blank when the site said nothing, because a
            // permanently empty icon is furniture.
            LibColumn::Sync => {
                let mut places = vec![(
                    theme::Icon::Pedal,
                    state,
                    match state {
                        theme::Sync::Absent => "Not on the pedal. Send it",
                        theme::Sync::Same => "On the pedal",
                        theme::Sync::Differs => {
                            "On the pedal under this name, but different. Send this one"
                        }
                        theme::Sync::Working => "Sending to the pedal…",
                        theme::Sync::Unknown if can_send => "Send it to the pedal",
                        theme::Sync::Unknown => "Not available for the connected pedal",
                    },
                    can_send && state != theme::Sync::Working,
                )];
                if cloud != theme::Sync::Unknown {
                    places.push((
                        theme::Icon::Cloud,
                        cloud,
                        match cloud {
                            theme::Sync::Same => "Published on TonePush. Open this Tone",
                            theme::Sync::Absent => "Not on TonePush. Publish its Song and Tone",
                            theme::Sync::Differs => {
                                "A different Tone artifact is published. Publish this one"
                            }
                            theme::Sync::Working => "Publishing…",
                            theme::Sync::Unknown => "",
                        },
                        cloud != theme::Sync::Working,
                    ));
                }
                table::Cell::Places(places)
            }
            _ => self.value_cell(entry, family_tag, true),
        }
    }
}

/// "10 Sep", or "10 Sep 2025" for another year: how the table writes a date.
fn day_month(timestamp: &str) -> String {
    let date = timestamp.get(..10).unwrap_or(timestamp);
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return date.to_owned();
    };
    let Ok(month) = month.parse::<usize>() else {
        return date.to_owned();
    };
    let Some(name) = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(month.wrapping_sub(1)) else {
        return date.to_owned();
    };
    let day = day.trim_start_matches('0');
    let this_year = jiff::Zoned::now().year().to_string();
    if year == this_year {
        format!("{day} {name}")
    } else {
        format!("{day} {name} {year}")
    }
}

fn format_count(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The three views of the library and its connected public catalog.
///
/// Not tabs on the window: the strip along the bottom *is* the library, and a
/// selector inside it says which of its own two views you are looking at. The
/// pedal's own libraries - impulse responses, favourite blocks - are not here
/// at all. They belong to the device, and they live behind the device's button,
/// which is the distinction HX Edit's tabs blur.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryView {
    Tones,
    Setlists,
    Cloud,
}

/// What a preset's right-click menu can do to it.
///
/// One preset, never all of them: the whole-pedal operations are Back up and
/// Restore, and they live on the list's header rather than on a preset.
#[derive(Clone, Copy)]
enum RowAction {
    /// Put the library's tone of this name back in step with the pedal's.
    Update,
    Copy,
    Paste,
    Export,
    Import,
    Keep,
    Remove,
}

/// A switch to another preset waiting on an answer about unsaved changes:
/// where to, and what to do once the pedal is there.
struct PendingSwitch {
    index: i64,
    then: AfterSwitch,
}

/// What a switch to a preset is for.
enum AfterSwitch {
    /// Showing it: it was picked to be played or edited.
    Load,
    /// Reading its document, for the clipboard, a file or the library.
    Copy(CopyTarget),
    /// Writing the copied preset over it, under the copied name.
    Paste(String, Vec<u8>),
    /// Loading a previewed tone into it. The whole preview rides along, so
    /// that Cancel can put the window back as it was.
    Tone(Box<Preview>),
}

/// The three answers to "discard the unsaved changes?".
#[derive(Clone, Copy)]
enum SwitchAnswer {
    Cancel,
    Discard,
    Save,
}

#[derive(Debug, PartialEq)]
enum Connection {
    Offline,
    Connecting,
    Online,
}

impl App {
    /// Which protocol adapter currently owns the editor surface.
    fn pro_active(&self) -> bool {
        self.pro.claims_ui() && self.connection != Connection::Online
    }

    /// Device presence as understood by shared library/setlist/cloud widgets.
    fn pedal_online(&self) -> bool {
        if self.pro_active() {
            self.pro.is_online()
        } else {
            matches!(self.connection, Connection::Online)
        }
    }

    fn end_audition(&mut self) {
        if self.pro_active() {
            self.pro.end_audition();
        } else {
            self.send(Cmd::EndAudition);
        }
    }

    fn keep_audition(&mut self) {
        if self.pro_active() {
            self.pro.keep_audition();
        } else {
            self.send(Cmd::KeepAudition);
        }
    }

    fn tone_kind_compatible(&self, hash: &str) -> bool {
        match library::kind(hash).as_deref() {
            Some("vxpreset") => self.pro_active(),
            Some("hxpreset" | "hlx") => !self.pro_active(),
            _ => false,
        }
    }

    fn device_is_pro(device: &str) -> bool {
        device.to_ascii_lowercase().contains("stompstation")
    }

    fn tone_matches_device(hash: &str, device: &str) -> bool {
        match library::kind(hash).as_deref() {
            Some("vxpreset") => Self::device_is_pro(device),
            Some("hxpreset" | "hlx") => !Self::device_is_pro(device),
            _ => false,
        }
    }

    fn tone_in_library_scope(&self, hash: &str) -> bool {
        self.library_device_filter
            .as_deref()
            .is_none_or(|device| Self::tone_matches_device(hash, device))
    }

    fn setlist_in_library_scope(&self, setlist: &library::Setlist) -> bool {
        self.library_device_filter.as_deref().is_none_or(|device| {
            setlist
                .slots
                .iter()
                .filter(|slot| !slot.is_empty())
                .all(|slot| Self::tone_matches_device(&slot.hash, device))
        })
    }

    fn scoped_tone_count(&self) -> usize {
        self.lib_entries
            .iter()
            .filter(|entry| self.tone_in_library_scope(&entry.hash))
            .count()
    }

    fn scoped_setlist_count(&self) -> usize {
        self.lib_setlists
            .iter()
            .filter(|(_, setlist)| self.setlist_in_library_scope(setlist))
            .count()
    }

    fn observe_library_device(&mut self) {
        let connected = if self.pedal_online() {
            self.device.trim().to_owned()
        } else {
            String::new()
        };
        if connected.is_empty() {
            self.library_connected_device.clear();
        } else if connected != self.library_connected_device {
            self.library_connected_device.clone_from(&connected);
            self.library_device_filter = Some(connected);
            self.lib_selected = None;
            self.lib_setlist = None;
            self.cloud_loaded_device = None;
            self.cloud_search_due = Some(std::time::Instant::now());
        }
    }

    fn cloud_device_scope(&self) -> String {
        self.library_device_filter.clone().unwrap_or_default()
    }

    fn active_slot_label(&self, slot: i64) -> String {
        if self.pro_active() {
            pro::slot_label(slot.max(0) as usize)
        } else {
            hx_proto::PROFILES
                .iter()
                .find(|profile| profile.name == self.device.trim())
                .map_or_else(
                    || hx_proto::rpc::slot_label(slot),
                    |profile| profile.slot_label(slot),
                )
        }
    }

    fn setlist_compatible(&self, setlist: &library::Setlist) -> bool {
        let capacity = if self.pro_active() {
            self.pro.preset_count()
        } else {
            self.preset_count as usize
        };
        setlist.slots.len() <= capacity
            && setlist
                .slots
                .iter()
                .filter(|slot| !slot.is_empty())
                .all(|slot| self.tone_kind_compatible(&slot.hash))
    }

    /// Styling is applied once here rather than per frame: it clones and
    /// rewrites the whole `Style`, which is pure waste sixty times a second.
    pub fn new(ctx: &egui::Context, to_device: Sender<Cmd>, from_device: Receiver<Evt>) -> Self {
        let config = config::Config::load();
        theme::install(ctx, config.appearance);
        let mut app = App {
            to_device,
            from_device,
            pro: pro::Panel::new(ctx.clone()),
            // Without HX Edit installed everything still works, just with
            // numbers where names would be.
            catalog: Catalog::load().ok(),
            connection: Connection::Offline,
            device: String::new(),
            firmware: String::new(),
            presets: Vec::new(),
            preset_count: 0,
            preset_index: -1,
            preset_name: String::new(),
            tempo: None,
            snapshots: Vec::new(),
            chain: Vec::new(),
            layout: hx_proto::preset::Layout::default(),
            copied_block: false,
            clipboard: None,
            pending_copy: CopyTarget::Clipboard,
            dirty: false,
            page: shell::Page::Edit,
            pedal_tab: pages::PedalTab::Backups,
            sidebar_hidden: false,
            automatic_backup: None,
            eq_wrote_at: None,
            global_eq: false,
            undo_depth: 0,
            redo_depth: 0,
            loading: false,
            busy_since: None,
            extracting: None,
            onboarding_status: None,
            show_onboarding: false,
            taps: Vec::new(),
            dragging: None,
            dragging_junction: None,
            gap_rects: Vec::new(),
            block_rects: Vec::new(),
            ghost_target: None,
            selected: 0,
            lens: pane::Lens::default(),
            browser: None,
            browsed_category: None,
            focused_source: None,
            snapshot_details: Vec::new(),
            pending_select: None,
            trial_kept: std::cell::Cell::new(false),
            reveal_preset: false,
            irs: Vec::new(),
            setlists: Vec::new(),
            setlist: 0,
            config,
            show_favorites_only: false,
            preview: None,
            display_only: false,
            lib_showing: LibraryView::Tones,
            library_device_filter: None,
            library_connected_device: String::new(),
            tone_search: String::new(),
            setlist_search: String::new(),
            library_search: String::new(),
            // Public discovery is useful before anybody types or signs in.
            // Give the automatic pedal connection a moment to identify its
            // compatible feed first; if no pedal appears, the public all-device
            // catalog still fills on its own shortly afterwards.
            cloud_search_due: Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
            cloud_searching: None,
            cloud_loaded_query: None,
            cloud_loaded_order: None,
            cloud_loaded_device: None,
            cloud_order: cloud::DiscoveryOrder::Popular,
            cloud_entries: Vec::new(),
            cloud_total: None,
            cloud_next_page: None,
            cloud_error: None,
            cloud_selected: None,
            cloud_tag_filter: None,
            cloud_tags: Vec::new(),
            cloud_sort: (LibColumn::Downloads, false),
            cloud_download: None,
            cloud_artifacts: Default::default(),
            auditioning: None,
            lib_setlists: Vec::new(),
            setlist_only_differing: false,
            capture_as: None,
            lib_setlist: None,
            lib_setlist_draft: library::Setlist::default(),
            setlist_save: None,
            confirm_push: None,
            backup_shelf: None,
            backup_chosen: None,
            backup_held: std::collections::HashMap::new(),
            restoring: None,
            keep_before_push: false,
            push_after_capture: None,
            lib_entries: Vec::new(),
            library_lookup: LibraryLookup::default(),
            lib_selected: None,
            lib_draft: library::Meta::default(),
            lib_tag_filter: None,
            lib_chosen: Default::default(),
            lib_anchor: None,
            lib_sort: (LibColumn::Name, true),
            confirm_delete: None,
            // Character remains available in Columns, but the default table
            // leads with the musical identity the user asked for.
            lib_hidden: LibColumn::hidden_at_first(),
            lib_editing: None,
            name_clash: None,
            sending: None,
            mirror: Default::default(),
            lib_genres_buf: String::new(),
            lib_tag_add: String::new(),
            tempo_draft: None,
            param_draft: None,
            snapshot_draft: None,
            switch_draft: None,
            travel_draft: None,
            signing_in: None,
            publishing: None,
            cc_drafts: Default::default(),
            switches: Vec::new(),
            assignments: Vec::new(),
            renaming: None,
            log: Vec::new(),
            status: "Looking for a device…".into(),
            working: None,
            settings: Default::default(),
            renaming_ir: None,
            favourites: Vec::new(),
            current_snapshot: 0,
            confirm_clear: None,
            closing: false,
            confirm_switch: None,
            renaming_header: None,
            updates: update::Updates::default(),
            cloud_check: None,
            cloud_files: None,
            portable_hashes: std::collections::HashMap::new(),
        };
        // The library is on screen from the first frame, so it is read before
        // the first frame rather than when something happens to refresh it.
        app.refresh_library();
        app.start_cloud_check();
        // Likewise what the pedal is holding, if a backup from a previous run
        // is on disk: the dots should say something true on the first frame
        // rather than after the first connection.
        app.refresh_mirror();
        // Without the model data there is nothing to edit with, so the
        // welcome window opens immediately and stays until the data exists.
        app.show_onboarding = app.catalog.is_none();
        // Machines that already have HX Edit need no ceremony at all: lift
        // the data from the installation while the welcome says so.
        if app.show_onboarding && hx_catalog::extract::installed_resources().is_some() {
            app.extract_installed();
        }
        // Connect straight away. Anyone opening this has a pedal plugged in;
        // making them press a button first is ceremony.
        let _ = app.to_device.send(Cmd::Connect);
        app.connection = Connection::Connecting;
        app
    }

    fn send(&self, cmd: Cmd) {
        // The worker keeps a model being tried when anything else arrives;
        // the browser follows on its next frame.
        if self.browser.as_ref().is_some_and(|b| b.playing.is_some())
            && !matches!(
                cmd,
                Cmd::TryModel { .. } | Cmd::PutBack | Cmd::KeepTry | Cmd::ReadSwitches
            )
        {
            self.trial_kept.set(true);
        }
        let _ = self.to_device.send(cmd);
    }

    /// Send something that changes the edit buffer, and remember that it did.
    fn edit(&mut self, cmd: Cmd) {
        self.dirty = true;
        self.send(cmd);
    }

    /// Drop everything half-done that names a place in the loaded preset: a
    /// value being typed, a drag, an open model picker. Each finishes by
    /// sending a block position, and once the pedal has loaded another
    /// preset that position is a different block.
    fn forget_drafts(&mut self) {
        self.param_draft = None;
        self.switch_draft = None;
        self.travel_draft = None;
        self.renaming_header = None;
        self.dragging = None;
        self.dragging_junction = None;
        // The worker has kept or dropped any try with the preset it was on.
        self.browser = None;
        self.trial_kept.set(false);
        self.pending_select = None;
    }

    fn drain_events(&mut self) {
        loop {
            match self.from_device.try_recv() {
                Ok(Evt::Connected { device, presets }) => {
                    self.connection = Connection::Online;
                    self.device = device;
                    self.preset_count = presets;
                    // Replace the startup's device-agnostic prefetch even when
                    // the Cloud tab is not open: its badge must already mean
                    // “compatible with this pedal” when somebody reaches it.
                    self.cloud_loaded_device = None;
                    self.cloud_search_due = Some(std::time::Instant::now());
                    // Nothing to say: the lit dot, the device's own name, its
                    // firmware and a Disconnect button already say it, and
                    // saying it again in the far corner only puts distance
                    // between the word and the button.
                    self.status.clear();
                    let _ = self.to_device.send(Cmd::ListIrs);
                    let _ = self.to_device.send(Cmd::ListSetlists);
                    // One automatic backup per session, taken as soon as the
                    // pedal is there. It costs a few seconds and means the copy
                    // on disk is never older than the day's work; every save
                    // after this refreshes just the preset it changed.
                    if let Some(dir) = session::automatic_dir() {
                        let _ = self.to_device.send(Cmd::BackUp(dir));
                    }
                    if self.page == shell::Page::Pedal {
                        self.read_pedal_page();
                    }
                }
                Ok(Evt::Disconnected) => {
                    self.connection = Connection::Offline;
                    // Nothing is restored onto a pedal that has gone.
                    self.restoring = None;
                    // A write waiting on a capture of a pedal that has gone
                    // is not made to whatever connects next.
                    if !self.pro_active() {
                        self.push_after_capture = None;
                    }
                    self.auditioning = None;
                    self.browser = None;
                    self.trial_kept.set(false);
                    self.snapshot_details.clear();
                    self.loading = false;
                    self.dirty = false;
                    self.preset_count = 0;
                    self.preset_index = -1;
                    self.preset_name.clear();
                    self.firmware.clear();
                    self.tempo = None;
                    self.snapshots.clear();
                    self.chain.clear();
                    self.layout = hx_proto::preset::Layout::default();
                    self.assignments.clear();
                    self.switches.clear();
                    self.copied_block = false;
                    self.presets.clear();
                    self.irs.clear();
                    self.setlists.clear();
                    self.favourites.clear();
                    self.confirm_switch = None;
                    self.working = None;
                    // Keep a preceding failure visible: it explains why the
                    // editor let go. A deliberate disconnect clears status at
                    // the button before this event arrives.
                }
                Ok(Evt::Presets(names)) => {
                    self.presets = names;
                    // A rename does not reload the preset - it only changes a
                    // slot's label - so the title bar kept showing the old name
                    // until something else happened to reload. The list is the
                    // authority on names; the title follows it.
                    if let Some(name) = self
                        .presets
                        .get(self.preset_index.max(0) as usize)
                        .filter(|n| !n.is_empty())
                    {
                        self.preset_name = name.clone();
                    }
                }
                Ok(Evt::Working { what, progress }) => {
                    self.working = if what.is_empty() {
                        None
                    } else {
                        Some((what, progress))
                    };
                }
                Ok(Evt::BackedUp {
                    dir,
                    presets,
                    settings,
                    irs,
                }) => {
                    self.working = None;
                    self.refresh_mirror();
                    self.forget_backups();
                    self.remember_backup_file(&dir);
                    self.write_hxb_beside(&dir);
                    self.note(format!(
                        "backed up {presets} presets, {settings} settings and {irs} \
                         impulse responses to {}",
                        dir.display()
                    ));
                }
                Ok(Evt::Loaded {
                    index,
                    name,
                    firmware,
                    tempo,
                    snapshots,
                    snapshot,
                    snapshot_details,
                    chain,
                    layout,
                    assignments,
                    dirty,
                }) => {
                    // A switch the editor asked for has already moved the
                    // selection, so a different preset arriving here is one
                    // the pedal loaded by itself, or the one still loaded
                    // after a switch was refused.
                    let moved = index != self.preset_index;
                    if moved {
                        self.forget_drafts();
                    }
                    self.layout = layout;
                    self.assignments = assignments;
                    // The document has caught up, so anything held while a
                    // number was being dragged has been answered.
                    self.cc_drafts.clear();
                    // The worker's word, not a blanket reset: most reloads are
                    // edits taking effect, and those leave changes to save.
                    self.dirty = dirty;
                    self.reveal_preset = moved;
                    self.loading = false;
                    self.preset_index = index;
                    self.preset_name = name;
                    self.firmware = firmware;
                    self.tempo = tempo;
                    self.snapshots = snapshots;
                    self.snapshot_details = snapshot_details;
                    // The pedal's word for which snapshot is active, whether
                    // it was picked here, on its footswitches, or came with
                    // the preset. A click's own guess does not outlive it.
                    self.current_snapshot = snapshot.unwrap_or_default();
                    self.chain = chain;
                    // Land on something editable rather than the input, which
                    // has nothing to show. A split or a join counts: changing
                    // its type reloads the preset, and treating a junction as
                    // not-editable threw you out of the very panel you were
                    // using every time you clicked Y, A/B or Crossover.
                    let editable = |app: &Self, b: &session::Block| {
                        use hx_proto::preset::Kind;
                        app.is_effect(b) || matches!(b.kind, Kind::Split | Kind::Join)
                    };
                    if !self
                        .chain
                        .get(self.selected)
                        .is_some_and(|b| editable(self, b))
                    {
                        self.selected = self
                            .chain
                            .iter()
                            .position(|b| self.is_effect(b))
                            .unwrap_or(0);
                    }
                    self.selected = self.selected.min(self.chain.len().saturating_sub(1));
                    // A block just added is the one to look at.
                    if let Some(slot) = self.pending_select.take() {
                        if let Some(index) = self.chain.iter().position(|b| b.position == slot) {
                            self.selected = index;
                        }
                    }
                    // The assignments came with the document; what a switch is
                    // called and what colour it lights did not.
                    self.read_switches();
                    self.renaming = None;
                    self.tempo_draft = None;
                    self.snapshot_draft = None;
                }
                Ok(Evt::Saved) => {
                    self.dirty = false;
                    // The bundle was refreshed before this arrived, so the
                    // dots can be brought in step with it now.
                    self.refresh_mirror();
                }
                Ok(Evt::Auditioning(key)) => self.auditioning = key,
                Ok(Evt::Busy(on)) => {
                    self.busy_since = if on {
                        self.busy_since.or(Some(std::time::Instant::now()))
                    } else {
                        None
                    };
                }
                Ok(Evt::History { undo, redo }) => {
                    self.undo_depth = undo;
                    self.redo_depth = redo;
                }
                Ok(Evt::Settings { global_eq }) => self.global_eq = global_eq,
                Ok(Evt::SettingValues(values)) => {
                    self.settings = values.into_iter().collect();
                }
                Ok(Evt::Copied { name, blob }) => {
                    let size = blob.len();
                    match std::mem::replace(&mut self.pending_copy, CopyTarget::Clipboard) {
                        CopyTarget::File(path) => match library::atomic_write(&path, &blob) {
                            Ok(()) => self.note(format!("exported {name} to {}", path.display())),
                            Err(e) => self.note(format!("could not write {}: {e}", path.display())),
                        },
                        CopyTarget::Clipboard => {
                            self.clipboard = Some((name.clone(), blob));
                            self.note(format!("copied {name} ({size} bytes)"));
                        }
                        CopyTarget::Library => self.keep_tone(&name, "hxpreset", &blob),
                    }
                }
                Ok(Evt::Irs(slots)) => self.irs = slots,
                Ok(Evt::Favourites(list)) => self.favourites = list,
                Ok(Evt::Setlists(names)) => self.setlists = names,
                Ok(Evt::CapturedSetlist(slots)) => self.keep_setlist(slots, "hxpreset"),
                Ok(Evt::Switches(switches)) => self.switches = switches,
                Ok(Evt::Activity(line)) => self.note(line),
                Ok(Evt::Failed(e)) => {
                    self.status = if self.connection == Connection::Connecting {
                        "No supported pedal found. Check USB and close any other pedal editor."
                            .to_owned()
                    } else {
                        e.clone()
                    };
                    self.note(e);
                    if self.connection == Connection::Connecting {
                        self.connection = Connection::Offline;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = "Device thread stopped".into();
                    // Nothing is talking to the pedal any more, so nothing
                    // should keep the window from closing.
                    self.busy_since = None;
                    self.working = None;
                    break;
                }
            }
        }
    }

    /// What the update helper passed to this launch: the receipt of an update
    /// it installed, the message of one it rolled back, and the arguments to
    /// relaunch with after the next update.
    pub fn launched(
        &mut self,
        receipt: Option<update::Receipt>,
        error: Option<String>,
        arguments: Vec<String>,
    ) {
        self.updates.launched(receipt, error, arguments);
    }

    fn note(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 300 {
            self.log.remove(0);
        }
    }

    /// A problem that must be visible without opening the diagnostic log.
    fn problem(&mut self, line: String) {
        self.status = line.clone();
        self.note(line);
    }

    /// A `file://` URI for a model's artwork, which egui loads and caches.
    /// The picture for a slot's tile.
    ///
    /// Endpoints have no model of their own - they report 0, which is a real
    /// entry in the symbol table, so asking for its artwork used to put an amp
    /// on the input tile. What they do have is a routing destination, and HX
    /// Edit draws that: a guitar for an instrument input, a jack for a 1/4"
    /// output. Those live as frames of one strip rather than separate files.
    fn artwork(&self, block: &session::Block) -> Option<theme::Art> {
        use hx_proto::preset::Kind;
        let catalog = self.catalog.as_ref()?;

        if matches!(block.kind, Kind::Input | Kind::Output) {
            let (path, frames) = catalog.endpoint_icons(block.kind == Kind::Input)?;
            // Frame 0 is a placeholder, so the destinations start at 1.
            let frame = block.routing.unwrap_or(0).max(0) as usize + 1;
            return Some(theme::Art::strip(
                format!("file://{}", path.display()),
                frame,
                frames,
            ));
        }

        let path = catalog.artwork(catalog.model_number(block.model)?)?;
        Some(theme::Art::whole(format!("file://{}", path.display())))
    }

    /// A model's display name.
    ///
    /// Model 0 is treated as "no model": the endpoints report it because they
    /// carry no model reference, and the symbol table's entry 0 is a real amp,
    /// so resolving it names the wrong thing entirely.
    /// The block the chain has selected, as `(position, model name)` - what a
    /// favourite would be made from. `None` when the selection is an input,
    /// output or junction, which are not blocks anyone would keep.
    fn selected_block(&self) -> Option<(i64, String)> {
        let block = self.chain.get(self.selected)?;
        if !self.is_effect(block) {
            return None;
        }
        Some((block.position, self.model_name(block.model)))
    }

    fn model_name(&self, model: u32) -> String {
        if model == 0 {
            return String::new();
        }
        self.catalog
            .as_ref()
            .and_then(|c| c.model_number(model))
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("model {model}"))
    }

    /// What to call a slot. Inputs and outputs have no model to name; splits
    /// and joins do - "Split Y", "Mixer" - and the real name says much more
    /// than the slot kind.
    fn slot_label(&self, block: &session::Block) -> String {
        use hx_proto::preset::Kind;
        let named = |fallback: &str| {
            let name = self.model_name(block.model);
            if name.is_empty() {
                fallback.to_owned()
            } else {
                name
            }
        };
        match block.kind {
            Kind::Input => "Input".into(),
            Kind::Output => "Output".into(),
            Kind::Split => named("Split"),
            Kind::Join => named("Join"),
            _ => self.model_name(block.model),
        }
    }

    /// The category a block belongs to, for the line under its name.
    ///
    /// Endpoints have none worth showing - "Input" already says what an input
    /// is. A paired block says Amp+Cab, which is how the name gets to stay the
    /// amp's own rather than growing a "+ Cab" that pushes it off the tile.
    fn block_category(&self, block: &session::Block) -> Option<String> {
        use hx_proto::preset::Kind;
        if matches!(block.kind, Kind::Input | Kind::Output) {
            return None;
        }
        if block.paired.is_some() {
            return Some("Amp+Cab".to_owned());
        }
        let catalog = self.catalog.as_ref()?;
        let model = catalog.model_number(block.model)?;
        let id = catalog.category_of(&model.id)?;
        Some(catalog.category(id)?.name.clone())
    }

    /// Only effects can have their model swapped from the browser.
    fn is_effect(&self, block: &session::Block) -> bool {
        block.kind == hx_proto::preset::Kind::Block
    }

    /// The catalog entry describing a slot's controls.
    ///
    /// Effects, splits and joins carry a model number the symbol table
    /// resolves. Inputs and outputs do not - the device knows what they are
    /// from their position - so they are looked up by symbolic id instead.
    /// They still have real controls: an input has a noise gate, an output has
    /// level and pan.
    fn slot_model(&self, block: &session::Block) -> Option<&hx_catalog::Model> {
        use hx_proto::preset::Kind;
        let catalog = self.catalog.as_ref()?;
        match block.kind {
            Kind::Input => ["HelixStomp_AppDSPFlowInput", "HD2_AppDSPFlow1Input"]
                .into_iter()
                .find_map(|id| catalog.model(id)),
            Kind::Output => ["HelixStomp_AppDSPFlowOutputMain", "HD2_AppDSPFlowOutput"]
                .into_iter()
                .find_map(|id| catalog.model(id)),
            _ => catalog.model_number(block.model),
        }
    }
}

impl eframe::App for App {
    /// Background work that must go on while the window is hidden: eframe
    /// calls this even when it does not paint.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.hold_close_while_busy(ctx);
        self.pro.drain();
        self.updates.poll(ctx);
        if let Some(problem) = self.updates.take_problem() {
            self.problem(problem);
        }
        for (name, bytes, replace) in self.pro.take_library_documents() {
            if replace {
                let updated = library::named(&name)
                    .ok_or_else(|| "that tone disappeared from the library".to_owned())
                    .and_then(|old| {
                        let hash = library::store(&name, &bytes, "vxpreset")?;
                        library::override_with(&old.hash, &hash, &old.meta.name)?;
                        Ok(())
                    });
                match updated {
                    Ok(()) => {
                        self.refresh_library();
                        self.note(format!("updated {name} from the pedal"));
                    }
                    Err(why) => self.note(why),
                }
            } else {
                self.keep_tone(&name, "vxpreset", &bytes);
            }
        }
        for slots in self.pro.take_captured_setlists() {
            self.keep_setlist(slots, "vxpreset");
        }
        if self.pro.take_capture_asked() {
            self.capture_pedal(None);
        }
        for key in self.pro.take_audition_events() {
            self.auditioning = key;
        }
        // Device events wake the UI directly. This slow fallback is for the
        // other background receivers (resource extraction and cloud work), so
        // an otherwise idle editor does not rebuild the whole immediate-mode
        // interface several times per second.
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The worker still drains its channel after the UI sender is dropped,
        // so a normal window close gets the same exact restoration as Done.
        self.end_audition();
        self.pro.disconnect();
    }
}

impl App {
    /// One frame of the whole window. Apart from `eframe::App::ui` so the
    /// design screenshots can draw the editor without a native window.
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Every token this frame reads the theme egui resolved for it.
        theme::follow(&ctx);
        let tier = theme::Tier::now(&ctx);

        let pro_active = self.pro_active();
        if pro_active {
            // The shared library/cloud code keys discovery and publishing off
            // these public device facts. They describe the active adapter; no
            // library component needs to know which wire protocol supplied
            // them.
            self.device = self.pro.device_name().to_owned();
            self.firmware = self.pro.firmware().to_owned();
            self.pro.shortcuts(&ctx);
        } else {
            self.shortcuts(&ctx);
        }
        self.frame_shortcuts(&ctx);
        self.observe_library_device();
        self.finish_extraction();
        // Both arrive from a thread and neither belongs to any one page: one
        // is a person approving a sign-in somewhere else, the other is the
        // site taking a tone.
        self.settle_signing_in();
        self.settle_publishing(&ctx);
        self.settle_cloud_search(&ctx);
        self.settle_cloud_download(&ctx);
        if pro_active {
            self.pro.dropped_files(&ctx);
        } else {
            self.dropped_files(&ctx);
        }

        // The sidebar claims the left edge first, so it runs the window's
        // full height; each page then lays out what is left.
        self.sidebar(ui, tier);
        match self.page {
            shell::Page::Edit => {
                self.deck(ui, tier);
                if pro_active {
                    let note = self.pro_library_note();
                    self.pro.body(ui, tier, &note);
                } else {
                    self.signal_chain(ui);
                    if self.shows_floor(tier) {
                        self.floor(ui, tier);
                    }
                    self.pane(ui, tier);
                }
            }
            shell::Page::Library => {
                self.small_deck(ui);
                self.library_page(ui, tier);
            }
            shell::Page::Pedal => {
                self.small_deck(ui);
                if pro_active {
                    self.pro.pedal_page(ui, tier);
                } else {
                    self.pedal_page(ui, tier);
                }
            }
        }

        // Questions and windows over whichever page is showing: each can be
        // asked from the sidebar, the library or the pedal page alike.
        if pro_active {
            self.pro.windows(&ctx);
        } else {
            self.confirm_clear_window(&ctx);
            self.preview_window(&ctx);
        }
        self.confirm_push_window(&ctx);
        self.confirm_restore_window(&ctx);
        self.confirm_delete_window(&ctx);
        self.name_clash_window(&ctx);
        self.save_setlist_window(&ctx);
        self.confirm_switch_window(&ctx);
        // Over everything: the one step the app cannot work without.
        self.onboarding_modal(&ctx);
        self.closing_window(&ctx);
        // The first frame is up: an update that relaunched into this version
        // has started, so its helper can stop standing ready to roll back.
        self.updates.acknowledge();
    }

    /// The window's own keys: Ctrl+B puts the sidebar away and brings it
    /// back. Skipped while a field has the keyboard.
    fn frame_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let toggle = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::B);
        if ctx.input_mut(|input| input.consume_shortcut(&toggle)) {
            self.sidebar_hidden = !self.sidebar_hidden;
        }
    }

    /// Keep the window while the worker is talking to the pedal, and close
    /// it as soon as the worker is done.
    ///
    /// Closing used to end the process 800 ms later whatever the worker was
    /// doing, and a transfer cut off part way leaves the pedal refusing new
    /// sessions until its power is pulled. A close asked for while the worker
    /// is busy is put off, and the window says why. This runs in `logic`
    /// because a window that is not being drawn can still be asked to close.
    fn hold_close_while_busy(&mut self, ctx: &egui::Context) {
        let busy = self.busy_since.is_some() || self.working.is_some();
        if busy && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = true;
        } else if self.closing && !busy {
            self.closing = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Say why the window has not closed yet.
    fn closing_window(&mut self, ctx: &egui::Context) {
        if !self.closing {
            return;
        }
        // Nothing to answer: it closes by itself, so Escape does nothing here.
        let working = self.working.clone();
        let _ = theme::dialog(ctx, "closing", 400.0, |ui| {
            theme::dialog_header(
                ui,
                "Finishing with the pedal",
                Some("TonePush closes as soon as the pedal is done."),
            );
            theme::dialog_body(ui, |ui| match &working {
                Some((what, progress)) => {
                    theme::label(ui, what, theme::regular(12.5), theme::muted());
                    theme::progress(ui, *progress, 6.0, None);
                }
                None => {
                    ui.horizontal(|ui| {
                        theme::spinner(ui);
                        theme::label(
                            ui,
                            "Waiting for the pedal",
                            theme::regular(12.5),
                            theme::muted(),
                        );
                    });
                }
            });
        });
    }

    /// Emptying a slot writes flash and cannot be undone, so it asks. The
    /// question names the preset: "are you sure" answers nothing on its own.
    fn confirm_clear_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.confirm_clear else {
            return;
        };
        let name = self
            .presets
            .get(index as usize)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| "this preset".to_owned());
        let slot = self.active_slot_label(index);
        let mut decided = None;
        let (_, close) = theme::dialog(ctx, "confirm-clear", 440.0, |ui| {
            theme::dialog_header(
                ui,
                &format!("Empty {slot}?"),
                Some(&format!(
                    "“{name}” goes from the pedal and the slot holds a blank preset."
                )),
            );
            theme::dialog_body(ui, |ui| {
                theme::outcomes(
                    ui,
                    &[theme::Outcome {
                        icon: theme::Icon::Remove,
                        count: "1".into(),
                        sentence: format!("slot emptied: {slot} {name}"),
                        quiet: false,
                    }],
                );
            });
            theme::dialog_footer(
                ui,
                "This writes the pedal's memory. Undo does not reach it.",
                |ui| {
                    if theme::Button::new("Empty the slot")
                        .danger()
                        .show(ui)
                        .clicked()
                    {
                        decided = Some(true);
                    }
                    if theme::Button::new("Cancel").show(ui).clicked() {
                        decided = Some(false);
                    }
                },
            );
        });
        if close && decided.is_none() {
            decided = Some(false);
        }
        match decided {
            Some(true) => {
                self.confirm_clear = None;
                self.send(Cmd::ClearPreset(index));
            }
            Some(false) => self.confirm_clear = None,
            None => {}
        }
    }

    /// Take the site's answer if it has arrived, and say whether a tone is on
    /// it.
    ///
    /// The site records the hash of the artifact it was given: an `.hlx` for
    /// HX or the native `.vxpreset` for PRO. A tone with no publishable artifact cannot be
    /// looked up at all, and answers `Unknown` rather than `Absent`: we do not
    /// know that it is missing, only that we cannot ask.
    fn cloud_sync(&mut self, hash: &str) -> theme::Sync {
        if let Some(rx) = &self.cloud_check {
            match rx.try_recv() {
                Ok(files) => {
                    // A completed publish POST is more authoritative than a
                    // catalog refresh that may have started before it. Merge
                    // the answer so that refresh can never erase the badge we
                    // just filled for this exact artifact.
                    self.cloud_files.get_or_insert_default().extend(files);
                    self.cloud_check = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.cloud_check = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(portable) = self.portable_hashes.get(hash).cloned().or_else(|| {
            let found = library::portable_hash(hash)?;
            self.portable_hashes.insert(hash.to_owned(), found.clone());
            Some(found)
        }) else {
            return theme::Sync::Unknown;
        };
        // No answer is different from a successful answer containing no
        // hashes. The latter is precisely when every row needs an actionable
        // outline cloud so the first tone can be published.
        cloud_presence(self.cloud_files.as_ref(), &portable)
    }

    fn start_cloud_check(&mut self) {
        let hashes: Vec<String> = self
            .lib_entries
            .iter()
            .filter_map(|entry| {
                let portable = self
                    .portable_hashes
                    .entry(entry.hash.clone())
                    .or_insert_with(|| library::portable_hash(&entry.hash).unwrap_or_default());
                (!portable.is_empty()).then(|| portable.clone())
            })
            .collect();
        self.cloud_check = Some(cloud::published(hashes));
    }

    /// The editing keys every editor answers to. Skipped while something has
    /// keyboard focus: Ctrl+Z inside a text field is the field's own undo.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) || ctx.any_popup_open() {
            return;
        }
        use egui::{Key, KeyboardShortcut, Modifiers};
        const REDO: KeyboardShortcut =
            KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
        const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);

        let pressed = |shortcut: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(shortcut));
        let live = matches!(self.connection, Connection::Online);
        // The shifted variant first, or plain Ctrl+Z would swallow it.
        if pressed(&REDO) {
            if live && self.redo_depth > 0 {
                self.send(Cmd::Redo);
            }
        } else if pressed(&UNDO) && live && self.undo_depth > 0 {
            self.send(Cmd::Undo);
        }
        if pressed(&SAVE) && self.dirty {
            self.send(Cmd::SavePreset);
        }

        // With no control holding the keyboard, the pedal's preset list is
        // the editor's default vertical selection. Exact unmodified arrows
        // matter here: Shift/Ctrl/Alt combinations remain free for controls
        // and future shortcuts.
        let preset_dialog_open = self.confirm_switch.is_some()
            || self.confirm_clear.is_some()
            || self.confirm_push.is_some()
            || self.confirm_delete.is_some()
            || self.name_clash.is_some()
            || self.sending.is_some()
            || self.show_onboarding;
        if live && !preset_dialog_open {
            let direction = ctx.input_mut(|input| {
                if input.modifiers != Modifiers::NONE {
                    return 0;
                }
                if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    -1
                } else if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    1
                } else {
                    0
                }
            });
            if let Some(index) = self.adjacent_preset(direction) {
                self.reveal_preset = true;
                self.request_preset(index);
            }
        }
    }

    /// Undo and redo, where you can see them.
    /// Carry out what the right-click menu asked for.
    ///
    /// Each of these acts on one preset. Anything that needs the device's own
    /// document - copy, paste, export - goes to the preset first, because the
    /// device hands back and takes the loaded one, and asks first when that
    /// would throw unsaved changes away; the rest are local.
    fn row_action(&mut self, index: i64, action: RowAction) {
        match action {
            RowAction::Copy => self.on_preset(index, AfterSwitch::Copy(CopyTarget::Clipboard)),
            RowAction::Paste => {
                if let Some((name, blob)) = self.clipboard.clone() {
                    self.on_preset(index, AfterSwitch::Paste(name, blob));
                }
            }
            RowAction::Export => {
                let name = self
                    .presets
                    .get(index as usize)
                    .cloned()
                    .unwrap_or_else(|| "preset".to_owned());
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{}.hxpreset", sanitise(&name)))
                    .add_filter("HX preset", &["hxpreset"])
                    .save_file()
                {
                    self.on_preset(index, AfterSwitch::Copy(CopyTarget::File(path)));
                }
            }
            RowAction::Import => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("HX tone", &["hxpreset", "hlx"])
                    .pick_file()
                {
                    // Looking at a file switches nothing. The preview opens
                    // aimed at this row, and its Load is what goes there.
                    self.open_tone_file_for(&path, index);
                }
            }
            // The bytes are already on this machine - the automatic backup
            // holds every slot - so keeping from a row does not have to select
            // the preset first. It used to, which threw away the edit buffer to
            // save a preset the person was not even looking at.
            RowAction::Keep => match self.slot_document(index) {
                Some((name, bytes)) => self.keep_tone(&name, "hxpreset", &bytes),
                None => self.on_preset(index, AfterSwitch::Copy(CopyTarget::Library)),
            },
            // The directed version, for when the library already has this name
            // holding something else: no question, because the dot has already
            // said what it would do. The tone that held the name goes, its notes
            // come across, and any setlist playing it is untouched.
            RowAction::Update => {
                let Some((name, bytes)) = self.slot_document(index) else {
                    return self.row_action(index, RowAction::Keep);
                };
                let Some(old) = library::named(&name) else {
                    return self.row_action(index, RowAction::Keep);
                };
                let outcome = library::store(&name, &bytes, "hxpreset")
                    .and_then(|hash| library::override_with(&old.hash, &hash, &name));
                match outcome {
                    Ok(()) => {
                        self.lib_showing = LibraryView::Tones;
                        self.refresh_library();
                        self.note(format!("updated {name} from the pedal"));
                    }
                    Err(why) => self.note(why),
                }
            }
            // The only row action that asks first, and the only one that cannot
            // be taken back.
            RowAction::Remove => self.confirm_clear = Some(index),
        }
    }

    /// Load a preset, saying so: it takes about a second, and a window that
    /// does not change for a second looks like it missed the click.
    fn load_preset(&mut self, index: i64) {
        self.loading = true;
        self.preset_index = index;
        self.send(Cmd::SelectPreset(index));
    }

    /// Throw away the edit buffer by asking the pedal to load its stored copy
    /// of the preset again. This is intentionally the same device operation as
    /// selecting a preset, even when the selected slot does not change.
    fn discard_changes(&mut self) {
        if self.dirty && self.preset_index >= 0 {
            self.load_preset(self.preset_index);
        }
    }

    /// The next visible preset in the requested direction. Favourites mode is
    /// a filtered list, so keyboard navigation follows what is actually on
    /// screen rather than landing on a hidden slot.
    fn adjacent_preset(&self, direction: i64) -> Option<i64> {
        if direction == 0 {
            return None;
        }
        let total = if self.presets.is_empty() {
            self.preset_count as i64
        } else {
            self.presets.len() as i64
        };
        let visible =
            |index| !self.show_favorites_only || self.config.is_favorite(self.setlist, index);
        if direction < 0 {
            let before = self.preset_index.clamp(0, total);
            (0..before).rev().find(|index| visible(*index))
        } else {
            let after = (self.preset_index + 1).max(0);
            (after..total).find(|index| visible(*index))
        }
    }

    /// Select a preset through the same unsaved-changes gate whether the
    /// request came from a mouse click or an arrow key. The preset already
    /// loaded goes through it too: loading it again reloads it from the
    /// pedal, which throws the changes away all the same.
    fn request_preset(&mut self, index: i64) {
        self.on_preset(index, AfterSwitch::Load);
    }

    /// Go to a preset and do something there.
    ///
    /// Going to another preset throws away the loaded one's unsaved changes,
    /// so it asks first, whatever asked for it. On the preset already loaded
    /// nothing is lost - a copy reads the edit buffer as it is, a paste or a
    /// load is one more edit that undo can take back - so those go at once.
    fn on_preset(&mut self, index: i64, then: AfterSwitch) {
        if index == self.preset_index && !matches!(then, AfterSwitch::Load) {
            for cmd in self.commands_for(index, then) {
                self.send(cmd);
            }
        } else if self.dirty {
            self.confirm_switch = Some(PendingSwitch { index, then });
        } else {
            self.switch_to(PendingSwitch { index, then }, false);
        }
    }

    /// What to send to carry `then` out on `index`, once the pedal is there.
    fn commands_for(&mut self, index: i64, then: AfterSwitch) -> Vec<Cmd> {
        match then {
            AfterSwitch::Load => Vec::new(),
            AfterSwitch::Copy(target) => {
                self.pending_copy = target;
                vec![Cmd::CopyPreset]
            }
            AfterSwitch::Paste(name, blob) => {
                self.note(format!("pasting {name}"));
                // The name travels with the tone. A document does not carry
                // one - the slot's label is a separate thing in flash - so
                // pasting without this left the new tone under the old one's
                // name. The label changes now and the chain follows when you
                // save, which is the same bargain every other edit on this bar
                // makes.
                vec![Cmd::PastePreset(blob), Cmd::Rename { index, name }]
            }
            AfterSwitch::Tone(preview) => {
                self.loading = true;
                let slot = self.active_slot_label(index);
                self.note(format!("loading {} into {slot}", preview.name));
                let Preview { name, load, .. } = *preview;
                vec![match load {
                    LoadKind::Document(bytes) => Cmd::LoadDocument { dest: index, bytes },
                    LoadKind::Steps(blocks) => Cmd::LoadSteps {
                        dest: index,
                        name,
                        blocks,
                    },
                }]
            }
        }
    }

    /// Switch to a preset, with what waits on it, as one request: the worker
    /// runs the rest only once the switch has worked, and with `save` only
    /// switches once the save has. As separate requests, a save that failed
    /// was followed by the switch that threw the changes away, and a refused
    /// switch by a paste onto the preset still loaded.
    fn switch_to(&mut self, switch: PendingSwitch, save: bool) {
        let index = switch.index;
        let then = self.commands_for(index, switch.then);
        if !save && then.is_empty() {
            // A plain switch stays one, which the worker can collapse while
            // somebody rides the list with the arrow keys.
            return self.load_preset(index);
        }
        self.loading = true;
        self.preset_index = index;
        self.send(Cmd::Switch { index, save, then });
    }

    /// Ask before a preset switch throws away unsaved changes.
    fn confirm_switch_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.confirm_switch.as_ref().map(|switch| switch.index) else {
            return;
        };
        let consequence = if index == self.preset_index {
            "Loading it again from the pedal discards them.".to_owned()
        } else {
            let going_to = self
                .presets
                .get(index as usize)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| self.active_slot_label(index));
            format!("Loading {going_to} discards them.")
        };
        let mut answer = None;
        let (_, close) = theme::dialog(ctx, "confirm-switch", 460.0, |ui| {
            theme::dialog_header(
                ui,
                &format!("“{}” has changes you have not saved", self.preset_name),
                Some(&consequence),
            );
            theme::dialog_footer(ui, "Save writes them into the preset first.", |ui| {
                if theme::Button::new("Save, then load")
                    .primary()
                    .show(ui)
                    .clicked()
                {
                    answer = Some(SwitchAnswer::Save);
                }
                if theme::Button::new("Discard and load")
                    .tone(theme::Tone::DangerLine)
                    .show(ui)
                    .clicked()
                {
                    answer = Some(SwitchAnswer::Discard);
                }
                if theme::Button::new("Cancel").show(ui).clicked() {
                    answer = Some(SwitchAnswer::Cancel);
                }
            });
        });
        if close && answer.is_none() {
            answer = Some(SwitchAnswer::Cancel);
        }
        if let Some(answer) = answer {
            self.answer_switch(answer);
        }
    }

    /// Carry out the answer about unsaved changes.
    fn answer_switch(&mut self, answer: SwitchAnswer) {
        let Some(switch) = self.confirm_switch.take() else {
            return;
        };
        match answer {
            // A previewed tone goes back on screen, still waiting for Load.
            SwitchAnswer::Cancel => {
                if let AfterSwitch::Tone(preview) = switch.then {
                    self.preview = Some(*preview);
                }
            }
            SwitchAnswer::Discard => self.switch_to(switch, false),
            SwitchAnswer::Save => self.switch_to(switch, true),
        }
    }

    /// Ask the pedal for a tempo, if it is one a preset can hold.
    ///
    /// The field takes whatever parses and a tap can come out anywhere from
    /// 20 to 999, but the pedal stores 40 to 240 BPM, so anything else is
    /// answered here. The reading changes when the reload after the write
    /// brings the pedal's own value back: shown at once, a refused tempo
    /// stayed on screen as though it had been set.
    fn set_tempo(&mut self, bpm: f32) {
        if !session::tempo_fits(bpm) {
            return self.problem(session::tempo_refusal());
        }
        self.edit(Cmd::SetTempo(bpm));
    }

    /// Keep the loaded preset's document in the library, as the device's own
    /// bytes.
    ///
    /// It used to keep `.hlx` where the catalog could name the models, which
    /// reads nicely and loses things: `.hlx` is a list of parameter values, so
    /// the snapshots and the routing did not survive the trip. A library whose
    /// tones come back different from what went in is not a library. The bytes
    /// go in verbatim, and `.hlx` stays available on export for anyone who
    /// wants the readable form.
    fn keep_tone(&mut self, name: &str, ext: &str, blob: &[u8]) {
        match library::keep(name, ext, blob) {
            Ok((_, library::Keeping::Kept)) => {
                self.lib_showing = LibraryView::Tones;
                self.refresh_library();
                self.note(format!("kept {name} in the library"));
            }
            // Keeping a tone that is already kept is not a mistake and not an
            // error; it just has nothing to do. Saying which name it is under
            // answers the question the person was really asking.
            Ok((_, library::Keeping::Already(under))) => {
                self.lib_showing = LibraryView::Tones;
                self.note(if under == name {
                    format!("{name} is already in your library")
                } else {
                    format!("that tone is already in your library, as “{under}”")
                });
            }
            // A name in use by different bytes is the one thing the library
            // will not decide on its own.
            Ok((hash, library::Keeping::NameTaken { holder })) => {
                self.name_clash = Some(NameClash {
                    hash,
                    draft: format!("{holder} 2"),
                    holder,
                    cloud_meta: None,
                    return_view: None,
                });
            }
            Err(why) => self.note(why),
        }
    }

    /// The question a taken name asks, and the two answers to it.
    ///
    /// New version and Save as, never a silent "-2": which of the two you meant is
    /// not something a program can work out, and getting it wrong either buries
    /// a tone you wanted or keeps one you did not.
    fn name_clash_window(&mut self, ctx: &egui::Context) {
        let Some(clash) = self.name_clash.as_mut() else {
            return;
        };
        let holder = clash.holder.clone();
        let hash = clash.hash.clone();
        let mut decided = None;
        let (_, close) = theme::dialog(ctx, "name-clash", 460.0, |ui| {
            theme::dialog_header(
                ui,
                "That name is taken",
                Some(&format!(
                    "Your library already has a different tone called “{holder}”."
                )),
            );
            let free = library::name_is_free(&clash.draft, &hash);
            theme::dialog_body(ui, |ui| {
                theme::label(
                    ui,
                    "Save new version keeps both revisions under that tone. Existing setlists \
                     go on playing the exact revision they saved.",
                    theme::regular(12.5),
                    theme::muted(),
                );
                ui.add_space(6.0);
                theme::caption(ui, "Or save as");
                ui.add(
                    egui::TextEdit::singleline(&mut clash.draft)
                        .desired_width(f32::INFINITY)
                        .hint_text("a name of its own"),
                );
                if !free {
                    theme::label(
                        ui,
                        "That name is taken too.",
                        theme::regular(12.0),
                        theme::hot(),
                    );
                }
            });
            theme::dialog_footer(ui, "", |ui| {
                let named = !clash.draft.trim().is_empty() && free;
                if theme::Button::new("Save as")
                    .primary()
                    .enabled(named)
                    .show(ui)
                    .clicked()
                {
                    decided = Some(Clash::SaveAs(clash.draft.trim().to_owned()));
                }
                if theme::Button::new("Save new version").show(ui).clicked() {
                    decided = Some(Clash::Override);
                }
                if theme::Button::new("Cancel").show(ui).clicked() {
                    decided = Some(Clash::Cancel);
                }
            });
        });
        if close && decided.is_none() {
            decided = Some(Clash::Cancel);
        }

        let Some(decided) = decided else { return };
        let clash = self.name_clash.take().expect("checked above");
        let kept_name = match &decided {
            Clash::Override => Some(clash.holder.clone()),
            Clash::SaveAs(name) => Some(name.clone()),
            Clash::Cancel => None,
        };
        let outcome = match decided {
            // Cancelling leaves the bytes in the store with nothing pointing at
            // them, which is exactly what the sweep is for.
            Clash::Cancel => {
                library::collect_garbage();
                return;
            }
            Clash::Override => library::named(&clash.holder)
                .ok_or_else(|| "that tone is no longer in the library".to_owned())
                .and_then(|old| {
                    library::override_with(&old.hash, &clash.hash, &clash.holder)
                        .map(|()| format!("saved a new version of “{}”", clash.holder))
                }),
            Clash::SaveAs(name) => {
                library::adopt(&clash.hash, &name).map(|()| format!("kept {name} in the library"))
            }
        };
        match outcome {
            Ok(said) => {
                if let (Some(mut meta), Some(name)) = (clash.cloud_meta, kept_name) {
                    meta.name = name;
                    if let Err(why) = library::save_meta(&clash.hash, &meta) {
                        self.note(why);
                    }
                }
                self.lib_showing = clash.return_view.unwrap_or(LibraryView::Tones);
                self.refresh_library();
                self.note(said);
            }
            Err(why) => self.note(why),
        }
    }

    /// What the library knows of the StompStation PRO's loaded preset, for
    /// the card beside it on a large window.
    fn pro_library_note(&self) -> pro::LibraryNote {
        let Some((name, sync)) = self.pro.loaded(&self.library_lookup) else {
            return pro::LibraryNote::default();
        };
        let Some(entry) = self
            .lib_entries
            .iter()
            .find(|entry| entry.pro && entry.name.trim().eq_ignore_ascii_case(name.trim()))
        else {
            return pro::LibraryNote {
                sync,
                ..pro::LibraryNote::default()
            };
        };
        let meta = &entry.meta;
        pro::LibraryNote {
            kept: Some((entry.name.clone(), entry.version)),
            rows: [
                ("Song", meta.song.clone()),
                ("Artist", meta.artist.clone()),
                ("Part", meta.part.clone()),
                ("Guitar", meta.guitar.clone()),
                ("Tags", meta.tags.join(", ")),
            ]
            .into_iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .collect(),
            sync,
        }
    }

    /// Keep a captured pedal in the library as a setlist.
    ///
    /// Every preset becomes a library tone in its own right - they are the
    /// library's now, not the setlist's, and any of them can be dropped into
    /// any other slot later. The setlist records the order, which is the part
    /// that was only ever on the pedal.
    fn keep_setlist(&mut self, slots: Vec<(String, Option<Vec<u8>>)>, kind: &str) {
        let then_push = self.push_after_capture.take();
        let mut kept = Vec::with_capacity(slots.len());
        let mut failures = 0;
        let device_label = if self.device.trim().is_empty() {
            if kind == "vxpreset" {
                "StompStation PRO"
            } else {
                "HX"
            }
        } else {
            self.device.trim()
        }
        .to_owned();
        for (name, bytes) in slots {
            let Some(bytes) = bytes else {
                kept.push(library::Slot::default());
                continue;
            };
            // A full capture cannot stop for 126 name questions. A changed
            // preset with an existing name is the next revision of that tone;
            // identical bytes remain the same content-addressed revision.
            let captured = library::keep(&name, kind, &bytes).and_then(|(hash, how)| {
                if matches!(how, library::Keeping::NameTaken { .. }) {
                    let old = library::named(&name)
                        .ok_or_else(|| "that tone disappeared during capture".to_owned())?;
                    let old_kind = library::kind(&old.hash).unwrap_or_default();
                    let same_family = (kind == "vxpreset") == (old_kind == "vxpreset");
                    if same_family {
                        library::override_with(&old.hash, &hash, &old.meta.name)?;
                    } else {
                        // Names are unique in the shared library, but a factory
                        // name is not a cross-device identity. Keep both and
                        // qualify only the computer-side label; the setlist
                        // below retains the exact name the pedal displays.
                        let base = format!("{name} · {device_label}");
                        let mut candidate = base.clone();
                        let mut suffix = 2;
                        while library::named(&candidate).is_some() {
                            candidate = format!("{base} {suffix}");
                            suffix += 1;
                        }
                        let (_, kept) = library::keep(&candidate, kind, &bytes)?;
                        if !matches!(kept, library::Keeping::Kept | library::Keeping::Already(_)) {
                            return Err("could not choose a distinct cross-device name".into());
                        }
                    }
                }
                Ok(hash)
            });
            match captured {
                Ok(hash) => kept.push(library::Slot::new(&hash, &name)),
                Err(_) => {
                    failures += 1;
                    kept.push(library::Slot::default());
                }
            }
        }

        let name = self
            .capture_as
            .take()
            .unwrap_or_else(|| self.setlist_draft_name());
        let setlist = library::Setlist {
            name,
            slots: kept,
            ..Default::default()
        };
        if let Some(push) = then_push {
            return self.keep_then_push(setlist, failures, &push);
        }
        self.setlist_save = Some(SetlistSave {
            draft: setlist.name.clone(),
            setlist,
            failures,
        });
        self.lib_showing = LibraryView::Setlists;
    }

    /// Save the pedal as it was, then write the setlist the confirmation
    /// asked for. The write waits for the copy: a capture that lost presets,
    /// or a setlist that could not be saved, stops it.
    fn keep_then_push(
        &mut self,
        mut kept: library::Setlist,
        failures: usize,
        push: &library::Setlist,
    ) {
        if failures > 0 {
            self.note(format!(
                "{failures} presets could not be kept, so “{}” was not written",
                push.name
            ));
            self.setlist_save = Some(SetlistSave {
                draft: kept.name.clone(),
                setlist: kept,
                failures,
            });
            return;
        }
        kept.description = format!("The pedal as it was before “{}” was written.", push.name);
        match library::save_setlist_version(&kept) {
            Ok((_, saved)) => {
                self.note(format!(
                    "kept {} presets as “{}” v{}",
                    saved.filled(),
                    saved.name,
                    saved.revision()
                ));
                self.refresh_library();
                self.push_setlist(push);
            }
            Err(why) => self.note(format!("{why}, so “{}” was not written", push.name)),
        }
    }

    /// The useful first suggestion for a freshly captured setlist. Keeping the
    /// same name is now intentional: it creates another revision.
    fn setlist_draft_name(&self) -> String {
        if self.device.is_empty() {
            "Setlist".to_owned()
        } else {
            self.device.clone()
        }
    }

    fn save_setlist_window(&mut self, ctx: &egui::Context) {
        let Some(saving) = self.setlist_save.as_mut() else {
            return;
        };
        let mut decided = None;
        let query = saving.draft.trim().to_ascii_lowercase();
        let mut names: Vec<String> = self
            .lib_setlists
            .iter()
            .map(|(_, setlist)| setlist.name.clone())
            .filter(|name| query.is_empty() || name.to_ascii_lowercase().contains(&query))
            .collect();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let setlists = &self.lib_setlists;
        let (_, close) = theme::dialog(ctx, "save-setlist", 460.0, |ui| {
            theme::dialog_header(
                ui,
                "Save the captured setlist",
                Some(
                    "Choose an existing name to save a new version, or type a new name to \
                     start a setlist.",
                ),
            );
            let mut submit = false;
            theme::dialog_body(ui, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut saving.draft)
                        .desired_width(f32::INFINITY)
                        .hint_text("setlist name"),
                );
                submit =
                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if !names.is_empty() {
                    ui.add_space(4.0);
                    theme::caption(ui, "Matching setlists");
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .show(ui, |ui| {
                            for name in &names {
                                let revisions = setlists
                                    .iter()
                                    .filter(|(_, setlist)| setlist.name.eq_ignore_ascii_case(name))
                                    .count();
                                if ui
                                    .selectable_label(
                                        saving.draft.eq_ignore_ascii_case(name),
                                        format!(
                                            "{name}  ·  {revisions} version{}",
                                            if revisions == 1 { "" } else { "s" }
                                        ),
                                    )
                                    .clicked()
                                {
                                    saving.draft.clone_from(name);
                                }
                            }
                        });
                }
            });
            theme::dialog_footer(ui, "", |ui| {
                let named = !saving.draft.trim().is_empty();
                if (theme::Button::new("Save version")
                    .primary()
                    .enabled(named)
                    .show(ui)
                    .clicked()
                    || submit)
                    && named
                {
                    decided = Some(true);
                }
                if theme::Button::new("Cancel").show(ui).clicked() {
                    decided = Some(false);
                }
            });
        });
        if close && decided.is_none() {
            decided = Some(false);
        }

        match decided {
            Some(false) => self.setlist_save = None,
            Some(true) => {
                let mut saving = self.setlist_save.take().expect("checked above");
                saving.setlist.name = saving.draft.trim().to_owned();
                match library::save_setlist_version(&saving.setlist) {
                    Ok((path, saved)) => {
                        self.note(format!(
                            "kept {} presets as “{}” v{}",
                            saved.filled(),
                            saved.name,
                            saved.revision()
                        ));
                        if saving.failures > 0 {
                            self.note(format!(
                                "{} presets could not be written to the library",
                                saving.failures
                            ));
                        }
                        self.refresh_library();
                        self.lib_setlist = self
                            .lib_setlists
                            .iter()
                            .position(|(known, _)| known == &path);
                        if let Some(i) = self.lib_setlist {
                            self.select_setlist_entry(i);
                        }
                        // The setlist just named is the thing to look at.
                        self.go_to(shell::Page::Library);
                    }
                    Err(why) => {
                        self.note(why);
                        self.setlist_save = Some(saving);
                    }
                }
            }
            None => {}
        }
    }

    /// Put the preset list into a picking state for one library tone.
    fn start_sending(&mut self, row: usize) {
        let Some(entry) = self.lib_entries.get(row) else {
            return;
        };
        if !self.pedal_online() {
            return self.note("no pedal to send to".into());
        }
        if !self.tone_kind_compatible(&entry.hash) {
            return self.note(format!("{} is for a different pedal family", entry.name));
        }
        self.sending = Some(Sending {
            hash: entry.hash.clone(),
            name: entry.name.clone(),
        });
    }

    /// Write the tone being sent into the slot that was picked.
    ///
    /// No confirmation window. The row itself said what it was holding before
    /// it was clicked, which is the same information a window would have shown
    /// and one fewer thing between a person and the thing they meant.
    fn finish_sending(&mut self, slot: i64) {
        let Some(sending) = self.sending.take() else {
            return;
        };
        let Some(bytes) = library::read(&sending.hash) else {
            return self.note(format!("{} is missing from the library", sending.name));
        };
        self.note(format!(
            "writing {} to {}",
            sending.name,
            self.active_slot_label(slot)
        ));
        if self.pro_active() {
            self.pro.send_tone(slot as usize, sending.name, bytes);
        } else {
            self.send(Cmd::PushSetlist(vec![(slot, Some((sending.name, bytes)))]));
        }
    }

    /// A slot's name and its bytes, out of the automatic backup.
    ///
    /// `None` when there is no backup to read, or when the slot in question is
    /// the one being edited right now and the edit buffer has moved on: the
    /// bundle holds what was saved, and what a person means by "keep this" is
    /// what they can hear. That case goes the long way, through the device.
    fn slot_document(&self, index: i64) -> Option<(String, Vec<u8>)> {
        if index == self.preset_index && self.dirty {
            return None;
        }
        let dir = session::automatic_dir()?;
        let bytes = std::fs::read(hx_usb::backup::slot_files(&dir).get(&(index as usize))?).ok()?;
        let name = self.presets.get(index as usize).cloned()?;
        Some((name, bytes))
    }

    /// Re-read what the pedal is holding, from the automatic backup.
    ///
    /// Whole rather than one slot at a time: 126 small files hashed is under a
    /// millisecond, and a mirror that is rebuilt entirely cannot drift, which
    /// a mirror patched in six places eventually would.
    fn refresh_mirror(&mut self) {
        self.read_automatic_backup();
        let Some(dir) = session::automatic_dir() else {
            return;
        };
        self.mirror = hx_usb::backup::slot_files(&dir)
            .into_iter()
            .filter_map(|(slot, path)| {
                let bytes = std::fs::read(path).ok()?;
                Some((slot as i64, library::hash_of(&bytes)))
            })
            .collect();
    }

    /// Whether the library has what the pedal is holding in a slot, and if not,
    /// whether it has something else under the same name.
    fn slot_sync(&self, index: i64) -> theme::Sync {
        let Some(hash) = self.mirror.get(&index) else {
            // No backup yet, or an empty slot. Either way there is nothing
            // truthful to say.
            return theme::Sync::Unknown;
        };
        let name = self
            .presets
            .get(index as usize)
            .map(String::as_str)
            .unwrap_or_default()
            .trim();
        self.library_lookup.sync(hash, name)
    }

    /// The same question from the library's side: is this tone on the pedal?
    fn tone_sync(&self, hash: &str, name: &str) -> theme::Sync {
        if !self.tone_kind_compatible(hash) {
            return theme::Sync::Unknown;
        }
        if self.pro_active() {
            return self.pro.tone_sync(hash, name);
        }
        if self.mirror.is_empty() {
            return theme::Sync::Unknown;
        }
        if self.mirror.values().any(|h| h == hash) {
            return theme::Sync::Same;
        }
        if self.presets.iter().any(|n| n == name) {
            theme::Sync::Differs
        } else {
            theme::Sync::Absent
        }
    }

    /// Rebuild the library rows from the files and the saved index, keeping the
    /// current selection pinned to its file across the refresh.
    fn refresh_library(&mut self) {
        // The setlists come along: they are the same library, and a capture
        // that did not appear until something else refreshed would read as a
        // capture that failed.
        let open = self
            .lib_setlist
            .and_then(|i| self.lib_setlists.get(i))
            .map(|(path, _)| path.clone());
        self.lib_setlists = library::setlists();
        self.lib_setlist = open.and_then(|path| {
            self.lib_setlists
                .iter()
                .position(|(known, _)| *known == path)
        });

        let selected = self
            .lib_selected
            .and_then(|i| self.lib_entries.get(i))
            .map(|e| e.hash.clone());
        let disk_entries = library::entries();
        let metadata = library::metadata();

        let indexed = metadata
            .keys()
            .filter(|hash| library::holds(hash))
            .cloned()
            .collect();
        // A setlist may pin an old tone revision that is no longer the
        // library's current row. Read each distinct document once now, not
        // every frame and not once for every setlist that happens to use it.
        let setlist_hashes: std::collections::BTreeSet<String> = self
            .lib_setlists
            .iter()
            .flat_map(|(_, setlist)| setlist.slots.iter().map(|slot| slot.hash.clone()))
            .filter(|hash| !hash.is_empty())
            .collect();
        let mut facts = std::collections::HashMap::new();
        let mut setlist_tones = std::collections::HashMap::new();
        for hash in setlist_hashes {
            let held = library::holds(&hash);
            let fact = if held {
                self.library_facts(&hash)
            } else {
                (String::new(), String::new(), Vec::new())
            };
            facts.insert(hash.clone(), fact.clone());
            setlist_tones.insert(
                hash.clone(),
                SetlistTone {
                    held,
                    meta: metadata.get(&hash).cloned().unwrap_or_default(),
                },
            );
        }

        let mut entries = Vec::with_capacity(disk_entries.len());
        for entry in disk_entries {
            let (derived, line, chain) = facts
                .get(&entry.hash)
                .cloned()
                .unwrap_or_else(|| self.library_facts(&entry.hash));
            let pro = library::kind(&entry.hash).as_deref() == Some("vxpreset");
            // The recorded name wins: it is what the pedal shows, colon and
            // all, where a name read back off the document may not be.
            let name = if entry.meta.name.is_empty() {
                derived
            } else {
                entry.meta.name.clone()
            };
            entries.push(LibEntry {
                hash: entry.hash,
                series: entry.series,
                name,
                line,
                added_at: entry.meta.added_at.clone(),
                modified_at: entry.meta.modified_at.clone(),
                downloads: None,
                rating: (entry.meta.rating > 0).then_some(entry.meta.rating as f32),
                version: entry.version,
                versions: entry.versions,
                meta: entry.meta,
                chain,
                pro,
            });
        }
        self.lib_entries = entries;
        self.library_lookup = LibraryLookup {
            indexed,
            setlist_tones,
            ..Default::default()
        };
        self.library_lookup.reindex(&self.lib_entries);
        self.lib_selected =
            selected.and_then(|h| self.lib_entries.iter().position(|e| e.hash == h));
        self.write_portable_copies();
    }

    /// Write the portable copy of every tone that has not got one.
    ///
    /// Both formats, always, rather than on request. A `.hxpreset` is the
    /// device's own document and the only thing that restores a tone exactly;
    /// a `.hlx` is what HX Edit, CustomTone and anything else can read. Keeping
    /// one without the other makes the library either lossy or a place tones
    /// cannot leave, and which of the two you need is not knowable in advance.
    ///
    /// Idempotent and quiet: it writes the ones that are missing, which after
    /// the first pass is the one just kept.
    fn write_portable_copies(&mut self) {
        let Some(catalog) = self.catalog.as_ref() else {
            // No catalog, no symbol names, no honest `.hlx`. The byte-exact
            // copy is already safe on disk; this can wait for the model data.
            return;
        };
        let mut written = 0;
        let mut refused = 0;
        for (hash, name) in library::awaiting_portable() {
            let Some(bytes) = library::read(&hash) else {
                continue;
            };
            let Some(preset) = hx_proto::preset::Preset::parse(&bytes) else {
                refused += 1;
                continue;
            };
            let hlx = hx_catalog::to_hlx(&preset, catalog, &name).to_pretty_string();
            if library::attach_portable(&hash, &hlx).is_ok() {
                written += 1;
            }
        }
        if written > 0 {
            self.log.push(format!("wrote {written} portable copies"));
        }
        if refused > 0 {
            self.log
                .push(format!("{refused} tones could not be read as presets"));
        }
    }

    /// Write the HX Edit bundle beside one of ours.
    ///
    /// The same bargain as a tone: `.hxbundle` is the pedal's own bytes and is
    /// what a restore should use, `.hxb` is what HX Edit will open. One is
    /// exact, the other travels, and there is no reason to make a person choose
    /// between them after the fact.
    fn write_hxb_beside(&mut self, bundle: &std::path::Path) {
        let Some(catalog) = self.catalog.as_ref() else {
            return;
        };
        let (manifest, presets, globals) = match hx_usb::backup::for_export(bundle) {
            Ok(export) => export,
            Err(error) => {
                self.log.push(format!("could not read the backup: {error}"));
                return;
            }
        };
        let (device, device_version, captured) = match hx_usb::backup::hxb_metadata(&manifest) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.log.push(format!(
                    "could not describe the backup for HX Edit: {error}"
                ));
                return;
            }
        };
        let profile = hx_proto::PROFILES
            .iter()
            .find(|profile| profile.device_id == device)
            .expect("hxb_metadata returns a recognised device profile");
        let tones: Vec<(String, Option<serde_json::Value>)> = presets
            .into_iter()
            .map(|(name, bytes)| {
                let tone = bytes
                    .and_then(|b| hx_proto::preset::Preset::parse(&b))
                    .map(|p| {
                        hx_catalog::to_hlx_for_device(&p, catalog, profile, &name).document["data"]
                            ["tone"]
                            .clone()
                    });
                (name, tone)
            })
            .collect();
        let bytes = hx_catalog::write_backup(&hx_catalog::NewBackup {
            setlist: manifest
                .setlists
                .first()
                .map(String::as_str)
                .unwrap_or("PRESETS"),
            presets: &tones,
            globals,
            device,
            device_version,
            captured,
        });
        let target = bundle.with_extension("hxb");
        match library::atomic_write(&target, &bytes) {
            Ok(()) => self.log.push(format!("wrote {}", target.display())),
            Err(e) => self.log.push(format!("could not write the .hxb: {e}")),
        }
    }

    /// The tone name and a one-line chain reading for a stored tone, read
    /// through the same codec the preview uses.
    fn library_facts(&self, hash: &str) -> (String, String, Vec<shell::Mini>) {
        let fallback = library::short(hash).to_owned();
        let Some(bytes) = library::read(hash) else {
            return (fallback, String::new(), Vec::new());
        };
        if library::kind(hash).as_deref() == Some("vxpreset") {
            return (
                fallback,
                pro::preset_content(&bytes).unwrap_or_default(),
                pro::preset_minis(&bytes),
            );
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return (fallback, String::new(), Vec::new());
        };
        if library::kind(hash).as_deref() == Some("hlx") {
            let tone = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .map(|json| hx_catalog::inspect(&json, catalog));
            return match tone {
                Some(t) => {
                    // A file says each block's path and place, not its lanes:
                    // its strip is the blocks in order.
                    let mut blocks: Vec<_> = t.blocks.iter().collect();
                    blocks.sort_by_key(|block| (block.path, block.position));
                    let chain = blocks
                        .iter()
                        .map(|block| shell::Mini::Block {
                            colour: block
                                .category
                                .and_then(theme::category_name)
                                .map_or_else(theme::muted, theme::category_colour),
                            on: block.enabled,
                        })
                        .collect();
                    (t.name.clone(), Self::tone_content(&t).to_owned(), chain)
                }
                None => (fallback, String::new(), Vec::new()),
            };
        }
        let Some(preset) = hx_proto::preset::Preset::parse(&bytes) else {
            return (fallback, String::new(), Vec::new());
        };
        let tone = hx_catalog::inspect(
            &hx_catalog::to_hlx(&preset, catalog, &fallback).document,
            catalog,
        );
        let chain = shell::minis(&session::chain_of(&preset), &preset.layout(), |block| {
            catalog_colour(catalog, block)
        });
        (
            tone.name.clone(),
            Self::tone_content(&tone).to_owned(),
            chain,
        )
    }

    /// Load a row's metadata into the editable draft.
    fn select_lib_entry(&mut self, i: usize) {
        if let Some(e) = self.lib_entries.get(i) {
            self.lib_selected = Some(i);
            self.lib_draft = e.meta.clone();
            self.lib_genres_buf = e.meta.genres.join(", ");
            self.lib_tag_add.clear();
        }
    }

    fn show_library_view(&mut self, view: LibraryView) {
        if self.lib_showing == LibraryView::Cloud
            && view != LibraryView::Cloud
            && self.auditioning.is_some()
        {
            self.end_audition();
        }
        self.lib_showing = view;
        let scoped_device = self.cloud_device_scope();
        if view == LibraryView::Cloud
            && (self.cloud_loaded_query.as_deref() != Some(self.library_search.trim())
                || self.cloud_loaded_order != Some(self.cloud_order)
                || self.cloud_loaded_device.as_deref() != Some(scoped_device.as_str()))
        {
            self.cloud_search_due = Some(std::time::Instant::now());
        }
    }

    fn refresh_cloud(&mut self) {
        self.cloud_search_due = Some(std::time::Instant::now());
        self.cloud_error = None;
    }

    /// Begin one Cloud page without tying its lifetime to the currently open
    /// library tab. Page one starts a new feed; subsequent pages extend it.
    fn start_cloud_page(&mut self, page: u32, ctx: &egui::Context) {
        let query = self.library_search.trim().to_owned();
        let order = self.cloud_order;
        let device = self.cloud_device_scope();
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        let requested = query.clone();
        let requested_device = device.clone();
        std::thread::spawn(move || {
            let answer = cloud::discover_page(
                &requested,
                order,
                (!requested_device.is_empty()).then_some(requested_device.as_str()),
                page,
            );
            let _ = tx.send(answer);
            repaint.request_repaint();
        });

        if page <= 1 {
            // These fields identify the feed from the moment it starts. Tab
            // switches therefore cannot mistake an in-flight request for an
            // absent one and restart it.
            self.cloud_loaded_query = Some(query.clone());
            self.cloud_loaded_order = Some(order);
            self.cloud_loaded_device = Some(device.clone());
            self.cloud_entries.clear();
            self.cloud_tags.clear();
            self.cloud_selected = None;
            self.cloud_total = None;
            self.cloud_next_page = None;
        }
        self.cloud_error = None;
        self.cloud_searching = Some(CloudSearchJob {
            query,
            order,
            device,
            page,
            answer: rx,
        });
    }

    fn request_next_cloud_page(&mut self, ctx: &egui::Context) {
        if self.cloud_searching.is_none() {
            if let Some(page) = self.cloud_next_page.take() {
                self.start_cloud_page(page, ctx);
            }
        }
    }

    /// Start a due TonePush feed and collect one finished page. Network work is
    /// never allowed onto egui's frame thread; pages keep finishing even while
    /// another library tab is open.
    fn settle_cloud_search(&mut self, ctx: &egui::Context) {
        if self
            .cloud_search_due
            .is_some_and(|due| std::time::Instant::now() >= due)
        {
            self.cloud_search_due = None;
            self.start_cloud_page(1, ctx);
        }

        let answer = {
            let Some(job) = &self.cloud_searching else {
                return;
            };
            match job.answer.try_recv() {
                Ok(answer) => answer,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("TonePush search stopped without an answer".to_owned())
                }
            }
        };
        let job = self
            .cloud_searching
            .take()
            .expect("the Cloud page was just present");
        // A late answer from text that has since changed is not the result on
        // screen. The newer debounce/job will provide that. Taking the stale
        // job first is important: an ignored answer must not leave a dead
        // receiver looking busy forever.
        if job.query != self.library_search.trim()
            || job.order != self.cloud_order
            || job.device != self.cloud_device_scope()
        {
            return;
        }

        match answer {
            Ok(page) => {
                self.cloud_total = Some(page.total);
                self.cloud_next_page = page.next_page;
                for entry in page.entries {
                    if !self
                        .cloud_entries
                        .iter()
                        .any(|known| known.discovered.tone.summary.id == entry.tone.summary.id)
                    {
                        let row = Self::cloud_row(&entry);
                        self.cloud_entries.push(CloudEntry {
                            discovered: entry,
                            row,
                        });
                    }
                }
                self.cloud_tags = self
                    .cloud_entries
                    .iter()
                    .flat_map(|entry| entry.discovered.song.tags.iter().cloned())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                self.cloud_error = None;
            }
            Err(why) => {
                // A failed later page does not erase the useful rows already
                // on screen. Refresh can restart the feed from page one.
                if job.page <= 1 {
                    self.cloud_total = Some(0);
                }
                self.cloud_error = Some(why);
            }
        }
    }

    fn settle_cloud_download(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.cloud_download else {
            return;
        };
        let answer = match job.answer.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the Tone download stopped without an answer".to_owned())
            }
        };
        let job = self
            .cloud_download
            .take()
            .expect("the job was just present");
        match answer {
            Ok(cloud::ToneDelivery::Artifact(bytes)) => {
                self.cloud_artifacts
                    .insert(job.entry.tone.summary.id, bytes.clone());
                self.apply_cloud_artifact(&job.entry, job.action, bytes);
            }
            Ok(cloud::ToneDelivery::External(url)) => {
                ctx.open_url(egui::OpenUrl::new_tab(url));
                self.note(format!(
                    "{} is hosted by its original catalog; opened it there",
                    job.entry.tone.summary.name
                ));
            }
            Err(why) => self.problem(why),
        }
    }

    /// Why this public row cannot currently replace the pedal's edit buffer.
    /// Discovery itself stays global; only this one device-writing action is
    /// compatibility constrained.
    fn cloud_audition_blocker(&self, entry: &cloud::DiscoveredTone) -> Option<&'static str> {
        if !self.pedal_online() {
            return Some("Connect a pedal to audition this Tone");
        }
        if !cloud::compatible_device(&self.device, &entry.tone.summary.device.name) {
            return Some("This Tone is for a different device; keep it on the computer instead");
        }
        if entry
            .tone
            .download
            .as_ref()
            .and_then(|download| download.artifact.as_ref())
            .is_none()
        {
            return Some(
                "The preset file is at its original catalog and cannot be auditioned directly",
            );
        }
        None
    }

    fn start_cloud_action(&mut self, entry: usize, action: CloudAction, ctx: &egui::Context) {
        let Some(entry) = self
            .cloud_entries
            .get(entry)
            .map(|entry| entry.discovered.clone())
        else {
            return;
        };
        self.start_cloud_entry_action(entry, action, ctx);
    }

    fn start_cloud_entry_action(
        &mut self,
        entry: cloud::DiscoveredTone,
        action: CloudAction,
        ctx: &egui::Context,
    ) {
        let id = entry.tone.summary.id;
        if matches!(action, CloudAction::Audition) && self.auditioning == Some(id) {
            self.end_audition();
            return;
        }
        if matches!(action, CloudAction::Audition) {
            if let Some(why) = self.cloud_audition_blocker(&entry) {
                self.note(why.to_owned());
                return;
            }
        }
        if let Some(url) = entry
            .tone
            .download
            .as_ref()
            .and_then(|download| download.external.clone())
        {
            ctx.open_url(egui::OpenUrl::new_tab(url));
            self.note(format!(
                "{} is hosted by its original catalog; opened it there",
                entry.tone.summary.name
            ));
            return;
        }
        if let Some(bytes) = self.cloud_artifacts.get(&id).cloned() {
            self.apply_cloud_artifact(&entry, action, bytes);
            return;
        }
        if let Some(job) = &self.cloud_download {
            self.note(format!(
                "{} is still downloading",
                job.entry.tone.summary.name
            ));
            return;
        }
        let tone = entry.tone.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(cloud::download(&tone));
            repaint.request_repaint();
        });
        self.cloud_download = Some(CloudDownloadJob {
            entry,
            action,
            answer: rx,
        });
    }

    /// Present one historical Cloud artifact through the same keep/audition
    /// path as the current revision. The negative UI key prevents its cached
    /// bytes from ever being mistaken for the stable Tone's current bytes.
    fn cloud_version_entry(
        entry: &cloud::DiscoveredTone,
        version: &cloud::ToneVersion,
    ) -> cloud::DiscoveredTone {
        let mut historical = entry.clone();
        let key = entry
            .tone
            .summary
            .id
            .saturating_mul(1_000)
            .saturating_add(version.number as i64)
            .saturating_neg();
        historical.tone.summary.id = key;
        historical.tone.summary.version_number = Some(version.number);
        historical.tone.file_sha256 = Some(version.file_sha256.clone());
        historical.tone.download = Some(version.download.clone());
        historical
    }

    fn apply_cloud_artifact(
        &mut self,
        entry: &cloud::DiscoveredTone,
        action: CloudAction,
        bytes: Vec<u8>,
    ) {
        match action {
            CloudAction::Computer => {
                let view = self.lib_showing;
                let kind = if voidx_proto::Preset::parse(&bytes).is_ok() {
                    "vxpreset"
                } else if hx_proto::preset::Preset::parse(&bytes).is_some() {
                    "hxpreset"
                } else {
                    "hlx"
                };
                self.keep_tone(&entry.tone.summary.name, kind, &bytes);
                self.lib_showing = view;
                let hash = library::hash_of(&bytes);
                let published = Self::cloud_meta(entry);
                // A name clash stores the object but deliberately does not
                // claim it in the index until the person answers. Do not jump
                // that question by attaching metadata here.
                if library::meta_of(&hash).is_some() {
                    let mut meta = published.clone();
                    meta.name = library::meta_of(&hash)
                        .map(|known| known.name)
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| entry.tone.summary.name.clone());
                    if let Err(why) = library::save_meta(&hash, &meta) {
                        self.note(why);
                    }
                    self.refresh_library();
                } else if let Some(clash) =
                    self.name_clash.as_mut().filter(|clash| clash.hash == hash)
                {
                    clash.cloud_meta = Some(published);
                    clash.return_view = Some(LibraryView::Cloud);
                }
            }
            CloudAction::Audition => {
                let id = entry.tone.summary.id;
                let name = entry.tone.summary.name.clone();
                if self.pro_active() {
                    if voidx_proto::Preset::parse(&bytes).is_err() {
                        return self
                            .problem("this Cloud tone is not a StompStation PRO preset".into());
                    }
                    self.pro.audition(id, name, bytes);
                    return;
                }
                if hx_proto::preset::Preset::parse(&bytes).is_some() {
                    self.send(Cmd::AuditionDocument {
                        key: id,
                        name,
                        bytes,
                    });
                    return;
                }
                match self.steps_from_hlx(&name, &bytes) {
                    Ok((_, blocks)) => self.send(Cmd::AuditionSteps {
                        key: id,
                        name,
                        blocks,
                    }),
                    Err(why) => self.problem(why),
                }
            }
        }
    }

    fn cloud_meta(entry: &cloud::DiscoveredTone) -> library::Meta {
        let value = |name: &str| {
            entry
                .tone
                .parsed_metadata
                .get(name)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        library::Meta {
            name: entry.tone.summary.name.clone(),
            description: entry.song.description.clone().unwrap_or_default(),
            tone_description: entry.tone.summary.description.clone().unwrap_or_default(),
            tags: entry.song.tags.clone(),
            character: value("character").replace('_', "-"),
            genres: entry.song.genres.clone(),
            artist: entry.song.artist.clone().unwrap_or_default(),
            song: entry.song.title.clone(),
            part: entry.song.part.clone().unwrap_or_default(),
            guitar: entry.song.guitar_type.clone().unwrap_or_default(),
            pickup_type: entry
                .song
                .pickup_type
                .clone()
                .unwrap_or_default()
                .replace('_', "-"),
            pickup_electronics: entry.song.pickup_electronics.clone().unwrap_or_default(),
            tuning: entry.song.tuning.clone().unwrap_or_default(),
            gain: value("gain"),
            added_at: String::new(),
            modified_at: String::new(),
            rating: 0,
        }
    }

    fn cloud_chain(entry: &cloud::DiscoveredTone) -> String {
        entry
            .tone
            .summary
            .signal_chain
            .iter()
            .filter_map(|block| block.get("name").and_then(|name| name.as_str()))
            .collect::<Vec<_>>()
            .join(" › ")
    }

    fn cloud_row(entry: &cloud::DiscoveredTone) -> LibEntry {
        LibEntry {
            hash: entry
                .tone
                .file_sha256
                .clone()
                .unwrap_or_else(|| format!("cloud:{}", entry.tone.summary.id)),
            series: entry
                .tone
                .summary
                .series_id
                .clone()
                .unwrap_or_else(|| entry.tone.summary.id.to_string()),
            name: entry.tone.summary.name.clone(),
            line: Self::cloud_chain(entry),
            meta: Self::cloud_meta(entry),
            added_at: entry.tone.summary.created_at.clone(),
            modified_at: entry.tone.summary.updated_at.clone(),
            downloads: Some(entry.tone.summary.installs_count),
            rating: entry.tone.summary.rating,
            version: entry.tone.summary.version_number.unwrap_or(1).max(1),
            versions: entry.tone.summary.versions_count.max(1),
            chain: entry
                .tone
                .summary
                .signal_chain
                .iter()
                .map(|block| shell::Mini::Block {
                    colour: block
                        .get("category")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|id| theme::category_name(id as u32))
                        .map_or_else(theme::muted, theme::category_colour),
                    on: block
                        .get("enabled")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true),
                })
                .collect(),
            pro: entry.tone.summary.device.name.contains("StompStation"),
        }
    }

    fn local_cloud_entry(&mut self, wanted: Option<&str>) -> Option<usize> {
        let wanted = wanted?;
        if let Some(index) = self
            .lib_entries
            .iter()
            .position(|entry| entry.hash == wanted)
        {
            return Some(index);
        }
        for (index, local) in self.lib_entries.iter().enumerate() {
            let portable = self
                .portable_hashes
                .entry(local.hash.clone())
                .or_insert_with(|| library::portable_hash(&local.hash).unwrap_or_default());
            if portable == wanted {
                return Some(index);
            }
        }
        None
    }

    fn cloud_table(&mut self, ui: &mut egui::Ui) {
        if let Some(why) = &self.cloud_error {
            ui.label(RichText::new(why).color(theme::accent()));
            ui.add_space(4.0);
        }
        let filter = self.cloud_tag_filter.clone();
        let source_rows: Vec<usize> = self
            .cloud_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                filter
                    .as_ref()
                    .is_none_or(|tag| entry.discovered.song.tags.contains(tag))
            })
            .map(|(index, _)| index)
            .collect();
        let shown = self.shown_columns();
        let mut grid = table::Grid {
            columns: shown.iter().map(|column| column.column(false)).collect(),
            sort: (
                shown
                    .iter()
                    .position(|column| *column == self.cloud_sort.0)
                    .unwrap_or(0),
                self.cloud_sort.1,
            ),
            sticky: 3.min(shown.len()),
            column_choices: self.column_choices(),
            nothing_yet: if self.cloud_searching.is_some() {
                "Searching TonePush…"
            } else {
                "No published tones match this search."
            },
            ..Default::default()
        };
        for &row in &source_rows {
            let wanted = self.cloud_entries[row].discovered.tone.file_sha256.clone();
            let on_computer = self.local_cloud_entry(wanted.as_deref()).is_some();
            let entry = &self.cloud_entries[row].discovered;
            let local = &self.cloud_entries[row].row;
            let audition_blocker = self.cloud_audition_blocker(entry);
            let downloading = self
                .cloud_download
                .as_ref()
                .filter(|job| job.entry.tone.summary.id == entry.tone.summary.id);
            grid.rows.push(
                shown
                    .iter()
                    .map(|column| match column {
                        LibColumn::Sync => table::Cell::Places(vec![
                            (
                                theme::Icon::Pedal,
                                if audition_blocker.is_some() {
                                    theme::Sync::Unknown
                                } else if downloading
                                    .is_some_and(|job| job.action == CloudAction::Audition)
                                {
                                    theme::Sync::Working
                                } else if self.auditioning == Some(entry.tone.summary.id) {
                                    theme::Sync::Same
                                } else {
                                    theme::Sync::Absent
                                },
                                if self.auditioning == Some(entry.tone.summary.id) {
                                    "Auditioning now. Keep it in the pedal's edit buffer"
                                } else if let Some(why) = audition_blocker {
                                    why
                                } else {
                                    "Audition this Tone on the pedal"
                                },
                                audition_blocker.is_none()
                                    && !downloading
                                        .is_some_and(|job| job.action == CloudAction::Audition),
                            ),
                            (
                                theme::Icon::Computer,
                                if downloading
                                    .is_some_and(|job| job.action == CloudAction::Computer)
                                {
                                    theme::Sync::Working
                                } else if on_computer {
                                    theme::Sync::Same
                                } else {
                                    theme::Sync::Absent
                                },
                                if on_computer {
                                    "In your library"
                                } else {
                                    "Keep this Tone in your library"
                                },
                                !downloading.is_some_and(|job| job.action == CloudAction::Computer),
                            ),
                        ]),
                        _ => column.value_cell(local, self.family_tag(local.pro), false),
                    })
                    .collect(),
            );
            grid.chosen.push(false);
        }

        grid.selected = self
            .cloud_selected
            .and_then(|selected| source_rows.iter().position(|&row| row == selected));
        let order = grid.sort_rows();
        let rows: Vec<usize> = order.iter().map(|&row| source_rows[row]).collect();

        let did = table::show(ui, "cloud-library", &mut grid);
        self.apply_column_visibility(did.column_visibility);
        // The feed stops after page one. Only approaching the end of the rows
        // already painted asks TonePush for another page; leaving this tab does
        // not cancel that request, and returning does not restart the feed.
        if did
            .last_visible
            .is_some_and(|last| last.saturating_add(12) >= grid.rows.len())
        {
            self.request_next_cloud_page(ui.ctx());
        }
        if let Some(column) = did.sort {
            let column = shown[column];
            self.cloud_sort = if column == self.cloud_sort.0 {
                (column, !self.cloud_sort.1)
            } else {
                (column, true)
            };
        }
        if let Some((row, ..)) = did.clicked {
            if let Some(&entry) = rows.get(row) {
                self.cloud_selected = Some(entry);
                self.start_cloud_action(entry, CloudAction::Audition, ui.ctx());
            }
        }
        if let Some((row, place)) = did.place {
            let Some(&entry) = rows.get(row) else { return };
            self.cloud_selected = Some(entry);
            let wanted = self
                .cloud_entries
                .get(entry)
                .and_then(|tone| tone.discovered.tone.file_sha256.clone());
            let local = self.local_cloud_entry(wanted.as_deref());
            match place {
                0 if self.auditioning
                    == self
                        .cloud_entries
                        .get(entry)
                        .map(|entry| entry.discovered.tone.summary.id) =>
                {
                    self.keep_audition()
                }
                0 => self.start_cloud_action(entry, CloudAction::Audition, ui.ctx()),
                _ if local.is_some() => {
                    self.show_library_view(LibraryView::Tones);
                    self.lib_tag_filter = None;
                    self.select_lib_entry(local.expect("checked above"));
                }
                _ => self.start_cloud_action(entry, CloudAction::Computer, ui.ctx()),
            }
        }
    }

    /// Put one preset out of a setlist back into its slot.
    fn send_one_slot(&mut self, slot: i64, entry: &library::Slot) {
        if !self.tone_kind_compatible(&entry.hash) {
            return self.note(format!("{} is for a different pedal family", entry.name));
        }
        match library::read(&entry.hash) {
            Some(bytes) => {
                self.note(format!(
                    "writing {} to {}",
                    entry.name,
                    self.active_slot_label(slot)
                ));
                if self.pro_active() {
                    self.pro.send_tone(slot as usize, entry.name.clone(), bytes);
                } else {
                    self.send(Cmd::PushSetlist(vec![(
                        slot,
                        Some((entry.name.clone(), bytes)),
                    )]));
                }
            }
            None => self.note(format!("{} is missing from the library", entry.name)),
        }
    }

    /// Load a setlist into the editable draft.
    fn select_setlist_entry(&mut self, i: usize) {
        if let Some((_, setlist)) = self.lib_setlists.get(i) {
            self.lib_setlist = Some(i);
            self.lib_setlist_draft = setlist.clone();
        }
    }

    /// Write the draft back, and re-read so the rail follows a rename.
    fn save_setlist_draft(&mut self) {
        let Some(i) = self.lib_setlist else { return };
        let Some((path, _)) = self.lib_setlists.get(i).cloned() else {
            return;
        };
        let (target, saved) = match library::update_setlist(&path, &self.lib_setlist_draft) {
            Ok(saved) => saved,
            Err(why) => {
                self.note(why);
                return;
            }
        };
        self.lib_setlist_draft = saved;
        self.lib_setlists = library::setlists();
        self.lib_setlist = self
            .lib_setlists
            .iter()
            .position(|(known, _)| known == &target);
    }

    /// Writing a setlist replaces every preset on the pedal, in flash, and
    /// there is no undo for that. It asks, and it says how many.
    fn confirm_push_window(&mut self, ctx: &egui::Context) {
        let Some(i) = self.confirm_push else { return };
        let Some((_, setlist)) = self.lib_setlists.get(i).cloned() else {
            self.confirm_push = None;
            return;
        };
        let mut decided = None;
        let pedal = if self.device.trim().is_empty() {
            "the pedal".to_owned()
        } else {
            format!("the {}", self.device.trim())
        };
        let filled = setlist.filled();
        let missing = setlist
            .slots
            .iter()
            .filter(|slot| !slot.is_empty() && !library::holds(&slot.hash))
            .count();
        let empty = setlist.slots.len() - filled;
        let written = filled - missing;
        // What the writes change, slot by slot, when the pedal's backup can
        // say; how many it writes, when it cannot. Either way every slot is
        // written, as the button says.
        let mut rows = match self.setlist_states(&setlist) {
            Some(states) => self.push_outcomes(&states),
            None => vec![
                theme::Outcome {
                    icon: theme::Icon::Download,
                    count: written.to_string(),
                    sentence: if written == 1 {
                        "preset written into its slot".to_owned()
                    } else {
                        "presets written, each into its slot".to_owned()
                    },
                    quiet: false,
                },
                theme::Outcome {
                    icon: theme::Icon::Remove,
                    count: empty.to_string(),
                    sentence: "slots the setlist leaves empty are emptied".to_owned(),
                    quiet: empty == 0,
                },
            ],
        };
        let kept = !self.pro_active() && self.backup_of_this_pedal().is_some();
        let explanation = if kept {
            "TonePush writes every slot of the setlist, in order, over what is on the pedal \
             now. Undo cannot reach the pedal's memory; the pedal as it is now stays in its \
             backups."
        } else {
            "TonePush writes every slot of the setlist, in order, over what is on the pedal \
             now. Undo cannot reach the pedal's memory."
        };
        if missing > 0 {
            rows.push(theme::Outcome {
                icon: theme::Icon::CircleAlert,
                count: missing.to_string(),
                sentence: "tones are missing from the library; their slots are left alone"
                    .to_owned(),
                quiet: true,
            });
        }
        let mut keep_first = self.keep_before_push;
        let (_, close) = theme::dialog(ctx, "confirm-push", 520.0, |ui| {
            theme::dialog_header(
                ui,
                &format!("Put {} on {pedal}?", setlist.name),
                Some(explanation),
            );
            theme::dialog_body(ui, |ui| {
                theme::outcomes(ui, &rows);
                ui.add_space(14.0);
                theme::checkbox(
                    ui,
                    &mut keep_first,
                    "Keep the pedal as it is now as a setlist first",
                );
            });
            theme::dialog_footer(ui, "Keep the pedal connected until it is done.", |ui| {
                let write = format!("Write {} slots", written + empty);
                if theme::Button::new(&write).danger().show(ui).clicked() {
                    decided = Some(true);
                }
                if theme::Button::new("Cancel").show(ui).clicked() {
                    decided = Some(false);
                }
            });
        });
        self.keep_before_push = keep_first;
        if close && decided.is_none() {
            decided = Some(false);
        }
        match decided {
            Some(true) => {
                self.confirm_push = None;
                self.keep_before_push = false;
                self.put_setlist(setlist, keep_first);
            }
            Some(false) => {
                self.confirm_push = None;
                self.keep_before_push = false;
            }
            None => {}
        }
    }

    /// Write the setlist the confirmation agreed to; with `keep_first`, only
    /// once the pedal as it is now is kept as a setlist.
    fn put_setlist(&mut self, setlist: library::Setlist, keep_first: bool) {
        if !keep_first {
            self.push_setlist(&setlist);
        } else if self.capture_pedal(None) {
            // The write follows once the copy is in the library.
            self.push_after_capture = Some(setlist);
        }
    }

    /// Read every tone the setlist names and send the lot to the pedal.
    fn push_setlist(&mut self, setlist: &library::Setlist) {
        if !self.setlist_compatible(setlist) {
            return self
                .note("this setlist contains tones or slots for a different pedal family".into());
        }
        let mut slots = Vec::with_capacity(setlist.slots.len());
        let mut missing = 0;
        for (index, entry) in setlist.slots.iter().enumerate() {
            if entry.is_empty() {
                slots.push((index as i64, None));
                continue;
            }
            match library::read(&entry.hash) {
                Some(bytes) => slots.push((index as i64, Some((entry.name.clone(), bytes)))),
                // A tone that is gone leaves its slot alone rather than
                // emptying it: losing a file should not also lose the preset
                // that is still on the pedal.
                None => missing += 1,
            }
        }
        if missing > 0 {
            self.note(format!("{missing} tones are missing and were skipped"));
        }
        self.note(format!("writing “{}” to the pedal", setlist.name));
        if self.pro_active() {
            self.pro.push_setlist(
                slots
                    .into_iter()
                    .map(|(slot, tone)| (slot as usize, tone))
                    .collect(),
            );
        } else {
            self.send(Cmd::WriteSetlist {
                name: setlist.name.clone(),
                slots,
            });
        }
    }

    /// The middle table: one row per tone, filtered by the chosen tag. Click to
    /// select for the inspector; double-click to open its preview.
    fn library_table(&mut self, ui: &mut egui::Ui) {
        let filter = self.lib_tag_filter.clone();
        let needle = self.tone_search.trim().to_ascii_lowercase();
        let rows: Vec<usize> = self
            .lib_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.tone_in_library_scope(&entry.hash))
            .filter(|(_, entry)| {
                filter
                    .as_ref()
                    .is_none_or(|tag| entry.meta.tags.contains(tag))
            })
            .filter(|(_, entry)| {
                needle.is_empty()
                    || entry.name.to_ascii_lowercase().contains(&needle)
                    || entry.line.to_ascii_lowercase().contains(&needle)
                    || entry.meta.artist.to_ascii_lowercase().contains(&needle)
                    || entry.meta.song.to_ascii_lowercase().contains(&needle)
                    || entry
                        .meta
                        .genres
                        .iter()
                        .chain(&entry.meta.tags)
                        .any(|value| value.to_ascii_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();

        let shown = self.shown_columns();
        let mut grid = table::Grid {
            columns: shown.iter().map(|column| column.column(true)).collect(),
            sort: (
                shown
                    .iter()
                    .position(|c| *c == self.lib_sort.0)
                    .unwrap_or(0),
                self.lib_sort.1,
            ),
            // The dot and the name travel together when the table is scrolled
            // sideways: a row whose name has gone off the left edge is a row you
            // cannot act on.
            sticky: 3.min(shown.len()),
            column_choices: self.column_choices(),
            menu: vec![if self.lib_chosen.len() > 1 {
                format!("Delete {} tones", self.lib_chosen.len())
            } else {
                "Delete".to_owned()
            }],
            nothing_yet: "No tones yet. Click",
            nothing_icon: Some(theme::Icon::Computer),
            nothing_after_icon: "beside a preset to save it.",
            ..Default::default()
        };
        for &i in &rows {
            // Both answers are worked out before the row is borrowed: asking
            // the site caches what it learns, so it needs the app mutably, and
            // a borrowed entry would still be held.
            let (hash, name) = {
                let entry = &self.lib_entries[i];
                (entry.hash.clone(), entry.name.clone())
            };
            let state = self.tone_sync(&hash, &name);
            let can_send = self.pedal_online() && self.tone_kind_compatible(&hash);
            let cloud = if self
                .publishing
                .as_ref()
                .is_some_and(|publishing| publishing.hash == hash)
            {
                theme::Sync::Working
            } else {
                self.cloud_sync(&hash)
            };
            let tag = self.family_tag(self.lib_entries[i].pro);
            let entry = &self.lib_entries[i];
            grid.rows.push(
                shown
                    .iter()
                    .map(|c| c.cell(entry, state, cloud, can_send, tag))
                    .collect(),
            );
            grid.chosen.push(self.lib_chosen.contains(&entry.hash));
        }

        grid.selected = self
            .lib_selected
            .and_then(|selected| rows.iter().position(|&row| row == selected));

        if let Some((hash, column, draft)) = self.lib_editing.clone() {
            let cell = rows
                .iter()
                .position(|&r| self.lib_entries[r].hash == hash)
                .zip(shown.iter().position(|c| *c == column));
            grid.editing = cell;
            grid.draft = draft;
        }
        // Sorted on the very strings the table shows, so the order can never
        // disagree with what is on screen.
        let order = grid.sort_rows();
        let rows: Vec<usize> = order.iter().map(|&row| rows[row]).collect();

        let did = table::show(ui, "library", &mut grid);
        self.apply_column_visibility(did.column_visibility);
        self.lib_draft_edit(&grid, &did, &rows, &shown);

        if let Some(col) = did.sort {
            let col = shown[col];
            // Clicking the column that is already sorting turns it around.
            self.lib_sort = if col == self.lib_sort.0 {
                (col, !self.lib_sort.1)
            } else {
                (col, true)
            };
        }
        if let Some((row, ctrl, shift)) = did.clicked {
            self.pick_lib_row(&rows, rows[row], ctrl, shift);
        }
        if let Some(row) = did.double_clicked {
            let hash = self.lib_entries[rows[row]].hash.clone();
            self.open_tone(&hash);
        }
        // Stars rate in place, as the inspector's do.
        if let Some((row, rating)) = did.rated {
            let hash = self.lib_entries[rows[row]].hash.clone();
            self.commit_cell(&hash, LibColumn::Rating, &rating.to_string());
        }
        // Which icon, not merely that one was pressed. The places are built in
        // a fixed order - the pedal, then the cloud when the site has answered
        // - and reading only the row sent a tone to the pedal whichever of them
        // was clicked, which is a write nobody asked for.
        if let Some((row, place)) = did.place {
            match place {
                0 => self.start_sending(rows[row]),
                // The cloud: already up there, and it takes you to it; not up
                // there, and it puts it there. A tone the site already has is
                // not worth uploading twice, and the icon says which case this
                // is before it is pressed.
                _ => {
                    let entry = rows[row];
                    let hash = self.lib_entries[entry].hash.clone();
                    let known = self.portable_hashes.get(&hash).and_then(|portable| {
                        self.cloud_files
                            .as_ref()
                            .filter(|files| files.contains(portable))
                            .map(|_| portable.clone())
                    });
                    match known {
                        Some(portable) => ui
                            .ctx()
                            .open_url(egui::OpenUrl::new_tab(cloud::tone_url(&portable))),
                        None => self.start_publishing(entry, ui.ctx()),
                    }
                }
            }
        }
        if did.chose.is_some() {
            self.ask_to_delete();
        }
    }

    /// The tag after a tone's name when it is for the other family of pedal
    /// than the one connected: "PRO" beside an HX, "HX" beside a PRO.
    fn family_tag(&self, pro: bool) -> Option<&'static str> {
        match (pro, self.pro_active()) {
            (true, false) => Some("PRO"),
            (false, true) => Some("HX"),
            _ => None,
        }
    }

    /// Begin, carry on, or finish typing in a cell.
    fn lib_draft_edit(
        &mut self,
        grid: &table::Grid,
        did: &table::Did,
        rows: &[usize],
        shown: &[LibColumn],
    ) {
        if let Some((row, col)) = did.edit {
            let entry = &self.lib_entries[rows[row]];
            let column = shown[col];
            self.lib_editing = Some((entry.hash.clone(), column, column.text(entry)));
            return;
        }
        // The draft lives in the app, not the table, so it survives the frame.
        if let Some((_, _, draft)) = self.lib_editing.as_mut() {
            draft.clone_from(&grid.draft);
        }
        if did.cancelled {
            self.lib_editing = None;
        }
        if did.committed {
            if let Some((hash, column, draft)) = self.lib_editing.take() {
                self.commit_cell(&hash, column, draft.trim());
            }
        }
    }

    /// Write what was typed into a cell back to the tone it belongs to.
    fn commit_cell(&mut self, hash: &str, column: LibColumn, typed: &str) {
        let Some(i) = self.lib_entries.iter().position(|e| e.hash == hash) else {
            return;
        };
        let mut meta = self.lib_entries[i].meta.clone();
        match column {
            // A name has to stay unique, and a taken one is refused rather than
            // quietly turned into something else.
            LibColumn::Name => {
                if typed.is_empty() {
                    return;
                }
                if !library::name_is_free(typed, hash) {
                    return self.note(format!("another tone is already called {typed}"));
                }
                meta.name = typed.to_owned();
            }
            LibColumn::Character => meta.character = typed.to_owned(),
            LibColumn::Artist => meta.artist = typed.to_owned(),
            LibColumn::Song => meta.song = typed.to_owned(),
            LibColumn::Genre => {
                meta.genres = typed
                    .split(',')
                    .map(str::trim)
                    .filter(|g| !g.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            LibColumn::Rating => {
                let Ok(rating) = typed.parse::<u8>() else {
                    return self.note("a rating is a number from 0 to 5".to_owned());
                };
                if rating > 5 {
                    return self.note("a rating is a number from 0 to 5".to_owned());
                }
                meta.rating = rating;
            }
            LibColumn::Sync
            | LibColumn::Version
            | LibColumn::Chain
            | LibColumn::SongArtist
            | LibColumn::Added
            | LibColumn::Modified
            | LibColumn::Downloads => return,
        }
        let meta = match library::save_meta(hash, &meta) {
            Ok(meta) => meta,
            Err(why) => return self.note(why),
        };
        if self.lib_selected == Some(i) {
            self.lib_draft = meta.clone();
            self.lib_genres_buf = meta.genres.join(", ");
        }
        self.lib_entries[i].meta = meta;
        self.lib_entries[i].name = self.lib_entries[i].meta.name.clone();
        self.lib_entries[i].added_at = self.lib_entries[i].meta.added_at.clone();
        self.lib_entries[i].modified_at = self.lib_entries[i].meta.modified_at.clone();
        self.lib_entries[i].rating =
            (self.lib_entries[i].meta.rating > 0).then_some(self.lib_entries[i].meta.rating as f32);
        self.library_lookup.reindex(&self.lib_entries);
    }

    /// Which columns the table is showing, in order, skipping the ones turned
    /// off. The dot and the name are not offered: a table of tones with no
    /// names is not a table of anything.
    fn shown_columns(&self) -> Vec<LibColumn> {
        LibColumn::ALL
            .into_iter()
            .filter(|column| {
                self.lib_showing == LibraryView::Cloud || *column != LibColumn::Downloads
            })
            .filter(|c| c.always() || !self.lib_hidden.contains(c))
            .collect()
    }

    /// Ask the site for a pairing, open it, and watch for the answer.
    ///
    /// All of it off the UI thread. Signing in ends in an email, so the wait is
    /// however long somebody takes to find their inbox, and none of that may
    /// happen on the frame this was clicked on.
    fn start_signing_in(&mut self, ctx: &egui::Context) {
        let pairing = match cloud::start_pairing() {
            Ok(pairing) => pairing,
            Err(why) => return self.note(why),
        };
        ctx.open_url(egui::OpenUrl::new_tab(pairing.url.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        let code = pairing.code.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            match cloud::poll_pairing(&code) {
                Ok(None) => {}
                Ok(Some(answer)) => {
                    let _ = tx.send(answer);
                    ctx.request_repaint();
                    return;
                }
                Err(why) => {
                    let _ = tx.send(cloud::Linked::GaveUp(why));
                    ctx.request_repaint();
                    return;
                }
            }
        });
        self.signing_in = Some(Signing {
            code: pairing.code,
            url: pairing.url,
            answer: rx,
        });
    }

    /// Publish this local Tone under its Song.
    ///
    /// A new Song is created first, then the publishable preset is attached as its
    /// first device-native Tone. These remain two calls in the cloud client so
    /// a failed second call can truthfully report the empty Song left behind.
    fn start_publishing(&mut self, entry: usize, ctx: &egui::Context) {
        let Some(token) = self.config.token.clone() else {
            return self.problem("sign in first, and then the cloud will publish".into());
        };
        if let Some(publishing) = &self.publishing {
            return self.problem(format!("{} is already being published", publishing.name));
        }
        let Some(entry) = self.lib_entries.get(entry) else {
            return;
        };
        let Some(path) = library::publish_path(&entry.hash) else {
            return self.problem(format!("{} has no publishable artifact", entry.name));
        };
        let Ok(artifact) = std::fs::read(&path) else {
            return self.problem(format!("{} could not be read", entry.name));
        };
        let pro_tone = library::kind(&entry.hash).as_deref() == Some("vxpreset");
        let catalog_song = !entry.meta.song.trim().is_empty();
        if catalog_song && entry.meta.artist.trim().is_empty() {
            return self.problem(format!(
                "{} names a catalog Song but has no Artist",
                entry.name
            ));
        }
        let hash = entry.hash.clone();
        let name = entry.name.clone();
        let present = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        let inspected = (!pro_tone)
            .then(|| serde_json::from_slice::<serde_json::Value>(&artifact).ok())
            .flatten()
            .and_then(|document| {
                self.catalog
                    .as_ref()
                    .map(|catalog| hx_catalog::inspect(&document, catalog))
            });
        let blocks = if pro_tone {
            pro::preset_blocks(&artifact)
        } else {
            inspected
                .as_ref()
                .map(|tone| {
                    tone.blocks
                        .iter()
                        .map(|block| {
                            serde_json::json!({
                                "name": block.model_name,
                                "category": block.category,
                                "enabled": block.enabled,
                                "path": block.path,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let parsed_metadata = if pro_tone {
            serde_json::json!({
                "format": "vxpreset",
                "protocol": "VoidX",
            })
        } else {
            inspected.as_ref().map_or(serde_json::Value::Null, |tone| {
                serde_json::json!({
                    "models_used": tone.models_used,
                    "skipped": tone.skipped,
                })
            })
        };
        let chain_content = if pro_tone {
            pro::preset_content(&artifact).map(|content| {
                match content.as_str() {
                    "Full rig" => "full_rig",
                    "Amp, no cab" => "amp_only",
                    "Cab and effects" => "effects_only",
                    _ => "effects_only",
                }
                .to_owned()
            })
        } else {
            inspected.as_ref().map(|tone| {
                match tone.chain_content {
                    hx_catalog::ChainContent::FullRig => "full_rig",
                    hx_catalog::ChainContent::AmpAndCab => "amp_and_cab",
                    hx_catalog::ChainContent::AmpOnly => "amp_only",
                    hx_catalog::ChainContent::EffectsOnly => "effects_only",
                }
                .to_owned()
            })
        };
        let output_target = inspected.as_ref().map(|tone| {
            match tone.output_target_guess {
                hx_catalog::OutputTarget::FrfrPa => "frfr_pa",
                hx_catalog::OutputTarget::GuitarCabOrDi => "guitar_cab",
            }
            .to_owned()
        });
        let character = library::character_key(&entry.meta.character).map(str::to_owned);
        let publish_device = if pro_tone {
            "StompStation PRO".to_owned()
        } else if !self.pro_active() && !self.device.is_empty() {
            self.device.clone()
        } else {
            "HX Stomp".to_owned()
        };
        let publish_firmware = if self.tone_kind_compatible(&entry.hash) {
            present(&self.firmware)
        } else {
            None
        };
        let request = cloud::PublishRequest {
            song: cloud::PublishSong::New(cloud::CreateSongRequest {
                creator_name: self.config.account.clone().unwrap_or_default(),
                song: cloud::NewSong {
                    title: if catalog_song {
                        entry.meta.song.trim().to_owned()
                    } else {
                        entry.name.clone()
                    },
                    kind: if catalog_song {
                        cloud::SongKind::Song
                    } else {
                        cloud::SongKind::Original
                    },
                    artist_name: catalog_song.then(|| entry.meta.artist.trim().to_owned()),
                    description: present(&entry.meta.description),
                    tags: entry.meta.tags.clone(),
                    genre_ids: Vec::new(),
                },
            }),
            tone: cloud::CreateToneRequest {
                creator_name: self.config.account.clone().unwrap_or_default(),
                tone: cloud::NewTone {
                    name: entry.name.clone(),
                    series_id: (!entry.series.is_empty()).then(|| entry.series.clone()),
                    description: present(&entry.meta.tone_description),
                    part: present(&entry.meta.part),
                    tuning: present(&entry.meta.tuning),
                    guitar_type: present(&entry.meta.guitar),
                    pickup_type: library::pickup_type_key(&entry.meta.pickup_type)
                        .map(str::to_owned),
                    pickup_electronics: library::pickup_electronics_key(
                        &entry.meta.pickup_electronics,
                    )
                    .map(str::to_owned),
                    device_id: None,
                    device_name: Some(publish_device),
                    firmware_version: publish_firmware,
                    parser_version: Some(update::VERSION.to_owned()),
                    output_target,
                    chain_content,
                    character,
                    blocks,
                    parsed_metadata,
                    preset: Some(cloud::PresetUpload {
                        filename: path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| {
                                format!(
                                    "{}.{}",
                                    entry.name,
                                    if pro_tone { "vxpreset" } else { "hlx" }
                                )
                            }),
                        bytes: artifact,
                    }),
                },
            },
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(cloud::publish(&token, &request));
            ctx.request_repaint();
        });
        self.status.clear();
        self.publishing = Some(PublishingJob {
            hash,
            name,
            answer: rx,
        });
    }

    /// Collect the answer to a publish, if one has arrived.
    fn settle_publishing(&mut self, ctx: &egui::Context) {
        let Some(publishing) = &self.publishing else {
            return;
        };
        let hash = publishing.hash.clone();
        let answer = match publishing.answer.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.publishing = None;
                self.problem("publishing stopped without an answer".to_owned());
                return;
            }
        };
        self.publishing = None;
        match answer {
            Ok(tone) => {
                // The Tone POST is authoritative. Fill this row immediately
                // instead of waiting for a full Song-index walk to reach the
                // same hash.
                let portable = self.portable_hashes.get(&hash).cloned().or_else(|| {
                    let found = library::portable_hash(&hash)?;
                    self.portable_hashes.insert(hash.clone(), found.clone());
                    Some(found)
                });
                if let Some(portable) = portable {
                    self.cloud_files
                        .get_or_insert_default()
                        .insert(portable.clone());
                    ctx.open_url(egui::OpenUrl::new_tab(cloud::tone_url(&portable)));
                }
                self.status.clear();
                self.note(format!("{} is published as a Tone", tone.summary.name));
                self.start_cloud_check();
            }
            Err(why) => self.problem(why.to_string()),
        }
    }

    /// Collect the answer to a sign-in, if one has arrived.
    fn settle_signing_in(&mut self) {
        let Some(signing) = &self.signing_in else {
            return;
        };
        match signing.answer.try_recv() {
            Ok(cloud::Linked::In { token, account }) => {
                self.config.sign_in(token, account.clone());
                self.signing_in = None;
                self.note(format!("signed in as {account}"));
            }
            Ok(cloud::Linked::GaveUp(why)) => {
                self.signing_in = None;
                self.note(why);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.signing_in = None,
        }
    }

    /// The choices every tone-table header offers on right-click. Push and
    /// Name are structural, and Downloads only exists in the Cloud view.
    fn column_choices(&self) -> Vec<(usize, String, bool)> {
        LibColumn::ALL
            .into_iter()
            .enumerate()
            .filter(|(_, column)| {
                !column.always()
                    && (self.lib_showing == LibraryView::Cloud || *column != LibColumn::Downloads)
            })
            .map(|(key, column)| {
                (
                    key,
                    column.title().to_owned(),
                    !self.lib_hidden.contains(&column),
                )
            })
            .collect()
    }

    fn apply_column_visibility(&mut self, changed: Option<(usize, bool)>) {
        let Some((key, visible)) = changed else {
            return;
        };
        let Some(column) = LibColumn::ALL
            .get(key)
            .copied()
            .filter(|column| !column.always())
        else {
            return;
        };
        if visible {
            self.lib_hidden.remove(&column);
        } else {
            self.lib_hidden.insert(column);
        }
    }

    /// What a click on a row means, with and without modifiers.
    ///
    /// Plain click picks one, command-click adds or removes one, shift-click
    /// takes everything between here and the last plain click - the three
    /// gestures every list in every file manager has had for thirty years.
    fn pick_lib_row(&mut self, rows: &[usize], row: usize, ctrl: bool, shift: bool) {
        let of = |app: &Self, i: usize| app.lib_entries[i].hash.clone();
        match (ctrl, shift) {
            (true, _) => {
                let hash = of(self, row);
                if !self.lib_chosen.remove(&hash) {
                    self.lib_chosen.insert(hash);
                }
                self.lib_anchor = Some(row);
            }
            (false, true) => {
                let anchor = self.lib_anchor.unwrap_or(row);
                let (from, to) = (
                    rows.iter().position(|&r| r == anchor).unwrap_or(0),
                    rows.iter().position(|&r| r == row).unwrap_or(0),
                );
                let span = if from <= to { from..=to } else { to..=from };
                self.lib_chosen = span.map(|k| of(self, rows[k])).collect();
            }
            _ => {
                self.lib_chosen = [of(self, row)].into_iter().collect();
                self.lib_anchor = Some(row);
            }
        }
        self.select_lib_entry(row);
    }

    /// Work out what deleting the chosen tones would cost, and ask.
    fn ask_to_delete(&mut self) {
        let chosen: Vec<String> = if self.lib_chosen.is_empty() {
            self.lib_selected
                .map(|i| self.lib_entries[i].hash.clone())
                .into_iter()
                .collect()
        } else {
            self.lib_chosen.iter().cloned().collect()
        };
        if chosen.is_empty() {
            return;
        }
        // A setlist names bytes, so deleting a tone from the library cannot
        // take it out of one. Saying which setlists still play it is the
        // reassurance, not the warning it used to be.
        let affected: Vec<String> = library::setlists()
            .into_iter()
            .filter(|(_, s)| chosen.iter().any(|hash| s.plays(hash)))
            .map(|(_, s)| s.name)
            .collect();
        self.confirm_delete = Some((chosen, affected));
    }

    /// The question, and what carrying it out does.
    fn confirm_delete_window(&mut self, ctx: &egui::Context) {
        let Some((chosen, affected)) = self.confirm_delete.clone() else {
            return;
        };
        let mut decided = None;
        let title = if chosen.len() == 1 {
            "Delete this tone from the library?".to_owned()
        } else {
            format!("Delete {} tones from the library?", chosen.len())
        };
        let still_played = (!affected.is_empty()).then(|| {
            format!(
                "{} played by the {} {}, and will keep playing there.",
                if chosen.len() == 1 {
                    "It is"
                } else {
                    "Some of them are"
                },
                if affected.len() == 1 {
                    "setlist"
                } else {
                    "setlists"
                },
                affected
                    .iter()
                    .map(|n| format!("“{n}”"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        let (_, close) = theme::dialog(ctx, "confirm-delete", 460.0, |ui| {
            theme::dialog_header(ui, &title, still_played.as_deref());
            theme::dialog_footer(
                ui,
                "Anything nothing else plays goes to the library's trash, not out of existence.",
                |ui| {
                    if theme::Button::new("Delete").danger().show(ui).clicked() {
                        decided = Some(true);
                    }
                    if theme::Button::new("Cancel").show(ui).clicked() {
                        decided = Some(false);
                    }
                },
            );
        });
        if close && decided.is_none() {
            decided = Some(false);
        }
        match decided {
            Some(true) => {
                self.confirm_delete = None;
                // Nothing here has to think about the setlists. Forgetting a
                // tone takes it out of the library; the object survives exactly
                // as long as something still points at it, which is the whole
                // reason the store is addressed by content.
                let mut gone = 0;
                for hash in &chosen {
                    match library::forget(hash) {
                        Ok(()) => gone += 1,
                        Err(why) => self.note(why),
                    }
                }
                self.note(if gone == 1 {
                    "removed 1 tone".to_owned()
                } else {
                    format!("removed {gone} tones")
                });
                self.lib_chosen.clear();
                self.lib_selected = None;
                self.refresh_library();
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    /// Write a library Tone and its Song facts in the cloud contract's shape.
    ///
    /// Two files, not one: the device-family artifact the site receives, and a
    /// `.json` of what only a person knows, in the site's own field names. HX
    /// native documents are converted to `.hlx`; a PRO `.vxpreset` is already
    /// the lossless publishable form.
    fn export_for_the_web(&mut self, index: usize) {
        let Some(entry) = self.lib_entries.get(index).cloned() else {
            return;
        };
        let pro_tone = library::kind(&entry.hash).as_deref() == Some("vxpreset");
        let catalog = if pro_tone {
            None
        } else {
            let Some(catalog) = self.catalog.as_ref() else {
                self.note("exporting this HX tone needs HX Edit's model data first".into());
                return;
            };
            Some(catalog)
        };
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Where to put the tone")
            .pick_folder()
        else {
            return;
        };

        let stem = sanitise(&entry.name);
        let Some(document) = library::read(&entry.hash) else {
            return self.note(format!("{} is missing from the library", entry.name));
        };
        // A library tone is a device document; the site wants the symbolic
        // form. A tone kept as .hlx already is passed through untouched.
        let artifact = if pro_tone {
            document
        } else if library::kind(&entry.hash).as_deref() == Some("hlx") {
            String::from_utf8_lossy(&document).into_owned().into_bytes()
        } else {
            match hx_proto::preset::Preset::parse(&document) {
                Some(preset) => hx_catalog::to_hlx(
                    &preset,
                    catalog.expect("HX catalog checked above"),
                    &entry.name,
                )
                .to_pretty_string()
                .into_bytes(),
                None => return self.note(format!("{} is not a readable preset", entry.name)),
            }
        };

        let details = dir.join(format!("{stem}.json"));
        let json = match serde_json::to_vec_pretty(&entry.meta.for_the_web(&entry.name)) {
            Ok(json) => json,
            Err(error) => return self.note(format!("could not encode the tone details: {error}")),
        };
        let tone = dir.join(format!(
            "{stem}.{}",
            if pro_tone { "vxpreset" } else { "hlx" }
        ));
        if let Err(e) = library::atomic_write(&tone, artifact) {
            return self.note(format!("could not write {}: {e}", tone.display()));
        }
        if let Err(e) = library::atomic_write(&details, json) {
            return self.note(format!("could not write {}: {e}", details.display()));
        }
        self.note(format!("exported {} to {}", entry.name, dir.display()));
    }

    /// The numbers under the curve, one group per handle.
    ///
    /// Each group wears its handle's colour, so the dot you just dragged and
    /// the three numbers that moved are visibly the same control. A flat list
    /// of eleven made you count to work out which "Freq" you were looking at.
    fn eq_controls(&mut self, ui: &mut egui::Ui) {
        let curve = self.eq_curve_now();
        let mut write: Option<(i64, f32)> = None;

        ui.columns(5, |cols| {
            // The cuts take away, the bands shape; each still gets its own
            // column so the row reads left to right as the spectrum does.
            write = write.or_else(|| {
                eq_cut_group(
                    &mut cols[0],
                    "Low Cut",
                    theme::rgb(eq::LOW_CUT_COLOUR),
                    id::LOW_CUT,
                    curve.low_cut,
                    19.9,
                    19.9..=500.0,
                )
            });
            for (i, (name, colour, freq_id, q_id, gain_id, band, range)) in [
                (
                    "Low",
                    eq::LOW_COLOUR,
                    id::LOW_FREQ,
                    id::LOW_Q,
                    id::LOW_GAIN,
                    curve.low,
                    20.0..=500.0,
                ),
                (
                    "Mid",
                    eq::MID_COLOUR,
                    id::MID_FREQ,
                    id::MID_Q,
                    id::MID_GAIN,
                    curve.mid,
                    200.0..=5000.0,
                ),
                (
                    "High",
                    eq::HIGH_COLOUR,
                    id::HIGH_FREQ,
                    id::HIGH_Q,
                    id::HIGH_GAIN,
                    curve.high,
                    1000.0..=20000.0,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let found = eq_band_group(
                    &mut cols[i + 1],
                    name,
                    theme::rgb(colour),
                    (freq_id, q_id, gain_id),
                    band,
                    range,
                );
                write = write.or(found);
            }
            write = write.or_else(|| {
                eq_cut_group(
                    &mut cols[4],
                    "High Cut",
                    theme::rgb(eq::HIGH_CUT_COLOUR),
                    id::HIGH_CUT,
                    curve.high_cut,
                    20100.0,
                    1000.0..=20100.0,
                )
            });
        });

        if let Some((id, value)) = write {
            self.settings.insert(id, value);
            self.send(Cmd::WriteSetting { id, value });
        }
    }

    /// Ask what each footswitch is called and what colour it lights.
    ///
    /// The document says what every controller drives, so this is no longer
    /// where assignments come from. What it still knows, and the document does
    /// not, is the switch's own furniture: a typed label, an LED colour,
    /// latching or momentary. A handful of round trips, once per preset.
    fn read_switches(&mut self) {
        if !matches!(self.connection, Connection::Online) {
            self.switches.clear();
            return;
        }
        self.send(Cmd::ReadSwitches);
    }

    /// Everything driving anything, from both places the pedal keeps it.
    ///
    /// The document's controller table has every parameter assignment and the
    /// bypasses an expression pedal drives - a wah's auto-engage - but **not a
    /// bypass on a footswitch**, which is not a controller assignment at all:
    /// it lives in the footswitch's own configuration, where opcode 33 reads
    /// it. Checked across every preset on the pedal: not one footswitch bypass
    /// appears in the document. Reading only the document was why the on/off
    /// switch never showed what carried it.
    fn all_assignments(&self) -> Vec<hx_proto::preset::Assignment> {
        use hx_proto::preset::{Assignment, Target};
        let mut found = self.assignments.clone();
        for switch in &self.switches {
            for carried in &switch.carries {
                let source = hx_proto::rpc::Source::Footswitch(switch.switch);
                // A switch's list names everything it drives on that block,
                // parameters included, and does not say which is which. The
                // document names every parameter assignment and no footswitch
                // bypass, so an entry the document already accounts for *is*
                // that parameter, and only one it does not know about is the
                // bypass. Matching on the block alone invented an On/Off row
                // for a switch that was really driving a knob.
                if found
                    .iter()
                    .any(|a| a.block == carried.block && a.source == source)
                {
                    continue;
                }
                found.push(Assignment {
                    block: carried.block,
                    source,
                    target: Target::Bypass,
                    // A switch is on or off; there is no travel to move.
                    min: 0.0,
                    max: 1.0,
                    // This one came from the footswitch's own configuration,
                    // where a MIDI CC has no place: a footswitch is not MIDI.
                    cc: None,
                });
            }
        }
        found
    }

    /// What drives one thing on one block, if anything does.
    fn assignment(
        &self,
        block: i64,
        target: hx_proto::preset::Target,
    ) -> Option<hx_proto::preset::Assignment> {
        self.all_assignments()
            .into_iter()
            .find(|a| a.block == block && a.target == target)
    }

    /// Everything driving one block, in source order so the list does not
    /// reshuffle itself when an assignment is added.
    fn assignments_on(&self, block: i64) -> Vec<hx_proto::preset::Assignment> {
        let mut found: Vec<_> = self
            .all_assignments()
            .into_iter()
            .filter(|a| a.block == block)
            .collect();
        found.sort_by_key(|a| {
            (
                a.source.ordinal(),
                a.target != hx_proto::preset::Target::Bypass,
            )
        });
        found
    }

    /// Whether an assignment is Line 6's auto-engage.
    ///
    /// A bypass under an expression pedal does not mean the pedal switches the
    /// block: it means the block switches *itself* on when the pedal moves off
    /// its heel, which is how every wah on the device works. "Expression Pedal
    /// 1 controls On/Off" is true and says none of that.
    fn auto_engage(source: hx_proto::rpc::Source, target: hx_proto::preset::Target) -> bool {
        matches!(
            (source, target),
            (
                hx_proto::rpc::Source::Expression(_),
                hx_proto::preset::Target::Bypass
            )
        )
    }

    /// What to call what an assignment drives: the parameter's name out of the
    /// catalog, or On/Off for a bypass.
    fn target_name(&self, block: i64, target: hx_proto::preset::Target) -> String {
        use hx_proto::preset::Target;
        let index = match target {
            Target::Bypass => return "On/Off".to_owned(),
            Target::Param(index) => index,
        };
        self.chain
            .iter()
            .find(|b| b.position == block)
            .and_then(|b| self.slot_model(b))
            .and_then(|model| {
                let catalog = self.catalog.as_ref()?;
                Some(
                    catalog
                        .ordered_params(model)
                        .get(index as usize)?
                        .name
                        .clone(),
                )
            })
            .unwrap_or_else(|| format!("Parameter {index}"))
    }

    /// One control's assignment menu, gathered before it draws.
    ///
    /// The same gathering for a knob and for a block's on/off, because they ask
    /// the same question. They used to be two menus with two vocabularies, and
    /// only one of them could tell you the footswitch you were reaching for is
    /// already carrying something else.
    fn assign_view(
        &self,
        block: i64,
        target: hx_proto::preset::Target,
        name: String,
    ) -> AssignMenu {
        use hx_proto::preset::Target;
        use hx_proto::rpc::Source;
        let bypass = target == Target::Bypass;
        let switches = self.switch_count();
        let driving = self.all_assignments();
        let sources = Source::all()
            .into_iter()
            // A bypass is a switch, so a pedal that sweeps cannot drive it.
            .filter(|source| !bypass || source.switches())
            // The protocol has room for five footswitches; a Stomp has three,
            // and offering the two it does not have is offering nothing.
            .filter(|source| !matches!(source, Source::Footswitch(n) if *n > switches))
            .map(|source| {
                let mut carries: Vec<String> = driving
                    .iter()
                    .filter(|a| a.source == source && (a.block, a.target) != (block, target))
                    .map(|a| self.target_name(a.block, a.target))
                    .collect();
                // A switch with a name typed for it answers to that name: it is
                // what is written under your foot.
                if let Source::Footswitch(n) = source {
                    if let Some(label) = self
                        .switches
                        .iter()
                        .find(|s| s.switch == n)
                        .and_then(|s| s.label.clone())
                    {
                        carries.insert(0, label);
                    }
                }
                (source, carries)
            })
            .collect();
        AssignMenu {
            name,
            under: self.assignment(block, target).map(|a| a.source),
            sources,
        }
    }

    /// One footswitch's own settings, gathered for the panel that draws them.
    fn switch_view(&self, switch: u8) -> Option<SwitchView> {
        let found = self.switches.iter().find(|s| s.switch == switch)?;
        Some(SwitchView {
            label: self
                .switch_draft
                .as_ref()
                .filter(|(drafting, _)| *drafting == switch)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| found.label.clone().unwrap_or_default()),
            carries: found.carries.first().map(|c| c.name.clone()),
            colour: found.colour,
            momentary: found.momentary,
        })
    }

    /// Carry out a change to a footswitch itself.
    fn switch_action(&mut self, change: SwitchChange) {
        match change {
            SwitchChange::Typing(switch, text) => self.switch_draft = Some((switch, text)),
            SwitchChange::Set { switch, edit } => {
                if matches!(edit, session::SwitchEdit::Label(_)) {
                    self.switch_draft = None;
                }
                self.edit(Cmd::EditSwitch { switch, edit });
            }
        }
    }

    /// Carry out what a control's assignment menu was asked to do.
    fn assign_action(
        &mut self,
        block: i64,
        target: hx_proto::preset::Target,
        action: AssignAction,
    ) {
        use hx_proto::preset::Target;
        use hx_proto::rpc::Source;
        let source = match action {
            AssignAction::To(source) => source,
            // Choosing the number, not choosing MIDI. The two travel by
            // different messages, and which one depends on what is assigned:
            // a bypass carries its CC on the assignment itself, a parameter
            // has an opcode of its own for it.
            AssignAction::Cc(cc) => {
                match target {
                    Target::Bypass => self.edit(Cmd::AssignMidi {
                        block,
                        on: true,
                        cc,
                    }),
                    Target::Param(param) => self.edit(Cmd::SetAssignCc { block, param, cc }),
                }
                return;
            }
        };
        match target {
            Target::Param(param) => self.edit(Cmd::AssignParameter {
                block,
                param,
                source,
            }),
            Target::Bypass => match source {
                Some(Source::Footswitch(switch)) => self.edit(Cmd::AssignBypassFootswitch {
                    block,
                    switch,
                    on: true,
                }),
                Some(Source::MidiCc) => self.edit(Cmd::AssignMidi {
                    block,
                    on: true,
                    // Whatever it already had, or the pedal's own default.
                    cc: self
                        .assignment(block, Target::Bypass)
                        .and_then(|a| a.cc)
                        .unwrap_or(DEFAULT_CC),
                }),
                // The menu offers a bypass nothing else, so this is unreachable
                // rather than unhandled.
                Some(_) => {}
                // Taking it off means undoing whatever the document says has
                // it, and the two are different messages.
                None => match self.assignment(block, Target::Bypass).map(|a| a.source) {
                    Some(Source::Footswitch(switch)) => self.edit(Cmd::AssignBypassFootswitch {
                        block,
                        switch,
                        on: false,
                    }),
                    Some(Source::MidiCc) => self.edit(Cmd::AssignMidi {
                        block,
                        on: false,
                        // Taking it off sends 0, which is what "no CC" is.
                        cc: 0,
                    }),
                    _ => {}
                },
            },
        }
    }

    /// Whether every value the EQ panel drives has actually been read off the
    /// device. Until it has, the panel shows nothing and touches nothing.
    fn eq_settings_known(&self) -> bool {
        [
            id::LOW_CUT,
            id::LOW_FREQ,
            id::LOW_Q,
            id::LOW_GAIN,
            id::MID_FREQ,
            id::MID_Q,
            id::MID_GAIN,
            id::HIGH_FREQ,
            id::HIGH_Q,
            id::HIGH_GAIN,
            id::HIGH_CUT,
        ]
        .iter()
        .all(|id| self.settings.contains_key(id))
    }

    /// Read the eleven numbers back out of the settings map as a curve.
    fn eq_curve_now(&self) -> eq::Curve {
        let at = |id: i64, fallback: f32| self.settings.get(&id).copied().unwrap_or(fallback);
        eq::Curve {
            low_cut: at(id::LOW_CUT, 20.0),
            low: eq::Band {
                freq: at(id::LOW_FREQ, 100.0),
                q: at(id::LOW_Q, 0.7),
                gain_db: at(id::LOW_GAIN, 0.0),
            },
            mid: eq::Band {
                freq: at(id::MID_FREQ, 1000.0),
                q: at(id::MID_Q, 0.7),
                gain_db: at(id::MID_GAIN, 0.0),
            },
            high: eq::Band {
                freq: at(id::HIGH_FREQ, 5000.0),
                q: at(id::HIGH_Q, 0.7),
                gain_db: at(id::HIGH_GAIN, 0.0),
            },
            high_cut: at(id::HIGH_CUT, 20000.0),
        }
    }

    /// Put every band back to no gain and park both cuts outside the band.
    fn flatten_eq(&mut self) {
        for (id, value) in [
            (id::LOW_GAIN, 0.0),
            (id::MID_GAIN, 0.0),
            (id::HIGH_GAIN, 0.0),
            (id::LOW_CUT, 19.9),
            (id::HIGH_CUT, 20100.0),
        ] {
            self.settings.insert(id, value);
            self.send(Cmd::WriteSetting { id, value });
        }
    }

    /// The curve itself, and the handles that move it.
    fn eq_curve(&mut self, ui: &mut egui::Ui, active: bool) {
        const HEIGHT: f32 = 200.0;
        const RANGE_DB: f32 = 15.0;

        let width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::Vec2::new(width, HEIGHT), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, egui::CornerRadius::same(4), theme::bg());

        let x_of = |hz: f32| rect.left() + eq::position(hz) * rect.width();
        let y_of = |db: f32| rect.center().y - (db / RANGE_DB) * (rect.height() / 2.0);
        let db_of = |y: f32| ((rect.center().y - y) / (rect.height() / 2.0)) * RANGE_DB;
        let hz_of = |x: f32| eq::from_position((x - rect.left()) / rect.width());

        // The grid, at the frequencies and gains anyone reads an EQ by.
        let grid = egui::Stroke::new(1.0_f32, theme::panel());
        for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
            let x = x_of(hz);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                grid,
            );
            let label = if hz >= 1000.0 {
                format!("{:.0}k", hz / 1000.0)
            } else {
                format!("{hz:.0}")
            };
            painter.text(
                egui::pos2(x, rect.bottom() - 2.0),
                egui::Align2::CENTER_BOTTOM,
                label,
                egui::FontId::proportional(9.0),
                theme::muted().gamma_multiply(0.7),
            );
        }
        for db in [-12.0, -6.0, 6.0, 12.0] {
            let y = y_of(db);
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                grid,
            );
        }
        // Unity, drawn heavier: it is the line every other reading is against.
        let zero = y_of(0.0);
        painter.line_segment(
            [
                egui::pos2(rect.left(), zero),
                egui::pos2(rect.right(), zero),
            ],
            egui::Stroke::new(1.0_f32, theme::muted().gamma_multiply(0.6)),
        );

        // The response. Dim when the EQ is bypassed - the shape is still worth
        // seeing, it just is not doing anything.
        let curve = self.eq_curve_now();
        let ink = if active {
            theme::accent()
        } else {
            theme::muted().gamma_multiply(0.8)
        };
        let points: Vec<egui::Pos2> = curve
            .sampled(rect.width().max(2.0) as usize)
            .into_iter()
            .map(|(t, db)| egui::pos2(rect.left() + t * rect.width(), y_of(db)))
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(2.0_f32, ink)));

        // The handles. Each writes as it moves, paced, so the pedal is heard
        // moving without being asked sixty times a second.
        let bands = [
            (
                "Low",
                eq::LOW_COLOUR,
                id::LOW_FREQ,
                id::LOW_Q,
                id::LOW_GAIN,
                curve.low,
            ),
            (
                "Mid",
                eq::MID_COLOUR,
                id::MID_FREQ,
                id::MID_Q,
                id::MID_GAIN,
                curve.mid,
            ),
            (
                "High",
                eq::HIGH_COLOUR,
                id::HIGH_FREQ,
                id::HIGH_Q,
                id::HIGH_GAIN,
                curve.high,
            ),
        ];
        let mut writes: Vec<(i64, f32)> = Vec::new();
        let mut released = false;
        for (name, colour, freq_id, q_id, gain_id, band) in bands {
            let at = egui::pos2(x_of(band.freq), y_of(band.gain_db));
            let hit = egui::Rect::from_center_size(at, egui::Vec2::splat(18.0));
            let response = ui
                .interact(hit, ui.id().with(("eq-band", freq_id)), egui::Sense::drag())
                .on_hover_text(format!(
                    "{name} - {:.0} Hz, {:+.1} dB, Q {:.2}\ndrag to move, scroll to narrow",
                    band.freq, band.gain_db, band.q
                ));
            if response.dragged() {
                let p = response.interact_pointer_pos().unwrap_or(at);
                let freq = hz_of(p.x).clamp(eq::MIN_HZ, eq::MAX_HZ);
                let gain = db_of(p.y).clamp(-12.0, 12.0);
                writes.push((freq_id, freq));
                writes.push((gain_id, gain));
            }
            // Scrolling over a band is how every EQ sets Q, and it saves the
            // panel a third handle per band.
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta().y);
                if scroll != 0.0 {
                    let q = (band.q * (1.0 + scroll.signum() * 0.12)).clamp(0.1, 10.0);
                    writes.push((q_id, q));
                }
            }
            released |= response.drag_stopped();

            // The handle keeps its own colour whether or not the EQ is on: it
            // is how you tell which of the three you have hold of, and that
            // does not stop being true when the EQ is bypassed.
            let handle = theme::rgb(colour);
            let handle = if active {
                handle
            } else {
                handle.gamma_multiply(0.55)
            };
            painter.circle_filled(at, 6.0, handle);
            painter.circle_stroke(at, 6.0, egui::Stroke::new(1.5_f32, theme::bg()));
            if response.hovered() || response.dragged() {
                painter.circle_stroke(at, 9.0, egui::Stroke::new(1.5_f32, handle));
            }
        }

        // The two cuts ride along the unity line, which is where they act.
        for (name, colour, id, value, parked) in [
            (
                "Low Cut",
                eq::LOW_CUT_COLOUR,
                id::LOW_CUT,
                curve.low_cut,
                19.9,
            ),
            (
                "High Cut",
                eq::HIGH_CUT_COLOUR,
                id::HIGH_CUT,
                curve.high_cut,
                20100.0,
            ),
        ] {
            let at = egui::pos2(x_of(value), zero);
            let hit = egui::Rect::from_center_size(at, egui::Vec2::new(14.0, 22.0));
            let off = (value - parked).abs() < 0.5;
            let response = ui
                .interact(hit, ui.id().with(("eq-cut", id)), egui::Sense::drag())
                .on_hover_text(if off {
                    format!("{name} - off\ndrag into the band to use it")
                } else {
                    format!("{name} - {value:.0} Hz\ndrag to move")
                });
            if response.dragged() {
                let p = response.interact_pointer_pos().unwrap_or(at);
                writes.push((id, hz_of(p.x).clamp(eq::MIN_HZ, eq::MAX_HZ)));
            }
            released |= response.drag_stopped();

            // A dot, like every other handle: five things you can grab should
            // look like five things you can grab. Parked outside the band it is
            // doing nothing, and drawing it as bright as a cut that is working
            // would be a lie, so it goes hollow instead.
            let mark = theme::rgb(colour);
            if off {
                painter.circle_filled(at, 5.0, theme::bg());
                painter.circle_stroke(
                    at,
                    5.0,
                    egui::Stroke::new(1.5_f32, mark.gamma_multiply(0.6)),
                );
            } else {
                painter.circle_filled(at, 6.0, mark);
                painter.circle_stroke(at, 6.0, egui::Stroke::new(1.5_f32, theme::bg()));
            }
            if response.hovered() || response.dragged() {
                painter.circle_stroke(at, 9.0, egui::Stroke::new(1.5_f32, mark));
            }
        }

        if !writes.is_empty() {
            // Show it immediately whatever happens, so the curve tracks the
            // finger; only the trip to the pedal is paced.
            for &(id, value) in &writes {
                self.settings.insert(id, value);
            }
            let due = self
                .eq_wrote_at
                .is_none_or(|t| t.elapsed() > Duration::from_millis(60));
            if due {
                self.eq_wrote_at = Some(std::time::Instant::now());
                for (id, value) in writes.drain(..) {
                    self.send(Cmd::WriteSetting { id, value });
                }
            }
        }
        // Whatever the pacing swallowed, the last position is not negotiable.
        if released {
            self.eq_wrote_at = None;
            let now = self.eq_curve_now();
            for (id, value) in [
                (id::LOW_CUT, now.low_cut),
                (id::LOW_FREQ, now.low.freq),
                (id::LOW_Q, now.low.q),
                (id::LOW_GAIN, now.low.gain_db),
                (id::MID_FREQ, now.mid.freq),
                (id::MID_Q, now.mid.q),
                (id::MID_GAIN, now.mid.gain_db),
                (id::HIGH_FREQ, now.high.freq),
                (id::HIGH_Q, now.high.q),
                (id::HIGH_GAIN, now.high.gain_db),
                (id::HIGH_CUT, now.high_cut),
            ] {
                self.send(Cmd::WriteSetting { id, value });
            }
        }
    }

    /// Start pulling model data out of an HX Edit installer, off the UI
    /// thread, and say so.
    fn extract_resources(&mut self, installer: std::path::PathBuf) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.onboarding_status = Some(format!(
            "reading {}…",
            installer
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        self.extracting = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(hx_catalog::extract::from_installer(&installer));
        });
    }

    /// Copy the model data from an HX Edit installed on this machine,
    /// off the UI thread.
    fn extract_installed(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.onboarding_status =
            Some("found HX Edit installed on this machine; copying its data…".into());
        self.extracting = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(hx_catalog::extract::from_installed());
        });
    }

    /// Collect a finished extraction and put the catalog straight to work:
    /// no restart, the names and artwork simply appear.
    fn finish_extraction(&mut self) {
        let Some(rx) = &self.extracting else { return };
        match rx.try_recv() {
            Ok(Ok(count)) => {
                self.extracting = None;
                self.catalog = Catalog::load().ok();
                self.onboarding_status = if self.catalog.is_some() {
                    self.show_onboarding = false;
                    self.note(format!("extracted {count} resource files"));
                    None
                } else {
                    Some("extracted files, but the catalog would not load".into())
                };
            }
            Ok(Err(why)) => {
                self.extracting = None;
                self.onboarding_status = Some(why);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.extracting = None;
                self.onboarding_status = Some("extraction stopped unexpectedly".into());
            }
        }
    }

    fn dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        for path in dropped {
            // A preset, an impulse response and an HX Edit installer are all
            // "a file you drop on the window"; the extension decides.
            if path
                .extension()
                .is_some_and(|e| e == "hxpreset" || e.eq_ignore_ascii_case("hlx"))
            {
                self.open_tone_file(&path);
                continue;
            }
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dmg") || e.eq_ignore_ascii_case("exe"))
            {
                self.extract_resources(path);
                continue;
            }
            self.load_ir(path);
        }
    }

    /// Send a WAV to the pedal's impulse responses. Which slot is the
    /// worker's choice, made from the pedal's own list as the upload starts:
    /// the list here can be a file or two behind, or not heard yet at all.
    fn load_ir(&mut self, file: std::path::PathBuf) {
        self.note(format!(
            "loading {} into the first free impulse response slot",
            file.display()
        ));
        self.send(Cmd::LoadIr(file));
    }

    /// Open a tone file aimed at one preset: the row whose menu asked.
    fn open_tone_file_for(&mut self, path: &std::path::Path, dest: i64) {
        let before = self.preview.take();
        self.open_tone_file(path);
        match self.preview.as_mut() {
            Some(preview) => preview.dest = dest,
            // A file that would not open leaves the preview that was there.
            None => self.preview = before,
        }
    }

    /// Open a tone file of either kind for a look. The extension decides the
    /// reader; both end at the same preview window.
    fn open_tone_file(&mut self, path: &std::path::Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return self.note(format!("could not read {}: {e}", path.display())),
        };
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("hlx"))
        {
            self.preview_hlx(name, bytes);
        } else {
            self.preview_hxpreset(name, bytes);
        }
    }

    /// Show a tone the library holds, read out of the object store.
    fn open_tone(&mut self, hash: &str) {
        let Some(bytes) = library::read(hash) else {
            return self.note("that tone is missing from the library".into());
        };
        let name = library::meta_of(hash)
            .map(|m| m.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| library::short(hash).to_owned());
        if library::kind(hash).as_deref() == Some("vxpreset") {
            if let Err(why) = self.pro.preview(name, &bytes) {
                self.note(why);
            }
        } else if library::kind(hash).as_deref() == Some("hlx") {
            self.preview_hlx(&name, bytes);
        } else {
            self.preview_hxpreset(&name, bytes);
        }
    }

    /// What a tone is made of, in two or three words.
    ///
    /// Short because it is a column. The sentence it used to be - "Full rig,
    /// for FRFR or a PA" - said the same thing at four times the width, and a
    /// hundred rows of it read as one grey block.
    fn tone_content(tone: &hx_catalog::Tone) -> &'static str {
        match tone.chain_content {
            hx_catalog::ChainContent::FullRig => "Full rig",
            hx_catalog::ChainContent::AmpAndCab => "Amp and cab",
            hx_catalog::ChainContent::AmpOnly => "Amp, no cab",
            hx_catalog::ChainContent::EffectsOnly => "Effects only",
        }
    }

    /// What to play it through, equally short.
    fn tone_output(tone: &hx_catalog::Tone) -> &'static str {
        match tone.output_target_guess {
            hx_catalog::OutputTarget::FrfrPa => "FRFR or PA",
            hx_catalog::OutputTarget::GuitarCabOrDi => "Real cab",
        }
    }

    /// Both together, for the one place with room for a sentence.
    fn tone_line(tone: &hx_catalog::Tone) -> String {
        format!("{}, {}", Self::tone_content(tone), Self::tone_output(tone))
    }

    /// Resolve a portable `.hlx` to the same device operations used by file
    /// previews and cloud auditions. Keeping this conversion singular is what
    /// makes a downloaded Tone sound like the one shown before it is pushed.
    fn steps_from_hlx(
        &self,
        label: &str,
        bytes: &[u8],
    ) -> Result<(hx_catalog::Tone, Vec<session::ApplyBlock>), String> {
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| "reading a tone needs HX Edit's model data first".to_owned())?;
        let json: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|error| format!("{label} is not a readable .hlx: {error}"))?;
        let mut tone = hx_catalog::inspect(&json, catalog);
        // The tone's blocks resolved to what applying them needs: parameter
        // names become indexes, switches marked as such. A parameter the
        // catalog cannot place is reported, not dropped.
        let mut blocks = Vec::new();
        for block in tone.blocks.iter().filter(|b| b.path == 0) {
            let mut params = Vec::new();
            for (name, value) in &block.params {
                match catalog.param_index(block.model_number, name) {
                    Some(index) => {
                        let kind = catalog
                            .param(block.model_number, index)
                            .map_or(Kind::Continuous, |p| p.kind);
                        params.push((index as i64, *value, kind));
                    }
                    None => tone
                        .skipped
                        .push(format!("{}: no parameter {name:?}", block.model_name)),
                }
            }
            blocks.push(session::ApplyBlock {
                model: block.model_number,
                enabled: block.enabled,
                params,
            });
        }
        if tone.blocks.iter().any(|b| b.path == 1) {
            tone.skipped
                .push("the second signal path is shown but not loaded yet".into());
        }
        Ok((tone, blocks))
    }

    /// Read a .hlx and show what it is, without touching the device.
    fn preview_hlx(&mut self, label: &str, bytes: Vec<u8>) {
        let (tone, blocks) = match self.steps_from_hlx(label, &bytes) {
            Ok(read) => read,
            Err(why) => return self.note(why),
        };

        // A chain to draw: input, the blocks, output - one path per DSP,
        // positions synthesised since a .hlx carries no slot array.
        let mut chain = Vec::new();
        let mut layout = hx_proto::preset::Layout::default();
        let mut base = 0usize;
        for path_no in [0u8, 1] {
            let on_path: Vec<&hx_catalog::ToneBlock> =
                tone.blocks.iter().filter(|b| b.path == path_no).collect();
            if on_path.is_empty() {
                continue;
            }
            let input = base;
            chain.push(session::Block {
                position: input as i64,
                routing: Some(0),
                kind: hx_proto::preset::Kind::Input,
                model: 0,
                enabled: true,
                values: Vec::new(),
                paired: None,
                paired_values: Vec::new(),
            });
            let mut head = Vec::new();
            for (i, block) in on_path.iter().enumerate() {
                let position = base + 1 + i;
                chain.push(session::Block {
                    position: position as i64,
                    routing: None,
                    kind: hx_proto::preset::Kind::Block,
                    model: block.model_number,
                    enabled: block.enabled,
                    values: Vec::new(),
                    paired: None,
                    paired_values: Vec::new(),
                });
                head.push(position);
            }
            let output = base + 1 + on_path.len();
            chain.push(session::Block {
                position: output as i64,
                routing: Some(0),
                kind: hx_proto::preset::Kind::Output,
                model: 0,
                enabled: true,
                values: Vec::new(),
                paired: None,
                paired_values: Vec::new(),
            });
            layout.paths.push(hx_proto::preset::Path {
                input: Some(input),
                output: Some(output),
                head,
                ..Default::default()
            });
            base = output + 1;
        }

        self.note(format!("previewing {}", tone.name));
        self.preview = Some(Preview {
            name: tone.name.clone(),
            line: Self::tone_line(&tone),
            chain,
            layout,
            skipped: tone.skipped,
            load: LoadKind::Steps(blocks),
            dest: self.preset_index.max(0),
            source: ("hlx".to_owned(), bytes),
        });
    }

    /// Read a .hxpreset and show what it is, through the same codec the .hlx
    /// path uses, so one window understands either. The original bytes are kept
    /// so Load can write the document exactly.
    fn preview_hxpreset(&mut self, label: &str, bytes: Vec<u8>) {
        let Some(catalog) = self.catalog.as_ref() else {
            self.note("reading a tone needs HX Edit's model data first".into());
            return;
        };
        let Some(preset) = hx_proto::preset::Preset::parse(&bytes) else {
            self.note(format!("{label} is not a readable preset"));
            return;
        };
        let name = label;
        // The document knows its own chain and layout; the codec supplies the
        // one-line reading of what the tone is.
        let written = hx_catalog::to_hlx(&preset, catalog, name);
        let mut tone = hx_catalog::inspect(&written.document, catalog);
        tone.skipped.extend(written.skipped);
        self.note(format!("previewing {name}"));
        self.preview = Some(Preview {
            name: name.to_owned(),
            line: Self::tone_line(&tone),
            chain: session::chain_of(&preset),
            layout: preset.layout(),
            skipped: tone.skipped,
            load: LoadKind::Document(bytes.clone()),
            dest: self.preset_index.max(0),
            source: ("hxpreset".to_owned(), bytes),
        });
    }

    /// A tone file, shown the way the app always shows a tone: by the chain
    /// renderer itself, in display mode. Load asks where it should land, so
    /// nothing is overwritten by surprise; nothing touches the device before.
    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(mut preview) = self.preview.take() else {
            return;
        };
        let live = matches!(self.connection, Connection::Online);
        // The window opens at the chain's full width, and narrower only when
        // the screen cannot hold it - then the chain scrolls inside.
        let natural = self.preview_width(&preview.layout, theme::Tier::now(ctx)) + 24.0;
        let width = natural.min(ctx.content_rect().width() - 48.0);
        let mut open = true;
        let mut load = false;
        let mut cancel = false;
        // Kept after the window closes, not inside it: keeping can ask a
        // question of its own, and a window opened from inside another one's
        // closure is a window that draws underneath it.
        let mut keeping: Option<(String, String, Vec<u8>)> = None;

        // The one renderer draws whatever chain and layout the app holds, so
        // for the length of this window it holds the file's. The selection
        // stays with the real chain: a previewed tone has nothing selected.
        std::mem::swap(&mut self.chain, &mut preview.chain);
        std::mem::swap(&mut self.layout, &mut preview.layout);
        let selected = std::mem::replace(&mut self.selected, usize::MAX);
        self.display_only = true;

        let title = preview.name.clone();
        egui::Window::new(title)
            .open(&mut open)
            .resizable(true)
            .default_width(width)
            .show(ctx, |ui| {
                ui.label(RichText::new(&preview.line).color(theme::muted()));
                ui.add_space(6.0);
                egui::ScrollArea::horizontal()
                    .id_salt("tone-preview")
                    .show(ui, |ui| self.board_preview(ui));
                for skipped in &preview.skipped {
                    ui.label(RichText::new(skipped).small().color(theme::muted()));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Load into").color(theme::muted()));
                    let total = if self.presets.is_empty() {
                        self.preset_count as i64
                    } else {
                        self.presets.len() as i64
                    };
                    let labels: Vec<String> = (0..total)
                        .map(|i| {
                            let name = self.presets.get(i as usize).cloned().unwrap_or_default();
                            format!("{}  {}", self.active_slot_label(i), name)
                        })
                        .collect();
                    egui::ComboBox::from_id_salt("load-dest")
                        .selected_text(
                            labels
                                .get(preview.dest as usize)
                                .cloned()
                                .unwrap_or_default(),
                        )
                        .show_ui(ui, |ui| {
                            for (i, label) in labels.iter().enumerate() {
                                ui.selectable_value(&mut preview.dest, i as i64, label);
                            }
                        });
                    if ui
                        .add_enabled(live, egui::Button::new("Load"))
                        .on_hover_text("into that preset's edit buffer; Save keeps it")
                        .clicked()
                    {
                        load = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    // A tone worth keeping goes into the library. The button
                    // stays put and says which of the two it is, rather than
                    // vanishing: a hidden control is not an answer to "is this
                    // one saved?", it is the same question with less to go on.
                    let (kind, bytes) = &preview.source;
                    let held = library::holds(&library::hash_of(bytes));
                    let keep = ui
                        .add_enabled(!held, egui::Button::new("Keep"))
                        .on_hover_text(if held {
                            "this tone is already in your library"
                        } else {
                            "copy this tone into your library"
                        });
                    if keep.clicked() {
                        keeping = Some((preview.name.clone(), kind.clone(), bytes.clone()));
                    }
                });
            });

        self.display_only = false;
        self.selected = selected;
        std::mem::swap(&mut self.chain, &mut preview.chain);
        std::mem::swap(&mut self.layout, &mut preview.layout);

        if let Some((name, kind, bytes)) = keeping {
            self.keep_tone(&name, &kind, &bytes);
        }

        if load {
            self.load_preview(preview);
        } else if open && !cancel {
            self.preview = Some(preview);
        }
    }

    /// Load a previewed tone into the preset it is aimed at, through the same
    /// question as any other switch when that is not the preset loaded.
    fn load_preview(&mut self, preview: Preview) {
        self.on_preset(preview.dest, AfterSwitch::Tone(Box::new(preview)));
    }

    /// Whether to offer a parallel branch: the path has somewhere to put one,
    /// something to parallel, and nothing on the branch yet.
    fn can_offer_branch(&self, path: &hx_proto::preset::Path) -> bool {
        !self.display_only
            && path.lanes.is_empty()
            && !path.head.is_empty()
            && self.free_on_branch(path).is_some()
    }

    /// Whether two positions sit in the same lane - the main line between the
    /// input and the output, or the same branch of the same split.
    fn same_lane(&self, a: usize, b: usize) -> bool {
        self.layout.paths.iter().any(|p| {
            let main = p.input.map_or(0, |i| i + 1)..p.output.unwrap_or(usize::MAX);
            if main.contains(&a) && main.contains(&b) {
                return true;
            }
            let Some(split) = p.split else { return false };
            let branch = split + 1..p.join.unwrap_or(usize::MAX);
            branch.contains(&a) && branch.contains(&b)
        })
    }

    /// The first free slot on a path's branch, if it can carry one.
    fn free_on_branch(&self, path: &hx_proto::preset::Path) -> Option<usize> {
        let split = path.split?;
        let join = path.join.unwrap_or(usize::MAX);
        // A slot between the split and the join that nothing occupies.
        (split + 1..join).find(|p| !self.chain.iter().any(|b| b.position == *p as i64))
    }

    fn index_of(&self, slot: usize) -> Option<usize> {
        self.chain.iter().position(|b| b.position == slot as i64)
    }

    /// The first-run welcome, as a modal over everything: how to give the
    /// pedals their names and faces.
    ///
    /// The model names, knob ranges, and artwork are Line 6's and cannot
    /// ship inside this app, so the one-time step is the user supplying HX
    /// Edit's own installer. It does not dismiss - without that data there
    /// are no names, no ranges, and no pictures, so there is nothing to
    /// edit with - and the moment extraction finishes it closes itself and
    /// the catalog loads in place: no restart.
    fn onboarding_modal(&mut self, ctx: &egui::Context) {
        if !self.show_onboarding {
            return;
        }
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("onboarding-modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // The veil: dims the app and swallows every click, so the
                // step in front is unmistakably the only step.
                let (veil, _) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                ui.painter()
                    .rect_filled(veil, 0.0, egui::Color32::from_black_alpha(170));

                let card = egui::Rect::from_center_size(
                    screen.center(),
                    egui::vec2(560.0_f32.min(screen.width() - 24.0), 560.0),
                );
                let mut card_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(card)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                egui::Frame::popup(ui.style())
                    .inner_margin(28.0)
                    .show(&mut card_ui, |ui| {
                        ui.set_width(card.width() - 56.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("🎸").size(30.0));
                            ui.add_space(2.0);
                            ui.heading("Welcome to TonePush");
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "Thanks for downloading! One quick step, and you only \
                                     ever do it once.",
                                )
                                .color(theme::accent()),
                            );
                            ui.add_space(14.0);
                            ui.label(
                                "TonePush needs HX Edit's data files: every model's name, \
                                 knob ranges, and artwork.",
                            );
                        });
                        ui.add_space(12.0);

                        // Skimmable, not a wall: one emoji-led line each.
                        let bullets = [
                            (
                                "⚖",
                                "They are Line 6's files, so they cannot ship in this app.",
                            ),
                            (
                                "💻",
                                "Either installer works: the Mac .dmg or the Windows .exe.",
                            ),
                            (
                                "🔒",
                                "Extraction happens here; nothing leaves your machine.",
                            ),
                        ];
                        let block = ((ui.available_width() - 440.0) / 2.0).max(0.0);
                        for (icon, line) in bullets {
                            ui.horizontal(|ui| {
                                ui.add_space(block);
                                ui.label(RichText::new(icon).size(16.0));
                                ui.label(RichText::new(line).color(theme::text()));
                            });
                            ui.add_space(4.0);
                        }
                        ui.add_space(12.0);

                        ui.vertical_centered(|ui| {
                            if ui
                                .button(RichText::new("Download HX Edit from line6.com").strong())
                                .on_hover_text("free, but a Line 6 account is required")
                                .clicked()
                            {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(
                                    "https://line6.com/software/",
                                ));
                            }
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("then, once it is downloaded:").color(theme::muted()),
                            );
                            ui.add_space(8.0);
                            let row = button_width(ui, "Check my Downloads folder")
                                + button_width(ui, "Browse…")
                                + ui.spacing().item_spacing.x;
                            ui.horizontal(|ui| {
                                center_row(ui, row, |ui| {
                                    if ui
                                        .button("Check my Downloads folder")
                                        .on_hover_text(
                                            "looks for an HX Edit installer you already downloaded",
                                        )
                                        .clicked()
                                    {
                                        match hx_catalog::extract::installer_in_downloads() {
                                            Some(installer) => self.extract_resources(installer),
                                            None => self.onboarding_status = Some(
                                                "no HX Edit installer in your Downloads folder yet"
                                                    .into(),
                                            ),
                                        }
                                    }
                                    if ui
                                        .button("Browse…")
                                        .on_hover_text("pick the installer wherever it landed")
                                        .clicked()
                                    {
                                        if let Some(installer) = rfd::FileDialog::new()
                                            .add_filter("HX Edit installer", &["dmg", "exe"])
                                            .pick_file()
                                        {
                                            self.extract_resources(installer);
                                        }
                                    }
                                });
                            });
                            // Wayland cannot deliver a file drop to this
                            // window (a windowing-library gap), so the hint
                            // only appears where dropping actually works.
                            if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("…or drop the installer anywhere on this window")
                                        .small()
                                        .color(theme::muted()),
                                );
                            }

                            ui.add_space(10.0);
                            if self.extracting.is_some() {
                                ui.horizontal(|ui| {
                                    theme::spinner(ui);
                                    if let Some(status) = &self.onboarding_status {
                                        ui.label(RichText::new(status).color(theme::muted()));
                                    }
                                });
                            } else if let Some(status) = &self.onboarding_status {
                                ui.label(RichText::new(status).color(theme::accent()));
                            }
                        });

                        ui.add_space(14.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(
                                    "Free and open source, MIT licensed. Not affiliated with \
                                     Yamaha Guitar Group.",
                                )
                                .small()
                                .color(theme::muted()),
                            );
                            ui.horizontal(|ui| {
                                center_row(ui, credits_width(ui), |ui| {
                                    ui.label(
                                        RichText::new("made with ♥ by")
                                            .small()
                                            .color(theme::muted()),
                                    );
                                    ui.hyperlink_to(
                                        RichText::new("Carmine Paolino").small(),
                                        "https://paolino.me",
                                    );
                                    ui.label(RichText::new("·").small().color(theme::muted()));
                                    ui.hyperlink_to(
                                        RichText::new("follow updates").small(),
                                        "https://x.com/paolino",
                                    );
                                    ui.label(RichText::new("·").small().color(theme::muted()));
                                    ui.hyperlink_to(
                                        RichText::new("♥ sponsor").small(),
                                        "https://github.com/sponsors/crmne",
                                    );
                                });
                            });
                        });
                    });
            });
    }

    /// Copy the block in a slot. The worker reads it out of the document and
    /// keeps it whole, for pasting into this preset or another.
    fn copy_block(&mut self, slot: usize, block: &session::Block) {
        self.copied_block = true;
        self.send(Cmd::CopyBlock(slot));
        self.note(format!("copied {}", self.slot_label(block)));
    }

    /// Put the copied block into a slot.
    fn paste_block(&mut self, slot: usize) {
        if self.copied_block {
            self.edit(Cmd::PasteBlock(slot));
        }
    }

    /// What reaches each block, by block, out of the preset document.
    ///
    /// Every source, not only the footswitches: an expression pedal on a wah is
    /// exactly the assignment you want to see without hunting for it, and the
    /// document lists it beside the switches at no extra cost. This used to come
    /// from opcode 33, which knows about switches and nothing else.
    fn control_marks(
        &self,
    ) -> std::collections::BTreeMap<i64, Vec<(hx_proto::rpc::Source, String)>> {
        let mut marks: std::collections::BTreeMap<i64, Vec<_>> = Default::default();
        for assignment in self.all_assignments() {
            marks
                .entry(assignment.block)
                .or_default()
                .push((assignment.source, assignment.target));
        }
        marks
            .into_iter()
            .map(|(block, mut found)| {
                found.sort_by_key(|(source, _)| source.ordinal());
                let named = found
                    .into_iter()
                    .map(|(source, target)| {
                        let what = match Self::auto_engage(source, target) {
                            true => "Auto-engage".to_owned(),
                            false => self.target_name(block, target),
                        };
                        (source, what)
                    })
                    .collect();
                (block, named)
            })
            .collect()
    }

    /// The same thing said in full, for the cell itself. A header speaks for a
    /// whole column; this speaks for one row, which is what you want when the
    /// two rows disagree.
    fn end_meaning(source: hx_proto::rpc::Source, high: bool) -> String {
        use hx_proto::rpc::Source;
        match (source, high) {
            (Source::Footswitch(n), false) => format!("what it reads with Footswitch {n} off"),
            (Source::Footswitch(n), true) => format!("what it reads with Footswitch {n} on"),
            (Source::Expression(n), false) => {
                format!("what it reads with Expression Pedal {n} at the heel")
            }
            (Source::Expression(n), true) => {
                format!("what it reads with Expression Pedal {n} at the toe")
            }
            (Source::MidiCc, false) => "what it reads when the CC sends 0".to_owned(),
            (Source::MidiCc, true) => "what it reads when the CC sends 127".to_owned(),
            (Source::Snapshots, high) => match high {
                false => "the low end of what a snapshot can set".to_owned(),
                true => "the high end of what a snapshot can set".to_owned(),
            },
        }
    }

    /// Move one end of an assignment's travel.
    ///
    /// `value` is in the parameter's own units, `range` its span: the document
    /// holds the ends that way and the table shows them that way, but opcodes
    /// 65 and 66 take an end as a fraction of the range. Sending the units
    /// meant every end past 1.0 - a pitch block's 12 semitones, a delay's
    /// milliseconds - was refused, and the refusal used to end the session.
    fn move_travel(
        &mut self,
        block: i64,
        target: hx_proto::preset::Target,
        high_end: bool,
        value: f32,
        range: &std::ops::RangeInclusive<f32>,
    ) {
        // 65 and 66 take a parameter index, so a bypass has no end to move.
        let hx_proto::preset::Target::Param(param) = target else {
            return;
        };
        let Some(fraction) = travel_fraction(range, value) else {
            return;
        };
        // Keep our own copy in step as it turns. Dragging streams one write per
        // intermediate value and the worker does not re-read the document for
        // each - that would be a document read per pixel - so nothing else
        // brings the new value back. Without this the knob is redrawn from the
        // value it had before the drag started and springs back under the
        // pointer, which is what a knob that cannot be moved looks like.
        if let Some(moved) = self
            .assignments
            .iter_mut()
            .find(|a| a.block == block && a.target == target)
        {
            if high_end {
                moved.max = value;
            } else {
                moved.min = value;
            }
        }
        self.edit(Cmd::SetAssignRange {
            block,
            param,
            value: fraction,
            high_end,
        });
    }

    /// The parameter an assignment drives, for drawing its ends the way that
    /// parameter is drawn everywhere else. `None` for a bypass, which is a
    /// switch and has no travel.
    fn travel_of(&self, block: i64, target: hx_proto::preset::Target) -> Option<Travel> {
        let hx_proto::preset::Target::Param(index) = target else {
            return None;
        };
        let catalog = self.catalog.as_ref()?;
        let model = self
            .chain
            .iter()
            .find(|b| b.position == block)
            .and_then(|b| self.slot_model(b))?;
        let param = catalog.ordered_params(model).get(index as usize).copied()?;
        Some(Travel {
            range: param.min..=param.max,
            param: param.clone(),
        })
    }

    /// Everything the bypass control needs, gathered before the controls draw.
    ///
    /// Gathered rather than looked up while drawing because the control sits in
    /// the same grid as the knobs, and that grid is laid out holding a borrow
    /// of the catalog. A value cannot fight a borrow.
    fn bypass_view(&self, position: i64) -> BypassView {
        use hx_proto::preset::Target;
        let block = self.chain.iter().find(|b| b.position == position);
        let menu = self.assign_view(position, Target::Bypass, "On/Off".to_owned());
        BypassView {
            position,
            enabled: block.is_some_and(|b| b.enabled),
            auto_engage: menu
                .under
                .is_some_and(|source| Self::auto_engage(source, Target::Bypass)),
            menu,
        }
    }

    /// Carry out what the bypass control was asked to do.
    fn bypass_action(&mut self, position: i64, action: BypassAction) {
        match action {
            BypassAction::Toggle(enabled) => {
                self.edit(Cmd::SetEnabled {
                    block: position,
                    enabled,
                });
                if let Some(slot) = self.chain.iter_mut().find(|b| b.position == position) {
                    slot.enabled = enabled;
                }
            }
            BypassAction::Assign(chose) => {
                self.assign_action(position, hx_proto::preset::Target::Bypass, chose)
            }
        }
    }

    /// How many footswitches to offer, from the device's own profile.
    fn switch_count(&self) -> u8 {
        if self.switches.is_empty() {
            5
        } else {
            self.switches.len() as u8
        }
    }

    /// What colour to paint a footswitch.
    ///
    /// The pedal speaks two dialects in the one field, and they are told apart
    /// by size. A colour *chosen* for a switch reads back as the index opcode
    /// 61 took: setting 1, 2, 5, 6, 8 and 11 and reading each back gave the
    /// same number every time, and Auto Color reads back as nothing at all.
    /// A colour *inherited* from what the switch carries is a real `0xRRGGBB`,
    /// and the darkest of those is far above the end of that list.
    fn led_colour(&self, colour: Option<i64>) -> egui::Color32 {
        let colours = self
            .catalog
            .as_ref()
            .and_then(|c| c.menu(hx_catalog::FOOTSWITCH_LED))
            .unwrap_or_default();
        if let Some(name) = colour
            .filter(|n| *n >= 0)
            .and_then(|n| colours.get(n as usize))
        {
            return theme::led_swatch(name);
        }
        Self::rgb_colour(colour)
    }

    /// The pedal sends an assignment's colour as `0xRRGGBB`.
    fn rgb_colour(colour: Option<i64>) -> egui::Color32 {
        match colour {
            Some(rgb) => egui::Color32::from_rgb(
                ((rgb >> 16) & 0xff) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            ),
            None => theme::muted(),
        }
    }

    /// The colour HX Edit gives this block's category.
    ///
    /// Effects only. An endpoint reports model 0, which is a real amp in the
    /// symbol table, so resolving it painted the input and output in the amp
    /// category's red.
    fn block_colour(&self, block: &session::Block) -> egui::Color32 {
        let fallback = theme::muted();
        if block.kind != hx_proto::preset::Kind::Block || block.model == 0 {
            return fallback;
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return fallback;
        };
        catalog
            .model_number(block.model)
            .and_then(|m| catalog.category_of(&m.id))
            .and_then(|c| catalog.category(c))
            .map(|c| theme::category_colour(&c.name))
            .unwrap_or(fallback)
    }
}

/// Centre a fixed-width row of widgets inside a horizontal `ui`.
///
/// egui lays a row out left to right with no idea how wide its content will
/// end up, so centring means telling it: pad by half of what is left over.
fn center_row(ui: &mut egui::Ui, content_width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(((ui.available_width() - content_width) / 2.0).max(0.0));
    add(ui);
}

/// What a button with this label will measure, for centring rows of them.
fn button_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, theme::text()));
    galley.size().x + 2.0 * ui.spacing().button_padding.x
}

/// The measured width of the credits line, so it can be centred exactly.
fn credits_width(ui: &egui::Ui) -> f32 {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let pieces = [
        "made with ♥ by",
        "Carmine Paolino",
        "·",
        "follow updates",
        "·",
        "♥ sponsor",
    ];
    let text: f32 = ui.fonts_mut(|fonts| {
        pieces
            .iter()
            .map(|piece| {
                fonts
                    .layout_no_wrap((*piece).to_owned(), font.clone(), theme::muted())
                    .size()
                    .x
            })
            .sum()
    });
    text + ui.spacing().item_spacing.x * (pieces.len() - 1) as f32
}

/// One line on what a split type does with the signal, for its chip's hover.
fn split_type_hint(name: &str) -> &'static str {
    match name {
        "Split Y" => "the signal runs down both branches",
        "Split A/B" => "the signal takes one branch at a time",
        "Split Crossover" => "splits the signal by frequency",
        "Split Dynamic" => "splits the signal by playing level",
        _ => "how the signal divides at the fork",
    }
}

/// The tag worn by a fork in the chain, for types that change how the preset
/// behaves. The default Y is silent - a tag is for the deviations worth
/// noticing at a glance.
fn split_tag(name: &str) -> Option<&'static str> {
    match name {
        "Split A/B" => Some("A/B"),
        "Split Crossover" => Some("XO"),
        "Split Dynamic" => Some("DYN"),
        _ => None,
    }
}

/// Where a dragged fork or merge may go: the lowest and highest slot it can
/// attach before, and where it is attached now.
///
/// A fork ranges from just after the input to the merge; a merge, from the
/// fork to the output. The ends may meet - a stretch of zero width is how the
/// device itself represents a branch that parallels nothing.
fn attach_range(path: &hx_proto::preset::Path, opening: bool) -> Option<(usize, usize, usize)> {
    // The stretch the lanes span *is* the pair of attach points.
    let span = path.lanes.first().map(|l| l.span.clone())?;
    Some(if opening {
        (path.input.map_or(0, |i| i + 1), span.end, span.start)
    } else {
        (span.start, path.output.unwrap_or(span.end), span.end)
    })
}

/// A block's category colour by the catalog, for a chain drawn from a
/// document: effects only, in the muted ink when the catalog does not know it.
fn catalog_colour(catalog: &Catalog, block: &session::Block) -> Option<egui::Color32> {
    if block.kind != hx_proto::preset::Kind::Block {
        return None;
    }
    Some(
        catalog
            .model_number(block.model)
            .and_then(|model| catalog.category_of(&model.id))
            .and_then(|id| catalog.category(id))
            .map_or_else(theme::muted, |category| {
                theme::category_colour(&category.name)
            }),
    )
}

/// The device speaks in model numbers, and only knows the models its firmware
/// carries - a catalog entry with no symbol cannot be sent.
fn number_of(catalog: &hx_catalog::Catalog, id: &str) -> Option<u32> {
    catalog
        .symbols()
        .iter()
        .find(|s| s.model.as_deref() == Some(id))
        .map(|s| s.number)
}

/// A band's heading: its colour as a dot, then its name in that colour.
fn eq_group_title(ui: &mut egui::Ui, name: &str, colour: egui::Color32) {
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(9.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, colour);
        ui.label(RichText::new(name).small().color(colour).strong());
    });
}

/// One peaking band's three numbers, under its own colour.
fn eq_band_group(
    ui: &mut egui::Ui,
    name: &str,
    colour: egui::Color32,
    ids: (i64, i64, i64),
    band: eq::Band,
    freq_range: std::ops::RangeInclusive<f32>,
) -> Option<(i64, f32)> {
    let (freq_id, q_id, gain_id) = ids;
    eq_group_title(ui, name, colour);
    let mut write = None;

    let mut freq = band.freq;
    if ui
        .add(
            egui::DragValue::new(&mut freq)
                .range(freq_range)
                // A hertz at a time down low and a hundred up top is one
                // control that suits both ends; a fixed step suits neither.
                .speed(band.freq.max(20.0) / 60.0)
                .suffix(" Hz")
                .fixed_decimals(0),
        )
        .on_hover_text("centre frequency")
        .changed()
    {
        write = Some((freq_id, freq));
    }
    let mut gain = band.gain_db;
    if ui
        .add(
            egui::DragValue::new(&mut gain)
                .range(-12.0..=12.0)
                .speed(0.1)
                .suffix(" dB")
                .fixed_decimals(1),
        )
        .on_hover_text("cut or boost at that frequency")
        .changed()
    {
        write = Some((gain_id, gain));
    }
    let mut q = band.q;
    if ui
        .add(
            egui::DragValue::new(&mut q)
                .range(0.1..=10.0)
                .speed(0.02)
                .prefix("Q ")
                .fixed_decimals(2),
        )
        .on_hover_text("how narrow the band is")
        .changed()
    {
        write = Some((q_id, q));
    }
    write
}

/// A cut's one number, and the word for having it switched off.
fn eq_cut_group(
    ui: &mut egui::Ui,
    name: &str,
    colour: egui::Color32,
    id: i64,
    value: f32,
    parked: f32,
    range: std::ops::RangeInclusive<f32>,
) -> Option<(i64, f32)> {
    eq_group_title(ui, name, colour);
    let mut write = None;
    let off = (value - parked).abs() < 0.5;

    let mut hz = value;
    if ui
        .add(
            egui::DragValue::new(&mut hz)
                .range(range)
                .speed(value.max(20.0) / 60.0)
                .suffix(" Hz")
                .fixed_decimals(0),
        )
        .on_hover_text("where the roll-off starts")
        .changed()
    {
        write = Some((id, hz));
    }
    // The device has no off switch for these: it parks them outside the band.
    // Saying so beats leaving someone to work out why 20100 Hz does nothing.
    ui.label(
        RichText::new(if off { "off" } else { "on" })
            .small()
            .color(if off { theme::muted() } else { colour }),
    );
    write
}

/// What one control's assignment menu shows, as values rather than lookups.
///
/// One shape for a knob and for a block's on/off. They were two menus once, and
/// only the bypass one could say "Footswitch 1 carries Trinity Chorus" - which
/// is the sentence you most want before putting a second thing on a switch.
struct AssignMenu {
    /// The control's own name, for the menu's first line.
    name: String,
    /// What drives it now.
    under: Option<hx_proto::rpc::Source>,
    /// Every source this control can take, and what each already carries.
    sources: Vec<(hx_proto::rpc::Source, Vec<String>)>,
}

/// A footswitch's own settings, as the panel draws them.
struct SwitchView {
    /// The name in the field, which is the draft while one is being typed.
    label: String,
    /// What the pedal writes under it when no name has been typed: the first
    /// thing it carries. Shown as the field's hint, so an empty field says what
    /// empty means rather than looking like a missing setting.
    carries: Option<String>,
    colour: Option<i64>,
    momentary: bool,
}

/// What choosing something in that menu means.
enum AssignAction {
    /// Put the control under this source, or take it off whatever has it.
    To(Option<hx_proto::rpc::Source>),
    /// Which CC drives it, once MIDI does. From the table rather than the menu:
    /// the number is an adjustment, not a choice of source.
    Cc(i64),
}

/// What the footswitch settings under the table were asked to do.
enum SwitchChange {
    /// A change to the switch itself rather than to what it carries.
    Set {
        switch: u8,
        edit: session::SwitchEdit,
    },
    /// The name field is being typed into. Kept in the app so the draft
    /// survives the frame, the same as every other field here.
    Typing(u8, String),
}

/// What the pedal picks for itself when a MIDI assignment is made, and so what
/// the field shows before anybody chooses.
const DEFAULT_CC: i64 = 4;

/// What the bypass control shows, beyond the menu every control shares.
struct BypassView {
    position: i64,
    enabled: bool,
    /// Whether what drives it is an expression pedal, which does not switch the
    /// block so much as let it switch itself. See `App::auto_engage`.
    auto_engage: bool,
    menu: AssignMenu,
}

/// What pressing something on it means.
enum BypassAction {
    Toggle(bool),
    Assign(AssignAction),
}

/// The parameter an assignment's ends belong to.
///
/// They are not percentages: the document holds a pitch block's ends as 7 and
/// 12 semitones, because they are values of that parameter. So they are shown
/// and typed exactly as that parameter is shown and typed under the knobs - the
/// same range, the same units, the same widget.
struct Travel {
    range: std::ops::RangeInclusive<f32>,
    param: hx_catalog::Param,
}

/// Where a value sits in a parameter's range, as the fraction from 0.0 to
/// 1.0 that the travel opcodes take. `None` for a value or a range that is not
/// a number, and for a range with no width to divide.
fn travel_fraction(range: &std::ops::RangeInclusive<f32>, value: f32) -> Option<f32> {
    let (low, high) = (*range.start(), *range.end());
    if !value.is_finite() || !low.is_finite() || !high.is_finite() || high <= low {
        return None;
    }
    Some(((value - low) / (high - low)).clamp(0.0, 1.0))
}

fn bypass_popup_id(block: i64) -> egui::Id {
    egui::Id::new(("bypass-assign", block))
}

fn popup_id(block: i64, param: usize) -> egui::Id {
    egui::Id::new(("assign", block, param))
}

/// Make a preset name safe to use as a filename.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            // Spaces are fine in a file name on every platform this runs on;
            // a colon is not, on two of them. Replacing everything that was
            // not alphanumeric turned "CT-Day CLN" into "CT-Day_CLN" and then
            // showed that back as the tone's name.
            if c.is_alphanumeric() || " -_.()&+'".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "preset".to_owned()
    } else {
        cleaned
    }
}

/// Whether a portable tone is present in the web library.
///
/// Kept separate from the asynchronous request so the important distinction
/// between no answer and a successful empty answer stays pinned by tests.
fn cloud_presence(
    files: Option<&std::collections::BTreeSet<String>>,
    portable: &str,
) -> theme::Sync {
    match files {
        None => theme::Sync::Unknown,
        Some(files) if files.contains(portable) => theme::Sync::Same,
        Some(_) => theme::Sync::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// An app with channels that go nowhere, for testing state handling.
    fn app() -> (App, mpsc::Sender<Evt>, mpsc::Receiver<Cmd>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        (
            App::new(&egui::Context::default(), cmd_tx, evt_rx),
            evt_tx,
            cmd_rx,
        )
    }

    #[test]
    fn an_empty_cloud_answer_offers_the_first_publish() {
        let empty = std::collections::BTreeSet::new();
        assert!(matches!(
            cloud_presence(None, "portable"),
            theme::Sync::Unknown
        ));
        assert!(matches!(
            cloud_presence(Some(&empty), "portable"),
            theme::Sync::Absent
        ));
    }

    #[test]
    fn a_cloud_refresh_never_forgets_an_authoritative_published_hash() {
        let (mut app, _events, _cmds) = app();
        let local = "local-object".to_owned();
        let portable = "a".repeat(64);
        app.portable_hashes.insert(local.clone(), portable.clone());
        app.cloud_files = Some(std::collections::BTreeSet::from([portable]));

        let (answer, received) = mpsc::channel();
        answer.send(std::collections::BTreeSet::new()).unwrap();
        app.cloud_check = Some(received);

        assert!(matches!(app.cloud_sync(&local), theme::Sync::Same));
    }

    #[test]
    fn a_finished_publish_opens_the_published_tone() {
        let (mut app, _events, _cmds) = app();
        let tone: cloud::ToneDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        let portable = tone.file_sha256.clone().unwrap();
        let local = "library-object".to_owned();
        app.portable_hashes.insert(local.clone(), portable.clone());

        let (answer, received) = mpsc::channel();
        answer.send(Ok(tone)).unwrap();
        app.publishing = Some(PublishingJob {
            hash: local,
            name: "Numb HX".to_owned(),
            answer: received,
        });

        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        app.settle_publishing(&ctx);
        let mut output = ctx.end_pass();
        let opened = output.platform_output.commands.iter().find_map(|command| {
            if let egui::OutputCommand::OpenUrl(url) = command {
                Some(url.url.clone())
            } else {
                None
            }
        });
        output.textures_delta.clear();

        let expected = cloud::tone_url(&portable);
        assert_eq!(opened.as_deref(), Some(expected.as_str()));
    }

    /// The app reaches for the device on startup, so it begins in Connecting
    /// rather than waiting to be told.
    #[test]
    fn connecting_populates_the_device_and_preset_count() {
        let (mut app, events, _cmds) = app();
        assert_eq!(app.connection, Connection::Connecting);

        events
            .send(Evt::Connected {
                device: "HX Stomp".into(),
                presets: 126,
            })
            .unwrap();
        app.drain_events();

        assert_eq!(app.connection, Connection::Online);
        assert_eq!(app.device, "HX Stomp");
        assert_eq!(app.preset_count, 126);
        assert!(app.cloud_loaded_device.is_none());
        assert!(
            app.cloud_search_due.is_some(),
            "connecting must replace the device-agnostic Cloud prefetch"
        );
    }

    #[test]
    fn a_new_pedal_becomes_the_library_scope_without_fighting_all_pedals() {
        let (mut app, events, _cmds) = app();
        events
            .send(Evt::Connected {
                device: "HX Stomp".into(),
                presets: 126,
            })
            .unwrap();
        app.drain_events();
        app.observe_library_device();
        assert_eq!(app.library_device_filter.as_deref(), Some("HX Stomp"));

        app.library_device_filter = None;
        app.observe_library_device();
        assert!(
            app.library_device_filter.is_none(),
            "a frame redraw must not undo the explicit All pedals choice"
        );

        app.connection = Connection::Offline;
        app.observe_library_device();
        events
            .send(Evt::Connected {
                device: "HX Stomp".into(),
                presets: 126,
            })
            .unwrap();
        app.drain_events();
        app.observe_library_device();
        assert_eq!(
            app.library_device_filter.as_deref(),
            Some("HX Stomp"),
            "a reconnect is a new pedal-selection event"
        );
    }

    /// A dropped session must not leave the last preset on screen, or the UI
    /// shows a chain the device is no longer holding.
    #[test]
    fn disconnecting_clears_what_was_on_screen() {
        let (mut app, events, _cmds) = app();
        events
            .send(Evt::Connected {
                device: "HX Stomp".into(),
                presets: 126,
            })
            .unwrap();
        events
            .send(Evt::Presets(vec!["One".into(), "Two".into()]))
            .unwrap();
        events
            .send(Evt::Loaded {
                index: 7,
                name: "CT-Sad".into(),
                firmware: "3.80".into(),
                tempo: Some(120.0),
                snapshots: vec!["SNAPSHOT 1".into()],
                snapshot: None,
                snapshot_details: Vec::new(),
                layout: hx_proto::preset::Layout::default(),
                assignments: Vec::new(),
                dirty: false,
                chain: vec![session::Block {
                    position: 1,
                    routing: None,
                    kind: hx_proto::preset::Kind::Block,
                    model: 101,
                    enabled: true,
                    values: vec![0.5],
                    paired: None,
                    paired_values: vec![],
                }],
            })
            .unwrap();
        app.drain_events();
        assert_eq!(app.chain.len(), 1);
        assert_eq!(app.preset_name, "CT-Sad");

        events
            .send(Evt::Failed("the USB session was lost".into()))
            .unwrap();
        events.send(Evt::Disconnected).unwrap();
        app.drain_events();

        assert_eq!(app.connection, Connection::Offline);
        assert!(app.chain.is_empty());
        assert!(app.presets.is_empty());
        assert_eq!(app.preset_count, 0);
        assert_eq!(app.preset_index, -1);
        assert!(app.preset_name.is_empty());
        assert!(app.firmware.is_empty());
        assert!(app.snapshots.is_empty());
        assert_eq!(app.status, "the USB session was lost");
    }

    /// Keeping the pedal first writes nothing until the copy is in the
    /// library, saved without asking for a name, and then writes the setlist
    /// that was confirmed.
    #[test]
    fn keeping_the_pedal_first_writes_only_once_the_copy_is_kept() {
        let _scratch = library::tests::Scratch::new("keep-first");
        let (mut app, events, cmds) = app();
        events
            .send(Evt::Connected {
                device: "HX Stomp".into(),
                presets: 126,
            })
            .unwrap();
        app.drain_events();
        let _ = cmds.try_iter().collect::<Vec<_>>();
        let (hash, _) = library::keep("Night Verb", "hxpreset", b"night").unwrap();
        let setlist = library::Setlist {
            name: "Album release show".into(),
            slots: vec![library::Slot::new(&hash, "Night Verb")],
            ..Default::default()
        };

        app.put_setlist(setlist, true);
        let sent: Vec<Cmd> = cmds.try_iter().collect();
        assert!(sent.iter().any(|cmd| matches!(cmd, Cmd::CaptureSetlist)));
        assert!(
            !sent.iter().any(|cmd| matches!(cmd, Cmd::PushSetlist(_))),
            "nothing is written before the pedal is kept"
        );

        events
            .send(Evt::CapturedSetlist(vec![
                ("Glass Clean".into(), Some(b"glass".to_vec())),
                (String::new(), None),
            ]))
            .unwrap();
        app.drain_events();
        let sent: Vec<Cmd> = cmds.try_iter().collect();
        assert!(sent.iter().any(|cmd| matches!(
            cmd,
            Cmd::WriteSetlist { name, slots } if name == "Album release show" && slots.len() == 1
        )));
        assert!(app.setlist_save.is_none(), "the copy is saved as it is");
        assert!(library::setlists().iter().any(|(_, kept)| {
            kept.name == "HX Stomp"
                && kept.filled() == 1
                && kept.description.contains("Album release show")
        }));
    }

    /// A capture that could not keep every preset stops the write it was
    /// meant to precede, and a later capture is only a capture.
    #[test]
    fn a_capture_that_lost_presets_stops_the_write() {
        let _scratch = library::tests::Scratch::new("keep-first-lost");
        let (mut app, events, cmds) = app();
        events
            .send(Evt::Connected {
                device: "HX Stomp".into(),
                presets: 126,
            })
            .unwrap();
        app.drain_events();
        let (hash, _) = library::keep("Night Verb", "hxpreset", b"night").unwrap();
        let setlist = library::Setlist {
            name: "Album release show".into(),
            slots: vec![library::Slot::new(&hash, "Night Verb")],
            ..Default::default()
        };
        app.put_setlist(setlist.clone(), true);
        app.keep_then_push(library::Setlist::default(), 1, &setlist);
        assert!(!cmds
            .try_iter()
            .any(|cmd| matches!(cmd, Cmd::WriteSetlist { .. })));
        assert!(app.setlist_save.is_some(), "what was read is still offered");

        // The capture that would have preceded it arrives late, after the
        // person asked for an ordinary one: it is kept, and nothing written.
        app.put_setlist(setlist, true);
        assert!(app.capture_pedal(None));
        events
            .send(Evt::CapturedSetlist(vec![(
                "Glass Clean".into(),
                Some(b"glass".to_vec()),
            )]))
            .unwrap();
        app.drain_events();
        assert!(!cmds
            .try_iter()
            .any(|cmd| matches!(cmd, Cmd::WriteSetlist { .. })));
    }

    #[test]
    fn a_failure_while_connecting_returns_to_offline() {
        let (mut app, events, _cmds) = app();
        app.connection = Connection::Connecting;
        events.send(Evt::Failed("no device".into())).unwrap();
        app.drain_events();

        assert_eq!(app.connection, Connection::Offline);
        assert_eq!(
            app.status,
            "No supported pedal found. Check USB and close any other pedal editor."
        );
    }

    /// The log is unbounded input from the device, so it must not grow forever.
    #[test]
    fn the_activity_log_is_bounded() {
        let (mut app, events, _cmds) = app();
        for i in 0..400 {
            events.send(Evt::Activity(format!("event {i}"))).unwrap();
        }
        app.drain_events();

        assert!(app.log.len() <= 300, "log grew to {}", app.log.len());
        assert_eq!(app.log.last().unwrap(), "event 399");
    }

    /// The fork wears a tag only when its type changes the preset's
    /// behaviour; the default Y stays quiet.
    #[test]
    fn only_the_notable_split_types_wear_a_tag() {
        assert_eq!(split_tag("Split Y"), None);
        assert_eq!(split_tag("Split A/B"), Some("A/B"));
        assert_eq!(split_tag("Split Crossover"), Some("XO"));
        assert_eq!(split_tag("Split Dynamic"), Some("DYN"));
        assert_eq!(split_tag("Mixer"), None, "a merge has no type to announce");
    }

    /// The lookups the type chips run: the split's category holds the whole
    /// family, and every member resolves to a number the firmware accepts.
    /// Needs HX Edit's resources; skips quietly where they are not installed.
    #[test]
    fn the_split_family_resolves_through_the_catalog() {
        let Ok(catalog) = Catalog::load() else {
            return;
        };
        let split_y = catalog.model_number(257).expect("Split Y is model 257");
        let family = catalog.models_in(catalog.category_of(&split_y.id).expect("a category"));
        let names: Vec<&str> = family.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["Split Y", "Split A/B", "Split Crossover", "Split Dynamic"],
            "the family, in the order the chips show"
        );
        for model in family {
            assert!(
                catalog
                    .symbols()
                    .iter()
                    .any(|s| s.model.as_deref() == Some(model.id.as_str())),
                "{} resolves to a firmware number",
                model.name
            );
        }
    }

    /// A fork may travel between the input and the merge, a merge between the
    /// fork and the output - and they may meet, because a zero-width stretch
    /// is how the device represents a branch that parallels nothing.
    #[test]
    fn a_dragged_junction_stays_between_its_neighbours() {
        use hx_proto::preset::{Lane, Path};
        let path = Path {
            input: Some(0),
            output: Some(9),
            split: Some(10),
            join: Some(19),
            head: vec![1],
            lanes: vec![
                Lane {
                    branch: 0,
                    blocks: vec![2, 3],
                    span: 2..4,
                },
                Lane {
                    branch: 1,
                    blocks: vec![11],
                    span: 11..19,
                },
            ],
            tail: vec![4],
        };

        assert_eq!(
            attach_range(&path, true),
            Some((1, 4, 2)),
            "the fork ranges from after the input to the merge"
        );
        assert_eq!(
            attach_range(&path, false),
            Some((2, 9, 4)),
            "the merge ranges from the fork to the output"
        );

        let straight = Path {
            head: vec![1],
            ..Path::default()
        };
        assert_eq!(attach_range(&straight, true), None, "no lanes, no drag");
    }

    /// An app holding a branched chain - a drive on the main line, a delay on
    /// the branch - for tests that drive the chain panel with a pointer.
    fn branched_app() -> (App, mpsc::Receiver<Cmd>) {
        use hx_proto::preset::{Kind, Lane, Layout, Path};
        let (mut app, events, cmds) = app();

        let slot = |position: i64, kind| session::Block {
            position,
            routing: None,
            kind,
            model: 0,
            enabled: true,
            values: vec![],
            paired: None,
            paired_values: vec![],
        };
        events
            .send(Evt::Loaded {
                index: 0,
                name: "Test".into(),
                firmware: String::new(),
                tempo: None,
                snapshots: vec![],
                snapshot: None,
                snapshot_details: Vec::new(),
                assignments: vec![],
                chain: vec![
                    slot(0, Kind::Input),
                    slot(1, Kind::Block),
                    slot(9, Kind::Output),
                    slot(10, Kind::Split),
                    slot(11, Kind::Block),
                    slot(19, Kind::Join),
                ],
                layout: Layout {
                    paths: vec![Path {
                        input: Some(0),
                        output: Some(9),
                        split: Some(10),
                        join: Some(19),
                        head: vec![],
                        lanes: vec![
                            Lane {
                                branch: 0,
                                blocks: vec![1],
                                span: 1..9,
                            },
                            Lane {
                                branch: 1,
                                blocks: vec![11],
                                span: 11..19,
                            },
                        ],
                        tail: vec![],
                    }],
                },
                dirty: true,
            })
            .unwrap();
        app.drain_events();
        (app, cmds)
    }

    /// Draw the chain once so the frame's gap and block rects are recorded,
    /// then release the pointer at `at` and draw again.
    fn draw_then_release_at(app: &mut App, at: impl FnOnce(&App) -> egui::Pos2) -> egui::Context {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 600.0));
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // The board is drawn in the editor's own type, which a context has
        // from the pass after it is installed.
        theme::install(&ctx, theme::Appearance::Dark);
        ctx.run_ui(input.clone(), |_| {})
            .drop_without_applying_deltas();
        ctx.run_ui(input.clone(), |ui| app.signal_chain(ui))
            .drop_without_applying_deltas();
        let pos = at(app);
        input.events.push(egui::Event::PointerMoved(pos));
        input.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.run_ui(input, |ui| app.signal_chain(ui))
            .drop_without_applying_deltas();
        ctx
    }

    fn gap_centre(app: &App, before: usize) -> egui::Pos2 {
        app.gap_rects
            .iter()
            .find(|(b, _)| *b == before)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| panic!("no gap before slot {before}"))
    }

    /// The whole drag, without a screen: draw the chain once so the gaps are
    /// known, put the merge in hand, release the pointer over the gap before
    /// the drive - and the worker must be asked to re-attach the join there.
    #[test]
    fn releasing_a_dragged_merge_reattaches_it_at_the_nearest_gap() {
        let (mut app, cmds) = branched_app();
        app.dragging_junction = Some((19, false));
        draw_then_release_at(&mut app, |app| gap_centre(app, 1));

        assert!(app.dragging_junction.is_none(), "the drag ended");
        assert!(
            cmds.try_iter().any(|c| matches!(
                c,
                Cmd::MoveJunction {
                    junction: 19,
                    before: 1
                }
            )),
            "the worker was asked to re-attach the join before slot 1"
        );
    }

    /// Dropping a block into a gap slides it in there - the branch's delay
    /// released over the main line's leading gap moves before the drive.
    #[test]
    fn dropping_a_block_into_a_gap_moves_it_before_that_slot() {
        let (mut app, cmds) = branched_app();
        app.dragging = Some(11);
        draw_then_release_at(&mut app, |app| gap_centre(app, 1));

        assert!(app.dragging.is_none(), "the drag ended");
        assert!(
            cmds.try_iter().any(|c| matches!(
                c,
                Cmd::MoveBlockBefore {
                    from: 11,
                    before: 1
                }
            )),
            "the delay was asked into the gap before the drive"
        );
    }

    /// Dropping a block onto a block in the other lane trades their places.
    #[test]
    fn dropping_a_block_onto_the_other_lane_trades_places() {
        let (mut app, cmds) = branched_app();
        app.dragging = Some(1);
        draw_then_release_at(&mut app, |app| {
            app.block_rects
                .iter()
                .find(|(slot, _)| *slot == 11)
                .map(|(_, rect)| rect.center())
                .expect("the branch block was drawn")
        });

        assert!(
            cmds.try_iter()
                .any(|c| matches!(c, Cmd::MoveBlock { from: 1, to: 11 })),
            "the two blocks were asked to trade places"
        );
    }

    /// Dragging a block down onto the offered branch moves it there - the
    /// gesture HX Edit taught everyone for "run this one in parallel".
    #[test]
    fn dropping_a_block_onto_the_offered_branch_moves_it_there() {
        use hx_proto::preset::{Kind, Layout, Path};
        let (mut app, events, cmds) = app();
        let slot = |position: i64, kind| session::Block {
            position,
            routing: None,
            kind,
            model: 0,
            enabled: true,
            values: vec![],
            paired: None,
            paired_values: vec![],
        };
        events
            .send(Evt::Loaded {
                index: 0,
                name: "Test".into(),
                firmware: String::new(),
                tempo: None,
                snapshots: vec![],
                snapshot: None,
                snapshot_details: Vec::new(),
                assignments: vec![],
                chain: vec![
                    slot(0, Kind::Input),
                    slot(1, Kind::Block),
                    slot(2, Kind::Block),
                    slot(9, Kind::Output),
                    slot(10, Kind::Split),
                    slot(19, Kind::Join),
                ],
                layout: Layout {
                    paths: vec![Path {
                        input: Some(0),
                        output: Some(9),
                        split: Some(10),
                        join: Some(19),
                        head: vec![1, 2],
                        lanes: vec![],
                        tail: vec![],
                    }],
                },
                dirty: true,
            })
            .unwrap();
        app.drain_events();

        app.dragging = Some(2);
        draw_then_release_at(&mut app, |app| {
            app.ghost_target
                .map(|(_, rect)| rect.center())
                .expect("a branch was on offer")
        });

        assert!(
            cmds.try_iter().any(|c| matches!(
                c,
                Cmd::MoveBlockBefore {
                    from: 2,
                    before: 11
                }
            )),
            "the block was asked onto the branch's first free slot"
        );
    }

    /// A release over nothing lets go without moving anything - the drop is
    /// resolved from where the pointer is, never from what it once crossed.
    #[test]
    fn releasing_over_nothing_moves_nothing() {
        let (mut app, cmds) = branched_app();
        app.dragging = Some(1);
        draw_then_release_at(&mut app, |_| egui::pos2(1200.0, 550.0));

        assert!(app.dragging.is_none(), "the drag ended");
        assert!(
            !cmds
                .try_iter()
                .any(|c| matches!(c, Cmd::MoveBlock { .. } | Cmd::MoveBlockBefore { .. })),
            "nothing moved"
        );
    }

    /// A reload that follows an edit must keep Save available: the worker says
    /// whether the buffer is dirty, and the app takes its word rather than
    /// assuming a load means a fresh preset.
    #[test]
    fn a_reload_after_an_edit_keeps_the_unsaved_changes_flag() {
        let (mut app, events, _cmds) = app();
        let loaded = |dirty| Evt::Loaded {
            index: 7,
            name: "CT-Sad".into(),
            firmware: "3.80".into(),
            tempo: None,
            snapshots: vec![],
            snapshot: None,
            snapshot_details: Vec::new(),
            layout: hx_proto::preset::Layout::default(),
            assignments: Vec::new(),
            chain: vec![],
            dirty,
        };

        events.send(loaded(true)).unwrap();
        app.drain_events();
        assert!(
            app.dirty,
            "an edit-triggered reload still has changes to save"
        );

        events.send(loaded(false)).unwrap();
        app.drain_events();
        assert!(!app.dirty, "a fresh load has nothing to save");
    }

    /// A preset as the worker presents it: three snapshots, one block.
    fn loaded(index: i64, snapshot: Option<usize>, dirty: bool) -> Evt {
        Evt::Loaded {
            index,
            name: format!("Preset {index}"),
            firmware: "3.80".into(),
            tempo: Some(120.0),
            snapshots: vec!["Verse".into(), "Chorus".into(), "Solo".into()],
            snapshot,
            snapshot_details: Vec::new(),
            layout: hx_proto::preset::Layout::default(),
            assignments: Vec::new(),
            chain: vec![session::Block {
                position: 1,
                routing: None,
                kind: hx_proto::preset::Kind::Block,
                model: 101,
                enabled: true,
                values: vec![0.5],
                paired: None,
                paired_values: vec![],
            }],
            dirty,
        }
    }

    /// The browser plays each model clicked in the block's place, once, and
    /// ends the try as asked: Put back restores the block, Keep keeps it.
    #[test]
    fn the_browser_tries_models_and_ends_the_try_as_asked() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), false)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();

        app.open_browser_swap();
        assert_eq!(app.browser_swap_slot(), Some(1));
        assert!(app.pick_model(("drive".into(), "Drive".into(), 7, None)));
        assert!(matches!(
            cmds.try_recv(),
            Ok(Cmd::TryModel {
                block: 1,
                model: 7,
                paired: None
            })
        ));
        assert!(app.dirty);
        assert!(
            !app.pick_model(("drive".into(), "Drive".into(), 7, None)),
            "what is playing already is not played again"
        );
        assert!(cmds.try_recv().is_err());
        app.close_browser(false);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::PutBack)));
        assert!(app.browser.is_none());

        app.open_browser_swap();
        app.pick_model(("fuzz".into(), "Fuzz".into(), 8, None));
        let _ = cmds.try_recv();
        app.close_browser(true);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::KeepTry)));

        // Closing with nothing tried asks the pedal nothing.
        app.open_browser_swap();
        app.close_browser(false);
        assert!(cmds.try_recv().is_err());
    }

    /// Choosing what the block held puts it back exactly, settings and all,
    /// rather than loading a fresh copy of the model over it.
    #[test]
    fn choosing_the_original_again_puts_the_block_back() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), false)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();
        app.open_browser_swap();
        if let Some(browser) = app.browser.as_mut() {
            if let browser::Target::Swap { original_id, .. } = &mut browser.target {
                *original_id = Some("drive".into());
            }
        }
        app.pick_model(("fuzz".into(), "Fuzz".into(), 8, None));
        let _ = cmds.try_recv();
        assert!(!app.pick_model(("drive".into(), "Drive".into(), 7, None)));
        assert!(matches!(cmds.try_recv(), Ok(Cmd::PutBack)));
        assert!(app.browser.as_ref().is_some_and(|b| b.playing.is_none()));
    }

    /// Anything else sent while a model is being tried keeps it, as the
    /// worker does, and the browser closes; reading the switches after a try
    /// is not something done.
    #[test]
    fn sending_anything_else_keeps_the_model_being_tried() {
        let (mut app, events, _cmds) = app();
        events.send(loaded(2, Some(0), false)).unwrap();
        app.drain_events();
        app.open_browser_swap();
        app.pick_model(("drive".into(), "Drive".into(), 7, None));
        app.send(Cmd::ReadSwitches);
        assert!(!app.trial_kept.get());
        app.send(Cmd::SavePreset);
        assert!(app.trial_kept.get());
    }

    /// A block added from the browser goes into the gap clicked, and is the
    /// one selected when the preset comes back with it.
    #[test]
    fn a_block_added_from_the_browser_is_selected_when_it_arrives() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), false)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();
        app.open_browser_insert(4);
        assert_eq!(app.browser_insert_at(), Some(4));
        assert!(app.pick_model(("delay".into(), "Delay".into(), 9, None)));
        assert!(matches!(
            cmds.try_recv(),
            Ok(Cmd::InsertBlock {
                at: 4,
                model: 9,
                paired: None
            })
        ));
        assert!(app.browser.is_none(), "adding is one click");
        let block = |position| session::Block {
            position,
            routing: None,
            kind: hx_proto::preset::Kind::Block,
            model: 101,
            enabled: true,
            values: vec![0.5],
            paired: None,
            paired_values: vec![],
        };
        events
            .send(Evt::Loaded {
                index: 2,
                name: "Preset 2".into(),
                firmware: "3.80".into(),
                tempo: None,
                snapshots: vec![],
                snapshot: None,
                snapshot_details: Vec::new(),
                layout: hx_proto::preset::Layout::default(),
                assignments: Vec::new(),
                chain: vec![block(1), block(4)],
                dirty: true,
            })
            .unwrap();
        app.drain_events();
        assert_eq!(app.chain[app.selected].position, 4);
    }

    /// The bar lights the snapshot the pedal says is active, whoever picked
    /// it. A preset that does not say starts at the first, rather than at
    /// whichever one the previous preset was left on.
    #[test]
    fn the_lit_snapshot_is_the_one_the_pedal_has_active() {
        let (mut app, events, _cmds) = app();
        events.send(loaded(2, Some(2), false)).unwrap();
        app.drain_events();
        assert_eq!(app.current_snapshot, 2);

        events.send(loaded(5, None, false)).unwrap();
        app.drain_events();
        assert_eq!(app.current_snapshot, 0);
    }

    /// A preset the pedal loaded by itself takes away whatever was half done
    /// for the old one. Each of these finishes by sending a block position,
    /// and in the new preset that position is a different block.
    #[test]
    fn a_preset_loaded_on_the_pedal_drops_what_was_half_done() {
        let (mut app, events, _cmds) = app();
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        app.param_draft = Some((1, 0, "7.5".into()));
        app.renaming_header = Some("Crunch".into());
        app.dragging = Some(1);
        app.open_browser_insert(2);

        events.send(loaded(5, Some(0), false)).unwrap();
        app.drain_events();

        assert_eq!(app.preset_index, 5);
        assert!(app.reveal_preset, "the list follows the pedal");
        assert!(app.param_draft.is_none());
        assert!(app.renaming_header.is_none());
        assert!(app.dragging.is_none());
        assert!(
            app.browser.is_none(),
            "the browser was aimed at the old preset"
        );

        // A reload of the same preset, after an edit, leaves them alone.
        app.param_draft = Some((1, 0, "7.5".into()));
        events.send(loaded(5, Some(0), true)).unwrap();
        app.drain_events();
        assert!(app.param_draft.is_some());
    }

    /// The selection moves when a preset is asked for, before the pedal
    /// answers. When the answer is another preset, because the switch was
    /// refused, the selection, the spinner and the snapshot follow it.
    #[test]
    fn a_switch_the_pedal_refused_puts_the_selection_back() {
        let (mut app, events, _cmds) = app();
        events.send(loaded(2, Some(1), true)).unwrap();
        app.drain_events();

        app.load_preset(5);
        assert_eq!(app.preset_index, 5);
        assert!(app.loading);
        // And a snapshot clicked meanwhile lights before it is answered too.
        app.current_snapshot = 2;

        events.send(loaded(2, Some(1), true)).unwrap();
        app.drain_events();

        assert_eq!(app.preset_index, 2);
        assert!(!app.loading);
        assert!(app.dirty, "the edits the pedal kept are still unsaved");
        assert_eq!(app.current_snapshot, 1);
    }

    /// Loading the preset already loaded reloads it from the pedal, which
    /// throws its changes away like loading any other, so it asks too.
    #[test]
    fn loading_the_loaded_preset_again_asks_about_unsaved_changes() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();

        app.request_preset(2);

        assert!(app.confirm_switch.is_some());
        assert!(cmds.try_recv().is_err(), "nothing goes before the answer");
    }

    /// A menu action that has to go to another preset asks before it throws
    /// unsaved changes away, as a click on the row does, and then goes as one
    /// request that runs only once the switch has worked.
    #[test]
    fn a_menu_action_on_another_preset_asks_before_discarding() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();
        app.clipboard = Some(("Crunch".into(), vec![1, 2, 3]));

        app.row_action(5, RowAction::Paste);
        assert!(app.confirm_switch.is_some());
        assert!(cmds.try_recv().is_err());

        app.answer_switch(SwitchAnswer::Discard);
        match cmds.try_recv() {
            Ok(Cmd::Switch {
                index: 5,
                save: false,
                then,
            }) => assert!(matches!(
                then.as_slice(),
                [Cmd::PastePreset(blob), Cmd::Rename { index: 5, name }]
                    if blob == &[1, 2, 3] && name == "Crunch"
            )),
            _ => panic!("the paste goes as one request with its switch"),
        }
        assert_eq!(app.preset_index, 5);
        assert!(app.loading);

        // A copy of another preset asks the same way, and Cancel sends nothing.
        events.send(loaded(5, Some(0), true)).unwrap();
        app.drain_events();
        app.row_action(2, RowAction::Copy);
        assert!(app.confirm_switch.is_some());
        app.answer_switch(SwitchAnswer::Cancel);
        assert!(app.confirm_switch.is_none());
        assert!(cmds.try_recv().is_err());
    }

    /// On the preset already loaded nothing is thrown away: a copy reads the
    /// edit buffer as it is, so it goes at once.
    #[test]
    fn a_menu_action_on_the_loaded_preset_goes_at_once() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();

        app.row_action(2, RowAction::Copy);

        assert!(app.confirm_switch.is_none());
        assert!(matches!(cmds.try_recv(), Ok(Cmd::CopyPreset)));
        assert!(matches!(app.pending_copy, CopyTarget::Clipboard));
    }

    /// "Save, then load" is one request, so a save that fails is never
    /// followed by the switch that throws the changes away.
    #[test]
    fn save_then_load_switches_only_after_the_save() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();

        app.request_preset(5);
        app.answer_switch(SwitchAnswer::Save);

        let sent: Vec<Cmd> = cmds.try_iter().collect();
        assert!(matches!(
            sent.as_slice(),
            [Cmd::Switch { index: 5, save: true, then }] if then.is_empty()
        ));
    }

    /// A previewed tone, as Load gets it.
    fn previewed(dest: i64) -> Preview {
        Preview {
            name: "From a file".into(),
            line: String::new(),
            chain: Vec::new(),
            layout: hx_proto::preset::Layout::default(),
            skipped: Vec::new(),
            load: LoadKind::Document(vec![9, 9]),
            dest,
            source: ("hxpreset".into(), vec![9, 9]),
        }
    }

    /// A previewed tone loaded into another preset asks first. Cancel puts
    /// the preview back; the load goes as one request with its switch. Into
    /// the preset already loaded it is one more edit, and goes at once.
    #[test]
    fn loading_a_tone_into_another_preset_asks_first() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();

        app.load_preview(previewed(5));
        assert!(app.confirm_switch.is_some());
        assert!(cmds.try_recv().is_err());
        app.answer_switch(SwitchAnswer::Cancel);
        assert_eq!(app.preview.as_ref().map(|p| p.dest), Some(5));

        let preview = app.preview.take().unwrap();
        app.load_preview(preview);
        app.answer_switch(SwitchAnswer::Discard);
        match cmds.try_recv() {
            Ok(Cmd::Switch {
                index: 5,
                save: false,
                then,
            }) => assert!(matches!(
                then.as_slice(),
                [Cmd::LoadDocument { dest: 5, bytes }] if bytes == &[9, 9]
            )),
            _ => panic!("the load goes as one request with its switch"),
        }

        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        app.load_preview(previewed(2));
        assert!(app.confirm_switch.is_none());
        assert!(matches!(
            cmds.try_recv(),
            Ok(Cmd::LoadDocument { dest: 2, .. })
        ));
    }

    /// Loading from a file switches nothing to look at it: the preview opens
    /// aimed at the row, and only its Load goes there.
    #[test]
    fn a_file_for_a_row_opens_aimed_at_it_without_switching() {
        let (mut app, events, cmds) = app();
        if app.catalog.is_none() {
            eprintln!("SKIPPED: HX Edit is not installed, so tones cannot be read");
            return;
        }
        events.send(loaded(2, Some(0), true)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();
        let file = std::env::temp_dir().join("tonepush-row-import-test.hxpreset");
        std::fs::write(&file, include_bytes!("../../hx-proto/tests/preset.bin")).unwrap();

        app.open_tone_file_for(&file, 5);

        assert_eq!(app.preview.as_ref().map(|p| p.dest), Some(5));
        assert_eq!(app.preset_index, 2, "nothing switched");
        assert!(cmds.try_recv().is_err());
        let _ = std::fs::remove_file(&file);
    }

    /// Copying a block asks the worker for the block itself, and Paste is
    /// offered from then on, into this preset or any other - until the pedal
    /// goes, because a block from it may not fit the next one.
    #[test]
    fn a_copied_block_is_offered_for_pasting_until_the_pedal_goes() {
        let (mut app, events, cmds) = app();
        events.send(loaded(2, Some(0), false)).unwrap();
        app.drain_events();
        let _ = cmds.try_iter().count();
        let block = app.chain[0].clone();

        app.copy_block(1, &block);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::CopyBlock(1))));

        events.send(loaded(5, Some(0), false)).unwrap();
        app.drain_events();
        app.paste_block(7);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::PasteBlock(7))));

        events.send(Evt::Disconnected).unwrap();
        app.drain_events();
        app.paste_block(7);
        assert!(cmds.try_recv().is_err());
    }

    /// A file dropped on the window, as the windowing layer hands it over.
    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    /// Dropped together, each WAV goes to the worker on its own, to take
    /// whichever slot the pedal has free by then. Picking the slot here gave
    /// them all the same one, and with the list not yet heard, slot 1.
    #[test]
    fn wavs_dropped_together_each_go_on_their_own() {
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().count();
        let files = ["/cabs/first.wav", "/cabs/second.wav"];
        let input = egui::RawInput {
            dropped_files: files
                .iter()
                .map(|file| std::sync::Arc::new(Dropped(file.into())) as egui::DroppedFileHandle)
                .collect(),
            ..Default::default()
        };

        let ctx = egui::Context::default();
        ctx.begin_pass(input);
        app.dropped_files(&ctx);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();

        let sent: Vec<std::path::PathBuf> = cmds
            .try_iter()
            .filter_map(|cmd| match cmd {
                Cmd::LoadIr(file) => Some(file),
                _ => None,
            })
            .collect();
        assert_eq!(sent, files.map(std::path::PathBuf::from));
    }

    /// One frame's logic, with or without a request to close the window,
    /// and what it asked of the window.
    fn closing_frame(
        app: &mut App,
        ctx: &egui::Context,
        close: bool,
    ) -> Vec<egui::ViewportCommand> {
        let mut input = egui::RawInput::default();
        if close {
            input.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    events: vec![egui::ViewportEvent::Close],
                    ..Default::default()
                },
            );
        }
        let output = ctx.run_ui(input, |ui| app.hold_close_while_busy(ui.ctx()));
        let asked = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.commands.clone())
            .unwrap_or_default();
        output.drop_without_applying_deltas();
        asked
    }

    /// A close asked for while the worker is talking to the pedal waits for
    /// it, and the window closes itself once the worker is done. With nothing
    /// under way it goes straight through.
    #[test]
    fn closing_waits_for_the_pedal_to_be_done() {
        let (mut app, events, _cmds) = app();
        let ctx = egui::Context::default();
        events.send(Evt::Busy(true)).unwrap();
        app.drain_events();

        let asked = closing_frame(&mut app, &ctx, true);
        assert!(asked.contains(&egui::ViewportCommand::CancelClose));
        assert!(app.closing);
        assert!(
            closing_frame(&mut app, &ctx, false).is_empty(),
            "still waiting"
        );

        events.send(Evt::Busy(false)).unwrap();
        app.drain_events();
        let asked = closing_frame(&mut app, &ctx, false);
        assert!(asked.contains(&egui::ViewportCommand::Close));
        let asked = closing_frame(&mut app, &ctx, true);
        assert!(!asked.contains(&egui::ViewportCommand::CancelClose));
    }

    #[test]
    fn discarding_reloads_the_current_preset_from_the_pedal() {
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().collect::<Vec<_>>();
        app.preset_index = 7;
        app.dirty = true;

        app.discard_changes();

        assert!(
            app.loading,
            "the title should show that the reload is under way"
        );
        assert!(matches!(cmds.try_recv(), Ok(Cmd::SelectPreset(7))));
    }

    /// The table holds a travel's ends in the parameter's own units, and the
    /// opcodes that move them take a fraction of its range. Sending the units
    /// made every end past 1.0 a refusal that ended the session.
    #[test]
    fn a_travel_end_goes_to_the_pedal_as_a_fraction_of_its_range() {
        use hx_proto::preset::Target;
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().count();

        app.move_travel(3, Target::Param(2), true, 7.5, &(0.0..=10.0));
        assert!(matches!(
            cmds.try_recv(),
            Ok(Cmd::SetAssignRange { block: 3, param: 2, value, high_end: true })
                if (value - 0.75).abs() < 1e-6
        ));

        // A pitch block holds its ends in semitones, either side of zero.
        app.move_travel(3, Target::Param(0), false, -12.0, &(-24.0..=24.0));
        assert!(matches!(
            cmds.try_recv(),
            Ok(Cmd::SetAssignRange { value, high_end: false, .. })
                if (value - 0.25).abs() < 1e-6
        ));

        // Nothing goes for a value that is not a number, or for a bypass,
        // which is a switch with no travel at all.
        app.move_travel(3, Target::Param(0), false, f32::NAN, &(0.0..=1.0));
        app.move_travel(3, Target::Bypass, false, 0.5, &(0.0..=1.0));
        assert!(cmds.try_recv().is_err());
    }

    /// A preset holds 40 to 240 BPM. Anything else, or no number at all, is
    /// answered here instead of being sent; a tempo the pedal takes is sent,
    /// and the reading waits for the pedal to say it has it.
    #[test]
    fn only_a_tempo_the_pedal_takes_is_sent() {
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().count();
        app.tempo = Some(120.0);

        for bpm in [20.0, 999.0, -1.0, f32::NAN, f32::INFINITY] {
            app.set_tempo(bpm);
        }
        assert!(cmds.try_recv().is_err(), "nothing out of range is sent");
        assert_eq!(app.status, "tempo must be between 40 and 240 BPM");

        app.set_tempo(96.0);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::SetTempo(bpm)) if bpm == 96.0));
        assert_eq!(app.tempo, Some(120.0), "the reading waits for the pedal");
    }

    #[test]
    fn a_travel_fraction_stays_between_the_ends() {
        assert_eq!(travel_fraction(&(20.0..=20_000.0), 20.0), Some(0.0));
        assert_eq!(travel_fraction(&(20.0..=20_000.0), 20_000.0), Some(1.0));
        assert_eq!(travel_fraction(&(0.0..=10.0), 12.0), Some(1.0));
        assert_eq!(travel_fraction(&(0.0..=10.0), -1.0), Some(0.0));
        assert_eq!(travel_fraction(&(5.0..=5.0), 5.0), None);
        assert_eq!(travel_fraction(&(0.0..=1.0), f32::INFINITY), None);
    }

    #[test]
    fn an_unknown_model_still_gets_a_label() {
        let (app, _events, _cmds) = app();
        assert_eq!(app.model_name(u32::MAX), format!("model {}", u32::MAX));
    }

    /// Copying keeps the device's own bytes, not a rebuild of what is on
    /// screen: a preset carries more than this editor models, and pasting a
    /// reconstruction would silently drop the rest.
    #[test]
    fn copying_a_preset_keeps_the_bytes_verbatim() {
        let (mut app, events, _cmds) = app();
        let blob = vec![0xde, 0xad, 0xbe, 0xef];

        events
            .send(Evt::Copied {
                name: "Crunch".into(),
                blob: blob.clone(),
            })
            .unwrap();
        app.drain_events();

        assert_eq!(app.clipboard, Some(("Crunch".into(), blob)));
    }

    /// An export writes the file itself rather than putting it on the
    /// clipboard, and the two use the same round trip to the device.
    #[test]
    fn exporting_writes_the_preset_to_the_chosen_file() {
        let (mut app, events, _cmds) = app();
        let file = std::env::temp_dir().join("tonepush-export-test.hxpreset");
        let _ = std::fs::remove_file(&file);

        app.pending_copy = CopyTarget::File(file.clone());
        events
            .send(Evt::Copied {
                name: "Clean".into(),
                blob: b"l6-helix".to_vec(),
            })
            .unwrap();
        app.drain_events();

        assert_eq!(std::fs::read(&file).unwrap(), b"l6-helix");
        assert!(
            app.clipboard.is_none(),
            "an export should not also occupy the clipboard"
        );
        let _ = std::fs::remove_file(&file);
    }

    /// Opening a .hlx lands in the preview - drawn, not loaded - and sends
    /// nothing to the device until Load is pressed.
    #[test]
    fn a_tone_file_opens_as_a_preview_not_a_write() {
        let (mut app, _events, cmds) = app();
        if app.catalog.is_none() {
            eprintln!("SKIPPED: HX Edit is not installed, so tones cannot be read");
            return;
        }
        let json = serde_json::json!({
            "data": { "meta": { "name": "Looked At" }, "tone": { "dsp0": {
                "block0": { "@model": "HD2_DistScream808Mono", "@enabled": true }
            }}}
        });
        let file = std::env::temp_dir().join("tonepush-preview-test.hlx");
        std::fs::write(&file, json.to_string()).unwrap();

        app.open_tone_file(&file);
        let preview = app.preview.as_ref().expect("a preview should open");
        assert_eq!(preview.name, "Looked At");
        // Drawn as a real chain: the block plus its input and output.
        assert_eq!(preview.chain.len(), 3);
        assert_eq!(preview.layout.paths.len(), 1);
        assert!(
            matches!(&preview.load, LoadKind::Steps(blocks) if blocks.len() == 1),
            "a .hlx loads by rebuilding its blocks"
        );
        assert!(
            !cmds
                .try_iter()
                .any(|c| matches!(c, Cmd::PastePreset(_) | Cmd::LoadSteps { .. })),
            "looking at a tone must not write it"
        );
        let _ = std::fs::remove_file(&file);
    }

    /// Importing a file that is not a preset must not reach the device: it
    /// would be accepted and then read back as an empty slot.
    #[test]
    fn importing_a_missing_file_reports_instead_of_sending() {
        let (mut app, _events, cmds) = app();
        app.open_tone_file(std::path::Path::new("/nonexistent/nope.hxpreset"));

        // The app connects on startup, so the queue is not empty - but nothing
        // that would write to the device may be in it.
        assert!(
            !cmds.try_iter().any(|c| matches!(c, Cmd::PastePreset(_))),
            "a file that could not be read must not reach the device"
        );
        assert!(app.log.iter().any(|l| l.contains("could not read")));
    }

    #[test]
    fn leaving_cloud_ends_a_live_audition() {
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().collect::<Vec<_>>();
        app.lib_showing = LibraryView::Cloud;
        app.auditioning = Some(456);

        app.show_library_view(LibraryView::Tones);

        assert!(matches!(cmds.try_recv(), Ok(Cmd::EndAudition)));
    }

    #[test]
    fn cloud_prefetches_the_public_popular_catalog_without_an_account() {
        let (mut app, _events, _cmds) = app();
        app.config.account = None;
        app.config.token = None;
        assert_eq!(app.cloud_order, cloud::DiscoveryOrder::Popular);
        assert!(app.cloud_search_due.is_some());
    }

    #[test]
    fn refreshing_cloud_retries_the_current_public_view() {
        let (mut app, _events, _cmds) = app();
        app.cloud_search_due = None;
        app.cloud_error = Some("offline".into());

        app.refresh_cloud();

        assert!(app.cloud_search_due.is_some());
        assert!(app.cloud_error.is_none());
    }

    #[test]
    fn a_cloud_page_finishes_in_the_background_and_survives_tab_switches() {
        let (mut app, _events, _cmds) = app();
        app.cloud_search_due = None;
        app.cloud_entries.clear();
        let tone: cloud::ToneDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        let entry = cloud::DiscoveredTone {
            song: tone.song.clone().unwrap(),
            tone,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        app.lib_showing = LibraryView::Tones;
        app.cloud_loaded_query = Some(String::new());
        app.cloud_loaded_order = Some(app.cloud_order);
        app.cloud_loaded_device = Some(app.device.clone());
        app.cloud_searching = Some(CloudSearchJob {
            query: String::new(),
            order: app.cloud_order,
            device: app.device.clone(),
            page: 1,
            answer: rx,
        });
        let ctx = egui::Context::default();

        tx.send(Ok(cloud::DiscoveryPage {
            entries: vec![entry],
            total: 51,
            next_page: Some(2),
        }))
        .unwrap();
        app.settle_cloud_search(&ctx);
        assert_eq!(app.cloud_entries.len(), 1);
        assert_eq!(app.cloud_total, Some(51));
        assert_eq!(app.cloud_next_page, Some(2));
        assert!(app.cloud_searching.is_none());

        app.show_library_view(LibraryView::Cloud);
        assert_eq!(app.cloud_entries.len(), 1);
        assert!(
            app.cloud_search_due.is_none(),
            "opening the already loaded feed must not restart page one"
        );
    }

    #[test]
    fn unmodified_arrows_load_adjacent_presets_only_without_keyboard_focus() {
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().collect::<Vec<_>>();
        app.show_onboarding = false;
        app.connection = Connection::Online;
        app.presets = vec!["One".into(), "Two".into(), "Three".into()];
        app.preset_index = 1;

        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.begin_pass(input);
        app.shortcuts(&ctx);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();

        assert_eq!(app.preset_index, 2);
        assert!(app.reveal_preset);
        assert!(matches!(cmds.try_recv(), Ok(Cmd::SelectPreset(2))));

        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::ArrowUp,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.begin_pass(input);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("a text field")));
        app.shortcuts(&ctx);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();

        assert_eq!(app.preset_index, 2);
        assert!(cmds.try_recv().is_err());
    }

    #[test]
    fn an_incompatible_tone_can_never_become_a_pedal_action() {
        let (mut app, _events, _cmds) = app();
        app.connection = Connection::Online;
        app.device = "HX Stomp".into();
        let tone: cloud::ToneDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        let mut entry = cloud::DiscoveredTone {
            song: tone.song.clone().unwrap(),
            tone,
        };
        assert!(app.cloud_audition_blocker(&entry).is_none());

        entry.tone.summary.device.name = "Helix".into();
        assert!(app.cloud_audition_blocker(&entry).is_some());
    }

    #[test]
    fn a_native_cloud_artifact_goes_to_the_temporary_audition_path() {
        let (mut app, _events, cmds) = app();
        let _ = cmds.try_iter().collect::<Vec<_>>();
        let tone: cloud::ToneDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        let entry = cloud::DiscoveredTone {
            song: tone.song.clone().unwrap(),
            tone,
        };
        let bytes = include_bytes!("../../hx-proto/tests/preset.bin").to_vec();

        app.apply_cloud_artifact(&entry, CloudAction::Audition, bytes.clone());

        assert!(matches!(
            cmds.try_recv(),
            Ok(Cmd::AuditionDocument {
                key: 456,
                name,
                bytes: sent,
            }) if name == "Numb HX" && sent == bytes
        ));
    }

    #[test]
    fn a_cloud_history_artifact_has_its_own_cache_and_audition_identity() {
        let tone: cloud::ToneDetails =
            serde_json::from_str(include_str!("../tests/fixtures/cloud/tone-details.json"))
                .unwrap();
        let entry = cloud::DiscoveredTone {
            song: tone.song.clone().unwrap(),
            tone,
        };
        let version = cloud::ToneVersion {
            number: 1,
            current: false,
            file_sha256: "b".repeat(64),
            created_at: "2026-08-15T10:00:00Z".into(),
            download: cloud::ToneDownload {
                artifact: Some("/tones/456/versions/1/artifact".into()),
                external: None,
            },
        };

        let historical = App::cloud_version_entry(&entry, &version);

        assert!(historical.tone.summary.id < 0);
        assert_ne!(historical.tone.summary.id, entry.tone.summary.id);
        assert_eq!(historical.tone.summary.version_number, Some(1));
        assert_eq!(historical.tone.file_sha256, Some("b".repeat(64)));
        assert_eq!(
            historical
                .tone
                .download
                .as_ref()
                .and_then(|download| download.artifact.as_deref()),
            Some("/tones/456/versions/1/artifact")
        );
    }

    #[test]
    fn the_library_lookup_follows_the_rows_it_serves() {
        let (mut app, _events, _cmds) = app();
        let hash = "a".repeat(64);
        let mut meta = library::Meta {
            name: "Loud Lead".into(),
            artist: "Anyone".into(),
            tags: vec!["lead".into(), "loud".into()],
            ..Default::default()
        };
        app.lib_entries = vec![LibEntry {
            hash: hash.clone(),
            series: hash.clone(),
            name: meta.name.clone(),
            line: "Amp › Delay".into(),
            added_at: String::new(),
            modified_at: String::new(),
            downloads: None,
            rating: None,
            version: 1,
            versions: 1,
            meta: meta.clone(),
            chain: Vec::new(),
            pro: false,
        }];
        app.library_lookup = LibraryLookup::default();
        app.library_lookup.setlist_tones.insert(
            hash.clone(),
            SetlistTone {
                held: true,
                meta: library::Meta::default(),
            },
        );

        app.library_lookup.reindex(&app.lib_entries);
        assert!(app.library_lookup.names.contains("loud lead"));
        assert_eq!(app.library_lookup.tags, ["lead", "loud"]);
        assert_eq!(
            app.library_lookup.setlist_tones[&hash].meta.artist,
            "Anyone"
        );
        app.library_lookup.indexed.insert(hash.clone());
        app.mirror.insert(0, hash.clone());
        assert!(matches!(app.slot_sync(0), theme::Sync::Same));
        app.mirror.insert(0, "different bytes".into());
        app.presets = vec!["LOUD LEAD".into()];
        assert!(matches!(app.slot_sync(0), theme::Sync::Differs));

        meta.artist = "Someone else".into();
        meta.tags = vec!["bright".into()];
        app.lib_entries[0].meta = meta;
        app.library_lookup.reindex(&app.lib_entries);
        assert_eq!(app.library_lookup.tags, ["bright"]);
        assert_eq!(
            app.library_lookup.setlist_tones[&hash].meta.artist,
            "Someone else"
        );
    }

    #[test]
    fn a_preset_name_becomes_a_usable_filename() {
        // A space is legal in a file name everywhere this runs, and taking it
        // out made the library show "CT-Day_CLN" for a preset the pedal calls
        // "CT-Day CLN". Only what a filesystem actually objects to is replaced.
        assert_eq!(sanitise("CT-Day CLN"), "CT-Day CLN");
        assert_eq!(sanitise("DIR:USDoubleNrm"), "DIR_USDoubleNrm");
        assert_eq!(sanitise("Brit / Clean"), "Brit _ Clean");
        assert_eq!(sanitise("  "), "preset");
        assert_eq!(sanitise("03A"), "03A");
    }
}
