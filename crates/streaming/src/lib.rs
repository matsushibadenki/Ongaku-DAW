//! Background decoding into a bounded realtime-safe read-ahead queue.
#![cfg_attr(test, allow(unsafe_code))]

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
    next_generation: AtomicU64,
    starvation_count: AtomicU64,
    worker_error_count: AtomicU64,
    shutdown: AtomicBool,
    end_of_stream: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamingSnapshot {
    pub starvation_count: u64,
    pub worker_error_count: u64,
    pub end_of_stream: bool,
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
        let generation = self.shared.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.seek_producer
            .push(SeekCommand { frame, generation })
            .map_err(|_| AudioError::Backend("stream seek queue is full".into()))
    }

    #[must_use]
    pub fn snapshot(&self) -> StreamingSnapshot {
        StreamingSnapshot {
            starvation_count: self.shared.starvation_count.load(Ordering::Relaxed),
            worker_error_count: self.shared.worker_error_count.load(Ordering::Relaxed),
            end_of_stream: self.shared.end_of_stream.load(Ordering::Acquire),
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
}

impl AudioStreamConsumer {
    #[must_use]
    pub const fn format(&self) -> StreamFormat {
        self.format
    }

    /// Copies available samples and writes silence for starvation. Never allocates or blocks.
    pub fn read_interleaved(&mut self, output: &mut [f32]) -> usize {
        let generation = self.shared.active_generation.load(Ordering::Acquire);
        let mut written = 0;
        let mut attempts = 0;
        let maximum_attempts = output.len().saturating_mul(2).max(1);
        while written < output.len() && attempts < maximum_attempts {
            let Ok(sample) = self.consumer.pop() else {
                break;
            };
            attempts += 1;
            if sample.generation == generation {
                output[written] = sample.value;
                written += 1;
            }
        }
        if written < output.len() {
            output[written..].fill(0.0);
            self.shared.starvation_count.fetch_add(1, Ordering::Relaxed);
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
        },
    ))
}

fn run_worker(
    reader: &mut dyn AudioFrameReader,
    mut samples: Producer<StreamSample>,
    mut seeks: Consumer<SeekCommand>,
    chunk_samples: usize,
    shared: &SharedState,
) {
    let mut decoded = vec![0.0; chunk_samples];
    let mut generation = 0;
    while !shared.shutdown.load(Ordering::Acquire) {
        while let Ok(command) = seeks.pop() {
            if reader.seek(command.frame).is_err() {
                shared.worker_error_count.fetch_add(1, Ordering::Relaxed);
                return;
            }
            generation = command.generation;
            shared.end_of_stream.store(false, Ordering::Release);
            shared
                .active_generation
                .store(generation, Ordering::Release);
        }
        if samples.slots() < chunk_samples {
            thread::yield_now();
            continue;
        }
        let read = match reader.read_interleaved(&mut decoded) {
            Ok(0) => {
                shared.end_of_stream.store(true, Ordering::Release);
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            Ok(read) => {
                shared.end_of_stream.store(false, Ordering::Release);
                read
            }
            Err(_) => {
                shared.worker_error_count.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        for value in decoded[..read].iter().copied() {
            if samples.push(StreamSample { generation, value }).is_err() {
                break;
            }
        }
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

    fn samples_match(actual: &[f32], expected: &[f32]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (actual - expected).abs() < 1.0e-6)
    }
}
