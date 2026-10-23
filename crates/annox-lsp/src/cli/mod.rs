//! Command-line access to a workspace's annotations, for people, scripts and
//! agents. Commands read and write `.annox/` directly through `annox-core`, and
//! return JSON, which `annox` prints as is with --json, or as text through
//! [`crate::render`]. Text is targeted by quoting it rather than by offsets.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use annox_core::anchor::{self, Anchor, State};
use annox_core::event::Event;
use annox_core::ops::{self, NewAnnotation};
use annox_core::read;
use annox_core::storage::{Area, Index, Workspace};
use annox_core::suggestion;
use annox_core::text::Text;
use anyhow::{anyhow, bail, Context};
use clap::Parser;
use serde_json::{json, Map, Value};

mod args;
mod commit;
mod config;
mod report;
mod watch;

use args::Cli;
pub use args::{Author, Command, Filter, Meta, MoveTo, Quote, Watch};
use commit::commit;
pub use commit::commit_workspace;
use config::config;
use report::report;
pub use watch::{follow_log, watch, Watcher};
use watch::{log_events, log_scope};

/// Parses and runs a command (the arguments after `annox`) with `cwd` as the
/// current directory, and returns its JSON output.
pub fn run(args: &[String], cwd: &Path) -> anyhow::Result<Value> {
    execute(parse(args)?, cwd)
}

/// Parses a command: the arguments after `annox`.
pub fn parse(args: &[String]) -> anyhow::Result<Command> {
    let cli = Cli::try_parse_from(std::iter::once("annox").chain(args.iter().map(String::as_str)))
        .map_err(|e| anyhow!("{}", e.render().to_string().trim_end()))?;
    Ok(cli.command)
}

