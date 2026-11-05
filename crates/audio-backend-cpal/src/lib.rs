//! CPAL adapter. No CPAL type crosses this crate's public boundary.

use audio_backend::{AudioBackend, AudioCallback, AudioOutputStream};
use audio_types::{AudioConfig, AudioDeviceInfo, AudioError};
use cpal::{
    BufferSize, SampleFormat, SampleRate, Stream, StreamConfig,
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
        Ok(AudioConfig {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
            buffer_size: 128,
        })
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
        let sample_format = device
            .default_output_config()
            .map_err(backend_error)?
            .sample_format();
        let stream_config = StreamConfig {
            channels: config.channels,
            sample_rate: config.sample_rate as SampleRate,
            buffer_size: BufferSize::Fixed(config.buffer_size),
        };
        let event_sink = callback.event_sink();
        let error_callback = move |_error: cpal::Error| {
            if let Some(sink) = &event_sink {
                sink.report_backend_error();
            }
        };

        let stream = match sample_format {
            SampleFormat::F32 => build_f32_stream(&device, stream_config, callback, error_callback),
            SampleFormat::I16 => build_i16_stream(&device, stream_config, callback, error_callback),
            SampleFormat::U16 => build_u16_stream(&device, stream_config, callback, error_callback),
            other => Err(AudioError::Backend(format!(
                "unsupported output sample format: {other:?}"
            ))),
        }?;
        Ok(Box::new(CpalOutputStream {
            stream,
            running: false,
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
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| callback.process(data),
            error_callback,
            None,
        )
        .map_err(backend_error)
}

#[allow(clippy::cast_possible_truncation)]
fn build_i16_stream<E>(
    device: &cpal::Device,
    config: StreamConfig,
    mut callback: Box<dyn AudioCallback>,
    error_callback: E,
) -> Result<Stream, AudioError>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    let capacity = config.channels as usize * buffer_capacity(&config);
    let mut scratch = vec![0.0_f32; capacity];
    device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| {
                if data.len() > scratch.len() {
                    data.fill(0);
                    return;
                }
                let buffer = &mut scratch[..data.len()];
                callback.process(buffer);
                for (output, sample) in data.iter_mut().zip(buffer.iter().copied()) {
                    *output = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                }
            },
            error_callback,
            None,
        )
        .map_err(backend_error)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn build_u16_stream<E>(
    device: &cpal::Device,
    config: StreamConfig,
    mut callback: Box<dyn AudioCallback>,
    error_callback: E,
) -> Result<Stream, AudioError>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    let capacity = config.channels as usize * buffer_capacity(&config);
    let mut scratch = vec![0.0_f32; capacity];
    device
        .build_output_stream(
            config,
            move |data: &mut [u16], _| {
                if data.len() > scratch.len() {
                    data.fill(u16::MAX / 2);
                    return;
                }
                let buffer = &mut scratch[..data.len()];
                callback.process(buffer);
                for (output, sample) in data.iter_mut().zip(buffer.iter().copied()) {
                    *output = ((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * f32::from(u16::MAX)) as u16;
                }
            },
            error_callback,
            None,
        )
        .map_err(backend_error)
}

fn buffer_capacity(config: &StreamConfig) -> usize {
    match config.buffer_size {
        BufferSize::Fixed(frames) => frames as usize,
        BufferSize::Default => 4_096,
    }
}

struct CpalOutputStream {
    stream: Stream,
    running: bool,
}

impl AudioOutputStream for CpalOutputStream {
    fn start(&mut self) -> Result<(), AudioError> {
        if !self.running {
            self.stream.play().map_err(backend_error)?;
            self.running = true;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        if self.running {
            self.stream.pause().map_err(backend_error)?;
            self.running = false;
        }
        Ok(())
    }
}

fn backend_error(error: impl std::fmt::Display) -> AudioError {
    AudioError::Backend(error.to_string())
}
