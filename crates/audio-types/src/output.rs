//! Advertised/validated output selection, not a claim about measured driver periods.
use crate::AudioConfig;
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputSampleFormat {
    F32,
    I16,
    U16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct OutputBufferRange {
    pub min: u32,
    pub max: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputConfiguration {
    pub device_id: String,
    pub requested: AudioConfig,
    pub validated: AudioConfig,
    pub sample_format: OutputSampleFormat,
    /// None means the driver did not advertise bounds. Actual callback sizes may vary.
    pub advertised_buffer_range: Option<OutputBufferRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFault {
    MalformedFrame,
    CapacityExceeded,
    NonFiniteSample,
}

/// Observations of delivered backend buffers, not physical latency or driver xruns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputMetricsSnapshot {
    pub callback_count: u64,
    pub period_count: u64,
    pub empty_callback_count: u64,
    pub last_frames: Option<u64>,
    pub minimum_frames: Option<u64>,
    pub maximum_frames: Option<u64>,
    pub buffer_mismatch_count: u64,
    pub driver_error_count: u64,
    pub adapter_error_count: u64,
    pub malformed_frame_count: u64,
    pub capacity_exceeded_count: u64,
    pub non_finite_sample_count: u64,
    pub deadline_miss_count: u64,
    pub average_callback_us: f64,
    pub maximum_callback_us: f64,
    pub cpu_load: f64,
}
