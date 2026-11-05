//! Device-independent realtime audio engine.
#![cfg_attr(test, allow(unsafe_code))]

mod metrics;

pub use metrics::RealtimeMetrics;

use audio_backend::{AudioCallback, RealtimeEventSink};
use audio_types::{AudioConfig, AudioError, RealtimeMetricsSnapshot};
use daw_core::transport::{Transport, TransportCommand, TransportSnapshot};
use daw_dsp::graph::PhaseOneGraph;
use daw_mixer::AudioGraph;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
};

const GRAPH_QUEUE_CAPACITY: usize = 4;
const TRANSPORT_QUEUE_CAPACITY: usize = 64;

pub enum EngineGraph {
    TestTone(PhaseOneGraph),
    Timeline(AudioGraph),
}

impl EngineGraph {
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn test_tone(config: AudioConfig) -> Self {
        let mut graph = PhaseOneGraph::new();
        graph.prepare(config.sample_rate as f32, usize::from(config.channels));
        Self::TestTone(graph)
    }

    #[must_use]
    pub const fn timeline(graph: AudioGraph) -> Self {
        Self::Timeline(graph)
    }

    fn process(&mut self, output: &mut [f32], transport: &Transport) {
        match self {
            Self::TestTone(graph) => graph.process(output),
            Self::Timeline(graph) => graph.process(output, transport),
        }
    }

    fn set_test_tone_gain(&mut self, gain: f32) {
        if let Self::TestTone(graph) = self {
            graph.set_gain(gain);
        }
    }
}

struct SharedTransport {
    sequence: AtomicU64,
    sample_rate: u32,
    current_sample: AtomicU64,
    tempo_bits: AtomicU64,
    numerator: AtomicU8,
    denominator: AtomicU8,
    playing: AtomicBool,
    recording: AtomicBool,
    loop_enabled: AtomicBool,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
}

impl SharedTransport {
    fn new(sample_rate: u32) -> Self {
        let initial = Transport::new(sample_rate).snapshot();
        Self {
            sequence: AtomicU64::new(0),
            sample_rate,
            current_sample: AtomicU64::new(0),
            tempo_bits: AtomicU64::new(initial.tempo_bpm.to_bits()),
            numerator: AtomicU8::new(initial.time_signature.numerator),
            denominator: AtomicU8::new(initial.time_signature.denominator),
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            loop_enabled: AtomicBool::new(false),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
        }
    }

    fn publish(&self, snapshot: TransportSnapshot) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.current_sample
            .store(snapshot.current_sample.0, Ordering::Release);
        self.tempo_bits
            .store(snapshot.tempo_bpm.to_bits(), Ordering::Release);
        self.numerator
            .store(snapshot.time_signature.numerator, Ordering::Release);
        self.denominator
            .store(snapshot.time_signature.denominator, Ordering::Release);
        self.playing.store(snapshot.playing, Ordering::Release);
        self.recording.store(snapshot.recording, Ordering::Release);
        if let Some(range) = snapshot.loop_range {
            self.loop_start.store(range.start.0, Ordering::Release);
            self.loop_end.store(range.end.0, Ordering::Release);
            self.loop_enabled.store(true, Ordering::Release);
        } else {
            self.loop_enabled.store(false, Ordering::Release);
        }
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self) -> TransportSnapshot {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let loop_range =
                self.loop_enabled
                    .load(Ordering::Relaxed)
                    .then(|| daw_core::SampleRange {
                        start: daw_core::SamplePosition(self.loop_start.load(Ordering::Relaxed)),
                        end: daw_core::SamplePosition(self.loop_end.load(Ordering::Relaxed)),
                    });
            let snapshot = TransportSnapshot {
                current_sample: daw_core::SamplePosition(
                    self.current_sample.load(Ordering::Relaxed),
                ),
                sample_rate: self.sample_rate,
                tempo_bpm: f64::from_bits(self.tempo_bits.load(Ordering::Relaxed)),
                time_signature: daw_core::transport::TimeSignature {
                    numerator: self.numerator.load(Ordering::Relaxed),
                    denominator: self.denominator.load(Ordering::Relaxed),
                },
                playing: self.playing.load(Ordering::Relaxed),
                recording: self.recording.load(Ordering::Relaxed),
                loop_range,
            };
            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                return snapshot;
            }
        }
    }
}

