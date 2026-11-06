use crate::output_metrics::OutputMetrics;
use audio_backend::RealtimeEventSink;
use audio_types::{AudioConfig, OutputFault, RealtimeMetricsSnapshot};
use std::{
    array,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const HISTOGRAM_BUCKETS: usize = 64;

/// Fixed-memory realtime metrics. Buckets are powers of two in microseconds.
pub struct RealtimeMetrics {
    output: OutputMetrics,
    default_deadline_ns: u64,
    deadline_ns: AtomicU64,
    total_budget_ns: AtomicU64,
    callback_count: AtomicU64,
    total_duration_ns: AtomicU64,
    maximum_duration_ns: AtomicU64,
    xrun_count: AtomicU64,
    backend_error_count: AtomicU64,
    histogram: [AtomicU64; HISTOGRAM_BUCKETS],
}

impl RealtimeEventSink for RealtimeMetrics {
    fn report_backend_error(&self) {
        self.record_backend_error();
    }
    fn report_output_fault(&self, fault: OutputFault) {
        self.backend_error_count.fetch_add(1, Ordering::Relaxed);
        self.output.fault(fault);
    }
    fn report_output_callback(&self, config: AudioConfig, samples: usize, duration: Duration) {
        self.output.record(config, samples, duration);
    }
}

impl RealtimeMetrics {
    #[must_use]
    pub fn new(deadline_ns: u64) -> Self {
        Self {
            output: OutputMetrics::new(),
            default_deadline_ns: deadline_ns,
            deadline_ns: AtomicU64::new(deadline_ns),
            total_budget_ns: AtomicU64::new(0),
            callback_count: AtomicU64::new(0),
            total_duration_ns: AtomicU64::new(0),
            maximum_duration_ns: AtomicU64::new(0),
            xrun_count: AtomicU64::new(0),
            backend_error_count: AtomicU64::new(0),
            histogram: array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Records against the configured period for callers with a fixed buffer size.
    pub fn record(&self, duration: Duration) {
        self.record_period(duration, self.default_deadline_ns);
    }

    /// Empty/invalid callbacks have no audio period and are excluded from timing statistics.
    pub fn record_callback(&self, duration: Duration, frames: usize, sample_rate: u32) {
        if frames == 0 || sample_rate == 0 {
            return;
        }
        let deadline = u128::try_from(frames)
            .unwrap_or(u128::MAX)
            .saturating_mul(1_000_000_000)
            / u128::from(sample_rate);
        self.record_period(duration, u64::try_from(deadline).unwrap_or(u64::MAX));
    }

    fn record_period(&self, duration: Duration, deadline_ns: u64) {
        if deadline_ns == 0 {
            return;
        }
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        self.total_duration_ns.fetch_add(nanos, Ordering::Relaxed);
        self.total_budget_ns
            .fetch_add(deadline_ns, Ordering::Relaxed);
        self.deadline_ns.store(deadline_ns, Ordering::Relaxed);
        self.maximum_duration_ns.fetch_max(nanos, Ordering::Relaxed);
        if nanos > deadline_ns {
            self.xrun_count.fetch_add(1, Ordering::Relaxed);
        }
        let micros = nanos.div_ceil(1000).max(1);
        // Upper power-of-two bound: exact powers must not jump into the next bucket.
        let bucket = if micros <= 1 {
            0
        } else {
            (u64::BITS - (micros - 1).leading_zeros()) as usize
        };
        self.histogram[bucket].fetch_add(1, Ordering::Relaxed);
        self.callback_count.fetch_add(1, Ordering::Release);
    }

    pub fn record_backend_error(&self) {
        self.output.driver_error();
        self.backend_error_count.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn snapshot(&self) -> RealtimeMetricsSnapshot {
        let count = self.callback_count.load(Ordering::Acquire);
        let total = self.total_duration_ns.load(Ordering::Relaxed);
        let maximum = self.maximum_duration_ns.load(Ordering::Relaxed);
        let deadline_us = self.deadline_ns.load(Ordering::Relaxed) as f64 / 1_000.0;
        let total_budget = self.total_budget_ns.load(Ordering::Relaxed);
        let average_us = if count == 0 {
            0.0
        } else {
            total as f64 / count as f64 / 1_000.0
        };
        let process_misses = self.xrun_count.load(Ordering::Relaxed);
        RealtimeMetricsSnapshot {
            output: self.output.snapshot(),
            callback_count: count,
            xrun_count: process_misses,
            process_deadline_miss_count: process_misses,
            backend_error_count: self.backend_error_count.load(Ordering::Relaxed),
            average_callback_us: average_us,
            maximum_callback_us: maximum as f64 / 1_000.0,
            p99_9_callback_us: self.percentile_us(count, 0.999),
            deadline_us,
            cpu_load: if total_budget == 0 {
                0.0
            } else {
                total as f64 / total_budget as f64
            },
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn percentile_us(&self, count: u64, percentile: f64) -> f64 {
        if count == 0 {
            return 0.0;
        }
        let target = (count as f64 * percentile).ceil() as u64;
        let mut cumulative = 0;
        for (index, bucket) in self.histogram.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target {
                return (1_u64 << index) as f64;
            }
        }
        self.maximum_duration_ns
            .load(Ordering::Relaxed)
            .div_ceil(1000) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::RealtimeMetrics;
    use std::time::Duration;

    #[test]
    fn tracks_worst_case_and_deadline_misses() {
        let metrics = RealtimeMetrics::new(1_000_000);
        metrics.record(Duration::from_micros(100));
        metrics.record(Duration::from_millis(2));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.callback_count, 2);
        assert_eq!(snapshot.xrun_count, 1);
        assert!((snapshot.maximum_callback_us - 2_000.0).abs() < f64::EPSILON);
    }
    #[test]
    fn variable_period_deadlines_and_weighted_load_are_accounted_separately() {
        let metrics = RealtimeMetrics::new(2_666_666);
        metrics.record_callback(Duration::from_millis(2), 64, 48000);
        metrics.record_callback(Duration::from_millis(2), 256, 48000);
        metrics.record_callback(Duration::from_secs(10), 0, 48000);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.callback_count, 2);
        assert_eq!(snapshot.xrun_count, 1);
        assert!((snapshot.deadline_us - 5333.333).abs() < 0.01);
        assert!((snapshot.cpu_load - 0.6).abs() < 0.000_001);
        assert!((snapshot.p99_9_callback_us - 2048.0).abs() < f64::EPSILON);
    }
    #[test]
    fn histogram_upper_bounds_handle_exact_powers_and_long_callbacks() {
        let metrics = RealtimeMetrics::new(100_000);
        metrics.record(Duration::from_micros(8));
        assert!((metrics.snapshot().p99_9_callback_us - 8.0).abs() < f64::EPSILON);
        metrics.record(Duration::from_micros(9));
        assert!((metrics.snapshot().p99_9_callback_us - 16.0).abs() < f64::EPSILON);
        metrics.record(Duration::from_secs(10000));
        assert!(metrics.snapshot().p99_9_callback_us >= 10_000_000_000.0);
    }
}
