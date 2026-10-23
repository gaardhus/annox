//! The CLI's text output: what each command prints unless it's asked for
//! JSON (see [`json_output`]).
//!
//! Each renderer takes the JSON the command returns, as [`crate::cli`]
//! documents it, and emits ANSI styles; print the result through `anstream`
//! (see [`style`]).

use std::collections::BTreeSet;

use anyhow::bail;
use serde_json::{json, Value};

use crate::cli::Command;
use crate::style::{self, paint};

/// A command's output as text.
pub type Render = Box<dyn Fn(&Value) -> String>;

/// Whether to print JSON rather than text: with `--json` (`flag`), or when
/// `ANNOX_OUTPUT` is `json`, or, without it, `output` in the user config file.
pub fn json_output(flag: bool) -> anyhow::Result<bool> {
    if flag {
        return Ok(true);
    }
    match std::env::var("ANNOX_OUTPUT").ok().filter(|v| !v.is_empty()).as_deref() {
        Some("json") => Ok(true),
        Some("text") => Ok(false),
        Some(other) => bail!("ANNOX_OUTPUT is {other:?}; use json or text"),
        // An invalid config file is reported by what needs the author in it.
        None => Ok(annox_core::config::load().ok().and_then(|c| c.output).as_deref() == Some("json")),
    }
}

/// How `command` prints its output as text.
pub fn of(command: &Command) -> Render {
    match command {
        Command::Init { .. } => {
            Box::new(|v| format!("Created a workspace in {}\n", paint(style::PATH, str(&v["root"]))))
        }
        Command::List { .. } => Box::new(threads),
        Command::Show { .. } => Box::new(|v| threads(&json!([v]))),
        Command::Report { verbose: true, .. } => Box::new(report_verbose),
        Command::Report { .. } => Box::new(report),
        Command::History { .. } => Box::new(history),
        Command::Log { .. } => Box::new(log),
        Command::Comment { .. } => Box::new(|v| created("Commented on", v)),
        Command::Highlight { .. } => Box::new(|v| created("Highlighted", v)),
        Command::Suggest { .. } => Box::new(|v| created("Suggested an edit to", v)),
        Command::Revert { .. } => Box::new(reverted),
        Command::Reply { .. } => Box::new(|v| done("Replied", v)),
        Command::Edit { .. } => Box::new(|v| done("Edited", v)),
        Command::Status { .. } => Box::new(|v| {
            let status = str(&v["status"]);
            let verb = match status {
                "open" => "Reopened",
                "resolved" => "Resolved",
                "rejected" => "Rejected",
                "withdrawn" => "Withdrew",
                other => other,
            };
            format!("{}  {}\n", paint(style::of(status), verb), id(v))
        }),
        Command::Accept { .. } => Box::new(accepted),
        Command::Retarget { .. } | Command::Reattach { .. } => {
            Box::new(|v| format!("Moved {} to {}\n", id(v), place(v)))
        }
        Command::Delete { .. } => Box::new(|v| done("Deleted", v)),
        Command::Restore { .. } => Box::new(|v| done("Restored", v)),
        Command::React { .. } => Box::new(reacted),
        Command::Read { .. } => Box::new(|v| {
            let n = v["annotations"].as_array().map_or(0, Vec::len) as u64;
            format!("Marked {} read\n", plural(n, "thread"))
        }),
        Command::Commit { .. } => Box::new(committed),
        Command::Config => Box::new(config),
    }
}