#[derive(Clone)]
pub struct EngineControl {
    gain_bits: Arc<AtomicU32>,
    metrics: Arc<RealtimeMetrics>,
    graph_updates: Arc<Mutex<Producer<EngineGraph>>>,
    retired_graphs: Arc<Mutex<Consumer<EngineGraph>>>,
    transport_commands: Arc<Mutex<Producer<TransportCommand>>>,
    transport: Arc<SharedTransport>,
}

impl EngineControl {
    /// Lock-free, bounded update suitable for a high-frequency UI control path.
    pub fn set_gain(&self, linear_gain: f32) {
        self.gain_bits
            .store(sanitize_gain(linear_gain).to_bits(), Ordering::Release);
    }

    #[must_use]
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Acquire))
    }

    #[must_use]
    pub fn metrics(&self) -> RealtimeMetricsSnapshot {
        self.metrics.snapshot()
    }

    pub fn submit_graph(&self, graph: EngineGraph) -> Result<(), AudioError> {
        self.reclaim_retired()?;
        self.graph_updates
            .lock()
            .map_err(|_| AudioError::InvalidState("graph update lock was poisoned"))?
            .push(graph)
            .map_err(|_| AudioError::Backend("graph update queue is full".into()))
    }

    pub fn send_transport(&self, command: TransportCommand) -> Result<(), AudioError> {
        self.transport_commands
            .lock()
            .map_err(|_| AudioError::InvalidState("transport command lock was poisoned"))?
            .push(command)
            .map_err(|_| AudioError::Backend("transport command queue is full".into()))
    }

    #[must_use]
    pub fn transport(&self) -> TransportSnapshot {
        self.transport.snapshot()
    }

    pub fn reclaim_retired(&self) -> Result<usize, AudioError> {
        let mut retired = self
            .retired_graphs
            .lock()
            .map_err(|_| AudioError::InvalidState("retired graph lock was poisoned"))?;
        let mut reclaimed = 0;
        while let Ok(graph) = retired.pop() {
            drop(graph);
            reclaimed += 1;
        }
        Ok(reclaimed)
    }
}

pub struct EngineProcessor {
    config: AudioConfig,
    graph: EngineGraph,
    gain_bits: Arc<AtomicU32>,
    applied_gain_bits: u32,
    metrics: Arc<RealtimeMetrics>,
    graph_updates: Consumer<EngineGraph>,
    retired_graphs: Producer<EngineGraph>,
    retired_fallback: Option<EngineGraph>,
    transport_commands: Consumer<TransportCommand>,
    transport: Transport,
    shared_transport: Arc<SharedTransport>,
}

impl EngineProcessor {
    #[allow(clippy::cast_precision_loss)]
    pub fn prepare(config: AudioConfig) -> Result<(Self, EngineControl), AudioError> {
        let config = config.validate()?;
        let initial_gain = 0.25_f32;
        let gain_bits = Arc::new(AtomicU32::new(initial_gain.to_bits()));
        let metrics = Arc::new(RealtimeMetrics::new(config.callback_deadline_ns()));
        let mut graph = EngineGraph::test_tone(config);
        graph.set_test_tone_gain(initial_gain);
        let (graph_producer, graph_consumer) = RingBuffer::new(GRAPH_QUEUE_CAPACITY);
        let (retired_producer, retired_consumer) = RingBuffer::new(GRAPH_QUEUE_CAPACITY);
        let (transport_producer, transport_consumer) = RingBuffer::new(TRANSPORT_QUEUE_CAPACITY);
        let shared_transport = Arc::new(SharedTransport::new(config.sample_rate));

        let processor = Self {
            config,
            graph,
            gain_bits: Arc::clone(&gain_bits),
            applied_gain_bits: initial_gain.to_bits(),
            metrics: Arc::clone(&metrics),
            graph_updates: graph_consumer,
            retired_graphs: retired_producer,
            retired_fallback: None,
            transport_commands: transport_consumer,
            transport: Transport::new(config.sample_rate),
            shared_transport: Arc::clone(&shared_transport),
        };
        let control = EngineControl {
            gain_bits,
            metrics,
            graph_updates: Arc::new(Mutex::new(graph_producer)),
            retired_graphs: Arc::new(Mutex::new(retired_consumer)),
            transport_commands: Arc::new(Mutex::new(transport_producer)),
            transport: shared_transport,
        };
        Ok((processor, control))
    }

