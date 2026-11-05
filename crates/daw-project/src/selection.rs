//! Atomic edits of an explicit set of notes in one MIDI clip.
use super::{Project, SamplePosition, TrackId};
use audio_types::AudioError;
use daw_midi::MidiNote;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MidiSelectionEdit {
    Move { delta_samples: i64, semitones: i16 },
    Resize { delta_samples: i64 },
    Delete,
    Quantize { division: u32 },
}

impl Project {
    pub fn edit_midi_selection(
        &mut self,
        track_id: TrackId,
        clip_id: u64,
        note_ids: &[u64],
        edit: MidiSelectionEdit,
    ) -> Result<(), AudioError> {
        let ids: HashSet<_> = note_ids.iter().copied().collect();
        if ids.is_empty() || ids.len() != note_ids.len() {
            return Err(AudioError::InvalidConfig(
                "note selection must be nonempty and unique",
            ));
        }
        let grid = if let MidiSelectionEdit::Quantize { division } = edit {
            if ![4, 8, 16, 32].contains(&division) {
                return Err(AudioError::InvalidConfig("unsupported quantize division"));
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let grid = (f64::from(self.sample_rate) * 60.0 / self.tempo_bpm * 4.0
                / f64::from(division))
            .round()
            .max(1.0) as u64;
            grid
        } else {
            1
        };
        let clip = self
            .track_mut(track_id)?
            .midi_clips
            .iter_mut()
            .find(|clip| clip.id == clip_id)
            .ok_or_else(|| AudioError::Backend("MIDI clip was not found".into()))?;
        let selected: Vec<_> = clip
            .notes
            .iter()
            .filter(|note| ids.contains(&note.id))
            .collect();
        if selected.len() != ids.len() {
            return Err(AudioError::Backend(
                "selected MIDI note was not found".into(),
            ));
        }
        // A common bounded delta preserves the relationships between selected notes.
        let (delta, pitch) = selection_delta(&selected, clip.duration_samples, edit);
        let mut candidate = clip.notes.clone();
        candidate
            .retain(|note| !matches!(edit, MidiSelectionEdit::Delete) || !ids.contains(&note.id));
        for note in &mut candidate {
            if !ids.contains(&note.id) {
                continue;
            }
            match edit {
                MidiSelectionEdit::Move { .. } => {
                    note.start = SamplePosition(
                        u64::try_from(i128::from(note.start.0) + delta)
                            .map_err(|_| AudioError::InvalidConfig("invalid note position"))?,
                    );
                    note.key = u8::try_from(i16::from(note.key) + pitch)
                        .map_err(|_| AudioError::InvalidConfig("invalid note pitch"))?;
                }
                MidiSelectionEdit::Resize { .. } => {
                    note.duration_samples =
                        u64::try_from(i128::from(note.duration_samples) + delta)
                            .map_err(|_| AudioError::InvalidConfig("invalid note duration"))?;
                }
                MidiSelectionEdit::Quantize { .. } => {
                    // Preserve duration, even for a long note near the end of the clip.
                    let snapped = note.start.0.saturating_add(grid / 2) / grid * grid;
                    note.start =
                        SamplePosition(snapped.min(clip.duration_samples - note.duration_samples));
                }
                MidiSelectionEdit::Delete => {}
            }
        }
        candidate.sort_by_key(|note| (note.start, note.id));
        clip.notes = candidate;
        Ok(())
    }
}

fn selection_delta(
    selected: &[&MidiNote],
    clip_duration: u64,
    edit: MidiSelectionEdit,
) -> (i128, i16) {
    let mut delta = 0_i128;
    let mut pitch = 0_i16;
    match edit {
        MidiSelectionEdit::Move {
            delta_samples,
            semitones,
        } => {
            let left = selected
                .iter()
                .map(|note| -i128::from(note.start.0))
                .max()
                .unwrap_or(0);
            let right = selected
                .iter()
                .map(|note| i128::from(clip_duration - note.start.0 - note.duration_samples))
                .min()
                .unwrap_or(0);
            delta = i128::from(delta_samples).clamp(left, right);
            let low = selected
                .iter()
                .map(|note| -i16::from(note.key))
                .max()
                .unwrap_or(0);
            let high = selected
                .iter()
                .map(|note| 127 - i16::from(note.key))
                .min()
                .unwrap_or(0);
            pitch = semitones.clamp(low, high);
        }
        MidiSelectionEdit::Resize { delta_samples } => {
            let left = selected
                .iter()
                .map(|note| 1 - i128::from(note.duration_samples))
                .max()
                .unwrap_or(0);
            let right = selected
                .iter()
                .map(|note| i128::from(clip_duration - note.start.0 - note.duration_samples))
                .min()
                .unwrap_or(0);
            delta = i128::from(delta_samples).clamp(left, right);
        }
        _ => {}
    }
    (delta, pitch)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_bounds_preserve_offsets_durations_and_unselected_notes() {
        let mut project = Project::demo(8_000).unwrap();
        project
            .write_midi_note(TrackId(4), 4100, None, 1000, 2000, 125)
            .unwrap();
        project
            .write_midi_note(TrackId(4), 4100, None, 5000, 500, 127)
            .unwrap();
        let original = project.snapshot();
        let ids = [4016, 4017];
        project
            .edit_midi_selection(
                TrackId(4),
                4100,
                &ids,
                MidiSelectionEdit::Move {
                    delta_samples: i64::MIN,
                    semitones: 10,
                },
            )
            .unwrap();
        let moved = project.snapshot();
        let notes = &moved.tracks[3].midi_clips[0].notes;
        let a = notes.iter().find(|n| n.id == 4016).unwrap();
        let b = notes.iter().find(|n| n.id == 4017).unwrap();
        assert_eq!((a.start.0, b.start.0, a.key, b.key), (0, 4000, 125, 127));
        project
            .edit_midi_selection(
                TrackId(4),
                4100,
                &ids,
                MidiSelectionEdit::Resize {
                    delta_samples: i64::MIN,
                },
            )
            .unwrap();
        let resized = project.snapshot();
        let notes = &resized.tracks[3].midi_clips[0].notes;
        assert_eq!(
            notes
                .iter()
                .find(|n| n.id == 4016)
                .unwrap()
                .duration_samples,
            1501
        );
        assert_eq!(
            notes
                .iter()
                .find(|n| n.id == 4017)
                .unwrap()
                .duration_samples,
            1
        );
        project
            .edit_midi_selection(
                TrackId(4),
                4100,
                &ids,
                MidiSelectionEdit::Move {
                    delta_samples: i64::MAX,
                    semitones: -200,
                },
            )
            .unwrap();
        let bounded = project.snapshot();
        let notes = &bounded.tracks[3].midi_clips[0].notes;
        let a = notes.iter().find(|n| n.id == 4016).unwrap();
        let b = notes.iter().find(|n| n.id == 4017).unwrap();
        assert_eq!((a.key, b.key), (0, 2));
        assert_eq!(b.start.0 - a.start.0, 4000);
        assert_eq!(b.start.0 + b.duration_samples, bounded.length_samples);
        for note in &original.tracks[3].midi_clips[0].notes {
            if !ids.contains(&note.id) {
                assert_eq!(notes.iter().find(|n| n.id == note.id).unwrap(), note);
            }
        }
    }

    #[test]
    fn invalid_selection_never_partially_edits_and_quantize_preserves_duration() {
        let mut project = Project::demo(8_000).unwrap();
        project
            .write_midi_note(TrackId(4), 4100, None, 1500, 62_000, 60)
            .unwrap();
        let before = project.snapshot();
        for ids in [&[][..], &[4016, 4016], &[4016, 999]] {
            assert!(
                project
                    .edit_midi_selection(TrackId(4), 4100, ids, MidiSelectionEdit::Delete)
                    .is_err()
            );
            assert_eq!(project.snapshot(), before);
        }
        assert!(
            project
                .edit_midi_selection(
                    TrackId(4),
                    4100,
                    &[4016],
                    MidiSelectionEdit::Quantize { division: 0 }
                )
                .is_err()
        );
        assert_eq!(project.snapshot(), before);
        project
            .edit_midi_selection(
                TrackId(4),
                4100,
                &[4016],
                MidiSelectionEdit::Quantize { division: 4 },
            )
            .unwrap();
        let snapshot = project.snapshot();
        let note = snapshot.tracks[3].midi_clips[0]
            .notes
            .iter()
            .find(|n| n.id == 4016)
            .unwrap();
        assert_eq!((note.start.0, note.duration_samples), (0, 62_000));
        project
            .edit_midi_selection(TrackId(4), 4100, &[4000, 4016], MidiSelectionEdit::Delete)
            .unwrap();
        assert_eq!(project.snapshot().tracks[3].midi_clips[0].notes.len(), 15);
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;
    #[test]
    fn commands_use_the_same_camel_case_shape_as_the_webview() {
        let command: MidiSelectionEdit =
            serde_json::from_str(r#"{"type":"move","deltaSamples":-100,"semitones":2}"#).unwrap();
        assert!(matches!(
            command,
            MidiSelectionEdit::Move {
                delta_samples: -100,
                semitones: 2
            }
        ));
        let command: MidiSelectionEdit =
            serde_json::from_str(r#"{"type":"resize","deltaSamples":100}"#).unwrap();
        assert!(matches!(
            command,
            MidiSelectionEdit::Resize { delta_samples: 100 }
        ));
        assert!(serde_json::from_str::<MidiSelectionEdit>(r#"{"type":"unknown"}"#).is_err());
    }
}
