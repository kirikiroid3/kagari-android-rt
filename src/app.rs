//! The Android runtime: a winit event loop on the native activity driving the engine, the
//! renderer and the audio, as the desktop runtime's window does.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kagari_engine::{BootError, Engine, Frame, InputEvent, Key, MouseButton};
use kagari_render::{Renderer, Viewport};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::platform::android::activity::AndroidApp;
use winit::window::{Window, WindowId};

use crate::audio::AudioOut;
use crate::clock::Clock;
use crate::gpu;
use crate::input::HeldKeys;
use crate::jni_bridge;
use crate::platform::AndroidPlatform;
use crate::store::Store;
use crate::touch::{Button, Gesture, Phase, Touch};

/// The wake-up interval while nothing can be drawn.
const IDLE_WAIT: Duration = Duration::from_millis(16);

/// The bytecode file `kagari build` writes.
const PROGRAM: &str = "game.kagari";

/// Boots the game the launcher chose and runs until the activity goes. A boot failure is shown
/// in a dialog that closes the activity.
pub fn run(app: AndroidApp) {
    jni_bridge::init(&app);
    let sdk = app.config().sdk_version();
    let saves_root = app.internal_data_path().map(|dir| dir.join("saves"));
    let game_dir = jni_bridge::game_dir(&app);
    let mut platform = AndroidPlatform::new(
        game_dir.as_deref().unwrap_or(Path::new("")),
        Store::new(saves_root),
    );
    let booted = game_dir.and_then(|dir| boot(&dir, &mut platform));
    let (engine, pending_fatal) = match booted {
        Ok(engine) => (Some(engine), None),
        Err(message) => (None, Some(message)),
    };
    let design = engine
        .as_ref()
        .map_or((1, 1), |engine| engine.window().resolution);
    let landscape = design.0 >= design.1;
    if engine.is_some() {
        jni_bridge::lock_orientation(&app, landscape);
    }

    let event_loop = match EventLoop::builder().with_android_app(app.clone()).build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            let message = format!("cannot open a window: {e}");
            log::error!(target: "kagari::android", "{message}");
            jni_bridge::show_fatal_error(&app, &message);
            return;
        }
    };
    let mut descriptor =
        wgpu::InstanceDescriptor::new_with_display_handle(Box::new(event_loop.owned_display_handle()));
    descriptor.backends = wgpu::Backends::VULKAN;
    // No object labels (`VK_EXT_debug_utils`), even in debug builds: the emulator's Vulkan
    // driver crashes naming objects, and devices have no debugger attached to read them.
    descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);
    let instance = wgpu::Instance::new(descriptor);
    let audio = match engine {
        Some(_) => AudioOut::new(),
        None => AudioOut::silent(),
    };
    let mut state = App {
        app,
        sdk,
        engine,
        platform,
        audio,
        keys: HeldKeys::default(),
        touch: Touch::new(1.0),
        touch_origin: Instant::now(),
        clock: Clock::new(),
        instance: Some(instance),
        window: None,
        surface: None,
        design,
        landscape,
        idle: false,
        stopped: false,
        pending_fatal,
    };
    if let Err(e) = event_loop.run_app(&mut state) {
        log::error!(target: "kagari::android", "the event loop failed: {e}");
    }
}

/// Reads `game.kagari` from `dir` and boots it at time 0.
fn boot(dir: &Path, platform: &mut AndroidPlatform) -> Result<Engine, String> {
    let bytes =
        fs::read(dir.join(PROGRAM)).map_err(|e| format!("cannot read {PROGRAM}: {e}"))?;
    Engine::boot(&bytes, platform, 0).map_err(|e| match e {
        BootError::Trap(message) => format!("game stopped: {message}"),
        other => format!("cannot start the game: {other}"),
    })
}

/// Uploads every save thumbnail `frame` draws whose generation the renderer lacks.
fn upload_thumbnails(renderer: &mut Renderer, engine: &Engine, frame: &Frame) {
    for (slot, _) in renderer.stale_thumbnails(frame) {
        if let Some((generation, rgba, width, height)) = engine.thumbnail(slot) {
            renderer.set_thumbnail(slot, generation, width, height, &rgba);
        }
    }
}

