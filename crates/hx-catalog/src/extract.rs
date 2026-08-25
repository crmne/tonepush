//! Pull HX Edit's model data out of its installer, from inside the app.
//!
//! The same job as `tools/hxresources/extract.sh`, in Rust, so the editor can
//! offer it as onboarding: those files are Line 6's and are not
//! redistributable, which is exactly why the user supplies their own
//! installer and everything stays on their machine.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The files worth taking: names, parameter ranges, display formatting, the
/// number-to-model table, and the artwork.
const WANTED: [&str; 5] = [
    "HX_ModelCatalog.json",
    "HelixControls.json",
    "Helix.sym",
    "icons_models",
    "icons_category",
];

/// Where extracted resources go: the directory [`resources_dir`] reads,
/// whether or not it exists yet.
///
/// [`resources_dir`]: crate::resources_dir
pub fn destination() -> Option<PathBuf> {
    crate::home::resources()
}

/// An `HX Edit` installer sitting in the user's Downloads folder, newest
/// first, for the "check my Downloads" button.
pub fn installer_in_downloads() -> Option<PathBuf> {
    let downloads = crate::home::home()?.join("Downloads");
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(downloads)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_lowercase();
            let installer = name.contains("hx") && name.contains("edit");
            let kind = name.ends_with(".dmg") || name.ends_with(".exe");
            (installer && kind).then(|| {
                let time = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                (time, path)
            })
        })
        .collect();
    found.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    found.into_iter().next().map(|(_, path)| path)
}

/// Extract from a `.dmg` or `.exe` installer. Returns how many items landed.
pub fn from_installer(path: &Path) -> Result<usize, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("installer")
        .to_lowercase();
    if !path.is_file() {
        return Err(format!("no such file: {}", path.display()));
    }
    if name.ends_with(".dmg") && cfg!(target_os = "macos") {
        from_dmg(path)
    } else if name.ends_with(".dmg") || name.ends_with(".exe") {
        // 7-Zip reads both Line 6's self-extracting Windows installer and,
        // on most builds, the dmg's HFS filesystem.
        from_archive(path)
    } else {
        Err("expected an HX Edit .dmg or .exe installer".into())
    }
}

/// The Resources directory of an HX Edit already installed on this machine,
/// where one can be installed at all.
pub fn installed_resources() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/Line6/HX Edit.app/Contents/Resources"),
        PathBuf::from(r"C:\Program Files\Line 6\HX Edit\resources"),
    ];
    // `HOME` rather than the shared home lookup: this is macOS's per-user
    // Applications folder, and there is no Windows counterpart to look in.
    if let Some(home) = std::env::var_os("HOME") {
        candidates
            .push(PathBuf::from(home).join("Applications/Line6/HX Edit.app/Contents/Resources"));
    }
    candidates.into_iter().find(|c| c.is_dir())
}

/// Copy from an installed HX Edit: the zero-step onboarding for machines
/// that already have it.
pub fn from_installed() -> Result<usize, String> {
    let resources =
        installed_resources().ok_or_else(|| "no installed HX Edit found".to_string())?;
    copy_from(&resources)
}

/// macOS: mount the dmg natively. Newer installers hold a .pkg rather than
/// the app itself, so both shapes are handled.
fn from_dmg(dmg: &Path) -> Result<usize, String> {
    let mount = tempdir("tonepush-dmg")?;
    let status = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(dmg)
        .output()
        .map_err(|e| format!("could not run hdiutil: {e}"))?;
    if !status.status.success() {
        return Err("could not mount the dmg".into());
    }
    let result = (|| {
        if let Some(resources) = find_named(&mount, "HX Edit.app", 2)
            .map(|app| app.join("Contents/Resources"))
            .filter(|r| r.is_dir())
        {
            return copy_from(&resources);
        }
        // The installer dmg: expand the package with the system's own tool.
        let pkg = find_named(&mount, "HX Edit.pkg", 2)
            .or_else(|| find_named(&mount, "HXEdit.pkg", 2))
            .ok_or_else(|| "no HX Edit app or installer package inside the dmg".to_string())?;
        let expanded = tempdir("tonepush-pkg")?.join("expanded");
        let status = Command::new("pkgutil")
            .arg("--expand-full")
            .arg(&pkg)
            .arg(&expanded)
            .output()
            .map_err(|e| format!("could not run pkgutil: {e}"))?;
        if !status.status.success() {
            return Err("could not expand the installer package".into());
        }
        let result = find_named(&expanded, "HX_ModelCatalog.json", 8)
            .and_then(|catalog| catalog.parent().map(Path::to_path_buf))
            .ok_or_else(|| "no HX Edit data inside the package".to_string())
            .and_then(|dir| copy_from(&dir));
        let _ = std::fs::remove_dir_all(&expanded);
        result
    })();
    let _ = Command::new("hdiutil")
        .args(["detach"])
        .arg(&mount)
        .output();
    let _ = std::fs::remove_dir(&mount);
    result
}

