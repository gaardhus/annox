# Design decisions

Each entry records a decision, when it was made, and why. Superseded decisions stay here and are marked as superseded.

## D1. v1 targets plain-text documents (2026-09-28)

v1 covers documents whose content is a sequence of Unicode characters: source code, Markdown, LaTeX, Typst, and similar. Rich formats (`.docx`, notebooks, PDF) are out of scope for v1.

**Why:** character-range anchoring is tractable and well understood. Rich formats need format-specific selectors, which would greatly widen the surface of v1. The anchor design should leave room to add selector types later.

## D2. Sidecar files are the primary storage model (2026-09-28)

Annotations are stored in files alongside the source and can be committed to version control. A server and protocol are optional layers on top of the files, not a requirement.

**Why:** this works offline and with no infrastructure, and it fits git-based workflows. Any tool can take part just by reading files.

## D3. Independent of the W3C Web Annotation model (2026-09-28)

annox defines its own plain-JSON format. It is not a profile of W3C Web Annotation, uses no JSON-LD, and publishes no official mapping.

**Why:** W3C Web Annotation has little uptake in editors. JSON-LD is a burden for implementers, and the model has no concept of suggestions, document versions, or re-anchoring. We borrow one idea, the text-quote selector (exact text plus prefix/suffix context), and credit it.

## D4. The spec is Markdown in this repository (2026-09-28)

The spec lives in `spec/` as numbered Markdown sections. A JSON Schema will be added once the data model stabilizes.

## D5. Offsets are code points over normalized text (2026-09-28)

Offsets count Unicode code points over text with the BOM stripped and line endings normalized to LF. The protocol may negotiate other units on the wire. See §3.2 and §3.3.

**Why:** this unit doesn't depend on how the file is encoded, and CRLF↔LF conversion doesn't shift offsets. It avoids the UTF-16 default that LSP had to work around later.

## D6. Re-anchoring is exact and deterministic; fuzzy matching is advisory only (2026-09-28)

The normative algorithm runs in stages: version match, position check, context search, quote search, whitespace-insensitive quote search, then orphaned. A quote that occurs more than once is relocated only if its context score picks out a single occurrence. Fuzzy heuristics may only produce suggested locations, which the user must confirm. See §3.7.

**Why:** every conforming implementation must agree on where an annotation is. Fuzzy algorithms can't be specified tightly enough to guarantee that.

## D7. Anchors record a content-hash version (2026-09-28)

The version is `sha256:` plus the hex digest of the normalized text. It doesn't depend on git. See §3.4.

**Why:** if the version matches, the offsets are trusted without any search. It also works with uncommitted edits and outside of git.

## D8. Point ranges are allowed (2026-09-28)

`start == end` is valid. Point anchors are resolved using the position and the prefix/suffix context only.

**Why:** they're needed for insertion suggestions and "comment here" markers.

## D9. Anchors are rewritten only after exact resolution or user confirmation (2026-09-28)

Clients may rewrite a stored anchor after an `exact` or `relocated` result, or after the user confirms a suggested location. Orphaned anchors are never modified or deleted automatically. See §3.8.

**Why:** this prevents silent drift when clients use different heuristics.

## D10. A suggestion is applicable if its quote resolves exactly (2026-09-28)

Applicability is derived, never stored. A suggestion is applicable if its anchor resolves by steps 0–3. After step 4 or when orphaned, it is stale. Anchors of suggestions are never rewritten in a way that changes `quote.exact`. A stale suggestion needs a user to re-target it. See §4.2.

**Why:** in steps 0–3 the resolved range contains exactly the text the author saw, so the suggestion's meaning is intact. Overlapping suggestions go stale on their own once one is applied, with no dependency tracking.

## D11. Accepting a suggestion applies it (2026-09-28)

Accepting a suggestion edits the document and records `accepted` with the applied version in one user action. `accepted` is terminal. `rejected` and `withdrawn` can be reopened. See §4.3 and §4.4.

**Why:** this matches Google Docs and Overleaf, and there's no approved-but-not-applied state that could get out of sync.

## D12. A suggestion covers exactly one range (2026-09-28)

Each suggestion has one anchor and one replacement string. Changes across several places are several suggestions, grouped in the data model.

**Why:** this keeps applicability, storage, and conflicts simple. Multi-range and multi-file edits can be added later.
