//! Keeps the events channel's byte stream on message boundaries.
//!
//! Now and then an HX Effects (3.80) sends an events frame with one
//! notification's first four bytes moved behind the rest of it:
//!
//! ```text
//! expected  00 00 04 00 | 0d 00 00 00 <13-byte body> | 00 00 04 00 0d ...
//! received  0d 00 00 00 <13-byte body> | 00 00 04 00 | 00 00 04 00 0d ...
//! ```
//!
//! The byte count is right, so the acknowledgement stays right, but the
//! stream reader then takes a body as a length: either "1779853698 bytes",
//! which loses the session, or 262144 bytes, which it waits for while every
//! later notification piles up behind it (PROTOCOL.md, "The events stream
//! can lose its place").
//!
//! Every notification in the HX Edit captures (about 70 000, none of them
//! out of order) starts `00 00 04 00` (from the device, events service) and
//! is at most 44 bytes with that header; the reply that opens the service
//! may carry service 0. So this passes on only whole messages with that
//! prefix and a modest length, and steps over anything else a byte at a time
//! until the stream makes sense again. The cost of a slip is the one
//! notification it cut through.
//!
//! Only where the device's profile says so
//! (`DeviceProfile::realigns_events`): the HX Effects' so far.

/// Largest notification taken as genuine. Real ones are tens of bytes; the
/// device never has more than 4 KB unacknowledged on this channel
/// (PROTOCOL.md, "The events channel needs ACK frames"), so 4 KB leaves room
/// for any not captured yet. A false header with a length under this is
/// waited on until a whole message arrives behind it, so the wait is short
/// on a channel that keeps sending, and nothing is held back on a quiet one.
const MAX_MESSAGE: usize = 4 * 1024;

/// Whether a whole candidate message's body is what the device sends here:
/// for service 4 an RPC message, for service 0 (the service-open reply) any
/// MessagePack value.
///
/// Stepping over an out-of-order frame a byte at a time can land on bytes
/// inside a notification that pass the header check: a parameter
/// notification carries floats, and `ca 00 00 00 00` (0.0) followed by a
/// small number reads as a header for service 0 with a modest length. The
/// stream reader then fails on the body ("not MessagePack"), which ends the
/// session. HX Effects 3.80, Pi 4, 2026-10-15, after front-panel knob turns.
fn plausible(service: u16, body: &[u8]) -> bool {
    match hx_proto::msgpack::Decoder::new(body).value() {
        Ok(value) if service == 4 => hx_proto::rpc::Message::try_from_value(value).is_ok(),
        Ok(_) => true,
        Err(_) => false,
    }
}

/// What the buffered bytes hold at one offset.
enum At {
    /// Fewer than eight bytes: no header to judge yet.
    Short,
    /// Not a message.
    Not,
    /// A header whose message has not all arrived.
    Waiting,
    /// A whole plausible message, ending at this offset.
    Whole(usize),
}

#[derive(Default)]
pub(crate) struct Realign {
    buf: Vec<u8>,
    /// Bytes stepped over to find a message boundary, over the session.
    skipped: u64,
}

