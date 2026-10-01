//! Reading and writing `.annox/` (§5).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::Event;
use crate::replay::{self, DocumentState};

/// A source of workspace files, by path relative to the workspace root with
/// `/` separators. Implemented for real directories and in-memory maps.
pub trait FileSource {
    /// Every file under `.annox/docs/`, `.annox/local/docs/`, and
    /// `.annox/synced/docs/`.
    fn doc_files(&self) -> Vec<String>;
    fn read(&self, path: &str) -> Option<String>;
}

impl FileSource for BTreeMap<String, String> {
    fn doc_files(&self) -> Vec<String> {
        self.keys()
            .filter(|p| TREES.iter().any(|t| p.strip_prefix(".annox/").is_some_and(|r| r.starts_with(t))))
            .cloned()
            .collect()
    }

    fn read(&self, path: &str) -> Option<String> {
        self.get(path).cloned()
    }
}

/// The folder trees under `.annox/`, in reading order: when an event is in
/// several, the first copy is used (§5.4).
const TREES: [&str; 3] = ["docs/", "local/docs/", "synced/docs/"];

/// Lines `.annox/.gitignore` must contain (§5.9).
const IGNORED: [&str; 3] = ["cache/", "local/", "synced/"];

/// A workspace on disk (§5.1).
#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    /// Finds the workspace containing `file`: the nearest ancestor directory
    /// with `.annox/annox.json` (§5.1).
    pub fn find(file: &Path) -> Option<Workspace> {
        let start = if file.is_dir() { file } else { file.parent()? };
        start
            .ancestors()
            .find(|dir| dir.join(".annox").join("annox.json").is_file())
            .map(|dir| Workspace { root: dir.to_path_buf() })
    }

    /// Initializes a workspace at `root` (§5.3, §5.9).
    pub fn init(root: &Path) -> io::Result<Workspace> {
        let annox = root.join(".annox");
        fs::create_dir_all(&annox)?;
        let marker = annox.join("annox.json");
        if !marker.exists() {
            fs::write(&marker, "{ \"format\": 1 }\n")?;
        }
        let ws = Workspace { root: root.to_path_buf() };
        ws.ensure_ignored()?;
        Ok(ws)
    }

    /// Adds any line of §5.9 missing from `.annox/.gitignore`, so that
    /// workspaces created before a line was required pick it up.
    pub fn ensure_ignored(&self) -> io::Result<()> {
        self.add_ignored(&IGNORED)
    }

    /// Makes git ignore all of `.annox/`, for a workspace that isn't shared
    /// through version control (§5.10).
    pub fn ignore_all(&self) -> io::Result<()> {
        self.add_ignored(&["*"])
    }

    /// Adds any of `lines` missing from `.annox/.gitignore`.
    fn add_ignored(&self, lines: &[&str]) -> io::Result<()> {
        let ignore = self.root.join(".annox").join(".gitignore");
        let existing = fs::read_to_string(&ignore).unwrap_or_default();
        let missing: Vec<&str> = lines.iter().copied().filter(|l| !existing.lines().any(|e| e.trim() == *l)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        let mut text = existing;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for line in missing {
            text.push_str(line);
            text.push('\n');
        }
        fs::write(ignore, text)
    }

    /// The document path (§3.1) of `file`, relative to the workspace root.
    pub fn relative(&self, file: &Path) -> Option<String> {
        let rel = file.strip_prefix(&self.root).ok()?;
        let parts: Vec<&str> = rel.components().map(|c| c.as_os_str().to_str()).collect::<Option<_>>()?;
        Some(parts.join("/"))
    }

    /// Writes `event` into `folder` (relative to `.annox/`, e.g.
    /// `docs/paper.tex~<id>` or `docs/paper.tex~<id>/document`), atomically
    /// (§5.4).
    pub fn write_event(&self, folder: &str, event: &Event) -> io::Result<()> {
        let dir = self.root.join(".annox").join(folder);
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".{}.json.tmp", event.id));
        let mut json = serde_json::to_string_pretty(event).map_err(io::Error::other)?;
        json.push('\n');
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, dir.join(format!("{}.json", event.id)))
    }
}

