//! Command-line access to a workspace's annotations, for scripts and agents.
//! Commands read and write `.annox/` directly through `annox-core`, and print
//! JSON. Text is targeted by quoting it rather than by offsets.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use annox_core::anchor::{self, Anchor, State};
use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::{Area, Index, Workspace};
use annox_core::suggestion;
use annox_core::text::Text;
use anyhow::{anyhow, bail, Context};
use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};

/// The annotation commands. `annox` flattens these into its own subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a workspace in DIR (default: the current directory)
    Init {
        /// The project root
        dir: Option<PathBuf>,
    },
    /// Print the annotations of FILE, or of every document, as JSON
    List {
        /// A document in the workspace
        file: Option<String>,
        /// Also list resolved, accepted, rejected, withdrawn and deleted annotations
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        filter: Filter,
    },
    /// Print one annotation and its thread as JSON
    Show {
        /// The annotation's id, as printed by `annox list`; a reply's shows its thread
        id: String,
    },
    /// Comment on a quote in FILE
    Comment {
        /// The document to annotate
        file: String,
        #[command(flatten)]
        at: Quote,
        /// The comment
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: String,
        #[command(flatten)]
        meta: Meta,
        #[command(flatten)]
        author: Author,
    },
    /// Suggest a replacement for a quote in FILE
    Suggest {
        /// The document to annotate
        file: String,
        #[command(flatten)]
        at: Quote,
        /// The text to put in place of the quote
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        replace: String,
        /// An explanation of the suggestion
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: Option<String>,
        #[command(flatten)]
        meta: Meta,
        #[command(flatten)]
        author: Author,
    },
    /// Reply to a comment or suggestion
    Reply {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The reply
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: String,
        #[command(flatten)]
        author: Author,
    },
    /// Change the body or label of an annotation
    Edit {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The new body
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: Option<String>,
        /// The new label; an empty one removes it
        #[arg(long, value_name = "L", allow_hyphen_values = true)]
        label: Option<String>,
        #[command(flatten)]
        author: Author,
    },
    /// Set a status: open or resolved (comments), open, rejected or withdrawn (suggestions)
    Status {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The new status
        status: String,
        #[command(flatten)]
        author: Author,
    },
    /// Apply a suggestion to its file and mark it accepted
    Accept {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// Accept a suggestion that was relocated, after checking where it now applies
        #[arg(long)]
        confirmed: bool,
        #[command(flatten)]
        author: Author,
    },
    /// Move an open suggestion to new text, with a reviewed replacement
    Retarget {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        at: Quote,
        /// The replacement, reviewed against the new text
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        replace: String,
        #[command(flatten)]
        author: Author,
    },
    /// Move an open comment to new text
    Reattach {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        at: Quote,
        #[command(flatten)]
        author: Author,
    },
    /// Hide an annotation (its events are kept)
    Delete {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        author: Author,
    },
    /// Undo a delete
    Restore {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        author: Author,
    },
}

/// The text a command targets.
#[derive(clap::Args, Debug)]
pub struct Quote {
    /// The text to target, exactly as it appears in the file
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    quote: String,
    /// Which occurrence of the quote to target (1-based), when it appears more than once
    #[arg(long, value_name = "N")]
    occurrence: Option<usize>,
}

/// Which annotations `list` prints. Filters combine with AND.
#[derive(clap::Args, Debug, Default)]
#[command(next_help_heading = "Filters")]
pub struct Filter {
    /// Only comments or only suggestions
    #[arg(long, value_name = "KIND", value_parser = ["comment", "suggestion"])]
    kind: Option<String>,
    /// Only these statuses (repeatable or comma-separated) [default: open, or any with --all]
    #[arg(long, value_name = "S", value_delimiter = ',',
          value_parser = ["open", "resolved", "accepted", "rejected", "withdrawn"])]
    status: Vec<String>,
    /// Only annotations by this author id (repeatable)
    #[arg(long, value_name = "ID")]
    author: Vec<String>,
    /// Skip annotations by this author id (repeatable)
    #[arg(long, value_name = "ID")]
    not_author: Vec<String>,
    /// Only your own annotations (the identity write commands use)
    #[arg(long, conflicts_with = "others")]
    mine: bool,
    /// Skip your own annotations (the identity write commands use)
    #[arg(long)]
    others: bool,
    /// Only open annotations that need fixing: orphaned, or suggestions that can't be applied
    #[arg(long)]
    broken: bool,
}

/// Options of a new comment or suggestion.
#[derive(clap::Args, Debug)]
pub struct Meta {
    /// A label, such as "question" or "typo"
    #[arg(long, value_name = "L", allow_hyphen_values = true)]
    label: Option<String>,
    /// Keep it in the local area, which isn't shared
    #[arg(long)]
    local: bool,
}

/// Who a write command writes as.
#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Author")]
pub struct Author {
    /// Author id [default: ANNOX_AUTHOR, then ~/.config/annox/config.json, then git's identity]
    #[arg(long, value_name = "ID")]
    author: Option<String>,
    /// Display name [default: ANNOX_AUTHOR_NAME, then the config file, then git]
    #[arg(long, value_name = "NAME", allow_hyphen_values = true)]
    name: Option<String>,
}

