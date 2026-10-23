//! Command-line access to a workspace's annotations, for people, scripts and
//! agents. Commands read and write `.annox/` directly through `annox-core`, and
//! return JSON, which `annox` prints as is with --json, or as text through
//! [`crate::render`]. Text is targeted by quoting it rather than by offsets.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use annox_core::anchor::{self, Anchor, State};
use annox_core::event::Event;
use annox_core::ops::{self, NewAnnotation};
use annox_core::read;
use annox_core::storage::{Area, Index, Workspace};
use annox_core::suggestion;
use annox_core::text::Text;
use anyhow::{anyhow, bail, Context};
use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};

/// The annotation commands. `annox` flattens these into its own subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print what everyone did to the annotations of FILE, or of every document, oldest first
    Log {
        /// A document in the workspace
        file: Option<String>,
        /// Keep running, and print new events as they're written, instead of the ones so far
        #[arg(short, long)]
        follow: bool,
    },
    /// Create a workspace in DIR (default: the current directory)
    Init {
        /// The project root
        dir: Option<PathBuf>,
        /// Keep `.annox/` out of version control, so annotations aren't shared
        #[arg(long)]
        local: bool,
    },
    /// Print the threads on FILE, or on every document
    List {
        /// A document in the workspace
        file: Option<String>,
        /// Also list resolved, accepted, rejected, withdrawn and deleted annotations
        #[arg(long)]
        all: bool,
        /// Also list resolved, accepted, rejected and withdrawn annotations, but not deleted ones
        #[arg(long, conflicts_with = "all")]
        closed: bool,
        /// Whose read state `unread` uses, and who `--mine` and `--others` mean [default: the identity write
        /// commands use]
        #[arg(long = "as", value_name = "ID")]
        reader: Option<String>,
        #[command(flatten)]
        filter: Filter,
    },
    /// Summarize the annotations of FILE, or of every document, one line per document or author
    Report {
        /// A document in the workspace
        file: Option<String>,
        /// Break each line's counts down by status, one line per kind
        #[arg(short, long)]
        verbose: bool,
        /// Group by document or by author; comma-separated to nest, as in `author,document`. An author counts
        /// the suggestions, comments and replies they wrote [default: document]
        #[arg(long, value_name = "GROUP", value_delimiter = ',', value_parser = ["document", "author"])]
        by: Vec<String>,
        /// Whose read state `unread` uses, and who `--mine` and `--others` mean [default: the identity write
        /// commands use]
        #[arg(long = "as", value_name = "ID")]
        reader: Option<String>,
        #[command(flatten)]
        filter: Filter,
    },
    /// Print one annotation and its thread
    Show {
        /// The annotation's id, as printed by `annox list`; a reply's shows its thread
        id: String,
    },
    /// Print who did what to a thread (the annotation and its replies), and when, oldest first
    History {
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
    /// Highlight a quote in FILE: a comment with no body
    Highlight {
        /// The document to annotate
        file: String,
        #[command(flatten)]
        at: Quote,
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
    /// Undo an accepted suggestion with a new suggestion that restores the original text, or the open one
    /// that already does
    Revert {
        /// The accepted suggestion's id, as printed by `annox list --closed`
        id: String,
        /// Accept the new suggestion right away, instead of leaving it open for review
        #[arg(long)]
        accept: bool,
        /// Why it's being reverted, in Markdown
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: Option<String>,
        #[command(flatten)]
        author: Author,
    },
    /// Move an open suggestion to new text, with a reviewed replacement
    Retarget {
        /// The annotation's id, as printed by `annox list`
        id: String,
        #[command(flatten)]
        at: MoveTo,
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
        at: MoveTo,
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
    /// React to an annotation or reply with an emoji; 👍 on a suggestion approves it
    React {
        /// The annotation's id, as printed by `annox list`
        id: String,
        /// The emoji, such as 👍 or 👀
        reaction: String,
        /// Remove the reaction instead
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        author: Author,
    },
    /// Mark threads read, so `annox list --unread` stops listing them until someone changes them again
    Read {
        /// Annotation ids, as printed by `annox list`; a reply's marks its thread
        #[arg(required_unless_present = "all")]
        ids: Vec<String>,
        /// Mark every thread read
        #[arg(long, conflicts_with = "ids")]
        all: bool,
        #[command(flatten)]
        author: Author,
    },
    /// Commit the uncommitted annotation events to git, and nothing else
    Commit {
        /// The commit message [default: a summary of what's being committed]
        #[arg(long, short, value_name = "TEXT", allow_hyphen_values = true)]
        message: Option<String>,
        /// Print what would be committed, without committing
        #[arg(long)]
        dry_run: bool,
    },
}

/// Options of `annox watch`, and of the channel of `annox mcp`.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct Watch {
    /// A document in the workspace [default: every document]
    pub file: Option<String>,
    /// Whose read state to follow; their own changes don't count [default: the identity write commands use]
    #[arg(long = "as", value_name = "ID")]
    pub reader: Option<String>,
    /// Only threads containing TEXT (case-insensitive), such as "@claude", or that the reader wrote in
    /// (repeatable)
    #[arg(long, value_name = "TEXT")]
    pub mention: Vec<String>,
    /// How long a thread must stay unchanged before it's printed, in milliseconds, so a reply still being
    /// written isn't picked up half done
    #[arg(long, value_name = "MS", default_value_t = 2000)]
    pub settle: u64,
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

/// Where `retarget` and `reattach` move an annotation.
#[derive(clap::Args, Debug)]
pub struct MoveTo {
    /// The text to target, exactly as it appears in the file
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true, required_unless_present = "suggested")]
    quote: Option<String>,
    /// Which occurrence of the quote to target (1-based), when it appears more than once
    #[arg(long, value_name = "N", requires = "quote")]
    occurrence: Option<usize>,
    /// Target the suggested location `annox list` shows for an orphaned annotation
    #[arg(long, conflicts_with = "quote")]
    suggested: bool,
}

