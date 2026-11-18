//! Compact, observation-only semantics snapshots. No native handles appear in text.
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

const PREVIEW: usize = 96;
const CUSTOM_ACTIONS: usize = 8;
const ACTIONS: [&str; 26] = [
    "tap",
    "longPress",
    "scrollLeft",
    "scrollRight",
    "scrollUp",
    "scrollDown",
    "increase",
    "decrease",
    "showOnScreen",
    "moveCursorForwardByCharacter",
    "moveCursorBackwardByCharacter",
    "setSelection",
    "copy",
    "cut",
    "paste",
    "didGainAccessibilityFocus",
    "didLoseAccessibilityFocus",
    "customAction",
    "dismiss",
    "moveCursorForwardByWord",
    "moveCursorBackwardByWord",
    "setText",
    "focus",
    "scrollToOffset",
    "expand",
    "collapse",
];

/// Returns JSON lines (node records followed by one `meta` record) and refs for
/// precisely the emitted nodes. The *entire string*, including metadata and
/// newlines, is at most `max_chars` Unicode scalar values. Metadata is separately
/// capped at 512 characters. If even metadata cannot fit, returns empty output.
///
/// `offset`/`nextOffset` index visible, non-empty records, not raw native nodes.
/// Empty wrappers are collapsed; `depth` and numeric `parent` describe the
/// resulting tree, including ancestors outside the current page. No ancestor
/// refs are minted just to supply context. Reuse a cursor only for the same
/// epoch/view/revision. `omitted` includes records before and after this page.
/// A blocked page retains its offset; retry with a larger budget. Text previews
/// shrink to fit, so the original length of a string cannot block pagination.
/// Refs are generation-scoped; the caller owns generation and handle lifetime.
pub(super) fn format(
    snapshot: &Value,
    max_chars: usize,
    offset: usize,
    generation: u64,
) -> (String, Vec<(String, Value)>) {
    let nodes = snapshot["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let by_id: BTreeMap<_, _> = nodes
        .iter()
        .filter_map(|n| Some((n["id"].as_i64()?, n)))
        .collect();
    let custom: BTreeMap<_, _> = snapshot["customActions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["id"].as_i64()?, a)))
        .collect();
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    let mut hidden_count = 0;
    let mut collapsed = 0;
    // Iterative traversal avoids stack overflow on deeply nested semantics.
    for seed in nodes {
        let Some(seed) = seed["id"].as_i64() else {
            continue;
        };
        let mut pending = vec![(seed, 0usize, None, false)];
        while let Some((id, depth, parent, inherited_hidden)) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(&node) = by_id.get(&id) else {
                continue;
            };
            let hidden =
                inherited_hidden || node["hidden"] == true || node["flags"]["isHidden"] == true;
            let meaningful = !describe(node, &custom, 0).is_empty();
            let (child_depth, child_parent) = if hidden {
                hidden_count += 1;
                (depth, parent)
            } else if meaningful {
                rows.push((node, depth, parent));
                (depth.saturating_add(1), Some(id))
            } else {
                collapsed += 1;
                (depth, parent)
            };
            for child in node["children"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .filter_map(Value::as_i64)
            {
                pending.push((child, child_depth, child_parent, hidden));
            }
        }
    }
    let total = rows.len();
    let start = offset.min(total);
    let meta = |returned: usize, next: Option<usize>| {
        json!({"meta": {
            "epoch": preview(snapshot["epoch"].as_str().unwrap_or(""), 32),
            "viewId": snapshot["viewId"].as_i64(),
            "revision": snapshot["revision"].as_u64(),
            "generation": generation, "offset": start, "total": total,
            "returned": returned, "omitted": total.saturating_sub(returned),
            "hidden": hidden_count, "collapsed": collapsed, "nextOffset": next
        }})
        .to_string()
    };
    // Reserve maximum widths for both counters and both forms of the cursor.
    // meta(0, ...) maximizes omitted; add returned's extra digits plus a newline.
    let metadata_reserve = meta(0, Some(total))
        .chars()
        .count()
        .max(meta(0, None).chars().count())
        + total.to_string().len();
    debug_assert!(metadata_reserve <= 512);
    if max_chars < metadata_reserve {
        let summary = meta(0, (start < total).then_some(start));
        return if summary.chars().count() <= max_chars {
            (summary, Vec::new())
        } else {
            (String::new(), Vec::new())
        };
    }
    let mut remaining = max_chars - metadata_reserve;
    let mut text = String::new();
    let mut refs = Vec::new();
    for &(node, depth, parent) in &rows[start..] {
        // Native identity also prevents collisions after an MCP process restart,
        // when the adapter's generation counter starts over.
        let reference = format!(
            "s{generation}-{}-{}-{}",
            snapshot["epoch"].as_str().unwrap_or(""),
            snapshot["revision"].as_u64().unwrap_or(0),
            node["id"].as_i64().unwrap()
        );
        let mut limit = PREVIEW;
        let line = loop {
            let mut record = describe(node, &custom, limit);
            record.insert("ref".into(), json!(reference));
            record.insert("depth".into(), json!(depth));
            if let Some(parent) = parent {
                record.insert("parent".into(), json!(parent));
            }
            let line = Value::Object(record).to_string();
            if line.chars().count() < remaining {
                break Some(line);
            }
            if limit == 0 {
                break None;
            }
            limit /= 2;
        };
        let Some(line) = line else { break };
        remaining -= line.chars().count() + 1;
        text.push_str(&line);
        text.push('\n');
        refs.push((reference, node["handle"].clone()));
    }
    let next = start + refs.len();
    text.push_str(&meta(refs.len(), (next < total).then_some(next)));
    debug_assert!(text.chars().count() <= max_chars);
    (text, refs)
}

// Bound the *escaped* preview, not just its input. JSON escaping happens once,
// at record serialization; embedded newlines cannot create spurious records.
fn preview(text: &str, limit: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let cost = match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
            '\u{00}'..='\u{1f}' => 6,
            _ => 1,
        };
        if used + cost > limit {
            result.push('…');
            break;
        }
        result.push(c);
        used += cost;
        if used == limit && chars.peek().is_some() {
            result.push('…');
            break;
        }
    }
    result
}

