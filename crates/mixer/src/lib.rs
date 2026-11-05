//! Prepared realtime mixer graph. Topology changes are built off the audio thread.

mod events;
use audio_clip::AudioClip;
use audio_types::{AudioConfig, AudioError, RealtimeAudioSource};
use daw_core::{SamplePosition, transport::Transport};
use events::{Controls, PreparedMixerEvents};
pub use events::{MAX_MIXER_EVENTS_PER_TRACK, MixerChange, MixerEvent, validate_mixer_events};

/// A prepared source whose output is derived from the engine Transport timeline.
/// Implementations run in the audio callback and must not allocate, lock, or perform I/O.
pub trait RealtimeTimelineSource: Send + std::any::Any + 'static {
    /// Copy compatible runtime state at publication; never allocate, drop resources, or block.
    /// The prepared schedule/parameters remain owned by the new source.
    fn inherit_state(&mut self, _previous: &dyn RealtimeTimelineSource, _position: SamplePosition) {
    }

    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn render_interleaved(
        &mut self,
        output: &mut [f32],
        transport: &Transport,
        frame_offset: usize,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackId(pub u64);

/// Five-millisecond linear ramps of the effective channel coefficients.
/// Runtime only: document/history store targets, never an old ramp cursor.
#[derive(Clone, Copy)]
struct MixRamp {
    current: [f32; 3],
    target: [f32; 3],
    step: [f32; 3],
    remaining: u32,
    initialized: bool,
}

impl MixRamp {
    const fn new() -> Self {
        Self {
            current: [0.0; 3],
            target: [0.0; 3],
            step: [0.0; 3],
            remaining: 0,
            initialized: false,
        }
    }

    #[allow(clippy::float_cmp)] // Any finite target change, including sub-epsilon edits, matters.
    fn retarget(&mut self, target: [f32; 3], samples: u32) {
        if !self.initialized {
            self.current = target;
            self.target = target;
            self.initialized = true;
        } else if self.target != target {
            self.target = target;
            self.remaining = samples;
            #[allow(clippy::cast_precision_loss)]
            for (index, value) in target.iter().enumerate() {
                self.step[index] = (*value - self.current[index]) / samples as f32;
            }
        }
    }

    fn advance(&mut self) {
        if self.remaining == 0 {
            return;
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.current = self.target;
        } else {
            for (value, step) in self.current.iter_mut().zip(self.step) {
                *value += step;
            }
        }
    }
}

fn ramp_samples(sample_rate: u32) -> u32 {
    (sample_rate / 200 + u32::from(sample_rate % 200 >= 100)).max(1)
}

pub struct Track {
    id: TrackId,
    clips: Vec<AudioClip>,
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    realtime_source: Option<PreparedRealtimeSource>,
    timeline_source: Option<PreparedTimelineSource>,
    prepared_config: Option<AudioConfig>,
    mix_ramp: MixRamp,
    mixer_events: PreparedMixerEvents,
    frame_controls: Controls,
}

struct PreparedRealtimeSource {
    source: Box<dyn RealtimeAudioSource>,
    scratch: Vec<f32>,
}

struct PreparedTimelineSource {
    source: Box<dyn RealtimeTimelineSource>,
    scratch: Vec<f32>,
}

impl Track {
    #[must_use]
    pub const fn new(id: TrackId) -> Self {
        Self {
            id,
            clips: Vec::new(),
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            realtime_source: None,
            timeline_source: None,
            prepared_config: None,
            mix_ramp: MixRamp::new(),
            mixer_events: PreparedMixerEvents::new(),
            frame_controls: Controls::UNITY,
        }
    }

    #[must_use]
    pub const fn id(&self) -> TrackId {
        self.id
    }

    pub fn add_clip(&mut self, clip: AudioClip) {
        self.clips.push(clip);
    }

    #[must_use]
    pub fn clips(&self) -> &[AudioClip] {
        &self.clips
    }

    pub fn remove_clip(&mut self, index: usize) -> Option<AudioClip> {
        (index < self.clips.len()).then(|| self.clips.remove(index))
    }

    /// Attaches a pull source such as a file-streaming ring-buffer consumer.
    /// Scratch storage is allocated here for a prepared track, or when it is added to a graph.
    pub fn set_realtime_source(
        &mut self,
        source: Box<dyn RealtimeAudioSource>,
    ) -> Result<(), AudioError> {
        let mut prepared = PreparedRealtimeSource {
            source,
            scratch: Vec::new(),
        };
        if let Some(config) = self.prepared_config {
            prepare_source(&mut prepared, config)?;
        }
        self.realtime_source = Some(prepared);
        Ok(())
    }

    pub fn clear_realtime_source(&mut self) {
        self.realtime_source = None;
    }

    /// Attaches a Transport-aware source such as a MIDI instrument or metronome.
    pub fn set_timeline_source(
        &mut self,
        source: Box<dyn RealtimeTimelineSource>,
    ) -> Result<(), AudioError> {
        let mut prepared = PreparedTimelineSource {
            source,
            scratch: Vec::new(),
        };
        if let Some(config) = self.prepared_config {
            prepare_timeline_source(&mut prepared, config)?;
        }
        self.timeline_source = Some(prepared);
        Ok(())
    }

    pub fn clear_timeline_source(&mut self) {
        self.timeline_source = None;
    }

    /// Prepared off the audio thread; latest (position, ID) wins per parameter.
    pub fn set_mixer_events(&mut self, events: &[MixerEvent]) -> Result<(), AudioError> {
        self.mixer_events = PreparedMixerEvents::prepare(events)?;
        Ok(())
    }

    fn base_controls(&self) -> Controls {
        Controls {
            gain: self.gain,
            pan: self.pan,
            mute: self.mute,
            solo: self.solo,
        }
    }

    fn retarget_mix(&mut self, any_solo: bool, channels: usize, ramp_length: u32) {
        let controls = self.frame_controls;
        let gain = if controls.mute || (any_solo && !controls.solo) {
            0.0
        } else {
            controls.gain
        };
        self.mix_ramp.retarget(
            [
                gain * pan_gain(controls.pan, 0, channels),
                gain * pan_gain(controls.pan, 1, channels),
                gain,
            ],
            ramp_length,
        );
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            0.0
        };
    }

    pub fn set_pan(&mut self, pan: f32) {
        self.pan = if pan.is_finite() {
            pan.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }

    pub const fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
    }
    pub const fn set_solo(&mut self, solo: bool) {
        self.solo = solo;
    }

    #[must_use]
    pub const fn gain(&self) -> f32 {
        self.gain
    }

    #[must_use]
    pub const fn pan(&self) -> f32 {
        self.pan
    }

    #[must_use]
    pub const fn is_muted(&self) -> bool {
        self.mute
    }

    #[must_use]
    pub const fn is_solo(&self) -> bool {
        self.solo
    }

    fn sample_at(&self, position: SamplePosition, channel: usize) -> f32 {
        let mut mixed = 0.0;
        for clip in &self.clips {
            mixed += clip.sample_at(position, channel);
        }
        mixed
    }

    fn prepare(&mut self, config: AudioConfig) -> Result<(), AudioError> {
        self.prepared_config = Some(config);
        if let Some(source) = &mut self.realtime_source {
            prepare_source(source, config)?;
        }
        if let Some(source) = &mut self.timeline_source {
            prepare_timeline_source(source, config)?;
        }
        Ok(())
    }

    fn read_realtime_source(&mut self, sample_count: usize) {
        let Some(source) = &mut self.realtime_source else {
            return;
        };
        let Some(scratch) = source.scratch.get_mut(..sample_count) else {
            return;
        };
        let written = source.source.read_interleaved(scratch).min(sample_count);
        scratch[written..].fill(0.0);
    }

    fn realtime_sample(&self, sample_index: usize) -> f32 {
        self.realtime_source
            .as_ref()
            .and_then(|source| source.scratch.get(sample_index))
            .copied()
            .unwrap_or(0.0)
    }

    fn render_timeline_source(
        &mut self,
        sample_count: usize,
        transport: &Transport,
        frame_offset: usize,
    ) {
        let Some(source) = &mut self.timeline_source else {
            return;
        };
        let Some(scratch) = source.scratch.get_mut(..sample_count) else {
            return;
        };
        source
            .source
            .render_interleaved(scratch, transport, frame_offset);
    }

    fn timeline_sample(&self, sample_index: usize) -> f32 {
        self.timeline_source
            .as_ref()
            .and_then(|source| source.scratch.get(sample_index))
            .copied()
            .unwrap_or(0.0)
    }
}

