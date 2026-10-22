//! Rust plugin registration and wgpu textures hosted by the standard Android
//! embedding. FlutterActivity owns the window, input, lifecycle and Dart VM.
#![deny(unsafe_op_in_unsafe_fn)]

use flutter_plugin_sdk::{
    GpuTextures, MainThreadDispatcher, MainThreadTask, PixelBufferTextureBackendHandle,
    PixelBufferTextureFrameBackend, PluginError, PluginRegistrar, Result, TextureDescriptor,
    WgpuTextureBackend, WgpuTextureBackendHandle, WgpuTextureFrameBackend,
};
use flutter_shell_core::{FlutterRustExternalTextureCallbacks, FlutterRustVulkanContextData};
use flutter_shell_wgpu::{GpuContext, WgpuTextureRing};
use std::{
    collections::BTreeMap,
    ffi::{CString, c_void},
    sync::{Arc, Mutex},
};

type TaskCallback = unsafe extern "C" fn(*mut c_void);
unsafe extern "C" {
    fn FlutterRustAndroidCreate(context: FlutterRustVulkanContextData) -> *mut c_void;
    fn FlutterRustAndroidDestroy(runtime: *mut c_void);
    fn FlutterRustAndroidPost(runtime: *mut c_void, callback: TaskCallback, data: *mut c_void);
    fn FlutterRustAndroidRegisterTexture(
        runtime: *mut c_void,
        callbacks: FlutterRustExternalTextureCallbacks,
    ) -> i64;
    fn FlutterRustAndroidMarkTexture(runtime: *mut c_void, id: i64);
    fn FlutterRustAndroidUnregisterTexture(
        runtime: *mut c_void,
        id: i64,
        callback: TaskCallback,
        data: *mut c_void,
    );
    fn FlutterRustAndroidStop(runtime: *mut c_void, callback: TaskCallback, data: *mut c_void);
    fn FlutterRustAndroidLockQueue(runtime: *mut c_void);
    fn FlutterRustAndroidUnlockQueue(runtime: *mut c_void);
}

struct NativeRuntime {
    address: usize,
    // The C++ context borrows this device. Drop C++ before the last GPU owner.
    gpu: Arc<GpuContext>,
}
impl NativeRuntime {
    fn ptr(&self) -> *mut c_void {
        self.address as *mut c_void
    }
}
impl Drop for NativeRuntime {
    fn drop(&mut self) {
        self.gpu.clear_queue_lock();
        unsafe { FlutterRustAndroidDestroy(self.ptr()) };
    }
}
unsafe extern "C" fn lock_queue(data: *mut c_void) {
    unsafe { FlutterRustAndroidLockQueue(data) };
}
unsafe extern "C" fn unlock_queue(data: *mut c_void) {
    unsafe { FlutterRustAndroidUnlockQueue(data) };
}
unsafe extern "C" fn run_task(data: *mut c_void) {
    let task = unsafe { Box::from_raw(data.cast::<MainThreadTask>()) };
    task();
}

