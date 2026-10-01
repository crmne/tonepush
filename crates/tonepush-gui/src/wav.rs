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
    let declared = u32::from_le_bytes(bytes[4..8].try_into().unwrap());

    let mut format = None;
    let mut samples = None;
    let mut pos = 12;
    let mut missing_pad = false;

    while pos < bytes.len() {
        if bytes.len() - pos < 8 {
            return Err(Error::Protocol(
                "WAV ends partway through a chunk header".into(),
            ));
        }
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
            b"fmt " => {
                if format.is_some() {
                    return Err(Error::Protocol("WAV has more than one fmt chunk".into()));
                }
                if body.len() < 16 {
                    return Err(Error::Protocol("WAV fmt chunk is truncated".into()));
                }
                format = Some(Format::parse(body));
            }
            b"data" => {
                if samples.is_some() {
                    return Err(Error::Protocol("WAV has more than one data chunk".into()));
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
        return Err(Error::Protocol(format!(
            "WAV declares {declared} bytes after its RIFF header, but {present} are present"
        )));
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
    if samples.is_empty()
        || samples.len() > 2048
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err(Error::Protocol(
            "a device IR WAV needs 1 to 2048 finite samples".into(),
        ));
    }
    let byte_rate = sample_rate
        .checked_mul(4)
        .filter(|_| sample_rate > 0)
        .ok_or_else(|| Error::Protocol("the WAV sample rate is out of range".into()))?;
    let data = u32::try_from(samples.len() * 4)
        .map_err(|_| Error::Protocol("the WAV data is too large".into()))?;

    let mut out = Vec::with_capacity(HEADER as usize + 8 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(HEADER + data).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    out.extend_from_slice(&3u16.to_le_bytes()); // 3 is IEEE float
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes()); // bytes per second
    out.extend_from_slice(&4u16.to_le_bytes()); // bytes per frame
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per sample

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }

    crate::library::atomic_write(path, out)
        .map_err(|e| Error::Protocol(format!("writing {path:?}: {e}")))
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
    fn invalid_device_ir_wavs_are_not_written() {
        let path = write_temp(b"old", "invalid-output.wav");
        assert!(write(&path, &[], 48_000).is_err());
        assert!(write(&path, &[f32::NAN], 48_000).is_err());
        assert!(write(&path, &[0.0], 0).is_err());
        assert!(write(&path, &[0.0], u32::MAX).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
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
        partial.push(0); // valid word padding after the one declared data byte
        let riff_len = (partial.len() - 8) as u32;
        partial[4..8].copy_from_slice(&riff_len.to_le_bytes());
        partial[40..44].copy_from_slice(&1u32.to_le_bytes());
        let path = write_temp(&partial, "tonepush-gui-partial-sample.wav");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("partway"), "{error}");

        let mut trailing = wav_16bit(&[1]);
        trailing.push(0);
        let riff_len = (trailing.len() - 8) as u32;
        trailing[4..8].copy_from_slice(&riff_len.to_le_bytes());
        let path = write_temp(&trailing, "tonepush-gui-partial-header.wav");
        assert!(read(&path).unwrap_err().to_string().contains("header"));

        let mut wrong_riff_size = wav_16bit(&[1]);
        wrong_riff_size[4..8].copy_from_slice(&12u32.to_le_bytes());
        let path = write_temp(&wrong_riff_size, "tonepush-gui-wrong-riff-size.wav");
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
            "tonepush-gui-extensible.wav",
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
        let path = write_temp(&unpadded, "tonepush-gui-unpadded.wav");
        assert!((read(&path).unwrap().samples[0] - 0.5).abs() < 1e-6);

        let mut counted = unpadded.clone();
        let riff_len = (counted.len() - 7) as u32;
        counted[4..8].copy_from_slice(&riff_len.to_le_bytes());
        let path = write_temp(&counted, "tonepush-gui-unpadded-counted.wav");
        assert_eq!(read(&path).unwrap().samples.len(), 1);

        let mut overcounted = unpadded;
        let riff_len = (overcounted.len() - 6) as u32;
        overcounted[4..8].copy_from_slice(&riff_len.to_le_bytes());
        let path = write_temp(&overcounted, "tonepush-gui-unpadded-overcounted.wav");
        assert!(read(&path).unwrap_err().to_string().contains("declares"));
    }
}
