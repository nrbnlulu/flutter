//! Compact observations and short-lived references; native handles remain authoritative.
use super::{Backend, compact, tool_result};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

// Never reset on detach, shutdown, finish, or construction of another backend.
static GENERATION: AtomicU64 = AtomicU64::new(0);
type View = (String, i64);

struct References {
    epoch: String,
    revision: u64,
    generation: u64,
    handles: HashMap<String, Value>,
}

pub(super) struct UiBackend<B: Backend> {
    backend: B,
    views: HashMap<View, References>,
}

fn error(message: String) -> Value {
    tool_result(json!({"error":{"code":"invalid_ui_request","message":message}}))
}

fn view(params: &Value) -> Result<View> {
    let Some(session) = params["session_id"].as_str() else {
        bail!("session_id is required; start or attach a session first");
    };
    let Some(view) = params["viewId"].as_i64().filter(|v| *v >= 0) else {
        bail!("viewId must be a non-negative integer; use windows.list");
    };
    Ok((session.to_owned(), view))
}

fn integer(params: &Value, field: &str, default: u64) -> Result<u64> {
    match params.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("{field} must be a non-negative integer")),
    }
}

fn snapshot_params(params: &Value) -> Value {
    let mut params = params.clone();
    if let Some(params) = params.as_object_mut() {
        params.retain(|key, _| matches!(key.as_str(), "session_id" | "viewId" | "timeoutMs"));
    }
    params
}

fn parse_snapshot(response: &Value, view_id: i64) -> Result<Value> {
    let text = response["content"]
        .as_array()
        .and_then(|content| content.iter().find(|c| c["type"] == "text"))
        .and_then(|c| c["text"].as_str())
        .ok_or_else(|| anyhow::anyhow!("semantics.snapshot returned no native JSON text"))?;
    let snapshot: Value = serde_json::from_str(text)?;
    if snapshot["epoch"].as_str().is_none()
        || snapshot["revision"].as_u64().is_none()
        || snapshot["viewId"].as_i64() != Some(view_id)
        || !snapshot["nodes"].is_array()
    {
        bail!("semantics.snapshot returned an invalid identity or tree; refresh with ui.snapshot");
    }
    Ok(snapshot)
}

impl<B: Backend> UiBackend<B> {
    pub(super) fn new(backend: B) -> Self {
        Self {
            backend,
            views: HashMap::new(),
        }
    }

