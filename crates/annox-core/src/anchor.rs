//! Creating and resolving anchors (§3.5–§3.8).

use serde::{Deserialize, Serialize};

use crate::text::Text;

/// Default length of the stored prefix and suffix (§3.6).
pub const CONTEXT: usize = 32;

/// An anchor: the `target` of a comment or suggestion (§3.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub path: String,
    pub version: String,
    pub selectors: Selectors,
}

/// Selectors of an anchor. Unknown selector types are ignored (§3.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selectors {
    pub position: PositionSelector,
    pub quote: QuoteSelector,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionSelector {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteSelector {
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
}

/// Creates an anchor for `[start, end)` in `text` (§3.6).
pub fn create(text: &Text, start: usize, end: usize, path: &str) -> Anchor {
    Anchor {
        path: path.to_owned(),
        version: text.version.clone(),
        selectors: Selectors {
            position: PositionSelector { start, end },
            quote: QuoteSelector {
                exact: text.slice(start, end),
                prefix: text.slice(start.saturating_sub(CONTEXT), start),
                suffix: text.slice(end, (end + CONTEXT).min(text.len())),
            },
        },
    }
}

/// The state of a resolved anchor (§3.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Exact,
    Relocated,
    Orphaned,
}

/// The result of resolving an anchor: its state, the range (absent when
/// orphaned), and the step of §3.7.2 that produced it (7 when orphaned).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub state: State,
    pub range: Option<(usize, usize)>,
    pub step: u8,
}

impl Resolution {
    fn exact(start: usize, end: usize, step: u8) -> Resolution {
        Resolution { state: State::Exact, range: Some((start, end)), step }
    }

    fn relocated(start: usize, end: usize, step: u8) -> Resolution {
        Resolution { state: State::Relocated, range: Some((start, end)), step }
    }

    const ORPHANED: Resolution = Resolution { state: State::Orphaned, range: None, step: 7 };
}

/// Resolves `anchor` against the current document `doc` (§3.7.2).
pub fn resolve(doc: &Text, anchor: &Anchor) -> Resolution {
    let d = &doc.chars;
    let s = anchor.selectors.position.start;
    let e = anchor.selectors.position.end;
    let q: Vec<char> = anchor.selectors.quote.exact.chars().collect();
    let p: Vec<char> = anchor.selectors.quote.prefix.chars().collect();
    let x: Vec<char> = anchor.selectors.quote.suffix.chars().collect();

    let consistent = s <= e && e <= d.len() && d[s..e] == q[..];

    // Step 0: version match.
    if consistent && doc.version == anchor.version {
        return Resolution::exact(s, e, 0);
    }

    // Step 1: position check.
    if consistent && s >= p.len() && d[s - p.len()..s] == p[..] && e + x.len() <= d.len() && d[e..e + x.len()] == x[..]
    {
        return Resolution::exact(s, e, 1);
    }

    // Step 2: context search.
    let quote = &anchor.selectors.quote;
    let context = format!("{}{}{}", quote.prefix, quote.exact, quote.suffix);
    let starts: Vec<usize> = doc.haystack().find_all(&context).into_iter().map(|i| i + p.len()).collect();
    if let Some(c) = nearest(&starts, s) {
        return Resolution::relocated(c, c + q.len(), 2);
    }

    let as_string = |chars: &[char]| chars.iter().collect::<String>();
    if !q.is_empty() {
        // Step 3: quote search.
        if let Some(c) = select_by_context(d, &doc.haystack().find_all(&quote.exact), q.len(), &p, &x) {
            return Resolution::relocated(c, c + q.len(), 3);
        }
        // Step 4, quote variant: whitespace-insensitive quote search.
        let (wq, _) = collapse(&q);
        if wq.iter().any(|&c| c != ' ') {
            let w = doc.collapsed();
            let (wp, _) = collapse(&p);
            let (wx, _) = collapse(&x);
            let occ = w.haystack.find_all(&as_string(&wq));
            if let Some(i) = select_by_context(&w.chars, &occ, wq.len(), &wp, &wx) {
                let j = i + wq.len();
                return Resolution::relocated(w.spans[i].0, w.spans[j - 1].1, 4);
            }
        }
        // Step 6: bracket search.
        if let Some((a, b)) = bracket(doc, &quote.prefix, &quote.suffix, p.len(), q.len()) {
            return Resolution::relocated(a, b, 6);
        }
    } else {
        // Step 4, point variant: whitespace-insensitive context search.
        let (wp, _) = collapse(&p);
        let (wx, _) = collapse(&x);
        let merged = wp.last() == Some(&' ') && wx.first() == Some(&' ');
        let pattern: Vec<char> = if merged { [&wp[..], &wx[1..]].concat() } else { [&wp[..], &wx[..]].concat() };
        if pattern.iter().any(|&c| c != ' ') {
            let w = doc.collapsed();
            let points: Vec<usize> = w
                .haystack
                .find_all(&as_string(&pattern))
                .into_iter()
                .map(|i| {
                    if merged {
                        w.spans[i + wp.len() - 1].0
                    } else {
                        let k = i + wp.len();
                        if k < w.chars.len() {
                            w.spans[k].0
                        } else {
                            d.len()
                        }
                    }
                })
                .collect();
            if let Some(c) = nearest(&points, s) {
                return Resolution::relocated(c, c, 4);
            }
        }
        // Step 5: partial context search for points.
        if let Some(c) = partial_context(doc, &p, &x) {
            return Resolution::relocated(c, c, 5);
        }
    }

    Resolution::ORPHANED
}

