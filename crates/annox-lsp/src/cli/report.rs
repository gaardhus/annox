//! `annox report`: counts of annotations, by document or author.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::bail;
use serde_json::{json, Value};

use super::commit::{summarize, uncommitted};
use super::{list, open_workspace, Filter};

/// Counts of the annotations that `filter` lists, without deleted ones,
/// grouped by each of `by` in turn (default: by document). An author's group
/// counts the suggestions, comments and replies they wrote, so the groups add
/// up. A missing document's open annotations count as orphaned (§5.10). In a
/// git repository, also the events `annox commit` would commit.
pub(super) fn report(
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
