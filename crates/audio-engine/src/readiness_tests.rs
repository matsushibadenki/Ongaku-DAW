use super::*;
use audio_clip::{AudioClip, PcmAsset, StreamedAudioClip};
use audio_file::AudioFrameReader;
use audio_types::PreparationToken;
use daw_core::{SamplePosition, SampleRange};
use daw_midi::{MidiClip, MidiNote, MidiSynthSource};
use daw_mixer::{AudioGraph, Track, TrackId};
use daw_streaming::TimelineStream;
use std::{
    sync::{Condvar, mpsc},
    time::{Duration, Instant},
};

struct Gate {
    open: Mutex<bool>,
    wake: Condvar,
}
impl Gate {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.wake.notify_all();
    }
    fn wait(&self) {
        let (open, _) = self
            .wake
            .wait_timeout_while(self.open.lock().unwrap(), Duration::from_secs(10), |open| {
                !*open
            })
            .unwrap();
        assert!(*open, "reader fixture gate timed out");
    }
}
struct Reader {
    cursor: usize,
    gate: Arc<Gate>,
    started: Option<mpsc::SyncSender<()>>,
    fail: bool,
}
fn value(frame: usize, channel: usize) -> f32 {
    let sample = f32::from(u16::try_from(frame + 1).unwrap()) / 2048.0;
    if channel == 0 { sample } else { -sample }
}
impl AudioFrameReader for Reader {
    fn sample_rate(&self) -> u32 {
        8000
    }
    fn channels(&self) -> u16 {
        2
    }
    fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        self.cursor = usize::try_from(frame).unwrap() * 2;
        if frame >= 8 {
            if let Some(sender) = self.started.take() {
                sender.send(()).unwrap();
            }
            self.gate.wait();
            if self.fail {
                return Err(AudioError::InvalidState("injected read failure"));
            }
        }
        Ok(())
    }
    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError> {
        for sample in &mut *output {
            *sample = value(self.cursor / 2, self.cursor % 2);
            self.cursor += 1;
        }
        Ok(output.len())
    }
}
fn fixture(fail: bool) -> (Arc<TimelineStream>, Arc<Gate>, mpsc::Receiver<()>) {
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        wake: Condvar::new(),
    });
    let (started, receiver) = mpsc::sync_channel(1);
    let reader = Reader {
        cursor: 0,
        gate: Arc::clone(&gate),
        started: Some(started),
        fail,
    };
    let media =
        TimelineStream::prepare(Box::new(reader), 128, 4, 2, &PreparationToken::default()).unwrap();
    (media, gate, receiver)
}
fn config() -> AudioConfig {
    AudioConfig {
        sample_rate: 8000,
        channels: 2,
        buffer_size: 4,
    }
}
fn graph(media: &Arc<TimelineStream>, direct: bool, gain: f32) -> EngineGraph {
    let mut track = Track::new(TrackId(1));
    track.set_gain(gain);
    if direct {
        let samples = (0..128)
            .flat_map(|frame| [value(frame, 0), value(frame, 1)])
            .collect();
        let asset = Arc::new(PcmAsset::new(8000, 2, samples).unwrap());
        track.add_clip(AudioClip::new(SamplePosition::ZERO, 0, 128, asset).unwrap());
    } else {
        track
            .add_streamed_clip(
                StreamedAudioClip::new(SamplePosition::ZERO, 0, 128, Arc::clone(media)).unwrap(),
            )
            .unwrap();
    }
    track
        .set_timeline_source(Box::new(
            MidiSynthSource::new(
                config(),
                &[MidiClip {
                    id: 1,
                    timeline_start: SamplePosition::ZERO,
                    duration_samples: 128,
                    notes: vec![MidiNote {
                        id: 1,
                        start: SamplePosition::ZERO,
                        duration_samples: 128,
                        key: 60,
                        velocity: 0.3,
                        channel: 0,
                    }],
                }],
                None,
            )
            .unwrap(),
        ))
        .unwrap();
    let mut graph = AudioGraph::new(config()).unwrap();
    graph.add_track(track).unwrap();
    EngineGraph::timeline(graph)
}
fn render(engine: &mut EngineProcessor, output: &mut [f32]) {
    crate::tests::assert_no_realtime_memory(|| engine.process_block(output));
}
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < until, "readiness never resolved");
        std::thread::yield_now();
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One continuous seek/loop/adoption scenario proves resource and PCM continuity.
fn cold_loop_wait_holds_midi_ramps_and_position_and_releases_old_graph_leases() {
    let (media, gate, started) = fixture(false);
    let (mut actual, control) = EngineProcessor::prepare(config()).unwrap();
    let (mut reference, ref_control) = EngineProcessor::prepare(config()).unwrap();
    control.submit_graph(graph(&media, false, 1.0)).unwrap();
    ref_control.submit_graph(graph(&media, true, 1.0)).unwrap();
    for ctl in [&control, &ref_control] {
        ctl.send_transport(TransportCommand::Play).unwrap();
    }
    let mut out = [0.0; 4];
    let mut expected = out;
    render(&mut actual, &mut out);
    reference.process_block(&mut expected);
    assert_eq!(out.map(f32::to_bits), expected.map(f32::to_bits));
    assert!(
        control
            .stream_readiness()
            .is_some_and(|s| s.readiness.state == StreamReadinessState::Ready)
    );
    control
        .send_transport(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(4),
            SamplePosition(12),
        )))
        .unwrap();
    control
        .send_transport(TransportCommand::Seek(SamplePosition(4)))
        .unwrap();
    let mut out = [1.0; 16];
    render(&mut actual, &mut out);
    assert!(out.iter().all(|sample| *sample == 0.0));
    assert_eq!(control.transport().current_sample, SamplePosition(4));
    let first = control.stream_readiness().unwrap();
    assert_eq!(first.readiness.state, StreamReadinessState::Waiting);
    assert_eq!(
        (first.readiness.required_pages, first.readiness.ready_pages),
        (2, 1)
    );
    assert_eq!(first.rendered_frames, 0);
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let canceled = control
        .submit_graph_revision(graph(&media, false, 0.25), 9, None)
        .unwrap();
    assert!(control.cancel_pending_graph(canceled).unwrap());
    render(&mut actual, &mut out);
    assert_eq!(
        control.stream_readiness().unwrap().graph_revision,
        first.graph_revision
    );
    // Change target and graph while the old reader is blocked. The retired graph
    // is deliberately NOT reclaimed before the new two-page window must become ready.
    let adopted = control
        .submit_graph_revision(graph(&media, false, 0.25), 10, None)
        .unwrap();
    control
        .send_transport(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(80),
            SamplePosition(88),
        )))
        .unwrap();
    control
        .send_transport(TransportCommand::Seek(SamplePosition(80)))
        .unwrap();
    render(&mut actual, &mut out);
    let pending = control.stream_readiness().unwrap();
    assert_eq!(pending.graph_revision, adopted);
    assert!(pending.transport_generation > first.transport_generation);
    assert_eq!(pending.position, SamplePosition(80));
    assert_eq!(pending.readiness.state, StreamReadinessState::Waiting);
    for _ in 0..5 {
        render(&mut actual, &mut out);
        assert_eq!(control.transport().current_sample, SamplePosition(80));
        assert!(out.iter().all(|sample| *sample == 0.0));
    }
    ref_control.submit_graph(graph(&media, true, 0.25)).unwrap();
    ref_control
        .send_transport(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(80),
            SamplePosition(88),
        )))
        .unwrap();
    ref_control
        .send_transport(TransportCommand::Seek(SamplePosition(80)))
        .unwrap();
    gate.release();
    wait(|| {
        render(&mut actual, &mut out);
        let status = control.stream_readiness().unwrap();
        if status.readiness.state == StreamReadinessState::Waiting {
            assert_eq!(control.transport().current_sample, SamplePosition(80));
            assert!(out.iter().all(|sample| *sample == 0.0));
            false
        } else {
            assert_eq!(status.readiness.state, StreamReadinessState::Ready);
            true
        }
    });
    let mut expected = [0.0; 16];
    reference.process_block(&mut expected);
    assert_eq!(out.map(f32::to_bits), expected.map(f32::to_bits));
    let reads = media.snapshot().page_reads;
    for _ in 0..10 {
        render(&mut actual, &mut out);
        reference.process_block(&mut expected);
        assert_eq!(out.map(f32::to_bits), expected.map(f32::to_bits));
        assert_eq!(
            control.stream_readiness().unwrap().readiness.state,
            StreamReadinessState::Ready
        );
    }
    assert_eq!(
        media.snapshot().page_reads,
        reads,
        "a full short loop must not prefetch unrelated tail pages"
    );
    control.reclaim_retired().unwrap();
}

