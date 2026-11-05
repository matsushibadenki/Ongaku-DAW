use super::*;
use audio_clip::{AudioClip, PcmAsset};
use daw_core::SamplePosition;
use daw_mixer::{MixerChange, MixerEvent, Track, TrackId};

fn timeline(gain: f32, mute_at: Option<u64>) -> AudioGraph {
    let config = AudioConfig::DEFAULT;
    let mut graph = AudioGraph::new(config).unwrap();
    let mut track = Track::new(TrackId(1));
    track.set_gain(gain);
    track.add_clip(
        AudioClip::new(
            SamplePosition::ZERO,
            0,
            2048,
            Arc::new(PcmAsset::new(48000, 2, vec![0.25; 4096]).unwrap()),
        )
        .unwrap(),
    );
    if let Some(position) = mute_at {
        track
            .set_mixer_events(&[MixerEvent {
                id: 1,
                position: SamplePosition(position),
                change: MixerChange::Mute(true),
            }])
            .unwrap();
    }
    graph.add_track(track).unwrap();
    graph
}

#[test]
fn latest_snapshots_resolve_superseded_ids_but_adopt_only_the_latest() {
    let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
    for document in 1..=4 {
        control
            .submit_latest_timeline(timeline(1.0, None), document)
            .unwrap();
    }
    control.send_transport(TransportCommand::Play).unwrap();
    let mut output = [0.0; 256];
    processor.process_block(&mut output);
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
    let status = control.graph_publication_status(0);
    assert_eq!(status.adopted_revision, 4);
    assert_eq!(status.settled_revision, 4);
    assert!(!status.pending);
    assert_eq!(status.superseded_count, 3);
    assert_eq!(status.acknowledgements.len(), 4);
    for receipt in &status.acknowledgements[..3] {
        assert_eq!(receipt.outcome, GraphOutcome::Superseded);
        assert!(receipt.adopted_sample.is_none());
        assert!(!receipt.late);
    }
    assert_eq!(status.acknowledgements[3].outcome, GraphOutcome::Adopted);
}

#[test]
fn explicit_cancel_cannot_be_confused_with_adoption_or_invalidate_ordered_updates() {
    let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
    let first = control
        .submit_graph_revision(
            EngineGraph::timeline(timeline(0.0, None)),
            1,
            Some(SamplePosition(0)),
        )
        .unwrap();
    assert!(control.cancel_pending_graph(first).unwrap());
    assert!(!control.cancel_pending_graph(first).unwrap());
    control
        .send_transport(TransportCommand::Seek(SamplePosition(129)))
        .unwrap();
    control.send_transport(TransportCommand::Play).unwrap();
    processor.process_block(&mut [0.0; 2]);
    let status = control.graph_publication_status(0);
    assert_eq!(status.adopted_revision, 0);
    assert_eq!(status.settled_revision, first);
    assert_eq!(status.canceled_count, 1);
    assert!(!status.pending);
    assert_eq!(status.late_count, 0);
    assert_eq!(status.acknowledgements[0].outcome, GraphOutcome::Canceled);
    assert!(status.acknowledgements[0].adopted_sample.is_none());
    let second = control
        .submit_graph_revision(EngineGraph::timeline(timeline(1.0, None)), 2, None)
        .unwrap();
    let third = control
        .submit_latest_timeline(timeline(1.0, None), 3)
        .unwrap();
    processor.process_block(&mut [0.0; 2]);
    assert_eq!(
        control.graph_publication_status(first).adopted_revision,
        second
    );
    assert!(!control.cancel_pending_graph(second).unwrap());
    processor.process_block(&mut [0.0; 2]);
    assert_eq!(
        control.graph_publication_status(second).adopted_revision,
        third
    );
}

