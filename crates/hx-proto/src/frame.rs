//! Layer 1 and 2: USB frames and the channels multiplexed over them.

use std::fmt;

/// Normal traffic.
pub const FLAG_DATA: u8 = 0x18;
/// Channel handshake, sent when a channel is first opened.
pub const FLAG_HANDSHAKE: u8 = 0x28;

/// One bulk transfer on endpoint `0x01`/`0x81`.
///
/// ```text
/// 0..3  payload length, u24 LE (excluding this 8-byte header)
/// 3     flags
/// 4..6  destination node, u16 LE
/// 6..8  source node, u16 LE
/// 8..   payload, padded to a 4-byte boundary
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub flags: u8,
    pub dst: u16,
    pub src: u16,
    pub payload: Vec<u8>,
    /// Bytes after the declared payload up to the transfer boundary. Host
    /// frames use zeroes; real device frames sometimes expose opaque non-zero
    /// bytes here, so a faithful decode/encode must retain them.
    pub padding: Vec<u8>,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Frame {{ {:#06x} -> {:#06x} flags {:#04x}, {} bytes }}",
            self.src,
            self.dst,
            self.flags,
            self.payload.len()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer than eight bytes: not even a header.
    TooShort(usize),
    /// The declared payload length does not fit the transfer.
    LengthMismatch { declared: usize, available: usize },
    /// The transfer has bytes beyond the one padded frame it declares.
    TransferLength { expected: usize, actual: usize },
    /// The on-wire length field has only 24 bits.
    PayloadTooLarge(usize),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooShort(n) => write!(f, "frame of {n} bytes is shorter than the header"),
            FrameError::LengthMismatch {
                declared,
                available,
            } => write!(
                f,
                "frame declares {declared} payload bytes but {available} are present"
            ),
            FrameError::TransferLength { expected, actual } => write!(
                f,
                "one padded frame should occupy {expected} bytes, but the transfer has {actual}"
            ),
            FrameError::PayloadTooLarge(size) => {
                write!(
                    f,
                    "frame payload of {size} bytes does not fit its 24-bit length"
                )
            }
        }
    }
}

impl std::error::Error for FrameError {}

fn pad4(n: usize) -> usize {
    n.div_ceil(4) * 4
}

impl Frame {
    pub fn new(dst: u16, src: u16, payload: Vec<u8>) -> Self {
        let padding = vec![0; pad4(8 + payload.len()) - (8 + payload.len())];
        Frame {
            flags: FLAG_DATA,
            dst,
            src,
            payload,
            padding,
        }
    }

    pub fn decode(buf: &[u8]) -> Result<Frame, FrameError> {
        if buf.len() < 8 {
            return Err(FrameError::TooShort(buf.len()));
        }
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], 0]) as usize;
        if 8 + len > buf.len() {
            return Err(FrameError::LengthMismatch {
                declared: len,
                available: buf.len() - 8,
            });
        }
        let frame_end = 8 + len;
        let expected = pad4(frame_end);
        if buf.len() != expected {
            return Err(FrameError::TransferLength {
                expected,
                actual: buf.len(),
            });
        }
        Ok(Frame {
            flags: buf[3],
            dst: u16::from_le_bytes([buf[4], buf[5]]),
            src: u16::from_le_bytes([buf[6], buf[7]]),
            payload: buf[8..frame_end].to_vec(),
            padding: buf[frame_end..].to_vec(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let len = self.payload.len();
        let len_u32 = u32::try_from(len)
            .ok()
            .filter(|len| *len <= 0x00ff_ffff)
            .ok_or(FrameError::PayloadTooLarge(len))?;
        let mut out = Vec::with_capacity(pad4(8 + len));
        out.extend_from_slice(&len_u32.to_le_bytes()[..3]);
        out.push(self.flags);
        out.extend_from_slice(&self.dst.to_le_bytes());
        out.extend_from_slice(&self.src.to_le_bytes());
        out.extend_from_slice(&self.payload);
        let padding = pad4(8 + len) - (8 + len);
        if self.padding.len() == padding {
            out.extend_from_slice(&self.padding);
        } else {
            out.resize(out.len() + padding, 0);
        }
        Ok(out)
    }

    /// Total on-wire size including padding.
    pub fn wire_len(&self) -> usize {
        pad4(8 + self.payload.len())
    }
}

// ----------------------------------------------------------------- channels ---

/// Bare acknowledgement, no data.
pub const MSG_ACK: u16 = 0x08;
/// Set on any message carrying stream data.
///
/// This is a flag, not a value: data arrives as `0x04` on its own, as `0x0c`
/// when it piggybacks an acknowledgement, and as `0x14` alongside a keep-alive.
/// Comparing for equality silently drops the combined forms.
pub const MSG_DATA: u16 = 0x04;
/// Channel handshake.
pub const MSG_HELLO: u16 = 0x02;
/// Periodic keep-alive, roughly once a second per channel.
pub const MSG_KEEPALIVE: u16 = 0x10;

/// The 8-byte header every channel payload starts with.
///
/// `ack` paces bulk transfers: it advances by exactly 256 per data chunk, and
/// the device stops feeding a large transfer if it stops moving. It is *not* a
/// bare byte count - the host adds a fixed base, and sending the count alone
/// makes the device go quiet. See `Channel::ACK_BASE` in `hx-usb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelHeader {
    pub seq: u16,
    pub msg_type: u16,
    pub ack: u32,
}