#[test]
fn capacity_is_checked_for_the_union_of_shared_media_views_and_is_recoverable() {
    let (media, gate, _started) = fixture(false);
    gate.release();
    let mut graph = AudioGraph::new(config()).unwrap();
    for id in 0..3 {
        let mut track = Track::new(TrackId(id));
        track
            .add_streamed_clip(
                StreamedAudioClip::new(SamplePosition::ZERO, id * 4, 4, Arc::clone(&media))
                    .unwrap(),
            )
            .unwrap();
        graph.add_track(track).unwrap();
    }
    let (mut actual, control) = EngineProcessor::prepare(config()).unwrap();
    control.submit_graph(EngineGraph::timeline(graph)).unwrap();
    control.send_transport(TransportCommand::Play).unwrap();
    let mut output = [1.0; 8];
    render(&mut actual, &mut output);
    assert_eq!(
        control.stream_readiness().unwrap().readiness.state,
        StreamReadinessState::CapacityExceeded
    );
    assert_eq!(
        control
            .transport_protocol_status()
            .latest
            .unwrap()
            .result
            .unwrap()
            .state,
        TransportCommandState::CapacityExceeded
    );
    assert_eq!(control.transport().current_sample, SamplePosition::ZERO);
    assert!(output.iter().all(|value| *value == 0.0));
    control
        .submit_graph(self::graph(&media, false, 1.0))
        .unwrap();
    render(&mut actual, &mut output);
    assert_eq!(
        control.stream_readiness().unwrap().readiness.state,
        StreamReadinessState::Ready
    );
    assert_eq!(control.transport().current_sample, SamplePosition(4));
}