struct App {
    app: AndroidApp,
    /// The device's API level.
    sdk: i32,
    /// `None` once the game failed to start or a fatal error stopped it.
    engine: Option<Engine>,
    platform: AndroidPlatform,
    audio: AudioOut,
    keys: HeldKeys,
    touch: Touch,
    /// Time 0 of the touch timestamps (the long press runs on wall time).
    touch_origin: Instant,
    /// Engine time, frozen while the app is in the background.
    clock: Clock,
    /// Kept until the first `resumed` creates the GPU from it.
    instance: Option<wgpu::Instance>,
    /// Kept across suspensions; only the surface goes.
    window: Option<Arc<Window>>,
    /// Present between `resumed` and `suspended`.
    surface: Option<(wgpu::Surface<'static>, wgpu::SurfaceConfiguration)>,
    /// The design resolution (`System::resolution`).
    design: (u32, u32),
    /// The orientation the screen is locked to.
    landscape: bool,
    /// Nothing could be drawn this frame: wait instead of spinning.
    idle: bool,
    /// The game stopped (its crash was logged); the crash frame keeps drawing.
    stopped: bool,
    /// A boot failure, shown once the activity is up.
    pending_fatal: Option<String>,
}

impl App {
    fn touch_ms(&self) -> u64 {
        u64::try_from(self.touch_origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn send(&mut self, event: InputEvent) {
        let now = self.clock.now_ms(Instant::now());
        if let Some(engine) = self.engine.as_mut() {
            engine.input(event, &mut self.platform, now);
        }
    }

    /// Shows `message` in a dialog that closes the activity, and stops the game.
    fn fatal(&mut self, message: String) {
        log::error!(target: "kagari::android", "{message}");
        jni_bridge::show_fatal_error(&self.app, &message);
        self.engine = None;
        self.audio = AudioOut::silent();
    }

    /// The engine events of touch gestures.
    fn dispatch(&mut self, gestures: Vec<Gesture>) {
        for gesture in gestures {
            let event = match gesture {
                Gesture::Move(x, y) => {
                    let Some((_, config)) = &self.surface else {
                        continue;
                    };
                    let viewport = Viewport::letterbox(self.design, (config.width, config.height));
                    let (x, y) = viewport.to_design(self.design, x, y);
                    InputEvent::MouseMove { x, y }
                }
                Gesture::Press(button) => InputEvent::MouseButton {
                    button: mouse_button(button),
                    pressed: true,
                },
                Gesture::Release(button) => InputEvent::MouseButton {
                    button: mouse_button(button),
                    pressed: false,
                },
                Gesture::Wheel(delta_y) => InputEvent::Wheel { delta_y },
                Gesture::Leave => InputEvent::MouseMove { x: -1.0, y: -1.0 },
            };
            self.send(event);
        }
    }

    /// Creates the window and the GPU the first time, and a surface for the current native
    /// window every time.
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let window = match &self.window {
            Some(window) => window.clone(),
            None => {
                let window = Arc::new(
                    event_loop
                        .create_window(Window::default_attributes())
                        .map_err(|e| format!("cannot create the window: {e}"))?,
                );
                self.touch = Touch::new(window.scale_factor());
                self.window = Some(window.clone());
                window
            }
        };
        let size = window.inner_size();
        let surface = match self.platform.gpu.as_ref() {
            Some(gpu) => gpu.instance.create_surface(window.clone()),
            None => {
                let instance = self
                    .instance
                    .take()
                    .ok_or_else(|| "the GPU instance is gone".to_owned())?;
                let surface = instance.create_surface(window.clone());
                if let Ok(surface) = &surface {
                    self.platform.gpu = Some(gpu::create(instance, Some(surface))?);
                }
                surface
            }
        }
        .map_err(|e| format!("cannot create the window surface: {e}"))?;
        let Some(gpu) = self.platform.gpu.as_ref() else {
            return Err("the GPU is gone".to_owned());
        };
        let config = gpu.surface_config(size.width, size.height);
        surface.configure(&gpu.device, &config);
        self.surface = Some((surface, config));
        window.request_redraw();
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (Some((surface, config)), Some(gpu)) =
            (self.surface.as_mut(), self.platform.gpu.as_ref())
        else {
            return;
        };
        if width == 0 || height == 0 {
            self.idle = true;
            return;
        }
        config.width = width;
        config.height = height;
        surface.configure(&gpu.device, config);
    }

    /// One frame: back gestures and long presses, update, audio, window state, then draw and
    /// present.
    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        if self.engine.is_none() || self.surface.is_none() {
            return;
        }
        let now = self.clock.now_ms(Instant::now());
        for _ in 0..jni_bridge::take_back_presses() {
            for pressed in [true, false] {
                self.send(InputEvent::Key {
                    key: Key::Escape,
                    pressed,
                });
            }
        }
        let gestures = self.touch.tick(self.touch_ms());
        self.dispatch(gestures);
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        engine.update(&mut self.platform, now);
        if !self.stopped
            && let Some(message) = engine.crash()
        {
            log::error!(target: "kagari::android", "game stopped: {message}");
            self.stopped = true;
        }
        if let Some(Err(message)) = self.platform.gpu.as_ref().map(gpu::Gpu::check) {
            self.fatal(message);
            return;
        }
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let commands = engine.take_audio();
        self.audio.apply(commands, &mut self.platform);

        let ws = engine.window();
        if ws.quit {
            jni_bridge::finish(&self.app);
            event_loop.exit();
            return;
        }
        // Title and fullscreen do not apply: the game is always immersive.
        self.design = ws.resolution;
        let landscape = ws.resolution.0 >= ws.resolution.1;
        if landscape != self.landscape {
            self.landscape = landscape;
            jni_bridge::lock_orientation(&self.app, landscape);
        }

        let frame = engine.frame(now);
        if let Some(gpu) = self.platform.gpu.as_mut() {
            upload_thumbnails(&mut gpu.renderer, engine, &frame);
        }
        self.platform.update_movies(&frame, engine.assets());
        if !self.present(&frame)
            && frame.capture_still
            && let (Some(gpu), Some(engine)) = (self.platform.gpu.as_mut(), self.engine.as_ref())
        {
            // The engine took the still from this frame and draws `Node::Still` from now on:
            // capture it even though nothing reached the screen.
            gpu.renderer.render_offscreen(&frame, engine.assets());
        }
    }

