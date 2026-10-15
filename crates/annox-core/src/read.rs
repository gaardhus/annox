//! Read state (§5.13): which events each reader has seen, kept in
//! `.annox/local/read.json`. It is local to one copy of the workspace, and
//! never committed or synced.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::Event;
use crate::replay::{self, Log};
use crate::storage::{Index, Workspace};

/// Where read state lives, relative to the workspace root.
pub const READ_FILE: &str = ".annox/local/read.json";

/// The annotations each reader has seen: per reader id, per annotation id,
/// the heads they had seen (§5.13).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadState {
    readers: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    readers: BTreeMap<String, Reader>,
}

#[derive(Serialize, Deserialize, Default)]
struct Reader {
    #[serde(default)]
    seen: BTreeMap<String, BTreeSet<String>>,
}

impl ReadState {
    /// Parses `read.json`. Anything unreadable counts as empty.
    pub fn parse(text: &str) -> ReadState {
        let Ok(file) = serde_json::from_str::<File>(text) else { return ReadState::default() };
        ReadState { readers: file.readers.into_iter().map(|(id, r)| (id, r.seen)).collect() }
    }

    pub fn to_json(&self) -> String {
        let readers = self.readers.iter().map(|(id, seen)| (id.clone(), Reader { seen: seen.clone() })).collect();
        let mut json = serde_json::to_string_pretty(&File { version: 1, readers }).expect("serializable");
        json.push('\n');
        json
    }

    /// The read state of `ws`, empty if there is none.
    pub fn load(ws: &Workspace) -> ReadState {
        fs::read_to_string(ws.root.join(READ_FILE)).map(|t| ReadState::parse(&t)).unwrap_or_default()
    }

    /// Whether `reader` has read state, so that it has a baseline.
    pub fn knows(&self, reader: &str) -> bool {
        self.readers.contains_key(reader)
    }

    /// The heads of annotation `id` that `reader` had seen.
    pub fn seen(&self, reader: &str, id: &str) -> Option<&BTreeSet<String>> {
        self.readers.get(reader)?.get(id)
    }

    /// Adds `heads` to what `reader` has seen. Seen sets only grow, so two
    /// copies merge by union.
    pub fn add(&mut self, reader: &str, heads: BTreeMap<String, Vec<String>>) {
        let seen = self.readers.entry(reader.to_owned()).or_default();
        for (id, hs) in heads {
            seen.entry(id).or_default().extend(hs);
        }
    }

    fn merge(&mut self, other: ReadState) {
        for (reader, seen) in other.readers {
            let mine = self.readers.entry(reader).or_default();
            for (id, hs) in seen {
                mine.entry(id).or_default().extend(hs);
            }
        }
    }

    /// Whether annotation `id`, with events `own`, has an event `reader`
    /// hasn't seen (§5.13): one by someone else, other than a `reanchor`, that
    /// is neither a seen head, nor an ancestor of a seen head or of one of the
    /// reader's own events.
    pub fn is_unread(&self, reader: &str, own: &[Event], id: &str) -> bool {
        let Some(log) = Log::build(own.iter().filter(|e| e.annotation.as_deref() == Some(id)), id) else {
            return false;
        };
        let seen = self.seen(reader, id);
        let covering: Vec<&str> = log
            .events
            .values()
            .filter(|e| author_id(e) == Some(reader))
            .map(|e| e.id.as_str())
            .chain(seen.into_iter().flatten().map(String::as_str))
            .collect();
        log.events.values().any(|e| {
            e.kind != "reanchor"
                && e.kind != "react"
                && author_id(e) != Some(reader)
                && !covering.iter().any(|c| *c == e.id || log.is_ancestor(&e.id, c))
        })
    }

    /// The annotations among `events` that are unread for `reader`.
    pub fn unread(&self, reader: &str, events: &[Event]) -> BTreeSet<String> {
        by_annotation(events)
            .into_iter()
            .filter(|(id, own)| self.is_unread(reader, own, id))
            .map(|(id, _)| id.to_owned())
            .collect()
    }
}

