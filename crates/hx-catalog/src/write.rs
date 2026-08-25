//! Writing a `.hlx` preset from a device-free [`Preset`], the save half of the
//! reader in [`inspect`](crate::inspect).
//!
//! This is the exact inverse of the inspector. The inspector turns Line 6's
//! symbolic JSON into "what this tone is"; this turns a parsed preset back into
//! that same JSON, so a library can hold tones as `.hlx` files and read them
//! back with the piece it was saved by. Like the inspector it runs entirely
//! from the catalog and the preset, never the pedal.
//!
//! Three shapes are worth calling out, all read off a real HX Edit `.hlx`:
//!
//! * Blocks are keyed `block0`, `block1`, … densely and in chain order, with
//!   the endpoints and junctions living under their own keys rather than in the
//!   block run. We number the blocks we emit, so the numbering stays dense even
//!   when a model cannot be written.
//! * An amp and its cab are two separate blocks. On the wire a paired cab rides
//!   in the amp's slot, so we split it back out into its own block, right after
//!   the amp - the way a standalone amp and cab already appear in a file.
//! * `@model` is matched back by exact string, both here and on the apply path,
//!   which has no base-name fallback. So we write the precise firmware symbol
//!   the wire number resolves to - suffix and all - rather than the base name a
//!   real file pairs with `@stereo`. It is unambiguous and it is what both of
//!   our readers require to resolve it.
//!
//! Parameter values are written in their native units, the way the format
//! stores them - no display scaling - keyed by the parameter's symbolic id, and
//! a switch as a bool. That is the mirror image of how the inspector reads them
//! back.

use serde_json::{json, Map, Value};

use hx_proto::preset::{Kind as SlotKind, Preset};

use crate::Catalog;

/// A serialised `.hlx` document together with a note of anything the catalog
/// could not name.
///
/// The `skipped` list is the same discipline the readers keep: a model with no
/// symbol, or a value with no parameter, is reported rather than dropped in
/// silence, so a caller can surface it.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The `.hlx` JSON, ready to hand to [`inspect`](crate::inspect) or to
    /// serialise to a file.
    pub document: Value,
    pub skipped: Vec<String>,
}

impl Written {
    /// The document as pretty-printed JSON with a trailing newline, matching how
    /// the other JSON dumps in this workspace are written to disk.
    pub fn to_pretty_string(&self) -> String {
        serde_json::to_string_pretty(&self.document).unwrap_or_default() + "\n"
    }
}

