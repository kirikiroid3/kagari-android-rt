//! Frames at a playback position, decoded by MediaCodec into an `AImageReader` and converted to
//! RGBA.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::ffi::{Codec, Extractor, Format, MediaError, Output, Reader};
use super::{Stall, feed, find_track};
use crate::colour::{self, Plane};
use crate::pick::{Picker, RgbaFrame, Source};

/// The platform's software decoders, used when the default (hardware) one fails.
const SOFTWARE: [(&str, &str); 3] = [
    ("video/avc", "c2.android.avc.decoder"),
    ("video/x-vnd.on2.vp9", "c2.android.vp9.decoder"),
    ("video/av01", "c2.android.av1.decoder"),
];

/// How long a rendered frame may take to reach the reader.
const PICTURE_WAIT: Duration = Duration::from_millis(500);

fn software_decoder(mime: &str) -> Option<&'static str> {
    SOFTWARE
        .iter()
        .find(|(m, _)| *m == mime)
        .map(|(_, name)| *name)
}

fn log_fallback(reason: &str) {
    log::info!(
        target: "kagari::video",
        "video: hardware decoding unavailable ({reason}); using software"
    );
}

/// The decoder and its picture queue. Field order: the codec is deleted before the reader it
/// renders into.
struct Stream {
    codec: Codec,
    reader: Reader,
    extractor: Extractor,
    track: Format,
    mime: String,
    /// The first sample's time, movie time 0.
    start_us: i64,
    /// The decoder in use is the software one.
    software: bool,
    input_done: bool,
    /// The decoder gave its end of stream.
    output_done: bool,
    /// The decoder gave an output since it started. A flush before that loses the
    /// codec-specific data the configuration supplied, so seeks skip it.
    output_seen: bool,
    /// The output's `color-standard` and `color-range`.
    standard: Option<i32>,
    range: Option<i32>,
}

/// A codec (the default for the type, or `name`) decoding `track` into a new reader.
fn open_codec(
    track: &Format,
    mime: &str,
    size: (i32, i32),
    name: Option<&str>,
) -> Result<(Codec, Reader), MediaError> {
    let reader = Reader::new(size.0, size.1)?;
    let mut codec = match name {
        Some(name) => Codec::by_name(name)?,
        None => Codec::by_type(mime)?,
    };
    codec.configure(track, Some(reader.window()?))?;
    codec.start()?;
    Ok((codec, reader))
}

impl Stream {
    /// Replaces the decoder with the software one for the stream's type.
    fn reopen(&mut self, name: &str) -> Result<(), MediaError> {
        let (codec, reader) = open_codec(&self.track, &self.mime, self.reader.size(), Some(name))?;
        self.codec = codec;
        self.reader = reader;
        self.software = true;
        self.input_done = false;
        self.output_done = false;
        self.output_seen = false;
        Ok(())
    }

    /// Reads the colour description of the new output format, and follows a size change with
    /// a reader of the new size.
    fn format_changed(&mut self) -> Result<(), MediaError> {
        let Some(format) = self.codec.output_format() else {
            return Ok(());
        };
        self.standard = format
            .i32(c"color-standard")
            .or_else(|| self.track.i32(c"color-standard"));
        self.range = format
            .i32(c"color-range")
            .or_else(|| self.track.i32(c"color-range"));
        if let (Some(w), Some(h)) = (format.i32(c"width"), format.i32(c"height"))
            && w > 0
            && h > 0
            && (w, h) != self.reader.size()
        {
            let reader = Reader::new(w, h)?;
            self.codec.set_output_surface(reader.window()?)?;
            self.reader = reader;
        }
        Ok(())
    }
}

impl Source for Stream {
    type Held = usize;

    fn next(&mut self) -> Result<Option<(usize, i64)>, String> {
        let mut stall = Stall::default();
        loop {
            if self.output_done {
                return Ok(None);
            }
            feed(&mut self.codec, &mut self.extractor, &mut self.input_done)?;
            let output = self.codec.dequeue_output(10_000)?;
            if !matches!(output, Output::TryAgain) {
                self.output_seen = true;
            }
            match output {
                Output::Buffer {
                    index,
                    pts_us,
                    size,
                    eos,
                    ..
                } => {
                    if eos {
                        self.output_done = true;
                        if size == 0 {
                            self.codec.release_output(index, false)?;
                            return Ok(None);
                        }
                    }
                    let pts_ms = pts_us.saturating_sub(self.start_us).max(0) / 1000;
                    return Ok(Some((index, pts_ms)));
                }
                Output::FormatChanged => self.format_changed()?,
                Output::TryAgain => stall.check(self.input_done, "frame")?,
            }
        }
    }