fn prepare_source(
    source: &mut PreparedRealtimeSource,
    config: AudioConfig,
) -> Result<(), AudioError> {
    if source.source.sample_rate() != config.sample_rate
        || source.source.channels() != config.channels
    {
        return Err(AudioError::InvalidConfig(
            "realtime source format must match the prepared graph",
        ));
    }
    let samples = usize::try_from(config.buffer_size)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(config.channels));
    source.scratch.resize(samples, 0.0);
    Ok(())
}

fn prepare_timeline_source(
    source: &mut PreparedTimelineSource,
    config: AudioConfig,
) -> Result<(), AudioError> {
    if source.source.sample_rate() != config.sample_rate
        || source.source.channels() != config.channels
    {
        return Err(AudioError::InvalidConfig(
            "timeline source format must match the prepared graph",
        ));
    }
    let samples = usize::try_from(config.buffer_size)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(config.channels));
    source.scratch.resize(samples, 0.0);
    Ok(())
}

fn pan_gain(pan: f32, channel: usize, output_channels: usize) -> f32 {
    if output_channels < 2 {
        return 1.0;
    }
    match channel {
        0 => (1.0 - pan).min(1.0),
        1 => (1.0 + pan).min(1.0),
        _ => 1.0,
    }
}

pub struct AudioGraph {
    config: AudioConfig,
    tracks: Vec<Track>,
    master_gain: f32,
    master_ramp: MixRamp,
}