/// Serialise a [`Preset`] into a Line 6 `.hlx` symbolic document.
///
/// `name` becomes `data.meta.name`. Every occupied block becomes an entry under
/// `data.tone.dsp0` - or `dsp1` for a second signal path, as a two-DSP device
/// carries - with its `@model`, `@enabled` and parameters. See the module docs
/// for the shape this produces and why.
pub fn to_hlx(preset: &Preset, catalog: &Catalog, name: &str) -> Written {
    let mut skipped = Vec::new();
    // Endpoint symbols carry the device in their name. Nothing in the document
    // says which device it came from, so an HX Stomp is assumed - the only one
    // this program has ever been run against.
    let device = "HelixStomp";

    // One JSON object per DSP path, plus a running blockN counter for each. A
    // new path opens at every input, the way `Preset::layout` reads them; dsp0
    // is always present so blocks that precede any input - as a bare captured
    // tone can carry - still have a home.
    let mut dsps: Vec<Map<String, Value>> = vec![Map::new()];
    let mut next_block: Vec<i64> = vec![0];
    let mut next_cab: Vec<i64> = vec![0];
    let mut path = 0usize;
    let mut opened = false;

    for (index, slot) in preset.slots.iter().enumerate() {
        match slot.kind {
            SlotKind::Input => {
                if opened {
                    path += 1;
                    dsps.push(Map::new());
                    next_block.push(0);
                    next_cab.push(0);
                }
                opened = true;
            }
            SlotKind::Block | SlotKind::Looper if slot.model.is_some() => {
                emit(
                    &mut dsps[path],
                    &mut next_block[path],
                    index,
                    slot.model,
                    &slot.values,
                    slot.enabled,
                    catalog,
                    &mut skipped,
                );
                // The cab rides in the amp's slot on the wire; write it out as
                // its own block, sharing the amp's bypass state.
                // The cab rides in the amp's slot on the wire, and HX Edit
                // writes it out as its own node named cabN rather than giving it
                // a block number - so the amp after it keeps the number it
                // would have had.
                if slot.paired.is_some() {
                    // The amp names its cab, which is how HX Edit says whose it
                    // is. `emit` has just written the amp as the block before
                    // this counter moved on.
                    let cab_key = format!("cab{}", next_cab[path]);
                    if let Some(Value::Object(amp)) =
                        dsps[path].get_mut(&format!("block{}", next_block[path] - 1))
                    {
                        amp.insert("@cab".into(), Value::String(cab_key.clone()));
                    }
                    emit_named(
                        &mut dsps[path],
                        cab_key,
                        index,
                        slot.paired,
                        &slot.paired_values,
                        slot.enabled,
                        catalog,
                        &mut skipped,
                    );
                    next_cab[path] += 1;
                }
            }
            _ => {}
        }
    }

    // The wiring: where the signal enters, where it forks and merges, and where
    // it leaves. HX Edit names these nodes rather than numbering them, and a
    // document without them describes a chain that starts nowhere - which is
    // what this writer used to produce.
    let layout = preset.layout();
    for (index, dsp_path) in layout.paths.iter().enumerate() {
        let Some(dsp) = dsps.get_mut(index) else {
            continue;
        };
        if dsp_path.input.is_some() {
            dsp.insert("inputA".into(), endpoint(device, true, false));
        }
        if dsp_path.output.is_some() {
            dsp.insert("outputA".into(), endpoint(device, false, false));
        }
        // A split and a join come as a pair, and each carries where it attaches
        // to the main line - which block the fork sits before.
        if let (Some(split), Some(join)) = (dsp_path.split, dsp_path.join) {
            // Which kind of split this is comes off the document: a Y, an A/B
            // and a crossover are three different models.
            let named = |position: usize, fallback: &str| {
                preset
                    .junction_model(position)
                    .and_then(|n| catalog.symbol(n))
                    .map(|s| s.symbol.clone())
                    .unwrap_or_else(|| fallback.to_owned())
            };
            // A junction's number means "before this cell", so it is the same
            // arithmetic a block's uses on the slot it attaches to.
            let at = |slot: Option<usize>| {
                slot.and_then(|slot| layout.position_of(index, slot))
                    .map(|(_, position)| position)
            };
            dsp.insert(
                "split".into(),
                junction(
                    &named(split, "HD2_AppDSPFlowSplitY"),
                    preset.attach_of(split),
                    at(preset.attach_of(split)),
                ),
            );
            dsp.insert(
                "join".into(),
                junction(
                    &named(join, "HD2_AppDSPFlowJoin"),
                    preset.attach_of(join),
                    at(preset.attach_of(join)),
                ),
            );
            // The lower branch has its own input and output endpoints.
            dsp.insert("inputB".into(), endpoint(device, true, false));
            dsp.insert("outputB".into(), endpoint(device, false, true));
        }
    }

    // The same fact in HX Edit's own vocabulary: which branch a block is on and
    // its place along that branch's row. We write `@slot` as well because it is
    // exact without a layout to read it against, and a file carrying both can
    // be read either way. A cab has neither, because it rides in its amp's slot
    // and HX Edit gives it none.
    for (index, dsp) in dsps.iter_mut().enumerate() {
        for (key, node) in dsp.iter_mut() {
            if !key.starts_with("block") {
                continue;
            }
            let Some((branch, position)) = node
                .get("@slot")
                .and_then(Value::as_u64)
                .and_then(|slot| layout.position_of(index, slot as usize))
            else {
                continue;
            };
            let Some(node) = node.as_object_mut() else {
                continue;
            };
            node.insert("@path".into(), json!(branch));
            node.insert("@position".into(), json!(position));
        }
    }

    let mut tone = Map::new();
    for (index, dsp) in dsps.into_iter().enumerate() {
        tone.insert(format!("dsp{index}"), Value::Object(dsp));
    }

    // Snapshots: the same blocks with a different set of them switched on. A
    // preset saved without these is a preset that has lost two thirds of what
    // the player set up.
    for (index, snapshot) in preset.snapshot_details().iter().enumerate() {
        let mut blocks = Map::new();
        let mut dsp0 = Map::new();
        // Snapshot state is indexed by slot; the document names blocks in the
        // order they were emitted, so walk the same slots the same way.
        let mut block_number = 0i64;
        for (position, slot) in preset.slots.iter().enumerate() {
            if !matches!(slot.kind, SlotKind::Block | SlotKind::Looper) || slot.model.is_none() {
                continue;
            }
            if let Some(Some(on)) = snapshot.enabled.get(position) {
                dsp0.insert(format!("block{block_number}"), Value::Bool(*on));
            }
            block_number += 1;
        }
        // A split is switched by a snapshot like any block, and HX Edit records
        // it under its node name rather than a block number.
        for dsp_path in &layout.paths {
            if let Some(split) = dsp_path.split {
                if preset.junction_switchable(split) {
                    if let Some(Some(on)) = snapshot.enabled.get(split) {
                        dsp0.insert("split".into(), Value::Bool(*on));
                    }
                }
            }
        }
        blocks.insert("dsp0".into(), Value::Object(dsp0));

        tone.insert(
            format!("snapshot{index}"),
            json!({
                "@name": snapshot.name,
                "@valid": snapshot.valid,
                "@custom_name": snapshot.named,
                "@tempo": snapshot.tempo.unwrap_or(120.0),
                "@ledcolor": 0,
                "@pedalstate": 0,
                "blocks": Value::Object(blocks),
            }),
        );
    }

    // The preset's own tempo, which lives beside the snapshots in HX Edit's
    // document rather than inside any of them.
    if let Some(tempo) = preset.tempo() {
        tone.insert("global".into(), json!({ "@tempo": tempo }));
    }

    let document = json!({
        "data": {
            "meta": { "name": name },
            "tone": Value::Object(tone),
        }
    });

    Written { document, skipped }
}

