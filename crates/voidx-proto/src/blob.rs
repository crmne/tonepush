use std::fmt::Write;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HexError {
    #[error("hex data has odd length {0}")]
    OddLength(usize),
    #[error("invalid hex byte at offset {offset}: {pair:?}")]
    InvalidByte { offset: usize, pair: String },
}

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub fn decode_hex(value: &str) -> Result<Vec<u8>, HexError> {
    if !value.len().is_multiple_of(2) {
        return Err(HexError::OddLength(value.len()));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .enumerate()
        .map(|(index, pair)| {
            let high = hex_nibble(pair[0]);
            let low = hex_nibble(pair[1]);
            match (high, low) {
                (Some(high), Some(low)) => Ok(high << 4 | low),
                _ => Err(HexError::InvalidByte {
                    offset: index * 2,
                    pair: String::from_utf8_lossy(pair).into_owned(),
                }),
            }
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn chunk_count(size: usize, chunk_size: usize) -> Option<usize> {
    (chunk_size != 0).then(|| size.div_ceil(chunk_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_validation() {
        let value = [0, 1, 0x7f, 0x80, 0xff];
        assert_eq!(encode_hex(&value), "00017f80ff");
        assert_eq!(decode_hex("00017F80ff").unwrap(), value);
        assert!(matches!(decode_hex("abc"), Err(HexError::OddLength(3))));
        assert!(matches!(
            decode_hex("0z"),
            Err(HexError::InvalidByte { offset: 0, .. })
        ));
        assert!(matches!(
            decode_hex("é"),
            Err(HexError::InvalidByte { offset: 0, .. })
        ));
    }
}
