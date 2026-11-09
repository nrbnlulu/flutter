//! Windows host: owned top-level windows, timer pacing, and Windows key JSON.

use super::super::{NativeWindowKind, PopupPlacement};
use super::PlatformBackend;
use flutter_shell_core::{FlutterRustKeyEvent, FlutterRustKeyEventType};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;
use std::time::Instant;
use winit::{
    event_loop::ActiveEventLoop,
    keyboard::PhysicalKey,
    platform::windows::WindowAttributesWindows,
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
    fn SetWindowPos(
        hwnd: *mut c_void,
        after: *mut c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn SetWindowLongPtrW(hwnd: *mut c_void, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
    fn GetKeyState(key: i32) -> i16;
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
    fn MonitorFromWindow(hwnd: *mut c_void, flags: u32) -> *mut c_void;
    fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfo) -> i32;
    fn ClientToScreen(hwnd: *mut c_void, point: *mut Point) -> i32;

}

pub(crate) struct WindowsPlatform;
#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
#[derive(Default)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetLastError(error: u32);
    fn GetLastError() -> u32;
}

fn hwnd(window: &dyn Window) -> Result<*mut c_void, String> {
    let handle = window.window_handle().map_err(|error| error.to_string())?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as *mut c_void),
        _ => Err("Expected a Win32 window handle".into()),
    }
}