    #[must_use]
    pub const fn config(&self) -> AudioConfig {
        self.config
    }

    /// Allocation-free and lock-free processing for an interleaved output block.
    pub fn process_block(&mut self, output: &mut [f32]) {
        let started = std::time::Instant::now();
        self.apply_graph_update();
        for _ in 0..TRANSPORT_QUEUE_CAPACITY {
            let Ok(command) = self.transport_commands.pop() else {
                break;
            };
            self.transport.apply(command);
        }
        let requested_gain = self.gain_bits.load(Ordering::Acquire);
        if requested_gain != self.applied_gain_bits {
            self.graph
                .set_test_tone_gain(f32::from_bits(requested_gain));
            self.applied_gain_bits = requested_gain;
        }
        self.graph.process(output, &self.transport);
        let frames = output.len() / usize::from(self.config.channels);
        self.transport.advance(frames);
        self.shared_transport.publish(self.transport.snapshot());
        self.metrics.record(started.elapsed());
    }

    fn apply_graph_update(&mut self) {
        if let Some(retired) = self.retired_fallback.take()
            && let Err(PushError::Full(retired)) = self.retired_graphs.push(retired)
        {
            self.retired_fallback = Some(retired);
            return;
        }
        let Ok(next) = self.graph_updates.pop() else {
            return;
        };
        let retired = std::mem::replace(&mut self.graph, next);
        if let Err(PushError::Full(retired)) = self.retired_graphs.push(retired) {
            self.retired_fallback = Some(retired);
        }
    }
}

impl AudioCallback for EngineProcessor {
    fn process(&mut self, output: &mut [f32]) {
        self.process_block(output);
    }

