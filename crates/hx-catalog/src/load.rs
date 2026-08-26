//! Reading HX Edit's JSON.
//!
//! Three files matter, and they divide the work cleanly: the `.models` files
//! define parameters and ranges, `HX_ModelCatalog.json` decides grouping and
//! ordering in the browser, and `HelixControls.json` says how values are
//! displayed.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::{Catalog, Category, Display, Error, Kind, Model, Param, Subcategory, Symbol};

pub(crate) fn catalog(dir: &Path) -> Result<Catalog, Error> {
    let mut models: HashMap<String, Model> = HashMap::new();
    for file in MODEL_FILES {
        // Not every HX Edit version ships every file, and a missing one should
        // cost you those models rather than the whole catalog.
        let path = dir.join(file);
        if !path.exists() {
            continue;
        }
        for raw in read::<Vec<RawModel>>(&path)? {
            let id = raw.symbolic_id.clone();
            let model = Model::try_from(raw).map_err(|reason| Error::Invalid {
                path: path.clone(),
                reason,
            })?;
            models.insert(id, model);
        }
    }

    let browse: RawCatalog = read(&dir.join("HX_ModelCatalog.json"))?;
    for (id, image) in artwork(&browse) {
        if let Some(model) = models.get_mut(&id) {
            model.image = Some(image);
        }
    }

    let displays = read::<HashMap<String, Display>>(&dir.join("HelixControls.json"))?;
    let symbols = symbols(dir, &models)?;
    let categories = categories(dir, &models)?;

    if models.is_empty() {
        return Err(Error::Invalid {
            path: dir.to_owned(),
            reason: "the model files contain no models".to_owned(),
        });
    }
    if categories.is_empty() {
        return Err(Error::Invalid {
            path: dir.join("HX_ModelCatalog.json"),
            reason: "the model catalog contains no categories".to_owned(),
        });
    }
    if symbols.is_empty() {
        return Err(Error::Invalid {
            path: dir.join("Helix.sym"),
            reason: "the symbol table contains no symbols".to_owned(),
        });
    }

    Ok(Catalog {
        resources: dir.to_owned(),
        symbols,
        categories,
        models,
        displays,
    })
}

/// The symbol table, whose position in the file is the device's model number.
fn symbols(dir: &Path, models: &HashMap<String, Model>) -> Result<Vec<Symbol>, Error> {
    let path = dir.join("Helix.sym");
    let raw: Vec<RawSymbol> = read(&path)?;
    for (number, symbol) in raw.iter().enumerate() {
        if symbol.symbol.is_empty() {
            return Err(Error::Invalid {
                path,
                reason: format!("symbol {number} has no name"),
            });
        }
        if symbol.parameters.iter().any(String::is_empty) {
            return Err(Error::Invalid {
                path,
                reason: format!("symbol {} has an unnamed parameter", symbol.symbol),
            });
        }
    }
    Ok(raw
        .into_iter()
        .enumerate()
        .map(|(number, s)| {
            // The symbol table keeps mono and stereo apart where the catalog
            // merges them, so fall back to the shared name.
            let model = [s.symbol.as_str()]
                .into_iter()
                .chain(s.symbol.strip_suffix("Mono"))
                .chain(s.symbol.strip_suffix("Stereo"))
                .find(|id| models.contains_key(*id))
                .map(str::to_owned);
            Symbol {
                number: number as u32,
                model,
                symbol: s.symbol,
                parameters: s.parameters,
            }
        })
        .collect())
}

/// Every `.models` file HX Edit 3.82 ships. Named explicitly rather than
/// globbed so a stray file in the directory cannot change what we load.
const MODEL_FILES: &[&str] = &[
    "amp.models",
    "cab.models",
    "cabmicirs.models",
    "cabmicirswithpan.models",
    "compressor.models",
    "delay.models",
    "distortion.models",
    "eq.models",
    "filter.models",
    "fixed.models",
    "gate.models",
    "io.models",
    "modulation.models",
    "pitch-synth.models",
    "preamp.models",
    "reverb.models",
    "sendreturn.models",
    "volumepan.models",
    "wah.models",
];

