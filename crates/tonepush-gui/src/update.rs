//! Is there a newer TonePush than the one running?
//!
//! The check is a courtesy, not a feature: it asks GitHub once at startup, off
//! the UI thread, and if the answer does not arrive - no network, rate limited,
//! GitHub down - nothing is said. An editor that nags about its own version
//! while you are trying to hear a preset has its priorities wrong.

use std::sync::mpsc::{channel, Receiver};

/// The version this binary was built as.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where a person goes to get a newer one.
pub const RELEASES: &str = "https://github.com/crmne/tonepush/releases/latest";

const LATEST: &str = "https://api.github.com/repos/crmne/tonepush/releases/latest";

/// Ask GitHub for the newest release, in the background.
///
/// The receiver yields at most one value: the tag of a release newer than this
/// build. Same version or older, or any failure at all, and it yields nothing
/// and hangs up.
pub fn check() -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Some(tag) = latest_tag() {
            if newer(&tag, VERSION) {
                let _ = tx.send(tag);
            }
        }
    });
    rx
}

/// The `tag_name` of the newest release, or nothing if the question could not
/// be answered. Every failure here is the same failure - say nothing.
fn latest_tag() -> Option<String> {
    let body = ureq::get(LATEST)
        // GitHub rejects requests without one, and the honest answer is more
        // useful to them than a browser's.
        .header("User-Agent", format!("TonePush/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    Some(tag.to_owned())
}

/// Whether `tag` names a version after `ours`.
///
/// Tags are compared as the three numbers they are, so `v0.10.0` beats
/// `v0.9.0` - which a string comparison gets backwards. Anything that will not
/// parse as three numbers is treated as not newer, because guessing about
/// versions is how an editor ends up telling people to downgrade.
fn newer(tag: &str, ours: &str) -> bool {
    match (parts(tag), parts(ours)) {
        (Some(theirs), Some(mine)) => theirs > mine,
        _ => false,
    }
}

/// `v1.2.3` or `1.2.3` as `(1, 2, 3)`. A trailing `-rc1` and the like is cut
/// off first: a prerelease of a version is close enough to it for this.
fn parts(version: &str) -> Option<(u64, u64, u64)> {
    let trimmed = version.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next().filter(|s| !s.is_empty())?;
    let mut fields = core.split('.');
    let major = fields.next()?.parse().ok()?;
    let minor = fields.next().unwrap_or("0").parse().ok()?;
    let patch = fields.next().unwrap_or("0").parse().ok()?;
    // "1.2.3.4" is not a version this project makes, and treating it as 1.2.3
    // would silently ignore the part that differs.
    if fields.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_release_is_newer() {
        assert!(newer("v0.3.0", "0.2.1"));
        assert!(newer("0.2.2", "0.2.1"));
        assert!(newer("v1.0.0", "0.99.99"));
    }

    #[test]
    fn the_same_release_is_not_newer() {
        assert!(!newer("v0.2.1", "0.2.1"));
        assert!(!newer("0.2.1", "0.2.1"));
    }

    #[test]
    fn an_earlier_release_is_not_newer() {
        assert!(!newer("v0.2.0", "0.2.1"));
        assert!(!newer("v0.1.9", "0.2.1"));
    }

    /// The bug a string comparison has and a numeric one does not.
    #[test]
    fn ten_comes_after_nine() {
        assert!(newer("v0.10.0", "0.9.0"));
        assert!(!newer("v0.9.0", "0.10.0"));
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_never_newer() {
        assert!(!newer("nightly", "0.2.1"));
        assert!(!newer("", "0.2.1"));
        assert!(!newer("v0.2.1.1", "0.2.1"));
    }

    /// A prerelease compares as the version it leads to, which is enough to
    /// tell someone on 0.2.1 that 0.3.0-rc1 exists.
    #[test]
    fn a_prerelease_compares_as_its_version() {
        assert!(newer("v0.3.0-rc1", "0.2.1"));
        assert!(!newer("v0.2.1-rc1", "0.2.1"));
    }

    /// This is what the app actually compares against, so it had better parse.
    #[test]
    fn this_build_has_a_version_that_parses() {
        assert!(parts(VERSION).is_some(), "CARGO_PKG_VERSION = {VERSION}");
    }
}
