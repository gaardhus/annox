//! Following changes as they're written: `annox watch`, and `annox log --follow`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use annox_core::storage::{Index, Workspace};
use serde_json::{json, Value};

use super::{author, list, no_workspace, open_file, reader, Author, Filter, Watch};

/// How often `watch` checks `.annox/` for changes.
pub(super) const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Follows the threads that are unread for a reader (§5.13) and, with
/// `mention`, addressed to them, and reports each one once it has settled.
/// A thread is reported again when someone else changes it after that, and
/// marking it read forgets it.
pub struct Watcher {
    options: Watch,
    cwd: PathBuf,
    root: PathBuf,
    reader: String,
    /// Per thread: what it looked like when first seen that way, and when.
    pending: BTreeMap<String, (String, Instant)>,
    /// Per thread: what it looked like when it was reported.
    reported: BTreeMap<String, String>,
    started: bool,
}

impl Watcher {
    pub fn new(options: Watch, cwd: &Path) -> anyhow::Result<Watcher> {
        let cwd = cwd.canonicalize()?;
        let ws = Workspace::find(&cwd).ok_or_else(|| no_workspace(&cwd))?;
        let reader = reader(&author(&Author { author: options.reader.clone(), name: None }, &ws)?)?;
        Ok(Watcher {
            options,
            cwd,
            root: ws.root,
            reader,
            pending: BTreeMap::new(),
            reported: BTreeMap::new(),
            started: false,
        })
    }

    pub fn reader(&self) -> &str {
        &self.reader
    }

    /// The threads to report as of `now`, as `annox list` prints them. On the
    /// first poll, threads that are already unread are reported without
    /// waiting for them to settle.
    pub fn poll(&mut self, now: Instant) -> anyhow::Result<Vec<Value>> {
        let filter = Filter { unread: true, ..Filter::default() };
        let views = list(self.options.file.as_deref(), false, false, Some(self.reader.clone()), filter, &self.cwd)?;
        let settle = Duration::from_millis(self.options.settle);
        let mut current = BTreeSet::new();
        let mut out = Vec::new();
        let addressed: Vec<Value> =
            views.as_array().into_iter().flatten().filter(|v| self.is_addressed(v)).cloned().collect();
        for view in addressed {
            let id = view["id"].as_str().unwrap_or_default().to_owned();
            current.insert(id.clone());
            let shape = thread_shape(&view);
            if self.reported.get(&id) == Some(&shape) {
                continue;
            }
            let since = match self.pending.get(&id) {
                Some((s, since)) if *s == shape => *since,
                _ => {
                    self.pending.insert(id.clone(), (shape.clone(), now));
                    now
                }
            };
            if !self.started || now.duration_since(since) >= settle {
                self.pending.remove(&id);
                self.reported.insert(id, shape);
                out.push(view);
            }
        }
        self.pending.retain(|id, _| current.contains(id));
        self.reported.retain(|id, _| current.contains(id));
        self.started = true;
        Ok(out)
    }

    /// Whether a thread mentions any of `mention`, or the reader wrote in it.
    fn is_addressed(&self, view: &Value) -> bool {
        if self.options.mention.is_empty() {
            return true;
        }
        let replies = view["replies"].as_array().map(Vec::as_slice).unwrap_or_default();
        let entries = || std::iter::once(view).chain(replies);
        let mentions: Vec<String> = self.options.mention.iter().map(|m| m.to_lowercase()).collect();
        entries().any(|e| e["author"]["id"] == json!(self.reader))
            || entries().filter_map(|e| e["body"].as_str()).any(|body| {
                let body = body.to_lowercase();
                mentions.iter().any(|m| body.contains(m.as_str()))
            })
    }
}

/// What makes a reported thread worth reporting again: its unread entries,
/// status and bodies, but not where it is in the document.
fn thread_shape(view: &Value) -> String {
    let replies = view["replies"].as_array().map(Vec::as_slice).unwrap_or_default();
    let entries: Vec<Value> =
        std::iter::once(view).chain(replies).map(|e| json!([e["id"], e["unread"], e["body"]])).collect();
    json!([view["status"], view["deleted"], entries]).to_string()
}

/// Runs `watcher` until `stop` is set, passing each thread it reports to
/// `emit`. Errors while polling, such as a half-written file, are printed
/// and retried; an error from `emit` ends the watch.
pub fn watch(
    mut watcher: Watcher,
    stop: &AtomicBool,
    mut emit: impl FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut last = None;
    while !stop.load(Ordering::Relaxed) {
        let print = crate::fingerprint(&watcher.root);
        if last.as_ref() != Some(&print) || !watcher.pending.is_empty() {
            last = Some(print);
            match watcher.poll(Instant::now()) {
                Ok(views) => views.into_iter().try_for_each(&mut emit)?,
                Err(e) => eprintln!("annox watch: {e:#}"),
            }
        }
        std::thread::sleep(WATCH_INTERVAL);
    }
    Ok(())
}

/// The workspace a log covers, and the one document it's limited to, if any.
pub(super) fn log_scope(file: Option<&str>, cwd: &Path) -> anyhow::Result<(Workspace, Option<String>)> {
    match file {
        Some(file) => open_file(cwd, file).map(|(ws, rel, _)| (ws, Some(rel))),
        None => Ok((Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?, None)),
    }
}

/// The annotation events in `index`, oldest first, except those in `skip`,
/// and only those on the document at `only` if given. Each has `path`, its
/// document's current path, and `thread`, the id of its root annotation.
pub(super) fn log_events(index: &Index, only: Option<&str>, skip: &BTreeSet<String>) -> Vec<Value> {
    let path = |loc: &annox_core::storage::Location| {
        index.documents.get(index.canonical(&loc.document)).and_then(|d| d.path.clone())
    };
    // Events are UUIDv7s, so their ids sort by time.
    index
        .events
        .iter()
        .filter(|(id, _)| !skip.contains(*id))
        .filter_map(|(_, (event, loc))| {
            let path = path(loc);
            if only.is_some() && path.as_deref() != only {
                return None;
            }
            let annotation = event.annotation.as_deref().unwrap_or_default();
            let parent = index.events.get(annotation).and_then(|(e, _)| e.field("parent")?.as_str());
            let mut out = serde_json::to_value(event).ok()?;
            out["path"] = json!(path);
            out["thread"] = json!(parent.unwrap_or(annotation));
            Some(out)
        })
        .collect()
}

/// Runs until `stop` is set, passing each annotation event written after it
/// started to `emit`, as `annox log --json` prints them, oldest first. Errors
/// while reading are printed and retried; an error from `emit` ends it.
pub fn follow_log(
    file: Option<&str>,
    cwd: &Path,
    stop: &AtomicBool,
    mut emit: impl FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let (ws, only) = log_scope(file, &cwd.canonicalize()?)?;
    let mut seen: BTreeSet<String> = Index::read(&ws).events.into_keys().collect();
    let mut last = crate::fingerprint(&ws.root);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(WATCH_INTERVAL);
        let print = crate::fingerprint(&ws.root);
        if print == last {
            continue;
        }
        last = print;
        let index = Index::read(&ws);
        let new = log_events(&index, only.as_deref(), &seen);
        seen.extend(index.events.into_keys());
        new.into_iter().try_for_each(&mut emit)?;
    }
    Ok(())
}
