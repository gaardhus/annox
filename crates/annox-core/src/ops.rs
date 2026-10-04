//! Operations that write events: creating annotations and appending events
//! to existing ones (§2.4, §5.7.2).

use std::io;

use serde_json::{json, Map, Value};

use crate::anchor;
use crate::event::Event;
use crate::replay;
use crate::storage::{folder_tree, Area, Index, Workspace};
use crate::text::Text;

fn area_prefix(area: Area) -> &'static str {
    match area {
        Area::Shared => "docs/",
        Area::Local => "local/docs/",
    }
}

/// What to create (§2.4 `create`).
pub struct NewAnnotation<'a> {
    pub kind: &'a str,
    pub start: usize,
    pub end: usize,
    pub body: Option<&'a str>,
    pub label: Option<&'a str>,
    /// Required for suggestions.
    pub replacement: Option<&'a str>,
    pub local: bool,
    /// For a suggestion: the accepted suggestion it undoes (§4.3.4).
    pub reverts: Option<&'a str>,
}

/// Creates a comment or suggestion on the document at `path` with content
/// `text`, creating a document record first if needed (§5.7.2, §5.11).
/// Returns the `create` event.
pub fn create_annotation(
    ws: &Workspace,
    index: &Index,
    path: &str,
    text: &Text,
    new: &NewAnnotation,
    author: &Value,
) -> io::Result<Event> {
    let at = index.documents_at(path);
    let area_of = |id: &str| {
        index.folders.get(id).map_or(Area::Shared, |folders| {
            if folders.iter().any(|f| !f.starts_with("local/")) {
                Area::Shared
            } else {
                Area::Local
            }
        })
    };
    let shared = at.iter().find(|d| area_of(d) == Area::Shared);
    let local = at.iter().find(|d| area_of(d) == Area::Local);
    let area = if new.local { Area::Local } else { Area::Shared };
    let doc = match (shared, local, area) {
        (Some(s), _, _) => s.clone(),
        (None, Some(l), Area::Local) => l.clone(),
        _ => {
            let id = crate::new_id();
            let record = Event {
                id: id.clone(),
                annotation: None,
                document: Some(id.clone()),
                after: vec![],
                kind: "document".into(),
                author: author.clone(),
                time: Event::now(),
                fields: Map::from_iter([("path".into(), json!(path))]),
            };
            ws.write_event(&format!("{}{path}~{id}/document", area_prefix(area)), &record)?;
            id
        }
    };

    let id = crate::new_id();
    let mut fields = Map::new();
    fields.insert("kind".into(), json!(new.kind));
    fields.insert("target".into(), json!(anchor::create(text, new.start, new.end, path)));
    if let Some(body) = new.body {
        fields.insert("body".into(), json!(body));
    }
    if let Some(label) = new.label {
        fields.insert("label".into(), json!(label));
    }
    if new.kind == "suggestion" {
        fields.insert("edit".into(), json!({ "replacement": new.replacement.unwrap_or_default() }));
        if let Some(reverts) = new.reverts {
            fields.insert("reverts".into(), json!(reverts));
        }
    }
    let event = Event {
        id: id.clone(),
        annotation: Some(id),
        document: None,
        after: vec![],
        kind: "create".into(),
        author: author.clone(),
        time: Event::now(),
        fields,
    };
    ws.write_event(&format!("{}{path}~{doc}", area_prefix(area)), &event)?;
    Ok(event)
}

/// The folder of document `doc` in `area`, named after its current path, or
/// `fallback` if the document has no known path. Never a folder of the sync
/// mirror, which holds only events received from the hub (§5.12).
fn folder_for(index: &Index, doc: &str, area: Area, fallback: &str) -> String {
    match index.documents.get(doc).and_then(|d| d.path.as_deref()) {
        Some(path) => format!("{}{path}~{doc}", area_prefix(area)),
        None => fallback.strip_prefix("synced/").unwrap_or(fallback).to_owned(),
    }
}

fn not_found(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("unknown {what}"))
}

/// Creates a reply to root `parent` (§2.4). The reply is local if asked, or
/// if its parent is local (§5.11).
pub fn create_reply(
    ws: &Workspace,
    index: &Index,
    parent: &str,
    body: &str,
    local: bool,
    author: &Value,
) -> io::Result<Event> {
    let (_, parent_loc) = index.events.get(parent).ok_or_else(|| not_found("annotation"))?;
    let area = if local || parent_loc.area == Area::Local { Area::Local } else { Area::Shared };
    let doc = index.canonical(&parent_loc.document);
    let id = crate::new_id();
    let event = Event {
        id: id.clone(),
        annotation: Some(id),
        document: None,
        after: vec![],
        kind: "create".into(),
        author: author.clone(),
        time: Event::now(),
        fields: Map::from_iter([
            ("kind".into(), json!("reply")),
            ("parent".into(), json!(parent)),
            ("body".into(), json!(body)),
        ]),
    };
    ws.write_event(&folder_for(index, doc, area, &parent_loc.folder), &event)?;
    Ok(event)
}

