//! Reading Line 6's `.hlx` preset files.
//!
//! An `.hlx` is plain JSON keyed by symbolic names - `"@model": "HD2_AmpCaliRectifire"`,
//! `"Drive": 0.68`. The device speaks numbers, so applying one means translating
//! through the catalog: symbol to model number, parameter name to position.
//!
//! Applying happens as a list of ordinary edits - set the model, then each
//! parameter, then the bypass state - rather than by synthesising a whole
//! preset document and writing it back. Those edits are individually verified
//! against hardware, whereas a synthesised document is not, and a rejected
//! document loses the whole preset rather than one parameter.

use std::path::Path;

use anyhow::{Context, Result};
use hx_catalog::Catalog;

/// One edit to make on the device.
#[derive(Debug, PartialEq)]
pub enum Step {
    Model {
        block: i64,
        model: u32,
        name: String,
        /// The cab riding along with an amp, making the block an Amp+Cab.
        cab: Option<Cab>,
    },
    Param {
        block: i64,
        index: i64,
        value: f32,
        switch: bool,
        name: String,
    },
    Enabled {
        block: i64,
        enabled: bool,
    },
}

/// The cab half of an Amp+Cab block.
#[derive(Debug, PartialEq)]
pub struct Cab {
    pub model: u32,
    pub name: String,
}

/// What a file would do, before anything is sent.
#[derive(Debug, Default)]
pub struct Plan {
    pub name: String,
    pub steps: Vec<Step>,
    /// Things in the file we could not translate, kept so they can be reported
    /// rather than silently dropped.
    pub skipped: Vec<String>,
}

#[cfg(test)]
pub fn read(path: &Path, catalog: &Catalog) -> Result<Plan> {
    read_for(path, catalog, None)
}

/// The same, against the chain the file is going onto. See [`plan_for`].
pub fn read_for(
    path: &Path,
    catalog: &Catalog,
    layout: Option<&hx_proto::preset::Layout>,
) -> Result<Plan> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {path:?} as JSON"))?;
    plan_for(&json, catalog, layout)
}

#[cfg(test)]
pub fn plan(json: &serde_json::Value, catalog: &Catalog) -> Result<Plan> {
    plan_for(json, catalog, None)
}

