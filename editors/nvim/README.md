# annox.nvim

A thin Neovim client for the annox language server. The server does all the annox work: storage, replay, anchoring, and conflicts. The plugin draws annotations and sends your actions.

Requires Neovim 0.11+ and the `annox` binary (`cargo build --release` at the repository root).

## Setup

With [lazy.nvim](https://github.com/folke/lazy.nvim), pointing at this directory:

```lua
{
  dir = "/path/to/annox/editors/nvim",
  config = function()
    require("annox").setup({
      cmd = { "/path/to/annox/target/release/annox", "lsp" },
      -- author = { id = "mailto:you@example.org", name = "You" },  -- defaults to your git identity
    })
  end,
}
```

The server attaches to files inside a directory that contains `.annox/`. To start annotating a project, create a workspace:

```sh
mkdir .annox && echo '{ "format": 1 }' > .annox/annox.json && printf 'cache/\nlocal/\n' > .annox/.gitignore
```

## Commands

| Command | Does |
|---|---|
| `:Annox comment` | Comment on the cursor position, or on the selection when run from visual mode (`:'<,'>Annox comment`). |
| `:Annox suggest` | Suggest a replacement for the selection. The prompt is pre-filled with the current text. |
| `:Annox reply` | Reply to the thread under the cursor. |
| `:Annox thread` | Show the thread under the cursor in a floating window. |
| `:Annox accept` / `reject` | Accept or reject the suggestion under the cursor. Accepting edits the buffer, and you can undo as usual. |
| `:Annox resolve` / `reopen` | Resolve or reopen a thread. |
| `:Annox orphans` | Pick from annotations whose text could no longer be found. |
| `:Annox reattach` | Attach an orphaned comment to the selection. |
| `:Annox retarget` | Point a stale suggestion at the selection, and review its replacement. |
| `:Annox draft` | Like `comment`, but local-only: stored in the git-ignored `.annox/local/` and shown with ✎. |
| `:Annox publish` | Publish the draft under the cursor. `:Annox! publish` publishes every draft in the buffer. |
| `:Annox conflicts` | Resolve a conflicting field by picking one of the competing values or writing a merged version. Run it again for any other conflicting fields. If an accepted suggestion loses, you're offered to revert its edit. |
| `:Annox history` | Show every event of the annotation under the cursor. |
| `:Annox list` | Put the buffer's annotations in the quickfix list. |

Suggested keymaps:

```lua
vim.keymap.set({ "n", "x" }, "<leader>ac", ":Annox comment<cr>")
vim.keymap.set("x", "<leader>as", ":Annox suggest<cr>")
vim.keymap.set("n", "<leader>at", "<cmd>Annox thread<cr>")
vim.keymap.set("n", "<leader>ar", "<cmd>Annox reply<cr>")
vim.keymap.set("n", "<leader>aa", "<cmd>Annox accept<cr>")
```

Highlight groups, all linked to diagnostic groups by default: `AnnoxComment`, `AnnoxSuggestion`, `AnnoxStale`, `AnnoxConflict`, `AnnoxLocal`, `AnnoxVirtualText`, and `AnnoxSign`.

## Tests

```sh
cargo build -p annox-lsp
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/e2e.lua
```
