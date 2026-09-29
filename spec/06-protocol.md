# 6. Protocol

The annox protocol lets editors hand the logic of §2–§5 to a shared **annox server**, so an editor plugin only has to provide the user interface. The server is a language server (LSP 3.17 or later) that also implements a set of `annox/*` extension methods:

- **Any LSP editor, with no plugin,** gets basic support through standard LSP features: annotations show as diagnostics with threads on hover, and suggestions can be accepted and threads resolved through code actions (§6.5).
- **An annox-aware plugin** uses the extension methods (§6.6) to add what LSP has no UI for: writing comments and replies, a thread view, suggestions, and conflict resolution.

The protocol is optional. Every tool can instead work on the files directly (§5). The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## 6.1 Transport

The transport is exactly LSP's: JSON-RPC 2.0 messages with `Content-Length` header framing, normally over the server's stdio. A server implementation SHOULD be startable as a command, e.g. `annox lsp`, so editors can configure it like any other language server. It runs alongside the language servers for a file's language.

## 6.2 Lifecycle and capabilities

The server follows LSP's lifecycle: `initialize`, `initialized`, `shutdown`, `exit`.

An annox-aware client declares itself in `initialize` under `capabilities.experimental.annox`:

```json
"capabilities": {
  "experimental": {
    "annox": { "version": "0.0" }
  }
}
```

The server answers under `capabilities.experimental.annox` in its `InitializeResult`, with the spec version it implements. A client that doesn't declare `annox` is a **plain LSP client** and gets only the features in §6.5.

`initializationOptions.annox` MAY contain:

| Option | Type | Meaning |
|---|---|---|
| `author` | Author (§2.1) | The identity for events written in this session. If absent, the server uses its configured identity or the git default (§2.7). |
| `diagnostics` | boolean | Whether to publish annotations as diagnostics (§6.5). The default is `true`. Annox-aware clients that render annotations themselves SHOULD set it to `false`. |

**Workspaces.** For each workspace folder, and for each opened document outside one, the server finds the annox workspace (§5.1). Documents outside any annox workspace have no annotations. The server creates a workspace (§5.3) only when asked to write the first annotation, and it SHOULD confirm this with the user through `window/showMessageRequest`.

## 6.3 Positions

LSP positions are `(line, character)` pairs, with `character` counted in the position encoding negotiated through `general.positionEncodings`. The server MUST support `utf-16`, and SHOULD support `utf-8` and `utf-32`. It converts positions to and from code-point offsets in normalized text (§3.2, §3.3). This works because LSP's line breaks (`\n`, `\r\n`, `\r`) are exactly the ones §3.2 normalizes.

All ranges on the wire are LSP `Range`s. Offsets appear only inside stored anchors.

## 6.4 Documents and changes

- **Open documents.** The server tracks open documents through `textDocument/didOpen`, `didChange`, and `didClose`, the same as any language server. For an open document, *D* (§3.7) is the normalized buffer content, even when it isn't saved (§4.3.2). The server re-resolves annotations as the buffer changes. It SHOULD debounce this.
- **Closed documents** are read from disk when needed.
- **Storage changes.** The server MUST notice changes to `.annox/` made by others, such as `git pull` or another tool. It registers for `workspace/didChangeWatchedFiles` on `.annox/**` if the client supports that, and otherwise watches the files itself. It reloads the affected documents and pushes updates (§6.6.3).
- **Sync.** If the workspace names a sync hub (§7.2), the server SHOULD act as a replica (§7) whenever credentials are configured, and push updates for events arriving from the hub like any other storage change.
- **Renames.** On `workspace/didRenameFiles`, the server writes `move` events and moves folders as described in §5.6. It SHOULD register for file-operation notifications to receive these.
- **Anchor rewrites** (§3.8) are done by the server only for documents that are saved, so anchors never refer to buffer content that may be thrown away.

## 6.5 Standard LSP features

These work in every LSP client. Annox-aware clients MAY use them too.

### 6.5.1 Diagnostics

Unless disabled, the server publishes a diagnostic for every open root annotation of a document (`textDocument/publishDiagnostics`):

| Annotation | Range | Severity | Message |
|---|---|---|---|
| Comment | resolved range | Hint | first line of the body, or the label, or "Highlight" |
| Suggestion, applicable | resolved range | Information | `Suggestion: "<quote>" → "<replacement>"` |
| Suggestion, stale but located (step 4) | resolved range | Information | the above, prefixed with "Stale" |
| Conflicted annotation | resolved range | Warning | the above, prefixed with "Conflict" |
| Orphaned annotation(s), including orphaned suggestions | start of the document | Warning | "N annotations could not be located" |

