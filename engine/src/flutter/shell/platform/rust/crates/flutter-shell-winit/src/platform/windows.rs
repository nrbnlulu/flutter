//! Minimal Windows host: ordinary windows, timer pacing, and Windows key JSON.

use super::super::{NativeWindowKind, PopupPlacement};
use super::PlatformBackend;
use flutter_shell_core::{FlutterRustKeyEvent, FlutterRustKeyEventType};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;
use std::time::Instant;
use winit::{
    event_loop::ActiveEventLoop,
    keyboard::PhysicalKey,
    window::{Window, WindowAttributes},
};

#[link(name = "user32")]
unsafe extern "system" {
    fn SetTimer(
        hwnd: *mut c_void,
        id: usize,
        interval: u32,
        callback: Option<unsafe extern "system" fn(*mut c_void, u32, usize, u32)>,
    ) -> usize;
    fn KillTimer(hwnd: *mut c_void, id: usize) -> i32;
    fn PostMessageW(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize) -> i32;
    fn GetKeyState(key: i32) -> i16;
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
}

pub(crate) struct WindowsPlatform;

const WAKE_TIMER: usize = 0x46525357;

unsafe extern "system" fn wake_timer(hwnd: *mut c_void, _: u32, id: usize, _: u32) {
    // A one-shot HWND timer is dispatched inside Win32's modal sizing loop.
    // Queue a paint event instead of entering Flutter from the native callback.
    unsafe {
        KillTimer(hwnd, id);
        PostMessageW(hwnd, 0x000f, 0, 0);
    }
}

impl WindowsPlatform {
    pub(crate) fn arm_wake(window: &dyn Window, deadline: Option<Instant>) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = handle.hwnd.get() as *mut c_void;
        // SAFETY: winit owns this live HWND on the calling thread. Windows
        // removes its timers on destruction, so no callback retains Rust data.
        unsafe {
            KillTimer(hwnd, WAKE_TIMER);
            if let Some(deadline) = deadline {
                let milliseconds = deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .saturating_add(1)
                    .min(u32::MAX as u128) as u32;
                if SetTimer(hwnd, WAKE_TIMER, milliseconds, Some(wake_timer)) == 0 {
                    log::error!("Failed to arm Win32 Flutter task deadline timer");
                }
            }
        }
    }
}

// Values mirror RawKeyEventDataWindows and keyboard_key_channel_handler.cc.
fn modifiers() -> u32 {
    let mut result = 0;
    for (key, bit) in [
        (0x10, 0),
        (0xa0, 1),
        (0xa1, 2),
        (0x11, 3),
        (0xa2, 4),
        (0xa3, 5),
        (0x12, 6),
        (0xa4, 7),
        (0xa5, 8),
        (0x5b, 9),
        (0x5c, 10),
    ] {
        // SAFETY: GetKeyState accepts any virtual key code and has no pointers.
        if unsafe { GetKeyState(key) } < 0 {
            result |= 1 << bit;
        }
    }
    for (key, bit) in [(0x14, 11), (0x90, 12), (0x91, 13)] {
        if unsafe { GetKeyState(key) } & 1 != 0 {
            result |= 1 << bit;
        }
    }
    result
}

fn encode_key(
    event: &FlutterRustKeyEvent,
    scan_code: u32,
    key_code: u32,
    modifiers: u32,
) -> Vec<u8> {
    let character = std::str::from_utf8(&event.character[..event.character_length as usize])
        .ok()
        .and_then(|text| text.chars().next())
        .map(u32::from)
        .unwrap_or(0);
    serde_json::to_vec(&serde_json::json!({
        "type": if event.event_type == FlutterRustKeyEventType::Up as u32 { "keyup" } else { "keydown" },
        "keymap": "windows", "keyCode": key_code, "scanCode": scan_code,
        "characterCodePoint": character, "modifiers": modifiers,
    })).expect("Windows key message serializes")
}

impl PlatformBackend for WindowsPlatform {
    const LEGACY_KEYMAP: &'static str = "windows";
    const LEGACY_TOOLKIT: &'static str = "windows";
    const FALLBACK_KEY_PLANE: u64 = 0x01600000000;
    const INFERS_NUM_LOCK: bool = false;

    fn text_modifiers(
        mut modifiers: winit::keyboard::ModifiersState,
        right_alt: bool,
    ) -> winit::keyboard::ModifiersState {
        // Win32 reports AltGr as Ctrl+RightAlt. Its produced Unicode text is
        // input, while Ctrl+LeftAlt remains a shortcut chord.
        if right_alt {
            modifiers.remove(
                winit::keyboard::ModifiersState::CONTROL | winit::keyboard::ModifiersState::ALT,
            );
        }
        modifiers
    }

    fn lock_state() -> Option<[bool; 3]> {
        // Unit tests inject lock state instead of reading the developer's keyboard.
        if cfg!(test) {
            return None;
        }
        Some([0x14, 0x90, 0x91].map(|key| unsafe { GetKeyState(key) } & 1 != 0))
    }

    fn supports_window_kind(kind: NativeWindowKind) -> bool {
        matches!(kind, NativeWindowKind::Regular)
    }

    fn legacy_key_message(physical: PhysicalKey, event: &FlutterRustKeyEvent) -> Option<Vec<u8>> {
        let scan = winit::platform::windows::physicalkey_to_scancode(physical).unwrap_or(0);
        // Winit uses 0xe0xx for extended scans; Flutter uses 0x1xx.
        let flutter_scan = if scan & 0xff00 == 0xe000 {
            (scan & 0xff) | 0x100
        } else {
            scan
        };
        let virtual_key = unsafe { MapVirtualKeyW(scan, 3) };
        Some(encode_key(event, flutter_scan, virtual_key, modifiers()))
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
        Err("native popups are not implemented on Windows".to_owned())
    }

    fn set_dialog_parent(_child: &dyn Window, _parent: &dyn Window) -> Result<(), String> {
        Err("native dialog parenting is not implemented on Windows".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_legacy_schema_uses_native_codes_and_modifier_bits() {
        let mut event = FlutterRustKeyEvent {
            timestamp_micros: 0,
            event_type: 0,
            physical: 0,
            logical: 0,
            synthesized: 0,
            character_length: 0,
            character: [0; flutter_shell_core::FLUTTER_RUST_KEY_CHARACTER_CAPACITY],
        };
        event.event_type = FlutterRustKeyEventType::Down as u32;
        event.character[0] = b'A';
        event.character_length = 1;
        let value: serde_json::Value =
            serde_json::from_slice(&encode_key(&event, 0x1e, 0x41, 3)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type":"keydown", "keymap":"windows",
            "keyCode":65, "scanCode":30, "characterCodePoint":65, "modifiers":3})
        );
        event.event_type = FlutterRustKeyEventType::Up as u32;
        let value: serde_json::Value =
            serde_json::from_slice(&encode_key(&event, 0x11d, 0xa3, 0)).unwrap();
        assert_eq!(value["type"], "keyup");
        assert_eq!(value["scanCode"], 0x11d);
    }
}
