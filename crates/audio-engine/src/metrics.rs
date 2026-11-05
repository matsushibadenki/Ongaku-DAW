use audio_backend::RealtimeEventSink;
use audio_types::RealtimeMetricsSnapshot;
use std::{
    array,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const HISTOGRAM_BUCKETS: usize = 32;

/// Fixed-memory realtime metrics. Buckets are powers of two in microseconds.
pub struct RealtimeMetrics {
    deadline_ns: u64,
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
}

impl RealtimeMetrics {
    #[must_use]
    pub fn new(deadline_ns: u64) -> Self {
        Self {
            deadline_ns,
            callback_count: AtomicU64::new(0),
            total_duration_ns: AtomicU64::new(0),
            maximum_duration_ns: AtomicU64::new(0),
            xrun_count: AtomicU64::new(0),
            backend_error_count: AtomicU64::new(0),
            histogram: array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub fn record(&self, duration: Duration) {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ns.fetch_add(nanos, Ordering::Relaxed);
        self.maximum_duration_ns.fetch_max(nanos, Ordering::Relaxed);
        if nanos > self.deadline_ns {
            self.xrun_count.fetch_add(1, Ordering::Relaxed);
        }
        let micros = nanos.div_ceil(1_000).max(1);
        let bucket = (u64::BITS - micros.leading_zeros()) as usize;
        self.histogram[bucket.min(HISTOGRAM_BUCKETS - 1)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_backend_error(&self) {
        self.backend_error_count.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn snapshot(&self) -> RealtimeMetricsSnapshot {
        let count = self.callback_count.load(Ordering::Relaxed);
        let total = self.total_duration_ns.load(Ordering::Relaxed);
        let maximum = self.maximum_duration_ns.load(Ordering::Relaxed);
        let deadline_us = self.deadline_ns as f64 / 1_000.0;
        let average_us = if count == 0 {
            0.0
        } else {
            total as f64 / count as f64 / 1_000.0
        };
        RealtimeMetricsSnapshot {
            callback_count: count,
            xrun_count: self.xrun_count.load(Ordering::Relaxed),
            backend_error_count: self.backend_error_count.load(Ordering::Relaxed),
            average_callback_us: average_us,
            maximum_callback_us: maximum as f64 / 1_000.0,
            p99_9_callback_us: self.percentile_us(count, 0.999),
            deadline_us,
            cpu_load: if deadline_us == 0.0 {
                0.0
            } else {
                average_us / deadline_us
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
        (1_u64 << (HISTOGRAM_BUCKETS - 1)) as f64
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
}
