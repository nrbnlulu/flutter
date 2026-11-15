//! AppKit implementation of the narrow native-window adapter.
//!
//! Owned windows use AppKit child windows (`addChildWindow:ordered:`): a child
//! stays ordered above its owner and is hidden with it, the macOS equivalent
//! of a Win32 owner or an X11/Wayland transient parent. Unlike Win32 owned
//! windows, AppKit child windows also move when their owner is dragged.
//!
//! Every AppKit call here runs on the event-loop thread, which on macOS is
//! the main thread; `main_thread` enforces that before touching an NSWindow.
//!
//! Frames are paced by the shared nominal-interval timer fallback for now;
//! a display-link source belongs in `uses_host_vsync`/`next_vsync`, as
//! `windows_vsync.rs` does for DWM.

use flutter_shell_core::{FlutterRustKeyEvent, FlutterRustKeyEventType};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSView, NSWindow, NSWindowOrderingMode};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    event_loop::ActiveEventLoop,
    keyboard::{ModifiersState, PhysicalKey},
    platform::scancode::PhysicalKeyExtScancode,
    window::{Window, WindowAttributes},
};

use super::super::{NativeWindowKind, PopupPlacement};
use super::PlatformBackend;

pub(crate) struct MacPlatform;

/// `kCGEventSourceStateCombinedSessionState`.
const COMBINED_SESSION_STATE: i32 = 0;
/// `NSEventModifierFlagCapsLock` / `kCGEventFlagMaskAlphaShift`.
const CAPS_LOCK_FLAG: u64 = 1 << 16;
/// The device-independent and device-dependent modifier bits that
/// `NSEvent.modifierFlags` carries and Flutter's `RawKeyEventDataMacOs`
/// decodes (including the left/right bits in the low 16).
const MODIFIER_FLAGS_MASK: u64 = 0xffff_ffff;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// Returns the current modifier state with the same bit layout as
    /// `NSEventModifierFlags`, including device-dependent left/right bits.
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
}

fn modifier_flags() -> u64 {
    // SAFETY: CGEventSourceFlagsState takes a plain state id and has no
    // pointer arguments or thread requirements.
    unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) & MODIFIER_FLAGS_MASK }
}

fn main_thread() -> Result<MainThreadMarker, String> {
    MainThreadMarker::new().ok_or_else(|| "AppKit windows must be used on the main thread".into())
}

fn ns_window(window: &dyn Window, _mtm: MainThreadMarker) -> Result<Retained<NSWindow>, String> {
    let handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("Expected an AppKit window handle".into());
    };
    // SAFETY: winit keeps this NSView alive for the lifetime of `window`, and
    // the marker proves we are on the main thread that owns it.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    view.window()
        .ok_or_else(|| "AppKit view is not installed in a window".into())
}

/// Moves `child` under `parent`, keeping its current screen position.
fn attach_child(child: &dyn Window, parent: &dyn Window) -> Result<(), String> {
    let mtm = main_thread()?;
    let child = ns_window(child, mtm)?;
    let parent = ns_window(parent, mtm)?;
    // SAFETY: both windows are live and used on the main thread. The shared
    // host rejects ownership cycles before reaching this adapter.
    unsafe {
        if let Some(previous) = child.parentWindow() {
            previous.removeChildWindow(&child);
        }
        parent.addChildWindow_ordered(&child, NSWindowOrderingMode::Above);
    }
    Ok(())
}

fn encode_key(event: &FlutterRustKeyEvent, key_code: u32, modifiers: u64) -> Vec<u8> {
    let characters = std::str::from_utf8(&event.character[..event.character_length as usize])
        .unwrap_or_default();
    serde_json::to_vec(&serde_json::json!({
        "type": if event.event_type == FlutterRustKeyEventType::Up as u32 { "keyup" } else { "keydown" },
        "keymap": "macos",
        "keyCode": key_code,
        "characters": characters,
        // winit does not expose AppKit's unmodified characters. Flutter's
        // legacy macOS data falls back to the physical key when this is
        // empty or non-printable; the modern key event path carries the
        // exact logical key.
        "charactersIgnoringModifiers": characters,
        "modifiers": modifiers,
    }))
    .expect("macOS key message serializes")
}

impl PlatformBackend for MacPlatform {
    const LEGACY_KEYMAP: &'static str = "macos";
    // Flutter's macOS legacy schema has no toolkit field; this is only used
    // by the shared Linux-shaped encoder, which macOS overrides below.
    const LEGACY_TOOLKIT: &'static str = "macos";
    // Flutter's `LogicalKeyboardKey.macosPlane`
    // (packages/flutter/lib/src/services/keyboard_key.g.dart).
    const FALLBACK_KEY_PLANE: u64 = 0x01400000000;
    const INFERS_NUM_LOCK: bool = false;

    fn text_modifiers(modifiers: ModifiersState, _right_alt: bool) -> ModifiersState {
        // Either Option key composes characters on macOS (for example
        // Option+E is a dead key). Text produced with Option held is input,
        // as AppKit's text system treats it; Command and Control chords
        // remain shortcuts.
        let mut modifiers = modifiers;
        modifiers.remove(ModifiersState::ALT);
        modifiers
    }

    fn lock_state() -> Option<[bool; 3]> {
        // Unit tests inject lock state instead of reading the developer's keyboard.
        if cfg!(test) {
            return None;
        }
        // macOS keyboards have no NumLock or ScrollLock state.
        Some([modifier_flags() & CAPS_LOCK_FLAG != 0, false, false])
    }

    fn supports_window_kind(kind: NativeWindowKind) -> bool {
        matches!(kind, NativeWindowKind::Regular | NativeWindowKind::Dialog)
    }

    fn legacy_key_message(physical: PhysicalKey, event: &FlutterRustKeyEvent) -> Option<Vec<u8>> {
        // On macOS winit's scancode is the AppKit virtual key code
        // (`NSEvent.keyCode`), which is what Flutter's legacy data expects.
        let key_code = physical.to_scancode().unwrap_or(0);
        Some(encode_key(event, key_code, modifier_flags()))
    }

    fn prepare_window_attributes(
        attributes: WindowAttributes,
        _kind: NativeWindowKind,
    ) -> WindowAttributes {
        attributes
    }

    fn is_wayland(_event_loop: &dyn ActiveEventLoop) -> bool {
        false
    }

    fn create_popup(
        _event_loop: &dyn ActiveEventLoop,
        _parent: &dyn Window,
        _placement: PopupPlacement,
        _grab: bool,
    ) -> Result<Box<dyn Window>, String> {
        Err("native popups are not supported on macOS yet".to_owned())
    }

    fn set_dialog_parent(child: &dyn Window, parent: &dyn Window) -> Result<(), String> {
        attach_child(child, parent)
    }
}
