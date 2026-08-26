use serde_json::Value;

/// One JSON record inside a VoidX frame.
///
/// The original JSON spelling is retained so reading and re-encoding a preset
/// does not silently alter floating-point formatting or object key order.
#[derive(Debug, Clone)]
pub struct Record {
    subject: String,
    value: Value,
    raw_json: String,
}

impl Record {
    pub fn parse(line: &str) -> Result<Self, DecodeError> {
        let (subject, raw_json) = line
            .split_once(':')
            .ok_or_else(|| DecodeError::MalformedRecord(line.to_owned()))?;
        if subject.is_empty() || subject.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) {
            return Err(DecodeError::MalformedRecord(line.to_owned()));
        }
        let value = match serde_json::from_str(raw_json) {
            Ok(value) => value,
            Err(original) => {
                let context = json_error_context(raw_json, original.line(), original.column());
                repair_firmware_json(raw_json)
                    .and_then(|repaired| serde_json::from_str(&repaired).ok())
                    .ok_or_else(|| DecodeError::Json {
                        subject: subject.to_owned(),
                        context,
                        source: original,
                    })?
            }
        };
        Ok(Self {
            subject: subject.to_owned(),
            value,
            raw_json: raw_json.to_owned(),
        })
    }

    /// Parse the first complete subject+JSON record in a string. PRO firmware
    /// 1.5.12 concatenates responses to batched commands without a line
    /// separator, so the JSON deserializer's byte offset is the only reliable
    /// boundary. Invalid single records still take the established repair path.
    fn parse_prefix(input: &str) -> Result<(Self, usize), DecodeError> {
        let (subject, raw_json) = input
            .split_once(':')
            .ok_or_else(|| DecodeError::MalformedRecord(input.to_owned()))?;
        let json_start = subject.len() + 1;
        let mut values = serde_json::Deserializer::from_str(raw_json).into_iter::<Value>();
        match values.next() {
            Some(Ok(value)) => {
                let json_len = values.byte_offset();
                let raw_json = &raw_json[..json_len];
                if subject.is_empty()
                    || subject
                        .chars()
                        .any(|character| matches!(character, '\0' | '\r' | '\n'))
                {
                    return Err(DecodeError::MalformedRecord(subject.to_owned()));
                }
                Ok((
                    Self {
                        subject: subject.to_owned(),
                        value,
                        raw_json: raw_json.to_owned(),
                    },
                    json_start + json_len,
                ))
            }
            _ => Self::parse(input).map(|record| (record, input.len())),
        }
    }

    pub fn new(subject: impl Into<String>, value: Value) -> Result<Self, DecodeError> {
        let subject = subject.into();
        if subject.is_empty()
            || subject
                .chars()
                .any(|c| matches!(c, '\0' | '\r' | '\n' | ':'))
        {
            return Err(DecodeError::MalformedRecord(subject));
        }
        let raw_json = serde_json::to_string(&value).map_err(|source| DecodeError::Json {
            subject: subject.clone(),
            context: "serializing record".into(),
            source,
        })?;
        Ok(Self {
            subject,
            value,
            raw_json,
        })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn set_value(&mut self, value: Value) -> Result<(), DecodeError> {
        self.raw_json = serde_json::to_string(&value).map_err(|source| DecodeError::Json {
            subject: self.subject.clone(),
            context: "serializing record".into(),
            source,
        })?;
        self.value = value;
        Ok(())
    }

    pub fn encode(&self) -> String {
        format!("{}:{}", self.subject, self.raw_json)
    }
}

fn json_error_context(raw: &str, line: usize, column: usize) -> String {
    let selected = raw.lines().nth(line.saturating_sub(1)).unwrap_or(raw);
    let at = column.saturating_sub(1);
    selected
        .chars()
        .skip(at.saturating_sub(20))
        .take(60)
        .collect::<String>()
        .escape_debug()
        .to_string()
}

fn repair_firmware_json(raw: &str) -> Option<String> {
    let missing_value = raw
        .replace("\"value\":,", "\"value\":null,")
        .replace("\"value\":}", "\"value\":null}");
    let repaired_paths = repair_bare_root_paths(&missing_value);
    if let Some(repaired) = repaired_paths {
        Some(repaired)
    } else if missing_value != raw {
        Some(missing_value)
    } else {
        None
    }
}

/// Firmware 1.5.12 emits assignment targets as `"root\app\amp"` rather than
/// valid JSON's `"root\\app\\amp"`. Repair only whole string values beginning
/// with the node-tree root, and only after strict JSON parsing has failed.
fn repair_bare_root_paths(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut repaired = Vec::with_capacity(raw.len() + 8);
    let mut cursor = 0;
    let mut changed = false;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"\"root\\") {
            repaired.extend_from_slice(b"\"root");
            cursor += 5;
            while cursor < bytes.len() && bytes[cursor] != b'"' {
                if bytes[cursor] == b'\\' {
                    repaired.extend_from_slice(b"\\\\");
                    changed = true;
                } else {
                    repaired.push(bytes[cursor]);
                }
                cursor += 1;
            }
        } else {
            repaired.push(bytes[cursor]);
            cursor += 1;
        }
    }
    changed.then(|| String::from_utf8(repaired).expect("repair preserves valid UTF-8"))
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.subject == other.subject && self.value == other.value
    }
}

/// A decoded NUL-delimited unit. Empty frames are valid and are ignored by a
/// client; the device emits one when a serial session starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    records: Vec<Record>,
}

