//! Wire protocol for Line 6 HX-family devices.
//!
//! The protocol has four layers, each of which this crate models separately so
//! that a capture, a live device and a test can all be driven through the same
//! code:
//!
//! 1. [`frame`] - bulk transfers on endpoint `0x01`/`0x81`, length-prefixed and
//!    padded to four bytes.
//! 2. [`frame::ChannelHeader`] - several independent channels multiplexed over
//!    those frames, each a reliable byte stream with cumulative acknowledgement.
//! 3. [`rpc::StreamReader`] - length-prefixed messages carved out of a channel's
//!    byte stream.
//! 4. [`rpc::Message`] - MessagePack request/response/notification.
//!
//! Nothing here performs I/O; see `hx-usb` for a transport.
//!
//! This crate has no dependencies, deliberately, so that a test, a capture
//! decoder or an Android shim can use it without pulling in a transport. That
//! is why its error types are written by hand rather than with `thiserror`, as
//! the other crates do.
//!
//! ```
//! use hx_proto::{frame::Frame, msgmap, msgpack::Value, rpc::{key, op, Message}};
//!
//! let request = Message::Request {
//!     txn: 1000,
//!     opcode: op::SELECT_PRESET,
//!     args: msgmap! { key::SETLIST => Value::Int(0), key::PRESET_INDEX => Value::Int(12) },
//! };
//! let frame = Frame::new(0x1001, 0x03ef, request.encode());
//! assert_eq!(frame.encode().unwrap().len() % 4, 0);
//! ```

pub mod frame;
pub mod msgpack;
pub mod preset;
pub mod rpc;
pub mod settings;

pub use frame::{ChannelHeader, ChannelId, Frame};
pub use msgpack::Value;
pub use preset::{Preset, Snapshot};
pub use rpc::Message;

/// Line 6's USB vendor id.
pub const VENDOR_ID: u16 = 0x0E41;

/// Endpoint carrying editor traffic to the device.
pub const EP_OUT: u8 = 0x01;
/// Endpoint carrying editor traffic from the device.
pub const EP_IN: u8 = 0x81;
/// The vendor-specific interface the editor protocol lives on.
pub const INTERFACE: u8 = 0;

/// A device this crate knows how to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceProfile {
    pub product_id: u16,
    pub name: &'static str,
    /// Number of user preset slots.
    pub presets: u16,
    /// Preset letters shown in each front-panel bank.
    pub presets_per_bank: u8,
    /// How many footswitches a bypass can be put on. Three on an HX Stomp,
    /// which is why its assign page offers five: FS1 to FS5 counts the two on
    /// the sides that a Stomp does not have and a Helix does.
    pub switches: u8,
    /// Family and member ids, as reported by a MIDI identity request and stored
    /// in `.hlx` files - HX Stomp is `0x00210006`.
    pub device_id: u32,
}

impl DeviceProfile {
    /// Read either a zero-based index or this device's front-panel label.
    pub fn parse_slot(&self, text: &str) -> Option<i64> {
        rpc::parse_slot_for(text, self.presets_per_bank)
    }

    /// Render a zero-based preset index as this device shows it.
    pub fn slot_label(&self, index: i64) -> String {
        rpc::slot_label_for(index, self.presets_per_bank)
    }
}

pub const HX_STOMP: DeviceProfile = DeviceProfile {
    product_id: 0x4246,
    name: "HX Stomp",
    presets: 126,
    presets_per_bank: 3,
    switches: 5,
    device_id: 0x0021_0006,
};
pub const HX_STOMP_XL: DeviceProfile = DeviceProfile {
    product_id: 0x4253,
    name: "HX Stomp XL",
    presets: 128,
    presets_per_bank: 3,
    switches: 8,
    device_id: 0x0021_000B,
};
pub const HELIX_FLOOR: DeviceProfile = DeviceProfile {
    product_id: 0x4248,
    name: "Helix Floor",
    presets: 128,
    presets_per_bank: 3,
    switches: 10,
    device_id: 0x0021_0001,
};
pub const HELIX_RACK: DeviceProfile = DeviceProfile {
    product_id: 0x4249,
    name: "Helix Rack",
    presets: 128,
    presets_per_bank: 3,
    switches: 10,
    device_id: 0x0021_0002,
};
pub const HELIX_LT: DeviceProfile = DeviceProfile {
    product_id: 0x424A,
    name: "Helix LT",
    presets: 128,
    presets_per_bank: 3,
    switches: 10,
    device_id: 0x0021_0004,
};
pub const HX_EFFECTS: DeviceProfile = DeviceProfile {
    product_id: 0x4245,
    name: "HX Effects",
    presets: 128,
    presets_per_bank: 4,
    switches: 6,
    device_id: 0x0021_0005,
};

/// Every device profile we recognise.
///
/// These are the six hardware families HX Edit 3.80 speaks this protocol to.
/// POD Go and HX One use their own editors and are not presumed compatible
/// merely because they share some models with the Helix/HX family.
pub const PROFILES: &[DeviceProfile] = &[
    HELIX_FLOOR,
    HELIX_RACK,
    HELIX_LT,
    HX_EFFECTS,
    HX_STOMP,
    HX_STOMP_XL,
];

pub fn profile_for(product_id: u16) -> Option<&'static DeviceProfile> {
    PROFILES.iter().find(|p| p.product_id == product_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hx_edit_device_is_recognised_by_its_usb_product_id() {
        let expected = [
            (0x4248, HELIX_FLOOR),
            (0x4249, HELIX_RACK),
            (0x424A, HELIX_LT),
            (0x4245, HX_EFFECTS),
            (0x4246, HX_STOMP),
            (0x4253, HX_STOMP_XL),
        ];
        for (product_id, profile) in expected {
            assert_eq!(profile_for(product_id), Some(&profile));
        }
    }

    #[test]
    fn hx_effects_uses_four_presets_per_bank() {
        assert_eq!(HX_EFFECTS.slot_label(3), "01D");
        assert_eq!(HX_EFFECTS.slot_label(104), "27A");
        assert_eq!(HX_EFFECTS.parse_slot("27A"), Some(104));
        assert_eq!(HX_EFFECTS.parse_slot("01D"), Some(3));
    }
}
