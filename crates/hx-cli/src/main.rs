//! `tonepush`, the command-line editor for Line 6 HX-family devices.

mod hlx;
mod wav;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;

fn write_file(path: &std::path::Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
    file.write_all(bytes.as_ref())?;
    file.commit()
}

fn one_based_i64(text: &str) -> std::result::Result<i64, String> {
    let value = text
        .parse::<i64>()
        .map_err(|_| format!("{text:?} is not a whole number"))?;
    (value >= 1)
        .then_some(value)
        .ok_or_else(|| "the number must be 1 or greater".to_owned())
}

fn non_negative_i64(text: &str) -> std::result::Result<i64, String> {
    let value = text
        .parse::<i64>()
        .map_err(|_| format!("{text:?} is not a whole number"))?;
    (value >= 0)
        .then_some(value)
        .ok_or_else(|| "the number cannot be negative".to_owned())
}

fn one_based_usize(text: &str) -> std::result::Result<usize, String> {
    let value = text
        .parse::<usize>()
        .map_err(|_| format!("{text:?} is not a positive whole number"))?;
    (value >= 1)
        .then_some(value)
        .ok_or_else(|| "the number must be 1 or greater".to_owned())
}

fn one_based_u8(text: &str) -> std::result::Result<u8, String> {
    let value = text
        .parse::<u8>()
        .map_err(|_| format!("{text:?} is not a footswitch number"))?;
    (value >= 1)
        .then_some(value)
        .ok_or_else(|| "footswitches are numbered from 1".to_owned())
}

fn midi_cc(text: &str) -> std::result::Result<i64, String> {
    let value = text
        .parse::<i64>()
        .map_err(|_| format!("{text:?} is not a MIDI CC"))?;
    (0..=127)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "a MIDI CC is 0 to 127".to_owned())
}

fn controller_source(text: &str) -> std::result::Result<i64, String> {
    let value = text
        .parse::<i64>()
        .map_err(|_| format!("{text:?} is not a controller source"))?;
    (0..=9)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "a controller source is 0 to 9".to_owned())
}

fn tempo_bpm(text: &str) -> std::result::Result<f32, String> {
    let value = text
        .parse::<f32>()
        .map_err(|_| format!("{text:?} is not a tempo"))?;
    (value.is_finite() && (40.0..=240.0).contains(&value))
        .then_some(value)
        .ok_or_else(|| "tempo must be between 40 and 240 BPM".to_owned())
}

fn normalised(text: &str) -> std::result::Result<f32, String> {
    let value = text
        .parse::<f32>()
        .map_err(|_| format!("{text:?} is not a number"))?;
    (value.is_finite() && (0.0..=1.0).contains(&value))
        .then_some(value)
        .ok_or_else(|| "the value must be between 0 and 1".to_owned())
}

fn zero_based(position: usize, what: &str) -> Result<usize> {
    position
        .checked_sub(1)
        .with_context(|| format!("{what} are numbered from 1"))
}

#[derive(Parser)]
#[command(
    name = "tonepush",
    version,
    about = "Talk to Line 6 HX hardware over USB"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Clone)]
