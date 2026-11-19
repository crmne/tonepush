//! Reversible write checks for a maintainer verifying StompStation PRO
//! firmware. Run it with the operator present on an idle pedal: nobody uses
//! the pedal or VoidX Control until it ends. Cleanup puts back what it can
//! prove the probe changed and reports anything else; it does not recover
//! from every combination of failed reads, so keep the backup at hand.

use std::path::PathBuf;

use voidx_client::backup;
use voidx_client::{list, Device, UploadStep};
use voidx_proto::NodePath;

const FIRST_NAME: &str = "__tonepush_probe_1";
const SECOND_NAME: &str = "__tonepush_probe_2";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let bundle_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: write-probe <verified-current.vxbundle> --yes")?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--yes")) {
        return Err("write probe requires the literal --yes argument".into());
    }
    let bundle = backup::open_verified(&bundle_path)?;
    let found = list()?;
    if found.len() != 1 {
        return Err(format!("expected one StompStation PRO, found {}", found.len()).into());
    }
    let mut device = Device::connect(found[0].open()?)?;
    // Everything up to enable_writes only reads: the backup must fit this
    // pedal, and the loaded preset must have no unsaved edits, since the save
    // probe loads other presets and the pedal drops such edits.
    let current = bundle.preflight(&mut device)?;
    let presets = current
        .into_iter()
        .find(|list| list.path.as_str() == "root\\presets")
        .ok_or("backup preflight did not return the preset list")?;
    refuse_unsaved_edits(&mut device, &presets)?;
    refuse_leftovers(&mut device, &bundle)?;
    // Every slot must hold what the backup holds, so a preset the probe has
    // to put back is the player's current one.
    let presets = bundle
        .verify_current_state(&mut device)?
        .into_iter()
        .find(|list| list.path.as_str() == "root\\presets")
        .ok_or("backup check did not return the preset list")?;
    println!("every slot matches the backup");
    device.enable_writes()?;
    let empty = presets
        .names
        .windows(2)
        .position(|pair| pair[0].is_none() && pair[1].is_none())
        .ok_or("write probe needs two consecutive empty preset slots")?;
    let source = bundle
        .blob("root\\presets", 0)
        .ok_or("backup has no source preset in slot 1")?;
    let active_path = NodePath::new("root\\app\\preset")?;
    let active_before = device.read_value(active_path.clone())?;

    println!(
        "using empty preset slots {} and {}; active preset is {}",
        empty + 1,
        empty + 2,
        active_before
    );
    let operation = (|| -> voidx_client::Result<()> {
        device.write_blob(&presets, empty, FIRST_NAME, source, |step| {
            if matches!(
                step,
                UploadStep::Name | UploadStep::Commit | UploadStep::Done
            ) || matches!(step, UploadStep::Data { chunk, .. } if chunk % 32 == 0)
            {
                println!("upload: {step:?}");
            }
        })?;
        println!("upload readback is byte-exact");

        device.rename_slot(&presets, empty, SECOND_NAME)?;
        let renamed_list = device.list_info(presets.path.clone())?;
        let renamed = device.read_blob(&renamed_list, empty)?;
        if renamed != source {
            return Err(voidx_client::Error::InvalidResponse {
                subject: presets.path.to_string(),
                detail: "rename changed preset content".into(),
            });
        }
        println!("rename preserved all preset bytes");

        device.swap_slots(&renamed_list, empty, empty + 1)?;
        let moved_list = device.list_info(presets.path.clone())?;
        let moved = device.read_blob(&moved_list, empty + 1)?;
        if moved != source {
            return Err(voidx_client::Error::InvalidResponse {
                subject: presets.path.to_string(),
                detail: "swap changed preset content".into(),
            });
        }
        println!("atomic swap moved name and content together");
        device.swap_slots(&moved_list, empty, empty + 1)?;
        Ok(())
    })();

    // Both slots were verified empty before writes. Clear both regardless of
    // which sub-step failed so the probe leaves no visible content behind.
    let clear_first = device.clear_slot(&presets, empty);
    let clear_second = device.clear_slot(&presets, empty + 1);
    let final_list = device.list_info(presets.path.clone())?;
    let active_after = device.read_value(active_path)?;
    if final_list.names[empty].is_some() || final_list.names[empty + 1].is_some() {
        return Err("probe cleanup left a temporary preset name behind".into());
    }
    clear_first?;
    clear_second?;
    if active_after != active_before {
        return Err(format!(
            "probe changed the active preset from {active_before} to {active_after}"
        )
        .into());
    }
    operation?;
    println!("cleanup verified; active preset was undisturbed");

    for (path, tag) in [
        ("root\\ir_list", "ir"),
        ("root\\nam_amp", "amp"),
        ("root\\nam_drive", "drive"),
    ] {
        probe_library(&mut device, &bundle, path, tag)?;
    }
    let final_active = device.read_value(NodePath::new("root\\app\\preset")?)?;
    if final_active != active_before {
        return Err("library probes changed the active preset".into());
    }
    println!("all four list types passed reversible write verification");

    probe_stereo_ir(&mut device, &bundle)?;
    probe_save(&mut device, &bundle)?;
    probe_live_edits(&mut device)?;
    let final_active = device.read_value(NodePath::new("root\\app\\preset")?)?;
    if final_active != active_before {
        return Err("stereo IR, save and live edit probes changed the active preset".into());
    }

    let setting_path = NodePath::new("root\\settings\\misc\\encoder")?;
    let setting_tree = device.browse(setting_path.clone())?;
    let description = setting_tree
        .get(&setting_path)
        .cloned()
        .ok_or("encoder setting browse omitted its own node")?;
    let old_value = device.read_value(setting_path.clone())?;
    let other = description
        .options
        .as_ref()
        .and_then(|options| options.iter().find(|value| **value != old_value))
        .cloned()
        .ok_or("encoder setting has no alternate advertised value")?;
    let change = device.write_node(setting_path.clone(), &description, other.clone());
    let observed = device.read_value(setting_path.clone());
    let restore = device.write_node(setting_path.clone(), &description, old_value.clone());
    let restored = device.read_value(setting_path)?;
    change?;
    if observed? != other {
        return Err("encoder setting did not change to its acknowledged value".into());
    }
    restore?;
    if restored != old_value {
        return Err("encoder setting was not restored".into());
    }
    println!("schema-validated global setting write and rollback verified");
    Ok(())
}