/// The same, against the chain the file is going onto.
///
/// A `.hlx` says where a block sits as `@path` and `@position`: the branch, and
/// the place along that branch's drawn row. That is only the device's own slot
/// number on a chain that does not split, so translating it needs the target's
/// layout. Without one the file is read the way it always was - the `blockN`
/// number as the slot - which is right for a straight chain and wrong the
/// moment there is a split.
pub fn plan_for(
    json: &serde_json::Value,
    catalog: &Catalog,
    layout: Option<&hx_proto::preset::Layout>,
) -> Result<Plan> {
    let data = json
        .get("data")
        .context("no `data` object; is this an .hlx preset?")?;
    let tone = data.get("tone").context("no `data.tone` object")?;

    let mut plan = Plan {
        name: data
            .pointer("/meta/name")
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)")
            .to_owned(),
        ..Default::default()
    };

    // A `dspN` is a whole signal path, which only hardware with two DSPs has;
    // a *branch* of one path is `@path` inside the same dsp. Reading dsp1 as a
    // branch was what made its blocks land on dsp0's and get skipped instead.
    for dsp_index in 0.. {
        let name = format!("dsp{dsp_index}");
        let Some(blocks) = tone.get(&name).and_then(|d| d.as_object()) else {
            break;
        };
        let mut claimed_cabs = std::collections::BTreeSet::new();
        for (key, node) in blocks {
            // split, join, inputs and outputs are the wiring, not the tone, and
            // they are not addressable as a block.
            let Some(numbered) = key
                .strip_prefix("block")
                .and_then(|n| n.parse::<i64>().ok())
            else {
                continue;
            };
            // An Amp+Cab is one slot on the device and two nodes in the file:
            // the amp as `blockN` and its cab as `cabN` beside it. Leaving the
            // cab out put the amp in alone, its cab gone without a word.
            let cab = cab_of(blocks, node);
            if let Ok(Some((cab_key, _))) = &cab {
                claimed_cabs.insert(cab_key.to_owned());
            }
            match slot_of(node, layout, dsp_index, numbered) {
                Ok(position) => read_block(&mut plan, position, node, cab, catalog, Bypass::Has),
                Err(why) => plan.skipped.push(format!("{name}/{key}: {why}")),
            }
        }
        for key in blocks.keys().filter(|key| is_cab_key(key)) {
            if !claimed_cabs.contains(key.as_str()) {
                plan.skipped
                    .push(format!("{name}/{key}: no block in the file names this cab"));
            }
        }

        // The wiring, which on the device is a slot holding a model like any
        // other: changing a split from a Y to an A/B is the same set-model a
        // block takes, which is exactly what the editor's Type chips send. So
        // an import can carry it, and a Y read as an A/B - which divides the
        // signal differently - no longer arrives as whatever the chain already
        // had.
        //
        // The junction's own slot comes from the chain being imported onto,
        // never from the file: a `.hlx` says where the split *attaches*, and
        // writing an attach point is the one edit that can wipe an edit buffer
        // (see `docs`, and `Preset::settle_branches`). The type and the
        // parameters are safe; where the branch begins is the device's.
        let path = layout.and_then(|l| l.paths.get(dsp_index));
        for (node_key, slot) in [
            ("split", path.and_then(|p| p.split)),
            ("join", path.and_then(|p| p.join)),
        ] {
            let Some(node) = blocks.get(node_key) else {
                continue;
            };
            if node.get("@model").is_none() {
                continue;
            }
            match slot {
                Some(slot) => {
                    read_block(
                        &mut plan,
                        slot as i64,
                        node,
                        Ok(None),
                        catalog,
                        Bypass::None,
                    );
                }
                // Without a layout there is no way to know which slot holds it,
                // and on a chain that does not divide there is nothing to hold.
                None if layout.is_some() => plan
                    .skipped
                    .push(format!("{name}/{node_key}: this chain has no {node_key}")),
                None => plan.skipped.push(format!(
                    "{name}/{node_key}: placing it needs the chain it is going onto"
                )),
            }
        }
    }

    plan.steps.sort_by_key(|s| match s {
        Step::Model { block, .. } | Step::Param { block, .. } | Step::Enabled { block, .. } => {
            *block
        }
    });
    Ok(plan)
}

/// Which slot a block goes in, said three ways in decreasing exactness: our own
/// `@slot`, HX Edit's `@path` and `@position` against the target's layout, and
/// the `blockN` number for a file that carries neither.
fn slot_of(
    node: &serde_json::Value,
    layout: Option<&hx_proto::preset::Layout>,
    dsp: usize,
    numbered: i64,
) -> Result<i64, String> {
    // Ours, and exact: the device's own index, which needs nothing to read it.
    if let Some(slot) = placement_number(node, "@slot")? {
        return i64::try_from(slot).map_err(|_| format!("slot {slot} is too large"));
    }
    let position = placement_number(node, "@position")?;
    let branch = placement_number(node, "@path")?.unwrap_or(0);
    let branch =
        usize::try_from(branch).map_err(|_| "the branch number is too large".to_owned())?;
    if let (Some(layout), Some(position)) = (layout, position) {
        let position =
            usize::try_from(position).map_err(|_| "the position number is too large".to_owned())?;
        return layout
            .slot_of(dsp, branch, position)
            .and_then(|slot| i64::try_from(slot).ok())
            .ok_or_else(|| {
                format!("this chain has no path {dsp} branch {branch} position {position}")
            });
    }
    // A row index and no chain to read it against. It is only a slot number
    // when there is one path and one row; a second DSP or a lower branch would
    // land on the main line and overwrite what is there.
    if dsp > 0 || branch > 0 {
        return Err(format!(
            "on path {dsp} branch {branch}; placing it needs the chain it is going onto"
        ));
    }
    position.map_or(Ok(numbered), |position| {
        i64::try_from(position).map_err(|_| format!("position {position} is too large"))
    })
}

fn placement_number(node: &serde_json::Value, field: &str) -> Result<Option<u64>, String> {
    match node.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{field} is not a non-negative integer")),
    }
}

