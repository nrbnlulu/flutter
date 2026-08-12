//! Public, semantically versioned Rust API for Flutter Rust-shell plugins.
//!
//! The SDK deliberately exposes no Flutter C++ or Impeller types. Those remain
//! behind the private, lockstep engine bridge.

#![forbid(unsafe_code)]

use std::task::{Context, Poll};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    thread::ThreadId,
};

/// The source compatibility version of this SDK.
pub const PLUGIN_SDK_API_VERSION: u32 = 1;

/// Errors returned while registering a Rust-shell plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginError {
    /// The plugin requires an SDK capability not provided by this shell build.
    Unsupported,
    /// A texture descriptor has zero or unsupported dimensions or format.
    InvalidDescriptor,
    /// Every ring slot is currently reserved, ready, or used by Flutter.
    Busy,
    /// The reserved frame has not recorded any commands yet.
    NoFrame,
    /// The texture or shell is shutting down.
    Shutdown,
}

/// A convenient result type for plugin registration.
pub type Result<T> = core::result::Result<T, PluginError>;

/// Pixel formats accepted by engine-owned GPU textures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// Eight-bit linear red, green, blue, and alpha channels.
    Rgba8Unorm,
}

/// Size and format of an engine-owned GPU texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDescriptor {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Storage and sampling format.
    pub format: TextureFormat,
}

impl TextureDescriptor {
    fn is_valid(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// One producer callback that records wgpu commands for an engine-owned slot.
#[doc(hidden)]
pub type WgpuRenderTask =
    Box<dyn FnOnce(&wgpu::Device, &mut wgpu::CommandEncoder, &wgpu::TextureView) + Send + 'static>;

/// Private runtime implementation behind a public [`WgpuTexture`].
#[doc(hidden)]
pub trait WgpuTextureBackendHandle: Send + Sync {
    /// Flutter texture-registry identifier.
    fn texture_id(&self) -> i64;
    /// Attempts to reserve one ring slot without blocking.
    fn try_next_frame(&self) -> Result<Arc<dyn WgpuTextureFrameBackend>>;
    /// Polls until one ring slot can be reserved.
    fn poll_next_frame(
        &self,
        context: &mut Context<'_>,
    ) -> Poll<Result<Arc<dyn WgpuTextureFrameBackend>>>;
}

/// Private runtime implementation behind one reserved [`WgpuTextureFrame`].
#[doc(hidden)]
pub trait WgpuTextureFrameBackend: Send + Sync {
    /// Records commands without submitting to the shared Vulkan queue.
    fn render(&self, task: WgpuRenderTask) -> Result<()>;
    /// Publishes the recorded slot and marks the Flutter texture dirty.
    fn present(&self) -> Result<()>;
}

/// Private runtime factory installed into [`PluginRegistrar`].
#[doc(hidden)]
pub trait WgpuTextureBackend: Send + Sync {
    /// Creates one engine-owned texture.
    fn create_texture(
        &self,
        descriptor: TextureDescriptor,
    ) -> Result<Arc<dyn WgpuTextureBackendHandle>>;
}

/// GPU texture creation capability exposed by the plugin registrar.
#[derive(Clone)]
pub struct GpuTextures {
    backend: Arc<dyn WgpuTextureBackend>,
}

impl GpuTextures {
    /// Creates a texture whose storage and synchronization are owned by the
    /// shell. The returned ID can be passed directly to Dart's `Texture` widget.
    pub fn create_texture(&self, descriptor: TextureDescriptor) -> Result<WgpuTexture> {
        if !descriptor.is_valid() {
            return Err(PluginError::InvalidDescriptor);
        }
        Ok(WgpuTexture {
            backend: self.backend.create_texture(descriptor)?,
        })
    }

    /// Constructs the capability from the private shell runtime.
    #[doc(hidden)]
    pub fn for_shell(backend: Arc<dyn WgpuTextureBackend>) -> Self {
        Self { backend }
    }
}

/// Safe producer handle for an engine-owned wgpu texture ring.
pub struct WgpuTexture {
    backend: Arc<dyn WgpuTextureBackendHandle>,
}

impl WgpuTexture {
    /// Identifier consumed by Flutter's Dart `Texture` widget.
    pub fn texture_id(&self) -> i64 {
        self.backend.texture_id()
    }

    /// Attempts to reserve an available ring slot without blocking.
    pub fn try_next_frame(&self) -> Result<WgpuTextureFrame> {
        Ok(WgpuTextureFrame::new(self.backend.try_next_frame()?))
    }

    /// Asynchronously waits until a ring slot can be reserved.
    /// Dropping the future cancels the wait without reserving a slot.
    pub async fn next_frame(&self) -> Result<WgpuTextureFrame> {
        std::future::poll_fn(|context| self.backend.poll_next_frame(context))
            .await
            .map(WgpuTextureFrame::new)
    }
}

/// Exclusive reservation of one engine-owned texture-ring slot.
pub struct WgpuTextureFrame {
    backend: Arc<dyn WgpuTextureFrameBackend>,
    rendered: bool,
}

impl WgpuTextureFrame {
    fn new(backend: Arc<dyn WgpuTextureFrameBackend>) -> Self {
        Self {
            backend,
            rendered: false,
        }
    }

    /// Records commands into this frame. A frame may be recorded once.
    pub fn render(
        &mut self,
        task: impl FnOnce(&wgpu::Device, &mut wgpu::CommandEncoder, &wgpu::TextureView) + Send + 'static,
    ) -> Result<()> {
        if self.rendered {
            return Err(PluginError::Busy);
        }
        self.backend.render(Box::new(task))?;
        self.rendered = true;
        Ok(())
    }

    /// Publishes this slot and schedules Flutter to repaint its existing
    /// texture layer. Consuming `self` prevents repeated publication.
    pub fn present(self) -> Result<()> {
        if !self.rendered {
            return Err(PluginError::NoFrame);
        }
        self.backend.present()
    }
}

/// A unit of work that is safe to transfer to the shell's main thread.
#[doc(hidden)]
pub type MainThreadTask = Box<dyn FnOnce() + Send + 'static>;

/// Error returned when work can no longer be posted to the main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchError {
    /// The shell has not completed initialization yet.
    NotReady,
    /// The application is shutting down and no longer accepts callbacks.
    Shutdown,
}

const DISPATCHER_STARTING: u8 = 0;
const DISPATCHER_RUNNING: u8 = 1;
const DISPATCHER_SHUTDOWN: u8 = 2;

/// Worker-safe handle for posting short operations to Flutter's main thread.
///
/// Dispatch is always asynchronous, including when called from the main
/// thread. This prevents a callback from unexpectedly re-entering Dart or
/// mutable platform state in the middle of an FFI call.
#[derive(Clone)]
pub struct MainThreadDispatcher {
    post: Arc<dyn Fn(MainThreadTask) -> bool + Send + Sync>,
    main_thread: ThreadId,
    state: Arc<AtomicU8>,
}

impl MainThreadDispatcher {
    /// Posts `task` for a later turn of the main event loop.
    pub fn dispatch(
        &self,
        task: impl FnOnce() + Send + 'static,
    ) -> core::result::Result<(), DispatchError> {
        match self.state.load(Ordering::Acquire) {
            DISPATCHER_STARTING => return Err(DispatchError::NotReady),
            DISPATCHER_SHUTDOWN => return Err(DispatchError::Shutdown),
            DISPATCHER_RUNNING => {}
            _ => unreachable!("invalid main-thread dispatcher state"),
        }
        let state = Arc::clone(&self.state);
        let guarded_task = Box::new(move || {
            if state.load(Ordering::Acquire) == DISPATCHER_RUNNING {
                task();
            }
        });
        if !(self.post)(guarded_task) {
            return Err(DispatchError::Shutdown);
        }
        Ok(())
    }

