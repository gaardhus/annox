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

## D35. Mentions are plain text (2026-09-29)

`@name` in a body is ordinary text. v1 has no structured mentions. See §2.8.

**Why:** no schema change and nothing to keep in sync. Structured mentions or a link convention can be added later without breaking anything.

## D36. Point anchors relocate through collapsed whitespace; insertions relocated that way are stale (2026-09-29)

Step 4 has a point variant that searches collapsed prefix plus suffix. When the point sat inside a whitespace run, it lands at the start of the matching run. Insertion suggestions relocated this way are stale, as with every step-4 relocation. See §3.7.2 and §4.2.

**Why:** point comments survive paragraph re-wrapping, which is common in LaTeX and Markdown. Keeping insertions stale keeps §4.2's rule uniform, and avoids guessing where in the whitespace an insertion belongs. The suggestion is shown in place, ready to be re-targeted.

## D37. Presence is ephemeral, shares document and cursor, and is on by default (2026-09-29)

The hub relays who is connected, their open document, and their cursor. Nothing is stored. Users can turn it off, and a replica that has turned it off sends nothing. Partial sync and hub federation are deferred. See §7.8.

**Why:** it gives a Google-Docs-like live experience, and keeping it out of the event model means no storage or history implications. Being on by default matches what users of collaborative editors expect. The opt-out is required.

## D38. Shared records are never merged into local ones; unknown merge targets are ignored (2026-09-29)

When duplicate document records span the local and shared areas, the survivor is the smallest shared id. Readers ignore a `mergedInto` value that names a document they don't know. See §5.5.1 and §5.8.

**Why:** found in the full review. Merging a shared record into a local one wrote a redirect that no one else could follow, which hid every shared annotation on that file from other users. The reader-side rule is a safety net, in case a buggy writer does it anyway.

## D39. Local records may merge into any shared record (2026-09-29)

This refines D38. The rule that `into` must be smaller applies only within an area. A local record may merge into a shared record with any id. See §5.5.1.

**Why:** found while implementing publishing. D38 made the smallest *shared* id the survivor, but the rule that `into` must be smaller then made the required merge invalid whenever the local record had the smaller id. Merges still can't loop, because shared records never merge into local ones.

## D40. Performance is validated: implementations should index, cache, and debounce (2026-09-29)

Measured with the reference implementation (`cargo run --release -p annox-core --example bench`, and `-p annox-lsp --example keystroke`):

- **Resolution** is linear in document length per anchor. For 1,000 anchors in a 100k-character document it takes about 5 ms when relocating, 35 ms when orphaned, and 0.2 ms when unchanged. Step 3 is the one slow case, when a quote occurs thousands of times.
- **Storage:** 20,000 event files read in about 170 ms, and 5,000 annotations derive in about 150 ms.
- **Server:** keystrokes cost nothing, because resolution is debounced by 150 ms. One refresh then takes about 80 ms for 1,000 annotations, and hover takes 0.3 ms.

**Why:** the spec's algorithms needed no changes. The first implementation's slowness came from naive character-by-character search, rebuilding the collapsed text for every anchor, a quadratic replay, and re-reading storage on every keystroke. §6.4 now recommends caching as well as debouncing.

## D41. Suggestion mode is client behavior, built on `retarget` (2026-09-29)

A mode in which typing makes suggestions is described in a non-normative §4.6. Edits are turned into suggestions when the user pauses, and the document keeps its text. A change touching one of the user's suggestions from the same session extends it with a `retarget` event. §4.2.1 now only requires marking re-targets made by someone other than the author.

**Why:** it matches online editors, and needs no new events or protocol methods. Keeping the buffer's text unchanged, rather than holding the proposed text until the user saves, means the file can never end up containing suggested text, and anchors stay exact. `retarget` already changes the anchor and the replacement together, which is exactly what extending a suggestion does. Showing "re-targeted by" for an author's own edits would mark nearly every suggestion made this way.

## D42. Points relocate by partial context (2026-09-29)

Resolution gains step 5 for point anchors (empty quotes, such as insertions and point comments). A point is placed at the single offset with the best context score, if that score is at least half of the stored prefix and suffix together. Orphaned is now step 6. Suggestions relocated by step 5 are applicable, but bulk accept leaves them for individual review, like step 3. See §3.7.2 and §4.2.

**Why:** found in use. Changing "What" to "How" a few characters before an insertion orphaned it, because steps 2 and 4 need the whole context and step 3 needs a quote. Replacements survived the same edit through step 3, so insertions were much more fragile. Step 5 is the point counterpart of step 3. It runs after step 4 so that every anchor that resolved before resolves the same way, including insertions next to re-wrapped whitespace, which stay stale (D36). The half-context minimum keeps a point from moving to a place that merely shares a few characters with its context.

## D43. Bulk accept may include partial-context relocations after one confirmation (2026-09-29)

`annox/acceptAll` takes an optional `confirmed` flag. With it, suggestions relocated by step 3 or 5 are accepted too, instead of being skipped with `NeedsReview`. Clients set it after a single prompt for the whole batch that says how many suggestions were relocated this way. See §4.3 and §6.6.2.

**Why:** found in use. Adding a line after three suggestions changed the 32-character context of all three, so all of them relocated by step 3 and bulk accept skipped every one, even though each was shown at the right place. Accepting one suggestion also changes the context of any suggestion next to it, so this happens often. The skip exists to protect users who can't see what they're accepting. A prompt that names the count and relies on the inline display restores that protection, without one prompt per suggestion. Suggestion mode now widens changes to whole words (§4.6), which makes step 3 more reliable in the first place.

