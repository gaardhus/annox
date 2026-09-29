# annox

An open standard for document annotations — highlights, comments, and suggestions — that any editor can support.

annox splits the problem the way LSP does: a portable **data format** that tools can read and write directly, and an optional **protocol** that lets editors talk to an annotation server instead of each editor integrating with each backend.

**Status:** early drafting. Nothing here is stable. See [`spec/`](spec/README.md).

## Reference implementation

A Rust reference implementation lives in [`crates/`](crates/):

- **`annox-core`** is the library. It covers normalization, anchoring (§3), suggestions (§4), event replay (§2), and storage (§5). Its conformance tests run every vector in [`spec/tests/`](spec/tests/).
- **`annox-lsp`** builds the `annox` binary. `annox lsp` runs the language server (§6) over stdio.

```sh
cargo test               # unit, conformance, and end-to-end server tests
cargo build --release    # produces target/release/annox
```

The server currently implements the plain-LSP features of §6.5: annotations as diagnostics, threads on hover, and code actions to accept or reject suggestions and to resolve or reopen threads. Accepting applies the edit through `workspace/applyEdit`, and the `accepted` event is written only once the editor confirms. The `annox/*` extension methods (§6.6), local-only annotations, and sync (§7) are not implemented yet.

To try it in Neovim (0.11+), in a directory containing `.annox/annox.json`:

```lua
vim.lsp.config("annox", { cmd = { "/path/to/annox", "lsp" }, root_markers = { ".annox" } })
vim.lsp.enable("annox")
```