/// `v` as a string, or "" if it isn't one.
fn str(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// The display name of the author `author`, or its id.
fn name(author: &Value) -> &str {
    author["name"].as_str().or(author["id"].as_str()).unwrap_or("unknown")
}

/// The annotation id of a command's output, muted.
fn id(v: &Value) -> String {
    paint(style::MUTED, str(&v["id"]))
}

/// Where a command's output says it wrote: `path:line`.
fn place(v: &Value) -> String {
    match v["line"].as_u64() {
        Some(line) => paint(style::PATH, format!("{}:{line}", str(&v["path"]))),
        None => paint(style::PATH, str(&v["path"])),
    }
}

/// What a write command that only names its annotation prints.
fn done(verb: &str, v: &Value) -> String {
    format!("{verb}  {}\n", id(v))
}

/// What `comment`, `highlight` and `suggest` print.
fn created(verb: &str, v: &Value) -> String {
    format!("{verb} {}  {}\n", place(v), id(v))
}

/// `text`, as one quoted line, cut short after a few words.
fn quoted(text: &str) -> String {
    const LONGEST: usize = 60;
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= LONGEST {
        return format!("“{line}”");
    }
    let cut: String = line.chars().take(LONGEST - 1).collect();
    format!("“{}…”", cut.trim_end())
}

/// `list`'s output as text: each thread under its document, a line with
/// where it is, its status and its quote, then what was said, one line each.
pub fn threads(list: &Value) -> String {
    let items = list.as_array().map_or(&[][..], Vec::as_slice);
    if items.is_empty() {
        return format!("{}\n", paint(style::MUTED, "no annotations"));
    }
    let width = items.iter().filter_map(|t| t["line"].as_u64()).max().map_or(1, |n| n.to_string().len());
    let mut out = String::new();
    let mut path = None;
    for t in items {
        if path != Some(&t["path"]) {
            path = Some(&t["path"]);
            out.push_str(&format!("{}\n", paint(style::PATH, t["path"].as_str().unwrap_or("(no path)"))));
        }
        out.push_str(&thread(t, width));
    }
    out
}

/// One thread of [`threads`], its line number right-aligned to `width`.
fn thread(t: &Value, width: usize) -> String {
    let status = str(&t["status"]);
    let mut head = vec![paint(style::of(status), status)];
    match t["replacement"].as_str() {
        Some(replacement) => head.push(format!(
            "{} → {}",
            paint(style::PROBLEM, quoted(str(&t["quote"]))),
            paint(style::DONE, quoted(replacement))
        )),
        None => head.push(quoted(str(&t["quote"]))),
    }
    if let Some(label) = t["label"].as_str() {
        head.push(paint(style::PENDING, format!("[{label}]")));
    }
    let mut flags = Vec::new();
    if t["unread"] == json!(true) {
        flags.push(paint(style::ACCENT, "unread"));
    }
    // A relocated thread is still on its text, which moved: nothing to do.
    if t["resolution"] == "orphaned" {
        flags.push(paint(style::PROBLEM, "orphaned"));
    }
    if t["applicable"] == json!(false) {
        flags.push(paint(style::PROBLEM, "can't be applied"));
    }
    if t["approvals"].as_array().is_some_and(|a| a.iter().any(|a| a["stale"] == json!(false))) {
        flags.push(paint(style::DONE, "approved"));
    }
    if t["conflicts"].is_array() {
        flags.push(paint(style::PROBLEM, "conflicted"));
    }
    if t["deleted"] == json!(true) {
        flags.push(paint(style::PROBLEM, "deleted"));
    }
    if t["local"] == json!(true) {
        flags.push(paint(style::MUTED, "local"));
    }
    if let Some(reverts) = t["reverts"].as_str() {
        flags.push(paint(style::MUTED, format!("reverts {reverts}")));
    }
    if !flags.is_empty() {
        head.push(flags.join(&separator()));
    }
    head.push(paint(style::MUTED, str(&t["id"])));
    let line = t["line"].as_u64().map_or(String::new(), |n| n.to_string());
    let indent = " ".repeat(width + 4);
    let mut out = format!("  {line:>width$}  {}\n", head.join("  "));
    match t["body"].as_str().filter(|b| !b.is_empty()) {
        Some(body) => out.push_str(&said(&indent, "", &t["author"], body, &t["reactions"])),
        None => out.push_str(&format!(
            "{indent}{} {}{}\n",
            paint(style::AUTHOR, name(&t["author"])),
            paint(style::MUTED, if t["kind"] == "suggestion" { "suggested it" } else { "highlighted it" }),
            reactions(&t["reactions"])
        )),
    }
    for reply in t["replies"].as_array().into_iter().flatten() {
        let mark = if reply["unread"] == json!(true) { paint(style::ACCENT, "↳ ") } else { "↳ ".to_owned() };
        out.push_str(&said(&indent, &mark, &reply["author"], str(&reply["body"]), &reply["reactions"]));
    }
    if let Some(suggested) = t["suggested"].as_object() {
        out.push_str(&format!(
            "{indent}{}\n",
            paint(
                style::MUTED,
                format!(
                    "maybe now at line {}: {}",
                    suggested["line"].as_u64().unwrap_or_default(),
                    quoted(suggested["quote"].as_str().unwrap_or_default())
                )
            )
        ));
    }
    out
}

/// What `author` said in a thread, at `indent` after `mark`: the first line
/// of `body` after their name, and the rest under it.
fn said(indent: &str, mark: &str, author: &Value, body: &str, reacted: &Value) -> String {
    let mut lines = body.lines();
    let mut out = format!(
        "{indent}{mark}{}: {}{}\n",
        paint(style::AUTHOR, name(author)),
        lines.next().unwrap_or_default(),
        reactions(reacted)
    );
    for line in lines {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{indent}{}{line}\n", if mark.is_empty() { "  " } else { "    " }));
        }
    }
    out
}

