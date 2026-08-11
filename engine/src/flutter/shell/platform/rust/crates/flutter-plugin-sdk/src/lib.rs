//! Public, semantically versioned Rust API for Flutter Rust-shell plugins.
//!
//! The SDK deliberately exposes no Flutter C++ or Impeller types. Those remain
//! behind the private, lockstep engine bridge.

#![forbid(unsafe_code)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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
}

/// A convenient result type for plugin registration.
pub type Result<T> = core::result::Result<T, PluginError>;

/// A unit of work that is safe to transfer to the shell's main thread.
#[doc(hidden)]
pub type MainThreadTask = Box<dyn FnOnce() + Send + 'static>;

/// Error returned when work can no longer be posted to the main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchError {
    /// The application is shutting down and no longer accepts callbacks.
    Shutdown,
}

/// Worker-safe handle for posting short operations to Flutter's main thread.
///
/// Dispatch is always asynchronous, including when called from the main
/// thread. This prevents a callback from unexpectedly re-entering Dart or
/// mutable platform state in the middle of an FFI call.
#[derive(Clone)]
pub struct MainThreadDispatcher {
    post: Arc<dyn Fn(MainThreadTask) -> bool + Send + Sync>,
    main_thread: ThreadId,
    accepting: Arc<AtomicBool>,
}

impl MainThreadDispatcher {
    /// Posts `task` for a later turn of the main event loop.
    pub fn dispatch(
        &self,
        task: impl FnOnce() + Send + 'static,
    ) -> core::result::Result<(), DispatchError> {
        if !self.accepting.load(Ordering::Acquire) || !(self.post)(Box::new(task)) {
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
        Self {
            post: Arc::new(post),
            main_thread,
            accepting: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Stops this dispatcher and every clone from accepting new work.
    #[doc(hidden)]
    pub fn shutdown_for_shell(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}

/// The shell-owned registration context passed to every plugin.
///
/// Capabilities are added here as the shell implements them. Keeping this type
/// opaque prevents plugins from depending on private engine handles.
pub struct PluginRegistrar {
    main_thread_dispatcher: MainThreadDispatcher,
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
        }
    }

    /// Returns the dispatcher for main-thread-only platform operations.
    pub fn main_thread_dispatcher(&self) -> &MainThreadDispatcher {
        &self.main_thread_dispatcher
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
    use std::{collections::VecDeque, sync::Mutex};

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
}
