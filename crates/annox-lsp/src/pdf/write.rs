//! Writing PDF objects, and measuring and writing text in Helvetica.

use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};

use super::Rect;

/// A PDF text string: PDFDocEncoding when it's ASCII, else UTF-16BE.
pub(super) fn text(s: &str) -> Object {
    if s.chars().all(|c| c.is_ascii() && (!c.is_ascii_control() || c == '\n')) {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// An RFC 3339 time as a PDF date, as in `D:20261007150054Z`.
pub(super) fn date(rfc3339: &str) -> Object {
    let digits: String = rfc3339.chars().take(19).filter(char::is_ascii_digit).collect();
    Object::String(format!("D:{digits}Z").into_bytes(), StringFormat::Literal)
}

pub(super) fn color(rgb: [f32; 3]) -> Object {
    Object::Array(rgb.into_iter().map(Object::Real).collect())
}

pub(super) fn rect_object(r: Rect) -> Object {
    Object::Array([r.x0, r.y0, r.x1, r.y1].into_iter().map(|v| Object::Real(v as f32)).collect())
}

pub(super) fn add_annots(doc: &mut Document, page: ObjectId, ids: Vec<ObjectId>) -> anyhow::Result<()> {
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
pub(super) fn media_box(doc: &Document, page: ObjectId) -> Rect {
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

pub(super) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One of the standard 14 fonts, which viewers have without it being embedded.
pub(super) fn helvetica(name: &str) -> Dictionary {
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
pub(super) fn helvetica_width(s: &str, size: f64) -> f64 {
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
pub(super) fn fit(s: &str, width: f64, size: f64) -> String {
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
pub(super) fn pdf_literal(s: &str) -> String {
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

    #[test]
    fn pdf_strings() {
        assert_eq!(text("hi"), Object::String(b"hi".to_vec(), StringFormat::Literal));
        assert_eq!(text("é"), Object::String(vec![0xFE, 0xFF, 0x00, 0xE9], StringFormat::Hexadecimal));
        assert_eq!(date("2026-10-07T15:00:54Z"), Object::String(b"D:20261007150054Z".to_vec(), StringFormat::Literal));
        assert_eq!(pdf_literal("“a(b)” é ☃"), r"\223a\(b\)\224 \351 ?");
    }

    #[test]
    fn measures_helvetica() {
        assert_eq!(helvetica_width("Hi!", 10.0), (722.0 + 222.0 + 278.0) / 100.0);
        assert_eq!(fit("short", 100.0, 10.0), "short");
        let cut = fit("a rather long replacement", 60.0, 10.0);
        assert!(cut.ends_with('…') && helvetica_width(&cut, 10.0) <= 60.0, "{cut}");
    }
}