/// Artwork lives in the browse catalog rather than the `.models` files, so it is
/// collected while walking the categories and merged into the models after.
fn artwork(raw: &RawCatalog) -> HashMap<String, String> {
    raw.categories
        .iter()
        .flat_map(|c| {
            c.models
                .iter()
                .chain(c.subcategories.iter().flat_map(|s| s.models.iter()))
        })
        .filter_map(|m| m.image.clone().map(|i| (m.id.clone(), i)))
        .collect()
}

fn categories(dir: &Path, models: &HashMap<String, Model>) -> Result<Vec<Category>, Error> {
    let path = dir.join("HX_ModelCatalog.json");
    let raw: RawCatalog = read(&path)?;
    let mut ids = std::collections::HashSet::new();
    for category in &raw.categories {
        if category.name.is_empty() {
            return Err(Error::Invalid {
                path,
                reason: format!("category {} has no name", category.id),
            });
        }
        if !ids.insert(category.id) {
            return Err(Error::Invalid {
                path,
                reason: format!("category id {} is duplicated", category.id),
            });
        }
        let colour = category.color.strip_prefix("0x").unwrap_or(&category.color);
        if !colour.is_empty()
            && (colour.len() != 6 || !colour.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(Error::Invalid {
                path,
                reason: format!("category {} has an invalid colour", category.name),
            });
        }
        if category.models.iter().any(|model| model.id.is_empty()) {
            return Err(Error::Invalid {
                path,
                reason: format!("category {} contains an unnamed model", category.name),
            });
        }
        for shelf in &category.subcategories {
            if shelf.name.is_empty() || shelf.models.iter().any(|model| model.id.is_empty()) {
                return Err(Error::Invalid {
                    path,
                    reason: format!("category {} contains an invalid shelf", category.name),
                });
            }
        }
    }
    let mut categories: Vec<Category> = raw
        .categories
        .into_iter()
        .map(|c| {
            // The shelves HX Edit shows - Mono / Stereo / Legacy and the like -
            // kept as their own list. Named, because the shelf ids repeat across
            // categories and so cannot tell one shelf from another.
            let subcategories = c
                .subcategories
                .iter()
                .map(|s| Subcategory {
                    name: s.name.clone(),
                    models: s.models.iter().map(|m| m.id.clone()).collect(),
                })
                .collect();
            // A category lists models directly or splits them across shelves;
            // both flatten to the same browse order for anyone ignoring shelves.
            let models = c
                .models
                .iter()
                .map(|m| m.id.clone())
                .chain(
                    c.subcategories
                        .iter()
                        .flat_map(|s| s.models.iter().map(|m| m.id.clone())),
                )
                .collect();
            let short_name = if c.short_name.is_empty() {
                c.name.clone()
            } else {
                c.short_name.clone()
            };
            // "0xf5901e" - a hex string, not a number. A category with no
            // colour falls back to plain white rather than black, which would
            // be indistinguishable from an unpainted block.
            let colour =
                u32::from_str_radix(c.color.trim_start_matches("0x"), 16).unwrap_or(0xff_ff_ff);
            Category {
                id: c.id,
                name: c.name,
                short_name,
                colour,
                image: c.image,
                paired: false,
                models,
                subcategories,
            }
        })
        .collect();

    if let Some(amp_cab) = amp_and_cab(&categories, models) {
        // Where HX Edit puts it: between Wah and Amp, which is where its own
        // missing id belongs.
        let at = categories
            .iter()
            .position(|c| c.id == Category::AMP)
            .unwrap_or(categories.len());
        categories.insert(at, amp_cab);
    }

    Ok(categories)
}

