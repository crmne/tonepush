//! WAV conversion for the PRO's 48 kHz mono-f32 IR slots and paired stereo IRs.

use crate::{Error, Result};

pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    /// One vector per channel; all channels have the same frame count.
    pub channels: Vec<Vec<f32>>,
}

impl Wav {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(format_error("file is not a RIFF/WAVE document"));
        }
        let declared = read_u32(&bytes[4..8]) as usize;
        if declared.checked_add(8) != Some(bytes.len()) {
            return Err(format_error(format!(
                "RIFF declares {declared} bytes after its size field, but the file has {}",
                bytes.len().saturating_sub(8)
            )));
        }

        let mut format = None;
        let mut data = None;
        let mut cursor = 12;
        while cursor < bytes.len() {
            if bytes.len() - cursor < 8 {
                return Err(format_error("WAV ends partway through a chunk header"));
            }
            let id = &bytes[cursor..cursor + 4];
            let length = read_u32(&bytes[cursor + 4..cursor + 8]) as usize;
            let start = cursor + 8;
            let end = start
                .checked_add(length)
                .ok_or_else(|| format_error("WAV chunk length overflow"))?;
            if end > bytes.len() {
                return Err(format_error(format!(
                    "{} chunk is truncated",
                    String::from_utf8_lossy(id)
                )));
            }
            match id {
                b"fmt " if format.is_none() => format = Some(Format::parse(&bytes[start..end])?),
                b"fmt " => return Err(format_error("WAV has more than one fmt chunk")),
                b"data" if data.is_none() => data = Some(&bytes[start..end]),
                b"data" => return Err(format_error("WAV has more than one data chunk")),
                _ => {}
            }
            cursor = end
                .checked_add(length & 1)
                .ok_or_else(|| format_error("WAV padding overflow"))?;
            if cursor > bytes.len() {
                return Err(format_error("WAV chunk padding is truncated"));
            }
        }
        let format = format.ok_or_else(|| format_error("WAV has no fmt chunk"))?;
        let data = data.ok_or_else(|| format_error("WAV has no data chunk"))?;
        format.decode(data)
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map(Vec::len).unwrap_or(0)
    }

    /// Convert each channel to a fixed-size IR blob. Mono yields one blob;
    /// stereo yields left and right blobs suitable for the PRO's two IR blocks.
    pub fn device_blobs(&self, slot_size: usize) -> Result<Vec<Vec<u8>>> {
        if self.sample_rate != SAMPLE_RATE {
            return Err(format_error(format!(
                "PRO IRs must be {SAMPLE_RATE} Hz; this WAV is {} Hz",
                self.sample_rate
            )));
        }
        if !(1..=2).contains(&self.channels.len()) {
            return Err(format_error(format!(
                "PRO IR import supports mono or stereo; WAV has {} channels",
                self.channels.len()
            )));
        }
        if slot_size == 0 || !slot_size.is_multiple_of(4) {
            return Err(format_error(
                "IR slot size is not a whole number of f32 samples",
            ));
        }
        let capacity = slot_size / 4;
        if self.frames() == 0 || self.frames() > capacity {
            return Err(format_error(format!(
                "IR has {} frames; the device accepts 1 to {capacity}",
                self.frames()
            )));
        }
        if self.channels.iter().any(|channel| {
            channel.len() != self.frames() || channel.iter().any(|sample| !sample.is_finite())
        }) {
            return Err(format_error(
                "IR channels differ in length or contain a non-finite sample",
            ));
        }
        Ok(self
            .channels
            .iter()
            .map(|channel| {
                let mut blob = Vec::with_capacity(slot_size);
                for sample in channel {
                    blob.extend_from_slice(&sample.to_le_bytes());
                }
                blob.resize(slot_size, 0);
                blob
            })
            .collect())
    }

    /// Canonical 32-bit float WAV, preserving one or two channels.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if !(1..=2).contains(&self.channels.len()) || self.frames() == 0 {
            return Err(format_error(
                "WAV output needs one or two non-empty channels",
            ));
        }
        if self.sample_rate == 0
            || self.channels.iter().any(|channel| {
                channel.len() != self.frames() || channel.iter().any(|sample| !sample.is_finite())
            })
        {
            return Err(format_error("WAV output has invalid rate or sample data"));
        }
        let channels = u16::try_from(self.channels.len())
            .map_err(|_| format_error("channel count does not fit WAV"))?;
        let block = channels
            .checked_mul(4)
            .ok_or_else(|| format_error("WAV block size overflow"))?;
        let data_len = self
            .frames()
            .checked_mul(usize::from(block))
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| format_error("WAV data is too large"))?;
        let byte_rate = self
            .sample_rate
            .checked_mul(u32::from(block))
            .ok_or_else(|| format_error("WAV byte rate overflow"))?;

        let mut out = Vec::with_capacity(data_len as usize + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&3_u16.to_le_bytes()); // IEEE float
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&32_u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for frame in 0..self.frames() {
            for channel in &self.channels {
                out.extend_from_slice(&channel[frame].to_le_bytes());
            }
        }
        Ok(out)
    }

    pub fn from_device_blobs(blobs: &[&[u8]], sample_rate: u32) -> Result<Self> {
        if !(1..=2).contains(&blobs.len()) {
            return Err(format_error("one or two IR device blobs are required"));
        }
        let mut channels = Vec::with_capacity(blobs.len());
        for blob in blobs {
            if blob.is_empty() || !blob.len().is_multiple_of(4) {
                return Err(format_error("IR device blob is not whole f32 samples"));
            }
            let samples = blob
                .chunks_exact(4)
                .map(|word| f32::from_le_bytes(word.try_into().expect("four-byte chunk")))
                .collect::<Vec<_>>();
            if samples.iter().any(|sample| !sample.is_finite()) {
                return Err(format_error("IR device blob contains a non-finite sample"));
            }
            channels.push(samples);
        }
        if channels
            .windows(2)
            .any(|pair| pair[0].len() != pair[1].len())
        {
            return Err(format_error("IR device channels have different lengths"));
        }
        Ok(Self {
            sample_rate,
            channels,
        })
    }
}