fn author_id(e: &Event) -> Option<&str> {
    e.author.get("id").and_then(Value::as_str)
}

fn by_annotation(events: &[Event]) -> BTreeMap<&str, Vec<Event>> {
    let mut out: BTreeMap<&str, Vec<Event>> = BTreeMap::new();
    for e in events {
        if let Some(a) = e.annotation.as_deref() {
            out.entry(a).or_default().push(e.clone());
        }
    }
    out
}

/// The annotations of the thread that `id` belongs to: its root and every
/// reply (§2.2).
pub fn thread_ids(events: &[Event], id: &str) -> BTreeSet<String> {
    replay::thread_events(events, id).into_iter().filter_map(|e| e.annotation).collect()
}

/// The current heads of each annotation in `ids` (§2.5.1).
pub fn heads(events: &[Event], ids: &BTreeSet<String>) -> BTreeMap<String, Vec<String>> {
    let grouped = by_annotation(events);
    ids.iter()
        .filter_map(|id| {
            let own = grouped.get(id.as_str())?;
            Some((id.clone(), replay::annotation_heads(own, id)))
        })
        .collect()
}

/// Adds `heads` to what `reader` has seen, merging with what is on disk now
/// so that concurrent writers, such as an editor and an agent's CLI, don't
/// lose each other's marks. Returns the merged state.
pub fn mark(ws: &Workspace, reader: &str, heads: BTreeMap<String, Vec<String>>) -> io::Result<ReadState> {
    let mut state = ReadState::load(ws);
    let before = state.clone();
    state.add(reader, heads);
    if state != before {
        save(ws, &state)?;
    }
    Ok(state)
}

/// Marks everything in `index` read for `reader` if they have no read state
/// yet, so that a new reader starts with nothing unread (§5.13).
pub fn ensure_reader(ws: &Workspace, index: &Index, reader: &str) -> io::Result<ReadState> {
    let state = ReadState::load(ws);
    if state.knows(reader) {
        return Ok(state);
    }
    let events: Vec<Event> = index.events.values().map(|(e, _)| e.clone()).collect();
    let ids: BTreeSet<String> = by_annotation(&events).into_keys().map(str::to_owned).collect();
    let mut state = state;
    state.readers.entry(reader.to_owned()).or_default();
    state.add(reader, heads(&events, &ids));
    let mut merged = ReadState::load(ws);
    merged.merge(state);
    save(ws, &merged)?;
    Ok(merged)
}

/// Marks the thread of annotation `id` read for `reader`, as of `index`:
/// writing to a thread implies having read it (§5.13).
pub fn mark_thread(ws: &Workspace, index: &Index, id: &str, reader: &str) -> io::Result<()> {
    ensure_reader(ws, index, reader)?;
    let events: Vec<Event> = index.events.values().map(|(e, _)| e.clone()).collect();
    mark(ws, reader, heads(&events, &thread_ids(&events, id)))?;
    Ok(())
}

/// Drops `reader`'s read state, so that if they turn unread tracking back on
/// they start with a new baseline (§5.13).
pub fn forget(ws: &Workspace, reader: &str) -> io::Result<()> {
    let mut state = ReadState::load(ws);
    if state.readers.remove(reader).is_some() {
        save(ws, &state)?;
    }
    Ok(())
}

