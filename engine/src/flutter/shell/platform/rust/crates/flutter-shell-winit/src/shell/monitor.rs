// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Opt-in observations for a normal Linux window. The app owns this endpoint;
//! disconnects and malformed clients never terminate it or dispatch UI input.
use super::*;
use flutter_shell_agent::{AgentError, Request, SemanticsCache, response};
use serde_json::{Value, json};
use std::io::{BufReader, Read, Write};
use std::os::unix::{
    fs::{MetadataExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::thread::{self, JoinHandle};

const HISTORY: usize = 256;
const HISTORY_BYTES: usize = 1024 * 1024;
struct State {
    epoch: String,
    mode: String,
    revision: String,
    windows: Value,
    semantics: SemanticsCache,
    events: VecDeque<Value>,
    event_bytes: usize,
    sequence: u64,
    errors: u64,
    bridge: bool,
    heartbeat: Instant,
}
impl State {
    fn event(&mut self, kind: &str, data: Value) {
        self.sequence += 1;
        let event = json!({"sequence":self.sequence,"type":kind,"data":data});
        self.event_bytes += event.to_string().len();
        self.events.push_back(event);
        while self.events.len() > HISTORY || self.event_bytes > HISTORY_BYTES {
            if let Some(old) = self.events.pop_front() {
                self.event_bytes -= old.to_string().len();
            }
        }
    }
    fn call(&self, request: &Request) -> Result<Value, AgentError> {
        request.timeout_ms()?;
        match request.method.as_str() {
            "session.handshake" => {
                request.check_params(&["protocolVersion"])?;
                if request.params.get("protocolVersion") != Some(&json!(1)) {
                    return Err(AgentError::new(
                        "invalid_request",
                        "protocolVersion must be 1",
                    ));
                }
                Ok(json!({"protocolVersion":1,"epoch":self.epoch,"monitor":true}))
            }
            "session.capabilities" => {
                request.check_params(&[])?;
                Ok(
                    json!({"epoch":self.epoch,"buildMode":self.mode,"engineRevision":self.revision,
                    "monitor":true,"readOnly":true,"frameworkErrors":self.bridge,
                    "methods":["session.health","diagnostics.snapshot","windows.list","semantics.snapshot","monitor.events"]}),
                )
            }
            "session.health" => {
                request.check_params(&[])?;
                let age = self.heartbeat.elapsed().as_millis() as u64;
                Ok(
                    json!({"epoch":self.epoch,"healthy":self.errors==0 && age<5000,
                    "errorCount":self.errors,"frameworkErrors":self.bridge,"uiHeartbeatAgeMs":age}),
                )
            }
            "windows.list" => {
                request.check_params(&[])?;
                Ok(self.windows.clone())
            }
            "semantics.snapshot" => {
                request.check_params(&["viewId"])?;
                let view = request.view_id()?;
                if !self.windows["views"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|v| v["viewId"] == view)
                {
                    return Err(AgentError::new("view_closed", "view is not live"));
                }
                Ok(self.semantics.snapshot(view))
            }
            "diagnostics.snapshot" => {
                request.check_params(&[])?;
                Ok(
                    json!({"epoch":self.epoch,"errorCount":self.errors,"windows":self.windows,
                    "events":self.events,"nextSequence":self.sequence}),
                )
            }
            "monitor.events" => {
                request.check_params(&["after"])?;
                let after = request
                    .params
                    .get("after")
                    .map(Value::as_u64)
                    .unwrap_or(Some(0))
                    .ok_or_else(|| {
                        AgentError::new("invalid_request", "after must be a nonnegative integer")
                    })?;
                let oldest = self
                    .events
                    .front()
                    .and_then(|e| e["sequence"].as_u64())
                    .unwrap_or(self.sequence + 1);
                Ok(json!({"epoch":self.epoch,"nextSequence":self.sequence,
                    "gap":after.saturating_add(1)<oldest,"events":self.events.iter()
                    .filter(|e|e["sequence"].as_u64().unwrap()>after).collect::<Vec<_>>()}))
            }
            _ => Err(AgentError::new(
                "unsupported_capability",
                "attached monitors expose observations only",
            )),
        }
    }
}

pub(super) struct Session {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    path: PathBuf,
}
impl Session {
    pub(super) fn start(shell: *mut c_void, path: PathBuf) -> Result<Self, String> {
        let parent = path
            .parent()
            .ok_or("socket needs a private parent directory")?;
        let metadata = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        let uid = std::fs::metadata("/proc/self")
            .map_err(|e| e.to_string())?
            .uid();
        if !path.is_absolute()
            || !metadata.is_dir()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
        {
            return Err("monitor socket must be in an absolute, user-owned 0700 directory".into());
        }
        let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        unsafe extern "C" {
            fn FlutterRustShellGetEngineRevision() -> *const std::ffi::c_char;
            fn FlutterRustShellGetBuildMode() -> *const std::ffi::c_char;
        }
        let epoch = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let state = Arc::new(Mutex::new(State {
            semantics: SemanticsCache::new(epoch.clone()),
            epoch,
            mode: unsafe { std::ffi::CStr::from_ptr(FlutterRustShellGetBuildMode()) }
                .to_string_lossy()
                .into_owned(),
            revision: unsafe { std::ffi::CStr::from_ptr(FlutterRustShellGetEngineRevision()) }
                .to_string_lossy()
                .into_owned(),
            windows: json!({"views":[]}),
            events: VecDeque::new(),
            event_bytes: 0,
            sequence: 0,
            errors: 0,
            bridge: false,
            heartbeat: Instant::now(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_state = state.clone();
        let thread_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("flutter-monitor".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = serve(stream, &thread_state, &thread_stop);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(e) => {
                            log::error!("monitor accept: {e}");
                            break;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        unsafe {
            FlutterRustShellSetSemanticsCallbacks(
                shell,
                flutter_shell_core::FlutterRustSemanticsCallbacks {
                    user_data: Arc::as_ptr(&state).cast_mut().cast(),
                    update: Some(semantics),
                },
            );
            FlutterRustShellSetDiagnosticCallbacks(
                shell,
                flutter_shell_core::FlutterRustDiagnosticCallbacks {
                    user_data: Arc::as_ptr(&state).cast_mut().cast(),
                    report: Some(diagnostic),
                },
            );
            FlutterRustShellSetSemanticsEnabled(shell, 1);
        }
        Ok(Self {
            state,
            stop,
            worker: Some(worker),
            path,
        })
    }
    pub(super) fn publish(&self, windows: &WindowRegistry, inbox: &TextInputInbox) {
        let mut views: Vec<_> = windows
            .views
            .values()
            .map(|v| {
                json!({"viewId":v.view_id.0,
            "title":v.window.title(),"focused":v.focused,"visible":v.visible,
            "width":v.window.surface_size().width,"height":v.window.surface_size().height})
            })
            .collect();
        views.sort_by_key(|v| v["viewId"].as_i64());
        let snapshot = json!({"views":views});
        let mut state = self.state.lock();
        if state.windows != snapshot {
            let closed: Vec<_> = state.windows["views"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["viewId"].as_i64())
                .filter(|id| {
                    !snapshot["views"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v["viewId"] == *id)
                })
                .collect();
            for view in closed {
                state.semantics.remove_view(view);
            }
            state.windows = snapshot.clone();
            state.event("windows.changed", snapshot);
        }
        state.heartbeat = Instant::now();
        for message in std::mem::take(&mut *inbox.agent_messages.lock()) {
            if let Ok(value) = serde_json::from_slice::<Value>(&message) {
                if value["type"] == "ready" {
                    state.bridge = true;
                }
                if value["type"] == "error" {
                    state.errors += 1;
                }
                state.event("bridge", value);
            }
        }
    }
    pub(super) fn stop(&mut self, shell: *mut c_void) {
        unsafe {
            FlutterRustShellSetSemanticsCallbacks(
                shell,
                flutter_shell_core::FlutterRustSemanticsCallbacks {
                    user_data: std::ptr::null_mut(),
                    update: None,
                },
            );
            FlutterRustShellSetDiagnosticCallbacks(
                shell,
                flutter_shell_core::FlutterRustDiagnosticCallbacks {
                    user_data: std::ptr::null_mut(),
                    report: None,
                },
            );
        }
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}
unsafe extern "C" {
    fn FlutterRustShellSetSemanticsCallbacks(
        shell: *mut c_void,
        callbacks: flutter_shell_core::FlutterRustSemanticsCallbacks,
    );
    fn FlutterRustShellSetDiagnosticCallbacks(
        shell: *mut c_void,
        callbacks: flutter_shell_core::FlutterRustDiagnosticCallbacks,
    );
    fn FlutterRustShellSetSemanticsEnabled(shell: *mut c_void, enabled: i32);
}
extern "C" fn semantics(data: *mut c_void, view: i64, bytes: *const u8, length: u64) {
    if length > 16 * 1024 * 1024 || (bytes.is_null() && length != 0) {
        return;
    }
    // Engine borrows the callback data until callbacks are cleared in stop().
    let state = unsafe { &*data.cast::<Mutex<State>>() };
    let bytes = if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, length as usize) }
    };
    let mut state = state.lock();
    if let Err(error) = state.semantics.apply(view, bytes) {
        state.event("monitor.error", json!(error));
    } else {
        state.event("semantics.updated", json!({"viewId":view}));
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
    if (message.is_null() && length != 0) || (stack.is_null() && stack_length != 0) {
        return;
    }
    let text = |ptr, len| {
        if len == 0 {
            String::new()
        } else {
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr, len as usize) })
                .into_owned()
        }
    };
    let mut state = unsafe { &*data.cast::<Mutex<State>>() }.lock();
    if kind == 1 {
        state.errors += 1;
    }
    state.event(
        if kind == 1 { "app.error" } else { "app.log" },
        json!({"message":text(message,length.min(65536)),"stack":text(stack,stack_length.min(65536))}),
    );
}
fn serve(stream: UnixStream, state: &Mutex<State>, stop: &AtomicBool) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut buffer = Vec::new();
    let mut handshake = false;
    while !stop.load(Ordering::Acquire) {
        // Preserve partial messages across timeouts, with a strict memory bound.
        let mut byte = [0];
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] != b'\n' {
                    buffer.push(byte[0]);
                    if buffer.len() > flutter_shell_agent::MAX_MESSAGE_BYTES {
                        break;
                    }
                    continue;
                }
                let result = Request::parse(&buffer);
                buffer.clear();
                let output = match result {
                    Ok(request) => {
                        let result = if !handshake && request.method != "session.handshake" {
                            Err(AgentError::new("invalid_request", "handshake required"))
                        } else {
                            state.lock().call(&request)
                        };
                        if request.method == "session.handshake" && result.is_ok() {
                            handshake = true;
                        }
                        response(request.id, result)
                    }
                    Err(error) => response(Value::Null, Err(error)),
                };
                serde_json::to_writer(&mut writer, &output)?;
                writer.write_all(b"\n")?;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        State {
            epoch: "test".into(),
            mode: "release".into(),
            revision: "test".into(),
            windows: json!({"views":[]}),
            semantics: SemanticsCache::new("test".into()),
            events: VecDeque::new(),
            event_bytes: 0,
            sequence: 0,
            errors: 0,
            bridge: false,
            heartbeat: Instant::now(),
        }
    }
    fn request(method: &str, params: Value) -> Request {
        Request::parse(
            &serde_json::to_vec(&json!({"id":1,"method":method,"params":params})).unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn history_is_bounded_and_reports_lost_events() {
        let mut state = state();
        for i in 0..300 {
            state.event("test", json!(i));
        }
        let result = state
            .call(&request("monitor.events", json!({"after":0})))
            .unwrap();
        assert_eq!(result["gap"], true);
        assert_eq!(result["nextSequence"], 300);
        assert_eq!(result["events"].as_array().unwrap().len(), 256);
        let result = state
            .call(&request("monitor.events", json!({"after":299})))
            .unwrap();
        assert_eq!(result["gap"], false);
        assert_eq!(result["events"].as_array().unwrap().len(), 1);
        state.event("oversized", json!("x".repeat(HISTORY_BYTES + 1)));
        assert!(state.event_bytes <= HISTORY_BYTES);
        assert!(
            state
                .call(&request("monitor.events", json!({"after":-1})))
                .is_err()
        );
    }
    #[test]
    fn health_observes_stalls_and_errors_without_stopping_app() {
        let mut state = state();
        assert_eq!(
            state.call(&request("session.health", json!({}))).unwrap()["healthy"],
            true
        );
        state.heartbeat = Instant::now() - Duration::from_secs(6);
        assert_eq!(
            state.call(&request("session.health", json!({}))).unwrap()["healthy"],
            false
        );
        state.heartbeat = Instant::now();
        state.errors = 1;
        assert_eq!(
            state.call(&request("session.health", json!({}))).unwrap()["healthy"],
            false
        );
        assert!(state.call(&request("session.shutdown", json!({}))).is_err());
        assert!(
            state
                .call(&request("input.tap", json!({"viewId":0,"x":0,"y":0})))
                .is_err()
        );
    }
    #[test]
    fn disconnect_partial_messages_and_malformed_clients_preserve_state() {
        let shared = Arc::new(Mutex::new(state()));
        shared.lock().event("before.attach", json!({}));
        for first in [true, false] {
            let (server, mut client) = UnixStream::pair().unwrap();
            let state = shared.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let worker = thread::spawn(move || serve(server, &state, &thread_stop).unwrap());
            client.write_all(b"broken\n").unwrap();
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let mut line = String::new();
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
            assert!(
                serde_json::from_str::<Value>(&line)
                    .unwrap()
                    .get("error")
                    .is_some()
            );
            client
                .write_all(b"{\"id\":1,\"method\":\"session.handshake\",")
                .unwrap();
            thread::sleep(Duration::from_millis(250));
            client
                .write_all(b"\"params\":{\"protocolVersion\":1}}\n")
                .unwrap();
            line.clear();
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["result"]["monitor"],
                true
            );
            if first {
                drop(client);
                drop(reader);
            } else {
                client.write_all(b"partial").unwrap();
                stop.store(true, Ordering::Release);
            }
            worker.join().unwrap();
        }
        assert_eq!(shared.lock().sequence, 1);
    }
}
