//! A movie's soundtrack as stereo `f32` frames at the stream's own rate, decoded by MediaCodec.

use std::sync::Arc;

use super::ffi::{Codec, Extractor, Format, MediaError, Output};
use super::{Stall, feed, find_track};
use crate::pcm::{self, PcmEncoding};

/// Frames of silence returned per `decode` call after the end of the stream.
const SILENCE_FRAMES: usize = 1024;

/// MediaFormat `pcm-encoding` for float samples (`AudioFormat.ENCODING_PCM_FLOAT`).
const ENCODING_FLOAT: i32 = 4;

/// Decoded frames with the sample index of the first one.
type Chunk = (usize, Vec<[f32; 2]>);

/// Decodes the first audio track of a movie, sample-exactly seekable.
pub struct AudioDecoder {
    /// Deleted before the extractor feeding it.
    codec: Codec,
    extractor: Extractor,
    /// The first sample's time, movie time 0.
    start_us: i64,
    rate: u32,
    channels: usize,
    encoding: PcmEncoding,
    num_frames: usize,
    /// Decoded frames not handed out yet.
    pending: Vec<[f32; 2]>,
    /// The sample index of the next decoded frame; `None` right after a seek, until a buffer
    /// with a timestamp arrives.
    cursor: Option<usize>,
    input_done: bool,
    /// The decoder gave its end of stream: only silence follows.
    eof: bool,
}

// SAFETY: the decoder owns its MediaCodec and MediaExtractor exclusively and is used from one
// thread at a time (kira's decode thread); in synchronous mode neither is bound to the thread
// that created it.
unsafe impl Send for AudioDecoder {}

impl AudioDecoder {
    /// Opens the first audio track; `Ok(None)` when the movie has none.
    pub fn open(bytes: Arc<[u8]>) -> Result<Option<AudioDecoder>, MediaError> {
        let mut extractor = Extractor::new(bytes)?;
        let Some((index, mut track, mime)) = find_track(&extractor, "audio/") else {
            return Ok(None);
        };
        extractor.select(index)?;
        let start_us = extractor.sample_time_us().max(0);
        // Float is asked for; decoders may give 16-bit samples anyway.
        track.set_i32(c"pcm-encoding", ENCODING_FLOAT);
        let mut codec = Codec::by_type(&mime)?;
        codec.configure(&track, None)?;
        codec.start()?;
        let mut decoder = AudioDecoder {
            codec,
            extractor,
            start_us,
            rate: track
                .i32(c"sample-rate")
                .and_then(|r| u32::try_from(r).ok())
                .unwrap_or(0),
            channels: track
                .i32(c"channel-count")
                .and_then(|c| usize::try_from(c).ok())
                .unwrap_or(0),
            encoding: PcmEncoding::I16,
            num_frames: 0,
            pending: Vec::new(),
            cursor: Some(0),
            input_done: false,
            eof: false,
        };
        // The output format (rate, channels, sample type) is known once a buffer arrives.
        while let Some((_, samples)) = decoder.next_chunk(true)? {
            if !samples.is_empty() {
                decoder.pending = samples;
                break;
            }
        }
        if decoder.rate == 0 {
            return Err(MediaError::new("the audio stream has no sample rate"));
        }
        // The track's duration, else the container's (as the FFmpeg decoder takes them).
        let us = track
            .i64(c"durationUs")
            .filter(|us| *us > 0)
            .or_else(|| {
                let file = decoder.extractor.file_format()?;
                file.i64(c"durationUs").filter(|us| *us > 0)
            })
            .unwrap_or(0);
        decoder.num_frames = scale_us(us, decoder.rate).unwrap_or(0);
        Ok(Some(decoder))
    }

    /// Frames per second.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// The length in frames, from the track (else the container) duration; 0 when unknown.
    pub fn num_frames(&self) -> usize {
        self.num_frames
    }

