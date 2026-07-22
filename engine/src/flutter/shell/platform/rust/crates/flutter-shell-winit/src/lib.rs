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
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread::ThreadId,
        time::{Duration, Instant},
    };

    use flutter_shell_core::FlutterRustTaskRunnerCallbacks;
    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop},
        window::{Window, WindowId},
    };

    /// Linux host configuration that is independent of GTK.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LinuxShellConfig {
        pub title: String,
    }

    impl Default for LinuxShellConfig {
        fn default() -> Self {
            Self {
                title: "Flutter Rust Shell".to_owned(),
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

    /// Rust-owned state for a single merged Flutter UI/platform task runner.
    ///
    /// The value must have a stable address for as long as C++ retains the
    /// callback table returned by [`Self::callbacks`]. A `Box<TaskRunnerHost>`
    /// satisfies that requirement. Winit integration owns the box and wakes its
    /// loop after scheduling; that wake is added with the C++ shell bootstrap.
    pub struct TaskRunnerHost {
        queue: Mutex<TaskQueue>,
        host_thread: ThreadId,
        destroyed: AtomicBool,
    }

    impl TaskRunnerHost {
        pub fn new() -> Self {
            Self {
                queue: Mutex::new(TaskQueue::default()),
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

    /// Runs the winit main loop for the Rust shell.
    pub fn run(config: LinuxShellConfig) -> Result<(), winit::error::EventLoopError> {
        let event_loop = EventLoop::new()?;
        let mut application = LinuxShellApplication {
            config,
            window: None,
        };
        event_loop.run_app(&mut application)
    }

    struct LinuxShellApplication {
        config: LinuxShellConfig,
        window: Option<Window>,
    }

    impl ApplicationHandler for LinuxShellApplication {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_none() {
                let attributes = Window::default_attributes().with_title(&self.config.title);
                self.window = Some(
                    event_loop
                        .create_window(attributes)
                        .expect("winit failed to create the Flutter Rust Shell window"),
                );
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if self
                .window
                .as_ref()
                .is_some_and(|window| window.id() == window_id)
                && matches!(event, WindowEvent::CloseRequested)
            {
                event_loop.exit();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn has_a_stable_default_window_title() {
            assert_eq!(LinuxShellConfig::default().title, "Flutter Rust Shell");
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
    }
}

#[cfg(target_os = "linux")]
pub use linux::{LinuxShellConfig, ScheduledTask, TaskQueue, TaskRunnerHost, run};
