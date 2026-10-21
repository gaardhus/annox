//! Exporting a document's threads into a PDF compiled from it, as native PDF
//! annotations, for reviewers who read the PDF instead of the source.
//!
//! Each thread is placed by finding its quote in the PDF's text: both sides
//! are reduced to lowercase letters and digits, which ignores whitespace, line
//! breaks, hyphenation and punctuation, after the source's markup is stripped.
//! A quote that occurs more than once is told apart by its context, and then
//! by how far into the document it is. Threads that can't be placed go on a
//! page appended to the end.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use pdf_extract::{MediaBox, OutputDev, OutputError, Transform};
use unicode_normalization::UnicodeNormalization;

/// A thread to export.
pub struct Thread {
    pub id: String,
    pub author: String,
    /// RFC 3339, as annox stores it.
    pub created: String,
    pub body: Option<String>,
    /// For a suggestion, the text it puts in place of the quote.
    pub replacement: Option<String>,
    pub quote: String,
    /// Source text just before and after the quote, to tell repeated quotes apart.
    pub prefix: String,
    pub suffix: String,
    /// How far into the source the quote starts, from 0 to 1.
    pub at: f64,
    pub line: Option<usize>,
    pub replies: Vec<Reply>,
}

pub struct Reply {
    pub id: String,
    pub author: String,
    pub created: String,
    pub body: String,
}

/// The markup the source is written in, which is stripped from quotes before
/// they're looked for in the PDF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    Markdown,
    Latex,
    Typst,
    Plain,
}

impl Syntax {
    pub fn of(path: &str) -> Syntax {
        match path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref() {
            Some("md" | "markdown" | "qmd" | "rmd") => Syntax::Markdown,
            Some("tex" | "latex" | "ltx") => Syntax::Latex,
            Some("typ") => Syntax::Typst,
            _ => Syntax::Plain,
        }
    }
}

/// Which threads were placed on the PDF's pages, and which went on the
/// appended page.
pub struct Exported {
    pub placed: Vec<String>,
    pub unplaced: Vec<String>,
}

/// Writes `input`, a PDF compiled from the document at `source`, with
/// `threads` added as annotations to `output`.
pub fn export(threads: &[Thread], source: &str, input: &Path, output: &Path) -> anyhow::Result<Exported> {
    let syntax = Syntax::of(source);
    let mut doc = Document::load(input).with_context(|| format!("can't read {}", input.display()))?;
    if doc.is_encrypted() {
        bail!("{} is encrypted", input.display());
    }
    let glyphs = extract(&doc)?;
    let (key, owner) = glyph_key(&glyphs);
    let pages = doc.get_pages();
    // The right edge of each page's text, which a replacement drawn above
    // struck text may run up to.
    let mut right: BTreeMap<u32, f64> = BTreeMap::new();
    for g in &glyphs {
        let r = right.entry(g.page).or_insert(g.rect.x1);
        *r = r.max(g.rect.x1);
    }

    let mut placed = Vec::new();
    let mut unplaced = Vec::new();
    let mut annots: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    for thread in threads {
        let marks = locate(&key, thread, syntax)
            .map(|(s, e)| marks(thread, syntax, &glyphs, &owner[s..e], &right))
            .unwrap_or_default();
        if marks.is_empty() {
            unplaced.push(thread);
            continue;
        }
        let mut root = None;
        for (n, mark) in marks.iter().enumerate() {
            let page_id = *pages.get(&mark.page).ok_or_else(|| anyhow!("no page {}", mark.page))?;
            let ids = annotate(&mut doc, page_id, thread, mark, n, &mut root);
            annots.entry(page_id).or_default().extend(ids);
        }
        placed.push(thread.id.clone());
    }
    if !unplaced.is_empty() {
        let page_id =
            appendix(&mut doc, &unplaced, source, *pages.values().next().ok_or_else(|| anyhow!("no pages"))?)?;
        for (thread, rect) in unplaced.iter().zip(appendix_slots(&doc, page_id, unplaced.len())) {
            let ids = note(&mut doc, page_id, thread, rect);
            annots.entry(page_id).or_default().extend(ids);
        }
    }
    for (page_id, ids) in annots {
        add_annots(&mut doc, page_id, ids)?;
    }
    doc.save(output).with_context(|| format!("can't write {}", output.display()))?;
    Ok(Exported { placed, unplaced: unplaced.iter().map(|t| t.id.clone()).collect() })
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

/// Every glyph of `doc`, page by page, in content stream order.
fn extract(doc: &Document) -> anyhow::Result<Vec<Glyph>> {
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
        Ok(Ok(())) => Ok(collector.glyphs),
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

fn source_key(s: &str, syntax: Syntax) -> Vec<char> {
    key_chars(&strip(s, syntax)).collect()
}

/// LaTeX commands whose first argument doesn't show up as text.
const HIDDEN_ARG: &[&str] = &[
    "cite",
    "citep",
    "citet",
    "parencite",
    "textcite",
    "autocite",
    "ref",
    "eqref",
    "autoref",
    "cref",
    "Cref",
    "pageref",
    "label",
    "href",
    "includegraphics",
    "input",
    "include",
    "bibliography",
    "bibliographystyle",
    "usepackage",
    "documentclass",
    "begin",
    "end",
    "vspace",
    "hspace",
];

/// Removes the markup of `syntax` that doesn't show up in the compiled text.
/// It doesn't need to be exact: what's left is only compared, never shown.
fn strip(s: &str, syntax: Syntax) -> String {
    strip_indexed(s, syntax).into_iter().map(|(c, _)| c).collect()
}

/// [`strip`], with the index in `s` of each character kept.
fn strip_indexed(s: &str, syntax: Syntax) -> Vec<(char, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    let skip_group = |i: usize, open: char, close: char| -> usize {
        if chars.get(i) != Some(&open) {
            return i;
        }
        let mut depth = 0;
        for (j, &c) in chars.iter().enumerate().skip(i) {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
        }
        chars.len()
    };
    let ident_end = |mut j: usize, extra: &dyn Fn(char) -> bool| {
        while j < chars.len() && (chars[j].is_ascii_alphabetic() || extra(chars[j])) {
            j += 1;
        }
        j
    };
    while i < chars.len() {
        let c = chars[i];
        match syntax {
            Syntax::Latex if c == '%' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            Syntax::Latex if c == '\\' && chars.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == '@') => {
                let end = ident_end(i + 1, &|c| c == '@');
                let name: String = chars[i + 1..end].iter().collect();
                i = end;
                if chars.get(i) == Some(&'*') {
                    i += 1;
                }
                if HIDDEN_ARG.contains(&name.as_str()) {
                    i = skip_group(i, '[', ']');
                    i = skip_group(i, '{', '}');
                }
                continue;
            }
            Syntax::Latex if c == '\\' => {
                // An escaped character, as in `\%`, is text.
                out.extend(chars.get(i + 1).map(|&c| (c, i + 1)));
                i += 2;
                continue;
            }
            Syntax::Typst if c == '/' && chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            Syntax::Typst if c == '#' && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic) => {
                i = ident_end(i + 1, &|c| c == '.' || c == '-' || c == '_' || c.is_ascii_digit());
                // A call's arguments aren't text; its trailing content block is.
                i = skip_group(i, '(', ')');
                continue;
            }
            Syntax::Typst if c == '<' && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic) => {
                let end = ident_end(i + 1, &|c| c == '.' || c == '-' || c == '_' || c == ':' || c.is_ascii_digit());
                if chars.get(end) == Some(&'>') {
                    i = end + 1;
                    continue;
                }
            }
            Syntax::Markdown if c == ']' && chars.get(i + 1) == Some(&'(') => {
                i = skip_group(i + 1, '(', ')');
                continue;
            }
            Syntax::Markdown if c == '<' && chars.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == '/') => {
                if let Some(end) = chars[i..].iter().position(|&c| c == '>' || c == '\n') {
                    if chars[i + end] == '>' {
                        i += end + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        out.push((c, i));
        i += 1;
    }
    out
}

/// The range of `key` that holds `thread`'s quote, if it can be found.
fn locate(key: &[char], thread: &Thread, syntax: Syntax) -> Option<(usize, usize)> {
    let quote = source_key(&thread.quote, syntax);
    if quote.is_empty() {
        return None;
    }
    let prefix = source_key(&thread.prefix, syntax);
    let suffix = source_key(&thread.suffix, syntax);
    let mut candidates: Vec<(usize, usize)> = find_all(key, &quote).map(|s| (s, s + quote.len())).collect();
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
        let distance = (s as f64 / key.len().max(1) as f64 - thread.at).abs();
        ((before + after) as f64, -distance)
    };
    candidates.into_iter().max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
}

fn find_all<'a>(hay: &'a [char], needle: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    hay.windows(needle.len().max(1)).enumerate().filter(move |(_, w)| *w == needle).map(|(i, _)| i)
}

