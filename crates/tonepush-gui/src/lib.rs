//! A cross-platform editor for supported guitar processors. HX devices retain
//! their familiar signal-chain editor; the StompStation PRO gets a native,
//! schema-driven VoidX editor and library manager.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use egui::RichText;
use hx_catalog::{Catalog, Kind};

pub mod cloud;
mod config;
mod eq;
/// Public so the desktop entry point can bring an older library across before
/// the first window opens. Nothing else here needs to be.
pub mod library;
mod pro;
mod processor;
mod session;
mod table;
mod theme;
mod update;
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
    line: String,
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
    /// Filter for the model browser. Empty means "show the chosen category".
    search: String,
    /// A copied block: which slot it came from. The block itself stays on the
    /// device - a copy is a document operation there, so the app only has to
    /// remember what to copy from.
    copied_block: Option<usize>,
    /// A copied preset: its name, and the document verbatim. Held in the app
    /// rather than the system clipboard because it is binary, and because
    /// pasting it into a text field would only produce noise.
    clipboard: Option<(String, Vec<u8>)>,
    /// Where the bytes should go once `Cmd::CopyPreset` answers.
    pending_copy: CopyTarget,
    /// Whether the device window is open. It holds the pedal's own libraries -
    /// impulse responses and favourite blocks - and nothing else; the global EQ
    /// and the preferences have their own windows behind their own buttons,
    /// because one window holding all four was a scroll, not a panel.
    show_device: bool,
    /// Whether the global EQ panel is open.
    show_eq: bool,
    /// Whether the preferences window is open.
    show_preferences: bool,
    /// When the EQ last wrote to the device. Dragging a band across the curve
    /// is sixty changes a second and the pedal does not want sixty writes; it
    /// wants to be heard moving, so the writes are paced rather than deferred.
    eq_wrote_at: Option<std::time::Instant>,
    /// The device's global EQ switch, as last read.
    global_eq: bool,
    /// How many steps the worker can undo and redo, for enabling the buttons.
    undo_depth: usize,
    redo_depth: usize,
    /// Where a click on a `+` in the chain wants to add a block, and where on
    /// screen to put the picker.
    inserting_at: Option<usize>,
    insert_pos: Option<egui::Pos2>,
    /// When the picker opened. The click that opens it is still in the input
    /// egui reports, and egui may run several passes for one frame, so a frame
    /// counter is not enough to tell "the opening click" from "a click
    /// somewhere else" - a moment of grace is.
    insert_opened: Option<std::time::Instant>,
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
    /// Category chosen in the browser, or none to follow the current block.
    browsing: Option<u32>,
    /// Subcategory chosen under it, by name. Cleared when the category
    /// changes, because "Mono" means a different set in every category that
    /// has one.
    browsing_shelf: Option<String>,
    /// Whether the model browser occupies its right-hand dock. Collapsing it
    /// gives the selected pedal the full editor width without losing the
    /// browser's familiar home or its current filters.
    shelf_open: bool,
    /// Forget the chain panel's manually dragged height on the next frame.
    /// Set only when a different topology arrives, not for ordinary parameter
    /// reloads, so each chain initially fits while subsequent resizing sticks.
    fit_chain_on_next_frame: bool,
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
    /// Which column orders the setlist table, and which way.
    lib_setlist_sort: (usize, bool),
    /// A setlist cell being typed into: which saved revision, which column,
    /// and the text so far.
    lib_setlist_editing: Option<(std::path::PathBuf, usize, String)>,
    /// Which setlist is open, and the draft of its details while it is edited.
    lib_setlist: Option<usize>,
    lib_setlist_draft: library::Setlist,
    /// A just-captured pedal waiting for an existing or new setlist name.
    setlist_save: Option<SetlistSave>,
    /// A setlist waiting on an answer about being written to the pedal. It is
    /// 126 flash writes and it overwrites everything, so it asks first.
    confirm_push: Option<usize>,
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
    /// The row of Controlled by the pointer last landed on. A cell is only
    /// typed into on a second click, the same rule the library table follows.
    assign_selected: Option<hx_proto::preset::Target>,
    /// The end of a controller's travel being typed: which assignment, whether
    /// it is the high end, and the text so far.
    assign_editing: Option<(hx_proto::preset::Target, bool, String)>,
    /// A footswitch's name being typed, and which switch it belongs to.
    switch_draft: Option<(u8, String)>,
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
    /// Which block is waiting for a control to be picked, after Assign control
    /// was pressed. Right-clicking a control does the same thing; this is the
    /// way you find without being told.
    assigning: Option<i64>,
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
    log: Vec<String>,
    /// The activity log is a debugging aid, not something to look at while
    /// playing, so it stays out of the way until asked for.
    show_activity: bool,
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
    /// A preset the list wants to load while the edit buffer has changes that
    /// are not saved. Loading discards them, so it asks.
    confirm_switch: Option<i64>,
    /// Renaming from the header, kept apart from the list's own rename state.
    /// Sharing one made both draw a field for the same preset, and two fields
    /// fighting over the keyboard is neither of them working.
    renaming_header: Option<String>,
    /// A preset the Remove button is waiting on an answer about. Emptying a
    /// slot writes flash and there is no undo for it, so it asks first.
    confirm_clear: Option<i64>,
    /// The version check: the answer while it is still coming, then the newer
    /// release's tag once it has. Both stay `None` when there is nothing to
    /// say, which is the common case and the quiet one.
    update_check: Option<std::sync::mpsc::Receiver<String>>,
    /// What TonePush already has, by the hash of each published Tone artifact,
    /// and the answer while it is still coming. `None` means the site has not
    /// answered; `Some(empty)` is a real answer saying nothing is published.
    cloud_check: Option<std::sync::mpsc::Receiver<std::collections::BTreeSet<String>>>,
    cloud_files: Option<std::collections::BTreeSet<String>>,
    /// Each library tone's publishable-artifact hash, worked out once. Reading
    /// and hashing a large library is nothing once and too much every frame.
    portable_hashes: std::collections::HashMap<String, String>,
    update_available: Option<String>,
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
    Version,
    Artist,
    Song,
    Genre,
    Rating,
    Downloads,
    Chain,
    Added,
    Modified,
    Character,
}

impl LibColumn {
    const ALL: [LibColumn; 12] = [
        LibColumn::Sync,
        LibColumn::Name,
        LibColumn::Version,
        LibColumn::Artist,
        LibColumn::Song,
        LibColumn::Genre,
        LibColumn::Rating,
        LibColumn::Downloads,
        LibColumn::Chain,
        LibColumn::Added,
        LibColumn::Modified,
        LibColumn::Character,
    ];

    fn title(self) -> &'static str {
        match self {
            LibColumn::Sync => "",
            LibColumn::Name => "Name",
            LibColumn::Version => "Version",
            LibColumn::Artist => "Artist",
            LibColumn::Song => "Song",
            LibColumn::Genre => "Genre",
            LibColumn::Rating => "Rating",
            LibColumn::Downloads => "Downloads",
            LibColumn::Chain => "Chain",
            LibColumn::Added => "Added",
            LibColumn::Modified => "Modified",
            LibColumn::Character => "Character",
        }
    }

    /// Columns that cannot be turned off. Without a name there is nothing to
    /// read, and without the dot there is nothing to press.
    fn always(self) -> bool {
        matches!(self, LibColumn::Sync | LibColumn::Name)
    }

    /// Local and Cloud tables share one column definition. Cloud passes false
    /// because published metadata is read-only; local Tones pass true.
    fn column(self, editable: bool) -> table::Column {
        let (title, width, can_edit, fills) = match self {
            LibColumn::Sync => ("Push", 60.0, false, false),
            LibColumn::Name => ("Name", 190.0, true, false),
            LibColumn::Version => ("Ver", 58.0, false, false),
            LibColumn::Artist => ("Artist", 130.0, true, false),
            LibColumn::Song => ("Song", 150.0, true, false),
            LibColumn::Genre => ("Genre", 130.0, true, false),
            LibColumn::Rating => ("Rating", 62.0, true, false),
            LibColumn::Downloads => ("Downloads", 88.0, false, false),
            LibColumn::Chain => ("Chain", 130.0, false, true),
            LibColumn::Added => ("Added", 92.0, false, false),
            LibColumn::Modified => ("Modified", 92.0, false, false),
            LibColumn::Character => ("Character", 110.0, true, false),
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

    /// What this column shows for a row, which is also what it sorts on - so
    /// the order can never disagree with what is on screen.
    fn text(self, entry: &LibEntry) -> String {
        match self {
            LibColumn::Sync => String::new(),
            LibColumn::Name => entry.name.clone(),
            LibColumn::Version => {
                if entry.versions > 1 {
                    format!("v{} of {}", entry.version, entry.versions)
                } else {
                    "v1".to_owned()
                }
            }
            LibColumn::Artist => entry.meta.artist.clone(),
            LibColumn::Song => entry.meta.song.clone(),
            LibColumn::Genre => entry.meta.genres.join(", "),
            LibColumn::Added => display_date(&entry.added_at),
            LibColumn::Modified => display_date(&entry.modified_at),
            LibColumn::Downloads => entry
                .downloads
                .map(format_count)
                .unwrap_or_else(|| "—".to_owned()),
            LibColumn::Rating => entry
                .rating
                .map(|rating| format!("{rating:.1}"))
                .unwrap_or_else(|| "—".to_owned()),
            LibColumn::Chain => entry.line.clone(),
            LibColumn::Character => entry.meta.character.clone(),
        }
    }

    fn value_cell(self, entry: &LibEntry) -> table::Cell {
        match self {
            LibColumn::Added => table::Cell::Value {
                text: display_date(&entry.added_at),
                key: entry.added_at.clone(),
                dim: entry.added_at.is_empty(),
            },
            LibColumn::Modified => table::Cell::Value {
                text: display_date(&entry.modified_at),
                key: entry.modified_at.clone(),
                dim: entry.modified_at.is_empty(),
            },
            LibColumn::Downloads => table::Cell::Value {
                text: entry
                    .downloads
                    .map(format_count)
                    .unwrap_or_else(|| "—".to_owned()),
                key: format!("{:020}", entry.downloads.unwrap_or(0)),
                dim: entry.downloads.is_none(),
            },
            LibColumn::Rating => table::Cell::Value {
                text: entry
                    .rating
                    .map(|rating| format!("{rating:.1}"))
                    .unwrap_or_else(|| "—".to_owned()),
                key: format!("{:020.6}", entry.rating.unwrap_or(-1.0)),
                dim: entry.rating.is_none(),
            },
            LibColumn::Chain => table::Cell::Dim(self.text(entry)),
            _ => table::Cell::Text(self.text(entry)),
        }
    }

    fn cell(
        self,
        entry: &LibEntry,
        state: theme::Sync,
        cloud: theme::Sync,
        can_send: bool,
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
            _ => self.value_cell(entry),
        }
    }
}

fn short_date(timestamp: &str) -> String {
    timestamp.get(..10).unwrap_or(timestamp).to_owned()
}

fn display_date(timestamp: &str) -> String {
    if timestamp.is_empty() {
        "—".to_owned()
    } else {
        short_date(timestamp)
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

/// The tag list shared by local and public Tones. Answering through the filter
/// itself keeps the two rails from growing subtly different click behavior.
fn tag_rail(ui: &mut egui::Ui, filter: &mut Option<String>, tags: &[String]) {
    if ui.selectable_label(filter.is_none(), "All tones").clicked() {
        *filter = None;
    }
    for tag in tags {
        let on = filter.as_deref() == Some(tag.as_str());
        if ui.selectable_label(on, format!("# {tag}")).clicked() {
            *filter = Some(tag.clone());
        }
    }
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
            format!("{}", slot + 1)
        } else {
            hx_proto::rpc::slot_label(slot)
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
        theme::fonts(ctx);
        theme::register_icons(ctx);
        theme::apply(ctx);
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
            search: String::new(),
            copied_block: None,
            clipboard: None,
            pending_copy: CopyTarget::Clipboard,
            dirty: false,
            show_device: false,
            show_eq: false,
            show_preferences: false,
            eq_wrote_at: None,
            global_eq: false,
            undo_depth: 0,
            redo_depth: 0,
            inserting_at: None,
            insert_pos: None,
            insert_opened: None,
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
            browsing: None,
            browsing_shelf: None,
            shelf_open: true,
            fit_chain_on_next_frame: true,
            reveal_preset: false,
            irs: Vec::new(),
            setlists: Vec::new(),
            setlist: 0,
            config: config::Config::load(),
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
            lib_setlist_sort: (0, true),
            lib_setlist_editing: None,
            lib_setlist: None,
            lib_setlist_draft: library::Setlist::default(),
            setlist_save: None,
            confirm_push: None,
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
            lib_hidden: std::collections::BTreeSet::from([LibColumn::Character]),
            lib_editing: None,
            name_clash: None,
            sending: None,
            mirror: Default::default(),
            lib_genres_buf: String::new(),
            lib_tag_add: String::new(),
            tempo_draft: None,
            param_draft: None,
            snapshot_draft: None,
            assign_selected: None,
            assign_editing: None,
            switch_draft: None,
            signing_in: None,
            publishing: None,
            cc_drafts: Default::default(),
            switches: Vec::new(),
            assigning: None,
            assignments: Vec::new(),
            renaming: None,
            log: Vec::new(),
            show_activity: false,
            status: "Looking for a device…".into(),
            working: None,
            settings: Default::default(),
            renaming_ir: None,
            favourites: Vec::new(),
            current_snapshot: 0,
            confirm_clear: None,
            confirm_switch: None,
            renaming_header: None,
            update_check: Some(update::check()),
            cloud_check: None,
            cloud_files: None,
            portable_hashes: std::collections::HashMap::new(),
            update_available: None,
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
        let _ = self.to_device.send(cmd);
    }

    /// Send something that changes the edit buffer, and remember that it did.
    fn edit(&mut self, cmd: Cmd) {
        self.dirty = true;
        self.send(cmd);
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
                }
                Ok(Evt::Disconnected) => {
                    self.connection = Connection::Offline;
                    self.auditioning = None;
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
                    chain,
                    layout,
                    assignments,
                    dirty,
                }) => {
                    if index != self.preset_index || layout != self.layout {
                        self.fit_chain_on_next_frame = true;
                    }
                    self.layout = layout;
                    self.assignments = assignments;
                    // The document has caught up, so anything held while a
                    // number was being dragged has been answered.
                    self.cc_drafts.clear();
                    // The worker's word, not a blanket reset: most reloads are
                    // edits taking effect, and those leave changes to save.
                    self.dirty = dirty;
                    self.reveal_preset = index != self.preset_index;
                    self.loading = false;
                    self.preset_index = index;
                    self.preset_name = name;
                    self.firmware = firmware;
                    self.tempo = tempo;
                    self.snapshots = snapshots;
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
                    // The assignments came with the document; what a switch is
                    // called and what colour it lights did not.
                    self.read_switches();
                    self.browsing = None;
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
                    break;
                }
            }
        }
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

    /// The same question by position, for the places that have one and not the
    /// block itself.
    fn is_effect_at(&self, position: i64) -> bool {
        self.chain
            .iter()
            .find(|b| b.position == position)
            .is_some_and(|b| self.is_effect(b))
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events();
        self.pro.drain();
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
        for key in self.pro.take_audition_events() {
            self.auditioning = key;
        }
        // Device events wake the UI directly. This slow fallback is for the
        // other background receivers (resource extraction and cloud work), so
        // an otherwise idle editor does not rebuild the whole immediate-mode
        // interface several times per second.
        ctx.request_repaint_after(Duration::from_secs(1));

        let pro_active = self.pro.claims_ui() && self.connection != Connection::Online;
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
        self.observe_library_device();
        self.finish_extraction();
        // Both arrive from a thread and neither belongs to any one panel: one
        // is a person approving a sign-in somewhere else, the other is the site
        // taking a tone.
        self.settle_signing_in();
        self.settle_publishing(&ctx);
        self.settle_cloud_search(&ctx);
        self.settle_cloud_download(&ctx);
        if !pro_active {
            self.dropped_files(&ctx);
        }
        if pro_active {
            self.pro.top_bar(ui);
            self.pro.status_bar(ui);
            // This is the exact TonePush library surface used by HX devices:
            // local Tones, ordered Setlists, and Cloud all occupy the same
            // shared state and the same widgets.
            self.library_strip(ui);
            let sending = self.sending.as_ref().map(|sending| sending.name.clone());
            if let Some(slot) = self.pro.preset_list(
                ui,
                &self.library_lookup,
                &mut self.config,
                sending.as_deref(),
            ) {
                if slot == usize::MAX {
                    self.sending = None;
                } else {
                    self.finish_sending(slot as i64);
                }
            }
            self.pro.body(ui);
            self.pro.windows(&ctx);
            return;
        }
        self.top_bar(ui);
        self.status_bar(ui);
        // Before the preset list on purpose: an egui panel claims its edge
        // from what is left, so the library has to be laid out first to run the
        // full width of the window rather than stopping at the preset list.
        self.library_strip(ui);
        self.preset_list(ui);
        self.activity(ui);
        self.signal_chain(ui);
        // The shelf sits beside the pedal being edited rather than under it,
        // so choosing a different model is plainly a secondary action.
        self.shelf(ui);
        self.editor(ui);
        self.insert_picker(&ctx);
        self.device_window(&ctx);
        self.eq_window(&ctx);
        self.preferences_window(&ctx);
        self.preview_window(&ctx);
        // Over everything: the one step the app cannot work without.
        self.onboarding_modal(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The worker still drains its channel after the UI sender is dropped,
        // so a normal window close gets the same exact restoration as Done.
        self.end_audition();
        self.pro.disconnect();
    }
}

