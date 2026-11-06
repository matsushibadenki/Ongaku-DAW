use super::*;
use audio_backend::{AudioCallback, AudioOutputStream, mock::MockAudioBackend};
use audio_types::{OutputConfiguration, OutputSampleFormat};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy)]
enum Failure {
    Preflight,
    Open,
    Start,
    MismatchedReport,
}
struct Backend {
    failure: Failure,
    opens: AtomicUsize,
    starts: Arc<AtomicUsize>,
}
impl AudioBackend for Backend {
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        MockAudioBackend::default().output_devices()
    }
    fn default_output_config(&self) -> Result<AudioConfig, AudioError> {
        Ok(AudioConfig::DEFAULT)
    }
    fn validate_output_config(
        &self,
        _id: Option<&str>,
        config: AudioConfig,
    ) -> Result<(), AudioError> {
        config.validate()?;
        if matches!(self.failure, Failure::Preflight) {
            Err(AudioError::InvalidConfig("injected unsupported output"))
        } else {
            Ok(())
        }
    }
    fn open_output(
        &self,
        _id: Option<&str>,
        config: AudioConfig,
        _callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        if matches!(self.failure, Failure::Open) {
            return Err(AudioError::Backend("injected open failure".into()));
        }
        let validated = if matches!(self.failure, Failure::MismatchedReport) {
            AudioConfig {
                sample_rate: config.sample_rate + 1,
                ..config
            }
        } else {
            config
        };
        Ok(Box::new(Stream {
            fail: matches!(self.failure, Failure::Start),
            starts: Arc::clone(&self.starts),
            report: OutputConfiguration {
                device_id: "fixture".into(),
                requested: config,
                validated,
                sample_format: OutputSampleFormat::F32,
                advertised_buffer_range: None,
            },
        }))
    }
}
struct Stream {
    fail: bool,
    starts: Arc<AtomicUsize>,
    report: OutputConfiguration,
}
impl AudioOutputStream for Stream {
    fn output_configuration(&self) -> Option<OutputConfiguration> {
        Some(self.report.clone())
    }
    fn start(&mut self) -> Result<(), AudioError> {
        self.starts.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            Err(AudioError::Backend("injected start failure".into()))
        } else {
            Ok(())
        }
    }
    fn stop(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
}

#[test]
fn rejected_output_never_replaces_project_history_or_commits_a_partial_engine_session() {
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    for failure in [
        Failure::Preflight,
        Failure::Open,
        Failure::Start,
        Failure::MismatchedReport,
    ] {
        let mut app = DawApplication::with_sample_rate(8000).unwrap();
        app.set_track_gain(1, 0.25).unwrap();
        app.undo().unwrap();
        let before = app.project();
        let revision = app.revision;
        let undo = app.undo.len();
        let redo = app.redo.len();
        let backend = Backend {
            failure,
            opens: AtomicUsize::new(0),
            starts: Arc::new(AtomicUsize::new(0)),
        };
        assert!(app.start(&backend, None, config).is_err());
        assert_eq!(app.project(), before);
        assert_eq!(app.revision, revision);
        assert_eq!(app.undo.len(), undo);
        assert_eq!(app.redo.len(), redo);
        assert_eq!(app.state, EngineState::Stopped);
        assert_eq!(app.engine_epoch, 0);
        assert!(app.control.is_none() && app.stream.is_none() && app.active_config.is_none());
        assert!(app.publication_status(0).output_configuration.is_none());
        if matches!(failure, Failure::Preflight) {
            assert_eq!(app.project.streaming_status().prepared_media, 0);
            assert_eq!(backend.opens.load(Ordering::Relaxed), 0);
        }
        if matches!(failure, Failure::MismatchedReport) {
            assert_eq!(backend.starts.load(Ordering::Relaxed), 0);
        }
        // A failed startup must not poison the next legitimate session or redo branch.
        app.start(&MockAudioBackend::default(), None, config)
            .unwrap();
        assert_eq!(app.engine_epoch, 1);
        assert_eq!(app.project(), before);
        app.redo().unwrap();
        assert_eq!(app.project().tracks[0].gain.to_bits(), 0.25_f32.to_bits());
        app.stop().unwrap();
    }
}
#[test]
fn active_selection_is_reported_and_survives_history_until_the_engine_stops() {
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(8000).unwrap();
    app.start(&MockAudioBackend::default(), None, config)
        .unwrap();
    let selected = app.publication_status(0).output_configuration.unwrap();
    let epoch = app.engine_epoch;
    app.start(&MockAudioBackend::default(), Some("mock-default"), config)
        .unwrap();
    assert_eq!(app.engine_epoch, epoch);
    assert!(
        app.start(
            &MockAudioBackend::default(),
            None,
            AudioConfig {
                buffer_size: 256,
                ..config
            }
        )
        .is_err()
    );
    assert!(
        app.start(&MockAudioBackend::default(), Some("other-device"), config)
            .is_err()
    );
    assert_eq!(app.state, EngineState::Running);
    assert_eq!(
        app.publication_status(0).output_configuration.as_ref(),
        Some(&selected)
    );
    assert_eq!(selected.requested, config);
    assert_eq!(selected.validated, config);
    assert_eq!(selected.sample_format, OutputSampleFormat::F32);
    assert_eq!(selected.device_id, "mock-default");
    assert!(selected.advertised_buffer_range.is_none());
    app.set_track_gain(1, 0.5).unwrap();
    app.undo().unwrap();
    app.redo().unwrap();
    assert_eq!(
        app.publication_status(0).output_configuration.unwrap(),
        selected
    );
    app.stop().unwrap();
    assert!(app.publication_status(0).output_configuration.is_none());
    let epoch = app.engine_epoch;
    assert!(
        app.start(&MockAudioBackend::default(), Some("missing-device"), config)
            .is_err()
    );
    assert_eq!(app.engine_epoch, epoch);
    assert!(app.publication_status(0).output_configuration.is_none());
}