/// Runs a parsed command with `cwd` as the current directory, and returns its
/// JSON output.
pub fn execute(command: Command, cwd: &Path) -> anyhow::Result<Value> {
    let cwd = cwd.canonicalize()?;
    match command {
        Command::Init { dir, local } => {
            let dir = dir.map_or(cwd.clone(), |d| cwd.join(d));
            let ws = Workspace::init(&dir)?;
            if local {
                ws.ignore_all()?;
            }
            Ok(json!({ "root": ws.root }))
        }
        Command::List { file, all, closed, reader, filter } => list(file.as_deref(), all, closed, reader, filter, &cwd),
        Command::Report { file, by, reader, filter, .. } => report(file.as_deref(), &by, reader, filter, &cwd),
        Command::Log { follow: true, .. } => bail!("`annox log --follow` streams; run it with `follow_log`"),
        Command::Log { file, .. } => {
            let (ws, only) = log_scope(file.as_deref(), &cwd)?;
            Ok(Value::Array(log_events(&read_index(&ws), only.as_deref(), &BTreeSet::new())))
        }
        Command::Show { id } => show(&id, &cwd),
        Command::History { id } => history(&id, &cwd),
        Command::Comment { file, at, body, meta, author } => {
            create(&file, &at, None, Some(&body), &meta, &author, &cwd)
        }
        Command::Highlight { file, at, meta, author } => create(&file, &at, None, None, &meta, &author, &cwd),
        Command::Suggest { file, at, replace, body, meta, author } => {
            create(&file, &at, Some(&replace), body.as_deref(), &meta, &author, &cwd)
        }
        Command::Reply { id, body, author: who } => {
            let (ws, index) = open_workspace(&cwd)?;
            let state = derived(&index, &id)?;
            if state["kind"] == "reply" {
                bail!("{id} is a reply; reply to its parent {}", state["parent"].as_str().unwrap_or_default());
            }
            let event = ops::create_reply(&ws, &index, &id, &body, false, &author(&who, &ws)?)?;
            Ok(json!({ "id": event.id }))
        }
        Command::Edit { id, body, label, author: who } => {
            let (ws, index) = open_workspace(&cwd)?;
            let state = derived(&index, &id)?;
            let mut fields = Map::new();
            if let Some(body) = body {
                fields.insert("body".into(), json!(body));
            }
            if let Some(label) = label {
                if state["kind"] == "reply" {
                    bail!("replies have no label");
                }
                fields.insert("label".into(), if label.is_empty() { Value::Null } else { json!(label) });
            }
            if fields.is_empty() {
                bail!("nothing to change: pass --body or --label");
            }
            ops::append_event(&ws, &index, &id, "edit", fields, &author(&who, &ws)?)?;
            Ok(json!({ "id": id }))
        }
        Command::Status { id, status, author: who } => {
            let (ws, index) = open_workspace(&cwd)?;
            let state = derived(&index, &id)?;
            let allowed: &[&str] = match state["kind"].as_str() {
                Some("comment") => &["open", "resolved"],
                Some("suggestion") => &["open", "rejected", "withdrawn"],
                _ => bail!("replies have no status"),
            };
            if status == "accepted" && state["kind"] == "suggestion" {
                bail!("use `annox accept {id}`, which also applies the suggestion");
            }
            if !allowed.contains(&status.as_str()) {
                bail!(
                    "a {} can't be {status}; use one of: {}",
                    state["kind"].as_str().unwrap_or_default(),
                    allowed.join(", ")
                );
            }
            if state["status"] == "accepted" {
                bail!("{id} is accepted, which is final; make a new suggestion to undo it");
            }
            let fields = Map::from_iter([("status".into(), json!(status))]);
            ops::append_event(&ws, &index, &id, "status", fields, &author(&who, &ws)?)?;
            Ok(json!({ "id": id, "status": status }))
        }
        Command::Accept { id, confirmed, author } => accept(&id, confirmed, &author, &cwd),
        Command::Revert { id, accept: then_accept, body, author } => {
            revert(&id, then_accept, body.as_deref(), &author, &cwd)
        }
        Command::Retarget { id, at, replace, author } => move_target(&id, &at, Some(&replace), &author, &cwd),
        Command::Reattach { id, at, author } => move_target(&id, &at, None, &author, &cwd),
        Command::Delete { id, author } => set_deleted(&id, true, &author, &cwd),
        Command::Restore { id, author } => set_deleted(&id, false, &author, &cwd),
        Command::React { id, reaction, remove, author } => react(&id, &reaction, remove, &author, &cwd),
        Command::Read { ids, all, author: who } => mark_read(&ids, all, &who, &cwd),
        Command::Commit { message, dry_run } => commit(message.as_deref(), dry_run, &cwd),
        Command::Config => config(&cwd),
    }
}

/// Marks the threads of `ids`, or every thread, read for the author (§5.13).
fn mark_read(ids: &[String], all: bool, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let reader = reader(&author(who, &ws)?)?;
    let events: Vec<Event> = index.events.values().map(|(e, _)| e.clone()).collect();
    let mut threads = BTreeSet::new();
    for id in ids {
        if !index.events.get(id).is_some_and(|(e, _)| e.kind == "create") {
            bail!("unknown annotation {id}");
        }
        threads.extend(read::thread_ids(&events, id));
    }
    if all {
        threads.extend(events.iter().filter_map(|e| e.annotation.clone()));
    }
    read::ensure_reader(&ws, &index, &reader)?;
    read::mark(&ws, &reader, read::heads(&events, &threads))?;
    Ok(json!({ "annotations": threads }))
}

/// The id of `author`, whose read state is used (§5.13).
fn reader(author: &Value) -> anyhow::Result<String> {
    author["id"].as_str().map(str::to_owned).ok_or_else(|| anyhow!("no identity: set ANNOX_AUTHOR or pass --author ID"))
}

fn set_deleted(id: &str, deleted: bool, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    derived(&index, id)?;
    let event = if deleted { "delete" } else { "restore" };
    ops::append_event(&ws, &index, id, event, Map::new(), &author(who, &ws)?)?;
    Ok(json!({ "id": id, "deleted": deleted }))
}

fn react(id: &str, reaction: &str, remove: bool, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    derived(&index, id)?;
    if reaction.is_empty() {
        bail!("the reaction is empty");
    }
    let changed = ops::react(&ws, &index, id, reaction, remove, &author(who, &ws)?)?.is_some();
    Ok(json!({ "id": id, "reaction": reaction, "removed": remove, "changed": changed }))
}

