//! UI-independent project model and prepared graph construction.

use audio_clip::{AudioClip, PcmAsset};
use audio_types::{AudioConfig, AudioError};
use daw_automation::{AutomationCurve, AutomationLane, AutomationParameter, AutomationPoint};
use daw_core::{SamplePosition, transport::TimeSignature};
use daw_midi::{MidiClip, MidiNote, MidiSynthSource};
use daw_mixer::{AudioGraph, Track, TrackId};
use daw_waveform::{WaveformPeak, WaveformPyramid};
use serde::{Deserialize, Serialize};
use std::{f32::consts::TAU, sync::Arc};

mod clip_edit;
mod multi_clip;
mod selection;
mod storage;
pub use clip_edit::{MidiClipEdit, MidiClipTarget};
pub use multi_clip::MidiNoteTarget;
pub use selection::MidiSelectionEdit;

const DEMO_BARS: u64 = 4;
const WAVEFORM_PEAKS: usize = 192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackRole {
    Rhythm,
    Bass,
    Atmosphere,
    Lead,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectClipSnapshot {
    pub id: u64,
    pub timeline_start: SamplePosition,
    pub duration_samples: u64,
    pub waveform: Vec<WaveformPeak>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrackSnapshot {
    pub id: u64,
    pub role: TrackRole,
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub clips: Vec<ProjectClipSnapshot>,
    pub midi_clips: Vec<MidiClip>,
    pub automation_lanes: Vec<AutomationLane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub name: String,
    pub sample_rate: u32,
    pub tempo_bpm: f64,
    pub time_signature: TimeSignature,
    pub length_samples: u64,
    pub tracks: Vec<ProjectTrackSnapshot>,
}

#[derive(Clone)]
struct ProjectClip {
    id: u64,
    timeline_start: SamplePosition,
    asset: Arc<PcmAsset>,
    waveform: WaveformPyramid,
}

#[derive(Clone)]
struct ProjectTrack {
    id: TrackId,
    role: TrackRole,
    gain: f32,
    pan: f32,
    muted: bool,
    solo: bool,
    clips: Vec<ProjectClip>,
    midi_clips: Vec<MidiClip>,
    automation_lanes: Vec<AutomationLane>,
}

#[derive(Clone)]
pub struct Project {
    name: String,
    sample_rate: u32,
    tempo_bpm: f64,
    time_signature: TimeSignature,
    length_samples: u64,
    tracks: Vec<ProjectTrack>,
}

impl Project {
    pub fn demo(sample_rate: u32) -> Result<Self, AudioError> {
        if sample_rate == 0 {
            return Err(AudioError::InvalidConfig(
                "project sample rate must be greater than zero",
            ));
        }
        let tempo_bpm = 120.0;
        let bar_frames = u64::from(sample_rate) * 2;
        let length_samples = bar_frames * DEMO_BARS;
        let definitions = [
            (TrackId(1), TrackRole::Rhythm, 0_u64, 0.8_f32, 0.0_f32),
            (TrackId(2), TrackRole::Bass, 0_u64, 0.62_f32, -0.12_f32),
            (
                TrackId(3),
                TrackRole::Atmosphere,
                bar_frames,
                0.45_f32,
                0.18_f32,
            ),
        ];
        let mut tracks = Vec::with_capacity(definitions.len());
        for (id, role, start, gain, pan) in definitions {
            let duration = length_samples.saturating_sub(start);
            let asset = Arc::new(PcmAsset::new(
                sample_rate,
                1,
                generate_demo_audio(role, sample_rate, duration),
            )?);
            let base_resolution = asset
                .frame_count()
                .div_ceil(u64::try_from(WAVEFORM_PEAKS).unwrap_or(u64::MAX))
                .max(1);
            let waveform = WaveformPyramid::analyze(&asset, base_resolution)?;
            tracks.push(ProjectTrack {
                id,
                role,
                gain,
                pan,
                muted: false,
                solo: false,
                clips: vec![ProjectClip {
                    id: id.0 * 100,
                    timeline_start: SamplePosition(start),
                    asset,
                    waveform,
                }],
                midi_clips: Vec::new(),
                automation_lanes: Vec::new(),
            });
        }
        tracks.push(demo_midi_track(sample_rate, length_samples)?);
        Ok(Self {
            name: "Night Sketch".into(),
            sample_rate,
            tempo_bpm,
            time_signature: TimeSignature::default(),
            length_samples,
            tracks,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> ProjectSnapshot {
        ProjectSnapshot {
            name: self.name.clone(),
            sample_rate: self.sample_rate,
            tempo_bpm: self.tempo_bpm,
            time_signature: self.time_signature,
            length_samples: self.length_samples,
            tracks: self
                .tracks
                .iter()
                .map(|track| ProjectTrackSnapshot {
                    id: track.id.0,
                    role: track.role,
                    gain: track.gain,
                    pan: track.pan,
                    muted: track.muted,
                    solo: track.solo,
                    clips: track
                        .clips
                        .iter()
                        .map(|clip| {
                            let level = clip.waveform.level_for_max_peaks(WAVEFORM_PEAKS);
                            ProjectClipSnapshot {
                                id: clip.id,
                                timeline_start: clip.timeline_start,
                                duration_samples: clip.asset.frame_count(),
                                waveform: level.peaks.to_vec(),
                            }
                        })
                        .collect(),
                    midi_clips: track.midi_clips.clone(),
                    automation_lanes: track.automation_lanes.clone(),
                })
                .collect(),
        }
    }

    pub fn build_audio_graph(&self, config: AudioConfig) -> Result<AudioGraph, AudioError> {
        if config.sample_rate != self.sample_rate {
            return Err(AudioError::InvalidConfig(
                "project and graph sample rates must match",
            ));
        }
        let mut graph = AudioGraph::new(config)?;
        for project_track in &self.tracks {
            let mut track = Track::new(project_track.id);
            track.set_gain(project_track.gain);
            track.set_pan(project_track.pan);
            track.set_mute(project_track.muted);
            track.set_solo(project_track.solo);
            for project_clip in &project_track.clips {
                track.add_clip(AudioClip::new(
                    project_clip.timeline_start,
                    0,
                    project_clip.asset.frame_count(),
                    Arc::clone(&project_clip.asset),
                )?);
            }
            if !project_track.midi_clips.is_empty() {
                let automation = project_track
                    .automation_lanes
                    .iter()
                    .find(|lane| lane.parameter == AutomationParameter::InstrumentLevel)
                    .cloned();
                track.set_timeline_source(Box::new(MidiSynthSource::new(
                    config,
                    &project_track.midi_clips,
                    automation,
                )?))?;
            }
            graph.add_track(track)?;
        }
        Ok(graph)
    }

    pub fn reconfigure_demo(&mut self, sample_rate: u32) -> Result<(), AudioError> {
        if self.sample_rate == sample_rate {
            return Ok(());
        }
        let settings: Vec<_> = self
            .tracks
            .iter()
            .map(|track| (track.id, track.gain, track.pan, track.muted, track.solo))
            .collect();
        let mut rebuilt = Self::demo(sample_rate)?;
        for track in &mut rebuilt.tracks {
            if let Some((_, gain, pan, muted, solo)) =
                settings.iter().find(|(id, ..)| *id == track.id)
            {
                track.gain = *gain;
                track.pan = *pan;
                track.muted = *muted;
                track.solo = *solo;
            }
        }
        *self = rebuilt;
        Ok(())
    }

    pub fn set_track_gain(&mut self, id: TrackId, gain: f32) -> Result<(), AudioError> {
        self.track_mut(id)?.gain = sanitize(gain, 0.0, 2.0);
        Ok(())
    }

    pub fn set_track_pan(&mut self, id: TrackId, pan: f32) -> Result<(), AudioError> {
        self.track_mut(id)?.pan = sanitize(pan, -1.0, 1.0);
        Ok(())
    }

    pub fn set_track_mute(&mut self, id: TrackId, muted: bool) -> Result<(), AudioError> {
        self.track_mut(id)?.muted = muted;
        Ok(())
    }

    pub fn set_track_solo(&mut self, id: TrackId, solo: bool) -> Result<(), AudioError> {
        self.track_mut(id)?.solo = solo;
        Ok(())
    }

    pub fn reorder_tracks(&mut self, ordered_ids: &[TrackId]) -> Result<(), AudioError> {
        let valid = ordered_ids.len() == self.tracks.len()
            && ordered_ids.iter().all(|id| {
                ordered_ids
                    .iter()
                    .filter(|candidate| *candidate == id)
                    .count()
                    == 1
                    && self.tracks.iter().any(|track| track.id == *id)
            });
        if !valid {
            return Err(AudioError::Backend(
                "track order did not match the project".into(),
            ));
        }
        self.tracks.sort_by_key(|track| {
            ordered_ids
                .iter()
                .position(|id| *id == track.id)
                .unwrap_or(usize::MAX)
        });
        Ok(())
    }

    /// Edits clip notes on the control thread; playback receives a prepared graph.
    pub fn edit_midi_clip(
        &mut self,
        track_id: TrackId,
        clip_id: u64,
        action: &str,
        amount: i32,
        note_id: Option<u64>,
    ) -> Result<(), AudioError> {
        let grid = match action {
            "quantize" if [4, 8, 16, 32].contains(&amount) => {
                (f64::from(self.sample_rate) * 60.0 / self.tempo_bpm * 4.0 / f64::from(amount))
                    .round()
                    .max(1.0) as u64
            }
            "transpose" if (-24..=24).contains(&amount) => 1,
            "delete" if note_id.is_some() => 1,
            _ => return Err(AudioError::Backend("invalid MIDI edit".into())),
        };
        let clip = self
            .track_mut(track_id)?
            .midi_clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| AudioError::Backend("MIDI clip was not found".into()))?;
        if let Some(id) = note_id {
            if !clip.notes.iter().any(|note| note.id == id) {
                return Err(AudioError::Backend("MIDI note was not found".into()));
            }
        }
        if action == "delete" {
            clip.notes.retain(|note| Some(note.id) != note_id);
            return Ok(());
        }
        for note in &mut clip.notes {
            if note_id.is_some_and(|id| note.id != id) {
                continue;
            }
            if action == "transpose" {
                note.key = (i32::from(note.key) + amount).clamp(0, 127) as u8;
            } else {
                let snapped = note.start.0.saturating_add(grid / 2) / grid * grid;
                note.start = SamplePosition(snapped.min(clip.duration_samples.saturating_sub(1)));
                note.duration_samples = note
                    .duration_samples
                    .min(clip.duration_samples - note.start.0);
            }
        }
        clip.notes.sort_by_key(|note| (note.start, note.id));
        Ok(())
    }

    /// Adds or replaces a note's geometry without changing its velocity or channel.
    /// Validation completes before mutation; positions are relative to the MIDI clip.
    pub fn write_midi_note(
        &mut self,
        track_id: TrackId,
        clip_id: u64,
        note_id: Option<u64>,
        start: u64,
        duration_samples: u64,
        key: u8,
    ) -> Result<(), AudioError> {
        let clip = self
            .track_mut(track_id)?
            .midi_clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| AudioError::Backend("MIDI clip was not found".into()))?;
        if key > 127
            || duration_samples == 0
            || start
                .checked_add(duration_samples)
                .is_none_or(|end| end > clip.duration_samples)
        {
            return Err(AudioError::InvalidConfig(
                "MIDI note must be within its clip and MIDI pitch range",
            ));
        }
        if let Some(id) = note_id {
            let note = clip
                .notes
                .iter_mut()
                .find(|note| note.id == id)
                .ok_or_else(|| AudioError::Backend("MIDI note was not found".into()))?;
            note.start = SamplePosition(start);
            note.duration_samples = duration_samples;
            note.key = key;
        } else {
            let id = clip
                .notes
                .iter()
                .map(|note| note.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| AudioError::Backend("MIDI note identifiers exhausted".into()))?;
            clip.notes.push(MidiNote {
                id,
                start: SamplePosition(start),
                duration_samples,
                key,
                velocity: 0.75,
                channel: 0,
            });
        }
        clip.notes.sort_by_key(|note| (note.start, note.id));
        Ok(())
    }

    pub fn set_midi_note_velocity(
        &mut self,
        track_id: TrackId,
        clip_id: u64,
        note_id: u64,
        velocity: f32,
    ) -> Result<(), AudioError> {
        let track = self.track_mut(track_id)?;
        let clip = track
            .midi_clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| AudioError::Backend(format!("MIDI clip {clip_id} was not found")))?;
        let note = clip
            .notes
            .iter_mut()
            .find(|note| note.id == note_id)
            .ok_or_else(|| AudioError::Backend(format!("MIDI note {note_id} was not found")))?;
        note.velocity = sanitize(velocity, 0.0, 1.0);
        Ok(())
    }

    pub fn set_automation_point_value(
        &mut self,
        track_id: TrackId,
        lane_id: u64,
        point_id: u64,
        value: f32,
    ) -> Result<(), AudioError> {
        let lane = self
            .track_mut(track_id)?
            .automation_lanes
            .iter_mut()
            .find(|lane| lane.id == lane_id)
            .ok_or_else(|| {
                AudioError::Backend(format!("automation lane {lane_id} was not found"))
            })?;
        lane.set_point_value(point_id, value)
    }

    fn track_mut(&mut self, id: TrackId) -> Result<&mut ProjectTrack, AudioError> {
        self.tracks
            .iter_mut()
            .find(|track| track.id == id)
            .ok_or_else(|| AudioError::Backend(format!("track {} was not found", id.0)))
    }
}

fn demo_midi_track(sample_rate: u32, length_samples: u64) -> Result<ProjectTrack, AudioError> {
    let beat = (u64::from(sample_rate) / 2).max(1);
    let note_length = beat.saturating_mul(3) / 4;
    let keys = [
        60_u8, 63, 67, 70, 67, 63, 65, 68, 72, 68, 65, 63, 60, 67, 65, 63,
    ];
    let notes = keys
        .into_iter()
        .enumerate()
        .map(|(index, key)| MidiNote {
            id: 4_000 + u64::try_from(index).unwrap_or(u64::MAX),
            start: SamplePosition(
                u64::try_from(index)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(beat),
            ),
            duration_samples: note_length,
            key,
            velocity: if index % 4 == 0 { 0.86 } else { 0.64 },
            channel: 0,
        })
        .collect();
    let automation = AutomationLane::new(
        4_200,
        AutomationParameter::InstrumentLevel,
        0.0,
        1.0,
        0.7,
        vec![
            AutomationPoint {
                id: 4_201,
                position: SamplePosition(0),
                value: 0.45,
                curve: AutomationCurve::Linear,
            },
            AutomationPoint {
                id: 4_202,
                position: SamplePosition(length_samples / 2),
                value: 0.9,
                curve: AutomationCurve::Linear,
            },
            AutomationPoint {
                id: 4_203,
                position: SamplePosition(length_samples),
                value: 0.55,
                curve: AutomationCurve::Linear,
            },
        ],
    )?;
    Ok(ProjectTrack {
        id: TrackId(4),
        role: TrackRole::Lead,
        gain: 0.5,
        pan: 0.08,
        muted: false,
        solo: false,
        clips: Vec::new(),
        midi_clips: vec![MidiClip {
            id: 4_100,
            timeline_start: SamplePosition(0),
            duration_samples: length_samples,
            notes,
        }],
        automation_lanes: vec![automation],
    })
}

fn sanitize(value: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        0.0
    }
}

#[allow(clippy::cast_precision_loss)]
fn generate_demo_audio(role: TrackRole, sample_rate: u32, frames: u64) -> Vec<f32> {
    let capacity = usize::try_from(frames).unwrap_or(usize::MAX);
    let mut output = Vec::with_capacity(capacity);
    let beat_frames = (u64::from(sample_rate) / 2).max(1);
    let sample_rate = sample_rate as f32;
    for frame in 0..frames {
        let time = frame as f32 / sample_rate;
        let beat_phase = time % 0.5;
        let sample = match role {
            TrackRole::Rhythm => {
                let beat = frame / beat_frames;
                let kick = (TAU * 54.0 * beat_phase).sin() * (-beat_phase * 34.0).exp();
                let tick_phase = time % 0.25;
                let tick = if beat % 2 == 1 {
                    (TAU * 1_800.0 * tick_phase).sin() * (-tick_phase * 80.0).exp() * 0.18
                } else {
                    0.0
                };
                kick.mul_add(0.55, tick)
            }
            TrackRole::Bass => {
                let envelope = (-beat_phase * 3.2).exp();
                (TAU * 110.0 * time).sin() * envelope * 0.42
            }
            TrackRole::Atmosphere => {
                let slow = (TAU * 0.125 * time).sin().mul_add(0.12, 0.28);
                ((TAU * 220.0 * time).sin() + (TAU * 277.18 * time).sin() * 0.6) * slow
            }
            TrackRole::Lead => 0.0,
        };
        output.push(sample.clamp(-1.0, 1.0));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::Project;
    use audio_types::AudioConfig;
    use daw_core::transport::{Transport, TransportCommand};
    use daw_mixer::TrackId;

    #[test]
    fn note_geometry_is_validated_and_preserves_expression() {
        let mut project = Project::demo(48_000).unwrap();
        let before = project.snapshot();
        for (start, duration, key) in [(0, 0, 60), (384_000, 1, 60), (u64::MAX, 2, 60), (0, 1, 128)]
        {
            assert!(
                project
                    .write_midi_note(TrackId(4), 4100, None, start, duration, key)
                    .is_err()
            );
            assert_eq!(project.snapshot(), before);
        }
        assert!(
            project
                .write_midi_note(TrackId(99), 4100, None, 0, 1, 60)
                .is_err()
        );
        assert!(
            project
                .write_midi_note(TrackId(4), 999, None, 0, 1, 60)
                .is_err()
        );
        assert!(
            project
                .write_midi_note(TrackId(4), 4100, Some(999), 0, 1, 60)
                .is_err()
        );
        project
            .write_midi_note(TrackId(4), 4100, None, 12_000, 6_000, 127)
            .unwrap();
        let added = project.snapshot().tracks[3].midi_clips[0]
            .notes
            .iter()
            .find(|note| note.id == 4016)
            .copied()
            .unwrap();
        project
            .write_midi_note(TrackId(4), 4100, Some(added.id), 0, 384_000, 0)
            .unwrap();
        let snapshot = project.snapshot();
        let note = snapshot.tracks[3].midi_clips[0]
            .notes
            .iter()
            .find(|note| note.id == added.id)
            .unwrap();
        assert_eq!(note.velocity, added.velocity);
        assert_eq!(note.channel, added.channel);
        assert_eq!(note.key, 0);
        assert_eq!(note.duration_samples, 384_000);
        project.build_audio_graph(AudioConfig::DEFAULT).unwrap();
    }

    #[test]
    fn midi_edits_validate_targets_and_preserve_clip_bounds() {
        let mut project = Project::demo(48_000).unwrap();
        let id = TrackId(4);
        let clip = &mut project.tracks[3].midi_clips[0];
        clip.notes[0].start = daw_core::SamplePosition(7_000);
        clip.notes[0].key = 127;
        project
            .edit_midi_clip(id, 4_100, "quantize", 16, None)
            .unwrap();
        project
            .edit_midi_clip(id, 4_100, "transpose", 1, Some(4_000))
            .unwrap();
        let snapshot = project.snapshot();
        let clip = &snapshot.tracks[3].midi_clips[0];
        let note = clip.notes.iter().find(|note| note.id == 4_000).unwrap();
        assert_eq!(note.start.0, 6_000);
        assert_eq!(note.key, 127);
        assert!(
            clip.notes
                .iter()
                .all(|note| note.start.0 + note.duration_samples <= clip.duration_samples)
        );
        assert!(
            project
                .edit_midi_clip(id, 4_100, "quantize", 0, None)
                .is_err()
        );
        assert!(
            project
                .edit_midi_clip(id, 4_100, "delete", 0, Some(999))
                .is_err()
        );
        project
            .edit_midi_clip(id, 4_100, "delete", 0, Some(4_000))
            .unwrap();
        assert_eq!(project.snapshot().tracks[3].midi_clips[0].notes.len(), 15);
        project.build_audio_graph(AudioConfig::DEFAULT).unwrap();
    }

    #[test]
    fn demo_project_builds_a_playable_graph_and_snapshot() {
        let project = Project::demo(48_000).unwrap();
        let snapshot = project.snapshot();
        assert_eq!(snapshot.tracks.len(), 4);
        assert!(
            snapshot
                .tracks
                .iter()
                .filter(|track| !track.clips.is_empty())
                .all(|track| !track.clips[0].waveform.is_empty())
        );
        assert_eq!(snapshot.tracks[3].midi_clips[0].notes.len(), 16);
        let mut graph = project.build_audio_graph(AudioConfig::DEFAULT).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 256];
        graph.process(&mut output, &transport);
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
    }

    #[test]
    fn mixer_edits_remain_in_the_rust_project_model() {
        let mut project = Project::demo(48_000).unwrap();
        project.set_track_gain(TrackId(2), 0.25).unwrap();
        project.set_track_mute(TrackId(2), true).unwrap();
        let snapshot = project.snapshot();
        let bass = &snapshot.tracks[1];
        assert!((bass.gain - 0.25).abs() < f32::EPSILON);
        assert!(bass.muted);
    }

    #[test]
    fn track_order_edits_remain_in_the_rust_project_model() {
        let mut project = Project::demo(48_000).unwrap();
        project
            .reorder_tracks(&[TrackId(4), TrackId(2), TrackId(1), TrackId(3)])
            .unwrap();
        let ids = project
            .snapshot()
            .tracks
            .into_iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![4, 2, 1, 3]);
    }

    #[test]
    fn midi_and_automation_edits_remain_in_the_project_model() {
        let mut project = Project::demo(48_000).unwrap();
        project
            .set_midi_note_velocity(TrackId(4), 4_100, 4_000, 0.25)
            .unwrap();
        project
            .set_automation_point_value(TrackId(4), 4_200, 4_202, 0.35)
            .unwrap();
        let lead = &project.snapshot().tracks[3];
        assert!((lead.midi_clips[0].notes[0].velocity - 0.25).abs() < f32::EPSILON);
        assert!((lead.automation_lanes[0].points()[1].value - 0.35).abs() < f32::EPSILON);
    }
}
