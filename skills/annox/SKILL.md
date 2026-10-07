---
name: annox
description: Read and write annox annotations — review comments and suggested edits stored in a project's `.annox/` directory. Use when a project has an `.annox/` directory and the user asks you to review, proofread, or give feedback on a document, to address or answer review comments, or to propose edits for them to accept rather than editing the file directly. Also use when the user mentions annox, annotations, suggestions, or review threads on a file.
---

# annox

annox stores comments and suggested edits beside the files they're about, in `.annox/`. The user reviews them in their editor: they see your comments in context, and accept or reject each suggestion. Use it whenever the user should decide on a change, not you.

Use the `annox` CLI through the shell. Every command prints JSON. If the `annox` MCP server is connected, its tools (`list_annotations`, `comment`, `suggest`, …) do the same things with the same arguments; use whichever is available.

## Identity

Write as yourself, not as the user, so your annotations are distinguishable from theirs. Use `urn:agent:` followed by your harness's name in lowercase, and the same id every time, so all your annotations share one author: `urn:agent:claude` for Claude Code, `urn:agent:codex`, `urn:agent:gemini`, `urn:agent:copilot`, `urn:agent:cursor`, `urn:agent:opencode`. Set it once per shell command, or export it:

```sh
export ANNOX_AUTHOR=urn:agent:claude ANNOX_AUTHOR_NAME=Claude
```

The MCP server may already be started with an identity; then you don't need to set one.

## Reading

```sh
annox list                  # open annotations in every document
annox list paper.md         # one file
annox list --all            # also resolved, accepted, rejected, withdrawn, deleted
annox list --closed         # also resolved, accepted, rejected, withdrawn, but not deleted
annox show ID               # one annotation and its thread
annox history ID --json     # who did what to a thread (replies included), and when
annox report --json         # counts per document: open, closed, orphaned, stale
```

Filter instead of reading everything. Filters combine:

```sh
annox list --kind comment --others   # review comments from other people
annox list --mine --kind suggestion  # your own suggestions
annox list --broken                  # orphaned, or suggestions that can't be applied
annox list --status resolved         # only these statuses (comma-separated)
annox list --author ID / --not-author ID
```

`--mine` and `--others` use your identity (see above).

Each entry has `id`, `path`, `kind` (`comment` or `suggestion`), `status`, `author`, `body` (null for a highlight), `quote` (the text it's attached to, as it reads now), `line`, `resolution`, and `replies`. Suggestions also have `replacement` and `applicable`, and `reverts` if they undo an accepted suggestion.

- `resolution: "orphaned"` means the quoted text is gone from the file. Fix it with `reattach` or `retarget` (below), or tell the user. If it has `suggested` (where the text probably went, as `line`, `quote`, and `score`: the share of its words found there), check that the suggested quote is the same passage reworded, then use `--suggested`. When it's unclear, ask the user rather than guessing.
- `applicable: false` means the suggestion can't be applied as is. Use `retarget` to fix it.

## Writing

You target text by **quoting it exactly** as it appears in the file, including punctuation and markup. No line numbers or offsets.

```sh
annox comment paper.md --quote "the bound is tight" --body "Is this proved? Cite Lemma 4."
annox highlight paper.md --quote "the bound is tight"   # a comment with no body
annox suggest paper.md --quote "teh bound" --replace "the bound" --body "Typo."
annox reply ID --body "Done in §3."
annox status ID resolved            # comments: open | resolved
annox status ID withdrawn           # suggestions: open | rejected | withdrawn
annox edit ID --body "…"            # change your own comment
annox retarget ID --quote "new text" --replace "…"   # move a suggestion to new text
annox reattach ID --quote "new text"                 # move a comment to new text
annox reattach ID --suggested                        # ...to its suggested location
annox delete ID / annox restore ID
```

- If the quote occurs more than once, the command fails and lists the lines. Quote a longer passage so it's unique. Use `--occurrence N` (1-based) only when longer text wouldn't be unique either.
- Quotes may span lines. Use real newlines, not `\n`.
- `--body` is Markdown. Write ≈ or "about" rather than `~`, since two single tildes strike through the text between them.
- `--local` keeps an annotation private to this machine, as a draft. Use it only if the user asks.

## How to work

- **Suggest, don't edit, when the user owns the text.** For prose, papers, and docs under review, make suggestions instead of editing the file. Edit the file directly only if the user asks you to.
- **Make one suggestion per logical change.** Then the user can accept some and reject others. Split unrelated fixes in the same sentence or paragraph into separate suggestions.
- **Quote as little as possible.** Quote only the words that change, plus a word or two on either side if that's needed to make the quote unique. annox records the surrounding text itself, so you don't need extra context to keep the suggestion anchored. Start short: if the quote isn't unique, the command fails and lists the matching lines, and only then do you lengthen it. The user sees unchanged text in the quote as deleted and re-added, which hides the actual change. To fix one word, quote a few words, not the sentence. To change one sentence, quote that sentence, not the paragraph.
- **Say why** in `--body` whenever the reason isn't obvious from the change.
- **Use comments for questions and for problems you can't fix yourself.** Use suggestions for concrete replacement text.
- **Answer review comments in their thread.** When the user asks you to address comments, run `annox list --kind comment --others`, then handle each one. Either make the change (as a suggestion, or as a direct edit if asked) and `reply` saying what you did, or `reply` explaining why not. Resolve a comment (`annox status ID resolved`) only when you've fully addressed it and it's addressed to you.
- **Don't accept suggestions unless the user asks you to.** `annox accept ID` writes the change into the file and closes the suggestion for good. If it says the suggestion was relocated, check the text at the reported line before passing `--confirmed`. To undo an accepted suggestion, `annox revert ID` suggests restoring the original text; add `--accept` only if the user asks you to undo it outright.
- **Don't commit unless the user asks.** `annox commit` commits the annotation files under `.annox/` and nothing else.
- **Don't change other people's annotations.** Don't edit, withdraw, or delete them. Reply instead.
- **Suggestions you make go stale if you then edit the same text yourself.** Finish your direct edits first, or check `annox list --mine --broken` afterwards and retarget what it lists.

## Setup

If a command says the file isn't in an annox workspace, ask the user before running `annox init` at the project root. It creates `.annox/`, which is meant to be committed; `annox init --local` keeps it out of git instead.