/// Refuse to run while the loaded preset differs from what is stored for it:
/// the pedal discards unsaved edits when another preset is loaded.
fn refuse_unsaved_edits(
    device: &mut Device<voidx_client::SerialLink>,
    presets: &voidx_client::BlobList,
) -> Result<(), Box<dyn std::error::Error>> {
    let active = device.read_value(NodePath::new("root\\app\\preset")?)?;
    let index = presets
        .names
        .iter()
        .position(|name| name.as_deref() == active.as_str())
        .ok_or("the active preset is not named in the preset list")?;
    let stored = voidx_proto::preset::Preset::parse(&device.read_blob(presets, index)?)?;
    let live = device.browse(NodePath::new("root\\app")?)?;
    let mut libraries = std::collections::HashMap::<String, Vec<Option<String>>>::new();
    for record in stored.records() {
        if record
            .subject()
            .rsplit('\\')
            .next()
            .is_some_and(|name| name.starts_with('_'))
        {
            continue;
        }
        let Ok(path) = NodePath::new(record.subject()) else {
            continue;
        };
        // A stored value with a live node to hold it must be compared: read
        // the node directly when the browse left its value out. A node this
        // firmware does not have holds nothing that could be lost.
        let (Some(saved), Some(node)) = (record.value().get("value"), live.get(&path)) else {
            continue;
        };
        let now = match &node.value {
            Some(now) => now.clone(),
            None => device.read_value(path.clone()).map_err(|error| {
                format!("cannot read {path} to check for unsaved changes: {error}")
            })?,
        };
        // A saved model the library no longer holds cannot be loaded; the
        // pedal shows another one, and nothing the player made is at stake.
        if let (Some(library), Some(model)) = (&node.reference, saved.as_str()) {
            if !libraries.contains_key(library) {
                let names = device.list_info(NodePath::new(library)?)?.names;
                libraries.insert(library.clone(), names);
            }
            if !libraries[library]
                .iter()
                .any(|name| name.as_deref() == Some(model))
            {
                continue;
            }
        }
        if !voidx_client::values_equivalent(saved, &now) {
            return Err(format!(
                "the loaded preset {active} has unsaved changes ({path} is {now}, saved as \
                 {saved}); save or discard them on the pedal first"
            )
            .into());
        }
    }
    // A preset saved without a chain loads the router's default chain, so
    // the live chain must still be that default; without a default to
    // compare against, an unsaved chain edit cannot be ruled out.
    let router = NodePath::new(voidx_proto::router::PATH)?;
    let saves_chain = stored
        .records()
        .iter()
        .any(|record| record.subject() == voidx_proto::router::PATH);
    if let (Some(node), false) = (live.get(&router), saves_chain) {
        let now = match &node.value {
            Some(now) => now.clone(),
            None => device.read_value(router.clone())?,
        };
        match &node.default {
            Some(default) if *default == now => {}
            Some(_) => {
                return Err(format!(
                    "the loaded preset {active} has an unsaved chain edit; save or \
                     discard it on the pedal first"
                )
                .into())
            }
            None => {
                return Err(format!(
                    "the loaded preset {active} saves no chain and the firmware names \
                     no default, so unsaved chain edits cannot be ruled out"
                )
                .into())
            }
        }
    }
    Ok(())
}

