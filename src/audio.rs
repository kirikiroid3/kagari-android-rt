//! The kira audio backend: plays the engine's [`AudioCommand`]s.

use std::collections::HashMap;
use std::time::Duration;

use kagari_engine::{AudioCommand, Bus};
use kira::sound::FromFileError;
use kira::sound::static_sound::StaticSoundHandle;
use kira::sound::streaming::{Decoder, StreamingSoundData, StreamingSoundHandle};
use kira::sound::{PlaybackPosition, PlaybackState};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{
    AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Easing, StartTime, Tween,
};

use crate::media::{AudioDecoder, MediaError};
use crate::platform::AndroidPlatform;
use crate::sounds::{Sound, stream};

/// A linear amplitude gain as kira volume.
fn db(gain: f32) -> Decibels {
    if gain <= 0.001 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * gain.log10())
    }
}

/// A linear tween of `ms` milliseconds, starting now.
fn tw(ms: u32) -> Tween {
    Tween {
        start_time: StartTime::Immediate,
        duration: Duration::from_millis(u64::from(ms)),
        easing: Easing::Linear,
    }
}

/// Milliseconds as frames at `rate` Hz, rounded to the nearest frame (as kira rounds seconds).
fn frames(ms: u32, rate: u32) -> usize {
    let scaled = u64::from(ms)
        .saturating_mul(u64::from(rate))
        .saturating_add(500)
        / 1000;
    usize::try_from(scaled).unwrap_or(usize::MAX)
}

/// The part of a decoded sound that plays, in frames, checked against its length so that kira
/// never indexes past the data, underflows, or spins on an empty loop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    /// The exclusive end of a sound that stops early (no loop).
    pub end: Option<usize>,
    /// The loop region `[start, end)`, non-empty and inside the data.
    pub loop_region: Option<(usize, usize)>,
}

/// Clamps a `Play`'s times (ms, file time) to a sound of `len` frames at `rate` Hz. A start or
/// end past the audio is clamped to its end; an end before the start is ignored; a loop region
/// that is empty once clamped is dropped. Each repair is warned about.
pub(crate) fn clamp_span(
    path: &str,
    len: usize,
    rate: u32,
    start_ms: u32,
    end_ms: Option<u32>,
    loop_region: Option<(u32, Option<u32>)>,
) -> Span {
    let mut start = frames(start_ms, rate);
    if start > len {
        log::warn!("sound `{path}`: start at {start_ms} ms is past the end of the audio; clamped");
        start = len;
    }
    let past_end = |what: &str, ms: u32, at: usize| {
        if at > len {
            log::warn!("sound `{path}`: {what} at {ms} ms is past the end of the audio; clamped");
            len
        } else {
            at
        }
    };
    if let Some((ls_ms, le_ms)) = loop_region {
        // The loop is in file time, so the start is a position and `end_ms` does not apply.
        let ls = frames(ls_ms, rate);
        let le = le_ms.map_or(len, |ms| past_end("the loop end", ms, frames(ms, rate)));
        if ls < le {
            return Span {
                start,
                end: None,
                loop_region: Some((ls, le)),
            };
        }
        log::warn!(
            "sound `{path}`: the loop region from {ls_ms} ms is empty or past the end of the \
             audio; not looping"
        );
        return Span {
            start,
            end: None,
            loop_region: None,
        };
    }
    let end = end_ms.and_then(|ms| {
        let end = past_end("the end", ms, frames(ms, rate));
        if end < start {
            log::warn!(
                "sound `{path}`: the end at {ms} ms is before its start at {start_ms} ms; ignored"
            );
            None
        } else {
            Some(end)
        }
    });
    Span {
        start,
        end,
        loop_region: None,
    }
}

/// What a `Play` asked for, kept for `Restart`.
#[derive(Clone)]
struct PlayParams {
    path: String,
    start_ms: u32,
    end_ms: Option<u32>,
    loop_region: Option<(u32, Option<u32>)>,
    gain: f32,
    fade_in_ms: u32,
    delay_ms: u32,
}

/// A playing sound's handle.
enum Handle {
    Decoded(StaticSoundHandle),
    Streamed(StreamingSoundHandle<FromFileError>),
    /// A movie's audio track.
    Movie(StreamingSoundHandle<MediaError>),
}

impl Handle {
    fn state(&self) -> PlaybackState {
        match self {
            Handle::Decoded(h) => h.state(),
            Handle::Streamed(h) => h.state(),
            Handle::Movie(h) => h.state(),
        }
    }

    fn stop(&mut self, tween: Tween) {
        match self {
            Handle::Decoded(h) => h.stop(tween),
            Handle::Streamed(h) => h.stop(tween),
            Handle::Movie(h) => h.stop(tween),
        }
    }

    fn set_volume(&mut self, volume: Decibels, tween: Tween) {
        match self {
            Handle::Decoded(h) => h.set_volume(volume, tween),
            Handle::Streamed(h) => h.set_volume(volume, tween),
            Handle::Movie(h) => h.set_volume(volume, tween),
        }
    }

