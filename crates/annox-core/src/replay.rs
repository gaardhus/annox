//! Deriving state from events (§2.5), for annotation logs and document logs
//! (§5.5.1).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Map, Value};

use crate::event::Event;

/// The causal graph of one log: the events of one annotation or document,
/// without dangling events (§2.5.1).
pub struct Log<'a> {
    pub root: &'a Event,
    /// Live (non-dangling) events by id.
    pub events: BTreeMap<&'a str, &'a Event>,
    ancestors: HashMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> Log<'a> {
    /// Builds the log rooted at `root_id` from events belonging to it. Returns
    /// `None` if the root event is missing or has a non-empty `after`.
    pub fn build(events: impl IntoIterator<Item = &'a Event>, root_id: &str) -> Option<Log<'a>> {
        let mut by_id: BTreeMap<&str, &Event> = BTreeMap::new();
        for e in events {
            by_id.entry(e.id.as_str()).or_insert(e);
        }
        let root = *by_id.get(root_id)?;
        if !root.after.is_empty() {
            return None;
        }
        let mut live: BTreeSet<&str> = BTreeSet::from([root.id.as_str()]);
        loop {
            let before = live.len();
            for (id, e) in &by_id {
                if !live.contains(id)
                    && !e.after.is_empty()
                    && e.after.iter().all(|a| live.contains(a.as_str()))
                {
                    live.insert(id);
                }
            }
            if live.len() == before {
                break;
            }
        }
        let events: BTreeMap<&str, &Event> = by_id.into_iter().filter(|(id, _)| live.contains(id)).collect();
        let mut log = Log { root, events, ancestors: HashMap::new() };
        let ids: Vec<&str> = log.events.keys().copied().collect();
        for id in ids {
            log.compute_ancestors(id);
        }
        Some(log)
    }

    fn compute_ancestors(&mut self, id: &'a str) -> BTreeSet<&'a str> {
        if let Some(a) = self.ancestors.get(id) {
            return a.clone();
        }
        let mut result = BTreeSet::new();
        let parents: Vec<&'a str> = self.events[id].after.iter().map(|s| s.as_str()).collect();
        for parent in parents {
            result.insert(parent);
            result.extend(self.compute_ancestors(parent));
        }
        self.ancestors.insert(id, result.clone());
        result
    }

    /// Whether `a` is an ancestor of `b`.
    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.ancestors.get(b).is_some_and(|set| set.contains(a))
    }

    /// The log's heads: events that no other live event lists in `after`
    /// (§2.5.1). A new event's `after` is exactly this list.
    pub fn heads(&self) -> Vec<String> {
        let referenced: BTreeSet<&str> =
            self.events.values().flat_map(|e| e.after.iter().map(|s| s.as_str())).collect();
        self.events.keys().filter(|id| !referenced.contains(*id)).map(|s| s.to_string()).collect()
    }

    /// The heads among `writers`: those not an ancestor of another writer
    /// (§2.5.2), sorted by id.
    pub fn field_heads<'b>(&self, writers: &[&'b str]) -> Vec<&'b str> {
        let mut heads: Vec<&str> = writers
            .iter()
            .copied()
            .filter(|w| !writers.iter().any(|o| o != w && self.is_ancestor(w, o)))
            .collect();
        heads.sort_unstable();
        heads
    }
}

/// Fields written by each event, as computed by a log-specific function.
type Writes = BTreeMap<&'static str, Value>;

/// Field values and conflicts of a log (§2.5.2, §2.5.3).
pub struct Fields<'a> {
    pub values: BTreeMap<&'static str, Value>,
    /// Conflicted fields and their competing event ids, sorted.
    pub conflicts: BTreeMap<&'static str, Vec<String>>,
    /// The event that supplied each field's (provisional) value.
    pub sources: BTreeMap<&'static str, &'a Event>,
    pub writes: BTreeMap<&'a str, Writes>,
}

