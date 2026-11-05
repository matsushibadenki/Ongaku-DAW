//! Allocation-free DSP building blocks.

pub mod gain;
pub mod graph;
pub mod oscillator;

/// Lifecycle shared by DSP nodes that can process interleaved audio in place.
pub trait AudioProcessor {
    fn prepare(&mut self, sample_rate: f32, channels: usize);
    fn reset(&mut self);
    fn process(&mut self, interleaved: &mut [f32], channels: usize);
    fn latency_samples(&self) -> u32 {
        0
    }
}