impl Frame {
    pub fn parse(bytes: &[u8]) -> Result<Self, DecodeError> {
        let text = std::str::from_utf8(bytes)?;
        let mut records = Vec::new();
        for line in text
            // Firmware 1.5.12 answers ordinary requests with CRLF, but a
            // multi-command dread request separates its reply records with a
            // bare LF or no delimiter at all. Either line-ending byte is
            // unambiguous because raw control characters cannot occur inside
            // JSON strings; parse_prefix handles the no-delimiter case.
            .split(['\r', '\n'])
            .filter(|line| !line.is_empty())
        {
            let mut remaining = line;
            while !remaining.is_empty() {
                let (record, consumed) = Record::parse_prefix(remaining)?;
                if consumed == 0 || consumed > remaining.len() {
                    return Err(DecodeError::MalformedRecord(remaining.to_owned()));
                }
                records.push(record);
                remaining = &remaining[consumed..];
            }
        }
        Ok(Self { records })
    }

    pub fn new(records: Vec<Record>) -> Self {
        Self { records }
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn into_records(self) -> Vec<Record> {
        self.records
    }

    pub fn encode(&self) -> Vec<u8> {
        self.records
            .iter()
            .map(Record::encode)
            .collect::<Vec<_>>()
            .join("\r\n")
            .into_bytes()
    }
}

/// Incrementally separates NUL-terminated frames from a byte stream.
#[derive(Debug, Clone)]
pub struct Decoder {
    buffered: Vec<u8>,
    max_frame_len: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(4 * 1024 * 1024)
    }
}

impl Decoder {
    pub fn new(max_frame_len: usize) -> Self {
        Self {
            buffered: Vec::new(),
            max_frame_len,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
        self.buffered.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some(end) = self.buffered.iter().position(|byte| *byte == 0) {
            if end > self.max_frame_len {
                self.buffered.clear();
                return Err(DecodeError::FrameTooLarge {
                    length: end,
                    limit: self.max_frame_len,
                });
            }
            let rest = self.buffered.split_off(end + 1);
            let frame_bytes = std::mem::replace(&mut self.buffered, rest);
            let frame = Frame::parse(&frame_bytes[..end])?;
            frames.push(frame);
        }
        if self.buffered.len() > self.max_frame_len {
            let length = self.buffered.len();
            self.buffered.clear();
            return Err(DecodeError::FrameTooLarge {
                length,
                limit: self.max_frame_len,
            });
        }
        Ok(frames)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("VoidX frame was not UTF-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("malformed VoidX record {0:?}")]
    MalformedRecord(String),
    #[error("invalid JSON for VoidX subject {subject:?} near {context:?}: {source}")]
    Json {
        subject: String,
        context: String,
        source: serde_json::Error,
    },
    #[error("VoidX frame is {length} bytes, exceeding the {limit}-byte limit")]
    FrameTooLarge { length: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_handles_fragmentation_coalescing_and_empty_frames() {
        let mut decoder = Decoder::default();
        assert!(decoder.push(b"root\\name:{\"val").unwrap().is_empty());
        let frames = decoder
            .push(b"ue\":\"PRO\"}\0\0root\\version:{\"value\":\"1.5.12\"}\0")
            .unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].records()[0].subject(), "root\\name");
        assert!(frames[1].records().is_empty());
        assert_eq!(frames[2].records()[0].value()["value"], "1.5.12");
    }

    #[test]
    fn frames_accept_firmware_batch_reply_line_endings() {
        for delimiter in ["\r\n", "\n", "\r"] {
            let bytes = format!(
                "dread root\\presets:{{\"index\":0,\"chunk\":1}}{delimiter}dread root\\presets:{{\"index\":0,\"chunk\":2}}"
            );
            assert_eq!(Frame::parse(bytes.as_bytes()).unwrap().records().len(), 2);
        }
    }

    #[test]
    fn frames_accept_firmware_batch_replies_without_delimiters() {
        let bytes = b"dread root\\presets:{\"index\":0,\"chunk\":1,\"value\":\"00\"}dread root\\presets:{\"index\":0,\"chunk\":2,\"value\":\"01\"}";
        let frame = Frame::parse(bytes).unwrap();
        assert_eq!(frame.records().len(), 2);
        assert_eq!(frame.records()[0].value()["chunk"], 1);
        assert_eq!(frame.records()[1].value()["chunk"], 2);
    }

    #[test]
    fn record_reencoding_preserves_json_spelling() {
        let raw = br#"root\app\output\volume:{"value":-80.000000,"max":10.0}"#;
        let frame = Frame::parse(raw).unwrap();
        assert_eq!(frame.encode(), raw);
    }

    #[test]
    fn firmware_bare_assignment_paths_are_narrowly_repaired() {
        let raw = br#"root\assign1:{"desc":"assing1","value":"root\app\reverb","type":"item"}"#;
        let frame = Frame::parse(raw).unwrap();
        assert_eq!(frame.records()[0].value()["value"], "root\\app\\reverb");
        assert_eq!(frame.encode(), raw);
        assert!(Frame::parse(br#"root\x:{"value":"bad\escape"}"#).is_err());
    }

    #[test]
    fn firmware_empty_control_value_becomes_null() {
        let raw = br#"root\multi1:{"desc":"Control","value":,"type":"ctrl","src":""}"#;
        let frame = Frame::parse(raw).unwrap();
        assert!(frame.records()[0].value()["value"].is_null());
        assert_eq!(frame.encode(), raw);
    }

    #[test]
    fn decoder_enforces_a_bound_without_a_terminator() {
        let mut decoder = Decoder::new(3);
        assert!(matches!(
            decoder.push(b"abcd"),
            Err(DecodeError::FrameTooLarge { .. })
        ));
    }
}