`source` is `"annox"`, `code` is the annotation kind, and `data` holds `{ "annotation": <id> }`. Resolved threads and closed suggestions are not published.

### 6.5.2 Hover

Hovering over an annotated range returns the thread as Markdown: the root's author, time, and body, then each reply. For suggestions, the proposed change comes first.

### 6.5.3 Code actions and commands

For a range that intersects annotations, `textDocument/codeAction` offers the applicable actions as commands, handled through `workspace/executeCommand`:

| Command | Arguments | Offered when |
|---|---|---|
| `annox.accept` | `{ annotation }` | open, applicable suggestion |
| `annox.reject` | `{ annotation }` | open suggestion |
| `annox.resolve` | `{ annotation }` | open comment thread |
| `annox.reopen` | `{ annotation }` | resolved thread, or rejected or withdrawn suggestion |

The server lists these commands in `executeCommandProvider`. `annox.accept` follows §6.6.2.

LSP has no standard way to ask the user for free text, so writing comments, replies, suggestions, and conflict resolutions requires an annox-aware client.

## 6.6 Extension methods

### 6.6.1 Wire types

**AnnotationView** is the derived annotation (§2.5.6), plus how it resolves in the current document:

```json
{
  "id": "0192f0c4-…",
  "kind": "suggestion",
  "author": { "id": "mailto:ada@example.org", "name": "Ada Lovelace" },
  "created": "2026-09-28T14:33:00Z",
  "body": null,
  "label": null,
  "status": "open",
  "edit": { "replacement": "we show that" },
  "deleted": false,
  "resolution": {
    "state": "relocated",
    "step": 3,
    "range": { "start": { "line": 1, "character": 23 }, "end": { "line": 1, "character": 36 } }
  },
  "applicable": true,
  "local": false,
  "conflicts": {},
  "replies": []
}
```

- `target` is omitted. Clients work with `resolution.range`.
- `resolution.range` is absent when the annotation is orphaned.
- `applicable` is present only for open suggestions (§4.2).
- `local` is true for local-only annotations (§5.11).
- `replies` holds the thread's reply AnnotationViews, in the order of §2.5.5. Replies have no `resolution`.
- `conflicts` maps each conflicted field to its competing values, so the client can show them:

```json
"conflicts": {
  "body": [
    { "event": "0192f0d1-…", "author": { "…": "…" }, "time": "…", "value": "Is this bound tight?" },
    { "event": "0192f0e2-…", "author": { "…": "…" }, "time": "…", "value": "Tight — see Lemma 4." }
  ]
}
```

A document-level conflict (a conflicted `path`, §5.5.1) is reported through `annox/didChangeAnnotations` (§6.6.3).

### 6.6.2 Requests (client → server)

Every request that changes something writes the corresponding events (§2.4), then pushes the new state (§6.6.3). It returns the affected AnnotationView.

| Method | Params | Effect |
|---|---|---|
| `annox/annotations` | `{ textDocument, includeClosed?: boolean, includeDeleted?: boolean }` | Returns `{ annotations: AnnotationView[], document: DocumentInfo }`. It changes nothing. |
| `annox/create` | `{ textDocument, kind: "comment" \| "suggestion", range, body?, label?, replacement?, local?: boolean }` | `create` event. The anchor is computed from the buffer (§3.6). For a suggestion, `replacement` is required. With `local: true`, the annotation is local-only (§5.11). |
| `annox/reply` | `{ parent, body, local?: boolean }` | `create` of a reply. It is local if `local` is true or the parent is local (§5.11). |
| `annox/publish` | `{ annotations: Id[] }` | Publishes local annotations and their local replies (§5.11). Returns the published AnnotationViews. |
| `annox/edit` | `{ annotation, body?, label? }` | `edit` event. |
| `annox/setStatus` | `{ annotation, status }` | `status` event. Not for `accepted`: use `annox/accept`. |
| `annox/accept` | `{ annotation }` | Applies the suggestion (below). |
| `annox/acceptAll` | `{ annotations: Id[] }` | Bulk accept (below). Returns `{ results: ({ annotation, accepted: true } \| { annotation, error })[] }`. |
| `annox/retarget` | `{ annotation, range, replacement }` | `retarget` event (§4.2.1). |
| `annox/reattach` | `{ annotation, range }` | A user-confirmed location for an orphaned comment (§3.7.4): `reanchor` event. Suggestions use `annox/retarget` instead. |
| `annox/delete`, `annox/restore` | `{ annotation }` | `delete` or `restore` event. |
| `annox/resolveConflict` | `{ annotation, field, value, revert?: boolean }` | Writes the resolving event described in §2.5.4, with `after` covering all competing heads. `value` has the type of the field. For a status conflict involving `accepted`, `revert: true` asks the server to revert the edit (§4.3.3), applied as below. |
| `annox/moveDocument` | `{ from: DocumentUri, to: DocumentUri }` | Re-attaches a missing document (§5.7.3): `move` events for the documents at `from`. |
| `annox/history` | `{ annotation }` | Returns the annotation's events in the order of §2.5.5, for history views. Changes nothing. |
| `annox/setPresence` | `{ textDocument?, selection?: Range }` (notification) | The user's current document and cursor, forwarded to the sync hub as presence (§7.8). Clients send it as the focus or cursor changes, and not at all if the user turned presence off. LSP has no cursor notifications, so plain LSP clients share no presence. |

