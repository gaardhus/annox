//! Importing the comments reviewers made on a PDF in their viewer, placed on
//! the source the PDF was compiled from.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context};
use lopdf::{Dictionary, Document, Object, ObjectId};

use super::layout::same_word;
use super::source::{strip_indexed, Syntax};
use super::{best_match, extract, glyph_key, key_chars, Glyph, Rect};

/// A comment, suggestion or reply found in a PDF.
#[derive(Debug, PartialEq)]
pub struct Found {
    /// The PDF annotation's name (`/NM`), or one made up from where it is.
    pub name: String,
    pub page: u32,
    /// The name of whoever wrote it (`/T`).
    pub author: Option<String>,
    /// When it was made, in RFC 3339.
    pub time: Option<String>,
    pub body: Option<String>,
    pub what: What,
}

#[derive(Debug, PartialEq)]
pub enum What {
    /// A comment, or a suggestion to replace its text with `replacement`, on
    /// the source's `range`, or `None` when its text isn't in the source.
    /// `quote` is the PDF's text it marks.
    Root { range: Option<(usize, usize)>, quote: String, replacement: Option<String> },
    /// A reply to the annotation `parent`.
    Reply { parent: Parent },
}

/// What a reply is to: the annotation's name, and when it was made.
#[derive(Debug, PartialEq)]
pub struct Parent {
    pub name: String,
    pub time: Option<String>,
}

/// How many letters and digits around a mark [`import`] compares, to tell
/// repeated text apart.
const CONTEXT: usize = 48;

/// The kinds of annotation [`import`] reads.
const MARKUP: [&[u8]; 7] = [b"Highlight", b"Underline", b"Squiggly", b"StrikeOut", b"Caret", b"Text", b"FreeText"];

/// An annotation of the PDF, as read from its dictionary.
struct Raw {
    id: Option<ObjectId>,
    page: u32,
    subtype: Vec<u8>,
    name: String,
    author: Option<String>,
    time: Option<String>,
    contents: Option<String>,
    rect: Option<Rect>,
    quads: Vec<Rect>,
    /// What it's in reply to (`/IRT`), and whether it's grouped with it
    /// instead (`/RT /Group`).
    irt: Option<ObjectId>,
    grouped: bool,
}

