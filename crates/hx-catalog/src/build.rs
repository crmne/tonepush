//! Building a device document's slots from a `.hlx`, the direction that did
//! not exist.
//!
//! [`to_hlx`](crate::to_hlx) turns a preset into Line 6's symbolic JSON, and
//! [`inspect`](crate::inspect) reads that JSON into tone facts. Neither goes
//! back to a document the pedal will accept, which is why restoring a `.hxb`
//! natively and importing a `.hlx` faithfully were the same missing piece: both
//! need JSON to become bytes.
//!
//! A slot on the wire is
//!
//! ```text
//! {19: kind, 20: {24: {23: paired?, 25: model, 26: cab or -1},
//!                  9: engine class, 10: enabled,
//!                 11: {2: n, 3: n', 4: [values]},
//!                 12: {…the cab's values…}}}
//! ```
//!
//! and every field of it can be read off a `.hlx` except two - the engine class
//! and that second count `n'` - which is what [`Catalog::type_tag`] and
//! [`Catalog::value_count_2`] exist for. See PROTOCOL.md.
//!
//! **The pedal will not take what this produces.** Checked on hardware: a
//! document rebuilt from its own symbolic form, with a chain identical to the
//! original block for block and value for value, is written and read back
//! empty. A `.hlx` does not record how each number was encoded - the same 1.0
//! is an integer in one preset and a float in another - and the document
//! carries a table of byte offsets into itself, so the wrong tag width is fatal
//! rather than cosmetic. This is the same failure the parser's own notes
//! describe: re-encode a wide tag narrow and "the device reads the result as
//! empty".
//!
//! So this is for reading a `.hxb` into something inspectable, and for building
//! tones offline - not for restoring onto a pedal. Restoring goes through
//! `.hxbundle`, which keeps the device's own bytes and cannot lose their shape.
//!
//! This writes slots into an existing document rather than inventing one from
//! nothing. A preset carries a great deal besides its chain - a section table
//! of byte offsets into itself, snapshot state, footswitch assignments - and
//! the honest way to get those right is to start from a document the device
//! wrote and replace the part being described.

use serde_json::Value as Json;

use hx_proto::msgpack::{Key, Value};
use hx_proto::Preset;

use crate::Catalog;

/// Wire keys, named. These mirror `hx_proto::preset::key`, which is private -
/// deliberately, since nothing outside the parser should be reading a document
/// by hand. Writing one is the exception that earns them.
mod key {
    pub const KIND: i64 = 19;
    pub const BODY: i64 = 20;
    pub const MODEL_REF: i64 = 24;
    pub const HAS_PAIRED: i64 = 23;
    pub const MODEL: i64 = 25;
    pub const PAIRED_MODEL: i64 = 26;
    pub const TYPE_TAG: i64 = 9;
    pub const ENABLED: i64 = 10;
    pub const VALUES: i64 = 11;
    pub const PAIRED_VALUES: i64 = 12;
    pub const COUNT: i64 = 2;
    pub const COUNT_2: i64 = 3;
    pub const ARRAY: i64 = 4;
    /// The wire's own number for an occupied block, and for an empty slot.
    pub const BLOCK: i64 = 6;
    pub const EMPTY: i64 = 8;
}

/// What could not be built, so a caller can say so rather than write a preset
/// that quietly lost something.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// How many blocks went in.
    pub blocks: usize,
    /// Models, parameters and slots that could not be resolved, each named.
    pub skipped: Vec<String>,
}