/// Refuse to run while a slot looks like a leftover of an interrupted run:
/// a probe name in a slot the backup has empty. A name alone cannot prove
/// the probe made it, so the operator checks and clears such a slot.
fn refuse_leftovers(
    device: &mut Device<voidx_client::SerialLink>,
    bundle: &backup::VerifiedBundle,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut found = Vec::new();
    for (path, library) in backup::LIBRARY_PATHS
        .into_iter()
        .zip(["presets", "irs", "amps", "drives"])
    {
        let saved = bundle
            .list(path)
            .ok_or_else(|| format!("backup omits {path}"))?;
        let list = device.list_info(NodePath::new(path)?)?;
        let leftovers: Vec<(usize, String)> = list
            .occupied()
            .filter(|(index, name)| {
                (name.starts_with("__tp_") || name.starts_with("__tonepush_probe_"))
                    && saved
                        .slots
                        .get(*index)
                        .is_some_and(|slot| slot.name.is_none())
            })
            .map(|(index, name)| (index, name.to_owned()))
            .collect();
        for (index, name) in leftovers {
            found.push(format!("{library} slot {} holds {name:?}", index + 1));
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{}; if these are left from an interrupted probe run, clear them one at \
         a time with `tonepush pro clear <library> <slot> --rollback <backup> --yes`, \
         taking a new backup before each clear, since a clear makes the previous \
         backup stale, and one more after the last; if a backup cannot be taken \
         because such a slot holds data it rejects, clear that slot in VoidX \
         Control instead",
        found.join(", ")
    )
    .into())
}