#[derive(Parser)]
#[command(name = "annox")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Parses and runs a command (the arguments after `annox`) with `cwd` as the
/// current directory, and returns its JSON output.
pub fn run(args: &[String], cwd: &Path) -> anyhow::Result<Value> {
    let cli = Cli::try_parse_from(std::iter::once("annox").chain(args.iter().map(String::as_str)))
        .map_err(|e| anyhow!("{}", e.render().to_string().trim_end()))?;
    execute(cli.command, cwd)
}

/// Runs a parsed command with `cwd` as the current directory, and returns its
/// JSON output.
pub fn execute(command: Command, cwd: &Path) -> anyhow::Result<Value> {
    let cwd = cwd.canonicalize()?;
    match command {
        Command::Init { dir } => {
            let dir = dir.map_or(cwd.clone(), |d| cwd.join(d));
            let ws = Workspace::init(&dir)?;
            Ok(json!({ "root": ws.root }))
        }
        Command::List { file, all, filter } => list(file.as_deref(), all, filter, &cwd),
        Command::Show { id } => show(&id, &cwd),
        Command::Comment { file, at, body, meta, author } => {
            create(&file, &at, None, Some(&body), &meta, &author, &cwd)
        }
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
        Command::Retarget { id, at, replace, author } => move_target(&id, &at, Some(&replace), &author, &cwd),
        Command::Reattach { id, at, author } => move_target(&id, &at, None, &author, &cwd),
        Command::Delete { id, author } => set_deleted(&id, true, &author, &cwd),
        Command::Restore { id, author } => set_deleted(&id, false, &author, &cwd),
    }
}

fn set_deleted(id: &str, deleted: bool, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    derived(&index, id)?;
    let event = if deleted { "delete" } else { "restore" };
    ops::append_event(&ws, &index, id, event, Map::new(), &author(who, &ws)?)?;
    Ok(json!({ "id": id, "deleted": deleted }))
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

fn open_workspace(cwd: &Path) -> anyhow::Result<(Workspace, Index)> {
    let ws = Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?;
    let index = Index::read(&ws);
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
    };
    let index = Index::read(&ws);
    let event = ops::create_annotation(&ws, &index, &rel, &text, &new, &author(who, &ws)?)?;
    Ok(json!({ "id": event.id, "path": rel, "line": line_of(&text, start) }))
}

fn list(file: Option<&str>, all: bool, mut filter: Filter, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, paths) = match file {
        Some(file) => {
            let (ws, rel, _) = open_file(cwd, file)?;
            (ws, BTreeSet::from([rel]))
        }
        None => {
            let ws = Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?;
            let index = Index::read(&ws);
            let paths =
                index.documents.values().filter(|d| d.merged_into.is_none()).filter_map(|d| d.path.clone()).collect();
            (ws, paths)
        }
    };
    if filter.mine || filter.others {
        let me = author(&Author { author: None, name: None }, &ws)?;
        let me = me["id"].as_str().ok_or_else(|| anyhow!("no identity: set ANNOX_AUTHOR or pass --author ID"))?;
        if filter.mine { &mut filter.author } else { &mut filter.not_author }.push(me.to_owned());
    }
    let index = Index::read(&ws);
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(ws.root.join(&path)).ok().map(|raw| Text::from_raw(&raw));
        let states = index.load(&path).derive();
        for (id, state) in &states {
            if state["kind"] == "reply" {
                continue;
            }
            let closed = state["status"] != "open";
            let deleted = state["deleted"] != json!(false);
            let status_ok = if filter.status.is_empty() {
                all || !closed
            } else {
                filter.status.iter().any(|s| state["status"] == json!(s))
            };
            if !status_ok || (deleted && !all) || !filter.matches_state(state) {
                continue;
            }
            let view = item(id, state, &path, text.as_ref(), &states, &index, all);
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

/// Whether a listed annotation needs `retarget` or `reattach`.
fn is_broken(view: &Value) -> bool {
    view["status"] == "open" && (view["resolution"] == "orphaned" || view["applicable"] == json!(false))
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
        .map(|r| json!({ "id": r["id"], "author": r["author"], "created": r["created"], "body": r["body"] }))
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
        "quote": quote,
    });
    if state["kind"] == "suggestion" {
        view["replacement"] = state["edit"]["replacement"].clone();
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
        }
        if state["kind"] == "suggestion" {
            view["applicable"] = json!(suggestion::is_applicable(&resolution));
        }
    }
    if state["deleted"] != json!(false) {
        view["deleted"] = json!(true);
    }
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
fn move_target(id: &str, at: &Quote, replace: Option<&str>, who: &Author, cwd: &Path) -> anyhow::Result<Value> {
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
    let (start, end) = find_quote(&text, at)?;
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
    let replacement = state["edit"]["replacement"].as_str().unwrap_or_default();
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
    std::fs::write(&file, out)?;
    let fields =
        Map::from_iter([("status".into(), json!("accepted")), ("appliedVersion".into(), json!(applied.text.version))]);
    ops::append_event(&ws, &index, id, "status", fields, &author(who, &ws)?)?;
    Ok(json!({ "id": id, "status": "accepted", "path": path, "appliedVersion": applied.text.version }))
}
