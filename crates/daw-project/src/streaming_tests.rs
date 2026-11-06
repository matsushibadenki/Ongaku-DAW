use super::*;
use audio_clip::AudioClip;
use daw_core::{
    SampleRange,
    transport::{Transport, TransportCommand},
};

fn fixture() -> Project {
    let mut project = Project::demo(8000).unwrap();
    project.tracks.truncate(1);
    let track = &mut project.tracks[0];
    track.gain = 1.0;
    track.pan = 0.0;
    let clip = &mut track.clips[0];
    let samples = (0..2000_u16)
        .flat_map(|n| {
            let value = f32::from(n + 1) / 4096.0;
            [value, -value]
        })
        .collect();
    clip.asset = Arc::new(PcmAsset::new(8000, 2, samples).unwrap());
    clip.timeline_start = SamplePosition(5);
    clip.stream = streaming::MediaCache::default();
    clip.waveform = WaveformPyramid::analyze(&clip.asset, 32).unwrap();
    project
}
fn pcm_reference(project: &Project, config: AudioConfig) -> AudioGraph {
    let mut graph = AudioGraph::new(config).unwrap();
    for pt in &project.tracks {
        let mut track = Track::new(pt.id);
        track.set_gain(pt.gain);
        track.set_pan(pt.pan);
        track.set_mute(pt.muted);
        track.set_solo(pt.solo);
        for clip in &pt.clips {
            track.add_clip(
                AudioClip::new(
                    clip.timeline_start,
                    0,
                    clip.asset.frame_count(),
                    Arc::clone(&clip.asset),
                )
                .unwrap(),
            );
        }
        graph.add_track(track).unwrap();
    }
    graph
}
#[test]
fn project_streaming_matches_pcm_through_seek_loop_publication_and_history_snapshots() {
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let original = fixture();
    let mut edited = original.clone();
    edited.tracks[0].gain = 0.4;
    let mut actual = original.build_audio_graph(config).unwrap();
    let mut expected = pcm_reference(&original, config);
    let initial = Arc::clone(&actual.tracks().first().unwrap().streamed_clips()[0].media);
    let mut transport = Transport::new(8000);
    transport.apply(TransportCommand::Play);
    for step in 0..40 {
        let document = if step % 3 == 1 { &edited } else { &original }; // edit, undo, redo snapshots
        let mut next = document.build_audio_graph(config).unwrap();
        assert!(Arc::ptr_eq(
            &initial,
            &next.tracks().first().unwrap().streamed_clips()[0].media
        ));
        assert_eq!(initial.snapshot().page_reads, 2); // Entire 2000-frame fixture is already warm.
        next.inherit_state(&actual, transport.snapshot().current_sample);
        actual = next;
        let mut next = pcm_reference(document, config);
        next.inherit_state(&expected, transport.snapshot().current_sample);
        expected = next;
        match step % 5 {
            0 => transport.apply(TransportCommand::Seek(SamplePosition(0))),
            1 => transport.apply(TransportCommand::Seek(SamplePosition(1999))),
            2 => transport.apply(TransportCommand::SetLoop(SampleRange::new(
                SamplePosition(7),
                SamplePosition(24),
            ))),
            3 => transport.apply(TransportCommand::SetLoop(None)),
            _ => transport.apply(TransportCommand::Seek(SamplePosition(2004))),
        }
        let frames = [1, 17, 128, 257, 513][step % 5];
        let mut out = vec![0.0; frames * 2];
        let mut reference = out.clone();
        actual.process(&mut out, &transport);
        expected.process(&mut reference, &transport);
        assert_eq!(out, reference, "step {step}");
        transport.advance(frames);
    }
    assert_eq!(initial.snapshot().underrun_frames, 0);
}
#[test]
fn loaded_project_gets_fresh_media_identity_with_identical_pcm() {
    let project = fixture();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stream.ongaku");
    project.save(&path).unwrap();
    let loaded = Project::load(&path).unwrap();
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut before = project.build_audio_graph(config).unwrap();
    let mut after = loaded.build_audio_graph(config).unwrap();
    assert!(!Arc::ptr_eq(
        &before.tracks().first().unwrap().streamed_clips()[0].media,
        &after.tracks().first().unwrap().streamed_clips()[0].media
    ));
    let mut transport = Transport::new(8000);
    transport.apply(TransportCommand::Seek(SamplePosition(120)));
    transport.apply(TransportCommand::Play);
    let mut a = [0.0; 256];
    let mut b = a;
    before.process(&mut a, &transport);
    after.process(&mut b, &transport);
    assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
}

#[test]
fn cold_project_position_is_silent_or_exact_and_recovers_at_the_current_frame() {
    let mut project = fixture();
    let clip = &mut project.tracks[0].clips[0];
    let samples = (0..20_000_u16)
        .flat_map(|n| {
            let value = f32::from(n % 1000 + 1) / 2048.0;
            [value, -value]
        })
        .collect();
    clip.asset = Arc::new(PcmAsset::new(8000, 2, samples).unwrap());
    clip.stream = streaming::MediaCache::default();
    let config = AudioConfig {
        sample_rate: 8000,
        ..AudioConfig::DEFAULT
    };
    let mut graph = project.build_audio_graph(config).unwrap();
    let mut reference = pcm_reference(&project, config);
    let media = Arc::clone(&graph.tracks()[0].streamed_clips()[0].media);
    let mut transport = Transport::new(8000);
    transport.apply(TransportCommand::Seek(SamplePosition(19_005)));
    transport.apply(TransportCommand::Play);
    let mut output = [0.0; 2];
    let mut expected = output;
    graph.process(&mut output, &transport);
    reference.process(&mut expected, &transport);
    // Scheduling may or may not let disk I/O finish before the first frame.
    assert!(
        output.map(f32::to_bits) == [0.0; 2].map(f32::to_bits)
            || output.map(f32::to_bits) == expected.map(f32::to_bits)
    );
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !media.read_frame(19_007, &mut output) {
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    }
    transport.apply(TransportCommand::Seek(SamplePosition(19_012)));
    graph.process(&mut output, &transport);
    reference.process(&mut expected, &transport);
    assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
    // The completed page is addressed at 19007, never replayed from the old 19000 request.
    assert!(output[0] > 0.0 && output[1] < 0.0);
}