    fn event_sink(&self) -> Option<Arc<dyn RealtimeEventSink>> {
        let sink: Arc<dyn RealtimeEventSink> = self.metrics.clone();
        Some(sink)
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
    use super::{EngineGraph, EngineProcessor};
    use audio_clip::{AudioClip, PcmAsset};
    use audio_types::{AudioConfig, RealtimeAudioSource};
    use daw_automation::{AutomationCurve, AutomationLane, AutomationParameter, AutomationPoint};
    use daw_core::{SamplePosition, transport::TransportCommand};
    use daw_midi::{MidiClip, MidiNote, MidiSynthSource};
    use daw_mixer::{AudioGraph, Track, TrackId};
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct RealtimeTrackingAllocator;
    static REALTIME_MEMORY_EVENTS: AtomicUsize = AtomicUsize::new(0);
    thread_local! { static IN_REALTIME_TEST: Cell<bool> = const { Cell::new(false) }; }

    // SAFETY: every operation delegates to `System` with the exact original pointer/layout.
    // The only additional work is thread-local inspection and a lock-free counter increment.
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
            // SAFETY: `pointer` and `layout` are forwarded unchanged to their allocator.
            unsafe { System.dealloc(pointer, layout) };
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if IN_REALTIME_TEST.with(Cell::get) {
                REALTIME_MEMORY_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: the caller provides the layout required by `GlobalAlloc`.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if IN_REALTIME_TEST.with(Cell::get) {
                REALTIME_MEMORY_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: all arguments are forwarded unchanged to the original allocator.
            unsafe { System.realloc(pointer, layout, size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: RealtimeTrackingAllocator = RealtimeTrackingAllocator;

    #[test]
    fn renders_a_finite_non_silent_test_tone() {
        let (mut engine, _) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        let mut output = [0.0; 256];
        engine.process_block(&mut output);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
    }

    #[test]
    fn control_updates_gain_without_locking() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        control.set_gain(0.0);
        let mut output = [1.0; 1_024];
        engine.process_block(&mut output);
        assert!(control.gain().abs() < f32::EPSILON);
        assert!(output.last().unwrap().abs() < f32::EPSILON);
    }

    #[test]
    fn realtime_process_does_not_allocate_or_free() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        let mut output = [0.0; 256];
        control.set_gain(0.75);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        engine.process_block(&mut output);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
    }

    #[test]
    fn rapid_control_updates_do_not_block_processing() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        let updater = std::thread::spawn(move || {
            for _ in 0..500 {
                for step in 0_u16..=200 {
                    control.set_gain(f32::from(step) / 100.0);
                }
            }
        });
        let mut output = [0.0; 256];
        for _ in 0..10_000 {
            engine.process_block(&mut output);
        }
        updater.join().unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    fn timeline_graph(config: AudioConfig, value: f32, frames: usize) -> EngineGraph {
        let asset = Arc::new(PcmAsset::new(config.sample_rate, 1, vec![value; frames]).unwrap());
        let clip = AudioClip::new(
            SamplePosition::ZERO,
            0,
            u64::try_from(frames).unwrap(),
            asset,
        )
        .unwrap();
        let mut track = Track::new(TrackId(1));
        track.add_clip(clip);
        let mut graph = AudioGraph::new(config).unwrap();
        graph.add_track(track).unwrap();
        EngineGraph::timeline(graph)
    }

    struct SilentSource;

    impl RealtimeAudioSource for SilentSource {
        fn sample_rate(&self) -> u32 {
            48_000
        }

        fn channels(&self) -> u16 {
            2
        }

        fn read_interleaved(&mut self, output: &mut [f32]) -> usize {
            output.fill(0.0);
            output.len()
        }
    }

    fn realtime_source_graph(config: AudioConfig) -> EngineGraph {
        let mut track = Track::new(TrackId(1));
        track.set_realtime_source(Box::new(SilentSource)).unwrap();
        let mut graph = AudioGraph::new(config).unwrap();
        graph.add_track(track).unwrap();
        EngineGraph::timeline(graph)
    }

    fn midi_graph(config: AudioConfig) -> EngineGraph {
        let clip = MidiClip {
            id: 1,
            timeline_start: SamplePosition::ZERO,
            duration_samples: 4_096,
            notes: vec![MidiNote {
                id: 1,
                start: SamplePosition::ZERO,
                duration_samples: 4_096,
                key: 60,
                velocity: 0.8,
                channel: 0,
            }],
        };
        let automation = AutomationLane::new(
            1,
            AutomationParameter::InstrumentLevel,
            0.0,
            1.0,
            0.5,
            vec![
                AutomationPoint {
                    id: 1,
                    position: SamplePosition::ZERO,
                    value: 0.5,
                    curve: AutomationCurve::Linear,
                },
                AutomationPoint {
                    id: 2,
                    position: SamplePosition(4_096),
                    value: 1.0,
                    curve: AutomationCurve::Linear,
                },
            ],
        )
        .unwrap();
        let source = MidiSynthSource::new(config, &[clip], Some(automation)).unwrap();
        let mut track = Track::new(TrackId(1));
        track.set_timeline_source(Box::new(source)).unwrap();
        let mut graph = AudioGraph::new(config).unwrap();
        graph.add_track(track).unwrap();
        EngineGraph::timeline(graph)
    }

    #[test]
    fn swaps_prepared_graph_at_a_block_boundary() {
        let config = AudioConfig::DEFAULT;
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control
            .submit_graph(timeline_graph(config, 0.25, 128))
            .unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        let mut output = [0.0; 128];
        engine.process_block(&mut output);
        assert!(output.iter().all(|sample| (*sample - 0.25).abs() < 1.0e-6));
        assert_eq!(control.transport().current_sample, SamplePosition(64));
        assert_eq!(control.reclaim_retired().unwrap(), 1);
    }

    #[test]
    fn graph_swap_and_transport_remain_allocation_free() {
        let config = AudioConfig::DEFAULT;
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control.submit_graph(realtime_source_graph(config)).unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        let mut output = [0.0; 128];
        engine.process_block(&mut output);
        control.reclaim_retired().unwrap();
        control
            .submit_graph(timeline_graph(config, 0.5, 128))
            .unwrap();
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        engine.process_block(&mut output);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
        assert_eq!(control.reclaim_retired().unwrap(), 1);
    }

    #[test]
    fn midi_scheduling_and_instrument_source_remain_allocation_free() {
        let config = AudioConfig::DEFAULT;
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control.submit_graph(midi_graph(config)).unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        let mut output = [0.0; 256];
        engine.process_block(&mut output);
        control.reclaim_retired().unwrap();
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        engine.process_block(&mut output);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
    }
}
