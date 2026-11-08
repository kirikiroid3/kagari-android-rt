//! Movie playback: decodes the frames a [`Frame`](kagari_engine::Frame) draws and uploads them
//! to the renderer as video textures.
//!
//! Each movie decodes on its own worker thread with MediaCodec (hardware when the device has a
//! decoder, else software); the main thread never waits for a frame after the first, and a late
//! frame keeps the previous one on screen.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use kagari_archive::archive::ArchiveDir;
use kagari_engine::{AssetStore, VideoKey, VideoRequest};
use kagari_render::Renderer;

use crate::media::{MediaInfo, VideoDecoder, probe};
use crate::pick::RgbaFrame;

/// The encoded movies kept in memory beyond those playing.
const BYTES_BUDGET: u64 = 256 << 20;

/// What a player's worker and the main thread share.
struct Shared {
    /// The position the main thread wants, in movie time (ms).
    target: u64,
    /// The position the worker last decoded.
    done: Option<u64>,
    /// Set when the player is dropped: the worker exits.
    stop: bool,
    /// The latest decoded frame.
    frame: Option<Arc<RgbaFrame>>,
}

/// The shared state and the condition variable the worker waits on.
type Handoff = Arc<(Mutex<Shared>, Condvar)>;

enum Decode {
    /// The movie could not be opened; nothing is drawn.
    Failed,
    Live(Handoff),
}

struct Player {
    path: String,
    decode: Decode,
    /// The frame last uploaded for this player's key.
    uploaded: Option<Arc<RgbaFrame>>,
}

impl Drop for Player {
    /// Stops the player's worker; the thread is detached and exits on its own.
    fn drop(&mut self) {
        if let Decode::Live(sync) = &self.decode {
            let (lock, cv) = &**sync;
            lock.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
            cv.notify_one();
        }
    }
}

/// The movies of the running game: their encoded bytes, facts and players.
pub struct Movies {
    infos: HashMap<String, Option<MediaInfo>>,
    /// Encoded movies by path, with the clock of their last use.
    bytes: HashMap<String, (Arc<[u8]>, u64)>,
    players: HashMap<VideoKey, Player>,
    clock: u64,
}

impl Movies {
    pub fn new() -> Movies {
        Movies {
            infos: HashMap::new(),
            bytes: HashMap::new(),
            players: HashMap::new(),
            clock: 0,
        }
    }

    /// The encoded movie at an asset path, read from the archives on first use.
    pub fn bytes(&mut self, path: &str, archives: &mut ArchiveDir) -> Option<Arc<[u8]>> {
        self.clock = self.clock.saturating_add(1);
        if let Some((bytes, used)) = self.bytes.get_mut(path) {
            *used = self.clock;
            return Some(bytes.clone());
        }
        let read = archives.read(path, |file| {
            log::warn!("missing archive {}", file.display());
        });
        let bytes: Arc<[u8]> = match read {
            Ok(Some(bytes)) => bytes.into(),
            Ok(None) => {
                log::warn!("movie `{path}` not found");
                return None;
            }
            Err(e) => {
                log::warn!("cannot read movie `{path}`: {e}");
                return None;
            }
        };
        self.bytes
            .insert(path.to_owned(), (bytes.clone(), self.clock));
        Some(bytes)
    }

    /// The facts of the movie at an asset path; `None` when it is missing or cannot be decoded.
    pub fn info(&mut self, path: &str, archives: &mut ArchiveDir) -> Option<MediaInfo> {
        if let Some(info) = self.infos.get(path) {
            return *info;
        }
        let info = self.bytes(path, archives).and_then(|bytes| {
            probe(bytes)
                .map_err(|e| log::warn!("cannot decode movie `{path}`: {e}"))
                .ok()
        });
        self.infos.insert(path.to_owned(), info);
        info
    }

