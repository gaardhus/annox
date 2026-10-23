# PDF viewer quirks

What PDF viewers and libraries do with the comments `export-pdf` writes and `import-pdf`
reads, that the PDF specification doesn't lead you to expect, and what annox does about
each. Read it before changing how annox writes or reads PDF comments: most of what looks
odd in [`pdf/`](../crates/annox-lsp/src/pdf/) is one of these. Each entry names where the
workaround lives, which has a comment saying why. [`testing/pdf-sync.md`](testing/pdf-sync.md)
says how to check them by hand.

## Okular, and other viewers built on poppler

- **A caret draws nothing without an appearance stream**, so a suggestion would show only
  as struck text. Zathura and Evince do the same. A suggestion's caret has an appearance
  that writes the replacement above the struck text (`annotate` and `draw` in
  `pdf/export.rs`).
- **A comment that anything points at with `/IRT` won't open.** That rules out replies as
  annotations of their own, and grouping a suggestion's caret with its strikeout the way
  Acrobat does (`/IRT /RT /Group`). Replies are written into their thread's note instead
  (`contents` in `pdf/export.rs`), and annox knows a caret by its name, the strikeout's with
  `-caret` (`annotate`).
- **Edits to the comments of a PDF whose cross-reference table skips object numbers are
  dropped when saving**, silently. An export that replaces earlier output leaves gaps, so
  objects are renumbered before saving (`save` in `pdf/export.rs`).
- **A new highlight's note is filled with the text it covers.** It says nothing the quote
  doesn't, so it's imported as a highlight without a note, unless annox wrote that note
  (`import` in `pdf/import.rs`).
- **A viewer can't reply to a comment.** A reviewer answers by adding to its note instead.
  Each note annox writes keeps a copy under `/AnnoxContents`, and what's added at the end
  is imported as a reply; other changes to the note are skipped (`set_note` in
  `pdf/export.rs`, `added_to_note` in `cli/pdf.rs`).
- **Okular reloads a PDF as soon as it changes**, so annox writes it through a file next
  to it and renames that, and Okular never reads it half written (`save`).

## Firefox, and other browsers

- **Firefox lists and edits only notes that have a `/Popup`.** Each note gets a closed one,
  in the page's right margin level with its text (`add_popup` in `pdf/export.rs`).
- **Browsers don't show replies that are annotations of their own**, another reason they
  go in their thread's note.
- **Firefox saves a copy** rather than into the file, so `sync-pdf --watch` doesn't see its
  edits until the copy is saved over the original.

## Acrobat

- **A suggestion is listed as two comments**, the strikeout and the caret, as they aren't
  grouped (see Okular above).

## PyMuPDF

- **Comments are numbered afresh in each session, and may have no time**, so a comment's
  name and time don't tell it apart from one imported before; a new one could pass for an
  imported one and be deleted by the next sync. Its page and place are part of what
  identifies it (`identity` in `pdf/import.rs`).

## Libraries annox uses

- **pdf-extract misreads CID font widths written as ranges**, which Typst writes, and
  places every glyph after a string's first in the wrong place. They're rewritten as
  lists on a copy of the document before extraction (`expand_cid_widths` in
  `pdf/mod.rs`).
- **lopdf drops tabs and line breaks from strings in PDFDocEncoding**, so a note of
  several paragraphs would run together. Such strings are decoded by annox
  (`decode_text` in `pdf/mod.rs`).
- **lopdf doesn't write the input's own cross-reference streams**, which leaves gaps in the
  object numbers too (see Okular above).

## Tried and dropped

- **Replies as `/IRT` annotations, hidden with the Hidden flag.** Poppler drew them as a
  note icon anyway, and some comment panels leave hidden comments out.
- **Replies with an empty appearance, at the top left of their thread's text.** Okular
  shows each as a note regardless, which takes clicks on an icon's worth of the page, so
  the start of the thread's highlight couldn't be clicked. Moving them to the margin
  fixed that, but not the `/IRT` problems above, which is why replies now go in the note.