/// Writes `state` atomically (§5.4).
fn save(ws: &Workspace, state: &ReadState) -> io::Result<()> {
    let path = ws.root.join(READ_FILE);
    let dir = path.parent().expect("READ_FILE has a parent");
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".read.json.{}.tmp", crate::new_id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(state.to_json().as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    const ADA: &str = "mailto:ada@example.org";
    const BOB: &str = "mailto:bob@example.org";

    fn ev(id: &str, after: &[&str], kind: &str, author: &str) -> Event {
        Event {
            id: id.into(),
            annotation: Some("a".into()),
            document: None,
            after: after.iter().map(|s| s.to_string()).collect(),
            kind: kind.into(),
            author: json!({ "id": author }),
            time: "2026-10-11T00:00:00Z".into(),
            fields: Map::new(),
        }
    }

    fn seen(heads: &[&str]) -> ReadState {
        let mut s = ReadState::default();
        s.add(ADA, BTreeMap::from([("a".to_owned(), heads.iter().map(|h| h.to_string()).collect())]));
        s
    }

    #[test]
    fn others_events_are_unread_until_seen() {
        let events = vec![ev("a", &[], "create", BOB)];
        assert!(ReadState::default().is_unread(ADA, &events, "a"));
        assert!(!seen(&["a"]).is_unread(ADA, &events, "a"));
        assert!(!ReadState::default().is_unread(BOB, &events, "a"));
    }

    #[test]
    fn concurrent_events_from_an_old_branch_are_unread() {
        // "b" is older by id than "c", but Ada only saw the branch with "c".
        let events = vec![ev("a", &[], "create", BOB), ev("b", &["a"], "edit", BOB), ev("c", &["a"], "status", BOB)];
        assert!(seen(&["c"]).is_unread(ADA, &events, "a"));
        assert!(!seen(&["b", "c"]).is_unread(ADA, &events, "a"));
    }

    #[test]
    fn own_events_cover_what_they_follow() {
        let events = vec![ev("a", &[], "create", BOB), ev("b", &["a"], "edit", ADA)];
        assert!(!ReadState::default().is_unread(ADA, &events, "a"));
        let events = vec![ev("a", &[], "create", BOB), ev("b", &["a"], "edit", ADA), ev("c", &["b"], "edit", BOB)];
        assert!(ReadState::default().is_unread(ADA, &events, "a"));
    }

    #[test]
    fn reanchors_are_not_unread() {
        let events = vec![ev("a", &[], "create", BOB), ev("b", &["a"], "reanchor", BOB)];
        assert!(!seen(&["a"]).is_unread(ADA, &events, "a"));
    }

    #[test]
    fn reactions_are_not_unread() {
        let events = vec![ev("a", &[], "create", BOB), ev("b", &["a"], "react", BOB)];
        assert!(!seen(&["a"]).is_unread(ADA, &events, "a"));
    }

    #[test]
    fn round_trips_and_merges() {
        let s = seen(&["a"]);
        assert_eq!(ReadState::parse(&s.to_json()), s);
        assert_eq!(ReadState::parse("not json"), ReadState::default());
        let mut t = ReadState::default();
        t.add(BOB, BTreeMap::new());
        t.merge(s.clone());
        assert!(t.knows(ADA) && t.knows(BOB));
    }

    #[test]
    fn new_readers_start_with_nothing_unread() {
        let root = std::env::temp_dir().join(format!("annox-read-{}", crate::new_id()));
        let ws = Workspace::init(&root).unwrap();
        let text = crate::text::Text::from_raw("Hello world.\n");
        let new = crate::ops::NewAnnotation {
            kind: "comment",
            start: 0,
            end: 5,
            body: Some("Hi"),
            label: None,
            replacement: None,
            local: false,
            reverts: None,
        };
        crate::ops::create_annotation(&ws, &Index::read(&ws), "a.md", &text, &new, &json!({ "id": BOB })).unwrap();
        let index = Index::read(&ws);
        let events: Vec<Event> = index.events.values().map(|(e, _)| e.clone()).collect();
        assert_eq!(ReadState::default().unread(ADA, &events).len(), 1);
        let state = ensure_reader(&ws, &index, ADA).unwrap();
        assert!(state.unread(ADA, &events).is_empty());
        assert_eq!(ReadState::load(&ws), state);
        forget(&ws, ADA).unwrap();
        assert!(!ReadState::load(&ws).knows(ADA));
        let _ = fs::remove_dir_all(&root);
    }
}