#[test]
fn stop_cancels_a_pending_wait_and_faults_do_not_advance_the_timeline() {
    let (media, gate, started) = fixture(true);
    let (mut actual, control) = EngineProcessor::prepare(config()).unwrap();
    control.submit_graph(graph(&media, false, 1.0)).unwrap();
    control
        .send_transport(TransportCommand::Seek(SamplePosition(40)))
        .unwrap();
    control.send_transport(TransportCommand::Play).unwrap();
    let mut output = [1.0; 8];
    render(&mut actual, &mut output);
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    control.send_transport(TransportCommand::Stop).unwrap();
    render(&mut actual, &mut output);
    assert!(!control.transport().playing);
    assert_eq!(
        control.stream_readiness().unwrap().readiness.state,
        StreamReadinessState::Inactive
    );
    gate.release();
    wait(|| media.snapshot().worker_failed);
    control.send_transport(TransportCommand::Play).unwrap();
    render(&mut actual, &mut output);
    assert_eq!(
        control.stream_readiness().unwrap().readiness.state,
        StreamReadinessState::Failed
    );
    assert_eq!(
        control
            .transport_protocol_status()
            .latest
            .unwrap()
            .result
            .unwrap()
            .state,
        TransportCommandState::Failed
    );
    assert_eq!(control.transport().current_sample, SamplePosition(40));
    assert!(output.iter().all(|value| *value == 0.0));
    // A failed inactive clip is not required by a window beyond its end.
    control
        .send_transport(TransportCommand::Seek(SamplePosition(140)))
        .unwrap();
    render(&mut actual, &mut output);
    assert_eq!(
        control.stream_readiness().unwrap().readiness.state,
        StreamReadinessState::Ready
    );
    assert_eq!(control.transport().current_sample, SamplePosition(144));
}

