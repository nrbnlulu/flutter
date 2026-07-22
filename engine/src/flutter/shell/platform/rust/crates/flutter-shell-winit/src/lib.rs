//! Winit-owned native host for the optional Flutter Rust shell.
//!
//! The host keeps Flutter UI/platform task batons in a monotonic task queue.
//! The private C++ bridge will install the callback that executes a due baton.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod linux {
    use std::{collections::BTreeMap, time::Instant};

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

    /// A monotonic queue for opaque Flutter task batons.
    ///
    /// C++ converts its `fml::TimePoint` to an `Instant` deadline before a
    /// baton enters this queue. The task itself remains owned by C++ and is
    /// executed only after the host loop returns its baton through the private
    /// bridge.
    #[derive(Debug, Default)]
    pub struct TaskQueue {
        tasks: BTreeMap<Instant, Vec<u64>>,
    }

    impl TaskQueue {
        pub fn schedule(&mut self, task_baton: u64, deadline: Instant) {
            self.tasks.entry(deadline).or_default().push(task_baton);
        }

        pub fn next_deadline(&self) -> Option<Instant> {
            self.tasks.first_key_value().map(|(deadline, _)| *deadline)
        }

        pub fn take_due(&mut self, now: Instant) -> Vec<u64> {
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
        use std::time::Duration;

        #[test]
        fn has_a_stable_default_window_title() {
            assert_eq!(LinuxShellConfig::default().title, "Flutter Rust Shell");
        }

        #[test]
        fn returns_only_due_task_batons() {
            let now = Instant::now();
            let mut queue = TaskQueue::default();
            queue.schedule(1, now);
            queue.schedule(2, now + Duration::from_secs(1));

            assert_eq!(queue.take_due(now), vec![1]);
            assert_eq!(queue.next_deadline(), Some(now + Duration::from_secs(1)));
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{LinuxShellConfig, TaskQueue, run};
