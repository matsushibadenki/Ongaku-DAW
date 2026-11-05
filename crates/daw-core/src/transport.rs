use crate::{SamplePosition, SampleRange};
use serde::{Deserialize, Serialize};

pub const TICKS_PER_BEAT: u32 = 960;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicalPosition {
    pub bar: u32,
    pub beat: u8,
    pub tick: u32,
}

impl MusicalPosition {
    #[must_use]
    pub fn to_sample(
        self,
        sample_rate: u32,
        tempo_bpm: f64,
        signature: TimeSignature,
    ) -> SamplePosition {
        let tempo_bpm = if tempo_bpm.is_finite() {
            tempo_bpm.clamp(20.0, 400.0)
        } else {
            120.0
        };
        let numerator = signature.numerator.max(1);
        let denominator = if signature.denominator.is_power_of_two() {
            signature.denominator
        } else {
            4
        };
        let beats_per_bar = f64::from(numerator);
        let beat_units = f64::from(self.bar) * beats_per_bar
            + f64::from(self.beat)
            + f64::from(self.tick) / f64::from(TICKS_PER_BEAT);
        let quarter_note_scale = 4.0 / f64::from(denominator);
        let seconds = beat_units * quarter_note_scale * 60.0 / tempo_bpm;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        SamplePosition((seconds * f64::from(sample_rate)).round().max(0.0) as u64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransportCommand {
    Play,
    Stop,
    Seek(SamplePosition),
    SetLoop(Option<SampleRange>),
    SetTempo(f64),
    SetTimeSignature(TimeSignature),
    SetRecording(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportSnapshot {
    pub current_sample: SamplePosition,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    pub time_signature: TimeSignature,
    pub playing: bool,
    pub recording: bool,
    pub loop_range: Option<SampleRange>,
}

#[derive(Clone, Debug)]
pub struct Transport {
    snapshot: TransportSnapshot,
}

impl Transport {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            snapshot: TransportSnapshot {
                current_sample: SamplePosition::ZERO,
                sample_rate,
                tempo_bpm: 120.0,
                time_signature: TimeSignature::default(),
                playing: false,
                recording: false,
                loop_range: None,
            },
        }
    }

    pub fn apply(&mut self, command: TransportCommand) {
        match command {
            TransportCommand::Play => self.snapshot.playing = true,
            TransportCommand::Stop => {
                self.snapshot.playing = false;
                self.snapshot.recording = false;
            }
            TransportCommand::Seek(position) => self.snapshot.current_sample = self.wrap(position),
            TransportCommand::SetLoop(range) => {
                self.snapshot.loop_range = range.filter(|range| range.end > range.start);
                self.snapshot.current_sample = self.wrap(self.snapshot.current_sample);
            }
            TransportCommand::SetTempo(tempo) => {
                if tempo.is_finite() {
                    self.snapshot.tempo_bpm = tempo.clamp(20.0, 400.0);
                }
            }
            TransportCommand::SetTimeSignature(signature) => {
                if signature.numerator > 0 && signature.denominator.is_power_of_two() {
                    self.snapshot.time_signature = signature;
                }
            }
            TransportCommand::SetRecording(recording) => self.snapshot.recording = recording,
        }
    }

    #[must_use]
    pub const fn snapshot(&self) -> TransportSnapshot {
        self.snapshot
    }

    #[must_use]
    pub fn sample_at_offset(&self, frame_offset: usize) -> SamplePosition {
        let offset = u64::try_from(frame_offset).unwrap_or(u64::MAX);
        let current = self.snapshot.current_sample.0;
        if let Some(range) = self.snapshot.loop_range {
            // Subtract the distance to the loop end before addition, so a large offset
            // wraps correctly even when current + offset would exceed u64::MAX.
            let until_end = range.end.0 - current;
            if offset >= until_end {
                return SamplePosition(range.start.0 + (offset - until_end) % range.length());
            }
        }
        self.snapshot.current_sample.saturating_add(offset)
    }

    pub fn advance(&mut self, frames: usize) {
        if self.snapshot.playing {
            self.snapshot.current_sample = self.sample_at_offset(frames);
        }
    }

    fn wrap(&self, position: SamplePosition) -> SamplePosition {
        let Some(loop_range) = self.snapshot.loop_range else {
            return position;
        };
        if loop_range.end <= loop_range.start {
            return position;
        }
        if position.0 < loop_range.end.0 {
            return position;
        }
        SamplePosition(loop_range.start.0 + (position.0 - loop_range.start.0) % loop_range.length())
    }
}

#[cfg(test)]
mod tests {
    use super::{MusicalPosition, TimeSignature, Transport, TransportCommand};
    use crate::{SamplePosition, SampleRange};

    #[test]
    fn musical_time_uses_integer_sample_result() {
        let position = MusicalPosition {
            bar: 1,
            beat: 0,
            tick: 0,
        };
        assert_eq!(
            position.to_sample(48_000, 120.0, TimeSignature::default()),
            SamplePosition(96_000)
        );
    }

    #[test]
    fn transport_wraps_at_loop_boundary() {
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(100),
            SamplePosition(200),
        )));
        transport.apply(TransportCommand::Seek(SamplePosition(190)));
        transport.apply(TransportCommand::Play);
        transport.advance(20);
        assert_eq!(transport.snapshot().current_sample, SamplePosition(110));
    }

