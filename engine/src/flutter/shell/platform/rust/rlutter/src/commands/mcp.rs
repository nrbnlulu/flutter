//! MCP stdio adapter. The native agent remains the authority for app operations.
use crate::cli::AgentArgs;
use anyhow::{Result, bail};
#[cfg(target_os = "linux")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use std::io::{BufRead, Read, Write};
use std::path::Path;

const MAX_MESSAGE: u64 = 1024 * 1024;

// Native method, description, required fields, optional fields. Parameters keep
// their native names so protocol documentation and artifacts remain applicable.
const TOOLS: &[(&str, &str, &str, &str)] = &[
    (
        "session.capabilities",
        "Build and launch if needed; report actual engine, GPU and bridge capabilities.",
        "",
        "",
    ),
    (
        "session.health",
        "Check native and framework errors; requires the Dart bridge for framework coverage.",
        "",
        "",
    ),
    (
        "session.shutdown",
        "Stop the owned app. This session cannot be restarted.",
        "",
        "",
    ),
    (
        "windows.list",
        "Discover real view IDs before sending input or querying semantics.",
        "",
        "",
    ),
    ("windows.inspect", "Inspect a window.", "viewId", ""),
    ("windows.focus", "Focus a window.", "viewId", ""),
    ("windows.close", "Close a window.", "viewId", ""),
    (
        "windows.resize",
        "Resize a window in logical pixels.",
        "viewId width height",
        "",
    ),
    (
        "semantics.snapshot",
        "Read the semantics tree; obscured values are redacted.",
        "viewId",
        "",
    ),
    (
        "semantics.find",
        "Find controls by identifier, label, role, enabled or focused. Prefer stable identifiers.",
        "viewId selector",
        "all",
    ),
    (
        "semantics.perform",
        "Perform a semantic action using selector or handle. Observe the result separately.",
        "viewId action",
        "selector handle args",
    ),
    (
        "input.tap",
        "Tap a selector/handle or logical x/y. Acknowledgement proves dispatch only; use verify.wait afterward.",
        "viewId",
        "selector handle x y",
    ),
    (
        "input.pointer",
        "Dispatch a pointer move/down/up/cancel in logical pixels.",
        "viewId phase x y",
        "",
    ),
    (
        "input.scroll",
        "Scroll at logical x/y.",
        "viewId x y deltaX deltaY",
        "",
    ),
    (
        "input.key",
        "Dispatch a physical/logical key event.",
        "viewId physical logical down",
        "character",
    ),
    (
        "input.insertText",
        "Insert text through the active editing client.",
        "viewId text",
        "",
    ),
    (
        "input.replaceText",
        "Replace active editing text using UTF-16 selection.",
        "viewId text",
        "",
    ),
    ("input.release", "Release held input.", "", ""),
    ("diagnostics.snapshot", "Read recent diagnostics.", "", ""),
    (
        "verify.assert",
        "Assert a semantics/windows/widget/app predicate once. Widget/app checks require FlutterRustAgent.install().",
        "",
        "kind viewId selector where query equals count exists value enabled focused stableMs",
    ),
    (
        "verify.wait",
        "Wait for a semantics/windows/widget/app predicate, bounded by timeoutMs. Use after actions; mutations are never retried.",
        "",
        "kind viewId selector where query equals count exists value enabled focused stableMs",
    ),
    (
        "capture.screenshot",
        "Return a fresh PNG image and frame metadata. Default afterObservation requires a newer UI build; afterFrame also needs sizeGeneration.",
        "viewId",
        "mode afterFrame sizeGeneration name",
    ),
    (
        "recording.start",
        "Start a lossless per-view recording (requires FFmpeg).",
        "viewId name",
        "rate",
    ),
    (
        "recording.stop",
        "Finalize recording evidence; consider timeoutMs=12000.",
        "name",
        "",
    ),
    (
        "recording.status",
        "Inspect recording state and partial evidence.",
        "name",
        "",
    ),
];
fn name(method: &str) -> String {
    format!("rlutter_{}", method.replace('.', "_"))
}
fn property(field: &str) -> Value {
    match field {
        "viewId" | "physical" | "logical" | "afterFrame" | "sizeGeneration" | "count"
        | "stableMs" | "rate" => json!({"type":"integer","minimum":0}),
        "width" | "height" | "x" | "y" | "deltaX" | "deltaY" => json!({"type":"number"}),
        "all" | "down" | "exists" | "enabled" | "focused" => json!({"type":"boolean"}),
        "selector" => {
            json!({"type":"object","properties":{"identifier":{"type":"string"},"label":{"type":"string"},"role":{"type":"string"},"enabled":{"type":"boolean"},"focused":{"type":"boolean"}},"additionalProperties":false})
        }
        "handle" | "where" | "query" => json!({"type":"object"}),
        "equals" | "args" => json!({}),
        "phase" => json!({"enum":["move","down","up","cancel"]}),
        "kind" => json!({"enum":["semantics","windows","widget","app"]}),
        "mode" => json!({"enum":["afterObservation","afterFrame","latestCompleted"]}),
        _ => json!({"type":"string"}),
    }
}
fn tools() -> Value {
    json!({"tools":TOOLS.iter().map(|(method, description, required, optional)| {
        let mut properties = serde_json::Map::new();
        for field in required.split_whitespace().chain(optional.split_whitespace()) {
            properties.insert(field.into(), property(field));
        }
        properties.insert("timeoutMs".into(), json!({"type":"integer","minimum":1,"maximum":120000,"default":5000}));
        json!({"name":name(method),"description":description,"inputSchema":{"type":"object","properties":properties,"required":required.split_whitespace().collect::<Vec<_>>(),"additionalProperties":false}})
    }).collect::<Vec<_>>()})
}
fn tool_result(response: Value) -> Value {
    let failed = response.get("error").is_some();
    let data = if failed {
        &response["error"]
    } else {
        &response["result"]
    };
    json!({"content":[{"type":"text","text":data.to_string()}],"isError":failed})
}

