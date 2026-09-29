<p align="center">
  <img src="assets/annox-header.svg" alt="annox" width="400" height="128">
</p>

<p align="center">
  <i>
    An open standard for portable document annotations — highlights, comments, and suggestions — that any editor can support.
  </i>
</p>

annox splits the problem the way LSP does: a portable **data format** that tools can read and write directly, and an optional **protocol** that lets editors talk to an annotation server instead of each editor integrating with each backend.

**Status:** spec version 0.1, a complete first draft. Expect incompatible changes before 1.0. See [`spec/`](spec/README.md) and the [changelog](spec/CHANGELOG.md).

## Reference implementation

A Rust reference implementation lives in [`crates/`](crates/). It covers every section:

| Spec           | Implemented in                             | Tested by                                        |
| -------------- | ------------------------------------------ | ------------------------------------------------ |
| §2 Data model  | `annox-core` (`event`, `replay`)           | replay vectors, reverse-order replay             |
| §3 Anchoring   | `annox-core` (`text`, `anchor`)            | anchoring vectors, worked example                |
| §4 Suggestions | `annox-core` (`suggestion`) and the server | suggestion vectors, accept/revert end-to-end     |
| §5 Storage     | `annox-core` (`storage`, `ops`)            | storage vectors, moves and publishing end-to-end |
| §6 Protocol    | `annox-lsp` (`annox lsp`)                  | in-process LSP tests, headless Neovim            |
| §7 Sync        | `annox-sync` (`annox hub`, replica)        | hub over WebSockets, two servers syncing         |

Crates:

- **`annox-core`** is the library. It covers normalization, anchoring (§3), suggestions (§4), event replay (§2), and storage (§5). Its conformance tests run every vector in [`spec/tests/`](spec/tests/).
- **`annox-sync`** has the sync hub and replica (§7).
- **`annox-lsp`** builds the `annox` binary. `annox lsp` runs the language server (§6) over stdio, and `annox hub` runs a sync hub.

