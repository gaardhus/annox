# 4. Suggestions

A **suggestion** is an annotation that proposes replacing the text of its anchored range with new text. This section defines what a suggestion stores, when it can be applied to a document that has changed since the suggestion was written, what applying it means, and its lifecycle.

Anchors, resolution, and resolution steps are defined in §3. The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## 4.1 Shape

A suggestion is an annotation with `kind: "suggestion"`, an anchor (`target`, §3.5), and an `edit`. Its derived state (§2.5.6) looks like this:

```json
{
  "kind": "suggestion",
  "target": {
    "path": "paper.tex",
    "version": "sha256:…",
    "selectors": {
      "position": { "start": 32, "end": 45 },
      "quote": { "exact": "we prove that", "prefix": "…", "suffix": "…" }
    }
  },
  "edit": { "replacement": "we show that" },
  "status": "open"
}
```

| Field | Type | Meaning |
|---|---|---|
| `edit.replacement` | string | Text that replaces the anchored range. Uses LF line endings, like all normalized text (§3.2). |
| `status` | string | Lifecycle state (§4.4). |

The other fields (id, author, timestamps, and an optional `body` explaining the suggestion) are defined in §2. Discussion happens in replies, in the suggestion's thread.

A suggestion covers exactly one range, and the edit forms follow from that:

| Edit | Anchor | `replacement` |
|---|---|---|
| Replacement | non-empty range | non-empty |
| Deletion | non-empty range | `""` |
| Insertion | point range | non-empty |

`replacement` MUST NOT equal `quote.exact`. A point range with an empty `replacement` is invalid, because it would change nothing. Readers SHOULD ignore invalid suggestions and MAY report them.

**Original text.** While a suggestion is open, `quote.exact` is the text it proposes to replace. Clients render the suggestion as a change from `quote.exact` to `replacement`.

**Base version.** `target.version` is the suggestion's base version: the document version its anchor was last checked against.

