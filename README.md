# annox

An open standard for document annotations — highlights, comments, and suggestions — that any editor can support.

annox splits the problem the way LSP does: a portable **data format** that tools can read and write directly, and an optional **protocol** that lets editors talk to an annotation server instead of each editor integrating with each backend.

**Status:** early drafting. Nothing here is stable. See [`spec/`](spec/README.md).

## Reference implementation

A Rust reference implementation lives in [`crates/`](crates/):

- **`annox-core`** is the library. It covers normalization, anchoring (§3), suggestions (§4), event replay (§2), and storage (§5). Its conformance tests run every vector in [`spec/tests/`](spec/tests/).
- **`annox-sync`** has the sync hub and replica (§7).
- **`annox-lsp`** builds the `annox` binary. `annox lsp` runs the language server (§6) over stdio, and `annox hub` runs a sync hub.

```sh
cargo test               # unit, conformance, and end-to-end server tests
cargo build --release    # produces target/release/annox
```

The server implements:

- **For plain LSP editors (§6.5):** annotations as diagnostics, threads on hover, and code actions to accept or reject suggestions and to resolve or reopen threads.
- **For annox-aware plugins (§6.6):** every `annox/*` extension method. That covers creating, replying, editing, publishing local drafts, setting status, accepting and bulk-accepting, re-targeting, re-attaching, deleting and restoring, resolving conflicts (including reverting an accepted edit), moving documents, and history. It pushes `annox/didChangeAnnotations` after every change.

Edits always go through `workspace/applyEdit`, and the status event is written only once the editor confirms. The server picks up outside changes to `.annox/`, such as a `git pull`. It uses the editor's file watching where available, and otherwise checks every second. It offers to create a workspace when you add the first annotation outside one.

## Live sync

To share annotations live instead of only through git, run a hub and point the workspace at it:

```sh
annox hub --data ~/annox-hub --listen 127.0.0.1:7878 --token-file ~/.annox-hub-token
```

In `.annox/annox.json`, which is safe to commit:

```json
{ "format": 1, "sync": { "url": "wss://hub.example.org/w/my-paper" } }
```

Each user adds the hub to `~/.config/annox/credentials.json`, which is never committed:

```json
{ "hubs": { "wss://hub.example.org/w/my-paper": { "token": "…" } } }
```

The server syncs only with hubs listed there. Remote hubs must use `wss://`, and plain `ws://` is accepted only to localhost. The hub itself speaks plain WebSocket, so put it behind a TLS-terminating proxy to serve `wss://`. Sync and git work together: events arriving both ways are the same files. Collaborators' cursors appear in the editor as presence (§7.8). The hub doesn't implement read-only access yet.

## Editor support

- **Neovim:** [`editors/nvim/`](editors/nvim/README.md) is a plugin covering the full workflow: highlights, comments and suggestions, threads, accept and reject, local drafts, re-targeting, conflict resolution, and history.
- **Any other LSP editor:** point it at `annox lsp` for the plain-LSP features.
