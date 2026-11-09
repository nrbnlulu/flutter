//! Phase 0 offscreen target/readback and experimental private-engine broker.
//!
//! Raw Impeller access is confined to OffscreenBroker's acquire/present pair.
//! It restores COLOR_ATTACHMENT_OPTIMAL before wgpu can access the image again;
//! arbitrary raw writes through OffscreenReadback would invalidate tracking.

use crate::GpuContext;
use std::{sync::Arc, time::Duration};

/// One owned RGBA8Unorm render target and one reusable, padded staging buffer.
/// Readback blocks the caller; use only on a probe/capture worker, never the
/// merged UI/platform thread. No surface or native window is created.
pub struct OffscreenReadback {
    context: Arc<GpuContext>,
    texture: wgpu::Texture,
    staging: wgpu::Buffer,
    layout: ReadbackLayout,
    width: u32,
    height: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct ReadbackLayout {
    row_bytes: u32,
    padded_row_bytes: u32,
    buffer_bytes: u64,
    output_bytes: usize,
}

impl ReadbackLayout {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("offscreen dimensions must be nonzero".into());
        }
        let row_bytes = width.checked_mul(4).ok_or("row size overflow")?;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row_bytes = row_bytes
            .checked_add(alignment - 1)
            .ok_or("padded row size overflow")?
            / alignment
            * alignment;
        let buffer_bytes = u64::from(padded_row_bytes) * u64::from(height);
        let output_bytes = usize::try_from(u64::from(row_bytes) * u64::from(height))
            .map_err(|_| "readback output size overflow")?;
        Ok(Self {
            row_bytes,
            padded_row_bytes,
            buffer_bytes,
            output_bytes,
        })
    }
}

impl OffscreenReadback {
    pub fn new(context: Arc<GpuContext>, width: u32, height: u32) -> Result<Self, String> {
        let layout = ReadbackLayout::new(width, height)?;
        let limits = context.device().limits();
        if width > limits.max_texture_dimension_2d
            || height > limits.max_texture_dimension_2d
            || layout.buffer_bytes > limits.max_buffer_size
        {
            return Err("offscreen dimensions exceed device limits".into());
        }
        let texture = context.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("Rust shell offscreen feasibility target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let staging = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("Rust shell offscreen feasibility readback"),
            size: layout.buffer_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            context,
            texture,
            staging,
            layout,
            width,
            height,
        })
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Copies completed queue work to tightly packed, top-to-bottom RGBA bytes.
    /// Values are linear UNORM, with alpha preserved and no color conversion.
    /// Submission and mapping completion are both required before bytes escape.
    pub fn read_rgba(&mut self, timeout: Duration) -> Result<Vec<u8>, String> {
        let mut encoder = self
            .context
            .device()
            .create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.layout.padded_row_bytes),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.context.shared_queue().submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.staging
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        // SharedQueue::submit released the engine queue lock before this wait.
        let result = self.context.device().poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(timeout),
        });
        if let Err(error) = result {
            self.staging.unmap();
            return Err(format!("offscreen GPU completion failed: {error}"));
        }
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            other => {
                self.staging.unmap();
                return Err(format!("offscreen readback mapping failed: {other:?}"));
            }
        }
        let mapped = match self.staging.get_mapped_range(..) {
            Ok(mapped) => mapped,
            Err(error) => {
                self.staging.unmap();
                return Err(format!("offscreen mapped range failed: {error}"));
            }
        };
        let mut rgba = Vec::with_capacity(self.layout.output_bytes);
        for row in mapped.chunks_exact(self.layout.padded_row_bytes as usize) {
            rgba.extend_from_slice(&row[..self.layout.row_bytes as usize]);
        }
        drop(mapped);
        self.staging.unmap();
        Ok(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_sizes_and_aligns_odd_rows() {
        assert!(ReadbackLayout::new(0, 1).is_err());
        assert!(ReadbackLayout::new(1, 0).is_err());
        assert!(ReadbackLayout::new(u32::MAX, 1).is_err());
        assert!(ReadbackLayout::new(u32::MAX / 4, 1).is_err());
        let layout = ReadbackLayout::new(257, 3).unwrap();
        assert_eq!(layout.row_bytes, 1028);
        assert_eq!(layout.padded_row_bytes, 1280);
        assert_eq!(layout.buffer_bytes, 3840);
        assert_eq!(layout.output_bytes, 3084);
        assert_eq!(ReadbackLayout::new(64, 1).unwrap().padded_row_bytes, 256);
    }
}

