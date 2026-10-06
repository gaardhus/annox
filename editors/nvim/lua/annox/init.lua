--- annox for Neovim: a thin client for the annox language server (spec §6).
---
--- The server owns all annox logic. This plugin renders the annotations it
--- pushes (`annox/didChangeAnnotations`) and sends user actions as `annox/*`
--- requests. Suggested edits come back as `workspace/applyEdit`, which
--- Neovim's LSP client applies to the buffer.

local store = require("annox.store")
local util = require("annox.util")
local client_for = util.client_for
local byte_col = util.byte_col
local first_line = util.first_line
local request = util.request
local cursor_position = util.cursor_position
local visual_range = util.visual_range
local before = util.before
local under_cursor = util.under_cursor
local buffer_annotations = util.buffer_annotations
local describe = util.describe
local with_annotation = util.with_annotation
local with_input = util.with_input
local target_range = util.target_range
local find_annotation = util.find_annotation
local resolved_text = util.resolved_text
local pick = util.pick

local worddiff = require("annox.worddiff")
local word_changes = worddiff.word_changes
local inserted_chunks = worddiff.inserted_chunks
local mark_words = worddiff.mark_words
local diff_block = worddiff.diff_block

local highlight = require("annox.highlight")
local tint = highlight.tint
local set_highlights = highlight.set_highlights
local highlight_group = highlight.highlight_group

local render = require("annox.render")
local send_presence = render.send_presence

local thread = require("annox.thread")
local open_thread = thread.open_thread
local style_hover = thread.style_hover

local actions = require("annox.actions")
local set_status_all = actions.set_status_all

local qflist = require("annox.qflist")
local refresh_list = qflist.refresh_list

local orphans = require("annox.orphans")

local edit = require("annox.edit")

local M = {}

-- Reading or setting these fields of this module goes through to the store.
local shared = { config = true, state = true, peers = true, suggesting = true, overlay_shown = true }
setmetatable(M, {
  __index = function(_, k)
    if shared[k] then
      return store[k]
    end
  end,
  __newindex = function(t, k, v)
    if shared[k] then
      store[k] = v
    else
      rawset(t, k, v)
    end
  end,
})

M.orphan_count = render.orphan_count
M.render = render.render
M.render_presence = render.render_presence
M.on_presence = render.on_presence
M.thread = thread.thread
M.history = thread.history
M.comment = actions.comment
M.highlight = actions.highlight
M.suggest = actions.suggest
M.reply = actions.reply
M.revert = actions.revert
M.accept = actions.accept
M.publish = actions.publish
M.commit = actions.commit
M.reopen = actions.reopen
M.reject = actions.reject
M.resolve = actions.resolve
M.list = qflist.list
M.orphans = orphans.orphans
M.edit = edit.edit
M.retarget = edit.retarget
M.reattach = edit.reattach
M.resolve_conflict = edit.resolve_conflict

local ns = store.ns

local function on_annotations(_, result)
  local bufnr = vim.uri_to_bufnr(result.textDocument.uri)
  if not vim.api.nvim_buf_is_loaded(bufnr) then
    return
  end
  -- Only open annotations are shown. The server pushes closed ones too while
  -- `M.revert` has asked for them.
  local annotations = vim.tbl_filter(function(a)
    return a.status == nil or a.status == "open"
  end, result.annotations)
  store.state[bufnr] = { annotations = annotations, document = result.document }
  local s = store.suggesting[bufnr]
  if s then
    -- Keep only suggestions that can still be extended.
    local fresh = {}
    for _, a in ipairs(annotations) do
      if s.views[a.id] then
        fresh[a.id] = a
      end
    end
    s.views = fresh
  end
  M.render(bufnr)
  refresh_list(bufnr)
end

--- Suggestion mode ---------------------------------------------------------
---
--- While it is on, edits to the buffer become suggestions (§4.6). Whenever
--- the user pauses (leaving insert mode, or after a normal-mode change), the
--- buffer is compared with its text from before the edit. Each changed
--- stretch, widened to whole words, becomes a new suggestion, or extends one made in this session that
--- it touches (a `retarget` event), and the buffer goes back to its original
--- text. The file therefore never contains suggested text.
---
--- State per buffer: { base = lines, views = { [id] = AnnotationView },
--- queue = { op… }, busy = boolean, undo = { entry… } }.

