use std::f32::consts::TAU;

#[derive(Clone, Debug)]
pub struct SineOscillator {
    phase: f32,
    phase_increment: f32,
    frequency_hz: f32,
    amplitude: f32,
}

impl SineOscillator {
    #[must_use]
    pub const fn new(frequency_hz: f32, amplitude: f32) -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
            frequency_hz,
            amplitude,
        }
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        self.phase_increment = TAU * self.frequency_hz / sample_rate;
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    pub fn render(&mut self, interleaved: &mut [f32], channels: usize) {
        if channels == 0 {
            interleaved.fill(0.0);
            return;
        }
        for frame in interleaved.chunks_exact_mut(channels) {
            let value = self.phase.sin() * self.amplitude;
            frame.fill(value);
            self.phase += self.phase_increment;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SineOscillator;

    #[test]
    fn renders_the_same_signal_to_each_channel() {
        let mut oscillator = SineOscillator::new(1_000.0, 0.25);
        oscillator.prepare(48_000.0);
        let mut output = [0.0; 8];
        oscillator.render(&mut output, 2);
        for frame in output.chunks_exact(2) {
            assert!((frame[0] - frame[1]).abs() < f32::EPSILON);
        }
    }
}