/// `glyphs` grouped into one box per line, per page, in order.
fn lines(glyphs: &[Glyph]) -> Vec<(u32, Vec<Rect>)> {
    let mut out: Vec<(u32, Vec<Rect>)> = Vec::new();
    for g in glyphs.iter().filter(|g| !g.text.trim().is_empty()) {
        let same_page = out.last().is_some_and(|(p, _)| *p == g.page);
        if !same_page {
            out.push((g.page, vec![g.rect]));
            continue;
        }
        let rects = &mut out.last_mut().expect("a page").1;
        let line = rects.last_mut().expect("a line");
        let mid = (g.rect.y0 + g.rect.y1) / 2.0;
        let same_line = mid > line.y0 && mid < line.y1 && g.rect.x0 >= line.x0 - line.height();
        if same_line {
            *line = line.union(g.rect);
        } else {
            rects.push(g.rect);
        }
    }
    out
}

/// A PDF text string: PDFDocEncoding when it's ASCII, else UTF-16BE.
fn text(s: &str) -> Object {
    if s.chars().all(|c| c.is_ascii() && (!c.is_ascii_control() || c == '\n')) {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// An RFC 3339 time as a PDF date, as in `D:20261007150054Z`.
fn date(rfc3339: &str) -> Object {
    let digits: String = rfc3339.chars().take(19).filter(char::is_ascii_digit).collect();
    Object::String(format!("D:{digits}Z").into_bytes(), StringFormat::Literal)
}

fn color(rgb: [f32; 3]) -> Object {
    Object::Array(rgb.into_iter().map(Object::Real).collect())
}

const YELLOW: [f32; 3] = [1.0, 0.85, 0.1];
const RED: [f32; 3] = [0.85, 0.15, 0.15];
/// Dark, so that small text stays legible on white.
const GREEN: [f32; 3] = [0.0, 0.5, 0.2];
/// Print, so the annotations show up when the PDF is printed.
const PRINT: i64 = 4;

fn base(thread: &Thread, subtype: &str) -> Dictionary {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    // The annotation's annox id, so the PDF's comments can be matched back to it.
    d.set("NM", text(&thread.id));
    d.set("T", text(&thread.author));
    d.set("M", date(&thread.created));
    d.set("CreationDate", date(&thread.created));
    d.set("F", PRINT);
    d.set("C", color(if thread.replacement.is_some() { RED } else { YELLOW }));
    d
}

fn set_quads(d: &mut Dictionary, rects: &[Rect], page: ObjectId) {
    let bbox = rects.iter().copied().reduce(Rect::union).expect("a rect");
    d.set("Rect", rect_object(bbox));
    // Each quad's corners go top left, top right, bottom left, bottom right,
    // which is the order viewers expect, whatever the spec's wording.
    let quads = rects.iter().flat_map(|r| [r.x0, r.y1, r.x1, r.y1, r.x0, r.y0, r.x1, r.y0]);
    d.set("QuadPoints", Object::Array(quads.map(|v| Object::Real(v as f32)).collect()));
    d.set("P", page);
}

fn rect_object(r: Rect) -> Object {
    Object::Array([r.x0, r.y0, r.x1, r.y1].into_iter().map(|v| Object::Real(v as f32)).collect())
}

/// The note a thread's root shows: its body, and for a suggestion whose
/// change isn't drawn in full on the page, what it replaces the quote with.
fn contents(thread: &Thread, with_quote: bool, with_replacement: bool) -> Option<String> {
    let mut parts = Vec::new();
    if with_quote {
        parts.push(format!("“{}”", thread.quote.trim()));
    }
    match thread.replacement.as_deref().filter(|_| with_replacement) {
        Some("") => parts.push("Delete".to_owned()),
        Some(r) => parts.push(format!("Replace with: “{r}”")),
        None => {}
    }
    parts.extend(thread.body.clone().filter(|b| !b.is_empty()));
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// A thread's markup on one page: the lines it marks, highlighted or, for a
/// suggestion, struck out, and for a suggestion, the words of the quote that
/// change, tinted, and on its first page, the replacement written above, with
/// the words that change tinted too. Like the Neovim plugin's inline
/// suggestions, which strike the whole quote and tint the words that change.
#[derive(Debug, Default)]
struct Mark {
    page: u32,
    struck: Vec<Rect>,
    deleted: Vec<Rect>,
    runs: Vec<Run>,
    inserted: Vec<Rect>,
    /// Whether the replacement was cut short to fit above the struck text.
    cut: bool,
}

/// The marks of `thread`, whose quote's key characters came from the glyphs
/// `owner`, one per page.
fn marks(thread: &Thread, syntax: Syntax, glyphs: &[Glyph], owner: &[usize], right: &BTreeMap<u32, f64>) -> Vec<Mark> {
    let (first, last) = (owner[0], owner[owner.len() - 1]);
    let mut marks: Vec<Mark> = lines(&glyphs[first..=last])
        .into_iter()
        .map(|(page, struck)| Mark { page, struck, ..Mark::default() })
        .collect();
    let Some(replacement) = thread.replacement.as_deref() else { return marks };
    let (deleted, inserted) = word_changes(&thread.quote, replacement);
    // Which character of the quote each key character came from. When the
    // quote was matched by its ends, they don't line up, and nothing's tinted.
    let sources: Vec<usize> = strip_indexed(&thread.quote, syntax)
        .into_iter()
        .flat_map(|(c, i)| key_chars(&c.to_string()).map(move |_| i).collect::<Vec<_>>())
        .collect();
    if sources.len() == owner.len() {
        for (a, b) in deleted {
            let mut keys = (0..sources.len()).filter(|&k| sources[k] >= a && sources[k] < b);
            let Some(k0) = keys.next() else { continue };
            let k1 = keys.next_back().unwrap_or(k0);
            for (page, rects) in lines(&glyphs[owner[k0]..=owner[k1]]) {
                if let Some(m) = marks.iter_mut().find(|m| m.page == page) {
                    m.deleted.extend(rects);
                }
            }
        }
    }
    if let Some(m) = marks.first_mut() {
        let (text, inserted) = collapse(replacement, &inserted);
        m.runs = above(&m.struck, right[&m.page], &text);
        m.inserted = tints(&m.runs, &inserted);
        m.cut = m.runs.last().map_or(!text.is_empty(), |r| r.start + r.shown < text.chars().count());
    }
    marks
}

/// A range `[start, end)` of a string's characters.
type Span = (usize, usize);

/// Whether `c` is part of a word for [`word_changes`]: as in the Neovim
/// plugin, letters, digits, underscores and anything outside ASCII.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// `s` split into words, runs of spaces and tabs, and single other
/// characters, as character ranges.
fn tokens(s: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let same = |c: char| {
            if is_word(s[i]) {
                is_word(c)
            } else if matches!(s[i], ' ' | '\t') {
                matches!(c, ' ' | '\t')
            } else {
                false
            }
        };
        let mut j = i + 1;
        while j < s.len() && same(s[j]) {
            j += 1;
        }
        out.push((i, j));
        i = j;
    }
    out
}

/// The words that differ between `old` and `new`, diff-so-fancy style, as
/// character ranges of each, the same as the Neovim plugin's: none when either
/// is empty, or when the two share no word, as then everything changed.
fn word_changes(old: &str, new: &str) -> (Vec<Span>, Vec<Span>) {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let (ta, tb) = (tokens(&a), tokens(&b));
    if ta.is_empty() || tb.is_empty() || ta.len() * tb.len() > 1_000_000 {
        return (Vec::new(), Vec::new());
    }
    fn word(s: &[char], (i, j): (usize, usize)) -> &[char] {
        &s[i..j]
    }
    // The longest common subsequence of tokens; what's not in it changed.
    let mut lcs = vec![vec![0u32; tb.len() + 1]; ta.len() + 1];
    for i in (0..ta.len()).rev() {
        for j in (0..tb.len()).rev() {
            lcs[i][j] = if word(&a, ta[i]) == word(&b, tb[j]) {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut changed_a, mut changed_b) = (vec![true; ta.len()], vec![true; tb.len()]);
    let (mut i, mut j) = (0, 0);
    while i < ta.len() && j < tb.len() {
        if word(&a, ta[i]) == word(&b, tb[j]) {
            (changed_a[i], changed_b[j]) = (false, false);
            (i, j) = (i + 1, j + 1);
        } else if lcs[i][j + 1] >= lcs[i + 1][j] {
            j += 1;
        } else {
            i += 1;
        }
    }
    let shared = ta.iter().zip(&changed_a).any(|(&t, &changed)| !changed && is_word(a[t.0]));
    if !shared {
        return (Vec::new(), Vec::new());
    }
    // Runs of changed tokens, joined when only spaces separate them.
    let ranges = |s: &[char], toks: &[(usize, usize)], changed: &[bool]| {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (&(t0, t1), _) in toks.iter().zip(changed).filter(|(_, &c)| c) {
            match out.last_mut() {
                Some(last) if s[last.1..t0].iter().all(|c| matches!(c, ' ' | '\t')) => last.1 = t1,
                _ => out.push((t0, t1)),
            }
        }
        // Which of two equal spaces counts as changed is arbitrary; neither is.
        for (i, j) in &mut out {
            while *i < *j && matches!(s[*i], ' ' | '\t') {
                *i += 1;
            }
            while *j > *i && matches!(s[*j - 1], ' ' | '\t') {
                *j -= 1;
            }
        }
        out.retain(|(i, j)| j > i);
        out
    };
    (ranges(&a, &ta, &changed_a), ranges(&b, &tb, &changed_b))
}

/// `s` with its whitespace collapsed to single spaces and trimmed, as it's
/// drawn, and `ranges` of its characters moved to match.
fn collapse(s: &str, ranges: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    let mut out = String::new();
    let mut len = 0;
    let mut at = Vec::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
            at.push(len);
            continue;
        }
        if space && len > 0 {
            out.push(' ');
            len += 1;
        }
        space = false;
        at.push(len);
        out.push(c);
        len += 1;
    }
    at.push(len);
    let moved = ranges.iter().map(|&(a, b)| (at[a], at[b])).filter(|(a, b)| b > a).collect();
    (out, moved)
}

/// The annotations of `mark`, the `n`th page of `thread`'s markup: a
/// highlight, or a strikeout and a caret grouped with it, whose appearance
/// writes the replacement above the struck text and tints the words that
/// change. The first page's highlight or strikeout is the thread's root,
/// which holds its note and its replies; `root` keeps it.
fn annotate(
    doc: &mut Document,
    page: ObjectId,
    thread: &Thread,
    mark: &Mark,
    n: usize,
    root: &mut Option<ObjectId>,
) -> Vec<ObjectId> {
    let suffix = |name: &str| match (name, n) {
        ("", 0) => thread.id.clone(),
        ("", n) => format!("{}-{n}", thread.id),
        (name, 0) => format!("{}-{name}", thread.id),
        (name, n) => format!("{}-{name}-{n}", thread.id),
    };
    let mut d = base(thread, if thread.replacement.is_some() { "StrikeOut" } else { "Highlight" });
    d.set("NM", text(&suffix("")));
    set_quads(&mut d, &mark.struck, page);
    let is_root = root.is_none();
    if is_root {
        // The page shows the change, so the note needn't repeat it, unless
        // it had to be cut short.
        if let Some(c) = contents(thread, false, mark.cut) {
            d.set("Contents", text(&c));
        }
    }
    let id = doc.add_object(d);
    let mut ids = vec![id];
    if !mark.runs.is_empty() || !mark.deleted.is_empty() {
        // Acrobat's "replace text" markup is a caret grouped with struck
        // text. Its appearance does the drawing, as viewers otherwise show the
        // replacement only in a popup, if at all.
        let mut c = base(thread, "Caret");
        c.set("NM", text(&suffix("caret")));
        c.set("C", color(GREEN));
        let (bbox, appearance) = draw(&mark.runs, &mark.deleted, &mark.inserted);
        c.set("Rect", rect_object(bbox));
        let form = doc.add_object(appearance);
        let mut ap = Dictionary::new();
        ap.set("N", form);
        c.set("AP", ap);
        c.set("Contents", text(thread.replacement.as_deref().unwrap_or_default()));
        c.set("IRT", id);
        c.set("RT", Object::Name(b"Group".to_vec()));
        c.set("P", page);
        ids.push(doc.add_object(c));
    }
    if is_root {
        *root = Some(id);
        ids.extend(replies(doc, page, thread, id, mark.struck[0]));
    }
    ids
}

/// A piece of a replacement, drawn in Helvetica with its baseline at `y`.
/// It shows `shown` characters of the replacement from `start`, then an
/// ellipsis if it was cut short.
#[derive(Debug, PartialEq)]
struct Run {
    x: f64,
    y: f64,
    size: f64,
    text: String,
    start: usize,
    shown: usize,
}

/// Lays `text`, with single spaces between its words, out in the gaps above
/// `lines`, each from the line's start to `right`, in a font about half the
/// size of the text below. What's left after the last line is cut short.
fn above(lines: &[Rect], right: f64, text: &str) -> Vec<Run> {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    let mut next = 0;
    let mut start = 0;
    let mut runs = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if next == words.len() {
            break;
        }
        // The glyph boxes run from 0.22 below the baseline to 0.8 above it.
        let font = line.height() / 1.02;
        let size = (font * 0.55).max(4.0);
        let y = line.y0 + 0.22 * font + 0.78 * font;
        let width = right.max(line.x1) - line.x0;
        let (run, shown) = if i == lines.len() - 1 {
            let rest = words[next..].join(" ");
            next = words.len();
            let run = fit(&rest, width, size);
            let shown = if run == rest { run.chars().count() } else { run.chars().count() - 1 };
            (run, shown)
        } else {
            let mut run = String::new();
            while let Some(word) = words.get(next) {
                let longer = if run.is_empty() { (*word).to_owned() } else { format!("{run} {word}") };
                if !run.is_empty() && helvetica_width(&longer, size) > width {
                    break;
                }
                run = longer;
                next += 1;
            }
            let shown = run.chars().count();
            (run, shown)
        };
        runs.push(Run { x: line.x0, y, size, text: run, start, shown });
        // The space after the run.
        start += shown + 1;
    }
    runs
}

/// Boxes around the parts of `runs` that `ranges` of their text cover.
fn tints(runs: &[Run], ranges: &[(usize, usize)]) -> Vec<Rect> {
    let mut out = Vec::new();
    for run in runs {
        let chars: Vec<char> = run.text.chars().collect();
        let width = |n: usize| helvetica_width(&chars[..n].iter().collect::<String>(), run.size);
        for &(a, b) in ranges {
            let (a, b) = (a.max(run.start), b.min(run.start + run.shown));
            if b > a {
                let (x0, x1) = (run.x + width(a - run.start), run.x + width(b - run.start));
                out.push(Rect { x0, y0: run.y - 0.25 * run.size, x1, y1: run.y + 0.85 * run.size });
            }
        }
    }
    out
}

const RED_TINT: [f32; 3] = [1.0, 0.78, 0.78];
const GREEN_TINT: [f32; 3] = [0.78, 0.93, 0.8];

/// An appearance stream drawing `runs` in green, over `inserted` tinted
/// green, and tinting `deleted` red, and the box it covers. The tints
/// multiply, so the text under them shows through.
fn draw(runs: &[Run], deleted: &[Rect], inserted: &[Rect]) -> (Rect, Stream) {
    let mut ops = String::from("q /Multiply gs\n");
    let mut bbox: Option<Rect> = None;
    let mut add = |b: Rect| bbox = Some(bbox.map_or(b, |acc| acc.union(b)));
    for (tint, rects) in [(RED_TINT, deleted), (GREEN_TINT, inserted)] {
        let [r, g, b] = tint;
        ops.push_str(&format!("{r} {g} {b} rg\n"));
        for t in rects {
            ops.push_str(&format!("{} {} {} {} re f\n", t.x0, t.y0, t.x1 - t.x0, t.y1 - t.y0));
            add(*t);
        }
    }
    let [r, g, b] = GREEN;
    ops.push_str(&format!("Q\n{r} {g} {b} rg\n"));
    for r in runs {
        ops.push_str(&format!("BT /Helv {} Tf {} {} Td ({}) Tj ET\n", r.size, r.x, r.y, pdf_literal(&r.text)));
        add(Rect { x0: r.x, y0: r.y - 0.25 * r.size, x1: r.x + helvetica_width(&r.text, r.size), y1: r.y + r.size });
    }
    let bbox = bbox.unwrap_or(Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 });
    let mut fonts = Dictionary::new();
    fonts.set("Helv", helvetica("Helvetica"));
    let mut multiply = Dictionary::new();
    multiply.set("Type", Object::Name(b"ExtGState".to_vec()));
    multiply.set("BM", Object::Name(b"Multiply".to_vec()));
    let mut states = Dictionary::new();
    states.set("Multiply", multiply);
    let mut resources = Dictionary::new();
    resources.set("Font", fonts);
    resources.set("ExtGState", states);
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Form".to_vec()));
    // The same coordinates as the page, so everything lands where it was laid out.
    d.set("BBox", rect_object(bbox));
    d.set("Resources", resources);
    (bbox, Stream::new(d, ops.into_bytes()))
}

