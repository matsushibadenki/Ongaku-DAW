//! Background decoding into a bounded realtime-safe read-ahead queue.
#![cfg_attr(test, allow(unsafe_code))]

mod timeline;
pub use timeline::{StreamWindow, TimelineStream, TimelineStreamSnapshot};

use audio_file::AudioFrameReader;
use audio_types::{AudioError, RealtimeAudioSource};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
struct StreamSample {
    generation: u64,
    channel: u16,
    value: f32,
}

#[derive(Clone, Copy, Debug)]
struct SeekCommand {
    frame: u64,
    generation: u64,
}

#[derive(Default)]
struct SharedState {
    active_generation: AtomicU64,
    requested_generation: AtomicU64,
    stale_samples_discarded: AtomicU64,
    seek_wait_count: AtomicU64,
    worker_failed: AtomicBool,
    starvation_count: AtomicU64,
    worker_error_count: AtomicU64,
    shutdown: AtomicBool,
    eof_marker: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamingSnapshot {
    pub starvation_count: u64,
    pub worker_error_count: u64,
    pub end_of_stream: bool,
    pub requested_generation: u64,
    pub applied_generation: u64,
    pub seek_pending: bool,
    pub worker_failed: bool,
    pub stale_samples_discarded: u64,
    pub seek_wait_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct StreamController {
    seek_producer: Producer<SeekCommand>,
    shared: Arc<SharedState>,
    worker: Option<JoinHandle<()>>,
    format: StreamFormat,
}

impl StreamController {
    pub fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        if self.shared.worker_failed.load(Ordering::Acquire)
            || self.shared.shutdown.load(Ordering::Acquire)
        {
            return Err(AudioError::InvalidState("stream worker is not available"));
        }
        let generation = self
            .shared
            .requested_generation
            .load(Ordering::Acquire)
            .checked_add(1)
            .filter(|value| *value < u64::MAX)
            .ok_or(AudioError::InvalidState("stream generations exhausted"))?;
        self.seek_producer
            .push(SeekCommand { frame, generation })
            .map_err(|_| AudioError::Backend("stream seek queue is full".into()))?;
        // Invalidate queued/partially decoded old audio at acceptance, not after disk seek.
        self.shared
            .requested_generation
            .store(generation, Ordering::Release);
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> StreamingSnapshot {
        let applied = self.shared.active_generation.load(Ordering::Acquire);
        let requested = self
            .shared
            .requested_generation
            .load(Ordering::Acquire)
            .max(applied);
        let failed = self.shared.worker_failed.load(Ordering::Acquire);
        StreamingSnapshot {
            requested_generation: requested,
            applied_generation: applied,
            seek_pending: !failed && requested != applied,
            worker_failed: failed,
            stale_samples_discarded: self.shared.stale_samples_discarded.load(Ordering::Relaxed),
            seek_wait_count: self.shared.seek_wait_count.load(Ordering::Relaxed),
            starvation_count: self.shared.starvation_count.load(Ordering::Relaxed),
            worker_error_count: self.shared.worker_error_count.load(Ordering::Relaxed),
            end_of_stream: !failed
                && requested == applied
                && self.shared.eof_marker.load(Ordering::Acquire) == requested + 1,
        }
    }

    #[must_use]
    pub const fn format(&self) -> StreamFormat {
        self.format
    }
}

impl Drop for StreamController {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct AudioStreamConsumer {
    consumer: Consumer<StreamSample>,
    shared: Arc<SharedState>,
    format: StreamFormat,
    frame: Vec<f32>,
    staged: usize,
    generation: u64,
    lookahead: Option<StreamSample>,
}

impl AudioStreamConsumer {
    #[must_use]
    pub const fn format(&self) -> StreamFormat {
        self.format
    }

    /// Copies available samples and writes silence for starvation. Never allocates or blocks.
    pub fn read_interleaved(&mut self, output: &mut [f32]) -> usize {
        output.fill(0.0);
        let channels = usize::from(self.format.channels);
        if output.len() < channels || self.shared.worker_failed.load(Ordering::Acquire) {
            return 0;
        }
        let generation = self.shared.requested_generation.load(Ordering::Acquire);
        if self.generation != generation {
            self.generation = generation;
            self.staged = 0;
        }
        let mut written = 0;
        let mut attempts = 0;
        let mut interrupted = false;
        let maximum_attempts = output.len().saturating_mul(2).max(channels);
        while written + channels <= output.len() && attempts < maximum_attempts {
            if self.shared.requested_generation.load(Ordering::Acquire) != generation {
                interrupted = true;
                break;
            }
            let sample = if let Some(sample) = self.lookahead.take() {
                sample
            } else if let Ok(sample) = self.consumer.pop() {
                sample
            } else {
                break;
            };
            attempts += 1;
            if sample.generation > generation {
                self.lookahead = Some(sample); // Never discard the first frame of a newer seek.
                interrupted = true;
                break;
            }
            if sample.generation != generation {
                self.staged = 0;
                self.shared
                    .stale_samples_discarded
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if usize::from(sample.channel) != self.staged {
                self.staged = 0;
                if sample.channel != 0 {
                    continue;
                }
            }
            self.frame[self.staged] = sample.value;
            self.staged += 1;
            if self.staged == channels {
                output[written..written + channels].copy_from_slice(&self.frame);
                written += channels;
                self.staged = 0;
            }
        }
        interrupted |= self.shared.requested_generation.load(Ordering::Acquire) != generation;
        if interrupted || self.shared.worker_failed.load(Ordering::Acquire) {
            output.fill(0.0);
            written = 0;
            self.staged = 0;
        }
        if written < output.len() / channels * channels
            && !self.shared.worker_failed.load(Ordering::Acquire)
        {
            let current = self.shared.requested_generation.load(Ordering::Acquire);
            if interrupted || current != self.shared.active_generation.load(Ordering::Acquire) {
                self.shared.seek_wait_count.fetch_add(1, Ordering::Relaxed);
            } else if self.shared.eof_marker.load(Ordering::Acquire) != current + 1 {
                self.shared.starvation_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        written
    }
}

impl RealtimeAudioSource for AudioStreamConsumer {
    fn sample_rate(&self) -> u32 {
        self.format.sample_rate
    }

    fn channels(&self) -> u16 {
        self.format.channels
    }

    fn read_interleaved(&mut self, output: &mut [f32]) -> usize {
        Self::read_interleaved(self, output)
    }
}

/// Read-ahead storage is capped at two seconds and must contain complete frames.
pub fn spawn_streaming_worker(
    mut reader: Box<dyn AudioFrameReader>,
    capacity_samples: usize,
    chunk_samples: usize,
) -> Result<(StreamController, AudioStreamConsumer), AudioError> {
    if capacity_samples == 0 || chunk_samples == 0 || chunk_samples > capacity_samples {
        return Err(AudioError::InvalidConfig(
            "stream capacities must be non-zero and bounded",
        ));
    }
    let format = StreamFormat {
        sample_rate: reader.sample_rate(),
        channels: reader.channels(),
    };
    if format.sample_rate == 0 || format.channels == 0 {
        return Err(AudioError::InvalidConfig("stream reader format is invalid"));
    }
    let channels = usize::from(format.channels);
    let max_samples = u64::from(format.sample_rate) * u64::from(format.channels) * 2;
    if !capacity_samples.is_multiple_of(channels)
        || !chunk_samples.is_multiple_of(channels)
        || u64::try_from(capacity_samples).unwrap_or(u64::MAX) > max_samples
    {
        return Err(AudioError::InvalidConfig(
            "stream capacity/chunks must be frame-aligned and read-ahead at most two seconds",
        ));
    }
    let (sample_producer, sample_consumer) = RingBuffer::new(capacity_samples);
    let (seek_producer, seek_consumer) = RingBuffer::new(8);
    let shared = Arc::new(SharedState::default());
    let worker_shared = Arc::clone(&shared);
    let worker = thread::Builder::new()
        .name("ongaku-streaming".into())
        .spawn(move || {
            run_worker(
                &mut *reader,
                sample_producer,
                seek_consumer,
                chunk_samples,
                channels,
                &worker_shared,
            );
        })
        .map_err(|error| {
            AudioError::Backend(format!("failed to start streaming worker: {error}"))
        })?;
    Ok((
        StreamController {
            seek_producer,
            shared: Arc::clone(&shared),
            worker: Some(worker),
            format,
        },
        AudioStreamConsumer {
            consumer: sample_consumer,
            shared,
            format,
            frame: vec![0.0; channels],
            staged: 0,
            generation: 0,
            lookahead: None,
        },
    ))
}

fn run_worker(
    reader: &mut dyn AudioFrameReader,
    mut samples: Producer<StreamSample>,
    mut seeks: Consumer<SeekCommand>,
    chunk_samples: usize,
    channels: usize,
    shared: &SharedState,
) {
    let mut decoded = vec![0.0; chunk_samples];
    let mut generation = 0;
    let mut carry = 0;
    let mut pending = None;
    let fail = || {
        shared.worker_error_count.fetch_add(1, Ordering::Relaxed);
        shared.worker_failed.store(true, Ordering::Release);
    };
    while !shared.shutdown.load(Ordering::Acquire) {
        // At most eight queued seeks; obsolete targets do not cause extra decoder I/O.
        for _ in 0..8 {
            if let Ok(command) = seeks.pop() {
                pending = Some(command);
            } else {
                break;
            }
        }
        if let Some(command) = pending {
            if command.generation != shared.requested_generation.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            pending = None;
            if reader.seek(command.frame).is_err() {
                if shared.requested_generation.load(Ordering::Acquire) != command.generation {
                    shared.worker_error_count.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                fail();
                return;
            }
            generation = command.generation;
            carry = 0;
            shared.eof_marker.store(0, Ordering::Release);
            shared
                .active_generation
                .store(generation, Ordering::Release);
        }
        if generation != shared.requested_generation.load(Ordering::Acquire)
            || samples.slots() < chunk_samples
        {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        let read = reader.read_interleaved(&mut decoded[carry..]);
        // A decode that completed after a newer accepted seek is never tagged as current.
        if generation != shared.requested_generation.load(Ordering::Acquire) {
            if read.is_err() {
                shared.worker_error_count.fetch_add(1, Ordering::Relaxed);
            }
            carry = 0;
            continue;
        }
        let read = match read {
            Ok(0) if carry == 0 => {
                shared.eof_marker.store(generation + 1, Ordering::Release);
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            Ok(read) if read > 0 && read <= chunk_samples - carry => read,
            _ => {
                fail();
                return;
            }
        };
        shared.eof_marker.store(0, Ordering::Release);
        let total = carry + read;
        let complete = total / channels * channels;
        for frame in decoded[..complete].chunks_exact(channels) {
            if generation != shared.requested_generation.load(Ordering::Acquire) {
                break;
            }
            for (channel, &value) in frame.iter().enumerate() {
                let value = if value.is_finite() { value } else { 0.0 };
                if samples
                    .push(StreamSample {
                        generation,
                        channel: u16::try_from(channel).unwrap_or(0),
                        value,
                    })
                    .is_err()
                {
                    fail();
                    return;
                }
            }
        }
        if generation != shared.requested_generation.load(Ordering::Acquire) {
            carry = 0;
            continue;
        }
        decoded.copy_within(complete..total, 0);
        carry = total - complete;
    }
}

#[cfg(test)]
mod tests {
    use super::spawn_streaming_worker;
    use audio_file::AudioFrameReader;
    use audio_types::AudioError;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    struct RealtimeTrackingAllocator;
    static REALTIME_MEMORY_EVENTS: AtomicUsize = AtomicUsize::new(0);
    thread_local! { static IN_REALTIME_TEST: Cell<bool> = const { Cell::new(false) }; }

    // SAFETY: all allocator operations delegate to `System` with unchanged arguments.
    unsafe impl GlobalAlloc for RealtimeTrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if IN_REALTIME_TEST.with(Cell::get) {
                REALTIME_MEMORY_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: the caller provides the layout required by `GlobalAlloc`.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if IN_REALTIME_TEST.with(Cell::get) {
                REALTIME_MEMORY_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: the pointer and layout are forwarded to their original allocator.
            unsafe { System.dealloc(pointer, layout) };
        }
    }

    #[global_allocator]
    static ALLOCATOR: RealtimeTrackingAllocator = RealtimeTrackingAllocator;

    struct MemoryReader {
        samples: Vec<f32>,
        cursor: usize,
    }
    impl AudioFrameReader for MemoryReader {
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn channels(&self) -> u16 {
            1
        }
        fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError> {
            let count = output
                .len()
                .min(self.samples.len().saturating_sub(self.cursor));
            output[..count].copy_from_slice(&self.samples[self.cursor..self.cursor + count]);
            self.cursor += count;
            Ok(count)
        }
        fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
            self.cursor = usize::try_from(frame)
                .unwrap_or(usize::MAX)
                .min(self.samples.len());
            Ok(())
        }
    }

    #[test]
    fn readiness_window_collection_poll_read_and_release_do_not_allocate_or_free() {
        let media = super::TimelineStream::prepare(
            Box::new(MemoryReader {
                samples: vec![0.25; 1000],
                cursor: 0,
            }),
            1000,
            16,
            2,
            &audio_types::PreparationToken::default(),
        )
        .unwrap();
        let mut output = [0.0; 2];
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        let mut window = media.window();
        window.require_frame(0);
        window.require_frame(600);
        window.poll();
        window.read_frame(0, &mut output);
        drop(window);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
    }

    #[test]
    fn timeline_cache_hits_misses_and_prefetch_do_not_allocate_or_free() {
        let media = super::TimelineStream::prepare(
            Box::new(MemoryReader {
                samples: vec![0.25; 1000],
                cursor: 0,
            }),
            1000,
            16,
            2,
            &audio_types::PreparationToken::default(),
        )
        .unwrap();
        let mut output = [0.0; 2];
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        media.read_frame(0, &mut output);
        media.read_frame(15, &mut output);
        media.read_frame(900, &mut output);
        media.request_frame(600);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
    }

    #[test]
    fn worker_prefetches_without_realtime_io() {
        let reader = MemoryReader {
            samples: vec![0.1, 0.2, 0.3, 0.4],
            cursor: 0,
        };
        let (_controller, mut consumer) = spawn_streaming_worker(Box::new(reader), 16, 4).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut output = [0.0; 4];
        while consumer.read_interleaved(&mut output) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            output
                .iter()
                .zip([0.1, 0.2, 0.3, 0.4])
                .all(|(a, b)| (a - b).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn realtime_consumer_does_not_allocate_or_free() {
        let reader = MemoryReader {
            samples: vec![0.1; 64],
            cursor: 0,
        };
        let (_controller, mut consumer) = spawn_streaming_worker(Box::new(reader), 64, 16).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut warmup = [0.0; 16];
        while consumer.read_interleaved(&mut warmup) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let mut output = [0.0; 16];
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        consumer.read_interleaved(&mut output);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
    }

    #[test]
    fn seek_invalidates_prefetched_samples() {
        let reader = MemoryReader {
            samples: vec![0.1, 0.2, 0.3, 0.4],
            cursor: 0,
        };
        let (mut controller, mut consumer) =
            spawn_streaming_worker(Box::new(reader), 16, 4).unwrap();
        controller.seek(2).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut output = [0.0; 2];
        loop {
            consumer.read_interleaved(&mut output);
            if samples_match(&output, &[0.3, 0.4]) || Instant::now() >= deadline {
                break;
            }
            std::thread::yield_now();
        }
        assert!(samples_match(&output, &[0.3, 0.4]));
    }

    #[test]
    fn stale_drain_and_stereo_seek_handoff_do_not_allocate_or_free() {
        let (mut producer, _seeks, mut controller, mut consumer) =
            super::p0_regressions::manual_stream(2);
        producer
            .push(super::StreamSample {
                generation: 0,
                channel: 0,
                value: 0.1,
            })
            .unwrap();
        consumer.read_interleaved(&mut [0.0; 2]); // Half of an old frame is staged.
        controller.seek(5).unwrap();
        producer
            .push(super::StreamSample {
                generation: 0,
                channel: 1,
                value: 0.2,
            })
            .unwrap();
        producer
            .push(super::StreamSample {
                generation: 1,
                channel: 0,
                value: 0.3,
            })
            .unwrap();
        producer
            .push(super::StreamSample {
                generation: 1,
                channel: 1,
                value: 0.4,
            })
            .unwrap();
        let mut output = [0.0; 2];
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        assert_eq!(consumer.read_interleaved(&mut output), 2);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
        assert_eq!(output.map(f32::to_bits), [0.3, 0.4].map(f32::to_bits));
    }

    fn samples_match(actual: &[f32], expected: &[f32]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (actual - expected).abs() < 1.0e-6)
    }
}

#[cfg(test)]
mod p0_regressions {
    use super::*;
    pub(super) fn manual_stream(
        channels: u16,
    ) -> (
        Producer<StreamSample>,
        Consumer<SeekCommand>,
        StreamController,
        AudioStreamConsumer,
    ) {
        let (producer, consumer) = RingBuffer::new(16);
        let (seek_producer, seeks) = RingBuffer::new(8);
        let shared = Arc::new(SharedState::default());
        let format = StreamFormat {
            sample_rate: 48000,
            channels,
        };
        (
            producer,
            seeks,
            StreamController {
                seek_producer,
                shared: Arc::clone(&shared),
                worker: None,
                format,
            },
            AudioStreamConsumer {
                consumer,
                shared,
                format,
                frame: vec![0.0; usize::from(channels)],
                staged: 0,
                generation: 0,
                lookahead: None,
            },
        )
    }
    #[test]
    fn accepted_seek_invalidates_prefetched_audio_before_the_worker_applies_it() {
        let (mut producer, _seeks, mut controller, mut consumer) = manual_stream(1);
        producer
            .push(StreamSample {
                generation: 0,
                channel: 0,
                value: 0.75,
            })
            .unwrap();
        controller.seek(10).unwrap();
        let mut output = [1.0];
        assert_eq!(consumer.read_interleaved(&mut output), 0);
        assert_eq!(output[0].to_bits(), 0.0_f32.to_bits());
    }
    #[test]
    fn an_incomplete_stereo_frame_must_not_shift_channels_across_callbacks() {
        let (mut producer, _seeks, _controller, mut consumer) = manual_stream(2);
        producer
            .push(StreamSample {
                generation: 0,
                channel: 0,
                value: 0.25,
            })
            .unwrap();
        let mut output = [1.0; 2];
        assert_eq!(consumer.read_interleaved(&mut output), 0);
        assert_eq!(output.map(f32::to_bits), [0.0; 2].map(f32::to_bits));
        producer
            .push(StreamSample {
                generation: 0,
                channel: 1,
                value: 0.75,
            })
            .unwrap();
        assert_eq!(consumer.read_interleaved(&mut output), 2);
        assert_eq!(output.map(f32::to_bits), [0.25, 0.75].map(f32::to_bits));
    }
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use std::sync::{Condvar, Mutex, mpsc};
    use std::time::Instant;
    struct Gate {
        open: Mutex<bool>,
        wake: Condvar,
    }
    impl Gate {
        fn new() -> Self {
            Self {
                open: Mutex::new(false),
                wake: Condvar::new(),
            }
        }
        fn wait(&self) {
            let (open, _) = self
                .wake
                .wait_timeout_while(self.open.lock().unwrap(), Duration::from_secs(5), |open| {
                    !*open
                })
                .unwrap();
            assert!(*open, "reader fixture timed out");
        }
        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.wake.notify_all();
        }
    }
    struct Reader {
        channels: u16,
        samples: Vec<f32>,
        cursor: usize,
        pattern: Vec<usize>,
        calls: usize,
        gate: Option<Arc<Gate>>,
        started: Option<mpsc::SyncSender<()>>,
        bad_count: bool,
        bad_seek: bool,
        seeks: Arc<Mutex<Vec<u64>>>,
    }
    impl Reader {
        fn new(channels: u16, samples: Vec<f32>) -> Self {
            Self {
                channels,
                samples,
                cursor: 0,
                pattern: vec![usize::MAX],
                calls: 0,
                gate: None,
                started: None,
                bad_count: false,
                bad_seek: false,
                seeks: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }
    impl AudioFrameReader for Reader {
        fn sample_rate(&self) -> u32 {
            48000
        }
        fn channels(&self) -> u16 {
            self.channels
        }
        fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError> {
            if self.calls == 0 {
                if let Some(started) = &self.started {
                    started.send(()).unwrap();
                }
                if let Some(gate) = &self.gate {
                    gate.wait();
                }
            }
            if self.bad_count {
                return Ok(output.len() + 1);
            }
            let count = output
                .len()
                .min(self.pattern[self.calls % self.pattern.len()])
                .min(self.samples.len().saturating_sub(self.cursor));
            output[..count].copy_from_slice(&self.samples[self.cursor..self.cursor + count]);
            self.cursor += count;
            self.calls += 1;
            Ok(count)
        }
        fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
            self.seeks.lock().unwrap().push(frame);
            if self.bad_seek {
                return Err(AudioError::InvalidConfig("fixture seek failed"));
            }
            self.cursor = usize::try_from(frame)
                .unwrap()
                .saturating_mul(usize::from(self.channels))
                .min(self.samples.len());
            Ok(())
        }
    }
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "stream fixture timed out");
            thread::yield_now();
        }
    }
    #[test]
    fn partial_decoder_reads_preserve_all_channels_and_ignore_callback_partitions() {
        let samples: Vec<_> = (0_u16..100)
            .flat_map(|value| [f32::from(value), -f32::from(value) - 0.5])
            .collect();
        let mut reader = Reader::new(2, samples.clone());
        reader.pattern = vec![1, 3, 2, 5];
        let (controller, mut consumer) = spawn_streaming_worker(Box::new(reader), 32, 8).unwrap();
        let mut result = Vec::new();
        let mut block = 0;
        wait_for(|| {
            let count = [2, 6, 4, 14, 10][block % 5];
            block += 1;
            let mut output = vec![0.0; count];
            let written = consumer.read_interleaved(&mut output);
            assert!(written.is_multiple_of(2));
            result.extend_from_slice(&output[..written]);
            result.len() == samples.len()
        });
        assert_eq!(result, samples);
        wait_for(|| controller.snapshot().end_of_stream);
        let before = controller.snapshot().starvation_count;
        assert_eq!(consumer.read_interleaved(&mut [1.0; 8]), 0);
        assert_eq!(controller.snapshot().starvation_count, before); // Known EOF is not an underrun.
    }
    #[test]
    fn stalled_decode_completing_after_seek_cannot_replay_old_samples() {
        let mut reader = Reader::new(2, (0_u16..40).map(f32::from).collect());
        let gate = Arc::new(Gate::new());
        reader.gate = Some(Arc::clone(&gate));
        let (started, receiver) = mpsc::sync_channel(1);
        reader.started = Some(started);
        let (mut controller, mut consumer) =
            spawn_streaming_worker(Box::new(reader), 16, 4).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        controller.seek(10).unwrap();
        let mut output = [1.0; 4];
        assert_eq!(consumer.read_interleaved(&mut output), 0);
        gate.release();
        wait_for(|| {
            let count = consumer.read_interleaved(&mut output);
            assert!(count == 0 || output[0] >= 20.0);
            count == 4
        });
        assert_eq!(
            output.map(f32::to_bits),
            [20.0, 21.0, 22.0, 23.0].map(f32::to_bits)
        );
        let state = controller.snapshot();
        assert_eq!(state.requested_generation, 1);
        assert_eq!(state.applied_generation, 1);
    }
    #[test]
    fn queued_seeks_coalesce_before_decoder_io() {
        let (producer, seeks, mut controller, _consumer) = super::p0_regressions::manual_stream(1);
        controller.seek(3).unwrap();
        controller.seek(6).unwrap();
        controller.seek(9).unwrap();
        let reader = Reader::new(1, Vec::new());
        let log = Arc::clone(&reader.seeks);
        let shared = Arc::clone(&controller.shared);
        let worker = thread::spawn(move || {
            let mut reader = reader;
            run_worker(&mut reader, producer, seeks, 4, 1, &shared);
        });
        wait_for(|| controller.snapshot().applied_generation == 3);
        controller.shared.shutdown.store(true, Ordering::Release);
        worker.join().unwrap();
        assert_eq!(*log.lock().unwrap(), vec![9]);
    }
    #[test]
    fn rejected_seek_does_not_invalidate_the_last_accepted_generation_and_ids_do_not_wrap() {
        let (_producer, _seeks, mut controller, _consumer) =
            super::p0_regressions::manual_stream(1);
        for frame in 0..8 {
            controller.seek(frame).unwrap();
        }
        assert!(controller.seek(99).is_err());
        assert_eq!(controller.snapshot().requested_generation, 8);
        controller
            .shared
            .requested_generation
            .store(u64::MAX - 1, Ordering::Release);
        assert!(controller.seek(0).is_err());
        assert_eq!(controller.snapshot().requested_generation, u64::MAX - 1);
    }
    #[test]
    fn callback_stale_drain_budget_and_incomplete_output_are_bounded() {
        let (mut producer, _seeks, mut controller, mut consumer) =
            super::p0_regressions::manual_stream(1);
        for _ in 0..16 {
            producer
                .push(StreamSample {
                    generation: 0,
                    channel: 0,
                    value: 1.0,
                })
                .unwrap();
        }
        controller.seek(10).unwrap();
        assert_eq!(consumer.read_interleaved(&mut [1.0]), 0);
        assert_eq!(controller.snapshot().stale_samples_discarded, 2);
        assert_eq!(producer.slots(), 2);
        let (mut producer, _seeks, _controller, mut consumer) =
            super::p0_regressions::manual_stream(2);
        producer
            .push(StreamSample {
                generation: 0,
                channel: 0,
                value: 0.25,
            })
            .unwrap();
        producer
            .push(StreamSample {
                generation: 0,
                channel: 1,
                value: 0.75,
            })
            .unwrap();
        assert_eq!(consumer.read_interleaved(&mut [1.0]), 0);
        let mut output = [1.0; 3];
        assert_eq!(consumer.read_interleaved(&mut output), 2);
        assert_eq!(
            output.map(f32::to_bits),
            [0.25, 0.75, 0.0].map(f32::to_bits)
        );
    }
    #[test]
    fn future_generation_lookahead_is_preserved_for_the_next_handoff() {
        let (mut producer, _seeks, mut controller, mut consumer) =
            super::p0_regressions::manual_stream(1);
        // Exercise a newer sample encountered by an old-generation callback snapshot.
        producer
            .push(StreamSample {
                generation: 1,
                channel: 0,
                value: 0.5,
            })
            .unwrap();
        assert_eq!(consumer.read_interleaved(&mut [1.0]), 0);
        controller.seek(10).unwrap();
        let mut output = [0.0];
        assert_eq!(consumer.read_interleaved(&mut output), 1);
        assert_eq!(output[0].to_bits(), 0.5_f32.to_bits());
    }
    #[test]
    fn decoder_contract_failures_and_failed_seeks_are_observable_without_panicking() {
        for case in 0..3 {
            let mut reader = Reader::new(
                2,
                if case == 2 {
                    vec![0.1; 4]
                } else {
                    vec![0.1, 0.2, 0.3]
                },
            );
            reader.bad_count = case == 0;
            reader.bad_seek = case == 2;
            let (mut controller, mut consumer) =
                spawn_streaming_worker(Box::new(reader), 16, 4).unwrap();
            if case == 2 {
                controller.seek(2).unwrap();
            }
            wait_for(|| controller.snapshot().worker_failed);
            assert!(controller.snapshot().worker_error_count > 0);
            assert!(!controller.snapshot().end_of_stream);
            assert!(!controller.snapshot().seek_pending);
            let mut output = [1.0; 2];
            assert_eq!(consumer.read_interleaved(&mut output), 0);
            assert_eq!(output.map(f32::to_bits), [0.0; 2].map(f32::to_bits));
            assert!(controller.seek(0).is_err());
        }
    }
    #[test]
    fn frame_alignment_and_read_ahead_limits_are_checked_before_spawning() {
        for (channels, capacity, chunk) in [(2, 3, 2), (2, 8, 3), (1, 96_001, 4), (2, 192_002, 4)] {
            assert!(
                spawn_streaming_worker(
                    Box::new(Reader::new(channels, Vec::new())),
                    capacity,
                    chunk
                )
                .is_err()
            );
        }
    }
}