/// Which annotations `list` prints and `report` counts. Filters combine with AND.
#[derive(clap::Args, Debug, Default)]
#[command(next_help_heading = "Filters")]
pub struct Filter {
    /// Only comments or only suggestions
    #[arg(long, value_name = "KIND", value_parser = ["comment", "suggestion"])]
    kind: Option<String>,
    /// Only these statuses (repeatable or comma-separated) [default: open, or any with --all, --closed or --quote;
    /// any in `report`]
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
    /// Only threads others changed since you last read them (see `annox read`), closed ones included
    #[arg(long)]
    unread: bool,
    /// Only annotations on this text in FILE, closed ones included. An accepted suggestion is on the text it put in
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true, requires = "file")]
    quote: Option<String>,
    /// Which occurrence of the quote to look at (1-based), when it appears more than once
    #[arg(long, value_name = "N", requires = "quote")]
    occurrence: Option<usize>,
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
            Ok(Value::Array(log_events(&Index::read(&ws), only.as_deref(), &BTreeSet::new())))
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

fn commit(message: Option<&str>, dry_run: bool, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    commit_workspace(&ws, &index, message, dry_run)
}

/// Commits everything under `.annox/` that git doesn't ignore, leaving
/// whatever else is staged alone. `cache/`, `local/` and `synced/` are
/// ignored (§5.10), so this commits the shared events written here. With
/// `dry_run`, it only reports what it would commit.
pub fn commit_workspace(ws: &Workspace, index: &Index, message: Option<&str>, dry_run: bool) -> anyhow::Result<Value> {
    let files = uncommitted(&ws.root)?;
    if files.is_empty() {
        return Ok(json!({ "commit": null, "files": 0, "documents": {}, "message": null }));
    }
    let summary = summarize(index, &files);
    let message = message.map_or_else(|| commit_message(&summary), str::to_owned);
    let documents: Map<String, Value> = summary
        .iter()
        .map(|(path, counts)| {
            let doc = CHANGES.iter().zip(counts).map(|((_, many), n)| (many.to_string(), json!(n)));
            (path.to_string(), Value::Object(doc.collect()))
        })
        .collect();
    let mut result = json!({ "commit": null, "files": files.len(), "documents": documents, "message": message });
    if !dry_run {
        git(&ws.root, &["add", "--all", "--", ".annox"])?;
        // Naming the paths commits only them, not the rest of the index.
        git(&ws.root, &["commit", "--quiet", "--message", &message, "--", ".annox"])?;
        result["commit"] = json!(git(&ws.root, &["rev-parse", "HEAD"])?.trim());
    }
    Ok(result)
}

