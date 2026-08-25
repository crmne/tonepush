//! Preparing audio for the HX impulse-response store.
//!
//! The wire format has no sample-rate field: samples sent to the device are
//! already expected to be 48 kHz mono, with at most 2048 of them. HX Edit does
//! this conversion before uploading; sending a 96 kHz WAV verbatim makes the
//! cabinet twice as long and shifts its response down an octave.

use crate::{Error, Result};

pub const SAMPLE_RATE: u32 = 48_000;
pub const MAX_SAMPLES: usize = 2048;

/// Resample mono audio to the format the pedal stores, truncating only after
/// rate conversion when its duration exceeds the device's 2048-sample window.
pub fn prepare(samples: &[f32], source_rate: u32) -> Result<Vec<f32>> {
    if source_rate == 0 {
        return Err(Error::Protocol(
            "an impulse response has a zero sample rate".into(),
        ));
    }
    if samples.is_empty() {
        return Err(Error::Protocol("an empty impulse response".into()));
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err(Error::Protocol(
            "an impulse response contains a non-finite sample".into(),
        ));
    }
    if source_rate == SAMPLE_RATE {
        return Ok(samples[..samples.len().min(MAX_SAMPLES)].to_vec());
    }

    let wanted = ((samples.len() as u128) * u128::from(SAMPLE_RATE))
        .div_ceil(u128::from(source_rate))
        .min(MAX_SAMPLES as u128) as usize;
    let ratio = f64::from(source_rate) / f64::from(SAMPLE_RATE);
    let cutoff = (f64::from(SAMPLE_RATE) / f64::from(source_rate)).min(1.0);

    // A Lanczos-windowed sinc removes frequencies above the new Nyquist limit
    // when downsampling. Thirty-two source samples on either side is ample for
    // a 43 ms IR and still only about 130,000 multiplies at the maximum size.
    const RADIUS: usize = 32;
    let mut out = Vec::with_capacity(wanted);
    for output in 0..wanted {
        let source = output as f64 * ratio;
        let centre = source.floor() as usize;
        let first = centre.saturating_sub(RADIUS - 1);
        let last = centre
            .saturating_add(RADIUS)
            .min(samples.len().saturating_sub(1));
        let mut value = 0.0f64;
        for (index, sample) in samples.iter().enumerate().take(last + 1).skip(first) {
            let distance = index as f64 - source;
            let kernel = cutoff * sinc(cutoff * distance) * sinc(distance / RADIUS as f64);
            value += f64::from(*sample) * kernel;
        }
        out.push(value as f32);
    }
    Ok(out)
}

fn sinc(value: f64) -> f64 {
    if value.abs() < f64::EPSILON {
        1.0
    } else {
        let angle = std::f64::consts::PI * value;
        angle.sin() / angle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_tracks_duration_and_the_device_limit() {
        assert_eq!(prepare(&vec![0.0; 441], 44_100).unwrap().len(), 480);
        assert_eq!(prepare(&vec![0.0; 4096], 96_000).unwrap().len(), 2048);
        assert_eq!(prepare(&vec![0.0; 3000], 48_000).unwrap().len(), 2048);
    }

    #[test]
    fn downsampling_filters_content_above_the_new_nyquist_limit() {
        let alternating: Vec<f32> = (0..4096)
            .map(|index| if index % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let down = prepare(&alternating, 96_000).unwrap();
        assert!(
            down[32..down.len() - 32]
                .iter()
                .all(|sample| sample.abs() < 0.02),
            "96 kHz Nyquist content must not alias into the stored IR"
        );
    }

    #[test]
    fn invalid_audio_never_reaches_the_uploader() {
        assert!(prepare(&[1.0], 0).is_err());
        assert!(prepare(&[], 48_000).is_err());
        assert!(prepare(&[f32::NAN], 48_000).is_err());
    }
}
