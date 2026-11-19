use super::*;
use audio_backend::{AudioCallback, mock::MockAudioBackend};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Backend {
    faulted: Arc<AtomicBool>,
    opens: AtomicUsize,
    fail_on_start: bool,
}
struct Stream {
    inner: Box<dyn AudioOutputStream>,
    faulted: Arc<AtomicBool>,
    fail_on_start: bool,
}
impl AudioOutputStream for Stream {
    fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }
    fn output_configuration(&self) -> Option<audio_types::OutputConfiguration> {
        self.inner.output_configuration()
    }
    fn start(&mut self) -> Result<(), AudioError> {
        self.inner.start()?;
        self.faulted.store(self.fail_on_start, Ordering::Release);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), AudioError> {
        self.inner.stop()
    }
}
impl AudioBackend for Backend {
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        MockAudioBackend::default().output_devices()
    }
    fn default_output_config(&self) -> Result<AudioConfig, AudioError> {
        Ok(AudioConfig::DEFAULT)
    }
    fn open_output(
        &self,
        id: Option<&str>,
        config: AudioConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(Stream {
            inner: MockAudioBackend::default().open_output(id, config, callback)?,
            faulted: self.faulted.clone(),
            fail_on_start: self.fail_on_start,
        }))
    }
}

#[test]
fn output_failure_preserves_edit_history_and_requires_a_new_engine_epoch() {
    let config = AudioConfig {
        sample_rate: 8_000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(config.sample_rate).unwrap();
    let backend = Backend::default();
    app.start(&backend, None, config).unwrap();
    let before = app.project();
    let epoch = app.engine_epoch;
    let request = app.requested_revision;
    let accepted = app.submit_transport(TransportCommand::Play).unwrap();
    backend.faulted.store(true, Ordering::Release);
    assert_eq!(app.state(), EngineState::Faulted);
    assert!(!app.transport().unwrap().playing);
    assert_eq!(
        app.transport_command_status(accepted.engine_epoch, accepted.command_id)
            .unwrap()
            .result
            .unwrap()
            .cancel_reason,
        Some(audio_engine::TransportCancelReason::OutputUnavailable)
    );
    assert!(app.publication_status(0).output_faulted);
    assert!(app.start(&backend, None, config).is_err());
    assert_eq!(backend.opens.load(Ordering::Relaxed), 1);
    assert!(app.submit_transport(TransportCommand::Play).is_err());
    assert!(
        app.send_transport(TransportCommand::Seek(daw_core::SamplePosition(100)))
            .is_err()
    );
    app.set_track_gain(1, 0.25).unwrap();
    let edited = app.project();
    assert_eq!(app.requested_revision, request);
    assert_eq!(app.undo().unwrap(), before);
    assert_eq!(app.redo().unwrap(), edited);
    assert_eq!(app.state(), EngineState::Faulted);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("faulted.ongaku");
    app.save_project(&path).unwrap();
    assert_eq!(Project::load(&path).unwrap().snapshot(), edited);
    app.stop().unwrap();
    assert_eq!(app.state(), EngineState::Stopped);
    assert!(!app.publication_status(0).output_faulted);
    app.start(&backend, None, config).unwrap();
    assert_eq!(app.state(), EngineState::Running);
    assert!(app.engine_epoch > epoch);
    assert_eq!(app.project(), edited);
    assert!(
        app.transport_command_status(accepted.engine_epoch, accepted.command_id)
            .is_err()
    );
    let engine = app.publication_status(0).engine.unwrap();
    assert_eq!(engine.adopted_revision, app.requested_revision.unwrap());
    assert_eq!(
        engine.acknowledgements.last().unwrap().document_revision,
        app.revision
    );
}

#[test]
fn failure_during_startup_never_commits_an_engine_session() {
    let config = AudioConfig {
        sample_rate: 8_000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(config.sample_rate).unwrap();
    let backend = Backend {
        fail_on_start: true,
        ..Backend::default()
    };
    let before = app.project();
    assert!(app.start(&backend, None, config).is_err());
    assert_eq!(app.state(), EngineState::Stopped);
    assert_eq!(app.engine_epoch, 0);
    assert!(app.control.is_none());
    assert!(app.stream.is_none());
    assert_eq!(app.project(), before);
    app.start(&Backend::default(), None, config).unwrap();
    assert_eq!(app.state(), EngineState::Running);
}