enum Cmd {
    /// List attached HX devices.
    List,
    /// Show device identity and firmware.
    Info,
    /// Load a preset, by front-panel label (`03B`) or zero-based index (`7`).
    Select {
        index: String,
        #[arg(long, default_value_t = 0, value_parser = non_negative_i64)]
        setlist: i64,
    },
    /// Dump the loaded preset.
    Preset {
        /// Print the whole decoded structure rather than a summary.
        #[arg(long)]
        raw: bool,
    },
    /// List every preset by name.
    Presets {
        #[arg(long, default_value_t = 0, value_parser = non_negative_i64)]
        setlist: i64,
    },
    /// Show the signal chain of the loaded preset, with parameter values.
    Chain,
    /// Set a parameter. Block is its position in `tonepush chain`; the parameter may
    /// be named or given by index.
    Set {
        #[arg(value_parser = one_based_i64)]
        block: i64,
        param: String,
        value: String,
    },
    /// Switch a block on or off. Off is what the front panel calls bypassed.
    Enable {
        #[arg(value_parser = one_based_i64)]
        block: i64,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Change a block's model, by name ("Room") or catalog number.
    Model {
        #[arg(value_parser = one_based_i64)]
        block: i64,
        model: String,
    },
    /// Remove a block, by position as shown in `tonepush chain`.
    Clear {
        #[arg(value_parser = one_based_i64)]
        block: i64,
    },
    /// Send an impulse response to a slot (1-based), from a mono WAV.
    ///
    IrLoad {
        #[arg(value_parser = one_based_i64)]
        slot: i64,
        file: std::path::PathBuf,
    },
    /// Put a block's bypass under MIDI, or take it off, by position as in
    /// `tonepush chain`.
    Assign {
        #[arg(value_parser = one_based_i64)]
        block: i64,
        /// Which CC drives it. 4 is what the pedal picks for itself.
        #[arg(long, default_value_t = 4, value_parser = midi_cc)]
        cc: i64,
        #[arg(long)]
        off: bool,
    },
    /// Set the tempo of the loaded preset, in BPM.
    Tempo {
        #[arg(value_parser = tempo_bpm)]
        bpm: f32,
    },
    /// Rename a snapshot (1-based).
    SnapshotName {
        #[arg(value_parser = one_based_usize)]
        number: usize,
        name: String,
    },
    /// Route an input or output, by slot position and destination name.
    ///
    /// `tonepush route 0 "Return L/R"` - see `tonepush chain` for slots,
    /// and pass a partial name; it is matched against the device's own menu.
    Route {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        to: String,
    },
    /// Print the signal path as the device is wired: one row per lane.
    Topology,
    /// Dump one slot's raw body, for protocol work.
    Slot { position: usize },
    /// Copy a block over another slot, by position as shown in `chain`.
    ///
    /// Writes the whole preset document, so the block arrives complete -
    /// model, values, paired cab and all.
    CopyBlock {
        #[arg(value_parser = one_based_usize)]
        from: usize,
        #[arg(value_parser = one_based_usize)]
        to: usize,
    },
    /// Copy a snapshot's settings over another, keeping the target's name.
    CopySnapshot {
        #[arg(value_parser = one_based_usize)]
        from: usize,
        #[arg(value_parser = one_based_usize)]
        to: usize,
    },
    /// Back up every preset in a setlist to a directory.
    ///
    /// One file per preset, byte for byte as the device holds it. Slow by
    /// nature: each preset has to be loaded before it can be read, so this
    /// walks the whole setlist and takes a few minutes.
    BackupAll {
        directory: std::path::PathBuf,
        #[arg(long, default_value_t = 0, value_parser = non_negative_i64)]
        setlist: i64,
    },
    /// Back up the whole pedal: every preset, setting and impulse response.
    ///
    /// Writes a bundle directory you can read, copy and restore from. Quick,
    /// because it reads each slot where it lies instead of loading it, and the
    /// preset you are playing never changes.
    BackUp {
        /// Where to write the bundle.
        directory: std::path::PathBuf,
    },
    /// Put a bundle back onto the pedal.
    ///
    /// Restores everything unless you name the parts you want.
    RestoreAll {
        directory: std::path::PathBuf,
        /// Restore only the presets.
        #[arg(long)]
        presets: bool,
        /// Restore only the global settings.
        #[arg(long)]
        globals: bool,
        /// Restore only the impulse responses.
        #[arg(long)]
        irs: bool,
    },
    /// Commit the edit buffer to a preset, making the changes permanent.
    ///
    /// Everything else edits the device's scratch buffer: change a parameter
    /// and it sounds different at once, but reload the preset and it is gone.
    Save {
        /// Where to save. Defaults to the loaded preset.
        index: Option<String>,
        /// Rename while saving. Defaults to the current name.
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 0, value_parser = non_negative_i64)]
        setlist: i64,
    },
    /// Read a device setting by numeric id, or list the ones that answer.
    Setting {
        #[arg(value_parser = non_negative_i64)]
        id: Option<i64>,
    },
    /// Write a device setting: a whole number, `on`/`off`, or a decimal.
    SetSetting {
        #[arg(value_parser = non_negative_i64)]
        id: i64,
        value: String,
    },
    /// List setlists.
    Setlists,
    /// List the impulse response slots.
    Irs,
    /// Empty an impulse response slot (1-based).
    IrClear {
        #[arg(value_parser = one_based_i64)]
        slot: i64,
    },
    /// Switch snapshot, by number as shown in `tonepush chain` (1-based).
    Snapshot {
        #[arg(value_parser = one_based_i64)]
        number: i64,
    },
    /// Move a block along the chain, by position as shown in `tonepush chain`.
    ///
    /// Writes the whole preset back, which is untested against hardware.
    Move {
        #[arg(value_parser = one_based_i64)]
        from: i64,
        #[arg(value_parser = one_based_i64)]
        to: i64,
    },
    /// Write the loaded preset to a file as JSON.
    Export { file: std::path::PathBuf },
    /// Save the loaded preset to a file exactly as the device holds it.
    ///
    /// Unlike `export`, this round-trips: `restore` puts it back byte for byte,
    /// including the parts this program does not model yet.
    Backup { file: std::path::PathBuf },
    /// Write a file saved by `backup` over the loaded preset.
    Restore { file: std::path::PathBuf },
    /// Apply a Line 6 `.hlx` preset file to the loaded preset.
    ///
    /// Applied as ordinary parameter edits, so it is as safe as editing by
    /// hand. Use --dry-run to see exactly what it would change first.
    Import {
        file: std::path::PathBuf,
        /// Print the changes without sending anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read an .hlx and show what the tone is - its blocks and what to play it
    /// through - touching no hardware.
    Inspect { file: std::path::PathBuf },
    /// Convert a .hxpreset into a portable .hlx, touching no hardware.
    ExportHlx {
        input: std::path::PathBuf,
        output: std::path::PathBuf,
    },
    /// Turn a TonePush backup into an HX Edit bundle (.hxb), touching no
    /// hardware.
    ///
    /// The presets go out as HX Edit's own symbolic JSON, which is portable
    /// across firmware in a way the pedal's own bytes are not. HX Edit 3.82
    /// reads the result: it reports the right device and firmware and extracts
    /// every preset. Restoring is still done from our own bundle, which carries
    /// the pedal's bytes and cannot lose what a conversion might.
    ExportHxb {
        /// A bundle directory written by `tonepush back-up`.
        bundle: std::path::PathBuf,
        /// Where to write the .hxb.
        output: std::path::PathBuf,
    },
    /// Lift every tone out of an HX Edit backup bundle (.hxb) into .hlx files,
    /// touching no hardware.
    ///
    /// Writes one `NNL Name.hlx` per occupied slot; empty and never-edited
    /// slots are skipped. The bundle is never modified.
    ExtractBackup {
        file: std::path::PathBuf,
        /// Directory to write the .hlx files into; created if missing.
        output: std::path::PathBuf,
    },
    /// Rebuild an HX Edit backup bundle (.hxb) into device documents.
    ///
    /// The other half of the bundle: a .hxb stores its presets as symbolic
    /// JSON, so putting one back means rebuilding the bytes the pedal reads.
    /// Writes one `NNN Name.hxpreset` per occupied slot rather than touching
    /// the pedal, so a restore can be looked at before it is trusted.
    ///
    /// Needs a device attached - not to write to, but because a .hlx does not
    /// describe everything a preset carries and the missing parts have to come
    /// from a document the device itself wrote.
    BundleToPresets {
        file: std::path::PathBuf,
        /// Directory to write the .hxpreset files into; created if missing.
        output: std::path::PathBuf,
    },
    /// Report a WAV impulse response and whether the device will accept it,
    /// touching no hardware.
    IrInfo { file: std::path::PathBuf },
    /// Rename a preset, by front-panel label (`03B`) or zero-based index (`7`).
    Rename {
        index: String,
        name: String,
        #[arg(long, default_value_t = 0, value_parser = non_negative_i64)]
        setlist: i64,
    },
    /// Fetch an object by numeric id.
    Fetch {
        #[arg(value_parser = non_negative_i64)]
        id: i64,
    },
    /// Every controller assignment in the loaded preset, from its document.
    Controllers,
    /// What controls each of a block's parameters. Reads only.
    Assignments {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        #[arg(value_parser = non_negative_i64)]
        count: i64,
    },
    /// Put a parameter under a controller, by source ordinal. Edit buffer only.
    AssignParam {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        #[arg(value_parser = non_negative_i64)]
        param: i64,
        #[arg(value_parser = controller_source)]
        source: i64,
    },
    /// Say which MIDI CC drives a parameter already under MIDI. Edit buffer
    /// only.
    ///
    /// A bypass carries its CC on the assignment itself - that is `assign
    /// --cc` - and a parameter does not: opcode 64 is the only message that
    /// says which number reaches it.
    AssignCc {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        #[arg(value_parser = non_negative_i64)]
        param: i64,
        #[arg(value_parser = midi_cc)]
        cc: i64,
    },
    /// The raw reply behind one parameter's assignment. Reads only.
    AssignmentRaw {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        #[arg(value_parser = non_negative_i64)]
        param: i64,
    },
    /// Move one end of a controller's travel. Edit buffer only.
    AssignRange {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        #[arg(value_parser = non_negative_i64)]
        param: i64,
        #[arg(value_parser = normalised)]
        value: f32,
        #[arg(long)]
        max: bool,
    },
    /// Read a footswitch's configuration, for protocol work. Reads only.
    Switch {
        #[arg(value_parser = one_based_u8)]
        switch: u8,
    },
    /// Put a block's bypass on a footswitch, or take it off. Edit buffer only.
    SwitchAssign {
        #[arg(value_parser = non_negative_i64)]
        block: i64,
        #[arg(value_parser = one_based_u8)]
        switch: u8,
        #[arg(long)]
        off: bool,
    },
    /// Change a footswitch itself: what it is called, what colour it lights,
    /// whether it holds or toggles. Edit buffer only.
    ///
    /// `--name ""` clears the name; `--colour 0` puts the LED back to Auto
    /// Color. The colour is an index into HX Edit's own list, which starts
    /// Auto Color, White, Red.
    SwitchSet {
        #[arg(value_parser = one_based_u8)]
        switch: u8,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = non_negative_i64)]
        colour: Option<i64>,
        #[arg(long)]
        momentary: Option<bool>,
    },
    /// Watch notifications the device pushes as you use the front panel.
    Watch,
    /// Decode a capture from `tools/hxsniff` without touching hardware.
    Decode { log: std::path::PathBuf },
    /// Browse the model catalog from your installed HX Edit. Needs no hardware.
    Models {
        /// Show only this category, by name.
        #[arg(long)]
        category: Option<String>,
        /// Show one model's parameters in full.
        #[arg(long)]
        model: Option<String>,
    },
}

fn main() -> Result<()> {
    // Before anything looks for the extracted resources, which a machine that
    // knew this program under its old name has filed under that name. Either
    // binary may be the first to run after an upgrade, so both do this.
    for dir in hx_catalog::home::adopt_former_name() {
        eprintln!("brought {} across from the old name", dir.display());
    }
    match Cli::parse().cmd {
        // Everything that needs no device, handled before we touch USB.
        Cmd::Decode { log } => decode_capture(&log),
        Cmd::Models { category, model } => browse_models(category, model),
        Cmd::Import {
            file,
            dry_run: true,
        } => show_import(&file),
        Cmd::Inspect { file } => inspect_hlx(&file),
        Cmd::ExportHlx { input, output } => export_hlx(&input, &output),
        Cmd::ExtractBackup { file, output } => extract_backup(&file, &output),
        Cmd::ExportHxb { bundle, output } => export_hxb(&bundle, &output),
        Cmd::IrInfo { file } => ir_info(&file),
        Cmd::List => list_devices(),
        // Reject a malformed preset address before opening anything: failing
        // after a five-second connect to say "that is not a preset" is rude.
        Cmd::Select { ref index, .. } | Cmd::Rename { ref index, .. } if slot(index).is_err() => {
            slot(index).map(|_| ())
        }
        cmd => on_device(cmd),
    }
}

/// Accept whichever form of preset address the user has to hand.
fn slot(text: &str) -> Result<i64> {
    hx_proto::rpc::parse_slot(text).with_context(|| {
        format!("{text:?} is not a preset; use a label like 03B or an index like 7")
    })
}

fn list_devices() -> Result<()> {
    let devices = hx_usb::list().context("enumerating USB devices")?;
    if devices.is_empty() {
        println!("no HX devices found");
    }
    for d in &devices {
        println!(
            "{} (pid {:#06x}) serial {} - {} presets",
            d.profile.name,
            d.profile.product_id,
            d.serial.as_deref().unwrap_or("?"),
            d.profile.presets
        );
    }
    Ok(())
}

