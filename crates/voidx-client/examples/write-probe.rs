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
    device.enable_writes()?;
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
