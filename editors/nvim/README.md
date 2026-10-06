# annox.nvim

A thin Neovim client for the annox language server. The server does all the annox work: storage, replay, anchoring, and conflicts. The plugin draws annotations and sends your actions.

![Replying to a comment, commenting on a selection, accepting a suggestion, and suggesting edits in suggestion mode](../../assets/nvim-demo.gif)

Requires Neovim 0.11+ and the `annox` binary (`cargo build --release` at the repository root).

## Setup

With [lazy.nvim](https://github.com/folke/lazy.nvim), pointing at this directory:

```lua
{
  dir = "/path/to/annox/editors/nvim",
  config = function()
    require("annox").setup({
      cmd = { "/path/to/annox/target/release/annox", "lsp" },
      -- author = { id = "mailto:you@example.org", name = "You" },  -- defaults to ~/.config/annox/config.json, then git
      -- presence = false,  -- stop sharing your cursor with collaborators on a sync hub
      -- inline_suggestions = true,  -- always draw suggestions inline, not only in suggestion mode
      -- suggest_undo_key = false,  -- keep `u` as plain undo in suggestion mode
      -- suggest_tint = false,  -- don't tint line numbers and cursor line in suggestion mode
      -- word_diff = false,  -- don't mark the words that changed within a suggestion
      -- strikethrough = false,  -- show deleted text of inline suggestions in red, not struck through
    })
  end,
}
```

The server attaches to files inside a directory that contains `.annox/`. To start annotating a project, run `:Annox init`. It creates the workspace at the git root, or the working directory if there's no git repository, after asking you to confirm.

Changes others make to `.annox/`, such as a `git pull`, show up within a second without reloading.

## Commands

| Command                                   | Does                                                                                                                                                                                                                                                                                     |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `:Annox init`                             | Create an annox workspace for this project and attach the server.                                                                                                                                                                                                                        |
| `:Annox comment`                          | Comment on the cursor position, or on the selection when run from visual mode (`:'<,'>Annox comment`).                                                                                                                                                                                   |
| `:'<,'>Annox highlight`                   | Highlight the selection: a comment with no body. Add a body later with `:Annox edit`.                                                                                                                                                                                                    |
| `:Annox suggest`                          | Suggest a replacement for the selection. The prompt is pre-filled with the current text.                                                                                                                                                                                                 |
| `:Annox edit`                             | Edit the suggested text of the suggestion under the cursor, or the text of a comment, in a floating window, with the suggestion's original text shown read-only above it. Esc (in normal mode) saves and closes, `:w` saves, `:q!` discards.                                                                                                           |
| `:Annox suggesting`                       | Turn suggestion mode on or off for the buffer (see below).                                                                                                                                                                                                                               |
| `:Annox overlay [on\|off]`                | Hide or show annotations and others' cursors in every buffer, toggling with no argument. Commands still act on hidden annotations, and a buffer in suggestion mode keeps showing its own.                                                                                                |
| `:Annox reply`                            | Reply to the thread under the cursor.                                                                                                                                                                                                                                                    |
| `:Annox thread`                           | Show the thread under the cursor in a floating window.                                                                                                                                                                                                                                   |
| `:Annox accept` / `reject`                | Accept or reject the suggestion under the cursor. Accepting edits the buffer, and you can undo as usual.                                                                                                                                                                                 |
| `:'<,'>Annox accept` / `:Annox! accept`   | Accept every suggestion touching the selection, or in the whole buffer, as one edit that a single undo reverts. If some suggestions moved because the text around them changed, you're asked once whether to accept them too. Stale or overlapping suggestions are skipped and reported. |
| `:'<,'>Annox reject` / `:Annox! reject`   | Reject every suggestion touching the selection, or in the whole buffer, after one confirmation. Rejected suggestions can be reopened one by one.                                                                                                                                         |
| `:Annox resolve` / `reopen`               | Resolve or reopen a thread.                                                                                                                                                                                                                                                              |
| `:'<,'>Annox resolve` / `:Annox! resolve` | Resolve every comment thread touching the selection, or in the whole buffer, after one confirmation.                                                                                                                                                                                     |
| `:Annox revert`                           | Undo an accepted suggestion in the buffer: pick it, then revert it now or leave the revert open as a suggestion for review.                                                                                                                                                              |
| `:Annox orphans`                          | Pick from annotations whose text could no longer be found.                                                                                                                                                                                                                               |
| `:Annox reattach`                         | Attach an orphaned comment to the selection.                                                                                                                                                                                                                                             |
| `:Annox retarget`                         | Point a stale suggestion at the selection, and review its replacement.                                                                                                                                                                                                                   |
| `:Annox draft`                            | Like `comment`, but local-only: stored in the git-ignored `.annox/local/` and shown with ✎.                                                                                                                                                                                              |
| `:Annox publish`                          | Publish the draft under the cursor. `:Annox! publish` publishes every draft in the buffer.                                                                                                                                                                                               |
| `:Annox conflicts`                        | Resolve a conflicting field by picking one of the competing values or writing a merged version. Run it again for any other conflicting fields. If an accepted suggestion loses, you're offered to revert its edit.                                                                       |
| `:Annox history`                          | Show every event of the thread under the cursor.                                                                                                                                                                                                                                         |
| `:Annox list`                             | Put the buffer's annotations in the quickfix list.                                                                                                                                                                                                                                       |
| `:Annox commit`                           | Commit the workspace's annotation files to git, and nothing else. It shows how many there are and asks you to confirm or edit the message, which summarizes them. It doesn't push.                                                                                                       |

Suggested keymaps:

```lua
vim.keymap.set({ "n", "x" }, "<leader>ac", ":Annox comment<cr>")
vim.keymap.set("x", "<leader>ah", ":Annox highlight<cr>")
vim.keymap.set("x", "<leader>as", ":Annox suggest<cr>")
vim.keymap.set("n", "<leader>at", "<cmd>Annox thread<cr>")
vim.keymap.set("n", "<leader>ar", "<cmd>Annox reply<cr>")
vim.keymap.set("n", "<leader>aa", "<cmd>Annox accept<cr>")
vim.keymap.set("x", "<leader>aa", ":Annox accept<cr>")
```

With lazy.nvim, as in LazyVim, you can put the keymaps in the plugin spec instead. which-key shows each `desc` as the label, and the second spec names the `<leader>a` group and gives every key a Nerd Font icon. Set `lazy = false`, because a spec with `keys` otherwise loads only when you press one of them. Visual-mode keymaps use `:` rather than `<cmd>` so that the selection is passed as a range. Some LazyVim extras also use `<leader>a`, so pick another prefix if yours does.

```lua
return {
  {
    dir = "/path/to/annox/editors/nvim",
    lazy = false,
    opts = { cmd = { "/path/to/annox/target/release/annox", "lsp" } },
    config = function(_, opts)
      require("annox").setup(opts)
    end,
    keys = {
      { "<leader>ac", ":Annox comment<cr>", mode = { "n", "x" }, desc = "Comment" },
      { "<leader>ad", ":Annox draft<cr>", mode = { "n", "x" }, desc = "Draft comment" },
      { "<leader>ah", ":Annox highlight<cr>", mode = "x", desc = "Highlight" },
      { "<leader>as", ":Annox suggest<cr>", mode = "x", desc = "Suggest replacement" },
      { "<leader>as", ":Annox suggest<cr>", desc = "Add suggestion" },
      { "<leader>aS", "<cmd>Annox suggesting<cr>", desc = "Toggle suggestion mode" },
      { "<leader>aT", "<cmd>Annox overlay<cr>", desc = "Toggle overlay" },
      { "<leader>ae", "<cmd>Annox edit<cr>", desc = "Edit annotation" },
      { "<leader>at", "<cmd>Annox thread<cr>", desc = "Show thread" },
      { "<leader>ar", "<cmd>Annox reply<cr>", desc = "Reply" },
      { "<leader>aa", "<cmd>Annox accept<cr>", desc = "Accept suggestion" },
      { "<leader>aa", ":Annox accept<cr>", mode = "x", desc = "Accept suggestions in selection" },
      { "<leader>aA", "<cmd>Annox! accept<cr>", desc = "Accept all in buffer" },
      { "<leader>ax", "<cmd>Annox reject<cr>", desc = "Reject suggestion" },
      { "<leader>ax", ":Annox reject<cr>", mode = "x", desc = "Reject suggestions in selection" },
      { "<leader>aR", "<cmd>Annox resolve<cr>", desc = "Resolve thread" },
      { "<leader>ao", "<cmd>Annox reopen<cr>", desc = "Reopen thread" },
      { "<leader>ap", "<cmd>Annox publish<cr>", desc = "Publish draft" },
      { "<leader>al", "<cmd>Annox list<cr>", desc = "List in quickfix" },
      { "<leader>aH", "<cmd>Annox history<cr>", desc = "History" },
      { "<leader>aO", "<cmd>Annox orphans<cr>", desc = "Orphans" },
      { "<leader>aC", "<cmd>Annox conflicts<cr>", desc = "Resolve conflicts" },
      { "<leader>ag", "<cmd>Annox commit<cr>", desc = "Commit annotations" },
    },
  },
  {
    "folke/which-key.nvim",
    opts = {
      spec = {
        { "<leader>a", group = "annox", icon = { icon = "\u{f086}", color = "green" }, mode = { "n", "x" } },
        { "<leader>ac", icon = "\u{f0e5}", mode = { "n", "x" } },
        { "<leader>ad", icon = "\u{f24a}", mode = { "n", "x" } },
        { "<leader>ah", icon = { icon = "\u{f0652}", color = "yellow" }, mode = "x" },
        { "<leader>as", icon = "\u{f040}" },
        { "<leader>aS", icon = { icon = "\u{f205}", color = "green" } },
        { "<leader>aT", icon = "\u{f06e}" },
        { "<leader>ae", icon = "\u{f044}" },
        { "<leader>at", icon = "\u{f0e6}" },
        { "<leader>ar", icon = "\u{f112}" },
        { "<leader>aa", icon = { icon = "\u{f00c}", color = "green" }, mode = { "n", "x" } },
        { "<leader>aA", icon = { icon = "\u{f058}", color = "green" } },
        { "<leader>ax", icon = { icon = "\u{f00d}", color = "red" }, mode = { "n", "x" } },
        { "<leader>aR", icon = "\u{f05d}" },
        { "<leader>ao", icon = "\u{f0e2}" },
        { "<leader>ap", icon = "\u{f1d8}" },
        { "<leader>al", icon = "\u{f03a}" },
        { "<leader>aH", icon = "\u{f1da}" },
        { "<leader>aO", icon = { icon = "\u{f127}", color = "yellow" } },
        { "<leader>aC", icon = { icon = "\u{f126}", color = "orange" } },
        { "<leader>ag", icon = { icon = "\u{e729}", color = "orange" } },
      },
    },
  },
}
```

## Suggestion mode

`:Annox suggesting` turns the buffer into suggestion mode, like "Suggesting" in online editors. You edit as usual, and each time you leave insert mode or finish a normal-mode change such as `dw`, your edits become suggestions and the buffer goes back to its original text. The file never contains suggested text, so saving is always safe.

- Suggestions are drawn inline: deleted text struck through, inserted text after it in green. The words that changed get a stronger tint, diff-so-fancy style, here and in the thread, hover (`K`) and edit windows. Set `word_diff = false` to turn that off, or `strikethrough = false` to keep the deleted text red but not struck through.
- Changes are widened to whole words: changing `pd` to `pl` suggests `pd` → `pl`, not `d` → `l`.
- Typing next to a suggestion you made in this session extends it. Delete a word and type its replacement right there, and you get one suggestion.
- To change text you've already suggested, run `:Annox edit` on it. The inline green text can't hold the cursor.
- `u` undoes your last suggestion, instead of undoing buffer changes. Set `suggest_undo_key` to change or disable this.
- Accepting a suggestion still edits the buffer.
- Suggestions are shared right away. Suggestion mode has no private-draft variant.

While suggestion mode is on, the buffer's line numbers and cursor line are tinted green (`AnnoxSuggestingLineNr`, `AnnoxSuggestingCursorLineNr`, `AnnoxSuggestingCursorLine`, set through `'winhighlight'`). Set `suggest_tint = false` to turn this off.

For a statusline, `require("annox").statusline()` returns `"SUGGESTING"` in suggestion mode and `""` otherwise, and `require("annox").is_suggesting()` returns the same as a boolean. The buffer variable `b:annox_suggesting` holds it too, and toggling fires `User AnnoxSuggesting` with `data = { buf, enabled }`. With lualine:

```lua
lualine_x = { { require("annox").statusline, color = "Added" } }
```

With a plain `'statusline'`:

```vim
set statusline+=%{get(b:,'annox_suggesting',0)?'SUGGESTING\ ':''}
```

`require("annox").overlay_shown` and `g:annox_overlay` tell whether the overlay is on, and `:Annox overlay` fires `User AnnoxOverlay` with `data = { enabled }`.

`require("annox").orphan_count()` tells how many annotations could not be located. They are also announced in a line above the text.

When the workspace syncs through a hub, collaborators' cursors appear as `▏Name` tags, and your own cursor is shared unless `presence = false`.

Highlight groups: `AnnoxComment`, `AnnoxHighlight`, `AnnoxSuggestion`, and `AnnoxLocal` tint the background of annotated text in the matching diagnostic color, and `AnnoxStale` and `AnnoxConflict` undercurl it. `AnnoxComment` is also underlined, so you can tell a comment from a highlight (a comment with no body or replies). The full list: `AnnoxComment`, `AnnoxHighlight`, `AnnoxSuggestion`, `AnnoxStale`, `AnnoxConflict`, `AnnoxLocal`, `AnnoxVirtualText`, `AnnoxSign`, `AnnoxPresence`, `AnnoxPresenceRange`, for inline suggestions, `AnnoxDeletion` and `AnnoxInsertion`, `AnnoxWordDeletion` and `AnnoxWordInsertion` for the words that changed within a suggestion (a background only, so the text keeps its color), `AnnoxEditOriginal` for the old text above the suggestion edit window, `AnnoxOrphans` for the notice about annotations that could not be located, and `AnnoxSuggestingLineNr`, `AnnoxSuggestingCursorLineNr` and `AnnoxSuggestingCursorLine` for the suggestion mode tint.

## Demo

`just nvim-demo` re-records the GIF above in `assets/nvim-demo.gif`, from the script in `demo/demo.lua`. It needs [asciinema](https://asciinema.org) 3 and [agg](https://github.com/asciinema/agg).

## Tests

```sh
cargo build -p annox-lsp
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/e2e.lua
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/init.lua
ANNOX_BIN=$PWD/target/debug/annox nvim --headless --clean -l editors/nvim/tests/suggesting.lua
```