impl App {
    /// One row: the preset you are editing, and what you can do to it.
    ///
    /// This had grown to two rows holding two menus, a connection state and a
    /// log toggle - an inventory of the program rather than of the music. The
    /// preset actions moved to the preset list they act on, the device moved
    /// to a status bar at the bottom, and what is left is the preset itself.
    fn top_bar(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        processor::top_bar(root_ui, "top", |ui| {
            // What preset, which of its three states, and what you can
            // do to it - in that order, because that is the order the
            // question comes in. Tempo is not part of that question, so
            // it is not in the middle of it.
            ui.add_space(8.0);
            self.preset_title(ui);
            ui.add_space(12.0);
            self.snapshot_bar(ui);
            ui.add_space(12.0);
            self.preset_tools(ui);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                self.tempo_control(ui);
            });
        });
        self.confirm_clear_window(&ctx);
    }

    /// What you can do to the loaded preset, as drawn actions beside its name.
    ///
    /// Only the three that act on the preset as a whole live here. Copy, paste
    /// and remove are on the things they act on - a block's own header, and a
    /// preset's own right-click menu - because a Remove button sitting next to
    /// Save, one that writes flash and cannot be undone, is a trap.
    fn preset_tools(&mut self, ui: &mut egui::Ui) {
        if self.preset_index < 0 {
            return;
        }
        let live = matches!(self.connection, Connection::Online);
        let actions = processor::preset_tools(
            ui,
            live,
            self.undo_depth,
            self.redo_depth,
            self.dirty,
            "Save - no changes to save",
        );
        if actions.undo {
            self.send(Cmd::Undo);
        }
        if actions.redo {
            self.send(Cmd::Redo);
        }
        if actions.save {
            self.send(Cmd::SavePreset);
        }
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
        let mut decided = None;
        theme::modal("Remove preset").show(ctx, |ui| {
            ui.set_max_width(320.0);
            ui.label(format!(
                "Empty {} - “{name}” - back to a blank preset?",
                hx_proto::rpc::slot_label(index)
            ));
            ui.label(
                RichText::new("This writes the pedal's flash. Undo does not reach it.")
                    .small()
                    .color(theme::DIM),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                if ui.button("Remove").clicked() {
                    decided = Some(true);
                }
            });
        });
        match decided {
            Some(true) => {
                self.confirm_clear = None;
                self.send(Cmd::ClearPreset(index));
            }
            Some(false) => self.confirm_clear = None,
            None => {}
        }
    }

    /// The device, along the bottom, where a status bar belongs.
    fn status_bar(&mut self, root_ui: &mut egui::Ui) {
        processor::status_bar(root_ui, "status", |ui| {
            ui.add_space(8.0);
            let colour = match self.connection {
                Connection::Online => egui::Color32::from_rgb(0x4c, 0xc0, 0x60),
                _ => theme::DIM,
            };
            theme::status_dot(ui, colour);

            // The device's name is the way in to its settings: that is
            // where you would look for them.
            let name = if self.device.is_empty() {
                if self.connection == Connection::Connecting {
                    "Looking for a pedal…".to_owned()
                } else {
                    "No device".to_owned()
                }
            } else {
                self.device.clone()
            };
            // A framed button, not a bare label. This is the way in
            // to everything about the device - its impulse responses,
            // its favourite blocks, and now backing it up and putting a
            // backup back - and a word you have to guess is clickable
            // is a door with no handle.
            if processor::device_button(
                ui,
                matches!(self.connection, Connection::Online),
                RichText::new(name).strong(),
                "everything about the pedal: backup and restore, \
                             impulse responses, favourite blocks",
            )
            .clicked()
            {
                self.show_device = !self.show_device;
                if self.show_device {
                    self.send(Cmd::ReadSettings);
                    self.send(Cmd::ListFavourites);
                }
            }
            if !self.firmware.is_empty() {
                // Same size as the device name, so the two share a
                // baseline instead of the smaller one riding high.
                ui.label(RichText::new(format!("firmware {}", self.firmware)).color(theme::DIM));
            }
            // The other two things that belong to the pedal rather than
            // to a preset, each behind its own button rather than
            // stacked under the impulse responses in one long scroll.
            let live = matches!(self.connection, Connection::Online);
            if theme::icon_button(ui, theme::Icon::Sliders, live)
                .on_hover_text("Global EQ")
                .clicked()
            {
                self.show_eq = !self.show_eq;
                if self.show_eq {
                    self.send(Cmd::ReadSettings);
                }
            }
            if theme::icon_button(ui, theme::Icon::Gear, live)
                .on_hover_text("Preferences")
                .clicked()
            {
                self.show_preferences = !self.show_preferences;
                if self.show_preferences {
                    self.send(Cmd::ReadSettings);
                }
            }

            // Connecting belongs with the device it connects to, not in
            // the far corner: "no device" and the button that does
            // something about it should be within a glance of each other.
            match self.connection {
                Connection::Online => {
                    if ui
                        .small_button("Disconnect")
                        .on_hover_text("let the pedal go, so another editor can have it")
                        .clicked()
                    {
                        self.status.clear();
                        self.send(Cmd::Disconnect);
                    }
                }
                Connection::Connecting => {
                    theme::spinner(ui);
                    ui.label(
                        RichText::new("Looking for a connected pedal…")
                            .small()
                            .color(theme::DIM),
                    );
                }
                Connection::Offline => {
                    if ui
                        .small_button("Connect")
                        .on_hover_text("look for a pedal on USB")
                        .clicked()
                    {
                        self.connection = Connection::Connecting;
                        self.send(Cmd::Connect);
                        self.pro.reconnect();
                    }
                }
            }
            // A backup reads all 126 presets. That is seconds rather
            // than minutes now, but silence for seconds still reads as
            // a window that has stopped answering.
            if let Some((what, progress)) = self.working.clone() {
                ui.add(
                    egui::ProgressBar::new(progress)
                        .desired_width(120.0)
                        .text(RichText::new(what).small()),
                );
            }

            // Laid out right to left, so what is added first sits
            // furthest right: the version in the corner, where a
            // version goes.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                self.version_label(ui);
                // Only failures reach here now, and a failure earns its
                // separator. Silence is the healthy state.
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(&self.status).small().color(theme::DIM));
                }
            });
        });
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

    /// Which TonePush this is, in the corner where a version belongs.
    ///
    /// It says nothing at all until there is something to say. When a newer
    /// release exists the label picks up the accent and gains a clause, which
    /// is enough - a modal on startup would be an editor interrupting a person
    /// to talk about itself.
    fn version_label(&mut self, ui: &mut egui::Ui) {
        // The answer arrives on its own schedule, and the receiver is dropped
        // once it has spoken or hung up, so this costs nothing thereafter.
        if let Some(rx) = &self.update_check {
            match rx.try_recv() {
                Ok(tag) => {
                    self.update_available = Some(tag);
                    self.update_check = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.update_check = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        let (text, hover, url) = match &self.update_available {
            Some(tag) => (
                RichText::new(format!("TonePush {} · {tag} available", update::VERSION))
                    .small()
                    .color(theme::ACCENT),
                "A newer TonePush release is available. Click to open it.".to_owned(),
                update::RELEASES,
            ),
            None => (
                RichText::new(format!("TonePush {}", update::VERSION))
                    .small()
                    .color(theme::DIM),
                format!("TonePush {} · click for the releases page", update::VERSION),
                update::RELEASES,
            ),
        };
        if ui
            .add(egui::Label::new(text).sense(egui::Sense::click()))
            .on_hover_text(hover)
            .clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
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
    /// document - copy, export - selects the preset first, because the device
    /// hands back the loaded one; the rest are local.
    fn row_action(&mut self, index: i64, action: RowAction) {
        match action {
            RowAction::Copy => {
                self.select_for_action(index);
                self.pending_copy = CopyTarget::Clipboard;
                self.send(Cmd::CopyPreset);
            }
            RowAction::Paste => {
                if let Some((name, blob)) = self.clipboard.clone() {
                    self.select_for_action(index);
                    self.note(format!("pasting {name}"));
                    self.send(Cmd::PastePreset(blob));
                    // The name travels with the tone. A document does not carry
                    // one - the slot's label is a separate thing in flash - so
                    // pasting without this left the new tone under the old
                    // one's name. The label changes now and the chain follows
                    // when you save, which is the same bargain every other edit
                    // on this bar makes.
                    self.send(Cmd::Rename { index, name });
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
                    self.select_for_action(index);
                    self.pending_copy = CopyTarget::File(path);
                    self.send(Cmd::CopyPreset);
                }
            }
            RowAction::Import => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("HX tone", &["hxpreset", "hlx"])
                    .pick_file()
                {
                    self.select_for_action(index);
                    self.open_tone_file(&path);
                }
            }
            // The bytes are already on this machine - the automatic backup
            // holds every slot - so keeping from a row does not have to select
            // the preset first. It used to, which threw away the edit buffer to
            // save a preset the person was not even looking at.
            RowAction::Keep => match self.slot_document(index) {
                Some((name, bytes)) => self.keep_tone(&name, "hxpreset", &bytes),
                None => {
                    self.select_for_action(index);
                    self.pending_copy = CopyTarget::Library;
                    self.send(Cmd::CopyPreset);
                }
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
    /// request came from a mouse click or an arrow key.
    fn request_preset(&mut self, index: i64) {
        if self.dirty && index != self.preset_index {
            self.confirm_switch = Some(index);
        } else {
            self.load_preset(index);
        }
    }

    /// Ask before a preset switch throws away unsaved changes.
    fn confirm_switch_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.confirm_switch else {
            return;
        };
        let going_to = self
            .presets
            .get(index as usize)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| hx_proto::rpc::slot_label(index));
        let mut decided = None;
        theme::modal("Unsaved changes").show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.label(format!(
                "“{}” has changes you have not saved. Loading {going_to} discards them.",
                self.preset_name
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(0);
                }
                if ui.button("Discard and load").clicked() {
                    decided = Some(1);
                }
                if ui.button("Save, then load").clicked() {
                    decided = Some(2);
                }
            });
        });
        match decided {
            Some(0) => self.confirm_switch = None,
            Some(1) => {
                self.confirm_switch = None;
                self.load_preset(index);
            }
            Some(2) => {
                self.confirm_switch = None;
                // Save first; the worker runs them in order, so the load lands
                // on a preset that has just been written.
                self.send(Cmd::SavePreset);
                self.load_preset(index);
            }
            _ => {}
        }
    }

    /// Put the device on a preset a menu action is about to work on.
    fn select_for_action(&mut self, index: i64) {
        if self.preset_index != index {
            self.loading = true;
            self.preset_index = index;
            self.send(Cmd::SelectPreset(index));
        }
    }

    /// Back up and restore, on the preset list.
    ///
    /// These act on the **whole pedal**, which is why they are here and named
    /// this way. What you can do to one preset - rename, copy, save to a file -
    /// lives on that preset's own right-click menu, where an action cannot be
    /// mistaken for one that touches all 126.
    fn backup_actions(&mut self, ui: &mut egui::Ui) {
        let live = matches!(self.connection, Connection::Online);
        if ui
            .add_enabled(live, egui::Button::new("Back up pedal…"))
            .on_hover_text("save every preset, setting and impulse response")
            .clicked()
        {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Where to put the backup")
                .set_file_name(format!("{}.hxbundle", sanitise(&self.device)))
                .save_file()
            {
                self.note("backing up the pedal".to_owned());
                self.send(Cmd::BackUp(dir));
            }
        }
        if ui
            .add_enabled(live, egui::Button::new("Restore pedal…"))
            .on_hover_text("replace the pedal with a complete backup")
            .clicked()
        {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Choose a backup to restore")
                .pick_folder()
            {
                match hx_usb::backup::open(&dir) {
                    Ok(m) => {
                        let kept = m.presets.iter().filter(|n| !n.is_empty()).count();
                        self.note(format!("restoring {kept} presets from {}", dir.display()));
                        self.send(Cmd::RestoreAll(dir));
                    }
                    Err(e) => self.note(format!("that is not a backup: {e}")),
                }
            }
        }
    }

    /// The loaded preset, click-to-rename.
    fn preset_title(&mut self, ui: &mut egui::Ui) {
        if self.preset_index < 0 {
            return;
        }
        if self.loading {
            theme::spinner(ui);
        } else if self
            .busy_since
            .is_some_and(|since| since.elapsed() > Duration::from_millis(150))
        {
            // An edit is on its way to the pedal. Only conversations that
            // last are worth announcing - a flicker per knob tick is noise.
            theme::spinner(ui).on_hover_text("writing to the pedal…");
        }
        if let Some(name) = processor::preset_title(
            ui,
            &hx_proto::rpc::slot_label(self.preset_index),
            &self.preset_name,
            self.dirty,
            true,
            &mut self.renaming_header,
        ) {
            self.send(Cmd::Rename {
                index: self.preset_index,
                name,
            });
        }
    }

    /// Laid out right to left, in the corner it sits in: Tap goes on first so
    /// it ends up rightmost, and the reading lands to its left.
    fn tempo_control(&mut self, ui: &mut egui::Ui) {
        let Some(tempo) = self.tempo else { return };
        if let Some(bpm) =
            processor::tempo_control(ui, tempo, &mut self.tempo_draft, &mut self.taps)
        {
            self.tempo = Some(bpm);
            self.edit(Cmd::SetTempo(bpm));
        }
    }

    /// Snapshots are three saved states of the same preset. The active one is
    /// highlighted, clicking switches, and right-clicking renames - none of
    /// which was discoverable when they were plain buttons.
    fn snapshot_bar(&mut self, ui: &mut egui::Ui) {
        if self.snapshots.is_empty() {
            return;
        }
        let mut pick = None;
        let mut rename = None;

        for (i, name) in self.snapshots.iter().enumerate() {
            match &mut self.snapshot_draft {
                Some((editing, draft)) if *editing == i => {
                    let edit = ui.add(egui::TextEdit::singleline(draft).desired_width(96.0));
                    if !edit.has_focus() && !edit.lost_focus() {
                        edit.request_focus();
                    }
                    if edit.lost_focus() {
                        if ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                            rename = Some((i, draft.clone()));
                        }
                        self.snapshot_draft = None;
                    }
                }
                _ => {
                    let active = i == self.current_snapshot;
                    let text = if active {
                        RichText::new(name).color(theme::ACCENT).strong()
                    } else {
                        RichText::new(name).color(theme::DIM)
                    };
                    let button = ui.selectable_label(active, text);
                    if button.clicked() {
                        pick = Some(i);
                    }
                    if button.secondary_clicked() {
                        self.snapshot_draft = Some((i, name.clone()));
                    }
                    button.on_hover_text(
                        "snapshots are saved states of this preset\nclick to switch, right-click to rename",
                    );
                }
            }
        }

        if let Some(index) = pick {
            self.current_snapshot = index;
            self.send(Cmd::SelectSnapshot(index as i64));
        }
        if let Some((index, name)) = rename {
            self.send(Cmd::RenameSnapshot { index, name });
        }
    }

    fn preset_list(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        let mut capture = false;
        let mut cancel_send = false;
        let mut picked: Option<i64> = None;
        processor::preset_panel(root_ui, "presets", |ui| {
            ui.add_space(4.0);
            // The actions sit on the list they act on, the way HX Edit
            // puts COPY / PASTE / IMPORT / EXPORT on its preset header. A
            // menu called "Preset" at the top of the window made you go
            // looking somewhere else for something that belongs here.
            ui.horizontal(|ui| {
                // SETLIST, not PRESETS: what the pedal holds is a setlist,
                // and it is the same word the device itself uses for a bank
                // of 126. Calling the panel Presets was always a half-truth.
                ui.label(RichText::new("SETLIST").small().color(theme::DIM))
                    .on_hover_text("Use ↑/↓ to move through presets when no field is active");
                // The same drawn star as the rows use. As a small text
                // glyph it sat on its own baseline, a few pixels above the
                // word beside it.
                let (mark, colour) = if self.show_favorites_only {
                    (theme::Icon::StarOn, theme::ACCENT)
                } else {
                    (theme::Icon::Star, theme::DIM)
                };
                if theme::small_icon_button(ui, mark, Some(colour))
                    .on_hover_text("Show favourites only")
                    .clicked()
                {
                    self.show_favorites_only = !self.show_favorites_only;
                }
                // The counterpart to the per-preset computer beside it:
                // that one keeps a tone, this one keeps the whole pedal.
                // Backup and Restore used to sit here too and do not
                // belong: they act on the whole device, settings and
                // impulse responses included, so they live behind the
                // device's own button.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let live = matches!(self.connection, Connection::Online);
                    if theme::place_enabled(ui, theme::Icon::Computer, theme::Sync::Absent, live)
                        .on_hover_text("keep every preset on the pedal, in order, as a setlist")
                        .clicked()
                    {
                        capture = true;
                    }
                });
            });
            // While a tone is on its way to the pedal, the list says so and
            // says how to stop. Without this the highlighted rows would be
            // a state with no explanation and no way out but a click.
            if let Some(sending) = &self.sending {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Choose a slot for {}", sending.name))
                            .small()
                            .color(theme::ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Cancel").clicked() {
                            cancel_send = true;
                        }
                    });
                });
            }
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Show slot labels alone until the names arrive.
                    let total = if self.presets.is_empty() {
                        self.preset_count as i64
                    } else {
                        self.presets.len() as i64
                    };
                    let setlist = self.setlist;
                    let mut load = None;
                    let mut toggle = None;
                    let mut rename_start = None;
                    let mut menu_action = None;
                    // Some(Some(name)) commits a rename, Some(None) cancels.
                    let mut rename_result: Option<Option<(i64, String)>> = None;
                    let mut shown_any = false;
                    for index in 0..total {
                        let fav = self.config.is_favorite(setlist, index);
                        if self.show_favorites_only && !fav {
                            continue;
                        }
                        shown_any = true;
                        let name = self
                            .presets
                            .get(index as usize)
                            .cloned()
                            .unwrap_or_default();
                        let selected = index == self.preset_index;
                        let label = format!("{}  {}", hx_proto::rpc::slot_label(index), name);
                        ui.horizontal(|ui| {
                            // One height for the row, and every widget in it
                            // centred on that: a text star, a drawn icon and
                            // a label are three different heights, and left
                            // to themselves they sat on three baselines.
                            ui.set_min_height(20.0);
                            // Two icons that belong together sit together;
                            // the default gap made them look like controls
                            // for different things.
                            ui.spacing_mut().item_spacing.x = 2.0;
                            // The star leads the row, clear of the scrollbar
                            // that overlaps the right edge and eats the click.
                            // The same kind of thing as the keep button
                            // beside it: a drawn icon that lights under the
                            // pointer. As a text glyph it did not read as
                            // something you could press at all.
                            let (mark, colour) = if fav {
                                (theme::Icon::StarOn, theme::ACCENT)
                            } else {
                                (theme::Icon::Star, theme::DIM)
                            };
                            if theme::small_icon_button(ui, mark, Some(colour))
                                .on_hover_text(if fav { "Remove favourite" } else { "Favourite" })
                                .clicked()
                            {
                                toggle = Some(index);
                            }
                            // Keeping is a thing you do to *a* preset, so it
                            // sits on the preset - beside its own star, not
                            // in the list's title where it could only ever
                            // mean the loaded one.
                            //
                            // And it is a dot rather than a button, because
                            // the question a person actually has is "is this
                            // one saved?", which a button can only answer by
                            // disappearing. Three states in one place: not
                            // in the library, in it and identical, in it
                            // under this name and different.
                            // This row *is* the pedal, so the pedal is not
                            // one of the icons: what it shows is the two
                            // other places a tone can be.
                            let state = self.slot_sync(index);
                            let held = theme::place(ui, theme::Icon::Computer, state);
                            let held = match state {
                                theme::Sync::Absent => {
                                    held.on_hover_text("Not in your library. Keep it")
                                }
                                theme::Sync::Same => held.on_hover_text("In your library"),
                                theme::Sync::Differs => held.on_hover_text(
                                    "In your library under this name, but different. \
                                         Update it from the pedal",
                                ),
                                theme::Sync::Working => held.on_hover_text("Saving…"),
                                theme::Sync::Unknown => held,
                            };
                            if held.clicked() {
                                match state {
                                    theme::Sync::Absent => {
                                        menu_action = Some((index, RowAction::Keep))
                                    }
                                    theme::Sync::Differs => {
                                        menu_action = Some((index, RowAction::Update))
                                    }
                                    _ => {}
                                }
                            }
                            let renaming_this = self.sending.is_none()
                                && matches!(&self.renaming, Some((i, _)) if *i == index);
                            if self.sending.is_some() {
                                // Every row is a target now. An empty slot
                                // is the safe one and reads as such; an
                                // occupied one says what it would cost
                                // before it costs it, which is the whole
                                // reason for picking in the list rather
                                // than in a dialog with a slot number in it.
                                let empty = name.trim().is_empty();
                                let text = if empty {
                                    RichText::new(format!(
                                        "{}  empty",
                                        hx_proto::rpc::slot_label(index)
                                    ))
                                    .color(theme::ACCENT)
                                } else {
                                    RichText::new(&label).color(theme::DIM)
                                };
                                let target = ui.add(
                                    egui::Button::new(())
                                        .left_text(text)
                                        .frame(false)
                                        .min_size(egui::vec2(ui.available_width(), 18.0)),
                                );
                                let target = if empty {
                                    target.on_hover_text("Put it here")
                                } else {
                                    target.on_hover_text(format!("Replace {name}"))
                                };
                                if target.clicked() {
                                    picked = Some(index);
                                }
                            } else if renaming_this {
                                if let Some((_, draft)) = self.renaming.as_mut() {
                                    let edit = ui.add(
                                        egui::TextEdit::singleline(draft)
                                            .desired_width(180.0)
                                            .hint_text("preset name"),
                                    );
                                    if !edit.has_focus() && !edit.lost_focus() {
                                        edit.request_focus();
                                    }
                                    if edit.lost_focus() {
                                        rename_result = Some(
                                            ui.input(|i| i.key_pressed(egui::Key::Enter))
                                                .then(|| (index, draft.clone())),
                                        );
                                    }
                                }
                            } else {
                                let text = if selected {
                                    RichText::new(&label).color(theme::ACCENT).strong()
                                } else {
                                    RichText::new(&label)
                                };
                                // A plain, content-sized label: left-aligned
                                // as before, never centered (no forced width).
                                let row = ui.selectable_label(selected, text);
                                if row.clicked() {
                                    load = Some(index);
                                }
                                // A preset picked on the pedal itself should
                                // be in view here too, without fighting
                                // manual scrolling.
                                if selected && self.reveal_preset {
                                    row.scroll_to_me(Some(egui::Align::Center));
                                    self.reveal_preset = false;
                                }
                                // Right-click a preset to rename it in place,
                                // whether or not it is the one loaded.
                                // Everything that acts on this one preset,
                                // on the preset itself. The header's Back up
                                // and Restore act on all 126, and keeping the
                                // two apart is what stops either being
                                // mistaken for the other.
                                row.context_menu(|ui| {
                                    if ui.button("Rename").clicked() {
                                        rename_start = Some((index, name.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Copy").clicked() {
                                        menu_action = Some((index, RowAction::Copy));
                                        ui.close();
                                    }
                                    let can_paste = self.clipboard.is_some();
                                    if ui
                                        .add_enabled(can_paste, egui::Button::new("Paste"))
                                        .clicked()
                                    {
                                        menu_action = Some((index, RowAction::Paste));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button("Save to file…").clicked() {
                                        menu_action = Some((index, RowAction::Export));
                                        ui.close();
                                    }
                                    if ui.button("Load from file…").clicked() {
                                        menu_action = Some((index, RowAction::Import));
                                        ui.close();
                                    }
                                    if ui.button("Keep in library").clicked() {
                                        menu_action = Some((index, RowAction::Keep));
                                        ui.close();
                                    }
                                    ui.separator();
                                    // Last, alone, below a line: it empties
                                    // the slot in flash and undo does not
                                    // reach it. It asks before it does.
                                    if ui.button("Remove").clicked() {
                                        menu_action = Some((index, RowAction::Remove));
                                        ui.close();
                                    }
                                });
                            }
                        });
                    }
                    if self.show_favorites_only && !shown_any {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("No favorites yet. Tap a star to add one.")
                                .small()
                                .color(theme::DIM),
                        );
                    }
                    if let Some(index) = toggle {
                        self.config.toggle_favorite(setlist, index);
                    }
                    if let Some(index) = load {
                        // Selecting a preset throws away whatever is in the
                        // edit buffer. That is the device's rule, not ours,
                        // and it costs a person their unsaved work in
                        // silence - so it asks first.
                        self.request_preset(index);
                    }
                    if let Some((index, action)) = menu_action {
                        self.row_action(index, action);
                    }
                    if let Some(started) = rename_start {
                        self.renaming = Some(started);
                    }
                    if let Some(result) = rename_result {
                        self.renaming = None;
                        if let Some((index, name)) = result {
                            self.send(Cmd::Rename { index, name });
                        }
                    }
                });
        });

        if capture {
            self.send(Cmd::CaptureSetlist);
        }
        if cancel_send {
            self.sending = None;
        }
        if let Some(slot) = picked {
            self.finish_sending(slot);
        }
        // Escape gets out of picking the way it gets out of everything else.
        if self.sending.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sending = None;
        }
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
        theme::modal("That name is taken").show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(format!(
                "Your library already has a different tone called “{holder}”."
            ));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Save new version keeps both revisions under that tone. \
                         Existing setlists continue to play the exact revision they saved.",
                )
                .small()
                .color(theme::DIM),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("Save as").small().color(theme::DIM));
            let free = library::name_is_free(&clash.draft, &hash);
            ui.add(
                egui::TextEdit::singleline(&mut clash.draft)
                    .desired_width(f32::INFINITY)
                    .hint_text("a name of its own"),
            );
            if !free {
                ui.label(
                    RichText::new("that name is taken too")
                        .small()
                        .color(theme::ACCENT),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(Clash::Cancel);
                }
                if ui.button("Save new version").clicked() {
                    decided = Some(Clash::Override);
                }
                let named = !clash.draft.trim().is_empty() && free;
                if ui
                    .add_enabled(named, egui::Button::new("Save as"))
                    .clicked()
                {
                    decided = Some(Clash::SaveAs(clash.draft.trim().to_owned()));
                }
            });
        });

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

    /// Keep a captured pedal in the library as a setlist.
    ///
    /// Every preset becomes a library tone in its own right - they are the
    /// library's now, not the setlist's, and any of them can be dropped into
    /// any other slot later. The setlist records the order, which is the part
    /// that was only ever on the pedal.
    fn keep_setlist(&mut self, slots: Vec<(String, Option<Vec<u8>>)>, kind: &str) {
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

        let setlist = library::Setlist {
            name: self.setlist_draft_name(),
            slots: kept,
            ..Default::default()
        };
        self.setlist_save = Some(SetlistSave {
            draft: setlist.name.clone(),
            setlist,
            failures,
        });
        self.lib_showing = LibraryView::Setlists;
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
        theme::modal("Save captured setlist")
            .show(ctx, |ui| {
                ui.set_width(390.0);
                ui.label(
                    "Choose an existing name to save a new version, or type a new name to create a setlist.",
                );
                ui.add_space(6.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut saving.draft)
                        .desired_width(f32::INFINITY)
                        .hint_text("setlist name"),
                );
                if !names.is_empty() {
                    ui.label(RichText::new("MATCHING SETLISTS").small().color(theme::DIM));
                    egui::ScrollArea::vertical()
                        .max_height(130.0)
                        .show(ui, |ui| {
                            for name in &names {
                                let revisions = self
                                    .lib_setlists
                                    .iter()
                                    .filter(|(_, setlist)| setlist.name.eq_ignore_ascii_case(name))
                                    .count();
                                if ui
                                    .selectable_label(
                                        saving.draft.eq_ignore_ascii_case(name),
                                        format!("{name}  ·  {revisions} version{}", if revisions == 1 { "" } else { "s" }),
                                    )
                                    .clicked()
                                {
                                    saving.draft.clone_from(name);
                                }
                            }
                        });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                    let submit = field.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if (ui
                        .add_enabled(
                            !saving.draft.trim().is_empty(),
                            egui::Button::new("Save version"),
                        )
                        .clicked()
                        || submit)
                        && !saving.draft.trim().is_empty()
                    {
                        decided = Some(true);
                    }
                });
            });

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
                (String::new(), String::new())
            };
            facts.insert(hash.clone(), fact.clone());
            setlist_tones.insert(
                hash.clone(),
                SetlistTone {
                    held,
                    meta: metadata.get(&hash).cloned().unwrap_or_default(),
                    line: fact.1,
                },
            );
        }

        let mut entries = Vec::with_capacity(disk_entries.len());
        for entry in disk_entries {
            let (derived, line) = facts
                .get(&entry.hash)
                .cloned()
                .unwrap_or_else(|| self.library_facts(&entry.hash));
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
    fn library_facts(&self, hash: &str) -> (String, String) {
        let fallback = library::short(hash).to_owned();
        let Some(bytes) = library::read(hash) else {
            return (fallback, String::new());
        };
        if library::kind(hash).as_deref() == Some("vxpreset") {
            return (fallback, pro::preset_content(&bytes).unwrap_or_default());
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return (fallback, String::new());
        };
        let tone = if library::kind(hash).as_deref() == Some("hlx") {
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .map(|json| hx_catalog::inspect(&json, catalog))
        } else {
            hx_proto::preset::Preset::parse(&bytes).map(|p| {
                hx_catalog::inspect(
                    &hx_catalog::to_hlx(&p, catalog, &fallback).document,
                    catalog,
                )
            })
        };
        match tone {
            Some(t) => (t.name.clone(), Self::tone_content(&t).to_owned()),
            None => (fallback, String::new()),
        }
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

    fn cloud_tags_rail(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("SORT").small().color(theme::DIM));
        let before = self.cloud_order;
        egui::ComboBox::from_id_salt("cloud-order")
            .width(ui.available_width())
            .selected_text(self.cloud_order.label())
            .show_ui(ui, |ui| {
                for order in cloud::DiscoveryOrder::ALL {
                    ui.selectable_value(&mut self.cloud_order, order, order.label());
                }
            });
        if self.cloud_order != before {
            self.cloud_sort = (
                match self.cloud_order {
                    cloud::DiscoveryOrder::Popular => LibColumn::Downloads,
                    cloud::DiscoveryOrder::Newest => LibColumn::Added,
                    cloud::DiscoveryOrder::Updated => LibColumn::Modified,
                    cloud::DiscoveryOrder::Rating => LibColumn::Rating,
                },
                false,
            );
            self.refresh_cloud();
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new("TAGS").small().color(theme::DIM));
        tag_rail(ui, &mut self.cloud_tag_filter, &self.cloud_tags);
    }

    fn cloud_table(&mut self, ui: &mut egui::Ui) {
        if let Some(why) = &self.cloud_error {
            ui.label(RichText::new(why).color(theme::ACCENT));
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
                        _ => column.value_cell(local),
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

    fn cloud_inspector(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let Some(entry) = self
            .cloud_selected
            .and_then(|selected| self.cloud_entries.get(selected))
            .map(|entry| {
                (
                    entry.discovered.clone(),
                    entry.row.meta.clone(),
                    entry.row.line.clone(),
                )
            })
        else {
            ui.label(
                RichText::new("Select a tone to see its published details.").color(theme::DIM),
            );
            return;
        };
        let (entry, meta, line) = entry;
        let downloads = format_count(entry.tone.summary.installs_count);
        let rating = entry
            .tone
            .summary
            .rating
            .map(|rating| format!("{rating:.1} / 5"))
            .unwrap_or_else(|| "—".to_owned());
        let added = display_date(&entry.tone.summary.created_at);
        let modified = display_date(&entry.tone.summary.updated_at);
        let version = format!(
            "v{} of {}",
            entry.tone.summary.version_number.unwrap_or(1).max(1),
            entry.tone.summary.versions_count.max(1)
        );
        ui.heading(&entry.tone.summary.name);
        if !line.is_empty() {
            ui.label(RichText::new(line).small().color(theme::DIM));
        }
        ui.add_space(6.0);
        egui::Grid::new("cloud-fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for (label, value) in [
                    ("Song artist", meta.artist.as_str()),
                    ("Song title", meta.song.as_str()),
                    ("Tone part", meta.part.as_str()),
                    ("Tone character", meta.character.as_str()),
                    ("Song genres", &meta.genres.join(", ")),
                    ("Tone guitar", meta.guitar.as_str()),
                    ("Tone pickups", meta.pickup_type.as_str()),
                    ("Tone electronics", meta.pickup_electronics.as_str()),
                    ("Tone tuning", meta.tuning.as_str()),
                    ("Device", entry.tone.summary.device.name.as_str()),
                    (
                        "Creator",
                        entry.tone.summary.creator.as_deref().unwrap_or(""),
                    ),
                    ("Version", version.as_str()),
                    ("Downloads", downloads.as_str()),
                    ("Rating", rating.as_str()),
                    ("Added", added.as_str()),
                    ("Modified", modified.as_str()),
                ] {
                    ui.label(label);
                    ui.label(if value.is_empty() { "—" } else { value });
                    ui.end_row();
                }
            });
        if entry.tone.versions.len() > 1 {
            let mut chosen_version = None;
            ui.add_space(8.0);
            ui.collapsing("Version history", |ui| {
                for historical in entry.tone.versions.iter().rev() {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "v{}  {}",
                            historical.number,
                            display_date(&historical.created_at)
                        ));
                        if historical.current {
                            ui.label(RichText::new("current").small().color(theme::DIM));
                        } else {
                            if ui.small_button("Audition").clicked() {
                                chosen_version = Some((historical.clone(), CloudAction::Audition));
                            }
                            if ui.small_button("Keep").clicked() {
                                chosen_version = Some((historical.clone(), CloudAction::Computer));
                            }
                        }
                    });
                }
            });
            if let Some((version, action)) = chosen_version {
                let historical = Self::cloud_version_entry(&entry, &version);
                self.start_cloud_entry_action(historical, action, ui.ctx());
            }
        }
        if !meta.description.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("Song description").small().color(theme::DIM));
            ui.label(meta.description);
        }
        if !meta.tone_description.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("Tone description").small().color(theme::DIM));
            ui.label(meta.tone_description);
        }
        if !meta.tags.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("Song tags").small().color(theme::DIM));
            ui.horizontal_wrapped(|ui| {
                for tag in &meta.tags {
                    ui.label(format!("# {tag}"));
                }
            });
        }
        if self.auditioning == Some(entry.tone.summary.id) {
            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Done auditioning").clicked() {
                    self.end_audition();
                }
                if ui
                    .button("Keep on pedal")
                    .on_hover_text("leave it in the edit buffer; Save writes it")
                    .clicked()
                {
                    self.keep_audition();
                }
            });
        }
    }

    /// The library's one-row heading: identity and views on the left, public
    /// search fixed to the true centre, actions on the right. These are three
    /// child UIs over one row so a growing window adds space around search
    /// instead of dragging it away from the centre.
    fn library_header(&mut self, ui: &mut egui::Ui, capture: &mut bool) {
        let available = ui.available_rect_before_wrap();
        let row = egui::Rect::from_min_size(
            available.min,
            egui::vec2(available.width(), ui.spacing().interact_size.y),
        );
        let search_width = 260.0f32.min((row.width() - 420.0).max(140.0));
        let search_rect =
            egui::Rect::from_center_size(row.center(), egui::vec2(search_width, row.height()));
        let left_rect = row.with_max_x(search_rect.min.x - 6.0);
        let right_rect = row.with_min_x(search_rect.max.x + 6.0);

        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("library-header-left")
                .max_rect(left_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.label(RichText::new("LIBRARY").small().color(theme::DIM));
                ui.add_space(6.0);
                // The selector, rather than tabs on the window: this is one
                // library with local and public views of the same Tones.
                let cloud_count = self.cloud_total.unwrap_or(self.cloud_entries.len());
                for (view, label, count) in [
                    (LibraryView::Tones, "Tones", self.scoped_tone_count()),
                    (
                        LibraryView::Setlists,
                        "Setlists",
                        self.scoped_setlist_count(),
                    ),
                    (LibraryView::Cloud, "Cloud", cloud_count),
                ] {
                    let on = self.lib_showing == view;
                    if theme::shelf_pill(ui, &format!("{label} {count}"), on).clicked() {
                        self.show_library_view(view);
                    }
                }
            },
        );

        let showing = self.lib_showing;
        let search = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .id_salt("library-header-search")
                    .max_rect(search_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    let (query, hint) = match showing {
                        LibraryView::Tones => (&mut self.tone_search, "Search this library…"),
                        LibraryView::Setlists => {
                            (&mut self.setlist_search, "Search these setlists…")
                        }
                        LibraryView::Cloud => (&mut self.library_search, "Search TonePush…"),
                    };
                    ui.add_sized(
                        search_rect.size(),
                        egui::TextEdit::singleline(query).hint_text(hint),
                    )
                },
            )
            .inner;
        if search.changed() && showing == LibraryView::Cloud {
            self.cloud_search_due =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(350));
        }
        if showing == LibraryView::Cloud
            && search.has_focus()
            && ui.input(|input| input.key_pressed(egui::Key::Enter))
        {
            self.cloud_search_due = Some(std::time::Instant::now());
        }

        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("library-header-right")
                .max_rect(right_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                let live = self.pedal_online();
                match self.lib_showing {
                    // Keeping a preset lives on the preset list now, beside
                    // the star: it is a thing you do to a preset, not a thing
                    // the library does. What remains belongs to the table.
                    LibraryView::Tones => {
                        self.column_menu(ui);
                        self.account_control(ui);
                    }
                    LibraryView::Setlists => {
                        if ui
                            .add_enabled(live, egui::Button::new("Capture the pedal"))
                            .on_hover_text("keep every preset on the pedal, in order, as a setlist")
                            .clicked()
                        {
                            *capture = true;
                        }
                    }
                    LibraryView::Cloud => {
                        self.column_menu(ui);
                        if ui
                            .button("Refresh")
                            .on_hover_text("fetch this search and order from TonePush again")
                            .clicked()
                        {
                            self.refresh_cloud();
                        }
                        if self.auditioning.is_some() && ui.button("Done auditioning").clicked() {
                            self.end_audition();
                        }
                        if self.cloud_searching.is_some() || self.cloud_download.is_some() {
                            theme::spinner(ui);
                        }
                    }
                }
                let selected = self
                    .library_device_filter
                    .as_deref()
                    .unwrap_or("All pedals")
                    .to_owned();
                let connected = self.library_connected_device.clone();
                let mut changed = false;
                egui::ComboBox::from_id_salt("library-device-scope")
                    .selected_text(selected)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.library_device_filter.is_none(), "All pedals")
                            .clicked()
                        {
                            self.library_device_filter = None;
                            changed = true;
                        }
                        if !connected.is_empty()
                            && ui
                                .selectable_label(
                                    self.library_device_filter.as_deref()
                                        == Some(connected.as_str()),
                                    &connected,
                                )
                                .clicked()
                        {
                            self.library_device_filter = Some(connected.clone());
                            changed = true;
                        }
                    });
                if changed {
                    self.lib_selected = None;
                    self.lib_setlist = None;
                    self.cloud_loaded_device = None;
                    self.refresh_cloud();
                }
            },
        );
        ui.allocate_rect(row, egui::Sense::hover());
    }

    /// The computer's library, along the bottom of the window.
    ///
    /// Always open and the full width of the window, under the preset list as
    /// well as under the chain - because the pedal is the top half and this is
    /// the other half, not a drawer that belongs to the editor. Drag its edge
    /// to give it more or less room; it does not close, the same way the pedal
    /// does not close.
    fn library_strip(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        let mut capture = false;
        egui::Panel::bottom("library")
            .resizable(true)
            .default_size(260.0)
            .size_range(96.0..=680.0)
            .show(root_ui, |ui| {
                ui.add_space(4.0);
                self.library_header(ui, &mut capture);
                ui.separator();
                match self.lib_showing {
                    // Local and public Tones are the same screen shape: a
                    // browse rail, rows, and an inspector. Keep that geometry
                    // in one place; only the contents know who owns the data.
                    view @ (LibraryView::Tones | LibraryView::Cloud) => {
                        let cloud = view == LibraryView::Cloud;
                        egui::Panel::left("tone-tags")
                            .resizable(false)
                            .default_size(150.0)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .id_salt("tone-tags-scroll")
                                    .show(ui, |ui| {
                                        if cloud {
                                            self.cloud_tags_rail(ui);
                                        } else {
                                            self.library_tags_rail(ui);
                                        }
                                    });
                            });
                        egui::Panel::right("tone-inspector")
                            .resizable(true)
                            .default_size(310.0)
                            .show(ui, |ui| {
                                // Scrolled, not grown. Without this the
                                // inspector's field stack decided the panel's
                                // height, which is how the library ended up
                                // taller than it was dragged to and sitting on
                                // top of the status bar.
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .id_salt("tone-inspector-scroll")
                                    .show(ui, |ui| {
                                        if cloud {
                                            self.cloud_inspector(ui);
                                        } else {
                                            self.library_inspector(ui);
                                        }
                                    });
                            });
                        egui::CentralPanel::default().show(ui, |ui| {
                            if cloud {
                                self.cloud_table(ui);
                            } else {
                                self.library_table(ui);
                            }
                        });
                    }
                    // Setlists on the left, what is in the chosen one on the
                    // right, drawn by the same table the tones use. A setlist
                    // is a list of tones, so it should look like one.
                    LibraryView::Setlists => {
                        egui::Panel::left("lib-setlists")
                            .resizable(true)
                            // A floor taken from the table it holds, not
                            // guessed. Without one the panel could be dragged
                            // - or restored from a remembered width - down to
                            // about a hundred pixels, where the headers sat on
                            // top of each other and "Put this setlist on the
                            // pedal" wrapped a word to a line.
                            .min_size(table::width_wanted(&setlist_rail_columns()))
                            // Clamp remembered pre-split widths too. An old
                            // full-width rail otherwise leaves the new slots
                            // table with literally no central panel.
                            .max_size(560.0)
                            .default_size(340.0)
                            // Not wrapped in a scroll area: the table does its
                            // own scrolling, and a virtualised table inside a
                            // scroll is two scrollbars fighting over one wheel.
                            .show(ui, |ui| self.setlist_rail(ui));
                        egui::CentralPanel::default().show(ui, |ui| self.setlist_slots(ui));
                    }
                }
            });
        if capture {
            self.note("reading the whole setlist off the pedal".to_owned());
            if self.pro_active() {
                self.pro.capture_setlist();
            } else {
                self.send(Cmd::CaptureSetlist);
            }
        }
        self.confirm_push_window(&ctx);
        self.confirm_delete_window(&ctx);
        self.name_clash_window(&ctx);
        self.save_setlist_window(&ctx);
        self.confirm_switch_window(&ctx);
    }

    /// Every setlist in the library, down the left, as the table everything
    /// else uses.
    ///
    /// A setlist is a thing with a name, a place, a date and a size, which is a
    /// row. It was a hand-rolled stack of two-line cards, which meant a second
    /// way of listing things to learn, no sorting, and no way to correct a
    /// venue without going to a panel below. The table brings all three along.
    fn setlist_rail(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let needle = self.setlist_search.trim().to_ascii_lowercase();
        let rows = self
            .lib_setlists
            .iter()
            .enumerate()
            .filter(|(_, (_, setlist))| self.setlist_in_library_scope(setlist))
            .filter(|(_, (_, setlist))| {
                needle.is_empty()
                    || setlist.name.to_ascii_lowercase().contains(&needle)
                    || setlist.venue.to_ascii_lowercase().contains(&needle)
                    || setlist.date.to_ascii_lowercase().contains(&needle)
                    || setlist.description.to_ascii_lowercase().contains(&needle)
                    || setlist
                        .slots
                        .iter()
                        .any(|slot| slot.name.to_ascii_lowercase().contains(&needle))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut grid = table::Grid {
            columns: setlist_rail_columns(),
            sticky: 1,
            sort: self.lib_setlist_sort,
            menu: vec!["Remove this setlist".to_owned()],
            nothing_yet: "No setlists yet. Press CAPTURE above the preset list \
                          to keep the pedal as one.",
            ..Default::default()
        };
        for &index in &rows {
            let setlist = &self.lib_setlists[index].1;
            grid.rows.push(vec![
                table::Cell::Text(setlist.name.clone()),
                table::Cell::Dim(format!("v{}", setlist.revision())),
                table::Cell::Text(setlist.venue.clone()),
                table::Cell::Text(setlist.date.clone()),
                table::Cell::Dim(setlist.filled().to_string()),
            ]);
        }

        grid.selected = self
            .lib_setlist
            .and_then(|selected| rows.iter().position(|&row| row == selected));
        if let Some((path, column, draft)) = self.lib_setlist_editing.clone() {
            grid.editing = self
                .lib_setlists
                .iter()
                .position(|(known, _)| *known == path)
                .and_then(|selected| rows.iter().position(|&row| row == selected))
                .map(|row| (row, column));
            grid.draft = draft;
        }
        // Sorted on the very strings the table shows, so the order on screen
        // and the order underneath can never disagree.
        let order = grid
            .sort_rows()
            .into_iter()
            .map(|row| rows[row])
            .collect::<Vec<_>>();

        // Its own height, so the details below it keep theirs. A long library
        // takes a little over half the panel and scrolls inside that.
        let wanted = table::ROW_HEIGHT * (grid.rows.len() + 1) as f32 + 6.0;
        let height = wanted.min((ui.available_height() * 0.55).max(table::ROW_HEIGHT * 4.0));
        let did = ui
            .allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                table::show(ui, "setlists", &mut grid)
            })
            .inner;

        if let Some(col) = did.sort {
            self.lib_setlist_sort = if col == self.lib_setlist_sort.0 {
                (col, !self.lib_setlist_sort.1)
            } else {
                (col, true)
            };
        }
        if let Some((row, ..)) = did.clicked {
            if let Some(&i) = order.get(row) {
                self.select_setlist_entry(i);
            }
        }
        self.setlist_draft_edit(&grid, &did, &order);
        if let Some((row, _)) = did.chose {
            if let Some((path, _)) = order
                .get(row)
                .and_then(|&i| self.lib_setlists.get(i))
                .cloned()
            {
                match library::remove_setlist(&path) {
                    Ok(()) => {
                        self.lib_setlist = None;
                        self.refresh_library();
                    }
                    Err(why) => self.note(why),
                }
            }
        }

        // The chosen setlist's notes and the two things you do to it, under the
        // list rather than in a panel of their own: they are read far less
        // often than the presets on the right.
        if self.lib_setlist.is_some() {
            ui.add_space(8.0);
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt("lib-setlist-details")
                .show(ui, |ui| self.setlist_details(ui));
        }
    }

    /// Begin, carry on, or finish typing in a setlist's cell.
    fn setlist_draft_edit(&mut self, grid: &table::Grid, did: &table::Did, order: &[usize]) {
        if let Some((row, col)) = did.edit {
            if let Some((path, setlist)) = order.get(row).and_then(|&i| self.lib_setlists.get(i)) {
                let was = match col {
                    2 => setlist.venue.clone(),
                    3 => setlist.date.clone(),
                    _ => setlist.name.clone(),
                };
                self.lib_setlist_editing = Some((path.clone(), col, was));
            }
            return;
        }
        // The draft lives in the app, not the table, so it survives the frame.
        if let Some((_, _, draft)) = self.lib_setlist_editing.as_mut() {
            draft.clone_from(&grid.draft);
        }
        if did.cancelled {
            self.lib_setlist_editing = None;
        }
        if did.committed {
            if let Some((path, column, draft)) = self.lib_setlist_editing.take() {
                self.commit_setlist_cell(&path, column, draft.trim());
            }
        }
    }

    /// Write what was typed into a cell back to the setlist it belongs to.
    fn commit_setlist_cell(&mut self, path: &std::path::Path, column: usize, typed: &str) {
        let Some(i) = self
            .lib_setlists
            .iter()
            .position(|(known, _)| known == path)
        else {
            return;
        };
        // Through the draft, so a rename takes the same path a rename from the
        // details below does: write the new file, then forget the old one.
        self.select_setlist_entry(i);
        match column {
            2 => self.lib_setlist_draft.venue = typed.to_owned(),
            3 => self.lib_setlist_draft.date = typed.to_owned(),
            // The revision is immutable and is never a text field.
            1 => return,
            // A setlist with no name has no file to live in.
            _ if typed.is_empty() => return,
            _ => self.lib_setlist_draft.name = typed.to_owned(),
        }
        self.save_setlist_draft();
    }

    /// What is in the chosen setlist, drawn by the table the tones use.
    ///
    /// The same columns, because these are the same kind of thing: a setlist is
    /// a list of tones in an order, and the order is the one extra column. A
    /// second way of listing tones would be a second thing to learn for no
    /// reason.
    fn setlist_slots(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.lib_setlist else {
            ui.add_space(8.0);
            ui.label(RichText::new("Choose a setlist to see what is in it.").color(theme::DIM));
            return;
        };
        let Some((_, setlist)) = self.lib_setlists.get(i).cloned() else {
            return;
        };
        let live = self.pedal_online() && self.setlist_compatible(&setlist);

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&setlist.name).strong());
            ui.label(
                RichText::new(format!("{} presets", setlist.filled()))
                    .small()
                    .color(theme::DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(live, egui::Button::new("Put this setlist on the pedal"))
                    .on_hover_text("write every preset, in order, over what is on the pedal now")
                    .clicked()
                {
                    self.confirm_push = Some(i);
                }
            });
        });
        ui.add_space(2.0);

        // Only the slots that hold something. 126 rows of "empty" is not a
        // setlist, it is a form.
        let played: Vec<(usize, library::Slot)> = setlist
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| !slot.is_empty())
            .map(|(n, slot)| (n, slot.clone()))
            .collect();

        let mut grid = table::Grid {
            columns: vec![
                table::Column::new("Push", 46.0),
                table::Column::new("Slot", 54.0),
                table::Column::new("Name", 190.0),
                table::Column::new("Artist", 130.0),
                table::Column::new("Chain", 130.0).fills(),
            ],
            sticky: 3,
            menu: vec!["Send this preset to its slot".to_owned()],
            nothing_yet: "Nothing in this setlist.",
            ..Default::default()
        };
        for (slot, entry) in &played {
            let tone = self.library_lookup.setlist_tones.get(&entry.hash);
            let held = tone.is_some_and(|tone| tone.held);
            grid.rows.push(vec![
                table::Cell::Places(vec![(
                    theme::Icon::Pedal,
                    if held && live {
                        theme::Sync::Absent
                    } else {
                        theme::Sync::Unknown
                    },
                    if !held {
                        "Missing from the library"
                    } else if !live {
                        "Connect a compatible pedal to send this preset"
                    } else {
                        "Send this preset to its slot"
                    },
                    held && live,
                )]),
                table::Cell::Dim(self.active_slot_label(*slot as i64)),
                table::Cell::Text(entry.name.clone()),
                table::Cell::Text(
                    tone.map(|tone| tone.meta.artist.clone())
                        .unwrap_or_default(),
                ),
                table::Cell::Dim(tone.map(|tone| tone.line.clone()).unwrap_or_default()),
            ]);
            grid.chosen.push(false);
        }

        let did = table::show(ui, "setlist-slots", &mut grid);
        // A row is one preset out of the setlist, and the one repair anybody
        // needs is putting it back where it came from.
        let send = setlist_send_row(&did);
        if let Some(row) = send.filter(|_| live) {
            if let Some((slot, entry)) = played.get(row).cloned() {
                self.send_one_slot(slot as i64, &entry);
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

    /// The chosen setlist's details, and the button that puts it on the pedal.
    fn setlist_details(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let Some(i) = self.lib_setlist else {
            ui.label(RichText::new("Select a setlist to edit its details.").color(theme::DIM));
            return;
        };
        if let Some((_, setlist)) = self.lib_setlists.get(i) {
            ui.label(
                RichText::new(format!(
                    "Version {} · captured {}",
                    setlist.revision(),
                    display_date(&setlist.modified_at)
                ))
                .small()
                .color(theme::DIM),
            );
            ui.add_space(4.0);
        }
        // Name, venue and date are columns in the table above and are typed
        // into there, the same as a tone's. What is left is the one field no
        // column could hold.
        let mut changed = false;
        ui.label(RichText::new("Notes").small().color(theme::DIM));
        changed |= ui
            .add(
                egui::TextEdit::multiline(&mut self.lib_setlist_draft.description)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3),
            )
            .changed();

        if changed {
            self.save_setlist_draft();
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        let compatible = self
            .lib_setlists
            .get(i)
            .is_some_and(|(_, setlist)| self.setlist_compatible(setlist));
        let live = self.pedal_online() && compatible;
        let filled = self.lib_setlists.get(i).map_or(0, |(_, s)| s.filled());
        if ui
            .add_enabled(live, egui::Button::new("Put this setlist on the pedal"))
            .on_hover_text(format!(
                "write all {filled} presets, in order, over what is on the pedal now"
            ))
            .clicked()
        {
            self.confirm_push = Some(i);
        }
        ui.add_space(4.0);
        if ui
            .button("Remove this version")
            .on_hover_text("this setlist revision goes; its tones and other revisions stay")
            .clicked()
        {
            if let Some((path, _)) = self.lib_setlists.get(i).cloned() {
                match library::remove_setlist(&path) {
                    Ok(()) => {
                        self.lib_setlist = None;
                        self.refresh_library();
                    }
                    Err(why) => self.note(why),
                }
            }
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
        theme::modal("Put the setlist on the pedal").show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.label(format!(
                "Write “{}” - {} presets - over everything on the pedal?",
                setlist.name,
                setlist.filled()
            ));
            ui.label(
                RichText::new(
                    "Every slot is a flash write and undo does not reach them. \
                         Capture the pedal first if you want what is on it now.",
                )
                .small()
                .color(theme::DIM),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                if ui.button("Write it").clicked() {
                    decided = Some(true);
                }
            });
        });
        match decided {
            Some(true) => {
                self.confirm_push = None;
                self.push_setlist(&setlist);
            }
            Some(false) => self.confirm_push = None,
            None => {}
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
            self.send(Cmd::PushSetlist(slots));
        }
    }

    /// The left rail: every tag, click one to filter the table to it.
    fn library_tags_rail(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("TAGS").small().color(theme::DIM));
        let tags = if self.library_device_filter.is_none() {
            self.library_lookup.tags.clone()
        } else {
            let mut tags = self
                .lib_entries
                .iter()
                .filter(|entry| self.tone_in_library_scope(&entry.hash))
                .flat_map(|entry| entry.meta.tags.iter().cloned())
                .collect::<Vec<_>>();
            tags.sort();
            tags.dedup();
            tags
        };
        tag_rail(ui, &mut self.lib_tag_filter, &tags);
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
            menu: vec![if self.lib_chosen.len() > 1 {
                format!("Delete {} tones", self.lib_chosen.len())
            } else {
                "Delete".to_owned()
            }],
            nothing_yet: "No tones yet. Press the computer icon beside a preset to keep it here.",
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
            let entry = &self.lib_entries[i];
            grid.rows.push(
                shown
                    .iter()
                    .map(|c| c.cell(entry, state, cloud, can_send))
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

    /// Who this computer publishes as, and how it gets to be anybody.
    ///
    /// Sitting in the library's own header because that is where publishing
    /// happens: signing in is not a preference, it is the thing that makes the
    /// cloud icon do something.
    fn account_control(&mut self, ui: &mut egui::Ui) {
        if let Some(signing) = &self.signing_in {
            ui.label(
                RichText::new(format!("code {}", signing.code))
                    .small()
                    .color(theme::ACCENT),
            )
            .on_hover_text(format!(
                "approve it at {}\nthen this signs itself in",
                signing.url
            ));
            theme::spinner(ui);
            if ui.small_button("Cancel").clicked() {
                self.signing_in = None;
            }
            return;
        }

        match self.config.account.clone() {
            Some(account) => {
                ui.menu_button(RichText::new(account).small(), |ui| {
                    if ui.button("Sign out").clicked() {
                        self.config.sign_out();
                        ui.close();
                    }
                    if ui.button("Open TonePush").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(cloud::site()));
                        ui.close();
                    }
                });
            }
            None => {
                if ui
                    .small_button("Sign in")
                    .on_hover_text("to publish Songs and Tones")
                    .clicked()
                {
                    self.start_signing_in(ui.ctx());
                }
            }
        }
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

    /// The menu that turns columns on and off.
    fn column_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(RichText::new("COLUMNS").small(), |ui| {
            for column in LibColumn::ALL {
                if column.always()
                    || (self.lib_showing != LibraryView::Cloud && column == LibColumn::Downloads)
                {
                    continue;
                }
                let mut on = !self.lib_hidden.contains(&column);
                if ui.checkbox(&mut on, column.title()).changed() {
                    if on {
                        self.lib_hidden.remove(&column);
                    } else {
                        self.lib_hidden.insert(column);
                    }
                }
            }
        });
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
        theme::modal("Delete tones").show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.label(if chosen.len() == 1 {
                "Delete this tone from the library?".to_owned()
            } else {
                format!("Delete {} tones from the library?", chosen.len())
            });
            if !affected.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "{} played by the {} {}, and will keep playing there.",
                        if chosen.len() == 1 {
                            "This tone is"
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
                    ))
                    .color(theme::DIM),
                );
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Nothing else playing them goes to the library's trash, \
                         not out of existence.",
                )
                .small()
                .color(theme::DIM),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                if ui.button("Delete").clicked() {
                    decided = Some(true);
                }
            });
        });
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

    /// The right inspector: the selected local Tone and the Song it realizes.
    /// The labels keep musical-idea facts distinct from device-preset facts.
    fn library_inspector(&mut self, ui: &mut egui::Ui) {
        fn combo(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[&str]) {
            egui::ComboBox::from_id_salt(id)
                .selected_text(if value.is_empty() {
                    "None"
                } else {
                    value.as_str()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(value, String::new(), "None");
                    for o in options {
                        ui.selectable_value(value, (*o).to_owned(), *o);
                    }
                });
        }

        ui.add_space(4.0);
        let Some(i) = self.lib_selected else {
            ui.label(RichText::new("Select a tone to edit its details.").color(theme::DIM));
            return;
        };
        // The name is editable here, and only here. It is a label rather than
        // an identity now, so changing it moves nothing and breaks no setlist;
        // what it must stay is unique, or the library grows two things a
        // person cannot tell apart.
        let hash = self.lib_entries[i].hash.clone();
        let free = library::name_is_free(&self.lib_draft.name, &hash);
        let name = ui.add(
            egui::TextEdit::singleline(&mut self.lib_draft.name)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Heading)
                .hint_text("name this tone"),
        );
        if !free {
            ui.label(
                RichText::new("another tone has that name")
                    .small()
                    .color(theme::ACCENT),
            );
        }
        // Held back until the field is done being typed in, so a name in
        // mid-flight is not written and then written again.
        if name.lost_focus() && !free {
            self.lib_draft.name = self.lib_entries[i].meta.name.clone();
        }
        if !self.lib_entries[i].line.is_empty() {
            ui.label(
                RichText::new(&self.lib_entries[i].line)
                    .small()
                    .color(theme::DIM),
            );
        }
        let history = library::versions_of(&hash);
        if history.len() > 1 {
            let current = history
                .iter()
                .find(|version| version.hash == hash)
                .map_or(1, |version| version.version);
            ui.label(
                RichText::new(format!("Version {current} of {}", history.len()))
                    .small()
                    .color(theme::DIM),
            );
            let mut restore = None;
            ui.collapsing("Version history", |ui| {
                for version in history.iter().rev() {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "v{}  {}",
                            version.version,
                            display_date(&version.meta.modified_at)
                        ));
                        if version.hash == hash {
                            ui.label(RichText::new("current").small().color(theme::DIM));
                        } else if ui.small_button("Make current").clicked() {
                            restore = Some(version.hash.clone());
                        }
                    });
                }
            });
            if let Some(version) = restore {
                match library::make_current(&version) {
                    Ok(()) => {
                        self.refresh_library();
                        if let Some(row) = self
                            .lib_entries
                            .iter()
                            .position(|entry| entry.hash == version)
                        {
                            self.select_lib_entry(row);
                        }
                        self.note("restored an earlier tone version".to_owned());
                    }
                    Err(why) => self.note(why),
                }
                return;
            }
        } else {
            ui.label(RichText::new("Version 1").small().color(theme::DIM));
        }
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("lib-fields")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Song artist");
                        ui.text_edit_singleline(&mut self.lib_draft.artist);
                        ui.end_row();
                        ui.label("Song title");
                        ui.text_edit_singleline(&mut self.lib_draft.song);
                        ui.end_row();
                        ui.label("Tone part");
                        ui.text_edit_singleline(&mut self.lib_draft.part);
                        ui.end_row();
                        ui.label("Tone character");
                        combo(
                            ui,
                            "char",
                            &mut self.lib_draft.character,
                            &["clean", "drive", "hi-gain", "fuzz", "other"],
                        );
                        ui.end_row();
                        ui.label("Song genres");
                        ui.text_edit_singleline(&mut self.lib_genres_buf);
                        ui.end_row();
                        ui.label("Tone guitar");
                        ui.text_edit_singleline(&mut self.lib_draft.guitar);
                        ui.end_row();
                        ui.label("Tone pickups");
                        combo(
                            ui,
                            "pt",
                            &mut self.lib_draft.pickup_type,
                            &["single-coil", "humbucker", "P90"],
                        );
                        ui.end_row();
                        ui.label("Tone electronics");
                        combo(
                            ui,
                            "pe",
                            &mut self.lib_draft.pickup_electronics,
                            &["passive", "active"],
                        );
                        ui.end_row();
                        ui.label("Tone tuning");
                        ui.text_edit_singleline(&mut self.lib_draft.tuning);
                        ui.end_row();
                        ui.label("Tone gain");
                        ui.text_edit_singleline(&mut self.lib_draft.gain);
                        ui.end_row();
                        ui.label("My rating");
                        ui.horizontal(|ui| {
                            for star in 1..=5 {
                                let filled = self.lib_draft.rating >= star;
                                let icon = if filled {
                                    theme::Icon::StarOn
                                } else {
                                    theme::Icon::Star
                                };
                                let colour = if filled { theme::ACCENT } else { theme::DIM };
                                if theme::small_icon_button(ui, icon, Some(colour))
                                    .on_hover_text(format!("{star} out of 5"))
                                    .clicked()
                                {
                                    self.lib_draft.rating = if self.lib_draft.rating == star {
                                        0
                                    } else {
                                        star
                                    };
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Added");
                        ui.label(display_date(&self.lib_draft.added_at));
                        ui.end_row();
                        ui.label("Modified");
                        ui.label(display_date(&self.lib_draft.modified_at));
                        ui.end_row();
                    });
                self.lib_draft.genres = self
                    .lib_genres_buf
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();

                ui.add_space(6.0);
                ui.label(RichText::new("Song description").small().color(theme::DIM));
                ui.add(
                    egui::TextEdit::multiline(&mut self.lib_draft.description)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(6.0);
                ui.label(RichText::new("Tone description").small().color(theme::DIM));
                ui.add(
                    egui::TextEdit::multiline(&mut self.lib_draft.tone_description)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(6.0);
                ui.label(RichText::new("Song tags").small().color(theme::DIM));
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    for (ti, tag) in self.lib_draft.tags.iter().enumerate() {
                        if ui.button(format!("{tag}  ✕")).clicked() {
                            remove = Some(ti);
                        }
                    }
                });
                if let Some(ti) = remove {
                    self.lib_draft.tags.remove(ti);
                }
                ui.horizontal(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.lib_tag_add)
                            .hint_text("add a tag")
                            .desired_width(120.0),
                    );
                    let submit =
                        field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("+").clicked() || submit) && !self.lib_tag_add.trim().is_empty() {
                        let tag = self.lib_tag_add.trim().to_owned();
                        if !self.lib_draft.tags.contains(&tag) {
                            self.lib_draft.tags.push(tag);
                        }
                        self.lib_tag_add.clear();
                    }
                });
            });

        // Persist as the fields change; one tone's metadata, the rest of the
        // index untouched. A name that is not free is the one thing not
        // written: it would put two tones under one name for as long as it
        // took to finish typing the rest of it.
        if self.lib_draft != self.lib_entries[i].meta && free {
            let hash = self.lib_entries[i].hash.clone();
            let saved = match library::save_meta(&hash, &self.lib_draft) {
                Ok(saved) => saved,
                Err(error) => {
                    self.note(error);
                    return;
                }
            };
            self.lib_draft = saved.clone();
            self.lib_entries[i].meta = saved;
            self.lib_entries[i].added_at = self.lib_entries[i].meta.added_at.clone();
            self.lib_entries[i].modified_at = self.lib_entries[i].meta.modified_at.clone();
            self.lib_entries[i].rating = (self.lib_entries[i].meta.rating > 0)
                .then_some(self.lib_entries[i].meta.rating as f32);
            // The table reads this rather than the metadata, so a rename shows
            // in the row as it is typed.
            if !self.lib_draft.name.is_empty() {
                self.lib_entries[i].name = self.lib_draft.name.clone();
            }
            self.library_lookup.reindex(&self.lib_entries);
        }

        ui.add_space(10.0);
        ui.separator();
        // The way a tone leaves this machine for the web: its publishable
        // device artifact and the details beside it in the site's own fields.
        if ui
            .button("Export for the web")
            .on_hover_text(
                "write this tone's publishable preset with its details alongside, \
                 ready to upload",
            )
            .clicked()
        {
            self.export_for_the_web(i);
        }
        ui.add_space(6.0);
        // Removal drops the file into the library's .trash - recoverable, so no
        // confirmation ceremony.
        if ui
            .add(
                egui::Button::new(RichText::new("Remove from library").color(theme::DIM))
                    .frame(false),
            )
            .clicked()
        {
            let hash = self.lib_entries[i].hash.clone();
            let name = self.lib_entries[i].name.clone();
            match library::forget(&hash) {
                Ok(()) => self.note(format!("removed {name}")),
                Err(why) => self.note(why),
            }
            self.lib_selected = None;
            self.refresh_library();
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

    /// The impulse response slots, mirroring HX Edit's IRs tab.
    /// Everything about the *device* rather than the preset.
    ///
    /// Impulse responses used to sit in a permanent side panel next to a
    /// browser category also called IR, which invited exactly the question of
    /// what the difference was. It is this: the **IR category** puts an IR
    /// *block* in your signal chain, and that block plays whichever of the
    /// device's IR slots you point it at. This window is those slots - the
    /// device's library, shared by every preset. The list refreshes itself
    /// whenever it changes, so there is nothing to press.
    fn device_window(&mut self, ctx: &egui::Context) {
        if !self.show_device {
            return;
        }
        let mut open = true;
        egui::Window::new("Device")
            .open(&mut open)
            .default_width(460.0)
            .default_height(480.0)
            .collapsible(false)
            .show(ctx, |ui| {
                // Explanatory text has to be told how wide it may be, or egui
                // lays it out on one line and the window grows to fit it.
                ui.set_max_width(440.0);
                ui.label(
                    RichText::new(format!("{}  ·  firmware {}", self.device, self.firmware))
                        .color(theme::DIM),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .id_salt("device")
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new("BACK UP & RESTORE").small().color(theme::DIM));
                        ui.add_space(4.0);
                        ui.horizontal(|ui| self.backup_actions(ui));

                        theme::section_break(ui);

                        let free_ir =
                            (0..128).find(|slot| !self.irs.iter().any(|(used, _)| used == slot));
                        let mut import_ir = false;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("IMPULSE RESPONSES").small().color(theme::DIM));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    import_ir = ui
                                        .add_enabled(
                                            free_ir.is_some(),
                                            egui::Button::new("Import IR…"),
                                        )
                                        .on_disabled_hover_text("every IR slot is in use")
                                        .clicked();
                                },
                            );
                        });
                        if import_ir {
                            if let (Some(slot), Some(file)) = (
                                free_ir,
                                rfd::FileDialog::new()
                                    .add_filter("WAV", &["wav"])
                                    .pick_file(),
                            ) {
                                self.note(format!("loading IR into slot {}", slot + 1));
                                self.send(Cmd::LoadIr { slot, file });
                            }
                        }

                        if self.irs.is_empty() {
                            ui.label(RichText::new("No impulse responses").color(theme::DIM));
                        }
                        let irs = self.irs.clone();
                        let mut ir_save = None;
                        let mut ir_rename_start = None;
                        let mut ir_rename: Option<Option<(i64, String)>> = None;
                        for (slot, name) in &irs {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{:>3}", slot + 1)).monospace());
                                // Renaming happens in place: the name is the
                                // only thing about a slot you can edit, so
                                // clicking it is where you would try.
                                match &mut self.renaming_ir {
                                    Some((editing, draft)) if editing == slot => {
                                        let field = ui.add(
                                            egui::TextEdit::singleline(draft).desired_width(180.0),
                                        );
                                        field.request_focus();
                                        if field.lost_focus() {
                                            let done =
                                                ui.input(|i| i.key_pressed(egui::Key::Enter));
                                            ir_rename = Some(done.then(|| (*slot, draft.clone())));
                                        }
                                    }
                                    _ => {
                                        if ui
                                            .add(egui::Button::new(name).frame(false))
                                            .on_hover_text("click to rename")
                                            .clicked()
                                        {
                                            ir_rename_start = Some((*slot, name.clone()));
                                        }
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Clear").clicked() {
                                            self.send(Cmd::ClearIr(*slot));
                                        }
                                        // An IR that only ever existed on the
                                        // pedal can now come back off it.
                                        if ui
                                            .small_button("Save…")
                                            .on_hover_text("write this IR out as a WAV")
                                            .clicked()
                                        {
                                            ir_save = Some((*slot, name.clone()));
                                        }
                                    },
                                );
                            });
                        }
                        if let Some((slot, name)) = ir_save {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name(format!("{}.wav", sanitise(&name)))
                                .add_filter("WAV", &["wav"])
                                .save_file()
                            {
                                self.send(Cmd::SaveIr { slot, file: path });
                            }
                        }
                        if let Some(started) = ir_rename_start {
                            self.renaming_ir = Some(started);
                        }
                        if let Some(result) = ir_rename {
                            self.renaming_ir = None;
                            if let Some((slot, name)) = result {
                                self.send(Cmd::RenameIr { slot, name });
                            }
                        }

                        theme::section_break(ui);

                        let favourites = self.favourites.clone();
                        let selected = self.selected_block();
                        let free = (0..16).find(|i| !favourites.iter().any(|(n, _)| n == i));
                        let add_label = selected.as_ref().map_or_else(
                            || "Add current block".to_owned(),
                            |(_, name)| format!("Add “{name}”"),
                        );
                        let mut add_favourite = false;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("FAVORITE BLOCKS").small().color(theme::DIM));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    add_favourite = ui
                                        .add_enabled(
                                            selected.is_some() && free.is_some(),
                                            egui::Button::new(add_label),
                                        )
                                        .on_disabled_hover_text(if selected.is_none() {
                                            "select an effect block in the chain first"
                                        } else {
                                            "every favorite slot is in use"
                                        })
                                        .clicked();
                                },
                            );
                        });
                        if add_favourite {
                            if let (Some((block, name)), Some(index)) = (selected, free) {
                                self.send(Cmd::SaveFavourite { block, index, name });
                            }
                        }
                        if favourites.is_empty() {
                            ui.label(RichText::new("No favorite blocks").color(theme::DIM));
                        }
                        let mut forget = None;
                        for (index, name) in &favourites {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{:>3}", index + 1)).monospace());
                                ui.label(name);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Remove").clicked() {
                                            forget = Some(*index);
                                        }
                                    },
                                );
                            });
                        }
                        if let Some(index) = forget {
                            self.send(Cmd::ClearFavourite(index));
                        }

                        theme::section_break(ui);
                        ui.label(RichText::new("DIAGNOSTICS").small().color(theme::DIM));
                        ui.checkbox(&mut self.show_activity, "Device activity");
                    });
            });
        self.show_device = open;
    }

    /// The device's preferences, behind the cogwheel.
    ///
    /// Everything global except the EQ, which is a shape rather than a list and
    /// has a panel of its own.
    fn preferences_window(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let mut open = true;
        egui::Window::new("Preferences")
            .open(&mut open)
            .default_width(420.0)
            .default_height(440.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.set_max_width(400.0);
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.settings_list(ui, |group| group != "Global EQ")
                    });
            });
        self.show_preferences = open;
    }

    /// The global EQ, drawn as what it does rather than as eleven numbers.
    ///
    /// Two cuts and three peaking bands, over a log frequency axis. The handles
    /// are the controls: drag a band to move and lift it, scroll on one to
    /// narrow it, drag a cut along the floor. The numbers underneath say
    /// exactly where everything landed, because a curve is for aiming and a
    /// number is for repeating.
    fn eq_window(&mut self, ctx: &egui::Context) {
        if !self.show_eq {
            return;
        }
        let mut open = true;
        egui::Window::new("Global EQ")
            .open(&mut open)
            .default_width(560.0)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                if !matches!(self.connection, Connection::Online) {
                    ui.label(RichText::new("connect to read the device's EQ").color(theme::DIM));
                    return;
                }
                // Not "some settings have arrived" but "the EQ's own have":
                // `eq_curve_now` substitutes sensible numbers for ids it has
                // not seen, and a panel that can be dragged while it is showing
                // substitutes would write them over the pedal's real ones.
                if !self.eq_settings_known() {
                    ui.label(RichText::new("reading the device's EQ…").color(theme::DIM));
                    return;
                }
                // The bypass leads, because a curve you cannot hear is the
                // first thing to check and the last thing anyone remembers.
                let mut on = self.settings.get(&id::EQ_ON).is_some_and(|v| *v >= 0.5);
                ui.horizontal(|ui| {
                    if ui.add(theme::switch(&mut on)).changed() {
                        self.settings.insert(id::EQ_ON, on as u8 as f32);
                        self.send(Cmd::WriteSetting {
                            id: id::EQ_ON,
                            value: on as u8 as f32,
                        });
                    }
                    ui.label(if on { "On" } else { "Off" });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("Flatten")
                            .on_hover_text("every band back to no gain, both cuts off")
                            .clicked()
                        {
                            self.flatten_eq();
                        }
                    });
                });
                ui.add_space(6.0);
                self.eq_curve(ui, on);
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                self.eq_controls(ui);
            });
        self.show_eq = open;
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
    fn switch_view(&self, switch: u8, tint: egui::Color32) -> Option<SwitchView> {
        let found = self.switches.iter().find(|s| s.switch == switch)?;
        Some(SwitchView {
            switch,
            label: self
                .switch_draft
                .as_ref()
                .filter(|(drafting, _)| *drafting == switch)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| found.label.clone().unwrap_or_default()),
            carries: found.carries.first().map(|c| c.name.clone()),
            colour: found.colour,
            lit: self.led_colour(found.lit()),
            momentary: found.momentary,
            tint,
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
        painter.rect_filled(rect, egui::CornerRadius::same(4), theme::BACKGROUND);

        let x_of = |hz: f32| rect.left() + eq::position(hz) * rect.width();
        let y_of = |db: f32| rect.center().y - (db / RANGE_DB) * (rect.height() / 2.0);
        let db_of = |y: f32| ((rect.center().y - y) / (rect.height() / 2.0)) * RANGE_DB;
        let hz_of = |x: f32| eq::from_position((x - rect.left()) / rect.width());

        // The grid, at the frequencies and gains anyone reads an EQ by.
        let grid = egui::Stroke::new(1.0_f32, theme::PANEL);
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
                theme::DIM.gamma_multiply(0.7),
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
            egui::Stroke::new(1.0_f32, theme::DIM.gamma_multiply(0.6)),
        );

        // The response. Dim when the EQ is bypassed - the shape is still worth
        // seeing, it just is not doing anything.
        let curve = self.eq_curve_now();
        let ink = if active {
            theme::ACCENT
        } else {
            theme::DIM.gamma_multiply(0.8)
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
            painter.circle_stroke(at, 6.0, egui::Stroke::new(1.5_f32, theme::BACKGROUND));
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
                painter.circle_filled(at, 5.0, theme::BACKGROUND);
                painter.circle_stroke(
                    at,
                    5.0,
                    egui::Stroke::new(1.5_f32, mark.gamma_multiply(0.6)),
                );
            } else {
                painter.circle_filled(at, 6.0, mark);
                painter.circle_stroke(at, 6.0, egui::Stroke::new(1.5_f32, theme::BACKGROUND));
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

    /// The device's global settings, by name.
    ///
    /// The namespace is 154 numbered objects with no names anywhere in HX
    /// Edit's data. The ones here were identified by watching HX Edit write
    /// them, one control at a time - see `hx_proto::settings`. The rest are
    /// reachable only from the pedal's own menu, so they are not shown rather
    /// than shown as numbers nobody can act on.
    ///
    /// `wanted` decides which groups appear, because the same list serves two
    /// windows now: the EQ panel takes its own group, preferences takes the
    /// rest.
    fn settings_list(&mut self, ui: &mut egui::Ui, wanted: impl Fn(&str) -> bool) {
        use hx_proto::settings::{self, Kind};

        if !matches!(self.connection, Connection::Online) {
            ui.label(RichText::new("connect to read the device's settings").color(theme::DIM));
            return;
        }
        if self.settings.is_empty() {
            ui.label(RichText::new("reading the device's settings…").color(theme::DIM));
            return;
        }

        let mut write: Option<(i64, f32)> = None;
        let mut first_group = true;
        for group in settings::groups().into_iter().filter(|g| wanted(g)) {
            ui.add_space(if first_group { 6.0 } else { 18.0 });
            first_group = false;
            ui.label(
                RichText::new(group.to_uppercase())
                    .small()
                    .color(theme::DIM),
            );
            for setting in settings::SETTINGS.iter().filter(|s| s.group == group) {
                let Some(&current) = self.settings.get(&setting.id) else {
                    continue;
                };
                ui.horizontal(|ui| {
                    ui.add_sized([120.0, 18.0], egui::Label::new(setting.name).truncate());
                    match &setting.kind {
                        Kind::Switch(off, on) => {
                            let mut yes = current >= 0.5;
                            let label = if yes { *on } else { *off };
                            if ui.selectable_label(yes, label).clicked() {
                                yes = !yes;
                                write = Some((setting.id, yes as u8 as f32));
                            }
                        }
                        Kind::Choice(options) => {
                            let index = (current.round() as usize).min(options.len() - 1);
                            egui::ComboBox::from_id_salt(setting.id)
                                .selected_text(options[index])
                                .show_ui(ui, |ui| {
                                    for (i, option) in options.iter().enumerate() {
                                        if ui.selectable_label(i == index, *option).clicked() {
                                            write = Some((setting.id, i as f32));
                                        }
                                    }
                                });
                        }
                        Kind::Number { min, max, unit } => {
                            let mut value = current;
                            let slider = egui::Slider::new(&mut value, *min..=*max)
                                .suffix(*unit)
                                .clamping(egui::SliderClamping::Always);
                            if ui.add(slider).changed() {
                                write = Some((setting.id, value));
                            }
                        }
                    }
                });
            }
        }

        if let Some((id, value)) = write {
            // Show it at once: the device is the truth, but waiting a round trip
            // to redraw makes a knob feel like it did not take.
            self.settings.insert(id, value);
            self.send(Cmd::WriteSetting { id, value });
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
            let free =
                (0..128).find(|s| !self.irs.iter().any(|(slot, n)| slot == s && !n.is_empty()));
            match free {
                Some(slot) => {
                    self.note(format!(
                        "loading {} into IR slot {}",
                        path.display(),
                        slot + 1
                    ));
                    self.send(Cmd::LoadIr { slot, file: path });
                }
                None => self.note("no free impulse response slot".into()),
            }
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
                        let switch = catalog
                            .param(block.model_number, index)
                            .is_some_and(|p| p.kind == Kind::Switch);
                        params.push((index as i64, *value, switch));
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

    /// How wide a chain draws, by the renderer's own arithmetic: the space
    /// before the input, a column per block, the junctions and the padded
    /// stretch when a path splits, and the wire into the output.
    fn chain_width(&self, layout: &hx_proto::preset::Layout) -> f32 {
        let mut widest = 0.0f32;
        for path in &layout.paths {
            let mut width = 6.0;
            if path.input.is_some() {
                width += theme::BLOCK_WIDTH;
            }
            width += path.head.len() as f32 * theme::COLUMN;
            if !path.lanes.is_empty() {
                let stretch = path
                    .lanes
                    .iter()
                    .map(|lane| self.lane_width(lane))
                    .fold(0.0, f32::max);
                width += 2.0 * theme::JUNCTION_WIDTH + stretch;
            }
            width += path.tail.len() as f32 * theme::COLUMN;
            if path.output.is_some() {
                width += theme::WIRE_WIDTH + theme::BLOCK_WIDTH;
            }
            widest = widest.max(width);
        }
        widest
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
        let natural = self.chain_width(&preview.layout) + 24.0;
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
                ui.label(RichText::new(&preview.line).color(theme::DIM));
                ui.add_space(6.0);
                egui::ScrollArea::horizontal()
                    .id_salt("tone-preview")
                    .show(ui, |ui| {
                        let paths = self.layout.paths.clone();
                        ui.vertical(|ui| {
                            for (n, path) in paths.iter().enumerate() {
                                if paths.len() > 1 {
                                    ui.label(
                                        RichText::new(format!("PATH {}", n + 1))
                                            .small()
                                            .color(theme::DIM),
                                    );
                                }
                                let _ = self.path_row(ui, path);
                            }
                        });
                    });
                for skipped in &preview.skipped {
                    ui.label(RichText::new(skipped).small().color(theme::DIM));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Load into").color(theme::DIM));
                    let total = if self.presets.is_empty() {
                        self.preset_count as i64
                    } else {
                        self.presets.len() as i64
                    };
                    let labels: Vec<String> = (0..total)
                        .map(|i| {
                            let name = self.presets.get(i as usize).cloned().unwrap_or_default();
                            format!("{}  {}", hx_proto::rpc::slot_label(i), name)
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
            self.loading = true;
            self.note(format!(
                "loading {} into {}",
                preview.name,
                hx_proto::rpc::slot_label(preview.dest)
            ));
            match preview.load {
                LoadKind::Document(bytes) => self.send(Cmd::LoadDocument {
                    dest: preview.dest,
                    bytes,
                }),
                LoadKind::Steps(blocks) => self.send(Cmd::LoadSteps {
                    dest: preview.dest,
                    name: preview.name,
                    blocks,
                }),
            }
        } else if open && !cancel {
            self.preview = Some(preview);
        }
    }

    fn activity(&mut self, root_ui: &mut egui::Ui) {
        if !self.show_activity {
            return;
        }
        egui::Panel::bottom("activity")
            .exact_size(100.0)
            .show(root_ui, |ui| {
                ui.label(RichText::new("DEVICE ACTIVITY").small().color(theme::DIM));
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(RichText::new(line).small().monospace().color(theme::DIM));
                        }
                    });
            });
    }

    /// The signal path, drawn the way it is wired.
    ///
    /// The slot array is a fixed topology rather than a running order: the
    /// split sits after the output in it even though the signal reaches it
    /// first. Read as a list it puts the split and join on the end of the
    /// chain, which is where they used to be drawn.
    ///
    /// The main line never changes height: input, output and everything the
    /// undivided signal passes through sit on one row, and a parallel branch
    /// hangs *below* the stretch it parallels, the way HX Edit draws it.
    /// Splits and joins are not blocks in the line either - they are drawn as
    /// the wiring forking and merging, still clickable for their own
    /// parameters.
    ///
    /// The number of lanes is not fixed at two: Helix and Helix LT carry two
    /// independent signal paths, so a preset that splits both has four.
    fn signal_chain(&mut self, root_ui: &mut egui::Ui) {
        let mut height = 40.0;
        for path in &self.layout.paths {
            height += theme::BLOCK_HEIGHT;
            height += path.lanes.len().saturating_sub(1) as f32 * theme::LANE_HEIGHT;
            if self.can_offer_branch(path) {
                height += 4.0 + theme::GHOST_HEIGHT;
            }
            if self.layout.paths.len() > 1 {
                height += 20.0;
            }
        }

        // The topology determines a useful first height, but the musician
        // decides how much of the window the chain needs right now. Keep a
        // small editor visible at the largest setting so the splitter cannot
        // strand the selected block completely off-screen.
        let min_height = 126.0;
        let max_height = (root_ui.available_height() - 96.0).max(min_height);
        let default_height = height.clamp(min_height, max_height);
        if std::mem::take(&mut self.fit_chain_on_next_frame) {
            root_ui.ctx().data_mut(|data| {
                data.remove::<egui::containers::panel::PanelState>(egui::Id::new("chain"));
            });
        }

        processor::chain_panel(
            root_ui,
            "chain",
            default_height,
            min_height..=max_height,
            |ui| {
                ui.add_space(6.0);
                if self.chain.is_empty() {
                    ui.centered_and_justified(|ui| {
                        if self.loading {
                            ui.horizontal(|ui| {
                                theme::spinner(ui);
                                ui.label(RichText::new("loading…").color(theme::DIM));
                            });
                        } else {
                            ui.label(RichText::new("No preset loaded").color(theme::DIM));
                        }
                    });
                    return;
                }

                let mut pick = None;
                // Drag-to-scroll off: it claims the pointer press, so a
                // click-only widget like an insert point never completes its
                // click - and it would fight dragging a block along the chain,
                // which is the same gesture.
                egui::ScrollArea::both()
                    // The scroll area must occupy the dragged panel height;
                    // otherwise its content's natural height pulls the panel
                    // back and makes the resize handle feel inert.
                    .auto_shrink([false, false])
                    .scroll_source(egui::scroll_area::ScrollSource {
                        drag: egui::scroll_area::DragScroll::Never,
                        ..Default::default()
                    })
                    .show(ui, |ui| {
                        self.gap_rects.clear();
                        self.block_rects.clear();
                        self.ghost_target = None;
                        let paths = self.layout.paths.clone();
                        ui.vertical(|ui| {
                            for (n, path) in paths.iter().enumerate() {
                                if paths.len() > 1 {
                                    ui.label(
                                        RichText::new(format!("PATH {}", n + 1))
                                            .small()
                                            .color(theme::DIM),
                                    );
                                }
                                pick = self.path_row(ui, path).or(pick);
                            }
                        });
                        self.block_drag(ui);
                    });

                if let Some(i) = pick {
                    // Purely a local view change. Mirroring the selection onto
                    // the device's own screen meant every click was a round
                    // trip, and clicking through a chain quickly wedged it.
                    self.selected = i;
                    self.browsing = None;
                    self.browsing_shelf = None;
                }
            },
        );
    }

    /// One signal path: the main line straight across, branches hanging below.
    ///
    /// A split divides a *stretch* of the path, not all of it - the split
    /// records the slot it attaches before, and the blocks on either side of
    /// that stretch carry the undivided signal. The first lane of the divided
    /// stretch *is* the main line, so it stays in the row; the other branches
    /// are drawn beneath it between the fork and the merge.
    fn path_row(&mut self, ui: &mut egui::Ui, path: &hx_proto::preset::Path) -> Option<usize> {
        let mut pick = None;
        let below = path.lanes.len().saturating_sub(1);
        // Every lane spans the same stretch, so they are padded to the widest
        // and the merge lands where all of them end.
        let stretch = path
            .lanes
            .iter()
            .map(|l| self.lane_width(l))
            .fold(0.0, f32::max);

        // Captured while the main row is drawn, to place what hangs below it:
        // the branch rows start under the fork, and the ghost of an offered
        // branch runs from the input's edge to the output's.
        let mut fork_end = None;
        let mut input_rect = None;
        let mut output_rect = None;

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(6.0);

                if let Some(input) = path.input {
                    let (hit, rect) = self.endpoint(ui, input);
                    pick = hit.or(pick);
                    input_rect = Some(rect);
                }
                // A gap before each block, so a chain can be built anywhere.
                for slot in &path.head {
                    self.insert_point(ui, *slot);
                    pick = self.block_at(ui, *slot).or(pick);
                }

                if !path.lanes.is_empty() {
                    if let Some(split) = path.split {
                        let (hit, rect) = self.junction(ui, split, below, true);
                        pick = hit.or(pick);
                        fork_end = Some(rect.right());
                    }
                    pick = self.lane_row(ui, &path.lanes[0], stretch).or(pick);
                    if let Some(join) = path.join {
                        let (hit, _) = self.junction(ui, join, below, false);
                        pick = hit.or(pick);
                    }
                }

                // And everything the recombined signal passes through.
                for slot in &path.tail {
                    self.insert_point(ui, *slot);
                    pick = self.block_at(ui, *slot).or(pick);
                }
                if let Some(output) = path.output {
                    self.insert_point(ui, output);
                    let (hit, rect) = self.endpoint(ui, output);
                    pick = hit.or(pick);
                    output_rect = Some(rect);
                }
            });

            // The branches, aligned column for column under the stretch they
            // parallel.
            for lane in path.lanes.iter().skip(1) {
                ui.add_space(theme::LANE_HEIGHT - theme::BLOCK_HEIGHT);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if let Some(x) = fork_end {
                        let indent = x - ui.cursor().min.x;
                        ui.add_space(indent.max(0.0));
                    }
                    pick = self.lane_row(ui, lane, stretch).or(pick);
                });
            }

            self.junction_drag(ui, path);

            // The offer of a parallel branch, where it would actually run.
            if self.can_offer_branch(path) {
                if let (Some(input), Some(output)) = (input_rect, output_rect) {
                    self.ghost_branch(ui, path, input, output.left());
                }
            }
        });
        pick
    }

    /// Follow a fork or merge being dragged along the main line.
    ///
    /// Every gap it can land in shows a dot while the drag lasts, the one
    /// nearest the pointer takes the accent, and releasing commits the move.
    /// A fork can go anywhere between the input and the merge; a merge,
    /// anywhere between the fork and the output. Escape lets go.
    fn junction_drag(&mut self, ui: &mut egui::Ui, path: &hx_proto::preset::Path) {
        if self.display_only {
            return;
        }
        let Some((slot, opening)) = self.dragging_junction else {
            return;
        };
        let dragged = if opening { path.split } else { path.join };
        if dragged != Some(slot) {
            return;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dragging_junction = None;
            return;
        }
        let Some((lowest, highest, current)) = attach_range(path, opening) else {
            return;
        };

        let candidates: Vec<(usize, egui::Rect)> = self
            .gap_rects
            .iter()
            .filter(|(before, _)| (lowest..=highest).contains(before))
            .copied()
            .collect();
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let nearest = pointer.and_then(|p| {
            candidates
                .iter()
                .min_by(|a, b| {
                    let da = (a.1.center().x - p.x).abs();
                    let db = (b.1.center().x - p.x).abs();
                    da.total_cmp(&db)
                })
                .copied()
        });

        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        for (before, rect) in &candidates {
            let hot = nearest.is_some_and(|(n, _)| n == *before);
            theme::attach_marker(ui, rect.center(), hot);
        }

        if ui.input(|i| i.pointer.any_released()) {
            if let Some((before, _)) = nearest {
                if before != current {
                    self.edit(Cmd::MoveJunction {
                        junction: slot,
                        before,
                    });
                }
            }
            self.dragging_junction = None;
        }
    }

    /// Whether to offer a parallel branch: the path has somewhere to put one,
    /// something to parallel, and nothing on the branch yet.
    fn can_offer_branch(&self, path: &hx_proto::preset::Path) -> bool {
        !self.display_only
            && path.lanes.is_empty()
            && !path.head.is_empty()
            && self.free_on_branch(path).is_some()
    }

    /// The dashed preview of the branch a click would create: it forks after
    /// the input, runs under the whole line, and merges before the output -
    /// which is exactly where the real one will go.
    fn ghost_branch(
        &mut self,
        ui: &mut egui::Ui,
        path: &hx_proto::preset::Path,
        input: egui::Rect,
        right: f32,
    ) {
        let Some(at) = self.free_on_branch(path) else {
            return;
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let indent = input.right() - ui.cursor().min.x;
            ui.add_space(indent.max(0.0));
            let hit = theme::ghost_branch(ui, (right - input.right()).max(90.0), input.center().y)
                .on_hover_text("add a block on a parallel branch\nor drag one down here");
            self.ghost_target = Some((at, hit.rect));
            if hit.clicked() {
                self.inserting_at = Some(at);
                self.insert_pos = Some(hit.rect.center_bottom() + egui::vec2(-260.0, 8.0));
                self.insert_opened = Some(std::time::Instant::now());
                self.browsing = None;
                self.search.clear();
            }
        });
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

    /// Follow a block being dragged along the chain, resolved fresh from the
    /// pointer every frame - never from what was hovered some frames ago,
    /// which is how a release over nothing came to move a block anyway.
    ///
    /// A ghost of the block rides the pointer. Every gap shows a dot;
    /// dropping into the nearest one slides the block in there, marked by a
    /// bar filling the gap. Dropping onto a block in *another* lane trades
    /// places with it, marked by outlining that block - the lanes are not
    /// contiguous, so between them a move is a trade. Escape lets go.
    fn block_drag(&mut self, ui: &mut egui::Ui) {
        if self.display_only {
            return;
        }
        let Some(from) = self.dragging else {
            return;
        };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dragging = None;
            return;
        }
        let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };

        // A drop means one of three things: onto the offered branch to run
        // the block in parallel, onto a block in the other lane to trade
        // places with it, or into the nearest gap within reach to slide in.
        let ghost = self
            .ghost_target
            .filter(|(_, rect)| rect.expand(6.0).contains(pointer));
        let swap = if ghost.is_some() {
            None
        } else {
            self.block_rects
                .iter()
                .find(|(slot, rect)| {
                    *slot != from && rect.contains(pointer) && !self.same_lane(from, *slot)
                })
                .map(|(slot, rect)| (*slot, *rect))
        };
        let gap = if ghost.is_some() || swap.is_some() {
            None
        } else {
            let candidates = self
                .gap_rects
                .iter()
                // The gaps either side of the block itself go nowhere.
                .filter(|(before, _)| *before != from && *before != from + 1);
            // A gap under the pointer wins outright; otherwise the nearest
            // one within reach - half a row vertically, so the main line and
            // a branch never compete for a drop between them, and a column
            // horizontally, so a drop far from any gap means nothing.
            candidates
                .clone()
                .find(|(_, rect)| rect.contains(pointer))
                .or_else(|| {
                    candidates
                        .filter(|(_, rect)| {
                            (pointer.y - rect.center().y).abs() < theme::LANE_HEIGHT * 0.5
                                && (pointer.x - rect.center().x).abs() < theme::COLUMN
                        })
                        .min_by(|a, b| {
                            let da = (a.1.center().x - pointer.x).abs();
                            let db = (b.1.center().x - pointer.x).abs();
                            da.total_cmp(&db)
                        })
                })
                .map(|(before, rect)| (*before, *rect))
        };

        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        for (before, rect) in &self.gap_rects {
            if *before == from || *before == from + 1 {
                continue;
            }
            let hot = gap.is_some_and(|(g, _)| g == *before);
            if hot {
                theme::insert_marker(ui, *rect);
            } else {
                theme::attach_marker(ui, rect.center(), false);
            }
        }
        if let Some((_, rect)) = swap {
            theme::swap_marker(ui, rect);
        }
        if let Some((_, rect)) = ghost {
            // The branch lights the way the + does when it is the drop.
            theme::attach_marker(ui, egui::pos2(rect.center().x, rect.bottom() - 12.0), true);
        }
        if let Some(i) = self.index_of(from) {
            let block = &self.chain[i];
            theme::drag_ghost(
                ui.ctx(),
                pointer,
                &self.slot_label(block),
                self.block_colour(block),
            );
        }

        if ui.input(|i| i.pointer.any_released()) {
            if let Some((before, _)) = ghost {
                self.edit(Cmd::MoveBlockBefore { from, before });
            } else if let Some((slot, _)) = swap {
                self.edit(Cmd::MoveBlock { from, to: slot });
            } else if let Some((before, _)) = gap {
                self.edit(Cmd::MoveBlockBefore { from, before });
            }
            self.dragging = None;
        }
    }

    /// A gap you can add a block to, at `before`.
    ///
    /// One click opens the picker at the gap, and the model chosen there goes
    /// in here. Adding a pedal used to mean finding an empty slot and changing
    /// its model, which required knowing the slot topology - this puts the
    /// action where the pedal goes.
    fn insert_point(&mut self, ui: &mut egui::Ui, before: usize) {
        // In display mode the gap is just wire: same width, nothing to click.
        if self.display_only {
            theme::wire_run(ui, theme::WIRE_WIDTH, theme::BLOCK_HEIGHT);
            return;
        }
        let response =
            theme::insert_point(ui, theme::BLOCK_HEIGHT).on_hover_text("add a block here");
        self.gap_rects.push((before, response.rect));
        if response.clicked() {
            self.inserting_at = Some(before);
            // Anchored under the gap, so the choosing happens where the
            // pedal will go.
            self.insert_pos = Some(response.rect.center_bottom() + egui::vec2(-260.0, 6.0));
            self.insert_opened = Some(std::time::Instant::now());
            self.browsing = None;
            self.search.clear();
        }
    }

    /// The first free slot on a path's branch, if it can carry one.
    fn free_on_branch(&self, path: &hx_proto::preset::Path) -> Option<usize> {
        let split = path.split?;
        let join = path.join.unwrap_or(usize::MAX);
        // A slot between the split and the join that nothing occupies.
        (split + 1..join).find(|p| !self.chain.iter().any(|b| b.position == *p as i64))
    }

    /// One block in the line: clickable to edit, draggable to move.
    fn block_at(&mut self, ui: &mut egui::Ui, slot: usize) -> Option<usize> {
        let i = self.index_of(slot)?;
        let block = self.chain[i].clone();
        let art = self.artwork(&block);
        let colour = self.block_colour(&block);
        let hit = theme::block_button_tinted(
            ui,
            &self.slot_label(&block),
            self.block_category(&block).as_deref(),
            art.as_ref(),
            i == self.selected,
            block.enabled,
            colour,
        );
        // A block something reaches wears a small tag saying what. An
        // assignment you cannot see is one you find out about on stage, and the
        // chain is the only place you see every block at once.
        if !self.display_only {
            if let Some(marks) = self.control_marks().get(&block.position) {
                // One tag per source, however many things that source drives on
                // this block: two entries reading "EXP1 EXP1" say nothing the
                // one says, and the room is four characters wide.
                let mut shown: Vec<String> = Vec::new();
                for (source, _) in marks {
                    let short = source.short();
                    if !shown.contains(&short) {
                        shown.push(short);
                    }
                }
                theme::block_tag(ui, hit.rect, &shown.join(" "), colour);
                let listed: Vec<String> = marks
                    .iter()
                    .map(|(source, what)| match what.as_str() {
                        "Auto-engage" => format!(
                            "{} engages this on its own when you move it",
                            source.label()
                        ),
                        what => format!("{} controls {what}", source.label()),
                    })
                    .collect();
                hit.clone().on_hover_text(listed.join("\n"));
            }
            self.block_rects.push((slot, hit.rect));
            if hit.drag_started() {
                self.dragging = Some(slot);
            }
            // The same three actions the block's own header offers, on the
            // block itself - which is where a hand goes when the block it wants
            // is not the one being edited.
            if self.is_effect(&block) {
                let copied = self.copied_block;
                let mut action = None;
                hit.context_menu(|ui| {
                    if ui.button("Copy").clicked() {
                        action = Some(RowAction::Copy);
                        ui.close();
                    }
                    if ui
                        .add_enabled(copied.is_some(), egui::Button::new("Paste"))
                        .clicked()
                    {
                        action = Some(RowAction::Paste);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Remove").clicked() {
                        action = Some(RowAction::Remove);
                        ui.close();
                    }
                });
                match action {
                    Some(RowAction::Copy) => {
                        self.copied_block = Some(slot);
                        self.note(format!("copied {}", self.slot_label(&block)));
                    }
                    Some(RowAction::Paste) => {
                        if let Some(from) = copied {
                            self.edit(Cmd::CopyBlock { from, to: slot });
                        }
                    }
                    Some(RowAction::Remove) => self.edit(Cmd::ClearBlock(block.position)),
                    _ => {}
                }
            }
        }
        hit.clicked().then_some(i)
    }

    /// One lane of the divided stretch: a gap before every block, one after
    /// the last while the lane has room, and plain wire out to the merge so
    /// every lane ends where the branches meet.
    fn lane_row(
        &mut self,
        ui: &mut egui::Ui,
        lane: &hx_proto::preset::Lane,
        stretch: f32,
    ) -> Option<usize> {
        let mut pick = None;
        let mut used = 0.0;
        if lane.blocks.is_empty() && !lane.span.is_empty() {
            // An empty stretch is a plain wire, but it still takes a block.
            self.insert_point(ui, lane.span.start);
            used += theme::WIRE_WIDTH;
        }
        for slot in &lane.blocks {
            self.insert_point(ui, *slot);
            pick = self.block_at(ui, *slot).or(pick);
            used += theme::COLUMN;
        }
        if let Some(last) = lane.blocks.last() {
            if lane.blocks.len() < lane.span.len() {
                self.insert_point(ui, *last + 1);
                used += theme::WIRE_WIDTH;
            }
        }
        if stretch > used {
            theme::wire_run(ui, stretch - used, theme::BLOCK_HEIGHT);
        }
        pick
    }

    /// How much room a lane's blocks and gaps ask for; see [`Self::lane_row`].
    fn lane_width(&self, lane: &hx_proto::preset::Lane) -> f32 {
        if lane.blocks.is_empty() {
            return if lane.span.is_empty() {
                0.0
            } else {
                theme::WIRE_WIDTH
            };
        }
        let mut width = lane.blocks.len() as f32 * theme::COLUMN;
        if lane.blocks.len() < lane.span.len() {
            width += theme::WIRE_WIDTH;
        }
        width
    }

    /// An input or output tile. Not draggable: the endpoints are fixtures of
    /// the topology, not blocks to reorder.
    fn endpoint(&mut self, ui: &mut egui::Ui, slot: usize) -> (Option<usize>, egui::Rect) {
        let Some(i) = self.index_of(slot) else {
            return (None, ui.cursor());
        };
        let block = self.chain[i].clone();
        let art = self.artwork(&block);
        let colour = self.block_colour(&block);
        let hit = theme::block_button_tinted(
            ui,
            &self.slot_label(&block),
            self.block_category(&block).as_deref(),
            art.as_ref(),
            i == self.selected,
            block.enabled,
            colour,
        );
        (hit.clicked().then_some(i), hit.rect)
    }

    /// The fork or merge itself, drawn as wiring: draggable along the line,
    /// clickable for its own parameters - a split's mode, a join's levels.
    fn junction(
        &mut self,
        ui: &mut egui::Ui,
        slot: usize,
        below: usize,
        opening: bool,
    ) -> (Option<usize>, egui::Rect) {
        let Some(i) = self.index_of(slot) else {
            return (None, ui.cursor());
        };
        let what = if opening {
            "the signal forks here\ndrag to move it, click for how it divides"
        } else {
            "the branches rejoin here\ndrag to move it, click for levels"
        };
        let label = self.slot_label(&self.chain[i]);
        let tag = if opening { split_tag(&label) } else { None };
        let held = self.dragging_junction == Some((slot, opening));
        let hit = theme::junction(ui, below, opening, i == self.selected || held, tag)
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text(format!("{label}\n{what}"));
        if hit.drag_started() && !self.display_only {
            self.dragging_junction = Some((slot, opening));
        }
        (hit.clicked().then_some(i), hit.rect)
    }

    fn index_of(&self, slot: usize) -> Option<usize> {
        self.chain.iter().position(|b| b.position == slot as i64)
    }

    /// The block being edited, given the whole middle of the window.
    ///
    /// This used to share a column with the model list, which made choosing a
    /// different pedal look as important as adjusting the one you have. The
    /// pedal is the work; the shelf is a side trip, so it is a panel of its
    /// own beside this one.
    fn editor(&mut self, root_ui: &mut egui::Ui) {
        processor::editor(root_ui, |ui| {
            let Some(block) = self.chain.get(self.selected).cloned() else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Connect a device to begin").color(theme::DIM));
                });
                return;
            };

            self.pedal_header(ui, &block);
            ui.separator();

            if !self.is_effect(&block) {
                self.endpoint_editor(ui, &block);
                return;
            }

            let Some(model) = self.slot_model(&block).cloned() else {
                ui.label(RichText::new("Install HX Edit for model names").color(theme::DIM));
                return;
            };
            egui::ScrollArea::vertical()
                .id_salt("pedal")
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    let art = self.artwork(&block);
                    self.pedal(
                        ui,
                        &model,
                        &block.values.clone(),
                        block.position,
                        false,
                        art.as_ref(),
                    );

                    // An Amp+Cab is two models sharing a block; the cab has its own
                    // controls and its own name.
                    if let Some(cab) = block.paired.and_then(|m| {
                        self.catalog
                            .as_ref()
                            .and_then(|c| c.model_number(m))
                            .cloned()
                    }) {
                        ui.add_space(14.0);
                        ui.separator();
                        let cab_art = self
                            .catalog
                            .as_ref()
                            .and_then(|c| c.artwork(&cab))
                            .map(|p| theme::Art::whole(format!("file://{}", p.display())));
                        self.pedal(
                            ui,
                            &cab,
                            &block.paired_values.clone(),
                            block.position,
                            true,
                            cab_art.as_ref(),
                        );
                    }
                });
        });
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
                                .color(theme::ACCENT),
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
                                ui.label(RichText::new(line).color(theme::TEXT));
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
                                RichText::new("then, once it is downloaded:").color(theme::DIM),
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
                                        .color(theme::DIM),
                                );
                            }

                            ui.add_space(10.0);
                            if self.extracting.is_some() {
                                ui.horizontal(|ui| {
                                    theme::spinner(ui);
                                    if let Some(status) = &self.onboarding_status {
                                        ui.label(RichText::new(status).color(theme::DIM));
                                    }
                                });
                            } else if let Some(status) = &self.onboarding_status {
                                ui.label(RichText::new(status).color(theme::ACCENT));
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
                                .color(theme::DIM),
                            );
                            ui.horizontal(|ui| {
                                center_row(ui, credits_width(ui), |ui| {
                                    ui.label(
                                        RichText::new("made with ♥ by").small().color(theme::DIM),
                                    );
                                    ui.hyperlink_to(
                                        RichText::new("Carmine Paolino").small(),
                                        "https://paolino.me",
                                    );
                                    ui.label(RichText::new("·").small().color(theme::DIM));
                                    ui.hyperlink_to(
                                        RichText::new("follow updates").small(),
                                        "https://x.com/paolino",
                                    );
                                    ui.label(RichText::new("·").small().color(theme::DIM));
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

    /// The pedal's name and the things you do to the block itself.
    ///
    /// Wrapping rather than right-aligned: at a narrow window the right-to-left
    /// layout ran these buttons back across the block's own name.
    fn pedal_header(&mut self, ui: &mut egui::Ui, block: &session::Block) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let colour = self.block_colour(block);
            theme::category_swatch(ui, colour);
            ui.heading(self.slot_label(block));

            if !self.is_effect(block) {
                return;
            }
            ui.add_space(12.0);

            // The way in that does not need to be discovered. Right-clicking a
            // control does the same thing and is faster once you know; nobody
            // finds a right-click on a knob by looking at it.
            let picking = self.assigning == Some(block.position);
            if ui
                .selectable_label(picking, "Assign control")
                .on_hover_text(
                    "put a knob or the on/off switch under a footswitch, \
                     an expression pedal or MIDI",
                )
                .clicked()
            {
                self.assigning = (!picking).then_some(block.position);
            }
            ui.add_space(8.0);

            if theme::icon_button(ui, theme::Icon::Copy, true)
                .on_hover_text("Copy this block")
                .clicked()
            {
                self.copied_block = Some(block.position as usize);
                self.note(format!("copied {}", self.slot_label(block)));
            }
            if theme::icon_button(ui, theme::Icon::Paste, self.copied_block.is_some())
                .on_hover_text("Paste - put the copied block here")
                .on_disabled_hover_text("Paste - no block copied yet")
                .clicked()
            {
                if let Some(from) = self.copied_block {
                    self.edit(Cmd::CopyBlock {
                        from,
                        to: block.position as usize,
                    });
                }
            }
            if theme::icon_button(ui, theme::Icon::Remove, true)
                .on_hover_text("Remove - take this block out of the chain")
                .clicked()
            {
                self.edit(Cmd::ClearBlock(block.position));
            }
        });
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

    /// What to call the two ends of a travel, which depends on what moves it.
    ///
    /// An expression pedal sweeps between them, so they are a minimum and a
    /// maximum. A footswitch has two positions, and those two numbers are the
    /// value when it is up and the value when it is down: `Gain | Footswitch 2
    /// | 0.0 | 10.0` under "Min" and "Max" reads like something has gone wrong,
    /// and it is an ordinary clean boost.
    fn travel_words(source: hx_proto::rpc::Source) -> (&'static str, &'static str) {
        match source {
            hx_proto::rpc::Source::Footswitch(_) => ("Off", "On"),
            _ => ("Min", "Max"),
        }
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

    /// Every assignment on this block, as the table everything else uses.
    ///
    /// The markers on the controls say *that* something is assigned; this says
    /// what, all of it in one place, which is the thing you want when you are
    /// working out why a switch does two things at once. The ends of the travel
    /// are the other half of an assignment, so they are columns here - drawn as
    /// the pedal's own knobs, in the parameter's own units, because that is what
    /// they are. The document knows them; opcode 36 reports the defaults however
    /// far they have been dragged.
    fn assignment_list(&mut self, ui: &mut egui::Ui, position: i64) {
        use hx_proto::preset::Target;
        let listed: Vec<Row> = self
            .assignments_on(position)
            .into_iter()
            .map(|a| {
                let travel = self.travel_of(position, a.target);
                let reading = |value: f32| match (&travel, self.catalog.as_ref()) {
                    (Some(travel), Some(catalog)) => catalog.format(&travel.param, value),
                    _ => format!("{value:.2}"),
                };
                Row {
                    target: a.target,
                    // A wah's bypass under EXP 1 is auto-engage, and the row
                    // should say the thing it does rather than the thing it is.
                    name: match Self::auto_engage(a.source, a.target) {
                        true => "Auto-engage".to_owned(),
                        false => self.target_name(position, a.target),
                    },
                    source: a.source,
                    // Where a number is being dragged, that is the number: the
                    // document does not hear about it until the drag stops, and
                    // a cell drawn from the document until then cannot be
                    // dragged anywhere at all.
                    cc: self.cc_drafts.get(&(position, a.target)).copied().or(a.cc),
                    min: a.min,
                    max: a.max,
                    min_text: reading(a.min),
                    max_text: reading(a.max),
                    travel,
                }
            })
            .collect();
        if listed.is_empty() {
            return;
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(RichText::new("ASSIGNMENTS").small().color(theme::DIM));
        ui.add_space(2.0);

        // One header covers every row under it, so it can only follow the
        // source while the rows agree about what the ends are. They usually do:
        // a block is driven by one thing. Where they do not, Min and Max are
        // the words that are true of both, and each cell says which end it is
        // in its own row's terms anyway.
        //
        // The rows that have a travel decide it, because they are the ones with
        // numbers under the heading. A table of nothing but bypasses has no
        // numbers at all, and then every row gets a say rather than none.
        let words = |rows: &mut dyn Iterator<Item = &Row>| {
            rows.map(|row| Self::travel_words(row.source))
                .reduce(|a, b| if a == b { a } else { ("Min", "Max") })
        };
        let ends = words(&mut listed.iter().filter(|row| row.travel.is_some()))
            .or_else(|| words(&mut listed.iter()))
            .unwrap_or(("Min", "Max"));

        // The block's colour, which is what its badges are painted in wherever
        // they appear.
        let tint = self
            .chain
            .iter()
            .find(|b| b.position == position)
            .map(|b| self.block_colour(b))
            .unwrap_or(theme::ACCENT);

        // Nothing fills. This table has five narrow columns and lives in a panel
        // as wide as the window; a column that took the slack put half a screen
        // of nothing between what a control is and what drives it.
        let mut grid = table::Grid {
            columns: vec![
                table::Column::new("Control", 150.0),
                table::Column::new("Source", 74.0),
                table::Column::new("CC", 56.0),
                table::Column::new(ends.0, 70.0).editable(),
                table::Column::new(ends.1, 70.0).editable(),
            ],
            sticky: 1,
            menu: vec!["Remove".to_owned()],
            row_height: 74.0,
            ..Default::default()
        };
        for row in &listed {
            // A switch has no travel to move: it is on or off. A parameter's
            // ends are values of that parameter, so they wear its own knob.
            let end = |value: f32, text: &str, high: bool| match &row.travel {
                Some(travel) => table::Cell::Knob {
                    value,
                    range: travel.range.clone(),
                    text: text.to_owned(),
                    hover: Self::end_meaning(row.source, high),
                },
                None => table::Cell::Dim("-".to_owned()),
            };
            grid.rows.push(vec![
                table::Cell::Text(row.name.clone()),
                // The chain's own badge, in the block's own colour. FS1 on the
                // block, FS1 beside the on/off switch, FS1 here: one thing,
                // written the same way, wherever you meet it.
                table::Cell::Tag {
                    text: row.source.short(),
                    colour: tint,
                    hover: row.source.label(),
                },
                // The number, beside the source that uses it. It used to be in
                // the assign popup, two right-clicks deep and invisible until
                // you were already there; the menu is for choosing what drives
                // a control, and this adjusts one that already exists.
                match row.source {
                    hx_proto::rpc::Source::MidiCc => table::Cell::Number {
                        value: row.cc.unwrap_or(DEFAULT_CC),
                        range: 0..=127,
                        hover: "which MIDI CC drives this\ndrag, or click to type",
                    },
                    _ => table::Cell::Dim("-".to_owned()),
                },
                end(row.min, &row.min_text, false),
                end(row.max, &row.max_text, true),
            ]);
        }
        let row_of = |target: Target| listed.iter().position(|row| row.target == target);
        grid.selected = self.assign_selected.and_then(row_of);
        if let Some((target, high, draft)) = self.assign_editing.clone() {
            grid.editing = row_of(target).map(|row| (row, if high { HIGH_END } else { LOW_END }));
            grid.draft = draft;
        }

        // Bounded to its own rows: it sits inside the panel's scroll area, and
        // a virtualised table handed the rest of the page would take it.
        let height = 74.0 * listed.len() as f32 + table::ROW_HEIGHT + 4.0;
        let did = ui
            .allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                // Claimed in full, not just where the table happens to have
                // painted. A virtualised table lays its rows out itself and
                // leaves the ui it was given no taller than a header, so
                // anything drawn after it landed on top of the first row.
                ui.set_min_height(height);
                table::show(ui, "assignments", &mut grid)
            })
            .inner;

        if let Some((row, ..)) = did.clicked {
            self.assign_selected = listed.get(row).map(|row| row.target);
        }
        // Turning a knob writes as it turns, the way the pedal's own do.
        if let Some((row, col, value)) = did.turned {
            if let Some(row) = listed.get(row) {
                self.move_travel(position, row.target, col == HIGH_END, value);
            }
        }
        // A CC is an address, not a sweep. Every step is kept so the field
        // follows the pointer; only the one it comes to rest on is sent, which
        // is what stops a drag from ordering a document read per pixel.
        if let Some((row, _, cc, settled)) = did.numbered {
            if let Some(row) = listed.get(row) {
                self.cc_drafts.insert((position, row.target), cc);
                if settled {
                    self.assign_action(position, row.target, AssignAction::Cc(cc));
                }
            }
        }
        if let Some((row, col)) = did.edit {
            match listed.get(row) {
                Some(row) if row.travel.is_some() => {
                    let high = col == HIGH_END;
                    let text = if high { &row.max_text } else { &row.min_text };
                    self.assign_editing = Some((row.target, high, text.clone()));
                }
                // A switch answers a click by selecting, as it did before.
                _ => self.assign_selected = listed.get(row).map(|row| row.target),
            }
        } else {
            // The draft lives in the app, not the table, so it survives the
            // frame.
            if let Some((_, _, draft)) = self.assign_editing.as_mut() {
                draft.clone_from(&grid.draft);
            }
            if did.cancelled {
                self.assign_editing = None;
            }
            if did.committed {
                if let Some((target, high, draft)) = self.assign_editing.take() {
                    let typed = listed
                        .iter()
                        .find(|row| row.target == target)
                        .and_then(|row| row.travel.as_ref())
                        .and_then(|travel| {
                            let value = match self.catalog.as_ref() {
                                Some(catalog) => catalog.parse(&travel.param, draft.trim()),
                                None => draft.trim().parse().ok(),
                            }?;
                            Some(value.clamp(*travel.range.start(), *travel.range.end()))
                        });
                    if let Some(value) = typed {
                        self.move_travel(position, target, high, value);
                    }
                }
            }
        }
        if let Some((row, _)) = did.chose {
            if let Some(row) = listed.get(row) {
                self.assign_action(position, row.target, AssignAction::To(None));
            }
        }

        let switches = listed
            .iter()
            .filter_map(|row| match row.source {
                hx_proto::rpc::Source::Footswitch(n) => Some(n),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        self.switch_settings(ui, switches, tint);
    }

    /// The footswitches this block's assignments land on, with the settings
    /// that belong to the switch rather than to what it drives.
    ///
    /// A switch is four things: what it carries, what is written under it, what
    /// colour it lights and whether it holds or toggles. Only the first is a
    /// choice, and the assign menu is where a choice is made. The other three
    /// are adjustments to something that already exists, which is what this
    /// panel is for - they were in that popup because that is where the code
    /// was, and it took two right-clicks and a bypass to find them.
    fn switch_settings(
        &mut self,
        ui: &mut egui::Ui,
        switches: std::collections::BTreeSet<u8>,
        tint: egui::Color32,
    ) {
        let views: Vec<SwitchView> = switches
            .into_iter()
            .filter_map(|switch| self.switch_view(switch, tint))
            .collect();
        if views.is_empty() {
            return;
        }
        let colours = self
            .catalog
            .as_ref()
            .and_then(|c| c.menu(hx_catalog::FOOTSWITCH_LED))
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        let mut change = None;
        for view in &views {
            ui.add_space(6.0);
            if let Some(chose) = switch_settings(ui, view, &colours) {
                change = Some(chose);
            }
        }
        if let Some(change) = change {
            self.switch_action(change);
        }
    }

    /// Move one end of an assignment's travel.
    fn move_travel(
        &mut self,
        block: i64,
        target: hx_proto::preset::Target,
        high_end: bool,
        value: f32,
    ) {
        // 65 and 66 take a parameter index, so a bypass has no end to move.
        let hx_proto::preset::Target::Param(param) = target else {
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
            value,
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
        let carried = self.carrying_switch(position);
        let menu = self.assign_view(position, Target::Bypass, "On/Off".to_owned());
        BypassView {
            position,
            enabled: block.is_some_and(|b| b.enabled),
            lit: carried
                .as_ref()
                .map(|s| self.led_colour(s.lit()))
                .or_else(|| block.map(|b| self.block_colour(b)))
                .unwrap_or(theme::ACCENT),
            driven: menu.under.map(|source| source.short()),
            tint: block.map(|b| self.block_colour(b)).unwrap_or(theme::ACCENT),
            on_a_switch: carried.is_some(),
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

    /// The footswitch carrying this block's bypass, if one is.
    fn carrying_switch(&self, block: i64) -> Option<hx_usb::Switch> {
        self.switches
            .iter()
            .find(|s| s.carries.iter().any(|c| c.block == block))
            .cloned()
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
            None => theme::DIM,
        }
    }

    /// Inputs, outputs, splits and joins: routing, and their own parameters.
    ///
    /// Resolved by slot *kind*, never by model number. An endpoint reports
    /// model 0, and 0 is a real entry in the symbol table - a Cali 400 - so
    /// looking it up put an amp's name and knobs on the input block.
    fn endpoint_editor(&mut self, ui: &mut egui::Ui, block: &session::Block) {
        let Some(model) = self.slot_model(block).cloned() else {
            ui.add_space(8.0);
            ui.label(RichText::new("nothing to edit here").color(theme::DIM));
            return;
        };
        ui.add_space(8.0);
        let art = self.artwork(block);
        self.pedal(
            ui,
            &model,
            &block.values.clone(),
            block.position,
            false,
            art.as_ref(),
        );
    }

    /// The colour HX Edit gives this block's category.
    ///
    /// Effects only. An endpoint reports model 0, which is a real amp in the
    /// symbol table, so resolving it painted the input and output in the amp
    /// category's red.
    fn block_colour(&self, block: &session::Block) -> egui::Color32 {
        let fallback = theme::DIM;
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
            .map(|c| theme::category_colour(c.colour))
            .unwrap_or(fallback)
    }

    /// The shelf: swap the selected block for another.
    ///
    /// Swapping only. Adding is done at the gap it goes into - see
    /// [`Self::insert_picker`] - because choosing a pedal in a panel on the
    /// far side of the window, after arming a mode there, was a lot of
    /// ceremony for "put a delay here".
    fn shelf(&mut self, root_ui: &mut egui::Ui) {
        let Some(block) = self.chain.get(self.selected).cloned() else {
            return;
        };
        // On a preset with nothing in it there is no block to swap, but the
        // obvious thing to want is a pedal - so the shelf adds instead.
        let empty = !self.chain.iter().any(|b| self.is_effect(b));
        if !(self.is_effect(&block) || empty) {
            return;
        }

        let heading = if empty { "ADD A BLOCK" } else { "SWAP FOR" };
        let current = self
            .catalog
            .as_ref()
            .and_then(|c| c.model_number(block.model))
            .map(|m| m.id.clone());

        if !self.shelf_open {
            let mut reopen = false;
            egui::Panel::right("shelf-collapsed")
                .resizable(false)
                .exact_size(32.0)
                .show(root_ui, |ui| {
                    ui.add_space(6.0);
                    reopen = ui
                        .add_sized(
                            [ui.available_width(), 26.0],
                            egui::Button::new("‹").frame(false),
                        )
                        .on_hover_text("show the model browser")
                        .clicked();
                });
            if reopen {
                self.shelf_open = true;
            }
            return;
        }

        let mut picked = None;
        egui::Panel::right("shelf")
            .default_size(430.0)
            .size_range(300.0..=620.0)
            .show(root_ui, |ui| {
                let App {
                    catalog,
                    search,
                    browsing,
                    browsing_shelf,
                    shelf_open,
                    ..
                } = self;
                let Some(catalog) = catalog.as_ref() else {
                    return;
                };
                ui.add_space(6.0);
                picked = model_picker(
                    ui,
                    catalog,
                    Browsing {
                        search,
                        category: browsing,
                        shelf: browsing_shelf,
                    },
                    Holding {
                        model: current.as_deref(),
                        paired: block.paired.is_some(),
                    },
                    PickerChrome {
                        heading,
                        focus_search: false,
                        open: Some(shelf_open),
                    },
                );
            });

        if let Some(Picked { model, paired }) = picked {
            if empty {
                // The first slot the signal reaches that is free.
                let at = self
                    .layout
                    .paths
                    .first()
                    .and_then(|p| p.input)
                    .map(|i| i + 1)
                    .unwrap_or(1);
                self.edit(Cmd::InsertBlock { at, model, paired });
            } else {
                self.edit(Cmd::SetModel {
                    block: block.position,
                    model,
                    paired,
                });
            }
        }
    }

    /// Choose a pedal for the gap you clicked, where you clicked it.
    ///
    /// Opens focused with the search field live, so the fastest way to add a
    /// delay is to click the gap and type "del". Escape closes it. Everything
    /// happens in one place: the previous flow put a menu on the gap, a mode
    /// on a panel across the window, and the actual choosing a third place
    /// again, which is why it never felt like it worked.
    fn insert_picker(&mut self, ctx: &egui::Context) {
        let (Some(at), Some(pos)) = (self.inserting_at, self.insert_pos) else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_picker();
            return;
        }

        let mut picked = None;
        let area = egui::Area::new(egui::Id::new("insert-picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_width(520.0);
                        ui.set_height(430.0);
                        let App {
                            catalog,
                            search,
                            browsing,
                            browsing_shelf,
                            ..
                        } = self;
                        let Some(catalog) = catalog.as_ref() else {
                            return;
                        };
                        picked = model_picker(
                            ui,
                            catalog,
                            Browsing {
                                search,
                                category: browsing,
                                shelf: browsing_shelf,
                            },
                            Holding::default(),
                            PickerChrome {
                                heading: "ADD A BLOCK",
                                focus_search: true,
                                open: None,
                            },
                        );
                    });
            });

        // Clicking anywhere else means "not that after all" - but not the
        // click that opened it, which egui still reports this frame, and which
        // landed on the gap rather than inside the popup. A moment's grace is
        // more reliable than a frame counter here, because egui may run
        // several passes for one frame.
        let settled = self
            .insert_opened
            .is_some_and(|t| t.elapsed() > Duration::from_millis(250));
        let outside = ctx.input(|i| {
            i.pointer.any_click()
                && !i
                    .pointer
                    .interact_pos()
                    .is_some_and(|p| area.response.rect.contains(p))
        });
        if settled && outside {
            self.close_picker();
            return;
        }
        if let Some(Picked { model, paired }) = picked {
            self.close_picker();
            self.edit(Cmd::InsertBlock { at, model, paired });
        }
    }

    fn close_picker(&mut self) {
        self.inserting_at = None;
        self.insert_pos = None;
        self.insert_opened = None;
        self.search.clear();
    }

    /// Where an Input or Main L/R block is routed.
    ///
    /// Editable via opcode 42, captured from HX Edit's own routing clicks - a
    /// document write is accepted but ignored for this field. Returns the
    /// chosen destination, so the caller can send it once the catalog borrow
    /// has ended.
    fn routing_menu(
        &self,
        ui: &mut egui::Ui,
        model: &hx_catalog::Model,
        position: i64,
    ) -> Option<i64> {
        let current = self
            .chain
            .iter()
            .find(|b| b.position == position)
            .and_then(|b| b.routing)?;
        let catalog = self.catalog.as_ref()?;
        let param = model
            .params
            .iter()
            .find(|p| p.id == "@input" || p.id == "@output")?;
        let choices = catalog.choices(param)?;

        let mut chosen = None;
        let showing = choices
            .get(current.max(0) as usize)
            .cloned()
            .unwrap_or_else(|| current.to_string());

        ui.horizontal(|ui| {
            ui.label(RichText::new(&param.name).small().color(theme::DIM));
            egui::ComboBox::from_id_salt(("routing", position))
                .selected_text(RichText::new(showing).color(theme::ACCENT))
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (index, label) in choices.iter().enumerate() {
                        if ui
                            .selectable_label(index as i64 == current, label)
                            .clicked()
                        {
                            chosen = Some(index as i64);
                        }
                    }
                });
        });
        ui.add_space(4.0);
        chosen.filter(|to| *to != current)
    }

    /// How a split divides the signal, as a row of chips - the defining
    /// choice for the block, in the same place an endpoint offers its
    /// routing. Returns the model number of a newly chosen type.
    ///
    /// Changing type is an ordinary model change on the split's slot; the
    /// attach points and the branch survive it (verified on hardware), the
    /// knobs below re-render for the new type, and undo steps back through it.
    fn split_type_menu(&self, ui: &mut egui::Ui, position: i64) -> Option<u32> {
        let block = self.chain.iter().find(|b| b.position == position)?;
        if block.kind != hx_proto::preset::Kind::Split {
            return None;
        }
        let catalog = self.catalog.as_ref()?;
        let current = catalog.model_number(block.model)?.id.clone();
        let family = catalog.models_in(catalog.category_of(&current)?);

        let mut picked = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Type").small().color(theme::DIM));
            for model in family {
                let name = model.name.strip_prefix("Split ").unwrap_or(&model.name);
                let on = model.id == current;
                // The split types are named, not pictured: they are three
                // variations on one thing, and HX Edit gives the category a
                // single glyph that would say the same on all three.
                let chip = theme::category_chip(ui, name, None, theme::ACCENT, on)
                    .on_hover_text(split_type_hint(&model.name));
                if chip.clicked() && !on {
                    picked = number_of(catalog, &model.id);
                }
            }
        });
        ui.add_space(4.0);
        picked
    }

    /// The selected block drawn as a pedal: its artwork, then its controls as
    /// knobs beneath, the way Logic's Pedalboard and the hardware itself do.
    /// Used for both halves of an Amp+Cab block, so the model is passed in
    /// rather than read off the block.
    fn pedal(
        &mut self,
        ui: &mut egui::Ui,
        model: &hx_catalog::Model,
        values: &[f32],
        position: i64,
        paired: bool,
        art: Option<&theme::Art>,
    ) {
        let Some(catalog) = self.catalog.as_ref() else {
            for (i, value) in values.iter().enumerate() {
                ui.label(format!("{i}: {value}"));
            }
            return;
        };

        let mut edit = None;
        let mut assign: Option<(i64, AssignAction)> = None;
        // The pedal, at a size worth looking at. This is the thing being
        // worked on, so it gets the room; the shelf next door is deliberately
        // smaller.
        // What kind of split this is goes with the block's name, not below its
        // knobs: it is what the block *is*, and it was the one control you had
        // to scroll past the picture to reach.
        let retype = self.split_type_menu(ui, position);
        ui.vertical_centered(|ui| {
            if let Some(art) = art {
                theme::pedal_image(ui, art, 240.0);
            }
            ui.add_space(4.0);
            ui.label(RichText::new(&model.name).heading());
        });
        ui.add_space(10.0);
        let reroute = self.routing_menu(ui, model, position);

        // Values arrive in the order the device indexes them, which the catalog
        // knows how to reproduce - it is not simply the model's parameter list.
        // An input's list starts with `@input`, which carries no value, and
        // using it directly shifted every knob by one.
        let params = catalog.ordered_params(model);

        // Knobs sit in rows under the pedal like the face of one, every row
        // starting at the same left edge so the columns line up - a wrapped
        // row that started at the margin made twelve knobs look scattered.
        let cell = processor::CONTROL_CELL;
        // The bypass is a control like any other and leads them: it is the
        // first thing you reach for on a real pedal, and putting it on a line
        // of its own said it was a different kind of thing, which was the whole
        // problem with the row of buttons it replaced. `None` is the bypass.
        let cells: Vec<Option<(usize, f32)>> = self
            .is_effect_at(position)
            .then_some(None)
            .into_iter()
            .chain(values.iter().copied().enumerate().map(Some))
            .collect();
        let (columns, indent) = processor::control_grid(ui, cells.len());
        let draft = self.param_draft.clone();
        let mut set_draft: Option<Option<(i64, i64, String)>> = None;
        let bypass = self.bypass_view(position);
        // Every knob's menu, gathered before the knobs draw: the grid below is
        // laid out holding a borrow of the catalog, and a lookup on `self`
        // cannot fight a borrow.
        let menus: Vec<AssignMenu> = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                self.assign_view(
                    position,
                    hx_proto::preset::Target::Param(index as i64),
                    param.name.clone(),
                )
            })
            .collect();
        let mut bypassed = None;
        // Which control a pick landed on, if the header's button is armed.
        let mut pick: Option<usize> = None;
        let mut pick_bypass = false;
        let catalog = self.catalog.as_ref().expect("checked above");
        let params = catalog.ordered_params(model);
        for row in cells.chunks(columns) {
            // Cells with an assignment badge or a wrapped name are taller.
            // Pin every cell to the top of the row so those extras extend
            // downward instead of shifting the knob and value upward.
            ui.horizontal_top(|ui| {
                ui.add_space(indent);
                for slot in row {
                    let Some((index, value)) = *slot else {
                        // The on/off switch is a control, so a pick lands on it
                        // like any other.
                        let before = ui.cursor().min;
                        if let Some(chose) = bypass_cell(ui, cell, &bypass) {
                            bypassed = Some(chose);
                        }
                        if self.assigning == Some(position) {
                            let rect = egui::Rect::from_min_size(before, cell);
                            ui.painter().rect_stroke(
                                rect.shrink(1.0),
                                egui::CornerRadius::same(4),
                                egui::Stroke::new(1.0_f32, theme::ACCENT),
                                egui::StrokeKind::Middle,
                            );
                            if ui
                                .put(rect, egui::Button::new("").frame(false))
                                .on_hover_text("assign the on/off switch")
                                .clicked()
                            {
                                pick_bypass = true;
                            }
                        }
                        continue;
                    };
                    let Some(param) = params.get(index).copied() else {
                        continue;
                    };
                    let mut current = value;

                    // Every part of a control opens its assignment menu, not
                    // only its name: a person right-clicks the knob, because
                    // the knob is the control. Collected as they are drawn and
                    // hooked up after, because the menu needs the whole cell's
                    // worth of state.
                    let mut parts: Vec<egui::Response> = Vec::new();
                    let picking = self.assigning == Some(position);
                    let drawn = ui.allocate_ui(cell, |ui| {
                        ui.vertical_centered(|ui| {
                            let mut changed = false;
                            match (param.kind, catalog.choices(param)) {
                                (Kind::Switch, _) => {
                                    let mut on = current >= 0.5;
                                    let hit = ui.add(theme::switch(&mut on));
                                    changed = hit.changed();
                                    parts.push(hit);
                                    current = on as u8 as f32;
                                    ui.label(
                                        RichText::new(catalog.format(param, current))
                                            .monospace()
                                            .color(theme::ACCENT),
                                    );
                                }
                                // Only a catalog entry with actual labels is a
                                // menu. `valueType: integer` also covers stepped
                                // numbers such as Pitch Wham's -24..+24 range;
                                // drawing those as a ComboBox produced an empty
                                // popup because there were no choices to list.
                                (Kind::Enum, Some(choices)) => {
                                    let choice = current.round().max(0.0) as usize;
                                    ui.add_space(11.0);
                                    egui::ComboBox::from_id_salt((
                                        "param", position, index, paired,
                                    ))
                                    .width(cell.x)
                                    .selected_text(
                                        RichText::new(catalog.format(param, current))
                                            .color(theme::ACCENT),
                                    )
                                    .show_ui(ui, |ui| {
                                        for (n, label) in choices.iter().enumerate() {
                                            if ui
                                                .selectable_label(n == choice, label)
                                                .clicked()
                                                && n != choice
                                            {
                                                current = n as f32;
                                                changed = true;
                                            }
                                        }
                                    });
                                    ui.add_space(11.0);
                                }
                                _ => {
                                    let hit =
                                        theme::knob(ui, &mut current, param.min..=param.max);
                                    changed = hit.changed();
                                    // Double-click puts the factory default
                                    // back, the way every DAW knob does.
                                    if hit.double_clicked() {
                                        current = param.default;
                                        changed = true;
                                    }
                                    parts.push(hit.clone().on_hover_text(
                                        "drag to turn; Shift-drag for fine adjustment\ndouble-click to reset; click the value to type it\nright-click to assign a control",
                                    ));
                                    match &draft {
                                        Some((block, i, text))
                                            if *block == position && *i == index as i64 =>
                                        {
                                            let mut text = text.clone();
                                            let field = ui.add(
                                                egui::TextEdit::singleline(&mut text)
                                                    .desired_width(64.0)
                                                    .font(egui::TextStyle::Monospace),
                                            );
                                            if !field.has_focus() && !field.lost_focus() {
                                                field.request_focus();
                                            }
                                            if field.lost_focus() {
                                                if ui.input(|inp| {
                                                    inp.key_pressed(egui::Key::Enter)
                                                }) {
                                                    if let Some(typed) =
                                                        catalog.parse(param, &text)
                                                    {
                                                        current = typed
                                                            .clamp(param.min, param.max);
                                                        changed = true;
                                                    }
                                                }
                                                set_draft = Some(None);
                                            } else {
                                                set_draft =
                                                    Some(Some((position, index as i64, text)));
                                            }
                                        }
                                        _ => {
                                            let shown = ui.add(
                                                egui::Label::new(
                                                    RichText::new(
                                                        catalog.format(param, current),
                                                    )
                                                    .monospace()
                                                    .color(theme::ACCENT),
                                                )
                                                .sense(egui::Sense::click()),
                                            );
                                            let shown = shown
                                                .on_hover_text("click to type a value");
                                            if shown.clicked() {
                                                set_draft = Some(Some((
                                                    position,
                                                    index as i64,
                                                    catalog.format(param, current),
                                                )));
                                            }
                                            parts.push(shown);
                                        }
                                    }
                                }
                            }
                            // What controls this knob, if anything, said the
                            // same way the chain says it: the source's own
                            // badge, under the name, in the block's colour. An
                            // assignment you cannot see is an assignment you
                            // will be surprised by on stage, and a bare dot
                            // told you only that there was one.
                            let menu = &menus[index];
                            let label = match menu.under {
                                Some(_) => RichText::new(&param.name).color(theme::ACCENT),
                                None => RichText::new(&param.name).color(theme::DIM),
                            };
                            let name = ui.add(
                                egui::Label::new(label).sense(egui::Sense::click()),
                            );
                            if let Some(source) = menu.under {
                                parts.push(theme::tag(
                                    ui,
                                    &source.short(),
                                    bypass.tint,
                                ));
                            }
                            let name = match menu.under {
                                Some(source) => name.on_hover_text(format!(
                                    "{} controls this\nclick to change",
                                    source.label(),
                                )),
                                None => name.on_hover_text(
                                    "click to put this under a pedal or a switch",
                                ),
                            };
                            // Left-click as well as right: the menu is the only
                            // way to reach this, so it should not be hidden
                            // behind the gesture people try second.
                            if name.clicked() {
                                egui::Popup::toggle_id(ui.ctx(), popup_id(position, index));
                            }
                            parts.push(name.clone());
                            for part in &parts {
                                part.context_menu(|ui| {
                                    if let Some(chose) = assign_menu(ui, menu) {
                                        assign = Some((index as i64, chose));
                                    }
                                });
                            }
                            egui::Popup::from_response(&name)
                                .id(popup_id(position, index))
                                .open_memory(None)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                                .show(|ui| {
                                    if let Some(chose) = assign_menu(ui, menu) {
                                        assign = Some((index as i64, chose));
                                    }
                                });
                            if changed {
                                // Integer parameters rendered as knobs move in
                                // whole native steps. Named menus already return
                                // an integer index, so this is harmless there too.
                                if param.kind == Kind::Enum {
                                    current = current.round();
                                }
                                edit = Some((index as i64, current, param.kind == Kind::Switch));
                            }
                        });
                    });
                    if picking {
                        let rect = drawn.response.rect;
                        ui.painter().rect_stroke(
                            rect.shrink(1.0),
                            egui::CornerRadius::same(4),
                            egui::Stroke::new(1.0_f32, theme::ACCENT),
                            egui::StrokeKind::Middle,
                        );
                        // On top of the control, so a pick cannot turn a knob
                        // by accident. It goes away the moment one is chosen.
                        let target = ui.put(
                            rect,
                            egui::Button::new("").frame(false).fill(
                                egui::Color32::TRANSPARENT,
                            ),
                        );
                        if target
                            .on_hover_text(format!("assign {}", param.name))
                            .clicked()
                        {
                            pick = Some(index);
                        }
                    }
                }
            });
        }
        if let Some(index) = pick {
            self.assigning = None;
            egui::Popup::open_id(ui.ctx(), popup_id(position, index));
        }
        if pick_bypass {
            self.assigning = None;
            egui::Popup::open_id(ui.ctx(), bypass_popup_id(position));
        }
        if let Some(update) = set_draft {
            self.param_draft = update;
        }
        if let Some(action) = bypassed {
            self.bypass_action(position, action);
        }
        // Once per block, not once per model: an Amp+Cab draws these controls
        // twice and there is one list of what drives the block.
        if !paired {
            self.assignment_list(ui, position);
        }

        if let Some((param, chose)) = assign {
            self.assign_action(position, hx_proto::preset::Target::Param(param), chose);
        }
        if let Some(to) = reroute {
            self.edit(Cmd::SetRouting {
                block: position,
                to,
            });
        }
        if let Some(model) = retype {
            self.edit(Cmd::SetModel {
                block: position,
                model,
                paired: None,
            });
        }
        if let Some((index, value, switch)) = edit {
            let slot = &mut self.chain[self.selected];
            let target = if paired {
                &mut slot.paired_values
            } else {
                &mut slot.values
            };
            target[index as usize] = value;
            // The cab's parameters are addressed on the same block; only which
            // half they belong to differs, and the device infers that from the
            // index range.
            self.edit(Cmd::SetParam {
                block: position,
                index,
                value,
                switch,
            });
        }
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
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, theme::TEXT));
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
                    .layout_no_wrap((*piece).to_owned(), font.clone(), theme::DIM)
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

