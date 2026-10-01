//! The two commands a StompStation PRO accepts in update mode.
//!
//! Holding UPD while the pedal starts runs a small updater built into it,
//! which reports `root\sys\_ver` as [`UPDATE_MODE_VERSION`]. Next to the
//! ordinary read commands it takes `supd <size>`, which announces an
//! application image of that many bytes, and `upd <hex>`, which appends up to
//! one batch of it. Unlike every other reply, theirs are bare decimal numbers
//! with no subject and no JSON: the batch size for `supd`, and the running
//! total of bytes received for `upd`. The pedal writes the image only once
//! that total reaches the announced size, so stopping earlier changes nothing.

use crate::blob::encode_hex;

/// `root\sys\_ver` while the pedal runs its built-in updater.
pub const UPDATE_MODE_VERSION: &str = "Update Mode";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplyError {
    #[error("update reply {0:?} is not a decimal byte count")]
    NotACount(String),
}

/// `supd <size>`: start an update of `size` bytes.
pub fn start(size: usize) -> Vec<u8> {
    format!("supd {size}\0").into_bytes()
}

/// `upd <hex>`: one batch of the image, as lowercase hex.
pub fn data(batch: &[u8]) -> Vec<u8> {
    let mut command = Vec::with_capacity(5 + batch.len() * 2);
    command.extend_from_slice(b"upd ");
    command.extend_from_slice(encode_hex(batch).as_bytes());
    command.push(0);
    command
}

/// Parse one NUL-delimited reply token, ignoring surrounding line endings.
/// `None` means the token is not an update reply at all (a subject:JSON
/// record such as a notification), which the caller skips.
pub fn parse_count(token: &[u8]) -> Option<Result<u64, ReplyError>> {
    let text = String::from_utf8_lossy(token);
    let text = text.trim_matches(|c: char| c == '\r' || c == '\n' || c == ' ');
    if text.is_empty() || text.contains(':') {
        return None;
    }
    Some(
        text.parse::<u64>()
            .ok()
            .filter(|_| text.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| ReplyError::NotACount(text.to_owned())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_nul_terminated_text() {
        assert_eq!(start(5_326_256), b"supd 5326256\0");
        assert_eq!(data(&[0x7f, 0x45, 0x0a]), b"upd 7f450a\0");
    }

    #[test]
    fn replies_are_bare_counts() {
        assert_eq!(parse_count(b"2048"), Some(Ok(2048)));
        assert_eq!(parse_count(b"\r\n4096\r\n"), Some(Ok(4096)));
        assert_eq!(parse_count(b""), None);
        assert_eq!(
            parse_count(b"root\\sys\\_meters\\in0:{\"value\":-12}"),
            None
        );
        assert!(matches!(parse_count(b"failed"), Some(Err(_))));
        assert!(matches!(parse_count(b"-1"), Some(Err(_))));
    }
}
