//! The model catalog: what every block model is called, and what its knobs do.
//!
//! The wire protocol identifies parameters by position and carries bare floats.
//! On its own that is unreadable - `{98: 1, 26: 0, 119: 0.78}` says nothing
//! about "Peak Reduction at 78%". The missing half is metadata, and HX Edit
//! already ships it as plain JSON.
//!
//! Those files are Line 6's, so we read the user's installed copy at runtime
//! and never redistribute them. Everything here degrades gracefully when HX
//! Edit is absent: you get a device you can still drive, just with numbers
//! instead of names.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod build;
pub mod extract;
mod format;
pub mod home;
mod hxb;
mod inspect;
mod load;
mod write;

pub use build::{documents_from_backup, empty_the_chain, resolve, slots_from_hlx, Built};
pub use format::Display;
pub use hxb::{
    read_backup, read_favourite_file, read_setlist_file, write_backup, Backup, BackupPreset, Block,
    Container, Favourite, NewBackup,
};
pub use inspect::{inspect, ChainContent, OutputTarget, Tone, ToneBlock};
pub use write::{to_hlx, to_hlx_for_device, Written};

/// The key of HX Edit's footswitch LED colour list, for [`Catalog::menu`].
/// Index 0 is Auto Color, which the protocol reaches by its own opcode.
pub const FOOTSWITCH_LED: &str = "footswitchLED";

/// Everything HX Edit knows about models and their parameters.
pub struct Catalog {
    models: HashMap<String, Model>,
    categories: Vec<Category>,
    displays: HashMap<String, Display>,
    symbols: Vec<Symbol>,
    resources: PathBuf,
}

/// A firmware symbol: the bridge between the numbers on the wire and the names
/// in the catalog.
///
/// The device identifies a model by a number and a parameter by its position.
/// `Helix.sym` is an array whose index *is* that number and whose entries carry
/// the parameters in the order the device indexes them - so it resolves both at
/// once. Nothing else in HX Edit's resources does.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The number the device uses, which is this entry's position in the file.
    pub number: u32,
    /// Firmware symbol, e.g. `HD2_ReverbRoomStereo`.
    pub symbol: String,
    /// Parameter symbolic ids, in the order the device indexes them.
    pub parameters: Vec<String>,
    /// The catalog model this resolves to, once mono/stereo variants are folded
    /// together. A handful of symbols are placeholders with no model.
    pub model: Option<String>,
}

/// One block model - an amp, a delay, a compressor.
#[derive(Debug, Clone)]
pub struct Model {
    /// Symbolic id, e.g. `HD2_CompressorLAStudioComp`. This is the identity
    /// used everywhere: catalog, preset files and `@model` fields alike.
    pub id: String,
    pub name: String,
    pub category: u32,
    pub stereo: bool,
    /// DSP cost, which the device uses to decide what still fits.
    pub load: f32,
    /// Artwork file name, e.g. `FX_HX_DIST_KinkyBoost.png`. Resolve it with
    /// [`Catalog::artwork`].
    pub image: Option<String>,
    /// The cab this model pairs with in an Amp+Cab block, if it is an amp.
    /// From `amp.models`' `cablink`; resolve it with [`Catalog::paired_cab`].
    pub cab_link: Option<String>,
    pub params: Vec<Param>,
}

/// One knob, switch or menu on a model.
#[derive(Debug, Clone)]
pub struct Param {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Key into the display table, deciding units and formatting.
    pub display: Option<String>,
}

impl Param {
    /// Whether a native value is safe to send for this parameter.
    ///
    /// External tone files carry bare floats and can therefore contain values
    /// that HX Edit would never let a person choose. Some HX hardware accepts
    /// those values and stops passing audio, so every route from a file to the
    /// device uses this same check.
    pub fn accepts(&self, value: f32) -> bool {
        value.is_finite() && value >= self.min && value <= self.max
    }
}

/// What sort of value a parameter holds.
///
/// Mirrors the catalog's `valueType`, which is an integer we would otherwise be
/// matching on at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A menu: discrete numbered choices.
    Enum,
    /// A knob: continuous between `min` and `max`.
    Continuous,
    /// A switch.
    Switch,
    /// Free text. Rare - three parameters in the whole catalog.
    Text,
}

