//! Private local transport. Parsing, semantics merging and PNG encoding run on
//! the session worker; mutations are serialized onto the merged host thread.
use super::*;
use flutter_shell_agent::{
    AgentError, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Selector, SemanticsCache, response,
    write_png,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

type Reply = Result<Value, AgentError>;
enum Event {
    Request(Result<Request, AgentError>),
    Semantics(i64, Vec<u8>),
    Bridge(Vec<u8>),
    Changed,
    Disconnected,
    Stop,
}
pub(super) struct Shared {
    sender: SyncSender<Event>,
    windows: Mutex<Value>,
    frames: Mutex<BTreeMap<i64, Arc<OffscreenFrame>>>,
    revisions: Mutex<BTreeMap<i64, u64>>,
    fatal: Mutex<Option<AgentError>>,
    stop: Arc<AtomicBool>,
    epoch: String,
    start: Instant,
    events: HostEventSender,
    adapter: Value,
    engine_revision: String,
    artifacts: PathBuf,
    sequence: std::sync::atomic::AtomicU64,
    logs: Mutex<VecDeque<Value>>,
    build_mode: String,
    bridge: Mutex<Value>,
    bridge_responses: Mutex<BTreeMap<u64, Value>>,
    bridge_sequence: std::sync::atomic::AtomicU64,
    recordings: Mutex<BTreeMap<i64, Arc<recording::Sink>>>,
}
impl Shared {
    pub(super) fn publish_bridge(&self, bytes: Vec<u8>) {
        if self.sender.try_send(Event::Bridge(bytes)).is_err() {
            self.fail(AgentError::new(
                "app_exception",
                "Dart bridge event queue overflow",
            ));
        }
    }
    fn bridge_event(&self, bytes: &[u8]) -> Result<(), AgentError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|e| {
            AgentError::new("app_exception", format!("invalid Dart bridge message: {e}"))
        })?;
        match value["type"].as_str() {
            Some("ready") => *self.bridge.lock() = value,
            Some("error") => self.fail(
                AgentError::new(
                    "app_exception",
                    value["message"]
                        .as_str()
                        .unwrap_or("Dart bridge reported an error"),
                )
                .with_data(value),
            ),
            Some("response") => {
                let id = value["id"]
                    .as_u64()
                    .ok_or_else(|| AgentError::new("app_exception", "bridge response has no id"))?;
                let mut responses = self.bridge_responses.lock();
                if responses.len() >= 32 {
                    responses.clear();
                }
                responses.insert(id, value);
            }
            _ => {
                return Err(AgentError::new(
                    "app_exception",
                    "unknown Dart bridge message",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn failure(&self) -> Option<AgentError> {
        self.fatal.lock().clone()
    }
    pub(super) fn publish_windows(&self, windows: Value) {
        self.recordings.lock().retain(|view, sink| {
            let alive = windows["views"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|v| v["viewId"].as_i64() == Some(*view) && v["removing"] != true);
            if !alive {
                sink.stop("view_closed");
            }
            alive
        });
        let mut previous = self.windows.lock();
        if *previous != windows {
            *previous = windows;
            let _ = self.sender.try_send(Event::Changed);
        }
    }
    pub(super) fn publish_frame(&self, frame: OffscreenFrame) {
        let frame = Arc::new(frame);
        if let Some(sink) = self.recordings.lock().get(&frame.view_id) {
            sink.publish(frame.clone());
        }
        self.frames.lock().insert(frame.view_id, frame);
        let _ = self.sender.try_send(Event::Changed);
    }
    pub(super) fn fail(&self, error: AgentError) {
        *self.fatal.lock() = Some(error);
        let _ = self.sender.try_send(Event::Changed);
        let _ = self.events.send_event(HostEvent::TaskScheduled);
    }
    fn view(&self, id: i64) -> Result<Value, AgentError> {
        self.windows.lock()["views"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["viewId"].as_i64() == Some(id) && v["removing"] != true)
            .cloned()
            .ok_or_else(|| AgentError::new("view_closed", format!("view {id} is not live")))
    }
    fn assert_revision(&self, handle: &Value) -> Result<(), AgentError> {
        if handle["epoch"].as_str() != Some(&self.epoch) {
            return Err(AgentError::new("stale_handle", "session epoch changed"));
        }
        let view = handle["viewId"]
            .as_i64()
            .ok_or_else(|| AgentError::new("invalid_request", "handle needs viewId"))?;
        if self.revisions.lock().get(&view).copied() != handle["revision"].as_u64() {
            return Err(AgentError::new(
                "stale_handle",
                "semantics revision changed before dispatch",
            ));
        }
        Ok(())
    }
}
extern "C" fn semantics(data: *mut c_void, view: i64, bytes: *const u8, length: u64) {
    // C++ borrows bytes for this call and stops callbacks before Session's Arc
    // is dropped. Arc::as_ptr addresses the stable shared allocation directly.
    let shared = unsafe { &*data.cast::<Shared>() };
    if shared.stop.load(Ordering::Acquire) {
        return;
    }
    if length > 16 * 1024 * 1024 || (length != 0 && bytes.is_null()) {
        shared.fail(AgentError::new(
            "app_exception",
            "semantics update exceeds transport limit",
        ));
        return;
    }
    let owned = if length == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec()
    };
    *shared.revisions.lock().entry(view).or_default() += 1;
    if shared
        .sender
        .try_send(Event::Semantics(view, owned))
        .is_err()
    {
        shared.fail(AgentError::new(
            "app_exception",
            "semantics queue overflow; cache invalidated",
        ));
    }
}

extern "C" fn diagnostic(
    data: *mut c_void,
    kind: i32,
    message: *const u8,
    length: u64,
    stack: *const u8,
    stack_length: u64,
) {
    let shared = unsafe { &*data.cast::<Shared>() };
    if shared.stop.load(Ordering::Acquire) {
        return;
    }
    if length > 1024 * 1024 || stack_length > 1024 * 1024 {
        shared.fail(AgentError::new(
            "app_exception",
            "diagnostic exceeds size limit",
        ));
        return;
    }
    let text = if length == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, length as usize) })
            .into_owned()
    };
    let stack = if stack_length == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(stack, stack_length as usize) })
            .into_owned()
    };
    if kind == 1 {
        shared
            .fail(AgentError::new("app_exception", text.clone()).with_data(json!({"stack":stack})));
    }
    let mut logs = shared.logs.lock();
    if logs.len() >= 256 {
        logs.pop_front();
    }
    logs.push_back(json!({"kind":if kind==1 {"exception"} else {"log"},"message":text,"stack":stack,"elapsedMicros":shared.start.elapsed().as_micros() as u64}));
}

