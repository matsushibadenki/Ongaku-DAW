//! Sample-accurate MIDI clips, scheduling, and a small reusable reference instrument.

use audio_types::{
    AudioConfig, AudioError, PreparationCheck, PreparationToken, preparation_sort_by,
};
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
    identity_index: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NoteIdentity {
    clip_id: u64,
    note_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScheduledNote {
    identity: NoteIdentity,
    start: SamplePosition,
    end: SamplePosition,
    key: u8,
    velocity: f32,
    channel: u8,
}

impl MidiSchedule {
    /// # Panics
    /// If an internal fresh, unshared cancellation token unexpectedly becomes canceled.
    #[must_use]
    pub fn from_clips(clips: &[MidiClip]) -> Self {
        Self::from_clips_cancellable(clips, &PreparationToken::default())
            .expect("fresh token cannot be canceled")
    }

    pub fn from_clips_cancellable(
        clips: &[MidiClip],
        check: &impl PreparationCheck,
    ) -> Result<Self, AudioError> {
        check.checkpoint()?;
        let note_count = clips.iter().map(|clip| clip.notes.len()).sum::<usize>();
        let mut notes = Vec::with_capacity(note_count);
        for clip in clips {
            check.checkpoint()?;
            for (index, note) in clip.notes.iter().enumerate() {
                if index % 256 == 0 {
                    check.checkpoint()?;
                }
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
        preparation_sort_by(
            &mut notes,
            |a, b| (a.start, a.identity).cmp(&(b.start, b.identity)),
            check,
        )?;
        let mut owned_events = Vec::with_capacity(note_count.saturating_mul(2));
        for (index, note) in notes.iter().enumerate() {
            if index % 256 == 0 {
                check.checkpoint()?;
            }
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
        preparation_sort_by(
            &mut owned_events,
            |(left, left_owner), (right, right_owner)| {
                left.sample_position
                    .cmp(&right.sample_position)
                    .then_with(|| message_order(left.message).cmp(&message_order(right.message)))
                    .then_with(|| left_owner.cmp(right_owner))
            },
            check,
        )?;
        // Ownership indexes are prepared alongside the public MIDI events, never searched by pitch.
        let mut events = Vec::with_capacity(owned_events.len());
        let mut event_notes = Vec::with_capacity(owned_events.len());
        for (index, (event, owner)) in owned_events.into_iter().enumerate() {
            if index % 256 == 0 {
                check.checkpoint()?;
            }
            events.push(event);
            event_notes.push(owner);
        }
        let mut identity_index = Vec::with_capacity(notes.len());
        for index in 0..notes.len() {
            if index % 256 == 0 {
                check.checkpoint()?;
            }
            identity_index.push(index);
        }
        preparation_sort_by(
            &mut identity_index,
            |&a, &b| notes[a].identity.cmp(&notes[b].identity),
            check,
        )?;
        check.checkpoint()?;
        Ok(Self {
            events,
            notes,
            event_notes,
            identity_index,
        })
    }

    fn note(&self, identity: NoteIdentity) -> Option<&ScheduledNote> {
        self.identity_index
            .binary_search_by_key(&identity, |&index| self.notes[index].identity)
            .ok()
            .map(|index| &self.notes[self.identity_index[index]])
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
    phase_steps: [f32; 128],
    last_position: Option<SamplePosition>,
}

impl MidiSynthSource {
    pub fn new(
        config: AudioConfig,
        clips: &[MidiClip],
        level_automation: Option<AutomationLane>,
    ) -> Result<Self, AudioError> {
        Self::new_cancellable(
            config,
            clips,
            level_automation,
            &PreparationToken::default(),
        )
    }

    pub fn new_cancellable(
        config: AudioConfig,
        clips: &[MidiClip],
        level_automation: Option<AutomationLane>,
        check: &impl PreparationCheck,
    ) -> Result<Self, AudioError> {
        check.checkpoint()?;
        validate_clips(clips, check)?;
        let config = config.validate()?;
        #[allow(clippy::cast_precision_loss)]
        let phase_steps = std::array::from_fn(|key| {
            440.0 * 2.0_f32.powf((key as f32 - 69.0) / 12.0) / config.sample_rate as f32
        });
        Ok(Self {
            config,
            phase_steps,
            schedule: MidiSchedule::from_clips_cancellable(clips, check)?,
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
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            sample += (voice.phase * TAU).sin() * voice.velocity * 0.12;
            voice.phase = (voice.phase + self.phase_steps[usize::from(voice.key)]).fract();
        }
        (sample * level).clamp(-1.0, 1.0)
    }
}

impl RealtimeTimelineSource for MidiSynthSource {
    fn inherit_state(&mut self, previous: &dyn RealtimeTimelineSource, position: SamplePosition) {
        let Some(old) = (previous as &dyn std::any::Any).downcast_ref::<Self>() else {
            return;
        };
        if self.config != old.config
            || old
                .last_position
                .is_none_or(|last| last.0.checked_add(1) != Some(position.0))
        {
            return;
        }
        // Copy only fixed-size runtime data; schedules and automation stay newly prepared.
        self.last_position = old.last_position;
        for (index, voice) in old.voices.iter().enumerate() {
            if !voice.active {
                continue;
            }
            let Some(note) = voice
                .identity
                .and_then(|identity| self.schedule.note(identity))
            else {
                continue;
            };
            let old_note = voice
                .identity
                .and_then(|identity| old.schedule.note(identity));
            if note.start < position
                && position < note.end
                && old_note.is_some_and(|old_note| {
                    old_note.start == note.start
                        && old_note.key == note.key
                        && old_note.channel == note.channel
                })
            {
                // Keep slot order as well as phase: floating-point summation and future
                // free-slot selection must match uninterrupted playback.
                self.voices[index] = *voice;
                self.voices[index].velocity = note.velocity;
                self.voices[index].active = note.velocity > 0.0;
            }
        }
        // New/changed notes already spanning publication chase once. Unchanged stolen voices
        // stay stolen; boundary Note Ons are left to normal event processing.
        for note in &self.schedule.notes {
            if note.start < position
                && position < note.end
                && !self
                    .voices
                    .iter()
                    .any(|voice| voice.active && voice.identity == Some(note.identity))
                && old.schedule.note(note.identity) != Some(note)
            {
                activate_voice(&mut self.voices, note);
            }
        }
    }

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
    // A zero-velocity piano-roll note is silent and must not steal an audible voice.
    if note.velocity <= 0.0 {
        return;
    }
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

fn validate_clips(clips: &[MidiClip], check: &impl PreparationCheck) -> Result<(), AudioError> {
    let mut clip_ids = HashSet::new();
    for clip in clips {
        check.checkpoint()?;
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
        for (index, note) in clip.notes.iter().enumerate() {
            if index % 256 == 0 {
                check.checkpoint()?;
            }
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
    #[test]
    fn publication_reconciles_changed_notes_without_reviving_stolen_voices() {
        let config = AudioConfig::DEFAULT;
        let clips = overlap_clips();
        let mut old = MidiSynthSource::new(config, &clips, None).unwrap();
        let mut transport = Transport::new(config.sample_rate);
        transport.apply(TransportCommand::Play);
        old.render_interleaved(&mut [0.0; 12], &transport, 0);
        transport.advance(6);
        let phase = old.voices[0].phase;
        let mut edited = clips.clone();
        edited[0].notes[0].duration_samples = 24;
        edited[0].notes[0].velocity = 0.25;
        edited[1].notes[0].key = 72;
        let mut next = MidiSynthSource::new(config, &edited, None).unwrap();
        next.inherit_state(&old, SamplePosition(6));
        assert_eq!(next.voices[0].phase.to_bits(), phase.to_bits());
        assert_eq!(next.voices[0].velocity.to_bits(), 0.25_f32.to_bits());
        assert!(
            next.voices
                .iter()
                .any(|voice| voice.active && voice.key == 72 && voice.phase == 0.0)
        );
        assert!(!next.voices.iter().any(|voice| voice.active
            && voice.key == 60
            && voice.identity
                == Some(NoteIdentity {
                    clip_id: 2,
                    note_id: 1
                })));
        // Deleting an active note releases it; Undo can chase it but cannot revive a stale phase.
        edited[0].notes.clear();
        let mut deleted = MidiSynthSource::new(config, &edited, None).unwrap();
        deleted.inherit_state(&old, SamplePosition(6));
        assert!(!deleted.voices.iter().any(|voice| voice.active
            && voice.identity
                == Some(NoteIdentity {
                    clip_id: 1,
                    note_id: 1
                })));
        let mut deleted_clips = clips.clone();
        deleted_clips[0].notes.clear();
        let mut deleted = MidiSynthSource::new(config, &deleted_clips, None).unwrap();
        deleted.inherit_state(&old, SamplePosition(6));
        deleted.render_interleaved(&mut [0.0; 2], &transport, 0);
        let surviving = deleted.voices.iter().find(|voice| voice.active).unwrap();
        let mut restored = MidiSynthSource::new(config, &clips, None).unwrap();
        restored.inherit_state(&deleted, SamplePosition(7));
        let restored_survivor = restored
            .voices
            .iter()
            .find(|voice| voice.active && voice.identity == surviving.identity)
            .unwrap();
        assert_eq!(restored_survivor.phase.to_bits(), surviving.phase.to_bits());
        assert!(restored.voices.iter().any(|voice| voice.active
            && voice.identity
                == Some(NoteIdentity {
                    clip_id: 1,
                    note_id: 1
                })
            && voice.phase == 0.0));
        transport.advance(1);
        next.render_interleaved(&mut [0.0; 40], &transport, 0);
        assert!(next.voices.iter().all(|voice| !voice.active));
        // Non-contiguous publication must defer to the existing seek/loop chase policy.
        let mut seek = MidiSynthSource::new(config, &clips, None).unwrap();
        seek.inherit_state(&old, SamplePosition(10));
        assert!(seek.last_position.is_none());
    }

    #[test]
    fn identical_publication_preserves_holes_and_stolen_voice_state() {
        let config = AudioConfig::DEFAULT;
        let mut clips = overlap_clips();
        clips[0].notes = (0_u64..17)
            .map(|id| MidiNote {
                id,
                start: SamplePosition(id),
                duration_samples: if id == 1 { 20 } else { 100 },
                key: 48 + u8::try_from(id).unwrap(),
                channel: 0,
                velocity: 0.3,
            })
            .collect();
        clips[0].duration_samples = 200;
        clips.truncate(1);
        let mut old = MidiSynthSource::new(config, &clips, None).unwrap();
        let mut transport = Transport::new(config.sample_rate);
        transport.apply(TransportCommand::Play);
        old.render_interleaved(&mut [0.0; 50], &transport, 0);
        transport.advance(25);
        let mut next = MidiSynthSource::new(config, &clips, None).unwrap();
        next.inherit_state(&old, SamplePosition(25));
        assert_eq!(next.voices.iter().filter(|voice| voice.active).count(), 15);
        for (next, old) in next.voices.iter().zip(&old.voices) {
            if old.active {
                assert_eq!(next.identity, old.identity);
                assert_eq!(next.phase.to_bits(), old.phase.to_bits());
            }
        }
        let mut actual = [0.0; 150];
        let mut expected = actual;
        next.render_interleaved(&mut actual, &transport, 0);
        old.render_interleaved(&mut expected, &transport, 0);
        assert_eq!(actual, expected);
    }

    #[test]
    fn silent_notes_do_not_steal_audible_voices() {
        let notes = (0_u64..17)
            .map(|id| MidiNote {
                id,
                start: SamplePosition(u64::from(id == 16)),
                duration_samples: 16,
                key: 60,
                channel: 0,
                velocity: if id == 16 { 0.0 } else { 0.3 },
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
        source.render_interleaved(&mut [0.0; 4], &transport, 0);
        assert_eq!(
            source.voices.iter().filter(|voice| voice.active).count(),
            16
        );
        assert!(source.voices.iter().any(|voice| voice.active
            && voice.identity
                == Some(NoteIdentity {
                    clip_id: 1,
                    note_id: 0
                })));
        assert!(source.voices.iter().all(|voice| voice.identity
            != Some(NoteIdentity {
                clip_id: 1,
                note_id: 16
            })));
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::cell::Cell;
    struct Budget(Cell<usize>);
    impl PreparationCheck for Budget {
        fn checkpoint(&self) -> Result<(), AudioError> {
            let remaining = self.0.get();
            if remaining == 0 {
                Err(AudioError::PreparationCanceled)
            } else {
                self.0.set(remaining - 1);
                Ok(())
            }
        }
    }
    #[test]
    fn dense_schedule_construction_can_stop_during_collection_and_sorting() {
        let clips = [MidiClip {
            id: 1,
            timeline_start: SamplePosition::ZERO,
            duration_samples: 10001,
            notes: (0..10000)
                .rev()
                .map(|id| MidiNote {
                    id,
                    start: SamplePosition(id),
                    duration_samples: 1,
                    key: 60,
                    velocity: 0.5,
                    channel: 0,
                })
                .collect(),
        }];
        for budget in [0, 4, 50, 100, 500] {
            assert!(matches!(
                MidiSchedule::from_clips_cancellable(&clips, &Budget(Cell::new(budget))),
                Err(AudioError::PreparationCanceled)
            ));
        }
        let schedule = MidiSchedule::from_clips(&clips);
        assert_eq!(schedule.events().len(), 20000);
        assert_eq!(schedule.events()[0].sample_position, SamplePosition::ZERO);
        assert_eq!(
            schedule.events().last().unwrap().sample_position,
            SamplePosition(10000)
        );
    }
}
