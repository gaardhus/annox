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
use serde_json::{json, Map, Value};

pub const USAGE: &str = "  annox init [DIR]                            create a workspace in DIR (default: current directory)
  annox list [FILE] [--all]                   annotations of FILE, or of every document, as JSON
  annox comment FILE --quote TEXT --body TEXT [--occurrence N] [--label L] [--local]
  annox suggest FILE --quote TEXT --replace TEXT [--body TEXT] [--occurrence N] [--label L] [--local]
  annox reply ID --body TEXT                  reply to a comment or suggestion
  annox edit ID [--body TEXT] [--label L]     change the body or label of an annotation
  annox status ID STATUS                      open, resolved (comments), rejected or withdrawn (suggestions)
  annox accept ID [--confirmed]               apply a suggestion to its file and mark it accepted
  annox retarget ID --quote TEXT --replace TEXT [--occurrence N]
                                              move an open suggestion to new text, with a reviewed replacement
  annox reattach ID --quote TEXT [--occurrence N]
                                              move an open comment to new text
  annox delete ID                             hide an annotation (its events are kept)
  annox restore ID                            undo a delete

  Write commands take --author ID and --name NAME, or read ANNOX_AUTHOR and
  ANNOX_AUTHOR_NAME, then the author in ~/.config/annox/config.json, and
  otherwise use git's identity.";

/// Parsed arguments: positionals, `--flag value` options, and switches.
struct Args {
    positional: Vec<String>,
    options: Map<String, Value>,
}

const SWITCHES: [&str; 3] = ["all", "local", "confirmed"];

impl Args {
    fn parse(args: &[String]) -> anyhow::Result<Args> {
        let mut positional = Vec::new();
        let mut options = Map::new();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let Some(name) = arg.strip_prefix("--") else {
                positional.push(arg.clone());
                continue;
            };
            if SWITCHES.contains(&name) {
                options.insert(name.to_owned(), json!(true));
            } else {
                let value = it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?;
                options.insert(name.to_owned(), json!(value));
            }
        }
        Ok(Args { positional, options })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.options.get(name).and_then(Value::as_str)
    }

    fn require(&self, name: &str) -> anyhow::Result<&str> {
        self.get(name).ok_or_else(|| anyhow!("--{name} is required"))
    }

    fn switch(&self, name: &str) -> bool {
        self.options.contains_key(name)
    }

    fn positional(&self, i: usize, what: &str) -> anyhow::Result<&str> {
        self.positional.get(i).map(String::as_str).ok_or_else(|| anyhow!("missing {what}"))
    }

    /// Fails on options the command doesn't take, so typos aren't ignored.
    fn only(&self, allowed: &[&str]) -> anyhow::Result<()> {
        let allowed: Vec<&str> = allowed.iter().chain(["author", "name"].iter()).copied().collect();
        match self.options.keys().find(|k| !allowed.contains(&k.as_str())) {
            Some(k) => bail!("unknown option --{k}"),
            None => Ok(()),
        }
    }
}

