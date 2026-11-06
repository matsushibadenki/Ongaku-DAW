//! Non-realtime application/control layer reusable by Tauri, CLI, or native UIs.

mod preparation;
use preparation::PreparationService;
pub use preparation::{
    PreparationFailure, PreparationOutcome, PreparationOutcomeKind, PreparationStatus,
};

use audio_backend::{AudioBackend, AudioOutputStream};
use audio_engine::{EngineControl, EngineGraph, EngineProcessor};
use audio_types::{AudioConfig, AudioDeviceInfo, AudioError, RealtimeMetricsSnapshot};
use daw_core::transport::{TransportCommand, TransportSnapshot};
use daw_mixer::AudioGraph;
use daw_mixer::TrackId;
use daw_project::{MidiSelectionEdit, Project, ProjectSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineState {
    #[default]
    Stopped,
    Running,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentStatus {
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: bool,
    pub path: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationStatus {
    pub stream_readiness: Option<audio_engine::StreamReadinessSnapshot>,
    pub streaming: daw_project::ProjectStreamingStatus,
    pub document_revision: u64,
    pub requested_revision: Option<u64>,
    pub engine_epoch: u64,
    pub engine: Option<audio_engine::GraphPublicationStatus>,
    pub preparation: Option<PreparationStatus>,
}

pub struct DawApplication {
    stream: Option<Box<dyn AudioOutputStream>>,
    control: Option<EngineControl>,
    state: EngineState,
    project: Project,
    active_config: Option<AudioConfig>,
    undo: Vec<(Project, u64)>,
    redo: Vec<(Project, u64)>,
    revision: u64,
    next_revision: u64,
    saved_revision: Option<u64>,
    path: Option<std::path::PathBuf>,
    last_edit: Option<(String, std::time::Instant)>,
    requested_revision: Option<u64>,
    engine_epoch: u64,
    background_preparation: bool,
    preparation: Option<PreparationService>,
}

impl DawApplication {
    /// Creates the application with the validated built-in project defaults.
    ///
    /// # Panics
    ///
    /// Panics if the compile-time default audio configuration becomes invalid.
    #[must_use]
    pub fn new() -> Self {
        Self::with_sample_rate(AudioConfig::DEFAULT.sample_rate)
            .expect("default project configuration is valid")
    }

    /// Desktop mode: edits commit document/history immediately and prepare audio on one worker.
    #[must_use]
    pub fn new_with_background_preparation() -> Self {
        let mut app = Self::new();
        app.background_preparation = true;
        app
    }

    pub fn with_sample_rate(sample_rate: u32) -> Result<Self, AudioError> {
        Ok(Self {
            stream: None,
            control: None,
            state: EngineState::Stopped,
            project: Project::demo(sample_rate)?,
            active_config: None,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            next_revision: 1,
            saved_revision: None,
            path: None,
            last_edit: None,
            requested_revision: None,
            engine_epoch: 0,
            background_preparation: false,
            preparation: None,
        })
    }

    pub fn output_devices(backend: &dyn AudioBackend) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        backend.output_devices()
    }

    pub fn default_config(backend: &dyn AudioBackend) -> Result<AudioConfig, AudioError> {
        backend.default_output_config()
    }

    pub fn start(
        &mut self,
        backend: &dyn AudioBackend,
        device_id: Option<&str>,
        config: AudioConfig,
    ) -> Result<(), AudioError> {
        if self.state == EngineState::Running {
            return Ok(());
        }
        if self.project.snapshot().sample_rate != config.sample_rate {
            return Err(AudioError::InvalidConfig(
                "output sample rate must match the project",
            ));
        }
        let next_epoch = self
            .engine_epoch
            .checked_add(1)
            .filter(|epoch| *epoch <= audio_engine::MAX_PUBLICATION_REVISION)
            .ok_or(AudioError::InvalidState("engine epochs exhausted"))?;
        let graph = self.project.build_audio_graph(config)?;
        let (processor, control) = EngineProcessor::prepare(config)?;
        let requested_revision =
            control.submit_graph_revision(EngineGraph::timeline(graph), self.revision, None)?;
        let project = self.project.snapshot();
        control.send_transport(TransportCommand::SetTempo(project.tempo_bpm))?;
        control.send_transport(TransportCommand::SetTimeSignature(project.time_signature))?;
        let preparation = if self.background_preparation {
            Some(PreparationService::spawn(control.clone(), config)?)
        } else {
            None
        };
        let mut stream = backend.open_output(device_id, config, Box::new(processor))?;
        stream.start()?;
        self.stream = Some(stream);
        self.control = Some(control);
        self.state = EngineState::Running;
        self.active_config = Some(config);
        self.requested_revision = Some(requested_revision);
        self.engine_epoch = next_epoch;
        self.preparation = preparation;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        if let Some(stream) = &mut self.stream {
            stream.stop()?;
        }
        self.preparation = None; // Cooperative cancellation and join are control-thread work.
        self.stream = None;
        self.control = None;
        self.state = EngineState::Stopped;
        self.active_config = None;
        self.requested_revision = None;
        Ok(())
    }

    pub fn set_gain(&self, linear_gain: f32) -> Result<(), AudioError> {
        let control = self.control()?;
        control.set_gain(linear_gain);
        Ok(())
    }

    /// Submits a fully prepared graph for atomic adoption at the next audio block boundary.
    pub fn submit_audio_graph(&self, graph: AudioGraph) -> Result<(), AudioError> {
        self.control()?.submit_graph(EngineGraph::timeline(graph))
    }

    pub fn send_transport(&self, command: TransportCommand) -> Result<(), AudioError> {
        self.control()?.send_transport(command)
    }

    pub fn transport(&self) -> Result<TransportSnapshot, AudioError> {
        Ok(self.control()?.transport())
    }

    #[must_use]
    pub fn project(&self) -> ProjectSnapshot {
        self.project.snapshot()
    }

    pub fn configure_project(&mut self, sample_rate: u32) -> Result<ProjectSnapshot, AudioError> {
        if self.state == EngineState::Running {
            return Err(AudioError::InvalidState(
                "stop the engine before changing project sample rate",
            ));
        }
        if self.project.snapshot().sample_rate != sample_rate {
            return Err(AudioError::InvalidConfig(
                "project sample rate cannot be changed without resampling",
            ));
        }
        Ok(self.project.snapshot())
    }

    fn edit(
        &mut self,
        key: Option<String>,
        change: impl FnOnce(&mut Project) -> Result<(), AudioError>,
    ) -> Result<(), AudioError> {
        let mut candidate = self.project.clone();
        change(&mut candidate)?;
        if candidate.snapshot() == self.project.snapshot() {
            return Ok(());
        }
        if self.next_revision > audio_engine::MAX_PUBLICATION_REVISION {
            return Err(AudioError::InvalidState("document revisions exhausted"));
        }
        let next_revision = self
            .next_revision
            .checked_add(1)
            .ok_or(AudioError::InvalidState("document revisions exhausted"))?;
        let requested_revision = self.publish_candidate(&candidate, self.next_revision)?;
        let now = std::time::Instant::now();
        let coalesce = key.as_ref().is_some_and(|key| {
            self.last_edit.as_ref().is_some_and(|(previous, time)| {
                previous == key && now.duration_since(*time).as_millis() < 500
            })
        });
        if !coalesce {
            self.undo.push((self.project.clone(), self.revision));
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.project = candidate;
        self.revision = self.next_revision;
        self.next_revision = next_revision;
        self.requested_revision = requested_revision;
        self.last_edit = key.map(|key| (key, now));
        Ok(())
    }

    #[must_use]
    pub fn document_status(&self) -> DocumentStatus {
        DocumentStatus {
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            dirty: self.saved_revision != Some(self.revision),
            path: self.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        }
    }

    pub fn undo(&mut self) -> Result<ProjectSnapshot, AudioError> {
        if let Some((project, revision)) = self.undo.last() {
            let requested_revision = self.publish_candidate(project, *revision)?;
            self.requested_revision = requested_revision;
            let (project, revision) = (project.clone(), *revision);
            self.undo.pop();
            self.redo
                .push((std::mem::replace(&mut self.project, project), self.revision));
            self.revision = revision;
        }
        self.last_edit = None;
        Ok(self.project())
    }

    pub fn redo(&mut self) -> Result<ProjectSnapshot, AudioError> {
        if let Some((project, revision)) = self.redo.last() {
            let requested_revision = self.publish_candidate(project, *revision)?;
            self.requested_revision = requested_revision;
            let (project, revision) = (project.clone(), *revision);
            self.redo.pop();
            self.undo
                .push((std::mem::replace(&mut self.project, project), self.revision));
            self.revision = revision;
        }
        self.last_edit = None;
        Ok(self.project())
    }

    pub fn save_project(&mut self, path: &std::path::Path) -> Result<DocumentStatus, AudioError> {
        self.project.save(path)?;
        self.path = Some(path.to_owned());
        self.saved_revision = Some(self.revision);
        self.last_edit = None;
        Ok(self.document_status())
    }

    pub fn load_project(&mut self, path: &std::path::Path) -> Result<ProjectSnapshot, AudioError> {
        let candidate = Project::load(path)?;
        if self.next_revision > audio_engine::MAX_PUBLICATION_REVISION {
            return Err(AudioError::InvalidState("document revisions exhausted"));
        }
        // A complete document replacement stops playback before adopting its clock and media.
        self.stop()?;
        self.project = candidate;
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.revision = self.next_revision;
        self.next_revision += 1;
        self.saved_revision = Some(self.revision);
        self.path = Some(path.to_owned());
        Ok(self.project())
    }

    /// Persist a timeline event; the normal atomic edit/history/publication path owns it.
    pub fn set_mixer_event(
        &mut self,
        track_id: u64,
        event: daw_mixer::MixerEvent,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |project| {
            project.set_mixer_event(TrackId(track_id), event)
        })?;
        Ok(self.project())
    }

    pub fn remove_mixer_event(
        &mut self,
        track_id: u64,
        event_id: u64,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |project| {
            project.remove_mixer_event(TrackId(track_id), event_id)
        })?;
        Ok(self.project())
    }

    pub fn set_track_gain(&mut self, track_id: u64, gain: f32) -> Result<(), AudioError> {
        self.edit(Some(format!("gain:{track_id}")), |p| {
            p.set_track_gain(TrackId(track_id), gain)
        })
    }
    pub fn set_track_pan(&mut self, track_id: u64, pan: f32) -> Result<(), AudioError> {
        self.edit(Some(format!("pan:{track_id}")), |p| {
            p.set_track_pan(TrackId(track_id), pan)
        })
    }
    pub fn set_track_mute(&mut self, track_id: u64, muted: bool) -> Result<(), AudioError> {
        self.edit(None, |p| p.set_track_mute(TrackId(track_id), muted))
    }
    pub fn set_track_solo(&mut self, track_id: u64, solo: bool) -> Result<(), AudioError> {
        self.edit(None, |p| p.set_track_solo(TrackId(track_id), solo))
    }
    pub fn reorder_tracks(&mut self, ordered_ids: &[u64]) -> Result<ProjectSnapshot, AudioError> {
        let ids = ordered_ids.iter().copied().map(TrackId).collect::<Vec<_>>();
        self.edit(None, |p| p.reorder_tracks(&ids))?;
        Ok(self.project())
    }
    pub fn edit_midi_clip(
        &mut self,
        track_id: u64,
        clip_id: u64,
        action: &str,
        amount: i32,
        note_id: Option<u64>,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |p| {
            p.edit_midi_clip(TrackId(track_id), clip_id, action, amount, note_id)
        })?;
        Ok(self.project())
    }
    pub fn edit_midi_clips(
        &mut self,
        targets: &[daw_project::MidiClipTarget],
        edit: daw_project::MidiClipEdit,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |project| project.edit_midi_clips(targets, edit))?;
        Ok(self.project())
    }

    pub fn add_midi_clip(
        &mut self,
        track_id: u64,
        start: u64,
        duration_samples: u64,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |project| {
            project.add_midi_clip(TrackId(track_id), start, duration_samples)
        })?;
        Ok(self.project())
    }

    pub fn edit_midi_notes(
        &mut self,
        targets: &[daw_project::MidiNoteTarget],
        edit: MidiSelectionEdit,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |project| project.edit_midi_notes(targets, edit))?;
        Ok(self.project())
    }

    pub fn edit_midi_selection(
        &mut self,
        track_id: u64,
        clip_id: u64,
        note_ids: &[u64],
        edit: MidiSelectionEdit,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(None, |project| {
            project.edit_midi_selection(TrackId(track_id), clip_id, note_ids, edit)
        })?;
        Ok(self.project())
    }

    pub fn write_midi_note(
        &mut self,
        track_id: u64,
        clip_id: u64,
        note_id: Option<u64>,
        start: u64,
        duration_samples: u64,
        key: u8,
    ) -> Result<ProjectSnapshot, AudioError> {
        // Each completed gesture is one undo step, separate from adjacent gestures.
        self.edit(None, |p| {
            p.write_midi_note(
                TrackId(track_id),
                clip_id,
                note_id,
                start,
                duration_samples,
                key,
            )
        })?;
        Ok(self.project())
    }

    pub fn set_midi_note_velocity(
        &mut self,
        track_id: u64,
        clip_id: u64,
        note_id: u64,
        velocity: f32,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(
            Some(format!("velocity:{track_id}:{clip_id}:{note_id}")),
            |p| p.set_midi_note_velocity(TrackId(track_id), clip_id, note_id, velocity),
        )?;
        Ok(self.project())
    }
    pub fn set_automation_point_value(
        &mut self,
        track_id: u64,
        lane_id: u64,
        point_id: u64,
        value: f32,
    ) -> Result<ProjectSnapshot, AudioError> {
        self.edit(
            Some(format!("automation:{track_id}:{lane_id}:{point_id}")),
            |p| p.set_automation_point_value(TrackId(track_id), lane_id, point_id, value),
        )?;
        Ok(self.project())
    }

    pub fn metrics(&self) -> Option<RealtimeMetricsSnapshot> {
        self.control.as_ref().map(EngineControl::metrics)
    }

    #[must_use]
    pub const fn state(&self) -> EngineState {
        self.state
    }

    fn control(&self) -> Result<&EngineControl, AudioError> {
        self.control
            .as_ref()
            .ok_or(AudioError::InvalidState("engine is not running"))
    }

    #[must_use]
    pub fn publication_status(&self, after_revision: u64) -> PublicationStatus {
        let preparation = self.preparation.as_ref().map(PreparationService::status);
        let requested_revision = preparation
            .as_ref()
            .and_then(|status| status.published_revision)
            .or(self.requested_revision);
        PublicationStatus {
            stream_readiness: self
                .control
                .as_ref()
                .and_then(EngineControl::stream_readiness),
            streaming: self.project.streaming_status(),
            document_revision: self.revision,
            requested_revision,
            engine_epoch: self.engine_epoch,
            preparation,
            engine: self
                .control
                .as_ref()
                .map(|control| control.graph_publication_status(after_revision)),
        }
    }

    /// Control-thread polling also reclaims retired/skipped graphs so backpressure can clear
    /// even when the user makes no further edits. Destruction never runs in the callback.
    pub fn service_publications(
        &self,
        after_revision: u64,
    ) -> Result<PublicationStatus, AudioError> {
        if let Some(control) = &self.control {
            control.reclaim_retired()?;
        }
        Ok(self.publication_status(after_revision))
    }

    fn publish_candidate(
        &self,
        project: &Project,
        document_revision: u64,
    ) -> Result<Option<u64>, AudioError> {
        let Some(config) = self.active_config else {
            return Ok(None);
        };
        if self.background_preparation {
            self.preparation
                .as_ref()
                .ok_or(AudioError::InvalidState(
                    "preparation worker is not running",
                ))?
                .request(project.clone(), document_revision)?;
            return Ok(self.requested_revision);
        }
        self.control()?
            .submit_latest_timeline(project.build_audio_graph(config)?, document_revision)
            .map(Some)
    }
}

