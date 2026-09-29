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
/// orphaned), and the step of §3.7.2 that produced it (5 when orphaned).
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

    const ORPHANED: Resolution = Resolution { state: State::Orphaned, range: None, step: 5 };
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
    if consistent
        && s >= p.len()
        && d[s - p.len()..s] == p[..]
        && e + x.len() <= d.len()
        && d[e..e + x.len()] == x[..]
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
    } else {
        // Step 4, point variant: whitespace-insensitive context search.
        let (wp, _) = collapse(&p);
        let (wx, _) = collapse(&x);
        let merged = wp.last() == Some(&' ') && wx.first() == Some(&' ');
        let pattern: Vec<char> = if merged {
            [&wp[..], &wx[1..]].concat()
        } else {
            [&wp[..], &wx[..]].concat()
        };
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
    }

    Resolution::ORPHANED
}

/// The candidate closest to `s`, with ties going to the lower offset (§3.7.1).
pub(crate) fn nearest(candidates: &[usize], s: usize) -> Option<usize> {
    candidates.iter().copied().min_by_key(|&c| (c.abs_diff(s), c))
}

/// Context score of the occurrence `[i, j)` of a quote in `t` (§3.7.1).
fn context_score(t: &[char], i: usize, j: usize, prefix: &[char], suffix: &[char]) -> usize {
    let left = (1..=prefix.len().min(i))
        .rev()
        .find(|&k| t[i - k..i] == prefix[prefix.len() - k..])
        .unwrap_or(0);
    let right = (1..=suffix.len().min(t.len() - j))
        .rev()
        .find(|&k| t[j..j + k] == suffix[..k])
        .unwrap_or(0);
    left + right
}

/// Selecting by context (§3.7.1).
fn select_by_context(
    t: &[char],
    occ: &[usize],
    qlen: usize,
    prefix: &[char],
    suffix: &[char],
) -> Option<usize> {
    match occ {
        [] => None,
        [only] => Some(*only),
        _ => {
            let scored: Vec<(usize, usize)> = occ
                .iter()
                .map(|&i| (context_score(t, i, i + qlen, prefix, suffix), i))
                .collect();
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
        let original = Text::from_raw(
            "\\section{Results}\nIn Section 3, we prove that the bound is tight for all $n$.\n",
        );
        let anchor = create(&original, 32, 45, "paper.tex");
        assert_eq!(anchor.selectors.quote.exact, "we prove that");
        let edited = Text::from_raw(
            "\\section{Results}\nAs shown in Section 3, we prove that the bound is tight for all $n \\geq 1$.\n",
        );
        let r = resolve(&edited, &anchor);
        assert_eq!((r.state, r.range, r.step), (State::Relocated, Some((41, 54)), 3));
    }
}
