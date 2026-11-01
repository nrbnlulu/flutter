//! Bounded latest-frame mailbox. The issued token owns the producer lease even
//! after texture unregistration; C++ returns it after the final GPU reader.
use flutter_plugin_sdk::{PluginError, Result, android::HardwareBufferFrame};
use flutter_shell_core::{FlutterRustExternalTextureCallbacks, FlutterRustExternalTextureFrame};
use std::{
    ffi::c_void,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    pending: Option<HardwareBufferFrame>,
    outstanding: usize,
    shutdown: bool,
    failed: bool,
}

#[derive(Default)]
pub(super) struct HardwareBufferMailbox(Mutex<State>);

struct AcquiredFrame {
    mailbox: Arc<HardwareBufferMailbox>,
    frame: HardwareBufferFrame,
}

impl Drop for AcquiredFrame {
    fn drop(&mut self) {
        self.mailbox.0.lock().unwrap().outstanding -= 1;
    }
}

impl HardwareBufferMailbox {
    pub fn present(&self, frame: HardwareBufferFrame) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.shutdown {
            return Err(PluginError::Shutdown);
        }
        if state.failed {
            return Err(PluginError::Unsupported);
        }
        // Two frames can still be sampled by Flutter while a third is pending.
        // Replacing a pending frame never increases decoder-buffer pressure.
        if state.pending.is_none() && state.outstanding >= 3 {
            return Err(PluginError::Busy);
        }
        let previous = state.pending.replace(frame);
        if previous.is_none() {
            state.outstanding += 1;
        }
        drop(state);
        // Decoder lease destruction can wake streaming threads. Never do that
        // under the mailbox lock, including during replacement and teardown.
        drop(previous);
        Ok(())
    }

    pub fn shutdown(&self) {
        let pending = {
            let mut state = self.0.lock().unwrap();
            state.shutdown = true;
            let pending = state.pending.take();
            if pending.is_some() {
                state.outstanding -= 1;
            }
            pending
        };
        drop(pending);
    }

    pub fn callbacks(self: &Arc<Self>) -> FlutterRustExternalTextureCallbacks {
        FlutterRustExternalTextureCallbacks {
            user_data: Arc::as_ptr(self).cast_mut().cast(),
            acquire_frame: Some(acquire),
            release_frame: Some(release),
        }
    }

    fn acquire(self: &Arc<Self>) -> Option<Box<AcquiredFrame>> {
        let frame = self.0.lock().unwrap().pending.take()?;
        Some(Box::new(AcquiredFrame {
            mailbox: self.clone(),
            frame,
        }))
    }
}

extern "C" fn acquire(
    data: *mut c_void,
    _: u32,
    _: u32,
    out: *mut FlutterRustExternalTextureFrame,
) -> i32 {
    if data.is_null() || out.is_null() {
        return 0;
    }
    // SAFETY: the registration retains this Arc through raster unregistration.
    let mailbox = unsafe { &*data.cast::<HardwareBufferMailbox>() };
    // The raw pointer comes from Arc::as_ptr, not an allocation of the value.
    unsafe { Arc::increment_strong_count(mailbox) };
    let mailbox = unsafe { Arc::from_raw(mailbox) };
    let Some(frame) = mailbox.acquire() else {
        return 0;
    };
    let crop = frame.frame.crop;
    let result = FlutterRustExternalTextureFrame {
        image: 0,
        image_view: 0,
        format: 0,
        width: crop[2],
        height: crop[3],
        acquire_semaphore: 0,
        render_semaphore: 0,
        hardware_buffer: frame.frame.as_ptr() as u64,
        frame_token: Box::into_raw(frame) as u64,
        crop,
    };
    unsafe {
        *out = result;
    }
    1
}

extern "C" fn release(_: *mut c_void, frame: FlutterRustExternalTextureFrame) {
    if frame.frame_token != 0 {
        // SAFETY: C++ returns each acquired token exactly once, after GPU use.
        // Do not dereference user_data: the texture may already be unregistered.
        let acquired = unsafe { Box::from_raw(frame.frame_token as *mut AcquiredFrame) };
        if frame.format == u32::MAX {
            acquired.mailbox.0.lock().unwrap().failed = true;
        }
        drop(acquired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Lease(Arc<AtomicUsize>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn frame(drops: &Arc<AtomicUsize>) -> HardwareBufferFrame {
        // The mailbox never dereferences the pointer; no frame is sent to C++.
        unsafe {
            HardwareBufferFrame::new(
                std::ptr::NonNull::dangling(),
                [0, 0, 16, 16],
                Lease(drops.clone()),
            )
            .unwrap()
        }
    }

    #[test]
    fn replaces_pending_and_bounds_issued_leases() {
        let mailbox = Arc::new(HardwareBufferMailbox::default());
        let drops = Arc::new(AtomicUsize::new(0));
        mailbox.present(frame(&drops)).unwrap();
        mailbox.present(frame(&drops)).unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        let first = mailbox.acquire().unwrap();
        mailbox.present(frame(&drops)).unwrap();
        let second = mailbox.acquire().unwrap();
        mailbox.present(frame(&drops)).unwrap();
        let third = mailbox.acquire().unwrap();
        assert_eq!(mailbox.present(frame(&drops)), Err(PluginError::Busy));
        drop(first);
        mailbox.present(frame(&drops)).unwrap();
        mailbox.shutdown();
        assert_eq!(mailbox.present(frame(&drops)), Err(PluginError::Shutdown));
        // Issued buffers remain leased through unregister/shutdown.
        assert_eq!(mailbox.0.lock().unwrap().outstanding, 2);
        drop((second, third));
        assert_eq!(mailbox.0.lock().unwrap().outstanding, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 7);
    }
    #[test]
    fn raster_import_failure_reaches_producer_after_unregister_safe_release() {
        let mailbox = Arc::new(HardwareBufferMailbox::default());
        let drops = Arc::new(AtomicUsize::new(0));
        mailbox.present(frame(&drops)).unwrap();
        let callbacks = mailbox.callbacks();
        let mut acquired = std::mem::MaybeUninit::uninit();
        assert_eq!(
            callbacks.acquire_frame.unwrap()(callbacks.user_data, 16, 16, acquired.as_mut_ptr()),
            1
        );
        let mut acquired = unsafe { acquired.assume_init() };
        acquired.format = u32::MAX;
        // The native image release is valid without the registration pointer.
        callbacks.release_frame.unwrap()(std::ptr::null_mut(), acquired);
        assert_eq!(
            mailbox.present(frame(&drops)),
            Err(PluginError::Unsupported)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }
}
