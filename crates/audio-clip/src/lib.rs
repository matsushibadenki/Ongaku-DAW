//! Immutable PCM assets and non-destructive timeline clip views.

use audio_types::AudioError;
use daw_core::SamplePosition;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PcmAsset {
    sample_rate: u32,
    channels: u16,
    interleaved: Arc<[f32]>,
}

impl PcmAsset {
    pub fn new(
        sample_rate: u32,
        channels: u16,
        mut interleaved: Vec<f32>,
    ) -> Result<Self, AudioError> {
        if sample_rate == 0 || channels == 0 {
            return Err(AudioError::InvalidConfig(
                "PCM format must have a sample rate and channels",
            ));
        }
        if !interleaved.len().is_multiple_of(usize::from(channels)) {
            return Err(AudioError::InvalidConfig(
                "PCM sample count must contain complete frames",
            ));
        }
        for sample in &mut interleaved {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }
        Ok(Self {
            sample_rate,
            channels,
            interleaved: interleaved.into(),
        })
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    #[must_use]
    pub fn frame_count(&self) -> u64 {
        u64::try_from(self.interleaved.len() / usize::from(self.channels)).unwrap_or(u64::MAX)
    }

    #[must_use]
    pub fn sample(&self, frame: u64, channel: usize) -> f32 {
        let source_channel = channel.min(usize::from(self.channels) - 1);
        let Ok(frame) = usize::try_from(frame) else {
            return 0.0;
        };
        let Some(index) = frame
            .checked_mul(usize::from(self.channels))
            .and_then(|frame_start| frame_start.checked_add(source_channel))
        else {
            return 0.0;
        };
        self.interleaved.get(index).copied().unwrap_or(0.0)
    }

    #[must_use]
    pub fn interleaved_samples(&self) -> &[f32] {
        &self.interleaved
    }
}

#[derive(Clone, Debug)]
pub struct AudioClip {
    pub timeline_start: SamplePosition,
    pub source_start_frame: u64,
    pub duration_frames: u64,
    pub asset: Arc<PcmAsset>,
}

impl AudioClip {
    pub fn new(
        timeline_start: SamplePosition,
        source_start_frame: u64,
        duration_frames: u64,
        asset: Arc<PcmAsset>,
    ) -> Result<Self, AudioError> {
        let available = asset.frame_count().saturating_sub(source_start_frame);
        if duration_frames == 0 || duration_frames > available {
            return Err(AudioError::InvalidConfig(
                "clip range exceeds its PCM asset",
            ));
        }
        Ok(Self {
            timeline_start,
            source_start_frame,
            duration_frames,
            asset,
        })
    }

    #[must_use]
    pub fn sample_at(&self, timeline_sample: SamplePosition, channel: usize) -> f32 {
        let Some(relative) = timeline_sample.0.checked_sub(self.timeline_start.0) else {
            return 0.0;
        };
        if relative >= self.duration_frames {
            return 0.0;
        }
        self.asset
            .sample(self.source_start_frame + relative, channel)
    }

    #[must_use]
    pub const fn timeline_end(&self) -> SamplePosition {
        self.timeline_start.saturating_add(self.duration_frames)
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioClip, PcmAsset};
    use daw_core::SamplePosition;
    use std::sync::Arc;

    #[test]
    fn clip_maps_timeline_to_non_destructive_source_range() {
        let asset = Arc::new(PcmAsset::new(48_000, 1, vec![0.1, 0.2, 0.3, 0.4]).unwrap());
        let clip = AudioClip::new(SamplePosition(10), 1, 2, asset).unwrap();
        assert!((clip.sample_at(SamplePosition(10), 0) - 0.2).abs() < f32::EPSILON);
        assert!(clip.sample_at(SamplePosition(12), 0).abs() < f32::EPSILON);
    }
}