/// A thread that couldn't be placed, as a note at `rect` on the appended page.
fn note(doc: &mut Document, page: ObjectId, thread: &Thread, rect: Rect) -> Vec<ObjectId> {
    let mut d = base(thread, "Text");
    d.set("Rect", rect_object(rect));
    d.set("P", page);
    d.set("Name", Object::Name(b"Comment".to_vec()));
    if let Some(c) = contents(thread, true, true) {
        d.set("Contents", text(&c));
    }
    let root = doc.add_object(d);
    let mut ids = vec![root];
    ids.extend(replies(doc, page, thread, root, rect));
    ids
}

fn replies(doc: &mut Document, page: ObjectId, thread: &Thread, root: ObjectId, at: Rect) -> Vec<ObjectId> {
    thread
        .replies
        .iter()
        .map(|r| {
            let mut d = Dictionary::new();
            d.set("Type", Object::Name(b"Annot".to_vec()));
            d.set("Subtype", Object::Name(b"Text".to_vec()));
            d.set("NM", text(&r.id));
            d.set("T", text(&r.author));
            d.set("M", date(&r.created));
            d.set("CreationDate", date(&r.created));
            d.set("Contents", text(&r.body));
            d.set("IRT", root);
            d.set("Rect", rect_object(Rect { x0: at.x0, y0: at.y1, x1: at.x0, y1: at.y1 }));
            d.set("P", page);
            // NoZoom and NoRotate, like a reply made in Acrobat, and an empty
            // appearance: viewers show a reply in its parent's thread, but
            // some, such as those built on poppler, also draw it as an icon
            // over the text unless its appearance is empty.
            d.set("F", 24);
            let mut form = Dictionary::new();
            form.set("Type", Object::Name(b"XObject".to_vec()));
            form.set("Subtype", Object::Name(b"Form".to_vec()));
            form.set("BBox", rect_object(Rect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 }));
            let mut ap = Dictionary::new();
            ap.set("N", doc.add_object(Stream::new(form, Vec::new())));
            d.set("AP", ap);
            doc.add_object(d)
        })
        .collect()
}

