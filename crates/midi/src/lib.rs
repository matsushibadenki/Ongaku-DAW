//! Sample-accurate MIDI clips, scheduling, and a small reusable reference instrument.

use audio_types::{AudioConfig, AudioError};
use daw_automation::AutomationLane;
use daw_core::{SamplePosition, transport::Transport};
use daw_mixer::RealtimeTimelineSource;
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

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
}

#[derive(Clone, Copy, Debug)]
struct ScheduledNote {
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
        let mut events = Vec::with_capacity(note_count.saturating_mul(2));
        let mut notes = Vec::with_capacity(note_count);
        for clip in clips {
            for note in &clip.notes {
                let start = clip.timeline_start.saturating_add(note.start.0);
                let end = start.saturating_add(note.duration_samples.max(1));
                notes.push(ScheduledNote {
                    start,
                    end,
                    key: note.key.min(127),
                    velocity: sanitize_velocity(note.velocity),
                    channel: note.channel.min(15),
                });
                events.push(MidiEvent {
                    sample_position: start,
                    channel: note.channel.min(15),
                    message: MidiMessage::NoteOn {
                        key: note.key.min(127),
                        velocity: sanitize_velocity(note.velocity),
                    },
                });
                events.push(MidiEvent {
                    sample_position: end,
                    channel: note.channel.min(15),
                    message: MidiMessage::NoteOff {
                        key: note.key.min(127),
                    },
                });
            }
        }
        events.sort_by(|left, right| {
            left.sample_position
                .cmp(&right.sample_position)
                .then_with(|| message_order(left.message).cmp(&message_order(right.message)))
        });
        notes.sort_by_key(|note| note.start);
        Self { events, notes }
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
    channel: u8,
    velocity: f32,
    phase: f32,
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
                activate_voice(&mut self.voices, note.key, note.channel, note.velocity);
            }
        }
    }

    fn apply_events(&mut self, position: SamplePosition) {
        for event in self.schedule.events_at(position) {
            match event.message {
                MidiMessage::NoteOn { key, velocity } => {
                    activate_voice(&mut self.voices, key, event.channel, velocity);
                }
                MidiMessage::NoteOff { key } => {
                    for voice in &mut self.voices {
                        if voice.active && voice.key == key && voice.channel == event.channel {
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

fn activate_voice(voices: &mut [Voice; VOICE_COUNT], key: u8, channel: u8, velocity: f32) {
    let index = voices
        .iter()
        .position(|voice| !voice.active)
        .unwrap_or_else(|| usize::from(key) % VOICE_COUNT);
    voices[index] = Voice {
        active: true,
        key,
        channel,
        velocity,
        phase: 0.0,
    };
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