```sh
cargo test               # unit, conformance, and end-to-end server tests
cargo build --release    # produces target/release/annox
just hooks               # install the git hooks (needs prek and just)
just check               # run the checks CI runs
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat: …`, `fix(nvim): …`). A commit-msg hook and CI check them with [committed](https://github.com/crate-ci/committed) (see `committed.toml`), and PR titles too, since PRs are squash-merged. Releases are automated: every push to `main` updates a release PR that bumps the version and `CHANGELOG.md` with [git-cliff](https://git-cliff.org) (see `cliff.toml`), and merging that PR tags `vX.Y.Z` and attaches `annox` binaries to the GitHub release. The binary, the crates, and the editor plugin share that one version; the spec is versioned separately.

The server implements:

- **For plain LSP editors (§6.5):** annotations as diagnostics, threads on hover, and code actions to accept or reject suggestions and to resolve or reopen threads.
- **For annox-aware plugins (§6.6):** every `annox/*` extension method. That covers creating, replying, editing, publishing local drafts, setting status, accepting and bulk-accepting, re-targeting, re-attaching, deleting and restoring, resolving conflicts (including reverting an accepted edit), moving documents, and history. It pushes `annox/didChangeAnnotations` after every change.

Edits always go through `workspace/applyEdit`, and the status event is written only once the editor confirms. The server picks up outside changes to `.annox/`, such as a `git pull`. It uses the editor's file watching where available, and otherwise checks every second. It offers to create a workspace when you add the first annotation outside one.

## Command line

The `annox` binary also reads and writes annotations directly, printing JSON. It's meant for scripts and AI agents: text is targeted by quoting it, not by offsets, and an ambiguous quote is an error that lists where it occurs.

```sh
annox list [FILE] [--all]
annox comment paper.md --quote "the bound is tight" --body "Cite Lemma 4?"
annox suggest paper.md --quote "teh" --replace "the" --body "typo"
annox reply ID --body "Fixed."
annox status ID resolved
annox accept ID
```

Write commands use `--author`/`--name`, or `ANNOX_AUTHOR`/`ANNOX_AUTHOR_NAME`, and otherwise your [configured identity](#configuration). Give an agent its own id, for example `ANNOX_AUTHOR=urn:agent:claude`, so its annotations are distinguishable from yours. A running `annox lsp` picks up the changes, so they appear in the editor. Run `annox help` for every command.

### Agents

[`skills/annox/`](skills/annox/SKILL.md) is a skill that teaches an agent the CLI and how to use it well: suggest instead of editing text you own, and answer comments in their threads. For Claude Code, link it into your skills:

```sh
ln -s "$PWD/skills/annox" ~/.claude/skills/annox
```

For agents that can't run shell commands, `annox mcp` serves the same commands as MCP tools over stdio:

```sh
claude mcp add annox -- annox mcp --author urn:agent:claude --name Claude
```

It works on the directory it's started in, or the one given with `--root`.

## Live sync

To share annotations live instead of only through git, run a hub and point the workspace at it:

```sh
annox hub --data ~/annox-hub --listen 127.0.0.1:7878 --token-file ~/.annox-hub-token
```

In `.annox/annox.json`, which is safe to commit:

```json
{ "format": 1, "sync": { "url": "wss://hub.example.org/w/my-paper" } }
```

Each user adds the hub to their [credentials file](#configuration), which is never committed:

```json
{ "hubs": { "wss://hub.example.org/w/my-paper": { "token": "…" } } }
```

The server syncs only with hubs listed there. Remote hubs must use `wss://`, and plain `ws://` is accepted only to localhost. The hub itself speaks plain WebSocket, so put it behind a TLS-terminating proxy to serve `wss://`. Sync and git work together. Events received from the hub are kept in the git-ignored `.annox/synced/`, so `git status` only lists the events you wrote, and pulling a collaborator's commit never collides with events that sync already delivered. Each person commits their own events. Collaborators' cursors appear in the editor as presence (§7.8). The hub doesn't implement read-only access yet.

## Editor support

- **Neovim:** [`editors/nvim/`](editors/nvim/README.md) is a plugin covering the full workflow: highlights, comments and suggestions, threads, accept and reject, suggestion mode, local drafts, re-targeting, conflict resolution, and history.
- **Any other LSP editor:** point it at `annox lsp` for the plain-LSP features. It accepts `initializationOptions` of the form `{ "annox": { "author": { "id": "…", "name": "…" }, "diagnostics": true } }` (§6.2). Both are optional.

## Configuration

annox reads configuration from four places:

| Where                              | Scope                    | Settings                                                                                              |
| ---------------------------------- | ------------------------ | ----------------------------------------------------------------------------------------------------- |
| `.annox/annox.json`                | The workspace, committed | `format` and `sync.url`, the hub to sync with (see [Live sync](#live-sync))                           |
| `~/.config/annox/config.json`      | You, on every workspace  | `author`, your identity                                                                               |
| `~/.config/annox/credentials.json` | You, never committed     | `hubs`, a token per hub URL (see [Live sync](#live-sync))                                             |
| Editor settings                    | One editor               | The LSP `initializationOptions` above, or the [Neovim plugin's options](editors/nvim/README.md#setup) |

The user files live under `$XDG_CONFIG_HOME/annox/` when it's set. `ANNOX_CONFIG_FILE` and `ANNOX_CREDENTIALS_FILE` point at a file elsewhere. A missing file is the same as an empty one.

`config.json` sets the identity your annotations are written as:

```json
{ "author": { "id": "mailto:ada@example.org", "name": "Ada Lovelace" } }
```

The identity is taken from the first of these that is set:

1. The command: `--author`/`--name` for the CLI and `annox mcp`, or the editor's `author` option for `annox lsp`.
2. `ANNOX_AUTHOR` and `ANNOX_AUTHOR_NAME`.
3. `author` in `config.json`.
4. Git's `user.email` and `user.name`, as `mailto:` plus the email.

A name alone only renames the identity from the next step down. An id without a name stands on its own, so an agent's `ANNOX_AUTHOR=urn:agent:claude` never picks up your name. If `config.json` is invalid, the CLI refuses to write, and `annox lsp` logs a warning and uses git's identity.