impl AudioGraph {
    pub fn new(config: AudioConfig) -> Result<Self, AudioError> {
        Ok(Self {
            config: config.validate()?,
            tracks: Vec::new(),
            master_gain: 1.0,
            master_ramp: MixRamp::new(),
        })
    }

    pub fn add_track(&mut self, mut track: Track) -> Result<(), AudioError> {
        if self.tracks.iter().any(|existing| existing.id == track.id) {
            return Err(AudioError::InvalidConfig("track id must be unique"));
        }
        if track
            .clips
            .iter()
            .any(|clip| clip.asset.sample_rate() != self.config.sample_rate)
        {
            return Err(AudioError::InvalidConfig(
                "clip sample rate must match the prepared graph",
            ));
        }
        track.prepare(self.config)?;
        self.tracks.push(track);
        Ok(())
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        self.master_gain = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            0.0
        };
    }

    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    #[must_use]
    pub const fn config(&self) -> AudioConfig {
        self.config
    }

    #[must_use]
    pub const fn master_gain(&self) -> f32 {
        self.master_gain
    }

    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.id == id)
    }

    pub fn remove_track(&mut self, id: TrackId) -> Option<Track> {
        self.tracks
            .iter()
            .position(|track| track.id == id)
            .map(|index| self.tracks.remove(index))
    }

    /// Compare the final prepared schedule to the graph actually sounding, without allocation.
    #[must_use]
    pub fn mixer_edit_deadline(&self, previous: &Self) -> Option<SamplePosition> {
        let mut deadline = None;
        for track in &self.tracks {
            let changed = previous
                .tracks
                .iter()
                .find(|old| old.id == track.id)
                .map_or_else(
                    || track.mixer_events.first_position(),
                    |old| track.mixer_events.deadline_vs(&old.mixer_events),
                );
            if let Some(position) = changed {
                deadline = Some(
                    deadline.map_or(position, |current: SamplePosition| current.min(position)),
                );
            }
        }
        for old in &previous.tracks {
            if !self.tracks.iter().any(|track| track.id == old.id)
                && let Some(position) = old.mixer_events.first_position()
            {
                deadline = Some(deadline.map_or(position, |current| current.min(position)));
            }
        }
        deadline
    }

    /// Match stable track IDs, independent of ordering. Unknown source types keep fresh state.
    /// Pull streams are intentionally not transplanted: they need an explicit media/generation contract.
    pub fn inherit_state(&mut self, previous: &Self, position: SamplePosition) {
        if self.config != previous.config {
            return;
        }
        self.master_ramp = previous.master_ramp;
        for track in &mut self.tracks {
            if let Some(old) = previous.tracks.iter().find(|old| old.id == track.id) {
                track.mix_ramp = old.mix_ramp;
                if let (Some(next), Some(old)) = (&mut track.timeline_source, &old.timeline_source)
                {
                    next.source.inherit_state(old.source.as_ref(), position);
                }
            } else {
                // A newly inserted track fades in instead of appearing at full level.
                track.mix_ramp.initialized = true;
            }
        }
    }

    /// Renders the prepared topology without allocation, locking, or I/O.
    pub fn process(&mut self, output: &mut [f32], transport: &Transport) {
        output.fill(0.0);
        if !transport.snapshot().playing {
            return;
        }
        let channels = usize::from(self.config.channels);
        if channels == 0 {
            return;
        }
        let maximum_samples = usize::try_from(self.config.buffer_size)
            .unwrap_or(usize::MAX)
            .saturating_mul(channels);
        let any_solo = self.tracks.iter().any(|track| track.solo);
        let ramp_length = ramp_samples(self.config.sample_rate);
        self.master_ramp
            .retarget([self.master_gain; 3], ramp_length);
        for track in &mut self.tracks {
            if !track.mix_ramp.initialized {
                // Establish the base level first, so an event at sample zero also ramps.
                track.frame_controls = track.base_controls();
                track.retarget_mix(any_solo, channels, ramp_length);
            }
        }
        let mut rendered_frames = 0;
        for block in output.chunks_mut(maximum_samples) {
            for track in &mut self.tracks {
                track.read_realtime_source(block.len());
                track.render_timeline_source(block.len(), transport, rendered_frames);
            }
            for (frame_offset, frame) in block.chunks_exact_mut(channels).enumerate() {
                let position = transport.sample_at_offset(rendered_frames + frame_offset);
                // Evaluate every Solo target before retargeting any channel: track order cannot
                // change same-sample exclusion. Binary searches also chase on backward seeks/loops.
                for track in &mut self.tracks {
                    track.frame_controls = track.mixer_events.at(position, track.base_controls());
                }
                let any_solo = self.tracks.iter().any(|track| track.frame_controls.solo);
                for track in &mut self.tracks {
                    track.retarget_mix(any_solo, channels, ramp_length);
                    track.mix_ramp.advance();
                    if track.mix_ramp.current.iter().all(|gain| *gain == 0.0) {
                        // Sources above still advance; settled silence needs no PCM summing.
                        continue;
                    }
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        let sample_index = frame_offset * channels + channel;
                        let generated = track.realtime_sample(sample_index)
                            + track.timeline_sample(sample_index);
                        let generated = if generated.is_finite() {
                            generated
                        } else {
                            0.0
                        };
                        let coefficient = track.mix_ramp.current[channel.min(2)];
                        *sample += track.sample_at(position, channel) * coefficient
                            + generated * coefficient;
                    }
                }
                self.master_ramp.advance();
                for sample in frame {
                    let mastered = *sample * self.master_ramp.current[0];
                    *sample = if mastered.is_nan() {
                        0.0
                    } else {
                        mastered.clamp(-1.0, 1.0)
                    };
                }
            }
            rendered_frames += block.len() / channels;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioGraph, Track, TrackId};
    use audio_clip::{AudioClip, PcmAsset};
    use audio_types::{AudioConfig, RealtimeAudioSource};
    use daw_core::{
        SamplePosition,
        transport::{Transport, TransportCommand},
    };
    use std::sync::Arc;

    fn clip(samples: Vec<f32>, start: u64) -> AudioClip {
        let asset = Arc::new(PcmAsset::new(48_000, 1, samples).unwrap());
        AudioClip::new(SamplePosition(start), 0, asset.frame_count(), asset).unwrap()
    }

    #[test]
    fn multiple_tracks_render_in_sample_sync() {
        let mut graph = AudioGraph::new(AudioConfig::DEFAULT).unwrap();
        let mut first = Track::new(TrackId(1));
        first.add_clip(clip(vec![0.1, 0.2], 0));
        let mut second = Track::new(TrackId(2));
        second.add_clip(clip(vec![0.3, 0.4], 0));
        graph.add_track(first).unwrap();
        graph.add_track(second).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 4];
        graph.process(&mut output, &transport);
        let expected = [0.4, 0.4, 0.6, 0.6];
        assert!(
            output
                .iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 1.0e-6)
        );
    }

    #[test]
    fn solo_excludes_non_solo_tracks() {
        let mut graph = AudioGraph::new(AudioConfig::DEFAULT).unwrap();
        let mut first = Track::new(TrackId(1));
        first.add_clip(clip(vec![0.25], 0));
        first.set_solo(true);
        let mut second = Track::new(TrackId(2));
        second.add_clip(clip(vec![0.5], 0));
        graph.add_track(first).unwrap();
        graph.add_track(second).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 2];
        graph.process(&mut output, &transport);
        assert!(
            output
                .iter()
                .all(|sample| (*sample - 0.25).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn mute_silences_a_track() {
        let mut graph = AudioGraph::new(AudioConfig::DEFAULT).unwrap();
        let mut track = Track::new(TrackId(1));
        track.add_clip(clip(vec![0.5], 0));
        track.set_mute(true);
        graph.add_track(track).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Play);
        let mut output = [1.0; 2];
        graph.process(&mut output, &transport);
        assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
    }

    struct FixedSource {
        samples: Vec<f32>,
        cursor: usize,
    }

    impl RealtimeAudioSource for FixedSource {
        fn sample_rate(&self) -> u32 {
            48_000
        }

        fn channels(&self) -> u16 {
            2
        }

        fn read_interleaved(&mut self, output: &mut [f32]) -> usize {
            let count = output
                .len()
                .min(self.samples.len().saturating_sub(self.cursor));
            output[..count].copy_from_slice(&self.samples[self.cursor..self.cursor + count]);
            self.cursor += count;
            count
        }
    }

    #[test]
    fn realtime_source_is_mixed_through_a_track() {
        let mut graph = AudioGraph::new(AudioConfig::DEFAULT).unwrap();
        let mut track = Track::new(TrackId(1));
        track
            .set_realtime_source(Box::new(FixedSource {
                samples: vec![0.2, 0.4, 0.6, 0.8],
                cursor: 0,
            }))
            .unwrap();
        track.set_gain(0.5);
        graph.add_track(track).unwrap();
        let mut transport = Transport::new(48_000);
        transport.apply(TransportCommand::Play);
        let mut output = [0.0; 4];
        graph.process(&mut output, &transport);
        let expected = [0.1, 0.2, 0.3, 0.4];
        assert!(
            output
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() < 1.0e-6)
        );
    }
}

