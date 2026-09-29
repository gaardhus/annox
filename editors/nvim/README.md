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
      -- presence = false,  -- stop sharing your cursor with collaborators on a sync hub
      -- inline_suggestions = true,  -- always draw suggestions inline, not only in suggestion mode
      -- suggest_undo_key = false,  -- keep `u` as plain undo in suggestion mode
    })
  end,
}
```

The server attaches to files inside a directory that contains `.annox/`. To start annotating a project, run `:Annox init`. It creates the workspace at the git root, or the working directory if there's no git repository, after asking you to confirm.

Changes others make to `.annox/`, such as a `git pull`, show up within a second without reloading.

## Commands

| Command | Does |
|---|---|
| `:Annox init` | Create an annox workspace for this project and attach the server. |
| `:Annox comment` | Comment on the cursor position, or on the selection when run from visual mode (`:'<,'>Annox comment`). |
| `:Annox suggest` | Suggest a replacement for the selection. The prompt is pre-filled with the current text. |
| `:Annox suggesting` | Turn suggestion mode on or off for the buffer (see below). |
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

## Suggestion mode

`:Annox suggesting` turns the buffer into suggestion mode, like "Suggesting" in online editors. You edit as usual, and each time you leave insert mode or finish a normal-mode change such as `dw`, your edits become suggestions and the buffer goes back to its original text. The file never contains suggested text, so saving is always safe.

- Suggestions are drawn inline: deleted text struck through, inserted text after it in green.
- Typing next to a suggestion you made in this session extends it. Delete a word and type its replacement right there, and you get one suggestion.
- `u` undoes your last suggestion, instead of undoing buffer changes. Set `suggest_undo_key` to change or disable this.
- Accepting a suggestion still edits the buffer.
- Suggestions are shared right away. Suggestion mode has no private-draft variant.

For a statusline, `require("annox").is_suggesting()` tells whether the current buffer is in suggestion mode.

When the workspace syncs through a hub, collaborators' cursors appear as `▏Name` tags, and your own cursor is shared unless `presence = false`.

Highlight groups, most linked to diagnostic groups by default: `AnnoxComment`, `AnnoxSuggestion`, `AnnoxStale`, `AnnoxConflict`, `AnnoxLocal`, `AnnoxVirtualText`, `AnnoxSign`, `AnnoxPresence`, `AnnoxPresenceRange`, and, for inline suggestions, `AnnoxDeletion` and `AnnoxInsertion`.

## Tests

```sh
cargo build -p annox-lsp
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/e2e.lua
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/init.lua
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/suggesting.lua
```
