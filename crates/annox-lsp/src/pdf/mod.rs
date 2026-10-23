//! PDFs: reading their text, exporting a document's threads into a PDF
//! compiled from it, and importing the comments reviewers made in one.
//!
//! Text is matched between a document and a PDF by reducing both sides to
//! lowercase letters and digits, which ignores whitespace, line breaks,
//! hyphenation and punctuation, after the source's markup is stripped. Text
//! that occurs more than once is told apart by its context, and then by how
//! far into the document it is.
//!
//! Much of what's written, and how, works around what PDF viewers do with
//! comments; `docs/pdf-viewers.md` lists each quirk and where it's handled.

mod appendix;
mod export;
mod import;
mod layout;
mod source;
mod words;
mod write;

use anyhow::{anyhow, bail};
use lopdf::{Document, Object, ObjectId};
use pdf_extract::{MediaBox, OutputDev, OutputError, Transform};
use unicode_normalization::UnicodeNormalization;

pub use export::{export, Exported, Owned, Reply, Thread};
pub use import::{import, Found, Parent, What};
pub use layout::{find_loose, is_pdf, read_document};
pub use source::Syntax;

/// The key of the page annox appends, in its page dictionary. The page isn't
/// part of the document's text, and each export replaces it.
const APPENDIX: &str = "AnnoxAppendix";

/// The numbers of the pages annox appended to `doc`.
fn appendix_pages(doc: &Document) -> Vec<u32> {
    doc.get_pages()
        .into_iter()
        .filter(|(_, id)| doc.get_dictionary(*id).is_ok_and(|d| d.has(APPENDIX.as_bytes())))
        .map(|(n, _)| n)
        .collect()
}

/// A PDF text string, such as an annotation's note, as text. Tabs and line
/// breaks are kept: lopdf drops them from strings in PDFDocEncoding, which
/// has them, so a note of several paragraphs would run together.
fn decode_text(o: &Object) -> Option<String> {
    let bytes = o.as_str().ok()?;
    if bytes.starts_with(b"\xFE\xFF") || bytes.starts_with(b"\xEF\xBB\xBF") {
        return lopdf::decode_text_string(o).ok();
    }
    let mut out = String::new();
    for piece in bytes.split_inclusive(|b| matches!(b, b'\t' | b'\n' | b'\r')) {
        let (text, end) = match piece.split_last() {
            Some((&b, text)) if matches!(b, b'\t' | b'\n' | b'\r') => (text, Some(b as char)),
            _ => (piece, None),
        };
        let text = Object::String(text.to_vec(), lopdf::StringFormat::Literal);
        out.push_str(&lopdf::decode_text_string(&text).ok()?);
        out.extend(end);
    }
    Some(out)
}

/// A glyph on a page, with its box in the page's default user space.
#[derive(Clone, Debug)]
struct Glyph {
    page: u32,
    text: String,
    rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    fn height(self) -> f64 {
        self.y1 - self.y0
    }
}

#[derive(Default)]
struct Collector {
    page: u32,
    glyphs: Vec<Glyph>,
}

impl OutputDev for Collector {
    fn begin_page(&mut self, page: u32, _: &MediaBox, _: Option<(f64, f64, f64, f64)>) -> Result<(), OutputError> {
        self.page = page;
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _: f64,
        size: f64,
        text: &str,
    ) -> Result<(), OutputError> {
        // The glyph's box in text space, from the descender to roughly the
        // ascender: fonts' real metrics aren't exposed.
        let w = if width > 0.0 { width * size } else { 0.5 * size };
        let corners = [(0.0, -0.22 * size), (w, -0.22 * size), (0.0, 0.8 * size), (w, 0.8 * size)];
        let mut rect: Option<Rect> = None;
        for (u, v) in corners {
            let (x, y) = (trm.m11 * u + trm.m21 * v + trm.m31, trm.m12 * u + trm.m22 * v + trm.m32);
            let r = Rect { x0: x, y0: y, x1: x, y1: y };
            rect = Some(rect.map_or(r, |acc| acc.union(r)));
        }
        if let Some(rect) = rect.filter(|r| r.x0.is_finite() && r.y0.is_finite()) {
            self.glyphs.push(Glyph { page: self.page, text: text.to_owned(), rect });
        }
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

/// Every glyph of `doc`, page by page, in content stream order, but for the
/// page annox appended.
fn extract(doc: &Document) -> anyhow::Result<Vec<Glyph>> {
    let appended = appendix_pages(doc);
    let mut doc = doc.clone();
    expand_cid_widths(&mut doc);
    let doc = &doc;
    let mut collector = Collector::default();
    // pdf-extract panics on some malformed PDFs rather than failing.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::output_doc(doc, &mut collector).map_err(|e| anyhow!("{e}"))
    }));
    std::panic::set_hook(hook);
    match result {
        Ok(Ok(())) => Ok(collector.glyphs.into_iter().filter(|g| !appended.contains(&g.page)).collect()),
        Ok(Err(e)) => Err(e.context("can't read the PDF's text")),
        Err(_) => bail!("can't read the PDF's text"),
    }
}

