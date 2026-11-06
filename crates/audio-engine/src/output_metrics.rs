//! Fixed-storage output-boundary observations, separate from engine process timing.
use audio_types::{AudioConfig, OutputFault, OutputMetricsSnapshot};
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

pub(super) struct OutputMetrics {
    calls: AtomicU64,
    periods: AtomicU64,
    empty: AtomicU64,
    last: AtomicU64,
    last_valid: AtomicBool,
    min: AtomicU64,
    max: AtomicU64,
    mismatch: AtomicU64,
    driver: AtomicU64,
    adapter: AtomicU64,
    malformed: AtomicU64,
    capacity: AtomicU64,
    nonfinite: AtomicU64,
    misses: AtomicU64,
    total: AtomicU64,
    maximum: AtomicU64,
    budget: AtomicU64,
}
impl OutputMetrics {
    pub fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            periods: AtomicU64::new(0),
            empty: AtomicU64::new(0),
            last: AtomicU64::new(0),
            last_valid: AtomicBool::new(false),
            min: AtomicU64::new(u64::MAX),
            max: AtomicU64::new(0),
            mismatch: AtomicU64::new(0),
            driver: AtomicU64::new(0),
            adapter: AtomicU64::new(0),
            malformed: AtomicU64::new(0),
            capacity: AtomicU64::new(0),
            nonfinite: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            total: AtomicU64::new(0),
            maximum: AtomicU64::new(0),
            budget: AtomicU64::new(0),
        }
    }
    pub fn driver_error(&self) {
        self.driver.fetch_add(1, Ordering::Relaxed);
    }
    pub fn fault(&self, fault: OutputFault) {
        self.adapter.fetch_add(1, Ordering::Relaxed);
        match fault {
            OutputFault::MalformedFrame => &self.malformed,
            OutputFault::CapacityExceeded => &self.capacity,
            OutputFault::NonFiniteSample => &self.nonfinite,
        }
        .fetch_add(1, Ordering::Relaxed);
    }
    pub fn record(&self, config: AudioConfig, samples: usize, duration: Duration) {
        let aligned = config.buffer_size != 0
            && config.channels != 0
            && config.sample_rate != 0
            && samples.is_multiple_of(usize::from(config.channels));
        self.last_valid.store(false, Ordering::Release);
        if aligned {
            let frames = u64::try_from(samples / usize::from(config.channels)).unwrap_or(u64::MAX);
            self.last.store(frames, Ordering::Relaxed);
            self.last_valid.store(true, Ordering::Release);
            if frames == 0 {
                self.empty.fetch_add(1, Ordering::Relaxed);
            } else {
                self.min.fetch_min(frames, Ordering::Relaxed);
                self.max.fetch_max(frames, Ordering::Relaxed);
                if frames != u64::from(config.buffer_size) {
                    self.mismatch.fetch_add(1, Ordering::Relaxed);
                }
                let budget = u64::try_from(
                    u128::from(frames) * 1_000_000_000 / u128::from(config.sample_rate),
                )
                .unwrap_or(u64::MAX);
                let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
                if nanos > budget {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                }
                self.total.fetch_add(nanos, Ordering::Relaxed);
                self.budget.fetch_add(budget, Ordering::Relaxed);
                self.maximum.fetch_max(nanos, Ordering::Relaxed);
                self.periods.fetch_add(1, Ordering::Release);
            }
        }
        self.calls.fetch_add(1, Ordering::Release);
    }
    #[allow(clippy::cast_precision_loss)]
    pub fn snapshot(&self) -> OutputMetricsSnapshot {
        let periods = self.periods.load(Ordering::Acquire);
        let total = self.total.load(Ordering::Relaxed);
        let budget = self.budget.load(Ordering::Relaxed);
        OutputMetricsSnapshot {
            callback_count: self.calls.load(Ordering::Acquire),
            period_count: periods,
            empty_callback_count: self.empty.load(Ordering::Relaxed),
            last_frames: self
                .last_valid
                .load(Ordering::Acquire)
                .then(|| self.last.load(Ordering::Relaxed)),
            minimum_frames: (periods != 0).then(|| self.min.load(Ordering::Relaxed)),
            maximum_frames: (periods != 0).then(|| self.max.load(Ordering::Relaxed)),
            buffer_mismatch_count: self.mismatch.load(Ordering::Relaxed),
            driver_error_count: self.driver.load(Ordering::Relaxed),
            adapter_error_count: self.adapter.load(Ordering::Relaxed),
            malformed_frame_count: self.malformed.load(Ordering::Relaxed),
            capacity_exceeded_count: self.capacity.load(Ordering::Relaxed),
            non_finite_sample_count: self.nonfinite.load(Ordering::Relaxed),
            deadline_miss_count: self.misses.load(Ordering::Relaxed),
            average_callback_us: if periods == 0 {
                0.0
            } else {
                total as f64 / periods as f64 / 1000.0
            },
            maximum_callback_us: self.maximum.load(Ordering::Relaxed) as f64 / 1000.0,
            cpu_load: if budget == 0 {
                0.0
            } else {
                total as f64 / budget as f64
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::RealtimeMetrics;
    use audio_backend::RealtimeEventSink;
    use audio_types::{AudioConfig, OutputFault};
    use std::time::Duration;

    #[test]
    fn delivered_periods_mismatch_and_weighted_boundary_load_are_not_engine_timings() {
        let metrics = RealtimeMetrics::new(1_000_000);
        let config = AudioConfig::DEFAULT;
        crate::tests::assert_no_realtime_memory(|| {
            metrics.report_output_callback(config, 128, Duration::from_millis(2)); // 64 frames
            metrics.report_output_callback(config, 512, Duration::from_millis(2)); // 256 frames
            metrics.report_output_callback(config, 0, Duration::from_secs(10));
            metrics.report_output_callback(config, 3, Duration::from_secs(10));
        });
        let snapshot = metrics.snapshot();
        let output = snapshot.output;
        assert_eq!(snapshot.callback_count, 0);
        assert_eq!(snapshot.xrun_count, 0);
        assert_eq!(output.callback_count, 4);
        assert_eq!(output.period_count, 2);
        assert_eq!(output.empty_callback_count, 1);
        assert_eq!(output.last_frames, None);
        assert_eq!(output.minimum_frames, Some(64));
        assert_eq!(output.maximum_frames, Some(256));
        assert_eq!(output.buffer_mismatch_count, 2);
        assert_eq!(output.deadline_miss_count, 1);
        assert!((output.cpu_load - 0.6).abs() < 0.000_001);
        assert!((output.average_callback_us - 2000.0).abs() < f64::EPSILON);
        assert!((output.maximum_callback_us - 2000.0).abs() < f64::EPSILON);
    }
    #[test]
    fn driver_adapter_and_process_overruns_are_separate_with_a_legacy_aggregate() {
        let metrics = RealtimeMetrics::new(1_000_000);
        crate::tests::assert_no_realtime_memory(|| {
            metrics.report_backend_error();
            metrics.report_output_fault(OutputFault::MalformedFrame);
            metrics.report_output_fault(OutputFault::CapacityExceeded);
            metrics.report_output_fault(OutputFault::NonFiniteSample);
            metrics.record(Duration::from_millis(2));
        });
        let snapshot = metrics.snapshot();
        let output = snapshot.output;
        assert_eq!(snapshot.backend_error_count, 4);
        assert_eq!(snapshot.process_deadline_miss_count, snapshot.xrun_count);
        assert_eq!(snapshot.xrun_count, 1);
        assert_eq!(output.driver_error_count, 1);
        assert_eq!(output.adapter_error_count, 3);
        assert_eq!(output.malformed_frame_count, 1);
        assert_eq!(output.capacity_exceeded_count, 1);
        assert_eq!(output.non_finite_sample_count, 1);
        assert_eq!(output.deadline_miss_count, 0);
        assert_eq!(output.callback_count, 0);
    }
    #[test]
    fn unknown_invalid_empty_and_matching_deliveries_do_not_fabricate_periods() {
        let metrics = RealtimeMetrics::new(1_000_000);
        let empty = metrics.snapshot().output;
        assert_eq!(empty.minimum_frames, None);
        assert_eq!(empty.last_frames, None);
        metrics.report_output_callback(
            AudioConfig {
                channels: 0,
                ..AudioConfig::DEFAULT
            },
            128,
            Duration::from_millis(1),
        );
        metrics.report_output_callback(
            AudioConfig {
                sample_rate: 0,
                ..AudioConfig::DEFAULT
            },
            128,
            Duration::from_millis(1),
        );
        metrics.report_output_callback(AudioConfig::DEFAULT, 0, Duration::ZERO);
        let output = metrics.snapshot().output;
        assert_eq!(output.period_count, 0);
        assert_eq!(output.minimum_frames, None);
        assert_eq!(output.last_frames, Some(0));
        assert_eq!(output.buffer_mismatch_count, 0);
        metrics.report_output_callback(AudioConfig::DEFAULT, 256, Duration::from_micros(1));
        let output = metrics.snapshot().output;
        assert_eq!(output.last_frames, Some(128));
        assert_eq!(output.minimum_frames, Some(128));
        assert_eq!(output.maximum_frames, Some(128));
        assert_eq!(output.buffer_mismatch_count, 0);
        assert_eq!(output.period_count, 1);
    }
}
