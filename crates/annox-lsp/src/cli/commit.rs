//! `annox commit`: committing the annotation events to git.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use annox_core::storage::{Index, Workspace};
use anyhow::{bail, Context};
use serde_json::{json, Map, Value};

use super::open_workspace;

pub(super) fn commit(message: Option<&str>, dry_run: bool, cwd: &Path) -> anyhow::Result<Value> {
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
pub(super) fn uncommitted(root: &Path) -> anyhow::Result<Vec<String>> {
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
pub(super) fn summarize<'a>(index: &'a Index, files: &[String]) -> BTreeMap<&'a str, [usize; 4]> {
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

/// The event id of an event file's path.
fn event_id(file: &str) -> Option<&str> {
    file.rsplit('/').next()?.strip_suffix(".json")
}