    fn snapshot(&mut self, params: Value) -> Result<Value> {
        let key = view(&params)?;
        let max_chars = integer(&params, "max_chars", 4000)?;
        if max_chars > 64000 {
            bail!("max_chars must be an integer in 0..=64000");
        }
        let offset = usize::try_from(integer(&params, "offset", 0)?)?;
        let requested_generation = params
            .get("generation")
            .map(|_| integer(&params, "generation", 0))
            .transpose()?;
        let generation = if offset > 0 {
            let Some(previous) = self.views.get(&key) else {
                bail!("Unknown continuation; refresh with ui.snapshot at offset 0");
            };
            if requested_generation != Some(previous.generation) {
                bail!("Stale or missing generation; refresh with ui.snapshot at offset 0");
            }
            previous.generation
        } else {
            // A new observation supersedes all refs, even if the native read fails.
            self.views.remove(&key);
            GENERATION
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| anyhow::anyhow!("UI reference generation exhausted"))?
                + 1
        };
        let response = self
            .backend
            .call("semantics.snapshot", snapshot_params(&params))?;
        if response["isError"] == true {
            return Ok(response);
        }
        let snapshot = parse_snapshot(&response, key.1)?;
        if offset > 0
            && !self.views.get(&key).is_some_and(|previous| {
                snapshot["epoch"].as_str() == Some(previous.epoch.as_str())
                    && snapshot["revision"].as_u64() == Some(previous.revision)
            })
        {
            self.views.remove(&key);
            bail!("Stale continuation: semantics changed; refresh with ui.snapshot at offset 0");
        }
        let (text, refs) = compact::format(&snapshot, max_chars as usize, offset, generation);
        let previous = self.views.entry(key).or_insert_with(|| References {
            epoch: snapshot["epoch"].as_str().unwrap().to_owned(),
            revision: snapshot["revision"].as_u64().unwrap(),
            generation,
            handles: HashMap::new(),
        });
        previous.handles.extend(refs);
        // Compact JSON lines are already text, not another JSON-encoded string.
        let mut result = tool_result(json!({"result":null}));
        result["content"][0]["text"] = json!(text);
        Ok(result)
    }

    fn action(&mut self, method: &str, mut params: Value) -> Result<Value> {
        let key = view(&params)?;
        let Some(reference) = params["ref"].as_str() else {
            bail!("ref must be a string from ui.snapshot; refresh before acting");
        };
        if ["selector", "handle", "x", "y"]
            .iter()
            .any(|field| params.get(field).is_some())
        {
            bail!("Do not combine ref with selector, handle, or coordinates");
        }
        let Some(previous) = self.views.get(&key) else {
            bail!("Unknown ref for this session/view; refresh with ui.snapshot before acting");
        };
        let Some(handle) = previous.handles.get(reference).cloned() else {
            bail!(
                "Unknown or stale ref for this session/view; refresh with ui.snapshot before acting"
            );
        };
        let epoch = previous.epoch.clone();
        let revision = previous.revision;
        let response = self
            .backend
            .call("semantics.snapshot", snapshot_params(&params))?;
        if response["isError"] == true {
            return Ok(response);
        }
        let snapshot = parse_snapshot(&response, key.1)?;
        if snapshot["epoch"].as_str() != Some(epoch.as_str())
            || snapshot["revision"].as_u64() != Some(revision)
            || handle["epoch"].as_str() != Some(epoch.as_str())
            || handle["revision"].as_u64() != Some(revision)
            || handle["viewId"].as_i64() != Some(key.1)
            || handle["nodeId"].as_i64().is_none()
            || !snapshot["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == handle["nodeId"] && node["handle"] == handle)
        {
            self.views.remove(&key);
            bail!(
                "Stale ref: semantics changed or the node was removed; refresh with ui.snapshot before acting"
            );
        }
        let params = params.as_object_mut().unwrap();
        params.remove("ref");
        params.insert("handle".into(), handle);
        // Forward the exact native handle, including its epoch/revision guards.
        self.backend.call(method, Value::Object(params.clone()))
    }

    fn call_inner(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "ui.snapshot" => self.snapshot(params),
            "ui.screenshot" => self.backend.call("capture.screenshot", params),
            "input.tap" | "semantics.perform" if params.get("ref").is_some() => {
                self.action(method, params)
            }
            "session.shutdown" | "session.detach" => {
                if let Some(session) = params["session_id"].as_str() {
                    self.views.retain(|(id, _), _| id != session);
                }
                self.backend.call(method, params)
            }
            _ => self.backend.call(method, params),
        }
    }
}

