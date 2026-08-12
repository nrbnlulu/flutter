//! The one unsafe Rust component that exposes wgpu's Vulkan objects to the
//! private Flutter engine bridge. Rust retains ownership of every object.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod linux {
    use ash::vk::Handle as _;
    use flutter_shell_core::{
        FlutterRustExternalTextureCallbacks, FlutterRustExternalTextureFrame,
        FlutterRustVulkanImage, FlutterRustVulkanPresentationCallbacks,
    };
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Application-scoped wgpu/Vulkan ownership shared by every native view.
    pub struct GpuContext {
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    /// Borrowed Vulkan object values suitable only for an immediate C++ call.
    /// Their lifetime is tied to the [`GpuBroker`] that supplied them.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct VulkanContextData {
        pub get_instance_proc_addr: usize,
        pub instance: usize,
        pub physical_device: usize,
        pub device: usize,
        pub queue: usize,
        pub queue_family_index: u32,
        pub instance_extensions: Vec<String>,
        pub device_extensions: Vec<String>,
    }

    /// Wgpu owns the instance, device, queue, and surface. The broker exposes
    /// Vulkan values only through [`Self::with_vulkan_context`], preventing a
    /// Rust reference from escaping the handoff into C++.
    pub struct GpuBroker {
        context: std::sync::Arc<GpuContext>,
        surface: wgpu::Surface<'static>,
        // Retained both for the unsafe surface lifetime and so presentation
        // can notify winit immediately before the Vulkan WSI commit.
        window: std::sync::Arc<dyn winit::window::Window>,
        surface_state: Mutex<SurfaceState>,
        presentation_stats: Option<Mutex<PresentationStats>>,
    }

    /// Engine-owned triple-buffered texture storage shared by wgpu producers
    /// and Flutter's Impeller raster thread.
    pub struct WgpuTextureRing {
        context: std::sync::Arc<GpuContext>,
        width: u32,
        height: u32,
        state: Mutex<TextureRingState>,
        pending_clear: Mutex<Option<[f64; 4]>>,
    }

    struct TextureRingState {
        slots: Vec<TextureSlot>,
        ready: Option<usize>,
    }

    struct TextureSlot {
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        image: ash::vk::Image,
        image_view: ash::vk::ImageView,
        sync: FrameSync,
        state: TextureSlotState,
        wait_for_flutter: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TextureSlotState {
        Available,
        Ready,
        InFlutter,
    }

    struct PresentationStats {
        file: File,
        count: u64,
    }

    impl PresentationStats {
        fn create(path: PathBuf) -> Result<Self, String> {
            let file = File::create(&path).map_err(|error| {
                format!(
                    "failed to create presentation stats file {}: {error}",
                    path.display()
                )
            })?;
            Ok(Self { file, count: 0 })
        }

        fn record(&mut self, width: u32, height: u32) {
            self.count += 1;
            let _ = writeln!(self.file, "{} {width} {height}", self.count);
            let _ = self.file.flush();
        }
    }

    struct SurfaceState {
        configuration: Option<wgpu::SurfaceConfiguration>,
        // Holds the acquired frame between `acquire_image` and `present_image`.
        // wgpu must not destroy the swapchain image while Impeller is drawing
        // into it through the raw handle handed to C++.
        pending_frame: Option<PendingFrame>,
        // Wgpu forbids reconfiguration while a SurfaceTexture is outstanding.
        // Resize events therefore replace this with the latest requested
        // configuration, which is applied at the next safe acquire boundary.
        deferred_configuration: Option<wgpu::SurfaceConfiguration>,
        // Synchronization objects are kept alive until the final wgpu
        // submission that consumed them has completed. A small bounded queue
        // preserves normal frame overlap without leaking one pair per frame.
        retired_frames: VecDeque<RetiredFrame>,
    }

    struct PendingFrame {
        texture: wgpu::SurfaceTexture,
        sync: FrameSync,
    }

    struct RetiredFrame {
        submission: wgpu::SubmissionIndex,
        sync: FrameSync,
    }

    #[derive(Debug, Clone, Copy)]
    struct FrameSync {
        acquire: ash::vk::Semaphore,
        render: ash::vk::Semaphore,
    }

    /// A Vulkan swapchain image borrowed from the broker's current frame.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AcquiredImage {
        pub image: u64,
        pub format: u32,
        pub acquire_semaphore: u64,
        pub render_semaphore: u64,
    }

    fn vulkan_format(format: wgpu::TextureFormat) -> Option<ash::vk::Format> {
        use wgpu::TextureFormat::*;
        Some(match format {
            Bgra8Unorm => ash::vk::Format::B8G8R8A8_UNORM,
            Bgra8UnormSrgb => ash::vk::Format::B8G8R8A8_SRGB,
            Rgba8Unorm => ash::vk::Format::R8G8B8A8_UNORM,
            Rgba8UnormSrgb => ash::vk::Format::R8G8B8A8_SRGB,
            Rgba16Float => ash::vk::Format::R16G16B16A16_SFLOAT,
            _ => return None,
        })
    }

    impl GpuContext {
        fn new_for_window(
            window: &std::sync::Arc<dyn winit::window::Window>,
        ) -> Result<std::sync::Arc<Self>, String> {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            // This temporary surface selects a device that can present to the
            // application's initial native window.
            let surface = unsafe {
                instance.create_surface_unsafe(
                    wgpu::SurfaceTargetUnsafe::from_display_and_window(window, window)
                        .map_err(|error| error.to_string())?,
                )
            }
            .map_err(|error| error.to_string())?;
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: Some(&surface),
                    apply_limit_buckets: false,
                }))
                .map_err(|error| error.to_string())?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("Flutter Rust Shell Vulkan device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    ..Default::default()
                }))
                .map_err(|error| error.to_string())?;
            Ok(std::sync::Arc::new(Self {
                instance,
                adapter,
                device,
                queue,
            }))
        }
    }

    impl WgpuTextureRing {
        fn new(
            context: std::sync::Arc<GpuContext>,
            width: u32,
            height: u32,
        ) -> Result<Box<Self>, String> {
            if width == 0 || height == 0 {
                return Err("external texture dimensions must be nonzero".to_owned());
            }
            let mut slots = Vec::with_capacity(3);
            for index in 0..3 {
                let texture = context.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Flutter Rust external texture slot"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                // SAFETY: only the numeric borrowed handles escape these
                // guards. The slot retains both wgpu objects through every
                // callback and until the device is idle during drop.
                let image = unsafe {
                    texture
                        .as_hal::<wgpu::hal::vulkan::Api>()
                        .ok_or("wgpu texture is not Vulkan")?
                        .raw_handle()
                };
                let image_view = unsafe {
                    view.as_hal::<wgpu::hal::vulkan::Api>()
                        .ok_or("wgpu texture view is not Vulkan")?
                        .raw_handle()
                };
                let device = unsafe { context.device.as_hal::<wgpu::hal::vulkan::Api>() }
                    .ok_or("wgpu device is not Vulkan")?;
                let raw = device.raw_device();
                let acquire =
                    unsafe { raw.create_semaphore(&ash::vk::SemaphoreCreateInfo::default(), None) }
                        .map_err(|error| format!("failed to create acquire semaphore: {error}"))?;
                let render = match unsafe {
                    raw.create_semaphore(&ash::vk::SemaphoreCreateInfo::default(), None)
                } {
                    Ok(semaphore) => semaphore,
                    Err(error) => {
                        unsafe { raw.destroy_semaphore(acquire, None) };
                        return Err(format!("failed to create render semaphore: {error}"));
                    }
                };
                slots.push(TextureSlot {
                    texture,
                    view,
                    image,
                    image_view,
                    sync: FrameSync { acquire, render },
                    state: TextureSlotState::Available,
                    wait_for_flutter: false,
                });
                debug_assert_eq!(slots.len(), index + 1);
            }
            Ok(Box::new(Self {
                context,
                width,
                height,
                state: Mutex::new(TextureRingState { slots, ready: None }),
                pending_clear: Mutex::new(None),
            }))
        }

        /// Requests a solid-color frame. Rendering is deliberately deferred to
        /// Flutter's acquire callback so wgpu and Impeller never submit to the
        /// shared Vulkan queue concurrently.
        pub fn request_clear(&self, color: [f64; 4]) {
            *self
                .pending_clear
                .lock()
                .expect("texture clear request lock poisoned") = Some(color);
        }

        fn render_pending_clear(&self) -> bool {
            let Some(color) = self
                .pending_clear
                .lock()
                .expect("texture clear request lock poisoned")
                .take()
            else {
                return false;
            };
            let mut state = self.state.lock().expect("texture ring lock poisoned");
            if state.ready.is_some() {
                *self
                    .pending_clear
                    .lock()
                    .expect("texture clear request lock poisoned") = Some(color);
                return false;
            }
            let Some(index) = state
                .slots
                .iter()
                .position(|slot| slot.state == TextureSlotState::Available)
            else {
                return false;
            };
            let slot = &mut state.slots[index];
            let mut encoder =
                self.context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Flutter Rust external texture producer"),
                    });
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Flutter Rust external texture clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &slot.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: color[0],
                                g: color[1],
                                b: color[2],
                                a: color[3],
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            encoder.transition_resources(
                std::iter::empty(),
                std::iter::once(wgpu::TextureTransition {
                    texture: &slot.texture,
                    selector: None,
                    state: wgpu::TextureUses::RESOURCE,
                }),
            );
            let Some(queue) = (unsafe { self.context.queue.as_hal::<wgpu::hal::vulkan::Api>() })
            else {
                return false;
            };
            if slot.wait_for_flutter {
                queue.add_wait_semaphore(
                    slot.sync.render,
                    None,
                    ash::vk::PipelineStageFlags::ALL_COMMANDS,
                );
                slot.wait_for_flutter = false;
            }
            queue.add_signal_semaphore(slot.sync.acquire, None);
            self.context.queue.submit([encoder.finish()]);
            slot.state = TextureSlotState::Ready;
            state.ready = Some(index);
            true
        }

        /// Returns callbacks whose user data is valid while this boxed ring
        /// remains alive.
        pub fn callbacks(&self) -> FlutterRustExternalTextureCallbacks {
            FlutterRustExternalTextureCallbacks {
                user_data: (self as *const Self).cast_mut().cast::<c_void>(),
                acquire_frame: Some(acquire_external_texture_frame),
                release_frame: Some(release_external_texture_frame),
            }
        }
    }

    impl Drop for WgpuTextureRing {
        fn drop(&mut self) {
            let Some(device) = (unsafe { self.context.device.as_hal::<wgpu::hal::vulkan::Api>() })
            else {
                return;
            };
            let _ = unsafe { device.raw_device().device_wait_idle() };
            let state = self
                .state
                .get_mut()
                .expect("texture ring lock poisoned during destruction");
            for slot in &state.slots {
                unsafe {
                    device
                        .raw_device()
                        .destroy_semaphore(slot.sync.acquire, None);
                    device
                        .raw_device()
                        .destroy_semaphore(slot.sync.render, None);
                }
            }
        }
    }

    impl GpuBroker {
        /// Creates an engine-owned external texture ring on this application's
        /// shared wgpu device.
        pub fn create_texture_ring(
            &self,
            width: u32,
            height: u32,
        ) -> Result<Box<WgpuTextureRing>, String> {
            WgpuTextureRing::new(std::sync::Arc::clone(&self.context), width, height)
        }

        pub fn new(
            window: std::sync::Arc<dyn winit::window::Window>,
            presentation_stats_path: Option<PathBuf>,
        ) -> Result<Self, String> {
            let context = GpuContext::new_for_window(&window)?;
            Self::from_context(context, window, presentation_stats_path)
        }

        pub fn from_context(
            context: std::sync::Arc<GpuContext>,
            window: std::sync::Arc<dyn winit::window::Window>,
            presentation_stats_path: Option<PathBuf>,
        ) -> Result<Self, String> {
            // SAFETY: the broker retains the window until after this surface
            // has been destroyed.
            let surface = unsafe {
                context.instance.create_surface_unsafe(
                    wgpu::SurfaceTargetUnsafe::from_display_and_window(&window, &window)
                        .map_err(|error| error.to_string())?,
                )
            }
            .map_err(|error| error.to_string())?;
            if surface
                .get_capabilities(&context.adapter)
                .formats
                .is_empty()
            {
                return Err("shared Vulkan adapter cannot present to this window".to_owned());
            }
            let presentation_stats = presentation_stats_path
                .map(PresentationStats::create)
                .transpose()?
                .map(Mutex::new);
            Ok(Self {
                context,
                surface,
                window,
                surface_state: Mutex::new(SurfaceState {
                    configuration: None,
                    pending_frame: None,
                    deferred_configuration: None,
                    retired_frames: VecDeque::new(),
                }),
                presentation_stats,
            })
        }

        pub fn shared_context(&self) -> std::sync::Arc<GpuContext> {
            std::sync::Arc::clone(&self.context)
        }

        fn create_frame_sync(&self) -> Option<FrameSync> {
            // SAFETY: the HAL guard keeps wgpu's device alive while the raw
            // Vulkan calls create objects owned by this broker.
            let device = unsafe { self.context.device.as_hal::<wgpu::hal::vulkan::Api>() }?;
            let raw = device.raw_device();
            let acquire = unsafe {
                raw.create_semaphore(&ash::vk::SemaphoreCreateInfo::default(), None)
                    .ok()?
            };
            let render = match unsafe {
                raw.create_semaphore(&ash::vk::SemaphoreCreateInfo::default(), None)
            } {
                Ok(semaphore) => semaphore,
                Err(_) => {
                    unsafe { raw.destroy_semaphore(acquire, None) };
                    return None;
                }
            };
            Some(FrameSync { acquire, render })
        }

        fn destroy_frame_sync(&self, sync: FrameSync) {
            // SAFETY: callers wait for the submission that consumed both
            // semaphores before destroying them.
            let Some(device) = (unsafe { self.context.device.as_hal::<wgpu::hal::vulkan::Api>() })
            else {
                return;
            };
            unsafe {
                device.raw_device().destroy_semaphore(sync.acquire, None);
                device.raw_device().destroy_semaphore(sync.render, None);
            }
        }

        fn wait_and_destroy(&self, retired: RetiredFrame) -> bool {
            if self
                .context
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(retired.submission),
                    timeout: None,
                })
                .is_err()
            {
                return false;
            }
            self.destroy_frame_sync(retired.sync);
            true
        }

        /// Calls `callback` while wgpu-hal guards keep the borrowed Vulkan
        /// device alive. The callback must copy values immediately and must
        /// not destroy or submit through the handles.
        pub fn with_vulkan_context<T>(
            &self,
            callback: impl FnOnce(VulkanContextData) -> T,
        ) -> Option<T> {
            // SAFETY: the broker retains wgpu ownership, and no HAL resource
            // is destroyed or submitted through this borrowed handle.
            let device = unsafe { self.context.device.as_hal::<wgpu::hal::vulkan::Api>() }?;
            let instance = device.shared_instance();
            Some(callback(VulkanContextData {
                get_instance_proc_addr: instance.entry().static_fn().get_instance_proc_addr
                    as usize,
                instance: instance.raw_instance().handle().as_raw() as usize,
                physical_device: device.raw_physical_device().as_raw() as usize,
                device: device.raw_device().handle().as_raw() as usize,
                queue: device.raw_queue().as_raw() as usize,
                queue_family_index: device.queue_family_index(),
                instance_extensions: instance
                    .extensions()
                    .iter()
                    .map(|extension| extension.to_string_lossy().into_owned())
                    .collect(),
                device_extensions: device
                    .enabled_device_extensions()
                    .iter()
                    .map(|extension| extension.to_string_lossy().into_owned())
                    .collect(),
            }))
        }

        /// Configure the Rust-owned surface after winit reports a non-zero
        /// physical size. Acquisition/presentation stays in this broker.
        pub fn configure(&self, width: u32, height: u32) -> Result<(), String> {
            if width == 0 || height == 0 {
                return Ok(());
            }
            let mut state = self.surface_state.lock().expect("surface lock poisoned");
            let capabilities = self.surface.get_capabilities(&self.context.adapter);
            // Impeller's Vulkan backend only recognizes these two swapchain
            // formats (see VkFormatToImpellerFormat); sRGB and other variants
            // the surface may prefer are rejected at frame-acquire time.
            let format = [
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureFormat::Rgba8Unorm,
            ]
            .into_iter()
            .find(|format| capabilities.formats.contains(format))
            .ok_or_else(|| "Vulkan surface has no format Impeller supports".to_owned())?;
            let configuration = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width,
                height,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: capabilities.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            // Once the surface is initialized, resize is applied at the next
            // acquire boundary. This coalesces compositor resize bursts and
            // prevents repeated configure calls between presentation and
            // wgpu's retirement of its internal WSI acquire fence.
            if state.configuration.is_some() {
                state.deferred_configuration = Some(configuration);
                return Ok(());
            }
            self.surface.configure(&self.context.device, &configuration);
            state.configuration = Some(configuration);
            state.deferred_configuration = None;
            Ok(())
        }

        /// Acquires the next swapchain image for Impeller to draw into.
        ///
        /// The returned handle stays valid until [`Self::present_image`] is
        /// called; the broker keeps the underlying `SurfaceTexture` alive in
        /// the meantime. Only one frame may be in flight at a time.
        pub fn acquire_image(
            &self,
            requested_width: u32,
            requested_height: u32,
        ) -> Option<AcquiredImage> {
            let mut state = self.surface_state.lock().expect("surface lock poisoned");
            if state.pending_frame.is_some() {
                return None;
            }
            // The dimensions Flutter passes here belong to the layer tree that
            // Impeller is about to render. A newer winit resize may already be
            // queued, but applying that newer size would combine a swapchain
            // color image with depth/stencil attachments from this older
            // layer-tree generation. Configure this acquire to the requested
            // generation and retain a newer deferred size for the next frame.
            let deferred_matches_request =
                state
                    .deferred_configuration
                    .as_ref()
                    .is_some_and(|configuration| {
                        configuration.width == requested_width
                            && configuration.height == requested_height
                    });
            let configuration_changed = state.configuration.as_ref().is_none_or(|configuration| {
                configuration.width != requested_width || configuration.height != requested_height
            });
            if configuration_changed {
                let mut configuration = if deferred_matches_request {
                    state.deferred_configuration.take()?
                } else {
                    state.configuration.clone()?
                };
                configuration.width = requested_width;
                configuration.height = requested_height;
                while let Some(retired) = state.retired_frames.pop_front() {
                    if !self.wait_and_destroy(retired) {
                        return None;
                    }
                }
                self.surface.configure(&self.context.device, &configuration);
                state.configuration = Some(configuration);
            } else if deferred_matches_request {
                // A matching deferred request has now reached its layer-tree
                // generation; the existing swapchain already has that size.
                state.deferred_configuration = None;
            }
            // Three pairs cover the configured two-frame surface latency plus
            // the frame being acquired. Recycle the oldest pair only after its
            // consuming submission has completed.
            if state.retired_frames.len() >= 3 {
                let retired = state.retired_frames.pop_front()?;
                if !self.wait_and_destroy(retired) {
                    return None;
                }
            }
            let configuration = state.configuration.clone()?;
            let format = configuration.format;
            let vk_format = vulkan_format(format)?;
            let (surface_texture, suboptimal) = match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
                wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
                wgpu::CurrentSurfaceTexture::Outdated => {
                    self.surface.configure(&self.context.device, &configuration);
                    match self.surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
                        wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
                        _ => return None,
                    }
                }
                _ => return None,
            };
            if surface_texture.texture.width() != requested_width
                || surface_texture.texture.height() != requested_height
            {
                return None;
            }
            if suboptimal && state.deferred_configuration.is_none() {
                state.deferred_configuration = Some(configuration);
            }
            // SAFETY: the returned guard is dropped immediately after copying
            // the raw handle; the image itself outlives it in `pending_frame`.
            let image = unsafe {
                let guard = surface_texture.texture.as_hal::<wgpu::hal::vulkan::Api>()?;
                guard.raw_handle()
            };
            let sync = self.create_frame_sync()?;
            // Register a real wgpu write to the acquired image before handing
            // its raw handle to Impeller. This makes wgpu's submission wait on
            // the swapchain acquire semaphore and marks the texture initialized;
            // otherwise Queue::present clears the image because Impeller's raw
            // Vulkan commands are invisible to wgpu's resource tracker.
            let view = surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                self.context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Flutter Rust Shell acquire barrier"),
                    });
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Flutter Rust Shell acquire barrier"),
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
            // Make the acquire-complete semaphore part of the same wgpu
            // submission that consumes the swapchain's private acquire
            // semaphore. Impeller waits on this before its first image use.
            let Some(queue) = (unsafe { self.context.queue.as_hal::<wgpu::hal::vulkan::Api>() })
            else {
                self.destroy_frame_sync(sync);
                return None;
            };
            queue.add_signal_semaphore(sync.acquire, None);
            self.context.queue.submit([encoder.finish()]);
            state.pending_frame = Some(PendingFrame {
                texture: surface_texture,
                sync,
            });
            Some(AcquiredImage {
                image: image.as_raw(),
                format: vk_format.as_raw() as u32,
                acquire_semaphore: sync.acquire.as_raw(),
                render_semaphore: sync.render.as_raw(),
            })
        }

        /// Presents the frame most recently returned by [`Self::acquire_image`].
        ///
        pub fn present_image(&self) -> bool {
            let mut state = self.surface_state.lock().expect("surface lock poisoned");
            let Some(queue) = (unsafe { self.context.queue.as_hal::<wgpu::hal::vulkan::Api>() })
            else {
                return false;
            };
            let Some(pending) = state.pending_frame.take() else {
                return false;
            };
            // Impeller signals `render` after its final layout transition. Make
            // a real wgpu submission wait on it and touch the surface texture,
            // so wgpu's own presentation semaphore is signalled only after all
            // Impeller work is complete.
            let view = pending
                .texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                self.context
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Flutter Rust Shell present handoff"),
                    });
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Flutter Rust Shell present handoff"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            queue.add_wait_semaphore(
                pending.sync.render,
                None,
                ash::vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            );
            let submission = self.context.queue.submit([encoder.finish()]);
            // Wayland frame callbacks must only be armed when a surface commit
            // is guaranteed. Doing this at the earlier vsync pulse can freeze
            // redraw delivery when Flutter requested a secondary vsync that
            // intentionally produced no frame.
            self.window.pre_present_notify();
            self.context.queue.present(pending.texture);
            if let (Some(stats), Some(configuration)) =
                (&self.presentation_stats, &state.configuration)
            {
                stats
                    .lock()
                    .expect("presentation stats lock poisoned")
                    .record(configuration.width, configuration.height);
            }
            state.retired_frames.push_back(RetiredFrame {
                submission,
                sync: pending.sync,
            });
            true
        }

        /// A presentation callback table whose `user_data` is this broker's
        /// address. The broker must outlive every use of the returned table.
        pub fn presentation_callbacks(&self) -> FlutterRustVulkanPresentationCallbacks {
            FlutterRustVulkanPresentationCallbacks {
                user_data: (self as *const Self).cast_mut().cast::<c_void>(),
                acquire_image: Some(acquire_image_callback),
                present_image: Some(present_image_callback),
            }
        }
    }

    impl Drop for GpuBroker {
        fn drop(&mut self) {
            let state = self
                .surface_state
                .get_mut()
                .expect("surface lock poisoned during broker destruction");
            // SAFETY: no callback can enter the broker during `drop`. Waiting
            // for the borrowed device to become idle makes every outstanding
            // broker semaphore safe to destroy.
            if let Some(device) = unsafe { self.context.device.as_hal::<wgpu::hal::vulkan::Api>() }
            {
                let _ = unsafe { device.raw_device().device_wait_idle() };
                if let Some(pending) = state.pending_frame.take() {
                    unsafe {
                        device
                            .raw_device()
                            .destroy_semaphore(pending.sync.acquire, None);
                        device
                            .raw_device()
                            .destroy_semaphore(pending.sync.render, None);
                    }
                }
                for retired in state.retired_frames.drain(..) {
                    unsafe {
                        device
                            .raw_device()
                            .destroy_semaphore(retired.sync.acquire, None);
                        device
                            .raw_device()
                            .destroy_semaphore(retired.sync.render, None);
                    }
                }
            }
        }
    }

    extern "C" fn acquire_external_texture_frame(
        user_data: *mut c_void,
        _requested_width: u32,
        _requested_height: u32,
        out_frame: *mut FlutterRustExternalTextureFrame,
    ) -> i32 {
        if user_data.is_null() || out_frame.is_null() {
            return 0;
        }
        // SAFETY: callbacks() points at a boxed ring retained by the host, and
        // the C++ bridge supplies a writable output for this call.
        let ring = unsafe { &*user_data.cast::<WgpuTextureRing>() };
        let has_ready = ring
            .state
            .lock()
            .expect("texture ring lock poisoned")
            .ready
            .is_some();
        if !has_ready {
            ring.render_pending_clear();
        }
        let mut state = ring.state.lock().expect("texture ring lock poisoned");
        let Some(index) = state.ready.take() else {
            return 0;
        };
        let slot = &mut state.slots[index];
        if slot.state != TextureSlotState::Ready {
            return 0;
        }
        slot.state = TextureSlotState::InFlutter;
        let frame = FlutterRustExternalTextureFrame {
            image: slot.image.as_raw(),
            image_view: slot.image_view.as_raw(),
            format: ash::vk::Format::R8G8B8A8_UNORM.as_raw() as u32,
            width: ring.width,
            height: ring.height,
            acquire_semaphore: slot.sync.acquire.as_raw(),
            render_semaphore: slot.sync.render.as_raw(),
        };
        unsafe { *out_frame = frame };
        1
    }

    extern "C" fn release_external_texture_frame(
        user_data: *mut c_void,
        frame: FlutterRustExternalTextureFrame,
    ) {
        if user_data.is_null() {
            return;
        }
        // SAFETY: see acquire_external_texture_frame.
        let ring = unsafe { &*user_data.cast::<WgpuTextureRing>() };
        let mut state = ring.state.lock().expect("texture ring lock poisoned");
        let Some(slot) = state
            .slots
            .iter_mut()
            .find(|slot| slot.image.as_raw() == frame.image)
        else {
            return;
        };
        if slot.state == TextureSlotState::InFlutter {
            slot.state = TextureSlotState::Available;
            slot.wait_for_flutter = true;
        }
    }

    extern "C" fn acquire_image_callback(
        user_data: *mut c_void,
        width: u32,
        height: u32,
        out_image: *mut FlutterRustVulkanImage,
    ) -> i32 {
        // SAFETY: presentation_callbacks() sets user_data to a GpuBroker
        // address that outlives every call through this callback table.
        let broker = unsafe { &*user_data.cast::<GpuBroker>() };
        match broker.acquire_image(width, height) {
            Some(image) => {
                // SAFETY: the C++ caller supplies a valid output pointer for
                // the duration of this call.
                unsafe {
                    *out_image = FlutterRustVulkanImage {
                        image: image.image,
                        format: image.format,
                        acquire_semaphore: image.acquire_semaphore,
                        render_semaphore: image.render_semaphore,
                    };
                }
                1
            }
            None => 0,
        }
    }

    extern "C" fn present_image_callback(
        user_data: *mut c_void,
        _image: FlutterRustVulkanImage,
    ) -> i32 {
        // SAFETY: see acquire_image_callback.
        let broker = unsafe { &*user_data.cast::<GpuBroker>() };
        i32::from(broker.present_image())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn context_values_are_plain_owned_data() {
            let context = VulkanContextData {
                get_instance_proc_addr: 1,
                instance: 2,
                physical_device: 3,
                device: 4,
                queue: 5,
                queue_family_index: 6,
                instance_extensions: vec!["VK_KHR_surface".to_owned()],
                device_extensions: vec!["VK_KHR_swapchain".to_owned()],
            };
            assert_eq!(context.queue_family_index, 6);
            assert_eq!(context.instance_extensions.len(), 1);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{GpuBroker, GpuContext, VulkanContextData, WgpuTextureRing};