pub(super) struct Session {
    pub(super) shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    socket: PathBuf,
    connection: Arc<Mutex<Option<UnixStream>>>,
}
impl Session {
    pub(super) fn start(
        host: &VirtualHost,
        path: PathBuf,
        artifacts: PathBuf,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&artifacts).map_err(|e| e.to_string())?;
        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("agent endpoint {}: {e}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::sync_channel(64);
        let epoch = format!(
            "{:x}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        );
        unsafe extern "C" {
            fn FlutterRustShellGetEngineRevision() -> *const std::ffi::c_char;
            fn FlutterRustShellGetBuildMode() -> *const std::ffi::c_char;
        }
        let revision = unsafe { std::ffi::CStr::from_ptr(FlutterRustShellGetEngineRevision()) }
            .to_string_lossy()
            .into_owned();
        let build_mode = unsafe { std::ffi::CStr::from_ptr(FlutterRustShellGetBuildMode()) }
            .to_string_lossy()
            .into_owned();
        let shared = Arc::new(Shared {
            sender,
            windows: Mutex::new(host.snapshot()),
            frames: Mutex::new(BTreeMap::new()),
            revisions: Mutex::new(BTreeMap::new()),
            fatal: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            epoch,
            start: Instant::now(),
            events: host.events.clone(),
            adapter: json!({"name":host.context.adapter_info().name,"backend":"Vulkan","driver":host.context.adapter_info().driver_info}),
            engine_revision: revision,
            artifacts,
            sequence: std::sync::atomic::AtomicU64::new(0),
            logs: Mutex::new(VecDeque::new()),
            build_mode,
            bridge: Mutex::new(Value::Null),
            bridge_responses: Mutex::new(BTreeMap::new()),
            bridge_sequence: std::sync::atomic::AtomicU64::new(0),
            recordings: Mutex::new(BTreeMap::new()),
        });
        let connection = Arc::new(Mutex::new(None));
        let thread_shared = shared.clone();
        let thread_connection = connection.clone();
        let host_address = host as *const VirtualHost as usize;
        let worker = thread::Builder::new()
            .name("flutter-agent".into())
            .spawn(move || {
                let result = serve(
                    listener,
                    receiver,
                    thread_shared.clone(),
                    host_address,
                    &thread_connection,
                );
                if let Err(error) = result {
                    thread_shared.fail(error);
                }
                let _ = thread_shared.events.send_event(HostEvent::ExitRequested);
            })
            .map_err(|e| e.to_string())?;
        let session = Self {
            shared,
            worker: Some(worker),
            socket: path,
            connection,
        };
        unsafe extern "C" {
            fn FlutterRustShellSetSemanticsCallbacks(
                shell: *mut c_void,
                callbacks: flutter_shell_core::FlutterRustSemanticsCallbacks,
            );
            fn FlutterRustShellSetSemanticsEnabled(shell: *mut c_void, enabled: i32);
            fn FlutterRustShellSetDiagnosticCallbacks(
                shell: *mut c_void,
                callbacks: flutter_shell_core::FlutterRustDiagnosticCallbacks,
            );
        }
        *host.agent.borrow_mut() = Some(session.shared.clone());
        unsafe {
            FlutterRustShellSetSemanticsCallbacks(
                host.shell.get(),
                flutter_shell_core::FlutterRustSemanticsCallbacks {
                    user_data: Arc::as_ptr(&session.shared).cast_mut().cast(),
                    update: Some(semantics),
                },
            );
            FlutterRustShellSetDiagnosticCallbacks(
                host.shell.get(),
                flutter_shell_core::FlutterRustDiagnosticCallbacks {
                    user_data: Arc::as_ptr(&session.shared).cast_mut().cast(),
                    report: Some(diagnostic),
                },
            );
            FlutterRustShellSetSemanticsEnabled(host.shell.get(), 1);
        }
        Ok(session)
    }
    pub(super) fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(stream) = self.connection.lock().as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = self.shared.sender.try_send(Event::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_file(&self.socket);
        if std::env::var_os("FLUTTER_RUST_AGENT_ENDPOINT_OWNED").is_some_and(|v| v == "1")
            && let Some(parent) = self.socket.parent()
        {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn read_requests(mut stream: UnixStream, shared: Arc<Shared>) {
    let mut pending = Vec::new();
    let mut buffer = [0u8; 8192];
    while !shared.stop.load(Ordering::Acquire) {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                pending.extend_from_slice(&buffer[..length]);
                while let Some(end) = pending.iter().position(|c| *c == b'\n') {
                    let line: Vec<_> = pending.drain(..=end).collect();
                    if shared
                        .sender
                        .send(Event::Request(Request::parse(&line[..line.len() - 1])))
                        .is_err()
                    {
                        return;
                    }
                }
                if pending.len() > MAX_MESSAGE_BYTES {
                    let _ = shared.sender.send(Event::Request(Err(AgentError::new(
                        "invalid_request",
                        "message exceeds 1 MiB",
                    ))));
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = shared.sender.send(Event::Disconnected);
}
fn save_failure_artifacts(shared: &Shared, cache: &SemanticsCache, error: &AgentError) {
    let result = (|| -> Reply {
        let windows = shared.windows.lock().clone();
        let semantics: Vec<_> = windows["views"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["viewId"].as_i64())
            .map(|id| cache.snapshot(id))
            .collect();
        let frames: Vec<_> = shared.frames.lock().values().cloned().collect();
        let mut evidence = Vec::new();
        for frame in frames {
            let path = shared
                .artifacts
                .join(format!("failure-view-{}.png", frame.view_id));
            let result = write_png(&path, frame.width, frame.height, &frame.rgba);
            evidence.push(json!({"path":path,"frameId":frame.frame,"uiFrameNumber":frame.ui_frame_number,"error":result.err()}));
        }
        let manifest = json!({"error":error,"epoch":shared.epoch,"windows":windows,"semantics":semantics,"logs":shared.logs.lock().clone(),"frames":evidence});
        std::fs::write(
            shared.artifacts.join("failure.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
        Ok(Value::Null)
    })();
    if let Err(error) = result {
        eprintln!("agent failure evidence: {error:?}");
    }
}

struct Client {
    stream: UnixStream,
    incoming: Receiver<Event>,
    pending: VecDeque<Request>,
    cache: SemanticsCache,
    shared: Arc<Shared>,
    host: usize,
    disconnected: bool,
    servicing: bool,
    subscriptions: BTreeSet<String>,
    last_windows: Value,
    trace: std::fs::File,
    trace_bytes: u64,
    capture_id: u64,
    recordings: BTreeMap<String, recording::Recording>,
}
impl Client {
    fn trace(&mut self, direction: &str, value: &Value) -> Result<(), AgentError> {
        // Never retain editing payloads; obscured fields are also redacted in snapshots.
        let mut value = value.clone();
        if let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) {
            for field in ["text", "character", "args"] {
                if params.contains_key(field) {
                    params.insert(field.into(), json!("<redacted>"));
                }
            }
        }
        let mut bytes = serde_json::to_vec(&json!({"direction":direction,"elapsedMicros":self.shared.start.elapsed().as_micros() as u64,"message":value})).unwrap();
        bytes.push(b'\n');
        self.trace_bytes += bytes.len() as u64;
        if self.trace_bytes > 64 * 1024 * 1024 {
            return Err(AgentError::new(
                "artifact_error",
                "protocol trace exceeded 64 MiB retention limit",
            ));
        }
        self.trace
            .write_all(&bytes)
            .map_err(|e| AgentError::new("artifact_error", e.to_string()))
    }
    fn output(&mut self, value: Value) -> Result<(), AgentError> {
        self.trace("out", &value)?;
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.stream
            .write_all(&bytes)
            .map_err(|e| AgentError::new("process_exit", e.to_string()))
    }
    fn write(&mut self, id: Value, result: Reply) -> Result<(), AgentError> {
        self.output(response(id, result))
    }
    fn notify(&mut self, kind: &str, data: Value) -> Result<(), AgentError> {
        let message = json!({"event":kind,"epoch":self.shared.epoch,"elapsedMicros":self.shared.start.elapsed().as_micros() as u64,"data":data});
        if self.subscriptions.contains(kind) {
            self.output(message)
        } else {
            self.trace("event", &message)
        }
    }
    fn action_event(&mut self, method: &str, result: &Reply) -> Result<(), AgentError> {
        if (method.starts_with("input.")
            || matches!(
                method,
                "windows.focus" | "windows.resize" | "windows.close" | "semantics.perform"
            ))
            && let Ok(acknowledgement) = result
        {
            self.notify(
                "action.dispatched",
                json!({"method":method,"acknowledgement":acknowledgement}),
            )?;
        }
        Ok(())
    }
    fn failure_artifacts(&mut self, error: &AgentError) {
        save_failure_artifacts(&self.shared, &self.cache, error);
    }
    fn event(&mut self, event: Event) -> Result<(), AgentError> {
        match event {
            Event::Request(Ok(request)) => {
                self.trace(
                    "in",
                    &json!({"id":request.id,"method":request.method,"params":request.params}),
                )?;
                if self.pending.len() >= 32 {
                    self.write(
                        request.id,
                        Err(AgentError::new("busy", "too many pending requests")),
                    )?;
                } else {
                    self.pending.push_back(request);
                }
            }
            Event::Request(Err(error)) => self.write(Value::Null, Err(error))?,
            Event::Bridge(bytes) => {
                self.shared.bridge_event(&bytes)?;
            }
            Event::Semantics(view, bytes) => {
                if self.shared.view(view).is_ok() {
                    let revision = self.cache.apply(view, &bytes)?;
                    self.notify(
                        "semantics.updated",
                        json!({"viewId":view,"revision":revision}),
                    )?;
                }
            }
            Event::Changed => {
                let windows = self.shared.windows.lock().clone();
                if windows != self.last_windows {
                    self.last_windows = windows.clone();
                    self.notify("windows.changed", windows)?;
                }
                let views = self.shared.windows.lock()["views"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                self.cache.retain_views(
                    &views
                        .iter()
                        .filter(|v| v["removing"] != true)
                        .filter_map(|v| v["viewId"].as_i64())
                        .collect(),
                );
                self.shared.frames.lock().retain(|id, _| {
                    views
                        .iter()
                        .any(|v| v["viewId"].as_i64() == Some(*id) && v["removing"] != true)
                });
            }
            Event::Disconnected | Event::Stop => self.disconnected = true,
        }
        Ok(())
    }
    fn drain(&mut self) -> Result<(), AgentError> {
        for _ in 0..64 {
            match self.incoming.try_recv() {
                Ok(event) => self.event(event)?,
                Err(_) => break,
            }
        }
        if let Some(error) = self.shared.fatal.lock().clone() {
            return Err(error);
        }
        if self.shared.stop.load(Ordering::Acquire) {
            return Err(AgentError::new("process_exit", "agent client disconnected"));
        }
        Ok(())
    }
    fn cancel(&mut self, id: &Value) -> Result<(), AgentError> {
        self.drain()?;
        if self.disconnected {
            return Err(AgentError::new("process_exit", "agent client disconnected"));
        }
        let index = self
            .pending
            .iter()
            .position(|r| r.method == "request.cancel" && r.params.get("id") == Some(id));
        if let Some(index) = index {
            let request = self.pending.remove(index).unwrap();
            request.check_params(&["id"])?;
            self.write(request.id, Ok(json!({"cancelled":id})))?;
            return Err(AgentError::new(
                "cancelled",
                "request cancelled; dispatched actions are not rolled back",
            ));
        }
        if !self.servicing {
            const QUICK: &[&str] = &[
                "session.health",
                "session.capabilities",
                "windows.list",
                "windows.inspect",
                "windows.focus",
                "windows.resize",
                "windows.close",
                "semantics.snapshot",
                "semantics.find",
                "semantics.perform",
                "input.tap",
                "input.pointer",
                "input.scroll",
                "input.key",
                "input.insertText",
                "input.replaceText",
                "input.release",
                "diagnostics.snapshot",
                "recording.status",
            ];
            if let Some(index) = self
                .pending
                .iter()
                .position(|r| QUICK.contains(&r.method.as_str()))
            {
                let request = self.pending.remove(index).unwrap();
                self.servicing = true;
                let result = self.handle(&request);
                self.servicing = false;
                self.action_event(&request.method, &result)?;
                self.write(request.id, result)?;
            }
        }
        Ok(())
    }
    fn wait_event(&mut self, deadline: Instant) -> Result<(), AgentError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AgentError::new("timeout", "request deadline exceeded"));
        }
        match self
            .incoming
            .recv_timeout(remaining.min(Duration::from_millis(20)))
        {
            Ok(event) => self.event(event),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
            Err(_) => Err(AgentError::new(
                "process_exit",
                "session worker disconnected",
            )),
        }
    }
    fn ui(&mut self, request: &Request, command: UiCommand, deadline: Instant) -> Reply {
        let (sender, receiver) = mpsc::sync_channel(1);
        let address = self.host;
        let shared = self.shared.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let abort = cancelled.clone();
        self.shared
            .events
            .send_event(HostEvent::MainThreadTask(Box::new(move || {
                if shared.stop.load(Ordering::Acquire)
                    || abort.load(Ordering::Acquire)
                    || Instant::now() >= deadline
                {
                    let _ = sender.send(Err(AgentError::new(
                        "timeout",
                        "UI request expired before dispatch",
                    )));
                    return;
                }
                // This closure runs only on the merged thread while host is alive.
                let host = unsafe { &*(address as *const VirtualHost) };
                let result = dispatch(host, &shared, command);
                let _ = sender.send(result);
            })))
            .map_err(|_| AgentError::new("process_exit", "UI dispatcher closed"))?;
        loop {
            match receiver.try_recv() {
                Ok(result) => return result,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(AgentError::new("process_exit", "UI response lost"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if let Err(error) = self
                .cancel(&request.id)
                .and_then(|_| self.wait_event(deadline))
            {
                cancelled.store(true, Ordering::Release);
                return Err(error);
            }
        }
    }
}

fn serve(
    listener: UnixListener,
    incoming: Receiver<Event>,
    shared: Arc<Shared>,
    host: usize,
    connection: &Mutex<Option<UnixStream>>,
) -> Result<(), AgentError> {
    let mut cache = SemanticsCache::new(shared.epoch.clone());
    let stream = loop {
        for _ in 0..64 {
            match incoming.try_recv() {
                Ok(Event::Semantics(view, bytes)) => {
                    cache.apply(view, &bytes)?;
                }
                Ok(Event::Bridge(bytes)) => {
                    shared.bridge_event(&bytes)?;
                }
                Ok(Event::Stop | Event::Disconnected) => {
                    if let Some(error) = shared.failure() {
                        save_failure_artifacts(&shared, &cache, &error);
                    }
                    return Ok(());
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if let Some(error) = shared.failure() {
            save_failure_artifacts(&shared, &cache, &error);
            return Err(error);
        }
        if shared.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Ok(parent) = std::env::var("FLUTTER_RUST_AGENT_PARENT_PID") {
            let parent = parent.parse::<u32>().map_err(|_| {
                AgentError::new("invalid_request", "invalid parent process identity")
            })?;
            if !std::path::Path::new(&format!("/proc/{parent}")).exists() {
                return Err(AgentError::new(
                    "process_exit",
                    "agent parent exited before connecting",
                ));
            }
        }
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(AgentError::new("process_exit", error.to_string())),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| AgentError::new("process_exit", e.to_string()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| AgentError::new("process_exit", e.to_string()))?;
    *connection.lock() = Some(
        stream
            .try_clone()
            .map_err(|e| AgentError::new("process_exit", e.to_string()))?,
    );
    let reader_stream = stream
        .try_clone()
        .map_err(|e| AgentError::new("process_exit", e.to_string()))?;
    let reader_shared = shared.clone();
    let reader = thread::Builder::new()
        .name("flutter-agent-reader".into())
        .spawn(move || read_requests(reader_stream, reader_shared))
        .map_err(|e| AgentError::new("process_exit", e.to_string()))?;
    let mut client = Client {
        stream,
        incoming,
        pending: VecDeque::new(),
        cache,
        shared: shared.clone(),
        host,
        disconnected: false,
        servicing: false,
        subscriptions: BTreeSet::new(),
        last_windows: shared.windows.lock().clone(),
        trace: std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(shared.artifacts.join("events.jsonl"))
            .map_err(|e| AgentError::new("artifact_error", e.to_string()))?,
        trace_bytes: 0,
        capture_id: 0,
        recordings: BTreeMap::new(),
    };
    let mut handshake = false;
    let result = (|| {
        while !shared.stop.load(Ordering::Acquire)
            && (!client.disconnected || !client.pending.is_empty())
        {
            if client.pending.is_empty() {
                match client.incoming.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => client.event(event)?,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
                continue;
            }
            let request = client.pending.pop_front().unwrap();
            let result = if !handshake && request.method != "session.handshake" {
                Err(AgentError::new(
                    "invalid_request",
                    "session.handshake must be first",
                ))
            } else {
                client.handle(&request)
            };
            if request.method == "session.handshake" && result.is_ok() {
                handshake = true;
            }
            client.action_event(&request.method, &result)?;
            let shutdown = request.method == "session.shutdown" && result.is_ok();
            client.write(request.id, result)?;
            if shutdown {
                break;
            }
        }
        Ok(())
    })();
    for recording in client.recordings.values() {
        recording.sink.stop("session_closed");
    }
    let mut recording_errors = Vec::new();
    for recording in client.recordings.values_mut() {
        if let Err(error) = recording.finish() {
            recording_errors.push(error);
        }
    }
    if !recording_errors.is_empty() {
        shared.fail(
            AgentError::new("recording_failed", "recording finalization failed")
                .with_data(json!(recording_errors)),
        );
    }
    let failure = result
        .as_ref()
        .err()
        .cloned()
        .or_else(|| shared.fatal.lock().clone());
    if let Some(error) = failure {
        let _ = client.notify("app.error", json!(error));
        client.failure_artifacts(&error);
    }
    shared.stop.store(true, Ordering::Release);
    let _ = client.stream.shutdown(Shutdown::Both);
    // A reader blocked by channel backpressure must be released before join.
    drop(client);
    let _ = reader.join();
    result
}

// UI commands contain owned, validated values. No protocol parsing or encoding
// is performed by their dispatcher.
enum UiCommand {
    Observation {
        view: i64,
        capture: bool,
    },
    Focus(i64),
    Resize(i64, f64, f64),
    Close(i64),
    Tap {
        view: i64,
        x: f64,
        y: f64,
        handle: Option<Value>,
    },
    Pointer {
        view: i64,
        x: f64,
        y: f64,
        phase: String,
    },
    Scroll {
        view: i64,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Key {
        view: i64,
        physical: u64,
        logical: u64,
        down: bool,
        character: String,
    },
    Text {
        view: i64,
        text: String,
        replace: bool,
    },
    Semantics {
        view: i64,
        node: i32,
        action: i32,
        args: Vec<u8>,
        handle: Value,
    },
    ReleaseInput,
    Bridge(Value),
    Recording {
        view: i64,
        sink: Option<Arc<recording::Sink>>,
    },
}

impl Client {
    fn capabilities(&self) -> Value {
        json!({"protocolVersion":PROTOCOL_VERSION,"epoch":self.shared.epoch,
            "engineRevision":self.shared.engine_revision,"shellAbi":flutter_shell_core::SHELL_ABI_VERSION,
            "buildMode":self.shared.build_mode,
            "adapter":self.shared.adapter,"windowDefaults":self.shared.windows.lock().clone(),
            "capabilities":{"windows":true,"semantics":true,"physicalInput":true,"textInput":true,
                "screenshots":true,"captureCorrelation":"uiBuildFrame","widgetBridge":!self.shared.bridge.lock().is_null(),
                "frameworkErrors":!self.shared.bridge.lock().is_null(),"assertions":true,
                "recording":true,"recordingEncoder":"optional FFmpeg ffv1","restart":false,"nativeDialogs":false}})
    }
    fn target(&self, request: &Request) -> Result<Option<Value>, AgentError> {
        let view = request.view_id()?;
        self.shared.view(view)?;
        if request.params.contains_key("handle") && request.params.contains_key("selector") {
            return Err(AgentError::new(
                "invalid_request",
                "choose handle or selector",
            ));
        }
        let node = if let Some(handle) = request.params.get("handle") {
            Some(self.cache.resolve_handle(handle)?)
        } else if let Some(selector) = request.params.get("selector") {
            Some(
                self.cache
                    .find_unique(view, &Selector::parse(selector.clone())?)?,
            )
        } else {
            None
        };
        if let Some(node) = &node {
            if node["handle"]["viewId"].as_i64() != Some(view) {
                return Err(AgentError::new(
                    "invalid_request",
                    "handle belongs to another view",
                ));
            }
            if node["hidden"] == true || node["enabled"] == false {
                return Err(AgentError::new(
                    "not_interactable",
                    "target is hidden or disabled",
                ));
            }
        }
        Ok(node)
    }
    fn handle(&mut self, request: &Request) -> Reply {
        self.drain()?;
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms()?);
        match request.method.as_str() {
            "session.handshake" => {
                request.check_params(&["protocolVersion"])?;
                if request
                    .params
                    .get("protocolVersion")
                    .and_then(Value::as_u64)
                    != Some(u64::from(PROTOCOL_VERSION))
                {
                    return Err(AgentError::new(
                        "version_mismatch",
                        "protocolVersion must be 1",
                    ));
                }
                Ok(self.capabilities())
            }
            "session.subscribe" | "session.unsubscribe" => {
                request.check_params(&["events"])?;
                let events = request
                    .params
                    .get("events")
                    .and_then(Value::as_array)
                    .ok_or_else(|| AgentError::new("invalid_request", "events must be an array"))?;
                for event in events {
                    let event = event
                        .as_str()
                        .filter(|v| {
                            [
                                "windows.changed",
                                "semantics.updated",
                                "capture.completed",
                                "action.dispatched",
                                "app.error",
                            ]
                            .contains(v)
                        })
                        .ok_or_else(|| AgentError::new("invalid_request", "unknown event"))?;
                    if request.method == "session.subscribe" {
                        self.subscriptions.insert(event.into());
                    } else {
                        self.subscriptions.remove(event);
                    }
                }
                Ok(json!({"events":self.subscriptions}))
            }
            "session.capabilities" => {
                request.check_params(&[])?;
                Ok(self.capabilities())
            }
            "session.health" => {
                request.check_params(&[])?;
                if !self.shared.bridge.lock().is_null() {
                    let query = Request {
                        id: request.id.clone(),
                        method: request.method.clone(),
                        params: serde_json::Map::from_iter([("query".into(), json!({}))]),
                    };
                    let health = self.bridge_query(&query, "diagnostics", deadline)?;
                    if health["frameworkHookInstalled"] != true
                        || health["platformHookInstalled"] != true
                        || health["errorCount"] != 0
                    {
                        return Err(AgentError::new(
                            "app_exception",
                            "Dart error coverage was replaced or reported errors",
                        )
                        .with_data(health));
                    }
                }
                if self
                    .recordings
                    .values()
                    .any(|r| r.status()["status"] == "failed")
                {
                    return Err(AgentError::new(
                        "recording_failed",
                        "one or more recordings failed",
                    ));
                }
                Ok(
                    json!({"healthy":true,"epoch":self.shared.epoch,"actionSequence":self.shared.sequence.load(Ordering::Acquire)}),
                )
            }
            "diagnostics.snapshot" => {
                request.check_params(&[])?;
                Ok(
                    json!({"epoch":self.shared.epoch,"logs":self.shared.logs.lock().clone(),"nativeSemanticsRevisions":self.shared.revisions.lock().clone(),"windows":self.shared.windows.lock().clone(),"actionSequence":self.shared.sequence.load(Ordering::Acquire)}),
                )
            }
            "session.shutdown" => {
                request.check_params(&[])?;
                Ok(json!({"shuttingDown":true}))
            }
            "request.cancel" => {
                request.check_params(&["id"])?;
                Err(AgentError::new("not_found", "request is not pending"))
            }
            "windows.list" => {
                request.check_params(&[])?;
                Ok(self.shared.windows.lock().clone())
            }
            "windows.inspect" => {
                request.check_params(&["viewId"])?;
                self.shared.view(request.view_id()?)
            }
            "windows.focus" => {
                request.check_params(&["viewId"])?;
                self.ui(request, UiCommand::Focus(request.view_id()?), deadline)
            }
            "windows.resize" => {
                request.check_params(&["viewId", "width", "height"])?;
                self.ui(
                    request,
                    UiCommand::Resize(
                        request.view_id()?,
                        request.number("width")?,
                        request.number("height")?,
                    ),
                    deadline,
                )
            }
            "windows.close" => {
                request.check_params(&["viewId"])?;
                self.ui(request, UiCommand::Close(request.view_id()?), deadline)
            }
            "semantics.snapshot" => {
                request.check_params(&["viewId"])?;
                let view = request.view_id()?;
                let window = self.shared.view(view)?;
                let mut snapshot = self.cache.snapshot(view);
                snapshot["pixelRatio"] = window["pixelRatio"].clone();
                snapshot["coordinateSpace"] = json!("physicalPixels");
                Ok(snapshot)
            }
            "semantics.find" => {
                request.check_params(&["viewId", "selector", "all"])?;
                let view = request.view_id()?;
                self.shared.view(view)?;
                let selector =
                    Selector::parse(request.params.get("selector").cloned().ok_or_else(|| {
                        AgentError::new("invalid_request", "selector is required")
                    })?)?;
                let all = request
                    .params
                    .get("all")
                    .map(|v| {
                        v.as_bool().ok_or_else(|| {
                            AgentError::new("invalid_request", "all must be boolean")
                        })
                    })
                    .transpose()?
                    .unwrap_or(false);
                if all {
                    Ok(
                        json!({"matches":self.cache.find_all(view,&selector),"revision":self.cache.revision(view)}),
                    )
                } else {
                    self.cache.find_unique(view, &selector)
                }
            }
            "semantics.perform" => {
                request.check_params(&["viewId", "selector", "handle", "action", "args"])?;
                let node = self.target(request)?.ok_or_else(|| {
                    AgentError::new("invalid_request", "semantics target is required")
                })?;
                let name = request
                    .params
                    .get("action")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::new("invalid_request", "action is required"))?;
                let action = semantic_action(name).ok_or_else(|| {
                    AgentError::new("unsupported_capability", "unknown semantics action")
                })?;
                if node["actions"].as_i64().unwrap_or(0) & i64::from(action) == 0 {
                    return Err(AgentError::new(
                        "unsupported_capability",
                        "node does not support this action",
                    ));
                }
                let args = request
                    .params
                    .get("args")
                    .map(flutter_shell_agent::encode_standard)
                    .transpose()?
                    .unwrap_or_default();
                self.ui(
                    request,
                    UiCommand::Semantics {
                        view: request.view_id()?,
                        node: node["id"].as_i64().unwrap() as i32,
                        action,
                        args,
                        handle: node["handle"].clone(),
                    },
                    deadline,
                )
            }
            "input.tap" => {
                request.check_params(&["viewId", "selector", "handle", "x", "y"])?;
                let node = self.target(request)?;
                let (x, y) = if let Some(node) = &node {
                    if request.params.contains_key("x") || request.params.contains_key("y") {
                        return Err(AgentError::new(
                            "invalid_request",
                            "coordinates cannot be combined with a semantics target",
                        ));
                    }
                    let bounds = node["viewBounds"]
                        .as_array()
                        .filter(|b| b.len() == 4)
                        .ok_or_else(|| {
                            AgentError::new("not_interactable", "target has no finite bounds")
                        })?;
                    let b = bounds
                        .iter()
                        .map(Value::as_f64)
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            AgentError::new("not_interactable", "target has invalid bounds")
                        })?;
                    let ratio = self.shared.view(request.view_id()?)?["pixelRatio"]
                        .as_f64()
                        .unwrap_or(1.);
                    ((b[0] + b[2]) / (2. * ratio), (b[1] + b[3]) / (2. * ratio))
                } else {
                    (request.number("x")?, request.number("y")?)
                };
                self.ui(
                    request,
                    UiCommand::Tap {
                        view: request.view_id()?,
                        x,
                        y,
                        handle: node.map(|n| n["handle"].clone()),
                    },
                    deadline,
                )
            }
            "input.pointer" => {
                request.check_params(&["viewId", "x", "y", "phase"])?;
                let phase = request
                    .params
                    .get("phase")
                    .and_then(Value::as_str)
                    .filter(|p| ["move", "down", "up", "cancel"].contains(p))
                    .ok_or_else(|| AgentError::new("invalid_request", "invalid pointer phase"))?
                    .to_owned();
                self.ui(
                    request,
                    UiCommand::Pointer {
                        view: request.view_id()?,
                        x: request.number("x")?,
                        y: request.number("y")?,
                        phase,
                    },
                    deadline,
                )
            }
            "input.scroll" => {
                request.check_params(&["viewId", "x", "y", "deltaX", "deltaY"])?;
                self.ui(
                    request,
                    UiCommand::Scroll {
                        view: request.view_id()?,
                        x: request.number("x")?,
                        y: request.number("y")?,
                        dx: request.number("deltaX")?,
                        dy: request.number("deltaY")?,
                    },
                    deadline,
                )
            }
            "input.key" => {
                request.check_params(&["viewId", "physical", "logical", "down", "character"])?;
                let key = |name: &str| {
                    request
                        .params
                        .get(name)
                        .and_then(Value::as_u64)
                        .filter(|v| *v > 0)
                        .ok_or_else(|| {
                            AgentError::new(
                                "invalid_request",
                                format!("{name} must be a positive key id"),
                            )
                        })
                };
                let character = request
                    .params
                    .get("character")
                    .map(|v| {
                        v.as_str().map(str::to_owned).ok_or_else(|| {
                            AgentError::new("invalid_request", "character must be a string")
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                if character.len() > FLUTTER_RUST_KEY_CHARACTER_CAPACITY {
                    return Err(AgentError::new(
                        "invalid_request",
                        "key character is too long",
                    ));
                }
                let down = request
                    .params
                    .get("down")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| AgentError::new("invalid_request", "down must be boolean"))?;
                self.ui(
                    request,
                    UiCommand::Key {
                        view: request.view_id()?,
                        physical: key("physical")?,
                        logical: key("logical")?,
                        down,
                        character,
                    },
                    deadline,
                )
            }
            "input.insertText" | "input.replaceText" => {
                request.check_params(&["viewId", "text"])?;
                let text = request
                    .params
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgentError::new("invalid_request", "text must be a string"))?
                    .to_owned();
                self.ui(
                    request,
                    UiCommand::Text {
                        view: request.view_id()?,
                        text,
                        replace: request.method == "input.replaceText",
                    },
                    deadline,
                )
            }
            "input.release" => {
                request.check_params(&[])?;
                self.ui(request, UiCommand::ReleaseInput, deadline)
            }
            "verify.assert" | "verify.wait" => self.verify(request, deadline),
            "recording.start" | "recording.stop" | "recording.status" => {
                self.recording(request, deadline)
            }
            "capture.screenshot" => self.capture(request, deadline),
            _ => Err(AgentError::new(
                "unsupported_capability",
                format!("unknown method {}", request.method),
            )),
        }
    }
    fn recording(&mut self, request: &Request, deadline: Instant) -> Reply {
        request.check_params(if request.method == "recording.start" {
            &["viewId", "name", "rate"]
        } else {
            &["name"]
        })?;
        let name = request
            .params
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 64
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            })
            .ok_or_else(|| {
                AgentError::new(
                    "invalid_request",
                    "recording name must be 1..=64 ASCII letters/digits/hyphens/underscores",
                )
            })?
            .to_owned();
        match request.method.as_str() {
            "recording.start" => {
                let view = request.view_id()?;
                self.shared.view(view)?;
                if self.recordings.len() >= 8
                    || self.recordings.contains_key(&name)
                    || self.shared.recordings.lock().contains_key(&view)
                {
                    return Err(AgentError::new(
                        "invalid_request",
                        "recording name/view already active or session limit (8) exceeded",
                    ));
                }
                let rate = request
                    .params
                    .get("rate")
                    .map(|v| {
                        v.as_u64().filter(|v| *v > 0 && *v <= 60).ok_or_else(|| {
                            AgentError::new("invalid_request", "rate must be 1..=60")
                        })
                    })
                    .transpose()?
                    .unwrap_or(15) as u32;
                let recording = recording::Recording::start(
                    self.shared.artifacts.clone(),
                    name.clone(),
                    view,
                    rate,
                    self.shared.start,
                )?;
                self.ui(
                    request,
                    UiCommand::Recording {
                        view,
                        sink: Some(recording.sink.clone()),
                    },
                    deadline,
                )?;
                let status = recording.status();
                self.recordings.insert(name, recording);
                Ok(status)
            }
            "recording.status" => self
                .recordings
                .get(&name)
                .map(recording::Recording::status)
                .ok_or_else(|| AgentError::new("not_found", "recording not found")),
            _ => {
                let recording = self
                    .recordings
                    .get(&name)
                    .ok_or_else(|| AgentError::new("not_found", "recording not found"))?;
                let view = recording.view;
                recording.sink.stop("requested");
                if self.shared.view(view).is_ok() {
                    self.ui(request, UiCommand::Recording { view, sink: None }, deadline)?;
                }
                while !self.recordings[&name].is_finished() {
                    self.cancel(&request.id)?;
                    self.wait_event(deadline)?;
                }
                self.recordings.get_mut(&name).unwrap().finish()
            }
        }
    }
    fn verify(&mut self, request: &Request, deadline: Instant) -> Reply {
        request.check_params(&[
            "kind", "viewId", "selector", "where", "count", "exists", "value", "enabled",
            "focused", "stableMs", "query", "equals",
        ])?;
        let kind = request
            .params
            .get("kind")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| AgentError::new("invalid_request", "kind must be a string"))
            })
            .transpose()?
            .unwrap_or("semantics");
        request.check_params(match kind {
            "semantics" => &[
                "kind", "viewId", "selector", "count", "exists", "value", "enabled", "focused",
                "stableMs",
            ],
            "windows" => &["kind", "where", "count", "exists", "stableMs"],
            "widget" => &["kind", "viewId", "query", "equals", "stableMs"],
            "app" => &["kind", "query", "equals", "stableMs"],
            _ => return Err(AgentError::new("invalid_request", "unknown assertion kind")),
        })?;
        let stable = request
            .params
            .get("stableMs")
            .map(|v| {
                v.as_u64()
                    .filter(|v| *v <= 120000)
                    .ok_or_else(|| AgentError::new("invalid_request", "invalid stableMs"))
            })
            .transpose()?
            .unwrap_or(0);
        if !["semantics", "windows", "widget", "app"].contains(&kind) {
            return Err(AgentError::new("invalid_request", "unknown assertion kind"));
        }
        let selector = if kind == "semantics" {
            Some(Selector::parse(
                request
                    .params
                    .get("selector")
                    .cloned()
                    .ok_or_else(|| AgentError::new("invalid_request", "selector required"))?,
            )?)
        } else {
            None
        };
        for field in ["exists", "enabled", "focused"] {
            if request.params.get(field).is_some_and(|v| !v.is_boolean()) {
                return Err(AgentError::new(
                    "invalid_request",
                    format!("{field} must be boolean"),
                ));
            }
        }
        if request
            .params
            .get("count")
            .is_some_and(|v| v.as_u64().is_none())
        {
            return Err(AgentError::new(
                "invalid_request",
                "count must be nonnegative integer",
            ));
        }
        let where_fields = request
            .params
            .get("where")
            .map(|v| {
                v.as_object()
                    .ok_or_else(|| AgentError::new("invalid_request", "where must be an object"))
            })
            .transpose()?;
        if let Some(fields) = where_fields {
            flutter_shell_agent::validate_window_filter(fields)?;
        }
        if kind == "widget" {
            request.view_id()?;
        }
        let mut stable_since = None;
        loop {
            self.cancel(&request.id)?;
            let observed = match kind {
                "semantics" => {
                    let view = request.view_id()?;
                    let nodes = if self.shared.view(view).is_ok() {
                        self.cache.find_all(view, selector.as_ref().unwrap())
                    } else {
                        Vec::new()
                    };
                    json!({"count":nodes.len(),"nodes":nodes,"semanticsRevision":self.cache.revision(view)})
                }
                "windows" => {
                    let windows = self.shared.windows.lock().clone();
                    let matches: Vec<_> = windows["views"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|v| {
                            where_fields.is_none_or(|fields| {
                                fields.iter().all(|(k, value)| v.get(k) == Some(value))
                            })
                        })
                        .cloned()
                        .collect();
                    json!({"count":matches.len(),"nodes":matches})
                }
                _ => self.bridge_query(request, kind, deadline)?,
            };
            let mut matched = true;
            if matches!(kind, "semantics" | "windows") {
                let count = observed["count"].as_u64().unwrap();
                if let Some(expected) = request.params.get("count") {
                    matched &= count == expected.as_u64().unwrap();
                }
                if let Some(exists) = request.params.get("exists") {
                    matched &= (count > 0) == exists.as_bool().unwrap();
                }
                if !request.params.contains_key("count") && !request.params.contains_key("exists") {
                    matched &= count == 1;
                }
                for field in ["value", "enabled", "focused"] {
                    if let Some(expected) = request.params.get(field) {
                        matched &= count == 1 && observed["nodes"][0].get(field) == Some(expected);
                    }
                }
            } else {
                let expected = request.params.get("equals").ok_or_else(|| {
                    AgentError::new("invalid_request", "bridge assertion needs equals")
                })?;
                matched = observed == *expected;
            }
            if matched {
                let since = *stable_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(stable) {
                    return Ok(
                        json!({"passed":true,"observed":observed,"actionSequence":self.shared.sequence.load(Ordering::Acquire),"epoch":self.shared.epoch}),
                    );
                }
            } else {
                stable_since = None;
            }
            if request.method == "verify.assert" || Instant::now() >= deadline {
                return Err(AgentError::new(if request.method=="verify.assert" {"assertion_failed"} else {"timeout"},"verification predicate did not match")
                    .with_data(json!({"predicate":request.params,"lastObserved":observed,"windows":self.shared.windows.lock().clone(),"logs":self.shared.logs.lock().clone()})));
            }
            if let Err(error) = self.wait_event(deadline) {
                return Err(error.with_data(json!({"predicate":request.params,"lastObserved":observed,"windows":self.shared.windows.lock().clone(),"logs":self.shared.logs.lock().clone()})));
            }
        }
    }
    fn bridge_query(&mut self, request: &Request, kind: &str, deadline: Instant) -> Reply {
        if self.shared.bridge.lock().is_null() {
            return Err(AgentError::new(
                "unsupported_capability",
                "optional Dart bridge is not attached",
            ));
        }
        let query = request
            .params
            .get("query")
            .cloned()
            .ok_or_else(|| AgentError::new("invalid_request", "bridge query required"))?;
        let id = self.shared.bridge_sequence.fetch_add(1, Ordering::AcqRel) + 1;
        self.ui(
            request,
            UiCommand::Bridge(
                json!({"id":id,"kind":kind,"query":query,"viewId":request.params.get("viewId")}),
            ),
            deadline,
        )?;
        loop {
            self.cancel(&request.id)?;
            if let Some(response) = self.shared.bridge_responses.lock().remove(&id) {
                if response.get("error").is_some() {
                    return Err(
                        AgentError::new("invalid_request", "Dart bridge query failed")
                            .with_data(response["error"].clone()),
                    );
                }
                return Ok(response["result"].clone());
            }
            self.wait_event(deadline)?;
        }
    }
    fn capture(&mut self, request: &Request, deadline: Instant) -> Reply {
        request.check_params(&["viewId", "mode", "afterFrame", "sizeGeneration", "name"])?;
        let view = request.view_id()?;
        self.shared.view(view)?;
        let mode = request
            .params
            .get("mode")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| AgentError::new("invalid_request", "mode must be a string"))
            })
            .transpose()?
            .unwrap_or("afterObservation");
        if !["latestCompleted", "afterFrame", "afterObservation"].contains(&mode) {
            return Err(AgentError::new("invalid_request", "invalid capture mode"));
        }
        let after = request
            .params
            .get("afterFrame")
            .map(|v| {
                v.as_u64().ok_or_else(|| {
                    AgentError::new("invalid_request", "afterFrame must be a frame id")
                })
            })
            .transpose()?;
        if mode == "afterFrame" && after.is_none() {
            return Err(AgentError::new(
                "invalid_request",
                "afterFrame mode requires afterFrame",
            ));
        }
        let generation = request
            .params
            .get("sizeGeneration")
            .map(|v| {
                v.as_u64().ok_or_else(|| {
                    AgentError::new("invalid_request", "sizeGeneration must be an integer")
                })
            })
            .transpose()?;
        if mode == "afterFrame" && generation.is_none() {
            return Err(AgentError::new(
                "invalid_request",
                "afterFrame requires sizeGeneration",
            ));
        }
        let cached = self.shared.frames.lock().get(&view).cloned();
        let boundary = if mode == "latestCompleted"
            && cached
                .as_ref()
                .is_some_and(|f| generation.is_none_or(|g| f.generation == g))
        {
            None
        } else {
            Some(
                self.ui(
                    request,
                    UiCommand::Observation {
                        view,
                        capture: true,
                    },
                    deadline,
                )?["uiFrameBoundary"]
                    .as_u64()
                    .unwrap(),
            )
        };
        let frame = loop {
            self.cancel(&request.id)?;
            self.shared.view(view)?;
            let frame = self.shared.frames.lock().get(&view).cloned();
            if let Some(frame) = frame {
                if let Some(generation) = generation
                    && frame.generation > generation
                {
                    return Err(AgentError::new(
                        "stale_handle",
                        "view size generation changed during capture",
                    ));
                }
                if boundary.is_none_or(|b| frame.ui_frame_number > b)
                    && after.is_none_or(|n| frame.frame > n)
                    && generation.is_none_or(|g| frame.generation == g)
                {
                    break frame;
                }
            }
            if let Err(error) = self.wait_event(deadline) {
                return Err(error.with_data(json!({"viewId":view,"lastFrame":self.shared.frames.lock().get(&view).map(|f|f.frame),"semanticsRevision":self.cache.revision(view)})));
            }
        };
        self.capture_id += 1;
        let name = request
            .params
            .get("name")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| AgentError::new("invalid_request", "name must be a filename"))
            })
            .transpose()?
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "capture-{}-view-{view}-generation-{}-frame-{}.png",
                    self.capture_id, frame.generation, frame.frame
                )
            });
        if name.len() > 128 || !name.ends_with(".png") || name.contains(['/', '\\']) || name == ".."
        {
            return Err(AgentError::new(
                "invalid_request",
                "capture name must be a PNG basename",
            ));
        }
        let path = self.shared.artifacts.join(&name);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if [
            "build-manifest",
            "failure",
            "observations",
            "result",
            "manifest",
            "sdk-manifest",
        ]
        .contains(&stem)
            || stem.starts_with("failure-view-")
            || path.with_extension("json").exists()
        {
            return Err(AgentError::new(
                "invalid_request",
                "capture name collides with reserved session metadata",
            ));
        }
        let retained: u64 = std::fs::read_dir(&self.shared.artifacts)
            .map_err(|e| AgentError::new("artifact_error", e.to_string()))?
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|s| s == "png"))
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        if self.capture_id > 256
            || retained.saturating_add(frame.rgba.len() as u64) > 128 * 1024 * 1024
        {
            return Err(AgentError::new(
                "artifact_error",
                "screenshot retention limit exceeded (256 captures / 128 MiB)",
            ));
        }
        write_png(&path, frame.width, frame.height, &frame.rgba)?;
        if let Err(error) = self.cancel(&request.id) {
            return Err(error.with_data(json!({"path":path,"frameId":frame.frame})));
        }
        if Instant::now() >= deadline {
            return Err(
                AgentError::new("timeout", "capture encoding exceeded request deadline")
                    .with_data(json!({"path":path,"frameId":frame.frame})),
            );
        }
        let metadata = json!({"path":path,"viewId":view,"frameId":frame.frame,"uiFrameNumber":frame.ui_frame_number,
            "uiFrameBoundary":boundary,"completedSessionMicros":frame.completed_at.saturating_duration_since(self.shared.start).as_micros() as u64,"readbackMicros":frame.readback_micros,"sizeGeneration":frame.generation,"width":frame.width,"height":frame.height,
            "pixelRatio":self.shared.view(view)?["pixelRatio"],"semanticsRevision":self.cache.revision(view),
            "actionSequence":self.shared.sequence.load(Ordering::Acquire),"epoch":self.shared.epoch,
            "colorConvention":"linear RGBA8, straight alpha, PNG gAMA=1.0"});
        std::fs::write(
            path.with_extension("json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .map_err(|e| AgentError::new("artifact_error", e.to_string()))?;
        self.notify("capture.completed", metadata.clone())?;
        Ok(metadata)
    }
}
fn semantic_action(name: &str) -> Option<i32> {
    let index = match name {
        "tap" => 0,
        "longPress" => 1,
        "scrollLeft" => 2,
        "scrollRight" => 3,
        "scrollUp" => 4,
        "scrollDown" => 5,
        "increase" => 6,
        "decrease" => 7,
        "showOnScreen" => 8,
        "moveCursorForwardByCharacter" => 9,
        "moveCursorBackwardByCharacter" => 10,
        "setSelection" => 11,
        "copy" => 12,
        "cut" => 13,
        "paste" => 14,
        "didGainAccessibilityFocus" => 15,
        "didLoseAccessibilityFocus" => 16,
        "customAction" => 17,
        "dismiss" => 18,
        "moveCursorForwardByWord" => 19,
        "moveCursorBackwardByWord" => 20,
        "setText" => 21,
        "focus" => 22,
        "scrollToOffset" => 23,
        "expand" => 24,
        "collapse" => 25,
        _ => return None,
    };
    Some(1 << index)
}

