//! Pure selection against CPAL's advertised ranges; no implicit resampling or channel conversion.
use audio_types::{AudioConfig, AudioError, OutputBufferRange, OutputSampleFormat};
use cpal::{SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub format: OutputSampleFormat,
    pub buffer_range: Option<OutputBufferRange>,
}
fn format(value: SampleFormat) -> Option<OutputSampleFormat> {
    match value {
        SampleFormat::F32 => Some(OutputSampleFormat::F32),
        SampleFormat::I16 => Some(OutputSampleFormat::I16),
        SampleFormat::U16 => Some(OutputSampleFormat::U16),
        _ => None,
    }
}
fn rank(selection: Selection) -> (bool, u8, u32, u32) {
    (
        selection.buffer_range.is_none(),
        match selection.format {
            OutputSampleFormat::F32 => 0,
            OutputSampleFormat::I16 => 1,
            OutputSampleFormat::U16 => 2,
        },
        selection
            .buffer_range
            .map_or(u32::MAX, |range| range.max - range.min),
        selection.buffer_range.map_or(0, |range| range.min),
    )
}
pub(super) fn select(
    ranges: &[SupportedStreamConfigRange],
    requested: AudioConfig,
) -> Result<Selection, AudioError> {
    requested.validate()?;
    ranges.iter().filter_map(|range| {
        if range.channels() != requested.channels || requested.sample_rate < range.min_sample_rate() || requested.sample_rate > range.max_sample_rate() { return None; }
        let format = format(range.sample_format())?;
        let buffer_range = match *range.buffer_size() {
            SupportedBufferSize::Range { min, max } if min <= max && (min..=max).contains(&requested.buffer_size) => Some(OutputBufferRange { min, max }),
            SupportedBufferSize::Unknown => None,
            SupportedBufferSize::Range { .. } => return None,
        };
        Some(Selection { format, buffer_range })
    }).min_by_key(|selection| rank(*selection)).ok_or(AudioError::InvalidConfig("output device does not advertise the requested rate, channels, buffer and supported sample format"))
}

pub(super) fn default_config(
    ranges: &[SupportedStreamConfigRange],
    rate: u32,
    channels: u16,
) -> Result<AudioConfig, AudioError> {
    ranges
        .iter()
        .filter_map(|range| {
            if range.channels() != channels
                || rate < range.min_sample_rate()
                || rate > range.max_sample_rate()
                || format(range.sample_format()).is_none()
            {
                return None;
            }
            let buffer_size = match *range.buffer_size() {
                SupportedBufferSize::Range { min, max } if max >= min.max(1) => {
                    128_u32.clamp(min.max(1), max)
                }
                SupportedBufferSize::Unknown => 128,
                SupportedBufferSize::Range { .. } => return None,
            };
            let config = AudioConfig {
                sample_rate: rate,
                channels,
                buffer_size,
            };
            let selection = select(ranges, config).ok()?;
            Some((config, selection))
        })
        .min_by_key(|(config, selection)| {
            (
                selection.buffer_range.is_none(),
                config.buffer_size.abs_diff(128),
                rank(*selection),
            )
        })
        .map(|(config, _)| config)
        .ok_or(AudioError::InvalidConfig(
            "device default rate/channels have no supported output configuration",
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn range(
        format: SampleFormat,
        channels: u16,
        min_rate: u32,
        max_rate: u32,
        buffer: SupportedBufferSize,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(channels, min_rate, max_rate, buffer, format)
    }
    fn known() -> SupportedBufferSize {
        SupportedBufferSize::Range { min: 64, max: 512 }
    }
    #[test]
    fn requested_tuple_selects_matching_format_rather_than_the_unrelated_default() {
        let ranges = [
            range(SampleFormat::U16, 1, 44_100, 44_100, known()),
            range(SampleFormat::F32, 2, 48_000, 96_000, known()),
        ];
        assert_eq!(
            select(&ranges, AudioConfig::DEFAULT).unwrap().format,
            OutputSampleFormat::F32
        );
        for (rate, channels, buffer) in [
            (44_100, 2, 128),
            (48_000, 1, 128),
            (48_000, 2, 63),
            (48_000, 2, 513),
        ] {
            assert!(
                select(
                    &ranges,
                    AudioConfig {
                        sample_rate: rate,
                        channels,
                        buffer_size: buffer
                    }
                )
                .is_err()
            );
        }
    }
    #[test]
    fn selection_is_order_independent_and_prefers_known_bounds_then_f32() {
        let mut ranges = [
            range(
                SampleFormat::F32,
                2,
                48_000,
                48_000,
                SupportedBufferSize::Unknown,
            ),
            range(SampleFormat::I16, 2, 48_000, 48_000, known()),
            range(SampleFormat::U16, 2, 48_000, 48_000, known()),
        ];
        let selected = select(&ranges, AudioConfig::DEFAULT).unwrap();
        assert_eq!(selected.format, OutputSampleFormat::I16);
        ranges.reverse();
        assert_eq!(select(&ranges, AudioConfig::DEFAULT).unwrap(), selected);
        ranges[0] = range(SampleFormat::F32, 2, 48_000, 48_000, known());
        assert_eq!(
            select(&ranges, AudioConfig::DEFAULT).unwrap().format,
            OutputSampleFormat::F32
        );
    }
    #[test]
    fn endpoints_unknown_bounds_and_invalid_ranges_are_explicit() {
        let ranges = [range(SampleFormat::I16, 2, 44_100, 96_000, known())];
        for (rate, buffer) in [(44_100, 64), (96_000, 512)] {
            assert!(
                select(
                    &ranges,
                    AudioConfig {
                        sample_rate: rate,
                        buffer_size: buffer,
                        ..AudioConfig::DEFAULT
                    }
                )
                .is_ok()
            );
        }
        let unknown = [range(
            SampleFormat::F32,
            2,
            48_000,
            48_000,
            SupportedBufferSize::Unknown,
        )];
        assert_eq!(
            select(&unknown, AudioConfig::DEFAULT).unwrap().buffer_range,
            None
        );
        let invalid = [range(
            SampleFormat::F32,
            2,
            48_000,
            48_000,
            SupportedBufferSize::Range { min: 512, max: 64 },
        )];
        assert!(select(&invalid, AudioConfig::DEFAULT).is_err());
        assert!(
            select(
                &[range(SampleFormat::F64, 2, 48_000, 48_000, known())],
                AudioConfig::DEFAULT
            )
            .is_err()
        );
    }
    #[test]
    fn default_buffer_is_supported_without_changing_rate_or_channels() {
        for (min, max, expected) in [(256, 1024, 256), (16, 64, 64), (64, 512, 128)] {
            let ranges = [range(
                SampleFormat::F32,
                2,
                48_000,
                48_000,
                SupportedBufferSize::Range { min, max },
            )];
            assert_eq!(
                default_config(&ranges, 48_000, 2).unwrap().buffer_size,
                expected
            );
            assert!(default_config(&ranges, 44_100, 2).is_err());
        }
    }
}
