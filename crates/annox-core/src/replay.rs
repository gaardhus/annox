//! Deriving state from events (§2.5), for annotation logs and document logs
//! (§5.5.1).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde_json::{json, Map, Value};

use crate::event::Event;

/// The causal graph of one log: the events of one annotation or document,
/// without dangling events (§2.5.1).
pub struct Log<'a> {
    pub root: &'a Event,
    /// Live (non-dangling) events by id.
    pub events: BTreeMap<&'a str, &'a Event>,
    /// The live events in display order (§2.5.5).
    order: Vec<&'a Event>,
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
        // An event is live once every event in its `after` is, starting from
        // the root. One that follows a missing event, or is in a cycle, which
        // hand-edited files can make, never is. Other events with no `after`
        // aren't either, as only the root may have none. Taking the smallest
        // id among those ready gives the display order (§2.5.5).
        let mut waiting: HashMap<&str, usize> = HashMap::new();
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for (id, e) in &by_id {
            if e.after.is_empty() {
                continue;
            }
            let parents: BTreeSet<&str> = e.after.iter().map(String::as_str).collect();
            waiting.insert(id, parents.len());
            for parent in parents {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut ready = BTreeSet::from([root.id.as_str()]);
        let mut order = Vec::new();
        while let Some(id) = ready.pop_first() {
            order.push(by_id[id]);
            for &child in children.get(id).into_iter().flatten() {
                let n = waiting.get_mut(child).expect("every child waits");
                *n -= 1;
                if *n == 0 {
                    ready.insert(child);
                }
            }
        }
        let events = order.iter().map(|e| (e.id.as_str(), *e)).collect();
        Some(Log { root, events, order })
    }

    /// Every ancestor of the events `ids`, not counting them unless one is
    /// an ancestor of another. Unknown ids have none.
    pub fn ancestors_of<'b>(&self, ids: impl IntoIterator<Item = &'b str>) -> HashSet<&'a str> {
        let mut found = HashSet::new();
        let mut stack: Vec<&'a str> = Vec::new();
        let parents = |id: &str| self.events.get(id).into_iter().flat_map(|e| e.after.iter().map(String::as_str));
        for id in ids {
            stack.extend(parents(id));
        }
        while let Some(id) = stack.pop() {
            if found.insert(id) {
                stack.extend(parents(id));
            }
        }
        found
    }

    /// The log's heads: events that no other live event lists in `after`
    /// (§2.5.1). A new event's `after` is exactly this list.
    pub fn heads(&self) -> Vec<String> {
        let referenced: BTreeSet<&str> =
            self.events.values().flat_map(|e| e.after.iter().map(|s| s.as_str())).collect();
        self.events.keys().filter(|id| !referenced.contains(*id)).map(|s| s.to_string()).collect()
    }

    /// The events in display order: topological along `after` links, ties by
    /// ascending id (§2.5.5).
    pub fn ordered(&self) -> &[&'a Event] {
        &self.order
    }

    /// The heads among `writers`: those not an ancestor of another writer
    /// (§2.5.2), sorted by id.
    pub fn field_heads<'b>(&self, writers: &[&'b str]) -> Vec<&'b str> {
        let behind = self.ancestors_of(writers.iter().copied());
        let mut heads: Vec<&str> = writers.iter().copied().filter(|w| !behind.contains(w)).collect();
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
            let human: Vec<&str> = heads.iter().copied().filter(|h| log.events[h].kind != "reanchor").collect();
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