/// Open the first attached device and run one command against it.
///
/// The reconnect retry lives in `hx-usb`, so every consumer gets it.
fn on_device(cmd: Cmd) -> Result<()> {
    let cmd = &cmd;
    let devices = hx_usb::list().context("enumerating USB devices")?;
    let Some(device) = devices.first() else {
        bail!("no HX device found - check the USB cable");
    };
    let mut session = device.open().context("opening the device")?;
    let s = &mut session;

    match cmd.clone() {
        Cmd::Info => show_info(s),
        Cmd::Preset { raw } => show_preset(s, raw),
        Cmd::Presets { setlist } => list_presets(s, setlist),
        Cmd::Chain => show_chain(s),
        Cmd::Set {
            block,
            param,
            value,
        } => set_param(s, block, &param, &value),
        Cmd::Select { index, setlist } => select(s, setlist, &index),
        Cmd::Rename {
            index,
            name,
            setlist,
        } => rename(s, setlist, &index, &name),
        Cmd::Enable { block, state } => enable(s, block, state == "on"),
        Cmd::Model { block, model } => set_model(s, block, &model),
        Cmd::Clear { block } => clear_block(s, block),
        Cmd::Move { from, to } => move_block(s, from, to),
        Cmd::Snapshot { number } => snapshot(s, number),
        Cmd::IrLoad { slot, file } => load_ir(s, slot, &file),
        Cmd::Assign { block, cc, off } => {
            if !off && !(0..=127).contains(&cc) {
                bail!("a MIDI CC is 0 to 127; {cc} is not one");
            }
            s.assign_bypass_midi(block - 1, (!off).then_some(cc))?;
            if off {
                println!("block {block} bypass no longer follows MIDI");
            } else {
                println!("block {block} bypass follows MIDI CC {cc}");
            }
            Ok(())
        }
        Cmd::Tempo { bpm } => {
            s.set_tempo(bpm)?;
            println!("tempo {bpm:.1} BPM");
            Ok(())
        }
        Cmd::Route { block, to } => route(s, block, &to),
        Cmd::SnapshotName { number, name } => {
            s.rename_snapshot(zero_based(number, "snapshots")?, &name)?;
            println!("snapshot {number} renamed to {name}");
            Ok(())
        }
        Cmd::Save {
            index,
            name,
            setlist,
        } => save_preset(s, setlist, index.as_deref(), name.as_deref()),
        Cmd::Setting { id } => show_setting(s, id),
        Cmd::SetSetting { id, value } => set_setting(s, id, &value),
        Cmd::CopyBlock { from, to } => copy_block(s, from, to),
        Cmd::CopySnapshot { from, to } => copy_snapshot(s, from, to),
        Cmd::BackupAll { directory, setlist } => backup_all(s, &directory, setlist),
        Cmd::BackUp { directory } => back_up(s, &directory),
        Cmd::RestoreAll {
            directory,
            presets,
            globals,
            irs,
        } => restore_all(s, &directory, presets, globals, irs),
        Cmd::Slot { position } => {
            let preset = session.read_preset()?;
            match preset.raw_slot(position) {
                Some(body) => println!(
                    "slot {position} kind {:?}\n{body:#?}",
                    preset.slots[position].kind
                ),
                None => println!("slot {position} has no body"),
            }
            Ok(())
        }
        Cmd::Topology => {
            let preset = session.read_preset()?;
            let catalog = hx_catalog::Catalog::load().ok();
            let name = |slot: &hx_proto::preset::Slot| -> String {
                slot.model
                    .and_then(|m| catalog.as_ref().and_then(|c| c.model_number(m)))
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| match slot.kind {
                        hx_proto::preset::Kind::Input => "Input".into(),
                        hx_proto::preset::Kind::Output => "Output".into(),
                        other => slot
                            .model
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| format!("{other:?}")),
                    })
            };
            // The endpoints carry a routing selector that is not among their
            // parameter values, and it is the first thing HX Edit shows.
            let routed = |position: usize| -> String {
                let Some(to) = preset.routing(position) else {
                    return String::new();
                };
                let slot = &preset.slots[position];
                let label = catalog
                    .as_ref()
                    .and_then(|c| {
                        let model = c.model(match slot.kind {
                            hx_proto::preset::Kind::Input => "HelixStomp_AppDSPFlowInput",
                            _ => "HelixStomp_AppDSPFlowOutputMain",
                        })?;
                        let param = model
                            .params
                            .iter()
                            .find(|p| p.id == "@input" || p.id == "@output")?;
                        c.choices(param)?.get(to as usize).cloned()
                    })
                    .unwrap_or_else(|| to.to_string());
                format!(" [{label}]")
            };
            let row = |position: usize| {
                let slot = &preset.slots[position];
                let off = if slot.enabled { "" } else { " (off)" };
                format!("{:>2} {}{off}{}", position, name(slot), routed(position))
            };

            let layout = preset.layout();
            for (n, path) in layout.paths.iter().enumerate() {
                if layout.paths.len() > 1 {
                    println!("path {}", n + 1);
                }
                let cells =
                    |slots: &[usize]| -> Vec<String> { slots.iter().map(|p| row(*p)).collect() };

                // Everything the undivided signal passes through, in order.
                let mut line = Vec::new();
                if let Some(i) = path.input {
                    line.push(row(i));
                }
                line.extend(cells(&path.head));

                if path.lanes.is_empty() {
                    line.extend(cells(&path.tail));
                    if let Some(i) = path.output {
                        line.push(row(i));
                    }
                    println!("    {}", line.join("  ->  "));
                    continue;
                }

                // The branches, then what they rejoin into.
                println!(
                    "    {}  ->  {}",
                    line.join("  ->  "),
                    path.split.map(row).unwrap_or_default()
                );
                for (l, lane) in path.lanes.iter().enumerate() {
                    println!(
                        "      {}  {}",
                        ["A", "B"][l.min(1)],
                        cells(&lane.blocks).join("  ->  ")
                    );
                }
                let mut rest = vec![path.join.map(row).unwrap_or_default()];
                rest.extend(cells(&path.tail));
                if let Some(i) = path.output {
                    rest.push(row(i));
                }
                println!("    {}", rest.join("  ->  "));
            }
            Ok(())
        }
        Cmd::Setlists => {
            for (i, name) in s.setlists()?.iter().enumerate() {
                println!("{i}  {name}");
            }
            Ok(())
        }
        Cmd::Irs => list_irs(s),
        Cmd::IrClear { slot } => clear_ir(s, slot),
        Cmd::Import { file, .. } => apply_import(s, &file),
        Cmd::Export { file } => export_to_file(s, &file),
        Cmd::Backup { file } => backup(s, &file),
        Cmd::Restore { file } => restore(s, &file),
        Cmd::BundleToPresets { file, output } => bundle_to_presets(s, &file, &output),
        Cmd::Fetch { id } => fetch(s, id),
        Cmd::Controllers => {
            let preset = s.read_preset()?;
            for a in preset.assignments() {
                println!(
                    "block {} {:?}  {}{}  {:.0}%..{:.0}%",
                    a.block,
                    a.target,
                    a.source.label(),
                    // Only MIDI has one, and the source already says "MIDI CC",
                    // so the number finishes that sentence rather than starting
                    // a second one.
                    a.cc.map(|cc| format!(" {cc}")).unwrap_or_default(),
                    a.min * 100.0,
                    a.max * 100.0
                );
            }
            Ok(())
        }
        Cmd::Assignments { block, count } => {
            for param in 0..count {
                match s.read_assignment(block, param)? {
                    Some(a) => println!("{param}: {}", a.source.label()),
                    None => println!("{param}: -"),
                }
            }
            Ok(())
        }
        Cmd::AssignmentRaw { block, param } => {
            println!("{:#?}", s.read_assignment_raw(block, param)?);
            Ok(())
        }
        Cmd::AssignRange {
            block,
            param,
            value,
            max,
        } => {
            s.set_assign_range(block, param, value, max)?;
            Ok(())
        }
        Cmd::AssignParam {
            block,
            param,
            source,
        } => {
            let source = hx_proto::rpc::Source::from_ordinal(source);
            s.assign_parameter(block, param, source)?;
            Ok(())
        }
        Cmd::AssignCc { block, param, cc } => {
            if !(0..=127).contains(&cc) {
                bail!("a MIDI CC is 0 to 127; {cc} is not one");
            }
            s.set_assign_cc(block, param, cc)?;
            println!("block {block} parameter {param} follows MIDI CC {cc}");
            Ok(())
        }
        Cmd::Switch { switch } => {
            validate_switch(s, switch)?;
            println!("{:#?}", s.read_switch(switch)?);
            Ok(())
        }
        Cmd::SwitchSet {
            switch,
            name,
            colour,
            momentary,
        } => {
            validate_switch(s, switch)?;
            if let Some(name) = name {
                // An empty name is not a name; it clears back to what the
                // switch carries, which is what opcode 60 is for.
                let name = name.trim();
                s.set_switch_label(switch, (!name.is_empty()).then_some(name))?;
            }
            if let Some(colour) = colour {
                s.set_switch_colour(switch, (colour > 0).then_some(colour))?;
            }
            if let Some(momentary) = momentary {
                s.set_switch_momentary(switch, momentary)?;
            }
            println!("{:#?}", s.read_switch(switch)?);
            Ok(())
        }
        Cmd::SwitchAssign { block, switch, off } => {
            validate_switch(s, switch)?;
            if off {
                s.unassign_bypass_footswitch(block, switch)?;
            } else {
                s.assign_bypass_footswitch(block, switch)?;
            }
            Ok(())
        }
        Cmd::Watch => watch(s),

        Cmd::List => list_devices(),
        Cmd::Decode { log } => decode_capture(&log),
        Cmd::Models { category, model } => browse_models(category, model),
        Cmd::Inspect { file } => inspect_hlx(&file),
        Cmd::ExportHlx { input, output } => export_hlx(&input, &output),
        Cmd::ExtractBackup { file, output } => extract_backup(&file, &output),
        Cmd::ExportHxb { bundle, output } => export_hxb(&bundle, &output),
        Cmd::IrInfo { file } => ir_info(&file),
    }
}

