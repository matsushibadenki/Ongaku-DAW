//! Cooperative control-thread graph preparation; never used by audio processing.
use crate::AudioError;
use std::{
    cmp::Ordering,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

pub trait PreparationCheck {
    fn checkpoint(&self) -> Result<(), AudioError>;
}

#[derive(Clone, Default)]
pub struct PreparationToken(Arc<AtomicBool>);
impl PreparationToken {
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::Release);
    }
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.0.load(AtomicOrdering::Acquire)
    }
}
impl PreparationCheck for PreparationToken {
    fn checkpoint(&self) -> Result<(), AudioError> {
        if self.is_canceled() {
            Err(AudioError::PreparationCanceled)
        } else {
            Ok(())
        }
    }
}

/// Stable sort with checkpoints between bounded 256-element sorts/merge batches.
/// A canceled buffer is partial preparation and must never be published.
pub fn preparation_sort_by<T: Copy>(
    values: &mut Vec<T>,
    mut compare: impl FnMut(&T, &T) -> Ordering,
    check: &impl PreparationCheck,
) -> Result<(), AudioError> {
    const BATCH: usize = 256;
    check.checkpoint()?;
    for chunk in values.chunks_mut(BATCH) {
        check.checkpoint()?;
        chunk.sort_by(&mut compare);
    }
    if values.len() <= BATCH {
        return check.checkpoint();
    }
    let mut scratch = Vec::with_capacity(values.len());
    for chunk in values.chunks(BATCH) {
        check.checkpoint()?;
        scratch.extend_from_slice(chunk);
    }
    let mut width = BATCH;
    while width < values.len() {
        for base in (0..values.len()).step_by(width.saturating_mul(2)) {
            let middle = base.saturating_add(width).min(values.len());
            let end = middle.saturating_add(width).min(values.len());
            let (mut left, mut right) = (base, middle);
            for (offset, slot) in scratch[base..end].iter_mut().enumerate() {
                if offset % BATCH == 0 {
                    check.checkpoint()?;
                }
                if left < middle
                    && (right == end || compare(&values[left], &values[right]) != Ordering::Greater)
                {
                    *slot = values[left];
                    left += 1;
                } else {
                    *slot = values[right];
                    right += 1;
                }
            }
        }
        std::mem::swap(values, &mut scratch);
        width = width.saturating_mul(2);
    }
    check.checkpoint()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Budget(Cell<usize>);
    impl PreparationCheck for Budget {
        fn checkpoint(&self) -> Result<(), AudioError> {
            let left = self.0.get();
            if left == 0 {
                Err(AudioError::PreparationCanceled)
            } else {
                self.0.set(left - 1);
                Ok(())
            }
        }
    }
    #[test]
    fn stable_checkpointed_sort_matches_the_serial_reference_and_can_stop_mid_merge() {
        let input: Vec<_> = (0..10000).map(|id| ((id * 17) % 31, id)).collect();
        let mut expected = input.clone();
        expected.sort_by_key(|item| item.0);
        let mut actual = input.clone();
        preparation_sort_by(
            &mut actual,
            |a, b| a.0.cmp(&b.0),
            &PreparationToken::default(),
        )
        .unwrap();
        assert_eq!(actual, expected);
        for budget in [0, 1, 45, 100, 150] {
            assert_eq!(
                preparation_sort_by(
                    &mut input.clone(),
                    |a, b| a.0.cmp(&b.0),
                    &Budget(Cell::new(budget))
                ),
                Err(AudioError::PreparationCanceled)
            );
        }
    }
}