/// Write the chain a `.hlx` describes into `preset`, replacing what is there.
///
/// `preset` supplies everything the JSON does not: the section table, the
/// snapshot section, the endpoints and the junctions. Pass a document the
/// device wrote - an empty preset is the natural template.
///
/// Blocks land in the order the document names them, `block0` first, into the
/// slots the template keeps for them. A block that will not resolve is reported
/// and its slot left empty rather than filled with a guess.
pub fn slots_from_hlx(preset: &mut Preset, document: &Json, catalog: &Catalog) -> Built {
    let mut skipped = Vec::new();
    let mut blocks = 0;

    // Where a path's blocks may go: everything between its input and its
    // output, and between its split and its join. Read off the template rather
    // than assumed, so a device with a different slot count still works.
    // One run of free slots per signal path: the main line between its input
    // and output, and the branch between its split and join. Both belong to the
    // same path and the same `dspN` in the file - a second `dsp` is a second
    // *path*, which only hardware with two DSPs has. Reading the branch as its
    // own dsp dropped every block on it.
    let layout = preset.layout();
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for path in &layout.paths {
        let mut run: Vec<usize> = Vec::new();
        if let (Some(input), Some(output)) = (path.input, path.output) {
            run.extend((input + 1)..output);
        }
        if let (Some(split), Some(join)) = (path.split, path.join) {
            run.extend((split + 1)..join);
        }
        runs.push(run);
    }

    let tone = document.get("data").and_then(|d| d.get("tone"));
    for (dsp_index, run) in runs.iter().enumerate() {
        let name = format!("dsp{dsp_index}");
        let Some(dsp) = tone.and_then(|t| t.get(&name)).and_then(Json::as_object) else {
            continue;
        };

        let numbered = |prefix: &str| {
            let mut found: Vec<(usize, String)> = dsp
                .keys()
                .filter_map(|k| {
                    let digits = k.strip_prefix(prefix)?;
                    Some((digits.parse::<usize>().ok()?, k.clone()))
                })
                .collect();
            found.sort_unstable();
            found
        };
        let named = numbered("block");
        // An Amp+Cab is one slot on the wire and two nodes in the file: the amp
        // as a block, its cab as `cab0`, `cab1`, … HX Edit's own convention,
        // and positional - the k-th cab belongs to the k-th block that can take
        // one. Ignoring them put the cab in a slot of its own, which changes
        // the chain and leaves the amp carrying the engine class of an amp with
        // no cab.
        // A cab names the slot of the amp it rides in, so whose cab it is is
        // said rather than guessed.
        let cab_nodes = numbered("cab");

        let mut used = std::collections::BTreeSet::new();
        for (_, block_key) in named {
            let Some(node) = dsp.get(&block_key) else {
                continue;
            };
            // Where the block sat, said one of two ways. `@slot` is ours and is
            // the device's own index. `@path` and `@position` are HX Edit's:
            // the branch, and the place along that branch's row, which is not
            // the slot the moment a chain splits. A file with neither packs
            // from the front, which is all a dense numbering can mean.
            let explicitly_placed = node.get("@slot").is_some() || node.get("@position").is_some();
            let recorded = if let Some(slot) = node.get("@slot") {
                slot.as_u64().and_then(|slot| usize::try_from(slot).ok())
            } else {
                let branch = node.get("@path").and_then(Json::as_u64).unwrap_or(0);
                let position = node.get("@position").and_then(Json::as_u64);
                usize::try_from(branch).ok().and_then(|branch| {
                    usize::try_from(position?)
                        .ok()
                        .and_then(|position| layout.slot_of(dsp_index, branch, position))
                })
            }
            .filter(|slot| run.contains(slot));
            if explicitly_placed && recorded.is_none() {
                skipped.push(format!(
                    "{block_key}: its recorded position is outside this chain"
                ));
                continue;
            }
            let Some(position) = recorded.or_else(|| {
                run.iter()
                    .copied()
                    .find(|position| !used.contains(position))
            }) else {
                skipped.push(format!("{block_key}: the chain has no room left"));
                continue;
            };
            if used.contains(&position) {
                skipped.push(format!(
                    "{block_key}: another block already uses slot {position}"
                ));
                continue;
            }
            // Whose cab it is, said rather than guessed - two ways again. HX
            // Edit has the amp name its cab node, `"@cab": "cab0"`; ours has
            // the cab name the amp's slot. Either beats counting cabs against
            // amps and hoping the k-th is the k-th.
            let cab = node
                .get("@cab")
                .and_then(Json::as_str)
                .and_then(|key| dsp.get(key))
                .or_else(|| {
                    let slot = recorded?;
                    cab_nodes
                        .iter()
                        .filter_map(|(_, key)| dsp.get(key))
                        .find(|c| c.get("@slot").and_then(Json::as_u64) == Some(slot as u64))
                });

            match build_slot(node, cab, catalog) {
                Ok(slot) => {
                    if preset.paste_slot(position, &slot) {
                        blocks += 1;
                        used.insert(position);
                    } else {
                        skipped.push(format!("{block_key}: slot {position} would not take it"));
                    }
                }
                Err(why) => skipped.push(format!("{block_key}: {why}")),
            }
        }

        // The wiring is the template's, not the file's. A `.hlx` says which
        // kind of split it has and where it attaches, and writing either would
        // mean building a junction slot rather than a block - a different shape
        // this does not make. Where the two disagree, say so: a Y read as an
        // A/B divides the signal differently, and a chain that quietly kept the
        // wrong one is worse than one that says it did.
        //
        // Applying it belongs to the other direction, and is done there. An
        // import reaches the device as ordinary edits, so it can set a
        // junction's model the way the editor's own Type chips do - see
        // `hx-cli`'s `hlx::plan_for`, confirmed on hardware. Here the result is
        // a whole document, and the pedal refuses documents built from `.hlx`
        // at all, so building a junction slot to put in one buys nothing.
        let Some(path) = layout.paths.get(dsp_index) else {
            continue;
        };
        for (node_key, junction) in [("split", path.split), ("join", path.join)] {
            let Some(wanted) = dsp
                .get(node_key)
                .and_then(|n| n.get("@model"))
                .and_then(Json::as_str)
            else {
                continue;
            };
            let holding = junction
                .and_then(|slot| preset.junction_model(slot))
                .and_then(|number| catalog.symbol(number))
                .map(|symbol| symbol.symbol.clone());
            match holding {
                Some(holding) if holding == wanted => {}
                Some(holding) => skipped.push(format!(
                    "{name}/{node_key}: the file wants {wanted}, the chain has {holding}"
                )),
                None => skipped.push(format!(
                    "{name}/{node_key}: the file wants {wanted}, the chain has no {node_key}"
                )),
            }
        }
    }

    Built { blocks, skipped }
}

