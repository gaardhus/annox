//! Applicability and applying suggestions (§4).

use crate::anchor::{self, nearest, Anchor, Resolution, State};
use crate::text::Text;

/// Whether a resolution makes a suggestion applicable (§4.2): exact, or
/// relocated by step 2, 3, or 5.
pub fn is_applicable(resolution: &Resolution) -> bool {
    resolution.state != State::Orphaned && resolution.step != 4
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

/// Finds the text a suggestion produces once applied (§4.3.3), used to
/// detect lost acceptances and to revert.
pub fn applied_text_search(doc: &Text, target: &Anchor, replacement: &str) -> Option<(usize, usize)> {
    let quote = &target.selectors.quote;
    let pattern = format!("{}{replacement}{}", quote.prefix, quote.suffix);
    let prefix_len = quote.prefix.chars().count();
    let starts: Vec<usize> = doc.haystack().find_all(&pattern).into_iter().map(|i| i + prefix_len).collect();
    nearest(&starts, target.selectors.position.start).map(|c| (c, c + replacement.chars().count()))
}
