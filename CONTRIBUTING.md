# Contributing to annox

Bug reports, fixes, and ideas are welcome. For anything bigger than a small fix, such as a new feature, a change to the spec, or a refactor across crates, please open an issue first so we can agree on the approach before you put work into it.

## Layout

- **`spec/`** is the specification: numbered sections, [`decisions.md`](spec/decisions.md) with the reasoning behind each design choice, a [changelog](spec/CHANGELOG.md), and conformance vectors in [`spec/tests/`](spec/tests/).
- **`crates/`** is the Rust reference implementation. `annox-core` is the library (§2–§5), `annox-sync` the hub and replica (§7), `annox-lsp` the `annox` binary (the CLI, `annox lsp`, `annox mcp`, and `annox hub`), and `annox-wasm` builds `annox-core` for the website's demo.
- **`editors/`** has the [Neovim plugin](editors/nvim/README.md) and the [VS Code extension](editors/vscode/README.md).
- **`skills/annox/`** is the agent skill.
- **`site/`** is the website, built with `just site`.

## Building and testing

You need a Rust toolchain. [just](https://github.com/casey/just) runs the common tasks, and [prek](https://github.com/j178/prek) runs the git hooks.

```sh
cargo test               # unit, conformance, and end-to-end server tests
cargo build --release    # produces target/release/annox
just hooks               # install the git hooks
just check               # run the checks CI runs
```

`just check` runs `just lint` (formatters, linters, typos, and `cargo clippy`), `just test`, and the editor tests:

- `just test-nvim` runs the Neovim plugin's tests in headless Neovim against a debug build of `annox`.
- `just test-vscode` type checks the VS Code extension and runs its unit and end-to-end tests. The end-to-end test downloads VS Code and runs it under `xvfb-run` when it's installed. `just test-vscode show` shows its window instead. It needs Node.js.

`just fmt` formats everything. CI also runs the tests on macOS and Windows, which `just check` doesn't.

## Changing the spec

The spec and the reference implementation change together. A change to the spec usually includes:

1. The change to the relevant section in `spec/`.
2. A new entry in [`decisions.md`](spec/decisions.md) when it's a design choice, numbered after the last one (`D49`, …) and dated, with a **Why:** explaining the reasoning. Superseded decisions stay and are marked as superseded.
3. A line under **Unreleased** in the [spec changelog](spec/CHANGELOG.md), naming the sections it touches.
4. Test vectors in `spec/tests/` for any behavior a conforming implementation must agree on. `crates/annox-core/tests/conformance.rs` runs every vector.
5. The implementation, in the crates and, if the protocol changed, the editor plugins.

The spec is versioned separately from the code. It's still a draft, so incompatible changes are fine when the better design needs them.

## Commits and pull requests

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat: …`, `fix(nvim): …`). A commit-msg hook and CI check them with [committed](https://github.com/crate-ci/committed) (see `committed.toml`), and PR titles too, since PRs are squash-merged. The PR title becomes the commit message on `main`, and the changelog entry.

Releases are automated. Every push to `main` updates a release PR that bumps the version and `CHANGELOG.md` with [git-cliff](https://git-cliff.org) (see `cliff.toml`), and merging that PR tags `vX.Y.Z` and attaches `annox` binaries and the VS Code extension to the GitHub release. The binary, the crates, and the editor plugins share that one version.