/// The files under `.annox/` that differ from the last commit and that git
/// doesn't ignore, relative to the workspace root.
fn uncommitted(root: &Path) -> anyhow::Result<Vec<String>> {
    let changed =
        git(root, &["ls-files", "--modified", "--deleted", "--others", "--exclude-standard", "--", ".annox"])?;
    let staged = git(root, &["diff", "--cached", "--name-only", "--relative", "--no-renames", "--", ".annox"])?;
    let files: BTreeSet<&str> = changed.lines().chain(staged.lines()).collect();
    Ok(files.into_iter().map(str::to_owned).collect())
}

/// What uncommitted events do, singular and plural, in the order they're
/// counted in.
const CHANGES: [(&str, &str); 4] =
    [("comment", "comments"), ("suggestion", "suggestions"), ("reply", "replies"), ("update", "updates")];

/// Counts the events among `files` by document path and by [`CHANGES`].
/// Other files, such as document events, aren't counted.
fn summarize<'a>(index: &'a Index, files: &[String]) -> BTreeMap<&'a str, [usize; 4]> {
    let mut counts = BTreeMap::new();
    for (event, location) in files.iter().filter_map(|f| index.events.get(event_id(f)?)) {
        let kind = if event.kind == "create" { event.field("kind").and_then(Value::as_str) } else { Some("update") };
        let document = index.documents.get(index.canonical(&location.document));
        let (Some(path), Some(i)) =
            (document.and_then(|d| d.path.as_deref()), CHANGES.iter().position(|(k, _)| Some(*k) == kind))
        else {
            continue;
        };
        counts.entry(path).or_insert([0; 4])[i] += 1;
    }
    counts
}

/// A Conventional Commits message for `summary`, such as
/// `chore(annox): 2 comments, 1 reply on paper.md`.
fn commit_message(summary: &BTreeMap<&str, [usize; 4]>) -> String {
    let parts: Vec<String> = CHANGES
        .iter()
        .enumerate()
        .map(|(i, (one, many))| (summary.values().map(|c| c[i]).sum::<usize>(), one, many))
        .filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect();
    let what = if parts.is_empty() { "update annotations".to_owned() } else { parts.join(", ") };
    match summary.len() {
        0 => format!("chore(annox): {what}"),
        1 => format!("chore(annox): {what} on {}", summary.keys().next().expect("one path")),
        n => format!("chore(annox): {what} on {n} documents"),
    }
}

/// Runs git in `root`, returning its output, or its error message if it fails.
fn git(root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = std::process::Command::new("git").arg("-C").arg(root).args(args).output().context("running git")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        bail!("git {}: {}", args[0], if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
        reverts: None,
    };
    let index = Index::read(&ws);
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
            let index = Index::read(&ws);
            let paths =
                index.documents.values().filter(|d| d.merged_into.is_none()).filter_map(|d| d.path.clone()).collect();
            (ws, paths)
        }
    };
    let me = reader(&author(&Author { author: reader_id, name: None }, &ws)?)?;
    if filter.mine || filter.others {
        if filter.mine { &mut filter.author } else { &mut filter.not_author }.push(me.clone());
    }
    let index = Index::read(&ws);
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

/// How often `watch` checks `.annox/` for changes.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

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

