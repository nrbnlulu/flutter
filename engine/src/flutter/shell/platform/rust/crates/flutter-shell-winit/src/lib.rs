//! Winit-owned native host for the optional Flutter Rust shell.
//!
//! The host keeps Flutter UI/platform task batons in a monotonic task queue.
//! The private C++ bridge will install the callback that executes a due baton.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        collections::BTreeMap,
        ffi::c_void,
        sync::Arc,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread::ThreadId,
        time::{Duration, Instant},
    };

    use flutter_shell_core::{
        FLUTTER_RUST_KEY_CHARACTER_CAPACITY, FlutterRustKeyEvent, FlutterRustKeyEventType,
        FlutterRustLifecycleState, FlutterRustPointerDeviceKind, FlutterRustPointerEvent,
        FlutterRustPointerPhase, FlutterRustPointerSignalKind, FlutterRustTaskRunnerCallbacks,
    };
    #[cfg(not(test))]
    use flutter_shell_core::{FlutterRustShellSettings, FlutterRustVulkanContextData};
    use flutter_shell_wgpu::GpuBroker;
    #[cfg(not(test))]
    use std::ffi::CString;
    use winit::{
        application::ApplicationHandler,
        event::{
            ElementState, KeyEvent as WinitKeyEvent, MouseButton, MouseScrollDelta, Touch,
            TouchPhase, WindowEvent,
        },
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
        keyboard::{Key, KeyCode, NamedKey, NativeKey, NativeKeyCode, PhysicalKey},
        window::{Window, WindowId},
    };

    #[derive(Debug, Clone, Copy)]
    enum HostEvent {
        TaskScheduled,
    }

    #[cfg(not(test))]
    fn create_cpp_task_runner(callbacks: FlutterRustTaskRunnerCallbacks) -> *mut c_void {
        unsafe extern "C" {
            fn FlutterRustShellCreateTaskRunner(
                callbacks: FlutterRustTaskRunnerCallbacks,
            ) -> *mut c_void;
        }
        // SAFETY: callbacks is ABI-compatible with rust_bridge.h.
        unsafe { FlutterRustShellCreateTaskRunner(callbacks) }
    }

    #[cfg(test)]
    fn create_cpp_task_runner(_: FlutterRustTaskRunnerCallbacks) -> *mut c_void {
        std::ptr::dangling_mut()
    }

    #[cfg(not(test))]
    fn run_cpp_task(task_runner: *mut c_void, task_baton: u64) -> i32 {
        unsafe extern "C" {
            fn FlutterRustShellRunTask(task_runner: *mut c_void, task_baton: u64) -> i32;
        }
        // SAFETY: the Rust host owns this opaque C++ runner handle.
        unsafe { FlutterRustShellRunTask(task_runner, task_baton) }
    }

    #[cfg(test)]
    fn run_cpp_task(_: *mut c_void, _: u64) -> i32 {
        0
    }

    #[cfg(not(test))]
    fn destroy_cpp_task_runner(task_runner: *mut c_void) {
        unsafe extern "C" {
            fn FlutterRustShellDestroyTaskRunner(task_runner: *mut c_void);
        }
        // SAFETY: the Rust host owns this opaque C++ runner handle.
        unsafe { FlutterRustShellDestroyTaskRunner(task_runner) }
    }

    #[cfg(test)]
    fn destroy_cpp_task_runner(_: *mut c_void) {}

    #[cfg(not(test))]
    fn create_cpp_shell(
        task_runner: *mut c_void,
        context_data: FlutterRustVulkanContextData,
        presentation_callbacks: flutter_shell_core::FlutterRustVulkanPresentationCallbacks,
        settings: FlutterRustShellSettings,
    ) -> *mut c_void {
        unsafe extern "C" {
            fn FlutterRustShellCreateShell(
                task_runner: *mut c_void,
                context_data: FlutterRustVulkanContextData,
                presentation_callbacks: flutter_shell_core::FlutterRustVulkanPresentationCallbacks,
                settings: FlutterRustShellSettings,
            ) -> *mut c_void;
        }
        // SAFETY: the struct layouts are ABI-compatible with rust_bridge.h.
        unsafe {
            FlutterRustShellCreateShell(task_runner, context_data, presentation_callbacks, settings)
        }
    }

    #[cfg(not(test))]
    fn run_cpp_shell(shell: *mut c_void) -> i32 {
        unsafe extern "C" {
            fn FlutterRustShellRunShell(shell: *mut c_void) -> i32;
        }
        // SAFETY: `shell` was returned by create_cpp_shell and not yet destroyed.
        unsafe { FlutterRustShellRunShell(shell) }
    }

    #[cfg(not(test))]
    fn destroy_cpp_shell(shell: *mut c_void) {
        unsafe extern "C" {
            fn FlutterRustShellDestroyShell(shell: *mut c_void);
        }
        // SAFETY: `shell` was returned by create_cpp_shell.
        unsafe { FlutterRustShellDestroyShell(shell) }
    }

    #[cfg(not(test))]
    fn set_cpp_shell_viewport_metrics(shell: *mut c_void, metrics: WindowMetrics) {
        unsafe extern "C" {
            fn FlutterRustShellSetViewportMetrics(
                shell: *mut c_void,
                width: f64,
                height: f64,
                pixel_ratio: f64,
                display_width: f64,
                display_height: f64,
                display_refresh_rate: f64,
            );
        }
        // SAFETY: `shell` was returned by create_cpp_shell and not yet destroyed.
        unsafe {
            FlutterRustShellSetViewportMetrics(
                shell,
                metrics.width as f64,
                metrics.height as f64,
                metrics.pixel_ratio,
                metrics.display_width as f64,
                metrics.display_height as f64,
                metrics.display_refresh_rate,
            );
        }
    }

    #[cfg(not(test))]
    fn send_cpp_pointer_event(shell: *mut c_void, event: FlutterRustPointerEvent) {
        unsafe extern "C" {
            fn FlutterRustShellSendPointerEvent(shell: *mut c_void, event: FlutterRustPointerEvent);
        }
        // SAFETY: `shell` was returned by create_cpp_shell and the event is an
        // ABI-compatible value with no borrowed fields.
        unsafe { FlutterRustShellSendPointerEvent(shell, event) }
    }

    #[cfg(not(test))]
    fn send_cpp_lifecycle_event(shell: *mut c_void, state: FlutterRustLifecycleState) {
        unsafe extern "C" {
            fn FlutterRustShellSendLifecycleEvent(shell: *mut c_void, state: u32);
        }
        // SAFETY: `shell` was returned by create_cpp_shell and the state is a
        // value from the private ABI enum.
        unsafe { FlutterRustShellSendLifecycleEvent(shell, state as u32) }
    }

    #[cfg(not(test))]
    fn send_cpp_key_event(shell: *mut c_void, event: FlutterRustKeyEvent) {
        unsafe extern "C" {
            fn FlutterRustShellSendKeyEvent(shell: *mut c_void, event: FlutterRustKeyEvent);
        }
        // SAFETY: `shell` was returned by create_cpp_shell and the event is an
        // ABI-compatible value containing no borrowed fields.
        unsafe { FlutterRustShellSendKeyEvent(shell, event) }
    }

    /// Winit host configuration, shared across the platforms this crate will
    /// eventually support.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ShellConfig {
        pub title: String,
        pub assets_path: String,
        pub icu_data_path: String,
    }

    impl Default for ShellConfig {
        fn default() -> Self {
            Self {
                title: "Flutter Rust Shell".to_owned(),
                assets_path: String::new(),
                icu_data_path: String::new(),
            }
        }
    }

    /// An opaque Flutter task scheduled on the Rust host loop.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ScheduledTask {
        pub task_runner: usize,
        pub task_baton: u64,
    }

    /// A monotonic queue for opaque Flutter task batons.
    ///
    /// C++ converts its `fml::TimePoint` to an `Instant` deadline before a
    /// baton enters this queue. The task itself remains owned by C++ and is
    /// executed only after the host loop returns its baton through the private
    /// bridge.
    #[derive(Debug, Default)]
    pub struct TaskQueue {
        tasks: BTreeMap<Instant, Vec<ScheduledTask>>,
    }

    impl TaskQueue {
        pub fn schedule(&mut self, task: ScheduledTask, deadline: Instant) {
            self.tasks.entry(deadline).or_default().push(task);
        }

        pub fn next_deadline(&self) -> Option<Instant> {
            self.tasks.first_key_value().map(|(deadline, _)| *deadline)
        }

        pub fn take_due(&mut self, now: Instant) -> Vec<ScheduledTask> {
            let tasks = std::mem::take(&mut self.tasks);
            let mut due = Vec::new();
            for (deadline, task_batons) in tasks {
                if deadline <= now {
                    due.extend(task_batons);
                } else {
                    self.tasks.insert(deadline, task_batons);
                }
            }
            due
        }
    }

    const MOUSE_DEVICE_ID: i64 = 0;
    const MOUSE_PRIMARY_BUTTON: i64 = 1 << 0;
    const MOUSE_SECONDARY_BUTTON: i64 = 1 << 1;
    const MOUSE_MIDDLE_BUTTON: i64 = 1 << 2;
    const MOUSE_BACK_BUTTON: i64 = 1 << 3;
    const MOUSE_FORWARD_BUTTON: i64 = 1 << 4;
    const SCROLL_LINE_PIXELS: f64 = 53.0;

    #[cfg(not(test))]
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct WindowMetrics {
        width: u32,
        height: u32,
        pixel_ratio: f64,
        display_width: u32,
        display_height: u32,
        display_refresh_rate: f64,
    }

    #[cfg(not(test))]
    impl WindowMetrics {
        fn from_window(window: &Window, pixel_ratio: f64) -> Self {
            let size = window.inner_size();
            let (display_width, display_height, display_refresh_rate) = window
                .current_monitor()
                .map(|monitor| {
                    let size = monitor.size();
                    let refresh_rate = monitor
                        .refresh_rate_millihertz()
                        .map_or(0.0, |rate| f64::from(rate) / 1000.0);
                    (size.width, size.height, refresh_rate)
                })
                .unwrap_or((size.width, size.height, 0.0));
            Self {
                width: size.width,
                height: size.height,
                pixel_ratio,
                display_width,
                display_height,
                display_refresh_rate,
            }
        }
    }

    #[derive(Debug)]
    struct LifecycleState {
        active: bool,
        visible: bool,
        focused: bool,
        last_sent: Option<FlutterRustLifecycleState>,
    }

    impl LifecycleState {
        fn new() -> Self {
            Self {
                active: false,
                visible: false,
                focused: false,
                last_sent: None,
            }
        }

        fn resumed(&mut self, visible: bool, focused: bool) -> Option<FlutterRustLifecycleState> {
            self.active = true;
            self.visible = visible;
            self.focused = focused;
            self.changed()
        }

        fn suspended(&mut self) -> Option<FlutterRustLifecycleState> {
            self.active = false;
            self.changed()
        }

        fn visibility_changed(&mut self, visible: bool) -> Option<FlutterRustLifecycleState> {
            self.visible = visible;
            self.changed()
        }

        fn focus_changed(&mut self, focused: bool) -> Option<FlutterRustLifecycleState> {
            self.focused = focused;
            self.changed()
        }

        fn detached(&mut self) -> Option<FlutterRustLifecycleState> {
            self.emit(FlutterRustLifecycleState::Detached)
        }

        fn changed(&mut self) -> Option<FlutterRustLifecycleState> {
            let state = if !self.active {
                FlutterRustLifecycleState::Paused
            } else if !self.visible {
                FlutterRustLifecycleState::Hidden
            } else if self.focused {
                FlutterRustLifecycleState::Resumed
            } else {
                FlutterRustLifecycleState::Inactive
            };
            self.emit(state)
        }

        fn emit(&mut self, state: FlutterRustLifecycleState) -> Option<FlutterRustLifecycleState> {
            if self.last_sent == Some(state) {
                return None;
            }
            self.last_sent = Some(state);
            Some(state)
        }
    }

    struct PointerState {
        started_at: Instant,
        physical_x: f64,
        physical_y: f64,
        buttons: i64,
        inside: bool,
        pointer_outside: bool,
    }

    impl PointerState {
        fn new() -> Self {
            Self {
                started_at: Instant::now(),
                physical_x: 0.0,
                physical_y: 0.0,
                buttons: 0,
                inside: false,
                pointer_outside: true,
            }
        }

        fn mouse_event(
            &self,
            phase: FlutterRustPointerPhase,
            signal_kind: FlutterRustPointerSignalKind,
            scroll_delta_x: f64,
            scroll_delta_y: f64,
        ) -> FlutterRustPointerEvent {
            FlutterRustPointerEvent {
                timestamp_micros: self.started_at.elapsed().as_micros().min(u64::MAX as u128)
                    as u64,
                phase: phase as u32,
                device_kind: FlutterRustPointerDeviceKind::Mouse as u32,
                signal_kind: signal_kind as u32,
                device: MOUSE_DEVICE_ID,
                physical_x: self.physical_x,
                physical_y: self.physical_y,
                scroll_delta_x,
                scroll_delta_y,
                buttons: self.buttons,
            }
        }

        fn entered(&mut self) -> Option<FlutterRustPointerEvent> {
            self.pointer_outside = false;
            self.ensure_added()
        }

        fn ensure_added(&mut self) -> Option<FlutterRustPointerEvent> {
            if self.inside {
                return None;
            }
            self.inside = true;
            Some(self.mouse_event(
                FlutterRustPointerPhase::Add,
                FlutterRustPointerSignalKind::None,
                0.0,
                0.0,
            ))
        }

        fn left(&mut self) -> Option<FlutterRustPointerEvent> {
            self.pointer_outside = true;
            // Keep the mouse added while a drag is captured outside the
            // window. Releasing the last button emits Up followed by Remove.
            if !self.inside || self.buttons != 0 {
                return None;
            }
            self.inside = false;
            Some(self.mouse_event(
                FlutterRustPointerPhase::Remove,
                FlutterRustPointerSignalKind::None,
                0.0,
                0.0,
            ))
        }

        fn moved(&mut self, physical_x: f64, physical_y: f64) -> Vec<FlutterRustPointerEvent> {
            self.physical_x = physical_x;
            self.physical_y = physical_y;
            self.pointer_outside = false;
            let mut events = self.entered().into_iter().collect::<Vec<_>>();
            events.push(self.mouse_event(
                if self.buttons == 0 {
                    FlutterRustPointerPhase::Hover
                } else {
                    FlutterRustPointerPhase::Move
                },
                FlutterRustPointerSignalKind::None,
                0.0,
                0.0,
            ));
            events
        }

        fn button(
            &mut self,
            button: MouseButton,
            state: ElementState,
        ) -> Vec<FlutterRustPointerEvent> {
            let Some(mask) = mouse_button_mask(button) else {
                return Vec::new();
            };
            if state == ElementState::Pressed {
                self.pointer_outside = false;
            }
            let mut events = self.ensure_added().into_iter().collect::<Vec<_>>();
            let phase = match state {
                ElementState::Pressed => {
                    if self.buttons & mask != 0 {
                        return events;
                    }
                    let was_up = self.buttons == 0;
                    self.buttons |= mask;
                    if was_up {
                        FlutterRustPointerPhase::Down
                    } else {
                        FlutterRustPointerPhase::Move
                    }
                }
                ElementState::Released => {
                    if self.buttons & mask == 0 {
                        return events;
                    }
                    self.buttons &= !mask;
                    if self.buttons == 0 {
                        FlutterRustPointerPhase::Up
                    } else {
                        FlutterRustPointerPhase::Move
                    }
                }
            };
            events.push(self.mouse_event(phase, FlutterRustPointerSignalKind::None, 0.0, 0.0));
            if self.buttons == 0 && self.pointer_outside {
                if let Some(event) = self.left() {
                    events.push(event);
                }
            }
            events
        }

        fn scroll(&mut self, delta: MouseScrollDelta) -> Vec<FlutterRustPointerEvent> {
            self.pointer_outside = false;
            let (scroll_delta_x, scroll_delta_y) = match delta {
                MouseScrollDelta::LineDelta(x, y) => (
                    f64::from(x) * SCROLL_LINE_PIXELS,
                    -f64::from(y) * SCROLL_LINE_PIXELS,
                ),
                MouseScrollDelta::PixelDelta(position) => (position.x, -position.y),
            };
            let mut events = self.entered().into_iter().collect::<Vec<_>>();
            events.push(self.mouse_event(
                if self.buttons == 0 {
                    FlutterRustPointerPhase::Hover
                } else {
                    FlutterRustPointerPhase::Move
                },
                FlutterRustPointerSignalKind::Scroll,
                scroll_delta_x,
                scroll_delta_y,
            ));
            events
        }

        fn touch(&self, touch: Touch) -> FlutterRustPointerEvent {
            let (phase, buttons) = match touch.phase {
                TouchPhase::Started => (FlutterRustPointerPhase::Down, 1),
                TouchPhase::Moved => (FlutterRustPointerPhase::Move, 1),
                TouchPhase::Ended => (FlutterRustPointerPhase::Up, 0),
                TouchPhase::Cancelled => (FlutterRustPointerPhase::Cancel, 0),
            };
            FlutterRustPointerEvent {
                timestamp_micros: self.started_at.elapsed().as_micros().min(u64::MAX as u128)
                    as u64,
                phase: phase as u32,
                device_kind: FlutterRustPointerDeviceKind::Touch as u32,
                signal_kind: FlutterRustPointerSignalKind::None as u32,
                // Reserve device 0 for the mouse. Winit touch IDs commonly
                // begin at zero, while Flutter keys pointer state by device.
                device: touch_device_id(touch.id),
                physical_x: touch.location.x,
                physical_y: touch.location.y,
                scroll_delta_x: 0.0,
                scroll_delta_y: 0.0,
                buttons,
            }
        }
    }

    const GTK_KEY_PLANE: u64 = 0x01500000000;
    const KEY_VALUE_MASK: u64 = 0x000ffffffff;

    struct KeyboardState {
        started_at: Instant,
    }

    impl KeyboardState {
        fn new() -> Self {
            Self {
                started_at: Instant::now(),
            }
        }

        fn event(&self, event: &WinitKeyEvent, synthesized: bool) -> Option<FlutterRustKeyEvent> {
            make_key_event(
                self.started_at.elapsed().as_micros().min(u64::MAX as u128) as u64,
                event.physical_key,
                &event.logical_key,
                event.text.as_deref(),
                event.state,
                event.repeat,
                synthesized,
            )
        }
    }

    fn make_key_event(
        timestamp_micros: u64,
        physical_key: PhysicalKey,
        logical_key: &Key,
        text: Option<&str>,
        state: ElementState,
        repeat: bool,
        synthesized: bool,
    ) -> Option<FlutterRustKeyEvent> {
        let physical = physical_key_id(physical_key)?;
        let logical = logical_key_id(logical_key, physical_key, physical);
        let event_type = match (state, repeat) {
            (ElementState::Released, _) => FlutterRustKeyEventType::Up,
            (ElementState::Pressed, true) => FlutterRustKeyEventType::Repeat,
            (ElementState::Pressed, false) => FlutterRustKeyEventType::Down,
        };
        let mut character = [0; FLUTTER_RUST_KEY_CHARACTER_CAPACITY];
        let character_length = if state == ElementState::Pressed {
            text.filter(|value| {
                value.len() <= FLUTTER_RUST_KEY_CHARACTER_CAPACITY && !value.as_bytes().contains(&0)
            })
            .map_or(0, |value| {
                character[..value.len()].copy_from_slice(value.as_bytes());
                value.len() as u32
            })
        } else {
            0
        };
        Some(FlutterRustKeyEvent {
            timestamp_micros,
            event_type: event_type as u32,
            physical,
            logical,
            synthesized: i32::from(synthesized),
            character_length,
            character,
        })
    }

    fn physical_key_id(key: PhysicalKey) -> Option<u64> {
        let usage = match key {
            PhysicalKey::Code(code) => key_code_usb_usage(code)?,
            PhysicalKey::Unidentified(NativeKeyCode::Xkb(code)) => {
                return Some(GTK_KEY_PLANE | u64::from(code));
            }
            PhysicalKey::Unidentified(_) => return None,
        };
        Some(0x00070000 | u64::from(usage))
    }

    fn key_code_usb_usage(code: KeyCode) -> Option<u16> {
        Some(match code {
            KeyCode::KeyA => 0x04,
            KeyCode::KeyB => 0x05,
            KeyCode::KeyC => 0x06,
            KeyCode::KeyD => 0x07,
            KeyCode::KeyE => 0x08,
            KeyCode::KeyF => 0x09,
            KeyCode::KeyG => 0x0a,
            KeyCode::KeyH => 0x0b,
            KeyCode::KeyI => 0x0c,
            KeyCode::KeyJ => 0x0d,
            KeyCode::KeyK => 0x0e,
            KeyCode::KeyL => 0x0f,
            KeyCode::KeyM => 0x10,
            KeyCode::KeyN => 0x11,
            KeyCode::KeyO => 0x12,
            KeyCode::KeyP => 0x13,
            KeyCode::KeyQ => 0x14,
            KeyCode::KeyR => 0x15,
            KeyCode::KeyS => 0x16,
            KeyCode::KeyT => 0x17,
            KeyCode::KeyU => 0x18,
            KeyCode::KeyV => 0x19,
            KeyCode::KeyW => 0x1a,
            KeyCode::KeyX => 0x1b,
            KeyCode::KeyY => 0x1c,
            KeyCode::KeyZ => 0x1d,
            KeyCode::Digit1 => 0x1e,
            KeyCode::Digit2 => 0x1f,
            KeyCode::Digit3 => 0x20,
            KeyCode::Digit4 => 0x21,
            KeyCode::Digit5 => 0x22,
            KeyCode::Digit6 => 0x23,
            KeyCode::Digit7 => 0x24,
            KeyCode::Digit8 => 0x25,
            KeyCode::Digit9 => 0x26,
            KeyCode::Digit0 => 0x27,
            KeyCode::Enter => 0x28,
            KeyCode::Escape => 0x29,
            KeyCode::Backspace => 0x2a,
            KeyCode::Tab => 0x2b,
            KeyCode::Space => 0x2c,
            KeyCode::Minus => 0x2d,
            KeyCode::Equal => 0x2e,
            KeyCode::BracketLeft => 0x2f,
            KeyCode::BracketRight => 0x30,
            KeyCode::Backslash => 0x31,
            KeyCode::Semicolon => 0x33,
            KeyCode::Quote => 0x34,
            KeyCode::Backquote => 0x35,
            KeyCode::Comma => 0x36,
            KeyCode::Period => 0x37,
            KeyCode::Slash => 0x38,
            KeyCode::CapsLock => 0x39,
            KeyCode::F1 => 0x3a,
            KeyCode::F2 => 0x3b,
            KeyCode::F3 => 0x3c,
            KeyCode::F4 => 0x3d,
            KeyCode::F5 => 0x3e,
            KeyCode::F6 => 0x3f,
            KeyCode::F7 => 0x40,
            KeyCode::F8 => 0x41,
            KeyCode::F9 => 0x42,
            KeyCode::F10 => 0x43,
            KeyCode::F11 => 0x44,
            KeyCode::F12 => 0x45,
            KeyCode::PrintScreen => 0x46,
            KeyCode::ScrollLock => 0x47,
            KeyCode::Pause => 0x48,
            KeyCode::Insert => 0x49,
            KeyCode::Home => 0x4a,
            KeyCode::PageUp => 0x4b,
            KeyCode::Delete => 0x4c,
            KeyCode::End => 0x4d,
            KeyCode::PageDown => 0x4e,
            KeyCode::ArrowRight => 0x4f,
            KeyCode::ArrowLeft => 0x50,
            KeyCode::ArrowDown => 0x51,
            KeyCode::ArrowUp => 0x52,
            KeyCode::NumLock => 0x53,
            KeyCode::NumpadDivide => 0x54,
            KeyCode::NumpadMultiply => 0x55,
            KeyCode::NumpadSubtract => 0x56,
            KeyCode::NumpadAdd => 0x57,
            KeyCode::NumpadEnter => 0x58,
            KeyCode::Numpad1 => 0x59,
            KeyCode::Numpad2 => 0x5a,
            KeyCode::Numpad3 => 0x5b,
            KeyCode::Numpad4 => 0x5c,
            KeyCode::Numpad5 => 0x5d,
            KeyCode::Numpad6 => 0x5e,
            KeyCode::Numpad7 => 0x5f,
            KeyCode::Numpad8 => 0x60,
            KeyCode::Numpad9 => 0x61,
            KeyCode::Numpad0 => 0x62,
            KeyCode::NumpadDecimal => 0x63,
            KeyCode::IntlBackslash => 0x64,
            KeyCode::ContextMenu => 0x65,
            KeyCode::Power => 0x66,
            KeyCode::NumpadEqual => 0x67,
            KeyCode::F13 => 0x68,
            KeyCode::F14 => 0x69,
            KeyCode::F15 => 0x6a,
            KeyCode::F16 => 0x6b,
            KeyCode::F17 => 0x6c,
            KeyCode::F18 => 0x6d,
            KeyCode::F19 => 0x6e,
            KeyCode::F20 => 0x6f,
            KeyCode::F21 => 0x70,
            KeyCode::F22 => 0x71,
            KeyCode::F23 => 0x72,
            KeyCode::F24 => 0x73,
            KeyCode::AudioVolumeMute => 0x7f,
            KeyCode::AudioVolumeUp => 0x80,
            KeyCode::AudioVolumeDown => 0x81,
            KeyCode::ControlLeft => 0xe0,
            KeyCode::ShiftLeft => 0xe1,
            KeyCode::AltLeft => 0xe2,
            KeyCode::SuperLeft => 0xe3,
            KeyCode::ControlRight => 0xe4,
            KeyCode::ShiftRight => 0xe5,
            KeyCode::AltRight => 0xe6,
            KeyCode::SuperRight => 0xe7,
            _ => return None,
        })
    }

    fn logical_key_id(key: &Key, physical_key: PhysicalKey, physical: u64) -> u64 {
        if let PhysicalKey::Code(code) = physical_key {
            if let Some(numpad) = numpad_logical_key(code) {
                return numpad;
            }
        }
        match key {
            Key::Character(value) => value
                .chars()
                .next()
                .and_then(|value| value.to_lowercase().next())
                .map_or(GTK_KEY_PLANE | (physical & KEY_VALUE_MASK), |value| {
                    u64::from(value as u32)
                }),
            Key::Named(named) => named_logical_key(*named, physical_key)
                .unwrap_or(GTK_KEY_PLANE | (physical & KEY_VALUE_MASK)),
            Key::Unidentified(NativeKey::Xkb(code)) => GTK_KEY_PLANE | u64::from(*code),
            Key::Unidentified(_) | Key::Dead(_) => GTK_KEY_PLANE | (physical & KEY_VALUE_MASK),
        }
    }

    fn numpad_logical_key(code: KeyCode) -> Option<u64> {
        Some(match code {
            KeyCode::Numpad0 => 0x00200000230,
            KeyCode::Numpad1 => 0x00200000231,
            KeyCode::Numpad2 => 0x00200000232,
            KeyCode::Numpad3 => 0x00200000233,
            KeyCode::Numpad4 => 0x00200000234,
            KeyCode::Numpad5 => 0x00200000235,
            KeyCode::Numpad6 => 0x00200000236,
            KeyCode::Numpad7 => 0x00200000237,
            KeyCode::Numpad8 => 0x00200000238,
            KeyCode::Numpad9 => 0x00200000239,
            _ => return None,
        })
    }

    fn named_logical_key(named: NamedKey, physical_key: PhysicalKey) -> Option<u64> {
        Some(match named {
            NamedKey::Backspace => 0x00100000008,
            NamedKey::Tab => 0x00100000009,
            NamedKey::Enter => 0x0010000000d,
            NamedKey::Space => 0x00000000020,
            NamedKey::Escape => 0x0010000001b,
            NamedKey::Delete => 0x0010000007f,
            NamedKey::CapsLock => 0x00100000104,
            NamedKey::NumLock => 0x0010000010a,
            NamedKey::ScrollLock => 0x0010000010c,
            NamedKey::ArrowDown => 0x00100000301,
            NamedKey::ArrowLeft => 0x00100000302,
            NamedKey::ArrowRight => 0x00100000303,
            NamedKey::ArrowUp => 0x00100000304,
            NamedKey::End => 0x00100000305,
            NamedKey::Home => 0x00100000306,
            NamedKey::PageDown => 0x00100000307,
            NamedKey::PageUp => 0x00100000308,
            NamedKey::Insert => 0x00100000407,
            NamedKey::ContextMenu => 0x00100000505,
            NamedKey::Pause => 0x00100000509,
            NamedKey::PrintScreen => 0x00100000608,
            NamedKey::F1 => 0x00100000801,
            NamedKey::F2 => 0x00100000802,
            NamedKey::F3 => 0x00100000803,
            NamedKey::F4 => 0x00100000804,
            NamedKey::F5 => 0x00100000805,
            NamedKey::F6 => 0x00100000806,
            NamedKey::F7 => 0x00100000807,
            NamedKey::F8 => 0x00100000808,
            NamedKey::F9 => 0x00100000809,
            NamedKey::F10 => 0x0010000080a,
            NamedKey::F11 => 0x0010000080b,
            NamedKey::F12 => 0x0010000080c,
            NamedKey::Control => match physical_key {
                PhysicalKey::Code(KeyCode::ControlRight) => 0x00200000101,
                _ => 0x00200000100,
            },
            NamedKey::Shift => match physical_key {
                PhysicalKey::Code(KeyCode::ShiftRight) => 0x00200000103,
                _ => 0x00200000102,
            },
            NamedKey::Alt | NamedKey::AltGraph => match physical_key {
                PhysicalKey::Code(KeyCode::AltRight) => 0x00200000105,
                _ => 0x00200000104,
            },
            NamedKey::Meta | NamedKey::Super => match physical_key {
                PhysicalKey::Code(KeyCode::SuperRight) => 0x00200000107,
                _ => 0x00200000106,
            },
            _ => return None,
        })
    }

    fn mouse_button_mask(button: MouseButton) -> Option<i64> {
        match button {
            MouseButton::Left => Some(MOUSE_PRIMARY_BUTTON),
            MouseButton::Right => Some(MOUSE_SECONDARY_BUTTON),
            MouseButton::Middle => Some(MOUSE_MIDDLE_BUTTON),
            MouseButton::Back => Some(MOUSE_BACK_BUTTON),
            MouseButton::Forward => Some(MOUSE_FORWARD_BUTTON),
            MouseButton::Other(_) => None,
        }
    }

    fn touch_device_id(winit_id: u64) -> i64 {
        i64::try_from(winit_id)
            .ok()
            .and_then(|id| id.checked_add(1))
            .unwrap_or(i64::MAX)
    }

    /// Rust-owned state for a single merged Flutter UI/platform task runner.
    ///
    /// The value must have a stable address for as long as C++ retains the
    /// callback table returned by [`Self::callbacks`]. A `Box<TaskRunnerHost>`
    /// satisfies that requirement. Winit integration owns the box and wakes its
    /// loop after scheduling; that wake is added with the C++ shell bootstrap.
    pub struct TaskRunnerHost {
        queue: Mutex<TaskQueue>,
        task_runner: Mutex<Option<usize>>,
        wake_proxy: Option<EventLoopProxy<HostEvent>>,
        host_thread: ThreadId,
        destroyed: AtomicBool,
    }

    impl TaskRunnerHost {
        pub fn new() -> Self {
            Self::with_wake_proxy(None)
        }

        fn with_wake_proxy(wake_proxy: Option<EventLoopProxy<HostEvent>>) -> Self {
            Self {
                queue: Mutex::new(TaskQueue::default()),
                task_runner: Mutex::new(None),
                wake_proxy,
                host_thread: std::thread::current().id(),
                destroyed: AtomicBool::new(false),
            }
        }

        pub fn callbacks(&self) -> FlutterRustTaskRunnerCallbacks {
            FlutterRustTaskRunnerCallbacks {
                user_data: (self as *const Self).cast_mut().cast::<c_void>(),
                schedule_task: Some(schedule_task),
                runs_tasks_on_current_thread: Some(runs_tasks_on_current_thread),
                task_runner_destroyed: Some(task_runner_destroyed),
            }
        }

        pub fn take_due(&self, now: Instant) -> Vec<ScheduledTask> {
            self.queue
                .lock()
                .expect("Flutter task queue poisoned")
                .take_due(now)
        }

        pub fn is_destroyed(&self) -> bool {
            self.destroyed.load(Ordering::Acquire)
        }

        /// The opaque C++ task runner handle installed by
        /// [`Self::install_cpp_task_runner`]. Used as the merged Flutter
        /// UI/platform task runner when creating the Rust shell.
        pub fn task_runner_handle(&self) -> *mut c_void {
            self.task_runner
                .lock()
                .expect("Flutter task runner poisoned")
                .expect("the C++ task runner has not been installed yet") as *mut c_void
        }

        fn install_cpp_task_runner(&self) {
            let task_runner = create_cpp_task_runner(self.callbacks());
            assert!(
                !task_runner.is_null(),
                "C++ failed to create the Flutter Rust task runner"
            );
            *self
                .task_runner
                .lock()
                .expect("Flutter task runner poisoned") = Some(task_runner as usize);
        }

        fn dispatch_due_tasks(&self) {
            for task in self.take_due(Instant::now()) {
                run_cpp_task(task.task_runner as *mut c_void, task.task_baton);
            }
        }

        fn next_deadline(&self) -> Option<Instant> {
            self.queue
                .lock()
                .expect("Flutter task queue poisoned")
                .next_deadline()
        }
    }

    impl Drop for TaskRunnerHost {
        fn drop(&mut self) {
            let task_runner = self
                .task_runner
                .get_mut()
                .expect("Flutter task runner poisoned")
                .take();
            if let Some(task_runner) = task_runner {
                destroy_cpp_task_runner(task_runner as *mut c_void);
            }
        }
    }

    extern "C" fn schedule_task(
        user_data: *mut c_void,
        task_runner: *mut c_void,
        task_baton: u64,
        delay_nanos: u64,
    ) {
        // SAFETY: callbacks() sets user_data to a stable TaskRunnerHost address,
        // and C++ promises to stop calling it before task_runner_destroyed.
        let host = unsafe { &*user_data.cast::<TaskRunnerHost>() };
        let deadline = Instant::now() + Duration::from_nanos(delay_nanos);
        host.queue
            .lock()
            .expect("Flutter task queue poisoned")
            .schedule(
                ScheduledTask {
                    task_runner: task_runner as usize,
                    task_baton,
                },
                deadline,
            );
        if let Some(wake_proxy) = &host.wake_proxy {
            let _ = wake_proxy.send_event(HostEvent::TaskScheduled);
        }
    }

    extern "C" fn runs_tasks_on_current_thread(user_data: *mut c_void) -> i32 {
        // SAFETY: see schedule_task; this callback has the same lifetime.
        let host = unsafe { &*user_data.cast::<TaskRunnerHost>() };
        i32::from(std::thread::current().id() == host.host_thread)
    }

    extern "C" fn task_runner_destroyed(user_data: *mut c_void) {
        // SAFETY: see schedule_task; this is the last C++ callback for the host.
        let host = unsafe { &*user_data.cast::<TaskRunnerHost>() };
        host.destroyed.store(true, Ordering::Release);
    }

    /// C entry point for the private C++ runner executable. `assets_path` and
    /// `icu_data_path` are borrowed only for the duration of this call.
    /// Returns non-zero once the winit event loop exits normally.
    ///
    /// # Safety
    ///
    /// `assets_path` and `icu_data_path` must be valid, NUL-terminated C
    /// strings for the duration of this call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn FlutterRustShellRun(
        assets_path: *const std::ffi::c_char,
        icu_data_path: *const std::ffi::c_char,
    ) -> i32 {
        // SAFETY: the caller guarantees both pointers are valid, NUL-terminated
        // C strings for the duration of this call.
        let config = unsafe {
            ShellConfig {
                assets_path: std::ffi::CStr::from_ptr(assets_path)
                    .to_string_lossy()
                    .into_owned(),
                icu_data_path: std::ffi::CStr::from_ptr(icu_data_path)
                    .to_string_lossy()
                    .into_owned(),
                ..ShellConfig::default()
            }
        };
        i32::from(run(config).is_ok())
    }

    /// Runs the winit main loop for the Rust shell.
    pub fn run(config: ShellConfig) -> Result<(), winit::error::EventLoopError> {
        let event_loop = EventLoop::<HostEvent>::with_user_event().build()?;
        let task_runner_host = Box::new(TaskRunnerHost::with_wake_proxy(Some(
            event_loop.create_proxy(),
        )));
        task_runner_host.install_cpp_task_runner();
        let mut application = ShellApplication {
            config,
            window: None,
            gpu_broker: None,
            task_runner_host,
            pointer_state: PointerState::new(),
            keyboard_state: KeyboardState::new(),
            lifecycle_state: LifecycleState::new(),
            #[cfg(not(test))]
            shell: None,
        };
        event_loop.run_app(&mut application)
    }

    struct ShellApplication {
        config: ShellConfig,
        window: Option<Arc<Window>>,
        gpu_broker: Option<GpuBroker>,
        task_runner_host: Box<TaskRunnerHost>,
        pointer_state: PointerState,
        keyboard_state: KeyboardState,
        lifecycle_state: LifecycleState,
        #[cfg(not(test))]
        shell: Option<*mut c_void>,
    }

    impl Drop for ShellApplication {
        fn drop(&mut self) {
            #[cfg(not(test))]
            if let Some(shell) = self.shell.take() {
                destroy_cpp_shell(shell);
            }

            // Wgpu's Vulkan surface owns a Wayland swapchain. It must be
            // dropped while winit still owns the native Window: dropping the
            // `window` field first lets the compositor tear down its Wayland
            // proxy, after which Vulkan's vkDestroySwapchainKHR can crash in
            // the driver. Taking these options makes this order explicit
            // instead of relying on ShellApplication's field declaration
            // order (which is window before gpu_broker for startup clarity).
            self.gpu_broker.take();
            self.window.take();
        }
    }

    impl ApplicationHandler<HostEvent> for ShellApplication {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_none() {
                let attributes = Window::default_attributes().with_title(&self.config.title);
                let window = Arc::new(
                    event_loop
                        .create_window(attributes)
                        .expect("winit failed to create the Flutter Rust Shell window"),
                );
                let gpu_broker = GpuBroker::new(Arc::clone(&window))
                    .expect("winit Vulkan surface creation failed");
                let size = window.inner_size();
                gpu_broker
                    .configure(size.width, size.height)
                    .expect("winit Vulkan surface configuration failed");
                // Move the broker into its final, stable location before
                // taking its address for the C++ presentation callback table:
                // that table's user_data pointer is held by C++ for the
                // shell's entire lifetime, so it must not point at this local
                // stack variable, which is about to go out of scope.
                self.gpu_broker = Some(gpu_broker);

                #[cfg(not(test))]
                {
                    let gpu_broker = self
                        .gpu_broker
                        .as_ref()
                        .expect("gpu broker was just stored");
                    let assets_path = CString::new(self.config.assets_path.as_str())
                        .expect("assets path contains a NUL byte");
                    let icu_data_path = CString::new(self.config.icu_data_path.as_str())
                        .expect("icu data path contains a NUL byte");
                    let settings = FlutterRustShellSettings {
                        assets_path: assets_path.as_ptr(),
                        icu_data_path: icu_data_path.as_ptr(),
                    };
                    let presentation_callbacks = gpu_broker.presentation_callbacks();
                    let task_runner_handle = self.task_runner_host.task_runner_handle();
                    let shell = gpu_broker
                        .with_vulkan_context(|context_data| {
                            let instance_extensions: Vec<CString> = context_data
                                .instance_extensions
                                .iter()
                                .map(|name| {
                                    CString::new(name.as_str())
                                        .expect("extension name contains a NUL byte")
                                })
                                .collect();
                            let instance_extension_ptrs: Vec<*const std::ffi::c_char> =
                                instance_extensions
                                    .iter()
                                    .map(|name| name.as_ptr())
                                    .collect();
                            let device_extensions: Vec<CString> = context_data
                                .device_extensions
                                .iter()
                                .map(|name| {
                                    CString::new(name.as_str())
                                        .expect("extension name contains a NUL byte")
                                })
                                .collect();
                            let device_extension_ptrs: Vec<*const std::ffi::c_char> =
                                device_extensions.iter().map(|name| name.as_ptr()).collect();
                            let ffi_context_data = FlutterRustVulkanContextData {
                                get_instance_proc_addr: context_data.get_instance_proc_addr
                                    as *mut c_void,
                                instance: context_data.instance as *mut c_void,
                                physical_device: context_data.physical_device as *mut c_void,
                                device: context_data.device as *mut c_void,
                                queue: context_data.queue as *mut c_void,
                                queue_family_index: context_data.queue_family_index,
                                instance_extensions: instance_extension_ptrs.as_ptr(),
                                instance_extensions_count: instance_extension_ptrs.len() as u32,
                                device_extensions: device_extension_ptrs.as_ptr(),
                                device_extensions_count: device_extension_ptrs.len() as u32,
                            };
                            create_cpp_shell(
                                task_runner_handle,
                                ffi_context_data,
                                presentation_callbacks,
                                settings,
                            )
                        })
                        .expect("wgpu Vulkan context extraction failed");
                    assert!(
                        !shell.is_null(),
                        "C++ failed to create the Flutter Rust shell"
                    );
                    assert!(
                        run_cpp_shell(shell) != 0,
                        "the Flutter Rust shell failed to start running"
                    );
                    // The engine has no valid view to schedule frames for
                    // until it knows the implicit view's size.
                    set_cpp_shell_viewport_metrics(
                        shell,
                        WindowMetrics::from_window(&window, window.scale_factor()),
                    );
                    self.shell = Some(shell);
                }

                self.window = Some(window);
            }
            if let Some(window) = &self.window {
                let visible = {
                    let size = window.inner_size();
                    size.width > 0 && size.height > 0
                };
                let state = self.lifecycle_state.resumed(visible, window.has_focus());
                self.send_lifecycle_event(state);
            }
        }

        fn suspended(&mut self, _: &ActiveEventLoop) {
            let state = self.lifecycle_state.suspended();
            self.send_lifecycle_event(state);
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if !self
                .window
                .as_ref()
                .is_some_and(|window| window.id() == window_id)
            {
                return;
            }
            match event {
                WindowEvent::Resized(size) => {
                    if let Some(gpu_broker) = &self.gpu_broker {
                        gpu_broker
                            .configure(size.width, size.height)
                            .expect("winit Vulkan surface reconfiguration failed");
                    }
                    #[cfg(not(test))]
                    if let Some(shell) = self.shell {
                        let window = self.window.as_ref().expect("window event without a window");
                        set_cpp_shell_viewport_metrics(
                            shell,
                            WindowMetrics::from_window(window, window.scale_factor()),
                        );
                    }
                    let state = self
                        .lifecycle_state
                        .visibility_changed(size.width > 0 && size.height > 0);
                    self.send_lifecycle_event(state);
                }
                WindowEvent::ScaleFactorChanged {
                    scale_factor: _scale_factor,
                    ..
                } => {
                    if let Some(window) = &self.window {
                        let size = window.inner_size();
                        if let Some(gpu_broker) = &self.gpu_broker {
                            gpu_broker
                                .configure(size.width, size.height)
                                .expect("winit Vulkan surface reconfiguration failed");
                        }
                        #[cfg(not(test))]
                        if let Some(shell) = self.shell {
                            set_cpp_shell_viewport_metrics(
                                shell,
                                WindowMetrics::from_window(window, _scale_factor),
                            );
                        }
                    }
                }
                WindowEvent::Focused(focused) => {
                    let state = self.lifecycle_state.focus_changed(focused);
                    self.send_lifecycle_event(state);
                }
                WindowEvent::KeyboardInput {
                    event,
                    is_synthetic,
                    ..
                } => {
                    if let Some(event) = self.keyboard_state.event(&event, is_synthetic) {
                        self.send_key_event(event);
                    }
                }
                WindowEvent::CursorEntered { .. } => {
                    if let Some(event) = self.pointer_state.entered() {
                        self.send_pointer_events([event]);
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    if let Some(event) = self.pointer_state.left() {
                        self.send_pointer_events([event]);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let events = self.pointer_state.moved(position.x, position.y);
                    self.send_pointer_events(events);
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let events = self.pointer_state.button(button, state);
                    self.send_pointer_events(events);
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let events = self.pointer_state.scroll(delta);
                    self.send_pointer_events(events);
                }
                WindowEvent::Touch(touch) => {
                    let event = self.pointer_state.touch(touch);
                    self.send_pointer_events([event]);
                }
                WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                    let state = self.lifecycle_state.detached();
                    self.send_lifecycle_event(state);
                    event_loop.exit();
                }
                _ => {}
            }
        }

        fn user_event(&mut self, _: &ActiveEventLoop, _: HostEvent) {}

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            self.task_runner_host.dispatch_due_tasks();
            match self.task_runner_host.next_deadline() {
                Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
                None => event_loop.set_control_flow(ControlFlow::Wait),
            }
        }
    }

    impl ShellApplication {
        fn send_pointer_events(&self, events: impl IntoIterator<Item = FlutterRustPointerEvent>) {
            #[cfg(not(test))]
            if let Some(shell) = self.shell {
                for event in events {
                    send_cpp_pointer_event(shell, event);
                }
            }
            #[cfg(test)]
            for _ in events {}
        }

        fn send_lifecycle_event(&self, state: Option<FlutterRustLifecycleState>) {
            #[cfg(not(test))]
            if let (Some(shell), Some(state)) = (self.shell, state) {
                send_cpp_lifecycle_event(shell, state);
            }
            #[cfg(test)]
            let _ = state;
        }

        fn send_key_event(&self, event: FlutterRustKeyEvent) {
            #[cfg(not(test))]
            if let Some(shell) = self.shell {
                send_cpp_key_event(shell, event);
            }
            #[cfg(test)]
            let _ = event;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn has_a_stable_default_window_title() {
            assert_eq!(ShellConfig::default().title, "Flutter Rust Shell");
        }

        #[test]
        fn returns_only_due_task_batons() {
            let now = Instant::now();
            let mut queue = TaskQueue::default();
            let first = ScheduledTask {
                task_runner: 1,
                task_baton: 1,
            };
            let second = ScheduledTask {
                task_runner: 1,
                task_baton: 2,
            };
            queue.schedule(first, now);
            queue.schedule(second, now + Duration::from_secs(1));

            assert_eq!(queue.take_due(now), vec![first]);
            assert_eq!(queue.next_deadline(), Some(now + Duration::from_secs(1)));
        }

        #[test]
        fn installs_callbacks_backed_by_the_rust_host() {
            let host = Box::new(TaskRunnerHost::new());
            let callbacks = host.callbacks();
            let schedule = callbacks.schedule_task.expect("schedule callback");
            let is_current = callbacks
                .runs_tasks_on_current_thread
                .expect("thread callback");
            let destroyed = callbacks
                .task_runner_destroyed
                .expect("destruction callback");

            assert_eq!(is_current(callbacks.user_data), 1);
            schedule(callbacks.user_data, 0x10usize as *mut c_void, 7, 0);
            assert_eq!(
                host.take_due(Instant::now()),
                vec![ScheduledTask {
                    task_runner: 0x10,
                    task_baton: 7,
                }]
            );
            destroyed(callbacks.user_data);
            assert!(host.is_destroyed());
        }

        #[test]
        fn translates_mouse_motion_and_button_state() {
            let mut pointer = PointerState::new();
            let motion = pointer.moved(12.5, 24.0);
            assert_eq!(motion.len(), 2);
            assert_eq!(motion[0].phase, FlutterRustPointerPhase::Add as u32);
            assert_eq!(motion[1].phase, FlutterRustPointerPhase::Hover as u32);
            assert_eq!((motion[1].physical_x, motion[1].physical_y), (12.5, 24.0));

            let down = pointer.button(MouseButton::Left, ElementState::Pressed);
            assert_eq!(down.len(), 1);
            assert_eq!(down[0].phase, FlutterRustPointerPhase::Down as u32);
            assert_eq!(down[0].buttons, MOUSE_PRIMARY_BUTTON);

            let second_down = pointer.button(MouseButton::Right, ElementState::Pressed);
            assert_eq!(second_down[0].phase, FlutterRustPointerPhase::Move as u32);
            assert_eq!(
                second_down[0].buttons,
                MOUSE_PRIMARY_BUTTON | MOUSE_SECONDARY_BUTTON
            );

            let second_up = pointer.button(MouseButton::Right, ElementState::Released);
            assert_eq!(second_up[0].phase, FlutterRustPointerPhase::Move as u32);
            assert_eq!(second_up[0].buttons, MOUSE_PRIMARY_BUTTON);

            let drag = pointer.moved(20.0, 30.0);
            assert_eq!(drag[0].phase, FlutterRustPointerPhase::Move as u32);
            assert_eq!(drag[0].buttons, MOUSE_PRIMARY_BUTTON);

            let up = pointer.button(MouseButton::Left, ElementState::Released);
            assert_eq!(up[0].phase, FlutterRustPointerPhase::Up as u32);
            assert_eq!(up[0].buttons, 0);
        }

        #[test]
        fn removes_a_dragged_pointer_after_its_last_button_is_released() {
            let mut pointer = PointerState::new();
            pointer.moved(12.5, 24.0);
            pointer.button(MouseButton::Left, ElementState::Pressed);

            assert!(pointer.left().is_none());
            let release = pointer.button(MouseButton::Left, ElementState::Released);

            assert_eq!(release.len(), 2);
            assert_eq!(release[0].phase, FlutterRustPointerPhase::Up as u32);
            assert_eq!(release[1].phase, FlutterRustPointerPhase::Remove as u32);
        }

        #[test]
        fn translates_scroll_direction_and_line_units() {
            let mut pointer = PointerState::new();
            pointer.moved(5.0, 6.0);
            let events = pointer.scroll(MouseScrollDelta::LineDelta(1.0, 2.0));
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].signal_kind,
                FlutterRustPointerSignalKind::Scroll as u32
            );
            assert_eq!(events[0].scroll_delta_x, SCROLL_LINE_PIXELS);
            assert_eq!(events[0].scroll_delta_y, -2.0 * SCROLL_LINE_PIXELS);
        }

        #[test]
        fn touch_devices_do_not_collide_with_the_mouse() {
            assert_eq!(touch_device_id(0), 1);
            assert_eq!(touch_device_id(7), 8);
            assert_eq!(touch_device_id(u64::MAX), i64::MAX);
        }

        #[test]
        fn translates_key_down_repeat_and_up() {
            let down = make_key_event(
                1234,
                PhysicalKey::Code(KeyCode::KeyA),
                &Key::Character("A".into()),
                Some("A"),
                ElementState::Pressed,
                false,
                false,
            )
            .expect("key A should be supported");
            assert_eq!(down.timestamp_micros, 1234);
            assert_eq!(down.event_type, FlutterRustKeyEventType::Down as u32);
            assert_eq!(down.physical, 0x00070004);
            assert_eq!(down.logical, u64::from('a'));
            assert_eq!(down.character_length, 1);
            assert_eq!(&down.character[..1], b"A");

            let repeat = make_key_event(
                1235,
                PhysicalKey::Code(KeyCode::KeyA),
                &Key::Character("a".into()),
                Some("a"),
                ElementState::Pressed,
                true,
                false,
            )
            .expect("repeated key A should be supported");
            assert_eq!(repeat.event_type, FlutterRustKeyEventType::Repeat as u32);

            let up = make_key_event(
                1236,
                PhysicalKey::Code(KeyCode::KeyA),
                &Key::Character("a".into()),
                Some("a"),
                ElementState::Released,
                false,
                false,
            )
            .expect("released key A should be supported");
            assert_eq!(up.event_type, FlutterRustKeyEventType::Up as u32);
            assert_eq!(up.character_length, 0);
        }

        #[test]
        fn preserves_modifier_side_and_synthetic_state() {
            let event = make_key_event(
                42,
                PhysicalKey::Code(KeyCode::ShiftRight),
                &Key::Named(NamedKey::Shift),
                None,
                ElementState::Released,
                false,
                true,
            )
            .expect("right shift should be supported");

            assert_eq!(event.physical, 0x000700e5);
            assert_eq!(event.logical, 0x00200000103);
            assert_eq!(event.synthesized, 1);
        }

        #[test]
        fn uses_the_gtk_plane_for_unidentified_xkb_keys() {
            let event = make_key_event(
                7,
                PhysicalKey::Unidentified(NativeKeyCode::Xkb(0x1234)),
                &Key::Unidentified(NativeKey::Xkb(0x5678)),
                None,
                ElementState::Pressed,
                false,
                false,
            )
            .expect("XKB keys should have a stable fallback");

            assert_eq!(event.physical, GTK_KEY_PLANE | 0x1234);
            assert_eq!(event.logical, GTK_KEY_PLANE | 0x5678);
        }

        #[test]
        fn lifecycle_tracks_focus_visibility_suspend_and_detach() {
            let mut lifecycle = LifecycleState::new();
            assert_eq!(
                lifecycle.resumed(true, false),
                Some(FlutterRustLifecycleState::Inactive)
            );
            assert_eq!(
                lifecycle.focus_changed(true),
                Some(FlutterRustLifecycleState::Resumed)
            );
            assert_eq!(lifecycle.focus_changed(true), None);
            assert_eq!(
                lifecycle.visibility_changed(false),
                Some(FlutterRustLifecycleState::Hidden)
            );
            assert_eq!(
                lifecycle.suspended(),
                Some(FlutterRustLifecycleState::Paused)
            );
            assert_eq!(
                lifecycle.detached(),
                Some(FlutterRustLifecycleState::Detached)
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{ScheduledTask, ShellConfig, TaskQueue, TaskRunnerHost, run};
