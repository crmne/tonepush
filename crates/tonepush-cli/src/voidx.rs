use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use voidx_client::backup::VerifiedBundle;
use voidx_client::backup::{self, Step};
use voidx_client::{Device, SerialLink, UploadStep};
use voidx_proto::{NodeDescription, NodeKind, NodePath, Preset};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Library {
    Presets,
    Irs,
    Amps,
    Drives,
}

impl Library {
    fn path(self) -> &'static str {
        match self {
            Self::Presets => "root\\presets",
            Self::Irs => "root\\ir_list",
            Self::Amps => "root\\nam_amp",
            Self::Drives => "root\\nam_drive",
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum Command {
    /// Show the attached PRO's identity and write-compatibility status.
    Info,
    /// List names and slots in one device library.
    List { library: Library },
    /// Print a self-described portion of the live parameter tree as JSON.
    Schema {
        #[arg(default_value = "root\\app")]
        path: String,
    },
    /// Export one occupied slot's exact padded device bytes.
    ExportRaw {
        library: Library,
        /// Slot number as shown on the pedal, starting at 1.
        #[arg(value_parser = one_based_slot)]
        slot: usize,
        file: PathBuf,
    },
    /// Export a preset, ordinary .nam JSON, or mono IR WAV.
    Export {
        library: Library,
        #[arg(value_parser = one_based_slot)]
        slot: usize,
        file: PathBuf,
    },
    /// Export two IR slots as one stereo 48 kHz WAV.
    ExportStereoIr {
        #[arg(value_parser = one_based_slot)]
        left: usize,
        #[arg(value_parser = one_based_slot)]
        right: usize,
        file: PathBuf,
    },
    /// Import a preset, .nam model, or mono 48 kHz IR into a slot.
    Import {
        library: Library,
        #[arg(value_parser = one_based_slot)]
        slot: usize,
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rollback: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Split a stereo 48 kHz WAV into two IR slots.
    ImportStereoIr {
        #[arg(value_parser = one_based_slot)]
        left: usize,
        #[arg(value_parser = one_based_slot)]
        right: usize,
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rollback: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Select a preset by its 1-based slot number.
    Select {
        #[arg(value_parser = one_based_slot)]
        slot: usize,
    },
    /// Set one node using its browsed schema. Global settings require a rollback.
    Set {
        path: String,
        value: String,
        #[arg(long)]
        rollback: Option<PathBuf>,
        #[arg(long)]
        yes: bool,
    },
    /// Save the current live state as a named preset.
    Save {
        name: String,
        #[arg(long)]
        rollback: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Rename a slot; model/IR renames are refused while presets reference it.
    Rename {
        library: Library,
        #[arg(value_parser = one_based_slot)]
        slot: usize,
        name: String,
        #[arg(long)]
        rollback: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Move a slot and shift the intervening entries using atomic swaps.
    Move {
        library: Library,
        #[arg(value_parser = one_based_slot)]
        from: usize,
        #[arg(value_parser = one_based_slot)]
        to: usize,
        #[arg(long)]
        rollback: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Clear one slot after verifying a current rollback bundle.
    Clear {
        library: Library,
        #[arg(value_parser = one_based_slot)]
        slot: usize,
        #[arg(long)]
        rollback: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Capture presets, stereo IRs, NAM amps/drives, and node schema.
    Backup { directory: PathBuf },
    /// Verify every file and hash in a backup without opening hardware.
    VerifyBackup { directory: PathBuf },
    /// Read-only compatibility check for a bundle against the connected PRO.
    PreflightRestore { directory: PathBuf },
    /// Restore an exact bundle, with a separately verified current-state bundle
    /// retained for rollback if the transport fails.
    Restore {
        directory: PathBuf,
        #[arg(long)]
        rollback: PathBuf,
        /// Confirm that the operation may overwrite and clear device slots.
        #[arg(long)]
        yes: bool,
    },
}

pub(crate) fn run(command: Command) -> Result<()> {
    if let Command::VerifyBackup { directory } = &command {
        let bundle = backup::open_verified(directory)
            .with_context(|| format!("verifying {}", directory.display()))?;
        let occupied = bundle
            .manifest()
            .lists
            .iter()
            .flat_map(|list| &list.slots)
            .filter(|slot| slot.blob.is_some())
            .count();
        println!(
            "verified {}: {occupied} occupied slots, firmware {}",
            directory.display(),
            bundle.manifest().identity.version
        );
        return Ok(());
    }
    if needs_confirmation(&command) {
        bail!("this operation changes persistent device state; inspect the rollback bundle, then repeat with --yes");
    }

    let mut device = open()?;
    match command {
        Command::Info => {
            let identity = device.identity();
            println!("{}", identity.name);
            println!("firmware: {}", identity.version);
            println!("architecture: {}", identity.architecture);
            println!("license: {}", identity.license);
            println!("transport: {}", device.transport());
            println!("write safety: {:?}", device.write_safety());
        }
        Command::List { library } => {
            let list = device.list_info(NodePath::new(library.path())?)?;
            for (index, name) in list.names.iter().enumerate() {
                println!("{:>2}  {}", index + 1, name.as_deref().unwrap_or("(empty)"));
            }
        }
        Command::Schema { path } => {
            let tree = device.browse(NodePath::new(path)?)?;
            let nodes = tree
                .nodes()
                .iter()
                .map(|(path, description)| {
                    serde_json::json!({ "path": path.as_str(), "description": description })
                })
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&nodes)?);
        }
        Command::ExportRaw {
            library,
            slot,
            file,
        } => {
            let index = slot - 1;
            let list = device.list_info(NodePath::new(library.path())?)?;
            let blob = device.read_blob(&list, index)?;
            atomic_write(&file, &blob).with_context(|| format!("writing {}", file.display()))?;
            println!("wrote {} bytes to {}", blob.len(), file.display());
        }
        Command::Export {
            library,
            slot,
            file,
        } => {
            let index = slot - 1;
            let list = device.list_info(NodePath::new(library.path())?)?;
            let blob = device.read_blob(&list, index)?;
            let bytes = export_blob(library, &blob, list.size)?;
            atomic_write(&file, &bytes).with_context(|| format!("writing {}", file.display()))?;
            println!(
                "exported {} slot {slot} to {}",
                library_name(library),
                file.display()
            );
        }
        Command::ExportStereoIr { left, right, file } => {
            if left == right {
                bail!("left and right must be different IR slots");
            }
            let list = device.list_info(NodePath::new(Library::Irs.path())?)?;
            let left_blob = device.read_blob(&list, left - 1)?;
            let right_blob = device.read_blob(&list, right - 1)?;
            let wav = voidx_client::ir::Wav::from_device_blobs(
                &[left_blob.as_slice(), right_blob.as_slice()],
                voidx_client::ir::SAMPLE_RATE,
            )?;
            atomic_write(&file, &wav.encode()?)
                .with_context(|| format!("writing {}", file.display()))?;
            println!("exported IR slots {left}/{right} to {}", file.display());
        }
        Command::Import {
            library,
            slot,
            file,
            name,
            rollback,
            yes: _,
        } => {
            let source =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let list = device.list_info(NodePath::new(library.path())?)?;
            let blob = import_blob(library, &source, list.size)?;
            let name = import_name(name.as_deref(), &file)?;
            let guard = authorize(&mut device, &rollback)?;
            refuse_orphaned_slot(&guard, &list, slot - 1, Some(&name))?;
            device.write_blob(&list, slot - 1, &name, &blob, print_upload)?;
            println!(
                "imported {} into {} slot {slot}; rollback remains at {}",
                file.display(),
                library_name(library),
                rollback.display()
            );
        }
        Command::ImportStereoIr {
            left,
            right,
            file,
            name,
            rollback,
            yes: _,
        } => {
            if left == right {
                bail!("left and right must be different IR slots");
            }
            let source =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let wav = voidx_client::ir::Wav::parse(&source)?;
            let list = device.list_info(NodePath::new(Library::Irs.path())?)?;
            let blobs = wav.device_blobs(list.size)?;
            if blobs.len() != 2 {
                bail!("stereo IR import requires a two-channel WAV");
            }
            let base = import_name(name.as_deref(), &file)?;
            let left_name = format!("{base} L");
            let right_name = format!("{base} R");
            let guard = authorize(&mut device, &rollback)?;
            refuse_orphaned_slot(&guard, &list, left - 1, Some(&left_name))?;
            refuse_orphaned_slot(&guard, &list, right - 1, Some(&right_name))?;
            device.write_blob(&list, left - 1, &left_name, &blobs[0], print_upload)?;
            device.write_blob(&list, right - 1, &right_name, &blobs[1], print_upload)?;
            println!(
                "imported stereo IR into slots {left}/{right}; rollback remains at {}",
                rollback.display()
            );
        }
        Command::Select { slot } => {
            device.enable_writes()?;
            device.select_preset(slot - 1)?;
            println!("selected preset slot {slot}");
        }
        Command::Set {
            path,
            value,
            rollback,
            yes: _,
        } => {
            let path = NodePath::new(path)?;
            let tree = device.browse(path.clone())?;
            let description = tree
                .get(&path)
                .cloned()
                .with_context(|| format!("{} was absent from its browse response", path))?;
            let value = parse_node_value(&description, &value)?;
            let persistent = !path.as_str().starts_with("root\\app\\");
            let guard = if persistent {
                let rollback = rollback.as_deref().context(
                    "non-app nodes require --rollback PATH and --yes because they may persist",
                )?;
                Some(authorize(&mut device, rollback)?)
            } else {
                device.enable_writes()?;
                None
            };
            device.write_node(path.clone(), &description, value.clone())?;
            let echoed = device.read_value(path.clone())?;
            if !voidx_client::values_equivalent(&value, &echoed) {
                bail!("{} read back as {echoed}, expected {value}", path);
            }
            drop(guard);
            println!("set {} to {value}", path);
        }
        Command::Save {
            name,
            rollback,
            yes: _,
        } => {
            let _guard = authorize(&mut device, &rollback)?;
            device.save_preset(&name)?;
            println!(
                "saved preset {name:?}; rollback remains at {}",
                rollback.display()
            );
        }
        Command::Rename {
            library,
            slot,
            name,
            rollback,
            yes: _,
        } => {
            let guard = authorize(&mut device, &rollback)?;
            let list = device.list_info(NodePath::new(library.path())?)?;
            refuse_orphaned_slot(&guard, &list, slot - 1, Some(&name))?;
            device.rename_slot(&list, slot - 1, &name)?;
            println!("renamed {} slot {slot} to {name:?}", library_name(library));
        }
        Command::Move {
            library,
            from,
            to,
            rollback,
            yes: _,
        } => {
            let _guard = authorize(&mut device, &rollback)?;
            let list = device.list_info(NodePath::new(library.path())?)?;
            device.move_slot(&list, from - 1, to - 1)?;
            println!("moved {} slot {from} to {to}", library_name(library));
        }
        Command::Clear {
            library,
            slot,
            rollback,
            yes: _,
        } => {
            let guard = authorize(&mut device, &rollback)?;
            let list = device.list_info(NodePath::new(library.path())?)?;
            refuse_orphaned_slot(&guard, &list, slot - 1, None)?;
            device.clear_slot(&list, slot - 1)?;
            println!("cleared {} slot {slot}", library_name(library));
        }
        Command::Backup { directory } => {
            let captured = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let started = std::time::Instant::now();
            let reuse = backup::open_verified(&directory).ok();
            let manifest = backup::capture_reusing(
                &mut device,
                &directory,
                captured,
                reuse.as_ref(),
                |step| match step {
                    Step::Schema => println!("schema"),
                    Step::List { path, occupied } => println!("{path}: {occupied} occupied"),
                    Step::Blob {
                        path,
                        index,
                        name,
                        chunk,
                        chunks,
                    } if chunk == 0 || chunk == chunks || chunk % 64 == 0 => {
                        println!("{path} {:>2} {name}: {chunk}/{chunks}", index + 1)
                    }
                    Step::Verifying { path, index } => {
                        println!("{path} {:>2}: verify", index + 1)
                    }
                    Step::Reused {
                        path, index, name, ..
                    } => println!("{path} {:>2} {name}: reused", index + 1),
                    Step::Done => println!("published {}", directory.display()),
                    _ => {}
                },
            )?;
            let occupied = manifest
                .lists
                .iter()
                .flat_map(|list| &list.slots)
                .filter(|slot| slot.blob.is_some())
                .count();
            println!(
                "captured {occupied} occupied slots in {:.1?}",
                started.elapsed()
            );
        }
        Command::PreflightRestore { directory } => {
            let bundle = backup::open_verified(&directory)?;
            let lists = bundle.preflight(&mut device)?;
            println!(
                "{} is compatible with {} firmware {}; {} lists preflighted",
                directory.display(),
                device.identity().name,
                device.identity().version,
                lists.len()
            );
        }
        Command::Restore {
            directory,
            rollback,
            yes: _,
        } => {
            let source = backup::open_verified(&directory)?;
            let rollback_bundle = backup::open_verified(&rollback)?;
            device.enable_writes()?;
            let report =
                backup::restore(&source, &rollback_bundle, &mut device, |step| match step {
                    backup::RestoreStep::Preflight => println!("preflight"),
                    backup::RestoreStep::Writing {
                        path,
                        index,
                        name,
                        upload,
                    } => println!("{path} {} {name}: {upload:?}", index + 1),
                    backup::RestoreStep::Clearing { path, index } => {
                        println!("{path} {}: clear", index + 1)
                    }
                    backup::RestoreStep::Setting { path } => println!("{path}: setting"),
                    backup::RestoreStep::Done => println!("restore verified"),
                })?;
            println!(
                "{} slots written, {} cleared, {} already identical; {} settings written, {} already identical; rollback remains at {}",
                report.written,
                report.cleared,
                report.unchanged,
                report.settings_written,
                report.settings_unchanged,
                rollback.display()
            );
        }
        Command::VerifyBackup { .. } => unreachable!("handled without opening hardware"),
    }
    Ok(())
}

fn needs_confirmation(command: &Command) -> bool {
    match command {
        Command::Import { yes, .. }
        | Command::ImportStereoIr { yes, .. }
        | Command::Save { yes, .. }
        | Command::Rename { yes, .. }
        | Command::Move { yes, .. }
        | Command::Clear { yes, .. }
        | Command::Restore { yes, .. } => !yes,
        Command::Set {
            path,
            rollback,
            yes,
            ..
        } => !path.starts_with("root\\app\\") && (!yes || rollback.is_none()),
        _ => false,
    }
}

fn authorize(device: &mut Device<SerialLink>, rollback: &Path) -> Result<VerifiedBundle> {
    let bundle = backup::open_verified(rollback)
        .with_context(|| format!("verifying rollback bundle {}", rollback.display()))?;
    bundle
        .verify_current_state(device)
        .context("rollback bundle does not describe the pedal's current state")?;
    device.enable_writes()?;
    Ok(bundle)
}

fn export_blob(library: Library, blob: &[u8], capacity: usize) -> Result<Vec<u8>> {
    match library {
        Library::Presets => Ok(Preset::parse(blob)?.content()),
        Library::Irs => Ok(voidx_client::ir::Wav::from_device_blobs(
            &[blob],
            voidx_client::ir::SAMPLE_RATE,
        )?
        .encode()?),
        Library::Amps | Library::Drives => Ok(voidx_client::nam::decode(blob, capacity)?),
    }
}

fn import_blob(library: Library, source: &[u8], capacity: usize) -> Result<Vec<u8>> {
    match library {
        Library::Presets => Ok(Preset::parse(source)?.encode_padded(capacity)?),
        Library::Irs => {
            let blobs = voidx_client::ir::Wav::parse(source)?.device_blobs(capacity)?;
            if blobs.len() != 1 {
                bail!("single-slot IR import requires a mono WAV; use import-stereo-ir");
            }
            Ok(blobs.into_iter().next().expect("one checked channel"))
        }
        Library::Amps | Library::Drives => Ok(voidx_client::nam::encode(source, capacity)?),
    }
}

fn import_name(provided: Option<&str>, file: &Path) -> Result<String> {
    if let Some(name) = provided {
        return Ok(name.to_owned());
    }
    file.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("{} has no usable file name; pass --name", file.display()))
}

fn parse_node_value(description: &NodeDescription, text: &str) -> Result<serde_json::Value> {
    let parsed = serde_json::from_str(text).ok();
    let value = match description.kind.as_ref() {
        Some(NodeKind::Float) => parsed.unwrap_or_else(|| serde_json::Value::String(text.into())),
        Some(NodeKind::Enum | NodeKind::Array) => {
            let options = description.options.as_ref().or(description.items.as_ref());
            let raw_string = serde_json::Value::String(text.to_owned());
            if options.is_some_and(|items| items.contains(&raw_string)) {
                raw_string
            } else {
                parsed.unwrap_or(raw_string)
            }
        }
        Some(NodeKind::PropertyList) => parsed
            .filter(serde_json::Value::is_string)
            .unwrap_or_else(|| serde_json::Value::String(text.to_owned())),
        _ => parsed.unwrap_or_else(|| serde_json::Value::String(text.to_owned())),
    };
    description.validate_value(&value)?;
    Ok(value)
}

fn refuse_orphaned_slot(
    guard: &VerifiedBundle,
    list: &voidx_client::BlobList,
    index: usize,
    replacement_name: Option<&str>,
) -> Result<()> {
    if list.path.as_str() == Library::Presets.path() {
        return Ok(());
    }
    let old = list
        .names
        .get(index)
        .with_context(|| format!("slot {} is outside {}", index + 1, list.path))?;
    let Some(old) = old else {
        return Ok(());
    };
    if replacement_name == Some(old.as_str()) {
        return Ok(());
    }
    let references = guard.references_to(list.path.as_str(), old)?;
    if !references.is_empty() {
        let examples = references
            .iter()
            .take(4)
            .map(|reference| format!("{} ({})", reference.preset_name, reference.node_path))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "{} slot {} is named {old:?} and is referenced by {} preset node(s): {examples}; rename/update those presets first",
            list.path,
            index + 1,
            references.len()
        );
    }
    Ok(())
}

fn print_upload(step: UploadStep) {
    match step {
        UploadStep::Name => println!("name"),
        UploadStep::Data { chunk, chunks } if chunk == 1 || chunk == chunks || chunk % 64 == 0 => {
            println!("upload {chunk}/{chunks}")
        }
        UploadStep::Commit => println!("commit"),
        UploadStep::Verify { chunk, chunks }
            if chunk == 1 || chunk == chunks || chunk % 64 == 0 =>
        {
            println!("verify {chunk}/{chunks}")
        }
        UploadStep::Done => println!("verified"),
        _ => {}
    }
}

fn library_name(library: Library) -> &'static str {
    match library {
        Library::Presets => "preset",
        Library::Irs => "IR",
        Library::Amps => "NAM amp",
        Library::Drives => "NAM drive",
    }
}

fn open() -> Result<Device<SerialLink>> {
    let found = voidx_client::list().context("enumerating serial ports")?;
    let Some(device) = found.first() else {
        bail!("no StompStation PRO found - check its USB cable");
    };
    if found.len() > 1 {
        bail!(
            "found {} StompStation PRO ports; explicit port selection is not implemented yet",
            found.len()
        );
    }
    Device::connect(device.open()?).context("opening the StompStation PRO")
}

fn one_based_slot(text: &str) -> std::result::Result<usize, String> {
    let slot = text
        .parse::<usize>()
        .map_err(|_| format!("{text:?} is not a slot number"))?;
    if slot == 0 {
        Err("slots are numbered from 1".into())
    } else {
        Ok(slot)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
    file.write_all(bytes)?;
    file.commit()
}
