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
//! prefix and a modest length whose body is one notification (or that
//! reply), and steps over anything else a byte at a time until the stream
//! makes sense again. The cost of a slip is the one notification it cut
//! through.
//!
//! Only where the device's profile says so
//! (`DeviceProfile::realigns_events`): the HX Effects' so far.

/// Largest notification taken as genuine. Real ones are tens of bytes; the
/// device never has more than 4 KB unacknowledged on this channel
/// (PROTOCOL.md, "The events channel needs ACK frames"), so 4 KB leaves room
/// for any not captured yet. A false header with a length under this is
/// waited on for one more frame, and then only until a whole notification
/// has arrived behind it, so the wait is short on a channel that keeps
/// sending, and nothing is held back on a quiet one.
const MAX_MESSAGE: usize = 4 * 1024;

/// The service notifications ride on, which is also the number the device
/// answers its opening with.
const EVENTS_SERVICE: u16 = 4;

/// What the buffered bytes hold at one offset.
enum At {
    /// Fewer than eight bytes: no header to judge yet.
    Short,
    /// Not a message.
    Not,
    /// A header whose message has not all arrived.
    Waiting,
    /// A whole notification, ending at this offset.
    Notification(usize),
    /// The whole reply to the service opening, ending at this offset.
    Opened(usize),
}

/// What a whole candidate message is, going by its body: `end` is where it
/// ends.
///
/// The device sends two things here, and in the HX Edit captures (70 507
/// messages) nothing else: a notification, and the bare service number that
/// answers the service opening. Either fills the declared body exactly.
/// Anything looser lets false headers through. Stepping over an
/// out-of-order frame a byte at a time can land on bytes inside a
/// notification that pass the header check: a parameter notification
/// carries floats, and `ca 00 00 00 00` (0.0) followed by a small number
/// reads as a header for service 0 with a modest length. The stream reader
/// then fails on the body ("not MessagePack"), which ends the session (HX
/// Effects 3.80, Pi 4, 2026-10-15, after front-panel knob turns). And a
/// header in front of a real notification, over a body that merely begins
/// with a value that decodes, would take that notification with it.
///
/// The service number in the header is not held against a notification. In
/// the captures it is always 4 there, but the stream reader trusts only the
/// length of a header, and with reason: on some replies the device leaves
/// the field uninitialised.
fn whole(body: &[u8], end: usize) -> At {
    let mut decoder = hx_proto::msgpack::Decoder::new(body);
    let Ok(value) = decoder.value() else {
        return At::Not;
    };
    if decoder.remaining() != 0 {
        return At::Not;
    }
    if value.as_i64() == Some(i64::from(EVENTS_SERVICE)) {
        return At::Opened(end);
    }
    match hx_proto::rpc::Message::try_from_value(value) {
        Ok(hx_proto::rpc::Message::Notification { .. }) => At::Notification(end),
        _ => At::Not,
    }
}

#[derive(Default)]
pub(crate) struct Realign {
    buf: Vec<u8>,
    /// Whether the header at the front of `buf` was already waiting for
    /// the rest of its message when the frame before this one ended.
    front_waited: bool,
    /// Bytes stepped over to find a message boundary, over the session.
    skipped: u64,
}

