//! Just enough WAV to read an impulse response.
//!
//! IRs are short mono files, and the device wants plain `f32` samples. A full
//! audio library would bring a dependency tree for one job: find the `fmt ` and
//! `data` chunks and convert. Anything exotic is rejected with a message that
//! says what to convert it to.

use anyhow::{bail, Context, Result};
use std::path::Path;

#[derive(Debug)]
pub struct Wav {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

pub fn read(path: &Path) -> Result<Wav> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("{path:?} is not a WAV file");
    }

    let mut format = None;
    let mut samples = None;
    let mut pos = 12;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(len)
            .context("WAV chunk length overflow")?;
        if body_end > bytes.len() {
            bail!(
                "WAV {} chunk is truncated: it declares {len} bytes but only {} remain",
                String::from_utf8_lossy(id),
                bytes.len() - body_start
            );
        }
        let body = &bytes[body_start..body_end];

        match id {
            b"fmt " if body.len() >= 16 => format = Some(Format::parse(body)),
            b"data" => samples = Some(body.to_vec()),
            _ => {}
        }
        // Chunks are word-aligned, and an odd length is padded.
        pos = body_end
            .checked_add(len & 1)
            .context("WAV chunk padding overflow")?;
    }

    let format = format.context("WAV has no fmt chunk")?;
    let data = samples.context("WAV has no data chunk")?;

    if format.channels != 1 {
        bail!(
            "impulse responses must be mono; this file has {} channels",
            format.channels
        );
    }

    Ok(Wav {
        sample_rate: format.sample_rate,
        samples: format.decode(&data)?,
    })
}

struct Format {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

impl Format {
    fn parse(body: &[u8]) -> Format {
        Format {
            tag: u16::from_le_bytes(body[0..2].try_into().unwrap()),
            channels: u16::from_le_bytes(body[2..4].try_into().unwrap()),
            sample_rate: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            bits: u16::from_le_bytes(body[14..16].try_into().unwrap()),
        }
    }

    /// Normalise to `f32` in -1.0..=1.0, whatever the file stored.
    fn decode(&self, data: &[u8]) -> Result<Vec<f32>> {
        const PCM: u16 = 1;
        const FLOAT: u16 = 3;

        let sample_bytes = match (self.tag, self.bits) {
            (PCM, 16) => 2,
            (PCM, 24) => 3,
            (PCM | FLOAT, 32) => 4,
            (tag, bits) => bail!(
                "unsupported WAV format (tag {tag}, {bits}-bit); \
                 convert to 16-, 24-, or 32-bit mono PCM, or 32-bit float"
            ),
        };
        if !data.len().is_multiple_of(sample_bytes) {
            bail!(
                "WAV data chunk ends partway through a {}-bit sample",
                self.bits
            );
        }

        Ok(match (self.tag, self.bits) {
            (PCM, 16) => data
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect(),
            (PCM, 24) => data
                .chunks_exact(3)
                .map(|b| {
                    let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                    v as f32 / 8_388_608.0
                })
                .collect(),
            (PCM, 32) => data
                .chunks_exact(4)
                .map(|b| i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2_147_483_648.0)
                .collect(),
            (FLOAT, 32) => data
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect(),
            _ => unreachable!("the format was validated above"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_16bit(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = b"RIFF".to_vec();
        out.extend(((36 + data.len()) as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes()); // PCM
        out.extend(1u16.to_le_bytes()); // mono
        out.extend(48_000u32.to_le_bytes());
        out.extend(96_000u32.to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    fn write_temp(bytes: &[u8], name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn reads_16_bit_mono() {
        let path = write_temp(&wav_16bit(&[0, 16384, -16384, 32767]), "hx-test-16.wav");
        let wav = read(&path).unwrap();
        assert_eq!(wav.sample_rate, 48_000);
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[1] - 0.5).abs() < 1e-4);
        assert!((wav.samples[2] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn rejects_stereo_with_a_useful_message() {
        let mut bytes = wav_16bit(&[0, 0]);
        bytes[22] = 2; // channels
        let path = write_temp(&bytes, "hx-test-stereo.wav");
        let err = read(&path).unwrap_err().to_string();
        assert!(err.contains("mono"), "{err}");
    }

    #[test]
    fn rejects_a_file_that_is_not_wav() {
        let path = write_temp(b"not a wav at all", "hx-test-bogus.wav");
        assert!(read(&path).is_err());
    }

    #[test]
    fn rejects_a_chunk_that_runs_past_the_file() {
        let mut bytes = wav_16bit(&[1]);
        bytes[40..44].copy_from_slice(&4u32.to_le_bytes());
        let path = write_temp(&bytes, "hx-test-truncated-chunk.wav");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("truncated"), "{error}");
    }

    #[test]
    fn rejects_a_data_chunk_that_ends_mid_sample() {
        let mut bytes = wav_16bit(&[1]);
        bytes.pop();
        let riff_len = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());
        bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
        let path = write_temp(&bytes, "hx-test-partial-sample.wav");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("partway"), "{error}");
    }
}
