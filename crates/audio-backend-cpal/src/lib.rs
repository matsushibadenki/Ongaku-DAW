//! CPAL adapter. No CPAL type crosses this crate's public boundary.

#![cfg_attr(test, allow(unsafe_code))]

mod boundary;
mod negotiation;

use audio_backend::{AudioBackend, AudioCallback, AudioOutputStream};
use audio_types::{
    AudioConfig, AudioDeviceInfo, AudioError, OutputConfiguration, OutputSampleFormat,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use cpal::{
    BufferSize, SampleRate, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CpalAudioBackend;

impl AudioBackend for CpalAudioBackend {
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        let host = cpal::default_host();
        let default_id = host
            .default_output_device()
            .and_then(|device| device.id().ok());
        let devices = host
            .output_devices()
            .map_err(backend_error)?
            .filter_map(|device| {
                let id = device.id().ok()?;
                Some(AudioDeviceInfo {
                    id: id.to_string(),
                    is_default: default_id.as_ref() == Some(&id),
                    name: device.to_string(),
                })
            })
            .collect();
        Ok(devices)
    }

    fn default_output_config(&self) -> Result<AudioConfig, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| AudioError::Backend("no default output device is available".into()))?;
        let config = device.default_output_config().map_err(backend_error)?;
        let ranges = device
            .supported_output_configs()
            .map_err(backend_error)?
            .collect::<Vec<_>>();
        negotiation::default_config(&ranges, config.sample_rate(), config.channels())
    }

    fn validate_output_config(
        &self,
        device_id: Option<&str>,
        config: AudioConfig,
    ) -> Result<(), AudioError> {
        config.validate()?;
        let device = select_device(&cpal::default_host(), device_id)?;
        let ranges = device
            .supported_output_configs()
            .map_err(backend_error)?
            .collect::<Vec<_>>();
        negotiation::select(&ranges, config).map(|_| ())
    }

    fn open_output(
        &self,
        device_id: Option<&str>,
        config: AudioConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError> {
        let config = config.validate()?;
        let host = cpal::default_host();
        let device = select_device(&host, device_id)?;
        let ranges = device
            .supported_output_configs()
            .map_err(backend_error)?
            .collect::<Vec<_>>();
        let selection = negotiation::select(&ranges, config)?;
        let output = OutputConfiguration {
            device_id: device.id().map_err(backend_error)?.to_string(),
            requested: config,
            validated: config,
            sample_format: selection.format,
            advertised_buffer_range: selection.buffer_range,
        };
        let stream_config = StreamConfig {
            channels: config.channels,
            sample_rate: config.sample_rate as SampleRate,
            buffer_size: BufferSize::Fixed(config.buffer_size),
        };
        let faulted = Arc::new(AtomicBool::new(false));
        let callback = Box::new(boundary::FaultGuard::new(callback, Arc::clone(&faulted)));
        let error_faulted = Arc::clone(&faulted);
        let event_sink = callback.event_sink();
        let error_callback = move |error: cpal::Error| {
            if requires_rebuild(error.kind()) {
                error_faulted.store(true, Ordering::Release);
            }
            if let Some(sink) = &event_sink {
                sink.report_backend_error();
            }
        };

        let stream = match selection.format {
            OutputSampleFormat::F32 => {
                build_f32_stream(&device, stream_config, callback, error_callback)
            }
            OutputSampleFormat::I16 => {
                build_converted_stream::<i16, _>(&device, stream_config, callback, error_callback)
            }
            OutputSampleFormat::U16 => {
                build_converted_stream::<u16, _>(&device, stream_config, callback, error_callback)
            }
        }?;
        Ok(Box::new(CpalOutputStream {
            stream,
            running: false,
            output,
            faulted,
        }))
    }
}

fn select_device(host: &cpal::Host, device_id: Option<&str>) -> Result<cpal::Device, AudioError> {
    let Some(requested) = device_id else {
        return host
            .default_output_device()
            .ok_or_else(|| AudioError::Backend("no default output device is available".into()));
    };
    let id = requested
        .parse()
        .map_err(|_| AudioError::DeviceNotFound(requested.to_owned()))?;
    host.device_by_id(&id)
        .ok_or_else(|| AudioError::DeviceNotFound(requested.to_owned()))
}

fn build_f32_stream<E>(
    device: &cpal::Device,
    config: StreamConfig,
    mut callback: Box<dyn AudioCallback>,
    error_callback: E,
) -> Result<Stream, AudioError>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    let observed_config = boundary_config(&config)?;
    let sink = callback.event_sink();
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| {
                boundary::process_f32(&mut *callback, observed_config, data, sink.as_deref());
            },
            error_callback,
            None,
        )
        .map_err(backend_error)
}

fn build_converted_stream<T, E>(
    device: &cpal::Device,
    config: StreamConfig,
    callback: Box<dyn AudioCallback>,
    error_callback: E,
) -> Result<Stream, AudioError>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let observed_config = boundary_config(&config)?;
    let channels = usize::from(config.channels);
    let capacity = channels
        .checked_mul(observed_config.buffer_size as usize)
        .ok_or(AudioError::InvalidConfig(
            "output scratch capacity overflow",
        ))?;
    let mut adapter = boundary::ConvertedOutput::<T>::new(callback, observed_config, capacity)?;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| adapter.process(data),
            error_callback,
            None,
        )
        .map_err(backend_error)
}

fn boundary_config(config: &StreamConfig) -> Result<AudioConfig, AudioError> {
    let BufferSize::Fixed(frames) = config.buffer_size else {
        return Err(AudioError::InvalidConfig(
            "output observation requires an explicit fixed-buffer request",
        ));
    };
    AudioConfig {
        channels: config.channels,
        sample_rate: config.sample_rate,
        buffer_size: frames,
    }
    .validate()
}

struct CpalOutputStream {
    stream: Stream,
    running: bool,
    output: OutputConfiguration,
    faulted: Arc<AtomicBool>,
}

impl AudioOutputStream for CpalOutputStream {
    fn is_faulted(&self) -> bool {
        self.faulted.load(Ordering::Acquire)
    }

    fn output_configuration(&self) -> Option<OutputConfiguration> {
        Some(self.output.clone())
    }
    fn start(&mut self) -> Result<(), AudioError> {
        if self.is_faulted() {
            return Err(AudioError::InvalidState(
                "output stream failed; stop and restart the engine",
            ));
        }
        if !self.running {
            self.stream.play().map_err(backend_error)?;
            self.running = true;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        // Failed devices may reject pause. Dropping the stream on the control
        // thread releases it and allows a new selected device to be opened.
        if self.is_faulted() {
            self.running = false;
            return Ok(());
        }
        if self.running {
            self.stream.pause().map_err(backend_error)?;
            self.running = false;
        }
        Ok(())
    }
}

fn requires_rebuild(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}

fn backend_error(error: impl std::fmt::Display) -> AudioError {
    AudioError::Backend(error.to_string())
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn only_stream_continuing_notifications_avoid_quarantine() {
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
        ] {
            assert!(!requires_rebuild(kind));
        }
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::HostUnavailable,
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::BackendError,
            cpal::ErrorKind::Other,
        ] {
            assert!(requires_rebuild(kind));
        }
    }
}
