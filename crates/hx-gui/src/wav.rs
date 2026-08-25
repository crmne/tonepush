//! Just enough WAV to read and write an impulse response.
//!
//! IRs are short mono files, and the device wants plain `f32` samples. A full
//! audio library would bring a dependency tree for one job: find the `fmt ` and
//! `data` chunks and convert. Anything exotic is rejected with a message that
//! says what to convert it to.

use hx_usb::{Error, Result};
use std::path::Path;

#[derive(Debug)]
pub struct Wav {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

pub fn read(path: &Path) -> Result<Wav> {
    let bytes =
        std::fs::read(path).map_err(|e| Error::Protocol(format!("reading {path:?}: {e}")))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Protocol(format!("{path:?} is not a WAV file")));
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
            .ok_or_else(|| Error::Protocol("WAV chunk length overflow".into()))?;
        if body_end > bytes.len() {
            return Err(Error::Protocol(format!(
                "WAV {} chunk is truncated: it declares {len} bytes but only {} remain",
                String::from_utf8_lossy(id),
                bytes.len() - body_start
            )));
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
            .ok_or_else(|| Error::Protocol("WAV chunk padding overflow".into()))?;
    }

    let format = format.ok_or_else(|| Error::Protocol("WAV has no fmt chunk".into()))?;
    let data = samples.ok_or_else(|| Error::Protocol("WAV has no data chunk".into()))?;

    if format.channels != 1 {
        return Err(Error::Protocol(format!(
            "impulse responses must be mono; this file has {} channels",
            format.channels
        )));
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
            (tag, bits) => {
                return Err(Error::Protocol(format!(
                    "unsupported WAV format (tag {tag}, {bits}-bit); \
                     convert to 16-, 24-, or 32-bit mono PCM, or 32-bit float"
                )))
            }
        };
        if !data.len().is_multiple_of(sample_bytes) {
            return Err(Error::Protocol(format!(
                "WAV data chunk ends partway through a {}-bit sample",
                self.bits
            )));
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

/// Write samples out as a mono 32-bit float WAV.
///
/// What comes off the device is 48 kHz mono `f32` and nothing else, so this
/// writes exactly that: a canonical 44-byte header and the samples. The point
/// is that an IR rescued from a pedal lands as a file any editor will open,
/// rather than as a blob only this program understands.
pub fn write(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    const HEADER: u32 = 36;
    let data = (samples.len() * 4) as u32;

    let mut out = Vec::with_capacity(HEADER as usize + 8 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(HEADER + data).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    out.extend_from_slice(&3u16.to_le_bytes()); // 3 is IEEE float
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 4).to_le_bytes()); // bytes per second
    out.extend_from_slice(&4u16.to_le_bytes()); // bytes per frame
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per sample

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }

    std::fs::write(path, out).map_err(|e| Error::Protocol(format!("writing {path:?}: {e}")))
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
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
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
        std::fs::write(&path, bytes).expect("writes fixture");
        path
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let path = std::env::temp_dir().join("tonepush-wav-roundtrip.wav");
        let samples: Vec<f32> = (0..2048).map(|i| i as f32 / 4096.0).collect();
        write(&path, &samples, 48_000).expect("writes");

        let back = read(&path).expect("reads");
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.samples, samples);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_chunks_are_rejected_instead_of_silently_shortened() {
        let mut truncated = wav_16bit(&[1]);
        truncated[40..44].copy_from_slice(&4u32.to_le_bytes());
        let path = write_temp(&truncated, "tonepush-gui-truncated-chunk.wav");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("truncated"), "{error}");

        let mut partial = wav_16bit(&[1]);
        partial.pop();
        let riff_len = (partial.len() - 8) as u32;
        partial[4..8].copy_from_slice(&riff_len.to_le_bytes());
        partial[40..44].copy_from_slice(&1u32.to_le_bytes());
        let path = write_temp(&partial, "tonepush-gui-partial-sample.wav");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("partway"), "{error}");
    }
}
