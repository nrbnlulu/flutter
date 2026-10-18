//! The only sanctioned way to touch the shared Vulkan queue.
//!
//! The Rust shell shares one `VkQueue` between wgpu and Impeller. Impeller
//! serializes its own submissions with an internal mutex, and Vulkan requires
//! host access to a queue to be externally synchronized. [`SharedQueue`] wraps
//! `wgpu::Queue` and brackets every operation that can reach `vkQueue*` with
//! that same mutex, through hooks the engine installs. The raw `wgpu::Queue`
//! is never exposed: code that needs queue access (shell internals and
//! plugins alike) must go through this crate.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use ash::vk;
    use parking_lot::RwLock;
    use std::{ffi::c_void, marker::PhantomData};

    /// Engine-provided lock functions guarding the shared `VkQueue`.
    ///
    /// `lock` and `unlock` are called on the same thread, never re-entrantly.
    #[derive(Clone, Copy)]
    pub struct QueueLockHooks {
        pub user_data: *mut c_void,
        pub lock: unsafe extern "C" fn(*mut c_void),
        pub unlock: unsafe extern "C" fn(*mut c_void),
    }

    // SAFETY: the hooks are plain function pointers plus an opaque engine
    // handle that the engine documents as callable from any thread.
    unsafe impl Send for QueueLockHooks {}
    unsafe impl Sync for QueueLockHooks {}

    /// Holds the engine's queue mutex until dropped. A no-op when no hooks
    /// are installed (before the engine exists, or after it shut down).
    #[must_use = "the queue is only protected while the guard is alive"]
    pub struct QueueGuard<'a> {
        hooks: Option<QueueLockHooks>,
        // Not Send: the engine mutex must be released on the locking thread.
        _not_send: PhantomData<(&'a (), *mut ())>,
    }

    impl Drop for QueueGuard<'_> {
        fn drop(&mut self) {
            if let Some(hooks) = self.hooks {
                // SAFETY: paired with the `lock` call in `SharedQueue::lock`.
                unsafe { (hooks.unlock)(hooks.user_data) };
            }
        }
    }

    /// A semaphore the next submission must wait on.
    #[derive(Clone, Copy)]
    pub struct WaitSemaphore {
        pub semaphore: vk::Semaphore,
        pub stage: vk::PipelineStageFlags,
    }

    /// `wgpu::Queue` whose host-side queue accesses are serialized with
    /// Impeller's.
    pub struct SharedQueue {
        queue: wgpu::Queue,
        hooks: RwLock<Option<QueueLockHooks>>,
    }

    impl SharedQueue {
        pub fn new(queue: wgpu::Queue) -> Self {
            Self {
                queue,
                hooks: RwLock::new(None),
            }
        }

        /// Installs the engine lock. Call right after the engine is created,
        /// before Impeller can submit from any thread.
        ///
        /// # Safety
        /// The hooks must stay valid until [`Self::clear_lock_hooks`] returns
        /// and no [`QueueGuard`] obtained afterwards is still alive.
        pub unsafe fn install_lock_hooks(&self, hooks: QueueLockHooks) {
            *self.hooks.write() = Some(hooks);
        }

        /// Removes the engine lock. Call before the engine is destroyed.
        pub fn clear_lock_hooks(&self) {
            *self.hooks.write() = None;
        }

        /// Takes the queue lock. Not re-entrant: do not call other
        /// `SharedQueue` methods while a guard is alive.
        pub fn lock(&self) -> QueueGuard<'_> {
            let hooks = *self.hooks.read();
            if let Some(hooks) = hooks {
                // SAFETY: guaranteed valid by `install_lock_hooks`.
                unsafe { (hooks.lock)(hooks.user_data) };
            }
            QueueGuard {
                hooks,
                _not_send: PhantomData,
            }
        }

        /// Submits command buffers.
        pub fn submit(
            &self,
            commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
        ) -> wgpu::SubmissionIndex {
            let _guard = self.lock();
            self.queue.submit(commands)
        }

        /// Submits command buffers that wait on and signal raw Vulkan binary
        /// semaphores. Registration and submission happen under one lock so
        /// the semaphores can never attach to another thread's submission.
        /// Returns `None` if the queue is not a Vulkan queue.
        pub fn submit_with_semaphores(
            &self,
            waits: &[WaitSemaphore],
            signals: &[vk::Semaphore],
            commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
        ) -> Option<wgpu::SubmissionIndex> {
            let _guard = self.lock();
            {
                // SAFETY: the HAL queue is only used to register semaphores.
                let hal = unsafe { self.queue.as_hal::<wgpu::hal::vulkan::Api>() }?;
                for wait in waits {
                    hal.add_wait_semaphore(wait.semaphore, None, wait.stage);
                }
                for signal in signals {
                    hal.add_signal_semaphore(*signal, None);
                }
            }
            Some(self.queue.submit(commands))
        }

        /// Presents a surface texture.
        pub fn present(&self, texture: wgpu::SurfaceTexture) {
            let _guard = self.lock();
            self.queue.present(texture);
        }

        /// Stages a texture write; it reaches the GPU on the next submit.
        pub fn write_texture(
            &self,
            texture: wgpu::TexelCopyTextureInfo<'_>,
            data: &[u8],
            layout: wgpu::TexelCopyBufferLayout,
            size: wgpu::Extent3d,
        ) {
            let _guard = self.lock();
            self.queue.write_texture(texture, data, layout, size);
        }

        /// `vkDeviceWaitIdle`, which touches every queue and so needs the
        /// same external synchronization. Returns `false` for non-Vulkan
        /// devices.
        pub fn wait_device_idle(&self, device: &wgpu::Device) -> bool {
            let _guard = self.lock();
            // SAFETY: the raw device is only used for the idle wait.
            let Some(hal) = (unsafe { device.as_hal::<wgpu::hal::vulkan::Api>() }) else {
                return false;
            };
            unsafe { hal.raw_device().device_wait_idle() }.is_ok()
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use imp::*;