/// Reads the comments, suggestions and replies of `pdf`, a PDF compiled from
/// `source`, written in `syntax`, or a PDF whose text `source` is. Each mark
/// on the text is placed in the source by finding the PDF text it covers, as
/// `export` does the other way round: a highlight, underline or squiggly is a
/// comment; struck text a suggestion to delete it, or with a caret grouped
/// with it to replace it with the caret's text; a caret alone a suggestion to
/// insert its text; and a note a comment on the word nearest to it. An
/// annotation in reply to another is a reply. Review states, links, form
/// fields and the like are left out.
pub fn import(pdf: &Path, source: &str, syntax: Syntax) -> anyhow::Result<Vec<Found>> {
    let doc = Document::load(pdf).with_context(|| format!("can't read {}", pdf.display()))?;
    if doc.is_encrypted() {
        bail!("{} is encrypted", pdf.display());
    }
    let glyphs = extract(&doc)?;
    let (gkey, gowner) = glyph_key(&glyphs);
    let (skey, sowner): (Vec<char>, Vec<usize>) = strip_indexed(source, syntax)
        .into_iter()
        .flat_map(|(c, i)| key_chars(&c.to_string()).map(move |k| (k, i)).collect::<Vec<_>>())
        .unzip();
    // The source range of the glyphs `first..=last`.
    let place = |first: usize, last: usize| -> Option<(usize, usize)> {
        let ks = gowner.partition_point(|&o| o < first);
        let ke = gowner.partition_point(|&o| o <= last);
        let prefix = &gkey[ks.saturating_sub(CONTEXT)..ks];
        let suffix = &gkey[ke..(ke + CONTEXT).min(gkey.len())];
        let at = ks as f64 / gkey.len().max(1) as f64;
        let (s, e) = best_match(&skey, gkey.get(ks..ke)?, prefix, suffix, at)?;
        Some((sowner[s], sowner[e - 1] + 1))
    };
    let shown = |first: usize, last: usize| -> String {
        let mut s = String::new();
        for i in first..=last {
            if i > first && !same_word(&glyphs[i - 1], &glyphs[i]) && !s.ends_with(' ') {
                s.push(' ');
            }
            s.push_str(glyphs[i].text.trim());
        }
        s
    };

    let raws = annotations(&doc);
    let by_id: BTreeMap<ObjectId, usize> = raws.iter().enumerate().filter_map(|(i, r)| Some((r.id?, i))).collect();
    // The annotation a reply or grouped one belongs to, following `/IRT`.
    let root = |mut i: usize| -> usize {
        for _ in 0..raws.len() {
            match raws[i].irt.and_then(|id| by_id.get(&id)) {
                Some(&parent) => i = parent,
                None => break,
            }
        }
        i
    };
    let mut out = Vec::new();
    for (i, raw) in raws.iter().enumerate() {
        let found = |what| Found {
            name: raw.name.clone(),
            page: raw.page,
            author: raw.author.clone(),
            time: raw.time.clone(),
            body: raw.contents.clone().filter(|c| !c.trim().is_empty()),
            what,
        };
        if let Some(irt) = raw.irt {
            if !raw.grouped && by_id.contains_key(&irt) {
                let parent = &raws[root(i)];
                out.push(found(What::Reply {
                    parent: Parent { name: parent.name.clone(), time: parent.time.clone() },
                }));
            }
            continue;
        }
        let group: Vec<&Raw> =
            raws.iter().enumerate().filter(|&(j, r)| j != i && r.grouped && root(j) == i).map(|(_, r)| r).collect();
        let caret = std::iter::once(raw).chain(group.iter().copied()).find(|r| r.subtype == b"Caret");
        let struck = std::iter::once(raw).chain(group.iter().copied()).find(|r| r.subtype == b"StrikeOut");
        let on_page = |g: &Glyph| g.page == raw.page && !g.text.trim().is_empty();
        let f = match (raw.subtype.as_slice(), struck, caret) {
            (_, Some(struck), caret) => {
                let Some((first, last)) = covered(&glyphs, raw.page, &struck.quads) else { continue };
                let replacement = caret.and_then(|c| c.contents.clone()).unwrap_or_default();
                let mut f = found(What::Root {
                    range: place(first, last),
                    quote: shown(first, last),
                    replacement: Some(replacement),
                });
                // The caret's text is the replacement, so the note is the
                // struck text's.
                f.body = struck.contents.clone().filter(|c| !c.trim().is_empty());
                f
            }
            (b"Caret", None, _) => {
                let Some(rect) = raw.rect else { continue };
                let Some((g, after)) = insertion_point(&glyphs, raw.page, rect) else { continue };
                let (first, last) = word_around(&glyphs, g);
                let range = place(first, last).map(|(s, e)| if after { (e, e) } else { (s, s) });
                let mut f = found(What::Root { range, quote: String::new(), replacement: raw.contents.clone() });
                f.body = None;
                f
            }
            (b"Text" | b"FreeText", None, _) => {
                let Some(rect) = raw.rect else { continue };
                if raw.contents.as_deref().is_none_or(|c| c.trim().is_empty()) {
                    continue;
                }
                let corner = (rect.x0, rect.y1);
                let distance = |g: &Glyph| {
                    let dx = (g.rect.x0 - corner.0).max(corner.0 - g.rect.x1).max(0.0);
                    let dy = (g.rect.y0 - corner.1).max(corner.1 - g.rect.y1).max(0.0);
                    dx * dx + dy * dy
                };
                let nearest = (0..glyphs.len())
                    .filter(|&j| on_page(&glyphs[j]))
                    .min_by(|&a, &b| distance(&glyphs[a]).total_cmp(&distance(&glyphs[b])));
                let Some(g) = nearest else { continue };
                let (first, last) = word_around(&glyphs, g);
                found(What::Root { range: place(first, last), quote: shown(first, last), replacement: None })
            }
            _ => {
                let Some((first, last)) = covered(&glyphs, raw.page, &raw.quads) else { continue };
                found(What::Root { range: place(first, last), quote: shown(first, last), replacement: None })
            }
        };
        out.push(f);
    }
    Ok(out)
}

/// The annotations of `doc` that [`import`] reads, page by page.
fn annotations(doc: &Document) -> Vec<Raw> {
    let string = |d: &Dictionary, key: &[u8]| {
        d.get(key).ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| lopdf::decode_text_string(o).ok())
    };
    let numbers = |d: &Dictionary, key: &[u8]| -> Vec<f64> {
        let Ok((_, Object::Array(a))) = d.get(key).and_then(|o| doc.dereference(o)) else { return Vec::new() };
        a.iter().filter_map(|o| o.as_float().ok()).map(f64::from).collect()
    };
    let bounds = |v: &[f64]| {
        let (xs, ys): (Vec<f64>, Vec<f64>) = v.chunks(2).map(|p| (p[0], p[1])).unzip();
        let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Rect { x0: min(&xs), y0: min(&ys), x1: max(&xs), y1: max(&ys) }
    };
    let mut out = Vec::new();
    for (page, page_id) in doc.get_pages() {
        let Ok(page_dict) = doc.get_dictionary(page_id) else { continue };
        let Ok((_, Object::Array(annots))) = page_dict.get(b"Annots").and_then(|o| doc.dereference(o)) else {
            continue;
        };
        for annot in annots {
            let id = annot.as_reference().ok();
            let Ok((_, Object::Dictionary(d))) = doc.dereference(annot) else { continue };
            let Ok(subtype) = d.get(b"Subtype").and_then(Object::as_name) else { continue };
            // A review state, such as "Accepted", is a reply with a /State.
            if !MARKUP.contains(&subtype) || d.has(b"State") {
                continue;
            }
            let rect = numbers(d, b"Rect");
            let rect = (rect.len() == 4).then(|| bounds(&rect));
            let quads: Vec<Rect> = numbers(d, b"QuadPoints").as_chunks::<8>().0.iter().map(|q| bounds(q)).collect();
            let name = string(d, b"NM").filter(|n| !n.is_empty()).unwrap_or_else(|| {
                let r = rect.unwrap_or(Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 });
                format!("page{page}-{}-{:.0}-{:.0}", String::from_utf8_lossy(subtype), r.x0, r.y0)
            });
            let time = string(d, b"CreationDate").or_else(|| string(d, b"M")).and_then(|t| rfc3339_of(&t));
            out.push(Raw {
                id,
                page,
                subtype: subtype.to_vec(),
                name,
                author: string(d, b"T").filter(|t| !t.trim().is_empty()),
                time,
                contents: string(d, b"Contents").map(|c| c.replace("\r\n", "\n").replace('\r', "\n")),
                rect,
                quads,
                irt: d.get(b"IRT").and_then(Object::as_reference).ok(),
                grouped: d.get(b"RT").and_then(Object::as_name).is_ok_and(|rt| rt == b"Group"),
            });
        }
    }
    out
}

