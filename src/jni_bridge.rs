//! Calls between the Rust runtime and `GameActivity` (Kotlin).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use jni::objects::{JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use winit::platform::android::activity::{AndroidApp, AndroidAppWaker};

/// The back gestures `GameActivity` reported (API 33+) since the last frame.
static BACK_PRESSES: AtomicU32 = AtomicU32::new(0);

/// Set when `GameActivity.onDestroy` began; it waits for `android_main` to return.
static DESTROYED: AtomicBool = AtomicBool::new(false);

/// Wakes the event loop from the UI thread.
static WAKER: OnceLock<AndroidAppWaker> = OnceLock::new();

/// Lets the activity's callbacks wake the event loop.
pub fn init(app: &AndroidApp) {
    let _ = WAKER.set(app.create_waker());
}

fn wake() {
    if let Some(waker) = WAKER.get() {
        waker.wake();
    }
}

/// Runs `f` with the activity on this thread, attached to the VM.
fn with_activity<T>(
    app: &AndroidApp,
    f: impl for<'local> FnOnce(&mut Env<'local>, &JObject<'local>) -> jni::errors::Result<T>,
) -> Result<T, String> {
    // SAFETY: the pointer is the process's `JavaVM`, valid for the life of the process.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| {
        // SAFETY: a global reference the native-activity glue holds until the activity is
        // destroyed, which waits for `android_main` to return; `JObject` never deletes it.
        let activity = unsafe { JObject::from_raw(env, app.activity_as_ptr().cast()) };
        f(env, &activity)
    })
    .map_err(|e: jni::errors::Error| e.to_string())
}

/// Logs a failed call; the runtime goes on without it.
fn logged(method: &str, result: Result<(), String>) {
    if let Err(e) = result {
        log::error!(target: "kagari::android", "GameActivity.{method}: {e}");
    }
}

/// The directory of the game to run, passed by the launcher.
pub fn game_dir(app: &AndroidApp) -> Result<PathBuf, String> {
    let dir = with_activity(app, |env, activity| {
        let value = env
            .call_method(
                activity,
                jni_str!("gameDir"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        env.cast_local::<JString>(value)?.try_to_string(env)
    })
    .map_err(|e| format!("GameActivity.gameDir: {e}"))?;
    if dir.is_empty() {
        return Err("no game directory was passed to the game activity".to_owned());
    }
    Ok(PathBuf::from(dir))
}

/// Locks the screen to landscape or portrait (either way up), to match the game's resolution.
pub fn lock_orientation(app: &AndroidApp, landscape: bool) {
    let result = with_activity(app, |env, activity| {
        env.call_method(
            activity,
            jni_str!("lockOrientation"),
            jni_sig!("(Z)V"),
            &[JValue::Bool(landscape)],
        )
        .map(drop)
    });
    logged("lockOrientation", result);
}

/// Shows `message` in a dialog whose button closes the game.
pub fn show_fatal_error(app: &AndroidApp, message: &str) {
    let result = with_activity(app, |env, activity| {
        let message = env.new_string(message)?;
        env.call_method(
            activity,
            jni_str!("showFatalError"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&message)],
        )
        .map(drop)
    });
    logged("showFatalError", result);
}

/// Closes the game activity (its process ends with it).
pub fn finish(app: &AndroidApp) {
    let result = with_activity(app, |env, activity| {
        env.call_method(activity, jni_str!("finishGame"), jni_sig!("()V"), &[])
            .map(drop)
    });
    logged("finishGame", result);
}

/// The back gesture on API 33+, from `GameActivity`'s `OnBackInvokedCallback` on the UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_club_suitsu31_kagari_simulator_GameActivity_nativeOnBack(
    _env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
) {
    BACK_PRESSES.fetch_add(1, Ordering::Relaxed);
    wake();
}

/// The back gestures since the last call.
pub fn take_back_presses() -> u32 {
    BACK_PRESSES.swap(0, Ordering::Relaxed)
}

/// `GameActivity.onDestroy`, on the UI thread, before it waits for `android_main` to return.
#[unsafe(no_mangle)]
pub extern "system" fn Java_club_suitsu31_kagari_simulator_GameActivity_nativeOnDestroy(
    _env: *mut jni::sys::JNIEnv,
    _this: jni::sys::jobject,
) {
    DESTROYED.store(true, Ordering::Relaxed);
    wake();
}

/// Whether the activity is being destroyed: the event loop must end.
pub fn destroyed() -> bool {
    DESTROYED.load(Ordering::Relaxed)
}