fn validate_switch(session: &hx_usb::Session, switch: u8) -> Result<()> {
    if switch > session.profile.switches {
        bail!(
            "{} has footswitches 1 to {}; {switch} is not one",
            session.profile.name,
            session.profile.switches
        );
    }
    Ok(())
}

fn select(session: &mut hx_usb::Session, setlist: i64, index: &str) -> Result<()> {
    let index = slot(index)?;
    session.select_preset(setlist, index)?;
    println!(
        "selected {} (setlist {setlist})",
        hx_proto::rpc::slot_label(index)
    );
    Ok(())
}

fn rename(session: &mut hx_usb::Session, setlist: i64, index: &str, name: &str) -> Result<()> {
    let index = slot(index)?;
    session.rename_preset(setlist, index, name)?;
    println!("{} renamed to {name}", hx_proto::rpc::slot_label(index));
    Ok(())
}

fn enable(session: &mut hx_usb::Session, block: i64, on: bool) -> Result<()> {
    session.set_enabled(block - 1, on)?;
    println!("block {block} {}", if on { "engaged" } else { "bypassed" });
    Ok(())
}

/// Swap a block's model, resolving a name through the catalog.
fn set_model(session: &mut hx_usb::Session, block: i64, model: &str) -> Result<()> {
    let number = match model.parse::<u32>() {
        Ok(n) => n,
        Err(_) => {
            let catalog = hx_catalog::Catalog::load()
                .context("naming a model needs HX Edit's catalog; use a number instead")?;
            catalog
                .symbols()
                .iter()
                .find(|s| {
                    s.model
                        .as_deref()
                        .and_then(|id| catalog.model(id))
                        .is_some_and(|m| m.name.eq_ignore_ascii_case(model))
                })
                .with_context(|| format!("no model named {model:?}"))?
                .number
        }
    };
    session.set_model(block - 1, number)?;
    println!("block {block} is now model {number}");
    Ok(())
}

fn clear_block(session: &mut hx_usb::Session, block: i64) -> Result<()> {
    session.clear_block(block - 1)?;
    println!("cleared block {block}");
    Ok(())
}

fn snapshot(session: &mut hx_usb::Session, number: i64) -> Result<()> {
    session.select_snapshot(number - 1)?;
    println!("snapshot {number}");
    Ok(())
}

fn list_irs(session: &mut hx_usb::Session) -> Result<()> {
    for (slot, name) in session.irs()? {
        println!(
            "{:>3}  {}",
            slot + 1,
            if name.is_empty() { "<empty>" } else { &name }
        );
    }
    Ok(())
}

fn clear_ir(session: &mut hx_usb::Session, slot: i64) -> Result<()> {
    session.clear_ir(slot - 1)?;
    println!("cleared IR slot {slot}");
    Ok(())
}

fn fetch(session: &mut hx_usb::Session, id: i64) -> Result<()> {
    println!("{:#?}", session.fetch(id)?);
    Ok(())
}

fn show_info(session: &mut hx_usb::Session) -> Result<()> {
    println!(
        "{} ({} presets)",
        session.profile.name, session.profile.presets
    );
    println!("device id: {:#010x}", session.profile.device_id);
    match session.read_preset() {
        Ok(p) => {
            println!("firmware:  {}", p.firmware().unwrap_or_else(|| "?".into()));
            println!("build:     {}", p.build().unwrap_or("?"));
        }
        Err(e) => println!("(could not read preset for firmware: {e})"),
    }
    Ok(())
}

fn show_preset(session: &mut hx_usb::Session, raw: bool) -> Result<()> {
    let preset = session.read_preset()?;
    if raw {
        println!("{:#?}", preset.tone);
        return Ok(());
    }
    match session.preset_info() {
        Ok((setlist, index, name)) => println!(
            "preset:   {} {}  (setlist {setlist}, index {index})",
            hx_proto::rpc::slot_label(index),
            name
        ),
        Err(e) => println!("preset:   (metadata unavailable: {e})"),
    }
    println!(
        "firmware: {}",
        preset.firmware().unwrap_or_else(|| "?".into())
    );
    println!("build:    {}", preset.build().unwrap_or("?"));
    println!("sections: {} bytes", preset.sections.len());
    Ok(())
}

fn list_presets(session: &mut hx_usb::Session, setlist: i64) -> Result<()> {
    let (_, current, _) = session.preset_info()?;
    for (index, name) in session.presets(setlist)?.iter().enumerate() {
        let index = index as i64;
        println!(
            "{} {:<24} {}",
            hx_proto::rpc::slot_label(index),
            name,
            if index == current { "<- loaded" } else { "" }
        );
    }
    Ok(())
}

/// Send a WAV to an IR slot, named after the file.
fn load_ir(session: &mut hx_usb::Session, slot: i64, file: &std::path::Path) -> Result<()> {
    let wav = wav::read(file)?;
    let samples = hx_usb::ir::prepare(&wav.samples, wav.sample_rate)?;
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("impulse")
        .chars()
        .take(20)
        .collect::<String>();

    session.upload_ir(slot - 1, &name, &samples)?;
    println!(
        "loaded {name} into IR slot {slot} ({} samples at {} Hz -> {} at 48000 Hz)",
        wav.samples.len(),
        wav.sample_rate,
        samples.len()
    );
    Ok(())
}

fn move_block(session: &mut hx_usb::Session, from: i64, to: i64) -> Result<()> {
    let mut preset = session.read_preset()?;
    if !preset.swap_slots((from - 1) as usize, (to - 1) as usize) {
        bail!("no block at position {from} or {to}; try `tonepush chain`");
    }
    session.write_preset(&preset)?;
    println!("moved block {from} to {to}");
    Ok(())
}

fn export_to_file(session: &mut hx_usb::Session, file: &std::path::Path) -> Result<()> {
    let preset = session.read_preset()?;
    let json = export_preset(&preset, hx_catalog::Catalog::load().ok().as_ref());
    write_file(file, json).with_context(|| format!("writing {file:?}"))?;
    println!("wrote {}", file.display());
    Ok(())
}

/// Save the loaded preset verbatim.
fn backup(session: &mut hx_usb::Session, file: &std::path::Path) -> Result<()> {
    let preset = session.read_preset()?;
    let bytes = preset.encode();
    write_file(file, &bytes).with_context(|| format!("writing {file:?}"))?;
    println!("wrote {} ({} bytes)", file.display(), bytes.len());
    Ok(())
}

/// Write a saved preset back over the loaded one.
///
/// The file is parsed before anything is sent. A malformed document is accepted
/// by the device and then reads back as an empty preset, so failing here is the
/// difference between "that is not a preset" and a wiped slot.
fn restore(session: &mut hx_usb::Session, file: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {file:?}"))?;
    let preset = hx_proto::Preset::parse(&bytes)
        .with_context(|| format!("{file:?} is not a preset saved by `tonepush backup`"))?;
    session.write_preset(&preset)?;
    println!("restored {} onto the loaded preset", file.display());
    Ok(())
}

fn watch(session: &mut hx_usb::Session) -> Result<()> {
    println!("watching for device notifications; ctrl-c to stop");
    loop {
        for (event, args) in session.poll_notifications()? {
            println!("event {event}: {args:?}");
        }
        // Polling returns after 20ms, so without this we would keep-alive all
        // three channels fifty times a second. Hammering the endpoint is what
        // wedged the device during development.
        std::thread::sleep(std::time::Duration::from_millis(700));
        session.keepalive()?;
    }
}

/// Commit the edit buffer to a preset slot.
fn save_preset(
    session: &mut hx_usb::Session,
    setlist: i64,
    index: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    let (_, loaded, current) = session.preset_info()?;
    let target = match index {
        Some(text) => slot(text)?,
        None => loaded,
    };
    let name = name.unwrap_or(&current);
    session.save_preset(setlist, target, name)?;
    println!("saved to {} as {name:?}", hx_proto::rpc::slot_label(target));
    Ok(())
}

