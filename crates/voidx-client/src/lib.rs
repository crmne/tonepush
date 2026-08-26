//! Transport-independent VoidX session and safe StompStation PRO operations.
//!
//! [`Link`] is deliberately just a blocking byte stream. USB CDC serial is
//! provided today; TCP and Bluetooth can be added without duplicating protocol,
//! backup, or verification logic.

pub mod backup;
mod device;
pub mod ir;
pub mod nam;
mod session;
pub mod transport;

pub use device::{BlobList, Device, Identity, UploadStep, WriteSafety};
pub use session::Notification;
pub use transport::{list, Found, Link, SerialLink};

/// Compare JSON values as the VoidX wire does. Firmware stores NodeFloat as
/// IEEE-754 and prints it back through its own JSON formatter, so the final
/// binary digit is not a semantic write failure. Non-numeric values remain
/// exact; compound values recurse without weakening their structure.
pub fn values_equivalent(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
    match (expected, actual) {
        (serde_json::Value::Number(left), serde_json::Value::Number(right)) => {
            let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) else {
                return left == right;
            };
            let tolerance = 1e-12_f64.max(left.abs().max(right.abs()) * 1e-12);
            (left - right).abs() <= tolerance
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| values_equivalent(left, right))
        }
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| values_equivalent(left, right))
                })
        }
        _ => expected == actual,
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error while {operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("serial-port error: {0}")]
    Serial(#[from] serialport::Error),
    #[error(transparent)]
    Command(#[from] voidx_proto::CommandError),
    #[error(transparent)]
    Decode(#[from] voidx_proto::DecodeError),
    #[error(transparent)]
    Node(#[from] voidx_proto::node::NodeError),
    #[error(transparent)]
    NodeValue(#[from] voidx_proto::NodeValueError),
    #[error(transparent)]
    Hex(#[from] voidx_proto::blob::HexError),
    #[error("timed out waiting for {subject:?}; reconnect before sending another command")]
    Timeout { subject: String },
    #[error("this session lost request/response alignment; reconnect before continuing")]
    SessionLost,
    #[error("device response for {subject:?} was invalid: {detail}")]
    InvalidResponse { subject: String, detail: String },
    #[error("{path} has no {field} metadata")]
    MissingMetadata {
        path: voidx_proto::NodePath,
        field: &'static str,
    },
    #[error("slot {index} is outside the 0..{count} range for {path}")]
    SlotOutOfRange {
        path: voidx_proto::NodePath,
        index: usize,
        count: usize,
    },
    #[error("slot {index} in {path} is empty")]
    EmptySlot {
        path: voidx_proto::NodePath,
        index: usize,
    },
    #[error("write operation refused: {0}")]
    WriteRefused(String),
    #[error("invalid slot name {0:?}; names must be 1 to 63 bytes without control characters")]
    InvalidSlotName(String),
    #[error("blob for {path} is {actual} bytes; expected exactly {expected}")]
    BlobSize {
        path: voidx_proto::NodePath,
        actual: usize,
        expected: usize,
    },
    #[error("backup error: {0}")]
    Backup(String),
    #[error("file format error: {0}")]
    Format(String),
}

impl Error {
    pub fn loses_session(&self) -> bool {
        matches!(
            self,
            Self::Io { .. } | Self::Decode(_) | Self::Timeout { .. } | Self::SessionLost
        )
    }
}

#[cfg(test)]
mod tests {
    use super::values_equivalent;
    use serde_json::json;

    #[test]
    fn firmware_float_printing_is_equivalent_but_real_changes_are_not() {
        assert!(values_equivalent(
            &json!(15.799985885620117_f64),
            &json!(15.799985885620115_f64)
        ));
        assert!(!values_equivalent(&json!(15.8), &json!(15.81)));
        assert!(values_equivalent(
            &json!({"values": [0.1, "ON"]}),
            &json!({"values": [0.10000000000000002, "ON"]})
        ));
        assert!(!values_equivalent(&json!("ON"), &json!("OFF")));
    }
}