/// Runs a command (the arguments after `annox`) with `cwd` as the current
/// directory, and returns its JSON output.
pub fn run(args: &[String], cwd: &Path) -> anyhow::Result<Value> {
    let (command, rest) = args.split_first().ok_or_else(|| anyhow!("missing command"))?;
    let args = Args::parse(rest)?;
    let cwd = cwd.canonicalize()?;
    match command.as_str() {
        "init" => {
            args.only(&[])?;
            let dir = args.positional.first().map_or(cwd.clone(), |d| cwd.join(d));
            let ws = Workspace::init(&dir)?;
            Ok(json!({ "root": ws.root }))
        }
        "list" => {
            args.only(&["all"])?;
            list(&args, &cwd)
        }
        "comment" | "suggest" => {
            args.only(&["quote", "body", "replace", "occurrence", "label", "local"])?;
            create(command, &args, &cwd)
        }
        "reply" => {
            args.only(&["body"])?;
            let (ws, index) = open_workspace(&cwd)?;
            let id = args.positional(0, "annotation id")?;
            let state = derived(&index, id)?;
            if state["kind"] == "reply" {
                bail!("{id} is a reply; reply to its parent {}", state["parent"].as_str().unwrap_or_default());
            }
            let event = ops::create_reply(&ws, &index, id, args.require("body")?, false, &author(&args, &ws)?)?;
            Ok(json!({ "id": event.id }))
        }
        "edit" => {
            args.only(&["body", "label"])?;
            let (ws, index) = open_workspace(&cwd)?;
            let id = args.positional(0, "annotation id")?;
            let state = derived(&index, id)?;
            let mut fields = Map::new();
            if let Some(body) = args.get("body") {
                fields.insert("body".into(), json!(body));
            }
            if let Some(label) = args.get("label") {
                if state["kind"] == "reply" {
                    bail!("replies have no label");
                }
                fields.insert("label".into(), if label.is_empty() { Value::Null } else { json!(label) });
            }
            if fields.is_empty() {
                bail!("nothing to change: pass --body or --label");
            }
            ops::append_event(&ws, &index, id, "edit", fields, &author(&args, &ws)?)?;
            Ok(json!({ "id": id }))
        }
        "status" => {
            args.only(&[])?;
            let (ws, index) = open_workspace(&cwd)?;
            let id = args.positional(0, "annotation id")?;
            let status = args.positional(1, "status")?;
            let state = derived(&index, id)?;
            let allowed: &[&str] = match state["kind"].as_str() {
                Some("comment") => &["open", "resolved"],
                Some("suggestion") => &["open", "rejected", "withdrawn"],
                _ => bail!("replies have no status"),
            };
            if status == "accepted" && state["kind"] == "suggestion" {
                bail!("use `annox accept {id}`, which also applies the suggestion");
            }
            if !allowed.contains(&status) {
                bail!("a {} can't be {status}; use one of: {}", state["kind"].as_str().unwrap_or_default(), allowed.join(", "));
            }
            if state["status"] == "accepted" {
                bail!("{id} is accepted, which is final; make a new suggestion to undo it");
            }
            let fields = Map::from_iter([("status".into(), json!(status))]);
            ops::append_event(&ws, &index, id, "status", fields, &author(&args, &ws)?)?;
            Ok(json!({ "id": id, "status": status }))
        }
        "accept" => {
            args.only(&["confirmed"])?;
            accept(&args, &cwd)
        }
        "retarget" | "reattach" => {
            args.only(&["quote", "replace", "occurrence"])?;
            move_target(command, &args, &cwd)
        }
        "delete" | "restore" => {
            args.only(&[])?;
            let (ws, index) = open_workspace(&cwd)?;
            let id = args.positional(0, "annotation id")?;
            derived(&index, id)?;
            ops::append_event(&ws, &index, id, command, Map::new(), &author(&args, &ws)?)?;
            Ok(json!({ "id": id, "deleted": command == "delete" }))
        }
        other => bail!("unknown command {other}"),
    }
}

