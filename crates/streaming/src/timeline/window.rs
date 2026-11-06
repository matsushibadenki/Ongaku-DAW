//! Fixed-capacity page leases. Shared across all clip views of one media in a graph.
use super::{EMPTY, TimelineStream};
use audio_types::{StreamReadiness, StreamReadinessState};
use std::sync::{Arc, atomic::Ordering};

const UNUSED: usize = usize::MAX;
pub struct StreamWindow {
    media: Arc<TimelineStream>,
    pages: [u64; 32],
    slots: [usize; 32],
    count: usize,
    overflow: bool,
}
impl StreamWindow {
    pub(super) fn new(media: Arc<TimelineStream>) -> Self {
        Self {
            media,
            pages: [EMPTY; 32],
            slots: [UNUSED; 32],
            count: 0,
            overflow: false,
        }
    }
    /// Collect requirements without I/O, allocation or an unbounded retry loop.
    pub fn require_frame(&mut self, frame: u64) {
        if frame >= self.media.frames || self.overflow {
            return;
        }
        let page = frame / self.media.page_frames as u64;
        if self.pages[..self.count].contains(&page) {
            return;
        }
        if self.count == self.media.shared.pages.len() {
            self.overflow = true;
            return;
        }
        self.pages[self.count] = page;
        self.count += 1;
    }
    /// Partial leases survive Waiting polls, allowing missing pages to load without thrashing.
    /// A caller must replace/drop the window when its transport/graph requirements change.
    pub fn poll(&mut self) -> StreamReadiness {
        if self.count == 0 {
            return StreamReadiness::ready();
        }
        let state = if self.media.shared.failed.load(Ordering::Acquire) {
            StreamReadinessState::Failed
        } else if self.overflow {
            StreamReadinessState::CapacityExceeded
        } else {
            StreamReadinessState::Ready
        };
        let mut result = StreamReadiness {
            state,
            required_pages: self.count as u64 + u64::from(self.overflow),
            ready_pages: 0,
        };
        if matches!(
            state,
            StreamReadinessState::Failed | StreamReadinessState::CapacityExceeded
        ) {
            return result;
        }
        let mut first_missing = None;
        for index in 0..self.count {
            if self.slots[index] == UNUSED {
                self.slots[index] = self
                    .media
                    .shared
                    .pages
                    .iter()
                    .enumerate()
                    .find_map(|(slot, page)| {
                        if page.tag.load(Ordering::SeqCst) != self.pages[index] {
                            return None;
                        }
                        let users = page.users.load(Ordering::SeqCst);
                        // One bounded CAS attempt per slot. Exclusive writers use u32::MAX.
                        if users >= u32::MAX - 1
                            || page
                                .users
                                .compare_exchange(
                                    users,
                                    users + 1,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_err()
                        {
                            return None;
                        }
                        if page.tag.load(Ordering::SeqCst) == self.pages[index] {
                            Some(slot)
                        } else {
                            page.users.fetch_sub(1, Ordering::SeqCst);
                            None
                        }
                    })
                    .unwrap_or(UNUSED);
            }
            if self.slots[index] == UNUSED {
                first_missing.get_or_insert(self.pages[index]);
                self.media
                    .request_frame(self.pages[index] * self.media.page_frames as u64);
                result.state = StreamReadinessState::Waiting;
            } else {
                result.ready_pages += 1;
            }
        }
        if let Some(page) = first_missing {
            self.media.shared.demand.store(page, Ordering::Release);
        }
        result
    }
    /// Exact immutable data from retained pages, including a fault racing after admission.
    /// The next admission poll reports that fault; this already admitted block stays coherent.
    pub fn read_frame(&self, frame: u64, output: &mut [f32]) -> bool {
        output.fill(0.0);
        if frame >= self.media.frames {
            return true;
        }
        let page_id = frame / self.media.page_frames as u64;
        let Some(index) = self.pages[..self.count]
            .iter()
            .position(|page| *page == page_id)
        else {
            return false;
        };
        let slot = self.slots[index];
        if slot == UNUSED {
            return false;
        }
        let page = &self.media.shared.pages[slot];
        let local = usize::try_from(frame % self.media.page_frames as u64).unwrap_or(0)
            * usize::from(self.media.channels);
        for (channel, sample) in output.iter_mut().enumerate() {
            *sample = f32::from_bits(
                page.samples[local + channel.min(usize::from(self.media.channels) - 1)]
                    .load(Ordering::SeqCst),
            );
        }
        true
    }
}
impl Drop for StreamWindow {
    fn drop(&mut self) {
        for &slot in &self.slots[..self.count] {
            if slot != UNUSED {
                self.media.shared.pages[slot]
                    .users
                    .fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}
