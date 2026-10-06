--- Suggestion mode. While it is on, edits to the buffer become suggestions
--- (§4.6). Whenever the user pauses (leaving insert mode, or after a
--- normal-mode change), the buffer is compared with its text from before the
--- edit. Each changed stretch, widened to whole words, becomes a new
--- suggestion, or extends one made in this session that it touches (a
--- `retarget` event), and the buffer goes back to its original text. The file
--- therefore never contains suggested text.
---
--- State per buffer, in `store.suggesting`: { base = lines, views = { [id] =
--- AnnotationView }, queue = { op… }, busy = boolean, undo = { entry… } }.

local store = require("annox.store")
local util = require("annox.util")
local render = require("annox.render").render

local client_for = util.client_for
local request = util.request

local M = {}

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
    return render(bufnr)
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
  render(bufnr)
end

M.rebase = rebase
M.sync_tint = sync_tint

return M
