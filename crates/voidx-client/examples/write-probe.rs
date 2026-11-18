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
    device.enable_writes()?;
    clear_leftovers(&mut device)?;
    let current = bundle.preflight(&mut device)?;
    let presets = current
        .into_iter()
        .find(|list| list.path.as_str() == "root\\presets")
        .ok_or("backup preflight did not return the preset list")?;
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

/// Clear slots a run of this probe made and did not get to clear, such as
/// after a crash, so each run starts from the backup's slot names.
fn clear_leftovers(
    device: &mut Device<voidx_client::SerialLink>,
) -> Result<(), Box<dyn std::error::Error>> {
    for path in backup::LIBRARY_PATHS {
        let list = device.list_info(NodePath::new(path)?)?;
        for (index, name) in list.occupied() {
            if name.starts_with("__tp_") || name.starts_with("__tonepush_probe_") {
                println!("{path}: clearing leftover {name:?} in slot {}", index + 1);
                device.clear_slot(&list, index)?;
            }
        }
    }
    Ok(())
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
    let (Some(first), Some(second)) = (occupied.next(), occupied.next()) else {
        return Err("stereo IR probe needs two occupied IR slots as a source".into());
    };
    let left = bundle
        .blob(PATH, first)
        .ok_or("backup omits the first source IR")?;
    let right = bundle
        .blob(PATH, second)
        .ok_or("backup omits the second source IR")?;
    let stereo =
        voidx_client::ir::Wav::from_device_blobs(&[left, right], voidx_client::ir::SAMPLE_RATE)?
            .encode()?;
    let blobs = voidx_client::ir::Wav::parse(&stereo)?.device_blobs(list.size)?;
    if blobs.len() != 2 {
        return Err("stereo WAV did not split into two device slots".into());
    }
    println!(
        "{PATH}: stereo pair from slots {} and {} into empty slots {} and {}",
        first + 1,
        second + 1,
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
    println!("root\\presets: save probe in empty slot {}", empty + 1);

    device.write_blob(&presets, empty, SCRATCH, source, |_| {})?;
    let operation = (|| -> Result<(), Box<dyn std::error::Error>> {
        device.select_preset(empty)?;

        device.save_preset(SCRATCH)?;
        let after = device.list_info(presets.path.clone())?;
        let mut expected = presets.names.clone();
        expected[empty] = Some(SCRATCH.to_owned());
        if after.names != expected {
            return Err("saving under the scratch preset's name changed other slots".into());
        }
        voidx_proto::preset::Preset::parse(&device.read_blob(&after, empty)?)?;
        println!("save under an existing name replaced it in place");

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

    // Whatever happened, put the live selection back and clear every slot
    // the probe made, found by comparing names with those before it.
    let selected = device.select_preset(original);
    let now = device.list_info(presets.path.clone())?;
    let mut cleared = Ok(());
    for (index, name) in now.names.iter().enumerate() {
        if name.is_some() && presets.names[index].is_none() {
            let result = device.clear_slot(&now, index);
            cleared = cleared.and(result);
        }
    }
    let final_list = device.list_info(presets.path.clone())?;
    let active_after = device.read_value(active_path)?;
    if final_list.names != presets.names {
        return Err("save probe cleanup left the preset names changed".into());
    }
    selected?;
    cleared?;
    if active_after != active_before {
        return Err(format!(
            "save probe changed the active preset from {active_before} to {active_after}"
        )
        .into());
    }
    operation?;
    println!("root\\presets: save, reload, and cleanup verified");
    Ok(())
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
    let (Some(now), Some(min), Some(max)) = (before.as_f64(), description.min, description.max)
    else {
        return Err("amp gain is not a bounded number".into());
    };
    let step = (max - min) / 10.0;
    let other = serde_json::Value::from(if now - min < max - now {
        now + step
    } else {
        now - step
    });
    let change = device.write_node(knob.clone(), &description, other.clone());
    let observed = device.read_value(knob.clone());
    let restore = device.write_node(knob.clone(), &description, before.clone());
    let restored = device.read_value(knob.clone())?;
    change?;
    if !voidx_client::values_equivalent(&other, &observed?) {
        return Err("amp gain did not read back as written".into());
    }
    restore?;
    if !voidx_client::values_equivalent(&before, &restored) {
        return Err(format!("amp gain was not restored: {before} became {restored}").into());
    }
    println!("live knob write read back and was restored ({before} to {other} and back)");

    let router_path = NodePath::new(voidx_proto::router::PATH)?;
    let description = device
        .browse(router_path.clone())?
        .get(&router_path)
        .cloned()
        .ok_or("router browse omitted its own node")?;
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
    let written = written?;
    if written != swapped {
        return Err(format!(
            "the pedal kept {:?} after positions {} and {} were exchanged",
            written.to_value(),
            pair.0 + 1,
            pair.1 + 1
        )
        .into());
    }
    if restored? != node.value {
        return Err("the chain was not restored".into());
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
