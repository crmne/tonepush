//! Reading a `.hlx` preset into "what this tone is", with no device attached.
//!
//! This is the read-only twin of the device-apply reader in `hx-cli`, and it
//! answers a different question. The apply reader turns a file into a list of
//! edits to send, so it is careful to touch only what it can address on the
//! wire - one DSP, no splits. Here nothing is sent, so nothing collides: we can
//! read both DSPs, every block, and describe the whole tone. That makes this
//! the piece a library browser or a tone list is built on, and it runs entirely
//! from the catalog and the JSON, never the pedal.
//!
//! Two readings deliberately differ from the apply path. It reads `dsp1` as well
//! as `dsp0`, keying blocks by `(path, position)` so the two never overwrite
//! each other. And it resolves a model by base name as well as exact symbol: a
//! `.hlx` often stores the shared name (`HD2_DistScream808`) while the symbol
//! table keeps mono and stereo apart (`...Mono`, `...Stereo`), so we fall back
//! to the variant the block says it is. Neither reading is safe for applying,
//! which is why the two readers are separate rather than shared.

use serde_json::Value;

use crate::{Catalog, Category};

/// Everything a `.hlx` says about a tone, derived without a device.
#[derive(Debug, Clone, PartialEq)]
pub struct Tone {
    pub name: String,
    pub blocks: Vec<ToneBlock>,
    /// The wire model numbers used, deduplicated, in the order they first
    /// appear along the chain.
    pub models_used: Vec<u32>,
    pub has_amp: bool,
    pub has_cab_or_ir: bool,
    pub chain_content: ChainContent,
    /// A guess, not a fact: the file does not record what the player plugs into,
    /// only whether the tone carries its own speaker.
    pub output_target_guess: OutputTarget,
    /// Models and parameters in the file we could not translate, kept so they
    /// can be reported rather than silently dropped - the same discipline as the
    /// apply reader.
    pub skipped: Vec<String>,
}

/// One block in the chain, identified across both DSPs by `(path, position)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneBlock {
    /// 0 for `dsp0`, 1 for `dsp1`.
    pub path: u8,
    /// The block's position within its DSP, from the `blockN` key.
    pub position: i64,
    pub model_number: u32,
    pub model_name: String,
    /// The browsable category id, per [`Catalog::category_of`]. `None` when the
    /// model resolves but sits in no browse category, which is rare.
    pub category: Option<u32>,
    pub enabled: bool,
    /// Parameter values by display name, in the order the JSON lists them.
    pub params: Vec<(String, f32)>,
}

/// What the chain is made of, in the broad strokes a browser sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainContent {
    /// An amp, a speaker (cab or IR), and effects around them.
    FullRig,
    /// An amp into a speaker, and nothing else.
    AmpAndCab,
    /// An amp with no speaker sim - effects may sit around it.
    AmpOnly,
    /// No amp at all: a pedalboard, meant to feed one.
    EffectsOnly,
}

/// A guess at what the tone is meant to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTarget {
    /// The tone carries its own speaker (a cab or IR), so it is voiced for a
    /// full-range system: an FRFR speaker, a PA, or a recording interface.
    FrfrPa,
    /// No speaker sim, so the tone expects a real guitar cab to colour it, or a
    /// direct feed that leaves it uncoloured.
    GuitarCabOrDi,
}