/// Write a stereo impulse response as the pair of slots `import-stereo-ir`
/// uses, read the pair back as one stereo WAV, and clear both slots.
fn probe_stereo_ir(
    device: &mut Device<voidx_client::SerialLink>,
    bundle: &backup::VerifiedBundle,
) -> Result<(), Box<dyn std::error::Error>> {
    const PATH: &str = "root\\ir_list";
    let list = device.list_info(NodePath::new(PATH)?)?;
    let empty = list
        .names
        .windows(2)
        .position(|pair| pair[0].is_none() && pair[1].is_none())
        .ok_or("stereo IR probe needs two consecutive empty IR slots")?;
    let mut occupied = list.occupied().map(|(index, _)| index);
    let first = occupied
        .next()
        .ok_or("stereo IR probe needs an occupied IR slot as a source")?;
    let second = occupied.next();
    let left = bundle
        .blob(PATH, first)
        .ok_or("backup omits the first source IR")?;
    // The two sides must differ so a swapped pair would not read back alike.
    // Without a second, different IR the right side is the left one
    // inverted, or for a silent IR a square wave that every sample format
    // holds exactly.
    let mut wav = voidx_client::ir::Wav::from_device_blobs(&[left], voidx_client::ir::SAMPLE_RATE)?;
    let other = match second {
        Some(second) => {
            let right = bundle
                .blob(PATH, second)
                .ok_or("backup omits the second source IR")?;
            voidx_client::ir::Wav::from_device_blobs(&[right], voidx_client::ir::SAMPLE_RATE)?
                .channels
                .pop()
                .filter(|right| *right != wav.channels[0])
                .map(|right| (second, right))
        }
        None => None,
    };
    let right_source = match other {
        Some((second, right)) => {
            wav.channels.push(right);
            format!("slot {}", second + 1)
        }
        _ if wav.channels[0].iter().any(|sample| *sample != 0.0) => {
            let inverted = wav.channels[0].iter().map(|sample| -sample).collect();
            wav.channels.push(inverted);
            format!("slot {} inverted", first + 1)
        }
        _ => {
            let square = (0..wav.channels[0].len())
                .map(|frame| if frame % 2 == 0 { 0.5 } else { -0.5 })
                .collect();
            wav.channels.push(square);
            "a square wave".to_owned()
        }
    };
    let stereo = wav.encode()?;
    let blobs = voidx_client::ir::Wav::parse(&stereo)?.device_blobs(list.size)?;
    if blobs.len() != 2 {
        return Err("stereo WAV did not split into two device slots".into());
    }
    println!(
        "{PATH}: stereo pair from slot {} and {right_source} into empty slots {} and {}",
        first + 1,
        empty + 1,
        empty + 2
    );

    let operation = (|| -> voidx_client::Result<()> {
        device.write_blob(&list, empty, "__tp_stereo_probe L", &blobs[0], |_| {})?;
        device.write_blob(&list, empty + 1, "__tp_stereo_probe R", &blobs[1], |_| {})?;
        let written = device.list_info(list.path.clone())?;
        let back_left = device.read_blob(&written, empty)?;
        let back_right = device.read_blob(&written, empty + 1)?;
        let back = voidx_client::ir::Wav::from_device_blobs(
            &[&back_left, &back_right],
            voidx_client::ir::SAMPLE_RATE,
        )?
        .encode()?;
        if back != stereo {
            return Err(voidx_client::Error::InvalidResponse {
                subject: PATH.to_owned(),
                detail: "stereo pair read back differently from what was written".into(),
            });
        }
        Ok(())
    })();

    let clear_first = device.clear_slot(&list, empty);
    let clear_second = device.clear_slot(&list, empty + 1);
    let final_list = device.list_info(list.path.clone())?;
    if final_list.names[empty].is_some() || final_list.names[empty + 1].is_some() {
        return Err(format!("{PATH} stereo cleanup left a temporary name").into());
    }
    clear_first?;
    clear_second?;
    operation?;
    println!("{PATH}: stereo pair upload, read back as one WAV, and cleanup verified");
    Ok(())
}

