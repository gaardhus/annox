//! Operations that write events: creating annotations and appending events
//! to existing ones (§2.4, §5.7.2).

use std::io;

use serde_json::{json, Map, Value};

use crate::anchor;
use crate::event::Event;
use crate::replay;
use crate::storage::{Area, Index, Workspace};
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
            if folders.iter().any(|f| f.starts_with("docs/")) { Area::Shared } else { Area::Local }
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
    let (_, create_loc) = index
        .events
        .get(annotation)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown annotation"))?;
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
    let folder = match index.documents.get(doc).and_then(|d| d.path.as_deref()) {
        Some(path) => format!("{}{path}~{doc}", area_prefix(create_loc.area)),
        None => create_loc.folder.clone(),
    };
    ws.write_event(&folder, &event)?;
    Ok(event)
}
