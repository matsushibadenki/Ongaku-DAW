//! Sample-accurate domain types shared by control and realtime layers.

pub mod transport;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SamplePosition(pub u64);

impl SamplePosition {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn saturating_add(self, frames: u64) -> Self {
        Self(self.0.saturating_add(frames))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRange {
    pub start: SamplePosition,
    pub end: SamplePosition,
}

impl SampleRange {
    #[must_use]
    pub fn new(start: SamplePosition, end: SamplePosition) -> Option<Self> {
        (end > start).then_some(Self { start, end })
    }

    #[must_use]
    pub const fn length(self) -> u64 {
        self.end.0 - self.start.0
    }

    #[must_use]
    pub const fn contains(self, position: SamplePosition) -> bool {
        position.0 >= self.start.0 && position.0 < self.end.0
    }
}