fn add_annots(doc: &mut Document, page: ObjectId, ids: Vec<ObjectId>) -> anyhow::Result<()> {
    let refs = ids.into_iter().map(Object::Reference);
    let existing = doc.get_dictionary(page)?.get(b"Annots").ok().cloned();
    match existing {
        Some(Object::Reference(array)) => doc.get_object_mut(array)?.as_array_mut()?.extend(refs),
        Some(Object::Array(mut array)) => {
            array.extend(refs);
            doc.get_object_mut(page)?.as_dict_mut()?.set("Annots", array);
        }
        _ => doc.get_object_mut(page)?.as_dict_mut()?.set("Annots", Object::Array(refs.collect())),
    }
    Ok(())
}

/// The media box of `page`, which it may inherit from its parents.
fn media_box(doc: &Document, page: ObjectId) -> Rect {
    let mut dict = doc.get_dictionary(page).ok();
    while let Some(d) = dict {
        if let Ok(Object::Array(a)) = d.get(b"MediaBox").and_then(|o| doc.dereference(o).map(|(_, o)| o)) {
            let v: Vec<f64> = a.iter().filter_map(|o| o.as_float().ok()).map(f64::from).collect();
            if let [x0, y0, x1, y1] = v[..] {
                return Rect { x0, y0, x1, y1 };
            }
        }
        dict = d.get(b"Parent").and_then(Object::as_reference).and_then(|p| doc.get_dictionary(p)).ok();
    }
    Rect { x0: 0.0, y0: 0.0, x1: 595.0, y1: 842.0 }
}