impl ChannelHeader {
    pub const SIZE: usize = 8;

    /// Whether this message carries stream bytes.
    pub fn has_data(&self) -> bool {
        self.msg_type & MSG_DATA != 0
    }

    pub fn decode(buf: &[u8]) -> Option<(ChannelHeader, &[u8])> {
        if buf.len() < Self::SIZE {
            return None;
        }
        let h = ChannelHeader {
            seq: u16::from_be_bytes([buf[0], buf[1]]),
            msg_type: u16::from_be_bytes([buf[2], buf[3]]),
            ack: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        };
        Some((h, &buf[Self::SIZE..]))
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.msg_type.to_be_bytes());
        out.extend_from_slice(&self.ack.to_le_bytes());
    }
}

/// Identifies one bidirectional channel by its device-side and host-side nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId {
    pub device: u16,
    pub host: u16,
}

impl ChannelId {
    /// Device/session control.
    pub const CONTROL: ChannelId = ChannelId {
        device: 0x1001,
        host: 0x03ef,
    };
    /// UI and state notifications.
    pub const EVENTS: ChannelId = ChannelId {
        device: 0x1002,
        host: 0x03f0,
    };
    /// Preset and global data.
    pub const DATA: ChannelId = ChannelId {
        device: 0x1080,
        host: 0x03ed,
    };

    pub const ALL: [ChannelId; 3] = [Self::CONTROL, Self::EVENTS, Self::DATA];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first frame HX Edit sends after claiming interface 0.
    const HELLO: &[u8] = &[
        0x0c, 0x00, 0x00, 0x28, 0x01, 0x10, 0xef, 0x03, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x00,
        0x21, 0x00, 0x10, 0x00, 0x00,
    ];

    /// A frame whose 17-byte payload forces padding out to 28 bytes.
    const PADDED: &[u8] = &[
        0x11, 0x00, 0x00, 0x18, 0x01, 0x10, 0xef, 0x03, 0x00, 0x02, 0x00, 0x04, 0x00, 0x10, 0x00,
        0x00, 0x01, 0x00, 0x05, 0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn decodes_the_opening_frame() {
        let f = Frame::decode(HELLO).unwrap();
        assert_eq!(f.flags, FLAG_HANDSHAKE);
        assert_eq!(f.dst, 0x1001);
        assert_eq!(f.src, 0x03ef);
        assert_eq!(f.payload.len(), 12);
    }

    #[test]
    fn round_trips_including_padding() {
        // The padded frame is the interesting case: its payload is 17 bytes but
        // it occupies 28 on the wire, so a naive encoder would corrupt it.
        for raw in [HELLO, PADDED] {
            let f = Frame::decode(raw).unwrap();
            assert_eq!(f.encode().unwrap(), raw, "round trip failed");
            assert_eq!(f.wire_len(), raw.len());
        }
    }

    #[test]
    fn reads_the_channel_header() {
        let f = Frame::decode(PADDED).unwrap();
        let (h, rest) = ChannelHeader::decode(&f.payload).unwrap();
        assert_eq!(h.seq, 2);
        assert_eq!(h.msg_type, MSG_DATA);
        assert_eq!(h.ack, 0x1000);
        assert_eq!(rest.len(), 9);
    }

    #[test]
    fn data_is_a_flag_not_a_value() {
        // 0x0c is data plus acknowledgement, 0x14 is data plus keep-alive.
        // Both were seen in captures and both must count as carrying data.
        for t in [0x04, 0x0c, 0x14] {
            let h = ChannelHeader {
                seq: 0,
                msg_type: t,
                ack: 0,
            };
            assert!(h.has_data(), "{t:#04x} should carry data");
        }
        for t in [MSG_ACK, MSG_HELLO, MSG_KEEPALIVE] {
            let h = ChannelHeader {
                seq: 0,
                msg_type: t,
                ack: 0,
            };
            assert!(!h.has_data(), "{t:#04x} should not carry data");
        }
    }

    #[test]
    fn rejects_a_truncated_frame() {
        assert!(matches!(
            Frame::decode(&HELLO[..4]),
            Err(FrameError::TooShort(4))
        ));
        assert!(matches!(
            Frame::decode(&HELLO[..12]),
            Err(FrameError::LengthMismatch {
                declared: 12,
                available: 4
            })
        ));
    }

    #[test]
    fn rejects_trailing_frames_and_preserves_opaque_padding() {
        let mut trailing = HELLO.to_vec();
        trailing.extend_from_slice(&[0; 4]);
        assert!(matches!(
            Frame::decode(&trailing),
            Err(FrameError::TransferLength { .. })
        ));

        let mut padded = PADDED.to_vec();
        *padded.last_mut().unwrap() = 1;
        assert_eq!(Frame::decode(&padded).unwrap().encode().unwrap(), padded);
    }

    #[test]
    fn refuses_a_payload_that_does_not_fit_the_length_field() {
        let frame = Frame::new(1, 2, vec![0; 0x0100_0000]);
        assert_eq!(
            frame.encode(),
            Err(FrameError::PayloadTooLarge(0x0100_0000))
        );
    }
}