struct Format {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits: u16,
}

impl Format {
    fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < 16 {
            return Err(format_error("WAV fmt chunk is truncated"));
        }
        let format = Self {
            tag: read_u16(&body[0..2]),
            channels: read_u16(&body[2..4]),
            sample_rate: read_u32(&body[4..8]),
            block_align: read_u16(&body[12..14]),
            bits: read_u16(&body[14..16]),
        };
        if !(1..=2).contains(&format.channels) {
            return Err(format_error(format!(
                "IR WAV must be mono or stereo; found {} channels",
                format.channels
            )));
        }
        Ok(format)
    }

    fn decode(&self, data: &[u8]) -> Result<Wav> {
        const PCM: u16 = 1;
        const FLOAT: u16 = 3;
        let sample_bytes = match (self.tag, self.bits) {
            (PCM, 16) => 2,
            (PCM, 24) => 3,
            (PCM | FLOAT, 32) => 4,
            _ => {
                return Err(format_error(format!(
                    "unsupported WAV format tag {} at {} bits",
                    self.tag, self.bits
                )))
            }
        };
        let expected_block = sample_bytes * usize::from(self.channels);
        if usize::from(self.block_align) != expected_block
            || !data.len().is_multiple_of(expected_block)
        {
            return Err(format_error(
                "WAV block alignment or data length is invalid",
            ));
        }
        let mut channels =
            vec![Vec::with_capacity(data.len() / expected_block); usize::from(self.channels)];
        for frame in data.chunks_exact(expected_block) {
            for (channel, output) in channels.iter_mut().enumerate() {
                let start = channel * sample_bytes;
                let bytes = &frame[start..start + sample_bytes];
                let sample = match (self.tag, self.bits) {
                    (PCM, 16) => i16::from_le_bytes(bytes.try_into().unwrap()) as f32 / 32768.0,
                    (PCM, 24) => {
                        let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                        value as f32 / 8_388_608.0
                    }
                    (PCM, 32) => {
                        i32::from_le_bytes(bytes.try_into().unwrap()) as f32 / 2_147_483_648.0
                    }
                    (FLOAT, 32) => f32::from_le_bytes(bytes.try_into().unwrap()),
                    _ => unreachable!(),
                };
                if !sample.is_finite() {
                    return Err(format_error("WAV contains a non-finite sample"));
                }
                output.push(sample);
            }
        }
        Ok(Wav {
            sample_rate: self.sample_rate,
            channels,
        })
    }
}

fn read_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().expect("two-byte slice"))
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("four-byte slice"))
}

fn format_error(detail: impl Into<String>) -> Error {
    Error::Format(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_float_wav_round_trips_and_splits_for_two_ir_blocks() {
        let wav = Wav {
            sample_rate: SAMPLE_RATE,
            channels: vec![vec![0.0, 0.5, -0.5], vec![1.0, -1.0, 0.25]],
        };
        let bytes = wav.encode().unwrap();
        assert_eq!(Wav::parse(&bytes).unwrap(), wav);
        let blobs = wav.device_blobs(32).unwrap();
        assert_eq!(blobs.len(), 2);
        assert_eq!(blobs[0].len(), 32);
        assert_eq!(f32::from_le_bytes(blobs[1][..4].try_into().unwrap()), 1.0);
    }

    #[test]
    fn wrong_rate_length_and_nonfinite_samples_are_rejected() {
        let mut wav = Wav {
            sample_rate: 44_100,
            channels: vec![vec![0.0]],
        };
        assert!(wav.device_blobs(8192).is_err());
        wav.sample_rate = SAMPLE_RATE;
        wav.channels[0] = vec![0.0; 2049];
        assert!(wav.device_blobs(8192).is_err());
        wav.channels[0] = vec![f32::NAN];
        assert!(wav.device_blobs(8192).is_err());
    }
}
