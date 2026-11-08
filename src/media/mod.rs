//! Movie decoding with the platform's MediaCodec decoders (no FFmpeg): facts, frames and the
//! soundtrack, with the contracts of `kagari-video`.

pub mod audio;
pub mod ffi;
pub mod video;

use std::sync::Arc;
use std::time::{Duration, Instant};

pub use audio::AudioDecoder;
pub use ffi::MediaError;
use ffi::{Codec, Extractor, Format};
pub use video::VideoDecoder;

/// The end-of-stream flag of a queued input buffer.
const FLAG_EOS: u32 = 4;

/// How long a decoder may stay silent after its last input before it is given up.
const STALL: Duration = Duration::from_secs(5);

/// The facts of a movie.
#[derive(Clone, Copy, Debug)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u32,
    pub has_audio: bool,
}

/// The size, length and soundtrack presence of a movie.
pub fn probe(bytes: Arc<[u8]>) -> Result<MediaInfo, MediaError> {
    let extractor = Extractor::new(bytes)?;
    let (_, video, _) =
        find_track(&extractor, "video/").ok_or_else(|| MediaError::new("no video stream"))?;
    let size = |key| {
        video
            .i32(key)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    let (width, height) = (size(c"width"), size(c"height"));
    if width == 0 || height == 0 {
        return Err(MediaError::new("the video has no size"));
    }
    let duration_us = duration_us(&extractor, &video).unwrap_or(0);
    Ok(MediaInfo {
        width,
        height,
        duration_ms: u32::try_from(duration_us / 1000).unwrap_or(0),
        has_audio: find_track(&extractor, "audio/").is_some(),
    })
}

/// The first track whose MIME type starts with `prefix`: index, format and MIME type.
fn find_track(extractor: &Extractor, prefix: &str) -> Option<(usize, Format, String)> {
    (0..extractor.track_count()).find_map(|index| {
        let format = extractor.track_format(index)?;
        let mime = format.string(c"mime")?;
        mime.starts_with(prefix).then_some((index, format, mime))
    })
}

/// The container's duration (µs), else the track's; positive or `None`.
fn duration_us(extractor: &Extractor, track: &Format) -> Option<i64> {
    let positive = |us: Option<i64>| us.filter(|us| *us > 0);
    positive(
        extractor
            .file_format()
            .and_then(|f| f.i64(c"durationUs")),
    )
    .or_else(|| positive(track.i64(c"durationUs")))
}

/// Queues the selected track's samples into every free input buffer, and the end of stream
/// once the samples run out.
fn feed(
    codec: &mut Codec,
    extractor: &mut Extractor,
    input_done: &mut bool,
) -> Result<(), MediaError> {
    while !*input_done {
        let Some(index) = codec.dequeue_input(0)? else {
            return Ok(());
        };
        let read = extractor.read_sample(codec.input_buffer(index)?);
        match usize::try_from(read) {
            Ok(size) => {
                codec.queue_input(index, size, extractor.sample_time_us(), 0)?;
                extractor.advance();
            }
            Err(_) => {
                codec.queue_input(index, 0, 0, FLAG_EOS)?;
                *input_done = true;
            }
        }
    }
    Ok(())
}

/// Tracks how long a decoder has produced nothing since its input ended.
#[derive(Default)]
struct Stall(Option<Instant>);

impl Stall {
    /// An error once the decoder has been silent for [`STALL`] after the end of its input.
    fn check(&mut self, input_done: bool, what: &str) -> Result<(), MediaError> {
        if !input_done {
            return Ok(());
        }
        let since = *self.0.get_or_insert_with(Instant::now);
        if since.elapsed() >= STALL {
            return Err(MediaError(format!("decoder produced no {what} for 5 s")));
        }
        Ok(())
    }
}