#[cfg(test)]
mod ramp_tests {
    use super::*;
    use audio_clip::PcmAsset;
    use daw_core::transport::TransportCommand;
    use std::sync::Arc;

    fn graph(config: AudioConfig) -> AudioGraph {
        let mut graph = AudioGraph::new(config).unwrap();
        for (id, value) in [(1, 0.25), (2, 0.1)] {
            let mut track = Track::new(TrackId(id));
            let asset = Arc::new(
                PcmAsset::new(
                    config.sample_rate,
                    config.channels,
                    vec![value; 2000 * usize::from(config.channels)],
                )
                .unwrap(),
            );
            track.add_clip(AudioClip::new(SamplePosition::ZERO, 0, 2000, asset).unwrap());
            graph.add_track(track).unwrap();
        }
        graph
    }

    fn edit(graph: &mut AudioGraph, stage: usize) {
        graph.track_mut(TrackId(1)).unwrap().set_gain(0.4);
        if stage >= 1 {
            graph.track_mut(TrackId(1)).unwrap().set_pan(0.8);
        }
        graph.track_mut(TrackId(1)).unwrap().set_mute(stage == 2);
        graph.track_mut(TrackId(2)).unwrap().set_solo(stage == 4);
        if stage >= 5 {
            graph.set_master_gain(0.3);
        }
    }