/// Whether `e` has the fields a `create` of `kind` requires and none it
/// mustn't have (§2.4). Anything else is invalid, and with an invalid
/// `create` there is no annotation: a suggestion without a replacement must
/// not derive one that accepting would turn into a deletion.
fn valid_create(kind: &str, e: &Event) -> bool {
    let absent = |name: &str| !e.has(name);
    let optional_str = |name: &str| e.field(name).is_none_or(|v| v.is_null() || v.is_string());
    match kind {
        "comment" | "suggestion" => {
            let edit = if kind == "suggestion" {
                e.field("edit").and_then(|v| v.get("replacement")).is_some_and(Value::is_string)
                    && e.field("reverts").is_none_or(Value::is_string)
            } else {
                absent("edit") && absent("reverts")
            };
            e.field("target").is_some_and(Value::is_object)
                && absent("parent")
                && optional_str("body")
                && optional_str("label")
                && edit
        }
        "reply" => {
            e.field("parent").is_some_and(Value::is_string)
                && str_field(e, "body").is_some_and(|b| !b.is_empty())
                && ["target", "label", "edit", "reverts"].into_iter().all(absent)
        }
        _ => false,
    }
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
                w.insert(
                    "replacement",
                    e.field("edit").and_then(|v| v.get("replacement")).cloned().unwrap_or(Value::Null),
                );
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
            let allowed: &[&str] =
                if kind == "comment" { &["open", "resolved"] } else { &["open", "accepted", "rejected", "withdrawn"] };
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

/// The reaction that marks approval of a suggestion (§4.4.2).
pub const APPROVAL: &str = "👍";

/// The author id and reaction of a valid `react` event, and whether it removes
/// the reaction (§2.4).
fn react(e: &Event) -> Option<(&str, &str, bool)> {
    if e.kind != "react" {
        return None;
    }
    let author = e.author.get("id").and_then(Value::as_str)?;
    let reaction = str_field(e, "reaction").filter(|r| !r.is_empty())?;
    let removed = match e.field("removed") {
        None => false,
        Some(r) => r.as_bool()?,
    };
    Some((author, reaction, removed))
}

/// The reactions present on a log (§2.5.7): for each reaction, the events
/// that added it, sorted by id. Each author and reaction is a field of its
/// own, decided by its head with the greatest id, so it never conflicts.
fn reactions<'a>(log: &Log<'a>) -> BTreeMap<&'a str, Vec<&'a Event>> {
    let mut writers: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
    for (id, e) in &log.events {
        if let Some((author, reaction, _)) = react(e) {
            writers.entry((author, reaction)).or_default().push(id);
        }
    }
    let mut present: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for ((_, reaction), ids) in writers {
        let head = log.events[log.field_heads(&ids).last().expect("a written field has a head")];
        if react(head).is_some_and(|(_, _, removed)| !removed) {
            present.entry(reaction).or_default().push(head);
        }
    }
    for events in present.values_mut() {
        events.sort_by(|a, b| a.id.cmp(&b.id));
    }
    present
}