trait Backend: Send {
    fn call(&mut self, method: &str, params: Value) -> Result<Value>;
    fn finish(&mut self) -> Result<()>;
}
#[derive(Clone)]
struct McpServer {
    backend: std::sync::Arc<std::sync::Mutex<Box<dyn Backend>>>,
    // Keep one native operation in flight, including after MCP cancellation.
    available: std::sync::Arc<tokio::sync::Semaphore>,
}
impl McpServer {
    fn new(backend: impl Backend + 'static) -> Self {
        Self {
            backend: std::sync::Arc::new(std::sync::Mutex::new(Box::new(backend))),
            available: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}
impl rmcp::ServerHandler for McpServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("rlutter", env!("CARGO_PKG_VERSION")))
            .with_instructions("Use rlutter_windows_list to discover view IDs. Prefer identifier selectors. Observe actions with rlutter_verify_wait; dispatch is not success. Never retry mutations after timeout or cancellation. The app builds lazily on the first tool call. Only one app operation may run at a time. Finish with rlutter_session_shutdown.")
    }
    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        tools()["tools"]
            .as_array()?
            .iter()
            .find(|tool| tool["name"] == name)
            .map(|tool| serde_json::from_value(tool.clone()).expect("static tool schema"))
    }
    async fn list_tools(
        &self,
        request: Option<rmcp::model::PaginatedRequestParams>,
        _: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> std::result::Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        if request.is_some_and(|r| r.cursor.is_some()) {
            return Err(rmcp::ErrorData::invalid_params("Invalid cursor", None));
        }
        Ok(serde_json::from_value(tools()).expect("static tool catalogue"))
    }
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> std::result::Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        let (method, _, required, optional) = TOOLS
            .iter()
            .find(|(method, ..)| request.name == name(method))
            .ok_or_else(|| rmcp::ErrorData::invalid_params("Unknown tool", None))?;
        let args = request.arguments.unwrap_or_default();
        if required.split_whitespace().any(|k| !args.contains_key(k))
            || args.keys().any(|k| {
                k != "timeoutMs"
                    && !required
                        .split_whitespace()
                        .chain(optional.split_whitespace())
                        .any(|v| v == k)
            })
        {
            return Ok(
                rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(
                    "Missing or unknown tool arguments",
                )])
                .into(),
            );
        }
        let permit = match self.available.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Ok(rmcp::model::CallToolResult::error(vec![
                rmcp::model::ContentBlock::text(
                    "App operation still running; observe its result before issuing another action",
                ),
            ])
            .into()),
        };
        let backend = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            backend
                .lock()
                .map_err(|_| anyhow::anyhow!("session worker panicked"))?
                .call(method, Value::Object(args))
        })
        .await;
        let value = match result {
            Ok(Ok(value)) => value,
            Ok(Err(err)) => {
                tool_result(json!({"error":{"code":"session_failed","message":format!("{err:#}")}}))
            }
            Err(err) => {
                tool_result(json!({"error":{"code":"session_failed","message":err.to_string()}}))
            }
        };
        let result: rmcp::model::CallToolResult = serde_json::from_value(value)
            .map_err(|err| rmcp::ErrorData::internal_error(err.to_string(), None))?;
        Ok(result.into())
    }
}

