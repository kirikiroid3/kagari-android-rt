//! The wgpu adapter, device and renderer.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use kagari_render::Renderer;

/// The [`Gpu::flags`] value after an out-of-memory error.
const OUT_OF_MEMORY: u8 = 1;
/// The [`Gpu::flags`] value after the device was lost.
const LOST: u8 = 2;

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: Renderer,
    /// The format `renderer` presents into: the surface's.
    pub format: wgpu::TextureFormat,
    /// 0 ok, 1 out of memory, 2 device lost.
    pub flags: Arc<AtomicU8>,
    lost: Arc<Mutex<String>>,
}

/// Picks an adapter (compatible with `surface` when given), opens the device and builds the
/// renderer for the surface's preferred format.
pub fn create(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Gpu, String> {
    let options = |force_fallback_adapter| wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter,
        compatible_surface: surface,
        apply_limit_buckets: false,
    };
    let adapter = match pollster::block_on(instance.request_adapter(&options(false))) {
        Ok(adapter) => adapter,
        Err(_) => pollster::block_on(instance.request_adapter(&options(true)))
            .map_err(|e| format!("no usable GPU adapter: {e}"))?,
    };
    let info = adapter.get_info();
    log::info!("using GPU adapter {} ({:?})", info.name, info.backend);
    let format = match surface {
        Some(surface) => {
            let caps = surface.get_capabilities(&adapter);
            caps.formats
                .iter()
                .copied()
                .find(wgpu::TextureFormat::is_srgb)
                .or_else(|| caps.formats.first().copied())
                .ok_or_else(|| "the window surface supports no texture format".to_owned())?
        }
        None => wgpu::TextureFormat::Rgba8UnormSrgb,
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("kagari"),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        // Small allocator blocks: the default (`Performance`) maps 64–128 MiB host-visible
        // blocks, resident in the process, for the staging copies of a few textures.
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    }))
    .map_err(|e| format!("cannot open the GPU device: {e}"))?;

    let flags = Arc::new(AtomicU8::new(0));
    let lost = Arc::new(Mutex::new(String::new()));
    let oom = flags.clone();
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
        if let wgpu::Error::OutOfMemory { .. } = e {
            log::error!("GPU out of memory");
            oom.store(OUT_OF_MEMORY, Ordering::SeqCst);
        } else {
            log::error!("GPU error: {e}");
        }
    }));
    let lost_flag = flags.clone();
    let lost_message = lost.clone();
    device.set_device_lost_callback(move |reason, msg| {
        log::error!("GPU device lost ({reason:?}): {msg}");
        if let Ok(mut m) = lost_message.lock() {
            *m = msg;
        }
        lost_flag.store(LOST, Ordering::SeqCst);
    });

    let renderer = Renderer::new(&device, &queue, format);
    Ok(Gpu {
        instance,
        device,
        queue,
        renderer,
        format,
        flags,
        lost,
    })
}

impl Gpu {
    /// An error once the device ran out of memory or was lost; it is never recreated.
    pub fn check(&self) -> Result<(), String> {
        match self.flags.load(Ordering::SeqCst) {
            OUT_OF_MEMORY => Err("GPU out of memory".to_owned()),
            LOST => {
                let msg = self.lost.lock().map(|m| m.clone()).unwrap_or_default();
                Err(format!("GPU device lost: {msg}"))
            }
            _ => Ok(()),
        }
    }

    /// The surface configuration of a `width`×`height` window (each at least 1).
    pub fn surface_config(&self, width: u32, height: u32) -> wgpu::SurfaceConfiguration {
        wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        }
    }
}
