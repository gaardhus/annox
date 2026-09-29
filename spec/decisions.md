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

*Refined by D20.*

## D10. A suggestion is applicable if its quote resolves exactly (2026-09-28)

Applicability is derived, never stored. A suggestion is applicable if its anchor resolves by steps 0–3. After step 4 or when orphaned, it is stale. Anchors of suggestions are never rewritten in a way that changes `quote.exact`. A stale suggestion needs a user to re-target it. See §4.2.

**Why:** in steps 0–3 the resolved range contains exactly the text the author saw, so the suggestion's meaning is intact. Overlapping suggestions go stale on their own once one is applied, with no dependency tracking.

## D11. Accepting a suggestion applies it (2026-09-28)

Accepting a suggestion edits the document and records `accepted` with the applied version in one user action. `accepted` is terminal. `rejected` and `withdrawn` can be reopened. See §4.3 and §4.4.

**Why:** this matches Google Docs and Overleaf, and there's no approved-but-not-applied state that could get out of sync.

## D12. A suggestion covers exactly one range (2026-09-28)

Each suggestion has one anchor and one replacement string. Changes across several places are several suggestions, grouped in the data model.

**Why:** this keeps applicability, storage, and conflicts simple. Multi-range and multi-file edits can be added later.

## D13. Annotations are append-only event logs (2026-09-28)

Every change is an immutable event, and annotation state is derived by replaying the events. See §2.3–§2.5.

**Why:** when annotations are committed to git and edited on branches, merging is a union of events and never a JSON merge conflict. History and authorship are built in.

## D14. Events record causal parents; concurrent writes are surfaced as conflicts (2026-09-28)

Each event lists `after`, the heads of the annotation's history that its writer had seen. A field with several concurrent heads is conflicted and shown to the user, with a deterministic provisional value (greatest id). There are no silent last-writer-wins rules, except that concurrent automatic `reanchor` events never conflict. A delete that is concurrent with other changes is a conflict. See §2.5.

**Why:** a Lamport counter was considered, but it can't reliably detect concurrent edits, and the goal is to never lose a human decision silently. The history of each annotation is small, so ancestry checks are cheap.

## D15. Ids are UUIDv7 (2026-09-28)

**Why:** they are standard, can be generated offline, sort roughly by creation time, and have libraries everywhere.

## D16. Kinds are comment, suggestion, and reply; a highlight is a comment with no body (2026-09-28)

An optional `label` covers highlighter colours and tags. See §2.2.

**Why:** fewer kinds, and a highlight can gain a comment later without changing kind.

## D17. Replies are annotations in flat threads (2026-09-28)

A reply is its own annotation with `parent` pointing to a comment or suggestion. Replies to replies are not allowed.

**Why:** concurrent replies never conflict, replies reuse all annotation machinery, and flat threads match Google Docs, Overleaf, and GitHub reviews.

## D18. Authors are a stable id plus a name, defaulting to the git identity (2026-09-28)

`author.id` SHOULD be a URI. Without explicit configuration, clients inside a git repository default to `mailto:<user.email>` and `user.name`. There is no authentication in v1. See §2.7.

**Why:** zero configuration in git repositories, without making git a requirement.

## D19. Bodies are CommonMark (2026-09-28)

**Why:** review comments need code and links, and the raw text is still readable in clients that can't render Markdown.

## D20. Anchors are rewritten only after relocation (2026-09-28)

This refines D9. Clients rewrite an anchor (a `reanchor` event) only after a `relocated` result, never after `exact`. See §3.8.

**Why:** it keeps logs and diffs quiet when documents are only being read, and still refreshes anchors that have actually drifted.

## D21. Conformance classes: Viewer, Client, Server (2026-09-28)

A Viewer is read-only. It replays events, resolves anchors, and must visibly mark conflicts and orphaned annotations. A Client must support everything: all kinds, full suggestion handling, and a way to resolve every conflict. Partial Clients are not conforming. A Server implements Client semantics plus §6. See §1.5.

**Why:** a strict Client class guarantees users a consistent experience in every editor. The Viewer class keeps read-only tools, such as CI checks, previews, and exports, conforming without a merge UI.

## D22. A single `.annox/` directory at the workspace root (2026-09-28)

The workspace root is the nearest ancestor directory containing `.annox/annox.json`, found the same way git finds `.git`. See §5.1.

**Why:** there's one place to look and one directory to commit, and all document paths are relative to it.

## D23. One file per event, grouped in per-document folders (2026-09-28)

Each event is its own immutable JSON file, and each document's events are grouped in a folder. See §5.2 and §5.4.

**Why:** merges only ever add files, so they never conflict in git, sync tools, or hosted merge UIs, and no `.gitattributes` is needed. Per-document folders keep the files for each document together. Per-document JSONL logs were considered. They are more intuitive and faster to read, but they need `merge=union` and still conflict in sync tools.

