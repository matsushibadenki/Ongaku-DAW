use crate::AudioProcessor;

/// A click-resistant gain stage using bounded per-sample linear smoothing.
#[derive(Clone, Debug)]
pub struct GainNode {
    current: f32,
    target: f32,
    smoothing_samples: u32,
}

impl GainNode {
    #[must_use]
    pub const fn new(linear_gain: f32) -> Self {
        Self {
            current: linear_gain,
            target: linear_gain,
            smoothing_samples: 1,
        }
    }

    pub fn set_gain(&mut self, linear_gain: f32) {
        self.target = sanitize_gain(linear_gain);
    }

    #[must_use]
    pub const fn current_gain(&self) -> f32 {
        self.current
    }
}

impl AudioProcessor for GainNode {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn prepare(&mut self, sample_rate: f32, _channels: usize) {
        // Audio sample rates are small positive integral values represented as f32 by the DSP API.
        self.smoothing_samples = (sample_rate * 0.005).max(1.0) as u32;
    }

    fn reset(&mut self) {
        self.current = self.target;
    }

    fn process(&mut self, interleaved: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let frames = interleaved.len() / channels;
        if frames == 0 {
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let smoothing = self.smoothing_samples.min(frames as u32).max(1) as f32;
        let increment = (self.target - self.current) / smoothing;

        for frame in interleaved.chunks_exact_mut(channels) {
            if (self.current - self.target).abs() > f32::EPSILON {
                self.current += increment;
                if increment.is_sign_positive() {
                    self.current = self.current.min(self.target);
                } else {
                    self.current = self.current.max(self.target);
                }
            }
            for sample in frame {
                *sample *= self.current;
            }
        }
    }
}

fn sanitize_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 2.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::GainNode;
    use crate::AudioProcessor;

    #[test]
    fn applies_gain_without_changing_buffer_shape() {
        let mut node = GainNode::new(0.5);
        node.prepare(48_000.0, 2);
        let mut audio = [1.0, -1.0, 0.5, -0.5];
        node.process(&mut audio, 2);
        let expected = [0.5, -0.5, 0.25, -0.25];
        assert!(
            audio
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn rejects_non_finite_targets() {
        let mut node = GainNode::new(1.0);
        node.set_gain(f32::NAN);
        node.reset();
        assert!(node.current_gain().abs() < f32::EPSILON);
    }
}