/// Save the live state under a scratch preset's own name (which replaces it
/// in place) and under a new name (which the pedal keeps in a slot of its
/// own), check that no other slot changes, reload the result, and clear what
/// was made. The scratch slot is selected first so a save cannot reach a
/// preset the player made.
fn probe_save(
    device: &mut Device<voidx_client::SerialLink>,
    bundle: &backup::VerifiedBundle,
) -> Result<(), Box<dyn std::error::Error>> {
    const SCRATCH: &str = "__tp_save_probe_1";
    const NEW: &str = "__tp_save_probe_2";
    let presets = device.list_info(NodePath::new("root\\presets")?)?;
    let empty = presets
        .names
        .iter()
        .position(Option::is_none)
        .ok_or("save probe needs one empty preset slot")?;
    let source = bundle
        .blob("root\\presets", 0)
        .ok_or("backup has no source preset in slot 1")?;
    let active_path = NodePath::new("root\\app\\preset")?;
    let active_before = device.read_value(active_path.clone())?;
    let original = presets
        .names
        .iter()
        .position(|name| name.as_deref() == active_before.as_str())
        .ok_or("the active preset is not named in the preset list")?;
    let gain = NodePath::new("root\\app\\amp\\gain")?;
    let gain_description = device
        .browse(gain.clone())?
        .get(&gain)
        .cloned()
        .ok_or("amp gain browse omitted its own node")?;
    println!("root\\presets: save probe in empty slot {}", empty + 1);

    let mut created_slot = None;
    let mut left_original = false;
    let mut save_attempted = false;
    let operation = (|| -> Result<(), Box<dyn std::error::Error>> {
        device.write_blob(&presets, empty, SCRATCH, source, |_| {})?;
        // Check again just before loading another preset: the player may
        // have turned a knob while the earlier probes ran.
        refuse_unsaved_edits(device, &presets)?;
        left_original = true;
        device.select_preset(empty)?;

        // Change a knob first, so the stored preset shows the save took the
        // live state rather than leaving the slot as uploaded.
        let loaded = device.read_value(gain.clone())?;
        let edited = nudged(&gain_description, &loaded)?;
        device.write_node(gain.clone(), &gain_description, edited.clone())?;
        save_attempted = true;
        device.save_preset(SCRATCH)?;
        let after = device.list_info(presets.path.clone())?;
        let mut expected = presets.names.clone();
        expected[empty] = Some(SCRATCH.to_owned());
        if after.names != expected {
            return Err("saving under the scratch preset's name changed other slots".into());
        }
        let saved = voidx_proto::preset::Preset::parse(&device.read_blob(&after, empty)?)?;
        let stored = saved
            .records()
            .iter()
            .find(|record| record.subject() == gain.as_str())
            .and_then(|record| record.value().get("value"))
            .ok_or("the saved preset holds no amp gain")?;
        if !voidx_client::values_equivalent(&edited, stored) {
            return Err(
                format!("saving in place stored amp gain {stored}, not the live {edited}").into(),
            );
        }
        println!("save under an existing name replaced it in place with the live edit");

        if let Some(taken) = device
            .list_info(presets.path.clone())?
            .names
            .iter()
            .position(|name| name.as_deref() == Some(NEW))
        {
            return Err(format!(
                "slot {} already holds {NEW:?}; saving would replace it",
                taken + 1
            )
            .into());
        }
        device.save_preset(NEW)?;
        let after = device.list_info(presets.path.clone())?;
        let created: Vec<usize> = after
            .names
            .iter()
            .enumerate()
            .filter(|(_, name)| name.as_deref() == Some(NEW))
            .map(|(index, _)| index)
            .collect();
        let [created] = created[..] else {
            return Err(format!("saving a new name made {} slots", created.len()).into());
        };
        created_slot = Some(created);
        if presets.names[created].is_some() {
            return Err(format!(
                "saving a new name replaced the occupied slot {}",
                created + 1
            )
            .into());
        }
        expected[created] = Some(NEW.to_owned());
        if after.names != expected {
            return Err("saving a new name changed slots other than the new one".into());
        }
        voidx_proto::preset::Preset::parse(&device.read_blob(&after, created)?)?;
        println!(
            "save under a new name made slot {} and nothing else",
            created + 1
        );

        device.select_preset(original)?;
        device.select_preset(created)?;
        let loaded = device.read_value(active_path.clone())?;
        if loaded.as_str() != Some(NEW) {
            return Err(format!("reloading the saved preset selected {loaded}").into());
        }
        println!("saved preset reloads under its new name");
        Ok(())
    })();

    // Whatever happened, put the live selection back, clear every slot the
    // probe made, and put back any preset of the player's a save replaced.
    // If the probe never loaded another preset, the player's live edits are
    // still there and selecting would drop them, so it stays put.
    let mut problems = Vec::new();
    if left_original {
        if let Err(error) = device.select_preset(original) {
            problems.push(format!("reselecting the original preset failed: {error}"));
        }
    }
    // A player's preset a save landed on is written back from the backup
    // even if its name came out the same.
    let displaced = created_slot.filter(|index| presets.names[*index].is_some());
    let mut put_back_any = false;
    let mut restored_displaced = false;
    match device.list_info(presets.path.clone()) {
        Ok(now) => {
            for index in 0..now.count {
                if now.names[index] == presets.names[index] && displaced != Some(index) {
                    continue;
                }
                // Before any save, only the scratch upload can be the
                // probe's; another change is the player's and stays.
                if !save_attempted && index != empty {
                    problems.push(format!(
                        "slot {} changed during the probe and was left as it is",
                        index + 1
                    ));
                    continue;
                }
                let result = match &presets.names[index] {
                    None => device.clear_slot(&now, index).map_err(Into::into),
                    Some(name) => {
                        put_back_any = true;
                        restored_displaced |= displaced == Some(index);
                        put_back(device, bundle, &now, index, name)
                    }
                };
                if let Err(error) = result {
                    problems.push(format!("cleaning slot {} failed: {error}", index + 1));
                }
            }
        }
        Err(error) => {
            problems.push(format!("refreshing the preset list failed: {error}"));
            for index in [Some(empty), created_slot].into_iter().flatten() {
                if presets.names[index].is_none() {
                    if let Err(error) = device.clear_slot(&presets, index) {
                        problems.push(format!("clearing slot {} failed: {error}", index + 1));
                    }
                }
            }
        }
    }
    if let (Some(index), false) = (displaced, restored_displaced) {
        put_back_any = true;
        let result = device
            .list_info(presets.path.clone())
            .map_err(Into::into)
            .and_then(|now| {
                let name = presets.names[index].clone().unwrap_or_default();
                put_back(device, bundle, &now, index, &name)
            });
        if let Err(error) = result {
            problems.push(format!("putting slot {} back failed: {error}", index + 1));
        }
    }
    // Load the original again from its restored contents.
    if put_back_any && left_original {
        if let Err(error) = device.select_preset(original) {
            problems.push(format!("reselecting the original preset failed: {error}"));
        }
    }
    match device.list_info(presets.path.clone()) {
        Ok(final_list) if final_list.names != presets.names => {
            problems.push("save probe cleanup left the preset names changed".into());
        }
        Ok(_) => {}
        Err(error) => problems.push(format!("rereading the preset list failed: {error}")),
    }
    match device.read_value(active_path) {
        Ok(active_after) if active_after != active_before => problems.push(format!(
            "save probe changed the active preset from {active_before} to {active_after}"
        )),
        Ok(_) => {}
        Err(error) => problems.push(format!("rereading the active preset failed: {error}")),
    }
    if let Err(error) = operation {
        problems.insert(0, error.to_string());
    }
    if !problems.is_empty() {
        return Err(problems.join("; ").into());
    }
    println!("root\\presets: save, reload, and cleanup verified");
    Ok(())
}

