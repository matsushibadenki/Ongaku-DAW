//! Position-addressed, immutable-media cache. All reader I/O stays off audio.
use audio_file::AudioFrameReader;
use audio_types::{AudioError, PreparationCheck};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

mod window;
pub use window::StreamWindow;

const EMPTY: u64 = u64::MAX;
struct Page {
    users: AtomicU32,
    version: AtomicU64,
    tag: AtomicU64,
    samples: Box<[AtomicU32]>,
}
struct Shared {
    pages: Vec<Page>,
    requests: Vec<AtomicU64>,
    demand: AtomicU64,
    stop: AtomicBool,
    failed: AtomicBool,
    misses: AtomicU64,
    reads: AtomicU64,
    errors: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineStreamSnapshot {
    pub underrun_frames: u64,
    pub page_reads: u64,
    pub worker_errors: u64,
    pub worker_failed: bool,
}

/// Shared by immutable document/history media and prepared graphs. Drop off audio only.
/// Absolute frame addressing makes obsolete seek/loop data unobservable; no read cursor is inherited.
pub struct TimelineStream {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    rate: u32,
    channels: u16,
    frames: u64,
    page_frames: usize,
}

impl TimelineStream {
    pub fn prepare(
        mut reader: Box<dyn AudioFrameReader>,
        frames: u64,
        page_frames: usize,
        page_count: usize,
        check: &impl PreparationCheck,
    ) -> Result<Arc<Self>, AudioError> {
        let rate = reader.sample_rate();
        let channels = reader.channels();
        if rate == 0
            || !(1..=2).contains(&channels)
            || frames == 0
            || !(1..=4096).contains(&page_frames)
            || !(2..=32).contains(&page_count)
            || page_frames
                .checked_mul(page_count)
                .is_none_or(|n| n as u64 > u64::from(rate) * 2)
        {
            return Err(AudioError::InvalidConfig(
                "invalid timeline stream cache bounds",
            ));
        }
        check.checkpoint()?;
        let samples = page_frames * usize::from(channels);
        let shared = Arc::new(Shared {
            pages: (0..page_count)
                .map(|_| Page {
                    users: AtomicU32::new(0),
                    version: AtomicU64::new(0),
                    tag: AtomicU64::new(EMPTY),
                    samples: (0..samples).map(|_| AtomicU32::new(0)).collect(),
                })
                .collect(),
            requests: (0..page_count).map(|_| AtomicU64::new(EMPTY)).collect(),
            demand: AtomicU64::new(EMPTY),
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            misses: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        });
        let mut scratch = vec![0.0; samples];
        // Warm the initial bounded window before publication. Cancellation never publishes a partial cache.
        for (slot, page) in (0..frames.div_ceil(page_frames as u64))
            .take(page_count)
            .enumerate()
        {
            check.checkpoint()?;
            load_page(
                &mut *reader,
                &shared,
                &mut scratch,
                page,
                slot,
                page_frames,
                frames,
                channels,
                check,
            )?;
        }
        check.checkpoint()?;
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("ongaku-timeline-media".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_timeline_worker(
                        &mut *reader,
                        &worker_shared,
                        &mut scratch,
                        page_frames,
                        frames,
                        channels,
                    );
                }))
                .is_err()
                {
                    worker_shared.errors.fetch_add(1, Ordering::Relaxed);
                    worker_shared.failed.store(true, Ordering::Release);
                }
            })
            .map_err(|e| AudioError::Backend(format!("timeline worker: {e}")))?;
        Ok(Arc::new(Self {
            shared,
            worker: Some(worker),
            rate,
            channels,
            frames,
            page_frames,
        }))
    }

    /// The media owner remains alive while callback-side leases are released.
    #[must_use]
    pub fn window(self: &Arc<Self>) -> StreamWindow {
        StreamWindow::new(Arc::clone(self))
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.rate
    }
    #[must_use]
    pub const fn frame_count(&self) -> u64 {
        self.frames
    }

    /// Cancel unstarted hints/demands on a discontinuous target. Single native
    /// engine ownership is required; an already executing reader may finish.
    pub fn cancel_requests(&self) {
        for request in &self.shared.requests {
            request.store(EMPTY, Ordering::Release);
        }
        self.shared.demand.store(EMPTY, Ordering::Release);
    }

    /// Bounded, allocation-free prefetch hint; a full mailbox never blocks audio.
    pub fn request_frame(&self, frame: u64) {
        if frame >= self.frames || self.shared.failed.load(Ordering::Acquire) {
            return;
        }
        let page = frame / self.page_frames as u64;
        if cached(&self.shared, page) {
            return;
        }
        for request in &self.shared.requests {
            let value = request.load(Ordering::Acquire);
            if value == page {
                return;
            }
            if value == EMPTY
                && request
                    .compare_exchange(EMPTY, page, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                return;
            }
        }
    }

    /// Copies a coherent frame, duplicating mono to output channels. Misses produce silence.
    /// Sequentially consistent atomic payload/tag accesses prevent mixed frames during eviction.
    pub fn read_frame(&self, frame: u64, output: &mut [f32]) -> bool {
        output.fill(0.0);
        if frame >= self.frames {
            return true;
        }
        if self.shared.failed.load(Ordering::Acquire) {
            return false;
        }
        let page_id = frame / self.page_frames as u64;
        let local = usize::try_from(frame % self.page_frames as u64).unwrap_or(0)
            * usize::from(self.channels);
        for page in &self.shared.pages {
            let version = page.version.load(Ordering::SeqCst);
            if version % 2 != 0 {
                continue;
            }
            if page.tag.load(Ordering::SeqCst) != page_id {
                continue;
            }
            let left = f32::from_bits(page.samples[local].load(Ordering::SeqCst));
            let right = f32::from_bits(
                page.samples[local + usize::from(self.channels) - 1].load(Ordering::SeqCst),
            );
            if page.version.load(Ordering::SeqCst) != version {
                break;
            }
            for (channel, value) in output.iter_mut().enumerate() {
                *value = if channel == 0 { left } else { right };
            }
            // Look ahead without changing the timeline cursor, including after a cache hit.
            if frame % self.page_frames as u64 >= self.page_frames as u64 / 2 {
                self.request_frame((page_id + 1).saturating_mul(self.page_frames as u64));
            }
            return true;
        }
        self.shared.misses.fetch_add(1, Ordering::Relaxed);
        self.shared.demand.store(page_id, Ordering::Release);
        false
    }

    #[must_use]
    pub fn snapshot(&self) -> TimelineStreamSnapshot {
        TimelineStreamSnapshot {
            underrun_frames: self.shared.misses.load(Ordering::Relaxed),
            page_reads: self.shared.reads.load(Ordering::Relaxed),
            worker_errors: self.shared.errors.load(Ordering::Relaxed),
            worker_failed: self.shared.failed.load(Ordering::Acquire),
        }
    }
}