    /// Whether the caller is currently running on the owning main thread.
    pub fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.main_thread
    }

    /// Creates a dispatcher backed by the private shell runtime.
    #[doc(hidden)]
    pub fn for_shell(
        post: impl Fn(MainThreadTask) -> bool + Send + Sync + 'static,
        main_thread: ThreadId,
    ) -> Self {
        Self::for_shell_with_state(post, main_thread, DISPATCHER_RUNNING)
    }

    /// Creates a dispatcher that rejects work until shell startup completes.
    #[doc(hidden)]
    pub fn for_shell_inactive(
        post: impl Fn(MainThreadTask) -> bool + Send + Sync + 'static,
        main_thread: ThreadId,
    ) -> Self {
        Self::for_shell_with_state(post, main_thread, DISPATCHER_STARTING)
    }

    fn for_shell_with_state(
        post: impl Fn(MainThreadTask) -> bool + Send + Sync + 'static,
        main_thread: ThreadId,
        state: u8,
    ) -> Self {
        Self {
            post: Arc::new(post),
            main_thread,
            state: Arc::new(AtomicU8::new(state)),
        }
    }

    /// Enables dispatch after the shell and its implicit view are initialized.
    #[doc(hidden)]
    pub fn start_for_shell(&self) -> bool {
        self.state
            .compare_exchange(
                DISPATCHER_STARTING,
                DISPATCHER_RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Stops this dispatcher and every clone from accepting new work.
    #[doc(hidden)]
    pub fn shutdown_for_shell(&self) {
        self.state.store(DISPATCHER_SHUTDOWN, Ordering::Release);
    }
}

/// The shell-owned registration context passed to every plugin.
///
/// Capabilities are added here as the shell implements them. Keeping this type
/// opaque prevents plugins from depending on private engine handles.
pub struct PluginRegistrar {
    main_thread_dispatcher: MainThreadDispatcher,
    gpu_textures: Option<GpuTextures>,
}

impl PluginRegistrar {
    /// Creates the registrar used by the shell during application startup.
    ///
    /// This is public only so the private shell runtime can construct the
    /// registrar across crate boundaries; plugin code should only receive it
    /// from [`FlutterRustPlugin::register`].
    #[doc(hidden)]
    pub fn for_shell(main_thread_dispatcher: MainThreadDispatcher) -> Self {
        Self {
            main_thread_dispatcher,
            gpu_textures: None,
        }
    }

    /// Returns the dispatcher for main-thread-only platform operations.
    pub fn main_thread_dispatcher(&self) -> &MainThreadDispatcher {
        &self.main_thread_dispatcher
    }

    /// Returns the engine-owned wgpu texture capability.
    pub fn gpu(&self) -> Result<&GpuTextures> {
        self.gpu_textures.as_ref().ok_or(PluginError::Unsupported)
    }

    /// Installs the shell's GPU backend after its engine and implicit view are
    /// ready. Plugin code must not call this method.
    #[doc(hidden)]
    pub fn install_gpu_for_shell(&mut self, gpu_textures: GpuTextures) -> bool {
        if self.gpu_textures.is_some() {
            return false;
        }
        self.gpu_textures = Some(gpu_textures);
        true
    }
}

/// A source-linked plugin compiled into the application's Rust aggregate.
pub trait FlutterRustPlugin: Send + Sync + 'static {
    /// Registers the plugin's platform services, FRB APIs, and textures.
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::Mutex,
        task::{Wake, Waker},
    };