fn set_window_long(hwnd: *mut c_void, index: i32, value: isize) -> Result<(), String> {
    // A zero return can be a successful change from zero, so clear/check the
    // thread-local error as required by SetWindowLongPtrW's contract.
    unsafe {
        SetLastError(0);
        if SetWindowLongPtrW(hwnd, index, value) == 0 && GetLastError() != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

fn popup_geometry(
    parent: &dyn Window,
    placement: PopupPlacement,
) -> Result<(i32, i32, f64, f64), String> {
    let owner = hwnd(parent)?;
    let mut origin = Point::default();
    let mut monitor = MonitorInfo {
        size: std::mem::size_of::<MonitorInfo>() as u32,
        ..Default::default()
    };
    unsafe {
        if ClientToScreen(owner, &mut origin) == 0
            || GetMonitorInfoW(MonitorFromWindow(owner, 2), &mut monitor) == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    let scale = parent.scale_factor();
    let rect = &monitor.work;
    let (x, y, width, height) = super::positioner::place(
        placement,
        (f64::from(origin.x) / scale, f64::from(origin.y) / scale),
        (
            f64::from(rect.left) / scale,
            f64::from(rect.top) / scale,
            f64::from(rect.right - rect.left) / scale,
            f64::from(rect.bottom - rect.top) / scale,
        ),
    );
    Ok((
        (x * scale).round() as i32,
        (y * scale).round() as i32,
        width,
        height,
    ))
}

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
        matches!(
            kind,
            NativeWindowKind::Regular
                | NativeWindowKind::Dialog
                | NativeWindowKind::Satellite
                | NativeWindowKind::Tooltip
        )
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
        kind: NativeWindowKind,
    ) -> WindowAttributes {
        if kind == NativeWindowKind::Satellite {
            attributes.with_platform_attributes(Box::new(
                WindowAttributesWindows::default().with_skip_taskbar(true),
            ))
        } else {
            attributes
        }
    }

    fn configure_native_window(window: &dyn Window, kind: NativeWindowKind) -> Result<(), String> {
        if kind == NativeWindowKind::Satellite {
            let hwnd = hwnd(window)?;
            // Owned utility windows use the tool-window caption and do not
            // acquire an independent taskbar/Alt-Tab entry.
            let style = unsafe { GetWindowLongPtrW(hwnd, -20) };
            set_window_long(hwnd, -20, (style | 0x80) & !0x40000)?;
            if unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, 0x37) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
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
        grab: bool,
    ) -> Result<Box<dyn Window>, String> {
        if grab {
            return Err("grabbing popups are not implemented on Windows".into());
        }
        let owner = hwnd(parent)?;
        let (x, y, width, height) = popup_geometry(parent, placement)?;
        let window = event_loop
            .create_window(
                WindowAttributes::default()
                    .with_visible(false)
                    .with_active(false)
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_position(winit::dpi::PhysicalPosition::new(x, y))
                    .with_surface_size(winit::dpi::LogicalSize::new(width, height))
                    .with_platform_attributes(Box::new(
                        WindowAttributesWindows::default()
                            .with_owner_window(owner)
                            .with_skip_taskbar(true),
                    )),
            )
            .map_err(|error| error.to_string())?;
        // Winit reapplies its extended styles when showing the window. Show
        // without activation first, then install the permanent NOACTIVATE bit.
        window.set_visible(true);
        let handle = hwnd(window.as_ref())?;
        let style = unsafe { GetWindowLongPtrW(handle, -16) };
        set_window_long(handle, -16, (style & !0x00cf0000) | 0x80000000u32 as isize)?;
        let extended = unsafe { GetWindowLongPtrW(handle, -20) };
        set_window_long(handle, -20, (extended | 0x08000080) & !0x40000)?;
        if unsafe { SetWindowPos(handle, std::ptr::null_mut(), 0, 0, 0, 0, 0x37) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(window)
    }

    fn reposition_popup(
        window: &dyn Window,
        parent: &dyn Window,
        placement: PopupPlacement,
    ) -> Result<(), String> {
        let (x, y, width, height) = popup_geometry(parent, placement)?;
        let width = (width * parent.scale_factor()).round() as u32;
        let height = (height * parent.scale_factor()).round() as u32;
        let position = window.outer_position().map_err(|error| error.to_string())?;
        let size = window.surface_size();
        // Native moves/resizes emit more winit events. Avoid a feedback loop
        // when a reconfiguration already produced the desired geometry.
        if position.x != x || position.y != y || size.width != width || size.height != height {
            if unsafe {
                SetWindowPos(
                    hwnd(window)?,
                    std::ptr::null_mut(),
                    x,
                    y,
                    width as i32,
                    height as i32,
                    0x14,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        Ok(())
    }

    fn set_dialog_parent(child: &dyn Window, parent: &dyn Window) -> Result<(), String> {
        // GWLP_HWNDPARENT sets the owner of a top-level window. SetParent
        // would instead turn it into a child with a different coordinate model.
        set_window_long(hwnd(child)?, -8, hwnd(parent)? as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_dialog_owner_and_satellite_reparenting_are_top_level() {
        use winit::application::ApplicationHandler;
        use winit::event::WindowEvent;
        use winit::event_loop::EventLoop;
        use winit::platform::windows::EventLoopBuilderExtWindows;
        use winit::window::WindowId;
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetWindow(hwnd: *mut c_void, command: u32) -> *mut c_void;
            fn ShowWindow(hwnd: *mut c_void, command: i32) -> i32;
            fn IsWindowVisible(hwnd: *mut c_void) -> i32;
            fn GetForegroundWindow() -> *mut c_void;
        }
        struct Probe;
        impl ApplicationHandler for Probe {
            fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
                let parent = event_loop
                    .create_window(WindowAttributes::default().with_visible(false))
                    .unwrap();
                let other = event_loop
                    .create_window(WindowAttributes::default().with_visible(false))
                    .unwrap();
                for kind in [NativeWindowKind::Dialog, NativeWindowKind::Satellite] {
                    let child = event_loop
                        .create_window(WindowsPlatform::prepare_window_attributes(
                            WindowAttributes::default().with_visible(false),
                            kind,
                        ))
                        .unwrap();
                    WindowsPlatform::configure_native_window(child.as_ref(), kind).unwrap();
                    WindowsPlatform::set_dialog_parent(child.as_ref(), parent.as_ref()).unwrap();
                    let child_handle = hwnd(child.as_ref()).unwrap();
                    assert_eq!(
                        unsafe { GetWindow(child_handle, 4) },
                        hwnd(parent.as_ref()).unwrap()
                    );
                    // WS_CHILD must remain clear: these are owned top-levels,
                    // not embedded child controls.
                    assert_eq!(
                        unsafe { GetWindowLongPtrW(child_handle, -16) } & 0x40000000,
                        0
                    );
                    let parent_handle = hwnd(parent.as_ref()).unwrap();
                    unsafe {
                        ShowWindow(parent_handle, 9);
                        ShowWindow(child_handle, 4);
                    }
                    assert_ne!(unsafe { IsWindowVisible(child_handle) }, 0);
                    unsafe {
                        ShowWindow(parent_handle, 6);
                    }
                    assert_eq!(unsafe { IsWindowVisible(child_handle) }, 0);
                    unsafe {
                        ShowWindow(parent_handle, 9);
                    }
                    assert_ne!(unsafe { IsWindowVisible(child_handle) }, 0);
                    if kind == NativeWindowKind::Satellite {
                        assert_ne!(unsafe { GetWindowLongPtrW(child_handle, -20) } & 0x80, 0);
                        let position = child.outer_position().unwrap();
                        WindowsPlatform::set_dialog_parent(child.as_ref(), other.as_ref()).unwrap();
                        assert_eq!(
                            unsafe { GetWindow(child_handle, 4) },
                            hwnd(other.as_ref()).unwrap()
                        );
                        assert_eq!(child.outer_position().unwrap(), position);
                    }
                }
                let foreground = unsafe { GetForegroundWindow() };
                let tooltip = WindowsPlatform::create_popup(
                    event_loop,
                    parent.as_ref(),
                    PopupPlacement {
                        width: 80,
                        height: 60,
                        anchor_rect: (10, 10, 20, 20),
                        anchor: super::super::super::PopupAnchor::BottomRight,
                        gravity: super::super::super::PopupAnchor::BottomRight,
                        offset: (0, 0),
                        constraint_adjustment: 0x3f,
                    },
                    false,
                )
                .unwrap();
                let tooltip_handle = hwnd(tooltip.as_ref()).unwrap();
                assert_eq!(
                    unsafe { GetWindow(tooltip_handle, 4) },
                    hwnd(parent.as_ref()).unwrap()
                );
                assert_eq!(unsafe { GetForegroundWindow() }, foreground);
                assert_ne!(
                    unsafe { GetWindowLongPtrW(tooltip_handle, -16) } & 0x80000000u32 as isize,
                    0
                );
                assert_ne!(
                    unsafe { GetWindowLongPtrW(tooltip_handle, -20) } & 0x08000000,
                    0
                );
                drop(tooltip);
                event_loop.exit();
            }
            fn window_event(&mut self, _: &dyn ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(Probe).unwrap();
    }

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