    fn seek(&mut self, target_ms: i64) -> Result<(), String> {
        self.extractor.seek_us(target_ms.saturating_mul(1000))?;
        // Before the first output nothing decoded yet; samples queued from before the seek
        // decode to frames the picker passes over.
        if self.output_seen || self.input_done {
            self.codec.flush()?;
        }
        self.input_done = false;
        self.output_done = false;
        Ok(())
    }

    fn discard(&mut self, held: usize) {
        if let Err(e) = self.codec.release_output(held, false) {
            log::warn!(target: "kagari::video", "video: {e}");
        }
    }

    fn convert(&mut self, held: usize, pts_ms: i64) -> Result<RgbaFrame, String> {
        self.codec.release_output(held, true)?;
        let started = Instant::now();
        let image = loop {
            if let Some(image) = self.reader.acquire_next()? {
                break image;
            }
            if started.elapsed() >= PICTURE_WAIT {
                return Err("decoder delivered no picture".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let (left, top, right, bottom) = image.crop()?;
        let unsupported = || "unsupported picture layout".to_owned();
        let size = |from: i32, to: i32| {
            to.checked_sub(from)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
        };
        let (width, height) = size(left, right)
            .zip(size(top, bottom))
            .ok_or_else(unsupported)?;
        let (left, top) = u32::try_from(left)
            .ok()
            .zip(u32::try_from(top).ok())
            .ok_or_else(unsupported)?;
        let plane = |index| {
            image.plane(index).map(|(data, row_stride, pixel_stride)| Plane {
                data,
                row_stride,
                pixel_stride,
            })
        };
        let rgba = colour::to_rgba(
            plane(0)?,
            plane(1)?,
            plane(2)?,
            (left, top, width, height),
            colour::matrix(self.standard, height),
            colour::range(self.range),
        )
        .ok_or_else(unsupported)?;
        drop(image);
        Ok(RgbaFrame {
            pts_ms: u64::try_from(pts_ms).unwrap_or(0),
            width,
            height,
            rgba,
        })
    }
}

/// Gives the frame on screen at a playback position, seeking when the position jumps.
pub struct VideoDecoder {
    picker: Picker<Stream>,
    stream: Stream,
}

// SAFETY: the decoder owns its MediaCodec, MediaExtractor and AImageReader exclusively and is
// used from one thread at a time; in synchronous mode none of them is bound to the thread that
// created it.
unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    /// Opens the first video track, on the default decoder for its type, else the software
    /// one.
    pub fn open(bytes: Arc<[u8]>) -> Result<VideoDecoder, MediaError> {
        let mut extractor = Extractor::new(bytes)?;
        let (index, track, mime) =
            find_track(&extractor, "video/").ok_or_else(|| MediaError::new("no video stream"))?;
        extractor.select(index)?;
        let start_us = extractor.sample_time_us().max(0);
        let size = (
            track.i32(c"width").unwrap_or(0),
            track.i32(c"height").unwrap_or(0),
        );
        let (codec, reader, software) = match open_codec(&track, &mime, size, None) {
            Ok((codec, reader)) => (codec, reader, false),
            Err(e) => {
                let Some(name) = software_decoder(&mime) else {
                    return Err(e);
                };
                log_fallback(&e.0);
                let (codec, reader) = open_codec(&track, &mime, size, Some(name))?;
                (codec, reader, true)
            }
        };
        let (standard, range) = (track.i32(c"color-standard"), track.i32(c"color-range"));
        Ok(VideoDecoder {
            picker: Picker::new(),
            stream: Stream {
                codec,
                reader,
                extractor,
                track,
                mime,
                start_us,
                software,
                input_done: false,
                output_done: false,
                output_seen: false,
                standard,
                range,
            },
        })
    }

    /// The decoder's codec name.
    pub fn backend(&self) -> String {
        self.stream.codec.name()
    }

    /// The frame shown at `target_ms`: the last frame at or before it, or the first frame when
    /// the target is before it. `None` when the stream has no frame at all. A failure on the
    /// default decoder moves to the software one, once.
    pub fn frame_at(&mut self, target_ms: u64) -> Result<Option<Arc<RgbaFrame>>, MediaError> {
        match self.picker.frame_at(&mut self.stream, target_ms) {
            Err(e) if !self.stream.software => {
                let Some(name) = software_decoder(&self.stream.mime) else {
                    return Err(MediaError(e));
                };
                log_fallback(&e);
                // The held buffers belong to the old codec, which goes with them.
                self.picker = Picker::new();
                self.stream.reopen(name)?;
                let target = i64::try_from(target_ms).unwrap_or(i64::MAX);
                self.stream.seek(target)?;
                Ok(self.picker.frame_at(&mut self.stream, target_ms)?)
            }
            result => Ok(result?),
        }
    }
}
