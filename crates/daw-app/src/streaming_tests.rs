use super::*;
use audio_engine::{EngineGraph, EngineProcessor};
use daw_core::{SamplePosition, SampleRange, transport::TransportCommand};

#[test]
fn native_streaming_survives_real_undo_redo_coalescing_and_transport_commands() {
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(8000).unwrap();
    let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
    let (mut reference, ref_control) = EngineProcessor::prepare(config).unwrap();
    for ctl in [&control, &ref_control] {
        ctl.submit_graph(EngineGraph::timeline(
            app.project.build_audio_graph(config).unwrap(),
        ))
        .unwrap();
        ctl.send_transport(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(17),
            SamplePosition(900),
        )))
        .unwrap();
        ctl.send_transport(TransportCommand::Play).unwrap();
    }
    app.active_config = Some(config);
    app.control = Some(control);
    let mut initial = [0.0; 34];
    let mut expected = initial;
    actual.process_block(&mut initial);
    reference.process_block(&mut expected);
    assert_eq!(initial.map(f32::to_bits), expected.map(f32::to_bits));
    let reads = app.project.streaming_status().page_reads;
    assert!(reads > 0);
    for step in 0..24 {
        // These edits rebuild the whole graph while changing no audible media/mixer targets.
        app.reorder_tracks(&[4, 3, 2, 1]).unwrap();
        app.undo().unwrap();
        app.redo().unwrap();
        if step % 3 == 0 {
            for ctl in [app.control.as_ref().unwrap(), &ref_control] {
                ctl.send_transport(TransportCommand::Seek(SamplePosition(899)))
                    .unwrap();
            }
        }
        let frames = [1, 17, 127, 257, 513][step % 5];
        let mut out = vec![0.0; frames * 2];
        let mut expected = out.clone();
        actual.process_block(&mut out);
        reference.process_block(&mut expected);
        for (a, b) in out.iter().zip(&expected) {
            assert!((a - b).abs() < 0.000_001);
        }
        assert!(out.iter().any(|value| value.abs() > 0.000_001));
        let status = app.service_publications(0).unwrap();
        assert_eq!(
            status.streaming.page_reads, reads,
            "history must reuse warm media"
        );
        assert_eq!(status.streaming.underrun_frames, 0);
        assert_eq!(status.streaming.failed_media, 0);
        assert!(status.engine.unwrap().adopted_revision > 0);
        // Return to original order so the next iteration is an actual edit again.
        app.undo().unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history-stream.ongaku");
    app.save_project(&path).unwrap();
    let snapshot = app.project();
    app.stop().unwrap();
    app.load_project(&path).unwrap();
    assert_eq!(snapshot, app.project());
    assert_eq!(app.project.streaming_status().prepared_media, 0); // runtime cache is not persisted
}

