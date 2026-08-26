use std::path::PathBuf;

use voidx_client::{backup, list, Device};
use voidx_proto::NodePath;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let bundle = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: setting-probe <verified-current.vxbundle> --yes")?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--yes")) {
        return Err("setting probe requires the literal --yes argument".into());
    }
    let bundle = backup::open_verified(&bundle)?;
    let found = list()?;
    if found.len() != 1 {
        return Err(format!("expected one StompStation PRO, found {}", found.len()).into());
    }
    let mut device = Device::connect(found[0].open()?)?;
    bundle.preflight(&mut device)?;
    let path = NodePath::new("root\\settings\\misc\\encoder")?;
    let tree = device.browse(path.clone())?;
    let description = tree
        .get(&path)
        .cloned()
        .ok_or("encoder setting browse omitted its own node")?;
    let before = device.read_value(path.clone())?;
    let alternate = description
        .options
        .as_ref()
        .and_then(|options| options.iter().find(|value| **value != before))
        .cloned()
        .ok_or("encoder setting has no alternate advertised value")?;

    device.enable_writes()?;
    let change = device.write_node(path.clone(), &description, alternate.clone());
    let changed = device.read_value(path.clone());
    // Attempt rollback regardless of ACK/read validation above.
    let rollback = device.write_node(path.clone(), &description, before.clone());
    let after = device.read_value(path)?;
    change?;
    if changed? != alternate {
        return Err("setting did not take its alternate value".into());
    }
    rollback?;
    if after != before {
        return Err(format!("setting rollback failed: began {before}, ended {after}").into());
    }
    println!("encoder setting changed {before} -> {alternate} -> {after}; rollback verified");
    Ok(())
}
