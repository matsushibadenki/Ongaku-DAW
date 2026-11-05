//! Prepared, sample-position parameter automation independent from any UI shell.

use audio_types::AudioError;
use daw_core::SamplePosition;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationCurve {
    Step,
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationParameter {
    TrackGain,
    Pan,
    InstrumentLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationPoint {
    pub id: u64,
    pub position: SamplePosition,
    pub value: f32,
    pub curve: AutomationCurve,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationLane {
    pub id: u64,
    pub parameter: AutomationParameter,
    pub minimum: f32,
    pub maximum: f32,
    pub default_value: f32,
    points: Vec<AutomationPoint>,
}

impl AutomationLane {
    pub fn new(
        id: u64,
        parameter: AutomationParameter,
        minimum: f32,
        maximum: f32,
        default_value: f32,
        mut points: Vec<AutomationPoint>,
    ) -> Result<Self, AudioError> {
        if !minimum.is_finite() || !maximum.is_finite() || minimum >= maximum {
            return Err(AudioError::InvalidConfig("invalid automation range"));
        }
        points.sort_by_key(|point| point.position);
        if points
            .windows(2)
            .any(|pair| pair[0].position == pair[1].position)
        {
            return Err(AudioError::InvalidConfig(
                "automation point positions must be unique",
            ));
        }
        for point in &mut points {
            point.value = sanitize(point.value, minimum, maximum, default_value);
        }
        Ok(Self {
            id,
            parameter,
            minimum,
            maximum,
            default_value: sanitize(default_value, minimum, maximum, minimum),
            points,
        })
    }

    #[must_use]
    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }

    #[must_use]
    pub fn value_at(&self, position: SamplePosition) -> f32 {
        let next = self
            .points
            .partition_point(|point| point.position <= position);
        let Some(left) = next.checked_sub(1).and_then(|index| self.points.get(index)) else {
            return self.default_value;
        };
        let Some(right) = self.points.get(next) else {
            return left.value;
        };
        if left.curve == AutomationCurve::Step {
            return left.value;
        }
        let span = right.position.0.saturating_sub(left.position.0);
        if span == 0 {
            return right.value;
        }
        #[allow(clippy::cast_precision_loss)]
        let phase = position.0.saturating_sub(left.position.0) as f32 / span as f32;
        left.value + (right.value - left.value) * phase.clamp(0.0, 1.0)
    }

    pub fn set_point_value(&mut self, point_id: u64, value: f32) -> Result<(), AudioError> {
        let minimum = self.minimum;
        let maximum = self.maximum;
        let default_value = self.default_value;
        let point = self
            .points
            .iter_mut()
            .find(|point| point.id == point_id)
            .ok_or_else(|| {
                AudioError::Backend(format!("automation point {point_id} was not found"))
            })?;
        point.value = sanitize(value, minimum, maximum, default_value);
        Ok(())
    }
}

fn sanitize(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback.clamp(minimum, maximum)
    }
}

#[cfg(test)]
mod tests {
    use super::{AutomationCurve, AutomationLane, AutomationParameter, AutomationPoint};
    use daw_core::SamplePosition;

    fn point(id: u64, position: u64, value: f32, curve: AutomationCurve) -> AutomationPoint {
        AutomationPoint {
            id,
            position: SamplePosition(position),
            value,
            curve,
        }
    }

    #[test]
    fn evaluates_linear_and_step_segments_at_sample_positions() {
        let lane = AutomationLane::new(
            1,
            AutomationParameter::InstrumentLevel,
            0.0,
            1.0,
            0.25,
            vec![
                point(1, 10, 0.0, AutomationCurve::Linear),
                point(2, 20, 1.0, AutomationCurve::Step),
                point(3, 30, 0.5, AutomationCurve::Linear),
            ],
        )
        .unwrap();
        assert!((lane.value_at(SamplePosition(0)) - 0.25).abs() < f32::EPSILON);
        assert!((lane.value_at(SamplePosition(15)) - 0.5).abs() < f32::EPSILON);
        assert!((lane.value_at(SamplePosition(25)) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn edits_are_clamped_without_changing_point_order() {
        let mut lane = AutomationLane::new(
            1,
            AutomationParameter::TrackGain,
            0.0,
            2.0,
            1.0,
            vec![point(7, 20, 1.0, AutomationCurve::Linear)],
        )
        .unwrap();
        lane.set_point_value(7, 4.0).unwrap();
        assert!((lane.points()[0].value - 2.0).abs() < f32::EPSILON);
    }
}