/// Show one device setting, or survey the whole namespace.
fn optional_device_value<T>(result: hx_usb::Result<T>) -> hx_usb::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(hx_usb::Error::Device(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn show_setting(session: &mut hx_usb::Session, id: Option<i64>) -> Result<()> {
    match id {
        Some(id) => {
            println!("{id}: {:?}", session.object(id)?);
        }
        None => {
            // The namespace is flat and undocumented, so the useful thing is
            // to show what answers rather than pretend to name it.
            for id in 0..256 {
                if let Some(v) = optional_device_value(session.object(id))? {
                    if v != hx_proto::msgpack::Value::Nil {
                        println!("{id:>4}: {v:?}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Back up a whole setlist, one file per preset.
///
/// There is no bulk-read opcode: a preset can only be read once it is loaded,
/// so this selects each in turn. It restores the preset that was loaded when
/// it started, and stops at the first failure rather than leaving a backup
/// with silent holes in it.
fn backup_all(
    session: &mut hx_usb::Session,
    directory: &std::path::Path,
    setlist: i64,
) -> Result<()> {
    let (_, started_at, _) = session.preset_info()?;
    let names = session.presets(setlist)?;
    std::fs::create_dir_all(directory).with_context(|| format!("creating {directory:?}"))?;

    println!(
        "backing up {} presets to {}",
        names.len(),
        directory.display()
    );
    for (index, name) in names.iter().enumerate() {
        let index = index as i64;
        session
            .select_preset(setlist, index)
            .with_context(|| format!("selecting preset {index}"))?;
        let preset = session
            .read_preset()
            .with_context(|| format!("reading preset {index}"))?;

        let label = hx_proto::rpc::slot_label(index);
        let file = directory.join(format!("{label}-{}.hxpreset", sanitise(name)));
        write_file(&file, preset.encode()).with_context(|| format!("writing {file:?}"))?;
        println!("  {label}  {name}");
    }

    session.select_preset(setlist, started_at)?;
    println!("done; the preset you had loaded is back");
    Ok(())
}

/// Back up the whole pedal into a bundle directory.
fn back_up(session: &mut hx_usb::Session, directory: &std::path::Path) -> Result<()> {
    use hx_usb::backup::Step;

    let started = std::time::Instant::now();
    let manifest = hx_usb::backup::capture(session, directory, now(), |step| match step {
        Step::Presets { done, total, name } => {
            if !name.is_empty() {
                println!("  {}  {name}", hx_proto::rpc::slot_label(done as i64));
            }
            let _ = total;
        }
        Step::Globals => println!("  settings"),
        Step::Irs { done, total } => println!("  impulse response {}/{total}", done + 1),
        Step::Done => {}
    })
    .context("backing up the pedal")?;

    let kept = manifest.presets.iter().filter(|n| !n.is_empty()).count();
    println!(
        "\nbacked up {kept} presets, {} settings and {} impulse responses to {} in {:.1?}",
        manifest.globals,
        manifest.irs.len(),
        directory.display(),
        started.elapsed(),
    );
    Ok(())
}

/// Put a bundle back onto the pedal.
fn restore_all(
    session: &mut hx_usb::Session,
    directory: &std::path::Path,
    presets: bool,
    globals: bool,
    irs: bool,
) -> Result<()> {
    use hx_usb::backup::{Parts, Step};

    // Naming no part means all of them, which is what a restore usually is.
    let parts = if presets || globals || irs {
        Parts {
            presets,
            globals,
            irs,
        }
    } else {
        Parts::default()
    };

    let manifest = hx_usb::backup::open(directory).context("reading the bundle")?;
    println!(
        "restoring {} ({}), taken {}",
        directory.display(),
        manifest.device,
        manifest.captured,
    );

    let started = std::time::Instant::now();
    hx_usb::backup::restore(directory, session, parts, |step| match step {
        Step::Presets { done, total, name } => {
            if done % 10 == 0 || done + 1 == total {
                println!("  presets {}/{total}", done + 1);
            }
            let _ = name;
        }
        Step::Globals => println!("  settings"),
        Step::Irs { done, total } => println!("  impulse response {}/{total}", done + 1),
        Step::Done => {}
    })
    .context("restoring the pedal")?;
    println!("done in {:.1?}", started.elapsed());
    Ok(())
}

/// Seconds since the epoch, for stamping a bundle.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Make a preset name safe to use as a filename.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "preset".to_owned()
    } else {
        cleaned
    }
}

/// Copy one block over another and write the document back.
fn copy_block(session: &mut hx_usb::Session, from: usize, to: usize) -> Result<()> {
    let source = zero_based(from, "block positions")?;
    let target = zero_based(to, "block positions")?;
    let mut preset = session.read_preset()?;
    let block = preset
        .copy_slot(source)
        .with_context(|| format!("no block at position {from}"))?;
    if !preset.paste_slot(target, &block) {
        bail!("slot {to} cannot hold a block - inputs, outputs, splits and joins are fixed");
    }
    session.write_preset(&preset)?;
    println!("copied block {from} to {to} (unsaved - run `tonepush save`)");
    Ok(())
}

/// Copy one snapshot's settings over another.
fn copy_snapshot(session: &mut hx_usb::Session, from: usize, to: usize) -> Result<()> {
    let mut preset = session.read_preset()?;
    let count = preset.snapshots().len();
    let (a, b) = (zero_based(from, "snapshots")?, zero_based(to, "snapshots")?);
    if a >= count || b >= count {
        bail!("snapshots are numbered 1 to {count}");
    }
    let snapshot = preset.copy_snapshot(a).context("copying the snapshot")?;
    if !preset.paste_snapshot(b, &snapshot) {
        bail!("could not write snapshot {to}");
    }
    session.write_preset(&preset)?;
    println!("copied snapshot {from} to {to} (unsaved - run `tonepush save`)");
    Ok(())
}

/// Write one device setting, matching the type the device already holds.
fn validate_setting_value(id: i64, value: &hx_proto::msgpack::Value) -> Result<()> {
    use hx_proto::msgpack::Value;

    let Some(setting) = hx_proto::settings::setting(id) else {
        return Ok(());
    };
    match &setting.kind {
        hx_proto::settings::Kind::Number { min, max, .. } => {
            let number = match value {
                Value::F32(number) => f64::from(*number),
                Value::F64(number) => *number,
                _ => return Ok(()),
            };
            if (f64::from(*min)..=f64::from(*max)).contains(&number) {
                return Ok(());
            }
            bail!(
                "{} must be between {min} and {max}; got {number}",
                setting.name
            );
        }
        hx_proto::settings::Kind::Choice(choices) => {
            let Some(index) = value.as_i64() else {
                return Ok(());
            };
            if usize::try_from(index).is_ok_and(|i| i < choices.len()) {
                return Ok(());
            }
            bail!(
                "{} is choice 0 to {}; got {index}",
                setting.name,
                choices.len().saturating_sub(1)
            );
        }
        hx_proto::settings::Kind::Switch(_, _) => Ok(()),
    }
}

fn parse_setting_value(
    id: i64,
    current: &hx_proto::msgpack::Value,
    text: &str,
) -> Result<hx_proto::msgpack::Value> {
    use hx_proto::msgpack::Value;

    let value = match current {
        Value::Bool(_) if matches!(text, "true" | "on" | "1") => Value::Bool(true),
        Value::Bool(_) if matches!(text, "false" | "off" | "0") => Value::Bool(false),
        Value::Bool(_) => bail!("setting {id} is a switch; use on or off"),
        Value::F32(_) => {
            let number: f32 = text
                .parse()
                .with_context(|| format!("{text:?} is not a number"))?;
            if !number.is_finite() {
                bail!("{text:?} is not a finite number");
            }
            Value::F32(number)
        }
        Value::F64(_) => {
            let number: f64 = text
                .parse()
                .with_context(|| format!("{text:?} is not a number"))?;
            if !number.is_finite() {
                bail!("{text:?} is not a finite number");
            }
            Value::F64(number)
        }
        Value::Int(_) => Value::Int(
            text.parse()
                .with_context(|| format!("{text:?} is not a whole number"))?,
        ),
        Value::UInt(_) => Value::UInt(
            text.parse()
                .with_context(|| format!("{text:?} is not a whole number"))?,
        ),
        Value::Wide(_, width) => Value::Wide(
            text.parse()
                .with_context(|| format!("{text:?} is not a whole number"))?,
            *width,
        ),
        Value::WideInt(_, width) => Value::WideInt(
            text.parse()
                .with_context(|| format!("{text:?} is not a whole number"))?,
            *width,
        ),
        Value::Nil => bail!("setting {id} does not exist on this device"),
        other => bail!("setting {id} has an unsupported value type: {other:?}"),
    };
    Ok(value)
}

fn set_setting(session: &mut hx_usb::Session, id: i64, text: &str) -> Result<()> {
    // A value of the wrong type is refused with error -3, so read first and
    // send back the same shape.
    let current = session.object(id)?;
    let value = parse_setting_value(id, &current, text)?;

    validate_setting_value(id, &value)?;
    session.set_object(id, value)?;
    println!("{id}: {current:?} -> {:?}", session.object(id)?);
    Ok(())
}

/// Route an endpoint, resolving the destination through the catalog's menu.
fn route(session: &mut hx_usb::Session, block: i64, to: &str) -> Result<()> {
    let preset = session.read_preset()?;
    let slot = preset
        .slots
        .get(block as usize)
        .with_context(|| format!("no slot {block}"))?;
    let param_id = match slot.kind {
        hx_proto::preset::Kind::Input => "@input",
        hx_proto::preset::Kind::Output => "@output",
        other => bail!("slot {block} is {other:?}; only inputs and outputs are routed"),
    };

    let catalog =
        hx_catalog::Catalog::load().context("routing destinations come from HX Edit's catalog")?;
    let model_id = match slot.kind {
        hx_proto::preset::Kind::Input => "HelixStomp_AppDSPFlowInput",
        _ => "HelixStomp_AppDSPFlowOutputMain",
    };
    let model = catalog.model(model_id).context("endpoint model")?;
    let param = model
        .params
        .iter()
        .find(|p| p.id == param_id)
        .context("routing parameter")?;
    let choices = catalog.choices(param).context("routing menu")?;

    let needle = to.to_lowercase();
    let index = choices
        .iter()
        .position(|c| c.to_lowercase().contains(&needle))
        .with_context(|| format!("{to:?} is not one of: {}", choices.join(", ")))?;

    session.set_routing(block, index as i64)?;
    println!("slot {block} routed to {}", choices[index]);
    Ok(())
}

/// Resolve a parameter by name or index, then send the new value.
///
/// Naming the parameter is the whole point of carrying the catalog around: you
/// write `tonepush set 4 Drive 5.0` rather than counting positions. Values are typed
/// in the units HX Edit displays, and the catalog converts them.
fn set_param(session: &mut hx_usb::Session, block: i64, param: &str, value: &str) -> Result<()> {
    use hx_proto::msgpack::Value;

    let preset = session.read_preset()?;
    let slot = preset
        .slots
        .get((block - 1) as usize)
        .filter(|s| s.model.is_some())
        .with_context(|| format!("no block at position {block}; try `tonepush chain`"))?;
    let model = slot.model.unwrap();
    let catalog = hx_catalog::Catalog::load().ok();

    let index = match param.parse::<i64>() {
        Ok(index) => index,
        Err(_) => catalog
            .as_ref()
            .context("naming a parameter needs HX Edit's catalog; use an index instead")?
            .param_index(model, param)
            .with_context(|| format!("no parameter named {param:?} on this block"))?
            as i64,
    };

    let Some((catalog, described)) = catalog
        .as_ref()
        .and_then(|c| c.param(model, index as usize).map(|p| (c, p)))
    else {
        return set_param_by_index(session, block, index, value);
    };

    let native = catalog
        .parse(described, value)
        .with_context(|| format!("{value:?} is not a valid {}", described.name))?;
    let wire = match described.kind {
        hx_catalog::Kind::Switch => Value::Bool(native >= 0.5),
        _ => Value::F32(native),
    };

    session.set_param(block - 1, index, wire)?;
    println!(
        "block {block}: {} = {}",
        described.name,
        catalog.format(described, native)
    );
    Ok(())
}

/// Without the catalog there is no honest way to interpret the number, so it
/// goes through untouched.
fn set_param_by_index(
    session: &mut hx_usb::Session,
    block: i64,
    index: i64,
    value: &str,
) -> Result<()> {
    let native: f32 = value
        .parse()
        .with_context(|| format!("{value:?} is not a number"))?;
    if !native.is_finite() {
        bail!("{value:?} is not a finite number");
    }
    session.set_param(block - 1, index, hx_proto::msgpack::Value::F32(native))?;
    println!("block {block}: parameter {index} = {native}");
    Ok(())
}

/// The signal chain, named. This is where the two halves meet: the device
/// supplies numbers, the catalog supplies meaning.
fn show_chain(session: &mut hx_usb::Session) -> Result<()> {
    let preset = session.read_preset()?;
    let catalog = hx_catalog::Catalog::load().ok();

    if let Some((_, index, name)) = optional_device_value(session.preset_info())? {
        print!("{} {}", hx_proto::rpc::slot_label(index), name);
    }
    if let Some(tempo) = preset.tempo() {
        print!("   {tempo:.1} BPM");
    }
    println!();
    let snapshots = preset.snapshots();
    if !snapshots.is_empty() {
        println!("snapshots: {}", snapshots.join(", "));
    }
    println!();

    for (position, block) in preset.blocks() {
        let model = block.model.unwrap_or_default();
        let named = catalog.as_ref().and_then(|c| c.model_number(model));
        println!(
            "{:>2}. {:<24} {}",
            position + 1,
            named.map_or_else(|| format!("model {model}"), |m| m.name.clone()),
            if block.enabled { "" } else { "(bypassed)" },
        );
        show_params(catalog.as_ref(), model, &block.values);

        // Amp+Cab blocks carry a second model with its own parameters.
        if let Some(cab) = block.paired {
            let named = catalog.as_ref().and_then(|c| c.model_number(cab));
            println!(
                "    + {}",
                named.map_or_else(|| format!("model {cab}"), |m| m.name.clone())
            );
            show_params(catalog.as_ref(), cab, &block.paired_values);
        }
    }

    if catalog.is_none() {
        eprintln!("\n(install HX Edit for model and parameter names)");
    }
    Ok(())
}

/// What a file would do to the chain it is going onto, so a block on a branch
/// lands on that branch rather than on the main line.
fn load_plan_for(
    file: &std::path::Path,
    layout: Option<&hx_proto::preset::Layout>,
) -> Result<hlx::Plan> {
    let catalog = hx_catalog::Catalog::load()
        .context("reading an .hlx needs HX Edit's catalog to translate model names")?;
    hlx::read_for(file, &catalog, layout)
}

/// Read an .hlx and say what the tone is, touching no hardware.
fn inspect_hlx(file: &std::path::Path) -> Result<()> {
    let catalog = hx_catalog::Catalog::load()
        .context("reading an .hlx needs HX Edit's catalog to translate model names")?;
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {file:?}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {file:?} as JSON"))?;
    let tone = hx_catalog::inspect(&json, &catalog);

    let content = match tone.chain_content {
        hx_catalog::ChainContent::FullRig => "Full rig",
        hx_catalog::ChainContent::AmpAndCab => "Amp and cab",
        hx_catalog::ChainContent::AmpOnly => "Amp, no cab",
        hx_catalog::ChainContent::EffectsOnly => "Effects only",
    };
    let output = match tone.output_target_guess {
        hx_catalog::OutputTarget::FrfrPa => "for FRFR or a PA",
        hx_catalog::OutputTarget::GuitarCabOrDi => "for a real cab or the front of an amp",
    };
    println!("{}", tone.name);
    println!("  {content}, {output}\n");

    for block in &tone.blocks {
        let path = if block.path == 1 { " (path 2)" } else { "" };
        let state = if block.enabled { "" } else { "  bypassed" };
        println!(
            "  {}{}  {}{}",
            block.position, path, block.model_name, state
        );
    }
    if tone.blocks.is_empty() {
        println!("  (no blocks)");
    }
    for skipped in &tone.skipped {
        eprintln!("  skipped: {skipped}");
    }
    Ok(())
}

/// Convert a .hxpreset file to a portable .hlx, touching no hardware. The name
/// is not in the device document, so the file's own name stands in.
fn export_hlx(input: &std::path::Path, output: &std::path::Path) -> Result<()> {
    let catalog = hx_catalog::Catalog::load()
        .context("writing an .hlx needs HX Edit's catalog to name models")?;
    let bytes = std::fs::read(input).with_context(|| format!("reading {input:?}"))?;
    let preset = hx_proto::preset::Preset::parse(&bytes)
        .with_context(|| format!("{input:?} is not a readable .hxpreset"))?;
    let name = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");

    let written = hx_catalog::to_hlx(&preset, &catalog, name);
    write_file(output, written.to_pretty_string())
        .with_context(|| format!("writing {output:?}"))?;
    println!("wrote {}", output.display());
    for skipped in &written.skipped {
        eprintln!("  skipped: {skipped}");
    }
    Ok(())
}

/// Turn a TonePush bundle into an HX Edit `.hxb`.
fn export_hxb(bundle: &std::path::Path, output: &std::path::Path) -> Result<()> {
    let catalog = hx_catalog::Catalog::load()
        .context("writing an .hxb needs HX Edit's catalog to name models")?;
    let (manifest, saved, globals) = hx_usb::backup::for_export(bundle)
        .with_context(|| format!("reading the TonePush backup {bundle:?}"))?;

    // Each slot: its name, and its tone as the symbolic JSON HX Edit stores.
    let mut presets = Vec::with_capacity(manifest.presets.len());
    for (name, bytes) in saved {
        let tone = bytes.map(|bytes| {
            let preset = hx_proto::preset::Preset::parse(&bytes)
                .expect("backup::for_export validates preset documents");
            hx_catalog::to_hlx(&preset, &catalog, &name).document["data"]["tone"].clone()
        });
        presets.push((name, tone));
    }

    let (device, device_version, captured) = hx_usb::backup::hxb_metadata(&manifest)?;

    let bytes = hx_catalog::write_backup(&hx_catalog::NewBackup {
        setlist: manifest
            .setlists
            .first()
            .map(String::as_str)
            .unwrap_or("PRESETS"),
        presets: &presets,
        globals,
        device,
        device_version,
        captured,
    });
    write_file(output, &bytes).with_context(|| format!("writing {output:?}"))?;

    let kept = presets.iter().filter(|(_, t)| t.is_some()).count();
    println!(
        "wrote {} ({kept} presets, {} bytes)",
        output.display(),
        bytes.len()
    );
    println!(
        "note: HX Edit reads this; name it \"HX Stomp Backup <YYYY-Mon-DD>.hxb\" or its\n      backup dialog will not list it"
    );
    Ok(())
}

/// Lift every occupied tone out of an HX Edit `.hxb` backup into `.hlx` files.
/// Turn an `.hxb` bundle into device documents, ready to write to a pedal.
///
/// The half of `.hxb` that did not exist until the JSON-to-document converter
/// did: a bundle stores its presets as HX Edit's symbolic JSON, so putting one
/// back means rebuilding the bytes. This writes them out as files rather than
/// to the device, so a restore can be inspected before it is trusted.
///
/// The template is a document the *device* wrote, because a `.hlx` does not
/// describe everything a preset carries. Any preset off the same pedal will do;
/// its own chain is emptied first.
fn bundle_to_presets(
    session: &mut hx_usb::Session,
    file: &std::path::Path,
    output: &std::path::Path,
) -> Result<()> {
    let catalog = hx_catalog::Catalog::load().context("this needs HX Edit's model data")?;
    let bytes = std::fs::read(file).with_context(|| format!("reading {file:?}"))?;
    let backup =
        hx_catalog::read_backup(&bytes).with_context(|| format!("reading the backup {file:?}"))?;

    // The template comes off the pedal: a .hlx does not describe everything a
    // preset carries, and the missing parts have to be a real document's. Read
    // only - nothing here writes to the device.
    let template = session
        .read_preset_at(0, 0)
        .context("reading a template preset")?
        .context("slot 01A is empty; a template needs a preset in it")?;

    std::fs::create_dir_all(output).with_context(|| format!("creating {output:?}"))?;
    let built = hx_catalog::documents_from_backup(&backup, &template, &catalog);
    let mut written = 0;
    for (index, entry) in built.iter().enumerate() {
        let Some((name, document, report)) = entry else {
            continue;
        };
        for note in &report.skipped {
            println!("  {index:>3}  {name}: {note}");
        }
        let path = output.join(format!("{index:03} {}.hxpreset", sanitise(name)));
        write_file(&path, document.encode()).with_context(|| format!("writing {path:?}"))?;
        println!("  {index:>3}  {name}  ({} blocks)", report.blocks);
        written += 1;
    }
    println!(
        "\nbuilt {written} presets from \"{}\" into {}",
        backup.name,
        output.display()
    );
    Ok(())
}

fn extract_backup(file: &std::path::Path, output: &std::path::Path) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {file:?}"))?;
    let backup =
        hx_catalog::read_backup(&bytes).with_context(|| format!("reading the backup {file:?}"))?;
    std::fs::create_dir_all(output).with_context(|| format!("creating {output:?}"))?;

    let mut kept = 0;
    for preset in backup.occupied() {
        let name = format!("{} {}", preset.label(), sanitise(&preset.name));
        let path = output.join(format!("{name}.hlx"));
        write_file(&path, preset.to_hlx_string()).with_context(|| format!("writing {path:?}"))?;
        println!("  {}  {}", preset.label(), preset.name);
        kept += 1;
    }
    println!(
        "\nextracted {kept} presets from \"{}\" into {}",
        backup.name,
        output.display()
    );
    Ok(())
}

/// Report a WAV impulse response and whether the device will accept it. The
/// device stores at most 2048 mono samples and wedges hard on anything longer,
/// so this catches a bad file before an upload ever reaches the pedal.
fn ir_info(file: &std::path::Path) -> Result<()> {
    let wav = wav::read(file)?;
    let samples = wav.samples.len();
    let prepared = hx_usb::ir::prepare(&wav.samples, wav.sample_rate)?;
    println!("{}", file.display());
    println!("  {} Hz, mono, {samples} samples", wav.sample_rate);
    println!(
        "  will load as {} samples at {} Hz",
        prepared.len(),
        hx_usb::ir::SAMPLE_RATE
    );
    Ok(())
}

/// Show what a file would change, touching no hardware.
/// What an import would do, read against the chain it would go onto whenever
/// there is one to read.
///
/// A dry run that ignores the device is a dry run that can be wrong: a `.hlx`
/// gives a block's place along its branch's row, and which slot that is depends
/// on the chain. Reading the preset changes nothing, so this asks the device
/// when one is there and says so plainly when it is not.
fn show_import(file: &std::path::Path) -> Result<()> {
    let layout = hx_usb::list()
        .ok()
        .and_then(|devices| devices.first()?.open().ok())
        .and_then(|mut session| session.read_preset().ok())
        .map(|preset| preset.layout());
    if layout.is_none() {
        eprintln!("no device to read: showing what the file says, not where it would land\n");
    }
    let plan = load_plan_for(file, layout.as_ref())?;
    println!("{}  ({} changes)\n", plan.name, plan.steps.len());
    for step in &plan.steps {
        match step {
            hlx::Step::Model { block, name, .. } => println!("  block {block}: {name}"),
            hlx::Step::Param {
                block, name, value, ..
            } => {
                println!("  block {block}:   {name} = {value}")
            }
            hlx::Step::Enabled { block, enabled } => {
                println!(
                    "  block {block}:   {}",
                    if *enabled { "on" } else { "bypassed" }
                )
            }
        }
    }
    for skipped in &plan.skipped {
        eprintln!("  skipped: {skipped}");
    }
    Ok(())
}

fn apply_import(session: &mut hx_usb::Session, file: &std::path::Path) -> Result<()> {
    use hx_proto::msgpack::Value;
    // Read the chain first: a `.hlx` gives a block's place along its branch's
    // row, and only the target's own layout says which slot that is.
    let layout = session.read_preset()?.layout();
    let plan = load_plan_for(file, Some(&layout))?;

    for step in &plan.steps {
        match step {
            hlx::Step::Model { block, model, .. } => session.set_model(*block, *model)?,
            hlx::Step::Param {
                block,
                index,
                value,
                switch,
                ..
            } => {
                let wire = if *switch {
                    Value::Bool(*value >= 0.5)
                } else {
                    Value::F32(*value)
                };
                session.set_param(*block, *index, wire)?;
            }
            hlx::Step::Enabled { block, enabled } => session.set_enabled(*block, *enabled)?,
        }
    }

    println!("applied {} ({} changes)", plan.name, plan.steps.len());
    for skipped in &plan.skipped {
        eprintln!("skipped: {skipped}");
    }
    Ok(())
}

/// Render a preset as JSON, using the catalog for names where it can.
///
/// Deliberately not `.hlx`: that format is Line 6's, and reproducing it exactly
/// enough for HX Edit to open is not something we can verify here. This is a
/// readable dump for diffing and version control, naming what it can so the
/// file means something to a human.
fn export_preset(preset: &hx_proto::Preset, catalog: Option<&hx_catalog::Catalog>) -> String {
    use serde_json::json;

    let name_of = |model: u32| {
        catalog
            .and_then(|c| c.model_number(model))
            .map(|m| m.name.clone())
            .unwrap_or_else(|| format!("model {model}"))
    };

    let blocks: Vec<_> = preset
        .blocks()
        .map(|(position, slot)| {
            let model = slot.model.unwrap_or_default();
            let mut entry = json!({
                "position": position + 1,
                "model": name_of(model),
                "enabled": slot.enabled,
                "params": describe_params(catalog, model, &slot.values),
            });
            if let Some(cab) = slot.paired {
                entry["cab"] = json!(name_of(cab));
                entry["cabParams"] = describe_params(catalog, cab, &slot.paired_values);
            }
            entry
        })
        .collect();

    let document = json!({
        "firmware": preset.firmware(),
        "build": preset.build(),
        "tempo": preset.tempo(),
        "snapshots": preset.snapshots(),
        "blocks": blocks,
    });
    serde_json::to_string_pretty(&document).unwrap_or_default() + "\n"
}

/// Parameter values keyed by name, formatted the way HX Edit shows them.
fn describe_params(
    catalog: Option<&hx_catalog::Catalog>,
    model: u32,
    values: &[f32],
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (i, value) in values.iter().enumerate() {
        match catalog.and_then(|c| c.param(model, i).map(|p| (c, p))) {
            Some((c, p)) => {
                out.insert(p.name.clone(), serde_json::json!(c.format(p, *value)));
            }
            None => {
                out.insert(format!("param{i}"), serde_json::json!(value));
            }
        }
    }
    serde_json::Value::Object(out)
}

fn show_params(catalog: Option<&hx_catalog::Catalog>, model: u32, values: &[f32]) {
    for (i, value) in values.iter().enumerate() {
        match catalog.and_then(|c| c.param(model, i).map(|p| (c, p))) {
            Some((c, p)) => println!("      {:<18} {}", p.name, c.format(p, *value)),
            None => println!("      param {i:<12} {value}"),
        }
    }
}

fn browse_models(category: Option<String>, model: Option<String>) -> Result<()> {
    let catalog = hx_catalog::Catalog::load().context(
        "loading the model catalog from HX Edit. It ships the model and parameter \
         metadata; install HX Edit or set HX_EDIT_RESOURCES",
    )?;

    if let Some(id) = model {
        let m = catalog
            .models()
            .find(|m| m.id == id || m.name.eq_ignore_ascii_case(&id))
            .with_context(|| format!("no model matching {id:?}"))?;
        println!("{}  ({})", m.name, m.id);
        println!(
            "category {}  load {:.2}{}\n",
            m.category,
            m.load,
            if m.stereo { "  stereo" } else { "" }
        );
        for (i, p) in m.params.iter().enumerate() {
            println!(
                "  {i:>2}  {:<20} {:<12} {} .. {}   default {}",
                p.name,
                format!("{:?}", p.kind),
                catalog.format(p, p.min),
                catalog.format(p, p.max),
                catalog.format(p, p.default),
            );
        }
        return Ok(());
    }

    for c in catalog.categories() {
        if category
            .as_ref()
            .is_some_and(|want| !c.name.eq_ignore_ascii_case(want))
        {
            continue;
        }
        let models = catalog.models_in(c.id);
        if models.is_empty() {
            continue;
        }
        println!("\n{} ({})", c.name, models.len());
        for m in models {
            println!("  {:<28} {}", m.name, m.id);
        }
    }
    Ok(())
}

/// Offline path: re-decode an hxsniff capture using the same codec the live
/// transport uses, so the parser is exercised against real traffic.
fn decode_capture(path: &std::path::Path) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    let mut frames = 0usize;
    let mut messages = 0usize;

    for block in parse_hexdumps(&text) {
        let Ok(frame) = hx_proto::Frame::decode(&block) else {
            continue;
        };
        frames += 1;
        let Some((hdr, rest)) = hx_proto::ChannelHeader::decode(&frame.payload) else {
            continue;
        };
        if hdr.msg_type != hx_proto::frame::MSG_DATA || rest.len() < 8 {
            continue;
        }
        let len = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        if rest.len() < 8 + len {
            continue; // spans transfers; the live reader reassembles these
        }
        if let Ok(v) = hx_proto::msgpack::Decoder::new(&rest[8..8 + len]).value() {
            messages += 1;
            println!(
                "{:#06x} -> {:#06x}  {:?}",
                frame.src,
                frame.dst,
                hx_proto::Message::from_value(v)
            );
        }
    }
    eprintln!("\n{frames} frames, {messages} single-transfer messages decoded");
    Ok(())
}

/// Pull hex byte blocks out of an hxsniff log's indented dump lines.
fn parse_hexdumps(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<u8>> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('\t') {
            let hex = rest.split('|').next().unwrap_or("");
            let bytes: Vec<u8> = hex
                .split_whitespace()
                .skip(1) // leading offset column
                .filter_map(|t| u8::from_str_radix(t, 16).ok())
                .collect();
            cur.get_or_insert_with(Vec::new).extend(bytes);
        } else if let Some(b) = cur.take() {
            out.push(b);
        }
    }
    out.extend(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bytes_from_a_dump_line() {
        let log = "[0] +1.0 ASYNC-OUT ep=0x01 len=20\n\t0000  0c 00 00 28 01 10 ef 03  00 00 00 02 00 01 00 21 |...(...........!|\n\t0010  00 10 00 00                                      |....|\nnext\n";
        let blocks = super::parse_hexdumps(log);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].len(), 20);
        let f = hx_proto::Frame::decode(&blocks[0]).unwrap();
        assert_eq!(f.dst, 0x1001);
    }

    #[test]
    fn one_based_positions_map_to_the_slots_chain_prints() {
        assert_eq!(zero_based(1, "blocks").unwrap(), 0);
        assert_eq!(zero_based(5, "blocks").unwrap(), 4);
        assert!(zero_based(0, "blocks").is_err());
    }

    #[test]
    fn unsafe_numeric_arguments_are_rejected_before_a_device_is_opened() {
        let invalid: &[&[&str]] = &[
            &["tonepush", "set", "0", "Gain", "0.5"],
            &["tonepush", "snapshot-name", "0", "Verse"],
            &["tonepush", "copy-block", "0", "1"],
            &["tonepush", "ir-clear", "-1"],
            &["tonepush", "presets", "--setlist=-1"],
            &["tonepush", "tempo", "NaN"],
            &["tonepush", "tempo", "39.9"],
            &["tonepush", "tempo", "240.1"],
            &["tonepush", "assign-range", "0", "0", "1.1"],
            &["tonepush", "assign-param", "0", "0", "10"],
            &["tonepush", "assign-cc", "0", "0", "128"],
            &["tonepush", "switch", "0"],
            &["tonepush", "switch-set", "1", "--colour=-1"],
        ];

        for args in invalid {
            assert!(
                Cli::try_parse_from(*args).is_err(),
                "unsafe arguments parsed: {args:?}"
            );
        }
    }

    #[test]
    fn boundary_numeric_arguments_still_parse() {
        for args in [["tonepush", "tempo", "40"], ["tonepush", "tempo", "240"]] {
            assert!(Cli::try_parse_from(args).is_ok(), "did not parse: {args:?}");
        }
        assert!(Cli::try_parse_from(["tonepush", "assign-range", "0", "0", "0"]).is_ok());
        assert!(Cli::try_parse_from(["tonepush", "assign-range", "0", "0", "1"]).is_ok());
    }

    #[test]
    fn known_global_settings_enforce_their_declared_ranges() {
        use hx_proto::msgpack::Value;

        assert!(validate_setting_value(16, &Value::F32(40.0)).is_ok());
        assert!(validate_setting_value(16, &Value::F32(240.0)).is_ok());
        assert!(validate_setting_value(16, &Value::F32(39.9)).is_err());
        assert!(validate_setting_value(16, &Value::F32(240.1)).is_err());
        assert!(validate_setting_value(16, &Value::F64(40.0)).is_ok());
        assert!(validate_setting_value(16, &Value::F64(240.1)).is_err());
        assert!(validate_setting_value(97, &Value::Int(0)).is_ok());
        assert!(validate_setting_value(97, &Value::Int(11)).is_ok());
        assert!(validate_setting_value(97, &Value::Int(12)).is_err());
        assert!(validate_setting_value(97, &Value::UInt(12)).is_err());
        assert!(validate_setting_value(97, &Value::WideInt(-1, 1)).is_err());
    }

    #[test]
    fn setting_edits_preserve_the_device_wire_type() {
        use hx_proto::msgpack::Value;

        let cases = [
            (Value::Bool(false), "on", Value::Bool(true)),
            (Value::Int(0), "-1", Value::Int(-1)),
            (Value::UInt(0), "1", Value::UInt(1)),
            (Value::Wide(0, 4), "2", Value::Wide(2, 4)),
            (Value::WideInt(0, 2), "-2", Value::WideInt(-2, 2)),
            (Value::F32(0.0), "1.25", Value::F32(1.25)),
            (Value::F64(0.0), "1.25", Value::F64(1.25)),
        ];
        for (current, text, expected) in cases {
            assert_eq!(parse_setting_value(7, &current, text).unwrap(), expected);
        }

        assert!(parse_setting_value(7, &Value::Str("old".into()), "new").is_err());
        assert!(parse_setting_value(7, &Value::F64(0.0), "NaN").is_err());
    }

    #[test]
    fn setting_surveys_skip_only_device_refusals() {
        assert_eq!(optional_device_value(Ok(7)).unwrap(), Some(7));
        assert_eq!(
            optional_device_value::<()>(Err(hx_usb::Error::Device(-3))).unwrap(),
            None
        );
        assert!(matches!(
            optional_device_value::<()>(Err(hx_usb::Error::Timeout(11))),
            Err(hx_usb::Error::Timeout(11))
        ));
    }
}