/// Turn a whole `.hxb` backup into documents ready for the pedal.
///
/// This is what makes `.hxb` a format TonePush can *restore* rather than only
/// write. A bundle stores its presets as symbolic JSON - HX Edit's own choice -
/// so putting one back has always needed this direction, and until now the only
/// route was rebuilding a tone through parameter edits, which loses whatever
/// the editor does not model.
///
/// `template` supplies everything a `.hlx` does not describe and must be a
/// document the device wrote. Its chain is emptied first, so nothing of the
/// template's own tone survives into the result.
///
/// Empty slots come back as `None`, so a caller can blank them rather than
/// leaving whatever the pedal happens to hold there.
pub fn documents_from_backup(
    backup: &crate::Backup,
    template: &Preset,
    catalog: &Catalog,
) -> Vec<Option<(String, Preset, Built)>> {
    let bytes = template.encode();
    backup
        .presets
        .iter()
        .map(|entry| {
            if entry.empty {
                return None;
            }
            // A fresh copy per preset: each starts from the same template
            // rather than from whatever the last one left behind.
            let mut document = Preset::parse(&bytes)?;
            empty_the_chain(&mut document);
            let built = slots_from_hlx(&mut document, &entry.hlx, catalog);
            Some((entry.name.clone(), document, built))
        })
        .collect()
}