/// A group in the model browser, matching HX Edit's own left-hand list.
#[derive(Debug, Clone)]
pub struct Category {
    pub id: u32,
    pub name: String,
    /// Abbreviation HX Edit uses where space is tight - "Dist", "Verb".
    pub short_name: String,
    /// The colour HX Edit tints this category's blocks, as `0xRRGGBB`.
    /// Taken from the catalog rather than invented, so a chain drawn here
    /// looks like the same chain drawn there.
    pub colour: u32,
    /// Icon file name, e.g. `FX_HX_Category_Amp.png`. Resolve it with
    /// [`Catalog::category_artwork`].
    pub image: Option<String>,
    /// Whether choosing from this category fills a block with *two* models -
    /// true only of Amp+Cab. Such a category re-lists models that belong to
    /// another one, so [`Catalog::category_of`] steps over it.
    pub paired: bool,
    /// Model ids in the order HX Edit lists them, flattened across
    /// subcategories.
    pub models: Vec<String>,
    /// The same models split into HX Edit's shelves - Mono / Stereo / Legacy on
    /// the effects, Guitar / Bass on amps, Single / Dual on cabs. Empty for a
    /// category with no second level. `models` stays the flat union of these, so
    /// nothing that ignores shelves has to change.
    pub subcategories: Vec<Subcategory>,
}

/// A second-level grouping within a category.
///
/// HX Edit shelves its block browser this way - Distortion splits into Mono,
/// Stereo and Legacy; Cab into Single and Dual - and keeping the shelves lets a
/// picker offer the same structure instead of one long flattened list. The
/// shelf ids HX Edit uses repeat across categories (Stereo is the same id
/// everywhere), so the name is what identifies a shelf, not an id.
#[derive(Debug, Clone)]
pub struct Subcategory {
    /// The shelf label, e.g. "Stereo" or "Legacy".
    pub name: String,
    /// Model ids on this shelf, in HX Edit's order.
    pub models: Vec<String>,
}

impl Category {
    /// Whether this category holds effects you choose between.
    ///
    /// Five of them do not: Input and Output are fixed endpoints of the
    /// topology, Split and Merge are the junctions between lanes, and
    /// Connected Devices is settings for external gear - a Variax, a Powercab.
    /// None belongs in a list of pedals to pick from, and offering them there
    /// only invites the question of what happens when you choose one.
    pub fn is_effect(&self) -> bool {
        !matches!(self.id, 0 | 18 | 19 | 20 | 21 | 22)
    }

    /// Structural categories, in the order they are useful.
    pub const INPUT: u32 = 18;
    pub const OUTPUT: u32 = 19;
    pub const SPLIT: u32 = 20;
    pub const MERGE: u32 = 21;
    pub const CONNECTED_DEVICES: u32 = 22;

    /// The rig-defining categories: an amp, a preamp, a speaker cab, an impulse
    /// response. Their presence is what tells one kind of tone from another - an
    /// amp into a cab is a full rig, the same effects with neither is a
    /// pedalboard. Used by [`inspect`](crate::inspect) to derive those facts.
    pub const AMP: u32 = 11;
    pub const PREAMP: u32 = 12;
    pub const CAB: u32 = 13;
    pub const IR: u32 = 14;

    /// Amp+Cab, the one category HX Edit's catalog does not contain: the file
    /// numbers its categories 0-9 and then jumps to 11. It is rebuilt in
    /// [`load`](crate::load) from the amps that name a cab.
    pub const AMP_CAB: u32 = 10;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// HX Edit is not installed where we looked.
    #[error("no HX Edit resources at {0}; install HX Edit or set HX_EDIT_RESOURCES")]
    NotInstalled(PathBuf),
    #[error("reading {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid catalog data in {path}: {reason}")]
    Invalid { path: PathBuf, reason: String },
    /// An `.hxb` backup bundle could not be read - not the expected container,
    /// or holding no setlist.
    #[error("{0}")]
    Backup(String),
}

impl Catalog {
    /// Load from wherever HX Edit is installed.
    pub fn load() -> Result<Catalog, Error> {
        let dir = resources_dir().ok_or_else(|| Error::NotInstalled(default_resources()))?;
        Catalog::load_from(&dir)
    }

    /// Load from an explicit resources directory.
    pub fn load_from(dir: &Path) -> Result<Catalog, Error> {
        load::catalog(dir)
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.get(id)
    }

    /// Look up a symbol by the number the device uses.
    pub fn symbol(&self, number: u32) -> Option<&Symbol> {
        self.symbols.get(number as usize)
    }

    /// The model behind a wire model number.
    pub fn model_number(&self, number: u32) -> Option<&Model> {
        let symbol = self.symbol(number)?;
        self.models.get(symbol.model.as_deref()?)
    }

