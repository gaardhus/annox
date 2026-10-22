//! Stripping a source's markup, so its text can be compared with a PDF's.

use super::key_chars;

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

pub(super) fn source_key(s: &str, syntax: Syntax) -> Vec<char> {
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
pub(super) fn strip_indexed(s: &str, syntax: Syntax) -> Vec<(char, usize)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup() {
        assert_eq!(strip(r"see \emph{this} \cite[p.~2]{knuth} 50\% % note", Syntax::Latex), "see {this}  50% ");
        assert_eq!(strip("a #emph[b] #link(\"x\")[c] <lbl> // d", Syntax::Typst), "a [b] [c]  ");
        assert_eq!(strip("a [b](http://x) <br/> c", Syntax::Markdown), "a [b  c");
    }
}