/// Write a player's preset back from the backup after a save replaced it.
fn put_back(
    device: &mut Device<voidx_client::SerialLink>,
    bundle: &backup::VerifiedBundle,
    list: &voidx_client::BlobList,
    index: usize,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let saved = bundle
        .list(list.path.as_str())
        .and_then(|saved| saved.slots.get(index))
        .ok_or("the backup omits this slot")?;
    if saved.name.as_deref() != Some(name) {
        return Err(format!("the backup does not hold {name:?} in this slot").into());
    }
    let blob = bundle
        .blob(list.path.as_str(), index)
        .ok_or("the backup omits this slot's content")?;
    device.write_blob(list, index, name, blob, |_| {})?;
    println!("put {name:?} back in slot {} from the backup", index + 1);
    Ok(())
}

/// A value a tenth of the range away from `now`, toward the middle.
fn nudged(
    description: &voidx_proto::NodeDescription,
    now: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let (Some(now), Some(min), Some(max)) = (now.as_f64(), description.min, description.max) else {
        return Err("amp gain is not a bounded number".into());
    };
    let step = (max - min) / 10.0;
    Ok(serde_json::Value::from(if now - min < max - now {
        now + step
    } else {
        now - step
    }))
}

/// Edit the loaded preset the way the editor does, one knob and then the
/// chain, read each result back, and put both as they were. Nothing here is
/// saved, so the stored preset is not touched.
fn probe_live_edits(
    device: &mut Device<voidx_client::SerialLink>,
) -> Result<(), Box<dyn std::error::Error>> {
    let knob = NodePath::new("root\\app\\amp\\gain")?;
    let description = device
        .browse(knob.clone())?
        .get(&knob)
        .cloned()
        .ok_or("amp gain browse omitted its own node")?;
    let before = device.read_value(knob.clone())?;
    let other = nudged(&description, &before)?;
    let change = device.write_node(knob.clone(), &description, other.clone());
    let observed = device.read_value(knob.clone());
    let restore = device.write_node(knob.clone(), &description, before.clone());
    let restored = device.read_value(knob.clone());
    // Report a failed restore together with a failed change, never instead.
    let mut problems = Vec::new();
    match (change, observed) {
        (Err(error), _) => problems.push(format!("writing amp gain failed: {error}")),
        (Ok(_), Err(error)) => problems.push(format!("reading amp gain back failed: {error}")),
        (Ok(_), Ok(observed)) if !voidx_client::values_equivalent(&other, &observed) => {
            problems.push(format!("amp gain read back as {observed}, not {other}"));
        }
        _ => {}
    }
    if let Err(error) = restore {
        problems.push(format!("restoring amp gain failed: {error}"));
    }
    match restored {
        Ok(restored) if !voidx_client::values_equivalent(&before, &restored) => {
            problems.push(format!(
                "amp gain was not restored: {before} became {restored}"
            ));
        }
        Ok(_) => {}
        Err(error) => problems.push(format!("rereading amp gain failed: {error}")),
    }
    if !problems.is_empty() {
        return Err(problems.join("; ").into());
    }
    println!("live knob write read back and was restored ({before} to {other} and back)");

    // Firmware 1.5 has no editable chain; skip that step there so the rest
    // of the probe still runs. Later firmware must describe its router.
    let router_path = NodePath::new(voidx_proto::router::PATH)?;
    let app = device.browse(NodePath::new("root\\app")?)?;
    let Some(description) = app.get(&router_path).cloned() else {
        let version = &device.identity().version;
        if version.starts_with("1.5.") {
            println!("no {router_path} on firmware {version}; chain edit skipped");
            return Ok(());
        }
        return Err(format!("firmware {version} did not describe {router_path}").into());
    };
    let node = voidx_proto::router::RouterNode::from_description(&description)?;
    let movable: Vec<usize> = (0..node.value.positions())
        .filter(|position| !node.fixed.position(*position))
        .collect();
    let pair = movable
        .iter()
        .flat_map(|a| movable.iter().map(move |b| (*a, *b)))
        .find(|(a, b)| {
            a < b
                && node.value.block(*a).is_some()
                && node.value.block(*b).is_some()
                && node.value.block(*a) != node.value.block(*b)
        })
        .ok_or("the chain has no two different blocks to exchange")?;
    let swapped = node.value.swap(&node.fixed, pair.0, pair.1)?;
    let written = device.write_router(&swapped);
    let restored = device.write_router(&node.value);
    let mut problems = Vec::new();
    match written {
        Ok(written) if written != swapped => problems.push(format!(
            "the pedal kept {:?} after positions {} and {} were exchanged",
            written.to_value(),
            pair.0 + 1,
            pair.1 + 1
        )),
        Ok(_) => {}
        Err(error) => problems.push(format!("exchanging chain positions failed: {error}")),
    }
    match restored {
        Ok(restored) if restored != node.value => {
            problems.push("the chain was not restored".into());
        }
        Ok(_) => {}
        Err(error) => problems.push(format!("restoring the chain failed: {error}")),
    }
    if !problems.is_empty() {
        return Err(problems.join("; ").into());
    }
    println!(
        "live chain edit exchanged positions {} and {}, read back, and was restored",
        pair.0 + 1,
        pair.1 + 1
    );
    Ok(())
}