/// An input or output node. The endpoint symbols carry the device - an HX Stomp
/// writes `HelixStomp_…`.
///
/// The A output is the main pair and the B output is the send, on every one of
/// the 97 presets HX Edit wrote in the backup this was checked against. Where an
/// output is *pointed* is a separate field this does not carry yet.
fn endpoint(device: &str, input: bool, send: bool) -> Value {
    let model = match (input, send) {
        (true, _) => format!("{device}_AppDSPFlowInput"),
        (false, false) => format!("{device}_AppDSPFlowOutputMain"),
        (false, true) => format!("{device}_AppDSPFlowOutputSend"),
    };
    json!({ "@model": model })
}

/// A split or a join, with the place it attaches to on the main line said
/// twice: as the device's own slot, and as the row index HX Edit writes.
fn junction(model: &str, attach: Option<usize>, position: Option<usize>) -> Value {
    json!({
        "@model": model,
        "@attach": attach.unwrap_or(0),
        "@position": position.unwrap_or(0),
    })
}

/// Write one model into a DSP's block map at its next position, or record why it
/// could not be. The amp and its cab both come through here, so both resolve
/// their model and parameters the same way.
#[allow(clippy::too_many_arguments)]
fn emit(
    dsp: &mut Map<String, Value>,
    next_block: &mut i64,
    slot: usize,
    model: Option<u32>,
    values: &[f32],
    enabled: bool,
    catalog: &Catalog,
    skipped: &mut Vec<String>,
) {
    let Some(number) = model else { return };

    let Some(symbol) = catalog.symbol(number) else {
        skipped.push(format!("model {number}: no symbol in the catalog"));
        return;
    };

    let mut block = Map::new();
    // HX Edit writes the shared model name - `HD2_DistScream808` - where the
    // firmware symbol is the mono or stereo variant of it. The symbol table
    // carries both, so use the one a `.hlx` is expected to name.
    let written_name = symbol
        .model
        .clone()
        .unwrap_or_else(|| symbol.symbol.clone());
    block.insert("@model".into(), Value::String(written_name));
    block.insert("@enabled".into(), Value::Bool(enabled));
    // Which slot of the chain this came out of. HX Edit numbers its blocks
    // densely and lets position speak for order, which is enough to *read* a
    // tone and not enough to rebuild one: a chain can leave gaps, and the split
    // and join carry the slot index they attach before. Repacking blocks
    // densely moved every one of those. A file without this still loads - the
    // blocks simply pack from the front, which is what HX Edit's own files
    // mean.
    block.insert("@slot".into(), json!(slot));
    // Mono and stereo are two firmware symbols sharing one model name, and the
    // name is what gets written - so without this the two are the same block on
    // paper and a stereo delay comes back mono. HX Edit carries the same flag.
    if symbol.symbol.ends_with("Stereo") {
        block.insert("@stereo".into(), Value::Bool(true));
    }

    // Values come in the order the device indexes them; each resolves to a
    // parameter whose symbolic id keys it in the file. A switch is written as a
    // bool, the way the reader folds it back to 0.0/1.0.
    // Cabs, delays, reverbs and the FX Loop store one value more than the
    // symbol table names - see PROTOCOL.md on the second count a value array
    // carries. Dropping it made a `.hlx` of a cab quietly lose a setting, and
    // made the trip back impossible; it is written under a key of our own,
    // after the named ones, so nothing that reads by parameter id sees it.
    let mut unnamed = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let Some(param) = catalog.param(number, index) else {
            unnamed.push(json!(*value));
            continue;
        };
        let written = if param.kind == crate::Kind::Switch {
            Value::Bool(*value >= 0.5)
        } else {
            json!(*value)
        };
        block.insert(param.id.clone(), written);
    }
    if !unnamed.is_empty() {
        skipped.push(format!(
            "{}: {} value(s) the catalog does not name, kept as @unnamed",
            symbol.symbol,
            unnamed.len()
        ));
        block.insert("@unnamed".into(), Value::Array(unnamed));
    }

    dsp.insert(format!("block{next_block}"), Value::Object(block));
    *next_block += 1;
}

