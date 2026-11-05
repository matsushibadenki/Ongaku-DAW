//! File decoding APIs used exclusively by background/control threads.

use audio_types::AudioError;
use hound::{SampleFormat, WavReader};
use std::{
    fs::File,
    io::{BufReader, Read, Seek},
    path::Path,
};

pub trait AudioFrameReader: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError>;
    fn seek(&mut self, frame: u64) -> Result<(), AudioError>;
}

pub struct WavPcmReader<R: Read + Seek + Send> {
    reader: WavReader<R>,
    format: SampleFormat,
    bits_per_sample: u16,
}

impl WavPcmReader<BufReader<File>> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        WavReader::open(path)
            .map(Self::from_reader)
            .map_err(file_error)
    }
}

impl<R: Read + Seek + Send> WavPcmReader<R> {
    #[must_use]
    pub fn from_reader(reader: WavReader<R>) -> Self {
        let spec = reader.spec();
        Self {
            reader,
            format: spec.sample_format,
            bits_per_sample: spec.bits_per_sample,
        }
    }
}

impl<R: Read + Seek + Send> AudioFrameReader for WavPcmReader<R> {
    fn sample_rate(&self) -> u32 {
        self.reader.spec().sample_rate
    }
    fn channels(&self) -> u16 {
        self.reader.spec().channels
    }

    #[allow(clippy::cast_precision_loss)]
    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError> {
        match self.format {
            SampleFormat::Float if self.bits_per_sample == 32 => {
                read_float(&mut self.reader, output)
            }
            SampleFormat::Int if (1..=32).contains(&self.bits_per_sample) => {
                let amplitude = ((1_u64 << (self.bits_per_sample - 1)) - 1) as f32;
                let mut written = 0;
                for sample in self.reader.samples::<i32>().take(output.len()) {
                    output[written] = sample.map_err(file_error)? as f32 / amplitude;
                    written += 1;
                }
                Ok(written)
            }
            _ => Err(AudioError::Backend("unsupported WAV sample format".into())),
        }
    }

    fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        let frame = u32::try_from(frame)
            .map_err(|_| AudioError::InvalidConfig("WAV seek exceeds format limit"))?;
        self.reader.seek(frame).map_err(file_error)
    }
}

fn read_float<R: Read + Seek>(
    reader: &mut WavReader<R>,
    output: &mut [f32],
) -> Result<usize, AudioError> {
    let mut written = 0;
    for sample in reader.samples::<f32>().take(output.len()) {
        let sample = sample.map_err(file_error)?;
        output[written] = if sample.is_finite() { sample } else { 0.0 };
        written += 1;
    }
    Ok(written)
}

fn file_error(error: impl std::fmt::Display) -> AudioError {
    AudioError::Backend(format!("audio file error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{AudioFrameReader, WavPcmReader};

    #[test]
    fn decodes_pcm_on_the_calling_background_thread() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
        writer.write_sample(0_i16).unwrap();
        writer.write_sample(i16::MAX).unwrap();
        writer.finalize().unwrap();
        let mut reader = WavPcmReader::open(file.path()).unwrap();
        let mut output = [0.0; 2];
        assert_eq!(reader.read_interleaved(&mut output).unwrap(), 2);
        assert!(output[0].abs() < f32::EPSILON);
        assert!((output[1] - 1.0).abs() < f32::EPSILON);
        reader.seek(0).unwrap();
        assert_eq!(reader.read_interleaved(&mut output[..1]).unwrap(), 1);
        assert!(output[0].abs() < f32::EPSILON);
    }
}