/// Parse a `.hlx` preset document into a [`Tone`].
///
/// Total by design: a file missing its `data` or `tone` object yields an empty,
/// unnamed tone rather than an error, because a browser scanning a folder should
/// skip a stray file, not stop on it.
pub fn inspect(json: &Value, catalog: &Catalog) -> Tone {
    let data = json.get("data");
    let tone = data.and_then(|d| d.get("tone"));

    let name = data
        .and_then(|d| d.pointer("/meta/name"))
        .and_then(Value::as_str)
        .unwrap_or("(unnamed)")
        .to_owned();

    let mut blocks = Vec::new();
    let mut skipped = Vec::new();

    // Both DSPs, keyed by (path, position) so dsp1/block0 never lands on
    // dsp0/block0 the way it would if it were an edit address.
    for (path, key) in [(0u8, "dsp0"), (1u8, "dsp1")] {
        let Some(entries) = tone.and_then(|t| t.get(key)).and_then(Value::as_object) else {
            continue;
        };
        // Blocks come out of the map in key order, which is not chain order;
        // sort by position so the chain reads front to back.
        // blockN keys are the chain, in the order their numbers say. Inputs,
        // outputs, split and join are the topology rather than the tone.
        let mut positioned: Vec<(i64, &Value)> = entries
            .iter()
            .filter_map(|(k, v)| {
                let n = k.strip_prefix("block")?.parse::<i64>().ok()?;
                Some((n, v))
            })
            .collect();
        positioned.sort_by_key(|(n, _)| *n);

        // A cab is written under its own name - `cab0` - and carries no
        // position, because it belongs to an amp rather than to a place in the
        // line. HX Edit reconstructs which one by order, and so does this: the
        // Nth cab follows the Nth amp.
        let mut cabs: Vec<(i64, &Value)> = entries
            .iter()
            .filter_map(|(k, v)| {
                let n = k.strip_prefix("cab")?.parse::<i64>().ok()?;
                Some((n, v))
            })
            .collect();
        cabs.sort_by_key(|(n, _)| *n);
        if !cabs.is_empty() {
            let is_amp_node = |v: &Value| {
                is_amp(
                    v.get("@model")
                        .and_then(Value::as_str)
                        .and_then(|id| catalog.category_of(id)),
                )
            };
            let mut merged = Vec::with_capacity(positioned.len() + cabs.len());
            let mut cabs = cabs.into_iter();
            for (n, block) in positioned {
                let amp = is_amp_node(block);
                merged.push((n, block));
                if amp {
                    if let Some((_, cab)) = cabs.next() {
                        merged.push((n, cab));
                    }
                }
            }
            // A cab with no amp before it still belongs in the chain.
            merged.extend(cabs);
            positioned = merged;
        }

        for (position, block) in positioned {
            read_block(&mut blocks, &mut skipped, catalog, path, position, block);
        }
    }

    let has_amp = blocks.iter().any(|b| is_amp(b.category));
    let has_cab_or_ir = blocks.iter().any(|b| is_cab_or_ir(b.category));
    let has_effects = blocks.iter().any(|b| is_effect(b.category));

    let chain_content = match (has_amp, has_cab_or_ir, has_effects) {
        (true, true, true) => ChainContent::FullRig,
        (true, true, false) => ChainContent::AmpAndCab,
        (true, false, _) => ChainContent::AmpOnly,
        (false, _, _) => ChainContent::EffectsOnly,
    };

    // A cab or IR sits at the tail of the amp stage - time-based effects may
    // follow it, so its presence, not its position, is the signal. With one the
    // speaker is modelled and the tone wants a full-range system; without one it
    // wants a real cab to finish it, or a DI that leaves it alone.
    let output_target_guess = if has_cab_or_ir {
        OutputTarget::FrfrPa
    } else {
        OutputTarget::GuitarCabOrDi
    };

    let mut models_used = Vec::new();
    for block in &blocks {
        if !models_used.contains(&block.model_number) {
            models_used.push(block.model_number);
        }
    }

    Tone {
        name,
        blocks,
        models_used,
        has_amp,
        has_cab_or_ir,
        chain_content,
        output_target_guess,
        skipped,
    }
}

