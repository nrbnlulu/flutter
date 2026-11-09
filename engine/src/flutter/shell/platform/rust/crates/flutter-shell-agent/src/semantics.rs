use crate::AgentError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
struct Tree {
    revision: u64,
    nodes: BTreeMap<i64, Value>,
    actions: BTreeMap<i64, Value>,
}
#[derive(Debug)]
pub struct SemanticsCache {
    epoch: String,
    views: BTreeMap<i64, Tree>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub identifier: Option<String>,
    pub label: Option<String>,
    pub role: Option<String>,
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
}
impl Selector {
    pub fn parse(value: Value) -> Result<Self, AgentError> {
        let selector: Self = serde_json::from_value(value)
            .map_err(|e| AgentError::new("invalid_request", e.to_string()))?;
        if selector.identifier.is_none() && selector.label.is_none() && selector.role.is_none() {
            return Err(AgentError::new(
                "invalid_request",
                "selector needs identifier, label or role",
            ));
        }
        Ok(selector)
    }
    fn matches(&self, node: &Value) -> bool {
        self.identifier
            .as_ref()
            .is_none_or(|v| node["identifier"].as_str() == Some(v))
            && self
                .label
                .as_ref()
                .is_none_or(|v| node["label"].as_str() == Some(v))
            && self
                .role
                .as_ref()
                .is_none_or(|v| node["roleName"].as_str() == Some(v))
            && self
                .enabled
                .is_none_or(|v| node["enabled"].as_bool() == Some(v))
            && self
                .focused
                .is_none_or(|v| node["focused"].as_bool() == Some(v))
    }
}
impl SemanticsCache {
    pub fn new(epoch: String) -> Self {
        Self {
            epoch,
            views: BTreeMap::new(),
        }
    }
    pub fn clear(&mut self) {
        self.views.clear();
    }
    pub fn remove_view(&mut self, view: i64) {
        self.views.remove(&view);
    }
    pub fn apply(&mut self, view: i64, bytes: &[u8]) -> Result<u64, AgentError> {
        let update: Value = serde_json::from_slice(bytes).map_err(|e| {
            AgentError::new("app_exception", format!("invalid semantics update: {e}"))
        })?;
        let tree = self.views.entry(view).or_default();
        for node in update["nodes"]
            .as_array()
            .ok_or_else(|| AgentError::new("app_exception", "semantics nodes missing"))?
        {
            let id = node["id"]
                .as_i64()
                .ok_or_else(|| AgentError::new("app_exception", "semantics node id missing"))?;
            let mut node = node.clone();
            if node["flags"]["isObscured"] == true {
                for field in ["value", "increasedValue", "decreasedValue"] {
                    node[field] = json!("<redacted>");
                }
            }
            tree.nodes.insert(id, node);
        }
        if let Some(actions) = update["customActions"].as_array() {
            for action in actions {
                if let Some(id) = action["id"].as_i64() {
                    tree.actions.insert(id, action.clone());
                }
            }
        }
        if tree.nodes.contains_key(&0) {
            let mut reachable = BTreeSet::new();
            let mut pending = vec![0];
            while let Some(id) = pending.pop() {
                if !reachable.insert(id) {
                    continue;
                }
                if let Some(node) = tree.nodes.get(&id) {
                    if let Some(children) = node["children"].as_array() {
                        pending.extend(children.iter().filter_map(Value::as_i64));
                    }
                }
            }
            tree.nodes.retain(|id, _| reachable.contains(id));
            let used = tree
                .nodes
                .values()
                .filter_map(|n| n["customActions"].as_array())
                .flatten()
                .filter_map(Value::as_i64)
                .collect::<BTreeSet<_>>();
            tree.actions.retain(|id, _| used.contains(id));
        }
        tree.revision += 1;
        Ok(tree.revision)
    }
    pub fn retain_views(&mut self, views: &BTreeSet<i64>) {
        self.views.retain(|id, _| views.contains(id));
    }
    pub fn revision(&self, view: i64) -> u64 {
        self.views.get(&view).map_or(0, |v| v.revision)
    }
    pub fn snapshot(&self, view: i64) -> Value {
        let Some(tree) = self.views.get(&view) else {
            return json!({"epoch":self.epoch,"viewId":view,"revision":0,"nodes":[]});
        };
        let mut nodes = Vec::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![(0, identity(), false)];
        while let Some((id, parent, hidden)) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(raw) = tree.nodes.get(&id) else {
                continue;
            };
            let local = matrix(&raw["transform"]);
            let combined = local.map(|local| multiply(parent, local));
            let hidden =
                hidden || raw["flags"]["isHidden"].as_bool().unwrap_or(false) || combined.is_none();
            let transform = combined.unwrap_or_else(identity);
            let mut node = raw.clone();
            node["handle"] =
                json!({"epoch":self.epoch,"viewId":view,"nodeId":id,"revision":tree.revision});
            node["viewTransform"] = json!(transform);
            node["viewBounds"] = bounds(&raw["rect"], transform).map_or(Value::Null, |b| json!(b));
            node["hidden"] = json!(hidden);
            node["enabled"] = json!(raw["flags"]["isEnabled"].as_i64() != Some(2));
            node["focused"] = json!(raw["flags"]["isFocused"].as_i64() == Some(1));
            node["roleName"] = json!(role(raw));
            nodes.push(node);
            if let Some(children) = raw["children"].as_array() {
                for child in children.iter().rev().filter_map(Value::as_i64) {
                    pending.push((child, transform, hidden));
                }
            }
        }
        json!({"epoch":self.epoch,"viewId":view,"revision":tree.revision,"nodes":nodes,"customActions":tree.actions.values().collect::<Vec<_>>()})
    }
    pub fn find_all(&self, view: i64, selector: &Selector) -> Vec<Value> {
        self.snapshot(view)["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|node| selector.matches(node))
            .cloned()
            .collect()
    }
    pub fn find_unique(&self, view: i64, selector: &Selector) -> Result<Value, AgentError> {
        let mut matches = self.find_all(view, selector);
        match matches.len() {
            0 => Err(AgentError::new(
                "not_found",
                "selector matched no semantics nodes",
            )),
            1 => Ok(matches.remove(0)),
            count => Err(AgentError::new(
                "ambiguous_selector",
                format!("selector matched {count} nodes"),
            )),
        }
    }
    pub fn resolve_handle(&self, handle: &Value) -> Result<Value, AgentError> {
        let view = handle["viewId"]
            .as_i64()
            .ok_or_else(|| AgentError::new("invalid_request", "handle needs viewId"))?;
        if handle["epoch"].as_str() != Some(self.epoch.as_str())
            || handle["revision"].as_u64() != Some(self.revision(view))
        {
            return Err(AgentError::new(
                "stale_handle",
                "semantics epoch or revision changed",
            ));
        }
        self.snapshot(view)["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| n["id"] == handle["nodeId"])
            .cloned()
            .ok_or_else(|| AgentError::new("stale_handle", "semantics node no longer exists"))
    }
}
fn identity() -> [f64; 16] {
    [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]
}
fn matrix(value: &Value) -> Option<[f64; 16]> {
    let array = value.as_array()?;
    if array.len() != 16 {
        return None;
    }
    let mut result = [0.; 16];
    for (out, value) in result.iter_mut().zip(array) {
        *out = value.as_f64().filter(|n| n.is_finite())?;
    }
    Some(result)
}
fn multiply(a: [f64; 16], b: [f64; 16]) -> [f64; 16] {
    let mut out = [0.; 16];
    for row in 0..4 {
        for col in 0..4 {
            for k in 0..4 {
                out[row * 4 + col] += a[row * 4 + k] * b[k * 4 + col];
            }
        }
    }
    out
}
fn bounds(rect: &Value, m: [f64; 16]) -> Option<[f64; 4]> {
    let r = rect.as_array()?;
    if r.len() != 4 {
        return None;
    }
    let values = r.iter().map(Value::as_f64).collect::<Option<Vec<_>>>()?;
    let mut out = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (x, y) in [
        (values[0], values[1]),
        (values[2], values[1]),
        (values[0], values[3]),
        (values[2], values[3]),
    ] {
        let w = m[12] * x + m[13] * y + m[15];
        if !w.is_finite() || w.abs() < 1e-12 {
            return None;
        }
        let x = (m[0] * x + m[1] * y + m[3]) / w;
        let y = (m[4] * x + m[5] * y + m[7]) / w;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        out[0] = out[0].min(x);
        out[1] = out[1].min(y);
        out[2] = out[2].max(x);
        out[3] = out[3].max(y);
    }
    Some(out)
}
fn role(node: &Value) -> &'static str {
    const ROLES: [&str; 36] = [
        "none",
        "tab",
        "tabBar",
        "tabPanel",
        "dialog",
        "alertDialog",
        "table",
        "cell",
        "row",
        "columnHeader",
        "dragHandle",
        "spinButton",
        "comboBox",
        "menuBar",
        "menu",
        "menuItem",
        "menuItemCheckbox",
        "menuItemRadio",
        "list",
        "listItem",
        "form",
        "tooltip",
        "loadingSpinner",
        "progressBar",
        "hotKey",
        "radioGroup",
        "status",
        "alert",
        "complementary",
        "contentInfo",
        "main",
        "navigation",
        "region",
        "slider",
        "listBox",
        "option",
    ];
    let role = node["role"].as_u64().unwrap_or(0) as usize;
    if role > 0 {
        return ROLES.get(role).copied().unwrap_or("unknown");
    }
    for (key, name) in [
        ("isButton", "button"),
        ("isTextField", "textField"),
        ("isSlider", "slider"),
        ("isLink", "link"),
        ("isImage", "image"),
    ] {
        if node["flags"][key].as_bool() == Some(true) {
            return name;
        }
    }
    "none"
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(id: i64, children: Vec<i64>, label: &str) -> Value {
        json!({"id":id,"label":label,"children":children,"transform":identity(),"rect":[0,0,10,20],"flags":{}})
    }
    #[test]
    fn incremental_pruning_and_revision_handles_prevent_stale_matches() {
        let mut cache = SemanticsCache::new("epoch".into());
        let update =
            json!({"nodes":[node(0,vec![1,2],"root"),node(1,vec![],"Save"),node(2,vec![],"Save")]});
        cache
            .apply(7, &serde_json::to_vec(&update).unwrap())
            .unwrap();
        let selector = Selector::parse(json!({"label":"Save"})).unwrap();
        assert_eq!(
            cache.find_unique(7, &selector).unwrap_err().code,
            "ambiguous_selector"
        );
        let handle = cache.find_all(7, &selector)[0]["handle"].clone();
        cache
            .apply(
                7,
                &serde_json::to_vec(&json!({"nodes":[node(0,vec![2],"root")]})).unwrap(),
            )
            .unwrap();
        assert_eq!(cache.find_all(7, &selector).len(), 1);
        assert_eq!(
            cache.resolve_handle(&handle).unwrap_err().code,
            "stale_handle"
        );
        cache.remove_view(7);
        assert_eq!(
            cache.find_unique(7, &selector).unwrap_err().code,
            "not_found"
        );
    }
    #[test]
    fn geometry_accumulates_parent_transform_and_hidden_state() {
        let mut root = node(0, vec![1], "root");
        let mut m = identity();
        m[3] = 12.;
        m[7] = -4.;
        root["transform"] = json!(m);
        root["flags"] = json!({"isHidden":true});
        let mut cache = SemanticsCache::new("epoch".into());
        cache
            .apply(
                0,
                &serde_json::to_vec(&json!({"nodes":[root,node(1,vec![],"child")]})).unwrap(),
            )
            .unwrap();
        let node = cache
            .find_unique(0, &Selector::parse(json!({"label":"child"})).unwrap())
            .unwrap();
        assert_eq!(node["viewBounds"], json!([12., -4., 22., 16.]));
        assert_eq!(node["hidden"], true);
    }
}
