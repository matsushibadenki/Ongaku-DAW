use crate::AudioProcessor;

/// A click-resistant gain stage using bounded per-sample linear smoothing.
#[derive(Clone, Debug)]
pub struct GainNode {
    current: f32,
    target: f32,
    smoothing_samples: u32,
    remaining_samples: u32,
    increment: f32,
}

impl GainNode {
    #[must_use]
    pub const fn new(linear_gain: f32) -> Self {
        let gain = if !linear_gain.is_finite() || linear_gain < 0.0 {
            0.0
        } else if linear_gain > 2.0 {
            2.0
        } else {
            linear_gain
        };
        Self {
            current: gain,
            target: gain,
            smoothing_samples: 1,
            remaining_samples: 0,
            increment: 0.0,
        }
    }

    pub fn set_gain(&mut self, linear_gain: f32) {
        let target = sanitize_gain(linear_gain);
        if target != self.target {
            self.target = target;
            self.restart_ramp();
        }
    }

    fn restart_ramp(&mut self) {
        self.remaining_samples = self.smoothing_samples;
        #[allow(clippy::cast_precision_loss)]
        {
            self.increment = (self.target - self.current) / self.remaining_samples as f32;
        }
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
        if self.remaining_samples > 0 {
            self.restart_ramp();
        }
    }

    fn reset(&mut self) {
        self.current = self.target;
        self.remaining_samples = 0;
        self.increment = 0.0;
    }

    fn process(&mut self, interleaved: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        // The ramp advances by samples, never by callback length.
        for frame in interleaved.chunks_exact_mut(channels) {
            if self.remaining_samples > 0 {
                self.current += self.increment;
                self.remaining_samples -= 1;
                if self.remaining_samples == 0 {
                    self.current = self.target;
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

#[cfg(test)]
mod p0_regressions {
    use super::*;
    fn render(partitions: &[usize]) -> Vec<f32> {
        let mut gain = GainNode::new(0.0);
        gain.prepare(48000.0, 2);
        gain.set_gain(1.0);
        let mut output = vec![1.0; 1024];
        let mut frame = 0;
        let mut index = 0;
        while frame < 512 {
            let count = partitions[index % partitions.len()].min(512 - frame);
            gain.process(&mut output[frame * 2..(frame + count) * 2], 2);
            frame += count;
            index += 1;
        }
        output
    }
    #[test]
    fn smoothing_is_independent_of_callback_partition() {
        let expected = render(&[512]);
        for partitions in [&[1][..], &[17, 127, 64, 257][..], &[128][..]] {
            assert_eq!(
                render(partitions),
                expected,
                "partition-dependent gain ramp: {partitions:?}"
            );
        }
    }
}
