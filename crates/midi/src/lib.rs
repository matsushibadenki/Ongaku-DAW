//! Sample-accurate MIDI clips, scheduling, and a small reusable reference instrument.

use audio_types::{AudioConfig, AudioError};
use daw_automation::AutomationLane;
use daw_core::{SamplePosition, transport::Transport};
use daw_mixer::RealtimeTimelineSource;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, f32::consts::TAU};

const VOICE_COUNT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiNote {
    pub id: u64,
    pub start: SamplePosition,
    pub duration_samples: u64,
    pub key: u8,
    pub velocity: f32,
    pub channel: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiClip {
    pub id: u64,
    pub timeline_start: SamplePosition,
    pub duration_samples: u64,
    pub notes: Vec<MidiNote>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum MidiMessage {
    NoteOff { key: u8 },
    NoteOn { key: u8, velocity: f32 },
    ControlChange { controller: u8, value: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiEvent {
    pub sample_position: SamplePosition,
    pub channel: u8,
    pub message: MidiMessage,
}

#[derive(Clone, Debug)]
pub struct MidiSchedule {
    events: Vec<MidiEvent>,
    notes: Vec<ScheduledNote>,
    event_notes: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NoteIdentity {
    clip_id: u64,
    note_id: u64,
}

#[derive(Clone, Copy, Debug)]
struct ScheduledNote {
    identity: NoteIdentity,
    start: SamplePosition,
    end: SamplePosition,
    key: u8,
    velocity: f32,
    channel: u8,
}

impl MidiSchedule {
    #[must_use]
    pub fn from_clips(clips: &[MidiClip]) -> Self {
        let note_count = clips.iter().map(|clip| clip.notes.len()).sum::<usize>();
        let mut notes = Vec::with_capacity(note_count);
        for clip in clips {
            for note in &clip.notes {
                let start = clip.timeline_start.saturating_add(note.start.0);
                notes.push(ScheduledNote {
                    identity: NoteIdentity {
                        clip_id: clip.id,
                        note_id: note.id,
                    },
                    start,
                    end: start.saturating_add(note.duration_samples.max(1)),
                    key: note.key.min(127),
                    velocity: sanitize_velocity(note.velocity),
                    channel: note.channel.min(15),
                });
            }
        }
        notes.sort_by_key(|note| (note.start, note.identity));
        let mut owned_events = Vec::with_capacity(note_count.saturating_mul(2));
        for (index, note) in notes.iter().enumerate() {
            owned_events.push((
                MidiEvent {
                    sample_position: note.start,
                    channel: note.channel,
                    message: MidiMessage::NoteOn {
                        key: note.key,
                        velocity: note.velocity,
                    },
                },
                index,
            ));
            owned_events.push((
                MidiEvent {
                    sample_position: note.end,
                    channel: note.channel,
                    message: MidiMessage::NoteOff { key: note.key },
                },
                index,
            ));
        }
        owned_events.sort_by(|(left, left_owner), (right, right_owner)| {
            left.sample_position
                .cmp(&right.sample_position)
                .then_with(|| message_order(left.message).cmp(&message_order(right.message)))
                .then_with(|| left_owner.cmp(right_owner))
        });
        // Ownership indexes are prepared alongside the public MIDI events, never searched by pitch.
        let (events, event_notes) = owned_events.into_iter().unzip();
        Self {
            events,
            notes,
            event_notes,
        }
    }

    #[must_use]
    pub fn events(&self) -> &[MidiEvent] {
        &self.events
    }

    #[must_use]
    pub fn events_at(&self, position: SamplePosition) -> &[MidiEvent] {
        let start = self
            .events
            .partition_point(|event| event.sample_position < position);
        let end =
            self.events[start..].partition_point(|event| event.sample_position == position) + start;
        &self.events[start..end]
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    active: bool,
    key: u8,
    velocity: f32,
    phase: f32,
    identity: Option<NoteIdentity>,
    started_at: SamplePosition,
}

pub struct MidiSynthSource {
    config: AudioConfig,
    schedule: MidiSchedule,
    level_automation: Option<AutomationLane>,
    voices: [Voice; VOICE_COUNT],
    last_position: Option<SamplePosition>,
}

impl MidiSynthSource {
    pub fn new(
        config: AudioConfig,
        clips: &[MidiClip],
        level_automation: Option<AutomationLane>,
    ) -> Result<Self, AudioError> {
        validate_clips(clips)?;
        Ok(Self {
            config: config.validate()?,
            schedule: MidiSchedule::from_clips(clips),
            level_automation,
            voices: [Voice::default(); VOICE_COUNT],
            last_position: None,
        })
    }

    fn synchronize(&mut self, position: SamplePosition) {
        self.voices.fill(Voice::default());
        for note in &self.schedule.notes {
            if note.start < position && position < note.end {
                activate_voice(&mut self.voices, note);
            }
        }
    }

    fn apply_events(&mut self, position: SamplePosition) {
        let start = self
            .schedule
            .events
            .partition_point(|event| event.sample_position < position);
        let count = self.schedule.events[start..]
            .partition_point(|event| event.sample_position == position);
        for index in start..start + count {
            let event = self.schedule.events[index];
            let note = &self.schedule.notes[self.schedule.event_notes[index]];
            match event.message {
                MidiMessage::NoteOn { .. } => activate_voice(&mut self.voices, note),
                MidiMessage::NoteOff { .. } => {
                    for voice in &mut self.voices {
                        if voice.active && voice.identity == Some(note.identity) {
                            voice.active = false;
                        }
                    }
                }
                MidiMessage::ControlChange { .. } => {}
            }
        }
    }

    fn next_sample(&mut self, position: SamplePosition) -> f32 {
        let contiguous = self
            .last_position
            .is_some_and(|last| position.0 == last.0.saturating_add(1));
        if !contiguous {
            self.synchronize(position);
        }
        self.apply_events(position);
        self.last_position = Some(position);
        let level = self
            .level_automation
            .as_ref()
            .map_or(1.0, |lane| lane.value_at(position));
        let mut sample = 0.0;
        #[allow(clippy::cast_precision_loss)]
        let sample_rate = self.config.sample_rate as f32;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let frequency = 440.0 * 2.0_f32.powf((f32::from(voice.key) - 69.0) / 12.0);
            sample += (voice.phase * TAU).sin() * voice.velocity * 0.12;
            voice.phase = (voice.phase + frequency / sample_rate).fract();
        }
        (sample * level).clamp(-1.0, 1.0)
    }
}

impl RealtimeTimelineSource for MidiSynthSource {
    fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }
    fn channels(&self) -> u16 {
        self.config.channels
    }

    fn render_interleaved(
        &mut self,
        output: &mut [f32],
        transport: &Transport,
        frame_offset: usize,
    ) {
        output.fill(0.0);
        if !transport.snapshot().playing {
            self.last_position = None;
            return;
        }
        let channels = usize::from(self.config.channels);
        for (local_frame, frame) in output.chunks_exact_mut(channels).enumerate() {
            let position = transport.sample_at_offset(frame_offset.saturating_add(local_frame));
            let sample = self.next_sample(position);
            frame.fill(sample);
        }
    }
}

fn activate_voice(voices: &mut [Voice; VOICE_COUNT], note: &ScheduledNote) {
    let index = voices
        .iter()
        .position(|voice| !voice.active)
        .unwrap_or_else(|| {
            voices
                .iter()
                .enumerate()
                .min_by_key(|(_, voice)| (voice.started_at, voice.identity))
                .map_or(0, |(index, _)| index)
        });
    voices[index] = Voice {
        active: true,
        key: note.key,
        velocity: note.velocity,
        phase: 0.0,
        identity: Some(note.identity),
        started_at: note.start,
    };
}

fn validate_clips(clips: &[MidiClip]) -> Result<(), AudioError> {
    let mut clip_ids = HashSet::new();
    for clip in clips {
        if !clip_ids.insert(clip.id)
            || clip.duration_samples == 0
            || clip
                .timeline_start
                .0
                .checked_add(clip.duration_samples)
                .is_none()
        {
            return Err(AudioError::InvalidConfig(
                "invalid MIDI clip identity or range",
            ));
        }
        let mut note_ids = HashSet::new();
        for note in &clip.notes {
            if !note_ids.insert(note.id)
                || note.key > 127
                || note.channel > 15
                || !(0.0..=1.0).contains(&note.velocity)
                || note.duration_samples == 0
                || note
                    .start
                    .0
                    .checked_add(note.duration_samples)
                    .is_none_or(|end| end > clip.duration_samples)
            {
                return Err(AudioError::InvalidConfig(
                    "invalid MIDI note identity, expression or range",
                ));
            }
        }
    }
    Ok(())
}

const fn message_order(message: MidiMessage) -> u8 {
    match message {
        MidiMessage::NoteOff { .. } => 0,
        MidiMessage::ControlChange { .. } => 1,
        MidiMessage::NoteOn { .. } => 2,
    }
}

fn sanitize_velocity(velocity: f32) -> f32 {
    if velocity.is_finite() {
        velocity.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{MidiClip, MidiMessage, MidiNote, MidiSchedule, MidiSynthSource};
    use audio_types::AudioConfig;
    use daw_core::{
        SamplePosition,
        transport::{Transport, TransportCommand},
    };
    use daw_mixer::RealtimeTimelineSource;

    fn clip() -> MidiClip {
        MidiClip {
            id: 1,
            timeline_start: SamplePosition(10),
            duration_samples: 100,
            notes: vec![MidiNote {
                id: 2,
                start: SamplePosition(5),
                duration_samples: 20,
                key: 60,
                velocity: 0.8,
                channel: 0,
            }],
        }
    }

    #[test]
    fn compiles_note_boundaries_to_sample_accurate_events() {
        let schedule = MidiSchedule::from_clips(&[clip()]);
        assert_eq!(schedule.events()[0].sample_position, SamplePosition(15));
        assert!(matches!(
            schedule.events()[0].message,
            MidiMessage::NoteOn { key: 60, .. }
        ));
        assert_eq!(schedule.events()[1].sample_position, SamplePosition(35));
    }

    #[test]
    fn reference_instrument_starts_exactly_at_the_note_sample() {
        let mut source = MidiSynthSource::new(AudioConfig::DEFAULT, &[clip()], None).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 40];
        source.render_interleaved(&mut output, &transport, 0);
        assert!(
            output[..30]
                .iter()
                .all(|sample| sample.abs() < f32::EPSILON)
        );
        assert!(
            output[32..]
                .iter()
                .any(|sample| sample.abs() > f32::EPSILON)
        );
    }

    #[test]
    fn seek_into_a_sustained_note_reconstructs_voice_state() {
        let mut source = MidiSynthSource::new(AudioConfig::DEFAULT, &[clip()], None).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Seek(SamplePosition(20)));
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 16];
        source.render_interleaved(&mut output, &transport, 0);
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
    }
}

#[cfg(test)]
mod p0_regressions {
    use super::*;
    use daw_core::transport::TransportCommand;
    fn overlap_clips() -> Vec<MidiClip> {
        [0, 4]
            .into_iter()
            .enumerate()
            .map(|(index, start)| MidiClip {
                id: index as u64 + 1,
                timeline_start: SamplePosition(start),
                duration_samples: 32,
                notes: vec![MidiNote {
                    id: 1,
                    start: SamplePosition(0),
                    duration_samples: if index == 0 { 8 } else { 12 },
                    key: 60,
                    channel: 0,
                    velocity: 0.5,
                }],
            })
            .collect()
    }
    #[test]
    fn note_off_does_not_kill_an_overlapping_note_with_the_same_key_and_channel() {
        let mut source =
            MidiSynthSource::new(AudioConfig::DEFAULT, &overlap_clips(), None).unwrap();
        let mut transport = Transport::new(48000);
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 48];
        source.render_interleaved(&mut output, &transport, 0);
        assert!(
            output[16..32]
                .iter()
                .any(|sample| sample.abs() > f32::EPSILON),
            "the later note must survive the earlier note-off"
        );
        assert!(
            output[32..]
                .iter()
                .all(|sample| sample.abs() < f32::EPSILON),
            "no voice may survive its own note-off"
        );
    }
    #[test]
    fn a_stolen_note_off_cannot_release_its_replacement_voice() {
        let notes = (0_u64..17)
            .map(|id| MidiNote {
                id,
                start: SamplePosition(u64::from(id == 16)),
                duration_samples: if id == 0 { 2 } else { 8 },
                key: 60,
                channel: 0,
                velocity: 0.3,
            })
            .collect();
        let clip = MidiClip {
            id: 1,
            timeline_start: SamplePosition(0),
            duration_samples: 32,
            notes,
        };
        let mut source = MidiSynthSource::new(AudioConfig::DEFAULT, &[clip], None).unwrap();
        let mut transport = Transport::new(48000);
        transport.apply(TransportCommand::Play);
        source.render_interleaved(&mut [0.0; 6], &transport, 0);
        assert_eq!(
            source.voices.iter().filter(|voice| voice.active).count(),
            16
        );
        assert!(source.voices.iter().any(|voice| voice.active
            && voice.identity
                == Some(NoteIdentity {
                    clip_id: 1,
                    note_id: 16
                })));
        transport.advance(3);
        source.render_interleaved(&mut [0.0; 26], &transport, 0);
        assert!(source.voices.iter().all(|voice| !voice.active));
    }

    fn partitioned_audio(partitions: &[usize], looping: bool) -> Vec<f32> {
        let mut source =
            MidiSynthSource::new(AudioConfig::DEFAULT, &overlap_clips(), None).unwrap();
        let mut transport = Transport::new(48000);
        if looping {
            transport.apply(TransportCommand::SetLoop(daw_core::SampleRange::new(
                SamplePosition(3),
                SamplePosition(19),
            )));
        }
        transport.apply(TransportCommand::Play);
        let mut output = vec![0.0; 4096];
        let mut frame = 0;
        let mut block = 0;
        while frame < 2048 {
            let count = partitions[block % partitions.len()].min(2048 - frame);
            source.render_interleaved(&mut output[frame * 2..(frame + count) * 2], &transport, 0);
            transport.advance(count);
            frame += count;
            block += 1;
        }
        output
    }
    #[test]
    fn overlapping_midi_pcm_is_identical_across_partitions_and_loop_wraps() {
        for looping in [false, true] {
            let expected = partitioned_audio(&[2048], looping);
            for blocks in [&[1][..], &[17, 64, 127, 128, 257, 512, 1024][..]] {
                assert_eq!(partitioned_audio(blocks, looping), expected);
            }
        }
    }
    #[test]
    fn malformed_note_ownership_and_ranges_are_rejected_before_playback() {
        let clips = overlap_clips();
        assert!(MidiSynthSource::new(AudioConfig::DEFAULT, &clips, None).is_ok());
        let mut duplicate = clips.clone();
        duplicate[1].id = duplicate[0].id;
        assert!(MidiSynthSource::new(AudioConfig::DEFAULT, &duplicate, None).is_err());
        let mut duplicate = clips.clone();
        let repeated = duplicate[0].notes[0];
        duplicate[0].notes.push(repeated);
        assert!(MidiSynthSource::new(AudioConfig::DEFAULT, &duplicate, None).is_err());
        let mut invalid = clips;
        invalid[0].notes[0].duration_samples = 33;
        assert!(MidiSynthSource::new(AudioConfig::DEFAULT, &invalid, None).is_err());
    }
}