/// Search, categories and a grid of pedals. Returns the model chosen.
///
/// A free function taking the pieces it needs rather than `&mut self`, so the
/// same widget serves the swap shelf and the insert popup - the two places you
/// choose a pedal should not look or behave differently.
/// Where the model browser is pointed: the filter typed in, the category
/// chosen, and the shelf under that category.
///
/// The three travel together because they are not independent - typing a
/// search overrides both, and choosing a category clears the shelf. Kept as
/// separate arguments they were three chances to update two of them.
struct Browsing<'a> {
    search: &'a mut String,
    category: &'a mut Option<u32>,
    shelf: &'a mut Option<String>,
}

/// What a block holds now, which decides where the picker opens and which tile
/// it marks as the current one.
#[derive(Debug, Clone, Copy, Default)]
struct Holding<'a> {
    model: Option<&'a str>,
    /// Whether a second model rides along - an Amp+Cab.
    paired: bool,
}

/// What the picker hands back: a model, and the cab that rides along with it
/// when the shelf it came from was Amp+Cab.
#[derive(Debug, Clone, Copy)]
struct Picked {
    model: u32,
    paired: Option<u32>,
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

struct PickerChrome<'a> {
    heading: &'a str,
    focus_search: bool,
    open: Option<&'a mut bool>,
}