impl PreparationCheck for Shared {
    fn checkpoint(&self) -> Result<(), AudioError> {
        if self.stop.load(Ordering::Acquire) {
            Err(AudioError::PreparationCanceled)
        } else {
            Ok(())
        }
    }
}
fn run_timeline_worker(
    reader: &mut dyn AudioFrameReader,
    shared: &Shared,
    scratch: &mut [f32],
    page_frames: usize,
    frames: u64,
    channels: u16,
) {
    let mut victim = 0;
    while !shared.stop.load(Ordering::Acquire) {
        // The current missing position supersedes older demands. Hints never
        // force a new seek to wait behind a full mailbox of obsolete positions.
        let demand = shared.demand.swap(EMPTY, Ordering::AcqRel);
        let page = if demand == EMPTY {
            shared
                .requests
                .iter()
                .find_map(|request| {
                    let value = request.swap(EMPTY, Ordering::AcqRel);
                    (value != EMPTY).then_some(value)
                })
                .unwrap_or(EMPTY)
        } else {
            demand
        };
        if page == EMPTY {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        if cached(shared, page) {
            continue;
        }
        let mut result = load_page(
            reader,
            shared,
            scratch,
            page,
            victim,
            page_frames,
            frames,
            channels,
            shared,
        );
        while matches!(result, Err(AudioError::PublicationBusy))
            && !shared.stop.load(Ordering::Acquire)
        {
            // Leases are held only by currently admitted/waiting windows. Cancellation
            // frees them; retain the decoded page while every cache slot is leased.
            let demand = shared.demand.load(Ordering::Acquire);
            if demand != EMPTY && demand != page {
                break;
            }
            thread::sleep(Duration::from_millis(1));
            result = publish_page(shared, scratch, page, victim);
        }
        if matches!(result, Err(AudioError::PublicationBusy)) {
            continue;
        }
        if result.is_err() {
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            shared.errors.fetch_add(1, Ordering::Relaxed);
            shared.failed.store(true, Ordering::Release);
            return;
        }
        victim = (victim + 1) % shared.pages.len();
    }
}

impl Drop for TimelineStream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn cached(shared: &Shared, page: u64) -> bool {
    shared
        .pages
        .iter()
        .any(|p| p.tag.load(Ordering::SeqCst) == page)
}
#[allow(clippy::too_many_arguments)]
fn load_page(
    reader: &mut dyn AudioFrameReader,
    shared: &Shared,
    scratch: &mut [f32],
    page: u64,
    slot: usize,
    page_frames: usize,
    frames: u64,
    channels: u16,
    check: &impl PreparationCheck,
) -> Result<(), AudioError> {
    let start = page
        .checked_mul(page_frames as u64)
        .filter(|start| *start < frames)
        .ok_or(AudioError::InvalidState("stream page outside media"))?;
    check.checkpoint()?;
    reader.seek(start)?;
    let count = usize::try_from((frames - start).min(page_frames as u64)).unwrap_or(0)
        * usize::from(channels);
    scratch.fill(0.0);
    let mut filled = 0;
    while filled < count {
        check.checkpoint()?;
        let read = reader.read_interleaved(&mut scratch[filled..count])?;
        if read == 0 || read > count - filled {
            return Err(AudioError::InvalidState("incomplete stream media page"));
        }
        filled += read;
    }
    check.checkpoint()?;
    publish_page(shared, scratch, page, slot)
}
fn publish_page(
    shared: &Shared,
    scratch: &[f32],
    page: u64,
    slot: usize,
) -> Result<(), AudioError> {
    let Some(destination) = (0..shared.pages.len())
        .map(|offset| &shared.pages[(slot + offset) % shared.pages.len()])
        .find(|page| {
            page.users
                .compare_exchange(0, u32::MAX, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        })
    else {
        return Err(AudioError::PublicationBusy);
    };
    let version = destination.version.load(Ordering::SeqCst);
    let Some(next) = version.checked_add(2) else {
        destination.users.store(0, Ordering::SeqCst);
        return Err(AudioError::InvalidState("stream page versions exhausted"));
    };
    // A tag alone permits ABA when the same slot is evicted and reloaded while
    // a reader is preempted. The non-wrapping version covers the entire payload.
    destination.version.store(version + 1, Ordering::SeqCst);
    destination.tag.store(EMPTY, Ordering::SeqCst);
    for (value, sample) in destination.samples.iter().zip(scratch.iter()) {
        value.store(
            if sample.is_finite() {
                sample.to_bits()
            } else {
                0.0_f32.to_bits()
            },
            Ordering::SeqCst,
        );
    }
    destination.tag.store(page, Ordering::SeqCst);
    destination.version.store(next, Ordering::SeqCst);
    destination.users.store(0, Ordering::SeqCst);
    shared.reads.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_types::PreparationToken;
    use std::sync::{Condvar, Mutex, mpsc};
    use std::time::Instant;
    struct Reader {
        position: usize,
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
        started: Option<mpsc::SyncSender<()>>,
        fail: bool,
        seeks: Arc<Mutex<Vec<u64>>>,
    }
    impl AudioFrameReader for Reader {
        fn sample_rate(&self) -> u32 {
            8000
        }
        fn channels(&self) -> u16 {
            2
        }
        fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
            self.seeks.lock().unwrap().push(frame);
            self.position = usize::try_from(frame).unwrap() * 2;
            if frame >= 8 {
                if let Some(sender) = self.started.take() {
                    sender.send(()).unwrap();
                }
                if let Some(gate) = self.gate.take() {
                    let (open, result) = gate
                        .1
                        .wait_timeout_while(
                            gate.0.lock().unwrap(),
                            Duration::from_secs(5),
                            |open| !*open,
                        )
                        .unwrap();
                    assert!(*open && !result.timed_out(), "test gate timed out");
                }
                if self.fail {
                    return Err(AudioError::InvalidState("injected storage failure"));
                }
            }
            Ok(())
        }
        fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError> {
            // Short, non-frame-aligned reads deliberately exercise page assembly.
            let count = output.len().min(3);
            for sample in &mut output[..count] {
                let frame = self.position / 2;
                let value = f32::from(u16::try_from(frame).unwrap() + 1) / 1024.0;
                *sample = if self.position.is_multiple_of(2) {
                    value
                } else {
                    -value
                };
                self.position += 1;
            }
            Ok(count)
        }
    }
    fn reader() -> Reader {
        Reader {
            position: 0,
            gate: None,
            started: None,
            fail: false,
            seeks: Arc::default(),
        }
    }
    fn wait(mut test: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !test() {
            assert!(Instant::now() < until);
            thread::yield_now();
        }
    }
    fn exact(media: &TimelineStream, frame: u64) -> bool {
        let mut out = [1.0; 2];
        let ready = media.read_frame(frame, &mut out);
        if ready {
            let value = f32::from(u16::try_from(frame).unwrap() + 1) / 1024.0;
            assert_eq!(out.map(f32::to_bits), [value, -value].map(f32::to_bits));
        } else {
            assert_eq!(out.map(f32::to_bits), [0.0; 2].map(f32::to_bits));
        }
        ready
    }
    #[test]
    fn stalled_old_position_never_replays_after_a_new_seek() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (started, receive) = mpsc::sync_channel(1);
        let mut reader = reader();
        let seeks = Arc::clone(&reader.seeks);
        reader.gate = Some(Arc::clone(&gate));
        reader.started = Some(started);
        let media =
            TimelineStream::prepare(Box::new(reader), 100, 4, 2, &PreparationToken::default())
                .unwrap();
        assert!(exact(&media, 1));
        assert!(!exact(&media, 40));
        receive.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!exact(&media, 60));
        assert!(!exact(&media, 80)); // New seek while old disk work is blocked.
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        wait(|| exact(&media, 80));
        // Later callbacks use their current position, never the late frame 40.
        wait(|| exact(&media, 81));
        assert!(media.snapshot().underrun_frames >= 3);
        assert!(
            !seeks.lock().unwrap().contains(&60),
            "obsolete pending demand must coalesce"
        );
    }
    #[test]
    fn concurrent_eviction_returns_exact_stereo_frames_or_silence() {
        let media =
            TimelineStream::prepare(Box::new(reader()), 200, 4, 2, &PreparationToken::default())
                .unwrap();
        for round in 0..2000_u64 {
            let frame = round * 17 % 200;
            exact(&media, frame);
            if round.is_multiple_of(16) {
                thread::yield_now();
            }
        }
        wait(|| exact(&media, 180));
        assert!(media.snapshot().page_reads > 2);
    }
    #[test]
    fn runtime_storage_failure_is_explicit_and_silences_cached_pages_too() {
        let mut reader = reader();
        reader.fail = true;
        let media =
            TimelineStream::prepare(Box::new(reader), 100, 4, 2, &PreparationToken::default())
                .unwrap();
        media.request_frame(40);
        wait(|| media.snapshot().worker_failed);
        assert_eq!(media.snapshot().worker_errors, 1);
        assert!(!exact(&media, 0));
    }
    #[test]
    fn canceled_preparation_has_no_published_media() {
        let token = PreparationToken::default();
        token.cancel();
        assert!(matches!(
            TimelineStream::prepare(Box::new(reader()), 100, 4, 2, &token),
            Err(AudioError::PreparationCanceled)
        ));
    }
    #[test]
    fn readiness_leases_prevent_eviction_and_release_without_extra_decode() {
        use audio_types::StreamReadinessState;
        let reader = reader();
        let seeks = Arc::clone(&reader.seeks);
        let media =
            TimelineStream::prepare(Box::new(reader), 200, 4, 2, &PreparationToken::default())
                .unwrap();
        let mut window = media.window();
        window.require_frame(0);
        window.require_frame(7);
        assert_eq!(window.poll().state, StreamReadinessState::Ready);
        media.request_frame(40);
        wait(|| seeks.lock().unwrap().contains(&40));
        let mut output = [0.0; 2];
        for _ in 0..100 {
            assert!(window.read_frame(7, &mut output));
            assert_eq!(
                output.map(f32::to_bits),
                [8.0_f32 / 1024.0, -8.0_f32 / 1024.0].map(f32::to_bits)
            );
        }
        assert_eq!(media.snapshot().page_reads, 2); // Both slots remain retained.
        drop(window);
        wait(|| exact(&media, 40));
        assert_eq!(
            seeks
                .lock()
                .unwrap()
                .iter()
                .filter(|frame| **frame == 40)
                .count(),
            1
        );
    }
    #[test]
    fn partial_readiness_retains_ready_pages_until_the_whole_window_is_available() {
        use audio_types::StreamReadinessState;
        let media =
            TimelineStream::prepare(Box::new(reader()), 200, 4, 2, &PreparationToken::default())
                .unwrap();
        let mut window = media.window();
        window.require_frame(0);
        window.require_frame(8);
        let first = window.poll();
        assert_eq!(first.state, StreamReadinessState::Waiting);
        assert_eq!(first.ready_pages, 1);
        wait(|| window.poll().state == StreamReadinessState::Ready);
        let mut output = [0.0; 2];
        assert!(window.read_frame(0, &mut output));
        assert_eq!(
            output.map(f32::to_bits),
            [1.0_f32 / 1024.0, -1.0_f32 / 1024.0].map(f32::to_bits)
        );
        assert_eq!(window.poll().ready_pages, 2);
    }
    #[test]
    fn impossible_window_is_capacity_exceeded_not_permanent_waiting() {
        use audio_types::StreamReadinessState;
        let media =
            TimelineStream::prepare(Box::new(reader()), 200, 4, 2, &PreparationToken::default())
                .unwrap();
        let mut window = media.window();
        for frame in [0, 4, 8] {
            window.require_frame(frame);
        }
        let status = window.poll();
        assert_eq!(status.state, StreamReadinessState::CapacityExceeded);
        assert_eq!(status.required_pages, 3);
        assert_eq!(status.ready_pages, 0);
    }
}