Related changes, such as renaming a term in several places, are expressed as several suggestions. v1 has no way to group them ([D33](decisions.md#d33-no-suggestion-grouping-bulk-accept-by-ids-2026-09-29)). A bulk accept takes a list of suggestions (§6.6.2).

## 4.2 Applicability

Whether a suggestion can be applied is **derived** from the current document. It is never stored. To find out, resolve the suggestion's anchor against *D* (§3.7):

| Resolution | Applicability |
|---|---|
| `exact` (step 0 or 1) | **applicable** |
| `relocated` by step 2, 3, or 5 | **applicable** |
| `relocated` by step 4 | **stale** |
| `orphaned` | **stale** |

In steps 0–3 and 5, the resolved range contains exactly `quote.exact`, so the text the suggestion was written against is still intact. For step 5 that text is empty: an insertion whose surrounding text was partly edited still inserts at a position its author saw. In step 4 the whitespace differs, so the suggestion author never saw the text it would now replace. For insertions, which relocate by the point variant of step 4, the surrounding whitespace changed, and it's unclear exactly where in the whitespace the author meant to insert. They are stale for the same reason, and are shown at their relocated position so that they can be re-targeted easily.

Applicability only matters while the suggestion is `open`. Clients SHOULD show stale suggestions differently from applicable ones, and MUST NOT offer to apply a stale suggestion (§4.3).

### 4.2.1 Anchor rewriting for suggestions

§3.8 allows clients to rewrite anchors. For suggestions that is restricted further:

- A client MAY rewrite the anchor of an open suggestion after it is relocated by step 2, 3, or 5. These steps leave `quote.exact` unchanged.
- A client MUST NOT rewrite the anchor of a suggestion after step 4 or after a confirmed suggested location (§3.7.4). Those rewrites change `quote.exact`, and the suggestion would silently become applicable to text its author never saw.

A stale suggestion becomes applicable again only when a user **re-targets** it. That means updating the anchor and reviewing `replacement` in the same action, recorded as a `retarget` event (§2.4).

Any user MAY re-target a suggestion, not only its author. That way a stale suggestion can be rescued, and its thread kept, when its author is unavailable. When the re-targeting author isn't the suggestion's author, clients MUST show who re-targeted it alongside the original author, e.g. "suggested by Ada, re-targeted by Bob" (`retargetedBy`, §2.5.6). Authors re-target their own suggestions routinely in suggestion mode (§4.6), and clients MAY leave those unmarked.

## 4.3 Applying

**Accepting** a suggestion applies it to the document and records the acceptance in a single user action. There is no accepted-but-not-applied state.

To accept an open suggestion, a client MUST:

1. Resolve the anchor against *D*. If the suggestion is stale, stop. It MUST NOT be applied.
2. Let `[c, d)` be the resolved range. Compute the new text `D' = D[0:c] + replacement + D[d:]`.
3. Write *D'* to the document. The client SHOULD keep the file's existing line-ending style, byte order mark, and encoding when writing. This includes converting LF in `replacement` to CRLF if the file uses CRLF.
4. Write a `status` event (§2.4) with `status: "accepted"` and `appliedVersion` set to the version of *D'* (§3.4).

Steps 3 and 4 write two different files and can't be atomic. Clients SHOULD write the document first. If step 4 is lost, the suggestion stays `open` with its text already applied, and it becomes stale because its quote is gone. A client MAY detect this case with an applied-text search (§4.3.3), and offer to mark the suggestion accepted.

**Location confirmation.** Relocation by step 3 can pick a different occurrence than the author intended, if the original was deleted and the same text appears elsewhere. Step 5 relies on partial context in the same way. Accepting one suggestion at a time is safe, because the user accepts it at the location shown to them. A client that accepts many suggestions without showing each one ("accept all") SHOULD apply only suggestions whose resolution is step 0, 1, or 2. It SHOULD leave step-3 and step-5 relocations for individual review, unless the user confirms them. One confirmation for the whole batch is enough, if the client says how many of the suggestions were relocated this way, and each one is shown at its resolved location.

### 4.3.1 Overlapping suggestions

Suggestions are not mutually exclusive, and the spec doesn't track dependencies between them. Once one suggestion is applied, every other open suggestion is resolved against the new text as usual. Suggestions whose quote was changed become stale. Disjoint suggestions stay applicable.

### 4.3.2 Unsaved buffers

A client that applies a suggestion to an editor buffer, rather than the file on disk, uses the buffer's normalized content as *D*. It records the applied version as the version of the buffer content after the edit.

### 4.3.3 Applied-text search and reverting

An **applied-text search** looks for the text a suggestion produces once applied. Find every occurrence *i* of `p + replacement + x` in *D*, where `p` and `x` are the anchor's stored prefix and suffix. If there is at least one, let `c` be the nearest (§3.7.1) of the candidate starts `i + len(p)`. The applied text is `[c, c+len(replacement))`. For an insertion, `replacement` is non-empty, so the pattern is never just `p + x`.

It is used in two places:

- **Lost acceptance** (§4.3): an open suggestion whose applied text is found was probably applied without the `status` event being written.
- **Reverting**: when a status conflict between `accepted` and another status is resolved in favour of the other status (§2.5.4), the document still contains the edit. The client SHOULD offer to revert it. If the applied-text search succeeds, the client replaces `[c, c+len(replacement))` with `quote.exact`. Otherwise the user reverts by hand. The `status` event is written in either case.

## 4.4 Lifecycle

| Status | Meaning | Terminal |
|---|---|---|
| `open` | Awaiting a decision. | no |
| `accepted` | Applied to the document (§4.3). | yes |
| `rejected` | Declined by a reviewer. | no |
| `withdrawn` | Retracted by its author. | no |

Allowed transitions:

```text
open ──accept──▶ accepted
open ──reject──▶ rejected ──reopen──▶ open
open ──withdraw─▶ withdrawn ──reopen──▶ open
```

- `accepted` is terminal. Undoing an accepted suggestion is done by making a new suggestion. The only exception is resolving a concurrent status conflict (§2.5.4, §4.3.3).
- A client SHOULD offer `withdraw` only to the suggestion's author, and `reject` and `accept` to anyone else. annox has no authentication (§1.3), so this is a user-interface convention. A reader MUST NOT treat a status as invalid because of who set it.
- Each transition is a `status` event (§2.4), which records who made it and when.

### 4.4.1 Closed suggestions

Suggestions that are `accepted`, `rejected`, or `withdrawn` are **closed**.

- Clients SHOULD NOT resolve closed suggestions against the current document. After it is applied, an accepted suggestion's quote no longer exists, and that is expected.
- A closed suggestion MUST NOT be presented as orphaned. §3.7.3 applies to open annotations only.
- Clients MUST NOT rewrite the anchors of closed suggestions. The anchor, together with the applied version, records exactly what changed.

## 4.5 Example

Using the document from §3.9 after its edit (`As shown in Section 3, …`), a suggestion on "we prove that" with `replacement: "we show that"` resolves by step 3 to `[41, 54)`. It is applicable, and accepting it produces:

```text
\section{Results}
As shown in Section 3, we show that the bound is tight for all $n \geq 1$.
```

Test vectors are in [`tests/suggestions.json`](tests/suggestions.json).

## 4.6 Suggestion mode (non-normative)

Online editors offer a mode in which typing makes suggestions instead of changing the document. A client can offer the same with the events above, and without protocol support. This section describes the recommended behavior, so that suggestions made this way look the same in every client.

- **The document keeps its text.** The client keeps the text from before the edit as a base. When the user pauses (for example on leaving insert mode), it compares the buffer with the base, turns each changed stretch into a suggestion, and puts the base text back. Suggested text is drawn in place, with deleted text struck through and inserted text after it, but never written to the file. Saving, a crash, or another tool therefore can't turn a suggestion into an edit.
- **Whole words.** A change that starts or ends inside a word is widened to the whole word, so changing `pd` to `pl` suggests `pd` → `pl`, not `d` → `l`. That's easier to read, and a longer quote is more distinctive when the suggestion has to be relocated (§3.7.2, step 3). An insertion between words, such as ` today` after `doing`, stays an insertion.
- **One suggestion per changed stretch.** A change that touches a suggestion the user made in the same session extends it instead of creating a new one: its range grows to cover both, and the replacement is composed. Since the anchor and the replacement change together, this is a `retarget` event (§4.2.1). Typing a sentence in several bursts thus yields one suggestion, and deleting a word next to an insertion yields one replacement.
- **Undo works on suggestions.** Undoing deletes the last suggestion created, or re-targets the last extended one back to its previous range and replacement.
- **Other edits pass through.** Edits that aren't the user's typing, such as accepting a suggestion (§4.3) or reloading the file, become the new base instead of new suggestions.

Suggestion mode needs a client that can draw text that isn't in the buffer. Plain LSP editors (§6.5) can't offer it.
