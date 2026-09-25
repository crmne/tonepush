//! The update flags reach fastframe-update before anything else in `main`.
//!
//! The previous release's helper runs the installed executable as
//! `tonepush --apply-update <job>`, checks a download with `--version`, and
//! relaunches the new one with `--update-receipt <job>` or `--update-error
//! <message>`. All of them must be handled before the library is migrated,
//! any directory is adopted from the old name, or a window opens.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh home under Cargo's own scratch directory for integration tests.
fn home(name: &str) -> PathBuf {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// Runs the built GUI with every per-user location inside `home`, and no
/// display, so a window that did open would fail rather than appear.
fn tonepush(home: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tonepush-gui"));
    command
        .args(arguments)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    for variable in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        command.env(variable, home);
    }
    command.output().expect("the tonepush-gui executable runs")
}

fn entries(directory: &Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn apply_update_runs_the_helper_before_anything_else() {
    let home = home("apply-update");
    let job = home.join("missing").join("handoff.json");
    let output = tonepush(&home, &["--apply-update", job.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The helper reports a job it cannot read and exits with 1.
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(!stderr.is_empty());
    // No library, setting or resource directory was created on the way.
    assert_eq!(entries(&home), Vec::<String>::new());
}

#[test]
fn update_receipt_and_error_are_taken_off_the_command_line() {
    let home = home("receipt");
    let job = home.join("handoff.json");
    let output = tonepush(
        &home,
        &[
            "--update-receipt",
            job.to_str().unwrap(),
            "--update-error",
            "The update could not start. The previous version has been restored.",
            "--version",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The helper of every earlier release checks this exact answer.
    assert!(output.status.success(), "stderr: {stderr}");
    assert_eq!(
        stdout.trim(),
        format!("tonepush {}", env!("CARGO_PKG_VERSION")),
        "stderr: {stderr}"
    );
    assert_eq!(entries(&home), Vec::<String>::new());
}
