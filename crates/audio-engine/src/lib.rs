//! Device-independent realtime audio engine.
#![cfg_attr(test, allow(unsafe_code))]

mod readiness;
#[cfg(test)]
mod readiness_tests;
use readiness::ReadinessReceipt;
pub use readiness::StreamReadinessSnapshot;
mod transport_protocol;
use transport_protocol::TransportReceipts;
pub use transport_protocol::{
    TRANSPORT_HISTORY_CAPACITY, TransportCancelReason, TransportCommandKind,
    TransportCommandResult, TransportCommandState, TransportCommandStatus, TransportProtocolStatus,
};
mod metrics;
mod output_metrics;
mod pending;
mod publication;
use pending::PendingPublications;
use publication::PublicationReceipts;
pub use publication::{
    ADOPTION_HISTORY_CAPACITY, GraphAdoption, GraphOutcome, GraphPublicationStatus,
};

pub use metrics::RealtimeMetrics;

use audio_backend::{AudioCallback, RealtimeEventSink};
use audio_types::{AudioConfig, AudioError, RealtimeMetricsSnapshot};
pub use audio_types::{StreamReadiness, StreamReadinessState};
use daw_core::transport::{Transport, TransportCommand, TransportSnapshot};
use daw_dsp::graph::PhaseOneGraph;
use daw_mixer::AudioGraph;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
};

struct GraphUpdate {
    graph: EngineGraph,
    revision: u64,
    document_revision: u64,
    deadline: Option<daw_core::SamplePosition>,
    slot: usize,
    compare_mixer_deadline: bool,
}
struct GraphPublisher {
    queue: Producer<GraphUpdate>,
    next_revision: u64,
}

/// IDs remain exactly representable by `WebView` JSON consumers; zero is reserved.
pub const MAX_PUBLICATION_REVISION: u64 = (1_u64 << 53) - 1;