use ash::vk::{self, Handle};
use flutter_shell_core::{FlutterRustVulkanImage, FlutterRustVulkanPresentationCallbacks};
use parking_lot::Mutex;
use std::{ffi::c_void, sync::mpsc::SyncSender};

/// A completed probe frame. The raster-thread callback publishes owned bytes,
/// never an in-flight image or a pointer into engine storage.
pub struct OffscreenFrame {
    pub width: u32,
    pub height: u32,
    pub generation: u64,
    pub frame: u64,
    pub rgba: Vec<u8>,
}

/// Experimental synchronous Phase 0 broker. This serializes render/readback on
/// raster to establish the layout contract before adding a pipelined host.
/// Keep its Box alive until the engine and all raster work have stopped.
pub struct OffscreenBroker {
    context: Arc<GpuContext>,
    state: Mutex<BrokerState>,
    frames: SyncSender<Result<OffscreenFrame, String>>,
}

struct BrokerState {
    target: Option<ProbeTarget>,
    generation: u64,
    frame: u64,
    failed: bool,
}

struct ProbeTarget {
    readback: OffscreenReadback,
    acquire: vk::Semaphore,
    render: vk::Semaphore,
    pending: bool,
}

impl Drop for ProbeTarget {
    fn drop(&mut self) {
        let context = &self.readback.context;
        // Phase 0 retirement only; never called on UI or under the queue lock.
        if context.shared_queue().wait_device_idle(context.device()) {
            if let Some(device) = unsafe { context.device().as_hal::<wgpu::hal::vulkan::Api>() } {
                unsafe {
                    device.raw_device().destroy_semaphore(self.acquire, None);
                    device.raw_device().destroy_semaphore(self.render, None);
                }
            }
        }
    }
}

impl OffscreenBroker {
    pub fn new(
        context: Arc<GpuContext>,
        frames: SyncSender<Result<OffscreenFrame, String>>,
    ) -> Box<Self> {
        Box::new(Self {
            context,
            state: Mutex::new(BrokerState {
                target: None,
                generation: 0,
                frame: 0,
                failed: false,
            }),
            frames,
        })
    }

    /// # Safety
    /// The broker must remain at this address until engine teardown completes.
    pub unsafe fn callbacks(&self) -> FlutterRustVulkanPresentationCallbacks {
        FlutterRustVulkanPresentationCallbacks {
            presentation_mode: 1,
            user_data: (self as *const Self).cast_mut().cast(),
            acquire_image: Some(probe_acquire),
            present_image: Some(probe_present),
            release_surface: Some(probe_release),
        }
    }

