//! Normalized text, offsets, and versions (§3.2–§3.4).

use sha2::{Digest, Sha256};

/// Normalizes raw document content (§3.2): strips a leading BOM and turns
/// CRLF and lone CR into LF. No other normalization is applied.
pub fn normalize(raw: &str) -> String {
    let s = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// The version of normalized text (§3.4): `sha256:` plus the lowercase hex
/// digest of its UTF-8 encoding.
pub fn version(normalized: &str) -> String {
    let digest = Sha256::digest(normalized.as_bytes());
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Normalized text indexed by code point, which is the unit of every
/// offset in annox (§3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub chars: Vec<char>,
    pub version: String,
}

impl Text {
    /// Normalizes `raw` and indexes it.
    pub fn from_raw(raw: &str) -> Text {
        Text::from_normalized(normalize(raw))
    }

    /// Indexes text that is already normalized.
    pub fn from_normalized(normalized: String) -> Text {
        let version = version(&normalized);
        Text {
            chars: normalized.chars().collect(),
            version,
        }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// The code points in `[start, end)` as a string.
    pub fn slice(&self, start: usize, end: usize) -> String {
        self.chars[start..end].iter().collect()
    }

    /// The text with `[start, end)` replaced by `replacement`.
    pub fn splice(&self, start: usize, end: usize, replacement: &str) -> Text {
        let mut s: String = self.chars[..start].iter().collect();
        s.push_str(replacement);
        s.extend(&self.chars[end..]);
        Text::from_normalized(s)
    }
}

impl std::fmt::Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for c in &self.chars {
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_bom_and_line_endings() {
        assert_eq!(normalize("\u{feff}a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn offsets_count_code_points() {
        let t = Text::from_raw("café\r\nok");
        assert_eq!(t.len(), 7);
        assert_eq!(t.slice(5, 7), "ok");
    }
}