impl Default for DawApplication {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DawApplication {
    fn drop(&mut self) {
        if let Some(stream) = &mut self.stream {
            let _ = stream.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DawApplication, EngineState};
    use audio_backend::mock::MockAudioBackend;
    use audio_types::AudioConfig;

    #[test]
    #[allow(clippy::too_many_lines)] // Continuous render/history oracle including saturated publication.
    fn scheduled_mute_history_is_sample_accurate_and_failed_publication_is_atomic() {
        use audio_engine::{EngineGraph, EngineProcessor};
        use daw_core::{SamplePosition, transport::TransportCommand};
        use daw_mixer::{MixerChange, MixerEvent};
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        let mut app = DawApplication::with_sample_rate(8000).unwrap();
        for track in 1..=3 {
            app.set_track_mute(track, true).unwrap();
        }
        app.set_track_pan(4, 0.0).unwrap();
        app.add_midi_clip(4, 0, 20000).unwrap();
        app.write_midi_note(4, 4101, None, 0, 19000, 60).unwrap();
        app.undo.clear();
        let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
        let (mut reference, reference_control) = EngineProcessor::prepare(config).unwrap();
        for ctl in [&control, &reference_control] {
            ctl.submit_graph(EngineGraph::timeline(
                app.project.build_audio_graph(config).unwrap(),
            ))
            .unwrap();
            ctl.send_transport(TransportCommand::Play).unwrap();
        }
        app.active_config = Some(config);
        app.control = Some(control);
        actual.process_block(&mut [0.0; 34]);
        reference.process_block(&mut [0.0; 34]);
        let event = MixerEvent {
            id: 1,
            position: SamplePosition(65),
            change: MixerChange::Mute(true),
        };
        let mut current = 1.0_f32;
        let mut start = current;
        let mut muted = false;
        let mut elapsed = 40_u16;
        let mut cursor = 1;
        let mut late_count = 0;
        for (step, frames) in [17, 17, 17, 19, 20, 17, 1, 17, 64, 33, 257, 32]
            .into_iter()
            .enumerate()
        {
            match step {
                0 => {
                    app.set_mixer_event(4, event).unwrap();
                }
                1 | 3 | 6 | 8 | 10 => {
                    app.undo().unwrap();
                }
                2 | 4 | 11 => {
                    app.redo().unwrap();
                }
                5 => {
                    app.reorder_tracks(&[4, 3, 2, 1]).unwrap();
                }
                7 => {
                    app.set_mixer_event(
                        4,
                        MixerEvent {
                            position: SamplePosition(400),
                            ..event
                        },
                    )
                    .unwrap();
                }
                _ => {
                    app.remove_mixer_event(4, 1).unwrap();
                }
            }
            let pending = app.publication_status(cursor);
            assert_eq!(pending.engine.as_ref().unwrap().adopted_revision, cursor);
            assert_eq!(
                pending.engine.as_ref().unwrap().accepted_revision,
                app.requested_revision.unwrap()
            );
            let position = app.control.as_ref().unwrap().transport().current_sample.0;
            let snapshot = app.project();
            let track = snapshot.tracks.iter().find(|track| track.id == 4).unwrap();
            let mut output = vec![0.0; frames * 2];
            let mut expected = output.clone();
            actual.process_block(&mut output);
            reference.process_block(&mut expected);
            let status = app.publication_status(cursor).engine.unwrap();
            assert_eq!(status.acknowledgements.len(), 1);
            let ack = status.acknowledgements[0];
            assert!(ack.revision > cursor);
            assert_eq!(ack.document_revision, app.revision);
            assert_eq!(ack.adopted_sample, Some(SamplePosition(position)));
            let deadline = if matches!(step, 5 | 6) {
                None
            } else {
                Some(SamplePosition(65))
            };
            assert_eq!(ack.deadline, deadline);
            assert_eq!(ack.late, deadline.is_some() && position > 65);
            late_count += u64::from(ack.late);
            assert_eq!(status.late_count, late_count);
            cursor = ack.revision;
            for (frame, (output, expected)) in output
                .as_chunks::<2>()
                .0
                .iter()
                .zip(expected.as_chunks::<2>().0)
                .enumerate()
            {
                let sample = position + u64::try_from(frame).unwrap();
                let desired = track
                    .mixer_events
                    .iter()
                    .any(|event| event.position.0 <= sample);
                if desired != muted {
                    start = current;
                    muted = desired;
                    elapsed = 0;
                }
                elapsed = (elapsed + 1).min(40);
                let target = if muted { 0.0 } else { 1.0 };
                current = if elapsed == 40 {
                    target
                } else {
                    start + (target - start) * f32::from(elapsed) / 40.0
                };
                for channel in 0..2 {
                    assert!(
                        (output[channel] - expected[channel] * current).abs() < 1e-6,
                        "history step {step}, sample {sample}"
                    );
                }
            }
        }
        // Fill the bounded publication queue. Rejected edits and Undo leave all document/history state intact.
        for id in 10..14 {
            app.set_mixer_event(4, MixerEvent { id, ..event }).unwrap();
        }
        let before = app.project();
        let revision = app.revision;
        let history = app.undo.len();
        let requested = app.requested_revision;
        assert!(
            app.set_mixer_event(4, MixerEvent { id: 99, ..event })
                .is_err()
        );
        assert!(app.undo().is_err());
        assert_eq!(app.project(), before);
        assert_eq!(app.revision, revision);
        assert_eq!(app.undo.len(), history);
        assert_eq!(app.requested_revision, requested);
        assert_eq!(
            app.publication_status(0).engine.unwrap().accepted_revision,
            requested.unwrap()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One continuous engine/history scenario shares a PCM oracle.
    fn edits_and_history_preserve_sustained_midi_pcm() {
        use audio_engine::{EngineGraph, EngineProcessor};
        use daw_core::{SamplePosition, transport::TransportCommand};
        let config = AudioConfig::DEFAULT;
        let mut app = DawApplication::new();
        for track in 1..=3 {
            app.set_track_mute(track, true).unwrap();
        }
        app.set_track_pan(4, 0.0).unwrap();
        app.add_midi_clip(4, 0, 20000).unwrap();
        app.write_midi_note(4, 4101, None, 0, 19000, 60).unwrap();
        app.undo.clear();
        let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
        let (mut reference, reference_control) = EngineProcessor::prepare(config).unwrap();
        control
            .submit_graph(EngineGraph::timeline(
                app.project.build_audio_graph(config).unwrap(),
            ))
            .unwrap();
        reference_control
            .submit_graph(EngineGraph::timeline(
                app.project.build_audio_graph(config).unwrap(),
            ))
            .unwrap();
        for ctl in [&control, &reference_control] {
            ctl.send_transport(TransportCommand::SetLoop(daw_core::SampleRange::new(
                SamplePosition(7),
                SamplePosition(1300),
            )))
            .unwrap();
            ctl.send_transport(TransportCommand::Play).unwrap();
        }
        let initial_gain = app
            .project()
            .tracks
            .iter()
            .find(|track| track.id == 4)
            .unwrap()
            .gain;
        app.active_config = Some(config);
        app.control = Some(control);
        // Adopt the initial graph before editing so each assertion observes that edit,
        // rather than a graph still queued from the previous step.
        let mut initial = [0.0; 34];
        let mut initial_reference = initial;
        actual.process_block(&mut initial);
        reference.process_block(&mut initial_reference);
        assert_eq!(
            initial.map(f32::to_bits),
            initial_reference.map(f32::to_bits)
        );
        for step in 0..36 {
            match step % 9 {
                0 => app.set_track_gain(1, 0.3).unwrap(),
                1 | 4 | 7 => {
                    app.undo().unwrap();
                }
                3 => {
                    app.reorder_tracks(&[4, 3, 2, 1]).unwrap();
                }
                6 => {
                    app.write_midi_note(4, 4101, None, 15000, 500, 72).unwrap();
                }
                _ => {
                    app.redo().unwrap();
                }
            }
            let frames = [17, 128, 257, 1, 64][step % 5];
            let mut output = vec![0.0; frames * 2];
            let mut expected = output.clone();
            actual.process_block(&mut output);
            reference.process_block(&mut expected);
            assert_eq!(output, expected, "edit/history step {step}");
            app.control.as_ref().unwrap().reclaim_retired().unwrap();
        }
        let mut current = [initial_gain; 2];
        let mut ramp_start = current;
        let mut target = current;
        let mut elapsed = 240_u16;
        for step in 0..10 {
            match step {
                0 => app.set_track_gain(4, 0.4).unwrap(),
                1 | 4 | 6 | 8 => {
                    app.undo().unwrap();
                }
                2 => {
                    app.redo().unwrap();
                }
                3 => app.set_track_pan(4, 0.35).unwrap(),
                5 => app.set_track_mute(4, true).unwrap(),
                7 => app.set_track_solo(1, true).unwrap(),
                _ => app.set_track_solo(4, true).unwrap(),
            }
            let snapshot = app.project();
            let track = snapshot.tracks.iter().find(|track| track.id == 4).unwrap();
            let excluded =
                track.muted || (snapshot.tracks.iter().any(|track| track.solo) && !track.solo);
            let mut output = [0.0; 256];
            let mut expected = output;
            actual.process_block(&mut output);
            reference.process_block(&mut expected);
            let desired = if excluded {
                [0.0; 2]
            } else {
                [
                    track.gain * (1.0 - track.pan).min(1.0),
                    track.gain * (1.0 + track.pan).min(1.0),
                ]
            };
            if desired.map(f32::to_bits) != target.map(f32::to_bits) {
                ramp_start = current;
                target = desired;
                elapsed = 0;
            }
            for (actual, expected) in output
                .as_chunks::<2>()
                .0
                .iter()
                .zip(expected.as_chunks::<2>().0)
            {
                elapsed = (elapsed + 1).min(240);
                for channel in 0..2 {
                    // Independent closed-form oracle; implementation accumulates increments.
                    current[channel] = if elapsed == 240 {
                        target[channel]
                    } else {
                        ramp_start[channel]
                            + (target[channel] - ramp_start[channel]) * f32::from(elapsed) / 240.0
                    };
                    let expected = expected[channel] / initial_gain * current[channel];
                    assert!(
                        (actual[channel] - expected).abs() < 1e-6,
                        "active mixer/history step {step}"
                    );
                }
            }
            app.control.as_ref().unwrap().reclaim_retired().unwrap();
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // A single sustained-note oracle spans queued Undo/Redo and coalescing.
    fn coalesced_history_uses_the_audible_schedule_for_deadlines_and_preserves_phase() {
        use audio_engine::{EngineGraph, EngineProcessor, GraphOutcome};
        use daw_core::{SamplePosition, transport::TransportCommand};
        use daw_mixer::{MixerChange, MixerEvent};
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        let mut app = DawApplication::with_sample_rate(8000).unwrap();
        for track in 1..=3 {
            app.set_track_mute(track, true).unwrap();
        }
        app.set_track_pan(4, 0.0).unwrap();
        app.add_midi_clip(4, 0, 20000).unwrap();
        app.write_midi_note(4, 4101, None, 0, 19000, 60).unwrap();
        app.undo.clear();
        let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
        let (mut reference, reference_control) = EngineProcessor::prepare(config).unwrap();
        for ctl in [&control, &reference_control] {
            ctl.submit_graph(EngineGraph::timeline(
                app.project.build_audio_graph(config).unwrap(),
            ))
            .unwrap();
            ctl.send_transport(TransportCommand::Play).unwrap();
        }
        app.active_config = Some(config);
        app.control = Some(control);
        actual.process_block(&mut [0.0; 200]);
        reference.process_block(&mut [0.0; 200]);
        let mut cursor = 1;
        for position in [165, 65] {
            app.set_mixer_event(
                4,
                MixerEvent {
                    id: 1,
                    position: SamplePosition(position),
                    change: MixerChange::Mute(true),
                },
            )
            .unwrap();
            app.undo().unwrap(); // Cancel a future or past event before it ever becomes audible.
            if position == 165 {
                app.redo().unwrap();
                app.undo().unwrap(); // Four pending snapshots, including repeated document revisions.
            }
            let mut output = [0.0; 512];
            let mut expected = output;
            actual.process_block(&mut output);
            reference.process_block(&mut expected);
            assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
            let status = app.publication_status(cursor).engine.unwrap();
            assert_eq!(
                status.acknowledgements.len(),
                if position == 165 { 4 } else { 2 }
            );
            assert!(
                status.acknowledgements[..status.acknowledgements.len() - 1]
                    .iter()
                    .all(|receipt| receipt.outcome == GraphOutcome::Superseded)
            );
            assert!(
                status
                    .acknowledgements
                    .windows(2)
                    .all(|pair| pair[0].revision < pair[1].revision)
            );
            let adopted = status.acknowledgements.last().unwrap();
            assert_eq!(adopted.outcome, GraphOutcome::Adopted);
            assert_eq!(adopted.document_revision, app.revision);
            assert_eq!(adopted.deadline, None);
            assert_eq!(status.late_count, 0);
            cursor = status.settled_revision;
        }
        app.redo().unwrap(); // Restore the past mute.
        app.set_track_gain(1, 0.3).unwrap(); // Supersede it with an unrelated edit retaining the mute.
        let mut output = [0.0; 128];
        let mut expected = output;
        actual.process_block(&mut output);
        reference.process_block(&mut expected);
        for (frame, (actual, reference)) in output
            .as_chunks::<2>()
            .0
            .iter()
            .zip(expected.as_chunks::<2>().0)
            .enumerate()
        {
            let fraction = f32::from(u16::try_from(frame + 1).unwrap().min(40)) / 40.0;
            for channel in 0..2 {
                assert!((actual[channel] - reference[channel] * (1.0 - fraction)).abs() < 1e-6);
            }
        }
        let status = app.publication_status(cursor).engine.unwrap();
        assert_eq!(status.acknowledgements[0].outcome, GraphOutcome::Superseded);
        let last = status.acknowledgements.last().unwrap();
        assert_eq!(last.document_revision, app.revision);
        assert_eq!(last.deadline, Some(SamplePosition(65)));
        assert!(last.late);
        assert_eq!(status.late_count, 1);
        assert_eq!(status.superseded_count, 5);
        assert!(!status.pending);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Full document/history/audio oracle while the first preparation is held.
    fn background_preparation_cancellation_keeps_history_persistence_phase_and_publication_ids_consistent()
     {
        use audio_engine::{EngineGraph, EngineProcessor, GraphOutcome};
        use daw_core::{SamplePosition, transport::TransportCommand};
        use daw_mixer::{MixerChange, MixerEvent};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        let mut app = DawApplication::with_sample_rate(8000).unwrap();
        app.background_preparation = true;
        for track in 1..=3 {
            app.set_track_mute(track, true).unwrap();
        }
        app.set_track_pan(4, 0.0).unwrap();
        app.add_midi_clip(4, 0, 20000).unwrap();
        app.write_midi_note(4, 4101, None, 0, 19000, 60).unwrap();
        app.undo.clear();
        let original = app.project();
        let original_revision = app.revision;
        let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
        let (mut reference, reference_control) = EngineProcessor::prepare(config).unwrap();
        for ctl in [&control, &reference_control] {
            ctl.submit_graph_revision(
                EngineGraph::timeline(app.project.build_audio_graph(config).unwrap()),
                original_revision,
                None,
            )
            .unwrap();
            ctl.send_transport(TransportCommand::Play).unwrap();
        }
        actual.process_block(&mut [0.0; 200]);
        reference.process_block(&mut [0.0; 200]);
        let release = Arc::new(super::preparation::TestGate::new());
        let worker_release = Arc::clone(&release);
        let calls = AtomicUsize::new(0);
        let (started, receiver) = mpsc::sync_channel(1);
        app.preparation = Some(
            super::PreparationService::spawn_with(
                control.clone(),
                config,
                Arc::new(move |project, config, token| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.send(()).unwrap();
                        worker_release.wait();
                    }
                    project.build_audio_graph_cancellable(config, token)
                }),
            )
            .unwrap(),
        );
        app.control = Some(control);
        app.active_config = Some(config);
        app.requested_revision = Some(1);
        app.set_mixer_event(
            4,
            MixerEvent {
                id: 1,
                position: SamplePosition(129),
                change: MixerChange::Mute(true),
            },
        )
        .unwrap();
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        app.undo().unwrap();
        app.redo().unwrap();
        app.undo().unwrap();
        assert_eq!(app.project(), original);
        assert_eq!(app.revision, original_revision);
        assert!(app.set_track_gain(999, 0.4).is_err());
        let pending = app.publication_status(1);
        assert_eq!(
            pending.preparation.as_ref().unwrap().requested_preparation,
            4
        );
        assert!(pending.preparation.as_ref().unwrap().pending);
        assert_eq!(pending.engine.as_ref().unwrap().accepted_revision, 1);
        assert_eq!(pending.requested_revision, Some(1));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("while-preparing.ongaku");
        app.save_project(&path).unwrap();
        assert_eq!(
            daw_project::Project::load(&path).unwrap().snapshot(),
            original
        );
        assert!(!app.document_status().dirty);
        let mut output = [0.0; 512];
        let mut expected = output;
        actual.process_block(&mut output);
        reference.process_block(&mut expected);
        assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits)); // Audio runs while preparation is held.
        release.release();
        let prepared = app
            .preparation
            .as_ref()
            .unwrap()
            .wait_for(|status| !status.pending);
        assert_eq!(prepared.canceled_count, 3);
        assert_eq!(prepared.started_count, 2);
        assert_eq!(prepared.published_preparation, Some(4));
        assert_eq!(prepared.published_revision, Some(2));
        actual.process_block(&mut output);
        reference.process_block(&mut expected);
        assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
        let status = app.service_publications(1).unwrap();
        assert_eq!(status.requested_revision, Some(2));
        let adopted = status.engine.unwrap().acknowledgements[0];
        assert_eq!(adopted.outcome, GraphOutcome::Adopted);
        assert_eq!(adopted.document_revision, original_revision);
        assert_eq!(adopted.deadline, None);
        assert!(!adopted.late);
        assert!(!app.document_status().dirty);
        app.stop().unwrap();
        assert!(app.publication_status(0).preparation.is_none());
        let backend = MockAudioBackend::default();
        app.start(&backend, None, config).unwrap();
        assert_eq!(
            app.publication_status(0)
                .preparation
                .unwrap()
                .requested_preparation,
            0
        );
        assert_eq!(app.project(), original);
    }

    #[test]
    fn desktop_constructor_runs_background_preparation_and_resets_it_on_restart() {
        let backend = MockAudioBackend::default();
        let mut app = DawApplication::new_with_background_preparation();
        app.start(&backend, None, AudioConfig::DEFAULT).unwrap();
        assert!(app.publication_status(0).preparation.is_some());
        app.set_track_pan(4, -0.5).unwrap();
        let prepared = app
            .preparation
            .as_ref()
            .unwrap()
            .wait_for(|status| !status.pending);
        assert_eq!(prepared.published_revision, Some(2));
        assert_eq!(prepared.published_document_revision, Some(app.revision));
        let status = app.service_publications(0).unwrap();
        assert_eq!(status.requested_revision, Some(2));
        let engine = status.engine.unwrap();
        assert_eq!(engine.accepted_revision, 2);
        assert_eq!(engine.adopted_revision, 1); // Mock processes only startup; preparation is not adoption.
        assert!(engine.pending);
        app.stop().unwrap();
        assert!(app.publication_status(0).preparation.is_none());
        app.start(&backend, None, AudioConfig::DEFAULT).unwrap();
        let status = app.publication_status(0);
        assert_eq!(status.engine_epoch, 2);
        assert_eq!(status.preparation.unwrap().requested_preparation, 0);
        assert_eq!(status.engine.unwrap().adopted_revision, 1);
    }

    #[test]
    fn engine_restart_uses_a_new_epoch_and_clears_old_receipts() {
        let backend = MockAudioBackend::default();
        let mut app = DawApplication::new();
        app.start(&backend, None, AudioConfig::DEFAULT).unwrap();
        let first = app.publication_status(0);
        assert_eq!(first.engine_epoch, 1);
        assert_eq!(first.engine.unwrap().adopted_revision, 1);
        app.stop().unwrap();
        assert!(app.publication_status(0).engine.is_none());
        assert!(app.publication_status(0).requested_revision.is_none());
        app.start(&backend, None, AudioConfig::DEFAULT).unwrap();
        let second = app.publication_status(0);
        assert_eq!(second.engine_epoch, 2);
        assert_eq!(second.engine.unwrap().late_count, 0);
    }

    #[test]
    fn arrangement_edits_are_one_history_step_and_survive_storage() {
        use daw_project::{MidiClipEdit, MidiClipTarget};
        let mut app = DawApplication::with_sample_rate(8000).unwrap();
        app.add_midi_clip(4, 1000, 12000).unwrap();
        app.write_midi_note(4, 4101, None, 0, 10000, 60).unwrap();
        let targets = [MidiClipTarget {
            track_id: 4,
            clip_id: 4101,
        }];
        let mut stages = vec![app.project()];
        for edit in [
            MidiClipEdit::Move {
                delta_samples: 1000,
            },
            MidiClipEdit::Duplicate {
                delta_samples: 16000,
            },
            MidiClipEdit::TrimLeft {
                delta_samples: 1000,
            },
            MidiClipEdit::TrimRight {
                delta_samples: -1000,
            },
            MidiClipEdit::Split { sample: 6000 },
        ] {
            let count = app.undo.len();
            app.edit_midi_clips(&targets, edit).unwrap();
            assert_eq!(app.undo.len(), count + 1);
            stages.push(app.project());
        }
        for stage in stages[..5].iter().rev() {
            assert_eq!(&app.undo().unwrap(), stage);
        }
        for stage in &stages[1..] {
            assert_eq!(&app.redo().unwrap(), stage);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arrangement.ongaku");
        let saved = app.project();
        app.save_project(&path).unwrap();
        app.edit_midi_clips(
            &targets,
            MidiClipEdit::Move {
                delta_samples: 1000,
            },
        )
        .unwrap();
        app.load_project(&path).unwrap();
        assert_eq!(app.project(), saved);
        assert!(!app.document_status().dirty);
        assert!(!app.document_status().can_undo);
        assert!(
            app.edit_midi_clips(
                &[
                    targets[0],
                    MidiClipTarget {
                        track_id: 4,
                        clip_id: 999
                    }
                ],
                MidiClipEdit::Move { delta_samples: 100 }
            )
            .is_err()
        );
        assert_eq!(app.project(), saved);
        assert!(!app.document_status().can_undo);
    }

    #[test]
    fn multi_clip_edits_are_one_history_step_and_roundtrip() {
        use daw_project::{MidiNoteTarget, MidiSelectionEdit};
        let mut app = DawApplication::with_sample_rate(8_000).unwrap();
        let before_clips = app.project();
        app.add_midi_clip(4, 2500, 10_000).unwrap();
        let one_clip = app.project();
        app.undo().unwrap();
        assert_eq!(app.project(), before_clips);
        app.redo().unwrap();
        assert_eq!(app.project(), one_clip);
        app.add_midi_clip(4, 7000, 10_000).unwrap();
        app.write_midi_note(4, 4101, None, 100, 2000, 60).unwrap();
        app.write_midi_note(4, 4102, None, 600, 1000, 64).unwrap();
        let targets = [
            MidiNoteTarget {
                track_id: 4,
                clip_id: 4101,
                note_id: 1,
            },
            MidiNoteTarget {
                track_id: 4,
                clip_id: 4102,
                note_id: 1,
            },
        ];
        let mut stages = vec![app.project()];
        for edit in [
            MidiSelectionEdit::Move {
                delta_samples: 1000,
                semitones: 2,
            },
            MidiSelectionEdit::Resize { delta_samples: 500 },
            MidiSelectionEdit::Quantize { division: 16 },
            MidiSelectionEdit::Delete,
        ] {
            let count = app.undo.len();
            app.edit_midi_notes(&targets, edit).unwrap();
            assert_eq!(app.undo.len(), count + 1);
            stages.push(app.project());
        }
        for stage in stages[..4].iter().rev() {
            assert_eq!(&app.undo().unwrap(), stage);
        }
        for stage in &stages[1..] {
            assert_eq!(&app.redo().unwrap(), stage);
        }
        app.undo().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("multi.ongaku");
        app.save_project(&path).unwrap();
        let saved = app.project();
        app.edit_midi_notes(&targets, MidiSelectionEdit::Delete)
            .unwrap();
        app.load_project(&path).unwrap();
        assert_eq!(app.project(), saved);
        assert!(!app.document_status().can_undo);
        assert!(!app.document_status().dirty);
        let invalid = [
            targets[0],
            MidiNoteTarget {
                track_id: 4,
                clip_id: 999,
                note_id: 1,
            },
        ];
        assert!(
            app.edit_midi_notes(&invalid, MidiSelectionEdit::Delete)
                .is_err()
        );
        assert_eq!(app.project(), saved);
        assert!(!app.document_status().can_undo);
    }

    #[test]
    fn selection_edits_are_atomic_history_steps_and_persist() {
        use daw_project::MidiSelectionEdit;
        let mut app = DawApplication::with_sample_rate(8_000).unwrap();
        app.write_midi_note(4, 4100, None, 1100, 2000, 60).unwrap();
        app.write_midi_note(4, 4100, None, 5100, 1000, 64).unwrap();
        let mut stages = vec![app.project()];
        let ids = [4016, 4017];
        for edit in [
            MidiSelectionEdit::Move {
                delta_samples: 3000,
                semitones: 2,
            },
            MidiSelectionEdit::Resize { delta_samples: 250 },
            MidiSelectionEdit::Quantize { division: 16 },
            MidiSelectionEdit::Delete,
        ] {
            app.edit_midi_selection(4, 4100, &ids, edit).unwrap();
            stages.push(app.project());
        }
        assert_eq!(app.undo.len(), 6);
        for expected in stages[..4].iter().rev() {
            assert_eq!(&app.undo().unwrap(), expected);
        }
        for expected in &stages[1..] {
            assert_eq!(&app.redo().unwrap(), expected);
        }
        app.undo().unwrap(); // Store the quantized notes, including their relative duration changes.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("selection.ongaku");
        app.save_project(&path).unwrap();
        let saved = app.project();
        app.edit_midi_selection(4, 4100, &ids, MidiSelectionEdit::Delete)
            .unwrap();
        app.load_project(&path).unwrap();
        assert_eq!(app.project(), saved);
        assert!(
            app.edit_midi_selection(4, 4100, &[4016, 999], MidiSelectionEdit::Delete)
                .is_err()
        );
        assert_eq!(app.project(), saved);
        assert!(!app.document_status().dirty);
        assert!(!app.document_status().can_undo);
    }

    #[test]
    fn note_gestures_roundtrip_through_history_and_storage() {
        let mut app = DawApplication::with_sample_rate(8_000).unwrap();
        let initial = app.project();
        let added = app.write_midi_note(4, 4100, None, 1000, 2000, 65).unwrap();
        let moved = app
            .write_midi_note(4, 4100, Some(4016), 3000, 2000, 67)
            .unwrap();
        let resized = app
            .write_midi_note(4, 4100, Some(4016), 3000, 4000, 67)
            .unwrap();
        assert_eq!(app.undo.len(), 3);
        assert_eq!(app.undo().unwrap(), moved);
        assert_eq!(app.undo().unwrap(), added);
        assert_eq!(app.undo().unwrap(), initial);
        for _ in 0..3 {
            app.redo().unwrap();
        }
        assert_eq!(app.project(), resized);
        assert!(
            app.write_midi_note(4, 4100, Some(4016), 63_999, 2, 67)
                .is_err()
        );
        assert_eq!(app.project(), resized);
        assert_eq!(app.undo.len(), 3);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.ongaku");
        app.save_project(&path).unwrap();
        app.write_midi_note(4, 4100, Some(4016), 3000, 1, 67)
            .unwrap();
        app.load_project(&path).unwrap();
        assert_eq!(app.project(), resized);
        app.write_midi_note(4, 4100, None, 0, 1, 60).unwrap();
        let notes = &app.project().tracks[3].midi_clips[0].notes;
        let ids = notes
            .iter()
            .map(|note| note.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), notes.len());
        assert_eq!(app.undo().unwrap(), resized);
    }

    #[test]
    fn history_branches_coalesces_and_tracks_saved_revision() {
        let mut app = DawApplication::with_sample_rate(8_000).unwrap();
        let initial = app.project();
        app.set_track_gain(1, 0.4).unwrap();
        app.set_track_gain(1, 0.3).unwrap();
        assert_eq!(app.undo.len(), 1);
        let edited = app.project();
        assert_eq!(app.undo().unwrap(), initial);
        assert_eq!(app.redo().unwrap(), edited);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("project.ongaku");
        app.save_project(&path).unwrap();
        assert!(!app.document_status().dirty);
        app.set_track_gain(1, 0.2).unwrap();
        app.undo().unwrap();
        assert!(!app.document_status().dirty);
        app.redo().unwrap();
        app.undo().unwrap();
        app.set_track_mute(2, true).unwrap();
        assert!(!app.document_status().can_redo);
        let before_failure = app.project();
        let undo_count = app.undo.len();
        assert!(app.set_track_gain(999, 1.0).is_err());
        assert_eq!(app.undo.len(), undo_count);
        assert_eq!(app.project(), before_failure);
        std::fs::write(directory.path().join("bad.ongaku"), "broken").unwrap();
        assert!(
            app.load_project(&directory.path().join("bad.ongaku"))
                .is_err()
        );
        assert_eq!(app.project(), before_failure);
        let saved_path = app.document_status().path;
        assert!(
            app.save_project(&directory.path().join("missing/session.ongaku"))
                .is_err()
        );
        assert_eq!(app.document_status().path, saved_path);
        assert!(app.document_status().dirty);
        app.start(
            &MockAudioBackend::default(),
            None,
            AudioConfig {
                sample_rate: 8_000,
                ..AudioConfig::DEFAULT
            },
        )
        .unwrap();
        app.load_project(&path).unwrap();
        assert_eq!(app.state(), EngineState::Stopped);
        assert_eq!(app.project(), edited);
        assert!(!app.document_status().can_undo);
        assert!(!app.document_status().dirty);
    }

    #[test]
    fn every_project_edit_can_be_undone() {
        let mut app = DawApplication::new();
        let initial = app.project();
        app.set_track_pan(1, 0.8).unwrap();
        app.set_track_solo(2, true).unwrap();
        app.reorder_tracks(&[4, 3, 2, 1]).unwrap();
        app.set_midi_note_velocity(4, 4100, 4000, 0.2).unwrap();
        app.set_automation_point_value(4, 4200, 4201, 0.1).unwrap();
        app.edit_midi_clip(4, 4100, "delete", 0, Some(4000))
            .unwrap();
        for _ in 0..6 {
            app.undo().unwrap();
        }
        assert_eq!(app.project(), initial);
        for _ in 0..6 {
            app.redo().unwrap();
        }
        assert_eq!(app.project().tracks[0].midi_clips[0].notes.len(), 15);
    }

    #[test]
    fn runs_the_engine_through_the_backend_contract() {
        let backend = MockAudioBackend::default();
        let captured = backend.captured_audio();
        let mut app = DawApplication::new();
        app.start(&backend, None, AudioConfig::DEFAULT).unwrap();
        assert_eq!(app.state(), EngineState::Running);
        assert!(!captured.lock().unwrap().is_empty());
        assert_eq!(app.project().tracks.len(), 4);
        app.set_gain(0.5).unwrap();
        app.stop().unwrap();
        assert_eq!(app.state(), EngineState::Stopped);
    }
}

#[cfg(test)]
mod streaming_tests;
