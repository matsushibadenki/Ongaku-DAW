//! Buffer admission result; ready means the complete requested window is retained.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamReadinessState {
    #[default]
    Inactive,
    Ready,
    Waiting,
    Failed,
    CapacityExceeded,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamReadiness {
    pub state: StreamReadinessState,
    pub required_pages: u64,
    pub ready_pages: u64,
}
impl StreamReadiness {
    #[must_use]
    pub const fn ready() -> Self {
        Self {
            state: StreamReadinessState::Ready,
            required_pages: 0,
            ready_pages: 0,
        }
    }
    /// Error states dominate waits; all contributors must be ready to admit a block.
    pub fn include(&mut self, other: Self) {
        let rank = |state| match state {
            StreamReadinessState::Inactive | StreamReadinessState::Ready => 0,
            StreamReadinessState::Waiting => 1,
            StreamReadinessState::CapacityExceeded => 2,
            StreamReadinessState::Failed => 3,
        };
        if rank(other.state) > rank(self.state) {
            self.state = other.state;
        }
        self.required_pages = self.required_pages.saturating_add(other.required_pages);
        self.ready_pages = self.ready_pages.saturating_add(other.ready_pages);
    }
}