--- The document text of `lines`, as the server sees it.
local function text_of(lines)
  return table.concat(lines, "\n") .. "\n"
end

--- 0-based byte offset of the start of each line, plus one past the end.
local function line_starts(lines)
  local starts, pos = {}, 0
  for i, l in ipairs(lines) do
    starts[i] = pos
    pos = pos + #l + 1
  end
  starts[#lines + 1] = pos
  return starts
end

local function is_continuation(byte)
  return byte ~= nil and byte >= 0x80 and byte < 0xC0
end

--- Letters, digits, underscore, and any non-ASCII byte (so multi-byte
--- characters are never split).
local function is_word(byte)
  return byte ~= nil and (byte >= 0x80 or string.char(byte):match("[%w_]") ~= nil)
end

--- The changes from `base` to `lines`: { a0, a1, edited_end, replacement,
--- restore }, with `[a0, a1)` a byte range of the base text, `edited_end`
--- where the edit itself ended before widening, and `restore` the line range
--- of `lines` to put back and the base lines to put there.
local function changes(base, lines)
  local a, b = text_of(base), text_of(lines)
  local sa, sb = line_starts(base), line_starts(lines)
  local out = {}
  -- `vim.diff` was renamed to `vim.text.diff` in Neovim 0.12.
  local diff = vim.text.diff or vim.diff
  for _, h in ipairs(diff(a, b, { result_type = "indices" })) do
    local la, ca, lb, cb = unpack(h)
    local fa, fb = ca == 0 and la + 1 or la, cb == 0 and lb + 1 or lb
    local a0, a1, b0, b1 = sa[fa], sa[fa + ca], sb[fb], sb[fb + cb]
    -- Trim what the old and new text share, keeping whole characters.
    while a0 < a1 and b0 < b1 and a:byte(a0 + 1) == b:byte(b0 + 1) do
      a0, b0 = a0 + 1, b0 + 1
    end
    while a0 > sa[fa] and is_continuation(a:byte(a0 + 1)) do
      a0, b0 = a0 - 1, b0 - 1
    end
    while a1 > a0 and b1 > b0 and a:byte(a1) == b:byte(b1) do
      a1, b1 = a1 - 1, b1 - 1
    end
    while is_continuation(a:byte(a1 + 1)) do
      a1, b1 = a1 + 1, b1 + 1
    end
    local edited_end = a1
    -- Widen a change that starts or ends inside a word to the whole word
    -- (§4.6). The text around the change is the same in both versions.
    local starts_in_word = is_word(a:byte(a0 + 1)) and a0 < a1 or is_word(b:byte(b0 + 1)) and b0 < b1
    if starts_in_word then
      while a0 > 0 and b0 > 0 and is_word(a:byte(a0)) and a:byte(a0) == b:byte(b0) do
        a0, b0 = a0 - 1, b0 - 1
      end
    end
    local ends_in_word = is_word(a:byte(a1)) and a0 < a1 or is_word(b:byte(b1)) and b0 < b1
    if ends_in_word then
      while is_word(a:byte(a1 + 1)) and a:byte(a1 + 1) == b:byte(b1 + 1) do
        a1, b1 = a1 + 1, b1 + 1
      end
    end
    table.insert(out, {
      a0 = a0,
      a1 = a1,
      edited_end = edited_end,
      replacement = b:sub(b0 + 1, b1),
      restore = { fb - 1, fb - 1 + cb, vim.list_slice(base, fa, fa + ca - 1) },
    })
  end
  return out
end

--- The LSP position of byte offset `off` in the text of `lines`.
local function offset_position(lines, starts, off, enc)
  local row = #lines + 1
  for i = 1, #lines do
    if starts[i + 1] > off then
      row = i
      break
    end
  end
  if row > #lines then
    return { line = #lines, character = 0 }
  end
  return { line = row - 1, character = vim.str_utfindex(lines[row], enc, off - starts[row], false) }
end

--- The byte offset of LSP position `pos` in the text of `lines`.
local function position_offset(lines, starts, pos, enc)
  local line = lines[pos.line + 1]
  if not line then
    return starts[#lines + 1]
  end
  local ok, col = pcall(vim.str_byteindex, line, enc, pos.character, false)
  return starts[pos.line + 1] + (ok and col or #line)
end

--- A suggestion made in this session that the change `[a0, a1)` touches.
local function extendable(s, a0, a1, starts, enc)
  for id, v in pairs(s.views) do
    local r = v.resolution and v.resolution.range
    if r and v.status == "open" and v.applicable then
      local r0 = position_offset(s.base, starts, r.start, enc)
      local r1 = position_offset(s.base, starts, r["end"], enc)
      if a1 >= r0 and a0 <= r1 then
        return id, v, r0, r1
      end
    end
  end
end

--- Sends the next queued operation, one at a time so that each change can
--- extend the suggestion the previous one created.
local function pump(bufnr)
  local s, client = store.suggesting[bufnr], client_for(bufnr)
  if not s or s.busy or #s.queue == 0 or not client then
    return
  end
  local op = table.remove(s.queue, 1)
  local enc = client.offset_encoding
  local starts = line_starts(s.base)
  local method, params, undo
  if op.undo then
    local u = op.undo
    if u.range then
      method, params = "annox/retarget", { annotation = u.id, range = u.range, replacement = u.replacement }
    else
      method, params = "annox/delete", { annotation = u.id }
    end
  else
    local a0, a1, replacement = op.a0, op.a1, op.replacement
    local id, v, r0, r1 = extendable(s, a0, a1, starts, enc)
    if id then
      -- Compose with the existing suggestion. Ties go after it, so typing at
      -- the end of an insertion continues it. A change covering all of the
      -- suggestion's text (struck through on screen) replaces it.
      local current = v.edit.replacement
      local covers = r0 < r1 and a0 <= r0 and a1 >= r1
      if covers then
        -- Keep the new text as it is.
      elseif a0 >= r1 then
        replacement = current .. replacement
      elseif a1 <= r0 then
        replacement = replacement .. current
      elseif a0 >= r0 then
        replacement = current .. replacement
      else
        replacement = replacement .. current
      end
      a0, a1 = math.min(a0, r0), math.max(a1, r1)
      if a0 == r0 and a1 == r1 and replacement == current then
        return pump(bufnr)
      end
      undo = { id = id, range = v.resolution.range, replacement = current }
    end
    local range = {
      start = offset_position(s.base, starts, a0, enc),
      ["end"] = offset_position(s.base, starts, a1, enc),
    }
    if id then
      method, params = "annox/retarget", { annotation = id, range = range, replacement = replacement }
    else
      method, params =
        "annox/create", {
          textDocument = { uri = vim.uri_from_bufnr(bufnr) },
          kind = "suggestion",
          range = range,
          replacement = replacement,
        }
    end
  end
  s.busy = true
  client:request(method, params, function(err, result)
    s.busy = false
    if err then
      vim.notify(string.format("annox: %s failed: %s", method, err.message), vim.log.levels.ERROR)
    elseif op.undo then
      s.views[op.undo.id] = op.undo.range and result or nil
    else
      s.views[result.id] = result
      table.insert(s.undo, undo or { id = result.id })
    end
    pump(bufnr)
  end, bufnr)
end

--- Turns the buffer's edits into suggestions and puts its text back.
local function capture(bufnr)
  local s = store.suggesting[bufnr]
  if not s or vim.api.nvim_get_mode().mode:find("^i") then
    return
  end
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local found = changes(s.base, lines)
  if #found == 0 then
    return
  end
  -- Join the restore to the edit, so plain undo can't bring the edit back.
  pcall(vim.cmd.undojoin)
  for i = #found, 1, -1 do
    local r = found[i].restore
    vim.api.nvim_buf_set_lines(bufnr, r[1], r[2], false, r[3])
  end
  local last = found[#found]
  if bufnr == vim.api.nvim_get_current_buf() then
    local pos = offset_position(s.base, line_starts(s.base), last.edited_end, "utf-8")
    local row = math.min(pos.line, #s.base - 1)
    pcall(vim.api.nvim_win_set_cursor, 0, { row + 1, pos.line > row and #s.base[row + 1] or pos.character })
  end
  for _, c in ipairs(found) do
    if c.a0 ~= c.a1 or c.replacement ~= "" then
      table.insert(s.queue, c)
    end
  end
  pump(bufnr)
end

--- Makes the buffer's current text the base, after a change that is not a
--- suggestion (an accepted suggestion, or reloading the file).
local function rebase(bufnr)
  local s = store.suggesting[bufnr]
  if s then
    s.base = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
    s.queue = {}
  end
end

--- Undoes your last suggestion in suggestion mode: deletes it, or puts back
--- what it was before it was extended.
function M.undo_suggestion(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local s = store.suggesting[bufnr]
  local entry = s and table.remove(s.undo)
  if not entry then
    return vim.notify("annox: no suggestion to undo", vim.log.levels.INFO)
  end
  table.insert(s.queue, { undo = entry })
  pump(bufnr)
end

--- Whether suggestion mode is on in `bufnr`, e.g. for a statusline.
function M.is_suggesting(bufnr)
  return store.suggesting[bufnr or vim.api.nvim_get_current_buf()] ~= nil
end

--- A statusline label for suggestion mode in `bufnr`, or "" when it's off.
function M.statusline(bufnr)
  return M.is_suggesting(bufnr) and "SUGGESTING" or ""
end

local tint_groups = { "LineNr", "CursorLineNr", "CursorLine" }

--- Adds or removes the suggestion mode tint in `win`'s 'winhighlight',
--- keeping any other entries.
local function sync_tint(win)
  local on = store.config.suggest_tint and store.suggesting[vim.api.nvim_win_get_buf(win)] ~= nil
  local old = vim.api.nvim_get_option_value("winhighlight", { win = win })
  local entries = vim.tbl_filter(function(e)
    return not e:find(":AnnoxSuggesting", 1, true)
  end, vim.split(old, ",", { trimempty = true }))
  if on then
    for _, g in ipairs(tint_groups) do
      table.insert(entries, g .. ":AnnoxSuggesting" .. g)
    end
  end
  local new = table.concat(entries, ",")
  if new ~= old then
    vim.api.nvim_set_option_value("winhighlight", new, { win = win, scope = "local" })
  end
end

--- Shows that suggestion mode changed in `bufnr`: the tint, `b:annox_suggesting`,
--- the `User AnnoxSuggesting` event and the statusline.
local function mode_changed(bufnr)
  local on = store.suggesting[bufnr] ~= nil
  vim.b[bufnr].annox_suggesting = on
  for _, win in ipairs(vim.fn.win_findbuf(bufnr)) do
    sync_tint(win)
  end
  vim.api.nvim_exec_autocmds(
    "User",
    { pattern = "AnnoxSuggesting", modeline = false, data = { buf = bufnr, enabled = on } }
  )
  vim.cmd.redrawstatus({ bang = true })
end

--- Turns suggestion mode on or off for the current buffer.
--- opts: { enable? (default: toggle) }
function M.suggest_mode(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local enable = opts.enable
  if enable == nil then
    enable = store.suggesting[bufnr] == nil
  end
  local group = vim.api.nvim_create_augroup("annox_suggesting_" .. bufnr, { clear = true })
  local key = store.config.suggest_undo_key
  if not enable then
    if store.suggesting[bufnr] then
      capture(bufnr)
      store.suggesting[bufnr] = nil
      if key then
        pcall(vim.keymap.del, "n", key, { buffer = bufnr })
      end
      mode_changed(bufnr)
      vim.notify("annox: suggestion mode off", vim.log.levels.INFO)
    end
    return M.render(bufnr)
  end
  if not client_for(bufnr) then
    return request(bufnr)
  end
  store.suggesting[bufnr] = {
    base = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false),
    views = {},
    queue = {},
    busy = false,
    undo = {},
  }
  vim.api.nvim_create_autocmd({ "InsertLeave", "TextChanged" }, {
    group = group,
    buffer = bufnr,
    callback = function()
      capture(bufnr)
    end,
  })
  vim.api.nvim_create_autocmd("BufReadPost", {
    group = group,
    buffer = bufnr,
    callback = function()
      rebase(bufnr)
    end,
  })
  if key then
    vim.keymap.set("n", key, function()
      M.undo_suggestion(bufnr)
    end, { buffer = bufnr, desc = "annox: undo last suggestion" })
  end
  mode_changed(bufnr)
  vim.notify("annox: suggestion mode on", vim.log.levels.INFO)
  M.render(bufnr)
end

--- Shows or hides annotations and others' cursors in every buffer. Commands
--- still act on hidden annotations, and a buffer in suggestion mode keeps
--- showing its own.
--- opts: { enable? (default: toggle) }
function M.overlay(opts)
  opts = opts or {}
  local enable = opts.enable
  if enable == nil then
    enable = not store.overlay_shown
  end
  store.overlay_shown = enable
  vim.g.annox_overlay = enable
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) then
      M.render(b)
      M.render_presence(b)
    end
  end
  vim.api.nvim_exec_autocmds("User", { pattern = "AnnoxOverlay", modeline = false, data = { enabled = enable } })
  vim.cmd.redrawstatus({ bang = true })
  vim.notify("annox: overlay " .. (enable and "on" or "off"), vim.log.levels.INFO)
end

--- Applies server edits (accepting a suggestion) without turning them into
--- new suggestions.
local function on_apply_edit(err, result, ctx)
  local response = vim.lsp.handlers["workspace/applyEdit"](err, result, ctx)
  for bufnr in pairs(store.suggesting) do
    rebase(bufnr)
  end
  return response
end

--- Creates an annox workspace (§5.3), by default at the buffer's git root or
--- the working directory, and attaches the server to its open buffers.
--- opts: { root?, confirm? (default true) }
function M.init(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local root = opts.root or vim.fs.root(bufnr, { ".git" }) or vim.fn.getcwd()
  local annox = vim.fs.joinpath(root, ".annox")
  if vim.uv.fs_stat(vim.fs.joinpath(annox, "annox.json")) then
    return vim.notify("annox: " .. root .. " is already an annox workspace", vim.log.levels.INFO)
  end
  if opts.confirm ~= false and vim.fn.confirm("Create an annox workspace in " .. root .. "?", "&Yes\n&No", 2) ~= 1 then
    return
  end
  vim.fn.mkdir(annox, "p")
  vim.fn.writefile({ '{ "format": 1 }' }, vim.fs.joinpath(annox, "annox.json"))
  vim.fn.writefile({ "cache/", "local/", "synced/" }, vim.fs.joinpath(annox, ".gitignore"))
  -- vim.lsp.enable attaches on FileType; replay it for buffers in the workspace.
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(b)
    if vim.api.nvim_buf_is_loaded(b) and vim.startswith(name, root .. "/") then
      vim.api.nvim_exec_autocmds("FileType", { buffer = b })
    end
  end
  vim.notify("annox: created a workspace in " .. root, vim.log.levels.INFO)
end

--- Waits until the server has attached to `bufnr` and pushed its annotations,
--- which it may still be doing when a command runs right after the plugin is
--- lazy-loaded. Starts the server if the workspace was created since the
--- buffer was opened, as `annox init` from a shell does. Returns whether the
--- buffer is ready, and says why not when it isn't.
local function wait_ready(bufnr)
  local function ready()
    return client_for(bufnr) ~= nil and store.state[bufnr] ~= nil
  end
  if ready() then
    return true
  end
  if not vim.fs.root(bufnr, { ".annox" }) then
    vim.notify("annox: this file is not in an annox workspace; run :Annox init", vim.log.levels.WARN)
    return false
  end
  if not client_for(bufnr) then
    -- Only vim.lsp.enable's handler, which reuses a server that is starting.
    vim.api.nvim_exec_autocmds("FileType", { group = "nvim.lsp.enable", buffer = bufnr })
  end
  if vim.wait(3000, ready, 10) then
    return true
  end
  vim.notify("annox: the server did not attach to this buffer; see :checkhealth vim.lsp", vim.log.levels.WARN)
  return false
end

local subcommands = {
  init = function()
    M.init()
  end,
  comment = function(o)
    M.comment({ visual = o.range > 0 })
  end,
  draft = function(o)
    M.comment({ visual = o.range > 0, ["local"] = true })
  end,
  highlight = function(o)
    M.highlight({ visual = o.range > 0 })
  end,
  publish = function(o)
    M.publish({ all = o.bang })
  end,
  retarget = function(o)
    M.retarget({ visual = o.range > 0 })
  end,
  reattach = function(o)
    M.reattach({ visual = o.range > 0 })
  end,
  conflicts = function()
    M.resolve_conflict()
  end,
  history = function()
    M.history()
  end,
  suggest = function(o)
    M.suggest({ visual = o.range > 0 })
  end,
  suggesting = function()
    M.suggest_mode()
  end,
  overlay = function(o)
    local arg = o.fargs[2]
    if arg ~= nil and arg ~= "on" and arg ~= "off" then
      return vim.notify("annox: usage: :Annox overlay [on|off]", vim.log.levels.ERROR)
    end
    M.overlay({ enable = arg and arg == "on" })
  end,
  edit = function()
    M.edit()
  end,
  reply = function()
    M.reply()
  end,
  resolve = function(o)
    M.resolve({ visual = o.range > 0, all = o.bang })
  end,
  reopen = function()
    M.reopen()
  end,
  revert = function()
    M.revert()
  end,
  accept = function(o)
    M.accept({ visual = o.range > 0, all = o.bang })
  end,
  reject = function(o)
    M.reject({ visual = o.range > 0, all = o.bang })
  end,
  thread = function()
    M.thread()
  end,
  orphans = function(o)
    M.orphans({ visual = o.range > 0 })
  end,
  list = function()
    M.list()
  end,
  commit = function()
    M.commit()
  end,
}

function M.setup(opts)
  store.config = vim.tbl_deep_extend("force", store.config, opts or {})
  vim.g.annox_overlay = store.overlay_shown
  set_highlights()
  vim.api.nvim_create_autocmd("ColorScheme", { callback = set_highlights })
  vim.lsp.config("annox", {
    cmd = store.config.cmd,
    root_dir = function(bufnr, on_dir)
      local root = vim.fs.root(bufnr, { ".annox" })
      if root then
        on_dir(root)
      end
    end,
    capabilities = { experimental = { annox = { version = "0.1" } } },
    init_options = { annox = { diagnostics = false, author = store.config.author } },
    handlers = {
      ["annox/didChangeAnnotations"] = on_annotations,
      ["annox/didChangePresence"] = M.on_presence,
      ["workspace/applyEdit"] = on_apply_edit,
    },
  })
  vim.lsp.enable("annox")
  vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI", "BufEnter" }, {
    group = vim.api.nvim_create_augroup("annox_presence", { clear = true }),
    callback = send_presence,
  })
  -- A window keeps its 'winhighlight' when it switches buffers; re-sync it.
  vim.api.nvim_create_autocmd({ "BufWinEnter", "WinEnter" }, {
    group = vim.api.nvim_create_augroup("annox_suggesting_tint", { clear = true }),
    callback = function()
      sync_tint(vim.api.nvim_get_current_win())
    end,
  })
  -- The hover (`K`) merges every server's answer into one float, with no
  -- hook per server, and plugins like noice.nvim draw it in windows opened
  -- without autocommands. So check each float as it is drawn, once per
  -- change, for the diff of a suggestion under the cursor.
  local seen = {}
  vim.api.nvim_set_decoration_provider(vim.api.nvim_create_namespace("annox_hover"), {
    on_win = function(_, win, fbuf)
      local tick = vim.api.nvim_buf_get_changedtick(fbuf)
      if seen[fbuf] ~= tick and vim.api.nvim_win_get_config(win).relative ~= "" then
        seen[fbuf] = tick
        vim.schedule(function()
          style_hover(win, fbuf)
        end)
      end
      return false
    end,
  })
  vim.api.nvim_create_user_command("Annox", function(o)
    local fn = subcommands[o.fargs[1]]
    if not fn then
      return vim.notify("annox: unknown subcommand " .. tostring(o.fargs[1]), vim.log.levels.ERROR)
    end
    if o.fargs[1] ~= "init" and o.fargs[1] ~= "overlay" and not wait_ready(vim.api.nvim_get_current_buf()) then
      return
    end
    fn(o)
  end, {
    nargs = "+",
    range = true,
    bang = true,
    complete = function(lead, line)
      local args = vim.split(line, "%s+", { trimempty = true })
      local words = vim.tbl_keys(subcommands)
      if #args > 2 or (#args == 2 and line:match("%s$")) then
        words = args[2] == "overlay" and { "on", "off" } or {}
      end
      return vim.tbl_filter(function(w)
        return vim.startswith(w, lead)
      end, words)
    end,
  })
end

return M