#[test]
#[allow(clippy::too_many_lines)] // One history scenario compares the real engine to independent PCM mixing.
fn audible_stream_mixer_history_matches_direct_pcm_without_restarting_audio() {
    use audio_clip::{AudioClip, PcmAsset};
    use daw_core::transport::Transport;
    use daw_mixer::{AudioGraph, Track, TrackId};
    use std::sync::Arc;
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(8000).unwrap();
    for track in 2..=4 {
        app.set_track_mute(track, true).unwrap();
    }
    app.undo.clear();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pcm-oracle.ongaku");
    app.project.save(&path).unwrap();
    let saved: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    let samples = serde_json::from_value::<Vec<f32>>(saved["media"][0]["samples"].clone()).unwrap();
    let asset = Arc::new(PcmAsset::new(8000, 1, samples).unwrap());
    let mut track = Track::new(TrackId(1));
    track.add_clip(AudioClip::new(SamplePosition::ZERO, 0, asset.frame_count(), asset).unwrap());
    track.set_gain(0.8);
    let mut reference = AudioGraph::new(config).unwrap();
    reference.add_track(track).unwrap();
    let mut transport = Transport::new(8000);
    transport.apply(TransportCommand::SetLoop(SampleRange::new(
        SamplePosition(17),
        SamplePosition(900),
    )));
    transport.apply(TransportCommand::Play);
    let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
    control
        .submit_graph(EngineGraph::timeline(
            app.project.build_audio_graph(config).unwrap(),
        ))
        .unwrap();
    control
        .send_transport(TransportCommand::SetLoop(SampleRange::new(
            SamplePosition(17),
            SamplePosition(900),
        )))
        .unwrap();
    control.send_transport(TransportCommand::Play).unwrap();
    app.active_config = Some(config);
    app.control = Some(control);
    let mut initial = [0.0; 34];
    let mut expected = initial;
    actual.process_block(&mut initial);
    reference.process(&mut expected, &transport);
    transport.advance(17);
    assert_eq!(initial.map(f32::to_bits), expected.map(f32::to_bits));
    let targets: [(f32, f32, bool); 9] = [
        (0.25, 0.0, false),
        (0.8, 0.0, false),
        (0.25, 0.0, false),
        (0.25, 0.35, false),
        (0.25, 0.0, false),
        (0.25, 0.35, false),
        (0.25, 0.35, true),
        (0.25, 0.35, false),
        (0.25, 0.35, true),
    ];
    for (step, (gain, pan, muted)) in targets.into_iter().enumerate() {
        match step {
            0 => app.set_track_gain(1, 0.25).unwrap(),
            3 => app.set_track_pan(1, 0.35).unwrap(),
            6 => app.set_track_mute(1, true).unwrap(),
            1 | 4 | 7 => {
                app.undo().unwrap();
            }
            _ => {
                app.redo().unwrap();
            }
        }
        let snapshot = app.project();
        let state = &snapshot.tracks[0];
        assert_eq!(state.gain.to_bits(), gain.to_bits());
        assert_eq!(state.pan.to_bits(), pan.to_bits());
        assert_eq!(state.muted, muted);
        let track = reference.track_mut(TrackId(1)).unwrap();
        track.set_gain(gain);
        track.set_pan(pan);
        track.set_mute(muted);
        let frames = [1, 17, 127, 257][step % 4];
        let mut out = vec![0.0; frames * 2];
        let mut expected = out.clone();
        actual.process_block(&mut out);
        reference.process(&mut expected, &transport);
        transport.advance(frames);
        assert_eq!(out, expected, "audible history step {step}");
        assert_eq!(
            app.service_publications(0)
                .unwrap()
                .streaming
                .underrun_frames,
            0
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // A single held read window spans real edits/history and an independent PCM oracle.
fn readiness_wait_survives_native_undo_redo_without_false_adoption_or_timeline_progress() {
    use audio_clip::{AudioClip, PcmAsset};
    use daw_core::transport::Transport;
    use daw_mixer::{AudioGraph, Track, TrackId};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(8000).unwrap();
    app.set_track_mute(4, true).unwrap();
    app.undo.clear();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("readiness-oracle.ongaku");
    app.project.save(&path).unwrap();
    let saved: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    let snapshot = app.project();
    let mut reference = AudioGraph::new(config).unwrap();
    for state in &snapshot.tracks {
        let mut track = Track::new(TrackId(state.id));
        track.set_gain(state.gain);
        track.set_pan(state.pan);
        track.set_mute(state.muted);
        track.set_solo(state.solo);
        for clip in &state.clips {
            let data = saved["media"]
                .as_array()
                .unwrap()
                .iter()
                .find(|media| {
                    media["trackId"].as_u64() == Some(state.id)
                        && media["clipId"].as_u64() == Some(clip.id)
                })
                .unwrap();
            let samples = serde_json::from_value::<Vec<f32>>(data["samples"].clone()).unwrap();
            let channels = u16::try_from(data["channels"].as_u64().unwrap()).unwrap();
            let asset = Arc::new(PcmAsset::new(8000, channels, samples).unwrap());
            track.add_clip(
                AudioClip::new(clip.timeline_start, 0, asset.frame_count(), asset).unwrap(),
            );
        }
        reference.add_track(track).unwrap();
    }
    let graph = app.project.build_audio_graph(config).unwrap();
    let media = graph
        .tracks()
        .iter()
        .flat_map(daw_mixer::Track::streamed_clips)
        .map(|clip| Arc::clone(&clip.media))
        .collect::<Vec<_>>();
    // Retain every warm page so cold publication is deterministically blocked,
    // without depending on disk speed or thread scheduling.
    let mut blockers = Vec::new();
    for source in &media {
        let mut window = source.window();
        for frame in (0..16_000).step_by(1000) {
            window.require_frame(frame);
        }
        assert_eq!(
            window.poll().state,
            audio_types::StreamReadinessState::Ready
        );
        blockers.push(window);
    }
    let (mut actual, control) = EngineProcessor::prepare(config).unwrap();
    control.submit_graph(EngineGraph::timeline(graph)).unwrap();
    control.send_transport(TransportCommand::Play).unwrap();
    let mut transport = Transport::new(8000);
    transport.apply(TransportCommand::Play);
    let mut output = [0.0; 256];
    let mut expected = output;
    actual.process_block(&mut output);
    reference.process(&mut expected, &transport);
    transport.advance(128);
    assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
    let accepted_seek = control
        .submit_transport(TransportCommand::Seek(SamplePosition(20_001)))
        .unwrap();
    actual.process_block(&mut output);
    app.active_config = Some(config);
    app.control = Some(control);
    assert_eq!(
        app.publication_status(0)
            .stream_readiness
            .unwrap()
            .readiness
            .state,
        audio_types::StreamReadinessState::Waiting
    );
    for step in 0..6 {
        match step {
            0 => app.set_track_gain(1, 0.25).unwrap(),
            1 | 3 | 5 => {
                app.undo().unwrap();
            }
            _ => {
                app.redo().unwrap();
            }
        }
        actual.process_block(&mut output);
        let status = app.service_publications(0).unwrap();
        let admission = status.stream_readiness.unwrap();
        assert_eq!(admission.transport_command_id, accepted_seek);
        let command = app
            .transport_command_status(app.engine_epoch, accepted_seek)
            .unwrap()
            .result
            .unwrap();
        assert_eq!(command.state, audio_engine::TransportCommandState::Waiting);
        assert_eq!(command.graph_revision, admission.graph_revision);
        assert_eq!(command.applied_sample, Some(SamplePosition(20_001)));
        assert_eq!(
            status.transport_commands.unwrap().accepted_id,
            accepted_seek
        );
        assert_eq!(
            admission.graph_revision,
            status.engine.unwrap().adopted_revision
        );
        assert_eq!(
            admission.readiness.state,
            audio_types::StreamReadinessState::Waiting
        );
        assert_eq!(admission.rendered_frames, 0);
        assert_eq!(admission.position, SamplePosition(20_001));
        assert_eq!(
            app.transport().unwrap().current_sample,
            SamplePosition(20_001)
        );
        assert!(output.iter().all(|sample| *sample == 0.0));
    }
    // Apply the final history targets to the direct-PCM graph. No callback has
    // advanced its initial mixer ramp or native MIDI state during the wait.
    for state in &app.project().tracks {
        let track = reference.track_mut(TrackId(state.id)).unwrap();
        track.set_gain(state.gain);
        track.set_pan(state.pan);
        track.set_mute(state.muted);
        track.set_solo(state.solo);
    }
    drop(blockers);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        actual.process_block(&mut output);
        let state = app
            .publication_status(0)
            .stream_readiness
            .unwrap()
            .readiness
            .state;
        if state == audio_types::StreamReadinessState::Ready {
            break;
        }
        assert_eq!(state, audio_types::StreamReadinessState::Waiting);
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    transport.apply(TransportCommand::Seek(SamplePosition(20_001)));
    reference.process(&mut expected, &transport);
    assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
    let admission = app.publication_status(0).stream_readiness.unwrap();
    let command = app
        .transport_command_status(app.engine_epoch, accepted_seek)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(command.state, audio_engine::TransportCommandState::Applied);
    assert_eq!(command.graph_revision, admission.graph_revision);
    assert_eq!(command.resolved_sample, Some(SamplePosition(20_001)));
    assert_eq!(admission.rendered_frames, 128);
    assert!(admission.waiting_callbacks >= 7);
    assert_eq!(
        app.transport().unwrap().current_sample,
        SamplePosition(20_129)
    );
}

#[test]
fn stale_transport_acceptances_cannot_control_a_restarted_engine() {
    let backend = audio_backend::mock::MockAudioBackend::default();
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut app = DawApplication::with_sample_rate(8000).unwrap();
    app.start(&backend, None, config).unwrap();
    let first = app
        .submit_transport(TransportCommand::Seek(SamplePosition(100)))
        .unwrap();
    assert!(
        app.transport_command_status(first.engine_epoch, first.command_id)
            .is_ok()
    );
    app.stop().unwrap();
    assert!(
        app.transport_command_status(first.engine_epoch, first.command_id)
            .is_err()
    );
    app.start(&backend, None, config).unwrap();
    let second = app
        .submit_transport(TransportCommand::Seek(SamplePosition(200)))
        .unwrap();
    assert_eq!(first.command_id, second.command_id);
    assert!(second.engine_epoch > first.engine_epoch);
    assert!(
        app.transport_command_status(first.engine_epoch, first.command_id)
            .is_err()
    );
    assert!(
        app.cancel_transport_command(first.engine_epoch, first.command_id)
            .is_err()
    );
    assert!(
        app.transport_command_status(second.engine_epoch, second.command_id)
            .is_ok()
    );
    app.stop().unwrap();
}