## D44. Presence carries a quote, and cursors are resolved as anchors (2026-09-29)

A peer's cursor includes the quote selector of its range, and a recipient whose text differs resolves it with the anchoring algorithm. A cursor that can't be resolved isn't shown. Buffers themselves aren't synced. See §7.8.

**Why:** collaborators' copies of a document routinely differ, through unsaved edits, other branches, or pulls not yet made. Raw offsets then point at the wrong text, off by the length of every edit before the cursor. Anchoring already solves locating a range in text that has changed, including points next to edits (D42), so presence reuses it at the cost of about 64 code points per message. Syncing the buffers instead would make annox a co-editing system, which is out of scope: each person owns their copy of the text, and suggestions are how changes are proposed.

## D45. Events received from a hub go to a git-ignored mirror (2026-09-29)

Replicas write received events to `.annox/synced/docs/`, which `.annox/.gitignore` lists. Readers load the mirror as part of the shared area. The mirror is never pruned, and tools never write their own events there. See §5.12.

**Why:** found in use. A replica wrote a collaborator's events into `docs/`, where they sat untracked until the collaborator committed them. Then `git pull` stopped with "untracked working tree files would be overwritten by merge", although the files were identical. Git overwrites ignored files, but not untracked ones. Keeping received events in an ignored mirror means everything untracked in `docs/` was written by the local user. Per-author folders were considered, but they don't help: a received event either sits at the path its author commits, which collides in the same way, or is committed once per collaborator. A `git pull` wrapper that removes identical copies first was rejected, because it treats the symptom and doesn't help anyone who pulls from an IDE or plain git. The mirror isn't pruned when git delivers a copy, because a branch switch or a revert can remove that copy after the replica's cursor has moved past the event. As a result, an event reaches version control only when someone commits it, normally its author.

## D46. Reverting an accepted suggestion is a new, linked suggestion (2026-10-03)

`accepted` stays terminal. A revert is a new suggestion, anchored to the applied text found by the applied-text search, whose replacement is the original `quote.exact`, and whose `create` event has `reverts` naming the original. Clients may accept it right away. See §4.3.4.

**Why:** raised in use: there was no way to undo an accept other than writing the inverse suggestion by hand. Allowing `accepted → open` with an implied edit was considered, but it would make `accepted` in a history ambiguous (was the text applied or not?) and need a second kind of edit-carrying status event. A new suggestion reuses review, conflicts, and history as they are, and the edit it makes is recorded like any other accept. `reverts` is a field rather than body text so that clients can link the two threads.

## D47. Edited quotes are found between their prefix and suffix, and accepts carry comments along (2026-10-05)

Resolution gains step 6 for non-empty quotes. When one of the stored prefix and suffix occurs once and the other follows or precedes it, the text between them is the result, if its length is between half and twice the quote's. Orphaned is now step 7. Suggestions relocated by step 6 are stale, and their anchors are not rewritten. Separately, accepting a suggestion re-anchors the open comments whose text it changes, mapping their ranges through the edit. See §3.7.2, §4.2, and §4.3.5.

**Why:** found in use. In a review of a 50,000-character LaTeX paper, six open comments were orphaned. In four, the quote had been edited in place (a word inserted, a sentence reworded, a table note re-cut) while its 32-character prefix and suffix were intact and unique. Every step searched for the quote itself, so any edit inside it orphaned the comment, though its location was obvious. Two of the four were orphaned by annox's own accept, which knew exactly what it replaced. Step 6 covers edits made by any tool. Carrying comments at accept covers annox's own edits exactly, without a heuristic. Requiring a unique prefix or suffix, as steps 3 and 4 require a unique selection, keeps step 6 from guessing between places. The length bounds keep a comment from spreading over a passage that grew into something else, or staying on a remnant of one that was deleted. Making step-6 suggestions stale follows step 4: the text changed, so the author never saw what would be replaced. A new state, such as `modified`, was considered, but `relocated` with a step number already distinguishes step 4 the same way.

## D48. Orphaned annotations get a suggested location from word alignment (2026-10-06)

The reference implementation proposes a suggested location (§3.7.4) for an open orphaned annotation. It finds the passage that matches the most of the quote's words in order, ignoring case and allowing words to be added, removed, or changed (a Smith–Waterman local alignment over words, where punctuation helps align but doesn't count). It suggests that passage if it has at least 40% of the quote's words, and at least 4. Equally good passages go to the one nearest the stored position. The protocol exposes it as `resolution.suggested` (§6.6.1), and comments can be re-attached there with one action. Nothing is moved without confirmation.

**Why:** found in use. Three comments on a plan were orphaned after an agent revised the plan to answer them. Two of the passages had been rewritten (a list condensed, a paragraph reworded) and were easy to find by eye. In both, the prefix or suffix had changed too, so step 6 couldn't help. The third passage had been removed, and the only shared text elsewhere was a name. Measured on these, the rewritten passages kept 49% and 58% of their words in order, and the best match for the removed one kept 3 of 8, which is how the thresholds were set. Character-level fuzzy matching, as in diff-match-patch, was considered, but it handles typos rather than restructured sentences and limits pattern length. Diffing against the old version from git history is exact when that version was committed, but in this case the edits were not, and the stored version is a SHA-256 of normalized text rather than a git blob id. This stays outside the normative algorithm because thresholds tuned on a few cases shouldn't decide resolution for every implementation.