/// A suggested location for an orphaned anchor (§3.7.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Suggested {
    pub range: (usize, usize),
    /// The share of the quote's words found in the range, in order, not
    /// counting punctuation.
    pub score: f64,
}

/// Fewest quote words a suggested location must contain.
const MIN_WORDS: usize = 4;

/// Largest alignment, in quote words times document words, worth computing.
const MAX_CELLS: usize = 20_000_000;

/// Proposes where an orphaned anchor's text most likely went (§3.7.4), when
/// it was reworded rather than removed. This is a heuristic, not part of
/// resolution: a suggested location is only offered to the user.
///
/// It finds the passage of `doc` matching the most of the quote's words in
/// order, allowing words to be added, removed, or changed (a local
/// alignment), and suggests it if it contains at least 40% of the quote's
/// words, and at least [`MIN_WORDS`]. A list condensed to its essentials
/// keeps about half its words, while an unrelated passage sharing a name or
/// two rarely gets past a third. Of equally good passages, the one
/// nearest the anchor's stored position wins.
pub fn suggest(doc: &Text, anchor: &Anchor) -> Option<Suggested> {
    let quote: Vec<char> = anchor.selectors.quote.exact.chars().collect();
    let q = words(&quote);
    let d = words(&doc.chars);
    // Punctuation helps align, but only words count towards the score.
    let is_word = |chars: &[char], (a, _): (usize, usize)| chars[a].is_alphanumeric() || chars[a] == '_';
    let q_words = q.iter().filter(|&&r| is_word(&quote, r)).count();
    if q_words < MIN_WORDS || q.len().saturating_mul(d.len()) > MAX_CELLS {
        return None;
    }
    // Words compare ignoring case, as rewording often moves a word to or
    // from the start of a sentence.
    let lower = |chars: &[char]| chars.iter().flat_map(|c| c.to_lowercase()).collect::<String>();
    let mut ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let qi: Vec<u32> = q
        .iter()
        .map(|&(a, b)| {
            let next = ids.len() as u32;
            *ids.entry(lower(&quote[a..b])).or_insert(next)
        })
        .collect();
    let di: Vec<u32> = d.iter().map(|&(a, b)| ids.get(&lower(&doc.chars[a..b])).copied().unwrap_or(u32::MAX)).collect();

    // Smith–Waterman over words: +3 for a match, -2 for a changed word, -1
    // for an added or removed one. Rewording adds and removes words more than
    // it keeps them, so gaps are cheap: a passage stays together across a
    // rewritten stretch as long as it keeps matching words on both sides.
    // Each cell keeps where its passage starts and how many words matched.
    #[derive(Clone, Copy, Default)]
    struct Cell {
        score: i32,
        start: usize,
        hits: usize,
    }
    let mut prev = vec![Cell::default(); q.len() + 1];
    let mut best: Vec<Cell> = Vec::new();
    let mut best_ends: Vec<usize> = Vec::new();
    let mut best_score = 0;
    for (i, &w) in di.iter().enumerate() {
        let mut cur = vec![Cell::default(); q.len() + 1];
        for j in 1..=q.len() {
            let hit = w == qi[j - 1];
            let counts = hit && is_word(&quote, q[j - 1]);
            let from = prev[j - 1];
            let diag = if from.score > 0 {
                Cell {
                    score: from.score + if hit { 3 } else { -2 },
                    start: from.start,
                    hits: from.hits + counts as usize,
                }
            } else {
                Cell { score: if hit { 3 } else { 0 }, start: i, hits: counts as usize }
            };
            let up = Cell { score: prev[j].score - 1, ..prev[j] };
            let left = Cell { score: cur[j - 1].score - 1, ..cur[j - 1] };
            let cell = [diag, up, left].into_iter().max_by_key(|c| (c.score, c.hits)).unwrap_or_default();
            cur[j] = if cell.score > 0 { cell } else { Cell::default() };
            if cur[j].score > best_score {
                best_score = cur[j].score;
                best.clear();
                best_ends.clear();
            }
            if cur[j].score == best_score && best_score > 0 {
                best.push(cur[j]);
                best_ends.push(i);
            }
        }
        prev = cur;
    }

    let s = anchor.selectors.position.start;
    let (cell, end) = best
        .into_iter()
        .zip(best_ends)
        .filter(|(c, _)| c.hits >= MIN_WORDS && 5 * c.hits >= 2 * q_words)
        .min_by_key(|(c, _)| (d[c.start].0.abs_diff(s), d[c.start].0))?;
    Some(Suggested { range: (d[cell.start].0, d[end].1), score: cell.hits as f64 / q_words as f64 })
}