    struct FakeGpuBackend {
        operations: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeTextureBackend {
        operations: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeFrameBackend {
        operations: Arc<Mutex<Vec<&'static str>>>,
    }

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    impl WgpuTextureBackend for FakeGpuBackend {
        fn create_texture(
            &self,
            _descriptor: TextureDescriptor,
        ) -> Result<Arc<dyn WgpuTextureBackendHandle>> {
            self.operations.lock().unwrap().push("create");
            Ok(Arc::new(FakeTextureBackend {
                operations: Arc::clone(&self.operations),
            }))
        }
    }

    impl WgpuTextureBackendHandle for FakeTextureBackend {
        fn texture_id(&self) -> i64 {
            17
        }

        fn try_next_frame(&self) -> Result<Arc<dyn WgpuTextureFrameBackend>> {
            self.operations.lock().unwrap().push("reserve");
            Ok(Arc::new(FakeFrameBackend {
                operations: Arc::clone(&self.operations),
            }))
        }

        fn poll_next_frame(
            &self,
            _context: &mut Context<'_>,
        ) -> Poll<Result<Arc<dyn WgpuTextureFrameBackend>>> {
            Poll::Ready(self.try_next_frame())
        }
    }

    impl WgpuTextureFrameBackend for FakeFrameBackend {
        fn render(&self, _task: WgpuRenderTask) -> Result<()> {
            self.operations.lock().unwrap().push("render");
            Ok(())
        }

        fn present(&self) -> Result<()> {
            self.operations.lock().unwrap().push("present");
            Ok(())
        }
    }

    struct TestPlugin;

    impl FlutterRustPlugin for TestPlugin {
        fn register(&self, _registrar: &mut PluginRegistrar) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn plugins_register_with_the_shell_registrar() {
        let queue = Arc::new(Mutex::new(VecDeque::<MainThreadTask>::new()));
        let queue_for_post = Arc::clone(&queue);
        let dispatcher = MainThreadDispatcher::for_shell(
            move |task| {
                queue_for_post.lock().unwrap().push_back(task);
                true
            },
            std::thread::current().id(),
        );
        let mut registrar = PluginRegistrar::for_shell(dispatcher);
        TestPlugin.register(&mut registrar).unwrap();
    }

    #[test]
    fn gpu_capability_validates_and_hides_the_runtime_backend() {
        let dispatcher = MainThreadDispatcher::for_shell(|_| true, std::thread::current().id());
        let mut registrar = PluginRegistrar::for_shell(dispatcher);
        assert!(matches!(registrar.gpu(), Err(PluginError::Unsupported)));

        let operations = Arc::new(Mutex::new(Vec::new()));
        let capability = GpuTextures::for_shell(Arc::new(FakeGpuBackend {
            operations: Arc::clone(&operations),
        }));
        assert!(registrar.install_gpu_for_shell(capability.clone()));
        assert!(!registrar.install_gpu_for_shell(capability));
        assert!(matches!(
            registrar.gpu().unwrap().create_texture(TextureDescriptor {
                width: 0,
                height: 32,
                format: TextureFormat::Rgba8Unorm,
            }),
            Err(PluginError::InvalidDescriptor)
        ));

        let texture = registrar
            .gpu()
            .unwrap()
            .create_texture(TextureDescriptor {
                width: 64,
                height: 32,
                format: TextureFormat::Rgba8Unorm,
            })
            .unwrap();
        assert_eq!(texture.texture_id(), 17);
        assert!(matches!(
            texture.try_next_frame().unwrap().present(),
            Err(PluginError::NoFrame)
        ));
        let mut frame = texture.try_next_frame().unwrap();
        frame.render(|_, _, _| {}).unwrap();
        assert_eq!(frame.render(|_, _, _| {}), Err(PluginError::Busy));
        frame.present().unwrap();

        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(texture.next_frame());
        let Poll::Ready(Ok(mut frame)) = future.as_mut().poll(&mut context) else {
            panic!("fake frame future was not ready");
        };
        frame.render(|_, _, _| {}).unwrap();
        frame.present().unwrap();
        assert_eq!(
            *operations.lock().unwrap(),
            vec![
                "create", "reserve", "reserve", "render", "present", "reserve", "render", "present"
            ]
        );
    }

    #[test]
    fn worker_dispatch_is_deferred_non_reentrant_and_rejects_shutdown() {
        let queue = Arc::new(Mutex::new(VecDeque::<MainThreadTask>::new()));
        let queue_for_post = Arc::clone(&queue);
        let dispatcher = MainThreadDispatcher::for_shell(
            move |task| {
                queue_for_post.lock().unwrap().push_back(task);
                true
            },
            std::thread::current().id(),
        );
        let order = Arc::new(Mutex::new(Vec::new()));
        let worker_dispatcher = dispatcher.clone();
        let nested_dispatcher = dispatcher.clone();
        let order_in_task = Arc::clone(&order);
        std::thread::spawn(move || {
            assert!(!worker_dispatcher.is_main_thread());
            worker_dispatcher
                .dispatch(move || {
                    assert!(nested_dispatcher.is_main_thread());
                    order_in_task.lock().unwrap().push(1);
                    let nested_order = Arc::clone(&order_in_task);
                    nested_dispatcher
                        .dispatch(move || nested_order.lock().unwrap().push(2))
                        .unwrap();
                })
                .unwrap();
        })
        .join()
        .unwrap();

        assert!(order.lock().unwrap().is_empty());
        let first = queue.lock().unwrap().pop_front().unwrap();
        first();
        assert_eq!(*order.lock().unwrap(), vec![1]);
        let second = queue.lock().unwrap().pop_front().unwrap();
        second();
        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
        assert!(dispatcher.is_main_thread());

        dispatcher.shutdown_for_shell();
        assert_eq!(dispatcher.dispatch(|| {}), Err(DispatchError::Shutdown));
    }

    #[test]
    fn startup_and_shutdown_gate_queued_callbacks_deterministically() {
        for _ in 0..100 {
            let queue = Arc::new(Mutex::new(VecDeque::<MainThreadTask>::new()));
            let queue_for_post = Arc::clone(&queue);
            let dispatcher = MainThreadDispatcher::for_shell_inactive(
                move |task| {
                    queue_for_post.lock().unwrap().push_back(task);
                    true
                },
                std::thread::current().id(),
            );
            assert_eq!(dispatcher.dispatch(|| {}), Err(DispatchError::NotReady));
            assert!(queue.lock().unwrap().is_empty());
            assert!(dispatcher.start_for_shell());
            assert!(!dispatcher.start_for_shell());

            let ran = Arc::new(AtomicU8::new(0));
            let ran_in_task = Arc::clone(&ran);
            dispatcher
                .dispatch(move || ran_in_task.store(1, Ordering::Release))
                .unwrap();
            dispatcher.shutdown_for_shell();
            queue.lock().unwrap().pop_front().unwrap()();
            assert_eq!(ran.load(Ordering::Acquire), 0);
            assert_eq!(dispatcher.dispatch(|| {}), Err(DispatchError::Shutdown));
        }
    }
}