/// The reactions `reactions` holds, as each emoji and how many gave it.
fn reactions(reactions: &Value) -> String {
    let Some(reactions) = reactions.as_object().filter(|r| !r.is_empty()) else { return String::new() };
    let counts: Vec<String> =
        reactions.iter().map(|(emoji, who)| format!("{emoji} {}", who.as_array().map_or(0, Vec::len))).collect();
    format!("  {}", paint(style::MUTED, counts.join(" ")))
}

/// What `accept` prints.
fn accepted(v: &Value) -> String {
    let mut out = format!("{} into {}  {}\n", paint(style::DONE, "Accepted"), place(v), id(v));
    if let Some(moved) = v["reanchored"].as_array() {
        out.push_str(&format!("Moved {} on the text it changed\n", plural(moved.len() as u64, "comment")));
    }
    out
}

/// What `revert` prints.
fn reverted(v: &Value) -> String {
    let reverts = paint(style::MUTED, str(&v["reverts"]));
    match v["status"].as_str() {
        Some("accepted") => format!("Reverted {reverts} in {}, by accepting {}\n", place(v), id(v)),
        _ => format!("Suggested reverting {reverts} at {}  {}\n", place(v), id(v)),
    }
}

/// What `react` prints.
fn reacted(v: &Value) -> String {
    let reaction = str(&v["reaction"]);
    match (v["removed"] == json!(true), v["changed"] == json!(true)) {
        (false, true) => format!("Reacted {reaction} to {}\n", id(v)),
        (true, true) => format!("Removed {reaction} from {}\n", id(v)),
        (false, false) => format!("{} already has your {reaction}\n", id(v)),
        (true, false) => format!("{} has no {reaction} of yours\n", id(v)),
    }
}

/// What `commit` prints.
fn committed(v: &Value) -> String {
    let files = v["files"].as_u64().unwrap_or(0);
    if files == 0 {
        return format!("{}\n", paint(style::MUTED, "nothing to commit"));
    }
    let message = str(&v["message"]);
    match v["commit"].as_str() {
        Some(commit) => format!(
            "Committed {} as {}\n{message}\n",
            plural(files, "file"),
            paint(style::ACCENT, commit.get(..7).unwrap_or(commit))
        ),
        None => format!("Would commit {}\n{message}\n", plural(files, "file")),
    }
}

