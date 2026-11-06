//! Backend contracts kept independent from CPAL and the application shell.

pub mod mock;

use audio_types::OutputFault;
use audio_types::{AudioConfig, AudioDeviceInfo, AudioError, OutputConfiguration};
use std::{sync::Arc, time::Duration};

pub trait RealtimeEventSink: Send + Sync + 'static {
    /// Driver/backend notification; not necessarily a hardware xrun.
    fn report_backend_error(&self);
    /// Default forwarding preserves existing third-party sinks.
    fn report_output_fault(&self, _fault: OutputFault) {
        self.report_backend_error();
    }
    /// Entire adapter body, including source work/conversion, excluding observer recording.
    fn report_output_callback(
        &self,
        _config: AudioConfig,
        _sample_count: usize,
        _duration: Duration,
    ) {
    }
}

/// Called only by an audio backend's realtime output thread.
pub trait AudioCallback: Send + 'static {
    /// Fills an interleaved `f32` output buffer. This method must not allocate or block.
    fn process(&mut self, output: &mut [f32]);
    fn event_sink(&self) -> Option<Arc<dyn RealtimeEventSink>> {
        None
    }
}

pub trait AudioOutputStream: Send {
    fn output_configuration(&self) -> Option<OutputConfiguration> {
        None
    }
    fn start(&mut self) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<(), AudioError>;
}

/// Pluggable control-thread interface for physical and simulated audio backends.
pub trait AudioBackend {
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError>;
    fn default_output_config(&self) -> Result<AudioConfig, AudioError>;
    /// Control-thread preflight. Backends must revalidate when actually opening.
    fn validate_output_config(
        &self,
        _device_id: Option<&str>,
        config: AudioConfig,
    ) -> Result<(), AudioError> {
        config.validate().map(|_| ())
    }
    fn open_output(
        &self,
        device_id: Option<&str>,
        config: AudioConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Legacy(AtomicUsize);
    impl RealtimeEventSink for Legacy {
        fn report_backend_error(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[test]
    fn typed_faults_preserve_legacy_sink_error_notifications() {
        let sink = Legacy(AtomicUsize::new(0));
        for fault in [
            OutputFault::MalformedFrame,
            OutputFault::CapacityExceeded,
            OutputFault::NonFiniteSample,
        ] {
            sink.report_output_fault(fault);
        }
        sink.report_output_callback(AudioConfig::DEFAULT, 256, Duration::ZERO);
        assert_eq!(sink.0.load(Ordering::Relaxed), 3);
    }
}