/// The author to write events as: flags, then environment, then the user
/// config file, then git (§2.7).
fn author(who: &Author, ws: &Workspace) -> anyhow::Result<Value> {
    let get = |flag: &Option<String>, var| flag.clone().or_else(|| std::env::var(var).ok().filter(|v| !v.is_empty()));
    crate::resolve_author(get(&who.author, "ANNOX_AUTHOR"), get(&who.name, "ANNOX_AUTHOR_NAME"), &ws.root)
        .map_err(|e| anyhow!(e))
}

fn no_workspace(from: &Path) -> anyhow::Error {
    anyhow!("{} is not in an annox workspace; run `annox init` at the project root", from.display())
}

/// The index of `ws`, warning once per run about event files it skipped,
/// and about read state that had to be repaired.
pub(crate) fn read_index(ws: &Workspace) -> Index {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Ok(Some(repaired)) = read::repair(ws) {
        eprintln!("annox: {repaired}");
    }
    let index = Index::read(ws);
    if let Some(first) = index.unreadable.first() {
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let n = index.unreadable.len();
            eprintln!("annox: ignoring {n} event file(s) that aren't valid events, such as {first}");
        }
    }
    index
}

fn open_workspace(cwd: &Path) -> anyhow::Result<(Workspace, Index)> {
    let ws = Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?;
    let index = read_index(&ws);
    Ok((ws, index))
}

/// A file argument: its workspace, document path, and absolute path.
fn open_file(cwd: &Path, file: &str) -> anyhow::Result<(Workspace, String, PathBuf)> {
    let abs = cwd.join(file).canonicalize().with_context(|| format!("can't open {file}"))?;
    let ws = Workspace::find(&abs).ok_or_else(|| no_workspace(&abs))?;
    let rel = ws.relative(&abs).ok_or_else(|| anyhow!("{file} is outside its workspace"))?;
    Ok((ws, rel, abs))
}

/// The current path of the document holding annotation `id`.
fn document_path(index: &Index, id: &str) -> anyhow::Result<String> {
    let (_, loc) = index.events.get(id).ok_or_else(|| anyhow!("unknown annotation {id}"))?;
    let doc = index.canonical(&loc.document);
    index.documents.get(doc).and_then(|d| d.path.clone()).ok_or_else(|| anyhow!("the document of {id} has no path"))
}

/// The derived state of annotation `id` (§2.5.6).
fn derived(index: &Index, id: &str) -> anyhow::Result<Value> {
    let path = document_path(index, id)?;
    index.load(&path).derive().remove(id).ok_or_else(|| anyhow!("unknown annotation {id}"))
}

/// Finds `quote` in `text`: its only occurrence, or the `occurrence`-th
/// (1-based) when given. Returns the code-point range.
fn find_quote(text: &Text, at: &Quote) -> anyhow::Result<(usize, usize)> {
    let Quote { quote, occurrence } = at;
    if quote.is_empty() {
        bail!("--quote can't be empty");
    }
    let normalized = annox_core::text::normalize(quote);
    let needle: Vec<char> = normalized.chars().collect();
    let starts: Vec<usize> =
        (0..=text.len().saturating_sub(needle.len())).filter(|&i| text.chars[i..].starts_with(&needle)).collect();
    let start = match (starts.as_slice(), occurrence) {
        ([], _) => bail!("the quote was not found in the file; quote the text exactly as it appears"),
        (_, &Some(n)) => *starts
            .get(n.wrapping_sub(1))
            .ok_or_else(|| anyhow!("--occurrence {n} is out of range: the quote occurs {} times", starts.len()))?,
        ([only], None) => *only,
        (many, None) => {
            let lines: Vec<String> = many.iter().map(|&s| line_of(text, s).to_string()).collect();
            bail!(
                "the quote occurs {} times (lines {}); quote more text, or pass --occurrence N",
                many.len(),
                lines.join(", ")
            )
        }
    };
    Ok((start, start + needle.len()))
}