fn read_block(
    blocks: &mut Vec<ToneBlock>,
    skipped: &mut Vec<String>,
    catalog: &Catalog,
    path: u8,
    position: i64,
    block: &Value,
) {
    let Some(symbol) = block.get("@model").and_then(Value::as_str) else {
        skipped.push(format!("dsp{path}/block{position}: no valid @model"));
        return;
    };
    let stereo = match block.get("@stereo") {
        None => false,
        Some(value) => match value.as_bool() {
            Some(stereo) => stereo,
            None => {
                skipped.push(format!(
                    "dsp{path}/block{position}: @stereo is not a boolean value"
                ));
                return;
            }
        },
    };

    let Some(number) = resolve(catalog, symbol, stereo) else {
        skipped.push(format!("dsp{path}/block{position}: unknown model {symbol}"));
        return;
    };

    let model = catalog.model_number(number);
    let model_name = model
        .map(|m| m.name.clone())
        .unwrap_or_else(|| symbol.to_owned());
    let category = model.and_then(|m| catalog.category_of(&m.id));

    // No `@enabled` means on: the endpoints and a handful of blocks omit it, and
    // treating them as bypassed would misread the tone.
    let enabled = match block.get("@enabled") {
        None => true,
        Some(value) => match value.as_bool() {
            Some(enabled) => enabled,
            None => {
                skipped.push(format!(
                    "dsp{path}/block{position}: @enabled is not a boolean value"
                ));
                return;
            }
        },
    };

    let mut params = Vec::new();
    for (key, value) in block.as_object().into_iter().flatten() {
        // `@`-prefixed keys are structural - model, position, stereo - not knobs.
        if key.starts_with('@') {
            continue;
        }
        let Some(index) = catalog.param_index(number, key) else {
            skipped.push(format!("{model_name}: no parameter {key:?}"));
            continue;
        };
        let Some(param) = catalog.param(number, index) else {
            continue;
        };
        // A `.hlx` stores values in the native units the wire uses, so no
        // conversion; a switch is written as a bool and folds to 0.0/1.0.
        let native = match value {
            Value::Bool(b) => *b as u8 as f32,
            Value::Number(n) => match n.as_f64() {
                Some(number) => number as f32,
                None => {
                    skipped.push(format!(
                        "{model_name}: {} is not a finite number",
                        param.name
                    ));
                    continue;
                }
            },
            _ => {
                skipped.push(format!(
                    "{model_name}: {} is not a numeric parameter value",
                    param.name
                ));
                continue;
            }
        };
        if !param.accepts(native) {
            skipped.push(format!(
                "{model_name}: {} value {native} is outside {}..{}",
                param.name, param.min, param.max
            ));
            continue;
        }
        params.push((param.name.clone(), native));
    }

    blocks.push(ToneBlock {
        path,
        position,
        model_number: number,
        model_name,
        category,
        enabled,
        params,
    });
}

/// Resolve a `.hlx` `@model` to a wire model number.
///
/// Exact first, which covers names a file stores with their own suffix. Failing
/// that the file gave the shared base name, and the symbol table keeps mono and
/// stereo apart, so we try the variant the block declares itself to be and then
/// the other. This is why the inspector reads presets the exact-match apply path
/// would report as unknown.
fn resolve(catalog: &Catalog, symbol: &str, stereo: bool) -> Option<u32> {
    let symbols = catalog.symbols();
    if let Some(s) = symbols.iter().find(|s| s.symbol == symbol) {
        return Some(s.number);
    }
    let order = if stereo {
        ["Stereo", "Mono"]
    } else {
        ["Mono", "Stereo"]
    };
    order.into_iter().find_map(|suffix| {
        let name = format!("{symbol}{suffix}");
        symbols.iter().find(|s| s.symbol == name).map(|s| s.number)
    })
}

fn is_amp(category: Option<u32>) -> bool {
    matches!(category, Some(Category::AMP) | Some(Category::PREAMP))
}

fn is_cab_or_ir(category: Option<u32>) -> bool {
    matches!(category, Some(Category::CAB) | Some(Category::IR))
}

