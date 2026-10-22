//! A PDF's text as a document: its text layer laid out a line per line of
//! text (§3.2.1), and finding quotes in it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{anyhow, bail};
use lopdf::Document;

use super::{extract, find_all, key_chars, Glyph};

/// Whether the document at `path` is a PDF, whose text is its text layer.
pub fn is_pdf(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("pdf"))
}

/// The text annox reads from the file at `path`: for a PDF, the text of its
/// text layer, laid out a line per line of text (§3.2.1); for anything else,
/// the file itself.
pub fn read_document(path: &Path) -> std::io::Result<String> {
    if !is_pdf(&path.to_string_lossy()) {
        return std::fs::read_to_string(path);
    }
    let meta = std::fs::metadata(path)?;
    let stamp = (meta.modified()?, meta.len());
    // A PDF doesn't change, but laying its text out takes a while, and a
    // server reads it for every command.
    static LAID_OUT: Mutex<BTreeMap<PathBuf, (Stamp, String)>> = Mutex::new(BTreeMap::new());
    if let Some((s, text)) = LAID_OUT.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        if *s == stamp {
            return Ok(text.clone());
        }
    }
    let text = Document::load(path)
        .map_err(|e| anyhow!("{e}"))
        .and_then(|doc| {
            if doc.is_encrypted() {
                bail!("it's encrypted");
            }
            extract(&doc)
        })
        .map(|glyphs| layout(&glyphs))
        .map_err(|e| std::io::Error::other(format!("can't read {}: {e:#}", path.display())))?;
    LAID_OUT.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_owned(), (stamp, text.clone()));
    Ok(text)
}

/// The ranges of `text` that hold `quote` when only letters and digits are
/// compared, as for a PDF's text, where a quote copied from a viewer may break
/// lines, hyphenate or space words differently. Punctuation at either end of
/// the quote is kept when the text has it too.
pub fn find_loose(text: &[char], quote: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = key_chars(quote).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let (key, owner): (Vec<char>, Vec<usize>) = text
        .iter()
        .enumerate()
        .flat_map(|(i, c)| key_chars(&c.to_string()).map(move |k| (k, i)).collect::<Vec<_>>())
        .unzip();
    let quote = quote.trim();
    let head: Vec<char> = quote.chars().take_while(|c| !c.is_alphanumeric()).collect();
    let mut tail: Vec<char> = quote.chars().rev().take_while(|c| !c.is_alphanumeric()).collect();
    tail.reverse();
    find_all(&key, &needle)
        .map(|s| {
            let (mut start, mut end) = (owner[s], owner[s + needle.len() - 1] + 1);
            if start >= head.len() && text[start - head.len()..start] == head[..] {
                start -= head.len();
            }
            if text[end..].starts_with(&tail) {
                end += tail.len();
            }
            (start, end)
        })
        .collect()
}

/// When a file was last changed, and its size.
type Stamp = (SystemTime, u64);

/// How far apart two glyphs on a line must be, as a share of their height,
/// for a space to go between them.
const WORD_GAP: f64 = 0.12;

/// Whether `b` comes after `a` on the same line of text.
fn same_line(a: &Glyph, b: &Glyph) -> bool {
    let mid = (b.rect.y0 + b.rect.y1) / 2.0;
    a.page == b.page && mid > a.rect.y0 && mid < a.rect.y1 && b.rect.x0 >= a.rect.x0 - a.rect.height()
}

/// Whether `b` continues the word `a` is in.
pub(super) fn same_word(a: &Glyph, b: &Glyph) -> bool {
    let shows = |g: &Glyph| !g.text.trim().is_empty();
    shows(a) && shows(b) && same_line(a, b) && b.rect.x0 - a.rect.x1 <= WORD_GAP * a.rect.height()
}

/// The text of `glyphs` as a document: a line per line of text, words apart
/// by a space, and a blank line between pages. Words hyphenated across lines
/// stay hyphenated.
fn layout(glyphs: &[Glyph]) -> String {
    let mut out = String::new();
    let mut prev: Option<&Glyph> = None;
    for g in glyphs {
        let shown: String =
            g.text.chars().filter(|c| !c.is_control()).map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
        if shown.trim().is_empty() {
            if prev.is_some() && !out.ends_with(' ') {
                out.push(' ');
            }
            continue;
        }
        if let Some(p) = prev {
            if p.page != g.page || !same_line(p, g) {
                out.truncate(out.trim_end_matches(' ').len());
                out.push_str(if p.page == g.page { "\n" } else { "\n\n" });
            } else if !same_word(p, g) && !out.ends_with(' ') {
                out.push(' ');
            }
        }
        out.push_str(shown.trim());
        prev = Some(g);
    }
    out.truncate(out.trim_end_matches(' ').len());
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::Rect;
    use super::*;

    #[test]
    fn lays_text_out_a_line_per_line_and_a_page_per_paragraph() {
        let g = |page, text: &str, x0, y0| Glyph {
            page,
            text: text.into(),
            rect: Rect { x0, y0, x1: x0 + 5.0, y1: y0 + 10.0 },
        };
        let glyphs = [
            g(1, "a", 100.0, 500.0),
            g(1, "b", 105.0, 500.0),
            // Apart, but with no space glyph between.
            g(1, "c", 113.0, 500.0),
            g(1, " ", 118.0, 500.0),
            g(1, "d", 72.0, 486.0),
            g(1, "\u{0}", 77.0, 486.0),
            g(2, "e", 72.0, 700.0),
        ];
        assert_eq!(layout(&glyphs), "ab c\nd\n\ne\n");
        assert_eq!(layout(&[]), "");
    }

    #[test]
    fn finds_quotes_on_their_letters_and_digits() {
        let text: Vec<char> = "the ef-\nﬁcient bound, is tight.\n".chars().collect();
        let found = |q: &str| {
            find_loose(&text, q).into_iter().map(|(s, e)| text[s..e].iter().collect()).collect::<Vec<String>>()
        };
        assert_eq!(found("efficient bound"), ["ef-\nﬁcient bound"]);
        // Punctuation at the ends is kept when it's there.
        assert_eq!(found("bound, is tight."), ["bound, is tight."]);
        assert_eq!(found("(tight)"), ["tight"]);
        assert!(found("...").is_empty());
    }
}