/// The 1-based line of code-point offset `at`.
fn line_of(text: &Text, at: usize) -> usize {
    1 + text.chars[..at].iter().filter(|&&c| c == '\n').count()
}

/// Creates a comment, or a suggestion when there's a `replacement`.
fn create(
    file: &str,
    at: &Quote,
    replacement: Option<&str>,
    body: Option<&str>,
    meta: &Meta,
    who: &Author,
    cwd: &Path,
) -> anyhow::Result<Value> {
    let (ws, rel, abs) = open_file(cwd, file)?;
    let text = Text::from_raw(&std::fs::read_to_string(&abs)?);
    let (start, end) = find_quote(&text, at)?;
    let replacement = replacement.map(annox_core::text::normalize);
    let new = NewAnnotation {
        kind: if replacement.is_some() { "suggestion" } else { "comment" },
        start,
        end,
        body,
        label: meta.label.as_deref(),
        replacement: replacement.as_deref(),
        local: meta.local,
        reverts: None,
    };
    let index = read_index(&ws);
    let event = ops::create_annotation(&ws, &index, &rel, &text, &new, &author(who, &ws)?)?;
    Ok(json!({ "id": event.id, "path": rel, "line": line_of(&text, start) }))
}

fn list(
    file: Option<&str>,
    all: bool,
    closed: bool,
    reader_id: Option<String>,
    mut filter: Filter,
    cwd: &Path,
) -> anyhow::Result<Value> {
    let (ws, paths) = match file {
        Some(file) => {
            let (ws, rel, _) = open_file(cwd, file)?;
            (ws, BTreeSet::from([rel]))
        }
        None => {
            let ws = Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?;
            let index = read_index(&ws);
            let paths =
                index.documents.values().filter(|d| d.merged_into.is_none()).filter_map(|d| d.path.clone()).collect();
            (ws, paths)
        }
    };
    let me = reader(&author(&Author { author: reader_id, name: None }, &ws)?)?;
    if filter.mine || filter.others {
        if filter.mine { &mut filter.author } else { &mut filter.not_author }.push(me.clone());
    }
    let index = read_index(&ws);
    let events: Vec<Event> = index.events.values().map(|(e, _)| e.clone()).collect();
    let unread = read::ensure_reader(&ws, &index, &me)?.unread(&me, &events);
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(ws.root.join(&path)).ok().map(|raw| Text::from_raw(&raw));
        let quoted = match (&filter.quote, &text) {
            (Some(quote), Some(text)) => {
                let at = Quote { quote: quote.clone(), occurrence: filter.occurrence };
                Some((text, find_quote(text, &at)?))
            }
            (Some(_), None) => bail!("can't read {path}"),
            (None, _) => None,
        };
        let states = index.load(&path).derive();
        for (id, state) in &states {
            if state["kind"] == "reply" {
                continue;
            }
            let is_closed = state["status"] != "open";
            let deleted = state["deleted"] != json!(false);
            let status_ok = if filter.status.is_empty() {
                all || closed || quoted.is_some() || filter.unread || !is_closed
            } else {
                filter.status.iter().any(|s| state["status"] == json!(s))
            };
            if !status_ok || (deleted && !all) || !filter.matches_state(state) {
                continue;
            }
            let mut view = item(id, state, &path, text.as_ref(), &states, &index, all);
            if !mark_unread(&mut view, &unread) && filter.unread {
                continue;
            }
            if let Some((text, around)) = quoted {
                let target = serde_json::from_value(state["target"].clone()).ok();
                match target.and_then(|target| suggestion::current_range(text, &target, state)) {
                    Some(range) if anchor::overlaps(range, around) => {
                        if view.get("line").is_none() {
                            view["line"] = json!(line_of(text, range.0));
                        }
                    }
                    _ => continue,
                }
            }
            if !filter.broken || is_broken(&view) {
                out.push(view);
            }
        }
    }
    Ok(Value::Array(out))
}

impl Filter {
    /// The filters that depend only on the annotation's state.
    fn matches_state(&self, state: &Value) -> bool {
        let author = state["author"]["id"].as_str().unwrap_or_default();
        self.kind.as_ref().is_none_or(|k| state["kind"] == json!(k))
            && (self.author.is_empty() || self.author.iter().any(|a| a == author))
            && !self.not_author.iter().any(|a| a == author)
    }
}