impl<B: Backend> Backend for UiBackend<B> {
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        Ok(self
            .call_inner(method, params)
            .unwrap_or_else(|err| error(format!("{err:#}"))))
    }

    fn finish(&mut self) -> Result<()> {
        self.views.clear();
        self.backend.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        snapshot: Value,
        calls: Vec<(String, Value)>,
        response: Option<Value>,
        fail: bool,
        finished: bool,
    }

    impl Backend for Fake {
        fn call(&mut self, method: &str, params: Value) -> Result<Value> {
            self.calls.push((method.into(), params.clone()));
            if self.fail {
                bail!("native failure");
            }
            if let Some(response) = &self.response {
                return Ok(response.clone());
            }
            if method == "semantics.snapshot" {
                let mut snapshot = self.snapshot.clone();
                snapshot["viewId"] = params["viewId"].clone();
                for node in snapshot["nodes"].as_array_mut().unwrap() {
                    node["handle"]["viewId"] = params["viewId"].clone();
                }
                return Ok(tool_result(json!({"result":snapshot})));
            }
            Ok(tool_result(json!({"result":{"dispatched":true}})))
        }
        fn finish(&mut self) -> Result<()> {
            self.finished = true;
            if self.fail {
                bail!("finish failed");
            }
            Ok(())
        }
    }

    fn backend() -> UiBackend<Fake> {
        let nodes: Vec<_> = (0..12)
            .map(|id| {
                json!({
                    "id":id,"label":format!("Button {id}"),"roleName":"button","children":[],
                    "handle":{"epoch":"epoch","revision":3,"viewId":7,"nodeId":id}
                })
            })
            .collect();
        UiBackend::new(Fake {
            snapshot: json!({"epoch":"epoch","revision":3,"viewId":7,"nodes":nodes}),
            calls: vec![],
            response: None,
            fail: false,
            finished: false,
        })
    }
    fn params() -> Value {
        json!({"session_id":"one","viewId":7})
    }
    fn call(ui: &mut UiBackend<Fake>, method: &str, params: Value) -> Value {
        ui.call(method, params).unwrap()
    }
    fn records(result: &Value) -> Vec<Value> {
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn observe(ui: &mut UiBackend<Fake>) -> String {
        records(&call(ui, "ui.snapshot", params()))[0]["ref"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn action(reference: &str) -> Value {
        let mut p = params();
        p["ref"] = json!(reference);
        p
    }
    fn assert_error(result: &Value) {
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
        assert_eq!(result["content"][0]["type"], "text");
        assert!(result.get("structuredContent").is_none());
    }

    #[test]
    fn snapshot_is_bounded_text_only_and_strips_ui_arguments() {
        let mut ui = backend();
        let mut p = params();
        p["max_chars"] = json!(550);
        p["offset"] = json!(0);
        p["generation"] = json!(123);
        p["timeoutMs"] = json!(1200);
        let result = call(&mut ui, "ui.snapshot", p);
        assert_eq!(result["isError"], false);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 550
        );
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
        assert!(result.get("structuredContent").is_none());
        assert_eq!(
            ui.backend.calls[0],
            (
                "semantics.snapshot".into(),
                json!({"session_id":"one","viewId":7,"timeoutMs":1200})
            )
        );
        let reference = records(&result)[0]["ref"].as_str().unwrap().to_owned();
        let ack = call(&mut ui, "input.tap", action(&reference));
        assert_eq!(ack, tool_result(json!({"result":{"dispatched":true}})));
        let (method, p) = ui.backend.calls.last().unwrap();
        assert_eq!(method, "input.tap");
        assert!(p.get("ref").is_none());
        assert_eq!(p["handle"], ui.backend.snapshot["nodes"][0]["handle"]);
        assert_eq!(ui.backend.calls[1].0, "semantics.snapshot");
    }

    #[test]
    fn invalid_pagination_arguments_never_call_native() {
        for (field, value) in [
            ("max_chars", json!(-1)),
            ("max_chars", json!(64001)),
            ("max_chars", json!(1.5)),
            ("max_chars", Value::Null),
            ("offset", json!(-1)),
            ("offset", json!("1")),
            ("offset", json!(0.5)),
            ("generation", json!(-1)),
            ("generation", Value::Null),
        ] {
            let mut ui = backend();
            let mut p = params();
            p[field] = value;
            assert_error(&call(&mut ui, "ui.snapshot", p));
            assert!(ui.backend.calls.is_empty());
        }
        for budget in [0, 1, 64000] {
            let mut ui = backend();
            let mut p = params();
            p["max_chars"] = json!(budget);
            let result = call(&mut ui, "ui.snapshot", p);
            assert_eq!(result["isError"], false);
            assert!(
                result["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .count()
                    <= budget
            );
        }
    }

    #[test]
    fn continuation_appends_only_emitted_refs_and_requires_generation() {
        let mut ui = backend();
        let mut p = params();
        p["max_chars"] = json!(550);
        let first = records(&call(&mut ui, "ui.snapshot", p.clone()));
        let meta = first.last().unwrap()["meta"].clone();
        let reference = first[0]["ref"].as_str().unwrap();
        let generation = meta["generation"].as_u64().unwrap();
        let next = meta["nextOffset"].as_u64().unwrap();
        assert!(next > 0 && next < 12);
        assert_error(&call(
            &mut ui,
            "input.tap",
            action(&format!("s{generation}-11")),
        ));
        p["offset"] = json!(next);
        assert_error(&call(&mut ui, "ui.snapshot", p.clone()));
        p["generation"] = json!(generation + 1);
        assert_error(&call(&mut ui, "ui.snapshot", p.clone()));
        p["generation"] = json!(generation);
        let second = records(&call(&mut ui, "ui.snapshot", p));
        assert_eq!(second.last().unwrap()["meta"]["generation"], generation);
        for r in [reference, second[0]["ref"].as_str().unwrap()] {
            assert_eq!(call(&mut ui, "input.tap", action(r))["isError"], false);
        }
    }

    #[test]
    fn changed_identity_rejects_continuations_and_invalidates_refs() {
        for field in ["epoch", "revision"] {
            let mut ui = backend();
            let mut p = params();
            p["max_chars"] = json!(550);
            let rows = records(&call(&mut ui, "ui.snapshot", p.clone()));
            let meta = &rows.last().unwrap()["meta"];
            p["offset"] = meta["nextOffset"].clone();
            p["generation"] = meta["generation"].clone();
            ui.backend.snapshot[field] = if field == "epoch" {
                json!("new")
            } else {
                json!(4)
            };
            assert_error(&call(&mut ui, "ui.snapshot", p));
            assert!(ui.views.is_empty());
            assert_error(&call(
                &mut ui,
                "input.tap",
                action(rows[0]["ref"].as_str().unwrap()),
            ));
            assert!(
                ui.backend
                    .calls
                    .iter()
                    .all(|(method, _)| method == "semantics.snapshot")
            );
        }
    }

    #[test]
    fn stale_removed_reused_or_mismatched_handles_never_dispatch() {
        for change in 0..6 {
            let mut ui = backend();
            let reference = observe(&mut ui);
            match change {
                0 => ui.backend.snapshot["revision"] = json!(4),
                1 => ui.backend.snapshot["epoch"] = json!("restarted"),
                2 => {
                    ui.backend.snapshot["nodes"]
                        .as_array_mut()
                        .unwrap()
                        .remove(0);
                }
                3 => ui.backend.snapshot["nodes"][0]["handle"]["revision"] = json!(4),
                4 => ui.backend.snapshot["nodes"][0]["handle"]["epoch"] = json!("reused"),
                _ => ui.backend.snapshot["nodes"][0]["id"] = json!(100),
            }
            assert_error(&call(&mut ui, "input.tap", action(&reference)));
            assert_eq!(ui.backend.calls.len(), 2);
            assert!(
                ui.backend
                    .calls
                    .iter()
                    .all(|(m, _)| m == "semantics.snapshot")
            );
        }
    }

    #[test]
    fn views_and_sessions_are_isolated_and_new_snapshots_replace_refs() {
        let mut ui = backend();
        let first = observe(&mut ui);
        for (session, view_id) in [("one", 8), ("two", 7)] {
            let mut p = params();
            p["session_id"] = json!(session);
            p["viewId"] = json!(view_id);
            call(&mut ui, "ui.snapshot", p.clone());
            p["ref"] = json!(first);
            let count = ui.backend.calls.len();
            assert_error(&call(&mut ui, "input.tap", p));
            assert_eq!(ui.backend.calls.len(), count);
        }
        assert_eq!(call(&mut ui, "input.tap", action(&first))["isError"], false);
        let second = observe(&mut ui);
        assert_ne!(first, second);
        assert_error(&call(&mut ui, "input.tap", action(&first)));
        assert_eq!(
            call(&mut ui, "input.tap", action(&second))["isError"],
            false
        );
    }

    #[test]
    fn ref_conflicts_and_unknown_refs_do_not_dispatch() {
        let mut ui = backend();
        let reference = observe(&mut ui);
        for field in ["selector", "handle", "x", "y"] {
            let mut p = action(&reference);
            p[field] = Value::Null;
            assert_error(&call(&mut ui, "input.tap", p));
        }
        for reference in [json!("unknown"), json!(12), Value::Null] {
            let mut p = params();
            p["ref"] = reference;
            assert_error(&call(&mut ui, "semantics.perform", p));
        }
        assert_eq!(ui.backend.calls.len(), 1);
        let mut p = action(&reference);
        p["action"] = json!("setText");
        p["args"] = json!({"text":"hi"});
        assert_eq!(
            call(&mut ui, "semantics.perform", p.clone())["isError"],
            false
        );
        let forwarded = &ui.backend.calls.last().unwrap().1;
        assert_eq!(forwarded["action"], p["action"]);
        assert_eq!(forwarded["args"], p["args"]);
    }

    #[test]
    fn lifecycle_clears_only_target_session_and_never_reuses_refs() {
        for method in ["session.shutdown", "session.detach"] {
            let mut ui = backend();
            let old = observe(&mut ui);
            let mut other = params();
            other["session_id"] = json!("two");
            call(&mut ui, "ui.snapshot", other);
            call(&mut ui, method, json!({"session_id":"one"}));
            assert_eq!(ui.views.len(), 1);
            assert_error(&call(&mut ui, "input.tap", action(&old)));
            let new = observe(&mut ui);
            assert_ne!(old, new);
            ui.backend.fail = true;
            assert!(ui.finish().is_err());
            assert!(ui.backend.finished);
            assert!(ui.views.is_empty());
            ui.backend.fail = false;
            assert_ne!(observe(&mut ui), new);
        }
        let first = observe(&mut backend());
        let second = observe(&mut backend());
        assert_ne!(first, second);
    }

    #[test]
    fn native_calls_and_explicit_screenshots_are_preserved_exactly() {
        let mut ui = backend();
        let image = json!({"content":[{"type":"text","text":"{\"frame\":42}"},
            {"type":"image","mimeType":"image/png","data":"png"}],"isError":false});
        ui.backend.response = Some(image.clone());
        let p = json!({"session_id":"one","viewId":7,"mode":"latestCompleted","name":"shot"});
        assert_eq!(call(&mut ui, "ui.screenshot", p.clone()), image);
        assert_eq!(
            ui.backend.calls.last().unwrap(),
            &("capture.screenshot".into(), p.clone())
        );
        assert_eq!(call(&mut ui, "capture.screenshot", p), image);
        let p = json!({"session_id":"one","viewId":7,"x":12,"y":24});
        assert_eq!(call(&mut ui, "input.tap", p.clone()), image);
        assert_eq!(ui.backend.calls.last().unwrap(), &("input.tap".into(), p));
    }

    #[test]
    fn malformed_and_failed_native_snapshots_are_text_errors_without_mutation() {
        let mut ui = backend();
        let reference = observe(&mut ui);
        for response in [
            tool_result(json!({"result":{"nodes":[]}})),
            json!({"content":[{"type":"text","text":"not JSON"}],"isError":false}),
            json!({"content":[],"isError":false}),
            tool_result(json!({"error":{"message":"bridge unavailable"}})),
        ] {
            ui.backend.response = Some(response);
            assert_error(&call(&mut ui, "input.tap", action(&reference)));
        }
        ui.backend.response = None;
        ui.backend.fail = true;
        assert_error(&call(&mut ui, "input.tap", action(&reference)));
        assert!(
            ui.backend
                .calls
                .iter()
                .all(|(m, _)| m == "semantics.snapshot")
        );
    }
}