fn move_file(ws: &Workspace, from: &str, to_folder: &str) -> io::Result<()> {
    let annox = ws.root.join(".annox");
    let file = std::path::Path::new(from).file_name().ok_or_else(|| not_found("file"))?;
    let dir = annox.join(to_folder);
    std::fs::create_dir_all(&dir)?;
    std::fs::rename(ws.root.join(from), dir.join(file))
}

/// Publishes local annotations and their local replies (§5.11). Returns the
/// ids that were moved to the shared area.
pub fn publish(ws: &Workspace, index: &Index, ids: &[String], author: &Value) -> io::Result<Vec<String>> {
    let is_local = |id: &str| index.events.get(id).is_some_and(|(_, loc)| loc.area == Area::Local);
    let mut set: Vec<String> = ids.iter().filter(|id| is_local(id)).cloned().collect();
    for (e, loc) in index.events.values() {
        let parent = e.field("parent").and_then(Value::as_str);
        if e.kind == "create" && loc.area == Area::Local && parent.is_some_and(|p| set.iter().any(|s| s == p)) {
            set.push(e.id.clone());
        }
    }
    // Where each local document's annotations go when published.
    let mut targets: std::collections::BTreeMap<String, String> = Default::default();
    for id in &set {
        let doc = index.canonical(&index.events[id].1.document).to_owned();
        if targets.contains_key(&doc) {
            continue;
        }
        let path = index.documents.get(&doc).and_then(|d| d.path.clone()).ok_or_else(|| not_found("document"))?;
        let target = if index.areas.get(&doc) == Some(&Area::Shared) {
            doc.clone()
        } else if let Some(shared) =
            index.documents_at(&path).into_iter().find(|d| index.areas.get(d) == Some(&Area::Shared))
        {
            // A shared record appeared meanwhile: merge the local one into it (§5.8).
            let events: Vec<Event> = index.document_events[&doc].iter().map(|(e, _)| e.clone()).collect();
            let merged = Event {
                id: crate::new_id(),
                annotation: None,
                document: Some(doc.clone()),
                after: replay::document_heads(&events, &doc),
                kind: "merged".into(),
                author: author.clone(),
                time: Event::now(),
                fields: Map::from_iter([("into".into(), json!(shared))]),
            };
            ws.write_event(&format!("local/docs/{path}~{doc}/document"), &merged)?;
            shared
        } else {
            // Publish the local record itself.
            for (_, file) in &index.document_events[&doc] {
                move_file(ws, file, &format!("docs/{path}~{doc}/document"))?;
            }
            doc.clone()
        };
        targets.insert(doc, target);
    }
    for id in &set {
        let doc = index.canonical(&index.events[id].1.document);
        let target = &targets[doc];
        let folder = folder_for(index, target, Area::Shared, &format!("docs/{target}"));
        for (e, loc) in index.events.values() {
            if e.annotation.as_deref() == Some(id.as_str()) && loc.area == Area::Local {
                move_file(ws, &format!(".annox/{}/{}.json", loc.folder, e.id), &folder)?;
            }
        }
    }
    Ok(set)
}

/// Moves every document at `from` to `to` (§5.6): a `move` event per record,
/// then its folders are renamed to match. Returns the number of records moved.
pub fn move_document(ws: &Workspace, index: &Index, from: &str, to: &str, author: &Value) -> io::Result<usize> {
    let docs = index.documents_at(from);
    for doc in &docs {
        let area = index.areas.get(doc).copied().unwrap_or(Area::Shared);
        let events: Vec<Event> = index.document_events[doc].iter().map(|(e, _)| e.clone()).collect();
        let event = Event {
            id: crate::new_id(),
            annotation: None,
            document: Some(doc.clone()),
            after: replay::document_heads(&events, doc),
            kind: "move".into(),
            author: author.clone(),
            time: Event::now(),
            fields: Map::from_iter([("path".into(), json!(to))]),
        };
        ws.write_event(&format!("{}{to}~{doc}/document", area_prefix(area)), &event)?;
        // Tidy: move files from folders with other names (§5.6 step 2),
        // keeping each in its tree, so mirror copies stay out of git (§5.12).
        for folder in &index.folders[doc] {
            let new_folder = format!("{}{to}~{doc}", folder_tree(folder));
            if *folder == new_folder {
                continue;
            }
            for (e, _) in index.events.values().filter(|(_, l)| l.folder == *folder) {
                move_file(ws, &format!(".annox/{folder}/{}.json", e.id), &new_folder)?;
            }
            for (_, file) in
                index.document_events[doc].iter().filter(|(_, f)| f.starts_with(&format!(".annox/{folder}/")))
            {
                move_file(ws, file, &format!("{new_folder}/document"))?;
            }
            let old = ws.root.join(".annox").join(folder);
            let _ = std::fs::remove_dir(old.join("document"));
            let _ = std::fs::remove_dir(old);
        }
    }
    Ok(docs.len())
}