#[test]
fn rejected_newest_update_does_not_supersede_the_last_accepted_snapshot() {
    let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
    for document in 1..=4 {
        control
            .submit_latest_timeline(timeline(1.0, None), document)
            .unwrap();
    }
    assert!(
        control
            .submit_latest_timeline(timeline(0.0, None), 5)
            .is_err()
    );
    assert_eq!(control.graph_publication_status(0).accepted_revision, 4);
    processor.process_block(&mut [0.0; 2]);
    assert_eq!(control.graph_publication_status(0).adopted_revision, 4);
    assert_eq!(
        control
            .submit_latest_timeline(timeline(1.0, None), 5)
            .unwrap(),
        5
    );
}

#[test]
fn final_snapshot_deadline_is_compared_to_the_audible_graph_not_a_skipped_parent() {
    let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
    control
        .submit_graph(EngineGraph::timeline(timeline(1.0, None)))
        .unwrap();
    control.send_transport(TransportCommand::Play).unwrap();
    processor.process_block(&mut [0.0; 2]);
    control
        .submit_latest_timeline(timeline(1.0, Some(129)), 1)
        .unwrap();
    let latest = control
        .submit_latest_timeline(timeline(0.8, Some(129)), 2)
        .unwrap();
    control
        .send_transport(TransportCommand::Seek(SamplePosition(130)))
        .unwrap();
    processor.process_block(&mut [0.0; 2]);
    let status = control.graph_publication_status(1);
    assert_eq!(status.acknowledgements[0].outcome, GraphOutcome::Superseded);
    let adopted = status.acknowledgements.last().unwrap();
    assert_eq!(adopted.revision, latest);
    assert_eq!(adopted.deadline, Some(SamplePosition(129)));
    assert!(adopted.late);
    assert_eq!(status.late_count, 1);
}

#[test]
fn superseded_retirement_backpressure_preserves_pending_latest_ownership() {
    let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
    for document in 1..=4 {
        control
            .submit_latest_timeline(timeline(1.0, None), document)
            .unwrap();
    }
    for _ in 0..GRAPH_QUEUE_CAPACITY {
        assert!(
            processor
                .retired_graphs
                .push(EngineGraph::timeline(timeline(1.0, None)))
                .is_ok()
        );
    }
    processor.process_block(&mut [0.0; 2]);
    let status = control.graph_publication_status(0);
    assert_eq!(status.settled_revision, 1);
    assert_eq!(status.adopted_revision, 0);
    assert!(status.pending);
    processor.process_block(&mut [0.0; 2]);
    assert_eq!(control.graph_publication_status(0).settled_revision, 1);
    control.reclaim_retired().unwrap();
    processor.process_block(&mut [0.0; 2]);
    assert_eq!(control.graph_publication_status(0).adopted_revision, 4);
}

#[test]
fn recoverable_backpressure_returns_the_same_graph_for_retry_and_reclaims_it_off_audio() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct DropProbe(Arc<AtomicUsize>);
    impl daw_mixer::RealtimeTimelineSource for DropProbe {
        fn sample_rate(&self) -> u32 {
            48000
        }
        fn channels(&self) -> u16 {
            2
        }
        fn render_interleaved(&mut self, output: &mut [f32], _: &Transport, _: usize) {
            output.fill(0.0);
        }
    }
    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (mut processor, control) = EngineProcessor::prepare(AudioConfig::DEFAULT).unwrap();
    for document in 0..4 {
        control
            .submit_graph_revision(EngineGraph::test_tone(AudioConfig::DEFAULT), document, None)
            .unwrap();
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut graph = timeline(0.7, None);
    graph
        .track_mut(TrackId(1))
        .unwrap()
        .set_timeline_source(Box::new(DropProbe(Arc::clone(&dropped))))
        .unwrap();
    let (graph, error) = control
        .try_submit_latest_graph(EngineGraph::timeline(graph), 5)
        .unwrap_err();
    assert_eq!(error, AudioError::PublicationBusy);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(control.graph_publication_status(0).accepted_revision, 4);
    processor.process_block(&mut [0.0; 2]);
    assert!(matches!(control.try_submit_latest_graph(graph, 5), Ok(5)));
    assert!(control.cancel_pending_graph(5).unwrap());
    for _ in 0..4 {
        processor.process_block(&mut [0.0; 2]);
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    control.reclaim_retired().unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
