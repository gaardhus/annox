//! Exporting a document's threads into a PDF compiled from it, as native PDF
//! annotations, for reviewers who read the PDF instead of the source.
//!
//! Each thread is placed by finding its quote in the PDF's text. Threads that
//! can't be placed go on a page appended to the end.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use super::appendix::{appendix, appendix_slots};
use super::source::{source_key, strip_indexed, Syntax};
use super::words::{collapse, word_changes};
use super::write::{add_annots, color, date, fit, helvetica, helvetica_width, pdf_literal, rect_object, text};
use super::{best_match, extract, glyph_key, key_chars, Glyph, Rect};

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

/// The range of `key` that holds `thread`'s quote, if it can be found.
fn locate(key: &[char], thread: &Thread, syntax: Syntax) -> Option<(usize, usize)> {
    let quote = source_key(&thread.quote, syntax);
    let prefix = source_key(&thread.prefix, syntax);
    let suffix = source_key(&thread.suffix, syntax);
    best_match(key, &quote, &prefix, &suffix, thread.at)
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

/// The note a thread's root shows: its body, and for a suggestion whose
/// change isn't drawn in full on the page, what it replaces the quote with.
pub(super) fn contents(thread: &Thread, with_quote: bool, with_replacement: bool) -> Option<String> {
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
    fn tints_the_changed_part_of_a_run() {
        let runs = above(&[Rect { x0: 100.0, y0: 500.0, x1: 400.0, y1: 510.2 }], 400.0, "a bc d");
        let t = tints(&runs, &[(2, 4)]);
        assert_eq!(t.len(), 1);
        let x0 = 100.0 + helvetica_width("a ", runs[0].size);
        assert!((t[0].x0 - x0).abs() < 1e-9 && (t[0].x1 - x0 - helvetica_width("bc", runs[0].size)).abs() < 1e-9);
    }
}