/// A genuine effect: a resolved block that is neither amp nor speaker. The
/// structural categories never reach here - they are not `blockN` entries.
fn is_effect(category: Option<u32>) -> bool {
    category.is_some_and(|_| !is_amp(category) && !is_cab_or_ir(category))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::catalog;

    #[test]
    fn parses_a_block_on_each_dsp() {
        let Some(catalog) = catalog() else { return };
        // dsp0 uses the shared base name; dsp1 an explicit stereo symbol. Both
        // must resolve, and land on their own path.
        let json = serde_json::json!({
            "data": { "meta": { "name": "Two DSPs" }, "tone": {
                "dsp0": { "block0": { "@model": "HD2_DistScream808", "@enabled": true } },
                "dsp1": { "block0": { "@model": "HD2_ReverbRoomStereo", "@enabled": false } }
            }}
        });

        let tone = inspect(&json, &catalog);
        assert_eq!(tone.name, "Two DSPs");
        assert!(tone.skipped.is_empty(), "{:?}", tone.skipped);
        assert_eq!(tone.blocks.len(), 2, "both DSPs parsed");

        let dsp0 = tone
            .blocks
            .iter()
            .find(|b| b.path == 0)
            .expect("a dsp0 block");
        assert_eq!(dsp0.position, 0);
        assert_eq!(dsp0.model_name, "Scream 808", "base name resolved");
        assert!(dsp0.enabled);

        let dsp1 = tone
            .blocks
            .iter()
            .find(|b| b.path == 1)
            .expect("a dsp1 block");
        assert_eq!(dsp1.model_name, "Room");
        assert!(!dsp1.enabled);
    }

    #[test]
    fn reports_a_block_without_a_valid_model() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": 101 }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone.blocks.is_empty());
        assert!(tone
            .skipped
            .iter()
            .any(|why| why.contains("block0") && why.contains("@model")));
    }

    #[test]
    fn reports_a_nonnumeric_parameter_value() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808Mono",
                    "Gain": "loud"
                }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone
            .skipped
            .iter()
            .any(|why| why.contains("Gain") && why.contains("not a numeric")));
        assert!(!tone.blocks[0].params.iter().any(|(name, _)| name == "Gain"));
    }

    #[test]
    fn reports_a_malformed_bypass_flag() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808Mono",
                    "@enabled": "yes"
                }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone.blocks.is_empty());
        assert!(tone
            .skipped
            .iter()
            .any(|why| why.contains("@enabled") && why.contains("not a boolean")));
    }

    #[test]
    fn reports_a_malformed_stereo_flag() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808",
                    "@stereo": "wide"
                }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone.blocks.is_empty());
        assert!(tone
            .skipped
            .iter()
            .any(|why| why.contains("@stereo") && why.contains("not a boolean")));
    }

    #[test]
    fn an_amp_into_a_cab_is_a_rig_bound_for_frfr() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "meta": { "name": "Rig" }, "tone": { "dsp0": {
                "block0": { "@model": "HD2_AmpUSDoubleVib", "@stereo": false },
                "block1": { "@model": "HD2_CabMicIr_2x12DoubleC12N" }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone.skipped.is_empty(), "{:?}", tone.skipped);
        assert!(tone.has_amp);
        assert!(tone.has_cab_or_ir);
        assert_eq!(tone.chain_content, ChainContent::AmpAndCab);
        assert_eq!(tone.output_target_guess, OutputTarget::FrfrPa);

        // The amp is category Amp, the cab category Cab - the facts come from
        // the catalog, not from any guess in the file.
        let amp = tone
            .blocks
            .iter()
            .find(|b| b.model_name.contains("Double"))
            .unwrap();
        assert_eq!(amp.category, Some(Category::AMP));
    }

    #[test]
    fn an_amp_with_a_pedal_and_no_cab_stays_on_the_amp() {
        let Some(catalog) = catalog() else { return };
        // An amp with a drive in front but no speaker sim: still amp-shaped, but
        // it expects a real cab downstream, so the output guess flips.
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": "HD2_DistScream808" },
                "block1": { "@model": "HD2_AmpUSDoubleVib" }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone.has_amp);
        assert!(!tone.has_cab_or_ir);
        assert_eq!(tone.chain_content, ChainContent::AmpOnly);
        assert_eq!(tone.output_target_guess, OutputTarget::GuitarCabOrDi);
        assert_eq!(tone.models_used.len(), 2, "two distinct models");
    }

    #[test]
    fn an_unknown_model_is_reported_not_dropped() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": "HD2_NotARealModel" },
                "block1": { "@model": "HD2_DistScream808", "Nonsense": 1.0 }
            }}}
        });

        let tone = inspect(&json, &catalog);
        // The unknown model is skipped, its one real neighbour kept.
        assert_eq!(tone.blocks.len(), 1, "the known block survives");
        assert_eq!(tone.blocks[0].model_name, "Scream 808");
        assert_eq!(tone.chain_content, ChainContent::EffectsOnly);

        assert_eq!(tone.skipped.len(), 2, "{:?}", tone.skipped);
        assert!(tone.skipped.iter().any(|s| s.contains("HD2_NotARealModel")));
        assert!(tone.skipped.iter().any(|s| s.contains("Nonsense")));
    }

    #[test]
    fn an_out_of_range_parameter_is_reported_and_never_made_loadable() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808Mono",
                    "Gain": 1.5
                }
            }}}
        });

        let tone = inspect(&json, &catalog);
        assert!(tone
            .skipped
            .iter()
            .any(|why| why.contains("Gain") && why.contains("outside")));
        assert!(tone.blocks[0].params.is_empty());
    }
}
