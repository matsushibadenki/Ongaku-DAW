//! Fixed publication cancellation cells. Graph ownership stays in queues/retirement.
use crate::publication::GraphOutcome;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::SeqCst};
const FREE: u8 = 0;
const PENDING: u8 = 1;
const CLAIMED: u8 = 2;
const CANCELED: u8 = 3;
const SUPERSEDED: u8 = 4;
struct Cell {
    revision: AtomicU64,
    replaceable: AtomicBool,
    state: AtomicU8,
}
pub(super) struct PendingPublications([Cell; super::GRAPH_QUEUE_CAPACITY + 1]);
impl PendingPublications {
    pub fn new() -> Self {
        Self(std::array::from_fn(|_| Cell {
            revision: AtomicU64::new(0),
            replaceable: AtomicBool::new(false),
            state: AtomicU8::new(FREE),
        }))
    }
    // Called under the producer mutex; a cell is reusable only after callback settlement.
    pub fn reserve(&self, revision: u64, replaceable: bool) -> Option<usize> {
        let index = self
            .0
            .iter()
            .position(|cell| cell.state.load(SeqCst) == FREE)?;
        let cell = &self.0[index];
        cell.revision.store(revision, SeqCst);
        cell.replaceable.store(replaceable, SeqCst);
        cell.state.store(PENDING, SeqCst);
        Some(index)
    }
    pub fn finish(&self, index: usize) {
        self.0[index].state.store(FREE, SeqCst);
    }
    pub fn supersede_before(&self, revision: u64) {
        for cell in &self.0 {
            if cell.revision.load(SeqCst) < revision && cell.replaceable.load(SeqCst) {
                let _ = cell
                    .state
                    .compare_exchange(PENDING, SUPERSEDED, SeqCst, SeqCst);
            }
        }
    }
    pub fn cancel(&self, revision: u64) -> bool {
        self.0.iter().any(|cell| {
            cell.revision.load(SeqCst) == revision
                && cell
                    .state
                    .compare_exchange(PENDING, CANCELED, SeqCst, SeqCst)
                    .is_ok()
        })
    }
    pub fn claim(&self, index: usize) -> GraphOutcome {
        match self.0[index]
            .state
            .compare_exchange(PENDING, CLAIMED, SeqCst, SeqCst)
        {
            Ok(_) => GraphOutcome::Adopted,
            Err(SUPERSEDED) => GraphOutcome::Superseded,
            Err(_) => GraphOutcome::Canceled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    #[test]
    fn cancel_and_claim_have_exactly_one_winner() {
        for _ in 0..64 {
            let pending = Arc::new(PendingPublications::new());
            let slot = pending.reserve(1, true).unwrap();
            let gate = Arc::new(Barrier::new(2));
            let audio = Arc::clone(&pending);
            let audio_gate = Arc::clone(&gate);
            let thread = std::thread::spawn(move || {
                audio_gate.wait();
                audio.claim(slot)
            });
            gate.wait();
            let canceled = pending.cancel(1);
            let outcome = thread.join().unwrap();
            assert_eq!(
                outcome,
                if canceled {
                    GraphOutcome::Canceled
                } else {
                    GraphOutcome::Adopted
                }
            );
        }
    }
}
