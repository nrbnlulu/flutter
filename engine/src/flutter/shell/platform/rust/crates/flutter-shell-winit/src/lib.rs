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
        FlutterRustPointerDeviceKind, FlutterRustPointerEvent, FlutterRustPointerPhase,
        FlutterRustPointerSignalKind, FlutterRustTaskRunnerCallbacks,
    };
    #[cfg(not(test))]
    use flutter_shell_core::{FlutterRustShellSettings, FlutterRustVulkanContextData};
    use flutter_shell_wgpu::GpuBroker;
    #[cfg(not(test))]
    use std::ffi::CString;
    use winit::{
        application::ApplicationHandler,
        event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent},
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
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
    fn set_cpp_shell_viewport_metrics(shell: *mut c_void, width: u32, height: u32) {
        unsafe extern "C" {
            fn FlutterRustShellSetViewportMetrics(
                shell: *mut c_void,
                width: f64,
                height: f64,
                pixel_ratio: f64,
            );
        }
        // SAFETY: `shell` was returned by create_cpp_shell and not yet destroyed.
        unsafe {
            FlutterRustShellSetViewportMetrics(shell, width as f64, height as f64, 1.0);
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
                    set_cpp_shell_viewport_metrics(shell, size.width, size.height);
                    self.shell = Some(shell);
                }

                self.window = Some(window);
            }
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
                        set_cpp_shell_viewport_metrics(shell, size.width, size.height);
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
                WindowEvent::CloseRequested => event_loop.exit(),
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
    }
}

#[cfg(target_os = "linux")]
pub use linux::{ScheduledTask, ShellConfig, TaskQueue, TaskRunnerHost, run};
