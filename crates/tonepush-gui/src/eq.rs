//! What the device's global EQ actually does to a signal, as a curve.
//!
//! The pedal hands back eleven numbers - two cut frequencies and three bands of
//! frequency, Q and gain - and a list of eleven sliders tells you nothing about
//! the sound. The shape does. So the numbers are run through the textbook
//! analog prototypes for the filters they describe and drawn as one response.
//!
//! Analog prototypes rather than sampled biquads on purpose: the curve is for
//! looking at, and a digital response would need the device's sample rate and
//! would bend near the top of the band in a way that says more about the
//! arithmetic than about the pedal.

/// The band this program can see, and the range the curve is drawn over.
pub const MIN_HZ: f32 = 20.0;
pub const MAX_HZ: f32 = 20_000.0;

/// A colour per band, so a handle on the curve and the numbers underneath it
/// are plainly the same control.
///
/// They run across the spectrum the way the bands do - warm at the bottom,
/// green through the middle, cool at the top - because that ordering is one
/// fewer thing to learn. The two cuts share a neutral: they take away rather
/// than shape, and they sit on the unity line rather than in the field.
pub const LOW_CUT_COLOUR: (u8, u8, u8) = (0x8c, 0x93, 0xa1);
pub const LOW_COLOUR: (u8, u8, u8) = (0xe0, 0x60, 0x3c);
pub const MID_COLOUR: (u8, u8, u8) = (0x5c, 0xc4, 0x6a);
pub const HIGH_COLOUR: (u8, u8, u8) = (0x4a, 0x9f, 0xe0);
pub const HIGH_CUT_COLOUR: (u8, u8, u8) = (0xa8, 0x7c, 0xd8);

/// The device's own "not filtering": Low Cut sits *below* the band and High Cut
/// just above it, rather than either carrying an off switch.
const LOW_CUT_OFF: f32 = 20.0;
const HIGH_CUT_OFF: f32 = 20_000.0;

/// How steep the two cuts are taken to be. Second-order Butterworth - 12 dB per
/// octave, no peak at the corner - which is what the pedal's own display looks
/// like and what these filters almost always are.
const CUT_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// One peaking band: where it sits, how wide it is, how much it lifts or cuts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub freq: f32,
    pub q: f32,
    pub gain_db: f32,
}

/// The whole global EQ, as the eleven numbers the device holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub low_cut: f32,
    pub low: Band,
    pub mid: Band,
    pub high: Band,
    pub high_cut: f32,
}

impl Curve {
    /// The response at one frequency, in dB: every stage multiplied together,
    /// which on a decibel scale is every stage added.
    pub fn gain_db(&self, hz: f32) -> f32 {
        let mut db = 0.0;
        // A cut parked at the edge of the band is the device saying "off", and
        // running it anyway would bend the curve where nothing is happening.
        if self.low_cut > LOW_CUT_OFF {
            db += highpass_db(hz, self.low_cut);
        }
        if self.high_cut < HIGH_CUT_OFF {
            db += lowpass_db(hz, self.high_cut);
        }
        for band in [self.low, self.mid, self.high] {
            db += peaking_db(hz, band);
        }
        db
    }

    /// The curve sampled evenly across the drawing, left to right.
    ///
    /// Evenly in *position*, which is evenly in log frequency - the only
    /// spacing that gives the bottom two octaves as much room as the top two.
    pub fn sampled(&self, points: usize) -> Vec<(f32, f32)> {
        (0..points)
            .map(|i| {
                let t = i as f32 / (points - 1).max(1) as f32;
                let hz = from_position(t);
                (t, self.gain_db(hz))
            })
            .collect()
    }
}

