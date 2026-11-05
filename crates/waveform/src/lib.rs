//! Multiresolution waveform analysis for timeline and overview renderers.

use audio_clip::PcmAsset;
use audio_types::AudioError;
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, mpsc},
    thread,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformPeak {
    pub minimum: f32,
    pub maximum: f32,
    pub rms: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaveformLevel {
    pub frames_per_peak: u64,
    pub peaks: Arc<[WaveformPeak]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaveformPyramid {
    levels: Vec<WaveformLevel>,
}

impl WaveformPyramid {
    pub fn analyze(asset: &PcmAsset, base_frames_per_peak: u64) -> Result<Self, AudioError> {
        if base_frames_per_peak == 0 {
            return Err(AudioError::InvalidConfig(
                "waveform resolution must be greater than zero",
            ));
        }
        let mut levels = Vec::new();
        let mut frames_per_peak = base_frames_per_peak;
        let mut peaks = analyze_level(asset, frames_per_peak);
        loop {
            levels.push(WaveformLevel {
                frames_per_peak,
                peaks: peaks.clone().into(),
            });
            if peaks.len() <= 1 {
                break;
            }
            peaks = reduce_level(&peaks);
            frames_per_peak = frames_per_peak.saturating_mul(2);
        }
        Ok(Self { levels })
    }

    #[must_use]
    pub fn levels(&self) -> &[WaveformLevel] {
        &self.levels
    }

    #[must_use]
    /// Selects the coarsest useful level for the requested display budget.
    ///
    /// # Panics
    ///
    /// Panics only if the pyramid invariant is violated and it contains no levels.
    /// Pyramids returned by [`Self::analyze`] always contain at least one level.
    pub fn level_for_max_peaks(&self, maximum_peaks: usize) -> &WaveformLevel {
        self.levels
            .iter()
            .find(|level| level.peaks.len() <= maximum_peaks.max(1))
            .unwrap_or_else(|| self.levels.last().expect("pyramid always has one level"))
    }
}

pub fn spawn_analysis(
    asset: Arc<PcmAsset>,
    base_frames_per_peak: u64,
) -> Result<mpsc::Receiver<Result<WaveformPyramid, AudioError>>, AudioError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("ongaku-waveform".into())
        .spawn(move || {
            let _ = sender.send(WaveformPyramid::analyze(&asset, base_frames_per_peak));
        })
        .map_err(|error| {
            AudioError::Backend(format!("failed to start waveform worker: {error}"))
        })?;
    Ok(receiver)
}

fn analyze_level(asset: &PcmAsset, frames_per_peak: u64) -> Vec<WaveformPeak> {
    let peak_count = asset.frame_count().div_ceil(frames_per_peak);
    let mut peaks = Vec::with_capacity(usize::try_from(peak_count).unwrap_or(usize::MAX));
    for peak_index in 0..peak_count {
        let start = peak_index.saturating_mul(frames_per_peak);
        let end = start
            .saturating_add(frames_per_peak)
            .min(asset.frame_count());
        let mut minimum = 1.0_f32;
        let mut maximum = -1.0_f32;
        let mut sum_squares = 0.0_f64;
        let mut sample_count = 0.0_f64;
        for frame in start..end {
            for channel in 0..usize::from(asset.channels()) {
                let sample = asset.sample(frame, channel);
                minimum = minimum.min(sample);
                maximum = maximum.max(sample);
                sum_squares += f64::from(sample) * f64::from(sample);
                sample_count += 1.0;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let rms = if sample_count == 0.0 {
            0.0
        } else {
            (sum_squares / sample_count).sqrt() as f32
        };
        peaks.push(WaveformPeak {
            minimum: if sample_count == 0.0 { 0.0 } else { minimum },
            maximum: if sample_count == 0.0 { 0.0 } else { maximum },
            rms,
        });
    }
    peaks
}

fn reduce_level(input: &[WaveformPeak]) -> Vec<WaveformPeak> {
    input
        .chunks(2)
        .map(|chunk| {
            let first = chunk[0];
            let second = chunk.get(1).copied().unwrap_or(first);
            WaveformPeak {
                minimum: first.minimum.min(second.minimum),
                maximum: first.maximum.max(second.maximum),
                rms: ((first.rms.mul_add(first.rms, second.rms * second.rms)) * 0.5).sqrt(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{WaveformPyramid, spawn_analysis};
    use audio_clip::PcmAsset;
    use std::sync::Arc;

    #[test]
    fn builds_a_multiresolution_peak_pyramid() {
        let asset = PcmAsset::new(48_000, 1, vec![-1.0, 0.5, -0.25, 1.0]).unwrap();
        let pyramid = WaveformPyramid::analyze(&asset, 2).unwrap();
        assert_eq!(pyramid.levels().len(), 2);
        assert!((pyramid.levels()[0].peaks[0].minimum + 1.0).abs() < f32::EPSILON);
        assert!((pyramid.levels()[1].peaks[0].maximum - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn analysis_can_run_on_a_background_worker() {
        let asset = Arc::new(PcmAsset::new(48_000, 1, vec![0.25; 64]).unwrap());
        let result = spawn_analysis(asset, 8).unwrap().recv().unwrap().unwrap();
        assert_eq!(result.levels()[0].peaks.len(), 8);
    }
}