/// Counts of the annotations that `filter` lists, without deleted ones,
/// grouped by each of `by` in turn (default: by document). An author's group
/// counts the suggestions, comments and replies they wrote, so the groups add
/// up. A missing document's open annotations count as orphaned (§5.10). In a
/// git repository, also the events `annox commit` would commit.
fn report(
    file: Option<&str>,
    by: &[String],
    reader: Option<String>,
    filter: Filter,
    cwd: &Path,
) -> anyhow::Result<Value> {
    let by: Vec<&str> = if by.is_empty() { vec!["document"] } else { by.iter().map(String::as_str).collect() };
    if let Some(twice) = by.iter().enumerate().find_map(|(i, b)| by[..i].contains(b).then_some(b)) {
        bail!("--by {twice} is given twice");
    }
    let (ws, index) = open_workspace(cwd)?;
    let files = uncommitted(&ws.root).ok();
    let uncommitted = files.as_ref().map(|files| summarize(&index, files));
    let views = list(file, false, true, reader, filter, cwd)?;
    let shares: Vec<Share> = views
        .as_array()
        .into_iter()
        .flatten()
        .map(|thread| Share {
            thread,
            missing: !ws.root.join(thread["path"].as_str().unwrap_or_default()).is_file(),
            root: true,
            replies: thread["replies"].as_array().into_iter().flatten().collect(),
        })
        .collect();
    let mut report = json!({ group_key(by[0]): group(&shares, &by, uncommitted.as_ref()) });
    if let Some(files) = files {
        report["uncommitted"] = json!(files.len());
    }
    Ok(report)
}

/// A listed thread, or the part of it one author wrote, as `report` counts it.
#[derive(Clone)]
struct Share<'a> {
    thread: &'a Value,
    /// Whether the thread's document is missing.
    missing: bool,
    /// Whether the comment or suggestion that starts the thread is part of it.
    root: bool,
    replies: Vec<&'a Value>,
}

/// The key of a `report` grouping's entries.
fn group_key(by: &str) -> &'static str {
    if by == "author" {
        "authors"
    } else {
        "documents"
    }
}

/// `report`'s entries for `shares` grouped by `by[0]`, each with its counts
/// and, nested, its entries grouped by the rest of `by`.
fn group(shares: &[Share], by: &[&str], uncommitted: Option<&BTreeMap<&str, [usize; 4]>>) -> Vec<Value> {
    let Some((first, rest)) = by.split_first() else { return vec![] };
    let mut groups: Vec<(Value, Vec<Share>)> = Vec::new();
    if *first == "author" {
        // Per author id, the part of each thread it wrote.
        let mut authors: BTreeMap<&str, Vec<Share>> = BTreeMap::new();
        for share in shares {
            let thread = share.thread;
            let entries = share.root.then_some(thread).into_iter().chain(share.replies.iter().copied());
            for entry in entries {
                let id = entry["author"]["id"].as_str().unwrap_or("unknown");
                let mine = authors.entry(id).or_default();
                if mine.last().is_none_or(|s| !std::ptr::eq(s.thread, thread)) {
                    let by_id = |e: &&Value| e["author"]["id"].as_str().unwrap_or("unknown") == id;
                    mine.push(Share {
                        root: share.root && by_id(&thread),
                        replies: share.replies.iter().copied().filter(by_id).collect(),
                        ..share.clone()
                    });
                }
            }
        }
        for (id, mine) in authors {
            let mut author = json!({ "id": id });
            if let Some(name) = authored_name(&mine) {
                author["name"] = json!(name);
            }
            groups.push((json!({ "author": author }), mine));
        }
    } else {
        for share in shares {
            let path = share.thread["path"].as_str().unwrap_or_default();
            if groups.last().is_none_or(|(g, _)| g["path"] != path) {
                let mut header = json!({ "path": path, "missing": share.missing });
                if let Some(counts) = uncommitted {
                    header["uncommitted"] = json!(counts.get(path).map_or(0, |c| c.iter().sum::<usize>()));
                }
                groups.push((header, Vec::new()));
            }
            groups.last_mut().expect("pushed above").1.push(share.clone());
        }
    }
    let mut entries: Vec<Value> = groups
        .into_iter()
        .map(|(mut entry, shares)| {
            let counts = count_shares(&shares);
            entry.as_object_mut().expect("an object").extend(counts.as_object().expect("an object").clone());
            if let Some(next) = rest.first() {
                entry[group_key(next)] = json!(group(&shares, rest, uncommitted));
            }
            entry
        })
        .collect();
    if *first == "author" {
        // Most open annotations first, then most replies.
        let count = |v: &Value| v.as_u64().unwrap_or(0);
        entries.sort_by_key(|e| {
            std::cmp::Reverse((count(&e["suggestions"]["open"]) + count(&e["comments"]["open"]), count(&e["replies"])))
        });
    }
    entries
}