/// The first and last glyph on `page` whose middle is in one of `quads`.
fn covered(glyphs: &[Glyph], page: u32, quads: &[Rect]) -> Option<(usize, usize)> {
    let inside = |g: &Glyph| {
        let (x, y) = ((g.rect.x0 + g.rect.x1) / 2.0, (g.rect.y0 + g.rect.y1) / 2.0);
        quads.iter().any(|q| x >= q.x0 && x <= q.x1 && y >= q.y0 && y <= q.y1)
    };
    let mut hits = (0..glyphs.len())
        .filter(|&i| glyphs[i].page == page && !glyphs[i].text.trim().is_empty())
        .filter(|&i| inside(&glyphs[i]));
    let first = hits.next()?;
    Some((first, hits.next_back().unwrap_or(first)))
}

/// Where a caret at `rect` on `page` inserts: the glyph on its line whose
/// edge is nearest, and whether it's after that glyph rather than before.
fn insertion_point(glyphs: &[Glyph], page: u32, rect: Rect) -> Option<(usize, bool)> {
    let (x, y) = ((rect.x0 + rect.x1) / 2.0, (rect.y0 + rect.y1) / 2.0);
    let on_line = |g: &Glyph| {
        g.page == page && !g.text.trim().is_empty() && y >= g.rect.y0 - g.rect.height() / 2.0 && y <= g.rect.y1
    };
    (0..glyphs.len())
        .filter(|&i| on_line(&glyphs[i]))
        .flat_map(|i| [(i, false, (glyphs[i].rect.x0 - x).abs()), (i, true, (glyphs[i].rect.x1 - x).abs())])
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, after, _)| (i, after))
}

/// The first and last glyph of the word glyph `i` is in.
fn word_around(glyphs: &[Glyph], i: usize) -> (usize, usize) {
    let (mut first, mut last) = (i, i);
    while first > 0 && same_word(&glyphs[first - 1], &glyphs[first]) {
        first -= 1;
    }
    while last + 1 < glyphs.len() && same_word(&glyphs[last], &glyphs[last + 1]) {
        last += 1;
    }
    (first, last)
}

/// A PDF date, as in `D:20261007170054+02'00'`, in RFC 3339 and UTC.
fn rfc3339_of(date: &str) -> Option<String> {
    let s = date.strip_prefix("D:").unwrap_or(date);
    let digits = |from: usize, len: usize, default: i64| -> Option<i64> {
        match s.get(from..from + len) {
            Some(d) if d.chars().all(|c| c.is_ascii_digit()) => d.parse().ok(),
            Some(_) => None,
            None => Some(default),
        }
    };
    let year = digits(0, 4, -1).filter(|&y| y >= 0)?;
    let (month, day) = (digits(4, 2, 1)?, digits(6, 2, 1)?);
    let (hour, minute, second) = (digits(8, 2, 0)?, digits(10, 2, 0)?, digits(12, 2, 0)?);
    let offset = match s.get(14..15) {
        Some(sign @ ("+" | "-")) => {
            let rest: String = s[15..].chars().filter(char::is_ascii_digit).collect();
            let hours: i64 = rest.get(0..2).and_then(|h| h.parse().ok()).unwrap_or(0);
            let minutes: i64 = rest.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
            (hours * 3600 + minutes * 60) * if sign == "+" { 1 } else { -1 }
        }
        _ => 0,
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(annox_core::event::rfc3339(days * 86_400 + hour * 3600 + minute * 60 + second - offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pdf_dates() {
        assert_eq!(rfc3339_of("D:20261007170054+02'00'").as_deref(), Some("2026-10-07T15:00:54Z"));
        assert_eq!(rfc3339_of("D:20261007150054Z").as_deref(), Some("2026-10-07T15:00:54Z"));
        assert_eq!(rfc3339_of("D:20260101003000-01'30").as_deref(), Some("2026-01-01T02:00:00Z"));
        assert_eq!(rfc3339_of("D:2026").as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(rfc3339_of("D:20261332"), None);
        assert_eq!(rfc3339_of("yesterday"), None);
    }
}
