//! Prepared realtime mixer graph. Topology changes are built off the audio thread.

use audio_clip::AudioClip;
use audio_types::{AudioConfig, AudioError, RealtimeAudioSource};
use daw_core::{SamplePosition, transport::Transport};

/// A prepared source whose output is derived from the engine Transport timeline.
/// Implementations run in the audio callback and must not allocate, lock, or perform I/O.
pub trait RealtimeTimelineSource: Send + 'static {
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

    fn sample_at(&self, position: SamplePosition, channel: usize, output_channels: usize) -> f32 {
        let mut mixed = 0.0;
        for clip in &self.clips {
            mixed += clip.sample_at(position, channel);
        }
        mixed * self.gain * pan_gain(self.pan, channel, output_channels)
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
}

impl AudioGraph {
    pub fn new(config: AudioConfig) -> Result<Self, AudioError> {
        Ok(Self {
            config: config.validate()?,
            tracks: Vec::new(),
            master_gain: 1.0,
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
        let mut rendered_frames = 0;
        for block in output.chunks_mut(maximum_samples) {
            for track in &mut self.tracks {
                track.read_realtime_source(block.len());
                track.render_timeline_source(block.len(), transport, rendered_frames);
            }
            for (frame_offset, frame) in block.chunks_exact_mut(channels).enumerate() {
                let position = transport.sample_at_offset(rendered_frames + frame_offset);
                for track in &self.tracks {
                    if track.mute || (any_solo && !track.solo) {
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
                        *sample += track.sample_at(position, channel, channels)
                            + generated * track.gain * pan_gain(track.pan, channel, channels);
                    }
                }
                for sample in frame {
                    let mastered = *sample * self.master_gain;
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
