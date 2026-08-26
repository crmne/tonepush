use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use voidx_client::backup::{self, Step};
use voidx_client::{list, Device};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: backup <target.vxbundle>")?;
    let found = list()?;
    if found.len() != 1 {
        return Err(format!("expected one StompStation PRO, found {}", found.len()).into());
    }
    let mut device = Device::connect(found[0].open()?)?;
    let captured = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let started = std::time::Instant::now();
    let reuse = backup::open_verified(&target).ok();
    let manifest = backup::capture_reusing(
        &mut device,
        &target,
        captured,
        reuse.as_ref(),
        |step| match step {
            Step::Schema => println!("capturing node schema"),
            Step::List { path, occupied } => println!("{path}: {occupied} occupied slots"),
            Step::Blob {
                path,
                index,
                name,
                chunk,
                chunks,
            } if chunk == 0 || chunk == chunks || chunk % 64 == 0 => println!(
                "{path} slot {} {name:?}: {chunk}/{chunks} chunks ({:.1?})",
                index + 1,
                started.elapsed()
            ),
            Step::Verifying { path, index } => {
                println!("{path} slot {}: verifying boundary chunks", index + 1)
            }
            Step::Reused {
                path, index, name, ..
            } => println!("{path} slot {} {name:?}: reused verified bytes", index + 1),
            Step::Done => println!("published complete bundle at {}", target.display()),
            _ => {}
        },
    )?;
    let verified = backup::open_verified(&target)?;
    if verified.manifest() != &manifest {
        return Err("published manifest did not round-trip".into());
    }
    let files = manifest
        .lists
        .iter()
        .flat_map(|list| &list.slots)
        .filter(|slot| slot.blob.is_some())
        .count();
    println!(
        "verified {files} occupied slots in {:.1?}",
        started.elapsed()
    );
    Ok(())
}