    fn acquire(&self, width: u32, height: u32) -> Result<FlutterRustVulkanImage, String> {
        let mut state = self.state.lock();
        if state.failed {
            return Err("offscreen broker has failed".into());
        }
        if state.target.as_ref().is_some_and(|t| t.pending) {
            return Err("offscreen frame already acquired".into());
        }
        if state
            .target
            .as_ref()
            .is_none_or(|t| t.readback.width != width || t.readback.height != height)
        {
            let readback = OffscreenReadback::new(self.context.clone(), width, height)?;
            let device = unsafe { self.context.device().as_hal::<wgpu::hal::vulkan::Api>() }
                .ok_or("offscreen probe requires Vulkan")?;
            let acquire = unsafe {
                device
                    .raw_device()
                    .create_semaphore(&Default::default(), None)
            }
            .map_err(|e| format!("acquire semaphore: {e}"))?;
            let render = match unsafe {
                device
                    .raw_device()
                    .create_semaphore(&Default::default(), None)
            } {
                Ok(render) => render,
                Err(error) => {
                    unsafe {
                        device.raw_device().destroy_semaphore(acquire, None);
                    }
                    return Err(format!("render semaphore: {error}"));
                }
            };
            state.target = Some(ProbeTarget {
                readback,
                acquire,
                render,
                pending: false,
            });
            state.generation += 1;
        }
        let target = state.target.as_mut().unwrap();
        let view = target.readback.texture.create_view(&Default::default());
        let mut encoder = self
            .context
            .device()
            .create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen acquire and COLOR_TARGET tracking"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        self.context
            .shared_queue()
            .submit_with_semaphores(&[], &[target.acquire], [encoder.finish()])
            .ok_or("offscreen acquire submission failed")?;
        // The initialized texture's tracked COLOR_TARGET state is restored by
        // C++ after Impeller. No safe wgpu access occurs during the borrow.
        let image = unsafe {
            target
                .readback
                .texture
                .as_hal::<wgpu::hal::vulkan::Api>()
                .ok_or("offscreen texture is not Vulkan")?
                .raw_handle()
                .as_raw()
        };
        target.pending = true;
        Ok(FlutterRustVulkanImage {
            image,
            format: vk::Format::R8G8B8A8_UNORM.as_raw() as u32,
            acquire_semaphore: target.acquire.as_raw(),
            render_semaphore: target.render.as_raw(),
            cpp_submits: 0,
            image_views: 0,
            transients: 0,
        })
    }

    fn present(&self, image: FlutterRustVulkanImage) -> Result<OffscreenFrame, String> {
        let mut state = self.state.lock();
        let target = state.target.as_mut().ok_or("offscreen target missing")?;
        if !target.pending || image.render_semaphore != target.render.as_raw() {
            return Err("offscreen presentation does not match acquisition".into());
        }
        self.context
            .shared_queue()
            .submit_with_semaphores(
                &[flutter_gpu_queue::WaitSemaphore {
                    semaphore: target.render,
                    stage: vk::PipelineStageFlags::ALL_COMMANDS,
                }],
                &[],
                std::iter::empty(),
            )
            .ok_or("offscreen render wait submission failed")?;
        let rgba = target.readback.read_rgba(Duration::from_secs(10))?;
        target.pending = false;
        let (width, height) = (target.readback.width, target.readback.height);
        state.frame += 1;
        Ok(OffscreenFrame {
            width,
            height,
            generation: state.generation,
            frame: state.frame,
            rgba,
        })
    }

    fn fail(&self, error: String) {
        self.state.lock().failed = true;
        eprintln!("offscreen broker failure: {error}");
        let _ = self.frames.try_send(Err(error));
    }
}

extern "C" fn probe_acquire(
    data: *mut c_void,
    width: u32,
    height: u32,
    out: *mut FlutterRustVulkanImage,
) -> i32 {
    // SAFETY: callbacks' caller retains the stable broker until engine teardown.
    let broker = unsafe { &*data.cast::<OffscreenBroker>() };
    match broker.acquire(width, height) {
        Ok(image) => {
            unsafe {
                *out = image;
            }
            1
        }
        Err(error) => {
            broker.fail(error);
            0
        }
    }
}
extern "C" fn probe_present(data: *mut c_void, image: FlutterRustVulkanImage) -> i32 {
    let broker = unsafe { &*data.cast::<OffscreenBroker>() };
    match broker.present(image) {
        Ok(frame) => match broker.frames.try_send(Ok(frame)) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => 1,
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => 0,
        },
        Err(error) => {
            broker.fail(error);
            0
        }
    }
}
extern "C" fn probe_release(data: *mut c_void) {
    let broker = unsafe { &*data.cast::<OffscreenBroker>() };
    broker.state.lock().target = None;
}