/// Clear every block slot, leaving the endpoints and junctions alone.
///
/// A template is only a source of the parts a `.hlx` cannot describe; carrying
/// its blocks through would put someone else's tone in the gaps of the one
/// being restored.
pub fn empty_the_chain(preset: &mut Preset) {
    let empty = Value::Map(vec![
        (Key::Int(key::KIND), Value::Int(key::EMPTY)),
        (Key::Int(key::BODY), Value::Nil),
    ]);
    for position in 0..preset.slots.len() {
        // `paste_slot` refuses anything that is not a block or already empty,
        // which is exactly the protection wanted here.
        let _ = preset.paste_slot(position, &empty);
    }
}

/// One `.hlx` block node as the slot the device expects.
fn build_slot(node: &Json, cab: Option<&Json>, catalog: &Catalog) -> Result<Value, String> {
    let symbol_name = node
        .get("@model")
        .and_then(Json::as_str)
        .ok_or("no @model")?;
    if node.get("@stereo").is_some_and(|value| !value.is_boolean()) {
        return Err("@stereo is not a boolean value".into());
    }
    let symbol = resolve(catalog, symbol_name, node)
        .ok_or_else(|| format!("the catalog does not know {symbol_name}"))?;
    let model = symbol.number;

    // The device indexes values by position, and the symbol's parameter list is
    // that order. A parameter the document does not mention keeps the value the
    // catalog gives as its default rather than becoming zero, which for a knob
    // like Master is the difference between a preset and a silent one.
    let values = values_for(symbol, node, catalog)?;

    // The cab that rides along, with its own values in its own order.
    let paired = match cab {
        Some(cab) => {
            let name = cab.get("@model").and_then(Json::as_str).unwrap_or_default();
            let symbol = resolve(catalog, name, cab)
                .ok_or_else(|| format!("the catalog does not know the cab {name}"))?;
            Some((symbol.number, values_for(symbol, cab, catalog)?))
        }
        None => None,
    };

    let enabled = match node.get("@enabled") {
        Some(value) => value.as_bool().ok_or("@enabled is not a boolean value")?,
        None => true,
    };
    let type_tag = catalog
        .type_tag(model, paired.is_some())
        .ok_or_else(|| format!("no engine class for {symbol_name}"))?;
    let count_2 = catalog
        .value_count_2(model, values.len())
        .ok_or_else(|| format!("no value count for {symbol_name}"))?;

    Ok(Value::Map(vec![
        (Key::Int(key::KIND), Value::Int(key::BLOCK)),
        (
            Key::Int(key::BODY),
            Value::Map(vec![
                (
                    Key::Int(key::MODEL_REF),
                    Value::Map(vec![
                        (Key::Int(key::HAS_PAIRED), Value::Bool(paired.is_some())),
                        (Key::Int(key::MODEL), Value::Int(model as i64)),
                        (
                            Key::Int(key::PAIRED_MODEL),
                            // Absent is written as -1, not omitted.
                            Value::Int(paired.as_ref().map_or(-1, |(n, _)| *n as i64)),
                        ),
                    ]),
                ),
                (Key::Int(key::TYPE_TAG), Value::Int(type_tag)),
                (Key::Int(key::ENABLED), Value::Bool(enabled)),
                (Key::Int(key::VALUES), counted(&values, count_2)),
                (
                    Key::Int(key::PAIRED_VALUES),
                    match &paired {
                        Some((number, values)) => counted(
                            values,
                            catalog.value_count_2(*number, values.len()).unwrap_or(0),
                        ),
                        None => counted(&[], 0),
                    },
                ),
            ]),
        ),
    ]))
}