## D24. Documents have stable ids; folder names are cosmetic (2026-09-28)

A document gets an id and an event log (`document`, `move`, `merged`) in a `document/` subfolder. Its folders are named `<path>~<id>`, and only the id suffix identifies the document. A rename is a `move` event, and the folder is moved as tidying. Anchor paths are informational. See §5.5 and §5.6.

**Why:** an earlier draft used path-named folders plus rename records. The storage test vectors showed that it breaks path reuse: after renaming `main.tex` to `old-main.tex`, a new `main.tex` inherited the old document's annotations. Stable ids fix this, stragglers from branches without the rename land correctly, and concurrent renames are surfaced as ordinary conflicts.

## D25. Duplicate document records are loaded together and merged by clients (2026-09-28)

If several documents are at the same path, readers MUST load all of them. Clients SHOULD merge them into the one with the smallest id, leaving a `merged` redirect. See §5.8.

**Why:** loading all of them keeps reads correct, including for stragglers. Merging stops later renames from moving only some of the duplicates, which would reintroduce the path-reuse bug.

## D26. The annox server owns the logic (2026-09-28)

Editors send intents and document content. The server writes events, re-anchors, applies suggestions, and returns resolved annotations. A low-level event-sync layer is deferred. See §6.

**Why:** this is the full LSP-style benefit. Plugins stay thin, and every editor behaves the same because §2–§5 are implemented once.

## D27. The protocol is LSP plus `annox/*` extensions (2026-09-28)

The server is a language server. It reuses LSP's transport, lifecycle, document sync, and position-encoding negotiation, and uses diagnostics, hover, and code actions to give plain LSP editors basic support without a plugin. Annox-aware clients declare `experimental.annox` and use extension methods for everything else. See §6.2–§6.6.

**Why:** it fits the original "LSP-compatible" goal, gives support in every LSP editor from day one, and document sync is already solved. The costs are supporting UTF-16 positions on the wire and a compromised plugin-free experience.

## D28. A thin plugin paired with a conforming Server is a Client (2026-09-28)

See §1.5 and §6.7.

**Why:** this is how LSP works, and without it no thin plugin could conform.

## D29. No compaction in v1 (2026-09-29)

Event files are never removed or rewritten, and logs only grow. The cache (§5.9) is the only answer to load performance.

**Why:** it keeps v1 small and removes any risk of losing events from branches that haven't been merged. The trade-off is a known cost: adding compaction later (for example snapshot events with a `covers` list) requires a `format` bump (§5.3), because v1 readers would treat events that point at compacted history as dangling.

## D30. Deleting hides content; it doesn't erase it (2026-09-29)

A `delete` event hides an annotation. Its content stays in the event files and in version-control history, and the spec says so explicitly. See §2.4.

**Why:** version-control history keeps the content no matter what, so an erase feature that only purges the working tree would suggest more than it does. Honest documentation is simpler and safer.

## D31. Live sync through a hub, using a changefeed over JSON-RPC and WebSocket (2026-09-29)

Replicas connect to one sync hub per workspace. The hub numbers events in the order it receives them, and replicas pull everything after their cursor, push what the hub lacks, and get live pushes. Authentication is delegated to the connection: TLS plus the hub's own scheme, with credentials kept outside `.annox/`. See §7.

**Why:** immutable events make sync a pure set union, so a simple changefeed is enough. The hub needs no annox logic. Peer-to-peer sync and set reconciliation were considered but judged too complex for v1. Sync coexists with git because synced events are ordinary files.

## D32. Local-only annotations live in a git-ignored `.annox/local/` (2026-09-29)

It has the same layout as `docs/`, and readers load both areas together. An annotation lives entirely in one area. Publishing moves the files, and local document records reuse the duplicate-merging mechanism when published. See §5.11.

**Why:** it enables private highlights and draft reviews, like GitHub's pending reviews, while reusing the existing machinery. A `private` field on annotations was rejected, because committed files aren't private.

## D33. No suggestion grouping; bulk accept by ids (2026-09-29)

Suggestions stay independent in v1. `annox/acceptAll` takes a list of ids, enforces the bulk rule of §4.3 (steps 0–2 only, no overlaps), and applies everything as one edit. See §6.6.2.

**Why:** grouping adds a field and partial-application semantics for little gain. Reviewers can still accept many suggestions at once, with a single undo.

## D34. Anyone may re-target a stale suggestion, with attribution (2026-09-29)

Any user may re-target a suggestion. Clients must show who did it (`retargetedBy`) next to the original author. See §4.2.1.

**Why:** stale suggestions from authors who are away can be rescued without losing their thread, and the attribution keeps it clear who changed what.