/// The author to write events as: flags, then environment, then the user
/// config file, then git (§2.7).
fn author(args: &Args, ws: &Workspace) -> anyhow::Result<Value> {
    let get = |flag, var| args.get(flag).map(str::to_owned).or_else(|| std::env::var(var).ok().filter(|v| !v.is_empty()));
    crate::resolve_author(get("author", "ANNOX_AUTHOR"), get("name", "ANNOX_AUTHOR_NAME"), &ws.root).map_err(|e| anyhow!(e))
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
fn find_quote(text: &Text, quote: &str, occurrence: Option<&str>) -> anyhow::Result<(usize, usize)> {
    if quote.is_empty() {
        bail!("--quote can't be empty");
    }
    let normalized = annox_core::text::normalize(quote);
    let needle: Vec<char> = normalized.chars().collect();
    let starts: Vec<usize> = (0..=text.len().saturating_sub(needle.len()))
        .filter(|&i| text.chars[i..].starts_with(&needle))
        .collect();
    let start = match (starts.as_slice(), occurrence) {
        ([], _) => bail!("the quote was not found in the file; quote the text exactly as it appears"),
        (_, Some(n)) => {
            let n: usize = n.parse().context("--occurrence must be a number")?;
            *starts.get(n.wrapping_sub(1)).ok_or_else(|| {
                anyhow!("--occurrence {n} is out of range: the quote occurs {} times", starts.len())
            })?
        }
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

fn create(command: &str, args: &Args, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, rel, abs) = open_file(cwd, args.positional(0, "file")?)?;
    let text = Text::from_raw(&std::fs::read_to_string(&abs)?);
    let (start, end) = find_quote(&text, args.require("quote")?, args.get("occurrence"))?;
    let replacement = if command == "suggest" {
        Some(annox_core::text::normalize(args.require("replace")?))
    } else {
        None
    };
    let body = args.get("body");
    if command == "comment" && body.is_none() {
        bail!("--body is required");
    }
    let new = NewAnnotation {
        kind: if command == "suggest" { "suggestion" } else { "comment" },
        start,
        end,
        body,
        label: args.get("label"),
        replacement: replacement.as_deref(),
        local: args.switch("local"),
    };
    let index = Index::read(&ws);
    let event = ops::create_annotation(&ws, &index, &rel, &text, &new, &author(args, &ws)?)?;
    Ok(json!({ "id": event.id, "path": rel, "line": line_of(&text, start) }))
}

fn list(args: &Args, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, paths) = match args.positional.first() {
        Some(file) => {
            let (ws, rel, _) = open_file(cwd, file)?;
            (ws, BTreeSet::from([rel]))
        }
        None => {
            let ws = Workspace::find(cwd).ok_or_else(|| no_workspace(cwd))?;
            let index = Index::read(&ws);
            let paths = index.documents.values().filter(|d| d.merged_into.is_none()).filter_map(|d| d.path.clone()).collect();
            (ws, paths)
        }
    };
    let index = Index::read(&ws);
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(ws.root.join(&path)).ok().map(|raw| Text::from_raw(&raw));
        let states = index.load(&path).derive();
        let mut replies: Vec<&Value> = states.values().filter(|s| s["kind"] == "reply").collect();
        for (id, state) in &states {
            if state["kind"] == "reply" {
                continue;
            }
            let closed = state["status"] != "open";
            let deleted = state["deleted"] != json!(false);
            if !args.switch("all") && (closed || deleted) {
                continue;
            }
            let area = index.events.get(id).map_or(Area::Shared, |(_, loc)| loc.area);
            let thread: Vec<Value> = replies
                .iter()
                .filter(|r| r["parent"] == json!(id) && (args.switch("all") || r["deleted"] == json!(false)))
                .map(|r| json!({ "id": r["id"], "author": r["author"], "created": r["created"], "body": r["body"] }))
                .collect();
            replies.retain(|r| r["parent"] != json!(id));
            out.push(item(id, state, &path, text.as_ref(), closed, area, thread));
        }
    }
    Ok(Value::Array(out))
}

/// One root annotation as listed: where it is now, and its thread.
fn item(id: &str, state: &Value, path: &str, text: Option<&Text>, closed: bool, area: Area, replies: Vec<Value>) -> Value {
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
fn move_target(command: &str, args: &Args, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let id = args.positional(0, "annotation id")?;
    let state = derived(&index, id)?;
    let kind = if command == "retarget" { "suggestion" } else { "comment" };
    if state["kind"] != kind {
        let other = if command == "retarget" { "reattach" } else { "retarget" };
        bail!("only {kind}s can be {command}ed; use `annox {other}` for a {}", state["kind"].as_str().unwrap_or("reply"));
    }
    // Closed annotations keep their anchors (§4.4.1).
    if state["status"] != "open" {
        bail!("{id} is {}; only open annotations can be moved", state["status"].as_str().unwrap_or_default());
    }
    let path = document_path(&index, id)?;
    let raw = std::fs::read_to_string(ws.root.join(&path)).with_context(|| format!("can't read {path}"))?;
    let text = Text::from_raw(&raw);
    let (start, end) = find_quote(&text, args.require("quote")?, args.get("occurrence"))?;
    let mut fields = Map::from_iter([("target".into(), json!(anchor::create(&text, start, end, &path)))]);
    let event = if command == "retarget" {
        let replacement = annox_core::text::normalize(args.require("replace")?);
        fields.insert("edit".into(), json!({ "replacement": replacement }));
        "retarget"
    } else {
        "reanchor"
    };
    ops::append_event(&ws, &index, id, event, fields, &author(args, &ws)?)?;
    Ok(json!({ "id": id, "path": path, "line": line_of(&text, start) }))
}

/// Accepts a suggestion (§4.3): writes the file first, then the status event.
fn accept(args: &Args, cwd: &Path) -> anyhow::Result<Value> {
    let (ws, index) = open_workspace(cwd)?;
    let id = args.positional(0, "annotation id")?;
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
    if matches!(applied.resolution.step, 3 | 5) && !args.switch("confirmed") {
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
    let fields = Map::from_iter([
        ("status".into(), json!("accepted")),
        ("appliedVersion".into(), json!(applied.text.version)),
    ]);
    ops::append_event(&ws, &index, id, "status", fields, &author(args, &ws)?)?;
    Ok(json!({ "id": id, "status": "accepted", "path": path, "appliedVersion": applied.text.version }))
}