fn by(e: &Event) -> Value {
    json!({ "author": e.author, "time": e.time })
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
    if !valid_create(kind, create) {
        return None;
    }
    let fields = derive_fields(&log, |e| annotation_writes(kind, id, e));
    let mut conflicts = fields.conflicts.clone();

    // Special rule 2: delete conflicts with changes the deleter hadn't seen.
    // `react` events write no fields here, so they never count (§2.5.7).
    if !conflicts.contains_key("deleted") {
        let delete = fields.sources["deleted"];
        if delete.kind == "delete" {
            let seen = log.ancestors_of([delete.id.as_str()]);
            let mut others: Vec<String> = fields
                .writes
                .iter()
                .filter(|(eid, w)| {
                    !w.is_empty() && log.events[*eid].kind != "reanchor" && **eid != delete.id && !seen.contains(*eid)
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
    let source = fields.sources["body"];
    out.insert("editedBy".into(), if source.kind == "edit" { by(source) } else { Value::Null });
    let reactions = reactions(&log);
    if kind != "reply" {
        out.insert("label".into(), fields.values["label"].clone());
        let status = &fields.values["status"];
        out.insert("status".into(), status[0].clone());
        if kind == "suggestion" {
            out.insert("edit".into(), json!({ "replacement": fields.values["replacement"] }));
            let source = fields.sources["replacement"];
            out.insert("retargetedBy".into(), if source.kind == "retarget" { by(source) } else { Value::Null });
            // An approval is stale if it hasn't seen every head of the
            // replacement (§2.5.7).
            let replacement_heads: Vec<String> =
                fields.conflicts.get("replacement").cloned().unwrap_or_else(|| vec![source.id.clone()]);
            let approvals: Vec<Value> = reactions
                .get(APPROVAL)
                .into_iter()
                .flatten()
                .map(|e| {
                    let seen = log.ancestors_of([e.id.as_str()]);
                    let stale = replacement_heads.iter().any(|h| !seen.contains(h.as_str()));
                    json!({ "author": e.author, "time": e.time, "stale": stale })
                })
                .collect();
            out.insert("approvals".into(), json!(approvals));
            if let Some(reverts) = create.field("reverts").filter(|r| r.is_string()) {
                out.insert("reverts".into(), reverts.clone());
            }
            if status[0] == "accepted" {
                out.insert("appliedVersion".into(), status[1].clone());
            }
        }
    }
    out.insert("deleted".into(), fields.values["deleted"].clone());
    let reactions: Map<String, Value> =
        reactions.into_iter().map(|(r, events)| (r.to_owned(), events.into_iter().map(by).collect())).collect();
    out.insert("reactions".into(), Value::Object(reactions));
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
pub fn derive_document(events: &[Event], id: &str, may_merge: &dyn Fn(&str, &str) -> bool) -> Option<DocumentState> {
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

/// The events of annotation `id` in display order: topological along `after`
/// links, ties by ascending id (§2.5.5).
pub fn ordered_events(events: &[Event], id: &str) -> Vec<Event> {
    let own = events.iter().filter(|e| e.annotation.as_deref() == Some(id));
    let Some(log) = Log::build(own, id) else { return vec![] };
    log.ordered().iter().map(|e| (*e).clone()).collect()
}

/// The events of the thread that annotation `id` belongs to: its root's and
/// every reply's. Each annotation's events keep the order of
/// [`ordered_events`], and the annotations are interleaved by ascending event
/// id (§2.5.5).
pub fn thread_events(events: &[Event], id: &str) -> Vec<Event> {
    fn parent(e: &Event) -> Option<&str> {
        e.field("parent").and_then(Value::as_str)
    }
    let is_reply = |e: &&Event| e.kind == "create" && e.field("kind") == Some(&json!("reply"));
    let root = events.iter().filter(is_reply).find(|e| e.id == id).and_then(|e| parent(e)).unwrap_or(id);
    let replies: BTreeSet<&str> =
        events.iter().filter(is_reply).filter(|e| parent(e) == Some(root)).map(|e| e.id.as_str()).collect();
    let mut logs: Vec<VecDeque<Event>> =
        std::iter::once(root).chain(replies).map(|a| ordered_events(events, a).into()).collect();
    let mut out = Vec::new();
    while let Some(next) = logs.iter_mut().filter(|l| !l.is_empty()).min_by(|a, b| a[0].id.cmp(&b[0].id)) {
        out.extend(next.pop_front());
    }
    out
}

/// The derived states of the suggestions that revert suggestion `id` (§4.3.4),
/// leaving out deleted ones.
pub fn reverts_of(events: &[Event], id: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|e| e.kind == "create" && e.field("reverts").and_then(Value::as_str) == Some(id))
        .filter_map(|e| derive_annotation(events, &e.id))
        .filter(|s| s["kind"] == "suggestion" && s["deleted"] == json!(false))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, after: &[&str]) -> Event {
        Event {
            id: id.into(),
            annotation: Some("a".into()),
            document: None,
            after: after.iter().map(|s| s.to_string()).collect(),
            kind: "edit".into(),
            author: json!({ "id": "mailto:ada@example.org" }),
            time: "2026-09-28T14:01:00Z".into(),
            fields: Map::new(),
        }
    }

    #[test]
    fn builds_the_live_graph() {
        let events = [
            ev("a", &[]),
            ev("c", &["a", "a"]),
            ev("b", &["a"]),
            ev("d", &["b", "c"]),
            // A cycle, an event after a missing one, and a second root.
            ev("x", &["y"]),
            ev("y", &["x"]),
            ev("z", &["missing"]),
            ev("r", &[]),
        ];
        let log = Log::build(&events, "a").unwrap();
        let order: Vec<&str> = log.ordered().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(order, ["a", "b", "c", "d"], "duplicate `after` entries count once");
        assert_eq!(log.heads(), ["d"]);
        assert_eq!(log.ancestors_of(["d"]), HashSet::from(["a", "b", "c"]));
        assert!(log.ancestors_of(["missing", "x"]).is_empty());
        assert_eq!(log.field_heads(&["b", "c", "a"]), ["b", "c"]);
        assert_eq!(log.field_heads(&["d", "b"]), ["d"]);
    }
}
