//! Backend contracts kept independent from CPAL and the application shell.

pub mod mock;

use audio_types::{AudioConfig, AudioDeviceInfo, AudioError};
use std::sync::Arc;

pub trait RealtimeEventSink: Send + Sync + 'static {
    fn report_backend_error(&self);
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
    fn start(&mut self) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<(), AudioError>;
}

/// Pluggable control-thread interface for physical and simulated audio backends.
pub trait AudioBackend {
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError>;
    fn default_output_config(&self) -> Result<AudioConfig, AudioError>;
    fn open_output(
        &self,
        device_id: Option<&str>,
        config: AudioConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError>;
}