    fn pause(&mut self, tween: Tween) {
        match self {
            Handle::Decoded(h) => h.pause(tween),
            Handle::Streamed(h) => h.pause(tween),
            Handle::Movie(h) => h.pause(tween),
        }
    }

    fn resume(&mut self, tween: Tween) {
        match self {
            Handle::Decoded(h) => h.resume(tween),
            Handle::Streamed(h) => h.resume(tween),
            Handle::Movie(h) => h.resume(tween),
        }
    }
}

/// A movie's audio track as a kira stream.
struct MovieAudio(AudioDecoder);

impl Decoder for MovieAudio {
    type Error = MediaError;

    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }

    fn num_frames(&self) -> usize {
        self.0.num_frames()
    }

    fn decode(&mut self) -> Result<Vec<kira::Frame>, MediaError> {
        Ok(self
            .0
            .decode()?
            .into_iter()
            .map(|[left, right]| kira::Frame { left, right })
            .collect())
    }

    fn seek(&mut self, index: usize) -> Result<usize, MediaError> {
        self.0.seek(index)
    }
}

struct Playing {
    handle: Handle,
    bus: Bus,
    params: PlayParams,
}

/// The audio output: three sub-tracks (music, sound, voice) and the playing sounds by engine id.
/// Without an output device every command is dropped.
pub struct AudioOut {
    manager: Option<AudioManager<DefaultBackend>>,
    buses: Option<[TrackHandle; 3]>,
    playing: HashMap<u64, Playing>,
}