/// Appends an event of type `kind` with `fields` to annotation `annotation`.
/// `after` is set to the annotation's current heads, and the file goes into
/// the canonical document's folder in the annotation's area (§5.7.2).
pub fn append_event(
    ws: &Workspace,
    index: &Index,
    annotation: &str,
    kind: &str,
    fields: Map<String, Value>,
    author: &Value,
) -> io::Result<Event> {
    let (_, create_loc) =
        index.events.get(annotation).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown annotation"))?;
    let own: Vec<Event> = index
        .events
        .values()
        .filter(|(e, _)| e.annotation.as_deref() == Some(annotation))
        .map(|(e, _)| e.clone())
        .collect();
    let event = Event {
        id: crate::new_id(),
        annotation: Some(annotation.to_owned()),
        document: None,
        after: replay::annotation_heads(&own, annotation),
        kind: kind.into(),
        author: author.clone(),
        time: Event::now(),
        fields,
    };
    let doc = index.canonical(&create_loc.document);
    ws.write_event(&folder_for(index, doc, create_loc.area, &create_loc.folder), &event)?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace whose `notes.md` record and comment arrived only through
    /// sync, so they're only in the mirror (§5.12).
    fn mirrored() -> (Workspace, String, String) {
        let root = std::env::temp_dir().join(format!("annox-ops-{}", crate::new_id()));
        let ws = Workspace::init(&root).unwrap();
        let author = json!({ "id": "mailto:ada@example.org" });
        let index = Index::read(&ws);
        let text = Text::from_raw("Hello world.\n");
        let new = NewAnnotation {
            kind: "comment",
            start: 6,
            end: 11,
            body: Some("Hi"),
            label: None,
            replacement: None,
            local: false,
            reverts: None,
        };
        let comment = create_annotation(&ws, &index, "notes.md", &text, &new, &author).unwrap();
        let annox = ws.root.join(".annox");
        std::fs::create_dir_all(annox.join("synced")).unwrap();
        std::fs::rename(annox.join("docs"), annox.join("synced/docs")).unwrap();
        let doc = Index::read(&ws).documents_at("notes.md").remove(0);
        (ws, doc, comment.id)
    }

    fn files(ws: &Workspace, tree: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![ws.root.join(".annox").join(tree)];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                } else {
                    let path = entry.path();
                    let rel = path.strip_prefix(ws.root.join(".annox")).unwrap();
                    let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
                    out.push(parts.join("/"));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn replies_to_mirrored_annotations_go_to_docs() {
        let (ws, doc, comment) = mirrored();
        let bob = json!({ "id": "mailto:bob@example.org" });
        let reply = create_reply(&ws, &Index::read(&ws), &comment, "Yes", false, &bob).unwrap();
        let resolved = append_event(&ws, &Index::read(&ws), &comment, "status", Map::new(), &bob).unwrap();
        let mut expected =
            vec![format!("docs/notes.md~{doc}/{}.json", reply.id), format!("docs/notes.md~{doc}/{}.json", resolved.id)];
        expected.sort();
        assert_eq!(files(&ws, "docs"), expected);
        let _ = std::fs::remove_dir_all(&ws.root);
    }

    #[test]
    fn renames_keep_mirror_files_in_the_mirror() {
        let (ws, doc, comment) = mirrored();
        let bob = json!({ "id": "mailto:bob@example.org" });
        move_document(&ws, &Index::read(&ws), "notes.md", "renamed.md", &bob).unwrap();
        let synced = files(&ws, "synced/docs");
        assert!(synced.contains(&format!("synced/docs/renamed.md~{doc}/{comment}.json")));
        assert!(synced.contains(&format!("synced/docs/renamed.md~{doc}/document/{doc}.json")));
        assert!(synced.iter().all(|f| f.contains("renamed.md~")));
        // The move event is written by bob, so it goes to docs/.
        assert_eq!(files(&ws, "docs").len(), 1);
        assert_eq!(Index::read(&ws).documents_at("renamed.md"), vec![doc]);
        let _ = std::fs::remove_dir_all(&ws.root);
    }
}