/// The words of `t` as code-point ranges: runs of letters, digits, and `_`,
/// and every other non-whitespace code point on its own. CJK ideographs and
/// kana are words on their own too, since those scripts don't use spaces.
fn words(t: &[char]) -> Vec<(usize, usize)> {
    let is_word = |c: char| {
        (c.is_alphanumeric() || c == '_')
            && !matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'
                | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{2fa1f}')
    };
    let mut out = Vec::new();
    let mut k = 0;
    while k < t.len() {
        if t[k].is_whitespace() {
            k += 1;
            continue;
        }
        let start = k;
        k += 1;
        if is_word(t[start]) {
            while k < t.len() && is_word(t[k]) {
                k += 1;
            }
        }
        out.push((start, k));
    }
    out
}

/// Step 5 (§3.7.2): the single offset with the best context score, if that
/// score is at least half of `len(p) + len(x)`.
///
/// An offset scoring at least `need` has at least `half = ceil(need / 2)` of
/// the prefix before it or of the suffix after it, so only offsets next to an
/// occurrence of `p`'s last `half` or `x`'s first `half` code points can win.
fn partial_context(doc: &Text, p: &[char], x: &[char]) -> Option<usize> {
    let need = (p.len() + x.len()).div_ceil(2);
    if need == 0 {
        return None;
    }
    let half = need.div_ceil(2);
    let as_string = |chars: &[char]| chars.iter().collect::<String>();
    let mut candidates = Vec::new();
    if p.len() >= half {
        let tail = as_string(&p[p.len() - half..]);
        candidates.extend(doc.haystack().find_all(&tail).into_iter().map(|i| i + half));
    }
    if x.len() >= half {
        candidates.extend(doc.haystack().find_all(&as_string(&x[..half])));
    }
    candidates.sort_unstable();
    candidates.dedup();
    let scored: Vec<(usize, usize)> =
        candidates.into_iter().map(|c| (context_score(&doc.chars, c, c, p, x), c)).collect();
    let best = scored.iter().map(|&(sc, _)| sc).max()?;
    let mut winners = scored.iter().filter(|&&(sc, _)| sc == best);
    match (winners.next(), winners.next()) {
        (Some(&(_, c)), None) if best >= need => Some(c),
        _ => None,
    }
}