/// The name an author's latest entry among `shares` gives, if any: names
/// can change while ids stay.
fn authored_name(shares: &[Share]) -> Option<String> {
    shares
        .iter()
        .flat_map(|s| s.root.then_some(s.thread).into_iter().chain(s.replies.iter().copied()))
        .filter_map(|e| Some((e["created"].as_str().unwrap_or_default(), e["author"]["name"].as_str()?)))
        .max()
        .map(|(_, name)| name.to_owned())
}

/// The counts of a `report` entry: suggestions and comments by status, the
/// open suggestions someone approved (§4.4.2), replies, and the open
/// annotations that need work.
fn count_shares(shares: &[Share]) -> Value {
    let mut counts = json!({
        "suggestions": { "open": 0, "accepted": 0, "rejected": 0, "withdrawn": 0 },
        "comments": { "open": 0, "resolved": 0 },
        "approved": 0,
        "replies": 0,
        "orphaned": 0,
        "stale": 0,
        "conflicted": 0,
    });
    let add = |count: &mut Value, n: usize| *count = json!(count.as_u64().unwrap_or(0) + n as u64);
    for share in shares {
        add(&mut counts["replies"], share.replies.len());
        if !share.root {
            continue;
        }
        let thread = share.thread;
        let (kind, status) =
            (thread["kind"].as_str().unwrap_or_default(), thread["status"].as_str().unwrap_or_default());
        add(&mut counts[format!("{kind}s")][status], 1);
        if status == "open" && kind == "suggestion" && is_approved(thread) {
            add(&mut counts["approved"], 1);
        }
        if status == "open" {
            if share.missing || thread["resolution"] == "orphaned" {
                add(&mut counts["orphaned"], 1);
            } else if thread["applicable"] == json!(false) {
                add(&mut counts["stale"], 1);
            }
        }
        if thread.get("conflicts").is_some() {
            add(&mut counts["conflicted"], 1);
        }
    }
    counts
}

/// Whether a listed suggestion has an approval that isn't stale (§2.5.7).
fn is_approved(view: &Value) -> bool {
    view["approvals"].as_array().is_some_and(|a| a.iter().any(|a| a["stale"] == json!(false)))
}

/// The event id of an event file's path.
fn event_id(file: &str) -> Option<&str> {
    file.rsplit('/').next()?.strip_suffix(".json")
}

/// The events of an annotation's thread in display order (§2.5.5).
fn history(id: &str, cwd: &Path) -> anyhow::Result<Value> {
    let (_, index) = open_workspace(cwd)?;
    let path = document_path(&index, id)?;
    Ok(json!(annox_core::replay::thread_events(&index.load(&path).events, id)))
}

/// The workspace a log covers, and the one document it's limited to, if any.
fn log_scope(file: Option<&str>, cwd: &Path) -> anyhow::Result<(Workspace, Option<String>)> {
    match file {
        Some(file) => open_file(cwd, file).map(|(ws, rel, _)| (ws, Some(rel))),
        None => Ok((Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?, None)),
    }
}

/// The annotation events in `index`, oldest first, except those in `skip`,
/// and only those on the document at `only` if given. Each has `path`, its
/// document's current path, and `thread`, the id of its root annotation.
fn log_events(index: &Index, only: Option<&str>, skip: &BTreeSet<String>) -> Vec<Value> {
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
    let replacement = state["edit"]["replacement"].as_str().unwrap_or_default();
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
