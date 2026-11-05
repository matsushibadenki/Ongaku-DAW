//! One control worker, one active snapshot and one replaceable waiting snapshot.
//! No application mutex or preparation state is ever accessed by the audio callback.
use audio_engine::{EngineControl, EngineGraph, MAX_PUBLICATION_REVISION};
use audio_types::{AudioConfig, AudioError, PreparationToken};
use daw_mixer::AudioGraph;
use daw_project::Project;
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreparationOutcomeKind {
    Canceled,
    Published,
    Failed,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparationOutcome {
    pub preparation_revision: u64,
    pub document_revision: u64,
    pub outcome: PreparationOutcomeKind,
    pub publication_revision: Option<u64>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparationFailure {
    pub preparation_revision: u64,
    pub document_revision: u64,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparationStatus {
    pub requested_preparation: u64,
    pub document_revision: u64,
    pub active_preparation: Option<u64>,
    pub waiting_preparation: Option<u64>,
    pub published_preparation: Option<u64>,
    pub published_revision: Option<u64>,
    pub published_document_revision: Option<u64>,
    pub started_count: u64,
    pub canceled_count: u64,
    pub waiting_for_publication: bool,
    pub pending: bool,
    pub failure: Option<PreparationFailure>,
    pub outcomes: Vec<PreparationOutcome>,
    pub outcomes_dropped: u64,
}
struct Job {
    revision: u64,
    document_revision: u64,
    project: Project,
    token: PreparationToken,
}
struct State {
    requested: u64,
    document: u64,
    pending: Option<Job>,
    active: Option<(u64, PreparationToken)>,
    closed: bool,
    published_preparation: Option<u64>,
    published_revision: Option<u64>,
    published_document: Option<u64>,
    started: u64,
    canceled: u64,
    waiting: bool,
    failure: Option<PreparationFailure>,
    outcomes: VecDeque<PreparationOutcome>,
    outcomes_dropped: u64,
}
impl State {
    fn record(&mut self, job: &Job, outcome: PreparationOutcomeKind, publication: Option<u64>) {
        if self.outcomes.len() == 64 {
            self.outcomes.pop_front();
            self.outcomes_dropped += 1;
        }
        self.outcomes.push_back(PreparationOutcome {
            preparation_revision: job.revision,
            document_revision: job.document_revision,
            outcome,
            publication_revision: publication,
        });
        if outcome == PreparationOutcomeKind::Canceled {
            self.canceled += 1;
        }
    }
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
pub(super) type Builder = dyn Fn(&Project, AudioConfig, &PreparationToken) -> Result<AudioGraph, AudioError>
    + Send
    + Sync;

pub(super) struct PreparationService {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}
impl PreparationService {
    pub fn spawn(control: EngineControl, config: AudioConfig) -> Result<Self, AudioError> {
        Self::spawn_with(
            control,
            config,
            Arc::new(Project::build_audio_graph_cancellable),
        )
    }
    pub(super) fn spawn_with(
        control: EngineControl,
        config: AudioConfig,
        builder: Arc<Builder>,
    ) -> Result<Self, AudioError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                requested: 0,
                document: 0,
                pending: None,
                active: None,
                closed: false,
                published_preparation: None,
                published_revision: None,
                published_document: None,
                started: 0,
                canceled: 0,
                waiting: false,
                failure: None,
                outcomes: VecDeque::with_capacity(64),
                outcomes_dropped: 0,
            }),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("ongaku-graph-preparation".into())
            .spawn(move || run(&worker, &control, config, builder.as_ref()))
            .map_err(|error| AudioError::Backend(format!("graph preparation worker: {error}")))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    pub fn request(&self, project: Project, document_revision: u64) -> Result<u64, AudioError> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| AudioError::InvalidState("preparation state lock was poisoned"))?;
        if state.closed {
            return Err(AudioError::InvalidState("preparation worker is closed"));
        }
        let revision = state
            .requested
            .checked_add(1)
            .filter(|value| *value <= MAX_PUBLICATION_REVISION)
            .ok_or(AudioError::InvalidState("preparation revisions exhausted"))?;
        if document_revision > MAX_PUBLICATION_REVISION {
            return Err(AudioError::InvalidState("document revisions exhausted"));
        }
        // Cancellation begins only after request validation; invalid edits never cancel valid work.
        if let Some((_, token)) = &state.active {
            token.cancel();
        }
        if let Some(old) = state.pending.take() {
            old.token.cancel();
            state.record(&old, PreparationOutcomeKind::Canceled, None);
        }
        state.requested = revision;
        state.document = document_revision;
        state.failure = None;
        state.pending = Some(Job {
            revision,
            document_revision,
            project,
            token: PreparationToken::default(),
        });
        self.shared.wake.notify_all();
        Ok(revision)
    }
    #[cfg(test)]
    pub(super) fn wait_for(
        &self,
        predicate: impl Fn(&PreparationStatus) -> bool,
    ) -> PreparationStatus {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            // All predicates are checked off the audio thread. A short timeout also avoids
            // missed notifications between reading status and beginning the condition wait.
            let status = self.status();
            if predicate(&status) {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "preparation wait timed out: {status:?}"
            );
            let state = self.shared.state.lock().unwrap();
            drop(
                self.shared
                    .wake
                    .wait_timeout(state, Duration::from_millis(5))
                    .unwrap(),
            );
        }
    }

    pub fn status(&self) -> PreparationStatus {
        let (state, poisoned) = match self.shared.state.lock() {
            Ok(state) => (state, false),
            Err(error) => (error.into_inner(), true),
        };
        PreparationStatus {
            requested_preparation: state.requested,
            document_revision: state.document,
            active_preparation: state.active.as_ref().map(|(id, _)| *id),
            waiting_preparation: state.pending.as_ref().map(|job| job.revision),
            published_preparation: state.published_preparation,
            published_revision: state.published_revision,
            published_document_revision: state.published_document,
            started_count: state.started,
            canceled_count: state.canceled,
            waiting_for_publication: state.waiting,
            pending: !poisoned && (state.active.is_some() || state.pending.is_some()),
            failure: if poisoned {
                Some(PreparationFailure {
                    preparation_revision: state.requested,
                    document_revision: state.document,
                    message: "graph preparation state failed; restart the engine".into(),
                })
            } else {
                state.failure.clone()
            },
            outcomes: state.outcomes.iter().copied().collect(),
            outcomes_dropped: state.outcomes_dropped,
        }
    }
}
impl Drop for PreparationService {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            if let Some((_, token)) = &state.active {
                token.cancel();
            }
            if let Some(old) = state.pending.take() {
                old.token.cancel();
                state.record(&old, PreparationOutcomeKind::Canceled, None);
            }
            self.shared.wake.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn run(shared: &Shared, control: &EngineControl, config: AudioConfig, builder: &Builder) {
    loop {
        let job = {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            while state.pending.is_none() && !state.closed {
                let Ok(next) = shared.wake.wait(state) else {
                    return;
                };
                state = next;
            }
            if state.closed {
                return;
            }
            let Some(job) = state.pending.take() else {
                continue;
            };
            state.active = Some((job.revision, job.token.clone()));
            state.started += 1;
            shared.wake.notify_all();
            job
        };
        // User-provided future processors must not leave the coordinator silently stuck on unwind.
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            builder(&job.project, config, &job.token)
        }))
        .unwrap_or_else(|_| Err(AudioError::InvalidState("graph preparation panicked")));
        let mut ready = built.map(EngineGraph::timeline);
        loop {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            if state.closed || state.requested != job.revision || job.token.is_canceled() {
                state.record(&job, PreparationOutcomeKind::Canceled, None);
                state.active = None;
                state.waiting = false;
                shared.wake.notify_all();
                break;
            }
            // This mutex is the publication fence. Newer requests cannot race past this gate.
            match ready {
                Ok(graph) => match control.try_submit_latest_graph(graph, job.document_revision) {
                    Ok(revision) => {
                        state.published_preparation = Some(job.revision);
                        state.published_revision = Some(revision);
                        state.published_document = Some(job.document_revision);
                        state.record(&job, PreparationOutcomeKind::Published, Some(revision));
                        state.active = None;
                        state.waiting = false;
                        shared.wake.notify_all();
                        break;
                    }
                    Err((graph, AudioError::PublicationBusy)) => {
                        ready = Ok(graph);
                        state.waiting = true;
                        shared.wake.notify_all();
                        let Ok((state, _)) =
                            shared.wake.wait_timeout(state, Duration::from_millis(5))
                        else {
                            return;
                        };
                        drop(state); // Ready ownership stays on this worker; no rebuild or audio wait.
                    }
                    Err((graph, error)) => {
                        drop(graph);
                        ready = Err(error);
                    }
                },
                Err(error) => {
                    state.failure = Some(PreparationFailure {
                        preparation_revision: job.revision,
                        document_revision: job.document_revision,
                        message: error.to_string(),
                    });
                    state.record(&job, PreparationOutcomeKind::Failed, None);
                    state.active = None;
                    state.waiting = false;
                    shared.wake.notify_all();
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
pub(super) struct TestGate {
    open: Mutex<bool>,
    wake: Condvar,
}
#[cfg(test)]
impl TestGate {
    pub(super) fn new() -> Self {
        Self {
            open: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
    pub(super) fn wait(&self) {
        let (open, _) = self
            .wake
            .wait_timeout_while(self.open.lock().unwrap(), Duration::from_secs(5), |open| {
                !*open
            })
            .unwrap();
        assert!(*open, "preparation fixture gate timed out");
    }
    pub(super) fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_engine::{EngineProcessor, GraphOutcome};
    use audio_types::PreparationCheck;
    use daw_core::{SamplePosition, transport::TransportCommand};
    use daw_mixer::{MixerChange, MixerEvent, TrackId};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    fn config() -> AudioConfig {
        AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        }
    }
    fn muted(original: &Project) -> Project {
        let mut project = original.clone();
        project
            .set_mixer_event(
                TrackId(4),
                MixerEvent {
                    id: 1,
                    position: SamplePosition(129),
                    change: MixerChange::Mute(true),
                },
            )
            .unwrap();
        project
    }
    #[test]
    fn newer_pending_snapshots_cancel_active_and_waiting_work_before_any_publication_id() {
        let (_processor, control) = EngineProcessor::prepare(config()).unwrap();
        let observer = control.clone();
        let original = Project::demo(8000).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let release = Arc::new(TestGate::new());
        let worker_release = Arc::clone(&release);
        let (started, receiver) = mpsc::sync_channel(1);
        let service = PreparationService::spawn_with(
            control,
            config(),
            Arc::new(move |project, config, token| {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    started.send(()).unwrap();
                    worker_release.wait();
                }
                project.build_audio_graph_cancellable(config, token)
            }),
        )
        .unwrap();
        service.request(original.clone(), 11).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        service.request(muted(&original), 12).unwrap();
        service.request(original, 11).unwrap(); // Undo restores the older document ID with a new preparation ID.
        release.release();
        let status = service.wait_for(|status| !status.pending);
        assert_eq!(status.requested_preparation, 3);
        assert_eq!(status.started_count, 2);
        assert_eq!(status.canceled_count, 2);
        assert_eq!(status.published_preparation, Some(3));
        assert_eq!(status.published_document_revision, Some(11));
        assert_eq!(status.published_revision, Some(1));
        assert_eq!(observer.graph_publication_status(0).accepted_revision, 1);
        assert!(
            status
                .outcomes
                .iter()
                .filter(|outcome| outcome.outcome == PreparationOutcomeKind::Canceled)
                .all(|outcome| outcome.publication_revision.is_none())
        );
    }

    #[test]
    fn busy_publication_reuses_the_prepared_graph_and_cancellation_still_wins() {
        let (mut processor, control) = EngineProcessor::prepare(config()).unwrap();
        let original = Project::demo(8000).unwrap();
        for revision in 1..=4 {
            control
                .submit_graph_revision(
                    EngineGraph::timeline(original.build_audio_graph(config()).unwrap()),
                    revision,
                    None,
                )
                .unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let service = PreparationService::spawn_with(
            control.clone(),
            config(),
            Arc::new(move |project, config, token| {
                count.fetch_add(1, Ordering::SeqCst);
                project.build_audio_graph_cancellable(config, token)
            }),
        )
        .unwrap();
        service.request(muted(&original), 5).unwrap();
        service.wait_for(|status| status.waiting_for_publication);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        service.request(original, 6).unwrap();
        service.wait_for(|status| {
            status.waiting_for_publication
                && status.requested_preparation == 2
                && status.started_count == 2
        });
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        processor.process_block(&mut [0.0; 2]); // Free one input slot; no sleep-based timing oracle.
        let status = service.wait_for(|status| !status.pending);
        assert_eq!(calls.load(Ordering::SeqCst), 2); // Retries did not rebuild.
        assert_eq!(status.canceled_count, 1);
        assert_eq!(status.published_revision, Some(5));
        assert_eq!(status.published_document_revision, Some(6));
    }

    #[test]
    fn publication_fence_and_actual_audible_deadline_survive_cancellation_and_queue_coalescing() {
        for retain_event in [false, true] {
            let original = Project::demo(8000).unwrap();
            let scheduled = muted(&original);
            let (mut processor, control) = EngineProcessor::prepare(config()).unwrap();
            control
                .submit_graph(EngineGraph::timeline(
                    original.build_audio_graph(config()).unwrap(),
                ))
                .unwrap();
            control.send_transport(TransportCommand::Play).unwrap();
            processor.process_block(&mut [0.0; 200]);
            let calls = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&calls);
            let release = Arc::new(TestGate::new());
            let worker_release = Arc::clone(&release);
            let (started, receiver) = mpsc::sync_channel(1);
            let service = PreparationService::spawn_with(
                control.clone(),
                config(),
                Arc::new(move |project, config, token| {
                    if count.fetch_add(1, Ordering::SeqCst) == 1 {
                        started.send(()).unwrap();
                        worker_release.wait();
                        // Simulate completion after the final cooperative checkpoint: the fence must still reject it.
                        return project.build_audio_graph(config);
                    }
                    project.build_audio_graph_cancellable(config, token)
                }),
            )
            .unwrap();
            service.request(scheduled.clone(), 1).unwrap();
            service.wait_for(|status| status.published_revision == Some(2));
            service.request(original.clone(), 2).unwrap();
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            let final_project = if retain_event { scheduled } else { original };
            service
                .request(final_project, u64::from(retain_event))
                .unwrap();
            release.release();
            let prepared = service.wait_for(|status| !status.pending);
            assert_eq!(prepared.published_revision, Some(3)); // Canceled preparation never consumed publication 3.
            assert_eq!(prepared.canceled_count, 1);
            control
                .send_transport(TransportCommand::Seek(SamplePosition(130)))
                .unwrap();
            processor.process_block(&mut [0.0; 2]);
            let status = control.graph_publication_status(1);
            assert_eq!(status.acknowledgements.len(), 2);
            assert_eq!(status.acknowledgements[0].outcome, GraphOutcome::Superseded);
            let adopted = status.acknowledgements[1];
            assert_eq!(adopted.outcome, GraphOutcome::Adopted);
            assert_eq!(adopted.document_revision, u64::from(retain_event));
            assert_eq!(adopted.adopted_sample, Some(SamplePosition(130)));
            assert_eq!(
                adopted.deadline,
                retain_event.then_some(SamplePosition(129))
            );
            assert_eq!(adopted.late, retain_event);
        }
    }

    #[test]
    fn failure_is_observable_and_a_new_valid_request_recovers_without_false_publication() {
        let (_processor, control) = EngineProcessor::prepare(config()).unwrap();
        let observer = control.clone();
        let calls = AtomicUsize::new(0);
        let service = PreparationService::spawn_with(
            control,
            config(),
            Arc::new(move |project, config, token| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(AudioError::InvalidConfig("fixture preparation failure"));
                }
                project.build_audio_graph_cancellable(config, token)
            }),
        )
        .unwrap();
        service.request(Project::demo(8000).unwrap(), 7).unwrap();
        let failed = service.wait_for(|status| !status.pending);
        assert!(failed.failure.is_some());
        assert_eq!(observer.graph_publication_status(0).accepted_revision, 0);
        service.request(Project::demo(8000).unwrap(), 8).unwrap();
        let recovered = service.wait_for(|status| !status.pending);
        assert!(recovered.failure.is_none());
        assert_eq!(recovered.published_document_revision, Some(8));
        assert_eq!(recovered.published_revision, Some(1));
    }

    #[test]
    fn shutdown_cancels_active_work_and_joins_before_it_can_publish() {
        let (_processor, control) = EngineProcessor::prepare(config()).unwrap();
        let observer = control.clone();
        let (started, receiver) = mpsc::sync_channel(1);
        let service = PreparationService::spawn_with(
            control,
            config(),
            Arc::new(move |_, _, token| {
                started.send(()).unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                loop {
                    token.checkpoint()?;
                    assert!(std::time::Instant::now() < deadline);
                    thread::yield_now();
                }
            }),
        )
        .unwrap();
        service.request(Project::demo(8000).unwrap(), 1).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(service);
        assert_eq!(observer.graph_publication_status(0).accepted_revision, 0);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use audio_engine::EngineProcessor;
    use std::sync::mpsc;
    #[test]
    fn pending_snapshot_and_outcome_retention_remain_bounded() {
        let config = AudioConfig {
            sample_rate: 8000,
            ..AudioConfig::DEFAULT
        };
        let (_processor, control) = EngineProcessor::prepare(config).unwrap();
        let original = Project::demo(8000).unwrap();
        let release = Arc::new(TestGate::new());
        let worker_release = Arc::clone(&release);
        let (started, receiver) = mpsc::sync_channel(1);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let service = PreparationService::spawn_with(
            control,
            config,
            Arc::new(move |project, config, token| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    started.send(()).unwrap();
                    worker_release.wait();
                }
                project.build_audio_graph_cancellable(config, token)
            }),
        )
        .unwrap();
        service.request(original.clone(), 1).unwrap();
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        for document in 2..=70 {
            service.request(original.clone(), document).unwrap();
        }
        release.release();
        let status = service.wait_for(|status| !status.pending);
        assert_eq!(status.started_count, 2);
        assert_eq!(status.canceled_count, 69);
        assert_eq!(status.outcomes.len(), 64);
        assert_eq!(status.outcomes_dropped, 6);
        assert_eq!(status.published_preparation, Some(70));
        assert_eq!(status.published_revision, Some(1));
    }
}