impl FileSource for Workspace {
    fn doc_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        for tree in TREES {
            walk(&self.root, &self.root.join(".annox").join(tree), &mut out);
        }
        out
    }

    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.root.join(path)).ok()
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Some(rel) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) {
            out.push(rel.replace(std::path::MAIN_SEPARATOR, "/"));
        }
    }
}

/// Whether `s` is a canonical lowercase UUID.
fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

/// The document id of a document folder name `<path>~<id>` (§5.5.2).
fn folder_doc_id(name: &str) -> Option<&str> {
    let (prefix, id) = name.rsplit_once('~')?;
    (!prefix.is_empty() && is_uuid(id)).then_some(id)
}

/// Which area a folder is in (§5.11). The sync mirror (§5.12) is part of
/// the shared area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Shared,
    Local,
}

/// Where an event file lives: its document folder (relative to `.annox/`),
/// document id, area, whether it's in the sync mirror (§5.12), and whether
/// it's a document event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub folder: String,
    pub document: String,
    pub area: Area,
    pub synced: bool,
    pub is_document_event: bool,
}

fn classify(path: &str) -> Option<Location> {
    let path = path.strip_prefix(".annox/")?;
    let tree = TREES.into_iter().find(|t| path.starts_with(t))?;
    let rest = &path[tree.len()..];
    let area = if tree == "local/docs/" { Area::Local } else { Area::Shared };
    let (dir, file) = rest.rsplit_once('/')?;
    file.strip_suffix(".json").filter(|s| is_uuid(s))?;
    let (folder, is_document_event) = match dir.rsplit_once('/') {
        Some((parent, "document")) => (parent, true),
        None if dir == "document" => return None,
        _ => (dir, false),
    };
    let name = folder.rsplit('/').next()?;
    let document = folder_doc_id(name)?.to_owned();
    let synced = tree == "synced/docs/";
    Some(Location { folder: format!("{tree}{folder}"), document, area, synced, is_document_event })
}

/// The tree (§5.2) a folder relative to `.annox/` is in.
pub fn folder_tree(folder: &str) -> &'static str {
    TREES.into_iter().find(|t| folder.starts_with(t)).unwrap_or("docs/")
}

fn parse_event(source: &impl FileSource, path: &str) -> Option<Event> {
    let text = source.read(path)?;
    let event: Event = serde_json::from_str(&text).ok()?;
    let stem = path.rsplit('/').next()?.strip_suffix(".json")?;
    (event.id == stem).then_some(event)
}

/// Everything read from a workspace (§5.7.1): document records, and every
/// annotation event with its location.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub documents: BTreeMap<String, DocumentState>,
    /// Folders of each document id.
    pub folders: BTreeMap<String, BTreeSet<String>>,
    /// Annotation events by id, with where they were read from.
    pub events: BTreeMap<String, (Event, Location)>,
    /// Document events of each document id, with their file paths.
    pub document_events: BTreeMap<String, Vec<(Event, String)>>,
    /// The area of each document record: where its `document/` events are.
    pub areas: BTreeMap<String, Area>,
    /// Ids of events with a copy in the sync mirror (§5.12).
    pub synced: BTreeSet<String>,
}

impl Index {
    /// Reads every document folder in both areas, and the sync mirror.
    pub fn read(source: &impl FileSource) -> Index {
        let mut index = Index::default();
        let mut paths = source.doc_files();
        // Committed copies before mirror copies (§5.4, §5.12).
        paths.sort_by_key(|p| TREES.iter().position(|t| p.strip_prefix(".annox/").is_some_and(|r| r.starts_with(t))));
        for path in paths {
            let Some(loc) = classify(&path) else { continue };
            index.folders.entry(loc.document.clone()).or_default().insert(loc.folder.clone());
            let Some(event) = parse_event(source, &path) else { continue };
            if loc.synced {
                index.synced.insert(event.id.clone());
            }
            if loc.is_document_event {
                if event.document.as_deref() == Some(loc.document.as_str()) {
                    index.areas.entry(loc.document.clone()).or_insert(loc.area);
                    index.document_events.entry(loc.document.clone()).or_default().push((event, path));
                }
            } else {
                index.events.entry(event.id.clone()).or_insert((event, loc));
            }
        }
        // Merge validity depends on areas (§5.5.1).
        let areas = index.areas.clone();
        let may_merge = |doc: &str, into: &str| {
            into < doc || (areas.get(doc) == Some(&Area::Local) && areas.get(into) == Some(&Area::Shared))
        };
        for id in index.folders.keys() {
            let events: Vec<Event> =
                index.document_events.get(id).into_iter().flatten().map(|(e, _)| e.clone()).collect();
            if let Some(state) = replay::derive_document(&events, id, &may_merge) {
                index.documents.insert(id.clone(), state);
            }
        }
        // Readers ignore merges into documents they don't know (§5.5.1).
        let known: BTreeSet<String> = index.documents.keys().cloned().collect();
        for state in index.documents.values_mut() {
            if state.merged_into.as_ref().is_some_and(|m| !known.contains(m)) {
                state.merged_into = None;
            }
        }
        index
    }

