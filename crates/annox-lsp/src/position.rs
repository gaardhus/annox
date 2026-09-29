//! Converting between LSP positions and code-point offsets (§6.3).

use annox_core::text::Text;
use lsp_types::{Position, PositionEncodingKind, Range};

/// A negotiated position encoding (§6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    /// Picks the client's most preferred encoding that the server supports,
    /// falling back to LSP's default, UTF-16.
    pub fn negotiate(offered: Option<&[PositionEncodingKind]>) -> Encoding {
        offered
            .unwrap_or_default()
            .iter()
            .find_map(|k| match k.as_str() {
                "utf-8" => Some(Encoding::Utf8),
                "utf-16" => Some(Encoding::Utf16),
                "utf-32" => Some(Encoding::Utf32),
                _ => None,
            })
            .unwrap_or(Encoding::Utf16)
    }

    pub fn kind(self) -> PositionEncodingKind {
        match self {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
            Encoding::Utf32 => PositionEncodingKind::UTF32,
        }
    }

    fn units(self, c: char) -> u32 {
        match self {
            Encoding::Utf8 => c.len_utf8() as u32,
            Encoding::Utf16 => c.len_utf16() as u32,
            Encoding::Utf32 => 1,
        }
    }
}

/// Line starts of normalized text. LSP's line breaks are exactly the ones
/// normalization turns into LF, so lines match the client's buffer (§6.3).
pub struct LineIndex<'a> {
    chars: &'a [char],
    starts: Vec<usize>,
    encoding: Encoding,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a Text, encoding: Encoding) -> LineIndex<'a> {
        let mut starts = vec![0];
        starts.extend(text.chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
        LineIndex { chars: &text.chars, starts, encoding }
    }

    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.chars.len());
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let start = self.starts[line];
        let character = self.chars[start..offset].iter().map(|&c| self.encoding.units(c)).sum();
        Position { line: line as u32, character }
    }

    pub fn range(&self, start: usize, end: usize) -> Range {
        Range { start: self.position(start), end: self.position(end) }
    }

    pub fn offset(&self, position: Position) -> usize {
        let Some(&start) = self.starts.get(position.line as usize) else {
            return self.chars.len();
        };
        let line_end = self.starts.get(position.line as usize + 1).map_or(self.chars.len(), |&s| s - 1);
        let mut units = 0;
        for (i, &c) in self.chars[start..line_end].iter().enumerate() {
            if units >= position.character {
                return start + i;
            }
            units += self.encoding.units(c);
        }
        line_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_in_every_encoding() {
        let text = Text::from_raw("a😀b\r\nçd\n");
        for enc in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            let index = LineIndex::new(&text, enc);
            for offset in 0..=text.len() {
                assert_eq!(index.offset(index.position(offset)), offset, "{enc:?} at {offset}");
            }
        }
        let utf16 = LineIndex::new(&text, Encoding::Utf16);
        assert_eq!(utf16.position(2), Position { line: 0, character: 3 });
        assert_eq!(utf16.position(5), Position { line: 1, character: 1 });
    }
}