fn derive_fields<'a>(log: &Log<'a>, writes_of: impl Fn(&Event) -> Writes) -> Fields<'a> {
    let writes: BTreeMap<&str, Writes> = log.events.iter().map(|(id, e)| (*id, writes_of(e))).collect();
    let names: BTreeSet<&'static str> = writes.values().flat_map(|w| w.keys().copied()).collect();
    let mut fields = Fields { values: BTreeMap::new(), conflicts: BTreeMap::new(), sources: BTreeMap::new(), writes };
    for name in names {
        let writers: Vec<&str> =
            fields.writes.iter().filter(|(_, w)| w.contains_key(name)).map(|(id, _)| *id).collect();
        let heads = log.field_heads(&writers);
        let mut source = *heads.last().expect("a written field has a head");
        let mut conflicted = heads.len() > 1;
        // Special rule 1: anchor rewrites never conflict (§2.5.3).
        if name == "target" && conflicted {
            let human: Vec<&str> =
                heads.iter().copied().filter(|h| log.events[h].kind != "reanchor").collect();
            if human.len() <= 1 {
                conflicted = false;
                if let Some(h) = human.first() {
                    source = h;
                }
            }
        }
        fields.values.insert(name, fields.writes[source][name].clone());
        fields.sources.insert(name, log.events[source]);
        if conflicted {
            fields.conflicts.insert(name, heads.iter().map(|s| s.to_string()).collect());
        }
    }
    fields
}

fn str_field<'e>(e: &'e Event, name: &str) -> Option<&'e str> {
    e.field(name).and_then(Value::as_str)
}

/// The fields an annotation event writes, or none if it is invalid or
/// unknown (§2.4, §2.5.2).
fn annotation_writes(kind: &str, root_id: &str, e: &Event) -> Writes {
    let is_root = kind == "comment" || kind == "suggestion";
    let mut w = Writes::new();
    match e.kind.as_str() {
        "create" => {
            if e.id != root_id {
                return w;
            }
            w.insert("body", e.field("body").cloned().unwrap_or(Value::Null));
            w.insert("deleted", json!(false));
            if is_root {
                w.insert("label", e.field("label").cloned().unwrap_or(Value::Null));
                w.insert("status", json!(["open", null]));
                w.insert("target", e.field("target").cloned().unwrap_or(Value::Null));
            }
            if kind == "suggestion" {
                w.insert("replacement", e.field("edit").and_then(|v| v.get("replacement")).cloned().unwrap_or(Value::Null));
            }
        }
        "edit" => {
            if let Some(body) = e.field("body") {
                let valid = body.is_null() || body.is_string();
                let empty = body.as_str().is_none_or(str::is_empty);
                if !valid || (kind == "reply" && empty) {
                    return Writes::new();
                }
                w.insert("body", body.clone());
            }
            if let Some(label) = e.field("label") {
                if kind == "reply" || !(label.is_null() || label.is_string()) {
                    return Writes::new();
                }
                w.insert("label", label.clone());
            }
        }
        "status" if is_root => {
            let allowed: &[&str] = if kind == "comment" {
                &["open", "resolved"]
            } else {
                &["open", "accepted", "rejected", "withdrawn"]
            };
            let Some(status) = str_field(e, "status").filter(|s| allowed.contains(s)) else {
                return w;
            };
            let applied = e.field("appliedVersion");
            if (status == "accepted") != applied.is_some() {
                return w;
            }
            w.insert("status", json!([status, applied.cloned().unwrap_or(Value::Null)]));
        }
        "reanchor" if is_root => {
            if let Some(t) = e.field("target").filter(|t| t.is_object()) {
                w.insert("target", t.clone());
            }
        }
        "retarget" if kind == "suggestion" => {
            let target = e.field("target").filter(|t| t.is_object());
            let replacement = e.field("edit").and_then(|v| v.get("replacement")).filter(|r| r.is_string());
            if let (Some(t), Some(r)) = (target, replacement) {
                w.insert("target", t.clone());
                w.insert("replacement", r.clone());
            }
        }
        "delete" => {
            w.insert("deleted", json!(true));
        }
        "restore" => {
            w.insert("deleted", json!(false));
        }
        _ => {}
    }
    w
}

