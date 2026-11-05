use super::{Project, TrackId};
use audio_types::AudioError;
use daw_mixer::{MixerEvent, validate_mixer_events};

impl Project {
    /// Insert or replace by stable ID. Invalid edits leave the document untouched.
    pub fn set_mixer_event(
        &mut self,
        track_id: TrackId,
        event: MixerEvent,
    ) -> Result<(), AudioError> {
        if event.position.0 >= self.length_samples {
            return Err(AudioError::InvalidConfig(
                "mixer event must be inside the project",
            ));
        }
        let track = self.track_mut(track_id)?;
        let mut events = track.mixer_events.clone();
        if let Some(existing) = events.iter_mut().find(|existing| existing.id == event.id) {
            *existing = event;
        } else {
            events.push(event);
        }
        validate_mixer_events(&events)?;
        events.sort_unstable_by_key(|event| (event.position, event.id));
        track.mixer_events = events;
        Ok(())
    }

    pub fn remove_mixer_event(
        &mut self,
        track_id: TrackId,
        event_id: u64,
    ) -> Result<(), AudioError> {
        let track = self.track_mut(track_id)?;
        let index = track
            .mixer_events
            .iter()
            .position(|event| event.id == event_id)
            .ok_or(AudioError::InvalidConfig("mixer event not found"))?;
        track.mixer_events.remove(index);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::SamplePosition;
    use daw_mixer::MixerChange;

    #[test]
    fn scheduled_controls_roundtrip_as_v2_and_legacy_v1_defaults_to_empty() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.ongaku");
        let mut project = Project::demo(8000).unwrap();
        project.save(&path).unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(legacy["version"], 1);
        for track in legacy["project"]["tracks"].as_array_mut().unwrap() {
            track.as_object_mut().unwrap().remove("mixerEvents");
        }
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(Project::load(&path).unwrap().snapshot(), project.snapshot());
        for (id, change) in [
            MixerChange::Gain(0.2),
            MixerChange::Pan(-0.5),
            MixerChange::Mute(true),
            MixerChange::Solo(true),
        ]
        .into_iter()
        .enumerate()
        {
            project
                .set_mixer_event(
                    TrackId(4),
                    MixerEvent {
                        id: u64::try_from(id).unwrap(),
                        position: SamplePosition(129),
                        change,
                    },
                )
                .unwrap();
        }
        project.save(&path).unwrap();
        let mut document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(document["version"], 2);
        assert_eq!(Project::load(&path).unwrap().snapshot(), project.snapshot());
        document["project"]["tracks"][3]["mixerEvents"][0]["position"] =
            serde_json::json!(u64::MAX);
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(Project::load(&path).is_err());
    }

    #[test]
    fn invalid_event_edit_leaves_project_intact() {
        let mut project = Project::demo(8000).unwrap();
        let before = project.snapshot();
        for event in [
            MixerEvent {
                id: 1,
                position: SamplePosition(0),
                change: MixerChange::Pan(f32::NAN),
            },
            MixerEvent {
                id: 1,
                position: SamplePosition(u64::MAX),
                change: MixerChange::Mute(true),
            },
        ] {
            assert!(project.set_mixer_event(TrackId(4), event).is_err());
            assert_eq!(project.snapshot(), before);
        }
        assert!(project.remove_mixer_event(TrackId(4), 1).is_err());
        assert_eq!(project.snapshot(), before);
    }
}