    /// The parameter a `set parameter` message is addressing.
    ///
    /// The wire gives a model number and a parameter position; the symbol table
    /// turns the position into a name and the model supplies its range and
    /// formatting. This pairing is what makes an edit message readable.
    pub fn param(&self, model_number: u32, index: usize) -> Option<&Param> {
        let name = self.symbol(model_number)?.parameters.get(index)?;
        let model = self.model_number(model_number)?;
        model.params.iter().find(|p| &p.id == name)
    }

    /// Find a parameter's position by name, accepting either the display name
    /// ("Peak Reduction") or the symbolic one ("PeakReduction").
    pub fn param_index(&self, model_number: u32, name: &str) -> Option<usize> {
        let symbol = self.symbol(model_number)?;
        let model = self.model_number(model_number)?;
        symbol.parameters.iter().position(|id| {
            id.eq_ignore_ascii_case(name)
                || model
                    .params
                    .iter()
                    .any(|p| &p.id == id && p.name.eq_ignore_ascii_case(name))
        })
    }

    /// Full path to a model's artwork, if HX Edit ships one for it.
    ///
    /// These are Line 6's images. We point at the user's installed copy rather
    /// than shipping them, the same as for the rest of the catalog.
    pub fn artwork(&self, model: &Model) -> Option<PathBuf> {
        let file = model.image.as_deref()?;
        let path = self.resources.join("icons_models").join(file);
        path.is_file().then_some(path)
    }

    /// A category's icon, and how many frames it holds.
    ///
    /// Same `%N` convention as the endpoints: `FX_HX_Category_SendReturn_%3.png`
    /// is a vertical strip of three, one per direction the block can face.
    /// Everything else is a single frame, reported as 1 so a caller can treat
    /// both alike.
    pub fn category_artwork(&self, category: &Category) -> Option<(PathBuf, usize)> {
        let file = category.image.as_deref()?;
        let path = self.resources.join("icons_category").join(file);
        if !path.is_file() {
            return None;
        }
        let frames = file
            .strip_suffix(".png")
            .and_then(|stem| stem.rsplit_once("_%"))
            .and_then(|(_, count)| count.parse().ok())
            .unwrap_or(1);
        Some((path, frames))
    }