impl Realign {
    /// Take bytes from the wire and return those that form whole, plausible
    /// messages, in order. A message cut across frames waits for the rest.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut pos = 0;
        loop {
            match self.at(pos) {
                At::Whole(end) => {
                    out.extend_from_slice(&self.buf[pos..end]);
                    pos = end;
                }
                At::Short => break,
                // The rest may still come. But the device sends a message's
                // bytes together, in two frames at most, so a whole message
                // already behind this one means its header was not a header:
                // waiting on would hold back everything behind it.
                At::Waiting if !self.whole_message_behind(pos) => break,
                At::Waiting | At::Not => {
                    pos += 1;
                    self.skipped += 1;
                }
            }
        }
        self.buf.drain(..pos);
        out
    }

    fn at(&self, pos: usize) -> At {
        let Some(header) = self.buf.get(pos..pos + 8) else {
            return At::Short;
        };
        let originator = u16::from_le_bytes([header[0], header[1]]);
        let service = u16::from_le_bytes([header[2], header[3]]);
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if originator != 0 || !matches!(service, 0 | 4) || len > MAX_MESSAGE {
            return At::Not;
        }
        let end = pos + 8 + len;
        if self.buf.len() < end {
            At::Waiting
        } else if plausible(service, &self.buf[pos + 8..end]) {
            At::Whole(end)
        } else {
            At::Not
        }
    }

    fn whole_message_behind(&self, pos: usize) -> bool {
        (pos + 1..self.buf.len()).any(|later| matches!(self.at(later), At::Whole(_)))
    }

    /// Forget a partial message, as when the channel is opened again.
    pub(crate) fn clear(&mut self) {
        self.buf.clear();
    }

    pub(crate) fn skipped(&self) -> u64 {
        self.skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_proto::rpc::StreamReader;

    /// `event 22 {68: 10, 121: 27}`, the steady tick, as the device sends it.
    const TICK: [u8; 21] = [
        0x00, 0x00, 0x04, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x82, 0x69, 0x16, 0x6a, 0x84, 0x52, 0x00,
        0x44, 0x0a, 0x79, 0x1b, 0x6a, 0xc0,
    ];

    fn ticks(n: usize) -> Vec<u8> {
        TICK.repeat(n)
    }

    /// The tick with its first four bytes moved behind the rest.
    fn rotated() -> Vec<u8> {
        [&TICK[4..], &TICK[..4]].concat()
    }

    /// How many messages a stream reader finds in what `realign` passes on.
    fn messages(realign: &mut Realign, pushes: &[Vec<u8>]) -> usize {
        let mut reader = StreamReader::new();
        for bytes in pushes {
            reader.push(&realign.push(bytes));
        }
        reader.take_messages().expect("only whole messages").len()
    }

    #[test]
    fn whole_messages_pass_unchanged() {
        let mut realign = Realign::default();
        assert_eq!(realign.push(&ticks(3)), ticks(3));
        assert_eq!(realign.skipped(), 0);
    }

    #[test]
    fn a_message_split_across_frames_waits_for_the_rest() {
        // The device splits after the first four bytes, often with a
        // keep-alive on the first frame.
        let mut realign = Realign::default();
        assert!(realign.push(&TICK[..4]).is_empty());
        assert_eq!(realign.push(&TICK[4..]), TICK.to_vec());
        assert_eq!(realign.skipped(), 0);
    }

    #[test]
    fn a_rotated_frame_costs_one_notification() {
        // HX Effects capture, events frame seq 7: one rotated tick, then four.
        let frame = [rotated(), ticks(4)].concat();
        let mut realign = Realign::default();
        assert_eq!(messages(&mut realign, &[ticks(3), frame, ticks(2)]), 9);
        assert_eq!(realign.skipped(), 21);
    }

    #[test]
    fn a_rotated_frame_after_a_split_does_not_stall_the_stream() {
        // A frame ending in a tick's first four bytes, then a rotated one:
        // unfiltered, the reader pairs them up and then waits for a
        // 262144-byte message that never comes.
        let mut realign = Realign::default();
        let pushes = [
            [ticks(2), TICK[..4].to_vec()].concat(),
            [rotated(), ticks(3)].concat(),
        ];
        assert_eq!(messages(&mut realign, &pushes), 6);
    }

    #[test]
    fn the_service_open_reply_passes() {
        // HX Edit capture: the events service answers with service 0.
        let reply = [0, 0, 0, 0, 1, 0, 0, 0, 4];
        let mut realign = Realign::default();
        assert_eq!(realign.push(&reply), reply.to_vec());
    }

    #[test]
    fn a_header_over_a_body_that_is_not_a_message_is_stepped_over() {
        // Plausible headers with small lengths, over bodies the stream reader
        // would refuse: not MessagePack at all (0xc1 is never used), and a
        // map with a string key, which is MessagePack but not an RPC message
        // ("map key was neither integer nor string" / "unexpected end of
        // buffer" ended the session on the HX Effects).
        let not_msgpack = [
            0x00, 0x00, 0x04, 0x00, 0x03, 0x00, 0x00, 0x00, 0xc1, 0xc1, 0xc1,
        ];
        let not_rpc = [
            0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00, 0x00, 0x81, 0xa1, 0x78, 0x01,
        ];
        // `ca 00 00 00 00 05 00 00 00`: a float 0.0, then a key, read as
        // a service-0 header of length 5 over a truncated body.
        let float_zero = [
            0xca, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0xdc, 0xff,
        ];
        for garbage in [&not_msgpack[..], &not_rpc[..], &float_zero[..]] {
            let mut realign = Realign::default();
            let pushes = [ticks(1), garbage.to_vec(), ticks(2)];
            assert_eq!(messages(&mut realign, &pushes), 3, "{garbage:02x?}");
            assert_eq!(realign.skipped(), garbage.len() as u64, "{garbage:02x?}");
        }
    }

    #[test]
    fn a_false_header_does_not_hold_back_what_follows() {
        // A plausible header that declares 4096 bytes which never come. The
        // whole ticks behind it show that it was not a header.
        let garbage = [0x00, 0x00, 0x04, 0x00, 0x00, 0x10, 0x00, 0x00];
        let mut realign = Realign::default();
        assert_eq!(messages(&mut realign, &[garbage.to_vec(), ticks(2)]), 2);
        assert_eq!(realign.skipped(), 8);
    }

    #[test]
    fn a_header_split_from_its_body_still_waits() {
        // The device also splits after eight bytes: a whole header, and
        // nothing behind it yet.
        let mut realign = Realign::default();
        assert!(realign.push(&TICK[..8]).is_empty());
        assert_eq!(
            realign.push(&[&TICK[8..], &ticks(1)[..]].concat()),
            ticks(2)
        );
        assert_eq!(realign.skipped(), 0);
    }

    #[test]
    fn a_body_read_as_a_huge_length_is_stepped_over() {
        let mut realign = Realign::default();
        let garbage = [0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00];
        assert_eq!(messages(&mut realign, &[garbage.to_vec(), ticks(1)]), 1);
        assert_eq!(realign.skipped(), 8);
    }
}
