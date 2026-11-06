//! Exact constant-tempo subdivision positions, prepared on the control thread.
use crate::SamplePosition;

#[derive(Clone, Copy, Debug)]
pub struct MusicalGrid {
    numerator: u128,
    denominator: u128,
}

impl MusicalGrid {
    /// Creates a whole-note subdivision (4 = quarter, 16 = sixteenth).
    /// Returns None for a zero sample rate or a non-power-of-two subdivision.
    /// Tempo normalization matches `MusicalPosition::to_sample`. Sub-sample grids
    /// use a one-sample minimum so quantization never loses integer positions.
    #[must_use]
    pub fn new(sample_rate: u32, tempo_bpm: f64, subdivision: u8) -> Option<Self> {
        if sample_rate == 0 || !subdivision.is_power_of_two() {
            return None;
        }
        let tempo = if tempo_bpm.is_finite() {
            tempo_bpm.clamp(20.0, 400.0)
        } else {
            120.0
        };
        let bits = tempo.to_bits();
        let significand = u128::from((bits & ((1_u64 << 52) - 1)) | (1_u64 << 52));
        let exponent = u32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
        let numerator = (u128::from(sample_rate) * 240) << (1075 - exponent);
        let denominator = u128::from(subdivision) * significand;
        Some(if numerator < denominator {
            Self {
                numerator: 1,
                denominator: 1,
            }
        } else {
            Self {
                numerator,
                denominator,
            }
        })
    }

    /// Snaps to the nearest musical subdivision, then rounds that absolute grid
    /// position to samples. Both ties go forward; final positions saturate.
    /// No repeated rounded step is added, so fractional periods cannot drift.
    #[must_use]
    pub fn snap(self, sample: SamplePosition) -> SamplePosition {
        // numerator < 2^88, denominator < 2^60. sample * denominator
        // and index * numerator are below 2^125, including both rounding terms.
        let index = (u128::from(sample.0) * self.denominator + self.numerator / 2) / self.numerator;
        let position = (index * self.numerator + self.denominator / 2) / self.denominator;
        SamplePosition(u64::try_from(position).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_periods_do_not_accumulate_rounded_step_error() {
        let grid = MusicalGrid::new(44_100, 120.0, 16).unwrap();
        assert_eq!(grid.snap(SamplePosition(5_512)), SamplePosition(5_513));
        assert_eq!(grid.snap(SamplePosition(88_200)), SamplePosition(88_200));
        assert_eq!(grid.snap(SamplePosition(2_756)), SamplePosition(0));
        assert_eq!(grid.snap(SamplePosition(2_757)), SamplePosition(5_513));
        assert_eq!(
            MusicalGrid::new(48_000, 120.0, 16)
                .unwrap()
                .snap(SamplePosition(3_000)),
            SamplePosition(6_000)
        );
    }

    #[test]
    fn subdivisions_match_an_independent_distance_oracle_and_are_idempotent() {
        // BPM is bpm_eighths/8. The oracle compares distances to the two
        // surrounding exact grid points; it does not decode floats.
        for bpm_eighths in [160_u32, 987, 960, 1020, 3199, 3200] {
            for rate in [1, 44_100, 48_000, 192_000, u32::MAX] {
                for subdivision in [1_u8, 4, 8, 16, 32, 128] {
                    let grid =
                        MusicalGrid::new(rate, f64::from(bpm_eighths) / 8.0, subdivision).unwrap();
                    let n = u128::from(rate) * 240 * 8;
                    let d = u128::from(subdivision) * u128::from(bpm_eighths);
                    let mut seed = 17_u64;
                    for _ in 0..256 {
                        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        let p = u128::from(seed) * d;
                        let lower = p / n;
                        let index = lower + u128::from(p - lower * n >= (lower + 1) * n - p);
                        let expected = if n < d {
                            seed
                        } else {
                            u64::try_from((index * n + d / 2) / d).unwrap_or(u64::MAX)
                        };
                        let snapped = grid.snap(SamplePosition(seed));
                        assert_eq!(snapped.0, expected);
                        assert_eq!(grid.snap(snapped), snapped);
                    }
                }
            }
        }
    }

    #[test]
    fn normalization_minimum_step_limits_and_invalid_grids_are_explicit() {
        assert!(MusicalGrid::new(0, 120.0, 16).is_none());
        for invalid in [0, 3, 255] {
            assert!(MusicalGrid::new(48_000, 120.0, invalid).is_none());
        }
        for tempo in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                MusicalGrid::new(44_100, tempo, 16)
                    .unwrap()
                    .snap(SamplePosition(88_200)),
                SamplePosition(88_200)
            );
        }
        let minimum = MusicalGrid::new(1, 400.0, 128).unwrap();
        for sample in [0, 1, u64::MAX - 1, u64::MAX] {
            assert_eq!(minimum.snap(SamplePosition(sample)).0, sample);
        }
        for tempo in [20.0, 123.456, 399.888_919_744_515_4, 400.0] {
            let grid = MusicalGrid::new(u32::MAX, tempo, 1).unwrap();
            let a = grid.snap(SamplePosition(u64::MAX - 1));
            let b = grid.snap(SamplePosition(u64::MAX));
            assert!(a <= b);
            assert_eq!(grid.snap(b), b);
        }
    }
}