/// Everywhere else: let 7-Zip open the installer, and keep opening what it
/// finds until the catalog appears.
///
/// The installers nest like matryoshkas: the Windows .exe holds the files
/// directly, but the Mac .dmg holds an HFS filesystem holding a .pkg holding
/// a gzip holding a cpio holding the app. 7-Zip reads every one of those
/// layers; this just keeps feeding it the next doll. Verified on a real
/// HX Edit 3.82 dmg, on Linux.
fn from_archive(installer: &Path) -> Result<usize, String> {
    let sevenzip = sevenzip().ok_or_else(|| {
        "reading the installer needs 7-Zip and none of the usual places had one \
         that runs: install p7zip (Linux) or 7-Zip (Windows), or put the one \
         you have on PATH"
            .to_string()
    })?;

    let work = tempdir("tonepush-extract")?;
    let result = (|| {
        unpack(&sevenzip, installer, &work)?;
        for _ in 0..6 {
            if let Some(catalog) = find_named(&work, "HX_ModelCatalog.json", 10) {
                return catalog
                    .parent()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| "the catalog has no directory".to_string())
                    .and_then(|dir| copy_from(&dir));
            }
            let nested = nested_archives(&work);
            if nested.is_empty() {
                break;
            }
            for archive in nested {
                let out = archive.with_file_name(format!(
                    "{}.unpacked",
                    archive.file_name().unwrap_or_default().to_string_lossy()
                ));
                // Best effort: what will not open is simply left behind.
                let _ = unpack(&sevenzip, &archive, &out);
                let _ = std::fs::remove_file(&archive);
            }
        }
        Err("no HX Edit data inside that file".to_string())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// A 7-Zip this machine will actually run.
///
/// PATH first, then the places an installer puts one without saying so. That
/// second half is the whole point on Windows: 7-Zip's installer never touches
/// PATH, so someone who has 7-Zip - sees it in Explorer's menu, opens archives
/// with it daily - still has nothing named `7z` for another program to run.
/// macOS has the same shape of problem, where an app launched from Finder
/// inherits a PATH that has never heard of Homebrew.
fn sevenzip() -> Option<PathBuf> {
    ["7z", "7za", "7zz"]
        .iter()
        .map(PathBuf::from)
        .chain(sevenzip_installed())
        .find(|bin| {
            // No arguments at all: every build of 7-Zip prints its banner and
            // exits zero, which is more than can be said for any one switch.
            Command::new(bin).output().is_ok_and(|o| o.status.success())
        })
}

/// Full paths worth trying when nothing on PATH answered.
fn sevenzip_installed() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if cfg!(target_os = "windows") {
        // Both Program Files, because a 32-bit build of this program is shown
        // the 64-bit one only under `ProgramW6432`; NanaZip because that is
        // what the Microsoft Store offers people who go looking for 7-Zip.
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            let Some(dir) = std::env::var_os(var).map(PathBuf::from) else {
                continue;
            };
            found.push(dir.join("7-Zip").join("7z.exe"));
            found.push(dir.join("7-Zip-Zstandard").join("7z.exe"));
            found.push(dir.join("NanaZip").join("NanaZipC.exe"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            found.push(local.join("Programs").join("7-Zip").join("7z.exe"));
        }
        found.extend(sevenzip_from_registry());
    } else {
        // Homebrew, MacPorts, and the distributions that keep p7zip's own
        // binaries beside its library rather than on PATH.
        for dir in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/local/bin",
            "/usr/lib/p7zip",
            "/usr/libexec/p7zip",
        ] {
            for name in ["7zz", "7z", "7za"] {
                found.push(Path::new(dir).join(name));
            }
        }
    }
    found
}

/// Ask the registry where 7-Zip went, for the install that is in neither
/// Program Files. Only ever reached on Windows, where `reg` is always there.
fn sevenzip_from_registry() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in ["HKCU", "HKLM"] {
        for value in ["Path64", "Path"] {
            let key = format!(r"{root}\SOFTWARE\7-Zip");
            if let Ok(out) = Command::new("reg")
                .args(["query", &key, "/v", value])
                .output()
            {
                found.extend(sevenzip_in(&String::from_utf8_lossy(&out.stdout)));
            }
        }
    }
    found
}

