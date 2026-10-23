//! Threads as Markdown, for hover (§6.5.2).

use annox_core::anchor::State;
use annox_core::replay;
use serde_json::{json, Value};

use crate::Item;

fn author_line(state: &Value) -> String {
    let name = |a: &Value| a["name"].as_str().or(a["id"].as_str()).unwrap_or("unknown").to_owned();
    let mut line = format!("**{}** · {}", name(&state["author"]), state["created"].as_str().unwrap_or_default());
    // Edits are marked, with the editor named if it isn't the author (§2.4).
    let editor = &state["editedBy"]["author"];
    if editor.is_object() {
        if editor["id"] == state["author"]["id"] {
            line.push_str(" · edited");
        } else {
            line.push_str(&format!(" · edited by {}", name(editor)));
        }
    }
    line
}

/// The names of the people in `entries` (each with an `author`), as in "Ada,
/// Bob and Cy".
fn names<'a>(entries: impl IntoIterator<Item = &'a Value>) -> String {
    let names: Vec<&str> = entries
        .into_iter()
        .map(|e| e["author"]["name"].as_str().or(e["author"]["id"].as_str()).unwrap_or("unknown"))
        .collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => (*last).to_owned(),
        None => String::new(),
    }
}

/// A message's reactions on one line, such as "👀 Bob · 🎉 Ada and Cy",
/// leaving out `skip` (§2.5.7).
fn reactions_line(state: &Value, skip: Option<&str>) -> Option<String> {
    let parts: Vec<String> = state["reactions"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(r, by)| Some(r.as_str()) != skip && by.as_array().is_some_and(|b| !b.is_empty()))
        .map(|(r, by)| format!("{r} {}", names(by.as_array().into_iter().flatten())))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Who approved a suggestion (§4.4.2): current approvals, then those of an
/// earlier version.
fn approval_lines(state: &Value) -> Vec<String> {
    let approvals = state["approvals"].as_array().map_or(&[][..], Vec::as_slice);
    let (stale, current): (Vec<&Value>, Vec<&Value>) = approvals.iter().partition(|a| a["stale"] == json!(true));
    let mut lines = Vec::new();
    if !current.is_empty() {
        lines.push(format!("👍 _approved by {}_", names(current)));
    }
    if !stale.is_empty() {
        lines.push(format!("_{} approved an earlier version_", names(stale)));
    }
    lines
}

/// A change as a fenced `diff` block, so editors color the old text red and
/// the new text green.
fn diff_block(old: &str, new: &str) -> String {
    let longest = [old, new].iter().flat_map(|t| t.split(|c| c != '`')).map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let mut out = format!("{fence}diff\n");
    for (sign, text) in [('-', old), ('+', new)] {
        if !text.is_empty() {
            for line in text.split('\n') {
                out.push_str(&format!("{sign} {line}\n"));
            }
        }
    }
    out + &fence
}

/// `md` with each lone `~` escaped, outside code: GFM reads a pair of them as
/// strikethrough, but authors mostly mean "approximately". `~~` is kept.
fn escape_tildes(md: &str) -> String {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in md.split('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            out.push(line.to_string());
            continue;
        }
        if fenced {
            out.push(line.to_string());
            continue;
        }
        let b = line.as_bytes();
        let (mut escaped, mut start, mut code, mut j) = (String::new(), 0, 0, 0);
        while j < b.len() {
            if b[j] == b'`' {
                let n = b[j..].iter().take_while(|&&c| c == b'`').count();
                code = if code == 0 {
                    n
                } else if code == n {
                    0
                } else {
                    code
                };
                j += n;
                continue;
            }
            let prev = j.checked_sub(1).map(|i| b[i]);
            if b[j] == b'~' && code == 0 && !matches!(prev, Some(b'~' | b'\\')) && b.get(j + 1) != Some(&b'~') {
                escaped.push_str(&line[start..j]);
                escaped.push('\\');
                start = j;
            }
            j += 1;
        }
        escaped.push_str(&line[start..]);
        out.push(escaped);
    }
    out.join("\n")
}

/// A thread as Markdown (§6.5.2): the proposed change first for suggestions,
/// then the root and its replies.
pub(crate) fn thread_markdown(item: &Item, replies: Option<&Vec<Value>>) -> String {
    let mut parts = Vec::new();
    if let (None, Some(s)) = (item.resolution.range, item.suggested) {
        let mut note =
            format!("⚠ **Orphaned {}, may belong here** ({:.0}% of its words).", item.kind(), s.score * 100.0);
        if item.kind() == "comment" {
            let quote: Vec<String> = item.target.selectors.quote.exact.lines().map(|l| format!("> {l}")).collect();
            note.push_str(&format!(" It was on:\n\n{}", quote.join("\n")));
        }
        parts.push(note);
    }
    if item.kind() == "suggestion" {
        let r = item.state["edit"]["replacement"].as_str().unwrap_or_default();
        let stale = if item.applicable() { "" } else { " (stale)" };
        let mut change = format!("**Suggestion{stale}:**\n{}", diff_block(&item.target.selectors.quote.exact, r));
        for line in approval_lines(&item.state) {
            change.push_str(&format!("\n\n{line}"));
        }
        parts.push(change);
    }
    if item.conflicted() {
        parts.push("⚠ **This annotation has conflicting changes.**".into());
    }
    let mut header = author_line(&item.state);
    if item.resolution.state == State::Relocated {
        header.push_str(" · relocated");
    }
    let mut root = match item.state["body"].as_str() {
        Some(body) => format!("{header}\n\n{}", escape_tildes(body)),
        None => header,
    };
    // A suggestion's 👍 is shown as its approval, above.
    let skip = (item.kind() == "suggestion").then_some(replay::APPROVAL);
    if let Some(line) = reactions_line(&item.state, skip) {
        root.push_str(&format!("\n\n{line}"));
    }
    parts.push(root);
    for reply in replies.into_iter().flatten().filter(|r| r["deleted"] == json!(false)) {
        // A thematic break renders as a full-width rule between messages.
        let body = escape_tildes(reply["body"].as_str().unwrap_or_default());
        let mut message = format!("---\n\n{}\n\n{body}", author_line(reply));
        if let Some(line) = reactions_line(reply, None) {
            message.push_str(&format!("\n\n{line}"));
        }
        parts.push(message);
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::escape_tildes;

    #[test]
    fn lone_tildes_are_escaped_outside_code() {
        let body = "less than ~0.1. With ~all, ~~struck~~, `a ~b~`, \\~ kept\n```\nx ~ y\n```";
        let want = "less than \\~0.1. With \\~all, ~~struck~~, `a ~b~`, \\~ kept\n```\nx ~ y\n```";
        assert_eq!(escape_tildes(body), want);
    }
}
