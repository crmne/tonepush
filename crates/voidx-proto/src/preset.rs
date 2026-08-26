use crate::{Frame, Record};

/// A StompStation preset document: ordered node records plus fixed-size NUL
/// padding. Records retain their original JSON text for byte-exact round trips.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    records: Vec<Record>,
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
        let frame = Frame::parse(&bytes[..content_len])?;
        if frame.records().is_empty() {
            return Err(PresetError::Empty);
        }
        for record in frame.records() {
            crate::NodePath::new(record.subject())?;
        }
        Ok(Self {
            records: frame.into_records(),
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
            original_len: 0,
        })
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn records_mut(&mut self) -> &mut [Record] {
        &mut self.records
    }

    pub fn content(&self) -> Vec<u8> {
        Frame::new(self.records.clone()).encode()
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

    #[test]
    fn rejects_hidden_data_after_padding() {
        assert!(matches!(
            Preset::parse(b"root\\x:{\"value\":1}\0bad"),
            Err(PresetError::NonZeroPadding)
        ));
    }
}