/// What `config` prints: each setting, its value and where it comes from,
/// then the files they're read from.
fn config(v: &Value) -> String {
    // What `config`'s `from` values say, as text.
    let from = |source: &Value| match str(source) {
        "config" => "config.json".to_owned(),
        "default" => "by default".to_owned(),
        "git" => "git's identity".to_owned(),
        other => other.to_owned(),
    };
    let row = |label: &str, value: String, note: String| {
        let note = if note.is_empty() { note } else { format!("  {}", paint(style::MUTED, note)) };
        format!("{label:<11}{value}{note}\n")
    };

    let author = &v["author"];
    let mut out = match author["id"].as_str() {
        Some(id) => {
            let (id_from, name_from) = (&author["from"]["id"], &author["from"]["name"]);
            let note = match name_from {
                Value::Null => format!("from {}", from(id_from)),
                n if n == id_from => format!("from {}", from(id_from)),
                n => format!("id from {}, name from {}", from(id_from), from(n)),
            };
            let value = match author["name"].as_str() {
                Some(name) => format!("{} {}", paint(style::AUTHOR, name), paint(style::MUTED, format!("<{id}>"))),
                None => paint(style::AUTHOR, id),
            };
            row("author", value, note)
        }
        None => row("author", paint(style::PROBLEM, "none"), "config.json is invalid, so write commands refuse".into()),
    };
    out += &row("output", str(&v["output"]["value"]).to_owned(), from(&v["output"]["from"]));

    let ws = &v["workspace"];
    if ws.is_null() {
        out += &row("workspace", paint(style::MUTED, "none here"), String::new());
    } else {
        out += &row("workspace", paint(style::PATH, str(&ws["root"])), String::new());
        out += &row("format", ws["format"].to_string(), "annox.json".into());
        out += &match ws["sync"]["url"].as_str() {
            None => row("sync", paint(style::MUTED, "off"), "no sync.url in annox.json".into()),
            Some(url) => {
                let (state, note) = match str(&ws["sync"]["credential"]) {
                    "token" => (style::DONE, "with a token from credentials.json"),
                    "none" => (style::DONE, "without a token, as credentials.json says"),
                    _ => (style::PROBLEM, "not syncing: no entry for it in credentials.json"),
                };
                row("sync", paint(state, url), note.into())
            }
        };
    }

    out += "\n";
    for file in v["files"].as_array().into_iter().flatten() {
        let name = match str(&file["name"]) {
            "config" => "config.json",
            "credentials" => "credentials.json",
            _ => "annox.json",
        };
        let path = file["path"].as_str().map_or_else(|| paint(style::MUTED, "no location"), |p| paint(style::PATH, p));
        let state = match str(&file["state"]) {
            "ok" => String::new(),
            "missing" => format!("  {}", paint(style::MUTED, "missing")),
            _ => format!("  {}", paint(style::PROBLEM, format!("invalid: {}", str(&file["error"])))),
        };
        out += &format!("{name:<18}{path}{state}\n");
    }
    out
}