/// Whether the slot being read is one that can be switched off. A block can; a
/// split or a join is wiring and has no bypass to set.
#[derive(PartialEq)]
enum Bypass {
    Has,
    None,
}

/// A file's `cabN` node, as opposed to a block, split or join.
fn is_cab_key(key: &str) -> bool {
    key.strip_prefix("cab")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The cab node riding with a block, said one of two ways, as the document
/// builder reads it (`hx_catalog::slots_from_hlx`): HX Edit has the amp name
/// its cab, `"@cab": "cab0"`; our own files have the cab name the amp's slot.
fn cab_of<'a>(
    blocks: &'a serde_json::Map<String, serde_json::Value>,
    node: &serde_json::Value,
) -> Result<Option<(&'a str, &'a serde_json::Value)>, String> {
    if let Some(named) = node.get("@cab") {
        let key = named
            .as_str()
            .ok_or_else(|| "@cab is not the name of a cab".to_owned())?;
        return blocks
            .get_key_value(key)
            .map(|(key, cab)| Some((key.as_str(), cab)))
            .ok_or_else(|| format!("its cab {key} is not in the file"));
    }
    let Some(slot) = node.get("@slot").and_then(serde_json::Value::as_u64) else {
        return Ok(None);
    };
    Ok(blocks
        .iter()
        .filter(|(key, _)| is_cab_key(key))
        .find(|(_, cab)| cab.get("@slot").and_then(serde_json::Value::as_u64) == Some(slot))
        .map(|(key, cab)| (key.as_str(), cab)))
}

/// The cab to pair with the amp called `amp`, or `None` with the reason
/// recorded. Its own parameters are reported rather than applied: the
/// device takes a cab as part of the amp's set-model, and there is no
/// verified message for setting a paired cab's values, so it starts from
/// its defaults.
fn read_cab(
    plan: &mut Plan,
    amp: &str,
    node: &serde_json::Value,
    catalog: &Catalog,
) -> Option<Cab> {
    let Some(symbol) = node.get("@model").and_then(|v| v.as_str()) else {
        plan.skipped.push(format!(
            "{amp}: its cab has no valid @model; placed without it"
        ));
        return None;
    };
    let Some(model) = hx_catalog::resolve(catalog, symbol, node) else {
        plan.skipped.push(format!(
            "{amp}: unknown cab model {symbol}; placed without it"
        ));
        return None;
    };
    let name = catalog
        .model_number(model.number)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| symbol.to_owned());
    // Named from the model's own parameter list rather than by skipping `@`
    // keys: a cab's parameters are `@`-prefixed themselves (`@mic`).
    let settings: Vec<String> = catalog
        .model_number(model.number)
        .into_iter()
        .flat_map(|m| &m.params)
        .filter(|param| node.get(param.id.as_str()).is_some())
        .map(|param| param.name.clone())
        .collect();
    if !settings.is_empty() {
        plan.skipped.push(format!(
            "{amp}: the {name} cab's own settings ({}) are not applied; it starts from its defaults",
            settings.join(", ")
        ));
    }
    Some(Cab {
        model: model.number,
        name,
    })
}

