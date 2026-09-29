# Changelog

## 0.1 — 2026-09-29

The first complete draft. The reasons behind each design choice are recorded as D1–D40 in [decisions.md](decisions.md).

### What 0.1 specifies

- **Data model (§2).** Annotations are append-only event logs. Events record their causal parents (`after`), so concurrent changes, for example from two git branches, are detected per field and shown as conflicts instead of being lost. The kinds are comment (with highlights as comments without a body), suggestion, and reply, in flat threads. Authors are identified without authentication, defaulting to the git identity.
- **Anchoring (§3).** Offsets count code points over normalized text. Anchors combine a position, a quote with context, and a content hash. A deterministic resolution algorithm produces `exact`, `relocated`, or `orphaned`, and survives edits by other tools, re-wrapped paragraphs, and repeated quotes.
- **Suggestions (§4).** One range per suggestion. Whether it can be applied is derived from whether its text is still intact. Accepting applies the edit. The spec defines the lifecycle, re-targeting stale suggestions, and reverting a concurrent acceptance.
- **Storage (§5).** `.annox/` sits at the workspace root, with one immutable file per event. Merges only ever add files, so git needs no special configuration. Documents have stable ids that survive renames, stragglers from other branches, and path reuse. A git-ignored local area holds private drafts.
- **Protocol (§6).** The annox server is a language server: plain LSP editors get diagnostics, hover, and code actions, and `annox/*` extension methods give annox-aware plugins everything else. Edits are applied through `workspace/applyEdit`.
- **Sync (§7).** A store-and-relay hub, a changefeed with cursors over JSON-RPC and WebSocket, authentication delegated to the connection, and ephemeral presence.
- **Conformance (§1.5).** Four classes: Viewer, Client, Server, and Hub. There are 61 test vectors in [`tests/`](tests/) covering anchoring, suggestions, replay, and storage.

### Known limitations and deferred work

- Plain-text documents only. Rich formats need new selector types.
- No compaction, so event logs only grow. Measured, this is fine at 5,000 annotations (D40). Adding compaction later requires a `format` bump.
- Deleting hides content but doesn't erase it (D30).
- No structured mentions, and no grouping of suggestions.
- Sync: partial sync and hub federation are deferred.
