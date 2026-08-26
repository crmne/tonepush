use std::collections::{BTreeMap, VecDeque};
use std::io::ErrorKind;
use std::time::{Duration, Instant};

use voidx_proto::{Command, Decoder, Frame};

use crate::{Error, Link, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub frame: Frame,
}

pub(crate) struct Session<L> {
    link: L,
    decoder: Decoder,
    pending: VecDeque<Frame>,
    notifications: VecDeque<Notification>,
    timeout: Duration,
    lost: bool,
}

const MAX_NOTIFICATIONS: usize = 1024;

impl<L: Link> Session<L> {
    pub(crate) fn new(link: L, timeout: Duration) -> Self {
        Self {
            link,
            decoder: Decoder::default(),
            pending: VecDeque::new(),
            notifications: VecDeque::new(),
            timeout,
            lost: false,
        }
    }

    pub(crate) fn request(&mut self, command: Command) -> Result<Frame> {
        if self.lost {
            return Err(Error::SessionLost);
        }
        let expected = command.response_subject();
        let browse = matches!(command, Command::Browse(_));
        let encoded = command.encode()?;
        if let Err(source) = self.link.write_all(&encoded) {
            self.lost = true;
            return Err(Error::Io {
                operation: "sending a VoidX command",
                source,
            });
        }
        if let Err(source) = self.link.flush() {
            self.lost = true;
            return Err(Error::Io {
                operation: "flushing a VoidX command",
                source,
            });
        }

        let deadline = Instant::now() + self.timeout;
        loop {
            while let Some(frame) = self.pending.pop_front() {
                if frame.records().is_empty() {
                    continue;
                }
                let mut matching = Vec::new();
                let mut unsolicited = Vec::new();
                for record in frame.into_records() {
                    let belongs = if browse {
                        record.subject() == expected
                            || record
                                .subject()
                                .strip_prefix(&expected)
                                .is_some_and(|tail| tail.starts_with('\\'))
                    } else {
                        record.subject() == expected
                    };
                    if belongs {
                        matching.push(record);
                    } else {
                        unsolicited.push(record);
                    }
                }
                if !unsolicited.is_empty() {
                    if self.notifications.len() == MAX_NOTIFICATIONS {
                        self.notifications.pop_front();
                    }
                    self.notifications.push_back(Notification {
                        frame: Frame::new(unsolicited),
                    });
                }
                if !matching.is_empty() {
                    return Ok(Frame::new(matching));
                }
            }

            if Instant::now() >= deadline {
                self.lost = true;
                return Err(Error::Timeout { subject: expected });
            }

            let mut buffer = [0_u8; 8192];
            match self.link.read(&mut buffer) {
                Ok(0) => continue,
                Ok(length) => match self.decoder.push(&buffer[..length]) {
                    Ok(frames) => self.pending.extend(frames),
                    Err(error) => {
                        self.lost = true;
                        return Err(error.into());
                    }
                },
                Err(error)
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(source) => {
                    self.lost = true;
                    return Err(Error::Io {
                        operation: "receiving a VoidX response",
                        source,
                    });
                }
            }
        }
    }

