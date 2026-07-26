//! The one unsafe Rust component that exposes wgpu's Vulkan objects to the
//! private Flutter engine bridge. Rust retains ownership of every object.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod linux {
    use ash::vk::Handle as _;
    use flutter_shell_core::{FlutterRustVulkanImage, FlutterRustVulkanPresentationCallbacks};
    use std::ffi::c_void;
    use std::sync::Mutex;

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
        _instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        _adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        configured_format: Mutex<Option<wgpu::TextureFormat>>,
        // Holds the acquired frame between `acquire_image` and `present_image`.
        // wgpu must not destroy the swapchain image while Impeller is drawing
        // into it through the raw handle handed to C++.
        pending_frame: Mutex<Option<wgpu::SurfaceTexture>>,
    }

    /// A Vulkan swapchain image borrowed from the broker's current frame.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AcquiredImage {
        pub image: u64,
        pub format: u32,
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

    impl GpuBroker {
        pub fn new(window: std::sync::Arc<winit::window::Window>) -> Result<Self, String> {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            // SAFETY: `window` is retained by the winit application until the
            // broker and its surface have been dropped.
            let surface = unsafe {
                instance.create_surface_unsafe(
                    wgpu::SurfaceTargetUnsafe::from_display_and_window(
                        window.as_ref(),
                        window.as_ref(),
                    )
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
            Ok(Self {
                _instance: instance,
                surface,
                _adapter: adapter,
                device,
                queue,
                configured_format: Mutex::new(None),
                pending_frame: Mutex::new(None),
            })
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
            let device = unsafe { self.device.as_hal::<wgpu::hal::vulkan::Api>() }?;
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
            let capabilities = self.surface.get_capabilities(&self._adapter);
            // Impeller's Vulkan backend only recognizes these two swapchain
            // formats (see VkFormatToImpellerFormat); sRGB and other variants
            // the surface may prefer are rejected at frame-acquire time.
            let format = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm]
                .into_iter()
                .find(|format| capabilities.formats.contains(format))
                .ok_or_else(|| {
                    "Vulkan surface has no format Impeller supports".to_owned()
                })?;
            self.surface.configure(
                &self.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    color_space: wgpu::SurfaceColorSpace::Auto,
                    width,
                    height,
                    present_mode: wgpu::PresentMode::Fifo,
                    alpha_mode: capabilities.alpha_modes[0],
                    view_formats: vec![],
                    desired_maximum_frame_latency: 2,
                },
            );
            *self.configured_format.lock().expect("format lock poisoned") = Some(format);
            Ok(())
        }

        /// Acquires the next swapchain image for Impeller to draw into.
        ///
        /// The returned handle stays valid until [`Self::present_image`] is
        /// called; the broker keeps the underlying `SurfaceTexture` alive in
        /// the meantime. Only one frame may be in flight at a time.
        pub fn acquire_image(&self) -> Option<AcquiredImage> {
            let format = (*self.configured_format.lock().expect("format lock poisoned"))?;
            let vk_format = vulkan_format(format)?;
            let surface_texture = match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(texture)
                | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
                _ => return None,
            };
            // SAFETY: the returned guard is dropped immediately after copying
            // the raw handle; the image itself outlives it in `pending_frame`.
            let image = unsafe {
                let guard = surface_texture.texture.as_hal::<wgpu::hal::vulkan::Api>()?;
                guard.raw_handle()
            };
            *self.pending_frame.lock().expect("frame lock poisoned") = Some(surface_texture);
            Some(AcquiredImage {
                image: image.as_raw(),
                format: vk_format.as_raw() as u32,
            })
        }

        /// Presents the frame most recently returned by [`Self::acquire_image`].
        ///
        /// Impeller's Vulkan submission of the drawing commands is not tracked
        /// by wgpu's queue; cross-queue synchronization between that submit and
        /// this present is a known gap carried forward from the interop broker
        /// design and is not part of proving the phase 0 seam.
        pub fn present_image(&self) -> bool {
            let Some(surface_texture) =
                self.pending_frame.lock().expect("frame lock poisoned").take()
            else {
                return false;
            };
            self.queue.present(surface_texture);
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

    extern "C" fn acquire_image_callback(
        user_data: *mut c_void,
        _width: u32,
        _height: u32,
        out_image: *mut FlutterRustVulkanImage,
    ) -> i32 {
        // SAFETY: presentation_callbacks() sets user_data to a GpuBroker
        // address that outlives every call through this callback table.
        let broker = unsafe { &*user_data.cast::<GpuBroker>() };
        match broker.acquire_image() {
            Some(image) => {
                // SAFETY: the C++ caller supplies a valid output pointer for
                // the duration of this call.
                unsafe {
                    *out_image = FlutterRustVulkanImage {
                        image: image.image,
                        format: image.format,
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
pub use linux::{GpuBroker, VulkanContextData};
