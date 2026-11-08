//! The Kagari Android runtime: a winit `NativeActivity` driving the engine, the renderer and
//! the audio, as the desktop runtime does.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

pub mod clock;
pub mod colour;
pub mod pcm;
pub mod pick;
pub mod touch;

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod audio;
#[cfg(target_os = "android")]
mod gpu;
#[cfg(target_os = "android")]
mod input;
#[cfg(target_os = "android")]
mod jni_bridge;
#[cfg(target_os = "android")]
mod media;
#[cfg(target_os = "android")]
mod movies;
#[cfg(target_os = "android")]
mod platform;
#[cfg(target_os = "android")]
mod sounds;
#[cfg(target_os = "android")]
mod store;

#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

/// The entry point the native-activity glue calls on its own thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    use android_logger::{Config, FilterBuilder};
    use log::LevelFilter;

    android_logger::init_once(
        Config::default()
            .with_tag("kagari")
            .with_max_level(LevelFilter::Info)
            .with_filter(FilterBuilder::new().parse("kagari=info,wgpu=warn").build()),
    );
    app::run(app);
}