    /// The next frames; never empty. After the end of the stream, silence.
    pub fn decode(&mut self) -> Result<Vec<[f32; 2]>, MediaError> {
        if !self.pending.is_empty() {
            return Ok(std::mem::take(&mut self.pending));
        }
        while let Some((_, samples)) = self.next_chunk(false)? {
            if !samples.is_empty() {
                return Ok(samples);
            }
        }
        Ok(vec![[0.0; 2]; SILENCE_FRAMES])
    }

    /// Moves to frame `index`, so that the next `decode` starts there. Returns `index`.
    pub fn seek(&mut self, index: usize) -> Result<usize, MediaError> {
        let us = u64::try_from(index)
            .ok()
            .and_then(|i| i.checked_mul(1_000_000))
            .and_then(|i| i.checked_div(u64::from(self.rate)))
            .and_then(|i| i64::try_from(i).ok())
            .unwrap_or(i64::MAX);
        self.extractor.seek_us(us)?;
        self.codec.flush()?;
        self.pending.clear();
        self.cursor = None;
        self.input_done = false;
        self.eof = false;
        while let Some((start, samples)) = self.next_chunk(false)? {
            if start.saturating_add(samples.len()) > index {
                let skip = index.saturating_sub(start);
                self.pending = samples.get(skip..).map(<[_]>::to_vec).unwrap_or_default();
                break;
            }
        }
        Ok(index)
    }

    /// The next decoded buffer with the sample index of its first frame; `None` once the
    /// stream is drained. `opening` lets the output format set the sample rate.
    fn next_chunk(&mut self, opening: bool) -> Result<Option<Chunk>, MediaError> {
        let mut stall = Stall::default();
        loop {
            if self.eof {
                return Ok(None);
            }
            feed(&mut self.codec, &mut self.extractor, &mut self.input_done)?;
            match self.codec.dequeue_output(10_000)? {
                Output::Buffer {
                    index,
                    pts_us,
                    size,
                    offset,
                    eos,
                } => {
                    let samples = self
                        .codec
                        .output_buffer(index)
                        .and_then(|buffer| buffer.get(offset..offset.checked_add(size)?))
                        .map(|data| pcm::to_stereo(data, self.encoding, self.channels))
                        .unwrap_or_default();
                    self.codec.release_output(index, false)?;
                    self.eof = eos;
                    let start = match self.cursor {
                        Some(cursor) => cursor,
                        None => {
                            let us = pts_us.saturating_sub(self.start_us).max(0);
                            scale_us_rounded(us, self.rate).unwrap_or(0)
                        }
                    };
                    self.cursor = Some(start.saturating_add(samples.len()));
                    return Ok(Some((start, samples)));
                }
                Output::FormatChanged => {
                    if let Some(format) = self.codec.output_format() {
                        self.apply(&format, opening);
                    }
                }
                Output::TryAgain => stall.check(self.input_done, "audio")?,
            }
        }
    }

    /// Takes the sample layout of an output format (and its rate while opening; kira keeps the
    /// rate it was given).
    fn apply(&mut self, format: &Format, opening: bool) {
        if opening
            && let Some(rate) = format.i32(c"sample-rate").and_then(|r| u32::try_from(r).ok())
            && rate > 0
        {
            self.rate = rate;
        }
        if let Some(channels) = format
            .i32(c"channel-count")
            .and_then(|c| usize::try_from(c).ok())
        {
            self.channels = channels;
        }
        self.encoding = match format.i32(c"pcm-encoding") {
            Some(ENCODING_FLOAT) => PcmEncoding::F32,
            _ => PcmEncoding::I16,
        };
    }
}

/// `us` microseconds as frames at `rate` Hz, rounded down.
fn scale_us(us: i64, rate: u32) -> Option<usize> {
    let frames = i128::from(us)
        .checked_mul(i128::from(rate))?
        .checked_div(1_000_000)?;
    usize::try_from(frames).ok()
}

/// `us` microseconds as frames at `rate` Hz, rounded to the nearest.
fn scale_us_rounded(us: i64, rate: u32) -> Option<usize> {
    let frames = i128::from(us)
        .checked_mul(i128::from(rate))?
        .checked_add(500_000)?
        .checked_div(1_000_000)?;
    usize::try_from(frames).ok()
}
