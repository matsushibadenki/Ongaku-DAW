use super::*;

// Independent oracle for rational BPM: direct cross multiplication, without
// decoding float bits or using the production quotient/remainder algorithm.
fn oracle(
    p: MusicalPosition,
    rate: u32,
    signature: TimeSignature,
    bpm_num: u64,
    bpm_den: u64,
) -> u64 {
    let ticks = (u128::from(p.bar) * u128::from(signature.numerator) + u128::from(p.beat)) * 960
        + u128::from(p.tick);
    let n = ticks * u128::from(rate) * 240 * u128::from(bpm_den);
    let d = 960 * u128::from(signature.denominator) * u128::from(bpm_num);
    u64::try_from((n + d / 2) / d).unwrap_or(u64::MAX)
}

#[test]
fn integer_and_binary_fractional_tempos_match_independent_rational_oracle() {
    let positions = [
        MusicalPosition {
            bar: 0,
            beat: 0,
            tick: 0,
        },
        MusicalPosition {
            bar: 1,
            beat: 3,
            tick: 959,
        },
        MusicalPosition {
            bar: 1_188_693_771,
            beat: 4,
            tick: 294,
        },
        MusicalPosition {
            bar: u32::MAX,
            beat: u8::MAX,
            tick: u32::MAX,
        },
    ];
    for bpm_num in 160..=3200_u32 {
        for rate in [0, 1, 44_100, 48_000, 192_000, u32::MAX] {
            for denominator in [1, 2, 4, 8, 16, 32, 64, 128] {
                let signature = TimeSignature {
                    numerator: 255,
                    denominator,
                };
                for p in positions {
                    assert_eq!(
                        p.to_sample(rate, f64::from(bpm_num) / 8.0, signature).0,
                        oracle(p, rate, signature, u64::from(bpm_num), 8)
                    );
                }
            }
        }
    }
}

#[test]
fn exact_half_sample_and_adjacent_binary_tempos_round_in_the_correct_direction() {
    let p = MusicalPosition {
        bar: 0,
        beat: 1,
        tick: 0,
    };
    let signature = TimeSignature::default();
    assert_eq!(p.to_sample(1, 120.0, signature).0, 1);
    assert_eq!(
        p.to_sample(1, f64::from_bits(120_f64.to_bits() + 1), signature)
            .0,
        0
    );
    assert_eq!(
        p.to_sample(1, f64::from_bits(120_f64.to_bits() - 1), signature)
            .0,
        1
    );
}

#[test]
fn long_timeline_regression_keeps_the_last_sample_bit() {
    let p = MusicalPosition {
        bar: 1_188_693_771,
        beat: 4,
        tick: 294,
    };
    assert_eq!(
        p.to_sample(
            48_000,
            400.0,
            TimeSignature {
                numerator: 7,
                denominator: 8
            }
        )
        .0,
        29_955_083_044_703
    );
}

#[test]
fn invalid_inputs_preserve_normalization_and_extreme_positions_saturate() {
    let p = MusicalPosition {
        bar: 1,
        beat: 0,
        tick: 0,
    };
    for tempo in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            p.to_sample(
                48_000,
                tempo,
                TimeSignature {
                    numerator: 0,
                    denominator: 3
                }
            ),
            MusicalPosition {
                bar: 0,
                beat: 1,
                tick: 0
            }
            .to_sample(48_000, 120.0, TimeSignature::default())
        );
    }
    for (tempo, normalized) in [(-1.0, 20.0), (0.0, 20.0), (401.0, 400.0)] {
        assert_eq!(
            p.to_sample(48_000, tempo, TimeSignature::default()),
            p.to_sample(48_000, normalized, TimeSignature::default())
        );
    }
    let extreme = MusicalPosition {
        bar: u32::MAX,
        beat: u8::MAX,
        tick: u32::MAX,
    };
    assert_eq!(
        extreme
            .to_sample(
                u32::MAX,
                20.0,
                TimeSignature {
                    numerator: 255,
                    denominator: 1
                }
            )
            .0,
        u64::MAX
    );
    assert_eq!(extreme.to_sample(0, 20.0, TimeSignature::default()).0, 0);
}

#[test]
fn tick_carry_and_absolute_positions_do_not_accumulate_rounded_step_error() {
    let signature = TimeSignature {
        numerator: 7,
        denominator: 8,
    };
    for tempo in [20.0, 123.456, 127.5, 400.0] {
        let mut previous = SamplePosition::ZERO;
        for tick in 0..20_000 {
            let p = MusicalPosition {
                bar: 0,
                beat: 0,
                tick,
            };
            let carried = MusicalPosition {
                bar: tick / (7 * 960),
                beat: u8::try_from(tick / 960 % 7).unwrap(),
                tick: tick % 960,
            };
            let sample = p.to_sample(44_100, tempo, signature);
            assert_eq!(sample, carried.to_sample(44_100, tempo, signature));
            assert!(sample >= previous);
            previous = sample;
        }
    }
}

#[test]
fn non_binary_decimal_tempos_match_external_exact_fraction_fixtures() {
    // Generated independently with Python Fraction(float), preserving the exact
    // IEEE-754 input rather than treating its decimal spelling as an exact BPM.
    for (bar, tempo, expected) in [
        (1, 123.456, 131_877),
        (1_188_693_771, 123.456, 97_055_090_217_413),
        (u32::MAX, 20.000_000_000_000_004, 2_164_663_516_990_050),
    ] {
        assert_eq!(
            MusicalPosition {
                bar,
                beat: 4,
                tick: 294
            }
            .to_sample(
                48_000,
                tempo,
                TimeSignature {
                    numerator: 7,
                    denominator: 8
                }
            )
            .0,
            expected
        );
    }
}

#[test]
fn ranges_are_half_open_at_zero_and_integer_limits() {
    for start in [0, 1, u64::MAX - 2] {
        assert!(SampleRange::new(SamplePosition(start), SamplePosition(start)).is_none());
        let end = start + 1;
        let range = SampleRange::new(SamplePosition(start), SamplePosition(end)).unwrap();
        assert_eq!(range.length(), 1);
        assert!(range.contains(SamplePosition(start)));
        assert!(!range.contains(SamplePosition(end)));
        if start > 0 {
            assert!(!range.contains(SamplePosition(start - 1)));
        }
        assert!(SampleRange::new(SamplePosition(end), SamplePosition(start)).is_none());
    }
}
