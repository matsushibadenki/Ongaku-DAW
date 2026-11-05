//! Cross-clip edits use composite identities and a common delta, committed atomically.
use super::{MidiSelectionEdit, Project, SamplePosition, TrackId};
use audio_types::AudioError;
use daw_midi::MidiClip;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiNoteTarget {
    pub track_id: u64,
    pub clip_id: u64,
    pub note_id: u64,
}

impl Project {
    pub fn add_midi_clip(
        &mut self,
        track_id: TrackId,
        start: u64,
        duration: u64,
    ) -> Result<(), AudioError> {
        if duration == 0
            || start
                .checked_add(duration)
                .is_none_or(|end| end > self.length_samples)
        {
            return Err(AudioError::InvalidConfig(
                "MIDI clip must be within the project",
            ));
        }
        let track = self.track_mut(track_id)?;
        if track.midi_clips.is_empty() {
            return Err(AudioError::InvalidConfig("select a MIDI track"));
        }
        let id = track
            .midi_clips
            .iter()
            .map(|clip| clip.id)
            .chain(track.clips.iter().map(|clip| clip.id))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| AudioError::Backend("clip identifiers exhausted".into()))?;
        track.midi_clips.push(MidiClip {
            id,
            timeline_start: SamplePosition(start),
            duration_samples: duration,
            notes: Vec::new(),
        });
        Ok(())
    }

    pub fn edit_midi_notes(
        &mut self,
        targets: &[MidiNoteTarget],
        edit: MidiSelectionEdit,
    ) -> Result<(), AudioError> {
        let unique: HashSet<_> = targets.iter().copied().collect();
        if unique.is_empty() || unique.len() != targets.len() {
            return Err(AudioError::InvalidConfig(
                "note targets must be nonempty and unique",
            ));
        }
        let mut groups: BTreeMap<(u64, u64), Vec<u64>> = BTreeMap::new();
        let (mut left, mut right, mut shorter, mut lower, mut higher) =
            (i128::MIN, i128::MAX, i128::MIN, i16::MIN, i16::MAX);
        for target in targets {
            let clip = self
                .tracks
                .iter()
                .find(|track| track.id.0 == target.track_id)
                .and_then(|track| {
                    track
                        .midi_clips
                        .iter()
                        .find(|clip| clip.id == target.clip_id)
                })
                .ok_or_else(|| AudioError::Backend("target MIDI clip was not found".into()))?;
            let note = clip
                .notes
                .iter()
                .find(|note| note.id == target.note_id)
                .ok_or_else(|| AudioError::Backend("target MIDI note was not found".into()))?;
            left = left.max(-i128::from(note.start.0));
            right = right.min(i128::from(
                clip.duration_samples - note.start.0 - note.duration_samples,
            ));
            shorter = shorter.max(1 - i128::from(note.duration_samples));
            lower = lower.max(-i16::from(note.key));
            higher = higher.min(127 - i16::from(note.key));
            groups
                .entry((target.track_id, target.clip_id))
                .or_default()
                .push(target.note_id);
        }
        let bounded = match edit {
            MidiSelectionEdit::Move {
                delta_samples,
                semitones,
            } => MidiSelectionEdit::Move {
                delta_samples: i64::try_from(i128::from(delta_samples).clamp(left, right))
                    .map_err(|_| AudioError::InvalidConfig("invalid sample delta"))?,
                semitones: semitones.clamp(lower, higher),
            },
            MidiSelectionEdit::Resize { delta_samples } => MidiSelectionEdit::Resize {
                delta_samples: i64::try_from(i128::from(delta_samples).clamp(shorter, right))
                    .map_err(|_| AudioError::InvalidConfig("invalid duration delta"))?,
            },
            other => other,
        };
        // Work on a candidate: invalid data in any clip leaves every other clip untouched.
        let mut candidate = self.clone();
        for ((track_id, clip_id), ids) in groups {
            if let MidiSelectionEdit::Quantize { division } = edit {
                if ![4, 8, 16, 32].contains(&division) {
                    return Err(AudioError::InvalidConfig("unsupported quantize division"));
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let grid = (f64::from(self.sample_rate) * 60.0 / self.tempo_bpm * 4.0
                    / f64::from(division))
                .round()
                .max(1.0) as u64;
                let clip = candidate
                    .track_mut(TrackId(track_id))?
                    .midi_clips
                    .iter_mut()
                    .find(|clip| clip.id == clip_id)
                    .ok_or_else(|| AudioError::Backend("MIDI clip was not found".into()))?;
                let selected_ids: HashSet<_> = ids.into_iter().collect();
                for note in &mut clip.notes {
                    if !selected_ids.contains(&note.id) {
                        continue;
                    }
                    let absolute = clip.timeline_start.0.saturating_add(note.start.0);
                    let snapped = absolute.saturating_add(grid / 2) / grid * grid;
                    note.start = SamplePosition(
                        snapped
                            .saturating_sub(clip.timeline_start.0)
                            .min(clip.duration_samples - note.duration_samples),
                    );
                }
                clip.notes.sort_by_key(|note| (note.start, note.id));
            } else {
                candidate.edit_midi_selection(TrackId(track_id), clip_id, &ids, bounded)?;
            }
        }
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(track_id: u64, clip_id: u64, note_id: u64) -> MidiNoteTarget {
        MidiNoteTarget {
            track_id,
            clip_id,
            note_id,
        }
    }
    #[test]
    fn composite_targets_and_different_clip_bounds_are_atomic() {
        let mut project = Project::demo(8_000).unwrap();
        project.add_midi_clip(TrackId(4), 2500, 10_000).unwrap();
        project
            .write_midi_note(TrackId(4), 4101, None, 500, 2000, 126)
            .unwrap();
        let mut other = project.tracks[3].clone();
        other.id = TrackId(5);
        project.tracks.push(other);
        let targets = [target(4, 4101, 1), target(5, 4101, 1)];
        project
            .write_midi_note(TrackId(5), 4101, Some(1), 1000, 1000, 124)
            .unwrap();
        let initial = project.snapshot();
        project
            .edit_midi_notes(
                &targets,
                MidiSelectionEdit::Move {
                    delta_samples: -1000,
                    semitones: 10,
                },
            )
            .unwrap();
        let a = &project.tracks[3].midi_clips[1].notes[0];
        let b = &project.tracks[4].midi_clips[1].notes[0];
        assert_eq!((a.start.0, b.start.0, a.key, b.key), (0, 500, 127, 125));
        assert_eq!(
            project.snapshot().tracks[3].midi_clips[0],
            initial.tracks[3].midi_clips[0]
        );
        project
            .edit_midi_notes(
                &targets,
                MidiSelectionEdit::Resize {
                    delta_samples: i64::MAX,
                },
            )
            .unwrap();
        assert_eq!(
            project.tracks[3].midi_clips[1].notes[0].duration_samples,
            10_000
        );
        assert_eq!(
            project.tracks[4].midi_clips[1].notes[0].duration_samples,
            9000
        );
        let before = project.snapshot();
        for invalid in [
            vec![target(4, 4101, 1), target(5, 999, 1)],
            vec![targets[0], targets[0]],
        ] {
            assert!(
                project
                    .edit_midi_notes(&invalid, MidiSelectionEdit::Delete)
                    .is_err()
            );
            assert_eq!(project.snapshot(), before);
        }
        assert!(
            project
                .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 0 })
                .is_err()
        );
        assert_eq!(project.snapshot(), before);
    }
    #[test]
    fn quantize_uses_project_time_and_retains_clip_ownership_and_duration() {
        let mut project = Project::demo(8_000).unwrap();
        project.add_midi_clip(TrackId(4), 2500, 10_000).unwrap();
        project.add_midi_clip(TrackId(4), 7000, 10_000).unwrap();
        project
            .write_midi_note(TrackId(4), 4101, None, 100, 2000, 60)
            .unwrap();
        project
            .write_midi_note(TrackId(4), 4102, None, 600, 1000, 64)
            .unwrap();
        let targets = [target(4, 4101, 1), target(4, 4102, 1)];
        project
            .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 16 })
            .unwrap();
        assert_eq!(project.tracks[3].midi_clips[1].notes[0].start.0, 500); // 3000 project samples
        assert_eq!(project.tracks[3].midi_clips[2].notes[0].start.0, 1000); // 8000 project samples
        assert_eq!(
            project.tracks[3].midi_clips[1].notes[0].duration_samples,
            2000
        );
        project
            .edit_midi_notes(&targets[..1], MidiSelectionEdit::Delete)
            .unwrap();
        assert_eq!(project.tracks[3].midi_clips[1].notes.len(), 0);
        assert_eq!(project.tracks[3].midi_clips[2].notes.len(), 1);
        let before = project.snapshot();
        assert!(project.add_midi_clip(TrackId(4), 63_999, 2).is_err());
        assert_eq!(project.snapshot(), before);
    }
}
