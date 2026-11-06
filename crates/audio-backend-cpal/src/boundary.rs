//! Prepared output adapters: no resize, allocation or I/O in processing.
use audio_backend::{AudioCallback, RealtimeEventSink};
use audio_types::{AudioConfig, AudioError, OutputFault};
use cpal::{FromSample, Sample};
use std::{marker::PhantomData, sync::Arc, time::Instant};

struct Observation<'a> {
    sink: Option<&'a dyn RealtimeEventSink>,
    config: AudioConfig,
    samples: usize,
    started: Instant,
}
impl Drop for Observation<'_> {
    fn drop(&mut self) {
        if let Some(sink) = self.sink {
            sink.report_output_callback(self.config, self.samples, self.started.elapsed());
        }
    }
}
fn report(sink: Option<&dyn RealtimeEventSink>, fault: OutputFault) {
    if let Some(sink) = sink {
        sink.report_output_fault(fault);
    }
}
fn normalized(sample: f32, invalid: &mut bool) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        *invalid = true;
        0.0
    }
}
pub(super) fn process_f32(
    callback: &mut dyn AudioCallback,
    config: AudioConfig,
    output: &mut [f32],
    sink: Option<&dyn RealtimeEventSink>,
) {
    let _observation = Observation {
        sink,
        config,
        samples: output.len(),
        started: Instant::now(),
    };
    let channels = usize::from(config.channels);
    output.fill(0.0);
    if channels == 0 || !output.len().is_multiple_of(channels) {
        report(sink, OutputFault::MalformedFrame);
        return;
    }
    callback.process(output);
    let mut invalid = false;
    for sample in output {
        *sample = normalized(*sample, &mut invalid);
    }
    if invalid {
        report(sink, OutputFault::NonFiniteSample);
    }
}

