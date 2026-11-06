//! Epoch-scoped, bounded Transport receipts. Control cancellation never locks audio.
use crate::MAX_PUBLICATION_REVISION;
use audio_types::{AudioError, StreamReadiness, StreamReadinessState};
use daw_core::{SamplePosition, transport::TransportCommand};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

pub const TRANSPORT_HISTORY_CAPACITY: usize = 256;
const WRITING: u64 = 15;
const MASK: u64 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportCommandKind {
    Play,
    Stop,
    Seek,
    Loop,
    Tempo,
    TimeSignature,
    Recording,
}
impl From<TransportCommand> for TransportCommandKind {
    fn from(command: TransportCommand) -> Self {
        match command {
            TransportCommand::Play => Self::Play,
            TransportCommand::Stop => Self::Stop,
            TransportCommand::Seek(_) => Self::Seek,
            TransportCommand::SetLoop(_) => Self::Loop,
            TransportCommand::SetTempo(_) => Self::Tempo,
            TransportCommand::SetTimeSignature(_) => Self::TimeSignature,
            TransportCommand::SetRecording(_) => Self::Recording,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[repr(u64)]
pub enum TransportCommandState {
    Accepted = 1,
    Pending,
    Waiting,
    Applied,
    CancelPending,
    Canceled,
    Failed,
    CapacityExceeded,
}
impl TransportCommandState {
    pub(super) fn terminal(self) -> bool {
        matches!(
            self,
            Self::Applied | Self::Canceled | Self::Failed | Self::CapacityExceeded
        )
    }
    fn decode(code: u64) -> Option<Self> {
        Some(match code {
            1 => Self::Accepted,
            2 => Self::Pending,
            3 => Self::Waiting,
            4 => Self::Applied,
            5 => Self::CancelPending,
            6 => Self::Canceled,
            7 => Self::Failed,
            8 => Self::CapacityExceeded,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[repr(u64)]
pub enum TransportCancelReason {
    Explicit = 1,
    Superseded,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportCommandResult {
    pub command_id: u64,
    pub kind: TransportCommandKind,
    pub state: TransportCommandState,
    pub effect_applied: bool,
    pub applied_sample: Option<SamplePosition>,
    pub resolved_sample: Option<SamplePosition>,
    pub graph_revision: u64,
    pub transport_generation: u64,
    pub cancel_reason: Option<TransportCancelReason>,
    pub readiness: StreamReadiness,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportCommandStatus {
    pub accepted_id: u64,
    pub command_id: u64,
    pub history_lost: bool,
    pub updating: bool,
    pub result: Option<TransportCommandResult>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransportProtocolStatus {
    pub accepted_id: u64,
    pub applied_stop_id: u64,
    pub lost_receipts: u64,
    pub latest: Option<TransportCommandStatus>,
}

struct Cell {
    tag: AtomicU64,
    version: AtomicU64,
    kind: AtomicU64,
    applied: AtomicU64,
    resolved: AtomicU64,
    flags: AtomicU64,
    graph: AtomicU64,
    generation: AtomicU64,
    reason: AtomicU64,
    readiness: AtomicU64,
    required: AtomicU64,
    ready: AtomicU64,
}
impl Cell {
    fn new() -> Self {
        Self {
            tag: AtomicU64::new(0),
            version: AtomicU64::new(0),
            kind: AtomicU64::new(0),
            applied: AtomicU64::new(0),
            resolved: AtomicU64::new(0),
            flags: AtomicU64::new(0),
            graph: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            reason: AtomicU64::new(0),
            readiness: AtomicU64::new(0),
            required: AtomicU64::new(0),
            ready: AtomicU64::new(0),
        }
    }
    fn read(&self, id: u64, state: TransportCommandState) -> TransportCommandResult {
        let flags = self.flags.load(SeqCst);
        TransportCommandResult {
            command_id: id,
            kind: match self.kind.load(SeqCst) {
                1 => TransportCommandKind::Stop,
                2 => TransportCommandKind::Seek,
                3 => TransportCommandKind::Loop,
                4 => TransportCommandKind::Tempo,
                5 => TransportCommandKind::TimeSignature,
                6 => TransportCommandKind::Recording,
                _ => TransportCommandKind::Play,
            },
            state,
            effect_applied: flags & 1 != 0,
            applied_sample: (flags & 1 != 0).then(|| SamplePosition(self.applied.load(SeqCst))),
            resolved_sample: (flags & 2 != 0).then(|| SamplePosition(self.resolved.load(SeqCst))),
            graph_revision: self.graph.load(SeqCst),
            transport_generation: self.generation.load(SeqCst),
            cancel_reason: match self.reason.load(SeqCst) {
                1 => Some(TransportCancelReason::Explicit),
                2 => Some(TransportCancelReason::Superseded),
                3 => Some(TransportCancelReason::Stopped),
                _ => None,
            },
            readiness: StreamReadiness {
                state: match self.readiness.load(SeqCst) {
                    1 => StreamReadinessState::Ready,
                    2 => StreamReadinessState::Waiting,
                    3 => StreamReadinessState::Failed,
                    4 => StreamReadinessState::CapacityExceeded,
                    _ => StreamReadinessState::Inactive,
                },
                required_pages: self.required.load(SeqCst),
                ready_pages: self.ready.load(SeqCst),
            },
        }
    }
    fn write(&self, result: TransportCommandResult) {
        self.kind.store(
            match result.kind {
                TransportCommandKind::Play => 0,
                TransportCommandKind::Stop => 1,
                TransportCommandKind::Seek => 2,
                TransportCommandKind::Loop => 3,
                TransportCommandKind::Tempo => 4,
                TransportCommandKind::TimeSignature => 5,
                TransportCommandKind::Recording => 6,
            },
            SeqCst,
        );
        self.applied
            .store(result.applied_sample.map_or(0, |p| p.0), SeqCst);
        self.resolved
            .store(result.resolved_sample.map_or(0, |p| p.0), SeqCst);
        self.flags.store(
            u64::from(result.effect_applied) | (u64::from(result.resolved_sample.is_some()) << 1),
            SeqCst,
        );
        self.graph.store(result.graph_revision, SeqCst);
        self.generation.store(result.transport_generation, SeqCst);
        self.reason
            .store(result.cancel_reason.map_or(0, |r| r as u64), SeqCst);
        self.readiness.store(
            match result.readiness.state {
                StreamReadinessState::Inactive => 0,
                StreamReadinessState::Ready => 1,
                StreamReadinessState::Waiting => 2,
                StreamReadinessState::Failed => 3,
                StreamReadinessState::CapacityExceeded => 4,
            },
            SeqCst,
        );
        self.required.store(result.readiness.required_pages, SeqCst);
        self.ready.store(result.readiness.ready_pages, SeqCst);
    }
}

pub(super) struct TransportReceipts {
    next: AtomicU64,
    pub accepted: AtomicU64,
    pub stop_dirty: AtomicBool,
    lost: AtomicU64,
    cells: [Cell; TRANSPORT_HISTORY_CAPACITY],
}
pub(super) struct ResultGuard<'a> {
    cell: &'a Cell,
    pub result: TransportCommandResult,
}
impl Drop for ResultGuard<'_> {
    fn drop(&mut self) {
        self.cell.version.fetch_add(1, SeqCst);
        self.cell.write(self.result);
        self.cell.version.fetch_add(1, SeqCst);
        self.cell.tag.store(
            (self.result.command_id << 4) | self.result.state as u64,
            SeqCst,
        );
    }
}
impl TransportReceipts {
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            accepted: AtomicU64::new(0),
            stop_dirty: AtomicBool::new(false),
            lost: AtomicU64::new(0),
            cells: std::array::from_fn(|_| Cell::new()),
        }
    }
    fn cell(&self, id: u64) -> &Cell {
        &self.cells[usize::try_from(id % TRANSPORT_HISTORY_CAPACITY as u64).unwrap_or(0)]
    }
    pub fn accept(&self, command: TransportCommand) -> Result<u64, AudioError> {
        let id = self
            .next
            .fetch_update(SeqCst, SeqCst, |next| {
                (next <= MAX_PUBLICATION_REVISION).then(|| next + 1)
            })
            .map_err(|_| AudioError::InvalidState("transport IDs exhausted"))?;
        self.register(id, command);
        Ok(id)
    }
    fn register(&self, id: u64, command: TransportCommand) {
        let cell = self.cell(id);
        let old = cell.tag.load(SeqCst);
        if old & MASK == WRITING
            || cell
                .tag
                .compare_exchange(old, (id << 4) | WRITING, SeqCst, SeqCst)
                .is_err()
        {
            // Stop must remain independent of receipt retention contention. Missing
            // bounded history is explicit; an older writer can never corrupt a new ID.
            self.lost.fetch_add(1, SeqCst);
        } else {
            let _guard = ResultGuard {
                cell,
                result: TransportCommandResult {
                    command_id: id,
                    kind: command.into(),
                    state: TransportCommandState::Accepted,
                    effect_applied: false,
                    applied_sample: None,
                    resolved_sample: None,
                    graph_revision: 0,
                    transport_generation: 0,
                    cancel_reason: None,
                    readiness: StreamReadiness::default(),
                },
            };
        }
        self.accepted.fetch_max(id, SeqCst);
    }
    pub fn begin(&self, id: u64) -> Option<ResultGuard<'_>> {
        let cell = self.cell(id);
        for _ in 0..4 {
            let tag = cell.tag.load(SeqCst);
            if tag >> 4 != id {
                return None;
            }
            let state = TransportCommandState::decode(tag & MASK)?;
            if cell
                .tag
                .compare_exchange(tag, (id << 4) | WRITING, SeqCst, SeqCst)
                .is_ok()
            {
                return Some(ResultGuard {
                    cell,
                    result: cell.read(id, state),
                });
            }
        }
        None
    }
    pub fn cancel(&self, id: u64) -> Result<bool, AudioError> {
        let cell = self.cell(id);
        let tag = cell.tag.load(SeqCst);
        if tag >> 4 != id {
            return Err(AudioError::InvalidState(
                "transport result is no longer retained",
            ));
        }
        let state = TransportCommandState::decode(tag & MASK).ok_or(AudioError::InvalidState(
            "transport result is being updated; retry cancellation",
        ))?;
        if state.terminal() {
            return Ok(false);
        }
        if cell.kind.load(SeqCst) == 1 {
            return Err(AudioError::InvalidState("Stop cannot be canceled"));
        }
        cell.tag
            .compare_exchange(
                tag,
                (id << 4) | TransportCommandState::CancelPending as u64,
                SeqCst,
                SeqCst,
            )
            .map(|_| true)
            .map_err(|_| {
                AudioError::InvalidState("transport result is being updated; retry cancellation")
            })
    }
    pub fn status(&self, id: u64) -> TransportCommandStatus {
        let cell = self.cell(id);
        for _ in 0..8 {
            let version = cell.version.load(SeqCst);
            if !version.is_multiple_of(2) {
                continue;
            }
            let tag = cell.tag.load(SeqCst);
            if tag >> 4 != id {
                return TransportCommandStatus {
                    accepted_id: self.accepted.load(SeqCst),
                    command_id: id,
                    history_lost: true,
                    updating: false,
                    result: None,
                };
            }
            if let Some(state) = TransportCommandState::decode(tag & MASK) {
                let result = cell.read(id, state);
                if cell.tag.load(SeqCst) == tag && cell.version.load(SeqCst) == version {
                    return TransportCommandStatus {
                        accepted_id: self.accepted.load(SeqCst),
                        command_id: id,
                        history_lost: false,
                        updating: false,
                        result: Some(result),
                    };
                }
            }
        }
        TransportCommandStatus {
            accepted_id: self.accepted.load(SeqCst),
            command_id: id,
            history_lost: false,
            updating: true,
            result: None,
        }
    }
    pub fn resolve_stops(
        &self,
        latest: u64,
        position: SamplePosition,
        graph: u64,
        generation: u64,
    ) {
        for cell in &self.cells {
            let tag = cell.tag.load(SeqCst);
            let id = tag >> 4;
            if id == 0 || id > latest || cell.kind.load(SeqCst) != 1 {
                continue;
            }
            if let Some(mut guard) = self.begin(id) {
                if guard.result.state.terminal() {
                    continue;
                }
                guard.result.effect_applied = id == latest;
                guard.result.applied_sample = (id == latest).then_some(position);
                guard.result.resolved_sample = Some(position);
                guard.result.graph_revision = graph;
                guard.result.transport_generation = generation;
                guard.result.state = if id == latest {
                    TransportCommandState::Applied
                } else {
                    TransportCommandState::Canceled
                };
                guard.result.cancel_reason =
                    (id != latest).then_some(TransportCancelReason::Superseded);
            }
        }
    }
    pub fn summary(&self, stop: u64) -> TransportProtocolStatus {
        let accepted = self.accepted.load(SeqCst);
        TransportProtocolStatus {
            accepted_id: accepted,
            applied_stop_id: stop,
            lost_receipts: self.lost.load(SeqCst),
            latest: (accepted != 0).then(|| self.status(accepted)),
        }
    }
}

pub(super) fn validate(command: TransportCommand) -> Result<(), AudioError> {
    let valid = match command {
        TransportCommand::SetTempo(value) => value.is_finite() && (20.0..=400.0).contains(&value),
        TransportCommand::SetTimeSignature(value) => {
            value.numerator > 0 && value.denominator.is_power_of_two()
        }
        TransportCommand::SetLoop(Some(range)) => range.end > range.start,
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(AudioError::InvalidConfig("invalid transport command"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EngineGraph, EngineProcessor};
    use audio_types::AudioConfig;
    use daw_mixer::AudioGraph;
    use std::sync::Arc;
    fn render(engine: &mut EngineProcessor) {
        crate::tests::assert_no_realtime_memory(|| engine.process_block(&mut [0.0; 8]));
    }
    #[test]
    fn queue_rejection_does_not_consume_ids_and_stop_does_not_take_the_producer_lock() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        let mut ids = Vec::new();
        for i in 0..64_u64 {
            ids.push(
                control
                    .submit_transport(if i.is_multiple_of(2) {
                        TransportCommand::Seek(SamplePosition(i))
                    } else {
                        TransportCommand::Play
                    })
                    .unwrap(),
            );
        }
        assert!(
            control
                .submit_transport(TransportCommand::Seek(SamplePosition(999)))
                .is_err()
        );
        assert_eq!(control.transport_protocol_status().accepted_id, 64);
        let lock = control.transport_commands.lock().unwrap();
        let stop = control.submit_transport(TransportCommand::Stop).unwrap();
        assert_eq!(stop, 65);
        drop(lock);
        render(&mut engine);
        let result = control
            .transport_command_status(stop)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.state, TransportCommandState::Applied);
        assert!(result.effect_applied);
        assert_eq!(control.transport_protocol_status().applied_stop_id, stop);
        assert!(!control.transport().playing);
        let canceled = control
            .transport_command_status(*ids.last().unwrap())
            .unwrap()
            .result
            .unwrap();
        assert_eq!(canceled.cancel_reason, Some(TransportCancelReason::Stopped));
        assert!(!canceled.effect_applied);
        // Telemetry retention cannot disable the safety barrier either.
        let writer = control.transport_receipts.begin(stop).unwrap();
        let mut latest = stop;
        for _ in 0..TRANSPORT_HISTORY_CAPACITY {
            latest = control.submit_transport(TransportCommand::Stop).unwrap();
        }
        assert!(
            control
                .transport_command_status(latest)
                .unwrap()
                .history_lost
        );
        drop(writer);
        render(&mut engine);
        assert_eq!(control.transport_protocol_status().applied_stop_id, latest);
        assert!(!control.transport().playing);
    }
    #[test]
    fn queued_cancellation_has_no_effect_and_terminal_results_cannot_be_canceled() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        let id = control
            .submit_transport(TransportCommand::Seek(SamplePosition(77)))
            .unwrap();
        assert_eq!(
            control
                .transport_command_status(id)
                .unwrap()
                .result
                .unwrap()
                .state,
            TransportCommandState::Accepted
        );
        assert!(control.cancel_transport_command(id).unwrap());
        assert_eq!(
            control
                .transport_command_status(id)
                .unwrap()
                .result
                .unwrap()
                .state,
            TransportCommandState::CancelPending
        );
        render(&mut engine);
        let result = control
            .transport_command_status(id)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.state, TransportCommandState::Canceled);
        assert!(!result.effect_applied);
        assert!(result.applied_sample.is_none());
        assert_eq!(control.transport().current_sample, SamplePosition::ZERO);
        assert!(!control.cancel_transport_command(id).unwrap());
        let applied = control
            .submit_transport(TransportCommand::Seek(SamplePosition(99)))
            .unwrap();
        render(&mut engine);
        assert_eq!(
            control
                .transport_command_status(applied)
                .unwrap()
                .result
                .unwrap()
                .state,
            TransportCommandState::Applied
        );
        assert!(!control.cancel_transport_command(applied).unwrap());
        assert_eq!(control.transport().current_sample, SamplePosition(99));
        let stop = control.submit_transport(TransportCommand::Stop).unwrap();
        assert!(control.cancel_transport_command(stop).is_err());
        render(&mut engine);
    }
    #[test]
    fn bounded_retention_and_protected_slots_never_allow_old_writers_to_corrupt_new_ids() {
        let registry = TransportReceipts::new();
        let first = registry
            .accept(TransportCommand::Seek(SamplePosition(1)))
            .unwrap();
        let guard = registry.begin(first).unwrap();
        let mut last = 0;
        for _ in 0..TRANSPORT_HISTORY_CAPACITY {
            last = registry.accept(TransportCommand::Stop).unwrap();
        }
        assert!(registry.status(last).history_lost);
        assert_eq!(registry.summary(0).lost_receipts, 1);
        drop(guard);
        assert_eq!(
            registry.status(first).result.unwrap().kind,
            TransportCommandKind::Seek
        );
        for _ in 0..TRANSPORT_HISTORY_CAPACITY {
            last = registry.accept(TransportCommand::SetTempo(100.0)).unwrap();
        }
        assert!(registry.status(first).history_lost);
        assert!(registry.begin(first).is_none());
        assert_eq!(
            registry.status(last).result.unwrap().kind,
            TransportCommandKind::Tempo
        );
    }
    #[test]
    fn concurrent_same_state_updates_never_produce_torn_results() {
        let registry = Arc::new(TransportReceipts::new());
        let id = registry
            .accept(TransportCommand::Seek(SamplePosition(3)))
            .unwrap();
        let writer = Arc::clone(&registry);
        let worker = std::thread::spawn(move || {
            for n in 1..10_000 {
                let mut guard = writer.begin(id).unwrap();
                guard.result.state = TransportCommandState::Waiting;
                guard.result.effect_applied = true;
                guard.result.graph_revision = n;
                guard.result.transport_generation = n * 2;
                guard.result.applied_sample = Some(SamplePosition(n * 3));
                guard.result.readiness = StreamReadiness {
                    state: StreamReadinessState::Waiting,
                    required_pages: n * 4,
                    ready_pages: n * 5,
                };
            }
        });
        for _ in 0..10_000 {
            if let Some(result) = registry.status(id).result
                && result.state == TransportCommandState::Waiting
            {
                assert_eq!(result.transport_generation, result.graph_revision * 2);
                assert_eq!(result.applied_sample.unwrap().0, result.graph_revision * 3);
                assert_eq!(result.readiness.required_pages, result.graph_revision * 4);
                assert_eq!(result.readiness.ready_pages, result.graph_revision * 5);
            }
        }
        worker.join().unwrap();
    }
    #[test]
    fn delayed_older_stop_is_settled_even_after_the_newer_barrier_was_applied() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        // Reserve an ID, then model preemption before the producer registers it.
        let delayed = control.transport_receipts.next.fetch_add(1, SeqCst);
        let latest = control.submit_transport(TransportCommand::Stop).unwrap();
        render(&mut engine);
        control
            .transport_receipts
            .register(delayed, TransportCommand::Stop);
        control.transport.stop_requested.fetch_max(delayed, SeqCst);
        control.transport_receipts.stop_dirty.store(true, SeqCst);
        render(&mut engine);
        let result = control
            .transport_command_status(delayed)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.state, TransportCommandState::Canceled);
        assert_eq!(
            result.cancel_reason,
            Some(TransportCancelReason::Superseded)
        );
        assert!(!result.effect_applied);
        assert_eq!(control.transport_protocol_status().applied_stop_id, latest);
    }
    #[test]
    fn invalid_commands_do_not_receive_ids_and_exhaustion_never_wraps_or_defeats_safety_stop() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        for tempo in [f64::NAN, f64::INFINITY, 0.0, 401.0] {
            assert!(
                control
                    .submit_transport(TransportCommand::SetTempo(tempo))
                    .is_err()
            );
        }
        assert_eq!(control.transport_protocol_status().accepted_id, 0);
        control
            .submit_graph(EngineGraph::timeline(
                AudioGraph::new(AudioConfig::DEFAULT).unwrap(),
            ))
            .unwrap();
        control.submit_transport(TransportCommand::Play).unwrap();
        control
            .transport_receipts
            .next
            .store(MAX_PUBLICATION_REVISION, SeqCst);
        let last = control
            .submit_transport(TransportCommand::Seek(SamplePosition(42)))
            .unwrap();
        assert_eq!(last, MAX_PUBLICATION_REVISION);
        assert!(
            control
                .submit_transport(TransportCommand::Seek(SamplePosition(43)))
                .is_err()
        );
        assert!(control.submit_transport(TransportCommand::Stop).is_err());
        render(&mut engine);
        assert!(!control.transport().playing);
        assert_eq!(control.transport().current_sample, SamplePosition(42));
        assert_eq!(
            control.transport_receipts.next.load(SeqCst),
            MAX_PUBLICATION_REVISION + 1
        );
    }
    #[test]
    fn cancellation_and_application_have_a_single_linearization_winner() {
        use std::sync::Barrier;
        let registry = TransportReceipts::new();
        for _ in 0..64 {
            let id = registry
                .accept(TransportCommand::Seek(SamplePosition(3)))
                .unwrap();
            let barrier = Barrier::new(2);
            let canceled = std::thread::scope(|scope| {
                let cancel = scope.spawn(|| {
                    barrier.wait();
                    registry.cancel(id)
                });
                let apply = scope.spawn(|| {
                    barrier.wait();
                    let mut guard = registry.begin(id).unwrap();
                    if guard.result.state == TransportCommandState::CancelPending {
                        guard.result.state = TransportCommandState::Canceled;
                        guard.result.cancel_reason = Some(TransportCancelReason::Explicit);
                    } else {
                        guard.result.state = TransportCommandState::Applied;
                        guard.result.effect_applied = true;
                        guard.result.applied_sample = Some(SamplePosition(3));
                    }
                });
                let canceled = cancel.join().unwrap();
                apply.join().unwrap();
                canceled
            });
            let result = registry.status(id).result.unwrap();
            if matches!(canceled, Ok(true)) {
                assert_eq!(result.state, TransportCommandState::Canceled);
                assert!(!result.effect_applied);
            } else {
                assert_eq!(result.state, TransportCommandState::Applied);
                assert!(result.effect_applied);
            }
        }
    }
    #[test]
    fn zero_frame_callbacks_never_complete_a_playing_context_before_admission() {
        let (mut engine, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
        control
            .submit_graph(EngineGraph::timeline(
                AudioGraph::new(AudioConfig::DEFAULT).unwrap(),
            ))
            .unwrap();
        control.submit_transport(TransportCommand::Play).unwrap();
        let seek = control
            .submit_transport(TransportCommand::Seek(SamplePosition(10)))
            .unwrap();
        crate::tests::assert_no_realtime_memory(|| engine.process_block(&mut []));
        let result = control
            .transport_command_status(seek)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.state, TransportCommandState::Pending);
        assert!(result.effect_applied);
        assert!(result.resolved_sample.is_none());
        assert_eq!(control.transport().current_sample, SamplePosition(10));
        render(&mut engine);
        let result = control
            .transport_command_status(seek)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.state, TransportCommandState::Applied);
        assert_eq!(result.resolved_sample, Some(SamplePosition(10)));
        assert_eq!(control.transport().current_sample, SamplePosition(14));
    }
}