    /// The icon strip for an input or output, and how many frames it holds.
    ///
    /// HX Edit does not give the endpoints a fixed picture: it draws whichever
    /// destination they are routed to, from a vertical strip of 72×72 frames
    /// named with the count - `icon-inputs_%18.png`. Frame 0 is a placeholder,
    /// so the frame for a routing value is one past it: value 0 (None) is the
    /// cross, value 1 (Multi) the guitars.
    pub fn endpoint_icons(&self, input: bool) -> Option<(PathBuf, usize)> {
        let prefix = if input {
            "icon-inputs_%"
        } else {
            "icon-outputs_%"
        };
        let dir = self.resources.join("icons_models");
        let entry = std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let frames = name.strip_prefix(prefix)?.strip_suffix(".png")?;
            Some((e.path(), frames.parse::<usize>().ok()?))
        })?;
        Some(entry)
    }

    /// A model's parameters in the order the device sends their values.
    ///
    /// Two orderings exist and they are not the same. Effects are indexed by
    /// the firmware symbol table, which lists only real controls. Inputs,
    /// outputs and the like have no symbol entry, and their catalog list mixes
    /// in `@`-prefixed structural fields - `@input`, `@enabled` - that carry no
    /// value, so those have to come out or every knob is shifted by one.
    pub fn ordered_params<'a>(&self, model: &'a Model) -> Vec<&'a Param> {
        let by_symbol = self
            .symbols
            .iter()
            .find(|s| s.model.as_deref() == Some(model.id.as_str()));

        match by_symbol {
            Some(symbol) => symbol
                .parameters
                .iter()
                .filter_map(|id| model.params.iter().find(|p| &p.id == id))
                .collect(),
            None => model
                .params
                .iter()
                .filter(|p| !p.id.starts_with('@'))
                .collect(),
        }
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn models(&self) -> impl Iterator<Item = &Model> {
        self.models.values()
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    pub fn category(&self, id: u32) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    /// Which browsable category a model belongs to.
    ///
    /// Not `Model::category`. That field comes from the `.models` files and is
    /// numbered in a different space from the catalog's own category ids -
    /// Cali Q Graphic carries 14 there, while the EQ category is 106 - so
    /// using it to open the browser landed on whichever category happened to
    /// share the number. The membership lists are the authority.
    /// Amp+Cab is skipped: it re-lists the amps, so searching it first would
    /// answer "Amp+Cab" for every amp and send the browser somewhere the block
    /// may well not be. A block that really is a pair is known by its second
    /// model, not by its first model's category.
    pub fn category_of(&self, model: &str) -> Option<u32> {
        self.categories
            .iter()
            .filter(|c| !c.paired)
            .find(|c| c.models.iter().any(|m| m == model))
            .map(|c| c.id)
    }

    /// The cab that rides along with an amp in an Amp+Cab block.
    pub fn paired_cab(&self, model: &Model) -> Option<&Model> {
        self.model(model.cab_link.as_deref()?)
    }

    /// The second count a slot's value array carries - key 3, beside key 2.
    ///
    /// Key 2 is the number of values. Key 3 is the same number for most models
    /// and one less for cabs, delays, reverbs and the FX Loop - the models that
    /// carry a parameter the array holds but this count does not admit to. What
    /// that parameter is remains open; what it costs is one, consistently, so a
    /// written document can carry the right number without knowing why.
    ///
    /// Checked over every captured preset: keyed by category, the difference is
    /// 0 or 1 with no category showing both, and the one category that did -
    /// Send/Return - splits by model exactly as the engine class does, the FX
    /// Loop taking 1 and Send 0.
    ///
    /// Like [`type_tag`](Self::type_tag), this matters only when *building* a
    /// document: editing one carries the array through as it came.
    pub fn value_count_2(&self, model: u32, values: usize) -> Option<i64> {
        let model = self.model_number(model)?;
        if model.name.starts_with("Send") {
            return Some(values as i64);
        }
        let category = self
            .category_of(&model.id)
            .and_then(|id| self.category(id))
            .map(|c| c.name.as_str())
            .unwrap_or("");
        let short = matches!(category, "Cab" | "Delay" | "Reverb" | "Send/Return");
        Some(values as i64 - short as i64)
    }

    /// The engine class the device stamps on a slot - key 9 in the document.
    ///
    /// Needed to *build* a preset rather than edit one: every other field of a
    /// slot can be read off a `.hlx`, and this one cannot, so without it a
    /// document can only ever be carried through from the device, never
    /// written from a symbolic tone.
    ///
    /// It is a function of the model's category and of whether a cab rides
    /// along - not of the model alone, which is what the field's own comment
    /// used to say. Checked across 240 presets holding 217 distinct models:
    /// keyed by model alone, 30 of them carry two different values; keyed by
    /// category and pairing, none is ambiguous.
    ///
    /// An amp alone is 17 and an amp with its cab 18; a cab alone 15 and a dual
    /// cab 16. Delay and reverb share 8, the delay-RAM class. Splits and joins
    /// carry 0. Everything ordinary is 1. Two models are their own class
    /// regardless of their category - the 3 Note Generator, which has no input,
    /// and Send, which is not a Return.
    pub fn type_tag(&self, model: u32, has_cab: bool) -> Option<i64> {
        let model = self.model_number(model)?;
        // Two models sit apart from their categories, so they are asked first.
        match model.name.as_str() {
            "3 Note Generator" => return Some(23),
            name if name.starts_with("Send") => return Some(25),
            _ => {}
        }
        let category = self
            .category_of(&model.id)
            .and_then(|id| self.category(id))
            .map(|c| c.name.as_str())
            .unwrap_or("");
        Some(match category {
            "Amp" | "Preamp" => {
                if has_cab {
                    18
                } else {
                    17
                }
            }
            "Cab" => {
                if has_cab {
                    16
                } else {
                    15
                }
            }
            "Delay" | "Reverb" => 8,
            "Send/Return" => 9,
            "Looper" => 22,
            "Split" | "Merge" => 0,
            _ => 1,
        })
    }

    /// Models in a category, in the order HX Edit shows them.
    pub fn models_in(&self, category: u32) -> Vec<&Model> {
        self.category(category)
            .map(|c| c.models.iter().filter_map(|id| self.model(id)).collect())
            .unwrap_or_default()
    }

    /// Render a parameter value the way HX Edit would: "78%", "-0.1 dB", "Limit".
    pub fn format(&self, param: &Param, value: f32) -> String {
        param
            .display
            .as_deref()
            .and_then(|key| self.displays.get(key))
            .map(|d| d.render(value, self))
            .unwrap_or_else(|| format::plain(param, value))
    }

    /// Turn text the user typed into the value the device expects.
    ///
    /// The inverse of [`format`](Self::format): `"5.0"` on a knob shown 0..10
    /// becomes `0.5`, `"Limit"` on a switch becomes `1.0`. Displayed units in,
    /// native units out - so nothing above this line has to know about scales.
    pub fn parse(&self, param: &Param, text: &str) -> Option<f32> {
        let entry = param.display.as_deref().and_then(|k| self.displays.get(k));

        if let Some(index) = entry.and_then(|d| d.label_index(text, self)) {
            return param.accepts(index).then_some(index);
        }
        if param.kind == Kind::Switch {
            let value = match text.to_ascii_lowercase().as_str() {
                "on" | "true" | "1" => Some(1.0),
                "off" | "false" | "0" => Some(0.0),
                _ => None,
            }?;
            return param.accepts(value).then_some(value);
        }

        let text = text.trim();
        let number_end = text
            .char_indices()
            .find(|(_, c)| !matches!(c, '0'..='9' | '.' | '-' | '+' | 'e' | 'E'))
            .map_or(text.len(), |(i, _)| i);
        let typed: f32 = text[..number_end].parse().ok()?;
        let unit = text[number_end..].trim();
        let native = entry.map_or(typed, |d| d.to_native(typed, unit, self));
        param.accepts(native).then_some(native)
    }

    /// The list a parameter is chosen from, when it is a menu rather than a
    /// knob - routing destinations, cab mics, waveform shapes.
    pub fn choices(&self, param: &Param) -> Option<&[String]> {
        param
            .display
            .as_deref()
            .and_then(|key| self.displays.get(key))
            .and_then(|d| d.choices(self))
    }

    /// A named list that belongs to no parameter - the footswitch LED colours,
    /// for one. HX Edit keeps them in the same table it keeps every other menu
    /// in, so they are read the same way rather than typed out here.
    pub fn menu(&self, key: &str) -> Option<&[String]> {
        self.displays.get(key)?.choices(self)
    }

    pub(crate) fn display(&self, key: &str) -> Option<&Display> {
        self.displays.get(key)
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// Where HX Edit keeps its resources, honouring an override for other
/// platforms and for testing.
pub fn resources_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HX_EDIT_RESOURCES") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    // HX Edit ships for macOS and Windows only, so on Linux there is nowhere
    // standard to look. Copying the Resources folder across from a machine that
    // has it is the practical route, and this is where we expect it.
    let shared = home::resources().inspect(|path| {
        // Extraction publishes by swapping whole directories. If the process
        // stopped in the one rename-wide gap, put the prior generation back
        // before deciding whether resources are installed.
        let _ = extract::recover_install(path);
    });

    [
        Some(default_resources()),
        Some(PathBuf::from(r"C:\Program Files\Line 6\HX Edit\resources")),
        shared,
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_dir())
}

fn default_resources() -> PathBuf {
    PathBuf::from("/Applications/Line6/HX Edit.app/Contents/Resources")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The real installed catalog, or `None` where HX Edit is not present.
    ///
    /// Only a missing install is a reason to skip. A catalog that is present
    /// but will not parse is a genuine failure, and must not pass quietly -
    /// swallowing it once already let a parse bug through.
    pub(crate) fn catalog() -> Option<Catalog> {
        match Catalog::load() {
            Ok(c) => Some(c),
            Err(Error::NotInstalled(_)) => {
                // A silent green tick would be a lie: on a machine without HX
                // Edit almost this whole suite is skipped.
                eprintln!("SKIPPED: HX Edit is not installed, so the catalog cannot be read");
                None
            }
            Err(e) => panic!("HX Edit is installed but its catalog failed to load: {e}"),
        }
    }

    #[test]
    fn reads_the_installed_catalog() {
        let Some(c) = catalog() else { return };
        assert!(
            c.len() > 600,
            "expected the full model set, got {}",
            c.len()
        );

        let comp = c
            .model("HD2_CompressorLAStudioComp")
            .expect("LA Studio Comp");
        assert_eq!(comp.name, "LA Studio Comp");
        assert_eq!(comp.params[0].name, "Peak Reduction");
        assert_eq!(comp.params[0].kind, Kind::Continuous);
        assert_eq!(comp.params[0].default, 0.78);
    }

    #[test]
    fn switches_are_distinguished_from_knobs() {
        let Some(c) = catalog() else { return };
        let comp = c.model("HD2_CompressorLAStudioComp").unwrap();
        let ty = comp.params.iter().find(|p| p.name == "Type").unwrap();
        assert_eq!(ty.kind, Kind::Switch);
    }

    #[test]
    fn parse_is_the_inverse_of_format() {
        let Some(c) = catalog() else { return };
        let comp = c.model("HD2_CompressorLAStudioComp").unwrap();

        // Drive-style knobs are stored 0..1 and shown 0..10, which is exactly
        // the trap that sent 5.0 to the device as "50".
        let peak = &comp.params[0];
        assert_eq!(c.parse(peak, "5.0"), Some(0.5));
        assert_eq!(c.format(peak, 0.5), "5.0");

        let mix = comp.params.iter().find(|p| p.name == "Mix").unwrap();
        assert_eq!(c.parse(mix, "100"), Some(1.0));

        // Switches accept their labels and the obvious words.
        let ty = comp.params.iter().find(|p| p.name == "Type").unwrap();
        assert_eq!(c.parse(ty, "Limit"), Some(1.0));
        assert_eq!(c.parse(ty, "Compress"), Some(0.0));
        assert_eq!(c.parse(ty, "on"), Some(1.0));
    }

    #[test]
    fn parse_understands_kilohertz_and_rejects_values_outside_the_parameter_range() {
        let frequency: Display = serde_json::from_str(
            r#"{
                "format": [
                    { "lowerBound": 20, "upperBound": 1000,
                      "formatUnits": "%.0f Hz" },
                    { "lowerBound": 1000, "upperBound": 999999,
                      "formatUnits": "%.1f kHz", "unitsMultiplier": 0.001 }
                ]
            }"#,
        )
        .unwrap();
        let catalog = Catalog {
            models: HashMap::new(),
            categories: Vec::new(),
            displays: HashMap::from([("frequency".to_owned(), frequency)]),
            symbols: Vec::new(),
            resources: PathBuf::new(),
        };
        let mid_frequency = Param {
            id: "MidFreq".to_owned(),
            name: "Mid Freq".to_owned(),
            kind: Kind::Continuous,
            min: 125.0,
            max: 4000.0,
            default: 600.0,
            display: Some("frequency".to_owned()),
        };

        assert_eq!(catalog.parse(&mid_frequency, "1.2 kHz"), Some(1200.0));
        assert_eq!(catalog.parse(&mid_frequency, "1.2 KHz"), Some(1200.0));
        assert_eq!(catalog.parse(&mid_frequency, "4000 Hz"), Some(4000.0));
        assert_eq!(catalog.parse(&mid_frequency, "1.2 Hz"), None);
        assert_eq!(catalog.parse(&mid_frequency, "4.1 kHz"), None);
    }

    #[test]
    fn cyclic_display_aliases_do_not_recurse_forever() {
        let displays = serde_json::from_str(
            r#"{
                "first": {
                    "alias": "second",
                    "dspToDisplayScale": 10,
                    "format": "%.1f first"
                },
                "second": {
                    "alias": "first",
                    "dspToDisplayScale": 100,
                    "format": ["Zero", "One"]
                }
            }"#,
        )
        .unwrap();
        let catalog = Catalog {
            models: HashMap::new(),
            categories: Vec::new(),
            displays,
            symbols: Vec::new(),
            resources: PathBuf::new(),
        };
        let continuous = Param {
            id: "Cycle".to_owned(),
            name: "Cycle".to_owned(),
            kind: Kind::Continuous,
            min: 0.0,
            max: 1.0,
            default: 0.0,
            display: Some("first".to_owned()),
        };
        let menu = Param {
            display: Some("second".to_owned()),
            kind: Kind::Enum,
            max: 1.0,
            ..continuous.clone()
        };

        assert_eq!(catalog.format(&continuous, 0.5), "5.0 first");
        assert_eq!(catalog.parse(&continuous, "5.0"), Some(0.5));
        assert_eq!(catalog.choices(&menu).unwrap(), ["Zero", "One"]);
        assert_eq!(catalog.parse(&menu, "One"), Some(1.0));
    }

    #[test]
    fn wire_numbers_resolve_to_models() {
        let Some(c) = catalog() else { return };
        // These three were read straight out of a captured preset.
        assert_eq!(c.symbol(247).unwrap().symbol, "HD2_ReverbRoomStereo");
        assert_eq!(c.model_number(247).unwrap().name, "Room");
        assert_eq!(c.model_number(296).unwrap().name, "Simple Pitch");
        assert_eq!(c.model_number(180).unwrap().name, "Bubble Vibrato");
    }

    #[test]
    fn parameters_resolve_by_position() {
        let Some(c) = catalog() else { return };
        // Room's parameters are Decay, Predelay, LowCut, HighCut, Mix, Level.
        assert_eq!(c.param(247, 0).unwrap().name, "Decay");
        assert_eq!(c.param(247, 4).unwrap().name, "Mix");
        assert!(c.param(247, 99).is_none());
    }

    #[test]
    fn parameters_are_found_by_either_name() {
        let Some(c) = catalog() else { return };
        // Room: Decay, Predelay, LowCut, HighCut, Mix, Level.
        assert_eq!(c.param_index(247, "Mix"), Some(4));
        assert_eq!(c.param_index(247, "mix"), Some(4));
        assert_eq!(c.param_index(247, "LowCut"), Some(2));
        assert_eq!(c.param_index(247, "nonsense"), None);
    }

    #[test]
    fn nearly_every_symbol_resolves() {
        let Some(c) = catalog() else { return };
        let resolved = c.symbols().iter().filter(|s| s.model.is_some()).count();
        assert!(
            resolved >= c.symbols().len() - 8,
            "only {resolved} of {} symbols resolved",
            c.symbols().len()
        );
    }

    #[test]
    fn models_resolve_to_artwork_on_disk() {
        let Some(c) = catalog() else { return };
        let room = c.model_number(247).unwrap();
        let art = c.artwork(room).expect("Room has artwork");
        assert!(art.is_file());
        assert_eq!(art.extension().unwrap(), "png");

        let with_art = c.models().filter(|m| c.artwork(m).is_some()).count();
        assert!(with_art > 400, "only {with_art} models resolved artwork");
    }

    /// The two category numberings are genuinely different, and confusing them
    /// opened the model browser on the wrong shelf.
    #[test]
    fn the_endpoints_have_an_icon_per_routing_destination() {
        let Some(c) = catalog() else { return };
        let (path, frames) = c.endpoint_icons(true).expect("an input icon strip");

        assert!(path.is_file());
        // One frame per destination in the menu, plus the leading placeholder.
        let input = c.model("HelixStomp_AppDSPFlowInput").unwrap();
        let from = input.params.iter().find(|p| p.id == "@input").unwrap();
        assert_eq!(frames, c.choices(from).unwrap().len() + 1);
    }

    /// HX Edit paints its categories, and those colours are in the catalog -
    /// so a chain drawn here can look like the same chain drawn there rather
    /// than like someone's guess at it.
    #[test]
    fn categories_carry_hx_edits_own_colours() {
        let Some(c) = catalog() else { return };
        let by_name = |n: &str| c.categories().iter().find(|c| c.name == n).unwrap();

        assert_eq!(by_name("Distortion").colour, 0xf5_90_1e);
        assert_eq!(by_name("Amp").colour, 0xdd_11_11);
        assert_eq!(by_name("Delay").short_name, "Delay");
        assert_eq!(by_name("Reverb").short_name, "Verb");

        // And the structural ones are marked so they stay out of the browser.
        assert!(by_name("Distortion").is_effect());
        for structural in ["Input", "Output", "Split", "Merge", "Connected Devices"] {
            assert!(
                !by_name(structural).is_effect(),
                "{structural} is not something you browse for"
            );
        }
    }

    /// Amp+Cab is the one category HX Edit's own catalog leaves out - its ids
    /// run 0-9 and then 11 - so it is rebuilt from the amps that name a cab.
    #[test]
    fn amp_and_cab_is_rebuilt_from_the_amps_that_name_a_cab() {
        let Some(c) = catalog() else { return };
        let amp_cab = c.category(Category::AMP_CAB).expect("Amp+Cab is rebuilt");

        assert_eq!(amp_cab.name, "Amp+Cab");
        assert!(amp_cab.paired, "choosing one fills a block with two models");
        assert!(amp_cab.is_effect(), "it belongs in the browser");

        // It sits where HX Edit shows it: after Wah, before Amp.
        let ids: Vec<u32> = c.categories().iter().map(|c| c.id).collect();
        let at = ids.iter().position(|id| *id == Category::AMP_CAB).unwrap();
        assert_eq!(ids[at - 1], 9, "Wah comes before it");
        assert_eq!(ids[at + 1], Category::AMP, "Amp comes after it");

        // Every model in it pairs with a cab, that cab is a real model, and
        // both halves are known to the firmware by number - which is what
        // choosing one actually sends, so a pair that cannot be numbered would
        // be a tile that quietly does nothing.
        assert!(!amp_cab.models.is_empty());
        let numbered = |id: &str| c.symbols().iter().any(|s| s.model.as_deref() == Some(id));
        for model in c.models_in(Category::AMP_CAB) {
            let cab = c
                .paired_cab(model)
                .unwrap_or_else(|| panic!("{} names a cab that exists", model.id));
            assert_eq!(c.category_of(&cab.id), Some(Category::CAB));
            assert!(numbered(&model.id), "{} has a model number", model.id);
            assert!(numbered(&cab.id), "{} has a model number", cab.id);
        }

        // And it re-lists amps rather than owning them, so an amp still
        // browses as an Amp - otherwise swapping one would land here instead.
        let amp = c.models_in(Category::AMP_CAB)[0];
        assert_eq!(c.category_of(&amp.id), Some(Category::AMP));
    }

    /// The browser's category glyphs, including the one for the category
    /// HX Edit's catalog never mentions.
    #[test]
    fn categories_resolve_to_icons_on_disk() {
        let Some(c) = catalog() else { return };
        let by_name = |n: &str| c.categories().iter().find(|c| c.name == n).unwrap();

        for name in ["Distortion", "Amp", "Amp+Cab", "Reverb"] {
            let (path, frames) = c
                .category_artwork(by_name(name))
                .unwrap_or_else(|| panic!("{name} has an icon"));
            assert!(path.is_file());
            assert_eq!(frames, 1, "{name} is a single image");
        }

        // Send/Return's is a strip, one frame per direction the block faces -
        // the `%3` in its filename, the same convention as the endpoints.
        let (_, frames) = c.category_artwork(by_name("Send/Return")).unwrap();
        assert_eq!(frames, 3);
    }

    /// HX Edit shelves each category into Mono / Stereo / Legacy and the like.
    /// The loader used to flatten those away; this keeps them, and the flat
    /// `models` stays the union so nothing that ignores shelves changes.
    #[test]
    fn categories_keep_hx_edits_shelves() {
        let Some(c) = catalog() else { return };
        let by_name = |n: &str| c.categories().iter().find(|c| c.name == n).unwrap();

        let distortion = by_name("Distortion");
        let shelves: Vec<&str> = distortion
            .subcategories
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(shelves, ["Mono", "Stereo", "Legacy"]);

        // The flat list is exactly the shelves' union, in order.
        let from_shelves: Vec<String> = distortion
            .subcategories
            .iter()
            .flat_map(|s| s.models.iter().cloned())
            .collect();
        assert_eq!(distortion.models, from_shelves);
        assert!(!from_shelves.is_empty());

        // Amps shelve by instrument, cabs by count - not everything is Mono/Stereo.
        assert!(by_name("Amp")
            .subcategories
            .iter()
            .any(|s| s.name == "Guitar"));
    }

    #[test]
    fn a_model_is_found_in_the_category_that_lists_it() {
        let Some(c) = catalog() else { return };
        let eq = c
            .category_of("HD2_CaliQ")
            .expect("Cali Q Graphic is in a category");

        assert_eq!(c.category(eq).unwrap().name, "EQ");
        assert!(c.models_in(eq).iter().any(|m| m.id == "HD2_CaliQ"));
        assert_ne!(
            eq,
            c.model("HD2_CaliQ").unwrap().category,
            "the model's own category field is in a different numbering"
        );
    }

    #[test]
    fn routing_parameters_offer_their_destinations_as_a_menu() {
        let Some(c) = catalog() else { return };
        let output = c.model("HelixStomp_AppDSPFlowOutputMain").unwrap();
        let to = output.params.iter().find(|p| p.id == "@output").unwrap();
        let choices = c.choices(to).expect("Output To is a menu");

        assert_eq!(choices[0], "None");
        assert!(choices.iter().any(|c| c == "XLR"), "{choices:?}");
        // A knob is not a menu.
        let level = output.params.iter().find(|p| p.id == "gain").unwrap();
        assert!(c.choices(level).is_none());
    }

    #[test]
    fn parameter_order_matches_the_values_the_device_sends() {
        let Some(c) = catalog() else { return };

        // An effect: ordered by the symbol table, structural fields excluded.
        let room = c.model_number(247).unwrap();
        let names: Vec<_> = c
            .ordered_params(room)
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Decay", "Predelay", "Low Cut", "High Cut", "Mix", "Level"]
        );

        // An input has no symbol entry, and `@input` must not occupy a slot -
        // the device sends three values for it, not four.
        let input = c.model("HD2_AppDSPFlow1Input").unwrap();
        let names: Vec<_> = c
            .ordered_params(input)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(names, ["noiseGate", "threshold", "decay"]);
    }

    #[test]
    fn categories_are_populated() {
        let Some(c) = catalog() else { return };
        let dist = c
            .categories()
            .iter()
            .find(|c| c.name == "Distortion")
            .unwrap();
        assert!(!dist.models.is_empty());
        assert!(!c.models_in(dist.id).is_empty());
    }
}