/// Sets `unread` on a listed thread and its replies, and returns whether any
/// of them is unread.
fn mark_unread(view: &mut Value, unread: &BTreeSet<String>) -> bool {
    let is = |v: &Value| v["id"].as_str().is_some_and(|id| unread.contains(id));
    let mut any = is(view);
    view["unread"] = json!(any);
    for reply in view["replies"].as_array_mut().into_iter().flatten() {
        let r = is(reply);
        reply["unread"] = json!(r);
        any |= r;
    }
    any
}

/// Whether a listed annotation needs `retarget` or `reattach`.
fn is_broken(view: &Value) -> bool {
    view["status"] == "open" && (view["resolution"] == "orphaned" || view["applicable"] == json!(false))
}

/// The events of an annotation's thread in display order (§2.5.5).
fn history(id: &str, cwd: &Path) -> anyhow::Result<Value> {
    let (_, index) = open_workspace(cwd)?;
    let path = document_path(&index, id)?;
    Ok(json!(annox_core::replay::thread_events(&index.load(&path).events, id)))
}

fn show(id: &str, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let state = derived(&index, id)?;
    let id = match state["kind"].as_str() {
        Some("reply") => state["parent"].as_str().unwrap_or_default().to_owned(),
        _ => id.to_owned(),
    };
    let path = document_path(&index, &id)?;
    let text = std::fs::read_to_string(ws.root.join(&path)).ok().map(|raw| Text::from_raw(&raw));
    let states = index.load(&path).derive();
    let state = states.get(&id).ok_or_else(|| anyhow!("unknown annotation {id}"))?;
    Ok(item(&id, state, &path, text.as_ref(), &states, &index, true))
}

/// Copies `reactions` and `approvals` from a derived state into a listed
/// view, if there are any.
fn add_reactions(view: &mut Value, state: &Value) {
    for field in ["reactions", "approvals"] {
        let any = match &state[field] {
            Value::Object(o) => !o.is_empty(),
            Value::Array(a) => !a.is_empty(),
            _ => false,
        };
        if any {
            view[field] = state[field].clone();
        }
    }
}

/// One root annotation as listed: where it is now, and its thread, with
/// deleted replies only when `deleted_replies`.
fn item(
    id: &str,
    state: &Value,
    path: &str,
    text: Option<&Text>,
    states: &std::collections::BTreeMap<String, Value>,
    index: &Index,
    deleted_replies: bool,
) -> Value {
    let closed = state["status"] != "open";
    let area = index.events.get(id).map_or(Area::Shared, |(_, loc)| loc.area);
    let replies: Vec<Value> = states
        .values()
        .filter(|r| r["kind"] == "reply" && r["parent"] == json!(id))
        .filter(|r| deleted_replies || r["deleted"] == json!(false))
        .map(|r| {
            let mut reply = json!({
                "id": r["id"],
                "author": r["author"],
                "created": r["created"],
                "body": r["body"],
                "editedBy": r["editedBy"],
            });
            add_reactions(&mut reply, r);
            reply
        })
        .collect();
    let target: Option<Anchor> = serde_json::from_value(state["target"].clone()).ok();
    let quote = target.as_ref().map(|t| t.selectors.quote.exact.clone());
    let mut view = json!({
        "id": id,
        "path": path,
        "kind": state["kind"],
        "status": state["status"],
        "author": state["author"],
        "created": state["created"],
        "label": state["label"],
        "body": state["body"],
        "editedBy": state["editedBy"],
        "quote": quote,
    });
    if state["kind"] == "suggestion" {
        view["replacement"] = state["edit"]["replacement"].clone();
    }
    if let Some(reverts) = state.get("reverts") {
        view["reverts"] = reverts.clone();
    }
    // Closed annotations aren't resolved against the document (§4.4.1).
    if let (Some(target), Some(text), false) = (&target, text, closed) {
        let resolution = anchor::resolve(text, target);
        view["resolution"] = json!(match resolution.state {
            State::Exact => "exact",
            State::Relocated => "relocated",
            State::Orphaned => "orphaned",
        });
        if let Some((start, end)) = resolution.range {
            view["line"] = json!(line_of(text, start));
            view["quote"] = json!(text.slice(start, end));
        } else if let Some(s) = anchor::suggest(text, target) {
            let (start, end) = s.range;
            view["suggested"] = json!({
                "line": line_of(text, start),
                "quote": text.slice(start, end),
                "score": (s.score * 100.0).round() / 100.0,
            });
        }
        if state["kind"] == "suggestion" {
            view["applicable"] = json!(suggestion::is_applicable(&resolution));
        }
    }
    if state["deleted"] != json!(false) {
        view["deleted"] = json!(true);
    }
    add_reactions(&mut view, state);
    if area == Area::Local {
        view["local"] = json!(true);
    }
    if let Some(conflicts) = state["conflicts"].as_object().filter(|c| !c.is_empty()) {
        view["conflicts"] = json!(conflicts.keys().collect::<Vec<_>>());
    }
    view["replies"] = json!(replies);
    view
}