/// A block node's values, in the order the device indexes them.
///
/// A parameter the document does not mention keeps the catalog's default rather
/// than becoming zero - for a knob like Master that is the difference between a
/// preset and a silent one. The values the symbol table does not name follow the
/// named ones; `to_hlx` keeps them under `@unnamed`, and a file from HX Edit
/// will not have them.
fn values_for(symbol: &crate::Symbol, node: &Json, catalog: &Catalog) -> Result<Vec<f32>, String> {
    let mut values = Vec::with_capacity(symbol.parameters.len());
    for (index, id) in symbol.parameters.iter().enumerate() {
        let found = match node.get(id) {
            Some(value) => Some(
                number_of(value).ok_or_else(|| format!("{id} is not a numeric parameter value"))?,
            ),
            None => None,
        };
        if let Some(value) = found {
            if !value.is_finite() {
                return Err(format!("{id} is not a finite number"));
            }
            if let Some(param) = catalog.param(symbol.number, index) {
                if !param.accepts(value) {
                    return Err(format!(
                        "{} value {value} is outside {}..{}",
                        param.name, param.min, param.max
                    ));
                }
            }
        }
        values.push(found.unwrap_or_else(|| default_of(catalog, symbol.number, id)));
    }
    if let Some(extra) = node.get("@unnamed") {
        let extra = extra
            .as_array()
            .ok_or("@unnamed parameters are not an array")?;
        for (index, raw) in extra.iter().enumerate() {
            let value = number_of(raw)
                .ok_or_else(|| format!("unnamed parameter {index} is not numeric"))?;
            if !value.is_finite() {
                return Err("an unnamed parameter is not a finite number".to_owned());
            }
            values.push(value);
        }
    }
    Ok(values)
}

/// Which firmware symbol a `@model` names.
///
/// A `.hlx` writes the *shared* model id - `HD2_DistMinotaur` - where the
/// firmware has a mono symbol and a stereo one, each with its own wire number
/// and its own parameter list. The name alone therefore does not say which, and
/// picking the first would silently turn every stereo block mono.
///
/// The parameters do say. A block node lists the parameters it actually has, so
/// the candidate whose list those keys match is the one that was written. Ties
/// go to the lower wire number, which is the mono variant and the one a file
/// naming neither is likelier to have meant.
pub fn resolve<'a>(catalog: &'a Catalog, name: &str, node: &Json) -> Option<&'a crate::Symbol> {
    let candidates: Vec<&crate::Symbol> = catalog
        .symbols()
        .iter()
        .filter(|s| s.symbol == name || s.model.as_deref() == Some(name))
        .collect();
    if candidates.len() <= 1 {
        return candidates.into_iter().next();
    }
    // A file that says which it is settles it outright.
    let wants_stereo = node.get("@stereo").and_then(Json::as_bool).unwrap_or(false);
    if let Some(exact) = candidates
        .iter()
        .find(|s| s.symbol.ends_with("Stereo") == wants_stereo)
        .filter(|_| candidates.iter().any(|s| s.symbol.ends_with("Stereo")))
    {
        return Some(exact);
    }
    let present = |s: &crate::Symbol| -> (usize, usize) {
        let hit = s
            .parameters
            .iter()
            .filter(|p| node.get(p.as_str()).is_some())
            .count();
        // Most parameters accounted for, then fewest left unexplained.
        (hit, s.parameters.len().saturating_sub(hit))
    };
    candidates.into_iter().min_by_key(|s| {
        let (hit, missing) = present(s);
        // Sorted ascending, so negate the hits to prefer more of them.
        (std::cmp::Reverse(hit), missing, s.number)
    })
}

/// A value array in the shape the wire uses: the count, the second count, and
/// the values.
fn counted(values: &[f32], count_2: i64) -> Value {
    Value::Map(vec![
        (Key::Int(key::COUNT), Value::Int(values.len() as i64)),
        (Key::Int(key::COUNT_2), Value::Int(count_2)),
        (
            Key::Int(key::ARRAY),
            Value::Array(values.iter().map(|v| Value::F32(*v)).collect()),
        ),
    ])
}

