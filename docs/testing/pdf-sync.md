# Testing PDF sync by hand

What to try after changing `export-pdf`, `import-pdf` or `sync-pdf`. The automated tests
check what annox writes; these steps check what PDF viewers make of it, which only a
person can. They need Okular, which saves comments into the file and reloads it when it
changes, and Firefox, whose viewer saves a copy.

## Setup

```sh
just pdf-playground          # or: just pdf-playground ~/somewhere
```

This builds the debug binary and makes `/tmp/annox-pdf-playground`, replacing an earlier
one:

| Folder          | What it is                                                                |
| --------------- | ------------------------------------------------------------------------- |
| `hub/`          | The hub's data                                                            |
| `sam/`, `jane/` | Two copies of `examples/proposal.pdf` that sync through the hub           |
| `okular-check/` | A copy on its own, no hub, with four open threads written into its PDF    |

In every terminal, `cd` into a copy and run `source env.sh`. It sets who you are there
and puts the debug `annox` first on the PATH. `threads` then prints that copy's threads.
To start over, close the PDFs, stop everything, and run `just pdf-playground` again.

## One copy, no hub (`okular-check/`)

Start with `annox sync-pdf proposal.pdf --watch` in one terminal, and `okular proposal.pdf`.

1. **The threads show up.** Okular's Reviews panel (F7) lists four: "Too short?" with
   Jane's reply in the same note, "Two things here." with its blank line, the strikeout
   on "one month" with "two months" above it, and a highlight without a note. The
   resolved "40 seats" thread isn't there. Each opens on double-click, and no note icons
   sit in the margin. In Firefox, the Comments panel lists the three with notes, and they
   can be edited.
2. **A line added to a note becomes a reply.** Add a line at the end of the "about a week"
   note and press Ctrl+S. The watcher prints that it imported a reply, Okular reloads,
   and the note ends with "You, DATE: your line". `threads` shows it once.
3. **Saving again works.** Repeat step 2 twice more. Each line arrives once, and Okular
   shows no "Some errors were found in the document" banner.
4. **A reworded note isn't imported.** Change "Too short?" to something else and save. The
   watcher lists it under "skipped", and after the reload the note reads "Too short?"
   again.
5. **New comments become threads.** Highlight a sentence with the annotation toolbar (F6),
   and put a sticky note somewhere, then save. Both become threads by the author name set
   in Okular. The highlight comes back without a note: Okular fills a new highlight's note
   with its text, which annox drops. The sticky note comes back as a highlight on the
   nearest word.
6. **Unsaved changes survive a rewrite.** Add a line to a note but don't save. In another
   terminal, `annox reply ID --body "From the CLI."` with an id from `threads`. Okular
   asks whether to reload: choose **Abort Reloading**, press Ctrl+S, and choose
   **Continue** when it warns the file changed. Both replies end up in `threads` and in
   the note, once each.
7. **Firefox edits come in too.** Open the PDF in Firefox, add a line to a note, and save
   it over the original. Without the watcher, `annox sync-pdf proposal.pdf` imports it as
   a reply.

## Two copies and a hub (`sam/`, `jane/`)

Run `annox hub --data hub --verbose` in the playground folder, after `source sam/env.sh`
for the PATH. Then, in a terminal for each of `sam/` and `jane/`,
`annox sync-pdf proposal.pdf --watch`, and open both PDFs in Okular.

1. **The CLI reaches its own PDF.** In `sam/`, `annox comment proposal.pdf --quote "about a
   week" --body "Too short?"`. It shows up in Sam's PDF.
2. **The hub carries it to the other PDF.** It shows up in Jane's PDF too, and the hub
   logs the push.
3. **A comment made in a viewer reaches everyone.** Highlight something in Jane's PDF,
   add a note, and save. It shows up in Sam's PDF and in `threads` in both.
4. **Replies go both ways.** Answer in Sam's PDF by adding a line to Jane's note, and from
   the CLI in `jane/` with `annox reply`. Both replies end up in both notes.
5. **Nothing doubles.** After all that, `threads` in both copies lists each thread and
   reply once, and so does each PDF.
6. **The hub can go away.** Stop the hub, comment in both copies, then start it again.
   Both copies end up with both comments.

## When something's wrong

Note what the viewer showed, word for word, and keep the watcher's output. These show
what annox read and what it has:

```sh
annox text proposal.pdf                   # the PDF's text, as annox reads it
annox import-pdf proposal.pdf --dry-run   # what the next sync would import, and skip
threads --json                            # the threads, in full
```

To look at the PDF itself, `uv run --with pymupdf python` and
`pymupdf.open("proposal.pdf")[0].annots()` lists its annotations.

## Known limits

- An insertion, a suggestion that replaces nothing, goes on a page at the end.
- Deleting a comment in the viewer doesn't delete it: the next sync writes it back.
- Review states set in a viewer, such as "Accepted", are ignored.
- An underline or squiggly comes back as a highlight.
- Acrobat lists a suggestion's strikeout and replacement as two comments.