const MARGIN: f64 = 56.0;
const TITLE_SIZE: f64 = 14.0;
const SIZE: f64 = 10.0;
const LEADING: f64 = 14.0;
/// The space between the top of the title and the first entry: the title,
/// the line saying where the comments came from, and a gap.
const HEADER: f64 = TITLE_SIZE + LEADING * 3.0;
/// Where readers can find out what annox is.
const SITE: &str = "https://gaardhus.github.io/annox/";
/// The space each unplaced thread takes on the appended page: its quote and
/// its note, each on one line.
const SLOT: f64 = LEADING * 2.5;

/// Appends a page listing `threads`, sized like `like`, and returns its id.
/// It says they're annox comments on `source`, and links to annox's site.
fn appendix(doc: &mut Document, threads: &[&Thread], source: &str, like: ObjectId) -> anyhow::Result<ObjectId> {
    let mb = media_box(doc, like);
    let mut ops = String::new();
    let x = mb.x0 + MARGIN;
    let mut y = mb.y1 - MARGIN - TITLE_SIZE;
    let width = mb.x1 - mb.x0 - 2.0 * MARGIN - 24.0;
    ops.push_str(&format!(
        "BT /F2 {TITLE_SIZE} Tf {x} {y} Td ({}) Tj ET\n",
        pdf_literal("Comments not found in the text")
    ));
    let by = format!("Review comments on {source}, exported by annox: ");
    let site = SITE.trim_start_matches("https://").trim_end_matches('/');
    let credit_y = y - LEADING * 1.3;
    ops.push_str(&format!(
        "0.4 g BT /F1 {SIZE} Tf {x} {credit_y} Td ({}) Tj ET 0 g\n",
        pdf_literal(&format!("{by}{site}"))
    ));
    // The site's address, clickable.
    let link_x = x + helvetica_width(&by, SIZE);
    let mut link = Dictionary::new();
    link.set("Type", Object::Name(b"Annot".to_vec()));
    link.set("Subtype", Object::Name(b"Link".to_vec()));
    link.set(
        "Rect",
        rect_object(Rect {
            x0: link_x,
            y0: credit_y - 3.0,
            x1: link_x + helvetica_width(site, SIZE),
            y1: credit_y + SIZE,
        }),
    );
    link.set("Border", Object::Array(vec![0.into(), 0.into(), 0.into()]));
    let mut uri = Dictionary::new();
    uri.set("S", Object::Name(b"URI".to_vec()));
    uri.set("URI", Object::String(SITE.as_bytes().to_vec(), StringFormat::Literal));
    link.set("A", uri);
    let link = doc.add_object(link);
    y = mb.y1 - MARGIN - HEADER;
    for thread in threads {
        if y < mb.y0 + MARGIN {
            break;
        }
        let at = thread.line.map_or_else(String::new, |l| format!("line {l}: "));
        let head = fit(&format!("{at}“{}”", one_line(&thread.quote)), width, SIZE);
        let note = fit(
            &format!("{}: {}", thread.author, one_line(&contents(thread, false, true).unwrap_or_default())),
            width,
            SIZE,
        );
        let x = mb.x0 + MARGIN + 24.0;
        ops.push_str(&format!("BT /F2 {SIZE} Tf {x} {y} Td ({}) Tj ET\n", pdf_literal(&head)));
        ops.push_str(&format!("BT /F1 {SIZE} Tf {x} {} Td ({}) Tj ET\n", y - LEADING, pdf_literal(&note)));
        y -= SLOT;
    }
    let mut fonts = Dictionary::new();
    fonts.set("F1", helvetica("Helvetica"));
    fonts.set("F2", helvetica("Helvetica-Bold"));
    let mut resources = Dictionary::new();
    resources.set("Font", fonts);
    let content = doc.add_object(Stream::new(Dictionary::new(), ops.into_bytes()));

    let pages_id = doc.catalog()?.get(b"Pages")?.as_reference()?;
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", pages_id);
    page.set("MediaBox", rect_object(mb));
    page.set("Resources", resources);
    page.set("Contents", content);
    page.set("Annots", Object::Array(vec![Object::Reference(link)]));
    let page_id = doc.add_object(page);
    let pages = doc.get_object_mut(pages_id)?.as_dict_mut()?;
    pages.get_mut(b"Kids")?.as_array_mut()?.push(Object::Reference(page_id));
    let count = pages.get(b"Count")?.as_i64()?;
    pages.set("Count", count + 1);
    Ok(page_id)
}