fn probe_library(
    device: &mut Device<voidx_client::SerialLink>,
    bundle: &backup::VerifiedBundle,
    path: &str,
    tag: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let list = device.list_info(NodePath::new(path)?)?;
    let empty = list
        .names
        .windows(2)
        .position(|pair| pair[0].is_none() && pair[1].is_none())
        .ok_or_else(|| format!("{path} needs two consecutive empty slots"))?;
    let source_index = list
        .occupied()
        .next()
        .map(|(index, _)| index)
        .ok_or_else(|| format!("{path} has no source blob"))?;
    let source = bundle
        .blob(path, source_index)
        .ok_or_else(|| format!("backup omits {path} slot {source_index}"))?;
    let first_name = format!("__tp_{tag}_probe_1");
    let second_name = format!("__tp_{tag}_probe_2");
    println!(
        "{path}: copying slot {} into empty slots {} and {}",
        source_index + 1,
        empty + 1,
        empty + 2
    );

    let operation = (|| -> voidx_client::Result<()> {
        device.write_blob(&list, empty, &first_name, source, |step| {
            if matches!(
                step,
                UploadStep::Name | UploadStep::Commit | UploadStep::Done
            ) || matches!(step, UploadStep::Data { chunk, .. } if chunk % 256 == 0)
            {
                println!("  upload: {step:?}");
            }
        })?;
        device.rename_slot(&list, empty, &second_name)?;
        let renamed_list = device.list_info(list.path.clone())?;
        if device.read_blob(&renamed_list, empty)? != source {
            return Err(voidx_client::Error::InvalidResponse {
                subject: path.to_owned(),
                detail: "rename changed content".into(),
            });
        }
        device.swap_slots(&renamed_list, empty, empty + 1)?;
        let moved_list = device.list_info(list.path.clone())?;
        if device.read_blob(&moved_list, empty + 1)? != source {
            return Err(voidx_client::Error::InvalidResponse {
                subject: path.to_owned(),
                detail: "swap changed content".into(),
            });
        }
        device.swap_slots(&moved_list, empty, empty + 1)?;
        Ok(())
    })();

    let clear_first = device.clear_slot(&list, empty);
    let clear_second = device.clear_slot(&list, empty + 1);
    let final_list = device.list_info(list.path.clone())?;
    if final_list.names[empty].is_some() || final_list.names[empty + 1].is_some() {
        return Err(format!("{path} cleanup left a temporary name").into());
    }
    clear_first?;
    clear_second?;
    operation?;
    println!("{path}: upload, rename, swap, and cleanup verified");
    Ok(())
}
