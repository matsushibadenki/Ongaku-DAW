//! Embedded PCM is spooled once into private immutable backing storage off audio.
//! History clones share media/cache identity; runtime state is never serialized.
use audio_clip::PcmAsset;
use audio_file::AudioFrameReader;
use audio_types::{AudioError, PreparationCheck};
use daw_streaming::TimelineStream;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub(super) struct MediaCache(Arc<Mutex<Option<Arc<TimelineStream>>>>);

impl MediaCache {
    pub(super) fn snapshot(&self) -> Option<daw_streaming::TimelineStreamSnapshot> {
        self.0
            .try_lock()
            .ok()?
            .as_ref()
            .map(|media| media.snapshot())
    }

    pub(super) fn prepare(
        &self,
        asset: &PcmAsset,
        check: &impl PreparationCheck,
    ) -> Result<Arc<TimelineStream>, AudioError> {
        check.checkpoint()?;
        let mut cache = self
            .0
            .lock()
            .map_err(|_| AudioError::InvalidState("media preparation lock poisoned"))?;
        if let Some(media) = &*cache {
            if media.snapshot().worker_failed {
                return Err(AudioError::InvalidState(
                    "stream media worker failed; reopen the project to recover",
                ));
            }
            return Ok(Arc::clone(media));
        }
        let mut file = tempfile::tempfile().map_err(io_error)?;
        let mut bytes = Vec::with_capacity(16_384);
        for chunk in asset.interleaved_samples().chunks(4096) {
            check.checkpoint()?;
            bytes.clear();
            for sample in chunk {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            file.write_all(&bytes).map_err(io_error)?;
        }
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let reader = EmbeddedReader {
            file,
            rate: asset.sample_rate(),
            channels: asset.channels(),
            bytes: Vec::new(),
        };
        let page_frames = (asset.sample_rate() as usize / 8).clamp(1, 4096);
        let media = TimelineStream::prepare(
            Box::new(reader),
            asset.frame_count(),
            page_frames,
            16,
            check,
        )?;
        check.checkpoint()?;
        *cache = Some(Arc::clone(&media));
        Ok(media)
    }
}
#[allow(clippy::needless_pass_by_value)] // Used directly as a map_err adapter.
fn io_error(error: std::io::Error) -> AudioError {
    AudioError::Backend(format!("embedded media backing: {error}"))
}
struct EmbeddedReader {
    file: File,
    rate: u32,
    channels: u16,
    bytes: Vec<u8>,
}
impl AudioFrameReader for EmbeddedReader {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        let offset = frame
            .checked_mul(u64::from(self.channels) * 4)
            .ok_or(AudioError::InvalidState("media offset overflow"))?;
        self.file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        Ok(())
    }
    fn read_interleaved(&mut self, output: &mut [f32]) -> Result<usize, AudioError> {
        self.bytes.resize(output.len() * 4, 0);
        self.file.read_exact(&mut self.bytes).map_err(io_error)?;
        for (sample, bytes) in output.iter_mut().zip(self.bytes.chunks_exact(4)) {
            *sample = f32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| AudioError::InvalidState("invalid embedded sample"))?,
            );
        }
        Ok(output.len())
    }
}
