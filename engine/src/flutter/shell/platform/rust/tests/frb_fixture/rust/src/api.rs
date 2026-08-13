// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use flutter_plugin_sdk::{
    FlutterRustPlugin, MainThreadDispatcher, PixelBufferTexture, PluginError, PluginRegistrar,
    Result as PluginResult, TextureDescriptor, TextureFormat, WgpuTexture, gpu::wgpu,
};
use flutter_rust_bridge::frb;

const TEXTURE_WIDTH: u32 = 256;
const TEXTURE_HEIGHT: u32 = 256;

static DISPATCHER: OnceLock<MainThreadDispatcher> = OnceLock::new();
static TEXTURES: OnceLock<Arc<TextureState>> = OnceLock::new();
static SYNCHRONOUS_CALL_ACTIVE: AtomicBool = AtomicBool::new(false);
static SYNCHRONOUS_CALLBACK_DEFERRED: AtomicBool = AtomicBool::new(false);

struct TextureState {
    gpu: Arc<WgpuTexture>,
    pixels: Arc<PixelBufferTexture>,
    producer_started: AtomicBool,
    gpu_frames: AtomicU64,
    pixel_frames: AtomicU64,
}

struct FixturePlugin {}

#[derive(Debug, Clone)]
pub struct DispatchProbe {
    pub caller_was_main_thread: bool,
    pub callback_was_main_thread: bool,
    pub synchronous_callback_was_deferred: bool,
}

#[derive(Debug, Clone)]
pub struct TextureProbe {
    pub gpu_texture_id: i64,
    pub pixel_texture_id: i64,
    pub gpu_frames: u64,
    pub pixel_frames: u64,
}

impl FlutterRustPlugin for FixturePlugin {
    fn register(&self, registrar: &mut PluginRegistrar) -> PluginResult<()> {
        let descriptor = TextureDescriptor {
            width: TEXTURE_WIDTH,
            height: TEXTURE_HEIGHT,
            format: TextureFormat::Rgba8Unorm,
        };
        let gpu = registrar.gpu()?;
        let state = Arc::new(TextureState {
            gpu: Arc::new(gpu.create_texture(descriptor)?),
            pixels: Arc::new(gpu.create_pixel_buffer_texture(descriptor)?),
            producer_started: AtomicBool::new(false),
            gpu_frames: AtomicU64::new(0),
            pixel_frames: AtomicU64::new(0),
        });

        DISPATCHER
            .set(registrar.main_thread_dispatcher().clone())
            .map_err(|_| PluginError::Unsupported)?;
        TEXTURES.set(state).map_err(|_| PluginError::Unsupported)?;
        Ok(())
    }
}

pub(crate) fn register(registrar: &mut PluginRegistrar) -> PluginResult<()> {
    FixturePlugin {}.register(registrar)
}

fn dispatcher() -> &'static MainThreadDispatcher {
    DISPATCHER
        .get()
        .expect("FRB fixture called before plugin registration")
}

#[frb(sync)]
pub fn synchronous_probe() -> bool {
    let dispatcher = dispatcher();
    let caller_was_main_thread = dispatcher.is_main_thread();
    SYNCHRONOUS_CALL_ACTIVE.store(true, Ordering::Release);
    dispatcher
        .dispatch(|| {
            SYNCHRONOUS_CALLBACK_DEFERRED.store(
                !SYNCHRONOUS_CALL_ACTIVE.load(Ordering::Acquire),
                Ordering::Release,
            );
        })
        .expect("fixture main-thread dispatch rejected");
    SYNCHRONOUS_CALL_ACTIVE.store(false, Ordering::Release);
    caller_was_main_thread
}

pub fn background_dispatch_probe() -> DispatchProbe {
    let dispatcher = dispatcher();
    let caller_was_main_thread = dispatcher.is_main_thread();
    let (sender, receiver) = mpsc::sync_channel(1);
    let callback_dispatcher = dispatcher.clone();
    dispatcher
        .dispatch(move || {
            let _ = sender.send(callback_dispatcher.is_main_thread());
        })
        .expect("fixture main-thread dispatch rejected");
    let callback_was_main_thread = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("fixture main-thread callback timed out");
    DispatchProbe {
        caller_was_main_thread,
        callback_was_main_thread,
        synchronous_callback_was_deferred: SYNCHRONOUS_CALLBACK_DEFERRED.load(Ordering::Acquire),
    }
}

#[frb(sync)]
pub fn texture_probe() -> TextureProbe {
    let state = TEXTURES
        .get()
        .expect("FRB fixture called before texture registration");
    TextureProbe {
        gpu_texture_id: state.gpu.texture_id(),
        pixel_texture_id: state.pixels.texture_id(),
        gpu_frames: state.gpu_frames.load(Ordering::Acquire),
        pixel_frames: state.pixel_frames.load(Ordering::Acquire),
    }
}

#[frb(sync)]
pub fn start_texture_producer() -> bool {
    let state = TEXTURES
        .get()
        .expect("FRB fixture called before texture registration");
    if state
        .producer_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let producer_state = Arc::clone(state);
    thread::Builder::new()
        .name("flutter-rust-frb-textures".to_owned())
        .spawn(move || produce_texture_frames(producer_state))
        .expect("failed to start FRB texture producer");
    true
}

fn produce_texture_frames(state: Arc<TextureState>) {
    let mut phase = 0_u64;
    loop {
        let gpu_result = render_gpu_frame(&state, phase);
        let pixel_result = render_pixel_frame(&state, phase);
        if matches!(gpu_result, Err(PluginError::Shutdown))
            || matches!(pixel_result, Err(PluginError::Shutdown))
        {
            return;
        }
        phase = phase.wrapping_add(1);
        thread::sleep(Duration::from_millis(16));
    }
}

fn render_gpu_frame(state: &TextureState, phase: u64) -> PluginResult<()> {
    let mut frame = match state.gpu.try_next_frame() {
        Ok(frame) => frame,
        Err(PluginError::Busy) => return Ok(()),
        Err(error) => return Err(error),
    };
    let angle = phase as f64 * 0.04;
    let color = wgpu::Color {
        r: angle.sin() * 0.5 + 0.5,
        g: (angle + 2.094).sin() * 0.5 + 0.5,
        b: (angle + 4.189).sin() * 0.5 + 0.5,
        a: 1.0,
    };
    frame.render(move |_, encoder, view| {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("FRB fixture wgpu texture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    })?;
    frame.present()?;
    state.gpu_frames.fetch_add(1, Ordering::Release);
    Ok(())
}

fn render_pixel_frame(state: &TextureState, phase: u64) -> PluginResult<()> {
    let mut frame = match state.pixels.try_next_frame() {
        Ok(frame) => frame,
        Err(PluginError::Busy) => return Ok(()),
        Err(error) => return Err(error),
    };
    let angle = phase as f64 * 0.055;
    let rgba = [
        ((angle + 4.189).sin() * 127.5 + 127.5) as u8,
        (angle.sin() * 127.5 + 127.5) as u8,
        ((angle + 2.094).sin() * 127.5 + 127.5) as u8,
        255,
    ];
    frame.write_pixels(move |pixels, row_bytes| {
        for row in pixels.chunks_mut(row_bytes).take(TEXTURE_HEIGHT as usize) {
            for pixel in row[..TEXTURE_WIDTH as usize * 4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&rgba);
            }
        }
    })?;
    frame.present()?;
    state.pixel_frames.fetch_add(1, Ordering::Release);
    Ok(())
}
