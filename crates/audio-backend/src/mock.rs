use crate::{AudioBackend, AudioCallback, AudioOutputStream};
use audio_types::{
    AudioConfig, AudioDeviceInfo, AudioError, OutputConfiguration, OutputSampleFormat,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct MockAudioBackend {
    captured: Arc<Mutex<Vec<f32>>>,
}

impl MockAudioBackend {
    #[must_use]
    pub fn captured_audio(&self) -> Arc<Mutex<Vec<f32>>> {
        Arc::clone(&self.captured)
    }
}

impl AudioBackend for MockAudioBackend {
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        Ok(vec![AudioDeviceInfo {
            id: "mock-default".into(),
            name: "Mock Output".into(),
            is_default: true,
        }])
    }

    fn default_output_config(&self) -> Result<AudioConfig, AudioError> {
        Ok(AudioConfig::DEFAULT)
    }

    fn validate_output_config(
        &self,
        device_id: Option<&str>,
        config: AudioConfig,
    ) -> Result<(), AudioError> {
        config.validate()?;
        if let Some(id) = device_id
            && id != "mock-default"
        {
            return Err(AudioError::DeviceNotFound(id.into()));
        }
        Ok(())
    }

    fn open_output(
        &self,
        device_id: Option<&str>,
        config: AudioConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError> {
        self.validate_output_config(device_id, config)?;
        Ok(Box::new(MockOutputStream {
            config,
            callback,
            captured: Arc::clone(&self.captured),
            running: false,
        }))
    }
}

struct MockOutputStream {
    config: AudioConfig,
    callback: Box<dyn AudioCallback>,
    captured: Arc<Mutex<Vec<f32>>>,
    running: bool,
}

impl MockOutputStream {
    fn process_one_block(&mut self) -> Result<(), AudioError> {
        let sample_count = self.config.buffer_size as usize * usize::from(self.config.channels);
        let mut block = vec![0.0; sample_count];
        self.callback.process(&mut block);
        self.captured
            .lock()
            .map_err(|_| AudioError::Backend("mock capture lock was poisoned".into()))?
            .extend_from_slice(&block);
        Ok(())
    }
}

impl AudioOutputStream for MockOutputStream {
    fn output_configuration(&self) -> Option<OutputConfiguration> {
        Some(OutputConfiguration {
            device_id: "mock-default".into(),
            requested: self.config,
            validated: self.config,
            sample_format: OutputSampleFormat::F32,
            advertised_buffer_range: None,
        })
    }

    fn start(&mut self) -> Result<(), AudioError> {
        if !self.running {
            self.running = true;
            self.process_one_block()?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.running = false;
        Ok(())
    }
}
