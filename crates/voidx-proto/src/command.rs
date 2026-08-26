use std::fmt;

use serde_json::Value;

/// A validated absolute path in the device's node tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodePath(String);

impl NodePath {
    pub fn new(path: impl Into<String>) -> Result<Self, CommandError> {
        let path = path.into();
        if path == "root" {
            return Ok(Self(path));
        }
        let Some(tail) = path.strip_prefix("root\\") else {
            return Err(CommandError::InvalidPath(path));
        };
        if tail.is_empty()
            || tail
                .split('\\')
                .any(|part| part.is_empty() || part.chars().any(char::is_whitespace))
            || path.contains(['\0', '\r', '\n', '/', ':'])
        {
            return Err(CommandError::InvalidPath(path));
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn child(&self, name: &str) -> Result<Self, CommandError> {
        Self::new(format!("{}\\{name}", self.0))
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for NodePath {
    type Error = CommandError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for NodePath {
    type Error = CommandError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Read(NodePath),
    Browse(NodePath),
    Write {
        path: NodePath,
        value: Value,
    },
    Save {
        path: NodePath,
        value: Value,
    },
    DataRead {
        path: NodePath,
        index: usize,
        /// Chunks are one-based in VoidX. Zero and negative values are not
        /// emitted because older firmware did not reject them safely.
        chunk: usize,
    },
    DataWrite {
        path: NodePath,
        index: usize,
        /// One-based data chunk, or `-1` for the list's commit operation.
        chunk: i32,
        value: Vec<u8>,
    },
    DataSwap {
        path: NodePath,
        first: usize,
        second: usize,
    },
}

impl Command {
    pub fn data_read(path: NodePath, index: usize, chunk: usize) -> Result<Self, CommandError> {
        if chunk == 0 {
            return Err(CommandError::InvalidReadChunk(chunk));
        }
        Ok(Self::DataRead { path, index, chunk })
    }

    pub fn data_write(
        path: NodePath,
        index: usize,
        chunk: i32,
        value: Vec<u8>,
    ) -> Result<Self, CommandError> {
        if chunk < -1 {
            return Err(CommandError::InvalidWriteChunk(chunk));
        }
        Ok(Self::DataWrite {
            path,
            index,
            chunk,
            value,
        })
    }

    pub fn path(&self) -> &NodePath {
        match self {
            Self::Read(path) | Self::Browse(path) => path,
            Self::Write { path, .. }
            | Self::Save { path, .. }
            | Self::DataRead { path, .. }
            | Self::DataWrite { path, .. }
            | Self::DataSwap { path, .. } => path,
        }
    }

    /// Subject that identifies this command's response.
    pub fn response_subject(&self) -> String {
        match self {
            Self::DataRead { path, .. } => format!("dread {path}"),
            Self::DataWrite { path, .. } => format!("dwrite {path}"),
            Self::DataSwap { path, .. } => format!("dswap {path}"),
            _ => self.path().to_string(),
        }
    }

    fn encode_text(&self) -> Result<String, CommandError> {
        Ok(match self {
            Self::Read(path) => format!("read {path}"),
            Self::Browse(path) => format!("browse {path}"),
            Self::Write { path, value } => {
                format!(
                    "write {path}:{{\"value\":{}}}",
                    serde_json::to_string(value)?
                )
            }
            Self::Save { path, value } => format!(
                "write {path}:{{\"value\":{},\"save\":\"save\"}}",
                serde_json::to_string(value)?
            ),
            Self::DataRead { path, index, chunk } => {
                format!("dread {path}:{{\"index\":{index},\"chunk\":{chunk}}}")
            }
            Self::DataWrite {
                path,
                index,
                chunk,
                value,
            } => format!(
                "dwrite {path}:{{\"index\":{index},\"chunk\":{chunk},\"value\":{}}}",
                serde_json::to_string(&crate::blob::encode_hex(value))?
            ),
            Self::DataSwap {
                path,
                first,
                second,
            } => format!("dswap {path}:{{\"index\":{first},\"index2\":{second}}}"),
        })
    }

    /// Encode one complete command, including its NUL frame terminator.
    pub fn encode(&self) -> Result<Vec<u8>, CommandError> {
        let mut encoded = self.encode_text()?;
        encoded.push('\0');
        Ok(encoded.into_bytes())
    }

    /// Encode several commands in one protocol request frame.
    ///
    /// VoidX separates commands inside a frame with CRLF and terminates the
    /// complete request with one NUL byte. Keeping this here ensures callers
    /// cannot accidentally introduce an unterminated or injectable command.
    pub fn encode_batch(commands: &[Self]) -> Result<Vec<u8>, CommandError> {
        if commands.is_empty() {
            return Err(CommandError::EmptyBatch);
        }
        let mut encoded = commands
            .iter()
            .map(Self::encode_text)
            .collect::<Result<Vec<_>, _>>()?
            .join("\r\n");
        encoded.push('\0');
        Ok(encoded.into_bytes())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("a VoidX command batch cannot be empty")]
    EmptyBatch,
    #[error("invalid VoidX node path {0:?}")]
    InvalidPath(String),
    #[error("data-read chunk must be one-based; got {0}")]
    InvalidReadChunk(usize),
    #[error("data-write chunk must be -1 or a non-negative integer; got {0}")]
    InvalidWriteChunk(i32),
    #[error("could not serialize command JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_absolute_and_injection_safe() {
        assert_eq!(
            NodePath::new("root\\app\\amp").unwrap().as_str(),
            "root\\app\\amp"
        );
        for path in [
            "app\\amp",
            "root\\",
            "root\\bad name",
            "root\\bad\nread root",
        ] {
            assert!(NodePath::new(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn read_and_data_commands_match_captured_wire_format() {
        let path = NodePath::new("root\\presets").unwrap();
        assert_eq!(
            Command::Read(path.clone()).encode().unwrap(),
            b"read root\\presets\0"
        );
        assert_eq!(
            Command::data_read(path.clone(), 0, 1)
                .unwrap()
                .encode()
                .unwrap(),
            b"dread root\\presets:{\"index\":0,\"chunk\":1}\0"
        );
        assert_eq!(
            Command::data_read(path, 0, 1).unwrap().response_subject(),
            "dread root\\presets"
        );
    }

    #[test]
    fn command_batches_share_one_frame_terminator() {
        let path = NodePath::new("root\\presets").unwrap();
        let commands = [
            Command::data_read(path.clone(), 2, 1).unwrap(),
            Command::data_read(path, 2, 2).unwrap(),
        ];
        assert_eq!(
            Command::encode_batch(&commands).unwrap(),
            b"dread root\\presets:{\"index\":2,\"chunk\":1}\r\ndread root\\presets:{\"index\":2,\"chunk\":2}\0"
        );
        assert!(Command::encode_batch(&[]).is_err());
    }

    #[test]
    fn invalid_chunks_are_rejected_before_they_reach_firmware() {
        let path = NodePath::new("root\\presets").unwrap();
        assert!(Command::data_read(path.clone(), 0, 0).is_err());
        assert!(Command::data_write(path.clone(), 0, 0, vec![]).is_ok());
        assert!(Command::data_write(path, 0, -2, vec![]).is_err());
    }

    #[test]
    fn mutation_commands_have_captured_field_order_and_escaped_values() {
        let list = NodePath::new("root\\nam_amp").unwrap();
        assert_eq!(
            Command::data_write(list.clone(), 2, 0, vec![0, 0xff])
                .unwrap()
                .encode()
                .unwrap(),
            b"dwrite root\\nam_amp:{\"index\":2,\"chunk\":0,\"value\":\"00ff\"}\0"
        );
        assert_eq!(
            Command::DataSwap {
                path: list,
                first: 2,
                second: 7,
            }
            .encode()
            .unwrap(),
            b"dswap root\\nam_amp:{\"index\":2,\"index2\":7}\0"
        );
        assert_eq!(
            Command::Save {
                path: NodePath::new("root\\app\\preset").unwrap(),
                value: Value::String("A \"name\"".into()),
            }
            .encode()
            .unwrap(),
            b"write root\\app\\preset:{\"value\":\"A \\\"name\\\"\",\"save\":\"save\"}\0"
        );
    }
}
