use crate::{Frame, Record};

/// A StompStation preset document: ordered node records plus fixed-size NUL
/// padding. Records retain their original JSON text, and the document its
/// line separator after the last record, for byte-exact round trips.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    records: Vec<Record>,
    /// The CR and LF bytes after the last record. Presets the pedal stores
    /// end in CRLF; dropping it made an exported preset, imported again,
    /// change the slot's bytes.
    trailer: String,
    original_len: usize,
}

impl Preset {
    pub fn parse(bytes: &[u8]) -> Result<Self, PresetError> {
        let content_len = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        if bytes[content_len..].iter().any(|byte| *byte != 0) {
            return Err(PresetError::NonZeroPadding);
        }
        let content = &bytes[..content_len];
        let frame = Frame::parse(content)?;
        if frame.records().is_empty() {
            return Err(PresetError::Empty);
        }
        for record in frame.records() {
            crate::NodePath::new(record.subject())?;
        }
        let body_len = content
            .iter()
            .rposition(|byte| !matches!(byte, b'\r' | b'\n'))
            .map_or(0, |last| last + 1);
        let trailer = String::from_utf8(content[body_len..].to_vec())
            .expect("CR and LF bytes are valid UTF-8");
        Ok(Self {
            records: frame.into_records(),
            trailer,
            original_len: bytes.len(),
        })
    }

    pub fn new(records: Vec<Record>) -> Result<Self, PresetError> {
        if records.is_empty() {
            return Err(PresetError::Empty);
        }
        for record in &records {
            crate::NodePath::new(record.subject())?;
        }
        Ok(Self {
            records,
            trailer: String::new(),
            original_len: 0,
        })
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn records_mut(&mut self) -> &mut [Record] {
        &mut self.records
    }

    /// The document without its fixed-capacity padding: the records and the
    /// separator the stored preset ended with, exactly as it was.
    pub fn content(&self) -> Vec<u8> {
        let mut content = Frame::new(self.records.clone()).encode();
        content.extend_from_slice(self.trailer.as_bytes());
        content
    }

    pub fn encode(&self) -> Result<Vec<u8>, PresetError> {
        let size = self.original_len.max(self.content().len());
        self.encode_padded(size)
    }

    pub fn encode_padded(&self, size: usize) -> Result<Vec<u8>, PresetError> {
        let mut content = self.content();
        if content.len() > size {
            return Err(PresetError::TooLarge {
                length: content.len(),
                limit: size,
            });
        }
        content.resize(size, 0);
        Ok(content)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("preset contains non-NUL data after its first NUL byte")]
    NonZeroPadding,
    #[error("preset has no node records")]
    Empty,
    #[error("preset content is {length} bytes and does not fit in {limit} bytes")]
    TooLarge { length: usize, limit: usize },
    #[error(transparent)]
    Decode(#[from] crate::DecodeError),
    #[error(transparent)]
    Path(#[from] crate::CommandError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_size_preset_round_trips_byte_for_byte() {
        let mut bytes =
            b"root\\app\\amp\\gain:{\"value\":3.000000}\r\nroot\\app\\amp\\on:{\"value\":1}"
                .to_vec();
        bytes.resize(128, 0);
        let preset = Preset::parse(&bytes).unwrap();
        assert_eq!(preset.encode().unwrap(), bytes);
    }

    /// Stored presets end in CRLF. Export keeps it, so exporting a slot and
    /// importing the file writes the slot's own bytes back.
    #[test]
    fn a_stored_presets_trailing_separator_survives_export_and_import() {
        let stored =
            b"root\\app\\amp\\gain:{\"value\":3.000000}\r\nroot\\app\\amp\\on:{\"value\":1}\r\n";
        let mut slot = stored.to_vec();
        slot.resize(256, 0);

        let exported = Preset::parse(&slot).unwrap().content();
        assert_eq!(exported, stored, "export removes only the padding");

        let imported = Preset::parse(&exported)
            .unwrap()
            .encode_padded(256)
            .unwrap();
        assert_eq!(imported, slot, "import writes the slot's bytes back");
    }

    #[test]
    fn a_preset_without_a_trailing_separator_gains_none() {
        let portable = b"root\\app\\amp\\gain:{\"value\":42.0}";
        let preset = Preset::parse(portable).unwrap();
        assert_eq!(preset.content(), portable);
        let mut slot = portable.to_vec();
        slot.resize(64, 0);
        assert_eq!(preset.encode_padded(64).unwrap(), slot);
        assert_eq!(Preset::parse(&slot).unwrap().content(), portable);
    }

    #[test]
    fn an_edited_preset_keeps_its_trailing_separator() {
        let stored = b"root\\app\\amp\\gain:{\"value\":3.0}\r\n";
        let mut preset = Preset::parse(stored).unwrap();
        preset.records_mut()[0]
            .set_value(serde_json::json!({"value": 4.0}))
            .unwrap();
        assert_eq!(
            preset.content(),
            b"root\\app\\amp\\gain:{\"value\":4.0}\r\n"
        );
    }

    #[test]
    fn rejects_hidden_data_after_padding() {
        assert!(matches!(
            Preset::parse(b"root\\x:{\"value\":1}\0bad"),
            Err(PresetError::NonZeroPadding)
        ));
    }
}