#[test]
#[allow(clippy::too_many_lines)] // One blocked-reader lifecycle spans acceptance, publication, cancellation and exact resume.
fn accepted_seek_ids_follow_wait_cancel_supersede_and_ready_across_graph_adoption() {
    let (media, gate, started) = fixture(false);
    let (mut actual, control) = EngineProcessor::prepare(config()).unwrap();
    control.submit_graph(graph(&media, false, 1.0)).unwrap();
    let play = control.submit_transport(TransportCommand::Play).unwrap();
    let seek = control
        .submit_transport(TransportCommand::Seek(SamplePosition(40)))
        .unwrap();
    let mut output = [0.0; 8];
    render(&mut actual, &mut output);
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let waiting = control
        .transport_command_status(seek)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(waiting.state, TransportCommandState::Waiting);
    assert!(waiting.effect_applied);
    assert_eq!(waiting.applied_sample, Some(SamplePosition(40)));
    assert_eq!(waiting.resolved_sample, None);
    assert_eq!(
        control
            .transport_command_status(play)
            .unwrap()
            .result
            .unwrap()
            .cancel_reason,
        Some(TransportCancelReason::Superseded)
    );
    let adopted = control
        .submit_graph_revision(graph(&media, false, 0.25), 9, None)
        .unwrap();
    render(&mut actual, &mut output);
    assert_eq!(
        control
            .transport_command_status(seek)
            .unwrap()
            .result
            .unwrap()
            .graph_revision,
        adopted
    );
    assert_eq!(
        control.stream_readiness().unwrap().transport_command_id,
        seek
    );
    assert!(control.cancel_transport_command(seek).unwrap());
    render(&mut actual, &mut output);
    let canceled = control
        .transport_command_status(seek)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(canceled.state, TransportCommandState::Canceled);
    assert_eq!(
        canceled.cancel_reason,
        Some(TransportCancelReason::Explicit)
    );
    assert!(canceled.effect_applied);
    assert!(!control.transport().playing);
    assert_eq!(control.transport().current_sample, SamplePosition(40));
    let newer_play = control.submit_transport(TransportCommand::Play).unwrap();
    let newer_seek = control
        .submit_transport(TransportCommand::Seek(SamplePosition(80)))
        .unwrap();
    render(&mut actual, &mut output);
    assert_eq!(
        control
            .transport_command_status(newer_play)
            .unwrap()
            .result
            .unwrap()
            .cancel_reason,
        Some(TransportCancelReason::Superseded)
    );
    assert_eq!(
        control
            .transport_command_status(newer_seek)
            .unwrap()
            .result
            .unwrap()
            .state,
        TransportCommandState::Waiting
    );
    gate.release();
    wait(|| {
        render(&mut actual, &mut output);
        control
            .transport_command_status(newer_seek)
            .unwrap()
            .result
            .unwrap()
            .state
            == TransportCommandState::Applied
    });
    let ready = control
        .transport_command_status(newer_seek)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(ready.resolved_sample, Some(SamplePosition(80)));
    assert_eq!(ready.graph_revision, adopted);
    assert_eq!(ready.readiness.state, StreamReadinessState::Ready);
    assert_eq!(control.transport().current_sample, SamplePosition(84));
    assert!(!control.cancel_transport_command(newer_seek).unwrap());
    assert_eq!(
        control
            .transport_command_status(seek)
            .unwrap()
            .result
            .unwrap(),
        canceled
    );
}