/// `report`'s entries in display order, each with how deeply it's nested.
fn report_entries(report: &Value) -> Vec<(usize, &Value)> {
    fn walk<'a>(entries: &'a Value, depth: usize, out: &mut Vec<(usize, &'a Value)>) {
        for entry in entries.as_array().into_iter().flatten() {
            out.push((depth, entry));
            for key in ["documents", "authors"] {
                if let Some(nested) = entry.get(key) {
                    walk(nested, depth + 1, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for key in ["documents", "authors"] {
        if let Some(entries) = report.get(key) {
            walk(entries, 0, &mut out);
        }
    }
    out
}

/// A `report` entry's label, plain and styled: a document's path, or an
/// author's name and id.
fn report_label(entry: &Value) -> (String, String) {
    if let Some(path) = entry["path"].as_str() {
        return (path.to_owned(), paint(style::PATH, path));
    }
    let id = entry["author"]["id"].as_str().unwrap_or("unknown");
    match entry["author"]["name"].as_str() {
        Some(name) => (
            format!("{name} <{id}>"),
            format!("{} {}", paint(style::AUTHOR, name), paint(style::MUTED, format!("<{id}>"))),
        ),
        None => (id.to_owned(), paint(style::AUTHOR, id)),
    }
}

/// `n` and `what`, made plural unless `n` is 1.
fn plural(n: u64, what: &str) -> String {
    match (n, what.strip_suffix('y')) {
        (1, _) => format!("1 {what}"),
        (_, Some(stem)) => format!("{n} {stem}ies"),
        _ => format!("{n} {what}s"),
    }
}

/// The summary line a report ends with, if any: the files `annox commit`
/// would commit.
fn report_footer(report: &Value) -> String {
    match report["uncommitted"].as_u64().unwrap_or(0) {
        0 => String::new(),
        n => format!(
            "{}; see `{}`\n",
            paint(style::PENDING, format!("{} to commit", plural(n, "file"))),
            paint(style::PATH, "annox commit --dry-run")
        ),
    }
}

/// `report`'s output as text, one line per entry, nested ones indented,
/// styled (see [`style`]).
pub fn report(report: &Value) -> String {
    let entries = report_entries(report);
    if entries.is_empty() {
        return format!("{}\n{}", paint(style::MUTED, "no annotations"), report_footer(report));
    }
    let count = |v: &Value| v.as_u64().unwrap_or(0);
    let labels: Vec<(String, String)> = entries.iter().map(|(_, e)| report_label(e)).collect();
    let width = entries
        .iter()
        .zip(&labels)
        .map(|((depth, _), (plain, _))| 2 * depth + plain.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for ((depth, entry), (plain, label)) in entries.iter().zip(labels) {
        let mut parts = Vec::new();
        if entry["missing"] == json!(true) {
            parts.push(paint(style::PROBLEM, "file missing"));
        }
        for (n, what) in [
            (count(&entry["suggestions"]["open"]), "open suggestion"),
            (count(&entry["comments"]["open"]), "open comment"),
        ] {
            if n > 0 {
                parts.push(paint(style::OPEN, plural(n, what)));
            }
        }
        if count(&entry["approved"]) > 0 {
            parts.push(paint(style::DONE, format!("{} approved", count(&entry["approved"]))));
        }
        if count(&entry["replies"]) > 0 {
            parts.push(plural(count(&entry["replies"]), "reply"));
        }
        for what in ["orphaned", "stale", "conflicted"] {
            if count(&entry[what]) > 0 {
                parts.push(paint(style::PROBLEM, format!("{} {what}", count(&entry[what]))));
            }
        }
        let closed = count(&entry["comments"]["resolved"])
            + ["accepted", "rejected", "withdrawn"].iter().map(|s| count(&entry["suggestions"][s])).sum::<u64>();
        if closed > 0 {
            parts.push(paint(style::MUTED, format!("{closed} closed")));
        }
        if count(&entry["uncommitted"]) > 0 {
            parts.push(paint(style::PENDING, plural(count(&entry["uncommitted"]), "uncommitted change")));
        }
        let pad = " ".repeat(width - 2 * depth - plain.chars().count());
        out.push_str(&format!("{}{label}{pad}  {}\n", "  ".repeat(*depth), parts.join(&separator())));
    }
    out + &report_footer(report)
}

/// What goes between the counts of a report line.
fn separator() -> String {
    paint(style::MUTED, " · ")
}

/// `report --verbose`'s output as text: each entry's counts broken down by
/// status, one indented line per kind, leaving out zeros.
pub fn report_verbose(report: &Value) -> String {
    let entries = report_entries(report);
    if entries.is_empty() {
        return self::report(report);
    }
    let count = |v: &Value| v.as_u64().unwrap_or(0);
    let counts = |of: &Value, names: &[&str]| -> Vec<String> {
        names
            .iter()
            .filter(|n| count(&of[n]) > 0)
            .map(|n| paint(style::of(n), format!("{} {n}", count(&of[n]))))
            .collect()
    };
    let mut out = String::new();
    for (depth, entry) in entries {
        let indent = "  ".repeat(depth);
        out.push_str(&format!("{indent}{}", report_label(entry).1));
        if entry["missing"] == json!(true) {
            out.push_str(&format!("  {}", paint(style::PROBLEM, "(file missing)")));
        }
        out.push('\n');
        let uncommitted = match count(&entry["uncommitted"]) {
            0 => vec![],
            1 => vec![paint(style::PENDING, "1 change")],
            n => vec![paint(style::PENDING, format!("{n} changes"))],
        };
        let mut suggestions = counts(&entry["suggestions"], &["open", "accepted", "rejected", "withdrawn"]);
        if count(&entry["approved"]) > 0 {
            suggestions.insert(1, paint(style::DONE, format!("{} approved", count(&entry["approved"]))));
        }
        let rows = [
            ("suggestions", suggestions),
            ("comments", counts(&entry["comments"], &["open", "resolved"])),
            ("replies", [count(&entry["replies"])].into_iter().filter(|n| *n > 0).map(|n| n.to_string()).collect()),
            ("needs work", counts(entry, &["orphaned", "stale", "conflicted"])),
            ("uncommitted", uncommitted),
        ];
        for (label, parts) in rows.iter().filter(|(_, parts)| !parts.is_empty()) {
            out.push_str(&format!(
                "{indent}  {}  {}\n",
                paint(style::MUTED, format!("{label:11}")),
                parts.join(&separator())
            ));
        }
    }
    out + &report_footer(report)
}

/// The text form of `annox history`: one line per event. Events that change a
/// reply say so.
pub fn history(events: &Value) -> String {
    let events = events.as_array().map_or(&[][..], Vec::as_slice);
    let replies: BTreeSet<&str> = events
        .iter()
        .filter(|e| e["type"] == "create" && e["kind"] == "reply")
        .filter_map(|e| e["id"].as_str())
        .collect();
    let name = |e: &Value| e["author"]["name"].as_str().or(e["author"]["id"].as_str()).unwrap_or("unknown").to_owned();
    let width = events.iter().map(|e| name(e).chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for e in events {
        let detail = e["body"].as_str().or(e["edit"]["replacement"].as_str()).and_then(|d| d.lines().next());
        let detail = detail.map_or(String::new(), |d| format!(": {d}"));
        let reply = e["type"] != "create" && e["annotation"].as_str().is_some_and(|a| replies.contains(a));
        out.push_str(&format!(
            "{}  {}  {}{detail}\n",
            paint(style::MUTED, e["time"].as_str().unwrap_or_default()),
            paint(style::AUTHOR, format!("{:width$}", name(e))),
            painted_action(e, reply),
        ));
    }
    out
}

/// The text form of `annox log`: one line per event, with its document.
pub fn log(events: &Value) -> String {
    let events = events.as_array().map_or(&[][..], Vec::as_slice);
    let name = |e: &Value| e["author"]["name"].as_str().or(e["author"]["id"].as_str()).unwrap_or("unknown").to_owned();
    let path = |e: &Value| e["path"].as_str().unwrap_or("(no path)").to_owned();
    let width = events.iter().map(|e| name(e).chars().count()).max().unwrap_or(0);
    let path_width = events.iter().map(|e| path(e).chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for e in events {
        let detail = e["body"].as_str().or(e["edit"]["replacement"].as_str()).and_then(|d| d.lines().next());
        let detail = detail.map_or(String::new(), |d| format!(": {d}"));
        let reply = e["type"] != "create" && e["annotation"] != e["thread"];
        out.push_str(&format!(
            "{}  {}  {}  {}{detail}\n",
            paint(style::MUTED, e["time"].as_str().unwrap_or_default()),
            paint(style::PATH, format!("{:path_width$}", path(e))),
            paint(style::AUTHOR, format!("{:width$}", name(e))),
            painted_action(e, reply),
        ));
    }
    out
}

/// [`action`], styled by what it did, and saying so if it changed a reply.
fn painted_action(event: &Value, reply: bool) -> String {
    let action = action(event);
    let verb = action.split(' ').next().unwrap_or_default();
    paint(style::of(verb), format!("{action}{}", if reply { " reply" } else { "" }))
}

/// What an event did, as a past-tense verb for history views.
fn action(event: &Value) -> String {
    let status = event["status"].as_str().unwrap_or_default();
    match (event["type"].as_str().unwrap_or_default(), event["kind"].as_str().unwrap_or_default()) {
        ("create", "comment") => "commented".into(),
        ("create", "suggestion") => "suggested".into(),
        ("create", "reply") => "replied".into(),
        ("edit", _) => "edited".into(),
        ("status", _) => match status {
            "open" => "reopened".into(),
            "withdrawn" => "withdrew".into(),
            _ => status.into(),
        },
        ("reanchor", _) => "re-anchored (automatic)".into(),
        ("retarget", _) => "retargeted".into(),
        ("delete", _) => "deleted".into(),
        ("restore", _) => "restored".into(),
        ("react", _) => {
            let reaction = event["reaction"].as_str().unwrap_or_default();
            if event["removed"] == json!(true) {
                format!("removed {reaction}")
            } else {
                format!("reacted {reaction}")
            }
        }
        (kind, _) => kind.into(),
    }
}