pub(super) struct ConvertedOutput<T> {
    callback: Box<dyn AudioCallback>,
    scratch: Vec<f32>,
    config: AudioConfig,
    sink: Option<Arc<dyn RealtimeEventSink>>,
    marker: PhantomData<T>,
}
impl<T: Sample + FromSample<f32>> ConvertedOutput<T> {
    pub fn new(
        callback: Box<dyn AudioCallback>,
        config: AudioConfig,
        capacity: usize,
    ) -> Result<Self, AudioError> {
        config.validate()?;
        let channels = usize::from(config.channels);
        if capacity == 0 || !capacity.is_multiple_of(channels) {
            return Err(AudioError::InvalidConfig(
                "invalid converted output capacity",
            ));
        }
        let sink = callback.event_sink();
        Ok(Self {
            callback,
            scratch: vec![0.0; capacity],
            config,
            sink,
            marker: PhantomData,
        })
    }
    pub fn process(&mut self, output: &mut [T]) {
        let _observation = Observation {
            sink: self.sink.as_deref(),
            config: self.config,
            samples: output.len(),
            started: Instant::now(),
        };
        output.fill(T::EQUILIBRIUM);
        if !output
            .len()
            .is_multiple_of(usize::from(self.config.channels))
        {
            report(self.sink.as_deref(), OutputFault::MalformedFrame);
            return;
        }
        if output.len() > self.scratch.len() {
            report(self.sink.as_deref(), OutputFault::CapacityExceeded);
            return;
        }
        let scratch = &mut self.scratch[..output.len()];
        scratch.fill(0.0);
        self.callback.process(scratch);
        let mut invalid = false;
        for (out, sample) in output.iter_mut().zip(scratch.iter().copied()) {
            *out = T::from_sample(normalized(sample, &mut invalid));
        }
        if invalid {
            report(self.sink.as_deref(), OutputFault::NonFiniteSample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Allocator;
    static EVENTS: AtomicUsize = AtomicUsize::new(0);
    thread_local! { static AUDIO: Cell<bool> = const { Cell::new(false) }; }
    // SAFETY: all operations forward unchanged allocation arguments to System.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if AUDIO.with(Cell::get) {
                EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if AUDIO.with(Cell::get) {
                EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            unsafe {
                System.dealloc(ptr, layout);
            }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if AUDIO.with(Cell::get) {
                EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;
    fn realtime(work: impl FnOnce()) {
        let before = EVENTS.load(Ordering::Relaxed);
        AUDIO.with(|flag| flag.set(true));
        work();
        AUDIO.with(|flag| flag.set(false));
        assert_eq!(EVENTS.load(Ordering::Relaxed), before);
    }
    #[derive(Default)]
    struct Probe {
        calls: AtomicUsize,
        errors: AtomicUsize,
        observations: AtomicUsize,
        samples: AtomicUsize,
        malformed: AtomicUsize,
        capacity: AtomicUsize,
        nonfinite: AtomicUsize,
        requested: AtomicUsize,
    }
    impl RealtimeEventSink for Probe {
        fn report_output_callback(
            &self,
            config: AudioConfig,
            samples: usize,
            _duration: std::time::Duration,
        ) {
            self.requested
                .store(config.buffer_size as usize, Ordering::Relaxed);
            self.samples.store(samples, Ordering::Relaxed);
            self.observations.fetch_add(1, Ordering::Relaxed);
        }
        fn report_output_fault(&self, fault: OutputFault) {
            match fault {
                OutputFault::MalformedFrame => &self.malformed,
                OutputFault::CapacityExceeded => &self.capacity,
                OutputFault::NonFiniteSample => &self.nonfinite,
            }
            .fetch_add(1, Ordering::Relaxed);
            self.report_backend_error();
        }
        fn report_backend_error(&self) {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
    struct Source {
        samples: &'static [f32],
        probe: Arc<Probe>,
    }
    impl AudioCallback for Source {
        fn process(&mut self, output: &mut [f32]) {
            self.probe.calls.fetch_add(1, Ordering::Relaxed);
            for (output, sample) in output.iter_mut().zip(self.samples) {
                *output = *sample;
            }
        }
        fn event_sink(&self) -> Option<Arc<dyn RealtimeEventSink>> {
            Some(self.probe.clone())
        }
    }
    fn source(samples: &'static [f32]) -> (Box<dyn AudioCallback>, Arc<Probe>) {
        let probe = Arc::new(Probe::default());
        (
            Box::new(Source {
                samples,
                probe: Arc::clone(&probe),
            }),
            probe,
        )
    }
    fn config(frames: u32) -> AudioConfig {
        AudioConfig {
            buffer_size: frames,
            ..AudioConfig::DEFAULT
        }
    }
    const SAMPLES: &[f32] = &[
        -2.0,
        -1.0,
        -0.5,
        0.0,
        0.5,
        1.0,
        2.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    #[test]
    fn signed_and_unsigned_pcm_preserve_equilibrium_endpoints_and_half_scale() {
        let (callback, probe) = source(SAMPLES);
        let mut signed = ConvertedOutput::<i16>::new(callback, config(5), 10).unwrap();
        let mut output = [1; 10];
        realtime(|| signed.process(&mut output));
        assert_eq!(
            output,
            [-32768, -32768, -16384, 0, 16384, 32767, 32767, 0, 0, 0]
        );
        assert_eq!(probe.errors.load(Ordering::Relaxed), 1);
        assert_eq!(probe.nonfinite.load(Ordering::Relaxed), 1);
        assert_eq!(probe.observations.load(Ordering::Relaxed), 1);
        let (callback, probe) = source(SAMPLES);
        let mut unsigned = ConvertedOutput::<u16>::new(callback, config(5), 10).unwrap();
        let mut output = [1; 10];
        realtime(|| unsigned.process(&mut output));
        assert_eq!(
            output,
            [0, 0, 16384, 32768, 49152, 65535, 65535, 32768, 32768, 32768]
        );
        assert_eq!(probe.errors.load(Ordering::Relaxed), 1);
        assert_eq!(probe.nonfinite.load(Ordering::Relaxed), 1);
        assert_eq!(probe.observations.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn oversized_and_partial_frames_are_silent_reported_and_do_not_advance_sources() {
        let (callback, probe) = source(&[0.5; 4]);
        let mut adapter = ConvertedOutput::<u16>::new(callback, config(2), 4).unwrap();
        let mut oversized = [1; 6];
        let mut partial = [1; 3];
        realtime(|| {
            adapter.process(&mut oversized);
            adapter.process(&mut partial);
        });
        assert_eq!(oversized, [32768; 6]);
        assert_eq!(partial, [32768; 3]);
        assert_eq!(probe.calls.load(Ordering::Relaxed), 0);
        assert_eq!(probe.errors.load(Ordering::Relaxed), 2);
        assert_eq!(probe.observations.load(Ordering::Relaxed), 2);
        assert_eq!(probe.samples.load(Ordering::Relaxed), 3);
        assert_eq!(probe.requested.load(Ordering::Relaxed), 2);
        assert_eq!(probe.malformed.load(Ordering::Relaxed), 1);
        assert_eq!(probe.capacity.load(Ordering::Relaxed), 1);
        let mut valid = [1; 4];
        realtime(|| adapter.process(&mut valid));
        assert_eq!(valid, [49152; 4]);
        assert_eq!(probe.calls.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn float_output_is_finite_and_frame_aligned_without_callback_memory_activity() {
        let (mut callback, probe) = source(SAMPLES);
        let mut output = [1.0; 10];
        realtime(|| process_f32(&mut *callback, config(5), &mut output, Some(&*probe)));
        assert_eq!(
            output.map(f32::to_bits),
            [-1.0_f32, -1.0, -0.5, 0.0, 0.5, 1.0, 1.0, 0.0, 0.0, 0.0].map(f32::to_bits)
        );
        let mut partial = [1.0; 3];
        realtime(|| process_f32(&mut *callback, config(5), &mut partial, Some(&*probe)));
        assert_eq!(partial.map(f32::to_bits), [0.0_f32; 3].map(f32::to_bits));
        assert_eq!(probe.calls.load(Ordering::Relaxed), 1);
        assert_eq!(probe.errors.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn empty_callbacks_are_forwarded_and_unwritten_scratch_never_replays_old_audio() {
        let (callback, probe) = source(&[0.5, -0.5]);
        let mut adapter = ConvertedOutput::<i16>::new(callback, config(2), 4).unwrap();
        let mut output = [1; 4];
        realtime(|| {
            adapter.process(&mut []);
            adapter.process(&mut output);
        });
        assert_eq!(output, [16384, -16384, 0, 0]);
        assert_eq!(probe.calls.load(Ordering::Relaxed), 2);
        assert_eq!(probe.errors.load(Ordering::Relaxed), 0);
    }
}