`DocumentInfo` is `{ documents: Id[], conflicts: { path?: [...] }, duplicates: boolean }`, covering the document records at the path (§5.7.1).

**Applying edits.** For `annox/accept`, and for `annox/resolveConflict` with `revert`, the server:

1. resolves the suggestion against the current buffer, and fails with `StaleSuggestion` if it is stale;
2. sends `workspace/applyEdit` with a `WorkspaceEdit` that replaces the resolved range;
3. only if the client reports `applied: true`, writes the `status` event, with `appliedVersion` set to the version of the buffer after the edit (§4.3.2).

The client applies the edit to its buffer, so the user can undo it and save as usual. If the edit isn't applied, no event is written.

**Bulk accept.** `annox/acceptAll` follows the rule in §4.3 for accepting several suggestions without showing each one. The server goes through `annotations` in the order given. It skips a suggestion, with an error in its result, if it is stale (`StaleSuggestion`), if it resolves only by step 3 (`NeedsReview`), or if its range overlaps one already chosen in this batch (`Overlap`). It then sends **one** `workspace/applyEdit` with all the chosen replacements, so a single undo reverts the whole batch. Only if that edit is applied does it write a `status` event for each chosen suggestion, all with the same `appliedVersion`.

### 6.6.3 Notifications (server → client)

| Method | Params |
|---|---|
| `annox/didChangeAnnotations` | `{ textDocument, annotations: AnnotationView[], document: DocumentInfo }` |
| `annox/didChangePresence` | `{ peers: { author, textDocument?, range? }[] }`: others' presence from the sync hub (§7.8), with documents as URIs and ranges as LSP `Range`s. Sent whenever it changes. |

The server sends this for an open document whenever its annotations or their resolution change, for whatever reason: a request, an edit to the buffer, a change in storage, or a rename. It carries the full current list for the document, so clients just replace their state. Closed threads and deleted annotations are included only if the client asked for them in its most recent `annox/annotations` request for that document.

### 6.6.4 Errors

Failed requests use JSON-RPC errors with these codes. They are outside the ranges reserved by JSON-RPC and LSP.

| Code | Name | Meaning |
|---|---|---|
| 1001 | `UnknownAnnotation` | No such annotation in this workspace. |
| 1002 | `StaleSuggestion` | The suggestion can't be applied (§4.2). |
| 1003 | `InvalidOperation` | The operation doesn't apply to this annotation, e.g. `retarget` on a comment or `reply` to a reply. |
| 1004 | `NotConflicted` | `resolveConflict` on a field that isn't conflicted. |
| 1005 | `NoWorkspace` | The document isn't in an annox workspace, and the user declined to create one. |
| 1006 | `UnsupportedFormat` | The workspace's `format` isn't supported (§5.3). |
| 1007 | `EditNotApplied` | The client declined or failed the `workspace/applyEdit`. |
| 1008 | `NeedsReview` | In a bulk accept: the suggestion resolves only by step 3 and must be accepted individually (§4.3). |
| 1009 | `Overlap` | In a bulk accept: the suggestion's range overlaps one already chosen in the same batch. |

## 6.7 Conformance

- A **Server** MUST implement §6.1–§6.6 and meet the Client semantics of §1.5 for everything it writes. Its core MUST pass all test vectors.
- An annox-aware editor plugin that exposes every Client action (§1.5) through the extension methods conforms as a **Client** when used with a conforming Server.
- A plain LSP client used with a Server gets diagnostics, hover, and the commands of §6.5. That doesn't amount to a Client, and it doesn't fully meet the Viewer requirements either, because conflicts are only flagged, not shown in detail.