/// Rewrites each CID font's widths written as ranges, `first last width`, as
/// lists, `first [width ...]`. pdf-extract misreads ranges, which Typst
/// writes, and takes every glyph to be as wide as the default, often 0, so
/// the glyphs after the first of each string land in the wrong place.
fn expand_cid_widths(doc: &mut Document) {
    let fonts: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter(|(_, o)| {
            o.as_dict()
                .is_ok_and(|d| d.get(b"Subtype").and_then(Object::as_name).is_ok_and(|s| s.starts_with(b"CIDFontType")))
        })
        .map(|(id, _)| *id)
        .collect();
    for id in fonts {
        let Ok(font) = doc.get_dictionary(id) else { continue };
        let Ok((_, Object::Array(w))) = font.get(b"W").and_then(|o| doc.dereference(o)) else { continue };
        let mut out = Vec::new();
        let mut i = 0;
        while i < w.len() {
            match (w.get(i), w.get(i + 1), w.get(i + 2)) {
                (Some(first), Some(Object::Array(widths)), _) => {
                    out.extend([first.clone(), Object::Array(widths.clone())]);
                    i += 2;
                }
                (Some(first), Some(last), Some(width)) => {
                    let (Ok(f), Ok(l)) = (first.as_i64(), last.as_i64()) else { break };
                    // Bounded, in case of a nonsensical range.
                    let n = (l - f + 1).clamp(0, 65536) as usize;
                    out.extend([first.clone(), Object::Array(vec![width.clone(); n])]);
                    i += 3;
                }
                _ => break,
            }
        }
        if let Ok(font) = doc.get_object_mut(id).and_then(Object::as_dict_mut) {
            font.set("W", Object::Array(out));
        }
    }
}

/// The characters a match compares: lowercase letters and digits, after
/// compatibility normalization, which splits ligatures such as "ﬁ".
fn key_chars(s: &str) -> impl Iterator<Item = char> + '_ {
    s.nfkc().flat_map(char::to_lowercase).filter(|c| c.is_alphanumeric())
}

/// The PDF's text as key characters, and the glyph each one came from.
fn glyph_key(glyphs: &[Glyph]) -> (Vec<char>, Vec<usize>) {
    let mut key = Vec::new();
    let mut owner = Vec::new();
    for (i, g) in glyphs.iter().enumerate() {
        for c in key_chars(&g.text) {
            key.push(c);
            owner.push(i);
        }
    }
    (key, owner)
}

/// The range of `key` that holds `quote`, if it can be found: of its
/// occurrences, the one whose surroundings match more of `prefix` and
/// `suffix`, then the one closest to `at`, from 0 to 1, of the way in.
fn best_match(key: &[char], quote: &[char], prefix: &[char], suffix: &[char], at: f64) -> Option<(usize, usize)> {
    if quote.is_empty() {
        return None;
    }
    let mut candidates: Vec<(usize, usize)> = find_all(key, quote).map(|s| (s, s + quote.len())).collect();
    if candidates.is_empty() && quote.len() >= 12 {
        // Something in the middle of the quote didn't make it into the PDF,
        // such as a citation key: match its ends instead.
        let n = (quote.len() / 3).min(16);
        let (head, tail) = (&quote[..n], &quote[quote.len() - n..]);
        let reach = quote.len() * 2 + 32;
        candidates = find_all(key, head)
            .filter_map(|s| {
                let window = &key[s + n..(s + reach).min(key.len())];
                find_all(window, tail).next().map(|t| (s, s + n + t + n))
            })
            .collect();
    }
    let score = |&(s, e): &(usize, usize)| {
        let before = prefix.iter().rev().zip(key[..s].iter().rev()).take_while(|(a, b)| a == b).count();
        let after = suffix.iter().zip(&key[e..]).take_while(|(a, b)| a == b).count();
        let distance = (s as f64 / key.len().max(1) as f64 - at).abs();
        ((before + after) as f64, -distance)
    };
    candidates.into_iter().max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
}

fn find_all<'a>(hay: &'a [char], needle: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    hay.windows(needle.len().max(1)).enumerate().filter(move |(_, w)| *w == needle).map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use lopdf::Dictionary;

    use super::*;

    #[test]
    fn decodes_text_strings_with_their_line_breaks() {
        let literal = |b: &[u8]| Object::String(b.to_vec(), lopdf::StringFormat::Literal);
        assert_eq!(
            decode_text(&literal(b"Prove it\n\nSee\tLemma 4.\r")).as_deref(),
            Some("Prove it\n\nSee\tLemma 4.\r")
        );
        // PDFDocEncoding: 0x8D and 0x8E are curly quotes.
        assert_eq!(decode_text(&literal(b"\x8Da\x8E\n")).as_deref(), Some("\u{201c}a\u{201d}\n"));
        assert_eq!(decode_text(&literal(b"\xFE\xFF\x00a\x00\n")).as_deref(), Some("a\n"));
    }

    #[test]
    fn expands_cid_width_ranges() {
        let mut doc = Document::with_version("1.7");
        let w = vec![0.into(), 1.into(), 500.into(), 2.into(), vec![Object::Integer(250), Object::Integer(300)].into()];
        let mut font = Dictionary::new();
        font.set("Subtype", Object::Name(b"CIDFontType0".to_vec()));
        font.set("W", w);
        let id = doc.add_object(font);
        expand_cid_widths(&mut doc);
        let ints = |o: &Object| o.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect::<Vec<_>>();
        let w = doc.get_dictionary(id).unwrap().get(b"W").unwrap().as_array().unwrap().clone();
        assert_eq!(w.len(), 4);
        assert_eq!((w[0].as_i64().unwrap(), ints(&w[1])), (0, vec![500, 500]));
        assert_eq!((w[2].as_i64().unwrap(), ints(&w[3])), (2, vec![250, 300]));
    }
}
