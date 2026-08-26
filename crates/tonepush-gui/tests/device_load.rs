//! Drives the worker's tone-load path against real hardware. Ignored by
//! default; run with a pedal attached and nothing else holding it:
//!
//! ```sh
//! cargo test -p tonepush-gui --test device_load -- --ignored
//! ```
//!
//! Everything happens in the edit buffer of whatever preset is loaded, and
//! the test ends with an undo that puts the original chain back. Nothing is
//! ever saved.

use std::time::{Duration, Instant};

use tonepush_gui::{spawn, ApplyBlock, Cmd, Evt};

fn wait_for<T>(
    rx: &std::sync::mpsc::Receiver<Evt>,
    what: &str,
    seconds: u64,
    mut pick: impl FnMut(Evt) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(evt) => {
                match &evt {
                    Evt::Failed(why) => eprintln!("  [failed] {why}"),
                    Evt::Activity(line) => eprintln!("  [activity] {line}"),
                    _ => {}
                }
                if let Some(found) = pick(evt) {
                    return found;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(e) => panic!("worker went away while waiting for {what}: {e}"),
        }
    }
    panic!("timed out waiting for {what}");
}

#[test]
#[ignore = "drives real hardware"]
fn a_symbolic_tone_loads_into_the_edit_buffer_and_undo_restores() {
    let (tx, rx) = spawn();
    tx.send(Cmd::Connect).unwrap();

    wait_for(&rx, "the device", 15, |evt| {
        matches!(evt, Evt::Connected { .. }).then_some(())
    });
    let (index, original) = wait_for(&rx, "the loaded preset", 15, |evt| match evt {
        Evt::Loaded { index, chain, .. } => Some((index, chain)),
        _ => None,
    });

    // Load into a DIFFERENT preset than the one open, so the select-first
    // branch runs against hardware too. The last preset is as far from
    // anyone's scratch work as a slot gets; only its edit buffer is touched.
    let dest = 125.min(index + 1);

    // A small tone: a Minotaur into a Scream 808 with its gain set low and
    // the whole 808 bypassed - models, a parameter, and a bypass state.
    tx.send(Cmd::LoadSteps {
        dest,
        name: "Load Test".into(),
        blocks: vec![
            ApplyBlock {
                model: 100,
                enabled: true,
                params: Vec::new(),
            },
            ApplyBlock {
                model: 101,
                enabled: false,
                params: vec![(0, 0.25, false)],
            },
        ],
    })
    .unwrap();

    let (landed_on, loaded) = wait_for(&rx, "the rebuilt chain", 90, |evt| match evt {
        Evt::Loaded { index, chain, .. } => Some((index, chain)),
        _ => None,
    });
    assert_eq!(landed_on, dest, "the load landed on the chosen preset");
    let models: Vec<u32> = loaded
        .iter()
        .filter(|b| b.kind == hx_proto::preset::Kind::Block)
        .map(|b| b.model)
        .collect();
    assert_eq!(models, vec![100, 101], "the tone's blocks, in order");
    let bypassed = loaded
        .iter()
        .find(|b| b.model == 101)
        .expect("the 808 is in the chain");
    assert!(!bypassed.enabled, "the 808 arrived bypassed");
    // The parameter came back from the device, not from our own send.
    assert!(
        bypassed
            .values
            .first()
            .is_some_and(|v| (*v - 0.25).abs() < 1e-3),
        "the 808's gain reads back as set: {:?}",
        bypassed.values.first()
    );

    // One undo puts the destination's own chain back: the load is one step.
    tx.send(Cmd::Undo).unwrap();
    let _ = wait_for(&rx, "the destination restored", 90, |evt| match evt {
        Evt::Loaded { chain, .. } => Some(chain),
        _ => None,
    });

    // And going home shows the original preset untouched by all of it.
    tx.send(Cmd::SelectPreset(index)).unwrap();
    let home = wait_for(&rx, "the original preset", 90, |evt| match evt {
        Evt::Loaded {
            index: at, chain, ..
        } => (at == index).then_some(chain),
        _ => None,
    });
    assert_eq!(
        home.iter().map(|b| b.model).collect::<Vec<_>>(),
        original.iter().map(|b| b.model).collect::<Vec<_>>(),
        "the preset the test started on still holds its chain"
    );
}

#[test]
#[ignore = "drives real hardware"]
fn a_cloud_audition_restores_the_exact_edit_buffer() {
    let (tx, rx) = spawn();
    tx.send(Cmd::Connect).unwrap();

    wait_for(&rx, "the device", 15, |evt| {
        matches!(evt, Evt::Connected { .. }).then_some(())
    });
    wait_for(&rx, "the loaded preset", 15, |evt| match evt {
        Evt::Loaded { .. } => Some(()),
        _ => None,
    });
    tx.send(Cmd::CopyPreset).unwrap();
    let original = wait_for(&rx, "the original document", 15, |evt| match evt {
        Evt::Copied { blob, .. } => Some(blob),
        _ => None,
    });

    tx.send(Cmd::AuditionSteps {
        key: 42,
        name: "Temporary Minotaur".into(),
        blocks: vec![ApplyBlock {
            model: 100,
            enabled: true,
            params: Vec::new(),
        }],
    })
    .unwrap();
    wait_for(&rx, "the audition", 90, |evt| match evt {
        Evt::Auditioning(Some(42)) => Some(()),
        _ => None,
    });

    tx.send(Cmd::EndAudition).unwrap();
    wait_for(&rx, "the audition restoration", 90, |evt| match evt {
        Evt::Auditioning(None) => Some(()),
        _ => None,
    });
    tx.send(Cmd::CopyPreset).unwrap();
    let restored = wait_for(&rx, "the restored document", 15, |evt| match evt {
        Evt::Copied { blob, .. } => Some(blob),
        _ => None,
    });

    assert_eq!(
        restored, original,
        "Done must restore every byte of the edit buffer, including unsaved work"
    );
}