fn model_picker(
    ui: &mut egui::Ui,
    catalog: &hx_catalog::Catalog,
    at: Browsing,
    holding: Holding,
    chrome: PickerChrome<'_>,
) -> Option<Picked> {
    let Browsing {
        search,
        category: browsing,
        shelf,
    } = at;
    let PickerChrome {
        heading,
        focus_search,
        open,
    } = chrome;
    ui.horizontal(|ui| {
        ui.label(RichText::new(heading).small().color(theme::DIM));
        let collapse_width = if open.is_some() { 24.0 } else { 0.0 };
        let field = ui.add(
            egui::TextEdit::singleline(search)
                .hint_text("Search pedals")
                .desired_width((ui.available_width() - collapse_width).max(80.0)),
        );
        // Typing is the fastest way to find one of several hundred, so the
        // popup opens ready for it.
        if focus_search && !field.has_focus() {
            field.request_focus();
        }
        if let Some(open) = open {
            if ui
                .small_button("›")
                .on_hover_text("hide the model browser")
                .clicked()
            {
                *open = false;
            }
        }
    });
    ui.add_space(4.0);

    let searching = !search.is_empty();
    // With no category explicitly chosen, show the one the current block is
    // already in - not the first category. Otherwise swapping an amp snapped
    // the browser back to Distortion every time.
    let showing = browsing.unwrap_or_else(|| {
        // A block that already holds a pair browses as Amp+Cab, which is where
        // its own model lives as far as the person looking at it is concerned.
        if holding.paired {
            return hx_catalog::Category::AMP_CAB;
        }
        holding
            .model
            .and_then(|id| catalog.category_of(id))
            .unwrap_or(1)
    });
    let categories: Vec<&hx_catalog::Category> = catalog
        .categories()
        .iter()
        .filter(|category| category.is_effect() && !catalog.models_in(category.id).is_empty())
        .collect();

    // Whether what is on screen fills a block with two models. A search cuts
    // across categories, so it can only offer single models.
    let pairing = !searching && catalog.category(showing).is_some_and(|c| c.paired);

    let models: Vec<&hx_catalog::Model> = if searching {
        let needle = search.to_lowercase();
        catalog
            .models()
            .filter(|m| m.name.to_lowercase().contains(&needle))
            .filter(|m| {
                catalog
                    .category_of(&m.id)
                    .and_then(|c| catalog.category(c))
                    .is_some_and(|c| c.is_effect())
            })
            .collect()
    } else {
        catalog.models_in(showing)
    };

    // HX Edit shelves a category - Mono, Stereo, Legacy on the effects, Guitar
    // and Bass on the amps, Single and Dual on the cabs - and those shelves are
    // how people talk about the models. A search cuts across them, so it stays
    // one flat list.
    let shelves: Vec<(&str, Vec<&hx_catalog::Model>)> = if searching {
        Vec::new()
    } else {
        catalog
            .category(showing)
            .map(|c| {
                c.subcategories
                    .iter()
                    .map(|sub| {
                        let models = sub
                            .models
                            .iter()
                            .filter_map(|id| catalog.model(id))
                            .collect::<Vec<_>>();
                        (sub.name.as_str(), models)
                    })
                    .filter(|(_, models)| !models.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    // A category with no second level shows flat, as does a search, which cuts
    // across shelves by definition. Amp+Cab does have one - Guitar and Bass,
    // inherited from the Amp category and narrowed to the amps that come with a
    // cab - and pairing changes only what a pick sends, never how the shelf is
    // laid out.
    let shelved = shelves.len() > 1;

    // Which shelf is open. The stored name is checked against this category's
    // own shelves rather than trusted, which is what makes changing category
    // reset it; failing that, the shelf holding the block's current model, so
    // swapping a stereo delay opens on Stereo rather than on Mono.
    let open = if shelved {
        let chosen = shelf
            .as_deref()
            .and_then(|want| shelves.iter().position(|(name, _)| *name == want));
        let holds_current = || {
            let held = holding.model?;
            shelves
                .iter()
                .position(|(_, models)| models.iter().any(|m| m.id == held))
        };
        chosen.or_else(holds_current).unwrap_or(0)
    } else {
        0
    };

    // One shelf at a time when there are shelves; everything otherwise.
    let models: Vec<&hx_catalog::Model> = if shelved {
        shelves[open].1.clone()
    } else {
        models
    };

    // Below this width a permanent rail would leave only one narrow model
    // column. Collapse the same category vocabulary into one compact menu
    // instead. This applies equally to swapping and inserting: there is one
    // model browser, responsive to the room it has.
    if ui.available_width() < 400.0 {
        ui.horizontal(|ui| {
            ui.label(RichText::new("CATEGORY").small().color(theme::DIM));
            egui::ComboBox::from_id_salt("model-category")
                .selected_text(if searching {
                    "All search results"
                } else {
                    catalog
                        .category(showing)
                        .map_or("Models", |category| category.name.as_str())
                })
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for category in &categories {
                        if ui
                            .selectable_label(!searching && category.id == showing, &category.name)
                            .clicked()
                        {
                            *browsing = Some(category.id);
                            *shelf = None;
                            search.clear();
                        }
                    }
                });
        });
        if shelved {
            ui.add_space(4.0);
            picker_shelves(ui, &shelves, open, shelf);
        }
        ui.separator();
        picker_models(ui, catalog, &models, holding, pairing, true)
    } else {
        let body = ui.available_size();
        let mut picked = None;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(118.0, body.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    egui::ScrollArea::vertical()
                        .id_salt("model-category-rail")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for category in &categories {
                                let colour = theme::category_colour(category.colour);
                                let on = !searching && category.id == showing;
                                let icon = picker_category_icon(catalog, category);
                                if theme::category_rail_row(
                                    ui,
                                    &category.name,
                                    icon.as_ref(),
                                    colour,
                                    on,
                                )
                                .clicked()
                                {
                                    *browsing = Some(category.id);
                                    *shelf = None;
                                    search.clear();
                                }
                            }
                        });
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                ui.available_size(),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let heading = if searching {
                            RichText::new("Search results").color(theme::DIM)
                        } else {
                            let category = catalog.category(showing);
                            RichText::new(category.map_or("Models", |c| c.name.as_str())).color(
                                category.map_or(theme::TEXT, |c| theme::category_colour(c.colour)),
                            )
                        };
                        ui.label(heading.strong());
                        if shelved {
                            for (i, (name, shelf_models)) in shelves.iter().enumerate() {
                                if theme::shelf_pill(ui, name, i == open)
                                    .on_hover_text(format!("{} models", shelf_models.len()))
                                    .clicked()
                                {
                                    *shelf = Some((*name).to_owned());
                                }
                            }
                        }
                    });
                    ui.separator();
                    picked = picker_models(ui, catalog, &models, holding, pairing, true);
                },
            );
        });
        picked
    }
}

