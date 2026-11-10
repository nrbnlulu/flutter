use super::*;

pub(crate) enum HostEvent {
    TaskScheduled,
    VsyncRequested,
    MainThreadTask(MainThreadTask),
    ViewOperationCompleted {
        view_id: FlutterRustViewId,
        operation: ViewOperation,
        succeeded: bool,
    },
    RequestAppExit {
        response: Option<PendingPlatformResponse>,
    },
    ExitRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingPlatformResponse(pub(crate) usize);

#[derive(Clone)]
pub(crate) struct HostEventSender {
    queue: Arc<Mutex<VecDeque<HostEvent>>>,
    wake: Arc<HostWake>,
}

enum HostWake {
    Native(EventLoopProxy),
    #[cfg(target_os = "linux")]
    Headless(parking_lot::Condvar),
}
impl HostWake {
    fn notify(&self) {
        match self {
            Self::Native(proxy) => proxy.wake_up(),
            #[cfg(target_os = "linux")]
            Self::Headless(wake) => {
                wake.notify_one();
            }
        }
    }
}

pub(crate) const MAX_HOST_EVENTS_PER_TURN: usize = 64;

impl HostEventSender {
    pub(crate) fn new(proxy: EventLoopProxy) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            wake: Arc::new(HostWake::Native(proxy)),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn headless() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            wake: Arc::new(HostWake::Headless(parking_lot::Condvar::new())),
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn wait_until(&self, deadline: Option<Instant>) {
        let HostWake::Headless(wake) = &*self.wake else {
            return;
        };
        let mut queue = self.queue.lock();
        if !queue.is_empty() {
            return;
        }
        // Queue check and condvar sleep share the producer's mutex, so a
        // message cannot be lost between checking and sleeping.
        match deadline {
            Some(deadline) => {
                wake.wait_until(&mut queue, deadline);
            }
            None => wake.wait(&mut queue),
        }
    }

    pub(crate) fn send_event(&self, event: HostEvent) -> Result<(), ()> {
        self.queue.lock().push_back(event);
        self.wake.notify();
        Ok(())
    }

    pub(crate) fn drain(&self) -> Vec<HostEvent> {
        let (events, has_more) = drain_host_event_batch(&self.queue);
        if has_more {
            self.wake.notify();
        }
        events
    }
}

pub(crate) fn drain_host_event_batch(queue: &Mutex<VecDeque<HostEvent>>) -> (Vec<HostEvent>, bool) {
    let mut queue = queue.lock();
    let count = queue.len().min(MAX_HOST_EVENTS_PER_TURN);
    let events = queue.drain(..count).collect();
    (events, !queue.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewOperation {
    Add,
    Remove,
}

pub(crate) struct ViewOperationContext {
    pub(crate) wake_proxy: HostEventSender,
    pub(crate) operation: ViewOperation,
}

pub(crate) struct ActiveWindowingContext {
    pub(crate) event_loop: *const dyn ActiveEventLoop,
    pub(crate) windows: Weak<RefCell<WindowRegistry>>,
}

thread_local! {
    pub(crate) static ACTIVE_WINDOWING_CONTEXT: RefCell<Option<ActiveWindowingContext>> = const {
        RefCell::new(None)
    };
}

pub(crate) fn with_active_windowing_context<T>(
    event_loop: &dyn ActiveEventLoop,
    windows: &Rc<RefCell<WindowRegistry>>,
    callback: impl FnOnce() -> T,
) -> T {
    ACTIVE_WINDOWING_CONTEXT.with(|slot| {
        assert!(slot.borrow().is_none(), "nested Rust windowing context");
        *slot.borrow_mut() = Some(ActiveWindowingContext {
            event_loop,
            windows: Rc::downgrade(windows),
        });
    });
    let result = callback();
    ACTIVE_WINDOWING_CONTEXT.with(|slot| *slot.borrow_mut() = None);
    result
}

#[cfg(all(test, target_os = "linux"))]
mod headless_tests {
    use super::*;

    #[test]
    fn sleeping_host_wakes_for_cross_thread_messages() {
        let sender = HostEventSender::headless();
        let worker = sender.clone();
        let (ready, waiting) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            ready.send(()).unwrap();
            worker.wait_until(Some(Instant::now() + Duration::from_secs(2)));
            worker.drain().len()
        });
        waiting.recv().unwrap();
        sender.send_event(HostEvent::TaskScheduled).unwrap();
        assert_eq!(task.join().unwrap(), 1);
    }

    #[test]
    fn a_message_queued_before_sleep_is_not_lost() {
        let sender = HostEventSender::headless();
        sender.send_event(HostEvent::ExitRequested).unwrap();
        let start = Instant::now();
        sender.wait_until(Some(start + Duration::from_secs(2)));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            sender.drain().as_slice(),
            [HostEvent::ExitRequested]
        ));
    }
}