pub fn run(args: &AgentArgs, flutter_root: Option<&Path>, engine_src: Option<&Path>) -> Result<()> {
    let server = McpServer::new(Native::new(
        args.clone(),
        flutter_root.map(Path::to_path_buf),
        engine_src.map(Path::to_path_buf),
    ));
    tokio::runtime::Runtime::new()?.block_on(serve_io(
        server,
        tokio::io::stdin(),
        tokio::io::stdout(),
    ))
}
async fn serve_io(
    server: McpServer,
    input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
) -> Result<()> {
    use rmcp::ServiceExt;
    let transport = rmcp::transport::async_rw::AsyncRwTransport::new(input, output)
        .with_max_line_length(MAX_MESSAGE as usize);
    let result = match server.clone().serve(transport).await {
        Ok(service) => service
            .waiting()
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from),
        Err(error) => Err(anyhow::Error::from(error)),
    };
    // EOF and cancellation never replay in-flight app operations. Wait for the
    // native deadline before finishing the owned process.
    let cleanup = tokio::task::spawn_blocking(move || {
        server
            .backend
            .lock()
            .map_err(|_| anyhow::anyhow!("session worker panicked"))?
            .finish()
    })
    .await?;
    result?;
    cleanup
}

#[cfg(target_os = "linux")]
struct Native {
    args: AgentArgs,
    flutter_root: Option<std::path::PathBuf>,
    engine_src: Option<std::path::PathBuf>,
    session: Option<super::agent::Session>,
    reader: Option<std::io::BufReader<std::os::unix::net::UnixStream>>,
    sequence: u64,
    closed: bool,
}
#[cfg(target_os = "linux")]
impl Native {
    fn new(
        args: AgentArgs,
        flutter_root: Option<std::path::PathBuf>,
        engine_src: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            args,
            flutter_root,
            engine_src,
            session: None,
            reader: None,
            sequence: 0,
            closed: false,
        }
    }
    fn call_inner(&mut self, method: &str, params: Value) -> Result<Value> {
        use super::{Ctx, agent::Session};
        use std::{io::BufReader, time::Duration};
        let Self {
            args,
            flutter_root,
            engine_src,
            session,
            reader,
            sequence,
            closed,
        } = self;
        if *closed {
            bail!("session closed or failed; start a new MCP process with fresh artifacts");
        }
        // Latch failure before building: a failed/mutating call is never replayed.
        *closed = true;
        if session.is_none() {
            let ctx = Ctx {
                flutter: crate::flutter::Flutter::discover(flutter_root.as_deref())?,
                sources: crate::sdk::EngineSources {
                    local_engine_src: engine_src.clone(),
                },
            };
            let owned = Session::launch(
                &ctx,
                args.mode.resolve()?,
                &args.target,
                &args.defines.dart_defines,
                &args.artifacts,
                args.size,
                args.pixel_ratio,
            )?;
            owned
                .stream
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            *reader = Some(BufReader::new(owned.stream.try_clone()?));
            *session = Some(owned);
            let handshake = native_rpc(
                reader.as_mut().unwrap(),
                &mut session.as_mut().unwrap().stream,
                sequence,
                "session.handshake",
                json!({"protocolVersion":1}),
            )?;
            if handshake.get("error").is_some() {
                bail!("agent handshake failed: {handshake}");
            }
        }
        let owned = session.as_mut().unwrap();
        let timeout = params["timeoutMs"].as_u64().unwrap_or(5000).min(120000);
        reader
            .as_ref()
            .unwrap()
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(timeout + 5000)))?;
        let response = match native_rpc(
            reader.as_mut().unwrap(),
            &mut owned.stream,
            sequence,
            method,
            params,
        ) {
            Ok(value) => value,
            Err(err) => {
                session.take();
                return Err(err);
            }
        };
        let mut result = tool_result(response.clone());
        if method == "session.shutdown" && response.get("error").is_none() {
            owned.finish()?;
            session.take();
        } else {
            *closed = false;
            if method == "capture.screenshot" && response.get("error").is_none() {
                let path = Path::new(
                    response["result"]["path"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing capture path"))?,
                )
                .canonicalize()?;
                if !path.starts_with(&owned.artifacts) {
                    bail!("capture outside session artifacts");
                }
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > 16 * 1024 * 1024 {
                    bail!("capture exceeds MCP image limit (16 MiB); resize the window");
                }
                result["content"].as_array_mut().unwrap().push(
                    json!({"type":"image","mimeType":"image/png","data":STANDARD.encode(bytes)}),
                );
            }
        }
        Ok(result)
    }
}
#[cfg(target_os = "linux")]
impl Backend for Native {
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let result = self.call_inner(method, params);
        if result.is_err() && self.closed {
            self.session.take();
        }
        result
    }
    fn finish(&mut self) -> Result<()> {
        self.closed = true;
        if let Some(mut session) = self.session.take() {
            session.finish()?;
        }
        Ok(())
    }
}
#[cfg(not(target_os = "linux"))]
struct Native;
#[cfg(not(target_os = "linux"))]
impl Native {
    fn new(_: AgentArgs, _: Option<std::path::PathBuf>, _: Option<std::path::PathBuf>) -> Self {
        Self
    }
}
#[cfg(not(target_os = "linux"))]
impl Backend for Native {
    fn call(&mut self, _: &str, _: Value) -> Result<Value> {
        bail!("MCP app control currently requires Linux x64")
    }
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn write_message(output: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
#[cfg(target_os = "linux")]
fn native_rpc(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    output: &mut std::os::unix::net::UnixStream,
    sequence: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    *sequence += 1;
    write_message(
        output,
        &json!({"id":sequence,"method":method,"params":params}),
    )?;
    loop {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            bail!("runner closed its control channel");
        }
        if line.len() > 16 * 1024 * 1024 {
            bail!("oversized runner response");
        }
        let response: Value = serde_json::from_slice(&line)?;
        if response.get("event").is_some() {
            continue;
        }
        if response["id"] != *sequence {
            bail!("mismatched runner response ID");
        }
        return Ok(response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    struct Mock {
        calls: Arc<AtomicUsize>,
        finished: Arc<AtomicUsize>,
    }
    impl Backend for Mock {
        fn call(&mut self, method: &str, params: Value) -> Result<Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if method == "session.health" {
                bail!("connection lost");
            }
            Ok(tool_result(
                json!({"result":{"method":method,"params":params}}),
            ))
        }
        fn finish(&mut self) -> Result<()> {
            self.finished.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    async fn send(output: &mut (impl tokio::io::AsyncWrite + Unpin), message: Value) {
        output
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn receive(input: &mut (impl tokio::io::AsyncBufRead + Unpin)) -> Value {
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            input.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    #[tokio::test]
    async fn sdk_discovery_calls_errors_and_eof_cleanup() {
        let calls = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(Mock {
            calls: calls.clone(),
            finished: finished.clone(),
        });
        let (client, transport) = tokio::io::duplex(65536);
        let (input, output) = tokio::io::split(transport);
        let worker = tokio::spawn(serve_io(server, input, output));
        let (input, mut output) = tokio::io::split(client);
        let mut input = tokio::io::BufReader::new(input);
        send(&mut output,json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
        let init = receive(&mut input).await;
        assert_eq!(init["id"], "init");
        assert_eq!(init["result"]["serverInfo"]["name"], "rlutter");
        send(
            &mut output,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        send(
            &mut output,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await;
        let catalogue = receive(&mut input).await;
        assert_eq!(
            catalogue["result"]["tools"].as_array().unwrap().len(),
            TOOLS.len()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        for (id, tool, args) in [
            (
                2,
                "rlutter_input_replaceText",
                json!({"viewId":9,"text":"שלום 🌍 café"}),
            ),
            (3, "rlutter_session_health", json!({})),
            (4, "rlutter_windows_list", json!({"typo":1})),
        ] {
            send(&mut output,json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":tool,"arguments":args}})).await;
            let result = receive(&mut input).await;
            assert_eq!(result["id"], id);
            if id == 2 {
                assert_eq!(result["result"]["isError"], false);
                let data: Value =
                    serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap())
                        .unwrap();
                assert_eq!(data["method"], "input.replaceText");
                assert_eq!(data["params"], args);
            } else {
                assert_eq!(result["result"]["isError"], true);
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        output.shutdown().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(finished.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn cancellation_keeps_dispatched_operation_exclusive() {
        struct Blocking {
            started: Arc<tokio::sync::Notify>,
            release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        }
        impl Backend for Blocking {
            fn call(&mut self, _: &str, _: Value) -> Result<Value> {
                self.started.notify_one();
                let (lock, condition) = &*self.release;
                let _guard = condition
                    .wait_while(lock.lock().unwrap(), |ready| !*ready)
                    .unwrap();
                Ok(tool_result(json!({"result":{}})))
            }
            fn finish(&mut self) -> Result<()> {
                Ok(())
            }
        }
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let server = McpServer::new(Blocking {
            started: started.clone(),
            release: release.clone(),
        });
        let available = server.available.clone();
        let (client, transport) = tokio::io::duplex(65536);
        let (input, output) = tokio::io::split(transport);
        let worker = tokio::spawn(serve_io(server, input, output));
        let (input, mut output) = tokio::io::split(client);
        let mut input = tokio::io::BufReader::new(input);
        send(&mut output,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
        receive(&mut input).await;
        send(
            &mut output,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        send(&mut output,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rlutter_input_tap","arguments":{"viewId":1,"x":1,"y":1}}})).await;
        tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        send(&mut output,json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"test"}})).await;
        send(&mut output,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rlutter_windows_list"}})).await;
        let busy = receive(&mut input).await;
        assert_eq!(busy["id"], 3);
        assert_eq!(busy["result"]["isError"], true);
        assert_eq!(available.available_permits(), 0);
        *release.0.lock().unwrap() = true;
        release.1.notify_one();
        output.shutdown().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(available.available_permits(), 1);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn native_transport_skips_events_and_rejects_wrong_ids() {
        use std::{io::BufReader, os::unix::net::UnixStream};
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let mut reader = BufReader::new(b.try_clone().unwrap());
            for id in [1, 2] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let value: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(value["id"], id);
                write_message(&mut b, &json!({"event":"semantics.updated"})).unwrap();
                write_message(&mut b, &json!({"id":if id==1 {1} else {99},"result":{}})).unwrap();
            }
        });
        let mut reader = BufReader::new(a.try_clone().unwrap());
        let mut seq = 0;
        assert!(native_rpc(&mut reader, &mut a, &mut seq, "windows.list", json!({})).is_ok());
        assert!(native_rpc(&mut reader, &mut a, &mut seq, "windows.list", json!({})).is_err());
        worker.join().unwrap();
    }
}
