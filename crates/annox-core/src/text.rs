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

/// A string prepared for fast substring search, with a map from byte
/// offsets back to code-point offsets.
#[derive(Clone, Debug)]
pub(crate) struct Haystack {
    text: String,
    /// Byte offset of each code point, plus the total length.
    char_bytes: Vec<usize>,
}

impl Haystack {
    pub(crate) fn new(chars: &[char]) -> Haystack {
        let text: String = chars.iter().collect();
        let mut char_bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        char_bytes.push(text.len());
        Haystack { text, char_bytes }
    }

    /// Code-point offsets of every occurrence of `needle`, overlaps included
    /// (§3.7.1).
    pub(crate) fn find_all(&self, needle: &str) -> Vec<usize> {
        if needle.is_empty() {
            return (0..self.char_bytes.len()).collect();
        }
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(pos) = self.text[from..].find(needle) {
            let byte = from + pos;
            let index = self.char_bytes.partition_point(|&b| b < byte);
            out.push(index);
            // Advance one code point, so overlapping occurrences are found.
            from = self.char_bytes[index + 1];
        }
        out
    }
}

/// The collapsed form of a text and the source span of each of its code
/// points (§3.7.1).
#[derive(Clone, Debug)]
pub(crate) struct Collapsed {
    pub(crate) chars: Vec<char>,
    pub(crate) spans: Vec<(usize, usize)>,
    pub(crate) haystack: Haystack,
}

/// Normalized text indexed by code point, which is the unit of every
/// offset in annox (§3.3).
#[derive(Clone, Debug)]
pub struct Text {
    pub chars: Vec<char>,
    pub version: String,
    haystack: std::sync::OnceLock<Haystack>,
    collapsed: std::sync::OnceLock<Collapsed>,
}

impl PartialEq for Text {
    fn eq(&self, other: &Text) -> bool {
        self.version == other.version && self.chars == other.chars
    }
}

impl Eq for Text {}

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
            haystack: std::sync::OnceLock::new(),
            collapsed: std::sync::OnceLock::new(),
        }
    }

    /// The search index, built on first use.
    pub(crate) fn haystack(&self) -> &Haystack {
        self.haystack.get_or_init(|| Haystack::new(&self.chars))
    }

    /// The collapsed form (§3.7.1), built on first use.
    pub(crate) fn collapsed(&self) -> &Collapsed {
        self.collapsed.get_or_init(|| {
            let (chars, spans) = crate::anchor::collapse(&self.chars);
            let haystack = Haystack::new(&chars);
            Collapsed { chars, spans, haystack }
        })
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
    fn find_all_returns_overlapping_code_point_offsets() {
        let h = Haystack::new(&"aé😀aaa😀a".chars().collect::<Vec<_>>());
        assert_eq!(h.find_all("aa"), vec![3, 4]);
        assert_eq!(h.find_all("😀a"), vec![2, 6]);
        assert_eq!(h.find_all("x"), Vec::<usize>::new());
        assert_eq!(h.find_all("").len(), 9);
    }

    #[test]
    fn offsets_count_code_points() {
        let t = Text::from_raw("café\r\nok");
        assert_eq!(t.len(), 7);
        assert_eq!(t.slice(5, 7), "ok");
    }
}