/// Where the note of each unplaced thread goes on the appended page: in the
/// margin, next to its entry. Threads past the page's end share the last slot.
fn appendix_slots(doc: &Document, page: ObjectId, n: usize) -> Vec<Rect> {
    let mb = media_box(doc, page);
    let top = mb.y1 - MARGIN - HEADER;
    let last = ((top - mb.y0 - MARGIN) / SLOT).floor().max(0.0) as usize;
    (0..n)
        .map(|i| {
            let y = top - SLOT * i.min(last) as f64;
            let x = mb.x0 + MARGIN;
            Rect { x0: x, y0: y - 4.0, x1: x + 16.0, y1: y + 12.0 }
        })
        .collect()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One of the standard 14 fonts, which viewers have without it being embedded.
fn helvetica(name: &str) -> Dictionary {
    let mut f = Dictionary::new();
    f.set("Type", Object::Name(b"Font".to_vec()));
    f.set("Subtype", Object::Name(b"Type1".to_vec()));
    f.set("BaseFont", Object::Name(name.as_bytes().to_vec()));
    f.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    f
}

/// Helvetica's widths of the printable ASCII characters, in thousandths of
/// the font size, from its AFM file.
const HELVETICA_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556,
    556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833,
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556,
    556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334,
    260, 334, 584,
];

