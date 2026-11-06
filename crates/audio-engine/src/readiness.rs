//! Last callback admission receipt. Fixed atomic storage; control readers retry boundedly.
use audio_types::{StreamReadiness, StreamReadinessState};
use daw_core::SamplePosition;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamReadinessSnapshot {
    pub transport_command_id: u64,
    pub graph_revision: u64,
    pub transport_generation: u64,
    pub position: SamplePosition,
    pub requested_frames: u64,
    pub rendered_frames: u64,
    pub waiting_callbacks: u64,
    pub readiness: StreamReadiness,
}
#[derive(Default)]
pub(super) struct ReadinessReceipt {
    initialized: AtomicBool,
    sequence: AtomicU64,
    command: AtomicU64,
    graph: AtomicU64,
    transport: AtomicU64,
    position: AtomicU64,
    frames: AtomicU64,
    rendered: AtomicU64,
    waits: AtomicU64,
    state: AtomicU64,
    required: AtomicU64,
    ready: AtomicU64,
}
impl ReadinessReceipt {
    pub fn publish(&self, snapshot: StreamReadinessSnapshot) {
        self.sequence.fetch_add(1, SeqCst);
        self.command.store(snapshot.transport_command_id, SeqCst);
        self.graph.store(snapshot.graph_revision, SeqCst);
        self.transport.store(snapshot.transport_generation, SeqCst);
        self.position.store(snapshot.position.0, SeqCst);
        self.frames.store(snapshot.requested_frames, SeqCst);
        self.rendered.store(snapshot.rendered_frames, SeqCst);
        self.waits.store(snapshot.waiting_callbacks, SeqCst);
        self.state.store(
            match snapshot.readiness.state {
                StreamReadinessState::Inactive => 0,
                StreamReadinessState::Ready => 1,
                StreamReadinessState::Waiting => 2,
                StreamReadinessState::Failed => 3,
                StreamReadinessState::CapacityExceeded => 4,
            },
            SeqCst,
        );
        self.required
            .store(snapshot.readiness.required_pages, SeqCst);
        self.ready.store(snapshot.readiness.ready_pages, SeqCst);
        self.initialized.store(true, SeqCst);
        self.sequence.fetch_add(1, SeqCst);
    }
    pub fn snapshot(&self) -> Option<StreamReadinessSnapshot> {
        for _ in 0..8 {
            let before = self.sequence.load(SeqCst);
            if !before.is_multiple_of(2) || !self.initialized.load(SeqCst) {
                continue;
            }
            let snapshot = StreamReadinessSnapshot {
                transport_command_id: self.command.load(SeqCst),
                graph_revision: self.graph.load(SeqCst),
                transport_generation: self.transport.load(SeqCst),
                position: SamplePosition(self.position.load(SeqCst)),
                requested_frames: self.frames.load(SeqCst),
                rendered_frames: self.rendered.load(SeqCst),
                waiting_callbacks: self.waits.load(SeqCst),
                readiness: StreamReadiness {
                    state: match self.state.load(SeqCst) {
                        1 => StreamReadinessState::Ready,
                        2 => StreamReadinessState::Waiting,
                        3 => StreamReadinessState::Failed,
                        4 => StreamReadinessState::CapacityExceeded,
                        _ => StreamReadinessState::Inactive,
                    },
                    required_pages: self.required.load(SeqCst),
                    ready_pages: self.ready.load(SeqCst),
                },
            };
            if self.sequence.load(SeqCst) == before {
                return Some(snapshot);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn concurrent_status_readers_never_accept_a_mixed_callback_receipt() {
        let receipt = Arc::new(ReadinessReceipt::default());
        assert!(receipt.snapshot().is_none());
        let writer = Arc::clone(&receipt);
        let worker = std::thread::spawn(move || {
            for generation in 1..10_000 {
                writer.publish(StreamReadinessSnapshot {
                    transport_command_id: generation,
                    graph_revision: generation,
                    transport_generation: generation * 2,
                    position: SamplePosition(generation * 3),
                    requested_frames: 4,
                    rendered_frames: 4,
                    waiting_callbacks: generation * 5,
                    readiness: StreamReadiness::ready(),
                });
            }
        });
        for _ in 0..10_000 {
            if let Some(status) = receipt.snapshot() {
                assert_eq!(status.transport_generation, status.graph_revision * 2);
                assert_eq!(status.position.0, status.graph_revision * 3);
                assert_eq!(status.waiting_callbacks, status.graph_revision * 5);
                assert_eq!(status.rendered_frames, 4);
            }
        }
        worker.join().unwrap();
        assert_eq!(receipt.snapshot().unwrap().graph_revision, 9999);
    }
}