/// Step 6 (§3.7.2): the text between the stored prefix and suffix, when one
/// of them occurs once, the other follows or precedes it, and the text
/// between them is between half and twice the quote's length.
fn bracket(doc: &Text, p: &str, x: &str, plen: usize, qlen: usize) -> Option<(usize, usize)> {
    if p.is_empty() || x.is_empty() {
        return None;
    }
    let ends: Vec<usize> = doc.haystack().find_all(p).into_iter().map(|i| i + plen).collect();
    let starts = doc.haystack().find_all(x);
    let (a, b) = match (&ends[..], &starts[..]) {
        ([a], _) => (*a, starts.iter().copied().find(|&b| b >= *a)?),
        (_, [b]) => (ends.iter().copied().rev().find(|&a| a <= *b)?, *b),
        _ => return None,
    };
    let gap = b - a;
    (2 * gap >= qlen && gap <= 2 * qlen).then_some((a, b))
}

/// The candidate closest to `s`, with ties going to the lower offset (§3.7.1).
pub(crate) fn nearest(candidates: &[usize], s: usize) -> Option<usize> {
    candidates.iter().copied().min_by_key(|&c| (c.abs_diff(s), c))
}

/// Context score of the occurrence `[i, j)` of a quote in `t` (§3.7.1).
fn context_score(t: &[char], i: usize, j: usize, prefix: &[char], suffix: &[char]) -> usize {
    let left = (1..=prefix.len().min(i)).rev().find(|&k| t[i - k..i] == prefix[prefix.len() - k..]).unwrap_or(0);
    let right = (1..=suffix.len().min(t.len() - j)).rev().find(|&k| t[j..j + k] == suffix[..k]).unwrap_or(0);
    left + right
}

/// Selecting by context (§3.7.1).
fn select_by_context(t: &[char], occ: &[usize], qlen: usize, prefix: &[char], suffix: &[char]) -> Option<usize> {
    match occ {
        [] => None,
        [only] => Some(*only),
        _ => {
            let scored: Vec<(usize, usize)> =
                occ.iter().map(|&i| (context_score(t, i, i + qlen, prefix, suffix), i)).collect();
            let best = scored.iter().map(|&(sc, _)| sc).max()?;
            let mut winners = scored.iter().filter(|&&(sc, _)| sc == best);
            match (winners.next(), winners.next()) {
                (Some(&(_, i)), None) if best > 0 => Some(i),
                _ => None,
            }
        }
    }
}

fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

