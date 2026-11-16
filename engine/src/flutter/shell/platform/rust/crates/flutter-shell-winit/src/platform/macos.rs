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
use objc2_app_kit::{NSScreen, NSView, NSWindow, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    event_loop::ActiveEventLoop,
    keyboard::{ModifiersState, PhysicalKey},
    platform::{macos::WindowAttributesMacOS, scancode::PhysicalKeyExtScancode},
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

/// Height of the primary screen, whose bottom-left corner is the origin of
/// AppKit's global coordinates. Top-left global coordinates (used by winit,
/// CGWindowList and the shared positioner) flip `y` against it.
fn primary_screen_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |screen| screen.frame().size.height)
}

/// Converts an AppKit rect to top-left global `(x, y, width, height)` points.
fn top_left(rect: NSRect, screen_height: f64) -> (f64, f64, f64, f64) {
    (
        rect.origin.x,
        screen_height - rect.origin.y - rect.size.height,
        rect.size.width,
        rect.size.height,
    )
}

/// Places a popup relative to `parent`'s content area and the work area
/// (`visibleFrame`, which excludes the menu bar and Dock) of its screen.
/// Returns an AppKit frame. macOS points are the shared positioner's
/// logical units.
fn popup_frame(parent: &NSWindow, placement: PopupPlacement, mtm: MainThreadMarker) -> NSRect {
    let height = primary_screen_height(mtm);
    let content = top_left(parent.contentRectForFrameRect(parent.frame()), height);
    let work = parent
        .screen()
        .or_else(|| NSScreen::mainScreen(mtm))
        .map_or(content, |screen| top_left(screen.visibleFrame(), height));
    let (x, y, width, height_points) =
        super::positioner::place(placement, (content.0, content.1), work);
    NSRect::new(
        NSPoint::new(x, height - y - height_points),
        NSSize::new(width, height_points),
    )
}

impl MacPlatform {
    /// AppKit has no pointer capture to release; outside presses are routed
    /// by the shared host instead (see `WindowEvent::PointerButton`).
    pub(crate) fn release_popup(_window: &dyn Window) {}
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
        matches!(
            kind,
            NativeWindowKind::Regular
                | NativeWindowKind::Dialog
                | NativeWindowKind::Satellite
                | NativeWindowKind::Tooltip
                | NativeWindowKind::Popup
        )
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

    fn configure_native_window(window: &dyn Window, kind: NativeWindowKind) -> Result<(), String> {
        if kind == NativeWindowKind::Satellite {
            // Like a Win32 tool window, an owned utility window gets no
            // independent entry in the app's Window menu.
            let mtm = main_thread()?;
            ns_window(window, mtm)?.setExcludedFromWindowsMenu(true);
        }
        Ok(())
    }

    fn is_wayland(_event_loop: &dyn ActiveEventLoop) -> bool {
        false
    }

    fn create_popup(
        event_loop: &dyn ActiveEventLoop,
        parent: &dyn Window,
        placement: PopupPlacement,
        _grab: bool,
    ) -> Result<Box<dyn Window>, String> {
        let mtm = main_thread()?;
        let frame = popup_frame(&*ns_window(parent, mtm)?, placement, mtm);
        let window = event_loop
            .create_window(
                WindowAttributes::default()
                    // Untitled, like a Win32 popup: no "winit window" default.
                    .with_title("")
                    .with_visible(false)
                    .with_active(false)
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_surface_size(winit::dpi::LogicalSize::new(
                        frame.size.width,
                        frame.size.height,
                    ))
                    .with_platform_attributes(Box::new(
                        WindowAttributesMacOS::default()
                            .with_panel(true)
                            .with_accepts_first_mouse(true),
                    )),
            )
            .map_err(|error| error.to_string())?;
        let native = ns_window(window.as_ref(), mtm)?;
        // A non-activating panel never activates the app, but AppKit still
        // makes it key when it is clicked. The shared host therefore treats
        // focus moving into a popup as staying inside the grab.
        // Panels hide when the app deactivates by default; owned Win32 and
        // Wayland tooltips stay until their owner removes them.
        native.setHidesOnDeactivate(false);
        native.setFrame_display(frame, false);
        // A child window follows its owner when the owner moves, is ordered
        // above it and is hidden with it. Show with orderFront: winit's
        // set_visible uses makeKeyAndOrderFront.
        attach_child(window.as_ref(), parent)?;
        native.orderFront(None);
        Ok(window)
    }

    fn reposition_popup(
        window: &dyn Window,
        parent: &dyn Window,
        placement: PopupPlacement,
    ) -> Result<(), String> {
        let mtm = main_thread()?;
        let native = ns_window(window, mtm)?;
        let frame = popup_frame(&*ns_window(parent, mtm)?, placement, mtm);
        let current = native.frame();
        // Native moves and resizes emit more winit events. Avoid a feedback
        // loop when the window already has the requested geometry.
        let same = |a: f64, b: f64| (a - b).abs() < 0.5;
        if !(same(current.origin.x, frame.origin.x)
            && same(current.origin.y, frame.origin.y)
            && same(current.size.width, frame.size.width)
            && same(current.size.height, frame.size.height))
        {
            native.setFrame_display(frame, true);
        }
        Ok(())
    }

    fn set_dialog_parent(child: &dyn Window, parent: &dyn Window) -> Result<(), String> {
        attach_child(child, parent)
    }
}