/// The `7z.exe` named by `reg query` output, whose value line reads
/// `    Path    REG_SZ    C:\Program Files\7-Zip\`.
fn sevenzip_in(registry_output: &str) -> Vec<PathBuf> {
    registry_output
        .lines()
        .filter_map(|line| line.split_once("REG_SZ"))
        .map(|(_, dir)| dir.trim())
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join("7z.exe"))
        .collect()
}

/// Run 7-Zip. Its exit code is not the verdict - it reports 2 on warnings
/// while still extracting what matters - so the caller checks for files.
fn unpack(sevenzip: &Path, archive: &Path, into: &Path) -> Result<(), String> {
    let out_flag = format!("-o{}", into.display());
    Command::new(sevenzip)
        .args(["x", &out_flag, "-y"])
        .arg(archive)
        .output()
        .map(|_| ())
        .map_err(|e| format!("could not run {}: {e}", sevenzip.display()))
}

/// Files inside the work directory that look like another layer of archive:
/// installer packages, compressed payloads, filesystem images.
fn nested_archives(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth - 1, found);
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let nested = name == "payload"
                || name == "payload~"
                || [
                    ".pkg", ".xar", ".cpio", ".gz", ".xz", ".bz2", ".hfs", ".apfs", ".dmg",
                ]
                .iter()
                .any(|ext| name.ends_with(ext));
            if nested {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, 10, &mut found);
    found
}

/// Copy the wanted files into the destination.
fn copy_from(src: &Path) -> Result<usize, String> {
    let dest = destination().ok_or_else(|| "no home directory to install into".to_string())?;
    std::fs::create_dir_all(&dest)
        .map_err(|e| format!("could not create {}: {e}", dest.display()))?;

    let mut copied = 0;
    for item in WANTED {
        let from = src.join(item);
        if !from.exists() {
            continue;
        }
        copy_recursive(&from, &dest.join(item)).map_err(|e| format!("copying {item}: {e}"))?;
        copied += 1;
    }
    for entry in
        std::fs::read_dir(src).map_err(|error| format!("reading extracted resources: {error}"))?
    {
        let entry = entry.map_err(|error| format!("reading extracted resources: {error}"))?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "models") {
            if let Some(name) = path.file_name() {
                std::fs::copy(&path, dest.join(name))
                    .map_err(|e| format!("copying models: {e}"))?;
                copied += 1;
            }
        }
    }
    if copied == 0 {
        return Err("found nothing to copy; is that an HX Edit installer?".into());
    }
    Ok(copied)
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Breadth-limited search for a file or directory by name.
fn find_named(root: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.into_iter()
        .find_map(|dir| find_named(&dir, name, depth - 1))
}

fn tempdir(prefix: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make a work directory: {e}"))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_names_a_seven_zip_wherever_it_was_installed() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\7-Zip\r\n    Path    REG_SZ    D:\\Tools\\7-Zip\\\r\n";
        assert_eq!(
            sevenzip_in(output),
            vec![PathBuf::from(r"D:\Tools\7-Zip\").join("7z.exe")]
        );
    }

    #[test]
    fn a_key_that_is_not_there_names_nothing() {
        let output = "ERROR: The system was unable to find the specified registry key or value.";
        assert!(sevenzip_in(output).is_empty());
    }

    /// The bug behind issue #1: 7-Zip's Windows installer leaves PATH alone,
    /// so its own default location has to be looked at directly.
    #[test]
    fn windows_looks_where_seven_zip_installs_itself() {
        if !cfg!(target_os = "windows") {
            return;
        }
        let found = sevenzip_installed();
        assert!(
            found.iter().any(|p| p.ends_with(r"7-Zip\7z.exe")),
            "nothing looked in 7-Zip's own install directory: {found:?}"
        );
    }
}
