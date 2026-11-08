//! Sounds by asset path: short ones decoded and cached, long ones streamed from their encoded
//! bytes.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;

use kira::sound::FromFileError;
use kira::sound::static_sound::StaticSoundData;
use kira::sound::streaming::StreamingSoundData;

/// Sounds whose decoded frames (8 bytes each: two `f32` channels) would take more than this are
/// streamed: about 6 s at 44.1 kHz.
const STREAM_ABOVE: usize = 2 << 20;

/// The cache budget: the decoded frames of short sounds plus the encoded bytes of streamed ones.
const CAPACITY: usize = 32 << 20;

/// The encoded bytes of a streamed sound, shared by its plays (WAV sizes already fitted).
#[derive(Clone)]
pub struct Encoded(Arc<Vec<u8>>);

impl AsRef<[u8]> for Encoded {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// A sound ready to play.
#[derive(Clone)]
pub enum Sound {
    /// Decoded whole; playing sounds share the frames. Boxed: kira's settings make it large.
    Decoded(Box<StaticSoundData>),
    /// Decoded while it plays, on kira's decode thread. `frames` and `rate` come from the
    /// container.
    Streamed {
        bytes: Encoded,
        frames: usize,
        rate: u32,
    },
}

impl Sound {
    /// The length in frames and the sample rate.
    pub fn frames_and_rate(&self) -> (usize, u32) {
        match self {
            Sound::Decoded(data) => (data.frames.len(), data.sample_rate),
            Sound::Streamed { frames, rate, .. } => (*frames, *rate),
        }
    }

    /// The length in milliseconds; `None` past `u32::MAX` or without a sample rate.
    pub fn duration_ms(&self) -> Option<u32> {
        let (frames, rate) = self.frames_and_rate();
        let ms = u64::try_from(frames)
            .ok()?
            .checked_mul(1000)?
            .checked_div(u64::from(rate))?;
        u32::try_from(ms).ok()
    }

    /// The bytes it holds in the cache.
    fn size(&self) -> usize {
        match self {
            Sound::Decoded(data) => data.frames.len().saturating_mul(8),
            Sound::Streamed { bytes, .. } => bytes.0.len(),
        }
    }
}

/// A new stream of `bytes` (a decoder on its own cursor).
pub fn stream(bytes: &Encoded) -> Result<StreamingSoundData<FromFileError>, FromFileError> {
    StreamingSoundData::from_cursor(Cursor::new(bytes.clone()))
}

/// The container's length of a stream and its sample rate, which kira gives only as the
/// duration `frames / rate` (exact to the nanosecond, so the rate rounds back exactly).
fn stream_length(data: &StreamingSoundData<FromFileError>) -> Option<(usize, u32)> {
    let frames = data.num_frames();
    let secs = data.duration().as_secs_f64();
    if frames == 0 || secs <= 0.0 {
        return None;
    }
    let rate = (frames as f64 / secs).round();
    (1.0..=f64::from(u32::MAX))
        .contains(&rate)
        .then_some((frames, rate as u32))
}

/// Sounds by path, least recently used evicted past [`CAPACITY`]. Playing sounds keep their own
/// reference (the frames, or the encoded bytes), so eviction never cuts one off.
#[derive(Default)]
pub struct SoundCache {
    sounds: HashMap<String, (Sound, u64)>,
    /// Paths that failed to decode, so the warning is logged once.
    failed: HashSet<String>,
    clock: u64,
    bytes: usize,
}

impl SoundCache {
    /// The cached sound at `path`, if loaded before.
    pub fn get(&mut self, path: &str) -> Option<Sound> {
        self.clock = self.clock.saturating_add(1);
        let clock = self.clock;
        self.sounds.get_mut(path).map(|(sound, used)| {
            *used = clock;
            sound.clone()
        })
    }

    /// Whether `path` failed to decode before.
    pub fn failed(&self, path: &str) -> bool {
        self.failed.contains(path)
    }

    /// The sound at `path`: cached, or loaded from its encoded `bytes` and cached. A sound
    /// longer than [`STREAM_ABOVE`] (by its container's frame count) is kept encoded and
    /// streamed; others, and files whose container gives no length, are decoded whole. A decode
    /// error is logged once and gives `None`.
    pub fn load(&mut self, path: &str, mut bytes: Vec<u8>) -> Option<Sound> {
        if let Some(sound) = self.get(path) {
            return Some(sound);
        }
        if self.failed.contains(path) {
            return None;
        }
        fit_wav_sizes(&mut bytes);
        let bytes = Encoded(Arc::new(bytes));
        let streamed = stream(&bytes)
            .ok()
            .as_ref()
            .and_then(stream_length)
            .filter(|(frames, _)| frames.saturating_mul(8) > STREAM_ABOVE);
        let sound = match streamed {
            Some((frames, rate)) => Sound::Streamed {
                bytes,
                frames,
                rate,
            },
            None => match StaticSoundData::from_cursor(Cursor::new(bytes)) {
                Ok(data) => Sound::Decoded(Box::new(data)),
                Err(e) => {
                    log::warn!("cannot decode audio `{path}`: {e}");
                    self.failed.insert(path.to_owned());
                    return None;
                }
            },
        };
        self.insert(path, sound.clone());
        Some(sound)
    }

    fn insert(&mut self, path: &str, sound: Sound) {
        self.bytes = self.bytes.saturating_add(sound.size());
        while self.bytes > CAPACITY {
            let Some(oldest) = self
                .sounds
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(p, _)| p.clone())
            else {
                break;
            };
            if let Some((old, _)) = self.sounds.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(old.size());
            }
        }
        self.sounds.insert(path.to_owned(), (sound, self.clock));
    }
}

/// Clamps the RIFF and `data` chunk sizes of a WAV to the bytes present, so a size past the end
/// of the file means "until the end": the `0xFFFFFFFF` placeholder that streaming encoders write
/// (ffmpeg to a pipe, TTS services), or a truncated file. Symphonia reads such a `data` chunk
/// until it runs out of bytes and then fails the whole decode. Other files are left alone.
fn fit_wav_sizes(bytes: &mut [u8]) -> Option<()> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    clamp_size(bytes, 4)?;
    let mut at: usize = 12;
    loop {
        let size_at = at.checked_add(4)?;
        if bytes.get(at..size_at)? == b"data" {
            return clamp_size(bytes, size_at).map(drop);
        }
        let size = usize::try_from(size_field(bytes, size_at)?).ok()?;
        // Chunks are padded to an even size.
        at = size_at
            .checked_add(4)?
            .checked_add(size)?
            .checked_add(size & 1)?;
    }
}

/// The little-endian chunk size at `at`.
fn size_field(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(field.try_into().ok()?))
}

/// Lowers the chunk size at `at` to the bytes after it, if it claims more; gives the new size.
fn clamp_size(bytes: &mut [u8], at: usize) -> Option<u32> {
    let body = at.checked_add(4)?;
    let left = u32::try_from(bytes.len().checked_sub(body)?).unwrap_or(u32::MAX);
    let size = size_field(bytes, at)?.min(left);
    bytes
        .get_mut(at..body)?
        .copy_from_slice(&size.to_le_bytes());
    Some(size)
}
