//! Shared, backend-independent contracts for the audio system.

mod preparation;
pub use preparation::{PreparationCheck, PreparationToken, preparation_sort_by};

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Pull-based source contract used only from an audio callback.
///
/// Implementations must never allocate, lock, perform I/O, or block in
/// `read_interleaved`. Slow producers should return fewer samples; the mixer
/// supplies silence for the remainder.
pub trait RealtimeAudioSource: Send + 'static {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn read_interleaved(&mut self, output: &mut [f32]) -> usize;
}

/// Runtime format requested from an audio output backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_size: u32,
}

impl AudioConfig {
    pub const DEFAULT: Self = Self {
        sample_rate: 48_000,
        channels: 2,
        buffer_size: 128,
    };

    #[must_use]
    pub const fn callback_deadline_ns(self) -> u64 {
        self.buffer_size as u64 * 1_000_000_000 / self.sample_rate as u64
    }

    pub fn validate(self) -> Result<Self, AudioError> {
        if self.sample_rate == 0 {
            return Err(AudioError::InvalidConfig(
                "sample rate must be greater than zero",
            ));
        }
        if self.channels == 0 {
            return Err(AudioError::InvalidConfig(
                "channel count must be greater than zero",
            ));
        }
        if self.buffer_size == 0 {
            return Err(AudioError::InvalidConfig(
                "buffer size must be greater than zero",
            ));
        }
        Ok(self)
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeMetricsSnapshot {
    pub callback_count: u64,
    pub xrun_count: u64,
    pub backend_error_count: u64,
    pub average_callback_us: f64,
    pub maximum_callback_us: f64,
    pub p99_9_callback_us: f64,
    pub deadline_us: f64,
    pub cpu_load: f64,
}

/// Errors cross the non-realtime control boundary only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    PreparationCanceled,
    PublicationBusy,
    InvalidConfig(&'static str),
    Backend(String),
    InvalidState(&'static str),
    DeviceNotFound(String),
}

impl Display for AudioError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreparationCanceled => formatter.write_str("graph preparation canceled"),
            Self::PublicationBusy => formatter.write_str("graph publication queue is busy"),
            Self::InvalidConfig(message) | Self::InvalidState(message) => {
                formatter.write_str(message)
            }
            Self::Backend(message) => write!(formatter, "audio backend error: {message}"),
            Self::DeviceNotFound(device) => write!(formatter, "audio device not found: {device}"),
        }
    }
}

impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::AudioConfig;

    #[test]
    fn deadline_matches_128_samples_at_48_khz() {
        assert_eq!(AudioConfig::DEFAULT.callback_deadline_ns(), 2_666_666);
    }
}