impl Realign {
    /// Take one frame's bytes from the wire and return those that form
    /// whole, plausible messages, in order. A message cut across frames
    /// waits for the rest.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut pos = 0;
        let mut waiting = false;
        loop {
            match self.at(pos) {
                At::Notification(end) | At::Opened(end) => {
                    out.extend_from_slice(&self.buf[pos..end]);
                    pos = end;
                }
                At::Short => break,
                // The rest may still come, so wait for it. Two things
                // together show that it will not, and that this header was
                // not a header: it has already waited through a whole frame
                // (no message has been seen in more than two), and a whole
                // notification has arrived behind it. Neither is enough
                // alone. A long message may take more frames, and
                // what lies behind an unfinished header is inside its
                // declared body, where the bytes of a notification can turn
                // up as data. Waiting on regardless would hold back
                // everything behind a false header.
                At::Waiting if pos == 0 && self.front_waited && self.notification_behind(pos) => {
                    pos += 1;
                    self.skipped += 1;
                }
                At::Waiting => {
                    waiting = true;
                    break;
                }
                At::Not => {
                    pos += 1;
                    self.skipped += 1;
                }
            }
        }
        self.buf.drain(..pos);
        self.front_waited = waiting;
        out
    }

    fn at(&self, pos: usize) -> At {
        let Some(header) = self.buf.get(pos..pos + 8) else {
            return At::Short;
        };
        let originator = u16::from_le_bytes([header[0], header[1]]);
        let service = u16::from_le_bytes([header[2], header[3]]);
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if originator != 0 || !matches!(service, 0 | EVENTS_SERVICE) || len > MAX_MESSAGE {
            return At::Not;
        }
        let end = pos + 8 + len;
        if self.buf.len() < end {
            At::Waiting
        } else {
            whole(&self.buf[pos + 8..end], end)
        }
    }

    fn notification_behind(&self, pos: usize) -> bool {
        (pos + 1..self.buf.len()).any(|later| matches!(self.at(later), At::Notification(_)))
    }

    /// Forget a partial message, as when the channel is opened again.
    pub(crate) fn clear(&mut self) {
        self.buf.clear();
        self.front_waited = false;
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
        // whole ticks behind it, a frame later, show that it was not a
        // header.
        let garbage = [0x00, 0x00, 0x04, 0x00, 0x00, 0x10, 0x00, 0x00];
        let mut realign = Realign::default();
        assert_eq!(messages(&mut realign, &[garbage.to_vec(), ticks(2)]), 2);
        assert_eq!(realign.skipped(), 8);
    }

    #[test]
    fn a_false_header_is_given_one_more_frame() {
        // The same header and the ticks behind it in one frame: the rest of
        // a real message could still be in the next frame, so it waits for
        // that frame and no longer.
        let garbage = [0x00, 0x00, 0x04, 0x00, 0x00, 0x10, 0x00, 0x00];
        let mut realign = Realign::default();
        assert!(realign
            .push(&[&garbage[..], &ticks(2)[..]].concat())
            .is_empty());
        assert_eq!(realign.push(&ticks(1)), ticks(3));
        assert_eq!(realign.skipped(), 8);
    }

    #[test]
    fn a_split_notification_is_kept_when_its_body_holds_a_notification() {
        // A notification that carries a whole tick, stream header and all,
        // as data, cut right after it. What is behind an unfinished header
        // cannot show on its own that the header was false: these bytes are
        // inside the message. The rest arriving in the next frame can.
        use hx_proto::msgpack::{Encoder, Value};
        use hx_proto::rpc::Message;
        let args = hx_proto::msgmap! {
            68 => Value::Bin(TICK.to_vec(), 1),
            121 => Value::Int(27),
        };
        let body = Encoder::encode(&Message::Notification { event: 22, args }.to_value());
        let mut message = vec![0x00, 0x00, 0x04, 0x00];
        message.extend_from_slice(&(body.len() as u32).to_le_bytes());
        message.extend_from_slice(&body);
        let cut = message
            .windows(TICK.len())
            .position(|bytes| bytes == TICK)
            .expect("the tick is in the body")
            + TICK.len();

        let mut realign = Realign::default();
        assert!(realign.push(&message[..cut]).is_empty());
        assert_eq!(realign.push(&message[cut..]), message);
        assert_eq!(realign.skipped(), 0);
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
    fn a_header_over_another_notification_is_not_taken_for_one() {
        // A false header that declares 22 bytes: a MessagePack nil, then a
        // whole tick. The nil decodes, but the tick is the message.
        let garbage = [0x00, 0x00, 0x04, 0x00, 0x16, 0x00, 0x00, 0x00, 0xc0];
        let mut realign = Realign::default();
        assert_eq!(
            realign.push(&[&garbage[..], &ticks(2)[..]].concat()),
            ticks(2)
        );
        assert_eq!(realign.skipped(), 9);
    }

    #[test]
    fn a_split_notification_is_kept_when_its_body_looks_like_a_reply() {
        // A notification that carries the nine bytes of the service-open
        // reply as data, cut right after them. Only a notification behind
        // an unfinished one shows that its header was not a header.
        use hx_proto::msgpack::{Encoder, Value};
        use hx_proto::rpc::Message;
        let reply = vec![0, 0, 0, 0, 1, 0, 0, 0, 4];
        let args = hx_proto::msgmap! {
            68 => Value::Bin(reply.clone(), 1),
            121 => Value::Int(27),
        };
        let body = Encoder::encode(&Message::Notification { event: 22, args }.to_value());
        let mut message = vec![0x00, 0x00, 0x04, 0x00];
        message.extend_from_slice(&(body.len() as u32).to_le_bytes());
        message.extend_from_slice(&body);
        let cut = message
            .windows(reply.len())
            .position(|bytes| bytes == reply)
            .expect("the reply's bytes are in the body")
            + reply.len();

        let mut realign = Realign::default();
        assert!(realign.push(&message[..cut]).is_empty());
        assert_eq!(realign.push(&message[cut..]), message);
        assert_eq!(realign.skipped(), 0);
    }

    #[test]
    fn a_notification_passes_under_either_service_number() {
        // The header's service field is not held against a notification:
        // the stream reader trusts only the length, and the pretend pedal
        // writes 0 there.
        let mut under_zero = TICK.to_vec();
        under_zero[2] = 0;
        let mut realign = Realign::default();
        assert_eq!(realign.push(&under_zero), under_zero);
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