fn picker_category_icon(
    catalog: &hx_catalog::Catalog,
    category: &hx_catalog::Category,
) -> Option<theme::Art> {
    // Ours first, HX Edit's only where we have not drawn one.
    theme::category_icon(&category.name).or_else(|| {
        catalog.category_artwork(category).map(|(path, frames)| {
            let uri = format!("file://{}", path.display());
            match frames {
                0 | 1 => theme::Art::whole(uri),
                n => theme::Art::strip(uri, 0, n),
            }
        })
    })
}

fn picker_shelves(
    ui: &mut egui::Ui,
    shelves: &[(&str, Vec<&hx_catalog::Model>)],
    open: usize,
    shelf: &mut Option<String>,
) {
    ui.horizontal_wrapped(|ui| {
        for (i, (name, models)) in shelves.iter().enumerate() {
            if theme::shelf_pill(ui, name, i == open)
                .on_hover_text(format!("{} models", models.len()))
                .clicked()
            {
                *shelf = Some((*name).to_owned());
            }
        }
    });
}

fn picker_models(
    ui: &mut egui::Ui,
    catalog: &hx_catalog::Catalog,
    models: &[&hx_catalog::Model],
    holding: Holding,
    pairing: bool,
    compact: bool,
) -> Option<Picked> {
    let mut picked = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if models.is_empty() {
                ui.label(RichText::new("Nothing matches").color(theme::DIM));
                return;
            }

            let size = if compact {
                let available = ui.available_width();
                let gap = ui.spacing().item_spacing.x;
                // Add Block opens wide enough for three models. The dock grows
                // into the same third column as it is resized, but never makes
                // the thumbnails too narrow just to squeeze one more in.
                let columns =
                    (((available + gap) / (112.0 + gap)).floor() as usize).clamp(1, 3) as f32;
                let width = ((available - gap * (columns - 1.0)) / columns)
                    .floor()
                    .clamp(112.0, 156.0)
                    .min(available);
                // Browser tiles only have a name below the art. Unlike signal
                // chain blocks, they do not reserve a second category line.
                egui::vec2(width, (width * 0.84).clamp(102.0, 132.0))
            } else {
                egui::vec2(156.0, 132.0)
            };

            ui.horizontal_wrapped(|ui| {
                for model in models {
                    let selected = holding.model == Some(model.id.as_str());
                    let colour = catalog
                        .category_of(&model.id)
                        .and_then(|id| catalog.category(id))
                        .map(|category| theme::category_colour(category.colour))
                        .unwrap_or(theme::ACCENT);
                    let art = catalog
                        .artwork(model)
                        .map(|p| theme::Art::whole(format!("file://{}", p.display())));
                    if theme::model_tile(ui, &model.name, art.as_ref(), selected, colour, size)
                        .clicked()
                    {
                        // Only models the firmware knows by number can be sent.
                        picked = number_of(catalog, &model.id).map(|model_number| Picked {
                            model: model_number,
                            // In a paired category the cab comes with the amp;
                            // if the firmware does not know that cab by number
                            // the amp still goes in, alone.
                            paired: pairing
                                .then(|| catalog.paired_cab(model))
                                .flatten()
                                .and_then(|cab| number_of(catalog, &cab.id)),
                        });
                    }
                }
            });
        });
    picked
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
            .color(if off { theme::DIM } else { colour }),
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
    switch: u8,
    /// The name in the field, which is the draft while one is being typed.
    label: String,
    /// What the pedal writes under it when no name has been typed: the first
    /// thing it carries. Shown as the field's hint, so an empty field says what
    /// empty means rather than looking like a missing setting.
    carries: Option<String>,
    colour: Option<i64>,
    /// What it is lighting right now, which under Auto Color is the colour of
    /// whatever it carries. A word for a colour is worth less than the colour.
    lit: egui::Color32,
    momentary: bool,
    /// The colour of the block these settings were reached through, so the
    /// switch's badge matches the one on that block in the chain.
    tint: egui::Color32,
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