/// The width of `s` in Helvetica at `size`. Characters outside ASCII count
/// as wide as a digit.
fn helvetica_width(s: &str, size: f64) -> f64 {
    let units: u32 = s
        .chars()
        .map(|c| match c {
            ' '..='~' => u32::from(HELVETICA_WIDTHS[c as usize - 32]),
            _ => 556,
        })
        .sum();
    f64::from(units) * size / 1000.0
}

/// `s` cut to fit `width` points of Helvetica at `size`.
fn fit(s: &str, width: f64, size: f64) -> String {
    if helvetica_width(s, size) <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    for c in s.chars() {
        if helvetica_width(&format!("{out}{c}…"), size) > width {
            break;
        }
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

/// `s` as a PDF literal string's contents in WinAnsiEncoding, with the
/// characters it lacks replaced by `?`.
fn pdf_literal(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let byte = match c {
            '“' => 0x93,
            '”' => 0x94,
            '‘' => 0x91,
            '’' => 0x92,
            '…' => 0x85,
            '–' => 0x96,
            '—' => 0x97,
            c if (c as u32) < 0x100 => c as u32 as u8,
            _ => b'?',
        };
        match byte {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(byte as char);
            }
            0x20..=0x7E => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:03o}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(quote: &str, prefix: &str, suffix: &str, at: f64) -> Thread {
        Thread {
            id: "a".into(),
            author: "Ada".into(),
            created: "2026-10-07T15:00:54Z".into(),
            body: None,
            replacement: None,
            quote: quote.into(),
            prefix: prefix.into(),
            suffix: suffix.into(),
            at,
            line: None,
            replies: vec![],
        }
    }

    fn key(s: &str) -> Vec<char> {
        key_chars(s).collect()
    }

    #[test]
    fn strips_markup() {
        assert_eq!(strip(r"see \emph{this} \cite[p.~2]{knuth} 50\% % note", Syntax::Latex), "see {this}  50% ");
        assert_eq!(strip("a #emph[b] #link(\"x\")[c] <lbl> // d", Syntax::Typst), "a [b] [c]  ");
        assert_eq!(strip("a [b](http://x) <br/> c", Syntax::Markdown), "a [b  c");
    }

    #[test]
    fn matches_across_line_breaks_hyphens_and_ligatures() {
        let pdf = key("the ef-\nﬁcient Bound is tight.");
        let t = thread("efficient bound", "", "", 0.0);
        let (s, e) = locate(&pdf, &t, Syntax::Plain).unwrap();
        assert_eq!(pdf[s..e].iter().collect::<String>(), "efficientbound");
    }

    #[test]
    fn tells_repeated_quotes_apart_by_context_then_position() {
        let pdf = key("one bound. two bound. three bound.");
        let by_context = thread("bound", "two ", "", 0.0);
        assert_eq!(locate(&pdf, &by_context, Syntax::Plain), Some((11, 16)));
        let by_position = thread("bound", "", "", 0.95);
        assert_eq!(locate(&pdf, &by_position, Syntax::Plain), Some((21, 26)));
    }

    #[test]
    fn matches_ends_when_the_middle_is_missing() {
        let pdf = key("as shown by [3], the bound holds");
        let t = thread(r"as shown by \cite{knoth}, the bound holds", "", "", 0.0);
        // `knoth` is stripped; a missing middle is bridged by the quote's ends.
        assert_eq!(locate(&pdf, &t, Syntax::Latex), Some((0, pdf.len())));
        let t = thread(r"as shown by \textbf{Knuth}, the bound holds", "", "", 0.0);
        assert_eq!(locate(&pdf, &t, Syntax::Latex), Some((0, pdf.len())));
    }

    #[test]
    fn pdf_strings() {
        assert_eq!(text("hi"), Object::String(b"hi".to_vec(), StringFormat::Literal));
        assert_eq!(text("é"), Object::String(vec![0xFE, 0xFF, 0x00, 0xE9], StringFormat::Hexadecimal));
        assert_eq!(date("2026-10-07T15:00:54Z"), Object::String(b"D:20261007150054Z".to_vec(), StringFormat::Literal));
        assert_eq!(pdf_literal("“a(b)” é ☃"), r"\223a\(b\)\224 \351 ?");
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

    #[test]
    fn groups_glyphs_into_lines_and_pages() {
        let g = |page, text: &str, x0, y0| Glyph {
            page,
            text: text.into(),
            rect: Rect { x0, y0, x1: x0 + 5.0, y1: y0 + 10.0 },
        };
        let glyphs = [
            g(1, "a", 100.0, 500.0),
            g(1, "b", 105.0, 500.0),
            g(1, " ", 110.0, 500.0),
            g(1, "c", 72.0, 486.0),
            g(2, "d", 72.0, 700.0),
        ];
        let out = lines(&glyphs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, 1);
        // The space doesn't stretch the first line.
        assert_eq!(out[0].1, vec![Rect { x0: 100.0, y0: 500.0, x1: 110.0, y1: 510.0 }, glyphs[3].rect]);
        assert_eq!(out[1], (2, vec![glyphs[4].rect]));
    }

    #[test]
    fn measures_helvetica() {
        assert_eq!(helvetica_width("Hi!", 10.0), (722.0 + 222.0 + 278.0) / 100.0);
        assert_eq!(fit("short", 100.0, 10.0), "short");
        let cut = fit("a rather long replacement", 60.0, 10.0);
        assert!(cut.ends_with('…') && helvetica_width(&cut, 10.0) <= 60.0, "{cut}");
    }

    #[test]
    fn lays_replacements_out_above_struck_lines() {
        // Two struck lines of 10pt text, the second starting at the margin.
        let line = |x0, y0, x1| Rect { x0, y0, x1, y1: y0 + 10.2 };
        let lines = [line(200.0, 500.0, 300.0), line(72.0, 486.0, 120.0)];
        let runs = above(&lines, 300.0, "it is");
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].x, runs[0].text.as_str()), (200.0, "it is"));
        // Above the first line's glyphs, and smaller than them.
        assert!(runs[0].y > 500.0 + 2.2 + 7.0 && runs[0].size < 6.0, "{runs:?}");

        // What doesn't fit above the first line continues above the next, and
        // the rest is cut short.
        let long = format!("one two {}", "word ".repeat(60));
        let runs = above(&lines, 300.0, &long);
        assert_eq!(runs.len(), 2);
        assert!(runs[0].text.starts_with("one two") && !runs[0].text.ends_with('…'));
        assert_eq!(runs[1].x, 72.0);
        assert!(runs[1].text.ends_with('…'), "{runs:?}");
        for r in &runs {
            assert!(r.x + helvetica_width(&r.text, r.size) <= 300.0 + 1e-9, "{r:?}");
        }
    }

    #[test]
    fn diffs_words_like_the_neovim_plugin() {
        let diff = |a: &str, b: &str| {
            let (del, add) = word_changes(a, b);
            let slice = |s: &str, (i, j): (usize, usize)| s.chars().skip(i).take(j - i).collect::<String>();
            (
                del.into_iter().map(|r| slice(a, r)).collect::<Vec<_>>(),
                add.into_iter().map(|r| slice(b, r)).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            diff("the code solves all three.", "the code addresses all three."),
            (vec!["solves".into()], vec!["addresses".into()])
        );
        // Punctuation is a token of its own.
        assert_eq!(diff("all three.", "all three!"), (vec![".".into()], vec!["!".into()]));
        // Changes only spaces apart are one.
        assert_eq!(diff("a b c d", "a x y d"), (vec!["b c".into()], vec!["x y".into()]));
        assert_eq!(
            diff("prove the old bound", "prove that the new bound"),
            (vec!["old".into()], vec!["that".into(), "new".into()])
        );
        // Nothing in common, or nothing on one side: everything changed, so nothing's marked.
        assert_eq!(diff("one two", "three four"), (vec![], vec![]));
        assert_eq!(diff("one two", ""), (vec![], vec![]));
    }

    #[test]
    fn collapses_whitespace_and_moves_ranges() {
        assert_eq!(collapse("  a\n  bc d ", &[(6, 8), (9, 10)]), ("a bc d".to_owned(), vec![(2, 4), (5, 6)]));
    }

    #[test]
    fn tints_the_changed_part_of_a_run() {
        let runs = above(&[Rect { x0: 100.0, y0: 500.0, x1: 400.0, y1: 510.2 }], 400.0, "a bc d");
        let t = tints(&runs, &[(2, 4)]);
        assert_eq!(t.len(), 1);
        let x0 = 100.0 + helvetica_width("a ", runs[0].size);
        assert!((t[0].x0 - x0).abs() < 1e-9 && (t[0].x1 - x0 - helvetica_width("bc", runs[0].size)).abs() < 1e-9);
    }
}