fn read_block(
    plan: &mut Plan,
    position: i64,
    block: &serde_json::Value,
    cab: Result<Option<(&str, &serde_json::Value)>, String>,
    catalog: &Catalog,
    bypass: Bypass,
) {
    let Some(symbol) = block.get("@model").and_then(|v| v.as_str()) else {
        plan.skipped
            .push(format!("block{position}: no valid @model"));
        return;
    };
    if block
        .get("@stereo")
        .is_some_and(|value| !value.is_boolean())
    {
        plan.skipped
            .push(format!("block{position}: @stereo is not a boolean value"));
        return;
    }
    // The same resolution the document builder uses. HX Edit writes the *shared*
    // model id - `HD2_ReverbPlate` - where the firmware has a mono symbol and a
    // stereo one, each with its own wire number. An exact match on the symbol
    // found neither, so importing a genuine HX Edit file skipped most of its
    // chain and said only "unknown model".
    let Some(model) = hx_catalog::resolve(catalog, symbol, block) else {
        plan.skipped
            .push(format!("block{position}: unknown model {symbol}"));
        return;
    };

    let name = catalog
        .model_number(model.number)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| symbol.to_owned());
    let cab = match cab {
        Ok(Some((_, node))) => read_cab(plan, &name, node, catalog),
        Ok(None) => None,
        Err(why) => {
            plan.skipped
                .push(format!("{name}: {why}; placed without its cab"));
            None
        }
    };
    plan.steps.push(Step::Model {
        block: position,
        model: model.number,
        name: name.clone(),
        cab,
    });

    for (key, value) in block.as_object().into_iter().flatten() {
        // `@`-prefixed keys are structural - model, position, stereo - and are
        // not parameters.
        if key.starts_with('@') {
            if key == "@enabled" && bypass == Bypass::Has {
                match value.as_bool() {
                    Some(on) => plan.steps.push(Step::Enabled {
                        block: position,
                        enabled: on,
                    }),
                    None => plan
                        .skipped
                        .push(format!("{name}: @enabled is not a boolean value")),
                }
            }
            continue;
        }

        let Some(index) = catalog.param_index(model.number, key) else {
            plan.skipped.push(format!("{name}: no parameter {key:?}"));
            continue;
        };
        let Some(param) = catalog.param(model.number, index) else {
            continue;
        };

        // .hlx stores values in the same native units the wire uses, so no
        // conversion - but a switch is written as a bool.
        let native = match value {
            serde_json::Value::Bool(b) => *b as u8 as f32,
            serde_json::Value::Number(n) => match n.as_f64() {
                Some(number) => number as f32,
                None => {
                    plan.skipped
                        .push(format!("{name}: {} is not a finite number", param.name));
                    continue;
                }
            },
            _ => {
                plan.skipped.push(format!(
                    "{name}: {} is not a numeric parameter value",
                    param.name
                ));
                continue;
            }
        };
        if !param.accepts(native) {
            plan.skipped.push(format!(
                "{name}: {} value {native} is outside {}..{}",
                param.name, param.min, param.max
            ));
            continue;
        }
        plan.steps.push(Step::Param {
            block: position,
            index: index as i64,
            value: native,
            switch: param.kind == hx_catalog::Kind::Switch,
            name: param.name.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skip only when HX Edit is absent. A catalog that is present but will
    /// not load is a real failure and must not pass quietly.
    fn catalog() -> Option<Catalog> {
        match Catalog::load() {
            Ok(c) => Some(c),
            Err(hx_catalog::Error::NotInstalled(_)) => {
                eprintln!("SKIPPED: HX Edit is not installed, so the catalog cannot be read");
                None
            }
            Err(e) => panic!("HX Edit is installed but its catalog failed to load: {e}"),
        }
    }

    #[test]
    fn reads_a_preset_shipped_with_hx_edit() {
        let Some(catalog) = catalog() else { return };
        let path = hx_catalog::resources_dir()
            .unwrap()
            .join("default_preset.hlx");
        // Extracted resources vary by HX Edit version; a set without the
        // default preset is a machine to skip on, not a failure.
        if !path.exists() {
            return;
        }
        let plan = read(&path, &catalog).expect("reads the shipped default preset");

        assert_eq!(plan.name, "New Preset");
        // The default preset is empty, so it should ask for nothing and, more
        // importantly, should not silently skip things it did not understand.
        assert!(
            plan.skipped.is_empty(),
            "unexpected skips: {:?}",
            plan.skipped
        );
    }

    #[test]
    fn translates_a_block_into_edits() {
        let Some(catalog) = catalog() else { return };
        // Scream 808 is model 101 with Gain, Tone, Level.
        let json = serde_json::json!({
            "data": {
                "meta": { "name": "Test" },
                "tone": {
                    "dsp0": {
                        "block2": {
                            "@model": "HD2_DistScream808Mono",
                            "@enabled": true,
                            "Gain": 0.25,
                            "Level": 0.5
                        }
                    }
                }
            }
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
        assert!(plan.steps.contains(&Step::Model {
            block: 2,
            model: 101,
            name: "Scream 808".into(),
            cab: None,
        }));
        assert!(plan.steps.iter().any(|s| matches!(
            s,
            Step::Param { block: 2, index: 0, value, name, .. }
                if name == "Gain" && (*value - 0.25).abs() < 1e-6
        )));
        assert!(plan.steps.contains(&Step::Enabled {
            block: 2,
            enabled: true
        }));
    }

    #[test]
    fn a_second_dsp_is_reported_rather_than_misapplied() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": {
                "dsp0": { "block0": { "@model": "HD2_DistScream808Mono" } },
                "dsp1": { "block0": { "@model": "HD2_ReverbRoomStereo" } }
            }}
        });

        let plan = plan(&json, &catalog).unwrap();
        // Only dsp0's block is applied. Without the target's layout there is
        // nothing to say where a second path begins, and dsp1/block0 read as a
        // slot number would land on dsp0/block0.
        assert_eq!(
            plan.steps
                .iter()
                .filter(|s| matches!(s, Step::Model { .. }))
                .count(),
            1
        );
        assert!(
            plan.skipped.iter().any(|s| s.contains("dsp1")),
            "{:?}",
            plan.skipped
        );
    }

    /// The numbers HX Edit writes are places along a drawn row, not device
    /// slots, and the two part company the moment a chain splits. Read against
    /// the chain it is going onto, a block on the lower branch lands there.
    ///
    /// The layout here is a real one: the HX Stomp's factory `DIR:Relief`, whose
    /// blocks the device holds in slots 1, 6 and 12 to 15 and whose HX Edit
    /// export numbers 0, 5 and 1 to 4.
    #[test]
    fn a_branch_is_placed_against_the_target_chain() {
        use hx_proto::preset::{Lane, Layout, Path};
        let Some(catalog) = catalog() else { return };
        let layout = Layout {
            paths: vec![Path {
                input: Some(0),
                output: Some(9),
                split: Some(10),
                join: Some(19),
                head: vec![1],
                lanes: vec![
                    Lane {
                        branch: 0,
                        blocks: vec![6],
                        span: 2..7,
                    },
                    Lane {
                        branch: 1,
                        blocks: vec![12],
                        span: 11..19,
                    },
                ],
                tail: vec![],
            }],
        };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": "HD2_DistScream808Mono", "@path": 0, "@position": 0 },
                "block1": { "@model": "HD2_ReverbPlate", "@path": 0, "@position": 5 },
                "block2": { "@model": "HD2_ReverbRoomStereo", "@path": 1, "@position": 1 }
            }}}
        });

        let plan = plan_for(&json, &catalog, Some(&layout)).unwrap();
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
        let mut slots: Vec<i64> = plan
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Model { block, .. } => Some(*block),
                _ => None,
            })
            .collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![1, 6, 12]);
    }

    /// Our own files say the slot outright, which needs no layout to read.
    #[test]
    fn our_own_slot_number_is_taken_as_written() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": "HD2_DistScream808Mono", "@slot": 12 }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
        assert!(plan
            .steps
            .iter()
            .any(|s| matches!(s, Step::Model { block: 12, .. })));
    }

    #[test]
    fn oversized_file_positions_are_reported_instead_of_wrapping_negative() {
        let too_large = i64::MAX as u64 + 1;
        assert!(slot_of(&serde_json::json!({ "@slot": too_large }), None, 0, 0).is_err());
        assert!(slot_of(&serde_json::json!({ "@position": too_large }), None, 0, 0).is_err());
    }

    #[test]
    fn malformed_file_positions_are_rejected_instead_of_relocated() {
        assert!(slot_of(&serde_json::json!({ "@slot": "first" }), None, 0, 7).is_err());
        assert!(slot_of(&serde_json::json!({ "@position": -1 }), None, 0, 7).is_err());
        assert!(slot_of(
            &serde_json::json!({ "@path": "lower", "@position": 1 }),
            None,
            0,
            7
        )
        .is_err());
    }

    /// A file's split is applied to the slot the *chain* keeps its split in,
    /// as an ordinary set-model, and its parameters follow. What is never taken
    /// from the file is `@attach`: where a branch begins belongs to the device,
    /// and writing one over an empty branch can take the edit buffer with it.
    #[test]
    fn a_split_type_is_applied_to_the_chain_that_has_one() {
        use hx_proto::preset::{Lane, Layout, Path};
        let Some(catalog) = catalog() else { return };
        let layout = Layout {
            paths: vec![Path {
                input: Some(0),
                output: Some(9),
                split: Some(10),
                join: Some(19),
                head: vec![1],
                lanes: vec![
                    Lane {
                        branch: 0,
                        blocks: vec![],
                        span: 2..7,
                    },
                    Lane {
                        branch: 1,
                        blocks: vec![],
                        span: 11..19,
                    },
                ],
                tail: vec![],
            }],
        };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                // The shape HX Edit writes, `@attach` and all.
                "split": { "@model": "HD2_AppDSPFlowSplitY", "@attach": 2, "Balance A": 0.25 },
                "join": { "@model": "HD2_AppDSPFlowJoin", "@attach": 6 }
            }}}
        });

        let plan = plan_for(&json, &catalog, Some(&layout)).unwrap();
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
        let slots: Vec<i64> = plan
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Model { block, .. } => Some(*block),
                _ => None,
            })
            .collect();
        assert_eq!(slots, vec![10, 19], "the chain's own junction slots");
        assert!(
            plan.steps
                .iter()
                .any(|s| matches!(s, Step::Param { block: 10, .. })),
            "the split's own parameters travel with it: {:?}",
            plan.steps
        );
        assert!(
            !plan.steps.iter().any(|s| matches!(s, Step::Enabled { .. })),
            "wiring has no bypass to switch: {:?}",
            plan.steps
        );
    }

    /// And on a chain that does not divide there is nowhere to put one, which
    /// is said rather than guessed at.
    #[test]
    fn a_split_needs_a_chain_that_has_somewhere_to_put_it() {
        use hx_proto::preset::{Layout, Path};
        let Some(catalog) = catalog() else { return };
        let straight = Layout {
            paths: vec![Path {
                input: Some(0),
                output: Some(9),
                split: None,
                join: None,
                head: vec![1],
                lanes: vec![],
                tail: vec![],
            }],
        };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "split": { "@model": "HD2_AppDSPFlowSplitY", "@attach": 2 }
            }}}
        });

        let plan = plan_for(&json, &catalog, Some(&straight)).unwrap();
        assert!(plan.steps.is_empty());
        assert!(
            plan.skipped.iter().any(|s| s.contains("no split")),
            "{:?}",
            plan.skipped
        );
    }

    /// An amp and the cab it pairs with, from the catalog, as wire numbers.
    fn amp_and_cab(catalog: &Catalog) -> (String, u32, String, u32, (String, String)) {
        let amp = catalog
            .models()
            .find(|m| catalog.paired_cab(m).is_some())
            .expect("an amp with a cab");
        let cab = catalog.paired_cab(amp).unwrap();
        let number = |id: &str| {
            hx_catalog::resolve(catalog, id, &serde_json::json!({}))
                .unwrap()
                .number
        };
        let setting = cab.params.first().expect("a cab setting");
        let setting = (setting.id.clone(), setting.name.clone());
        (
            amp.id.clone(),
            number(&amp.id),
            cab.id.clone(),
            number(&cab.id),
            setting,
        )
    }

    /// HX Edit writes an Amp+Cab as the amp's block naming its cab node.
    /// The cab used to be dropped without a word: the `cab0` node was not a
    /// block, and `@cab` read as structure. It now rides with the amp's
    /// model, and its own settings, which nothing applies, are reported.
    #[test]
    fn an_amp_keeps_the_cab_hx_edit_names_for_it() {
        let Some(catalog) = catalog() else { return };
        let (amp, amp_number, cab, cab_number, setting) = amp_and_cab(&catalog);
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": amp, "@cab": "cab0", "@enabled": true },
                "cab0": { "@model": cab, setting.0.clone(): 0.5 }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(
            plan.steps.iter().any(|s| matches!(
                s,
                Step::Model { block: 0, model, cab: Some(Cab { model: paired, .. }), .. }
                    if *model == amp_number && *paired == cab_number
            )),
            "{:?}",
            plan.steps
        );
        assert_eq!(plan.skipped.len(), 1, "{:?}", plan.skipped);
        assert!(
            plan.skipped[0].contains("not applied") && plan.skipped[0].contains(&setting.1),
            "{:?}",
            plan.skipped
        );
    }

    /// Our own files say whose cab it is from the other side: the cab names
    /// the amp's slot.
    #[test]
    fn an_amp_keeps_the_cab_that_names_its_slot() {
        let Some(catalog) = catalog() else { return };
        let (amp, amp_number, cab, cab_number, _) = amp_and_cab(&catalog);
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": amp, "@slot": 3 },
                "cab0": { "@model": cab, "@slot": 3 }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
        assert!(plan.steps.contains(&Step::Model {
            block: 3,
            model: amp_number,
            name: catalog.model_number(amp_number).unwrap().name.clone(),
            cab: Some(Cab {
                model: cab_number,
                name: catalog.model_number(cab_number).unwrap().name.clone(),
            }),
        }));
    }

    /// A cab nothing claims, or a claim on a cab that is not there, is said
    /// rather than dropped.
    #[test]
    fn a_cab_that_cannot_be_placed_is_reported() {
        let Some(catalog) = catalog() else { return };
        let (amp, _, cab, _, _) = amp_and_cab(&catalog);
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": amp, "@cab": "cab4" },
                "cab1": { "@model": cab }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan.steps.iter().any(|s| matches!(
            s,
            Step::Model {
                block: 0,
                cab: None,
                ..
            }
        )));
        assert!(
            plan.skipped.iter().any(|s| s.contains("cab4")),
            "{:?}",
            plan.skipped
        );
        assert!(
            plan.skipped.iter().any(|s| s.contains("cab1")),
            "{:?}",
            plan.skipped
        );
    }

    #[test]
    fn reports_what_it_cannot_translate() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@model": "HD2_NotARealModel" },
                "block1": { "@model": "HD2_DistScream808Mono", "Nonsense": 1.0 }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert_eq!(plan.skipped.len(), 2);
        assert!(plan.skipped[0].contains("HD2_NotARealModel"));
        assert!(plan.skipped[1].contains("Nonsense"));
    }

    #[test]
    fn refuses_out_of_range_parameter_values_from_a_tone_file() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808Mono",
                    "Gain": 1.5
                }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan
            .skipped
            .iter()
            .any(|why| why.contains("Gain") && why.contains("outside")));
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, Step::Param { name, .. } if name == "Gain")));
    }

    #[test]
    fn reports_nonnumeric_parameter_values_from_a_tone_file() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808Mono",
                    "Gain": "loud"
                }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan
            .skipped
            .iter()
            .any(|why| why.contains("Gain") && why.contains("not a numeric")));
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, Step::Param { name, .. } if name == "Gain")));
    }

    #[test]
    fn reports_a_malformed_bypass_flag_from_a_tone_file() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808Mono",
                    "@enabled": "yes"
                }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan
            .skipped
            .iter()
            .any(|why| why.contains("@enabled") && why.contains("not a boolean")));
        assert!(!plan
            .steps
            .iter()
            .any(|step| matches!(step, Step::Enabled { .. })));
    }

    #[test]
    fn reports_a_missing_model_from_a_tone_file() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": { "@enabled": true }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan.steps.is_empty());
        assert!(plan
            .skipped
            .iter()
            .any(|why| why.contains("block0") && why.contains("@model")));
    }

    #[test]
    fn reports_a_malformed_stereo_flag_from_a_tone_file() {
        let Some(catalog) = catalog() else { return };
        let json = serde_json::json!({
            "data": { "tone": { "dsp0": {
                "block0": {
                    "@model": "HD2_DistScream808",
                    "@stereo": "wide"
                }
            }}}
        });

        let plan = plan(&json, &catalog).unwrap();
        assert!(plan.steps.is_empty());
        assert!(plan
            .skipped
            .iter()
            .any(|why| why.contains("@stereo") && why.contains("not a boolean")));
    }
}