/// Points an open annotation at new text: `retarget` for a suggestion, with
/// its replacement reviewed in the same action (§4.2.1), or `reattach` for a
/// comment, which writes a `reanchor` (§3.8).
fn move_target(id: &str, at: &MoveTo, replace: Option<&str>, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let state = derived(&index, id)?;
    let (command, kind, other) =
        if replace.is_some() { ("retarget", "suggestion", "reattach") } else { ("reattach", "comment", "retarget") };
    if state["kind"] != kind {
        bail!(
            "only {kind}s can be {command}ed; use `annox {other}` for a {}",
            state["kind"].as_str().unwrap_or("reply")
        );
    }
    // Closed annotations keep their anchors (§4.4.1).
    if state["status"] != "open" {
        bail!("{id} is {}; only open annotations can be moved", state["status"].as_str().unwrap_or_default());
    }
    let path = document_path(&index, id)?;
    let raw = std::fs::read_to_string(ws.root.join(&path)).with_context(|| format!("can't read {path}"))?;
    let text = Text::from_raw(&raw);
    let (start, end) = match &at.quote {
        Some(quote) => find_quote(&text, &Quote { quote: quote.clone(), occurrence: at.occurrence })?,
        None => {
            let target: Anchor = serde_json::from_value(state["target"].clone())?;
            if anchor::resolve(&text, &target).state != State::Orphaned {
                bail!("{id} isn't orphaned, so it has no suggested location; pass --quote");
            }
            anchor::suggest(&text, &target)
                .ok_or_else(|| anyhow!("{id} has no suggested location; pass --quote"))?
                .range
        }
    };
    let mut fields = Map::from_iter([("target".into(), json!(anchor::create(&text, start, end, &path)))]);
    let event = if let Some(replace) = replace {
        let replacement = annox_core::text::normalize(replace);
        fields.insert("edit".into(), json!({ "replacement": replacement }));
        "retarget"
    } else {
        "reanchor"
    };
    ops::append_event(&ws, &index, id, event, fields, &author(who, &ws)?)?;
    Ok(json!({ "id": id, "path": path, "line": line_of(&text, start) }))
}

