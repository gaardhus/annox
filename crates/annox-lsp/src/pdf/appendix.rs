//! The page listing the threads that couldn't be placed on the PDF's text.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use super::export::{contents, Thread};
use super::write::{fit, helvetica, helvetica_width, media_box, one_line, pdf_literal, rect_object};
use super::Rect;

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
pub(super) fn appendix(
    doc: &mut Document,
    threads: &[&Thread],
    source: &str,
    like: ObjectId,
) -> anyhow::Result<ObjectId> {
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
pub(super) fn appendix_slots(doc: &Document, page: ObjectId, n: usize) -> Vec<Rect> {
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
