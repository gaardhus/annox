--- State shared by the annox modules. `require("annox")` exposes `config`,
--- `state`, `peers`, `suggesting` and `overlay_shown` as its own fields.

local M = {}

M.ns = vim.api.nvim_create_namespace("annox")

M.config = {
  cmd = { "annox", "lsp" },
  --- Author for new events ({ id = "mailto:…", name = "…" }). Defaults to
  --- the git identity on the server side (§2.7).
  author = nil,
  --- Show the first line of each comment at the end of its line.
  virtual_text = true,
  --- Share your document and cursor with others through a sync hub (§7.8).
  --- Others' cursors are always shown.
  presence = true,
  --- Draw suggestions inline, as struck-through and inserted text, even
  --- outside suggestion mode.
  inline_suggestions = false,
  --- Key that undoes your last suggestion while in suggestion mode, or false
  --- to leave undo alone.
  suggest_undo_key = "u",
  --- Tint the line numbers and cursor line of windows in suggestion mode.
  suggest_tint = true,
  --- Mark the words that changed within a suggestion, diff-so-fancy style,
  --- inline and in the thread, hover and edit windows.
  word_diff = true,
  --- Strike through the deleted text of inline suggestions.
  strikethrough = true,
}

--- Others' presence, as last pushed by the server.
M.peers = {}

--- Latest state pushed by the server, per buffer: { annotations, document }.
M.state = {}

--- Whether annotations and others' cursors are drawn. Toggled with
--- `:Annox overlay`; buffers in suggestion mode draw theirs regardless.
M.overlay_shown = true

--- Suggestion mode state per buffer (see suggest_mode.lua).
M.suggesting = {}

return M
