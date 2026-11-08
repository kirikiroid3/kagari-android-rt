//! [`AndroidPlatform`]: the engine's view of this device.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use kagari_engine::{AssetStore, Frame, Platform, Timestamp, VideoInfo};

use kagari_archive::archive::ArchiveDir;

use crate::gpu::Gpu;
use crate::movies::Movies;
use crate::sounds::{Sound, SoundCache};
use crate::store::Store;

pub struct AndroidPlatform {
    archives: ArchiveDir,
    store: Store,
    pub sounds: SoundCache,
    pub movies: Movies,
    /// Set once the window and the GPU exist.
    pub gpu: Option<Gpu>,
}

impl AndroidPlatform {
    pub fn new(game_dir: &Path, store: Store) -> AndroidPlatform {
        AndroidPlatform {
            archives: ArchiveDir::new(game_dir),
            store,
            sounds: SoundCache::default(),
            movies: Movies::new(),
            gpu: None,
        }
    }

    /// The sound at an asset path, read from the archives on first use.
    pub fn sound(&mut self, path: &str) -> Option<Sound> {
        if let Some(sound) = self.sounds.get(path) {
            return Some(sound);
        }
        if self.sounds.failed(path) {
            return None;
        }
        match self.read_asset(path) {
            Ok(Some(bytes)) => self.sounds.load(path, bytes),
            Ok(None) => {
                log::warn!("audio `{path}` not found");
                None
            }
            Err(e) => {
                log::warn!("cannot read audio `{path}`: {e}");
                None
            }
        }
    }

    /// The encoded movie at an asset path, for its audio.
    pub fn movie_bytes(&mut self, path: &str) -> Option<Arc<[u8]>> {
        self.movies.bytes(path, &mut self.archives)
    }

    /// Decodes and uploads the movie frames `frame` draws.
    pub fn update_movies(&mut self, frame: &Frame, assets: &AssetStore) {
        self.movies.update(
            &frame.video_requests(),
            assets,
            self.gpu.as_mut().map(|g| &mut g.renderer),
            &mut self.archives,
        );
    }
}

impl Platform for AndroidPlatform {
    fn read_store(&mut self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.store.read_store(name)
    }

    fn write_store(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.store.write_store(name, bytes)
    }

    fn delete_store(&mut self, name: &str) -> io::Result<()> {
        self.store.delete_store(name)
    }

    fn read_asset(&mut self, path: &str) -> io::Result<Option<Vec<u8>>> {
        self.archives.read(path, |file| {
            log::warn!("missing archive {}", file.display());
        })
    }

    fn audio_duration_ms(&mut self, path: &str, bytes: &[u8]) -> Option<u32> {
        let sound = match self.sounds.get(path) {
            Some(sound) => sound,
            None => self.sounds.load(path, bytes.to_vec())?,
        };
        sound.duration_ms()
    }

    fn video_info(&mut self, path: &str) -> Option<VideoInfo> {
        self.movies
            .info(path, &mut self.archives)
            .map(|i| VideoInfo {
                width: i.width,
                height: i.height,
                duration_ms: i.duration_ms,
                has_audio: i.has_audio,
            })
    }

    fn render_thumbnail(
        &mut self,
        frame: &Frame,
        assets: &AssetStore,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        let Some(gpu) = self.gpu.as_mut() else {
            log::warn!("thumbnail requested before the GPU is ready");
            return None;
        };
        let rgba = gpu.renderer.render_to_rgba(frame, assets, width, height);
        (!rgba.is_empty()).then_some(rgba)
    }

    fn local_time(&self) -> Timestamp {
        let now = jiff::Zoned::now();
        Timestamp {
            year: u16::try_from(now.year()).unwrap_or(0),
            month: u8::try_from(now.month()).unwrap_or(1),
            day: u8::try_from(now.day()).unwrap_or(1),
            hour: u8::try_from(now.hour()).unwrap_or(0),
            minute: u8::try_from(now.minute()).unwrap_or(0),
            second: u8::try_from(now.second()).unwrap_or(0),
        }
    }

    fn random_seed(&mut self) -> u64 {
        // Truncating the nanoseconds keeps their fast-changing low bits.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        nanos ^ u64::from(std::process::id())
    }

    fn set_title(&mut self, title: &str) {
        self.store.set_title(title);
    }
}