/// The two travel columns of the assignments table, by index. Named because
/// they are read back in three places and adding the CC column beside Source
/// moved both of them.
const LOW_END: usize = 3;
const HIGH_END: usize = 4;

/// The one assignment menu, for a knob and for a block's on/off alike.
///
/// It chooses what drives a control and nothing else. Everything that adjusts
/// an assignment already made - which CC reaches it, where its two ends sit,
/// what the switch carrying it is called - is in the ASSIGNMENTS panel, where
/// you can see it without opening anything.
fn assign_menu(ui: &mut egui::Ui, menu: &AssignMenu) -> Option<AssignAction> {
    ui.set_min_width(230.0);
    ui.label(
        RichText::new(format!("Control {} with", menu.name))
            .small()
            .color(theme::DIM),
    );
    let mut action = None;
    if ui
        .selectable_label(menu.under.is_none(), "Nothing")
        .clicked()
    {
        action = Some(AssignAction::To(None));
        ui.close();
    }
    for (source, carries) in &menu.sources {
        let on = menu.under == Some(*source);
        // Say what a source is already busy with, so a footswitch is not
        // quietly given a second job the night you find out about it.
        let label = match carries.first() {
            Some(what) if !on => format!("{}   carries {what}", source.label()),
            _ => source.label(),
        };
        if ui.selectable_label(on, label).clicked() {
            action = Some(AssignAction::To((!on).then_some(*source)));
            ui.close();
        }
    }
    action
}

