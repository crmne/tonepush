//! Everything the program keeps on this machine sits in one directory named
//! after the program, so renaming the program moves all of it at once.
//!
//! Which would be nothing at all, except that the directory is where somebody's
//! library and setlists live, where every automatic backup of their pedal has
//! been written, and where the resources they went to some trouble to extract
//! sit. Changing the name without this does not fail: it starts up looking
//! somewhere new, finds nothing, and presents an empty library as though that
//! were the truth. Losing the lot quietly is worse than losing it loudly, and
//! neither is acceptable when the fix is one rename.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What the program used to be called, and what it is called now. Both appear
/// here and nowhere else; every other path in the program is built from the
/// current name.
const FORMER: &str = "stompchain";
const CURRENT: &str = "tonepush";

/// Bring what the old name owned across to the new one, answering with the
/// directories that moved.
///
/// Safe to run at every start, from either binary. It keys off the old
/// directory being there and the new one not, so a machine that has already
/// moved costs two directory checks, and one where somebody has started fresh
/// under the new name is left alone rather than merged into. Merging would have
/// to choose between two libraries that both claim the same slots, and there is
/// no answer to that which is right often enough to make silently.
pub fn adopt_former_name() -> Vec<PathBuf> {
    [data_home(), config_home()]
        .into_iter()
        .flatten()
        .filter_map(|base| adopt_in(&base))
        .collect()
}

/// The one move, given the directory both names live under.
///
/// Split out from the search for that directory so it can be tested against a
/// scratch directory. A test that reads `HOME` is a test that can reach the
/// real library, and this is the code that would move it.
fn adopt_in(base: &Path) -> Option<PathBuf> {
    let former = base.join(FORMER);
    let current = base.join(CURRENT);
    if !former.is_dir() || current.exists() {
        return None;
    }
    // Both sides share a parent, so this is a rename within one filesystem:
    // atomic, instant however large the library is, and either wholly done or
    // not begun. A copy would have to decide what to do when it ran out of room
    // half way through somebody's tones.
    std::fs::rename(&former, &current).ok()?;
    Some(current)
}

/// `~/.local/share`, by the same reckoning the library, the backups and the
/// extracted resources all use to find themselves.
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local/share")))
}

/// `~/.config`, likewise for the config file.
fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
}

/// This machine's home directory, under whichever of the two names it keeps
/// it. Public because anything reaching for `~` wants this and not one of the
/// two halves of it.
pub fn home() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

/// Which of the two names for a home directory this machine uses, split from
/// the environment so the Windows shape - no `HOME`, only `USERPROFILE` - can
/// be tested on a machine that is not Windows.
fn home_from(home: Option<OsString>, userprofile: Option<OsString>) -> Option<PathBuf> {
    home.or(userprofile).map(PathBuf::from)
}

/// The one directory the program owns on this machine.
fn ours() -> Option<PathBuf> {
    data_home().map(|d| d.join(CURRENT))
}