impl Project {
    /// Earliest changed schedule-definition sample, including removals and moves.
    /// Control-thread maps keep comparison linear in the number of definitions.
    #[must_use]
    pub fn mixer_edit_deadline(&self, next: &Self) -> Option<daw_core::SamplePosition> {
        use std::collections::HashMap;
        let collect = |project: &Self| {
            project
                .tracks
                .iter()
                .flat_map(|track| {
                    track
                        .mixer_events
                        .iter()
                        .map(move |event| ((track.id, event.id), *event))
                })
                .collect::<HashMap<_, _>>()
        };
        let previous = collect(self);
        let following = collect(next);
        previous
            .iter()
            .filter(|(id, event)| following.get(id) != Some(event))
            .map(|(_, event)| event.position)
            .chain(
                following
                    .iter()
                    .filter(|(id, event)| previous.get(id) != Some(event))
                    .map(|(_, event)| event.position),
            )
            .min()
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use daw_core::SamplePosition;
    use daw_mixer::MixerChange;

    #[test]
    fn deadlines_include_removed_positions_and_ignore_unrelated_graph_edits() {
        let original = Project::demo(8000).unwrap();
        let mut scheduled = original.clone();
        let event = MixerEvent {
            id: 1,
            position: SamplePosition(129),
            change: MixerChange::Mute(true),
        };
        scheduled.set_mixer_event(TrackId(4), event).unwrap();
        assert_eq!(
            original.mixer_edit_deadline(&scheduled),
            Some(SamplePosition(129))
        );
        assert_eq!(
            scheduled.mixer_edit_deadline(&original),
            Some(SamplePosition(129))
        );
        let mut moved = scheduled.clone();
        moved
            .set_mixer_event(
                TrackId(4),
                MixerEvent {
                    position: SamplePosition(511),
                    ..event
                },
            )
            .unwrap();
        assert_eq!(
            scheduled.mixer_edit_deadline(&moved),
            Some(SamplePosition(129))
        );
        let mut unrelated = scheduled.clone();
        unrelated.set_track_gain(TrackId(1), 0.4).unwrap();
        assert_eq!(scheduled.mixer_edit_deadline(&unrelated), None);
    }
}

#[cfg(test)]
mod audible_deadline_tests {
    use super::*;
    use audio_types::AudioConfig;
    use daw_core::SamplePosition;
    use daw_mixer::MixerChange;
    #[test]
    fn prepared_callback_deadlines_match_independent_document_map_diffs() {
        let mut previous = Project::demo(8000).unwrap();
        for event in [
            MixerEvent {
                id: 8,
                position: SamplePosition(511),
                change: MixerChange::Gain(0.4),
            },
            MixerEvent {
                id: 1,
                position: SamplePosition(129),
                change: MixerChange::Mute(true),
            },
            MixerEvent {
                id: 12,
                position: SamplePosition(64),
                change: MixerChange::Pan(-0.2),
            },
            MixerEvent {
                id: 2,
                position: SamplePosition(17),
                change: MixerChange::Solo(true),
            },
        ] {
            previous.set_mixer_event(TrackId(4), event).unwrap();
        }
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        let audible = previous.build_audio_graph(config).unwrap();
        for variant in 0..5 {
            let mut next = previous.clone();
            match variant {
                0 => next
                    .set_mixer_event(
                        TrackId(4),
                        MixerEvent {
                            id: 8,
                            position: SamplePosition(65),
                            change: MixerChange::Gain(0.8),
                        },
                    )
                    .unwrap(),
                1 => next.remove_mixer_event(TrackId(4), 1).unwrap(),
                2 => next
                    .set_mixer_event(
                        TrackId(1),
                        MixerEvent {
                            id: 1,
                            position: SamplePosition(151),
                            change: MixerChange::Pan(0.5),
                        },
                    )
                    .unwrap(),
                3 => next
                    .set_mixer_event(
                        TrackId(4),
                        MixerEvent {
                            id: 12,
                            position: SamplePosition(64),
                            change: MixerChange::Mute(false),
                        },
                    )
                    .unwrap(),
                _ => {
                    next.reorder_tracks(&[TrackId(4), TrackId(1), TrackId(3), TrackId(2)])
                        .unwrap();
                }
            }
            let prepared = next.build_audio_graph(config).unwrap();
            assert_eq!(
                prepared.mixer_edit_deadline(&audible),
                previous.mixer_edit_deadline(&next)
            );
        }
    }
}

#[cfg(test)]
mod canceled_build_tests {
    use super::*;
    use audio_types::{AudioConfig, PreparationCheck};
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
    fn partial_graph_preparation_is_cancelable_without_changing_the_document() {
        let project = Project::demo(8000).unwrap();
        let before = project.snapshot();
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        for budget in [0, 2, 20] {
            assert!(matches!(
                project.build_audio_graph_cancellable(config, &Budget(Cell::new(budget))),
                Err(AudioError::PreparationCanceled)
            ));
            assert_eq!(project.snapshot(), before);
        }
    }
}
