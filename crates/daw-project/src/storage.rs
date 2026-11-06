//! Versioned, self-contained project files. PCM never crosses the `WebView` bridge.
use super::{Project, ProjectClip, ProjectSnapshot, ProjectTrack, WAVEFORM_PEAKS};
use audio_clip::PcmAsset;
use audio_types::AudioError;
use daw_automation::AutomationLane;
use daw_mixer::TrackId;
use daw_waveform::WaveformPyramid;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    format: String,
    version: u32,
    project: ProjectSnapshot,
    media: Vec<Media>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Media {
    track_id: u64,
    clip_id: u64,
    channels: u16,
    samples: Vec<f32>,
}

fn failure(error: impl std::fmt::Display) -> AudioError {
    AudioError::Backend(format!("project file: {error}"))
}
fn require(valid: bool) -> Result<(), AudioError> {
    if valid {
        Ok(())
    } else {
        Err(failure("invalid project data"))
    }
}

impl Project {
    /// Writes beside the destination, then atomically replaces it after a successful flush.
    pub fn save(&self, path: &Path) -> Result<(), AudioError> {
        let document = Document {
            format: "ongaku-project".into(),
            version: if self
                .tracks
                .iter()
                .any(|track| !track.mixer_events.is_empty())
            {
                2
            } else {
                1
            },
            project: self.snapshot(),
            media: self
                .tracks
                .iter()
                .flat_map(|track| {
                    track.clips.iter().map(|clip| Media {
                        track_id: track.id.0,
                        clip_id: clip.id,
                        channels: clip.asset.channels(),
                        samples: clip.asset.interleaved_samples().to_vec(),
                    })
                })
                .collect(),
        };
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(failure)?;
        serde_json::to_writer(&mut temporary, &document).map_err(failure)?;
        temporary.flush().map_err(failure)?;
        temporary.as_file().sync_all().map_err(failure)?;
        temporary.persist(path).map_err(failure)?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Validate the complete document before constructing a replacement.
    pub fn load(path: &Path) -> Result<Self, AudioError> {
        let file = File::open(path).map_err(failure)?;
        require(file.metadata().map_err(failure)?.len() <= 512 * 1024 * 1024)?;
        let document: Document = serde_json::from_reader(BufReader::new(file)).map_err(failure)?;
        require(document.format == "ongaku-project" && matches!(document.version, 1 | 2))?;
        let version = document.version;
        let snapshot = document.project;
        require(
            (8_000..=192_000).contains(&snapshot.sample_rate)
                && (20.0..=400.0).contains(&snapshot.tempo_bpm)
                && snapshot.time_signature.numerator > 0
                && snapshot.time_signature.denominator.is_power_of_two()
                && snapshot.length_samples > 0
                && snapshot.tracks.len() <= 1024,
        )?;
        require(
            version == 2
                || snapshot
                    .tracks
                    .iter()
                    .all(|track| track.mixer_events.is_empty()),
        )?;
        let mut ids = HashSet::new();
        let mut media = document.media;
        let mut tracks = Vec::new();
        for track in snapshot.tracks {
            daw_mixer::validate_mixer_events(&track.mixer_events)?;
            require(
                track
                    .mixer_events
                    .iter()
                    .all(|event| event.position.0 < snapshot.length_samples),
            )?;
            require(
                ids.insert(track.id)
                    && (0.0..=2.0).contains(&track.gain)
                    && (-1.0..=1.0).contains(&track.pan),
            )?;
            let mut clip_ids = HashSet::new();
            let mut clips = Vec::new();
            for clip in track.clips {
                require(
                    clip_ids.insert(clip.id)
                        && clip.duration_samples > 0
                        && clip
                            .timeline_start
                            .0
                            .checked_add(clip.duration_samples)
                            .is_some_and(|end| end <= snapshot.length_samples),
                )?;
                let matches = media
                    .iter()
                    .filter(|m| m.track_id == track.id && m.clip_id == clip.id)
                    .count();
                require(matches == 1)?;
                let index = media
                    .iter()
                    .position(|m| m.track_id == track.id && m.clip_id == clip.id)
                    .ok_or_else(|| failure("missing media"))?;
                let data = media.swap_remove(index);
                require(
                    (1..=2).contains(&data.channels) && data.samples.iter().all(|s| s.is_finite()),
                )?;
                let asset = Arc::new(PcmAsset::new(
                    snapshot.sample_rate,
                    data.channels,
                    data.samples,
                )?);
                require(asset.frame_count() == clip.duration_samples)?;
                let waveform = WaveformPyramid::analyze(
                    &asset,
                    asset.frame_count().div_ceil(WAVEFORM_PEAKS as u64).max(1),
                )?;
                clips.push(ProjectClip {
                    id: clip.id,
                    timeline_start: clip.timeline_start,
                    asset,
                    waveform,
                    stream: super::streaming::MediaCache::default(),
                });
            }
            for clip in &track.midi_clips {
                require(
                    clip_ids.insert(clip.id)
                        && clip.duration_samples > 0
                        && clip
                            .timeline_start
                            .0
                            .checked_add(clip.duration_samples)
                            .is_some_and(|end| end <= snapshot.length_samples),
                )?;
                let mut note_ids = HashSet::new();
                for note in &clip.notes {
                    require(
                        note_ids.insert(note.id)
                            && note.key <= 127
                            && note.channel <= 15
                            && (0.0..=1.0).contains(&note.velocity)
                            && note.duration_samples > 0
                            && note
                                .start
                                .0
                                .checked_add(note.duration_samples)
                                .is_some_and(|end| end <= clip.duration_samples),
                    )?;
                }
            }
            let mut lane_ids = HashSet::new();
            let mut lanes = Vec::new();
            for lane in track.automation_lanes {
                require(lane_ids.insert(lane.id) && lane.default_value.is_finite())?;
                let mut point_ids = HashSet::new();
                require(lane.points().iter().all(|p| {
                    point_ids.insert(p.id)
                        && p.value.is_finite()
                        && p.position.0 <= snapshot.length_samples
                }))?;
                lanes.push(AutomationLane::new(
                    lane.id,
                    lane.parameter,
                    lane.minimum,
                    lane.maximum,
                    lane.default_value,
                    lane.points().to_vec(),
                )?);
            }
            tracks.push(ProjectTrack {
                id: TrackId(track.id),
                role: track.role,
                gain: track.gain,
                pan: track.pan,
                muted: track.muted,
                solo: track.solo,
                clips,
                midi_clips: track.midi_clips,
                automation_lanes: lanes,
                mixer_events: track.mixer_events,
            });
        }
        require(media.is_empty())?;
        Ok(Self {
            name: snapshot.name,
            sample_rate: snapshot.sample_rate,
            tempo_bpm: snapshot.tempo_bpm,
            time_signature: snapshot.time_signature,
            length_samples: snapshot.length_samples,
            tracks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn self_contained_roundtrip_preserves_audio_and_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.ongaku");
        let mut project = Project::demo(8_000).unwrap();
        project
            .edit_midi_clip(TrackId(4), 4100, "delete", 0, Some(4000))
            .unwrap();
        project.set_track_gain(TrackId(1), 0.3).unwrap();
        project.save(&path).unwrap();
        let loaded = Project::load(&path).unwrap();
        assert_eq!(project.snapshot(), loaded.snapshot());
        assert_eq!(
            project.tracks[0].clips[0].asset.interleaved_samples(),
            loaded.tracks[0].clips[0].asset.interleaved_samples()
        );
        std::fs::write(&path, b"{\"version\":999}").unwrap();
        assert!(Project::load(&path).is_err());
    }
}
