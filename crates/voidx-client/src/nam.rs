//! Conversion between Neural Amp Modeler `.nam` JSON and the PRO's fixed-size
//! gzip slot representation.

use std::io::{Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::{Error, Result};

/// Validate a source `.nam`, pad its uncompressed representation to the slot
/// capacity, gzip it, then pad the compressed representation to that same
/// capacity. This matches the representation read from firmware 1.5.12.
pub fn encode(source: &[u8], capacity: usize) -> Result<Vec<u8>> {
    validate_document(source)?;
    if source.len() > capacity {
        return Err(Error::Format(format!(
            "NAM document is {} bytes; the device slot holds {capacity}",
            source.len()
        )));
    }
    let mut padded = Vec::with_capacity(capacity);
    padded.extend_from_slice(source);
    padded.resize(capacity, 0);

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&padded)
        .map_err(|error| Error::Format(format!("compressing NAM document: {error}")))?;
    let mut compressed = encoder
        .finish()
        .map_err(|error| Error::Format(format!("finishing NAM compression: {error}")))?;
    if compressed.len() > capacity {
        return Err(Error::Format(format!(
            "compressed NAM is {} bytes; the device slot holds {capacity}",
            compressed.len()
        )));
    }
    compressed.resize(capacity, 0);
    Ok(compressed)
}

/// Recover the ordinary `.nam` JSON from a fixed-size device blob. Both gzip's
/// checksum and the exact uncompressed capacity are verified.
pub fn decode(blob: &[u8], capacity: usize) -> Result<Vec<u8>> {
    if blob.len() != capacity {
        return Err(Error::Format(format!(
            "NAM device blob is {} bytes; expected {capacity}",
            blob.len()
        )));
    }
    let mut decoder = GzDecoder::new(blob);
    let mut padded = Vec::with_capacity(capacity + 1);
    let limit = u64::try_from(capacity)
        .ok()
        .and_then(|value| value.checked_add(1))
        .unwrap_or(u64::MAX);
    decoder
        .by_ref()
        .take(limit)
        .read_to_end(&mut padded)
        .map_err(|error| Error::Format(format!("decompressing NAM device blob: {error}")))?;
    if padded.len() != capacity {
        return Err(Error::Format(format!(
            "NAM blob expands to {} bytes; expected exactly {capacity}",
            padded.len()
        )));
    }
    let content = padded
        .iter()
        .rposition(|byte| *byte != 0)
        .map(|end| padded[..=end].to_vec())
        .unwrap_or_default();
    validate_document(&content)?;
    Ok(content)
}

fn validate_document(bytes: &[u8]) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| Error::Format(format!("NAM file is not valid JSON: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::Format("NAM document root is not an object".into()))?;
    let current_shape = object.contains_key("architecture")
        && object.contains_key("config")
        && object.contains_key("weights");
    if !object.contains_key("version") || !(object.contains_key("model") || current_shape) {
        return Err(Error::Format(
            "NAM document has no supported model/configuration fields".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_round_trips_through_fixed_gzip_slot() {
        let source = br#"{"version":"0.5.4","model":{"config":{},"weights":[1,2,3]}}"#;
        let encoded = encode(source, 1024).unwrap();
        assert_eq!(encoded.len(), 1024);
        assert_eq!(&encoded[..3], &[0x1f, 0x8b, 0x08]);
        assert_eq!(decode(&encoded, 1024).unwrap(), source);
    }

    #[test]
    fn malformed_or_oversized_models_are_rejected() {
        assert!(encode(b"not json", 128).is_err());
        assert!(encode(br#"{"version":"x"}"#, 128).is_err());
        let huge = format!(
            "{{\"version\":\"x\",\"model\":{{\"weights\":\"{}\"}}}}",
            "x".repeat(200)
        );
        assert!(encode(huge.as_bytes(), 128).is_err());
    }
}
