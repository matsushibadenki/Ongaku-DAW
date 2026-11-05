//! Bounded graph adoption receipts. Only fixed atomic storage is written by audio.
use daw_core::SamplePosition;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

pub const ADOPTION_HISTORY_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GraphOutcome {
    Adopted,
    Canceled,
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphAdoption {
    pub revision: u64,
    pub document_revision: u64,
    pub adopted_sample: Option<SamplePosition>,
    pub resolved_sample: SamplePosition,
    pub outcome: GraphOutcome,
    pub deadline: Option<SamplePosition>,
    pub late: bool,
    pub playing: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPublicationStatus {
    pub accepted_revision: u64,
    pub adopted_revision: u64,
    pub settled_revision: u64,
    pub canceled_count: u64,
    pub superseded_count: u64,
    pub pending: bool,
    pub late_count: u64,
    pub history_lost: bool,
    pub acknowledgements: Vec<GraphAdoption>,
}

struct Receipt {
    stamp: AtomicU64,
    document: AtomicU64,
    sample: AtomicU64,
    deadline: AtomicU64,
    flags: AtomicU64,
}
impl Receipt {
    fn new() -> Self {
        Self {
            stamp: AtomicU64::new(0),
            document: AtomicU64::new(0),
            sample: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
            flags: AtomicU64::new(0),
        }
    }
    fn write(&self, ack: GraphAdoption) {
        // All fields are atomic and sequentially consistent. A matching even stamp on
        // both sides of the reads proves no partial/overwritten receipt was accepted.
        self.stamp.store(ack.revision * 2 + 1, SeqCst);
        self.document.store(ack.document_revision, SeqCst);
        self.sample.store(ack.resolved_sample.0, SeqCst);
        self.deadline
            .store(ack.deadline.map_or(0, |position| position.0), SeqCst);
        self.flags.store(
            u64::from(ack.deadline.is_some())
                | (u64::from(ack.late) << 1)
                | (u64::from(ack.playing) << 2)
                | (match ack.outcome {
                    GraphOutcome::Adopted => 0,
                    GraphOutcome::Canceled => 1,
                    GraphOutcome::Superseded => 2,
                } << 3),
            SeqCst,
        );
        self.stamp.store(ack.revision * 2, SeqCst);
    }
    fn read(&self, revision: u64) -> Option<GraphAdoption> {
        if self.stamp.load(SeqCst) != revision * 2 {
            return None;
        }
        let document_revision = self.document.load(SeqCst);
        let adopted_sample = SamplePosition(self.sample.load(SeqCst));
        let deadline = SamplePosition(self.deadline.load(SeqCst));
        let flags = self.flags.load(SeqCst);
        if self.stamp.load(SeqCst) != revision * 2 {
            return None;
        }
        let outcome = match flags >> 3 {
            0 => GraphOutcome::Adopted,
            1 => GraphOutcome::Canceled,
            _ => GraphOutcome::Superseded,
        };
        Some(GraphAdoption {
            revision,
            document_revision,
            adopted_sample: (outcome == GraphOutcome::Adopted).then_some(adopted_sample),
            resolved_sample: adopted_sample,
            outcome,
            deadline: (flags & 1 != 0).then_some(deadline),
            late: flags & 2 != 0,
            playing: flags & 4 != 0,
        })
    }
}

pub(super) struct PublicationReceipts {
    pub accepted: AtomicU64,
    adopted: AtomicU64,
    settled: AtomicU64,
    canceled: AtomicU64,
    superseded: AtomicU64,
    late_count: AtomicU64,
    receipts: [Receipt; ADOPTION_HISTORY_CAPACITY],
}
impl PublicationReceipts {
    pub fn new() -> Self {
        Self {
            accepted: AtomicU64::new(0),
            adopted: AtomicU64::new(0),
            settled: AtomicU64::new(0),
            canceled: AtomicU64::new(0),
            superseded: AtomicU64::new(0),
            late_count: AtomicU64::new(0),
            receipts: std::array::from_fn(|_| Receipt::new()),
        }
    }
    fn slot(revision: u64) -> usize {
        usize::try_from(revision % 64).unwrap_or(0)
    }
    pub fn record(&self, ack: GraphAdoption) {
        self.receipts[Self::slot(ack.revision)].write(ack);
        if ack.late {
            self.late_count.fetch_add(1, SeqCst);
        }
        match ack.outcome {
            GraphOutcome::Adopted => self.adopted.store(ack.revision, SeqCst),
            GraphOutcome::Canceled => {
                self.canceled.fetch_add(1, SeqCst);
            }
            GraphOutcome::Superseded => {
                self.superseded.fetch_add(1, SeqCst);
            }
        }
        self.settled.store(ack.revision, SeqCst);
    }
    pub fn status(&self, after: u64) -> GraphPublicationStatus {
        let settled_revision = self.settled.load(SeqCst);
        let adopted_revision = self.adopted.load(SeqCst);
        // The adoption store follows the coherent receipt write. Include it even if the
        // writer has not yet published its settlement watermark.
        let settled_revision = settled_revision.max(adopted_revision);
        let accepted_revision = self
            .accepted
            .load(SeqCst)
            .max(settled_revision)
            .max(adopted_revision);
        let first = settled_revision.saturating_sub(63).max(1);
        let mut history_lost = after.saturating_add(1) < first;
        let mut acknowledgements = Vec::new();
        for revision in after.saturating_add(1).max(first)..=settled_revision {
            if let Some(ack) = self.receipts[Self::slot(revision)].read(revision) {
                acknowledgements.push(ack);
            } else {
                history_lost = true;
            }
        }
        GraphPublicationStatus {
            accepted_revision,
            adopted_revision,
            settled_revision,
            canceled_count: self.canceled.load(SeqCst),
            superseded_count: self.superseded.load(SeqCst),
            pending: accepted_revision > settled_revision,
            late_count: self.late_count.load(SeqCst),
            history_lost,
            acknowledgements,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_readers_never_accept_torn_or_overwritten_receipts() {
        let receipts = Arc::new(PublicationReceipts::new());
        let writer = Arc::clone(&receipts);
        let thread = std::thread::spawn(move || {
            for revision in 1..=10000 {
                let outcome = match revision % 3 {
                    0 => GraphOutcome::Adopted,
                    1 => GraphOutcome::Canceled,
                    _ => GraphOutcome::Superseded,
                };
                writer.record(GraphAdoption {
                    revision,
                    document_revision: revision * 3,
                    adopted_sample: (outcome == GraphOutcome::Adopted)
                        .then_some(SamplePosition(revision * 7)),
                    resolved_sample: SamplePosition(revision * 7),
                    outcome,
                    deadline: Some(SamplePosition(revision * 7)),
                    late: false,
                    playing: true,
                });
            }
        });
        while receipts.settled.load(SeqCst) < 10000 {
            let status = receipts.status(0);
            assert!(status.acknowledgements.len() <= ADOPTION_HISTORY_CAPACITY);
            assert!(status.adopted_revision <= status.settled_revision);
            assert!(status.settled_revision <= status.accepted_revision);
            for ack in status.acknowledgements {
                assert_eq!(ack.document_revision, ack.revision * 3);
                let expected = match ack.revision % 3 {
                    0 => GraphOutcome::Adopted,
                    1 => GraphOutcome::Canceled,
                    _ => GraphOutcome::Superseded,
                };
                assert_eq!(ack.outcome, expected);
                assert_eq!(ack.resolved_sample, SamplePosition(ack.revision * 7));
                assert_eq!(
                    ack.adopted_sample,
                    (expected == GraphOutcome::Adopted).then_some(ack.resolved_sample)
                );
                assert_eq!(ack.deadline, Some(ack.resolved_sample));
                assert!(!ack.late && ack.playing);
            }
        }
        thread.join().unwrap();
        let status = receipts.status(0);
        assert!(status.history_lost);
        assert_eq!(status.settled_revision, 10000);
        assert_eq!(status.adopted_revision, 9999);
        assert_eq!(status.canceled_count, 3334);
        assert_eq!(status.superseded_count, 3333);
        assert!(!status.pending);
        assert_eq!(status.acknowledgements.len(), 64);
        assert_eq!(status.acknowledgements[0].revision, 9937);
        assert!(receipts.status(10000).acknowledgements.is_empty());
    }
}
