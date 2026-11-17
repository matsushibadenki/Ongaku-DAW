use super::*;

#[test]
fn selection_and_cross_clip_quantize_use_exact_local_and_project_origins() {
    let mut project = Project::demo(44_100).unwrap();
    project
        .write_midi_note(TrackId(4), 4100, Some(4000), 88_200, 1000, 60)
        .unwrap();
    let before = project.snapshot();
    project
        .edit_midi_selection(
            TrackId(4),
            4100,
            &[4000],
            MidiSelectionEdit::Quantize { division: 16 },
        )
        .unwrap();
    assert_eq!(project.snapshot(), before); // Rounded period formerly moved this by eight samples.
    for (start, duration) in [(1001, 100_000), (60_007, 40_000)] {
        project.add_midi_clip(TrackId(4), start, duration).unwrap();
    }
    for (clip, start) in [(4101, 1001), (4102, 60_007)] {
        project
            .write_midi_note(TrackId(4), clip, None, 88_100 - start, 1000, 64)
            .unwrap();
        project
            .write_midi_note(TrackId(4), clip, None, 0, 500, 67)
            .unwrap();
    }
    let targets = [4101, 4102].map(|clip_id| MidiNoteTarget {
        track_id: 4,
        clip_id,
        note_id: 1,
    });
    let before = project.snapshot();
    project
        .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 16 })
        .unwrap();
    let after = project.snapshot();
    assert_eq!(
        after.tracks[3].midi_clips[0],
        before.tracks[3].midi_clips[0]
    );
    for index in [1, 2] {
        let clip = &after.tracks[3].midi_clips[index];
        let note = clip.notes.iter().find(|note| note.id == 1).unwrap();
        let old = before.tracks[3].midi_clips[index]
            .notes
            .iter()
            .find(|note| note.id == 1)
            .unwrap();
        assert_eq!(clip.timeline_start.0 + note.start.0, 88_200);
        assert_eq!(note.duration_samples, old.duration_samples);
        assert_eq!(note.key, old.key);
        assert_eq!(note.velocity.to_bits(), old.velocity.to_bits());
        assert_eq!(note.channel, old.channel);
        assert_eq!(
            clip.notes.iter().find(|note| note.id == 2),
            before.tracks[3].midi_clips[index]
                .notes
                .iter()
                .find(|note| note.id == 2)
        );
    }
    project
        .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 16 })
        .unwrap();
    assert_eq!(project.snapshot(), after);
    for targets in [
        vec![
            targets[0],
            MidiNoteTarget {
                note_id: 999,
                ..targets[1]
            },
        ],
        vec![targets[0], targets[0]],
    ] {
        assert!(
            project
                .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 16 })
                .is_err()
        );
        assert_eq!(project.snapshot(), after);
    }
    assert!(
        project
            .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 7 })
            .is_err()
    );
    assert!(
        project
            .edit_midi_selection(
                TrackId(4),
                4100,
                &[4000],
                MidiSelectionEdit::Quantize { division: 7 }
            )
            .is_err()
    );
    assert_eq!(project.snapshot(), after);
}

#[test]
fn quantize_preserves_duration_at_both_clip_edges_even_without_an_interior_grid_point() {
    let mut project = Project::demo(44_100).unwrap();
    project.add_midi_clip(TrackId(4), 1001, 1000).unwrap();
    project
        .write_midi_note(TrackId(4), 4101, None, 1, 999, 60)
        .unwrap();
    let targets = [MidiNoteTarget {
        track_id: 4,
        clip_id: 4101,
        note_id: 1,
    }];
    project
        .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 16 })
        .unwrap();
    let clip = &project.snapshot().tracks[3].midi_clips[1];
    assert_eq!(clip.notes[0].start.0, 0);
    assert_eq!(clip.notes[0].duration_samples, 999);
    let before = project.snapshot();
    project
        .edit_midi_notes(&targets, MidiSelectionEdit::Quantize { division: 16 })
        .unwrap();
    assert_eq!(project.snapshot(), before);
}
