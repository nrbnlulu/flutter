//! Direct sampling of producer-owned Android hardware buffers.
//!
//! The producer must finish its acquire-fence wait before presenting. The shell
//! retains the supplied owner until GPU sampling and the foreign-queue ownership
//! release complete. Keeping an AHardwareBuffer allocation alive alone does not
//! prevent an ImageReader/decoder from reusing its pixels: retain the image lease.

use crate::{PluginError, Result};
use std::{ffi::c_void, ptr::NonNull, sync::Arc};

/// A ready, immutable Android hardware buffer and its producer's image lease.
pub struct HardwareBufferFrame {
    buffer: NonNull<c_void>,
    /// Visible source rectangle in hardware-buffer pixels: x, y, width, height.
    pub crop: [u32; 4],
    _owner: Box<dyn Send + Sync>,
}

// SAFETY: the constructor requires cross-thread validity, ready pixels, and an
// owner that prevents recycling until the last GPU reader has completed.
unsafe impl Send for HardwareBufferFrame {}
unsafe impl Sync for HardwareBufferFrame {}

impl HardwareBufferFrame {
    /// Wraps a ready native buffer without copying or acquiring its allocation.
    ///
    /// # Safety
    /// `buffer` must point to a valid, GPU-sampleable AHardwareBuffer. `owner`
    /// must keep both its allocation and its immutable contents alive on any
    /// thread until dropped. All producer writes and acquire-fence waits must
    /// be complete. The buffer must be available for foreign-queue acquisition.
    /// Only one frame lease may import this buffer at a time.
    pub unsafe fn new(
        buffer: NonNull<c_void>,
        crop: [u32; 4],
        owner: impl Send + Sync + 'static,
    ) -> Result<Self> {
        if crop[2] == 0
            || crop[3] == 0
            || crop[0].checked_add(crop[2]).is_none()
            || crop[1].checked_add(crop[3]).is_none()
        {
            return Err(PluginError::InvalidDescriptor);
        }
        Ok(Self {
            buffer,
            crop,
            _owner: Box::new(owner),
        })
    }

    /// Borrowed pointer, valid only while this frame remains retained.
    pub fn as_ptr(&self) -> *mut c_void {
        self.buffer.as_ptr()
    }
}

/// Private runtime implementation behind a hardware-buffer texture.
#[doc(hidden)]
pub trait HardwareBufferTextureBackend: Send + Sync {
    fn texture_id(&self) -> i64;
    /// Transfers the image lease; `Busy` drops the incoming frame normally.
    fn present(&self, frame: HardwareBufferFrame) -> Result<()>;
}

/// Engine-registered Android texture; no CPU pixels or intermediate GPU image.
pub struct HardwareBufferTexture {
    backend: Arc<dyn HardwareBufferTextureBackend>,
}

impl HardwareBufferTexture {
    #[doc(hidden)]
    pub fn for_shell(backend: Arc<dyn HardwareBufferTextureBackend>) -> Self {
        Self { backend }
    }
    pub fn texture_id(&self) -> i64 {
        self.backend.texture_id()
    }
    /// Publishes the latest frame. The runtime bounds outstanding image leases.
    pub fn present(&self, frame: HardwareBufferFrame) -> Result<()> {
        self.backend.present(frame)
    }
}
