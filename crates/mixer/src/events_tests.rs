use super::*;
use audio_clip::PcmAsset;
use daw_core::transport::TransportCommand;
use std::sync::Arc;

fn event(id: u64, position: u64, change: MixerChange) -> MixerEvent {
    MixerEvent {
        id,
        position: SamplePosition(position),
        change,
    }
}
fn graph(events: &[MixerEvent]) -> AudioGraph {
    let config = AudioConfig::DEFAULT;
    let mut graph = AudioGraph::new(config).unwrap();
    let mut track = Track::new(TrackId(1));
    track.add_clip(
        AudioClip::new(
            SamplePosition::ZERO,
            0,
            4096,
            Arc::new(PcmAsset::new(config.sample_rate, 2, vec![0.25; 8192]).unwrap()),
        )
        .unwrap(),
    );
    track.set_mixer_events(events).unwrap();
    graph.add_track(track).unwrap();
    graph
}
fn playing() -> Transport {
    let mut transport = Transport::new(48000);
    transport.apply(TransportCommand::Play);
    transport
}

#[test]
fn events_begin_the_ramp_on_the_exact_sample_including_zero_and_block_edges() {
    for position in [0, 1, 127, 128, 129, 511] {
        for change in [
            MixerChange::Gain(0.0),
            MixerChange::Pan(1.0),
            MixerChange::Mute(true),
        ] {
            // A repeated target must not extend the first event's ramp deadline.
            let mut graph = graph(&[event(1, position, change), event(2, position + 10, change)]);
            let position = usize::try_from(position).unwrap();
            let mut output = vec![0.0; (position + 242) * 2];
            graph.process(&mut output, &playing());
            assert!(
                output[..position * 2]
                    .iter()
                    .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
            );
            assert!((output[position * 2] - 0.25 * (1.0 - 1.0 / 240.0)).abs() < 1e-7);
            assert!(
                output.as_chunks::<2>().0[position + 239..]
                    .iter()
                    .all(|frame| frame[0] == 0.0)
            );
            if matches!(change, MixerChange::Pan(_)) {
                assert!(
                    output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .all(|frame| frame[1].to_bits() == 0.25_f32.to_bits())
                );
            }
        }
    }
}

#[test]
fn same_sample_ids_and_solo_targets_are_independent_of_track_order() {
    for reverse in [false, true] {
        let mut graph = graph(&[
            event(1, 17, MixerChange::Solo(true)),
            event(9, 17, MixerChange::Solo(false)),
        ]);
        let mut other = Track::new(TrackId(2));
        other
            .set_mixer_events(&[event(3, 17, MixerChange::Solo(true))])
            .unwrap();
        graph.add_track(other).unwrap();
        if reverse {
            graph.tracks.reverse();
        }
        let mut output = [0.0; 40];
        graph.process(&mut output, &playing());
        assert_eq!(output[32].to_bits(), 0.25_f32.to_bits());
        assert!((output[34] - 0.25 * (1.0 - 1.0 / 240.0)).abs() < 1e-7);
    }
}

fn scheduled_graph() -> AudioGraph {
    graph(&[
        event(1, 17, MixerChange::Gain(0.3)),
        event(2, 64, MixerChange::Pan(-0.8)),
        event(3, 129, MixerChange::Mute(true)),
        event(4, 161, MixerChange::Mute(false)),
        event(5, 200, MixerChange::Gain(0.8)),
    ])
}
fn render(partitions: &[usize], rebuild: bool, looping: bool) -> Vec<f32> {
    let mut graph = scheduled_graph();
    let mut transport = playing();
    if looping {
        transport.apply(TransportCommand::SetLoop(daw_core::SampleRange::new(
            SamplePosition(7),
            SamplePosition(211),
        )));
    }
    let mut result = vec![0.0; 4096];
    let mut rendered = 0;
    let mut block = 0;
    while rendered < 2048 {
        let count = partitions[block % partitions.len()].min(2048 - rendered);
        graph.process(
            &mut result[rendered * 2..(rendered + count) * 2],
            &transport,
        );
        transport.advance(count);
        if rebuild {
            let mut next = scheduled_graph();
            next.inherit_state(&graph, transport.snapshot().current_sample);
            graph = next;
        }
        rendered += count;
        block += 1;
    }
    result
}

#[test]
fn sample_events_are_partition_invariant_through_loops_and_publication() {
    for looping in [false, true] {
        let expected = render(&[2048], false, looping);
        for blocks in [&[1][..], &[17, 64, 127, 128, 257, 512][..]] {
            for rebuild in [false, true] {
                assert_eq!(render(blocks, rebuild, looping), expected);
            }
        }
    }
}

#[test]
fn backward_seek_chases_the_destination_instead_of_leaving_a_stale_mute() {
    let mut graph = graph(&[event(1, 10, MixerChange::Mute(true))]);
    let mut transport = playing();
    graph.process(&mut [0.0; 1024], &transport);
    transport.advance(512);
    transport.apply(TransportCommand::Seek(SamplePosition::ZERO));
    let mut output = [0.0; 2];
    graph.process(&mut output, &transport);
    assert!((output[0] - 0.25 / 240.0).abs() < 1e-7);
}

#[test]
fn event_validation_is_bounded_and_rejects_nonfinite_values_and_duplicate_ids() {
    for change in [
        MixerChange::Gain(f32::NAN),
        MixerChange::Gain(2.1),
        MixerChange::Pan(-1.1),
    ] {
        assert!(validate_mixer_events(&[event(1, 0, change)]).is_err());
    }
    assert!(validate_mixer_events(&[event(1, 0, MixerChange::Mute(true)); 2]).is_err());
    let events: Vec<_> = (0..=4096)
        .map(|id| event(id, id, MixerChange::Mute(false)))
        .collect();
    assert!(validate_mixer_events(&events).is_err());
}