    /// The canonical document of `id` (§5.5.1).
    pub fn canonical<'a>(&'a self, mut id: &'a str) -> &'a str {
        while let Some(next) = self.documents.get(id).and_then(|d| d.merged_into.as_deref()) {
            id = next;
        }
        id
    }

    /// Documents at `path`: not merged, with that current path (§5.5.1).
    pub fn documents_at(&self, path: &str) -> Vec<String> {
        self.documents
            .iter()
            .filter(|(_, d)| d.merged_into.is_none() && d.path.as_deref() == Some(path))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Loads the annotations of the file at `path` (§5.7.1).
    pub fn load(&self, path: &str) -> Loaded {
        let at = self.documents_at(path);
        let loaded: Vec<String> =
            self.documents.keys().filter(|d| at.iter().any(|a| a == self.canonical(d))).cloned().collect();
        let events: Vec<Event> =
            self.events.values().filter(|(_, loc)| loaded.contains(&loc.document)).map(|(e, _)| e.clone()).collect();
        let annotations = events
            .iter()
            .filter(|e| e.kind == "create" && e.annotation.as_deref() == Some(e.id.as_str()))
            .map(|e| e.id.clone())
            .collect();
        let folders = loaded.iter().flat_map(|d| self.folders[d].iter().cloned()).collect();
        Loaded { at, loaded, folders, events, annotations }
    }
}

/// The result of loading one document (§5.7.1).
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// Documents at the path (*R*).
    pub at: Vec<String>,
    /// Documents whose annotations are loaded (*S*).
    pub loaded: Vec<String>,
    /// Folders read, relative to `.annox/`, sorted.
    pub folders: BTreeSet<String>,
    /// Annotation events read.
    pub events: Vec<Event>,
    /// Ids of annotations with a `create` event, sorted.
    pub annotations: BTreeSet<String>,
}

impl Loaded {
    /// Derived state of every loaded annotation (§2.5.6), by id.
    pub fn derive(&self) -> BTreeMap<String, Value> {
        // Group once, so each annotation only replays its own events.
        let mut by_annotation: BTreeMap<&str, Vec<Event>> = BTreeMap::new();
        for e in &self.events {
            if let Some(a) = e.annotation.as_deref() {
                by_annotation.entry(a).or_default().push(e.clone());
            }
        }
        self.annotations
            .iter()
            .filter_map(|id| {
                let events = by_annotation.get(id.as_str())?;
                replay::derive_annotation(events, id).map(|v| (id.clone(), v))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_folders() {
        let id = "01926d3a-0100-7000-8000-000000000000";
        let ev = "01926d3a-0001-7000-8000-000000000000";
        let loc = classify(&format!(".annox/docs/ch2/intro.md~{id}/{ev}.json")).unwrap();
        assert_eq!(loc.folder, format!("docs/ch2/intro.md~{id}"));
        assert!(!loc.is_document_event);
        let loc = classify(&format!(".annox/local/docs/a.md~{id}/document/{ev}.json")).unwrap();
        assert_eq!((loc.area, loc.is_document_event), (Area::Local, true));
        let loc = classify(&format!(".annox/synced/docs/a.md~{id}/{ev}.json")).unwrap();
        assert_eq!(
            (loc.folder.as_str(), loc.area, loc.synced),
            (format!("synced/docs/a.md~{id}").as_str(), Area::Shared, true)
        );
        assert!(classify(&format!(".annox/docs/paper.tex/{ev}.json")).is_none());
        assert!(classify(&format!(".annox/docs/paper.tex~{id}/notes.json")).is_none());
    }
}