/// Where a frequency sits across the drawing, 0 at 20 Hz and 1 at 20 kHz.
pub fn position(hz: f32) -> f32 {
    let lo = MIN_HZ.log10();
    let hi = MAX_HZ.log10();
    ((hz.max(MIN_HZ).log10() - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// And back again: the frequency at a position across the drawing.
pub fn from_position(t: f32) -> f32 {
    let lo = MIN_HZ.log10();
    let hi = MAX_HZ.log10();
    10f32.powf(lo + t.clamp(0.0, 1.0) * (hi - lo))
}

/// A peaking band's contribution at one frequency.
///
/// The analog prototype, whose squared magnitude at `x = ω/ω₀` is
/// `((1-x²)² + (A·x/Q)²) / ((1-x²)² + (x/(A·Q))²)` with `A = 10^(gain/40)`.
fn peaking_db(hz: f32, band: Band) -> f32 {
    // A band at unity gain is not a filter, and dividing by its `A` is a slow
    // way of finding that out.
    if band.gain_db == 0.0 || band.freq <= 0.0 || band.q <= 0.0 {
        return 0.0;
    }
    let a = 10f32.powf(band.gain_db / 40.0);
    let x = hz / band.freq;
    let common = (1.0 - x * x).powi(2);
    let numerator = common + (a * x / band.q).powi(2);
    let denominator = common + (x / (a * band.q)).powi(2);
    10.0 * (numerator / denominator).log10()
}

/// A second-order high-pass at `cut`: `|H|² = x⁴ / ((1-x²)² + (x/Q)²)`.
fn highpass_db(hz: f32, cut: f32) -> f32 {
    if cut <= 0.0 {
        return 0.0;
    }
    let x = hz / cut;
    let squared = x.powi(4) / ((1.0 - x * x).powi(2) + (x / CUT_Q).powi(2));
    10.0 * squared.max(f32::MIN_POSITIVE).log10()
}

/// A second-order low-pass at `cut`: `|H|² = 1 / ((1-x²)² + (x/Q)²)`.
fn lowpass_db(hz: f32, cut: f32) -> f32 {
    if cut <= 0.0 {
        return 0.0;
    }
    let x = hz / cut;
    let squared = 1.0 / ((1.0 - x * x).powi(2) + (x / CUT_Q).powi(2));
    10.0 * squared.max(f32::MIN_POSITIVE).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> Curve {
        Curve {
            low_cut: LOW_CUT_OFF,
            low: Band {
                freq: 100.0,
                q: 0.7,
                gain_db: 0.0,
            },
            mid: Band {
                freq: 1000.0,
                q: 0.7,
                gain_db: 0.0,
            },
            high: Band {
                freq: 5000.0,
                q: 0.7,
                gain_db: 0.0,
            },
            high_cut: HIGH_CUT_OFF,
        }
    }

    /// Nothing turned up and nothing cut is a flat line, all the way across.
    /// If this drifts, every other reading of the curve is off by that drift.
    #[test]
    fn an_untouched_eq_is_flat() {
        let curve = flat();
        for (_, db) in curve.sampled(64) {
            assert!(db.abs() < 1e-4, "flat curve wandered to {db} dB");
        }
    }

    /// A peaking band puts its gain exactly at its centre frequency. This is
    /// the one point on the curve a person will check against the number.
    #[test]
    fn a_band_gives_its_full_gain_at_its_centre() {
        let mut curve = flat();
        curve.mid = Band {
            freq: 1000.0,
            q: 1.0,
            gain_db: 6.0,
        };
        assert!((curve.gain_db(1000.0) - 6.0).abs() < 0.01);

        curve.mid.gain_db = -9.0;
        assert!((curve.gain_db(1000.0) + 9.0).abs() < 0.01);
    }

    /// And leaves the far ends of the band alone.
    #[test]
    fn a_band_is_local_to_itself() {
        let mut curve = flat();
        curve.mid = Band {
            freq: 1000.0,
            q: 2.0,
            gain_db: 12.0,
        };
        assert!(curve.gain_db(20.0).abs() < 0.2, "{}", curve.gain_db(20.0));
        assert!(curve.gain_db(20000.0).abs() < 0.2);
    }

    /// A higher Q is a narrower band: same lift at the centre, less of it an
    /// octave away.
    #[test]
    fn a_higher_q_is_a_narrower_band() {
        let wide = Curve {
            mid: Band {
                freq: 1000.0,
                q: 0.5,
                gain_db: 12.0,
            },
            ..flat()
        };
        let narrow = Curve {
            mid: Band {
                freq: 1000.0,
                q: 6.0,
                gain_db: 12.0,
            },
            ..flat()
        };
        assert!((wide.gain_db(1000.0) - narrow.gain_db(1000.0)).abs() < 0.01);
        assert!(wide.gain_db(2000.0) > narrow.gain_db(2000.0) + 3.0);
    }

    /// A cut is 3 dB down at its corner and falls away below it. 12 dB per
    /// octave means two octaves under the corner is roughly 24 dB down.
    #[test]
    fn a_low_cut_is_three_db_down_at_its_corner() {
        let curve = Curve {
            low_cut: 100.0,
            ..flat()
        };
        assert!(
            (curve.gain_db(100.0) + 3.0).abs() < 0.1,
            "{}",
            curve.gain_db(100.0)
        );
        let two_octaves_down = curve.gain_db(25.0);
        assert!(
            (-27.0..-21.0).contains(&two_octaves_down),
            "expected about -24 dB, got {two_octaves_down}"
        );
        // And leaves everything well above it alone.
        assert!(curve.gain_db(2000.0).abs() < 0.1);
    }

    #[test]
    fn a_high_cut_is_three_db_down_at_its_corner() {
        let curve = Curve {
            high_cut: 5000.0,
            ..flat()
        };
        assert!((curve.gain_db(5000.0) + 3.0).abs() < 0.1);
        assert!(curve.gain_db(200.0).abs() < 0.1);
        assert!(curve.gain_db(20000.0) < -20.0);
    }

    /// The device parks its cuts at the edge of the band to mean "off". Parked
    /// there they must not bend the curve at all - otherwise an EQ that is
    /// doing nothing draws as one that is rolling off.
    #[test]
    fn a_parked_cut_does_nothing() {
        let parked = Curve {
            low_cut: 19.9,
            high_cut: 20100.0,
            ..flat()
        };
        for (_, db) in parked.sampled(64) {
            assert!(db.abs() < 1e-4, "a parked cut bent the curve to {db} dB");
        }
    }

    /// Position and frequency are inverses, and the decade marks land where a
    /// person expects: 20 Hz at the left edge, 20 kHz at the right, 200 Hz and
    /// 2 kHz evenly spaced between.
    #[test]
    fn the_frequency_axis_is_logarithmic() {
        assert!((position(MIN_HZ) - 0.0).abs() < 1e-6);
        assert!((position(MAX_HZ) - 1.0).abs() < 1e-6);
        assert!((position(200.0) - 1.0 / 3.0).abs() < 1e-5);
        assert!((position(2000.0) - 2.0 / 3.0).abs() < 1e-5);
        for hz in [20.0, 63.0, 250.0, 1000.0, 8000.0, 20000.0] {
            assert!((from_position(position(hz)) - hz).abs() < hz * 1e-4);
        }
    }

    /// Bands add up rather than fighting: two lifts stacked at one frequency
    /// give more than either alone.
    #[test]
    fn the_bands_combine() {
        let curve = Curve {
            low: Band {
                freq: 1000.0,
                q: 1.0,
                gain_db: 6.0,
            },
            mid: Band {
                freq: 1000.0,
                q: 1.0,
                gain_db: 6.0,
            },
            ..flat()
        };
        assert!((curve.gain_db(1000.0) - 12.0).abs() < 0.01);
    }
}