    /// Draws `frame` into the window and presents it; false when nothing was drawn.
    fn present(&mut self, frame: &Frame) -> bool {
        let (Some(engine), Some((surface, config)), Some(gpu)) = (
            self.engine.as_ref(),
            self.surface.as_ref(),
            self.platform.gpu.as_mut(),
        ) else {
            return false;
        };
        let texture = match surface.get_current_texture() {
            // Pre-rotation makes Vulkan report every frame as suboptimal on Android:
            // reconfiguring would not change that.
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                t
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.idle = true;
                return false;
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(&gpu.device, config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.recreate_surface();
                return false;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error; frame skipped");
                return false;
            }
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let viewport = Viewport::letterbox(self.design, (config.width, config.height));
        gpu.renderer.render(frame, engine.assets(), &view, viewport);
        gpu.queue.present(texture);
        true
    }

    /// Replaces a lost surface. The old one is dropped first: a window may hold only one
    /// surface at a time (Vulkan `NATIVE_WINDOW_IN_USE`).
    fn recreate_surface(&mut self) {
        let Some((surface, config)) = self.surface.take() else {
            return;
        };
        drop(surface);
        let (Some(gpu), Some(window)) = (self.platform.gpu.as_ref(), self.window.as_ref()) else {
            return;
        };
        match gpu.instance.create_surface(window.clone()) {
            Ok(surface) => {
                surface.configure(&gpu.device, &config);
                self.surface = Some((surface, config));
            }
            Err(e) => self.fatal(format!("cannot create the window surface: {e}")),
        }
    }
}

fn mouse_button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Right => MouseButton::Right,
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(message) = self.pending_fatal.take() {
            self.fatal(message);
            return;
        }
        if self.engine.is_none() {
            return;
        }
        if let Err(message) = self.open(event_loop) {
            self.fatal(message);
            return;
        }
        let now = Instant::now();
        self.clock.start(now);
        self.clock.resume(now);
        self.audio.set_suspended(false);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The native window is going: its surface must go first. The window and the GPU (and
        // the renderer's textures) stay.
        self.surface = None;
        self.clock.pause(Instant::now());
        self.audio.set_suspended(true);
        let gestures = self.touch.reset();
        self.dispatch(gestures);
        self.keys.clear();
        self.send(InputEvent::Focus(false));
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.touch.set_scale(scale_factor);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(e) = self.keys.key(&event, self.sdk) {
                    self.send(e);
                }
            }
            WindowEvent::Touch(touch) => {
                let phase = match touch.phase {
                    TouchPhase::Started => Phase::Started,
                    TouchPhase::Moved => Phase::Moved,
                    TouchPhase::Ended => Phase::Ended,
                    TouchPhase::Cancelled => Phase::Cancelled,
                };
                let now = self.touch_ms();
                let gestures =
                    self.touch
                        .event(touch.id, phase, touch.location.x, touch.location.y, now);
                self.dispatch(gestures);
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.keys.clear();
                    let gestures = self.touch.reset();
                    self.dispatch(gestures);
                }
                self.send(InputEvent::Focus(focused));
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if jni_bridge::destroyed() {
            // The activity is being destroyed and waits for `android_main` to return.
            event_loop.exit();
            return;
        }
        if self.engine.is_none() || self.surface.is_none() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if self.idle {
            self.idle = false;
            let until = Instant::now().checked_add(IDLE_WAIT);
            event_loop.set_control_flow(until.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// The activity is going: autosave and write the stores, as closing the desktop window
    /// does (nothing once the game quit by itself).
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(engine) = self.engine.as_mut() {
            engine.quit(&mut self.platform);
        }
    }
}