/// An override, where somebody has pointed one of these directories
/// elsewhere. Each is read here rather than at the call site, so that a
/// directory and the way to move it stay one fact.
fn overridden(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// Where HX Edit's extracted model data lives.
///
/// Every directory below is named here and nowhere else, because the code
/// that writes one and the code that reads it have to agree, and twice now
/// they have not. Both of those were the same mistake: a second copy of this
/// reckoning that had not been told Windows keeps its home directory under
/// `USERPROFILE`. One copy cannot disagree with itself.
pub fn resources() -> Option<PathBuf> {
    overridden("HX_RESOURCES_DEST").or_else(|| ours().map(|d| d.join("hx-resources")))
}

/// Where kept tones live.
pub fn library() -> Option<PathBuf> {
    overridden("TONEPUSH_LIBRARY").or_else(|| ours().map(|d| d.join("library")))
}

/// Where automatic backups live, one directory per pedal.
pub fn backups() -> Option<PathBuf> {
    overridden("TONEPUSH_BACKUPS").or_else(|| ours().map(|d| d.join("backups")))
}

/// The config file itself, which unlike the rest hangs off `~/.config`.
pub fn config() -> Option<PathBuf> {
    overridden("TONEPUSH_CONFIG")
        .or_else(|| config_home().map(|d| d.join(CURRENT).join("config.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory to play the two names out in. Nothing here reads
    /// `HOME`, so no test can reach the library on the machine running it.
    fn scratch(name: &str) -> PathBuf {
        crate::tests::scratch(&format!("home-{name}"))
    }

    /// Windows has no `HOME`. Forgetting that on one side of the extraction
    /// and not the other is what made a successful extraction unloadable.
    #[test]
    fn a_machine_with_only_userprofile_still_has_a_home() {
        assert_eq!(
            home_from(None, Some(OsString::from(r"C:\Users\cj"))),
            Some(PathBuf::from(r"C:\Users\cj"))
        );
        assert_eq!(home_from(None, None), None);
    }

    /// Every directory the program keeps hangs off one root, so a machine
    /// that resolves a home resolves all of them. The backup directory once
    /// did not: it kept a second copy of this reckoning, and answered `None`
    /// on Windows, where the automatic backups were therefore never written.
    #[test]
    fn every_directory_resolves_wherever_a_home_does() {
        let overridden = ["HX_RESOURCES_DEST", "TONEPUSH_LIBRARY", "TONEPUSH_BACKUPS"]
            .iter()
            .any(|name| std::env::var_os(name).is_some());
        let Some(root) = ours().filter(|_| !overridden) else {
            return;
        };
        for dir in [resources(), library(), backups()] {
            let dir = dir.expect("a directory beside the others");
            assert!(dir.starts_with(&root), "{dir:?} is not under {root:?}");
        }
        assert!(config().is_some(), "a home with no config file to write");
    }

    /// `HOME` wins where both are set, so a Unix machine that also sets
    /// `USERPROFILE` is unaffected.
    #[test]
    fn home_is_preferred_over_userprofile() {
        assert_eq!(
            home_from(
                Some(OsString::from("/home/cj")),
                Some(OsString::from(r"C:\Users\cj"))
            ),
            Some(PathBuf::from("/home/cj"))
        );
    }

    #[test]
    fn what_the_old_name_owned_comes_across() {
        let base = scratch("adopts");
        std::fs::create_dir_all(base.join(FORMER).join("library/objects")).unwrap();
        std::fs::write(base.join(FORMER).join("library/index.json"), b"{}").unwrap();

        assert_eq!(adopt_in(&base), Some(base.join(CURRENT)));
        assert!(base.join(CURRENT).join("library/objects").is_dir());
        assert_eq!(
            std::fs::read(base.join(CURRENT).join("library/index.json")).unwrap(),
            b"{}"
        );
        assert!(!base.join(FORMER).exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_library_already_under_the_new_name_is_not_merged_into() {
        let base = scratch("keeps");
        std::fs::create_dir_all(base.join(FORMER)).unwrap();
        std::fs::write(base.join(FORMER).join("index.json"), b"old").unwrap();
        std::fs::create_dir_all(base.join(CURRENT)).unwrap();
        std::fs::write(base.join(CURRENT).join("index.json"), b"new").unwrap();

        assert_eq!(adopt_in(&base), None);
        assert_eq!(
            std::fs::read(base.join(CURRENT).join("index.json")).unwrap(),
            b"new"
        );
        assert!(base.join(FORMER).exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_machine_that_never_knew_the_old_name_is_untouched() {
        let base = scratch("nothing");
        assert_eq!(adopt_in(&base), None);
        assert!(!base.join(CURRENT).exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The second start, and every one after it.
    #[test]
    fn running_it_twice_moves_nothing_the_second_time() {
        let base = scratch("twice");
        std::fs::create_dir_all(base.join(FORMER).join("library")).unwrap();

        assert_eq!(adopt_in(&base), Some(base.join(CURRENT)));
        assert_eq!(adopt_in(&base), None);
        assert!(base.join(CURRENT).join("library").is_dir());

        let _ = std::fs::remove_dir_all(&base);
    }
}