/// One footswitch's own settings, on one line under the table.
///
/// The switch itself, rather than what it carries: what is written under it on
/// the pedal, what colour it lights, and what your foot gets for pressing it.
/// The pedal has had opcodes for all three since the protocol was mapped, and
/// for a long time nothing to press them with.
///
/// Every control says what it is in the words a person would use standing in
/// front of the pedal. It read `[what it carries] [Auto Color] (o) Momentary`
/// for a while, which is three settings none of which says what it is.
fn switch_settings(
    ui: &mut egui::Ui,
    view: &SwitchView,
    colours: &[String],
) -> Option<SwitchChange> {
    use session::SwitchEdit;
    let mut change = None;
    let set = |edit| {
        Some(SwitchChange::Set {
            switch: view.switch,
            edit,
        })
    };
    let dim = |ui: &mut egui::Ui, text: &str| ui.label(RichText::new(text).color(theme::DIM));

    // Wrapped rather than laid out in columns: the panel is as wide as the
    // window leaves it, and controls that run off the edge of a narrow one are
    // worse than controls that take two lines.
    ui.horizontal_wrapped(|ui| {
        theme::tag(ui, &format!("FS{}", view.switch), view.tint)
            .on_hover_text("this footswitch, and what it is like to use");

        dim(ui, "  Name");
        let mut text = view.label.clone();
        let field = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(110.0)
                // Empty, the pedal writes what the switch carries under it. So
                // that is what the empty field shows, greyed: the hint is not a
                // suggestion, it is what you will get.
                .hint_text(view.carries.clone().unwrap_or_default()),
        );
        if field.changed() {
            change = Some(SwitchChange::Typing(view.switch, text.clone()));
        }
        // Committed on Enter or on leaving, like every other field here. An
        // empty name is not a name: it clears back to what the switch carries,
        // which is the same thing opcode 60 does.
        if field.lost_focus() {
            let typed = text.trim();
            change = set(SwitchEdit::Label(
                (!typed.is_empty()).then(|| typed.to_owned()),
            ));
        }
        field.on_hover_text("what the pedal writes under this switch\nempty: whatever it carries");

        if !colours.is_empty() {
            dim(ui, "  Light");
            // Auto Color is index 0 of HX Edit's list and `None` here, because
            // the protocol reaches it by an opcode of its own rather than a
            // value.
            let chosen = view.colour.unwrap_or(0);
            let showing = colours
                .get(chosen.max(0) as usize)
                .cloned()
                .unwrap_or_else(|| format!("Colour {chosen}"));
            // The colour it is lighting, beside the name of it. Under Auto
            // Color the name says nothing at all - the switch takes the colour
            // of what it carries - and this is that colour.
            theme::led_dot(ui, view.lit);
            egui::ComboBox::from_id_salt(("switch-colour", view.switch))
                .selected_text(RichText::new(&showing).color(theme::ACCENT))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for (n, name) in colours.iter().enumerate() {
                        ui.horizontal(|ui| {
                            theme::led_dot(ui, theme::led_swatch(name));
                            if ui.selectable_label(n as i64 == chosen, name).clicked() {
                                change = set(SwitchEdit::Colour((n > 0).then_some(n as i64)));
                            }
                        });
                    }
                })
                .response
                .on_hover_text("the colour this switch lights up");
        }

        dim(ui, "  Press");
        // Two words rather than a toggle called "Momentary". A toggle says
        // "momentary: on" and leaves you to work out what the other one is;
        // this says both, and one of them is lit.
        for (momentary, word, what) in [
            (false, "Toggles", "press once for on, press again for off"),
            (true, "Holds", "on only while your foot is down"),
        ] {
            if ui
                .selectable_label(view.momentary == momentary, word)
                .on_hover_text(what)
                .clicked()
                && view.momentary != momentary
            {
                change = set(SwitchEdit::Momentary(momentary));
            }
        }
    });
    change
}