    fn render(partitions: &[usize], rebuild: bool) -> Vec<f32> {
        let config = AudioConfig::DEFAULT;
        let mut graph = graph(config);
        let mut transport = Transport::new(config.sample_rate);
        transport.apply(TransportCommand::Play);
        graph.process(&mut [0.0; 2], &transport);
        transport.advance(1);
        let mut output = Vec::new();
        for (stage, frames) in [100, 45, 55, 30, 20, 500, 500].into_iter().enumerate() {
            if rebuild {
                let mut next = self::graph(config);
                edit(&mut next, stage);
                next.inherit_state(&graph, transport.snapshot().current_sample);
                graph = next;
            } else {
                edit(&mut graph, stage);
            }
            let mut rendered = 0;
            let mut block = 0;
            while rendered < frames {
                let count = partitions[block % partitions.len()].min(frames - rendered);
                let mut samples = vec![0.0; count * 2];
                graph.process(&mut samples, &transport);
                transport.advance(count);
                output.extend(samples);
                rendered += count;
                block += 1;
                if rebuild {
                    // Unrelated publications during a ramp must not restart its deadline.
                    let mut next = self::graph(config);
                    edit(&mut next, stage);
                    next.inherit_state(&graph, transport.snapshot().current_sample);
                    graph = next;
                }
            }
        }
        output
    }

