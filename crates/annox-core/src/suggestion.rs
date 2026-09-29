//! Applicability and applying suggestions (§4).

use crate::anchor::{self, nearest, occurrences, Anchor, Resolution, State};
use crate::text::Text;

/// Whether a resolution makes a suggestion applicable (§4.2): exact, or
/// relocated by step 2 or 3.
pub fn is_applicable(resolution: &Resolution) -> bool {
    resolution.state != State::Orphaned && resolution.step <= 3
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
        Some((start, end)) if is_applicable(&resolution) => Ok(Applied {
            start,
            end,
            text: doc.splice(start, end, replacement),
            resolution,
        }),
        _ => Err(resolution),
    }
}

/// Finds the text a suggestion produces once applied (§4.3.3), used to
/// detect lost acceptances and to revert.
pub fn applied_text_search(doc: &Text, target: &Anchor, replacement: &str) -> Option<(usize, usize)> {
    let p: Vec<char> = target.selectors.quote.prefix.chars().collect();
    let x: Vec<char> = target.selectors.quote.suffix.chars().collect();
    let r: Vec<char> = replacement.chars().collect();
    let pattern: Vec<char> = [&p[..], &r[..], &x[..]].concat();
    let starts: Vec<usize> = occurrences(&doc.chars, &pattern).into_iter().map(|i| i + p.len()).collect();
    nearest(&starts, target.selectors.position.start).map(|c| (c, c + r.len()))
}