/// The collapsed form of `t` and the source span of each of its code points
/// (§3.7.1).
pub(crate) fn collapse(t: &[char]) -> (Vec<char>, Vec<(usize, usize)>) {
    let mut out = Vec::with_capacity(t.len());
    let mut spans = Vec::with_capacity(t.len());
    let mut k = 0;
    while k < t.len() {
        if is_whitespace(t[k]) {
            let start = k;
            while k < t.len() && is_whitespace(t[k]) {
                k += 1;
            }
            out.push(' ');
            spans.push((start, k));
        } else {
            out.push(t[k]);
            spans.push((k, k + 1));
            k += 1;
        }
    }
    (out, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worked_example_from_spec() {
        let original =
            Text::from_raw("\\section{Results}\nIn Section 3, we prove that the bound is tight for all $n$.\n");
        let anchor = create(&original, 32, 45, "paper.tex");
        assert_eq!(anchor.selectors.quote.exact, "we prove that");
        let edited = Text::from_raw(
            "\\section{Results}\nAs shown in Section 3, we prove that the bound is tight for all $n \\geq 1$.\n",
        );
        let r = resolve(&edited, &anchor);
        assert_eq!((r.state, r.range, r.step), (State::Relocated, Some((41, 54)), 3));
    }

    #[test]
    fn point_survives_nearby_edit() {
        // Changing "What" leaves 14 of the prefix's 32 code points and all
        // of the suffix: 46 of 64.
        let original = Text::from_raw("print(\"Hello Tobias! What are you doing?\")\nplt.plot([1, 2, 3], [1, 2, 3])\n");
        let anchor = create(&original, 39, 39, "app.py");
        let edited = Text::from_raw("print(\"Hello Tobias! How are you doing?\")\nplt.plot([1, 2, 3], [1, 2, 3])\n");
        let r = resolve(&edited, &anchor);
        assert_eq!((r.state, r.range, r.step), (State::Relocated, Some((38, 38)), 5));
    }

    /// An anchor on `quote`, the only occurrence in `before`, resolved
    /// against `after`: orphaned, with this suggested location.
    fn suggested(before: &str, quote: &str, after: &str) -> Option<(String, f64)> {
        let original = Text::from_raw(before);
        let start = original.to_string().find(quote).map(|b| before[..b].chars().count()).unwrap();
        let anchor = create(&original, start, start + quote.chars().count(), "plan.md");
        let edited = Text::from_raw(after);
        assert_eq!(resolve(&edited, &anchor).state, State::Orphaned);
        suggest(&edited, &anchor).map(|s| (edited.slice(s.range.0, s.range.1), s.score))
    }

    #[test]
    fn suggests_a_rewritten_passage() {
        let before = "## Backend\n\n- Alembic migration: `runtime_version: str`, set at project\n  creation to the \
                      default line. Existing projects are backfilled to `NULL`, which\n  means in-process legacy. \
                      An admin-only action exists but is\n  not exposed to researchers yet.\n- Settings: a map.\n";
        let quote = "Alembic migration: `runtime_version: str`, set at project\n  creation to the default line. \
                     Existing projects are backfilled to `NULL`, which\n  means in-process legacy. An admin-only \
                     action exists but is\n  not exposed to researchers yet.";
        let after = "## Backend\n\n- Alembic migration: `runtime_version: str NOT NULL`. Every\n  existing project \
                     is backfilled to the first line. An admin-only action\n  exists but is not exposed to \
                     researchers yet.\n- Worker address by naming convention.\n";
        let (text, score) = suggested(before, quote, after).unwrap();
        assert!(text.starts_with("Alembic migration") && text.ends_with("researchers yet."), "{text}");
        assert!(score >= 0.5, "{score}");
    }

    #[test]
    fn no_suggestion_when_the_text_is_gone() {
        let before = "- `compose/*.yml`: services `worker-0-5` and so on.\n  Internal network only.\n";
        let after = "Router: `0.5` ─▶ worker-0-5\n\nLine `X.Y` maps to `ws://worker-X-Y:8700`.\n";
        assert_eq!(suggested(before, "services `worker-0-5` and so on.", after), None);
    }

    #[test]
    fn short_quotes_get_no_suggestion() {
        assert_eq!(suggested("we prove that it holds\n", "prove that", "we show it holds\n"), None);
    }

    #[test]
    fn equally_good_passages_go_to_the_nearest() {
        // Three identical paragraphs, so neither the prefix nor the suffix
        // of the middle one is unique and bracket search can't rescue it.
        let para = "the quick brown fox jumps over the lazy dog";
        let before = [para; 3].join("\n\n");
        let quote = "fox jumps over the lazy";
        let start = para.len() + 2 + para.find(quote).unwrap();
        let anchor = create(&Text::from_raw(&before), start, start + quote.len(), "a.md");
        let edited = Text::from_raw(&before.replace("jumps", "leaps"));
        assert_eq!(resolve(&edited, &anchor).state, State::Orphaned);
        let s = suggest(&edited, &anchor).unwrap();
        assert_eq!(s.range, (start, start + quote.len()));
        assert_eq!(edited.slice(s.range.0, s.range.1), "fox leaps over the lazy");
    }

    #[test]
    fn ideographs_are_words() {
        assert_eq!(words(&"検索 abc_1, x".chars().collect::<Vec<_>>()), vec![(0, 1), (1, 2), (3, 8), (8, 9), (10, 11)]);
    }
}
