//! Non-realtime application/control layer reusable by Tauri, CLI, or native UIs.

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
        let graph = self.project.build_audio_graph(config)?;
        let (processor, control) = EngineProcessor::prepare(config)?;
        control.submit_graph(EngineGraph::timeline(graph))?;
        let project = self.project.snapshot();
        control.send_transport(TransportCommand::SetTempo(project.tempo_bpm))?;
        control.send_transport(TransportCommand::SetTimeSignature(project.time_signature))?;
        let mut stream = backend.open_output(device_id, config, Box::new(processor))?;
        stream.start()?;
        self.stream = Some(stream);
        self.control = Some(control);
        self.state = EngineState::Running;
        self.active_config = Some(config);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        if let Some(stream) = &mut self.stream {
            stream.stop()?;
        }
        self.stream = None;
        self.control = None;
        self.state = EngineState::Stopped;
        self.active_config = None;
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
        self.publish_candidate(&candidate)?;
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
        self.next_revision += 1;
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
            self.publish_candidate(project)?;
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
            self.publish_candidate(project)?;
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

    fn publish_candidate(&self, project: &Project) -> Result<(), AudioError> {
        let Some(config) = self.active_config else {
            return Ok(());
        };
        self.submit_audio_graph(project.build_audio_graph(config)?)
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
