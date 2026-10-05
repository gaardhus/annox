//! Applicability and applying suggestions (§4).

use serde_json::Value;

use crate::anchor::{self, nearest, Anchor, Resolution, State};
use crate::text::Text;

/// Whether a resolution makes a suggestion applicable (§4.2): exact, or
/// relocated by step 2, 3, or 5.
pub fn is_applicable(resolution: &Resolution) -> bool {
    resolution.state != State::Orphaned && !matches!(resolution.step, 4 | 6)
}

/// A suggestion applied to a document (§4.3 step 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The range that was replaced in the original document.
    pub start: usize,
    pub end: usize,
    /// The resulting document.
    pub text: Text,
    pub resolution: Resolution,
}

/// Applies a suggestion to `doc`, or returns `Err` with the resolution if
/// the suggestion is stale (§4.3 steps 1–2).
pub fn apply(doc: &Text, target: &Anchor, replacement: &str) -> Result<Applied, Resolution> {
    let resolution = anchor::resolve(doc, target);
    match resolution.range {
        Some((start, end)) if is_applicable(&resolution) => {
            Ok(Applied { start, end, text: doc.splice(start, end, replacement), resolution })
        }
        _ => Err(resolution),
    }
}

/// New anchors for the open comments whose text an applied edit changes
/// (§4.3.5). `edits` are `(start, end, replacement length)` in `before`,
/// sorted and disjoint; `after` is `before` with them applied. Each
/// comment's range is mapped through the edits and anchored in `after`.
pub fn carry_comments<'a>(
    before: &Text,
    after: &Text,
    path: &str,
    edits: &[(usize, usize, usize)],
    states: impl IntoIterator<Item = (&'a String, &'a Value)>,
) -> Vec<(String, Anchor)> {
    let mut out = Vec::new();
    for (id, state) in states {
        if state["kind"] != "comment" || state["status"] != "open" || state["deleted"] != Value::Bool(false) {
            continue;
        }
        let Ok(target) = serde_json::from_value::<Anchor>(state["target"].clone()) else { continue };
        let Some((s, e)) = anchor::resolve(before, &target).range else { continue };
        if !edits.iter().any(|&(c, d, _)| touches((s, e), (c, d))) {
            continue;
        }
        let start = map_offset(s, edits, false);
        let end = if s == e { start } else { map_offset(e, edits, true) };
        out.push((id.clone(), anchor::create(after, start, end, path)));
    }
    out
}

/// Whether the edit replacing `[c, d)` changes the text of the range
/// `[s, e)`, or, for a point, replaces the text around it.
fn touches((s, e): (usize, usize), (c, d): (usize, usize)) -> bool {
    if s == e {
        c < s && s < d
    } else if c == d {
        s < c && c < e
    } else {
        s < d && c < e
    }
}

/// Maps an offset through `edits`. An offset inside a replaced range moves
/// to the start of its replacement, or to the end if `end` is set. Text
/// inserted at a range's start goes before it; at its end, after it.
fn map_offset(pos: usize, edits: &[(usize, usize, usize)], end: bool) -> usize {
    let mut shift = 0isize;
    for &(c, d, r) in edits {
        let delta = r as isize - (d - c) as isize;
        if c == d {
            if c < pos || (c == pos && !end) {
                shift += delta;
            } else {
                break;
            }
        } else if pos <= c {
            break;
        } else if pos >= d {
            shift += delta;
        } else {
            return (c as isize + shift) as usize + if end { r } else { 0 };
        }
    }
    (pos as isize + shift) as usize
}

/// Finds the text a suggestion produces once applied (§4.3.3), used to
/// detect lost acceptances and to revert.
pub fn applied_text_search(doc: &Text, target: &Anchor, replacement: &str) -> Option<(usize, usize)> {
    let quote = &target.selectors.quote;
    let pattern = format!("{}{replacement}{}", quote.prefix, quote.suffix);
    let prefix_len = quote.prefix.chars().count();
    let starts: Vec<usize> = doc.haystack().find_all(&pattern).into_iter().map(|i| i + prefix_len).collect();
    nearest(&starts, target.selectors.position.start).map(|c| (c, c + replacement.chars().count()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn comment(text: &Text, start: usize, end: usize) -> (String, Value) {
        let target = anchor::create(text, start, end, "doc.txt");
        ("c".to_owned(), json!({ "kind": "comment", "status": "open", "deleted": false, "target": target }))
    }

    /// Accepts "we prove that" → "we show that" and returns the range each
    /// comment is carried to, if any.
    fn carry(start: usize, end: usize) -> Option<(usize, usize)> {
        let before = Text::from_raw("In Section 3, we prove that the bound is tight.\n");
        let after = before.splice(14, 27, "we show that");
        let (id, state) = comment(&before, start, end);
        let carried = carry_comments(&before, &after, "doc.txt", &[(14, 27, 12)], [(&id, &state)]);
        carried.first().map(|(_, a)| {
            assert_eq!(anchor::resolve(&after, a).step, 0);
            (a.selectors.position.start, a.selectors.position.end)
        })
    }

    #[test]
    fn comments_follow_the_text_that_replaced_theirs() {
        // Same range as the suggestion.
        assert_eq!(carry(14, 27), Some((14, 26)));
        // Contains it: the end shifts by the change in length.
        assert_eq!(carry(0, 39), Some((0, 38)));
        // Overlaps its start: the end moves to the end of the replacement.
        assert_eq!(carry(3, 20), Some((3, 26)));
        // Inside it: the replacement as a whole.
        assert_eq!(carry(17, 22), Some((14, 26)));
        // A point inside it moves to the start of the replacement.
        assert_eq!(carry(20, 20), Some((14, 14)));
    }

    #[test]
    fn comments_beside_the_edit_are_left_to_resolution() {
        assert_eq!(carry(0, 14), None);
        assert_eq!(carry(27, 37), None);
        assert_eq!(carry(14, 14), None);
    }

    #[test]
    fn insertions_inside_a_comment_widen_it() {
        let before = Text::from_raw("one two three\n");
        let after = before.splice(4, 4, "and ");
        let (id, state) = comment(&before, 0, 7);
        let carried = carry_comments(&before, &after, "doc.txt", &[(4, 4, 4)], [(&id, &state)]);
        assert_eq!(carried[0].1.selectors.quote.exact, "one and two");
    }
}