    /// Send one request frame containing several independent commands, then
    /// collect exactly one response record per command. This is deliberately
    /// unavailable for browse commands, whose descendant records make their
    /// response cardinality open-ended.
    pub(crate) fn request_many(&mut self, commands: &[Command]) -> Result<Frame> {
        if self.lost {
            return Err(Error::SessionLost);
        }
        if commands
            .iter()
            .any(|command| matches!(command, Command::Browse(_)))
        {
            return Err(Error::InvalidResponse {
                subject: "command batch".into(),
                detail: "browse commands cannot be batched".into(),
            });
        }
        let mut remaining = BTreeMap::<String, usize>::new();
        for command in commands {
            *remaining.entry(command.response_subject()).or_default() += 1;
        }
        let expected_total = commands.len();
        let encoded = Command::encode_batch(commands)?;
        if let Err(source) = self.link.write_all(&encoded) {
            self.lost = true;
            return Err(Error::Io {
                operation: "sending a VoidX command batch",
                source,
            });
        }
        if let Err(source) = self.link.flush() {
            self.lost = true;
            return Err(Error::Io {
                operation: "flushing a VoidX command batch",
                source,
            });
        }

        let deadline = Instant::now() + self.timeout;
        let mut matching = Vec::with_capacity(expected_total);
        loop {
            while let Some(frame) = self.pending.pop_front() {
                for record in frame.into_records() {
                    if let Some(count) = remaining
                        .get_mut(record.subject())
                        .filter(|count| **count > 0)
                    {
                        *count -= 1;
                        matching.push(record);
                    } else {
                        if self.notifications.len() == MAX_NOTIFICATIONS {
                            self.notifications.pop_front();
                        }
                        self.notifications.push_back(Notification {
                            frame: Frame::new(vec![record]),
                        });
                    }
                }
                if matching.len() == expected_total {
                    return Ok(Frame::new(matching));
                }
            }

            if Instant::now() >= deadline {
                self.lost = true;
                return Err(Error::Timeout {
                    subject: "VoidX command batch".into(),
                });
            }

            let mut buffer = [0_u8; 8192];
            match self.link.read(&mut buffer) {
                Ok(0) => continue,
                Ok(length) => match self.decoder.push(&buffer[..length]) {
                    Ok(frames) => self.pending.extend(frames),
                    Err(error) => {
                        self.lost = true;
                        return Err(error.into());
                    }
                },
                Err(error)
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(source) => {
                    self.lost = true;
                    return Err(Error::Io {
                        operation: "receiving a VoidX response batch",
                        source,
                    });
                }
            }
        }
    }

    pub(crate) fn drain_notifications(&mut self) -> Vec<Notification> {
        self.notifications.drain(..).collect()
    }

    pub(crate) fn description(&self) -> &str {
        self.link.description()
    }

    pub(crate) fn into_link(self) -> L {
        self.link
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use super::*;
    use voidx_proto::NodePath;

    struct ScriptedLink {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for ScriptedLink {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for ScriptedLink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Link for ScriptedLink {
        fn description(&self) -> &str {
            "script"
        }
    }

    #[test]
    fn meters_are_separated_from_the_ordered_response() {
        let input = b"\0root\\sys\\_meters\\in0:{\"value\":-12}\0root\\name:{\"value\":\"StompStation PRO\"}\0";
        let link = ScriptedLink {
            input: Cursor::new(input.to_vec()),
            output: vec![],
        };
        let mut session = Session::new(link, Duration::from_secs(1));
        let frame = session
            .request(Command::Read(NodePath::new("root\\name").unwrap()))
            .unwrap();
        assert_eq!(frame.records()[0].value()["value"], "StompStation PRO");
        assert_eq!(session.drain_notifications().len(), 1);
        assert_eq!(session.into_link().output, b"read root\\name\0");
    }

    #[test]
    fn browse_accepts_descendant_records() {
        let input = b"root\\app\\amp:{\"type\":\"item\"}\r\nroot\\app\\drive:{\"type\":\"item\"}\0";
        let link = ScriptedLink {
            input: Cursor::new(input.to_vec()),
            output: vec![],
        };
        let mut session = Session::new(link, Duration::from_secs(1));
        let frame = session
            .request(Command::Browse(NodePath::new("root\\app").unwrap()))
            .unwrap();
        assert_eq!(frame.records().len(), 2);
    }

    #[test]
    fn a_batch_collects_records_across_response_frames() {
        let input = b"dread root\\presets:{\"index\":0,\"chunk\":1,\"value\":\"00\"}\0dread root\\presets:{\"index\":0,\"chunk\":2,\"value\":\"01\"}\0";
        let link = ScriptedLink {
            input: Cursor::new(input.to_vec()),
            output: vec![],
        };
        let path = NodePath::new("root\\presets").unwrap();
        let commands = [
            Command::data_read(path.clone(), 0, 1).unwrap(),
            Command::data_read(path, 0, 2).unwrap(),
        ];
        let mut session = Session::new(link, Duration::from_secs(1));
        let frame = session.request_many(&commands).unwrap();
        assert_eq!(frame.records().len(), 2);
        assert_eq!(
            session.into_link().output,
            b"dread root\\presets:{\"index\":0,\"chunk\":1}\r\ndread root\\presets:{\"index\":0,\"chunk\":2}\0"
        );
    }
}