/// Reverts an accepted suggestion (§4.3.4): suggests putting its original
/// text back where the applied text is now, and accepts that if asked. An
/// open suggestion that already reverts it is used instead of a new one.
fn revert(id: &str, then_accept: bool, body: Option<&str>, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let state = derived(&index, id)?;
    if state["kind"] != "suggestion" || state["status"] != "accepted" || state["deleted"] != json!(false) {
        bail!("only accepted suggestions can be reverted");
    }
    let path = document_path(&index, id)?;
    let reverts = annox_core::replay::reverts_of(&index.load(&path).events, id);
    if let Some(done) = reverts.iter().find(|s| s["status"] == "accepted") {
        bail!("it was already reverted by {}", done["id"].as_str().unwrap_or_default());
    }
    let raw = std::fs::read_to_string(ws.root.join(&path)).with_context(|| format!("can't read {path}"))?;
    let text = Text::from_raw(&raw);
    let target: Anchor = serde_json::from_value(state["target"].clone())?;
    let replacement = state["edit"]["replacement"].as_str().context("the suggestion has no replacement")?;
    let (start, end) = suggestion::applied_text_search(&text, &target, replacement)
        .ok_or_else(|| anyhow!("the accepted text was changed since, so it can't be reverted automatically"))?;
    let revert_id = match reverts.iter().find(|s| s["status"] == "open") {
        Some(open) => open["id"].as_str().unwrap_or_default().to_owned(),
        None => {
            let new = NewAnnotation {
                kind: "suggestion",
                start,
                end,
                body,
                label: None,
                replacement: Some(&target.selectors.quote.exact),
                local: index.events.get(id).is_some_and(|(_, loc)| loc.area == Area::Local),
                reverts: Some(id),
            };
            ops::create_annotation(&ws, &index, &path, &text, &new, &author(who, &ws)?)?.id
        }
    };
    let mut out = if then_accept {
        accept(&revert_id, false, who, cwd)
            .with_context(|| format!("suggestion {revert_id} reverts it, but it couldn't be accepted"))?
    } else {
        json!({ "id": revert_id, "path": path, "line": line_of(&text, start) })
    };
    out["reverts"] = json!(id);
    Ok(out)
}

/// Accepts a suggestion (§4.3): writes the file first, then the status event.
fn accept(id: &str, confirmed: bool, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let state = derived(&index, id)?;
    if state["kind"] != "suggestion" || state["status"] != "open" || state["deleted"] != json!(false) {
        bail!("only open suggestions can be accepted");
    }
    let path = document_path(&index, id)?;
    let file = ws.root.join(&path);
    let raw = std::fs::read_to_string(&file).with_context(|| format!("can't read {path}"))?;
    let target: Anchor = serde_json::from_value(state["target"].clone())?;
    let replacement = state["edit"]["replacement"].as_str().context("the suggestion has no replacement")?;
    let applied = suggestion::apply(&Text::from_raw(&raw), &target, replacement)
        .map_err(|_| anyhow!("the suggestion is stale: its text is no longer in {path}"))?;
    // Relocation by quote or partial context may have picked another spot.
    if matches!(applied.resolution.step, 3 | 5) && !confirmed {
        let text = Text::from_raw(&raw);
        bail!(
            "the suggestion was relocated to line {} and may not be where it was made; check it, then pass --confirmed",
            line_of(&text, applied.start)
        );
    }
    // Keep the file's byte order mark and line endings (§4.3 step 3).
    let mut out = String::new();
    if raw.starts_with('\u{feff}') {
        out.push('\u{feff}');
    }
    let new_text = applied.text.to_string();
    if raw.contains("\r\n") {
        out.push_str(&new_text.replace('\n', "\r\n"));
    } else {
        out.push_str(&new_text);
    }
    // Refuse before changing the file, not when the status event can't be
    // written (§5.3).
    ws.check_format()?;
    std::fs::write(&file, out)?;
    let fields =
        Map::from_iter([("status".into(), json!("accepted")), ("appliedVersion".into(), json!(applied.text.version))]);
    ops::append_event(&ws, &index, id, "status", fields, &author(who, &ws)?)?;
    // Keep comments on the replaced text attached to its replacement (§4.3.5).
    let edits = [(applied.start, applied.end, replacement.chars().count())];
    let states = index.load(&path).derive();
    let mut carried = Vec::new();
    for (comment, target) in
        suggestion::carry_comments(&Text::from_raw(&raw), &applied.text, &path, &edits, states.iter())
    {
        let fields = Map::from_iter([("target".into(), serde_json::to_value(target)?)]);
        ops::append_event(&ws, &index, &comment, "reanchor", fields, &author(who, &ws)?)?;
        carried.push(comment);
    }
    let mut out = json!({ "id": id, "status": "accepted", "path": path, "appliedVersion": applied.text.version });
    if !carried.is_empty() {
        out["reanchored"] = json!(carried);
    }
    Ok(out)
}