pub(super) struct InputState {
    pointers: BTreeMap<FlutterRustViewId, PointerState>,
    pointer_view: Option<FlutterRustViewId>,
    keyboard: KeyboardState,
    keyboard_view: Option<FlutterRustViewId>,
    pub(super) text: TextInputSession,
    pub(super) text_view: Option<FlutterRustViewId>,
}
impl InputState {
    pub(super) fn new() -> Self {
        Self {
            pointers: BTreeMap::new(),
            pointer_view: None,
            keyboard: KeyboardState::new(),
            keyboard_view: None,
            text: TextInputSession::default(),
            text_view: None,
        }
    }
}
fn validate_view(host: &VirtualHost, view: i64) -> Result<FlutterRustViewId, AgentError> {
    let id = FlutterRustViewId(view);
    let registry = host.registry.borrow();
    let target = registry
        .views
        .get(&id)
        .filter(|v| !v.removing)
        .ok_or_else(|| AgentError::new("view_closed", "view is closed"))?;
    if !target.ready {
        return Err(AgentError::new(
            "not_ready",
            "view creation has not completed",
        ));
    }
    if target.state.minimized != 0 || target.kind == NativeWindowKind::Tooltip {
        return Err(AgentError::new(
            "not_interactable",
            "view cannot receive input",
        ));
    }
    // A parented modal dialog blocks its owning chain, while unrelated regular
    // windows remain independently addressable.
    let mut chain = BTreeSet::new();
    let mut ancestor = Some(id);
    while let Some(current) = ancestor {
        chain.insert(current);
        ancestor = registry.views.get(&current).and_then(|v| v.parent);
    }
    for parent in &chain {
        if registry.views.iter().any(|(other, v)| {
            !chain.contains(other)
                && v.parent == Some(*parent)
                && v.kind == NativeWindowKind::Dialog
                && !v.removing
        }) {
            return Err(AgentError::new(
                "not_interactable",
                "view is blocked by a modal dialog",
            ));
        }
    }

    Ok(id)
}
fn validate_point(
    host: &VirtualHost,
    view: i64,
    x: f64,
    y: f64,
    allow_outside: bool,
) -> Result<FlutterRustViewId, AgentError> {
    let id = validate_view(host, view)?;
    let registry = host.registry.borrow();
    let v = &registry.views[&id];
    if !x.is_finite()
        || !y.is_finite()
        || (!allow_outside && (x < 0. || y < 0. || x >= v.bounds.2 || y >= v.bounds.3))
    {
        return Err(AgentError::new(
            "not_interactable",
            "point is outside the view",
        ));
    }
    Ok(id)
}
fn pointer_events(
    host: &VirtualHost,
    id: FlutterRustViewId,
    x: f64,
    y: f64,
    phase: &str,
) -> Result<Vec<FlutterRustPointerEvent>, AgentError> {
    let mut input = host.input.borrow_mut();
    let mut events = Vec::new();
    if input.pointer_view != Some(id) {
        if let Some(old) = input.pointer_view
            && let Some(pointer) = input.pointers.get_mut(&old)
        {
            if pointer.buttons != 0 {
                return Err(AgentError::new(
                    "invalid_request",
                    "pointer is captured by another view",
                ));
            }
            events.extend(pointer.left());
        }
        input.pointer_view = Some(id);
    }
    let pointer = input
        .pointers
        .entry(id)
        .or_insert_with(|| PointerState::for_view(id));
    events.extend(pointer.moved(x * host.ratio, y * host.ratio));
    match phase {
        "down" => events.extend(pointer.button(MouseButton::Left, ElementState::Pressed)),
        "up" => events.extend(pointer.button(MouseButton::Left, ElementState::Released)),
        "tap" => {
            if pointer.buttons != 0 {
                return Err(AgentError::new(
                    "invalid_request",
                    "release held pointer before tapping",
                ));
            }
            events.extend(pointer.button(MouseButton::Left, ElementState::Pressed));
            events.extend(pointer.button(MouseButton::Left, ElementState::Released));
        }
        "cancel" => {
            pointer.buttons = 0;
            events.push(pointer.mouse_event(
                FlutterRustPointerPhase::Cancel,
                FlutterRustPointerSignalKind::None,
                0.,
                0.,
            ));
            events.extend(pointer.left());
        }
        _ => {}
    }
    Ok(events)
}
pub(super) fn release_for_view(host: &VirtualHost, id: FlutterRustViewId) {
    let (pointers, keys) = {
        let mut input = host.input.borrow_mut();
        let mut pointers = Vec::new();
        if let Some(pointer) = input.pointers.get_mut(&id) {
            if pointer.buttons != 0 {
                pointer.buttons = 0;
                pointers.push(pointer.mouse_event(
                    FlutterRustPointerPhase::Cancel,
                    FlutterRustPointerSignalKind::None,
                    0.,
                    0.,
                ));
            }
            pointers.extend(pointer.left());
        }
        let keys = if input.keyboard_view == Some(id) {
            input.keyboard_view = None;
            input.keyboard.release_all()
        } else {
            Vec::new()
        };
        (pointers, keys)
    };
    for event in pointers {
        send_cpp_pointer_event(host.shell.get(), event);
    }
    for event in keys {
        send_key(host, event);
    }
}
pub(super) fn release_inputs(host: &VirtualHost) {
    let ids = host
        .registry
        .borrow()
        .views
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for id in ids {
        release_for_view(host, id);
    }
}
fn send_key(host: &VirtualHost, event: FlutterRustKeyEvent) {
    let raw = host.input.borrow().keyboard.raw_event_message(&event);
    send_cpp_key_event(host.shell.get(), event);
    send_cpp_platform_message(host.shell.get(), KEY_EVENT_CHANNEL, &raw);
}
fn dispatch(host: &VirtualHost, shared: &Shared, command: UiCommand) -> Reply {
    let result = match command {
        UiCommand::Recording { view, sink } => {
            let id = FlutterRustViewId(view);
            let registry = host.registry.borrow();
            let target = registry
                .views
                .get(&id)
                .filter(|v| !v.removing && v.ready)
                .ok_or_else(|| AgentError::new("view_closed", "recording view is closed"))?;
            if let Some(sink) = sink {
                unsafe extern "C" {
                    fn FlutterRustShellRequestObservationFrame(shell: *mut c_void) -> u64;
                }
                let boundary = unsafe { FlutterRustShellRequestObservationFrame(host.shell.get()) };
                sink.minimum_ui.store(boundary + 1, Ordering::Release);
                shared.recordings.lock().insert(view, sink);
                target._broker.set_recording_enabled(true);
            } else {
                shared.recordings.lock().remove(&view);
                target._broker.set_recording_enabled(false);
            }
            return Ok(json!({"viewId":view,"dispatched":true}));
        }
        UiCommand::Bridge(message) => {
            send_cpp_platform_message(
                host.shell.get(),
                b"flutter.rs.agent/query",
                &serde_json::to_vec(&message).unwrap(),
            );
            return Ok(json!({"dispatched":true}));
        }
        UiCommand::Observation { view, capture } => {
            let id = FlutterRustViewId(view);
            if !host
                .registry
                .borrow()
                .views
                .get(&id)
                .is_some_and(|v| !v.removing && v.ready)
            {
                return Err(AgentError::new(
                    "view_closed",
                    "view is closed or not ready",
                ));
            }
            unsafe extern "C" {
                fn FlutterRustShellRequestObservationFrame(shell: *mut c_void) -> u64;
            }
            let boundary = unsafe { FlutterRustShellRequestObservationFrame(host.shell.get()) };
            if capture {
                host.registry.borrow().views[&id]
                    ._broker
                    .request_capture_after(boundary);
            }
            return Ok(
                json!({"uiFrameBoundary":boundary,"actionSequence":shared.sequence.load(Ordering::Acquire)}),
            );
        }
        UiCommand::Focus(view) => {
            let id = validate_view(host, view)?;
            host.focus(id);
            json!({"viewId":view,"focused":true})
        }
        UiCommand::Resize(view, width, height) => {
            if !host.valid_size(width, height) {
                return Err(AgentError::new(
                    "invalid_request",
                    "dimensions exceed device limits",
                ));
            }
            let id = FlutterRustViewId(view);
            if host
                .registry
                .borrow()
                .views
                .get(&id)
                .is_none_or(|v| v.removing)
            {
                return Err(AgentError::new("view_closed", "view closed"));
            }
            host.resize(id, width, height);
            host.snapshot()
        }
        UiCommand::Close(view) => {
            if view == 0 {
                return Err(AgentError::new(
                    "invalid_request",
                    "use session.shutdown for the implicit view",
                ));
            }
            shared.view(view)?;
            host.notify(
                FlutterRustViewId(view),
                FlutterRustWindowEvent::CloseRequested,
            );
            json!({"viewId":view,"closeRequested":true})
        }
        UiCommand::Tap { view, x, y, handle } => {
            if let Some(handle) = handle {
                shared.assert_revision(&handle)?;
            }
            let id = validate_point(host, view, x, y, false)?;
            host.focus(id);
            for event in pointer_events(host, id, x, y, "tap")? {
                send_cpp_pointer_event(host.shell.get(), event);
            }
            json!({"viewId":view,"dispatched":true,"x":x,"y":y})
        }
        UiCommand::Pointer { view, x, y, phase } => {
            let id = FlutterRustViewId(view);
            let captured = host
                .input
                .borrow()
                .pointers
                .get(&id)
                .is_some_and(|p| p.buttons != 0);
            let id = validate_point(host, view, x, y, captured)?;
            if phase == "down" {
                host.focus(id);
            }
            for event in pointer_events(host, id, x, y, &phase)? {
                send_cpp_pointer_event(host.shell.get(), event);
            }
            json!({"viewId":view,"dispatched":true})
        }
        UiCommand::Scroll { view, x, y, dx, dy } => {
            let id = validate_point(host, view, x, y, false)?;
            host.focus(id);
            let mut events = pointer_events(host, id, x, y, "move")?;
            events.extend(
                host.input
                    .borrow_mut()
                    .pointers
                    .get_mut(&id)
                    .unwrap()
                    .scroll(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                        dx * host.ratio,
                        -dy * host.ratio,
                    ))),
            );
            for event in events {
                send_cpp_pointer_event(host.shell.get(), event);
            }
            json!({"viewId":view,"dispatched":true})
        }
        UiCommand::Key {
            view,
            physical,
            logical,
            down,
            character,
        } => {
            let id = validate_view(host, view)?;
            host.focus(id);
            let event = {
                let mut input = host.input.borrow_mut();
                let previous = input.keyboard.pressed.get(&physical).copied();
                let kind = if down {
                    if previous.is_some() {
                        FlutterRustKeyEventType::Repeat
                    } else {
                        FlutterRustKeyEventType::Down
                    }
                } else {
                    FlutterRustKeyEventType::Up
                };
                let logical = if down {
                    logical
                } else {
                    previous
                        .ok_or_else(|| AgentError::new("invalid_request", "key was not held"))?
                };
                if down {
                    input.keyboard.pressed.insert(physical, logical);
                    input.keyboard_view = Some(id);
                } else {
                    input.keyboard.pressed.remove(&physical);
                }
                let mut modifiers = ModifiersState::empty();
                for key in input.keyboard.pressed.keys() {
                    match key {
                        0x700e1 | 0x700e5 => modifiers.insert(ModifiersState::SHIFT),
                        0x700e0 | 0x700e4 => modifiers.insert(ModifiersState::CONTROL),
                        0x700e2 | 0x700e6 => modifiers.insert(ModifiersState::ALT),
                        0x700e3 | 0x700e7 => modifiers.insert(ModifiersState::META),
                        _ => {}
                    }
                }
                input.keyboard.modifiers = modifiers;
                let mut event = FlutterRustKeyEvent {
                    timestamp_micros: shared.start.elapsed().as_micros().min(u64::MAX as u128)
                        as u64,
                    event_type: kind as u32,
                    physical,
                    logical,
                    synthesized: 0,
                    character_length: character.len() as u32,
                    character: [0; FLUTTER_RUST_KEY_CHARACTER_CAPACITY],
                };
                event.character[..character.len()].copy_from_slice(character.as_bytes());
                event
            };
            send_key(host, event);
            if down && !character.is_empty() {
                let message = {
                    let mut input = host.input.borrow_mut();
                    if input.text_view == Some(id)
                        && !input.keyboard.modifiers.intersects(
                            ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::META,
                        )
                    {
                        input.text.keyboard_text(&character)
                    } else {
                        None
                    }
                };
                if let Some(message) = message {
                    send_cpp_platform_message(host.shell.get(), TEXT_INPUT_CHANNEL, &message);
                }
            }
            json!({"viewId":view,"dispatched":true})
        }
        UiCommand::Text {
            view,
            text,
            replace,
        } => {
            let id = validate_view(host, view)?;
            if host.registry.borrow().views[&id].state.focused == 0 {
                return Err(AgentError::new("not_interactable", "view is not focused"));
            }
            let message = {
                let mut input = host.input.borrow_mut();
                if input.text_view != Some(id) {
                    return Err(AgentError::new(
                        "not_ready",
                        "target view has no active editing client",
                    ));
                }
                let client = input
                    .text
                    .active_client
                    .ok_or_else(|| AgentError::new("not_ready", "no active editing client"))?;
                let previous = input.text.editing_state.clone();
                if replace {
                    let selection = text.encode_utf16().count() as i64;
                    input.text.editing_state = TextEditingState {
                        text,
                        selection_base: selection,
                        selection_extent: selection,
                        composing_base: -1,
                        composing_extent: -1,
                        ..Default::default()
                    };
                } else if !input.text.editing_state.replace_for_ime(&text, false, None) {
                    return Err(AgentError::new(
                        "invalid_request",
                        "invalid UTF-16 editing selection",
                    ));
                }
                input.text.encode_update(client, &previous)
            };
            send_cpp_platform_message(host.shell.get(), TEXT_INPUT_CHANNEL, &message);
            json!({"viewId":view,"dispatched":true,"replacement":replace})
        }
        UiCommand::Semantics {
            view,
            node,
            action,
            args,
            handle,
        } => {
            shared.assert_revision(&handle)?;
            shared.view(view)?;
            unsafe extern "C" {
                fn FlutterRustShellPerformSemanticsAction(
                    shell: *mut c_void,
                    view: i64,
                    node: i32,
                    action: i32,
                    args: *const u8,
                    length: u64,
                ) -> i32;
            }
            if unsafe {
                FlutterRustShellPerformSemanticsAction(
                    host.shell.get(),
                    view,
                    node,
                    action,
                    args.as_ptr(),
                    args.len() as u64,
                )
            } == 0
            {
                return Err(AgentError::new(
                    "unsupported_capability",
                    "semantics action was rejected",
                ));
            }
            json!({"viewId":view,"nodeId":node,"dispatched":true})
        }
        UiCommand::ReleaseInput => {
            release_inputs(host);
            json!({"released":true})
        }
    };
    let sequence = shared.sequence.fetch_add(1, Ordering::AcqRel) + 1;
    let mut result = result;
    result["actionSequence"] = json!(sequence);
    Ok(result)
}
