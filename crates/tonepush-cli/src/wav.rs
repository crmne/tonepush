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
    let declared = u32::from_le_bytes(bytes[4..8].try_into().unwrap());

    let mut format = None;
    let mut samples = None;
    let mut pos = 12;
    let mut missing_pad = false;

    while pos < bytes.len() {
        if bytes.len() - pos < 8 {
            bail!("WAV ends partway through a chunk header");
        }
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
            b"fmt " => {
                if format.is_some() {
                    bail!("WAV has more than one fmt chunk");
                }
                if body.len() < 16 {
                    bail!("WAV fmt chunk is truncated");
                }
                format = Some(Format::parse(body));
            }
            b"data" => {
                if samples.is_some() {
                    bail!("WAV has more than one data chunk");
                }
                samples = Some(body.to_vec());
            }
            _ => {}
        }
        // Chunks are word-aligned, and an odd length is padded. Plenty of
        // writers leave the pad byte off the last chunk, where nothing follows
        // to be misaligned, so only there may it be missing.
        let padded_end = body_end + (len & 1);
        missing_pad = padded_end > bytes.len();
        pos = padded_end.min(bytes.len());
    }

    // The RIFF size counts what is present, or the pad byte a writer left off.
    let present = bytes.len() - 8;
    let counted = usize::try_from(declared).ok();
    if counted != Some(present) && !(missing_pad && counted == Some(present + 1)) {
        bail!("WAV declares {declared} bytes after its RIFF header, but {present} are present");
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
        const EXTENSIBLE: u16 = 0xFFFE;
        let mut tag = u16::from_le_bytes(body[0..2].try_into().unwrap());
        // WAVE_FORMAT_EXTENSIBLE, which most editors write for 24-bit audio,
        // names the real format in the first two bytes of its SubFormat GUID,
        // after the extension size, valid bits and channel mask.
        if tag == EXTENSIBLE && body.len() >= 26 {
            tag = u16::from_le_bytes(body[24..26].try_into().unwrap());
        }
        Format {
            tag,
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
        bytes.push(0); // valid word padding after the one declared data byte
        let riff_len = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());
        bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
        let path = write_temp(&bytes, "hx-test-partial-sample.wav");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("partway"), "{error}");
    }

    #[test]
    fn rejects_trailing_bytes_and_a_false_riff_size() {
        let mut trailing = wav_16bit(&[1]);
        trailing.push(0);
        let riff_len = (trailing.len() - 8) as u32;
        trailing[4..8].copy_from_slice(&riff_len.to_le_bytes());
        let path = write_temp(&trailing, "hx-test-partial-header.wav");
        assert!(read(&path).unwrap_err().to_string().contains("header"));

        let mut wrong_riff_size = wav_16bit(&[1]);
        wrong_riff_size[4..8].copy_from_slice(&12u32.to_le_bytes());
        let path = write_temp(&wrong_riff_size, "hx-test-wrong-riff-size.wav");
        assert!(read(&path).unwrap_err().to_string().contains("declares"));
    }

    /// A mono 24-bit PCM file the way most editors save one: in a
    /// WAVE_FORMAT_EXTENSIBLE fmt chunk.
    fn wav_24bit_extensible(samples: &[i32]) -> Vec<u8> {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| s.to_le_bytes()[..3].to_vec())
            .collect();
        let mut out = b"RIFF".to_vec();
        out.extend(((60 + data.len()) as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(40u32.to_le_bytes());
        out.extend(0xFFFEu16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(48_000u32.to_le_bytes());
        out.extend(144_000u32.to_le_bytes());
        out.extend(3u16.to_le_bytes());
        out.extend(24u16.to_le_bytes());
        out.extend(22u16.to_le_bytes()); // extension size
        out.extend(24u16.to_le_bytes()); // valid bits
        out.extend(4u32.to_le_bytes()); // front centre
        out.extend(1u16.to_le_bytes()); // KSDATAFORMAT_SUBTYPE_PCM
        out.extend([
            0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
        ]);
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn reads_24_bit_extensible_mono() {
        let path = write_temp(
            &wav_24bit_extensible(&[0, 4_194_304, -4_194_304, 8_388_607]),
            "hx-test-extensible.wav",
        );
        let wav = read(&path).unwrap();
        assert_eq!(wav.sample_rate, 48_000);
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[1] - 0.5).abs() < 1e-6);
        assert!((wav.samples[2] + 0.5).abs() < 1e-6);
    }

    /// Three bytes of one 24-bit sample make an odd data chunk, and the pad
    /// byte after it is often left off, with or without the RIFF size
    /// counting it.
    #[test]
    fn tolerates_a_missing_pad_byte_after_the_last_chunk() {
        let unpadded = wav_24bit_extensible(&[4_194_304]);
        let path = write_temp(&unpadded, "hx-test-unpadded.wav");
        assert!((read(&path).unwrap().samples[0] - 0.5).abs() < 1e-6);

        let mut counted = unpadded.clone();
        let riff_len = (counted.len() - 7) as u32;
        counted[4..8].copy_from_slice(&riff_len.to_le_bytes());
        let path = write_temp(&counted, "hx-test-unpadded-counted.wav");
        assert_eq!(read(&path).unwrap().samples.len(), 1);

        let mut overcounted = unpadded;
        let riff_len = (overcounted.len() - 6) as u32;
        overcounted[4..8].copy_from_slice(&riff_len.to_le_bytes());
        let path = write_temp(&overcounted, "hx-test-unpadded-overcounted.wav");
        assert!(read(&path).unwrap_err().to_string().contains("declares"));
    }
}