fn describe(node: &Value, custom: &BTreeMap<i64, &Value>, limit: usize) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(role) = node["roleName"]
        .as_str()
        .filter(|r| !r.is_empty() && *r != "none")
    {
        out.insert("role".into(), json!(preview(role, limit)));
    }
    for field in ["label", "value", "identifier", "hint", "tooltip"] {
        if let Some(text) = node[field].as_str().filter(|s| !s.is_empty()) {
            let text = if field == "value" && node["flags"]["isObscured"] == true {
                "<redacted>"
            } else {
                text
            };
            out.insert(field.into(), json!(preview(text, limit)));
        }
    }
    let flags = &node["flags"];
    let mut states = Vec::new();
    // Native tristates: 0=unspecified, 1=true, 2=false; checked also has 3=mixed.
    for (flag, yes, no) in [
        ("isChecked", "checked", "unchecked"),
        ("isSelected", "selected", "unselected"),
        ("isToggled", "on", "off"),
        ("isExpanded", "expanded", "collapsed"),
        ("isRequired", "required", "optional"),
    ] {
        match flags[flag].as_i64() {
            Some(1) => states.push(yes),
            Some(2) => states.push(no),
            Some(3) if flag == "isChecked" => states.push("mixed"),
            _ => {}
        }
    }
    if node["enabled"] == false || flags["isEnabled"] == 2 {
        states.push("disabled");
    }
    if node["focused"] == true || flags["isFocused"] == 1 {
        states.push("focused");
    }
    for (flag, state) in [
        ("isObscured", "obscured"),
        ("isReadOnly", "readOnly"),
        ("isMultiline", "multiline"),
        ("isHeader", "header"),
        ("isLiveRegion", "liveRegion"),
        ("scopesRoute", "scopesRoute"),
        ("namesRoute", "namesRoute"),
        ("isInMutuallyExclusiveGroup", "mutuallyExclusive"),
        ("isKeyboardKey", "keyboardKey"),
        ("isAccessibilityFocusBlocked", "accessibilityFocusBlocked"),
    ] {
        if flags[flag] == true {
            states.push(state);
        }
    }
    if !states.is_empty() {
        out.insert("states".into(), json!(states));
    }
    let bits = node["actions"].as_u64().unwrap_or(0);
    let actions: Vec<_> = ACTIONS
        .iter()
        .enumerate()
        .filter_map(|(bit, name)| (bits & (1 << bit) != 0).then_some(*name))
        .collect();
    if !actions.is_empty() {
        out.insert("actions".into(), json!(actions));
    }
    if let Some(ids) = node["customActions"]
        .as_array()
        .filter(|ids| !ids.is_empty())
    {
        let actions: Vec<_> = ids
            .iter()
            .filter_map(Value::as_i64)
            .take(CUSTOM_ACTIONS)
            .map(|id| {
                let mut action = json!({"id": id});
                if let Some(label) = custom.get(&id).and_then(|a| a["label"].as_str()) {
                    action["label"] = json!(preview(label, limit));
                }
                action
            })
            .collect();
        out.insert("customActions".into(), json!(actions));
        if ids.len() > CUSTOM_ACTIONS {
            out.insert(
                "customActionsOmitted".into(),
                json!(ids.len() - CUSTOM_ACTIONS),
            );
        }
    }
    if flags["hasImplicitScrolling"] == true
        || bits & ((0b1111 << 2) | (1 << 23)) != 0
        || node["scrollChildren"].as_i64().is_some_and(|n| n > 0)
    {
        let mut scroll = Map::new();
        for field in [
            "scrollChildren",
            "scrollIndex",
            "scrollPosition",
            "scrollExtentMin",
            "scrollExtentMax",
        ] {
            if node[field].is_number() {
                scroll.insert(field.into(), node[field].clone());
            }
        }
        out.insert("scroll".into(), Value::Object(scroll));
    }
    if node["headingLevel"].as_u64().is_some_and(|n| n > 0) {
        out.insert("headingLevel".into(), node["headingLevel"].clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, label: &str, children: &[i64]) -> Value {
        json!({"id": id, "label": label, "roleName": "none", "children": children,
            "enabled": true, "focused": false, "flags": {}, "actions": 0,
            "handle": {"epoch": "epoch", "viewId": 7, "nodeId": id, "revision": 3}})
    }
    fn snapshot(nodes: Vec<Value>) -> Value {
        json!({"epoch": "epoch", "viewId": 7, "revision": 3, "nodes": nodes})
    }
    #[test]
    fn refs_include_native_identity_even_if_adapter_generation_restarts() {
        let mut input = snapshot(vec![node(0, "Save", &[])]);
        let (_, first) = format(&input, 4000, 0, 1);
        input["epoch"] = json!("restarted-runtime");
        let (_, restarted) = format(&input, 4000, 0, 1);
        assert_ne!(first[0].0, restarted[0].0);
        input["revision"] = json!(4);
        let (_, updated) = format(&input, 4000, 0, 1);
        assert_ne!(restarted[0].0, updated[0].0);
    }

    fn lines(text: &str) -> Vec<Value> {
        text.lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn metadata(text: &str) -> Value {
        lines(text).last().unwrap()["meta"].clone()
    }

    #[test]
    fn zero_tiny_and_unicode_budgets_are_complete() {
        let input = snapshot(vec![node(0, "🦀漢字\n\"\\\u{00}", &[])]);
        for budget in 0..700 {
            let (text, refs) = format(&input, budget, 0, 8);
            assert!(text.chars().count() <= budget);
            let records = lines(&text);
            assert_eq!(
                refs.len(),
                records.iter().filter(|n| n.get("ref").is_some()).count()
            );
            if let Some(last) = records.last() {
                assert!(last.get("meta").is_some());
                assert!(last.to_string().chars().count() <= 512);
            }
        }
        assert_eq!(format(&input, 0, 0, 8), (String::new(), vec![]));
        let (text, _) = format(&input, 1000, 0, 8);
        assert_eq!(lines(&text)[0]["label"], input["nodes"][0]["label"]);
    }

    #[test]
    fn hidden_subtrees_are_excluded_and_empty_wrappers_collapse() {
        let mut hidden = node(2, "secret", &[3]);
        hidden["flags"]["isHidden"] = json!(true);
        let input = snapshot(vec![
            node(0, "root", &[1, 2]),
            node(1, "", &[4]),
            node(4, "visible", &[]),
            hidden,
            node(3, "hidden descendant", &[]),
        ]);
        let (text, refs) = format(&input, 4000, 0, 1);
        assert_eq!(refs.len(), 2);
        assert!(!text.contains("secret"));
        assert!(!text.contains("hidden descendant"));
        assert_eq!(lines(&text)[1]["depth"], 1);
        assert_eq!(lines(&text)[1]["parent"], 0);
        assert_eq!(metadata(&text)["hidden"], 2);
        assert_eq!(metadata(&text)["collapsed"], 1);
        let mut input = input;
        input["nodes"][0]["hidden"] = json!(true);
        assert!(format(&input, 4000, 0, 1).1.is_empty());
    }

    #[test]
    fn obscured_values_are_defensively_redacted_and_native_details_do_not_leak() {
        let mut n = node(0, "Password", &[]);
        n["flags"]["isObscured"] = json!(true);
        for field in ["value", "increasedValue", "decreasedValue"] {
            n[field] = json!("TOP SECRET");
        }
        n["rect"] = json!([1, 2, 3, 4]);
        n["viewTransform"] = json!([1, 2, 3]);
        let input = snapshot(vec![n]);
        let (text, refs) = format(&input, 2000, 0, 42);
        assert!(text.contains("<redacted>"));
        for forbidden in [
            "TOP SECRET",
            "flags",
            "rect",
            "Transform",
            "handle",
            "nodeId",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        assert_eq!(
            refs,
            vec![("s42-epoch-3-0".into(), input["nodes"][0]["handle"].clone())]
        );
    }

    #[test]
    fn states_actions_roles_and_scroll_are_named() {
        let mut n = node(0, "", &[]);
        n["roleName"] = json!("textField");
        n["enabled"] = json!(false);
        n["focused"] = json!(true);
        n["flags"] = json!({"isChecked": 3, "isSelected": 2, "isToggled": 1,
            "isExpanded": 2, "isRequired": 1, "isReadOnly": true, "isMultiline": true});
        n["actions"] = json!((1u64 << 21) | (1 << 5) | 1);
        n["scrollChildren"] = json!(20);
        n["customActions"] = json!([9]);
        let mut input = snapshot(vec![n]);
        input["customActions"] = json!([{"id": 9, "label": "Archive"}]);
        let (text, _) = format(&input, 4000, 0, 0);
        let record = &lines(&text)[0];
        assert_eq!(record["role"], "textField");
        for state in [
            "mixed",
            "unselected",
            "on",
            "collapsed",
            "required",
            "disabled",
            "focused",
            "readOnly",
            "multiline",
        ] {
            assert!(record["states"].as_array().unwrap().contains(&json!(state)));
        }
        assert_eq!(record["actions"], json!(["tap", "scrollDown", "setText"]));
        assert_eq!(record["scroll"]["scrollChildren"], 20);
        assert_eq!(
            record["customActions"][0],
            json!({"id": 9, "label": "Archive"})
        );
    }

    #[test]
    fn empty_trees_and_out_of_range_offsets() {
        for input in [
            json!({}),
            snapshot(vec![]),
            snapshot(vec![node(0, "", &[])]),
        ] {
            let (text, refs) = format(&input, 1000, usize::MAX, 0);
            assert!(refs.is_empty());
            assert_eq!(metadata(&text)["total"], 0);
            assert!(metadata(&text)["nextOffset"].is_null());
        }
        let (text, refs) = format(
            &snapshot(vec![node(0, "Text without role", &[])]),
            1000,
            usize::MAX,
            0,
        );
        assert!(refs.is_empty());
        assert_eq!(metadata(&text)["offset"], 1);
        assert_eq!(metadata(&text)["omitted"], 1);
    }

    #[test]
    fn continuation_has_no_refs_for_omitted_nodes_and_long_labels_do_not_block() {
        let input = snapshot(
            (0..12)
                .map(|id| node(id, &"🦀\n\u{00}".repeat(2000), &[]))
                .collect(),
        );
        let mut offset = 0;
        let mut all_refs = Vec::new();
        loop {
            let (text, refs) = format(&input, 520, offset, 99);
            assert!(text.chars().count() <= 520);
            assert!(!refs.is_empty());
            let records = lines(&text);
            for (record, (reference, handle)) in records.iter().zip(&refs) {
                assert_eq!(record["ref"], *reference);
                assert_eq!(*handle, input["nodes"][all_refs.len()]["handle"]);
                all_refs.push(reference.clone());
            }
            let meta = metadata(&text);
            assert_eq!(meta["omitted"], 12 - refs.len());
            let Some(next) = meta["nextOffset"].as_u64() else {
                break;
            };
            assert!(next as usize > offset);
            offset = next as usize;
        }
        assert_eq!(
            all_refs,
            (0..12)
                .map(|id| format!("s99-epoch-3-{id}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn focused_continuation_keeps_grouping_without_ancestor_refs() {
        let input = snapshot(vec![
            node(0, "Dialog", &[1]),
            node(1, "", &[2]),
            node(2, "Save", &[]),
        ]);
        let (text, refs) = format(&input, 1000, 1, 4);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, "s4-epoch-3-2");
        let record = &lines(&text)[0];
        assert_eq!(record["depth"], 1);
        assert_eq!(record["parent"], 0);
        assert_eq!(record["label"], "Save");
        assert!(metadata(&text)["nextOffset"].is_null());
    }

    #[test]
    fn metadata_reserves_counter_widths_and_blocked_pages_keep_the_cursor() {
        let mut input = snapshot((0..101).map(|id| node(id, "Text", &[])).collect());
        input["epoch"] = json!("\u{00}".repeat(1000));
        input["viewId"] = json!(i64::MIN);
        input["revision"] = json!(u64::MAX);
        for offset in [0, 25, 100, usize::MAX] {
            for budget in [0, 1, 250, 300, 400, 800, 4000] {
                let (text, refs) = format(&input, budget, offset, u64::MAX);
                assert!(text.chars().count() <= budget);
                if text.is_empty() {
                    assert!(refs.is_empty());
                    continue;
                }
                let meta = metadata(&text);
                assert!(lines(&text).last().unwrap().to_string().chars().count() <= 512);
                assert_eq!(meta["omitted"], 101 - refs.len());
                let next = offset.min(101) + refs.len();
                if next < 101 {
                    assert_eq!(meta["nextOffset"], next);
                } else {
                    assert!(meta["nextOffset"].is_null());
                }
            }
        }
    }

    #[test]
    fn text_and_custom_action_previews_are_escaped_and_bounded() {
        let mut n = node(0, &"\u{00}".repeat(10000), &[]);
        n["customActions"] = json!((0..100).collect::<Vec<_>>());
        let mut input = snapshot(vec![n]);
        input["customActions"] = json!(
            (0..100)
                .map(|id| json!({"id": id, "label": "\"\n".repeat(10000)}))
                .collect::<Vec<_>>()
        );
        let (text, refs) = format(&input, 4000, 0, 1);
        assert_eq!(refs.len(), 1);
        let record = &lines(&text)[0];
        assert!(record["label"].to_string().chars().count() <= PREVIEW + 3);
        assert_eq!(
            record["customActions"].as_array().unwrap().len(),
            CUSTOM_ACTIONS
        );
        assert_eq!(record["customActionsOmitted"], 100 - CUSTOM_ACTIONS);
    }
}
