//! Native window-system operations that are not expressed by winit's shared API.

use winit::{
    event_loop::ActiveEventLoop,
    window::{Window, WindowAttributes},
};

use super::{NativeWindowKind, PopupPlacement};

#[cfg(any(target_os = "windows", test))]
#[path = "platform/positioner.rs"]
mod positioner;

#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
pub(crate) use linux::LinuxPlatform as CurrentPlatform;

#[cfg(target_os = "android")]
#[path = "platform/android.rs"]
mod android;

#[cfg(target_os = "android")]
pub(crate) use android::AndroidPlatform as CurrentPlatform;

#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod windows;

#[cfg(target_os = "windows")]
pub(crate) use windows::WindowsPlatform as CurrentPlatform;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "android")))]
compile_error!(
    "flutter-shell-winit needs a platform adapter for this target; the shared host is platform-neutral"
);

/// Statically selected native operations not covered by winit's shared API.
pub(crate) trait PlatformBackend {
    const LEGACY_KEYMAP: &'static str;
    const LEGACY_TOOLKIT: &'static str;
    const FALLBACK_KEY_PLANE: u64;
    /// Whether Flutter's text shortcuts depend on NumLock on this platform
    /// and winit exposes no lock state, so the host infers it from numpad
    /// keys (see `KeyboardState::inferred_num_lock`).
    const INFERS_NUM_LOCK: bool;

    fn text_modifiers(
        modifiers: winit::keyboard::ModifiersState,
        _right_alt: bool,
    ) -> winit::keyboard::ModifiersState {
        modifiers
    }

    /// CapsLock, NumLock, ScrollLock as observed on the native event thread.
    fn lock_state() -> Option<[bool; 3]> {
        None
    }

    fn supports_window_kind(_kind: NativeWindowKind) -> bool {
        true
    }

    fn legacy_key_message(
        _physical: winit::keyboard::PhysicalKey,
        _event: &flutter_shell_core::FlutterRustKeyEvent,
    ) -> Option<Vec<u8>> {
        None
    }

    fn prepare_window_attributes(
        attributes: WindowAttributes,
        kind: NativeWindowKind,
    ) -> WindowAttributes;

    fn configure_native_window(
        _window: &dyn Window,
        _kind: NativeWindowKind,
    ) -> Result<(), String> {
        Ok(())
    }

    fn is_wayland(event_loop: &dyn ActiveEventLoop) -> bool;

    fn create_popup(
        event_loop: &dyn ActiveEventLoop,
        parent: &dyn Window,
        placement: PopupPlacement,
        grab: bool,
    ) -> Result<Box<dyn Window>, String>;

    fn set_dialog_parent(child: &dyn Window, parent: &dyn Window) -> Result<(), String>;
}