/// Derives the state of annotation `id` (§2.5.6) from events, which may
/// include events of other annotations. Returns `None` if the annotation has
/// no valid `create` event.
pub fn derive_annotation(events: &[Event], id: &str) -> Option<Value> {
    let own = events.iter().filter(|e| e.annotation.as_deref() == Some(id));
    let log = Log::build(own, id)?;
    let create = log.root;
    if create.kind != "create" {
        return None;
    }
    let kind = str_field(create, "kind")?;
    match kind {
        "comment" | "suggestion" if create.has("target") && !create.has("parent") => {}
        "reply" if create.has("parent") && !create.has("target") => {}
        _ => return None,
    }
    let fields = derive_fields(&log, |e| annotation_writes(kind, id, e));
    let mut conflicts = fields.conflicts.clone();

    // Special rule 2: delete conflicts with changes the deleter hadn't seen.
    if !conflicts.contains_key("deleted") {
        let delete = fields.sources["deleted"];
        if delete.kind == "delete" {
            let mut others: Vec<String> = fields
                .writes
                .iter()
                .filter(|(eid, w)| {
                    !w.is_empty()
                        && log.events[*eid].kind != "reanchor"
                        && **eid != delete.id
                        && !log.is_ancestor(eid, &delete.id)
                })
                .map(|(eid, _)| eid.to_string())
                .collect();
            if !others.is_empty() {
                others.push(delete.id.clone());
                others.sort();
                conflicts.insert("deleted", others);
            }
        }
    }

    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("kind".into(), json!(kind));
    out.insert("author".into(), create.author.clone());
    out.insert("created".into(), json!(create.time));
    if kind == "reply" {
        out.insert("parent".into(), create.field("parent").cloned().unwrap_or(Value::Null));
    } else {
        out.insert("target".into(), fields.values["target"].clone());
    }
    out.insert("body".into(), fields.values["body"].clone());
    if kind != "reply" {
        out.insert("label".into(), fields.values["label"].clone());
        let status = &fields.values["status"];
        out.insert("status".into(), status[0].clone());
        if kind == "suggestion" {
            out.insert("edit".into(), json!({ "replacement": fields.values["replacement"] }));
            let source = fields.sources["replacement"];
            let retargeted = if source.kind == "retarget" {
                json!({ "author": source.author, "time": source.time })
            } else {
                Value::Null
            };
            out.insert("retargetedBy".into(), retargeted);
            if status[0] == "accepted" {
                out.insert("appliedVersion".into(), status[1].clone());
            }
        }
    }
    out.insert("deleted".into(), fields.values["deleted"].clone());
    out.insert("conflicts".into(), json!(conflicts));
    Some(Value::Object(out))
}

/// The derived state of a document record (§5.5.1).
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentState {
    pub path: Option<String>,
    pub merged_into: Option<String>,
    pub conflicts: BTreeMap<&'static str, Vec<String>>,
}

fn document_writes(e: &Event, may_merge: &dyn Fn(&str, &str) -> bool) -> Writes {
    let mut w = Writes::new();
    match e.kind.as_str() {
        "document" | "move" => {
            if let Some(p) = e.field("path").filter(|p| p.is_string()) {
                w.insert("path", p.clone());
            }
        }
        "merged" => {
            let into = str_field(e, "into");
            if let (Some(into), Some(doc)) = (into, e.document.as_deref()) {
                if may_merge(doc, into) {
                    w.insert("mergedInto", json!(into));
                }
            }
        }
        _ => {}
    }
    w
}

/// Derives the state of document record `id` from its events. Returns `None`
/// if there is no valid `document` event. `may_merge(doc, into)` decides
/// whether a `merged` event is valid (§5.5.1), which depends on areas.
pub fn derive_document(
    events: &[Event],
    id: &str,
    may_merge: &dyn Fn(&str, &str) -> bool,
) -> Option<DocumentState> {
    let own = events.iter().filter(|e| e.document.as_deref() == Some(id));
    let log = Log::build(own, id)?;
    if log.root.kind != "document" {
        return None;
    }
    let fields = derive_fields(&log, |e| document_writes(e, may_merge));
    Some(DocumentState {
        path: fields.values.get("path").and_then(Value::as_str).map(str::to_owned),
        merged_into: fields.values.get("mergedInto").and_then(Value::as_str).map(str::to_owned),
        conflicts: fields.conflicts,
    })
}

/// The heads of document record `id`, for the `after` of a new event.
pub fn document_heads(events: &[Event], id: &str) -> Vec<String> {
    let own = events.iter().filter(|e| e.document.as_deref() == Some(id));
    Log::build(own, id).map(|log| log.heads()).unwrap_or_default()
}

/// The heads of annotation `id`, for the `after` of a new event (§2.5.1).
pub fn annotation_heads(events: &[Event], id: &str) -> Vec<String> {
    let own = events.iter().filter(|e| e.annotation.as_deref() == Some(id));
    Log::build(own, id).map(|log| log.heads()).unwrap_or_default()
}