/// Rebuild the Amp+Cab category, which `HX_ModelCatalog.json` does not carry.
///
/// The file numbers its categories 0-9 and then jumps to 11 - there is no 10 -
/// yet `icons_category` ships `FX_HX_Category_Amp+Cab.png` and HX Edit shows
/// the category between Wah and Amp. What it lists is not a separate set of
/// models: every amp in `amp.models` carries a `cablink` naming the cab it
/// pairs with, and an Amp+Cab block is one slot holding both. So the category
/// is the amps that name a cab, in Amp's own order, shelves and colour.
fn amp_and_cab(categories: &[Category], models: &HashMap<String, Model>) -> Option<Category> {
    let amp = categories.iter().find(|c| c.id == Category::AMP)?;
    let pairs = |ids: &[String]| -> Vec<String> {
        ids.iter()
            .filter(|id| models.get(*id).is_some_and(|m| m.cab_link.is_some()))
            .cloned()
            .collect()
    };

    let models_with_cabs = pairs(&amp.models);
    if models_with_cabs.is_empty() {
        return None;
    }

    Some(Category {
        id: Category::AMP_CAB,
        name: "Amp+Cab".to_owned(),
        short_name: "Amp+Cab".to_owned(),
        colour: amp.colour,
        image: Some("FX_HX_Category_Amp+Cab.png".to_owned()),
        paired: true,
        models: models_with_cabs,
        subcategories: amp
            .subcategories
            .iter()
            .map(|s| Subcategory {
                name: s.name.clone(),
                models: pairs(&s.models),
            })
            .filter(|s| !s.models.is_empty())
            .collect(),
    })
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, Error> {
    let bytes = std::fs::read(path).map_err(|source| Error::Read {
        path: path.to_owned(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Parse {
        path: path.to_owned(),
        source,
    })
}

// ------------------------------------------------------------------- shapes ---

#[derive(Deserialize)]
struct RawCatalog {
    categories: Vec<RawCategory>,
}

#[derive(Deserialize)]
struct RawCategory {
    id: u32,
    name: String,
    #[serde(default)]
    image: Option<String>,
    #[serde(default, rename = "shortName")]
    short_name: String,
    /// Written as a hex string - "0xf5901e" - not a number.
    #[serde(default)]
    color: String,
    #[serde(default)]
    models: Vec<RawCatalogModel>,
    #[serde(default)]
    subcategories: Vec<RawSubcategory>,
}

#[derive(Deserialize)]
struct RawSubcategory {
    /// The shelf label - "Mono", "Stereo", "Legacy", "Guitar", "Single". This
    /// is the field the loader used to drop, flattening the shelves away.
    #[serde(default)]
    name: String,
    #[serde(default)]
    models: Vec<RawCatalogModel>,
}

#[derive(Deserialize)]
struct RawCatalogModel {
    id: String,
    image: Option<String>,
}

#[derive(Deserialize)]
struct RawSymbol {
    symbol: String,
    #[serde(default)]
    parameters: Vec<String>,
}

#[derive(Deserialize)]
struct RawModel {
    #[serde(rename = "symbolicID")]
    symbolic_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: u32,
    #[serde(default)]
    stereo: bool,
    #[serde(default)]
    load: Option<f32>,
    #[serde(default)]
    load_mono: Option<f32>,
    #[serde(default)]
    load_stereo: Option<f32>,
    /// Only amps carry this: the cab they pair with in an Amp+Cab block.
    #[serde(default, rename = "cablink")]
    cab_link: Option<String>,
    #[serde(default)]
    params: Vec<Fields>,
}

/// Parameters come through as a raw map rather than a struct.
///
/// Two reasons. Some entries in Line 6's files repeat a key - `distortion.models`
/// has a parameter with two `assign` fields - which a derived deserialiser
/// rejects outright; taking the last value is both tolerant and obviously
/// right. And bounds are written as whichever type suits the parameter, so
/// `false`/`true` for a switch, strings for text, and numbers for a knob. The
/// checked conversion below handles those shapes after the raw map is read.
type Fields = serde_json::Map<String, serde_json::Value>;

fn required_text(fields: &Fields, key: &str) -> Result<String, String> {
    match fields.get(key).and_then(|value| value.as_str()) {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => Err(format!("parameter {key} must be a non-empty string")),
    }
}

fn required_number(fields: &Fields, key: &str) -> Result<f32, String> {
    let number = match fields.get(key) {
        Some(serde_json::Value::Bool(value)) => *value as u8 as f32,
        Some(serde_json::Value::Number(value)) => value
            .as_f64()
            .map(|value| value as f32)
            .ok_or_else(|| format!("parameter {key} is not a representable number"))?,
        _ => return Err(format!("parameter {key} must be a number or boolean")),
    };
    number
        .is_finite()
        .then_some(number)
        .ok_or_else(|| format!("parameter {key} must be finite"))
}

impl TryFrom<RawModel> for Model {
    type Error = String;

    fn try_from(m: RawModel) -> Result<Model, String> {
        let params = m
            .params
            .into_iter()
            .enumerate()
            .map(|(index, fields)| {
                Param::try_from(fields).map_err(|reason| {
                    format!("model {} parameter {index}: {reason}", m.symbolic_id)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Model {
            id: m.symbolic_id,
            name: m.name,
            category: m.category,
            stereo: m.stereo,
            // Most models call their mono cost `load`, while stereo-only
            // models have only `load_stereo`. Keep preferring the ordinary
            // field for models that offer both variants.
            load: m.load.or(m.load_mono).or(m.load_stereo).unwrap_or_default(),
            image: None,
            cab_link: m.cab_link,
            params,
        })
    }
}

impl TryFrom<Fields> for Param {
    type Error = String;

    fn try_from(f: Fields) -> Result<Param, String> {
        let value_type = required_number(&f, "valueType")?;
        if value_type.fract() != 0.0 || !(0.0..=3.0).contains(&value_type) {
            return Err(format!("parameter valueType {value_type} is not supported"));
        }
        let kind = match value_type as u8 {
            0 => Kind::Enum,
            2 => Kind::Switch,
            3 => Kind::Text,
            _ => Kind::Continuous,
        };
        let (min, max, default) = if kind == Kind::Text {
            for key in ["min", "max", "default"] {
                if !matches!(f.get(key), Some(serde_json::Value::String(_))) {
                    return Err(format!("text parameter {key} must be a string"));
                }
            }
            // Text parameters are not sent through the numeric edit paths,
            // but Param keeps one uniform representation for every kind.
            (0.0, 1.0, 0.0)
        } else {
            let min = required_number(&f, "min")?;
            let max = required_number(&f, "max")?;
            let default = required_number(&f, "default")?;
            if min > max {
                return Err(format!("parameter range is reversed: {min} to {max}"));
            }
            if !(min..=max).contains(&default) {
                return Err(format!(
                    "parameter default {default} is outside {min} to {max}"
                ));
            }
            (min, max, default)
        };
        let display = match f.get("displayType") {
            Some(serde_json::Value::String(display)) => Some(display.clone()),
            Some(_) => return Err("parameter displayType must be a string".to_owned()),
            None => None,
        };
        Ok(Param {
            kind,
            min,
            max,
            default,
            display,
            id: required_text(&f, "symbolicID")?,
            name: required_text(&f, "name")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tonepush-catalog-load-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn model(json: &str) -> Model {
        serde_json::from_str::<RawModel>(json)
            .unwrap()
            .try_into()
            .unwrap()
    }

    #[test]
    fn stereo_only_models_use_their_stereo_load() {
        let chamber = model(
            r#"{
                "symbolicID": "HD2_ReverbChamber",
                "name": "Chamber",
                "stereo": true,
                "load_stereo": 7.93
            }"#,
        );

        assert_eq!(chamber.load, 7.93);
    }

    #[test]
    fn models_with_both_loads_keep_their_mono_load() {
        let simple_eq = model(
            r#"{
                "symbolicID": "HD2_EQSimple3Band",
                "name": "Simple EQ",
                "stereo": true,
                "load": 1.28,
                "load_stereo": 1.63
            }"#,
        );

        assert_eq!(simple_eq.load, 1.28);
    }

    #[test]
    fn malformed_display_catalog_is_not_silently_replaced_with_an_empty_one() {
        let dir = scratch("bad-displays");
        std::fs::write(dir.join("HX_ModelCatalog.json"), r#"{"categories": []}"#).unwrap();
        std::fs::write(dir.join("HelixControls.json"), b"{").unwrap();

        match catalog(&dir) {
            Err(Error::Parse { path, .. }) => {
                assert_eq!(path, dir.join("HelixControls.json"));
            }
            _ => panic!("a malformed display catalog should be reported"),
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_symbol_table_is_not_silently_replaced_with_an_empty_one() {
        let dir = scratch("bad-symbols");
        std::fs::write(dir.join("HX_ModelCatalog.json"), r#"{"categories": []}"#).unwrap();
        std::fs::write(dir.join("HelixControls.json"), b"{}").unwrap();
        std::fs::write(dir.join("Helix.sym"), b"[").unwrap();

        match catalog(&dir) {
            Err(Error::Parse { path, .. }) => {
                assert_eq!(path, dir.join("Helix.sym"));
            }
            _ => panic!("a malformed symbol table should be reported"),
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_parameter_records_are_not_filled_with_defaults() {
        let dir = scratch("bad-parameter");
        std::fs::write(dir.join("HX_ModelCatalog.json"), r#"{"categories": []}"#).unwrap();
        std::fs::write(dir.join("HelixControls.json"), b"{}").unwrap();
        std::fs::write(dir.join("Helix.sym"), b"[]").unwrap();
        std::fs::write(
            dir.join("amp.models"),
            r#"[{"symbolicID":"Amp","params":[{"symbolicID":"Drive","valueType":1,"min":0,"max":1,"default":0.5}]}]"#,
        )
        .unwrap();

        match catalog(&dir) {
            Err(Error::Invalid { path, reason }) => {
                assert_eq!(path, dir.join("amp.models"));
                assert!(reason.contains("name"), "{reason}");
            }
            _ => panic!("a malformed parameter should be reported"),
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn structurally_empty_catalogs_are_rejected() {
        let dir = scratch("empty-catalog");
        std::fs::write(dir.join("HX_ModelCatalog.json"), r#"{"categories": []}"#).unwrap();
        std::fs::write(dir.join("HelixControls.json"), b"{}").unwrap();
        std::fs::write(dir.join("Helix.sym"), b"[]").unwrap();

        match catalog(&dir) {
            Err(Error::Invalid { path, reason }) => {
                assert_eq!(path, dir);
                assert!(reason.contains("no models"), "{reason}");
            }
            _ => panic!("an empty catalog should be reported"),
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_catalog_indexes_are_rejected() {
        let dir = scratch("bad-index");
        std::fs::write(
            dir.join("HX_ModelCatalog.json"),
            r#"{"categories":[{"id":11,"name":"Amp","color":"blue","models":[{"id":"Amp"}]}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("HelixControls.json"), b"{}").unwrap();
        std::fs::write(dir.join("Helix.sym"), r#"[{"symbol":"Amp"}]"#).unwrap();
        std::fs::write(dir.join("amp.models"), r#"[{"symbolicID":"Amp"}]"#).unwrap();

        match catalog(&dir) {
            Err(Error::Invalid { path, reason }) => {
                assert_eq!(path, dir.join("HX_ModelCatalog.json"));
                assert!(reason.contains("colour"), "{reason}");
            }
            _ => panic!("a malformed catalog index should be reported"),
        }

        let _ = std::fs::remove_dir_all(dir);
    }
}