/// One FlutterEngine's Rust plugin state. All lifecycle methods run on the
/// Android UI thread, while texture production may run on worker threads.
pub struct AndroidRuntime {
    native: Arc<NativeRuntime>,
    dispatcher: MainThreadDispatcher,
    registrar: PluginRegistrar,
    textures: Arc<Mutex<BTreeMap<i64, Arc<WgpuTextureRing>>>>,
    registered: bool,
    stopped: bool,
}
impl AndroidRuntime {
    /// Creates a wgpu-owned Vulkan device for the engine's Impeller context.
    pub fn prepare() -> core::result::Result<Self, String> {
        let gpu = GpuContext::headless()?;
        let address = gpu
            .with_vulkan_context(|data| {
                let instance: Vec<_> = data
                    .instance_extensions
                    .iter()
                    .map(|e| CString::new(e.as_str()).unwrap())
                    .collect();
                let device: Vec<_> = data
                    .device_extensions
                    .iter()
                    .map(|e| CString::new(e.as_str()).unwrap())
                    .collect();
                let instance_ptrs: Vec<_> = instance.iter().map(|e| e.as_ptr()).collect();
                let device_ptrs: Vec<_> = device.iter().map(|e| e.as_ptr()).collect();
                unsafe {
                    FlutterRustAndroidCreate(FlutterRustVulkanContextData {
                        get_instance_proc_addr: data.get_instance_proc_addr as *mut c_void,
                        instance: data.instance as *mut c_void,
                        physical_device: data.physical_device as *mut c_void,
                        device: data.device as *mut c_void,
                        queue: data.queue as *mut c_void,
                        queue_family_index: data.queue_family_index,
                        instance_extensions: instance_ptrs.as_ptr(),
                        instance_extensions_count: instance_ptrs.len() as u32,
                        device_extensions: device_ptrs.as_ptr(),
                        device_extensions_count: device_ptrs.len() as u32,
                    }) as usize
                }
            })
            .ok_or("wgpu did not provide a Vulkan device")?;
        if address == 0 {
            return Err("Impeller could not adopt the wgpu Vulkan device".into());
        }
        let native = Arc::new(NativeRuntime { address, gpu });
        unsafe {
            native
                .gpu
                .install_queue_lock(flutter_gpu_queue::QueueLockHooks {
                    user_data: native.ptr(),
                    lock: lock_queue,
                    unlock: unlock_queue,
                })
        };
        let target = native.clone();
        let dispatcher = MainThreadDispatcher::for_shell_inactive(
            move |task| {
                let data = Box::into_raw(Box::new(task)).cast();
                unsafe { FlutterRustAndroidPost(target.ptr(), run_task, data) };
                true
            },
            std::thread::current().id(),
        );
        let textures = Arc::new(Mutex::new(BTreeMap::new()));
        let mut registrar = PluginRegistrar::for_shell(dispatcher.clone());
        registrar.install_gpu_for_shell(GpuTextures::for_shell(Arc::new(Backend {
            native: native.clone(),
            dispatcher: dispatcher.clone(),
            textures: textures.clone(),
        })));
        Ok(Self {
            native,
            dispatcher,
            registrar,
            textures,
            registered: false,
            stopped: false,
        })
    }

    /// Private native context passed to FlutterJNI before engine attachment.
    pub fn context_handle(&self) -> i64 {
        self.native.address as i64
    }

    /// Registers application Rust plugins once, after Java plugins and the
    /// Android Activity have been attached and before Dart starts.
    pub fn register(&mut self, register: fn(&mut PluginRegistrar) -> Result<()>) -> Result<()> {
        if self.registered || self.stopped {
            return Err(PluginError::Shutdown);
        }
        self.registered = true;
        if !self.dispatcher.start_for_shell() {
            return Err(PluginError::Shutdown);
        }
        register(&mut self.registrar)
    }

