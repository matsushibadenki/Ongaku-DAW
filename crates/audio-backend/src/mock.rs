use crate::{AudioBackend, AudioCallback, AudioOutputStream};
use audio_types::{AudioConfig, AudioDeviceInfo, AudioError};
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

    fn open_output(
        &self,
        _device_id: Option<&str>,
        config: AudioConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<Box<dyn AudioOutputStream>, AudioError> {
        config.validate()?;
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
