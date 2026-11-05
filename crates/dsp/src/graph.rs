use crate::{AudioProcessor, gain::GainNode, oscillator::SineOscillator};

/// Phase 1's immutable-topology graph: oscillator -> gain -> master output.
#[derive(Clone, Debug)]
pub struct PhaseOneGraph {
    oscillator: SineOscillator,
    gain: GainNode,
    channels: usize,
}

impl PhaseOneGraph {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            oscillator: SineOscillator::new(440.0, 0.2),
            gain: GainNode::new(0.25),
            channels: 0,
        }
    }

    pub fn prepare(&mut self, sample_rate: f32, channels: usize) {
        self.channels = channels;
        self.oscillator.prepare(sample_rate);
        self.gain.prepare(sample_rate, channels);
    }

    pub fn set_gain(&mut self, linear_gain: f32) {
        self.gain.set_gain(linear_gain);
    }

    pub fn process(&mut self, output: &mut [f32]) {
        self.oscillator.render(output, self.channels);
        self.gain.process(output, self.channels);
        for sample in output {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

impl Default for PhaseOneGraph {
    fn default() -> Self {
        Self::new()
    }
}