const GRAPH_QUEUE_CAPACITY: usize = 4;
const TRANSPORT_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
struct TransportRequest {
    id: u64,
    command: TransportCommand,
    stop_generation: u64,
}

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

    fn process(
        &mut self,
        output: &mut [f32],
        transport: &Transport,
        frames: usize,
    ) -> StreamReadiness {
        if frames == 0 {
            output.fill(0.0);
            if let Self::Timeline(graph) = self {
                graph.release_streaming();
            }
            return StreamReadiness::default();
        }
        match self {
            Self::TestTone(graph) => {
                graph.process(output);
                if transport.snapshot().playing {
                    StreamReadiness::ready()
                } else {
                    StreamReadiness::default()
                }
            }
            Self::Timeline(graph) => graph.process_with_readiness(output, transport),
        }
    }
    fn cancel_streaming(&mut self) {
        if let Self::Timeline(graph) = self {
            graph.cancel_streaming();
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
    stop_requested: AtomicU64,
    emergency_stop: AtomicBool,
    stop_applied: AtomicU64,
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
            stop_requested: AtomicU64::new(0),
            emergency_stop: AtomicBool::new(false),
            stop_applied: AtomicU64::new(0),
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
    readiness: Arc<ReadinessReceipt>,
    transport_receipts: Arc<TransportReceipts>,
    metrics: Arc<RealtimeMetrics>,
    graph_updates: Arc<Mutex<GraphPublisher>>,
    publications: Arc<PublicationReceipts>,
    pending_publications: Arc<PendingPublications>,
    retired_graphs: Arc<Mutex<Consumer<EngineGraph>>>,
    transport_commands: Arc<Mutex<Producer<TransportRequest>>>,
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
        self.submit_graph_revision(graph, 0, None).map(|_| ())
    }

    /// Accepted publication ID; completion is observable through `graph_publication_status`.
    pub fn submit_graph_revision(
        &self,
        graph: EngineGraph,
        document_revision: u64,
        deadline: Option<daw_core::SamplePosition>,
    ) -> Result<u64, AudioError> {
        self.publish_graph(graph, document_revision, deadline, false, false)
    }

    /// Latest-wins snapshots; older still-pending replaceable updates are superseded.
    pub fn submit_latest_graph_revision(
        &self,
        graph: EngineGraph,
        document_revision: u64,
        deadline: Option<daw_core::SamplePosition>,
    ) -> Result<u64, AudioError> {
        self.publish_graph(graph, document_revision, deadline, true, false)
    }

    /// Application snapshots compare mixer deadlines against the actual audible graph.
    pub fn submit_latest_timeline(
        &self,
        graph: AudioGraph,
        document_revision: u64,
    ) -> Result<u64, AudioError> {
        self.publish_graph(
            EngineGraph::timeline(graph),
            document_revision,
            None,
            true,
            true,
        )
    }

    /// True means cancellation won against callback claiming. It never undoes a document.
    pub fn cancel_pending_graph(&self, revision: u64) -> Result<bool, AudioError> {
        let _publisher = self
            .graph_updates
            .lock()
            .map_err(|_| AudioError::InvalidState("graph update lock was poisoned"))?;
        Ok(self.pending_publications.cancel(revision))
    }

    fn publish_graph(
        &self,
        graph: EngineGraph,
        document_revision: u64,
        deadline: Option<daw_core::SamplePosition>,
        replaceable: bool,
        compare_mixer_deadline: bool,
    ) -> Result<u64, AudioError> {
        self.publish_graph_owned(
            graph,
            document_revision,
            deadline,
            replaceable,
            compare_mixer_deadline,
        )
        .map_err(|(_, error)| error)
    }

    /// Recover the same prepared graph on backpressure; retry without rebuilding it.
    pub fn try_submit_latest_graph(
        &self,
        graph: EngineGraph,
        document_revision: u64,
    ) -> Result<u64, (EngineGraph, AudioError)> {
        self.publish_graph_owned(graph, document_revision, None, true, true)
    }

    fn publish_graph_owned(
        &self,
        graph: EngineGraph,
        document_revision: u64,
        deadline: Option<daw_core::SamplePosition>,
        replaceable: bool,
        compare_mixer_deadline: bool,
    ) -> Result<u64, (EngineGraph, AudioError)> {
        if let Err(error) = self.reclaim_retired() {
            return Err((graph, error));
        }
        let Ok(mut publisher) = self.graph_updates.lock() else {
            return Err((
                graph,
                AudioError::InvalidState("graph update lock was poisoned"),
            ));
        };
        let revision = publisher.next_revision;
        if revision > MAX_PUBLICATION_REVISION || document_revision > MAX_PUBLICATION_REVISION {
            return Err((
                graph,
                AudioError::InvalidState("graph publication revisions exhausted"),
            ));
        }
        let Some(slot) = self.pending_publications.reserve(revision, replaceable) else {
            return Err((graph, AudioError::PublicationBusy));
        };
        if let Err(PushError::Full(update)) = publisher.queue.push(GraphUpdate {
            graph,
            revision,
            document_revision,
            deadline,
            slot,
            compare_mixer_deadline,
        }) {
            self.pending_publications.finish(slot);
            return Err((update.graph, AudioError::PublicationBusy));
        }
        // Only successful acceptance may invalidate earlier snapshots. Claiming wins races.
        if replaceable {
            self.pending_publications.supersede_before(revision);
        }
        publisher.next_revision += 1;
        self.publications.accepted.store(revision, Ordering::SeqCst);
        Ok(revision)
    }

    #[must_use]
    pub fn graph_publication_status(&self, after_revision: u64) -> GraphPublicationStatus {
        self.publications.status(after_revision)
    }

    /// Compatibility entry point. Use `submit_transport` to retain the accepted ID.
    pub fn send_transport(&self, command: TransportCommand) -> Result<(), AudioError> {
        self.submit_transport(command).map(|_| ())
    }

    /// Stop remains out of band and never takes the normal producer mutex.
    pub fn submit_transport(&self, command: TransportCommand) -> Result<u64, AudioError> {
        transport_protocol::validate(command)?;
        if command == TransportCommand::Stop {
            let id = self.transport_receipts.accept(command).map_err(|_| {
                self.transport.emergency_stop.store(true, Ordering::Release);
                AudioError::InvalidState(
                    "transport IDs exhausted; safety Stop requested; restart the engine",
                )
            })?;
            self.transport
                .stop_requested
                .fetch_max(id, Ordering::AcqRel);
            self.transport_receipts
                .stop_dirty
                .store(true, Ordering::SeqCst);
            return Ok(id);
        }
        let mut producer = self
            .transport_commands
            .lock()
            .map_err(|_| AudioError::InvalidState("transport command lock was poisoned"))?;
        let stop_generation = self.transport.stop_requested.load(Ordering::Acquire);
        if matches!(
            command,
            TransportCommand::Play | TransportCommand::SetRecording(true)
        ) && stop_generation != self.transport.stop_applied.load(Ordering::Acquire)
        {
            return Err(AudioError::InvalidState(
                "stop is pending; retry playback after acknowledgement",
            ));
        }
        if producer.slots() == 0 {
            return Err(AudioError::Backend(
                "transport command queue is full".into(),
            ));
        }
        let id = self.transport_receipts.accept(command)?;
        producer
            .push(TransportRequest {
                id,
                command,
                stop_generation,
            })
            .map_err(|_| AudioError::Backend("transport queue publication failed".into()))?;
        Ok(id)
    }

    pub fn cancel_transport_command(&self, id: u64) -> Result<bool, AudioError> {
        self.validate_transport_id(id)?;
        self.transport_receipts.cancel(id)
    }
    pub fn transport_command_status(&self, id: u64) -> Result<TransportCommandStatus, AudioError> {
        self.validate_transport_id(id)?;
        Ok(self.transport_receipts.status(id))
    }
    fn validate_transport_id(&self, id: u64) -> Result<(), AudioError> {
        if id == 0 || id > self.transport_receipts.accepted.load(Ordering::SeqCst) {
            Err(AudioError::InvalidConfig(
                "transport ID was not accepted by this engine",
            ))
        } else {
            Ok(())
        }
    }
    #[must_use]
    pub fn transport_protocol_status(&self) -> TransportProtocolStatus {
        self.transport_receipts
            .summary(self.transport.stop_applied.load(Ordering::Acquire))
    }

    /// Last completed callback's admission, keyed to its actual adopted graph and transport generation.
    #[must_use]
    pub fn stream_readiness(&self) -> Option<StreamReadinessSnapshot> {
        self.readiness.snapshot()
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
    graph_updates: Consumer<GraphUpdate>,
    publications: Arc<PublicationReceipts>,
    pending_publications: Arc<PendingPublications>,
    retired_graphs: Producer<EngineGraph>,
    retired_fallback: Option<EngineGraph>,
    transport_commands: Consumer<TransportRequest>,
    transport: Transport,
    applied_stop_generation: u64,
    shared_transport: Arc<SharedTransport>,
    readiness: Arc<ReadinessReceipt>,
    active_graph_revision: u64,
    transport_generation: u64,
    waiting_callbacks: u64,
    transport_receipts: Arc<TransportReceipts>,
    pending_transport: Option<u64>,
    transport_context_id: u64,
}

impl EngineProcessor {
    #[allow(clippy::cast_precision_loss)]
    pub fn prepare(config: AudioConfig) -> Result<(Self, EngineControl), AudioError> {
        let config = config.validate()?;
        let initial_gain = 0.25_f32;
        let gain_bits = Arc::new(AtomicU32::new(initial_gain.to_bits()));
        let metrics = Arc::new(RealtimeMetrics::new(config.callback_deadline_ns()));
        let readiness = Arc::new(ReadinessReceipt::default());
        let transport_receipts = Arc::new(TransportReceipts::new());
        let mut graph = EngineGraph::test_tone(config);
        graph.set_test_tone_gain(initial_gain);
        let (graph_producer, graph_consumer) = RingBuffer::new(GRAPH_QUEUE_CAPACITY);
        let (retired_producer, retired_consumer) = RingBuffer::new(GRAPH_QUEUE_CAPACITY);
        let (transport_producer, transport_consumer) = RingBuffer::new(TRANSPORT_QUEUE_CAPACITY);
        let shared_transport = Arc::new(SharedTransport::new(config.sample_rate));
        let publications = Arc::new(PublicationReceipts::new());
        let pending_publications = Arc::new(PendingPublications::new());

        let processor = Self {
            config,
            graph,
            gain_bits: Arc::clone(&gain_bits),
            applied_gain_bits: initial_gain.to_bits(),
            metrics: Arc::clone(&metrics),
            graph_updates: graph_consumer,
            publications: Arc::clone(&publications),
            pending_publications: Arc::clone(&pending_publications),
            retired_graphs: retired_producer,
            retired_fallback: None,
            transport_commands: transport_consumer,
            transport: Transport::new(config.sample_rate),
            applied_stop_generation: 0,
            shared_transport: Arc::clone(&shared_transport),
            readiness: Arc::clone(&readiness),
            active_graph_revision: 0,
            transport_generation: 0,
            waiting_callbacks: 0,
            transport_receipts: Arc::clone(&transport_receipts),
            pending_transport: None,
            transport_context_id: 0,
        };
        let control = EngineControl {
            gain_bits,
            transport_receipts,
            readiness,
            metrics,
            graph_updates: Arc::new(Mutex::new(GraphPublisher {
                queue: graph_producer,
                next_revision: 1,
            })),
            publications,
            pending_publications,
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
        let previous_transport = self.transport.snapshot();
        self.apply_stop_barrier();
        self.cancel_requested_wait();
        for _ in 0..TRANSPORT_QUEUE_CAPACITY {
            let Ok(request) = self.transport_commands.pop() else {
                break;
            };
            self.apply_transport_request(request);
        }
        // A stop arriving during command draining still wins before rendering this block.
        self.apply_stop_barrier();
        let current_transport = self.transport.snapshot();
        if previous_transport.current_sample != current_transport.current_sample
            || previous_transport.loop_range != current_transport.loop_range
            || previous_transport.playing != current_transport.playing
        {
            self.graph.cancel_streaming();
        }
        // Adoption/deadline position must include this callback's seek/loop/stop commands.
        self.apply_graph_update();
        let requested_gain = self.gain_bits.load(Ordering::Acquire);
        if requested_gain != self.applied_gain_bits {
            self.graph
                .set_test_tone_gain(f32::from_bits(requested_gain));
            self.applied_gain_bits = requested_gain;
        }
        self.cancel_requested_wait();
        let protocol = Arc::clone(&self.transport_receipts);
        let mut admission = self.pending_transport.and_then(|id| protocol.begin(id));
        if admission
            .as_ref()
            .is_some_and(|guard| guard.result.state == TransportCommandState::CancelPending)
        {
            self.transport.apply(TransportCommand::Stop);
            self.transport_generation = self
                .transport_generation
                .saturating_add(1)
                .min(MAX_PUBLICATION_REVISION);
            self.graph.cancel_streaming();
            if let Some(guard) = &mut admission {
                Self::canceled(
                    &mut guard.result,
                    TransportCancelReason::Explicit,
                    self.transport.snapshot().current_sample,
                    self.active_graph_revision,
                    self.transport_generation,
                );
            }
            self.pending_transport = None;
        }
        let position = self.transport.snapshot().current_sample;
        let frames = output.len() / usize::from(self.config.channels);
        let readiness = self.graph.process(output, &self.transport, frames);
        if let Some(guard) = &mut admission {
            if Self::settle_admission(
                &mut guard.result,
                readiness,
                position,
                self.active_graph_revision,
                self.transport_generation,
                self.transport.snapshot().playing.then_some(frames),
            ) {
                self.pending_transport = None;
            }
        } else if readiness.state != StreamReadinessState::Waiting {
            self.pending_transport = None;
        }
        drop(admission);
        let rendered = if readiness.state == StreamReadinessState::Ready {
            frames
        } else {
            0
        };
        if readiness.state == StreamReadinessState::Waiting {
            self.waiting_callbacks = self.waiting_callbacks.saturating_add(1);
        }
        // The complete audio/MIDI/mixer timeline is held on unavailable data, never
        // advanced behind a silent hole or reconstructed from late sequential PCM.
        self.transport.advance(rendered);
        self.readiness.publish(StreamReadinessSnapshot {
            transport_command_id: self.transport_context_id,
            graph_revision: self.active_graph_revision,
            transport_generation: self.transport_generation,
            position,
            requested_frames: frames as u64,
            rendered_frames: rendered as u64,
            waiting_callbacks: self.waiting_callbacks,
            readiness,
        });
        self.shared_transport.publish(self.transport.snapshot());
        self.shared_transport
            .stop_applied
            .store(self.applied_stop_generation, Ordering::Release);
        self.metrics
            .record_callback(started.elapsed(), frames, self.config.sample_rate);
    }

    fn settle_admission(
        result: &mut TransportCommandResult,
        readiness: StreamReadiness,
        position: daw_core::SamplePosition,
        graph: u64,
        generation: u64,
        playing_frames: Option<usize>,
    ) -> bool {
        if !result.state.terminal() {
            result.readiness = readiness;
            result.graph_revision = graph;
            result.transport_generation = generation;
            // An empty callback must not acknowledge audio readiness for a playing
            // context. It has not admitted or produced even one complete frame.
            result.state = if playing_frames == Some(0) {
                TransportCommandState::Pending
            } else {
                match readiness.state {
                    StreamReadinessState::Waiting => TransportCommandState::Waiting,
                    StreamReadinessState::Failed => TransportCommandState::Failed,
                    StreamReadinessState::CapacityExceeded => {
                        TransportCommandState::CapacityExceeded
                    }
                    StreamReadinessState::Ready | StreamReadinessState::Inactive => {
                        TransportCommandState::Applied
                    }
                }
            };
            if result.state.terminal() {
                result.resolved_sample = Some(position);
            }
        }
        result.state.terminal()
    }

    fn canceled(
        result: &mut TransportCommandResult,
        reason: TransportCancelReason,
        position: daw_core::SamplePosition,
        graph: u64,
        generation: u64,
    ) {
        result.state = TransportCommandState::Canceled;
        result.readiness = StreamReadiness::default();
        result.cancel_reason = Some(reason);
        result.resolved_sample = Some(position);
        result.graph_revision = graph;
        result.transport_generation = generation;
    }
    fn resolve_pending(&mut self, reason: TransportCancelReason) {
        if let Some(id) = self.pending_transport.take()
            && let Some(mut guard) = self.transport_receipts.begin(id)
        {
            let reason = if guard.result.state == TransportCommandState::CancelPending {
                TransportCancelReason::Explicit
            } else {
                reason
            };
            Self::canceled(
                &mut guard.result,
                reason,
                self.transport.snapshot().current_sample,
                self.active_graph_revision,
                self.transport_generation,
            );
        }
    }
    fn cancel_requested_wait(&mut self) {
        if let Some(id) = self.pending_transport {
            let status = self.transport_receipts.status(id);
            if status
                .result
                .is_some_and(|result| result.state == TransportCommandState::CancelPending)
            {
                self.transport.apply(TransportCommand::Stop);
                self.transport_generation = self
                    .transport_generation
                    .saturating_add(1)
                    .min(MAX_PUBLICATION_REVISION);
                self.graph.cancel_streaming();
                self.resolve_pending(TransportCancelReason::Explicit);
            }
        }
    }
    fn apply_transport_request(&mut self, request: TransportRequest) {
        let protocol = Arc::clone(&self.transport_receipts);
        let mut receipt = protocol.begin(request.id);
        if receipt
            .as_ref()
            .is_some_and(|guard| guard.result.state == TransportCommandState::CancelPending)
        {
            if let Some(guard) = &mut receipt {
                Self::canceled(
                    &mut guard.result,
                    TransportCancelReason::Explicit,
                    self.transport.snapshot().current_sample,
                    self.active_graph_revision,
                    self.transport_generation,
                );
            }
            return;
        }
        let starts = matches!(
            request.command,
            TransportCommand::Play | TransportCommand::SetRecording(true)
        );
        if starts && request.stop_generation != self.applied_stop_generation {
            if let Some(guard) = &mut receipt {
                Self::canceled(
                    &mut guard.result,
                    TransportCancelReason::Stopped,
                    self.transport.snapshot().current_sample,
                    self.active_graph_revision,
                    self.transport_generation,
                );
            }
            return;
        }
        let admission = matches!(
            request.command,
            TransportCommand::Play
                | TransportCommand::Seek(_)
                | TransportCommand::SetLoop(_)
                | TransportCommand::SetRecording(true)
        );
        if admission {
            self.resolve_pending(TransportCancelReason::Superseded);
            self.transport_context_id = request.id;
        }
        self.transport.apply(request.command);
        self.transport_generation = self
            .transport_generation
            .saturating_add(1)
            .min(MAX_PUBLICATION_REVISION);
        let position = self.transport.snapshot().current_sample;
        if let Some(guard) = &mut receipt {
            guard.result.effect_applied = true;
            guard.result.applied_sample = Some(position);
            guard.result.graph_revision = self.active_graph_revision;
            guard.result.transport_generation = self.transport_generation;
            guard.result.state = if admission && self.transport.snapshot().playing {
                TransportCommandState::Pending
            } else {
                TransportCommandState::Applied
            };
            if guard.result.state == TransportCommandState::Applied {
                guard.result.resolved_sample = Some(position);
            }
        }
        if admission && self.transport.snapshot().playing {
            self.pending_transport = Some(request.id);
        }
    }

    fn apply_stop_barrier(&mut self) {
        let dirty = self
            .transport_receipts
            .stop_dirty
            .swap(false, Ordering::SeqCst);
        let generation = self.shared_transport.stop_requested.load(Ordering::Acquire);
        let emergency = self.shared_transport.emergency_stop.load(Ordering::Acquire);
        let changed = generation != self.applied_stop_generation;
        if changed || emergency {
            self.resolve_pending(TransportCancelReason::Stopped);
            self.transport.apply(TransportCommand::Stop);
            self.applied_stop_generation = generation;
            self.transport_context_id = generation;
            self.transport_generation = self
                .transport_generation
                .saturating_add(1)
                .min(MAX_PUBLICATION_REVISION);
        }
        if dirty || changed {
            self.transport_receipts.resolve_stops(
                generation,
                self.transport.snapshot().current_sample,
                self.active_graph_revision,
                self.transport_generation,
            );
        }
    }

    fn apply_graph_update(&mut self) {
        if let Some(retired) = self.retired_fallback.take()
            && let Err(PushError::Full(retired)) = self.retired_graphs.push(retired)
        {
            self.retired_fallback = Some(retired);
            return;
        }
        // Four queued entries at most; skipped graph ownership is retired, never dropped here.
        for _ in 0..GRAPH_QUEUE_CAPACITY {
            let Ok(update) = self.graph_updates.pop() else {
                break;
            };
            let GraphUpdate {
                mut graph,
                revision,
                document_revision,
                mut deadline,
                slot,
                compare_mixer_deadline,
            } = update;
            let snapshot = self.transport.snapshot();
            let outcome = self.pending_publications.claim(slot);
            if outcome == GraphOutcome::Adopted {
                if let (EngineGraph::Timeline(next), EngineGraph::Timeline(previous)) =
                    (&mut graph, &self.graph)
                {
                    if compare_mixer_deadline {
                        deadline = next.mixer_edit_deadline(previous);
                    }
                    next.inherit_state(previous, snapshot.current_sample);
                }
                self.graph.cancel_streaming();
                graph = std::mem::replace(&mut self.graph, graph);
                self.active_graph_revision = revision;
            }
            if let Err(PushError::Full(graph)) = self.retired_graphs.push(graph) {
                self.retired_fallback = Some(graph);
            }
            self.publications.record(GraphAdoption {
                revision,
                document_revision,
                adopted_sample: (outcome == GraphOutcome::Adopted)
                    .then_some(snapshot.current_sample),
                resolved_sample: snapshot.current_sample,
                outcome,
                deadline,
                late: outcome == GraphOutcome::Adopted
                    && snapshot.playing
                    && deadline.is_some_and(|position| snapshot.current_sample > position),
                playing: snapshot.playing,
            });
            self.pending_publications.finish(slot);
            if outcome == GraphOutcome::Adopted || self.retired_fallback.is_some() {
                break;
            }
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

    pub(super) fn assert_no_realtime_memory(work: impl FnOnce()) {
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        work();
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
    }

    #[test]
    fn project_stream_seek_loop_and_graph_adoption_do_not_allocate_or_free() {
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        let project = daw_project::Project::demo(config.sample_rate).unwrap();
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control
            .submit_graph(EngineGraph::timeline(
                project.build_audio_graph(config).unwrap(),
            ))
            .unwrap();
        control
            .send_transport(TransportCommand::SetLoop(daw_core::SampleRange::new(
                SamplePosition(13),
                SamplePosition(51),
            )))
            .unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        engine.process_block(&mut [0.0; 34]);
        for position in [14, 50, 20000, 17] {
            if position == 20000 {
                control
                    .send_transport(TransportCommand::SetLoop(None))
                    .unwrap();
            }
            control
                .submit_graph(EngineGraph::timeline(
                    project.build_audio_graph(config).unwrap(),
                ))
                .unwrap();
            control
                .send_transport(TransportCommand::Seek(SamplePosition(position)))
                .unwrap();
            let mut output = [0.0; 514];
            let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
            IN_REALTIME_TEST.with(|flag| flag.set(true));
            engine.process_block(&mut output);
            IN_REALTIME_TEST.with(|flag| flag.set(false));
            assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
            control.reclaim_retired().unwrap();
        }
    }

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
    #[test]
    fn overlap_loop_seek_and_saturated_stop_remain_allocation_free() {
        let config = AudioConfig::DEFAULT;
        let notes = (0_u64..20)
            .map(|id| MidiNote {
                id,
                start: SamplePosition(if id < 16 { 0 } else { 4 }),
                duration_samples: if id == 0 { 8 } else { 20 + id },
                key: 60,
                channel: 0,
                velocity: 0.4,
            })
            .collect();
        let clip = MidiClip {
            id: 1,
            timeline_start: SamplePosition(0),
            duration_samples: 4096,
            notes,
        };
        let source = MidiSynthSource::new(config, &[clip], None).unwrap();
        let mut track = Track::new(TrackId(1));
        track.set_timeline_source(Box::new(source)).unwrap();
        let mut graph = AudioGraph::new(config).unwrap();
        graph.add_track(track).unwrap();
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control.submit_graph(EngineGraph::timeline(graph)).unwrap();
        control
            .send_transport(TransportCommand::SetLoop(daw_core::SampleRange::new(
                SamplePosition(4),
                SamplePosition(31),
            )))
            .unwrap();
        control
            .send_transport(TransportCommand::Seek(SamplePosition(7)))
            .unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        let mut output = [0.0; 256];
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        engine.process_block(&mut output);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
        for _ in 0..super::TRANSPORT_QUEUE_CAPACITY {
            control.send_transport(TransportCommand::Play).unwrap();
        }
        control.send_transport(TransportCommand::Stop).unwrap();
        let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
        IN_REALTIME_TEST.with(|flag| flag.set(true));
        engine.process_block(&mut output);
        IN_REALTIME_TEST.with(|flag| flag.set(false));
        assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
        assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
        assert!(!control.transport().playing);
        control.send_transport(TransportCommand::Play).unwrap();
        engine.process_block(&mut output);
        assert!(control.transport().playing);
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
    }
    #[test]
    fn graph_publication_preserves_pcm_and_does_not_allocate_or_free() {
        let config = AudioConfig::DEFAULT;
        let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
        let (mut reference, reference_control) = EngineProcessor::prepare(config).unwrap();
        control.submit_graph(midi_graph(config)).unwrap();
        reference_control.submit_graph(midi_graph(config)).unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        reference_control
            .send_transport(TransportCommand::Play)
            .unwrap();
        actual.process_block(&mut [0.0; 34]);
        reference.process_block(&mut [0.0; 34]);
        for frames in [17, 128, 257, 1, 127, 512] {
            let mut output = vec![0.0; frames * 2];
            let mut expected = output.clone();
            // Prepare outside instrumentation; only publication/render is realtime.
            control.submit_graph(midi_graph(config)).unwrap();
            let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
            IN_REALTIME_TEST.with(|flag| flag.set(true));
            actual.process_block(&mut output);
            IN_REALTIME_TEST.with(|flag| flag.set(false));
            assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
            reference.process_block(&mut expected);
            assert_eq!(output, expected);
            control.reclaim_retired().unwrap();
        }
    }

    #[test]
    fn active_mixer_ramps_and_publication_do_not_allocate_or_free() {
        let config = AudioConfig::DEFAULT;
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control.submit_graph(midi_graph(config)).unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        engine.process_block(&mut [0.0; 34]);
        for stage in 0..32 {
            let EngineGraph::Timeline(mut graph) = midi_graph(config) else {
                unreachable!()
            };
            let track = graph.track_mut(TrackId(1)).unwrap();
            track.set_gain(if stage % 2 == 0 { 0.2 } else { 0.9 });
            track.set_pan(if stage % 3 == 0 { -0.7 } else { 0.8 });
            track.set_mute(stage % 5 == 0);
            track.set_solo(stage % 4 == 0);
            let position = control.transport().current_sample.0;
            track
                .set_mixer_events(&[
                    daw_mixer::MixerEvent {
                        id: 1,
                        position: SamplePosition(position + 1),
                        change: daw_mixer::MixerChange::Gain(0.4),
                    },
                    daw_mixer::MixerEvent {
                        id: 2,
                        position: SamplePosition(position + 3),
                        change: daw_mixer::MixerChange::Pan(-0.5),
                    },
                    daw_mixer::MixerEvent {
                        id: 3,
                        position: SamplePosition(position + 5),
                        change: daw_mixer::MixerChange::Mute(true),
                    },
                    daw_mixer::MixerEvent {
                        id: 4,
                        position: SamplePosition(position + 7),
                        change: daw_mixer::MixerChange::Mute(false),
                    },
                    daw_mixer::MixerEvent {
                        id: 5,
                        position: SamplePosition(position + 11),
                        change: daw_mixer::MixerChange::Solo(true),
                    },
                ])
                .unwrap();
            graph.set_master_gain(if stage % 2 == 0 { 0.7 } else { 1.0 });
            control.submit_graph(EngineGraph::timeline(graph)).unwrap();
            let mut output = [0.0; 34];
            let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
            IN_REALTIME_TEST.with(|flag| flag.set(true));
            engine.process_block(&mut output);
            IN_REALTIME_TEST.with(|flag| flag.set(false));
            assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
            assert!(output.iter().all(|sample| sample.is_finite()));
            control.reclaim_retired().unwrap();
        }
    }

    #[test]
    fn canceled_and_coalesced_graphs_preserve_pcm_without_callback_allocation_or_free() {
        let config = AudioConfig::DEFAULT;
        let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
        let (mut reference, reference_control) = EngineProcessor::prepare(config).unwrap();
        for ctl in [&control, &reference_control] {
            ctl.submit_graph(midi_graph(config)).unwrap();
            ctl.send_transport(TransportCommand::SetLoop(daw_core::SampleRange::new(
                SamplePosition(7),
                SamplePosition(131),
            )))
            .unwrap();
            ctl.send_transport(TransportCommand::Play).unwrap();
        }
        actual.process_block(&mut [0.0; 34]);
        reference.process_block(&mut [0.0; 34]);
        for frames in [1, 17, 64, 127, 128, 257, 512, 1] {
            for document in 1..=4 {
                let EngineGraph::Timeline(mut graph) = midi_graph(config) else {
                    unreachable!()
                };
                graph
                    .track_mut(TrackId(1))
                    .unwrap()
                    .set_mixer_events(&[
                        daw_mixer::MixerEvent {
                            id: 7,
                            position: SamplePosition(3000),
                            change: daw_mixer::MixerChange::Gain(0.8),
                        },
                        daw_mixer::MixerEvent {
                            id: 3,
                            position: SamplePosition(3500),
                            change: daw_mixer::MixerChange::Mute(false),
                        },
                    ])
                    .unwrap(); // Outside the loop, but exercises nonempty deadline indexes and ownership.
                let revision = control.submit_latest_timeline(graph, document).unwrap();
                if document == 1 {
                    assert!(control.cancel_pending_graph(revision).unwrap());
                }
            }
            let mut output = vec![0.0; frames * 2];
            let mut expected = output.clone();
            let before = REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed);
            IN_REALTIME_TEST.with(|flag| flag.set(true));
            actual.process_block(&mut output);
            IN_REALTIME_TEST.with(|flag| flag.set(false));
            assert_eq!(REALTIME_MEMORY_EVENTS.load(Ordering::Relaxed), before);
            reference.process_block(&mut expected);
            assert_eq!(output, expected);
            let status = control.graph_publication_status(0);
            assert!(!status.pending);
            assert_eq!(status.adopted_revision, status.accepted_revision);
            control.reclaim_retired().unwrap();
        }
    }

    fn render_engine_partitions(partitions: &[usize], looping: bool) -> (Vec<f32>, u64) {
        let config = AudioConfig::DEFAULT;
        let (mut engine, control) = EngineProcessor::prepare(config).unwrap();
        control.submit_graph(midi_graph(config)).unwrap();
        if looping {
            control
                .send_transport(TransportCommand::SetLoop(daw_core::SampleRange::new(
                    SamplePosition(7),
                    SamplePosition(130),
                )))
                .unwrap();
        }
        control
            .send_transport(TransportCommand::Seek(SamplePosition(5)))
            .unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        let mut output = vec![0.0; 4096];
        let mut frame = 0;
        let mut block = 0;
        while frame < 2048 {
            let count = partitions[block % partitions.len()].min(2048 - frame);
            engine.process_block(&mut output[frame * 2..(frame + count) * 2]);
            frame += count;
            block += 1;
        }
        (output, control.transport().current_sample.0)
    }
    #[test]
    fn full_engine_midi_automation_and_loop_output_is_partition_invariant() {
        for looping in [false, true] {
            let expected = render_engine_partitions(&[2048], looping);
            for blocks in [&[1][..], &[17, 64, 127, 128, 257, 512, 1024][..]] {
                assert_eq!(render_engine_partitions(blocks, looping), expected);
            }
        }
    }
}

#[cfg(test)]
mod p0_regressions {
    use super::*;
    #[test]
    fn publication_receipts_distinguish_acceptance_adoption_and_actual_seek_deadlines() {
        use daw_core::SamplePosition;
        let config = AudioConfig::DEFAULT;
        let (mut processor, control) = EngineProcessor::prepare(config).unwrap();
        let first = control
            .submit_graph_revision(EngineGraph::test_tone(config), 7, Some(SamplePosition(128)))
            .unwrap();
        let pending = control.graph_publication_status(0);
        assert_eq!(pending.accepted_revision, first);
        assert_eq!(pending.adopted_revision, 0);
        assert!(pending.acknowledgements.is_empty());
        control
            .send_transport(TransportCommand::Seek(SamplePosition(128)))
            .unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        processor.process_block(&mut [0.0; 2]);
        let ack = control.graph_publication_status(0).acknowledgements[0];
        assert_eq!(ack.document_revision, 7);
        assert_eq!(ack.adopted_sample, Some(SamplePosition(128)));
        assert!(!ack.late); // equality is on time
        let second = control
            .submit_graph_revision(EngineGraph::test_tone(config), 3, Some(SamplePosition(128)))
            .unwrap();
        processor.process_block(&mut [0.0; 2]);
        let status = control.graph_publication_status(first);
        assert_eq!(status.adopted_revision, second);
        assert_eq!(status.late_count, 1);
        assert!(status.acknowledgements[0].late);
        assert_eq!(
            status.acknowledgements[0].adopted_sample,
            Some(SamplePosition(129))
        );
        assert_eq!(status.acknowledgements[0].document_revision, 3); // history can move backwards
        control.send_transport(TransportCommand::Stop).unwrap();
        control
            .submit_graph_revision(EngineGraph::test_tone(config), 9, Some(SamplePosition(0)))
            .unwrap();
        processor.process_block(&mut [0.0; 2]);
        let ack = control.graph_publication_status(second).acknowledgements[0];
        assert!(!ack.playing && !ack.late);
        assert_eq!(control.graph_publication_status(0).late_count, 1);
    }

    #[test]
    fn retirement_backpressure_does_not_acknowledge_a_graph_that_is_still_pending() {
        let config = AudioConfig::DEFAULT;
        let (mut processor, control) = EngineProcessor::prepare(config).unwrap();
        control
            .submit_graph_revision(EngineGraph::test_tone(config), 1, None)
            .unwrap();
        control
            .submit_graph_revision(EngineGraph::test_tone(config), 2, None)
            .unwrap();
        for _ in 0..GRAPH_QUEUE_CAPACITY {
            assert!(
                processor
                    .retired_graphs
                    .push(EngineGraph::test_tone(config))
                    .is_ok()
            );
        }
        processor.process_block(&mut [0.0; 2]);
        assert_eq!(control.graph_publication_status(0).adopted_revision, 1);
        processor.process_block(&mut [0.0; 2]);
        assert_eq!(control.graph_publication_status(0).adopted_revision, 1);
        assert_eq!(control.graph_publication_status(0).accepted_revision, 2);
        control.reclaim_retired().unwrap();
        processor.process_block(&mut [0.0; 2]);
        assert_eq!(control.graph_publication_status(0).adopted_revision, 2);
    }

    #[test]
    fn full_queues_and_exhausted_revision_ids_do_not_create_false_acceptance() {
        let config = AudioConfig::DEFAULT;
        let (mut processor, control) = EngineProcessor::prepare(config).unwrap();
        for document in 0..4 {
            control
                .submit_graph_revision(EngineGraph::test_tone(config), document, None)
                .unwrap();
        }
        assert!(
            control
                .submit_graph_revision(EngineGraph::test_tone(config), 99, None)
                .is_err()
        );
        assert_eq!(control.graph_publication_status(0).accepted_revision, 4);
        processor.process_block(&mut [0.0; 2]);
        assert_eq!(
            control
                .submit_graph_revision(EngineGraph::test_tone(config), 8, None)
                .unwrap(),
            5
        );
        control.graph_updates.lock().unwrap().next_revision = MAX_PUBLICATION_REVISION + 1;
        assert!(
            control
                .submit_graph_revision(EngineGraph::test_tone(config), 8, None)
                .is_err()
        );
        assert_eq!(control.graph_publication_status(0).accepted_revision, 5);
    }

    #[test]
    fn stop_is_accepted_when_the_transport_queue_is_full() {
        let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        for _ in 0..TRANSPORT_QUEUE_CAPACITY {
            control.send_transport(TransportCommand::Play).unwrap();
        }
        assert!(
            control.send_transport(TransportCommand::Stop).is_ok(),
            "stop must not compete for ordinary queue capacity"
        );
        processor.process_block(&mut [0.0; 256]);
        assert!(
            !control.transport().playing,
            "queued play requests must not override the stop barrier"
        );
    }
    #[test]
    fn metrics_use_the_actual_callback_frame_count() {
        let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        processor.process_block(&mut [0.0; 128]); // 64 stereo frames, requested size is 128 frames.
        assert!((control.metrics().deadline_us - 1333.333).abs() < 0.01);
    }
    #[test]
    fn stop_barrier_rejects_early_restart_but_preserves_clock_settings() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        control
            .send_transport(TransportCommand::SetTempo(135.0))
            .unwrap();
        control
            .send_transport(TransportCommand::SetTimeSignature(
                daw_core::transport::TimeSignature {
                    numerator: 7,
                    denominator: 8,
                },
            ))
            .unwrap();
        control.send_transport(TransportCommand::Play).unwrap();
        control
            .send_transport(TransportCommand::SetRecording(true))
            .unwrap();
        control.send_transport(TransportCommand::Stop).unwrap();
        assert!(control.send_transport(TransportCommand::Play).is_err());
        assert!(
            control
                .send_transport(TransportCommand::SetRecording(true))
                .is_err()
        );
        control
            .send_transport(TransportCommand::Seek(daw_core::SamplePosition(123)))
            .unwrap();
        engine.process_block(&mut [0.0; 256]);
        let stopped = control.transport();
        assert!(!stopped.playing && !stopped.recording);
        assert_eq!(stopped.current_sample.0, 123);
        assert!((stopped.tempo_bpm - 135.0).abs() < f64::EPSILON);
        assert_eq!(stopped.time_signature.denominator, 8);
        control.send_transport(TransportCommand::Play).unwrap();
        engine.process_block(&mut [0.0; 256]);
        assert!(control.transport().playing);
        assert_eq!(control.transport().current_sample.0, 251);
    }

    #[test]
    fn repeated_stops_invalidate_queued_play_from_an_earlier_generation() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        let mut output = [0.0; 256];
        for _ in 0..2000 {
            control.send_transport(TransportCommand::Play).unwrap();
            control.send_transport(TransportCommand::Stop).unwrap();
            control.send_transport(TransportCommand::Stop).unwrap();
            engine.process_block(&mut output);
            assert!(!control.transport().playing);
        }
    }
}

#[cfg(test)]
mod coalescing_tests;
