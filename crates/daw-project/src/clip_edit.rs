//! Arrangement edits are prepared outside the realtime thread and committed as a unit.
use super::{Project, SamplePosition, TrackId};
use audio_types::AudioError;
use daw_midi::MidiClip;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiClipTarget {
    pub track_id: u64,
    pub clip_id: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MidiClipEdit {
    Move { delta_samples: i64 },
    Duplicate { delta_samples: i64 },
    TrimLeft { delta_samples: i64 },
    TrimRight { delta_samples: i64 },
    Split { sample: u64 },
}

fn invalid() -> AudioError {
    AudioError::InvalidConfig("invalid MIDI clip edit")
}
fn bounded_delta(clips: &[MidiClip], length: u64, edit: MidiClipEdit) -> Result<i128, AudioError> {
    let requested = match edit {
        MidiClipEdit::Move { delta_samples }
        | MidiClipEdit::Duplicate { delta_samples }
        | MidiClipEdit::TrimLeft { delta_samples }
        | MidiClipEdit::TrimRight { delta_samples } => i128::from(delta_samples),
        MidiClipEdit::Split { .. } => return Ok(0),
    };
    let left = clips
        .iter()
        .map(|clip| {
            if matches!(edit, MidiClipEdit::TrimRight { .. }) {
                1 - i128::from(clip.duration_samples)
            } else {
                -i128::from(clip.timeline_start.0)
            }
        })
        .max()
        .ok_or_else(invalid)?;
    let right = clips
        .iter()
        .map(|clip| {
            if matches!(edit, MidiClipEdit::TrimLeft { .. }) {
                i128::from(clip.duration_samples) - 1
            } else {
                i128::from(length - clip.timeline_start.0 - clip.duration_samples)
            }
        })
        .min()
        .ok_or_else(invalid)?;
    Ok(requested.clamp(left, right))
}

/// Keeps the intersection with the new range, preserving absolute note times and expression.
fn cropped(clip: &MidiClip, start: u64, end: u64, id: u64) -> MidiClip {
    let mut notes: Vec<_> = clip
        .notes
        .iter()
        .filter_map(|original| {
            let original_start = clip.timeline_start.0 + original.start.0;
            let left = original_start.max(start);
            let right = (original_start + original.duration_samples).min(end);
            (left < right).then(|| {
                let mut note = *original;
                note.start = SamplePosition(left - start);
                note.duration_samples = right - left;
                note
            })
        })
        .collect();
    notes.sort_by_key(|note| (note.start, note.id));
    MidiClip {
        id,
        timeline_start: SamplePosition(start),
        duration_samples: end - start,
        notes,
    }
}

impl Project {
    pub fn edit_midi_clips(
        &mut self,
        targets: &[MidiClipTarget],
        edit: MidiClipEdit,
    ) -> Result<(), AudioError> {
        let unique: HashSet<_> = targets.iter().copied().collect();
        if unique.is_empty() || unique.len() != targets.len() {
            return Err(invalid());
        }
        let originals: Vec<_> = targets
            .iter()
            .map(|target| {
                self.tracks
                    .iter()
                    .find(|track| track.id.0 == target.track_id)
                    .and_then(|track| {
                        track
                            .midi_clips
                            .iter()
                            .find(|clip| clip.id == target.clip_id)
                    })
                    .cloned()
                    .ok_or_else(invalid)
            })
            .collect::<Result<_, _>>()?;
        if let MidiClipEdit::Split { sample } = edit
            && !originals.iter().any(|clip| {
                sample > clip.timeline_start.0
                    && sample < clip.timeline_start.0 + clip.duration_samples
            })
        {
            return Err(invalid());
        }
        let delta = bounded_delta(&originals, self.length_samples, edit)?;
        let mut candidate = self.clone();
        for (target, original) in targets.iter().zip(originals) {
            let track = candidate.track_mut(TrackId(target.track_id))?;
            let index = track
                .midi_clips
                .iter()
                .position(|clip| clip.id == target.clip_id)
                .ok_or_else(invalid)?;
            let start = original.timeline_start.0;
            let end = start + original.duration_samples;
            match edit {
                MidiClipEdit::Move { .. } => {
                    track.midi_clips[index].timeline_start = SamplePosition(
                        u64::try_from(i128::from(start) + delta).map_err(|_| invalid())?,
                    );
                }
                MidiClipEdit::Duplicate { .. } => {
                    let id = track
                        .midi_clips
                        .iter()
                        .map(|clip| clip.id)
                        .chain(track.clips.iter().map(|clip| clip.id))
                        .max()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(invalid)?;
                    let mut copy = original;
                    copy.id = id;
                    copy.timeline_start = SamplePosition(
                        u64::try_from(i128::from(start) + delta).map_err(|_| invalid())?,
                    );
                    track.midi_clips.push(copy);
                }
                MidiClipEdit::TrimLeft { .. } => {
                    let new_start =
                        u64::try_from(i128::from(start) + delta).map_err(|_| invalid())?;
                    track.midi_clips[index] = cropped(&original, new_start, end, original.id);
                }
                MidiClipEdit::TrimRight { .. } => {
                    let new_end = u64::try_from(i128::from(end) + delta).map_err(|_| invalid())?;
                    track.midi_clips[index] = cropped(&original, start, new_end, original.id);
                }
                MidiClipEdit::Split { sample } if sample > start && sample < end => {
                    let id = track
                        .midi_clips
                        .iter()
                        .map(|clip| clip.id)
                        .chain(track.clips.iter().map(|clip| clip.id))
                        .max()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(invalid)?;
                    track.midi_clips[index] = cropped(&original, start, sample, original.id);
                    track.midi_clips.push(cropped(&original, sample, end, id));
                }
                MidiClipEdit::Split { .. } => {}
            }
        }
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_types::AudioConfig;
    use daw_core::transport::Transport;
    fn target(id: u64) -> MidiClipTarget {
        MidiClipTarget {
            track_id: 4,
            clip_id: id,
        }
    }
    fn fixture() -> Project {
        let mut project = Project::demo(8000).unwrap();
        project.add_midi_clip(TrackId(4), 1000, 10000).unwrap();
        for (start, length, key) in [
            (0, 1000, 60),
            (1000, 6000, 64),
            (7000, 1000, 67),
            (9500, 500, 72),
        ] {
            project
                .write_midi_note(TrackId(4), 4101, None, start, length, key)
                .unwrap();
        }
        project
    }
    #[test]
    fn split_cuts_crossing_notes_and_keeps_exclusive_edges() {
        let mut project = fixture();
        project
            .edit_midi_clips(&[target(4101)], MidiClipEdit::Split { sample: 5000 })
            .unwrap();
        let clips = &project.tracks[3].midi_clips;
        let left = &clips[1];
        let right = &clips[2];
        assert_eq!(
            (left.id, left.timeline_start.0, left.duration_samples),
            (4101, 1000, 4000)
        );
        assert_eq!(
            (right.id, right.timeline_start.0, right.duration_samples),
            (4102, 5000, 6000)
        );
        assert_eq!(
            left.notes
                .iter()
                .map(|n| (n.id, n.start.0, n.duration_samples))
                .collect::<Vec<_>>(),
            vec![(1, 0, 1000), (2, 1000, 3000)]
        );
        assert_eq!(
            right
                .notes
                .iter()
                .map(|n| (n.id, n.start.0, n.duration_samples))
                .collect::<Vec<_>>(),
            vec![(2, 0, 3000), (3, 3000, 1000), (4, 5500, 500)]
        );
        let mut graph = project
            .build_audio_graph(AudioConfig {
                sample_rate: 8000,
                ..AudioConfig::DEFAULT
            })
            .unwrap();
        graph.process(&mut [0.0; 256], &Transport::new(8000));
    }
    #[test]
    fn trimming_preserves_absolute_note_time_and_expression() {
        let mut project = fixture();
        project
            .set_midi_note_velocity(TrackId(4), 4101, 2, 0.32)
            .unwrap();
        project
            .edit_midi_clips(
                &[target(4101)],
                MidiClipEdit::TrimLeft {
                    delta_samples: 2000,
                },
            )
            .unwrap();
        let clip = &project.tracks[3].midi_clips[1];
        assert_eq!((clip.timeline_start.0, clip.duration_samples), (3000, 8000));
        assert_eq!(
            clip.notes
                .iter()
                .map(|n| (n.id, n.start.0, n.duration_samples))
                .collect::<Vec<_>>(),
            vec![(2, 0, 5000), (3, 5000, 1000), (4, 7500, 500)]
        );
        assert!((clip.notes[0].velocity - 0.32).abs() < f32::EPSILON);
        project
            .edit_midi_clips(
                &[target(4101)],
                MidiClipEdit::TrimRight {
                    delta_samples: -3000,
                },
            )
            .unwrap();
        assert_eq!(project.tracks[3].midi_clips[1].notes.len(), 1);
        project
            .edit_midi_clips(
                &[target(4101)],
                MidiClipEdit::TrimLeft {
                    delta_samples: -1000,
                },
            )
            .unwrap();
        let clip = &project.tracks[3].midi_clips[1];
        assert_eq!(
            (
                clip.timeline_start.0,
                clip.notes[0].start.0,
                clip.notes[0].duration_samples
            ),
            (2000, 1000, 5000)
        );
    }
    #[test]
    fn group_move_and_duplicate_preserve_offsets_and_independence() {
        let mut project = fixture();
        project.add_midi_clip(TrackId(4), 7000, 10000).unwrap();
        project
            .edit_midi_clips(
                &[target(4101), target(4102)],
                MidiClipEdit::Move {
                    delta_samples: i64::MIN,
                },
            )
            .unwrap();
        assert_eq!(
            (
                project.tracks[3].midi_clips[1].timeline_start.0,
                project.tracks[3].midi_clips[2].timeline_start.0
            ),
            (0, 6000)
        );
        project
            .edit_midi_clips(
                &[target(4101), target(4102)],
                MidiClipEdit::Duplicate {
                    delta_samples: 16000,
                },
            )
            .unwrap();
        let copies = &project.tracks[3].midi_clips[3..];
        assert_eq!(
            (
                copies[0].id,
                copies[0].timeline_start.0,
                copies[1].id,
                copies[1].timeline_start.0
            ),
            (4103, 16000, 4104, 22000)
        );
        let note_snapshot = copies[0].notes.clone();
        project
            .set_midi_note_velocity(TrackId(4), 4101, 1, 0.1)
            .unwrap();
        assert_eq!(project.tracks[3].midi_clips[3].notes, note_snapshot);
        project
            .edit_midi_clips(
                &[target(4101), target(4102)],
                MidiClipEdit::Move {
                    delta_samples: i64::MAX,
                },
            )
            .unwrap();
        assert_eq!(
            project.tracks[3].midi_clips[2].timeline_start.0 + 10000,
            64000
        );
    }
    #[test]
    fn invalid_targets_and_split_points_do_not_mutate_any_clip() {
        let mut project = fixture();
        let before = project.snapshot();
        for targets in [
            vec![],
            vec![target(4101), target(4101)],
            vec![target(4101), target(999)],
        ] {
            assert!(
                project
                    .edit_midi_clips(
                        &targets,
                        MidiClipEdit::Duplicate {
                            delta_samples: 1000
                        }
                    )
                    .is_err()
            );
            assert_eq!(project.snapshot(), before);
        }
        for sample in [0, 1000, 11000, u64::MAX] {
            assert!(
                project
                    .edit_midi_clips(&[target(4101)], MidiClipEdit::Split { sample })
                    .is_err()
            );
            assert_eq!(project.snapshot(), before);
        }
        project
            .edit_midi_clips(
                &[target(4101)],
                MidiClipEdit::TrimLeft {
                    delta_samples: i64::MAX,
                },
            )
            .unwrap();
        assert_eq!(project.tracks[3].midi_clips[1].duration_samples, 1);
        project
            .edit_midi_clips(
                &[target(4101)],
                MidiClipEdit::TrimRight {
                    delta_samples: i64::MIN,
                },
            )
            .unwrap();
        assert_eq!(project.tracks[3].midi_clips[1].duration_samples, 1);
    }
}