    /// Unregisters textures and reclaims GPU storage on raster before the
    /// Java FlutterEngine destroys its native shell.
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.dispatcher.shutdown_for_shell();
        let rings: Vec<_> = std::mem::take(&mut *self.textures.lock().unwrap())
            .into_values()
            .collect();
        for ring in &rings {
            ring.shutdown();
        }
        unsafe extern "C" fn release(data: *mut c_void) {
            let rings = unsafe { Box::from_raw(data.cast::<Vec<Arc<WgpuTextureRing>>>()) };
            for ring in rings.iter() {
                ring.release_gpu_resources();
            }
        }
        unsafe {
            FlutterRustAndroidStop(
                self.native.ptr(),
                release,
                Box::into_raw(Box::new(rings)).cast(),
            )
        };
    }
}
impl Drop for AndroidRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Backend {
    native: Arc<NativeRuntime>,
    dispatcher: MainThreadDispatcher,
    textures: Arc<Mutex<BTreeMap<i64, Arc<WgpuTextureRing>>>>,
}
impl Backend {
    fn create(&self, descriptor: TextureDescriptor, pixels: bool) -> Result<Arc<Texture>> {
        if !self.dispatcher.is_main_thread() {
            return Err(PluginError::Unsupported);
        }
        let ring = if pixels {
            self.native
                .gpu
                .create_pixel_buffer_texture_ring(descriptor.width, descriptor.height)
        } else {
            self.native
                .gpu
                .create_texture_ring(descriptor.width, descriptor.height)
        }
        .map_err(|_| PluginError::InvalidDescriptor)?;
        let ring = Arc::new(*ring);
        let id = unsafe { FlutterRustAndroidRegisterTexture(self.native.ptr(), ring.callbacks()) };
        if id < 0 {
            return Err(PluginError::Shutdown);
        }
        let native = self.native.clone();
        let dispatcher = self.dispatcher.clone();
        ring.set_registration(id, move |id| {
            let native = native.clone();
            dispatcher
                .dispatch(move || unsafe { FlutterRustAndroidMarkTexture(native.ptr(), id) })
                .map_err(|_| PluginError::Shutdown)
        });
        self.textures.lock().unwrap().insert(id, ring.clone());
        Ok(Arc::new(Texture {
            id,
            ring,
            native: self.native.clone(),
            dispatcher: self.dispatcher.clone(),
            textures: self.textures.clone(),
        }))
    }
}
impl WgpuTextureBackend for Backend {
    fn create_texture(
        &self,
        descriptor: TextureDescriptor,
    ) -> Result<Arc<dyn WgpuTextureBackendHandle>> {
        Ok(self.create(descriptor, false)?)
    }
    fn create_pixel_buffer_texture(
        &self,
        descriptor: TextureDescriptor,
    ) -> Result<Arc<dyn PixelBufferTextureBackendHandle>> {
        Ok(self.create(descriptor, true)?)
    }
}
struct Texture {
    id: i64,
    ring: Arc<WgpuTextureRing>,
    native: Arc<NativeRuntime>,
    dispatcher: MainThreadDispatcher,
    textures: Arc<Mutex<BTreeMap<i64, Arc<WgpuTextureRing>>>>,
}
impl Drop for Texture {
    fn drop(&mut self) {
        self.ring.shutdown();
        let native = self.native.clone();
        let textures = self.textures.clone();
        let id = self.id;
        let _ = self.dispatcher.dispatch(move || {
            if let Some(ring) = textures.lock().unwrap().remove(&id) {
                unsafe extern "C" fn release(data: *mut c_void) {
                    let ring = unsafe { Box::from_raw(data.cast::<Arc<WgpuTextureRing>>()) };
                    ring.release_gpu_resources();
                }
                unsafe {
                    FlutterRustAndroidUnregisterTexture(
                        native.ptr(),
                        id,
                        release,
                        Box::into_raw(Box::new(ring)).cast(),
                    )
                };
            }
        });
    }
}
#[async_trait::async_trait]
impl WgpuTextureBackendHandle for Texture {
    fn texture_id(&self) -> i64 {
        self.id
    }
    fn try_next_frame(&self) -> Result<Arc<dyn WgpuTextureFrameBackend>> {
        WgpuTextureBackendHandle::try_next_frame(self.ring.as_ref())
    }
    async fn next_frame(&self) -> Result<Arc<dyn WgpuTextureFrameBackend>> {
        WgpuTextureBackendHandle::next_frame(self.ring.as_ref()).await
    }
}
#[async_trait::async_trait]
impl PixelBufferTextureBackendHandle for Texture {
    fn texture_id(&self) -> i64 {
        self.id
    }
    fn try_next_frame(&self) -> Result<Arc<dyn PixelBufferTextureFrameBackend>> {
        PixelBufferTextureBackendHandle::try_next_frame(self.ring.as_ref())
    }
    async fn next_frame(&self) -> Result<Arc<dyn PixelBufferTextureFrameBackend>> {
        PixelBufferTextureBackendHandle::next_frame(self.ring.as_ref()).await
    }
}