    #[test]
    fn mixer_ramps_are_partition_invariant_and_survive_publication() {
        let expected = render(&[1024], false);
        for partitions in [&[1][..], &[17, 64, 127, 128, 257, 512][..]] {
            for rebuild in [false, true] {
                assert_eq!(render(partitions, rebuild), expected);
            }
        }
        // Constant sources isolate control-induced discontinuities from waveform slope.
        for channel in 0..2 {
            let mut previous = 0.35_f32;
            for frame in expected.as_chunks::<2>().0 {
                assert!((frame[channel] - previous).abs() < 0.003);
                previous = frame[channel];
            }
        }
    }

    #[test]
    fn mute_reaches_zero_after_exactly_five_ms_for_each_sample_rate() {
        for (rate, frames) in [(1, 1), (8000, 40), (44100, 221), (48000, 240), (96000, 480)] {
            for channels in [1, 2, 4] {
                let config = AudioConfig {
                    sample_rate: rate,
                    channels,
                    ..AudioConfig::DEFAULT
                };
                let mut graph = graph(config);
                graph.track_mut(TrackId(2)).unwrap().set_mute(true);
                let mut transport = Transport::new(rate);
                transport.apply(TransportCommand::Play);
                graph.process(&mut vec![0.0; usize::from(channels)], &transport);
                transport.advance(1);
                graph.track_mut(TrackId(1)).unwrap().set_mute(true);
                assert_eq!(usize::try_from(ramp_samples(rate)).unwrap(), frames);
                let mut output = vec![0.0; (frames + 1) * usize::from(channels)];
                graph.process(&mut output, &transport);
                assert!(
                    output[(frames - 1) * usize::from(channels)..]
                        .iter()
                        .all(|value| *value == 0.0)
                );
                if frames > 1 {
                    assert!(output[(frames - 2) * usize::from(channels)] > 0.0);
                }
                for frame in output.chunks_exact(usize::from(channels)) {
                    assert!(
                        frame
                            .iter()
                            .all(|value| value.to_bits() == frame[0].to_bits())
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod ramp_retarget_test {
    use super::MixRamp;

    #[test]
    fn retarget_keeps_current_level_and_unchanged_targets_keep_deadline() {
        let mut ramp = MixRamp::new();
        ramp.retarget([1.0; 3], 240);
        ramp.retarget([0.0; 3], 240);
        for _ in 0..100 {
            ramp.advance();
        }
        let current = ramp.current;
        ramp.retarget([0.0; 3], 240);
        assert_eq!(ramp.remaining, 140);
        assert_eq!(ramp.current.map(f32::to_bits), current.map(f32::to_bits));
        ramp.retarget([0.75; 3], 240);
        assert_eq!(ramp.remaining, 240);
        assert_eq!(ramp.current.map(f32::to_bits), current.map(f32::to_bits));
        for _ in 0..240 {
            ramp.advance();
        }
        assert_eq!(ramp.current.map(f32::to_bits), [0.75; 3].map(f32::to_bits));
        assert_eq!(ramp.remaining, 0);
    }
}

#[cfg(test)]
mod events_tests;