/// A `.hlx` value as the number the wire holds. A switch is written as a bool
/// and stored as 0 or 1.
fn number_of(value: &Json) -> Option<f32> {
    match value {
        Json::Bool(b) => Some(*b as u8 as f32),
        Json::Number(n) => n.as_f64().map(|f| f as f32),
        _ => None,
    }
}

/// What a parameter should be when the document does not say.
fn default_of(catalog: &Catalog, model: u32, id: &str) -> f32 {
    catalog
        .model_number(model)
        .and_then(|m| catalog.ordered_params(m).into_iter().find(|p| p.id == id))
        .map(|p| p.default)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_building_refuses_an_out_of_range_parameter() {
        let Some(catalog) = crate::tests::catalog() else {
            return;
        };
        let symbol = catalog
            .symbols()
            .iter()
            .find(|symbol| symbol.symbol == "HD2_DistScream808Mono")
            .expect("Scream 808 symbol");
        let node = serde_json::json!({
            "@model": "HD2_DistScream808Mono",
            "Gain": 1.5
        });

        let error = values_for(symbol, &node, &catalog).unwrap_err();
        assert!(
            error.contains("Gain") && error.contains("outside"),
            "{error}"
        );
    }

    #[test]
    fn document_building_refuses_malformed_parameter_values() {
        let Some(catalog) = crate::tests::catalog() else {
            return;
        };
        let symbol = catalog
            .symbols()
            .iter()
            .find(|symbol| symbol.symbol == "HD2_DistScream808Mono")
            .expect("Scream 808 symbol");

        let named = serde_json::json!({ "Gain": "loud" });
        assert!(values_for(symbol, &named, &catalog).is_err());

        let unnamed = serde_json::json!({ "@unnamed": [0.5, "broken", 0.7] });
        assert!(values_for(symbol, &unnamed, &catalog).is_err());

        let wrong_container = serde_json::json!({ "@unnamed": 0.5 });
        assert!(values_for(symbol, &wrong_container, &catalog).is_err());
    }

    #[test]
    fn document_building_refuses_a_malformed_bypass_flag() {
        let Some(catalog) = crate::tests::catalog() else {
            return;
        };
        let node = serde_json::json!({
            "@model": "HD2_DistScream808Mono",
            "@enabled": "yes"
        });
        assert!(build_slot(&node, None, &catalog).is_err());
    }

    #[test]
    fn document_building_refuses_a_malformed_stereo_flag() {
        let Some(catalog) = crate::tests::catalog() else {
            return;
        };
        let node = serde_json::json!({
            "@model": "HD2_DistScream808Mono",
            "@stereo": "wide"
        });
        let error = build_slot(&node, None, &catalog).unwrap_err();
        assert!(error.contains("@stereo"), "{error}");
    }

    #[test]
    fn recorded_and_dense_blocks_never_overwrite_each_other() {
        const FIXTURE: &[u8] = include_bytes!("../../hx-proto/tests/preset.bin");
        let Some(catalog) = crate::tests::catalog() else {
            return;
        };
        let mut preset = Preset::parse(FIXTURE).expect("fixture");
        empty_the_chain(&mut preset);
        let first = preset
            .slots
            .iter()
            .position(|slot| slot.kind == hx_proto::preset::Kind::Empty)
            .expect("a block slot");
        let document = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": "HD2_DistScream808Mono", "@slot": first },
                // No location: this one takes the first slot not already used.
                "block1": { "@model": "HD2_DistScream808Mono" },
                // An explicit bad location is reported, never treated as if it
                // had no location and packed somewhere unrelated.
                "block2": { "@model": "HD2_DistScream808Mono", "@slot": u64::MAX }
            }}}
        });

        let built = slots_from_hlx(&mut preset, &document, &catalog);
        assert_eq!(built.blocks, 2);
        assert_eq!(preset.blocks().count(), 2);
        assert_eq!(built.skipped.len(), 1);
        assert!(built.skipped[0].contains("outside"));
    }
}
