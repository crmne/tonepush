//! Pure codec and data model for the VoidX control protocol.
//!
//! VoidX is a NUL-framed, CRLF-delimited protocol whose records consist of a
//! subject, a colon, and JSON. This crate intentionally performs no I/O. A
//! serial, TCP, Bluetooth, capture, or test transport can all use the same
//! command and frame types.

pub mod blob;
pub mod command;
pub mod frame;
pub mod node;
pub mod preset;

pub use command::{Command, CommandError, NodePath};
pub use frame::{DecodeError, Decoder, Frame, Record};
pub use node::{NodeDescription, NodeKind, NodeTree, NodeValueError};
pub use preset::{Preset, PresetError};