/// What the bypass control shows, beyond the menu every control shares.
struct BypassView {
    position: i64,
    enabled: bool,
    /// The colour the pedal lights the switch, or the block's own until it has
    /// said.
    lit: egui::Color32,
    /// What drives it, in four characters, for the badge beside its name.
    driven: Option<String>,
    /// The block's own colour, which is what its badges are painted in.
    tint: egui::Color32,
    /// Whether a footswitch has it, which is what the switch graphic shows.
    on_a_switch: bool,
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

/// The block's bypass, drawn as the footswitch it is and sitting with the
/// block's other controls.
///
/// It used to be a tick box called "Engaged" in the header and a row of buttons
/// called "Bypass switched by" underneath: two controls and two vocabularies
/// for one thing. It is one thing. A switch is on or off, and something can be
/// assigned to drive it, so it looks like a switch, it sits where the other
/// controls are, and its name opens the same kind of popup a knob's name does.
fn bypass_cell(ui: &mut egui::Ui, cell: egui::Vec2, view: &BypassView) -> Option<BypassAction> {
    let mut action = None;
    let mut open = false;
    // Every part of the control opens its menu, the same as a knob's.
    let mut parts: Vec<egui::Response> = Vec::new();
    ui.allocate_ui(cell, |ui| {
        ui.vertical_centered(|ui| {
            let switch = theme::footswitch(ui, view.enabled, Some(view.lit), view.on_a_switch);
            let switch = switch.on_hover_text(if view.enabled {
                "on. Press to turn it off\nright-click to assign a control"
            } else {
                "off. Press to turn it on\nright-click to assign a control"
            });
            if switch.clicked() {
                action = Some(BypassAction::Toggle(!view.enabled));
            }
            parts.push(switch);
            // "On" and "Off", because that is what a guitarist calls a pedal
            // that is or is not doing anything. "Engaged" and "Bypassed" are
            // the engineer's words for the same two states.
            parts.push(
                ui.add(
                    egui::Label::new(
                        RichText::new(if view.enabled { "On" } else { "Off" })
                            .monospace()
                            .color(if view.enabled {
                                theme::ACCENT
                            } else {
                                theme::DIM
                            }),
                    )
                    .selectable(false)
                    .sense(egui::Sense::click()),
                ),
            );
            // The name is the way in to the assignment, exactly as it is for a
            // knob. It stays "On/Off" whatever drives it: a control that
            // renames itself to whatever is driving it has stopped saying what
            // it is. What drives it is the badge beside it, the same badge the
            // block wears in the chain.
            let label = ui.add(
                egui::Label::new(RichText::new("On/Off").color(match view.driven {
                    Some(_) => theme::ACCENT,
                    None => theme::DIM,
                }))
                .selectable(false)
                .sense(egui::Sense::click()),
            );
            // Under the name rather than beside it: a control cell is as wide
            // as a knob, and "On/Off MIDI" on one line runs out over its
            // neighbours.
            let tagged = view.driven.as_ref().map(|what| {
                ui.add_space(1.0);
                theme::tag(ui, what, view.tint)
            });
            let label = label.on_hover_text(match (view.menu.under, view.auto_engage) {
                // A wah does not wait to be switched on: it engages itself
                // the moment the pedal leaves its heel.
                (Some(source), true) => format!(
                    "{} engages this on its own when you move it\nclick to change",
                    source.label()
                ),
                (Some(source), false) => {
                    format!("{} switches this\nclick to change", source.label())
                }
                (None, _) => "click to put this under a footswitch or a CC".to_owned(),
            });
            // The badge is part of the control, not a decoration on it: a
            // person aiming at what drives this is aiming at the badge.
            if let Some(tagged) = tagged {
                if tagged.clicked() {
                    open = true;
                }
                parts.push(tagged);
            }
            if label.clicked() {
                open = true;
            }
            egui::Popup::from_response(&label)
                .id(bypass_popup_id(view.position))
                .open_memory(None)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    if let Some(chose) = assign_menu(ui, &view.menu) {
                        action = Some(BypassAction::Assign(chose));
                    }
                });
            parts.push(label);
            for part in &parts {
                part.context_menu(|ui| {
                    if let Some(chose) = assign_menu(ui, &view.menu) {
                        action = Some(BypassAction::Assign(chose));
                    }
                });
            }
        });
    });
    if open {
        egui::Popup::toggle_id(ui.ctx(), bypass_popup_id(view.position));
    }
    action
}

/// One row of the assignments table, gathered before it draws.
struct Row {
    target: hx_proto::preset::Target,
    /// What is driven: a parameter's name, or On/Off.
    name: String,
    /// What drives it. Kept as the source rather than its name, because what
    /// the two ends are *called* depends on it.
    source: hx_proto::rpc::Source,
    /// Which CC reaches it, when MIDI is what drives it.
    cc: Option<i64>,
    min: f32,
    max: f32,
    /// The same two, read the way that parameter reads under the knobs.
    min_text: String,
    max_text: String,
    /// The parameter the ends are values of. `None` for a bypass.
    travel: Option<Travel>,
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

/// The bypass popup's own id, distinct from any parameter's.
/// What the setlist rail shows about a setlist, and how wide each part is.
///
/// One function rather than a list written where the table is built, because
/// the panel holding that table has to know how narrow it may be dragged. A
/// floor stated separately is a floor that stops matching the day a column
/// changes width, and the failure is silent: the table just starts drawing its
/// headers over each other.
fn setlist_rail_columns() -> Vec<table::Column> {
    vec![
        // The name is the stable setlist identity. New names and new versions
        // are chosen in the capture dialog; changing one historical row would
        // otherwise split a version family in two.
        table::Column::new("Setlist", 120.0).fills(),
        table::Column::new("Ver", 42.0),
        table::Column::new("Venue", 90.0).editable(),
        table::Column::new("Date", 80.0).editable(),
        table::Column::new("#", 34.0),
    ]
}

/// Turn every gesture offered by a setlist row into the row to restore.
///
/// The first place is the pedal icon in the Push column. Keeping that path
/// explicit prevents the icon from looking actionable while only double-click
/// and the context menu are actually wired.
fn setlist_send_row(did: &table::Did) -> Option<usize> {
    did.place
        .and_then(|(row, place)| (place == 0).then_some(row))
        .or(did.double_clicked)
        .or(did.chose.map(|(row, _)| row))
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
        }];
        app.library_lookup = LibraryLookup::default();
        app.library_lookup.setlist_tones.insert(
            hash.clone(),
            SetlistTone {
                held: true,
                meta: library::Meta::default(),
                line: "Amp › Delay".into(),
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
    fn a_setlist_push_icon_restores_its_own_row() {
        let pushed = table::Did {
            place: Some((7, 0)),
            ..Default::default()
        };
        assert_eq!(setlist_send_row(&pushed), Some(7));

        let unrelated_place = table::Did {
            place: Some((7, 1)),
            ..Default::default()
        };
        assert_eq!(setlist_send_row(&unrelated_place), None);
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
