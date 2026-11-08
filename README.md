# kagari-android-rt
Kagari Runtime for Android

The Kagari Simulator app (`club.suitsu31.kagari.simulator`): import a game built by
`kagari build` (`game.kagari` plus its `*.kiarchive` files) from a ZIP file or a folder, and play
it as the native desktop runtime (`native_v0`) would. Android is a tier-2 platform: no debugger,
no headless mode.

- `src/`: the Rust runtime (`libkagari_android.so`), a winit `NativeActivity` on the Kagari
  engine and renderer (registry `Suitsu31`, pinned `=0.3.1`), with kira audio and NDK
  MediaCodec movies.
- `app/`: the launcher (Jetpack Compose, Material 3: game list and import) and `GameActivity`,
  which runs one game in the `:game` process.

## Requirements

- Android SDK with platform 37 (Compose needs it to compile; the app targets 36), build-tools
  37.0.0 and NDK 30.0.16248370 (`local.properties`: `sdk.dir=…`; `.envrc` sets `ANDROID_HOME` and
  `ANDROID_NDK_HOME`).
- Rust 1.99 with the `aarch64-linux-android` and `x86_64-linux-android` targets
  (`rust-toolchain.toml`), and `cargo install cargo-ndk --version 4.1.2 --locked`.
- JDK 17 or later.

## Build

```sh
cargo test                   # the platform-neutral modules, on the host
./gradlew assembleDebug      # app/build/outputs/apk/debug/app-debug.apk
./gradlew assembleRelease    # R8, signed with the debug key for now
```

Gradle runs `cargo ndk` for `arm64-v8a` and `x86_64`. A device or emulator needs Vulkan (the
manifest requires 1.0.3) and API 29 or later. Logs: `adb logcat -s kagari:*`.

Pushing a tag builds the release APK on a Blacksmith runner (`.github/workflows/release.yml`, no
caches); it is the run's `kagari-simulator-<tag>` artifact.

## Playing

Import a game with **Import** › **ZIP file** (the `game.kagari` nearest the archive root is used,
with the archives beside it) or **Folder** (its direct children), or open a ZIP with the app. An
import runs in the background with its progress at the top of the list. Tap a game to play it; a
game left running is marked and tapping it resumes it. Its ⋮ menu (or a long press) deletes it (its
saves stay).

| Touch | Mouse input |
|---|---|
| Tap | Left click |
| Long press (0.5 s) | Right click |
| One-finger drag | Left button held while moving |
| Two-finger vertical drag | Wheel (down = wheel up) |
| Back gesture / key | Escape |

Gamepad buttons map to `PadButton`s, the D-pad to the arrow keys. In the background the game's
clock and audio pause.