impl AudioOut {
    /// Opens the default output device.
    pub fn new() -> AudioOut {
        let mut out = AudioOut::silent();
        let mut manager = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
        {
            Ok(manager) => manager,
            Err(e) => {
                log::warn!("audio output unavailable ({e}); running silent");
                return out;
            }
        };
        let mut track = || manager.add_sub_track(TrackBuilder::new());
        match (track(), track(), track()) {
            (Ok(music), Ok(sound), Ok(voice)) => {
                out.buses = Some([music, sound, voice]);
                out.manager = Some(manager);
            }
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                log::warn!("audio output unavailable ({e}); running silent");
            }
        }
        out
    }

    /// No output device (after a fatal error).
    pub fn silent() -> AudioOut {
        AudioOut {
            manager: None,
            buses: None,
            playing: HashMap::new(),
        }
    }

    /// The app went to the background (`true`) or came back: every bus pauses or resumes.
    pub fn set_suspended(&mut self, suspended: bool) {
        for track in self.buses.iter_mut().flatten() {
            if suspended {
                track.pause(tw(0));
            } else {
                track.resume(tw(0));
            }
        }
    }

    pub fn apply(&mut self, cmds: Vec<AudioCommand>, platform: &mut AndroidPlatform) {
        if self.manager.is_none() {
            return;
        }
        self.playing.retain(|_, p| {
            match &mut p.handle {
                Handle::Streamed(h) => {
                    while let Some(e) = h.pop_error() {
                        log::warn!("cannot stream `{}`: {e}", p.params.path);
                    }
                }
                Handle::Movie(h) => {
                    while let Some(e) = h.pop_error() {
                        log::warn!("cannot stream `{}`: {e}", p.params.path);
                    }
                }
                Handle::Decoded(_) => {}
            }
            p.handle.state() != PlaybackState::Stopped
        });
        for cmd in cmds {
            match cmd {
                AudioCommand::SetBusGain { bus, gain } => {
                    if let Some(track) = self.bus(bus) {
                        track.set_volume(db(gain), tw(0));
                    }
                }
                AudioCommand::Play {
                    id,
                    path,
                    bus,
                    start_ms,
                    end_ms,
                    loop_region,
                    gain,
                    fade_in_ms,
                    delay_ms,
                } => {
                    let params = PlayParams {
                        path,
                        start_ms,
                        end_ms,
                        loop_region,
                        gain,
                        fade_in_ms,
                        delay_ms,
                    };
                    self.play(id, bus, params, platform);
                }
                AudioCommand::SetGain { id, gain, tween_ms } => match self.playing.get_mut(&id) {
                    Some(p) => {
                        p.handle.set_volume(db(gain), tw(tween_ms));
                        p.params.gain = gain;
                    }
                    None => log::debug!("audio: SetGain for unknown sound {id}"),
                },
                AudioCommand::Stop { id, fade_out_ms } => match self.playing.remove(&id) {
                    Some(mut p) => p.handle.stop(tw(fade_out_ms)),
                    None => log::debug!("audio: Stop for unknown sound {id}"),
                },
                AudioCommand::Restart { id } => match self.playing.get(&id) {
                    Some(p) if matches!(p.handle, Handle::Movie(_)) => {
                        log::debug!("audio: Restart for movie audio {id} ignored");
                    }
                    Some(p) => {
                        let bus = p.bus;
                        let params = PlayParams {
                            fade_in_ms: 0,
                            delay_ms: 0,
                            ..p.params.clone()
                        };
                        self.play(id, bus, params, platform);
                    }
                    None => log::debug!("audio: Restart for unknown sound {id}"),
                },
                AudioCommand::PlayMovie {
                    id,
                    path,
                    looping,
                    gain,
                } => self.play_movie(id, path, looping, gain, platform),
                AudioCommand::Pause { id } => match self.playing.get_mut(&id) {
                    Some(p) => p.handle.pause(tw(0)),
                    None => log::debug!("audio: Pause for unknown sound {id}"),
                },
                AudioCommand::Resume { id } => match self.playing.get_mut(&id) {
                    Some(p) => p.handle.resume(tw(0)),
                    None => log::debug!("audio: Resume for unknown sound {id}"),
                },
            }
        }
    }

    fn bus(&mut self, bus: Bus) -> Option<&mut TrackHandle> {
        let index = match bus {
            Bus::Music => 0,
            Bus::Sound => 1,
            Bus::Voice => 2,
        };
        self.buses.as_mut()?.get_mut(index)
    }

    /// Starts the audio track of the movie at `path` under `id` on the music bus, stopping
    /// whatever played under it. A movie without audio plays nothing.
    fn play_movie(
        &mut self,
        id: u64,
        path: String,
        looping: bool,
        gain: f32,
        platform: &mut AndroidPlatform,
    ) {
        if let Some(mut old) = self.playing.remove(&id) {
            old.handle.stop(tw(0));
        }
        let Some(bytes) = platform.movie_bytes(&path) else {
            return;
        };
        let decoder = match AudioDecoder::open(bytes) {
            Ok(Some(decoder)) => decoder,
            Ok(None) => return,
            Err(e) => {
                log::warn!("cannot play movie audio `{path}`: {e}");
                return;
            }
        };
        let mut data = StreamingSoundData::from_decoder(MovieAudio(decoder)).volume(db(gain));
        if looping {
            data = data.loop_region(..);
        }
        let Some(track) = self.bus(Bus::Music) else {
            return;
        };
        match track.play(data) {
            Ok(handle) => {
                let params = PlayParams {
                    path,
                    start_ms: 0,
                    end_ms: None,
                    loop_region: None,
                    gain,
                    fade_in_ms: 0,
                    delay_ms: 0,
                };
                self.playing.insert(
                    id,
                    Playing {
                        handle: Handle::Movie(handle),
                        bus: Bus::Music,
                        params,
                    },
                );
            }
            Err(e) => log::warn!("cannot play movie audio `{path}`: {e}"),
        }
    }

    /// Starts `params` under `id`, stopping whatever played under it.
    fn play(&mut self, id: u64, bus: Bus, params: PlayParams, platform: &mut AndroidPlatform) {
        if let Some(mut old) = self.playing.remove(&id) {
            old.handle.stop(tw(0));
        }
        let Some(sound) = platform.sound(&params.path) else {
            return;
        };
        let (len, rate) = sound.frames_and_rate();
        let span = clamp_span(
            &params.path,
            len,
            rate,
            params.start_ms,
            params.end_ms,
            params.loop_region,
        );
        // The same settings on either kind of sound data, which share the builder methods.
        macro_rules! configure {
            ($data:expr) => {{
                let start = PlaybackPosition::Samples(span.start);
                let mut data = match (span.loop_region, span.end) {
                    (Some((ls, le)), _) => $data
                        .start_position(start)
                        .loop_region(PlaybackPosition::Samples(ls)..PlaybackPosition::Samples(le)),
                    (None, Some(end)) => $data.slice(start..PlaybackPosition::Samples(end)),
                    (None, None) => $data.start_position(start),
                };
                data = data.volume(db(params.gain));
                if params.fade_in_ms > 0 {
                    data = data.fade_in_tween(Some(tw(params.fade_in_ms)));
                }
                if params.delay_ms > 0 {
                    data = data.start_time(StartTime::Delayed(Duration::from_millis(u64::from(
                        params.delay_ms,
                    ))));
                }
                data
            }};
        }
        let Some(track) = self.bus(bus) else {
            return;
        };
        let handle = match sound {
            Sound::Decoded(data) => track
                .play(configure!(*data))
                .map(Handle::Decoded)
                .map_err(|e| e.to_string()),
            Sound::Streamed { bytes, .. } => match stream(&bytes) {
                Ok(data) => track
                    .play(configure!(data))
                    .map(Handle::Streamed)
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            },
        };
        match handle {
            Ok(handle) => {
                self.playing.insert(
                    id,
                    Playing {
                        handle,
                        bus,
                        params,
                    },
                );
            }
            Err(e) => log::warn!("cannot play `{}`: {e}", params.path),
        }
    }
}