    #[test]
    fn commands_update_the_complete_transport_snapshot() {
        let mut transport = Transport::new(96_000);
        transport.apply(TransportCommand::Seek(SamplePosition(42)));
        transport.apply(TransportCommand::SetTempo(135.0));
        transport.apply(TransportCommand::SetTimeSignature(TimeSignature {
            numerator: 3,
            denominator: 4,
        }));
        transport.apply(TransportCommand::SetRecording(true));
        transport.apply(TransportCommand::Play);
        let snapshot = transport.snapshot();
        assert_eq!(snapshot.current_sample, SamplePosition(42));
        assert_eq!(snapshot.sample_rate, 96_000);
        assert!((snapshot.tempo_bpm - 135.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.time_signature.numerator, 3);
        assert!(snapshot.playing);
        assert!(snapshot.recording);
        transport.apply(TransportCommand::Stop);
        assert!(!transport.snapshot().playing);
        assert!(!transport.snapshot().recording);
    }
}

#[cfg(test)]
mod p0_regressions {
    use super::*;
    #[test]
    fn looping_offsets_do_not_saturate_before_modulo() {
        let mut transport = Transport::new(48000);
        let start = u64::MAX - 20;
        let end = u64::MAX - 3;
        transport.apply(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(start),
            SamplePosition(end),
        )));
        transport.apply(TransportCommand::Seek(SamplePosition(end - 2)));
        let expected = start + 98 % 17;
        assert_eq!(transport.sample_at_offset(100), SamplePosition(expected));
    }
    #[test]
    fn sample_positions_match_an_independent_u128_loop_oracle_for_every_partition() {
        for start in [5_u64, u64::MAX - 2000] {
            for length in [1_u64, 2, 17, 130] {
                let end = start + length;
                for initial in [start - 3, start, end - 1] {
                    for partitions in [&[1][..], &[17, 64, 127, 128, 257, 512, 1024][..]] {
                        let expected = |elapsed: usize| {
                            let total = u128::from(initial) + elapsed as u128;
                            if total < u128::from(end) {
                                u64::try_from(total).unwrap()
                            } else {
                                start
                                    + u64::try_from(
                                        (total - u128::from(start)) % u128::from(length),
                                    )
                                    .unwrap()
                            }
                        };
                        let mut transport = Transport::new(48000);
                        transport.apply(TransportCommand::SetLoop(SampleRange::new(
                            SamplePosition(start),
                            SamplePosition(end),
                        )));
                        transport.apply(TransportCommand::Seek(SamplePosition(initial)));
                        transport.apply(TransportCommand::Play);
                        let mut frame = 0;
                        let mut block = 0;
                        while frame < 4096 {
                            let count = partitions[block % partitions.len()].min(4096 - frame);
                            for offset in 0..count {
                                assert_eq!(
                                    transport.sample_at_offset(offset).0,
                                    expected(frame + offset)
                                );
                            }
                            transport.advance(count);
                            frame += count;
                            block += 1;
                            assert_eq!(transport.snapshot().current_sample.0, expected(frame));
                        }
                    }
                }
            }
        }
    }
}