/// The same as [`emit`], under a name the caller chooses - what a paired cab
/// needs, since HX Edit calls it `cab0` rather than giving it a block number.
#[allow(clippy::too_many_arguments)]
fn emit_named(
    dsp: &mut Map<String, Value>,
    node: String,
    slot: usize,
    model: Option<u32>,
    values: &[f32],
    enabled: bool,
    catalog: &Catalog,
    skipped: &mut Vec<String>,
) {
    let mut scratch = 0i64;
    let mut one = Map::new();
    // A cab rides in its amp's slot rather than holding one of its own, so it
    // carries the amp's index - which is what says whose cab it is. Positional
    // pairing guessed wrong whenever a preset held a standalone amp as well as
    // a paired one.
    emit(
        &mut one,
        &mut scratch,
        slot,
        model,
        values,
        enabled,
        catalog,
        skipped,
    );
    if let Some((_, body)) = one.into_iter().next() {
        dsp.insert(node, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::catalog;
    use crate::{inspect, Category, ChainContent};
    use hx_proto::msgpack::{Encoder, Key};
    // The synthetic-preset helpers build MessagePack values; this explicit
    // import shadows the `serde_json::Value` the glob above brings in.
    use hx_proto::{msgmap, Preset, Value};
    use std::collections::BTreeMap;

    // Tone field keys, mirroring `hx_proto::preset`'s private `key` module. Kept
    // here so a synthetic preset can be built without a device or a fixture.
    const KIND: i64 = 19;
    const BODY: i64 = 20;
    const PATH: i64 = 0;
    const SLOTS: i64 = 22;
    const MODEL_REF: i64 = 24;
    const MODEL: i64 = 25;
    const PAIRED_MODEL: i64 = 26;
    const PAIRED_VALUES: i64 = 12;
    const VALUES: i64 = 11;
    const ENABLED: i64 = 10;
    const ARRAY_VALUES: i64 = 4;

    // Slot kinds on the wire.
    const INPUT: i64 = 0;
    const BLOCK: i64 = 6;

    fn values(vals: &[f32]) -> Value {
        msgmap! { ARRAY_VALUES => Value::Array(vals.iter().map(|v| Value::F32(*v)).collect()) }
    }

    /// An input slot, which opens a DSP path but carries no block.
    fn input() -> Value {
        msgmap! { KIND => Value::Int(INPUT) }
    }

    /// A block slot holding `model`, optionally with a cab riding in the same
    /// slot the way `paired` records it on the wire.
    fn block(
        model: u32,
        paired: Option<u32>,
        vals: &[f32],
        cab_vals: &[f32],
        enabled: bool,
    ) -> Value {
        let model_ref = Value::Map(vec![
            (Key::Int(MODEL), Value::Int(model as i64)),
            (
                Key::Int(PAIRED_MODEL),
                Value::Int(paired.map(|p| p as i64).unwrap_or(-1)),
            ),
        ]);
        let body = msgmap! {
            MODEL_REF => model_ref,
            ENABLED => Value::Bool(enabled),
            VALUES => values(vals),
            PAIRED_VALUES => values(cab_vals),
        };
        msgmap! { KIND => Value::Int(BLOCK), BODY => body }
    }

    /// Wrap a list of slots in the smallest blob `Preset::parse` accepts.
    fn preset(slots: Vec<Value>) -> Preset {
        let tone = msgmap! { PATH => msgmap! { SLOTS => Value::Array(slots) } };
        let mut blob = Encoder::encode(&Value::Str(Preset::MAGIC.into()));
        blob.extend(Encoder::encode(&Value::Bin(vec![0x3d], 0)));
        blob.extend(Encoder::encode(&tone));
        Preset::parse(&blob).expect("the synthetic blob parses")
    }

    /// The blocks a preset should read back as, derived straight from it through
    /// the same catalog the writer uses - amp then its cab, values folded the
    /// way a switch is - so a faithful round trip compares equal to it.
    fn expected(preset: &Preset, catalog: &Catalog) -> Vec<(u32, bool, BTreeMap<String, f32>)> {
        let mut out = Vec::new();
        let mut push = |number: u32, enabled: bool, vals: &[f32]| {
            let mut params = BTreeMap::new();
            for (i, v) in vals.iter().enumerate() {
                if let Some(param) = catalog.param(number, i) {
                    let folded = if param.kind == crate::Kind::Switch {
                        (*v >= 0.5) as u8 as f32
                    } else {
                        *v
                    };
                    params.insert(param.name.clone(), folded);
                }
            }
            out.push((number, enabled, params));
        };
        for (_, slot) in preset.blocks() {
            push(slot.model.unwrap(), slot.enabled, &slot.values);
            if let Some(cab) = slot.paired {
                push(cab, slot.enabled, &slot.paired_values);
            }
        }
        out
    }

    /// What the inspector read back, in the same shape as `expected`.
    fn read_back(tone: &crate::Tone) -> Vec<(u32, bool, BTreeMap<String, f32>)> {
        tone.blocks
            .iter()
            .map(|b| {
                (
                    b.model_number,
                    b.enabled,
                    b.params.iter().cloned().collect(),
                )
            })
            .collect()
    }

    /// First model number in a browse category, so the paired-cab test names a
    /// genuine amp and cab rather than pretending two effects are one.
    fn first_in_category(catalog: &Catalog, category: u32) -> Option<u32> {
        catalog
            .symbols()
            .iter()
            .find(|s| {
                s.model
                    .as_deref()
                    .and_then(|id| catalog.category_of(id))
                    .is_some_and(|c| c == category)
            })
            .map(|s| s.number)
    }

    #[test]
    fn a_blocks_models_params_and_bypass_round_trip() {
        let Some(catalog) = catalog() else { return };
        // Scream 808 (101) on, Room (247) bypassed - two effects whose parameter
        // counts are known, so the values map cleanly onto names both ways.
        let preset = preset(vec![
            input(),
            block(101, None, &[0.1, 0.2, 0.3], &[], true),
            // Room's Low/High Cut are native frequencies, not normalised
            // knobs. Keep the synthetic document inside the same bounds a
            // real pedal enforces so this remains a valid round-trip fixture.
            block(247, None, &[0.4, 0.05, 100.0, 5000.0, 0.8, 0.9], &[], false),
        ]);

        let written = to_hlx(&preset, &catalog, "Round Trip");
        assert!(written.skipped.is_empty(), "{:?}", written.skipped);

        let tone = inspect(&written.document, &catalog);
        assert_eq!(tone.name, "Round Trip");
        assert_eq!(read_back(&tone), expected(&preset, &catalog));

        // And spell out the facts the round trip stands on, so a regression
        // reads plainly rather than as a diff of two derived lists.
        assert_eq!(tone.models_used, vec![101, 247]);
        let scream = tone.blocks.iter().find(|b| b.model_number == 101).unwrap();
        assert!(scream.enabled);
        let room = tone.blocks.iter().find(|b| b.model_number == 247).unwrap();
        assert!(!room.enabled);
        let mix = room.params.iter().find(|(n, _)| n == "Mix").unwrap();
        assert!((mix.1 - 0.8).abs() < 1e-6, "Mix did not survive: {}", mix.1);
    }

    #[test]
    fn both_dsps_serialise_and_a_paired_cab_becomes_its_own_block() {
        let Some(catalog) = catalog() else { return };
        let (Some(amp), Some(cab)) = (
            first_in_category(&catalog, Category::AMP),
            first_in_category(&catalog, Category::CAB),
        ) else {
            eprintln!("SKIPPED: the catalog has no amp or cab to pair");
            return;
        };

        // Path 0: an amp with a cab riding in its slot. Path 1: a lone effect.
        let preset = preset(vec![
            input(),
            block(amp, Some(cab), &[0.5, 0.5], &[0.5, 0.5], true),
            input(),
            block(101, None, &[0.2, 0.3, 0.4], &[], true),
        ]);

        let written = to_hlx(&preset, &catalog, "Two DSPs");
        assert!(written.skipped.is_empty(), "{:?}", written.skipped);

        // The document carries both paths. The amp and its cab come apart into
        // two nodes, and HX Edit names the cab `cab0` rather than giving it a
        // block number - checked against its own output for 94 real presets.
        let dsp0 = written.document.pointer("/data/tone/dsp0").unwrap();
        let dsp1 = written.document.pointer("/data/tone/dsp1").unwrap();
        let dsp0 = dsp0.as_object().unwrap();
        assert!(dsp0.contains_key("block0"), "the amp");
        assert!(dsp0.contains_key("cab0"), "and its cab, split apart");
        assert!(
            dsp1.as_object().unwrap().contains_key("block0"),
            "the second path's effect"
        );

        let tone = inspect(&written.document, &catalog);
        assert!(tone.has_amp, "the amp survived");
        assert!(tone.has_cab_or_ir, "the paired cab became a real block");
        assert_eq!(tone.chain_content, ChainContent::FullRig);

        // Two blocks on dsp0 (amp, cab), one on dsp1 (the effect).
        assert_eq!(tone.blocks.iter().filter(|b| b.path == 0).count(), 2);
        let second = tone
            .blocks
            .iter()
            .find(|b| b.path == 1)
            .expect("a dsp1 block");
        assert_eq!(second.model_number, 101);
    }

    #[test]
    fn a_model_with_no_symbol_is_reported_not_dropped() {
        let Some(catalog) = catalog() else { return };
        // A number past the end of the symbol table cannot be named.
        let preset = preset(vec![input(), block(999_999, None, &[0.5], &[], true)]);

        let written = to_hlx(&preset, &catalog, "Unknown");
        assert_eq!(written.skipped.len(), 1, "{:?}", written.skipped);
        assert!(written.skipped[0].contains("999999"));

        // Nothing was written for it, and the inspector reads back an empty tone
        // rather than a mystery block.
        let tone = inspect(&written.document, &catalog);
        assert!(tone.blocks.is_empty());
    }
}

#[cfg(test)]
mod faithful_tests {
    use super::*;
    use crate::tests::catalog;

    fn fixture(name: &str) -> Option<Preset> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hx-proto/tests/fixtures")
            .join(name);
        Preset::parse(&std::fs::read(path).ok()?)
    }

    /// A written document carries the wiring, not just the blocks.
    ///
    /// This is what separates a tone file that reproduces a preset from one
    /// that merely lists what was in it: the endpoints say where the signal
    /// enters and leaves, and a split and join say where it forks and merges.
    /// Checked against HX Edit's own output for 94 real presets, every one of
    /// which agreed on nodes, model names and snapshots.
    #[test]
    fn the_wiring_is_written_out_with_the_blocks() {
        let Some(catalog) = catalog() else { return };
        let Some(preset) = fixture("gen-04-full-rig.hxpreset") else {
            return;
        };
        let written = to_hlx(&preset, &catalog, "Full Rig");
        let dsp0 = written.document["data"]["tone"]["dsp0"]
            .as_object()
            .expect("a dsp0 object");

        assert!(
            dsp0.contains_key("inputA"),
            "the signal has to enter somewhere"
        );
        assert!(dsp0.contains_key("outputA"), "and leave somewhere");
        assert_eq!(
            dsp0["outputA"]["@model"].as_str().unwrap(),
            "HelixStomp_AppDSPFlowOutputMain",
            "the A output is the main pair"
        );
        assert!(
            dsp0.keys().any(|k| k.starts_with("block")),
            "and pass through something on the way"
        );
    }

    /// Snapshots survive being written out.
    #[test]
    fn snapshots_are_written_with_what_each_switches_on() {
        let Some(catalog) = catalog() else { return };
        let Some(preset) = fixture("gen-08-snapshots.hxpreset") else {
            return;
        };
        let written = to_hlx(&preset, &catalog, "Snapshots");
        let tone = written.document["data"]["tone"].as_object().unwrap();

        let names = preset.snapshots();
        for (index, name) in names.iter().enumerate() {
            let snapshot = &tone[&format!("snapshot{index}")];
            assert_eq!(snapshot["@name"].as_str().unwrap(), name);
            // Each one remembers the state of the blocks, not just its name.
            assert!(
                snapshot["blocks"]["dsp0"]
                    .as_object()
                    .is_some_and(|b| !b.is_empty()),
                "snapshot {index} should record which blocks were on"
            );
        }
        assert!(!names.is_empty(), "the fixture has snapshots");
    }
}
