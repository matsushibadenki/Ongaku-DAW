//! Prepared, persistent sample-position mixer events; no callback queue or allocation.
use audio_types::AudioError;
use daw_core::SamplePosition;
use serde::{Deserialize, Serialize};

pub const MAX_MIXER_EVENTS_PER_TRACK: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "parameter",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum MixerChange {
    Gain(f32),
    Pan(f32),
    Mute(bool),
    Solo(bool),
}

impl MixerChange {
    const fn lane(self) -> usize {
        match self {
            Self::Gain(_) => 0,
            Self::Pan(_) => 1,
            Self::Mute(_) => 2,
            Self::Solo(_) => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MixerEvent {
    pub id: u64,
    pub position: SamplePosition,
    pub change: MixerChange,
}

pub fn validate_mixer_events(events: &[MixerEvent]) -> Result<(), AudioError> {
    if events.len() > MAX_MIXER_EVENTS_PER_TRACK {
        return Err(AudioError::InvalidConfig("mixer event limit exceeded"));
    }
    let mut ids = std::collections::HashSet::with_capacity(events.len());
    for event in events {
        if !ids.insert(event.id)
            || !match event.change {
                MixerChange::Gain(value) => value.is_finite() && (0.0..=2.0).contains(&value),
                MixerChange::Pan(value) => value.is_finite() && (-1.0..=1.0).contains(&value),
                MixerChange::Mute(_) | MixerChange::Solo(_) => true,
            }
        {
            return Err(AudioError::InvalidConfig(
                "invalid or duplicate mixer event",
            ));
        }
    }
    Ok(())
}

pub(super) struct PreparedMixerEvents {
    lanes: [Vec<MixerEvent>; 4],
    definitions: Vec<MixerEvent>,
}
impl PreparedMixerEvents {
    pub const fn new() -> Self {
        Self {
            lanes: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            definitions: Vec::new(),
        }
    }
    pub fn prepare(events: &[MixerEvent]) -> Result<Self, AudioError> {
        validate_mixer_events(events)?;
        let mut result = Self::new();
        for &event in events {
            result.lanes[event.change.lane()].push(event);
        }
        for lane in &mut result.lanes {
            lane.sort_unstable_by_key(|event| (event.position, event.id));
        }
        result.definitions = events.to_vec();
        result.definitions.sort_unstable_by_key(|event| event.id);
        Ok(result)
    }
    pub fn deadline_vs(&self, previous: &Self) -> Option<SamplePosition> {
        let mut old = previous.definitions.iter().peekable();
        let mut new = self.definitions.iter().peekable();
        let mut deadline = None;
        let mut include = |position| {
            deadline =
                Some(deadline.map_or(position, |current: SamplePosition| current.min(position)));
        };
        while let (Some(a), Some(b)) = (old.peek(), new.peek()) {
            match a.id.cmp(&b.id) {
                std::cmp::Ordering::Less => {
                    include(a.position);
                    old.next();
                }
                std::cmp::Ordering::Greater => {
                    include(b.position);
                    new.next();
                }
                std::cmp::Ordering::Equal => {
                    if a != b {
                        include(a.position);
                        include(b.position);
                    }
                    old.next();
                    new.next();
                }
            }
        }
        for event in old.chain(new) {
            include(event.position);
        }
        deadline
    }
    pub fn first_position(&self) -> Option<SamplePosition> {
        self.definitions.iter().map(|event| event.position).min()
    }
    pub fn at(&self, position: SamplePosition, mut controls: Controls) -> Controls {
        for lane in &self.lanes {
            let end = lane.partition_point(|event| event.position <= position);
            if let Some(event) = end.checked_sub(1).map(|index| lane[index]) {
                match event.change {
                    MixerChange::Gain(value) => controls.gain = value,
                    MixerChange::Pan(value) => controls.pan = value,
                    MixerChange::Mute(value) => controls.mute = value,
                    MixerChange::Solo(value) => controls.solo = value,
                }
            }
        }
        controls
    }
}

#[derive(Clone, Copy)]
pub(super) struct Controls {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}
impl Controls {
    pub const UNITY: Self = Self {
        gain: 1.0,
        pan: 0.0,
        mute: false,
        solo: false,
    };
}