    /// Decodes the frame of every requested movie and uploads the new ones; drops the players
    /// (and their textures) no longer requested.
    pub fn update(
        &mut self,
        requests: &[VideoRequest],
        assets: &AssetStore,
        mut renderer: Option<&mut Renderer>,
        archives: &mut ArchiveDir,
    ) {
        let mut seen = HashSet::with_capacity(requests.len());
        for req in requests {
            seen.insert(req.key);
            let path = assets.path(req.key.video);
            let Some(info) = self.info(path, archives) else {
                continue;
            };
            let duration = u64::from(info.duration_ms);
            let target = match req.looping {
                true if duration > 0 => req.position_ms.checked_rem(duration).unwrap_or(0),
                _ => req.position_ms,
            };
            if !self.players.contains_key(&req.key) {
                let player = self.open(path, target, archives);
                self.players.insert(req.key, player);
            }
            let Some(player) = self.players.get_mut(&req.key) else {
                continue;
            };
            let frame = match &mut player.decode {
                Decode::Failed => None,
                Decode::Live(sync) => {
                    let (lock, cv) = &**sync;
                    let mut shared = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    if shared.target != target {
                        shared.target = target;
                        cv.notify_one();
                    }
                    shared.frame.clone()
                }
            };
            let (Some(frame), Some(renderer)) = (frame, renderer.as_deref_mut()) else {
                continue;
            };
            if player
                .uploaded
                .as_ref()
                .is_some_and(|last| Arc::ptr_eq(last, &frame))
            {
                continue;
            }
            renderer.set_video_frame(req.key, frame.width, frame.height, &frame.rgba);
            player.uploaded = Some(frame);
        }
        let gone: Vec<VideoKey> = self
            .players
            .keys()
            .filter(|key| !seen.contains(key))
            .copied()
            .collect();
        for key in gone {
            self.players.remove(&key);
            if let Some(renderer) = renderer.as_deref_mut() {
                renderer.remove_video(key);
            }
        }
        self.evict();
    }

    /// A player for the movie at `path`, showing `target` once it has a frame.
    fn open(&mut self, path: &str, target: u64, archives: &mut ArchiveDir) -> Player {
        let failed = || Player {
            path: path.to_owned(),
            decode: Decode::Failed,
            uploaded: None,
        };
        let Some(bytes) = self.bytes(path, archives) else {
            return failed();
        };
        let mut decoder = match VideoDecoder::open(bytes) {
            Ok(decoder) => decoder,
            Err(e) => {
                log::warn!("cannot decode movie `{path}`: {e}");
                return failed();
            }
        };
        log::info!(target: "kagari::video", "movie `{path}`: {} decoding", decoder.backend());
        let frame = match decoder.frame_at(target) {
            Ok(frame) => frame,
            Err(e) => {
                log::warn!("cannot decode movie `{path}`: {e}");
                return failed();
            }
        };
        let sync: Handoff = Arc::new((
            Mutex::new(Shared {
                target,
                done: Some(target),
                stop: false,
                frame,
            }),
            Condvar::new(),
        ));
        let worker = sync.clone();
        let name = path.to_owned();
        let spawned = std::thread::Builder::new()
            .name("kagari-video".into())
            .spawn(move || decode_loop(decoder, &worker, &name));
        if let Err(e) = spawned {
            log::warn!("cannot start decoding movie `{path}`: {e}");
            return failed();
        }
        Player {
            path: path.to_owned(),
            decode: Decode::Live(sync),
            uploaded: None,
        }
    }

    /// Evicts the least recently used bytes no player holds while they exceed the budget.
    fn evict(&mut self) {
        let playing: HashSet<&str> = self.players.values().map(|p| p.path.as_str()).collect();
        let mut idle: Vec<(u64, String)> = self
            .bytes
            .iter()
            .filter(|(path, _)| !playing.contains(path.as_str()))
            .map(|(path, (_, used))| (*used, path.clone()))
            .collect();
        let mut total: u64 = self
            .bytes
            .values()
            .map(|(bytes, _)| bytes.len() as u64)
            .fold(0, u64::saturating_add);
        idle.sort_unstable();
        for (_, path) in idle {
            if total <= BYTES_BUDGET {
                break;
            }
            if let Some((bytes, _)) = self.bytes.remove(&path) {
                total = total.saturating_sub(bytes.len() as u64);
            }
        }
    }
}

/// A player's worker: decodes each new target and publishes the frame, until stopped or the
/// decoder fails.
fn decode_loop(mut decoder: VideoDecoder, sync: &Handoff, path: &str) {
    let (lock, cv) = &**sync;
    loop {
        let target = {
            let shared = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let shared = cv
                .wait_while(shared, |s| s.done == Some(s.target) && !s.stop)
                .unwrap_or_else(PoisonError::into_inner);
            if shared.stop {
                return;
            }
            shared.target
        };
        match decoder.frame_at(target) {
            Ok(frame) => {
                let mut shared = lock.lock().unwrap_or_else(PoisonError::into_inner);
                shared.done = Some(target);
                if frame.is_some() {
                    shared.frame = frame;
                }
            }
            Err(e) => {
                log::warn!("cannot decode movie `{path}`: {e}");
                return;
            }
        }
    }
}
