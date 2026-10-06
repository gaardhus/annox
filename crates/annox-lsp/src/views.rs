//! Wire types of the extension methods (§6.6.1): AnnotationView,
//! ConflictEntry, and DocumentInfo.

use annox_core::anchor::State;
use annox_core::event::Event;
use annox_core::storage::Area;
use serde_json::{json, Map, Value};

use crate::position::{Encoding, LineIndex};
use crate::{Analysis, Item};

/// The value a conflicting event wrote to `field`, for a ConflictEntry.
fn conflict_value(field: &str, e: &Event) -> Value {
    match field {
        "status" if e.kind == "create" => json!("open"),
        "status" => e.field("status").cloned().unwrap_or(Value::Null),
        "replacement" => e.field("edit").and_then(|v| v.get("replacement")).cloned().unwrap_or(Value::Null),
        "deleted" => json!(e.kind),
        _ => e.field(field).cloned().unwrap_or(Value::Null),
    }
}

fn entry(e: &Event, value: Value) -> Value {
    json!({ "event": e.id, "author": e.author, "time": e.time, "value": value })
}

/// `conflicts` with event ids expanded into ConflictEntries.
fn expand_conflicts(a: &Analysis, conflicts: &Value) -> Value {
    let mut out = Map::new();
    for (field, ids) in conflicts.as_object().into_iter().flatten() {
        let entries: Vec<Value> = ids
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| a.index.events.get(id.as_str()?))
            .map(|(e, _)| entry(e, conflict_value(field, e)))
            .collect();
        out.insert(field.clone(), json!(entries));
    }
    Value::Object(out)
}

/// A derived state turned into a view: no `target`, expanded conflicts, and
/// the `local` flag.
fn base_view(a: &Analysis, state: &Value) -> Map<String, Value> {
    let mut view = state.as_object().cloned().unwrap_or_default();
    view.remove("target");
    let id = state["id"].as_str().unwrap_or_default();
    view.insert("conflicts".into(), expand_conflicts(a, &state["conflicts"]));
    view.insert("local".into(), json!(a.area(id) == Area::Local));
    view
}

fn reply_view(a: &Analysis, state: &Value) -> Value {
    Value::Object(base_view(a, state))
}

fn item_view(a: &Analysis, item: &Item, lines: &LineIndex, include_deleted: bool) -> Value {
    let mut view = base_view(a, &item.state);
    let state = match item.resolution.state {
        State::Exact => "exact",
        State::Relocated => "relocated",
        State::Orphaned => "orphaned",
    };
    let mut resolution = json!({ "state": state, "step": item.resolution.step });
    if let Some((s, e)) = item.resolution.range {
        resolution["range"] = json!(lines.range(s, e));
    }
    if let Some(s) = item.suggested {
        let score = (s.score * 100.0).round() / 100.0;
        resolution["suggested"] = json!({ "range": lines.range(s.range.0, s.range.1), "score": score });
    }
    view.insert("resolution".into(), resolution);
    if item.kind() == "suggestion" && item.status() == "open" {
        view.insert("applicable".into(), json!(item.applicable()));
    }
    let replies: Vec<Value> = a
        .replies
        .get(&item.id)
        .into_iter()
        .flatten()
        .filter(|r| include_deleted || r["deleted"] == json!(false))
        .map(|r| reply_view(a, r))
        .collect();
    view.insert("replies".into(), json!(replies));
    Value::Object(view)
}

fn is_closed(item: &Item) -> bool {
    item.status() != "open"
}

/// AnnotationViews of a document's roots, filtered as requested (§6.6.2).
pub(crate) fn views(a: &Analysis, encoding: Encoding, include_closed: bool, include_deleted: bool) -> Vec<Value> {
    let lines = LineIndex::new(&a.text, encoding);
    a.items
        .iter()
        .filter(|i| include_closed || !is_closed(i))
        .filter(|i| include_deleted || !i.deleted())
        .map(|i| item_view(a, i, &lines, include_deleted))
        .collect()
}

/// The view of one root or reply, regardless of filters.
pub(crate) fn view_by_id(a: &Analysis, id: &str, encoding: Encoding) -> Option<Value> {
    let lines = LineIndex::new(&a.text, encoding);
    if let Some(item) = a.item(id) {
        return Some(item_view(a, item, &lines, true));
    }
    a.reply(id).map(|r| reply_view(a, r))
}

/// DocumentInfo for the document records at the analyzed path (§6.6.2).
pub(crate) fn document_info(a: &Analysis) -> Value {
    let mut path_conflicts = Vec::new();
    for doc in &a.loaded.at {
        let Some(state) = a.index.documents.get(doc) else { continue };
        for id in state.conflicts.get("path").into_iter().flatten() {
            let event = a.index.document_events[doc].iter().find(|(e, _)| &e.id == id);
            if let Some((e, _)) = event {
                path_conflicts.push(entry(e, e.field("path").cloned().unwrap_or(Value::Null)));
            }
        }
    }
    let conflicts = if path_conflicts.is_empty() { json!({}) } else { json!({ "path": path_conflicts }) };
    json!({ "documents": a.loaded.at, "conflicts": conflicts, "duplicates": a.loaded.at.len() > 1 })
}

/// A per-item error in `acceptAll` results.
pub(crate) fn result_error(id: &str, code: i32, message: &str) -> Value {
    json!({ "annotation": id, "error": { "code": code, "message": message } })
}
