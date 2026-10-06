--- annox for Neovim: a thin client for the annox language server (spec §6).
---
--- The server owns all annox logic. This plugin renders the annotations it
--- pushes (`annox/didChangeAnnotations`) and sends user actions as `annox/*`
--- requests. Suggested edits come back as `workspace/applyEdit`, which
--- Neovim's LSP client applies to the buffer.

local M = {}

local ns = vim.api.nvim_create_namespace("annox")
-- Diff blocks styled in floats, some opened by other code: kept apart from
-- `ns`, so clearing them never touches the marks of an annotated buffer.
local diff_ns = vim.api.nvim_create_namespace("annox_diff")

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

local presence_ns = vim.api.nvim_create_namespace("annox_presence")

--- Others' presence, as last pushed by the server.
M.peers = {}

--- Latest state pushed by the server, per buffer: { annotations, document }.
M.state = {}

--- Whether annotations and others' cursors are drawn. Toggled with
--- `:Annox overlay`; buffers in suggestion mode draw theirs regardless.
M.overlay_shown = true

--- `color` blended over the editor background, `alpha` of the way, as "#rrggbb".
--- A transparent background counts as black, or white with a light 'background'.
local function tint(color, alpha)
  local bg = vim.api.nvim_get_hl(0, { name = "Normal", link = false }).bg
    or (vim.o.background == "light" and 0xffffff or 0)
  local function channel(shift)
    local c, b = bit.band(bit.rshift(color, shift), 0xff), bit.band(bit.rshift(bg, shift), 0xff)
    return math.floor(b + (c - b) * alpha + 0.5)
  end
  return string.format("#%02x%02x%02x", channel(16), channel(8), channel(0))
end

local function set_highlights()
  -- Annotated text gets a background tint in the diagnostic color, so it
  -- isn't mistaken for a diagnostic. Problems (stale, conflict) keep the
  -- undercurl. Without true colors, fall back to the underlines.
  -- A comment is also underlined, to say there's a note to read; a highlight
  -- (a comment with nothing to read) is only tinted.
  local tinted = { AnnoxComment = "Info", AnnoxHighlight = "Info", AnnoxSuggestion = "Hint", AnnoxLocal = "Ok" }
  for group, severity in pairs(tinted) do
    local fg = vim.api.nvim_get_hl(0, { name = "Diagnostic" .. severity, link = false }).fg
    if vim.o.termguicolors and fg then
      local underline = group == "AnnoxComment"
      vim.api.nvim_set_hl(
        0,
        group,
        { default = true, bg = tint(fg, 0.2), underline = underline, sp = underline and fg or nil }
      )
    else
      vim.api.nvim_set_hl(0, group, { default = true, link = "DiagnosticUnderline" .. severity })
    end
  end
  local links = {
    AnnoxStale = "DiagnosticUnderlineWarn",
    AnnoxConflict = "DiagnosticUnderlineError",
    AnnoxVirtualText = "Comment",
    AnnoxSign = "DiagnosticSignInfo",
    AnnoxPresence = "DiagnosticVirtualTextHint",
    AnnoxPresenceRange = "Visual",
    AnnoxInsertion = "Added",
    AnnoxOrphans = "DiagnosticVirtualTextWarn",
  }
  for group, link in pairs(links) do
    vim.api.nvim_set_hl(0, group, { default = true, link = link })
  end
  local removed = vim.api.nvim_get_hl(0, { name = "Removed", link = false })
  vim.api.nvim_set_hl(0, "AnnoxDeletion", { default = true, strikethrough = M.config.strikethrough, fg = removed.fg })
  -- The old text above the suggestion edit window.
  vim.api.nvim_set_hl(0, "AnnoxEditOriginal", { default = true, fg = removed.fg })
  -- A suggestion's lines in the thread's diff block: plain text over the diff
  -- background, instead of the code block's color.
  local text = vim.api.nvim_get_hl(0, { name = "NormalFloat", link = false }).fg
    or vim.api.nvim_get_hl(0, { name = "Normal", link = false }).fg
  vim.api.nvim_set_hl(0, "AnnoxThreadDeletion", { default = true, fg = text })
  vim.api.nvim_set_hl(0, "AnnoxThreadInsertion", { default = true, fg = text })
  -- The words that changed within a suggestion, diff-so-fancy style. Only a
  -- background, so the text keeps its color, and a light one, as inline that
  -- text is red or green itself.
  local added_fg = vim.api.nvim_get_hl(0, { name = "Added", link = false }).fg
  if vim.o.termguicolors and removed.fg and added_fg then
    vim.api.nvim_set_hl(0, "AnnoxWordDeletion", { default = true, bg = tint(removed.fg, 0.3) })
    vim.api.nvim_set_hl(0, "AnnoxWordInsertion", { default = true, bg = tint(added_fg, 0.3) })
  else
    vim.api.nvim_set_hl(0, "AnnoxWordDeletion", { default = true, link = "DiffDelete" })
    vim.api.nvim_set_hl(0, "AnnoxWordInsertion", { default = true, link = "DiffAdd" })
  end
  -- Suggestion mode tints the number column and cursor line toward "Added".
  local added = vim.api.nvim_get_hl(0, { name = "Added", link = false }).fg
  vim.api.nvim_set_hl(0, "AnnoxSuggestingCursorLineNr", { default = true, link = "Added" })
  if vim.o.termguicolors and added then
    vim.api.nvim_set_hl(0, "AnnoxSuggestingLineNr", { default = true, fg = tint(added, 0.5) })
    vim.api.nvim_set_hl(0, "AnnoxSuggestingCursorLine", { default = true, bg = tint(added, 0.1) })
  else
    vim.api.nvim_set_hl(0, "AnnoxSuggestingLineNr", { default = true, link = "Added" })
    vim.api.nvim_set_hl(0, "AnnoxSuggestingCursorLine", { default = true, link = "CursorLine" })
  end
end

local function client_for(bufnr)
  return vim.lsp.get_clients({ bufnr = bufnr, name = "annox" })[1]
end

--- Byte column of an LSP position in `bufnr`.
local function byte_col(bufnr, pos, encoding)
  local line = vim.api.nvim_buf_get_lines(bufnr, pos.line, pos.line + 1, false)[1] or ""
  local ok, col = pcall(vim.str_byteindex, line, encoding, pos.character, false)
  return ok and col or #line
end

local function first_line(text)
  return type(text) == "string" and text:match("^[^\n]*") or nil
end

local function highlight_group(a)
  if type(a.conflicts) == "table" and next(a.conflicts) then
    return "AnnoxConflict"
  elseif a["local"] then
    return "AnnoxLocal"
  elseif a.kind == "suggestion" then
    return a.applicable and "AnnoxSuggestion" or "AnnoxStale"
  elseif first_line(a.body) == nil and #(a.replies or {}) == 0 then
    return "AnnoxHighlight"
  end
  return "AnnoxComment"
end

--- Suggestion mode state per buffer (see "Suggestion mode" below).
M.suggesting = {}

--- Whether suggestions in `bufnr` are drawn as struck-through and inserted
--- text instead of underlined.
local function inline(bufnr, a)
  return a.kind == "suggestion" and a.applicable and (M.config.inline_suggestions or M.suggesting[bufnr] ~= nil)
end

--- The number of open annotations in `bufnr` whose text could not be found,
--- e.g. for a statusline.
function M.orphan_count(bufnr)
  local state = M.state[bufnr or vim.api.nvim_get_current_buf()]
  local n = 0
  for _, a in ipairs(state and state.annotations or {}) do
    if a.resolution and a.resolution.state == "orphaned" then
      n = n + 1
    end
  end
  return n
end

--- `s` split into words, runs of spaces, newlines, and single other bytes,
--- with the 0-based byte offset of each.
local function tokens(s)
  local toks, offs, pos = {}, {}, 1
  while pos <= #s do
    local tok = s:match("^[%w_\128-\255]+", pos) or s:match("^[ \t]+", pos) or s:sub(pos, pos)
    table.insert(toks, tok)
    table.insert(offs, pos - 1)
    pos = pos + #tok
  end
  return toks, offs
end

--- The words that differ between `old` and `new`, diff-so-fancy style:
--- { del, add }, each a list of { row, start_col, end_col } (0-based, bytes)
--- into the lines of that text. Empty when the two share no word, as then
--- everything changed.
local function word_changes(old, new)
  local del, add = {}, {}
  if not M.config.word_diff or old == "" or new == "" then
    return del, add
  end
  local ta, oa = tokens(old)
  local tb, ob = tokens(new)
  -- One token per line for the line diff; a newline token can't be a line.
  local function joined(toks)
    return table.concat(
      vim.tbl_map(function(t)
        return t == "\n" and "\1" or t
      end, toks),
      "\n"
    ) .. "\n"
  end
  local function add_range(out, s, toks, offs, first, count)
    if count == 0 then
      return
    end
    local a0, a1 = offs[first], offs[first + count - 1] + #toks[first + count - 1]
    -- Join with the previous change when only spaces separate them.
    local last = out[#out]
    if last and s:sub(last[2] + 1, a0):match("^[ \t]*$") then
      last[2] = a1
    else
      table.insert(out, { a0, a1 })
    end
  end
  local ra, rb = {}, {}
  local diff = vim.text.diff or vim.diff
  local hunks = diff(joined(ta), joined(tb), { result_type = "indices" })
  -- Without a word in common everything changed, and marking it all is noise.
  local changed = {}
  for _, h in ipairs(hunks) do
    for i = h[1], h[1] + h[2] - 1 do
      changed[i] = true
    end
  end
  local shared = false
  for i, t in ipairs(ta) do
    shared = shared or (not changed[i] and t:find("^[%w_\128-\255]") ~= nil)
  end
  if not shared then
    return del, add
  end
  for _, h in ipairs(hunks) do
    add_range(ra, old, ta, oa, h[1], h[2])
    add_range(rb, new, tb, ob, h[3], h[4])
  end
  -- Byte ranges to per-line column ranges.
  local function split(s, ranges, out)
    local lines = vim.split(s, "\n")
    local starts = { 0 }
    for i, l in ipairs(lines) do
      starts[i + 1] = starts[i] + #l + 1
    end
    local row = 1
    for _, r in ipairs(ranges) do
      while starts[row + 1] <= r[1] do
        row = row + 1
      end
      local i = row
      while i <= #lines and starts[i] < r[2] do
        local s0, s1 = math.max(r[1], starts[i]) - starts[i], math.min(r[2], starts[i] + #lines[i]) - starts[i]
        if s1 > s0 then
          table.insert(out, { i - 1, s0, s1 })
        end
        i = i + 1
      end
    end
  end
  split(old, ra, del)
  split(new, rb, add)
  return del, add
end

--- `text` as inline virtual text chunks, newlines as "↵", with the changed
--- words `add` (from `word_changes`) tinted.
local function inserted_chunks(text, add)
  local chunks = {}
  for row, line in ipairs(vim.split(text, "\n")) do
    if row > 1 then
      table.insert(chunks, { "↵", "AnnoxInsertion" })
    end
    local col = 0
    for _, c in ipairs(add) do
      if c[1] == row - 1 then
        table.insert(chunks, { line:sub(col + 1, c[2]), "AnnoxInsertion" })
        table.insert(chunks, { line:sub(c[2] + 1, c[3]), { "AnnoxInsertion", "AnnoxWordInsertion" } })
        col = c[3]
      end
    end
    table.insert(chunks, { line:sub(col + 1), "AnnoxInsertion" })
  end
  return vim.tbl_filter(function(c)
    return c[1] ~= ""
  end, chunks)
end

--- Draws the annotations of `bufnr` as extmarks.
function M.render(bufnr)
  vim.api.nvim_buf_clear_namespace(bufnr, ns, 0, -1)
  if not M.overlay_shown and not M.suggesting[bufnr] then
    return
  end
  local state = M.state[bufnr]
  local client = client_for(bufnr)
  if not state or not client then
    return
  end
  local enc = client.offset_encoding
  local line_count = vim.api.nvim_buf_line_count(bufnr)
  -- Orphaned annotations have no place in the text, so say so above it (§3.7.3).
  local orphans = M.orphan_count(bufnr)
  if orphans > 0 then
    local text =
      string.format("⚠ %d annotation%s could not be located (:Annox orphans)", orphans, orphans == 1 and "" or "s")
    vim.api.nvim_buf_set_extmark(
      bufnr,
      ns,
      0,
      0,
      { virt_lines = { { { text, "AnnoxOrphans" } } }, virt_lines_above = true }
    )
  end
  for _, a in ipairs(state.annotations) do
    local r = a.resolution and a.resolution.range
    if r and r.start.line < line_count then
      local group = highlight_group(a)
      local sl, el = r.start.line, math.min(r["end"].line, line_count - 1)
      local sc, ec = byte_col(bufnr, r.start, enc), byte_col(bufnr, r["end"], enc)
      local mark = {
        sign_text = a["local"] and "✎" or a.kind == "suggestion" and "±" or "»",
        sign_hl_group = "AnnoxSign",
        priority = 150,
      }
      local label = a.kind == "suggestion" and ("→ " .. (a.edit and a.edit.replacement or ""))
        or first_line(a.body)
        or a.label
      if inline(bufnr, a) then
        -- Deleted text struck through, followed by the inserted text.
        if sl ~= el or sc ~= ec then
          mark.end_row, mark.end_col, mark.hl_group = el, ec, "AnnoxDeletion"
        end
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
        local replacement = a.edit and a.edit.replacement or ""
        -- The words that changed get a stronger tint on both sides.
        local ok, old = pcall(vim.api.nvim_buf_get_text, bufnr, sl, sc, el, ec, {})
        local del, add = word_changes(ok and table.concat(old, "\n") or "", replacement)
        for _, c in ipairs(del) do
          pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl + c[1], (c[1] == 0 and sc or 0) + c[2], {
            end_col = (c[1] == 0 and sc or 0) + c[3],
            hl_group = "AnnoxWordDeletion",
            priority = 160,
          })
        end
        if replacement ~= "" then
          pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, el, ec, {
            virt_text = inserted_chunks(replacement, add),
            virt_text_pos = "inline",
            right_gravity = false,
          })
        end
        label = first_line(a.body)
      elseif sl == el and sc == ec then
        mark.virt_text = { { "◆", group } }
        mark.virt_text_pos = "inline"
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
      else
        mark.end_row, mark.end_col, mark.hl_group = el, ec, group
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
      end
      if M.config.virtual_text and label then
        local replies = #(a.replies or {})
        local text = replies > 0 and string.format("%s (+%d)", label, replies) or label
        if a["local"] then
          text = "[draft] " .. text
        end
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, 0, {
          virt_text = { { "  " .. text, "AnnoxVirtualText" } },
          virt_text_pos = "eol",
        })
      end
    end
  end
end

--- Draws others' cursors in `bufnr` (§7.8.2).
function M.render_presence(bufnr)
  vim.api.nvim_buf_clear_namespace(bufnr, presence_ns, 0, -1)
  local client = client_for(bufnr)
  if not client or not M.overlay_shown then
    return
  end
  local uri = vim.uri_from_bufnr(bufnr)
  local line_count = vim.api.nvim_buf_line_count(bufnr)
  for _, peer in ipairs(M.peers) do
    local r = peer.range
    if peer.textDocument and peer.textDocument.uri == uri and r and r.start.line < line_count then
      local author = peer.author or {}
      local name = author.name or author.id or "someone"
      local sl, el = r.start.line, math.min(r["end"].line, line_count - 1)
      local sc, ec = byte_col(bufnr, r.start, client.offset_encoding), byte_col(bufnr, r["end"], client.offset_encoding)
      if sl ~= el or sc ~= ec then
        pcall(vim.api.nvim_buf_set_extmark, bufnr, presence_ns, sl, sc, {
          end_row = el,
          end_col = ec,
          hl_group = "AnnoxPresenceRange",
        })
      end
      pcall(vim.api.nvim_buf_set_extmark, bufnr, presence_ns, sl, sc, {
        virt_text = { { "▏" .. name, "AnnoxPresence" } },
        virt_text_pos = "inline",
      })
    end
  end
end

--- Handles `annox/didChangePresence` (§6.6.3).
function M.on_presence(_, result)
  M.peers = result.peers or {}
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) then
      M.render_presence(b)
    end
  end
end

--- Sends the cursor as presence, at most every 150 ms (§7.8.1).
local presence_timer
local function send_presence()
  if not M.config.presence then
    return
  end
  presence_timer = presence_timer or vim.uv.new_timer()
  presence_timer:stop()
  presence_timer:start(
    150,
    0,
    vim.schedule_wrap(function()
      local bufnr = vim.api.nvim_get_current_buf()
      local client = client_for(bufnr)
      if not client then
        return
      end
      local row, col = unpack(vim.api.nvim_win_get_cursor(0))
      local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
      local pos =
        { line = row - 1, character = vim.str_utfindex(line, client.offset_encoding, math.min(col, #line), false) }
      client:notify("annox/setPresence", {
        textDocument = { uri = vim.uri_from_bufnr(bufnr) },
        selection = { start = pos, ["end"] = pos },
      })
    end)
  )
end

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
  M.state[bufnr] = { annotations = annotations, document = result.document }
  local s = M.suggesting[bufnr]
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
end

--- Sends an `annox/*` request for the current buffer.
local function request(bufnr, method, params, done)
  local client = client_for(bufnr)
  if not client then
    vim.notify("annox: no annox server attached to this buffer", vim.log.levels.WARN)
    return
  end
  client:request(method, params, function(err, result)
    if err then
      vim.notify(string.format("annox: %s failed: %s", method, err.message), vim.log.levels.ERROR)
    elseif done then
      done(result)
    end
  end, bufnr)
end

--- The LSP position of the cursor.
local function cursor_position(bufnr, encoding)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
  return { line = row - 1, character = vim.str_utfindex(line, encoding, math.min(col, #line), false) }
end

--- The LSP range of the last visual selection.
local function visual_range(bufnr, encoding)
  local s, e = vim.fn.getpos("'<"), vim.fn.getpos("'>")
  local function position(row, byte)
    local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
    byte = math.min(byte, #line)
    return { line = row - 1, character = vim.str_utfindex(line, encoding, byte, false) }
  end
  local end_line = vim.api.nvim_buf_get_lines(bufnr, e[2] - 1, e[2], false)[1] or ""
  local end_byte = #end_line
  if e[3] <= #end_line then
    end_byte = e[3] + vim.str_utf_end(end_line, e[3]) -- one past the last selected character
  end
  return { start = position(s[2], s[3] - 1), ["end"] = position(e[2], end_byte) }
end

local function before(a, b)
  return a.line < b.line or (a.line == b.line and a.character <= b.character)
end

--- Annotations whose range contains the cursor.
local function under_cursor(bufnr)
  local state, client = M.state[bufnr], client_for(bufnr)
  if not state or not client then
    return {}
  end
  local enc = client.offset_encoding
  local pos = cursor_position(bufnr, enc)
  -- The cursor's character ends where an empty range starts: an insertion
  -- or point comment is drawn between the two characters, so both count.
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
  local after = col < #line and col + vim.str_utf_end(line, col + 1) + 1 or col
  local hits = {}
  for _, a in ipairs(state.annotations) do
    local r = a.resolution and a.resolution.range
    local empty = r and r.start.line == r["end"].line and r.start.character == r["end"].character
    local touching = empty and r.start.line == row - 1 and byte_col(bufnr, r.start, enc) == after
    if r and (touching or (before(r.start, pos) and before(pos, r["end"]))) then
      table.insert(hits, a)
    end
  end
  return hits
end

local function buffer_annotations(bufnr, keep)
  return vim.tbl_filter(keep, (M.state[bufnr] or {}).annotations or {})
end

local function describe(a)
  if a.kind == "suggestion" then
    return string.format("suggestion → %s", a.edit and a.edit.replacement or "")
  end
  return string.format("comment: %s", first_line(a.body) or a.label or "(highlight)")
end

--- Calls `fn(id)` with `opts.annotation`, or with the annotation under the
--- cursor (asking if there are several), filtered by `keep`.
local function with_annotation(opts, keep, fn)
  if opts.annotation then
    return fn(opts.annotation)
  end
  local hits = vim.tbl_filter(keep or function()
    return true
  end, under_cursor(vim.api.nvim_get_current_buf()))
  if #hits == 0 then
    vim.notify("annox: no matching annotation under the cursor", vim.log.levels.INFO)
  elseif #hits == 1 then
    fn(hits[1].id)
  else
    vim.ui.select(hits, { prompt = "Annotation", format_item = describe }, function(a)
      if a then
        fn(a.id)
      end
    end)
  end
end

--- Calls `fn(text)` with `value`, or asks for it.
local function with_input(value, prompt, default, fn)
  if value then
    return fn(value)
  end
  vim.ui.input({ prompt = prompt, default = default }, function(text)
    if text and text ~= "" then
      fn(text)
    end
  end)
end

local function target_range(bufnr, opts)
  local enc = client_for(bufnr).offset_encoding
  if opts.range then
    return opts.range
  elseif opts.visual then
    return visual_range(bufnr, enc)
  end
  local pos = cursor_position(bufnr, enc)
  return { start = pos, ["end"] = pos }
end

--- Creates a comment on `opts.range`, the visual selection, or the cursor.
--- opts: { body?, range?, visual?, ["local"]? }
function M.comment(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  local range = target_range(bufnr, opts)
  with_input(opts.body, "Comment: ", nil, function(body)
    request(bufnr, "annox/create", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      kind = "comment",
      range = range,
      body = body,
      ["local"] = opts["local"] or false,
    })
  end)
end

--- Highlights `opts.range` or the visual selection: a comment with no body.
--- opts: { range?, visual?, ["local"]? }
function M.highlight(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  if not opts.range and not opts.visual then
    return vim.notify("annox: select the text to highlight", vim.log.levels.WARN)
  end
  request(bufnr, "annox/create", {
    textDocument = { uri = vim.uri_from_bufnr(bufnr) },
    kind = "comment",
    range = target_range(bufnr, opts),
    ["local"] = opts["local"] or false,
  })
end

--- Suggests replacing the selection (or inserting at the cursor).
--- opts: { replacement?, range?, visual? }
function M.suggest(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  local range = target_range(bufnr, opts)
  local lines = vim.api.nvim_buf_get_text(
    bufnr,
    range.start.line,
    byte_col(bufnr, range.start, client_for(bufnr).offset_encoding),
    range["end"].line,
    byte_col(bufnr, range["end"], client_for(bufnr).offset_encoding),
    {}
  )
  with_input(opts.replacement, "Replace with: ", table.concat(lines, "\n"), function(replacement)
    request(bufnr, "annox/create", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      kind = "suggestion",
      range = range,
      replacement = replacement,
    })
  end)
end

local thread_lines, open_thread, style_hover

local function find_annotation(bufnr, id)
  for _, a in ipairs((M.state[bufnr] or {}).annotations or {}) do
    if a.id == id then
      return a
    end
  end
end

--- Replies to the thread under the cursor, showing the thread while the reply
--- is typed. opts: { annotation?, body? }
function M.reply(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  with_annotation(opts, nil, function(id)
    local send = function(body)
      request(bufnr, "annox/reply", { parent = id, body = body })
    end
    local a = find_annotation(bufnr, id)
    if opts.body or not a then
      return with_input(opts.body, "Reply: ", nil, send)
    end
    local _, win = open_thread(a, bufnr)
    -- The preview closes when its buffer is left; keep it up while the
    -- input (often a float of its own) has focus.
    pcall(vim.api.nvim_del_augroup_by_name, "nvim.preview_window_" .. win)
    vim.ui.input({ prompt = "Reply: " }, function(body)
      if vim.api.nvim_win_is_valid(win) then
        vim.api.nvim_win_close(win, true)
      end
      if body and body ~= "" then
        send(body)
      end
    end)
  end)
end

local function status_action(status, keep)
  return function(opts)
    opts = opts or {}
    local bufnr = vim.api.nvim_get_current_buf()
    with_annotation(opts, keep, function(id)
      request(bufnr, "annox/setStatus", { annotation = id, status = status })
    end)
  end
end

local function is_kind(kind, status)
  return function(a)
    return a.kind == kind and (status == nil or a.status == status)
  end
end

M.reopen = status_action("open", function(a)
  return a.status ~= "open"
end)

--- Undoes an accepted suggestion with a new suggestion that restores the
--- original text (§4.3.4). `opts.annotation` names it; otherwise the buffer's
--- accepted suggestions are offered, newest first. `opts.accept` applies the
--- revert now (true) or leaves it open for review (false); otherwise you're
--- asked.
function M.revert(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local function revert(id, accept)
    request(bufnr, "annox/revert", { annotation = id, accept = accept })
  end
  local function choose(id)
    if opts.accept ~= nil then
      return revert(id, opts.accept)
    end
    local choices = { "Revert now", "Suggest reverting (leave it open for review)" }
    vim.ui.select(choices, { prompt = "Revert the accepted suggestion" }, function(_, i)
      if i then
        revert(id, i == 1)
      end
    end)
  end
  if opts.annotation then
    return choose(opts.annotation)
  end
  -- Closed annotations aren't pushed, so ask for them once, then go back to
  -- open ones only, which the next pushes follow (§6.6.3). A push in between
  -- may include closed ones, which `on_annotations` drops.
  local doc = { uri = vim.uri_from_bufnr(bufnr) }
  request(bufnr, "annox/annotations", { textDocument = doc, includeClosed = true }, function(result)
    request(bufnr, "annox/annotations", { textDocument = doc })
    -- Leave out those already reverted, and mark those with a revert open.
    local reverts = {}
    for _, a in ipairs(result.annotations) do
      if type(a.reverts) == "string" and (a.status == "accepted" or a.status == "open") then
        reverts[a.reverts] = reverts[a.reverts] == "accepted" and "accepted" or a.status
      end
    end
    local accepted = vim.tbl_filter(function(a)
      return a.kind == "suggestion" and a.status == "accepted" and reverts[a.id] ~= "accepted"
    end, result.annotations)
    if #accepted == 0 then
      return vim.notify("annox: no accepted suggestions to revert in this buffer", vim.log.levels.INFO)
    end
    table.sort(accepted, function(a, b)
      return (a.created or "") > (b.created or "")
    end)
    local function format(a)
      return describe(a) .. (reverts[a.id] == "open" and " (revert suggested)" or "")
    end
    vim.ui.select(accepted, { prompt = "Accepted suggestion", format_item = format }, function(a)
      if a then
        choose(a.id)
      end
    end)
  end)
end

--- The open annotations of `kind` shown in the buffer: those touching the
--- last visual selection with `visual`, and all of them otherwise. Nil if no
--- server is attached.
local function open_in_buffer(bufnr, kind, visual)
  local client = client_for(bufnr)
  if not client then
    return request(bufnr)
  end
  local sel = visual and visual_range(bufnr, client.offset_encoding)
  return buffer_annotations(bufnr, function(a)
    local r = a.resolution and a.resolution.range
    return a.kind == kind
      and a.status == "open"
      and r ~= nil
      and (not sel or (before(r.start, sel["end"]) and before(sel.start, r["end"])))
  end)
end

--- Accepts several suggestions as one edit, which a single undo reverts
--- (§6.6.2), and reports the ones the server skipped. Suggestions found by
--- partial context (steps 3 and 5) need `confirmed`, which is asked for once
--- for the whole batch if it isn't given (§4.3).
local function accept_all(bufnr, suggestions, confirmed, fresh)
  if #suggestions == 0 then
    return vim.notify("annox: no suggestions to accept", vim.log.levels.INFO)
  end
  if not fresh then
    -- Pushed state can lag behind the buffer; resolve against it now.
    local params = { textDocument = { uri = vim.uri_from_bufnr(bufnr) } }
    return request(bufnr, "annox/annotations", params, function(result)
      local wanted = {}
      for _, a in ipairs(suggestions) do
        wanted[a.id] = true
      end
      local current = vim.tbl_filter(function(a)
        return wanted[a.id] and a.resolution and a.resolution.range ~= nil
      end, result.annotations)
      accept_all(bufnr, current, confirmed, true)
    end)
  end
  local partial = #vim.tbl_filter(function(a)
    return a.resolution.step == 3 or a.resolution.step == 5
  end, suggestions)
  if partial > 0 and confirmed == nil then
    local all = string.format("Accept all %d", #suggestions)
    local rest = string.format("Accept only the other %d", #suggestions - partial)
    local choices = partial < #suggestions and { all, rest, "Cancel" } or { all, "Cancel" }
    local prompt = string.format(
      "%d of these suggestion%s moved because the text around %s changed. Check %s shown in the right place.",
      partial,
      partial == 1 and "" or "s",
      partial == 1 and "it" or "them",
      partial == 1 and "it is" or "they are"
    )
    return vim.ui.select(choices, { prompt = prompt }, function(choice)
      if choice == all or choice == rest then
        accept_all(bufnr, suggestions, choice == all, true)
      end
    end)
  end
  table.sort(suggestions, function(a, b)
    return before(a.resolution.range.start, b.resolution.range.start)
  end)
  local ids = vim.tbl_map(function(a)
    return a.id
  end, suggestions)
  request(bufnr, "annox/acceptAll", { annotations = ids, confirmed = confirmed or false }, function(result)
    local accepted, skipped = 0, {}
    for _, r in ipairs(result.results or {}) do
      if r.error then
        local message = type(r.error) == "table" and r.error.message or tostring(r.error)
        skipped[message] = (skipped[message] or 0) + 1
      else
        accepted = accepted + 1
      end
    end
    local parts = { string.format("accepted %d", accepted) }
    for message, n in pairs(skipped) do
      table.insert(parts, string.format("skipped %d (%s)", n, message))
    end
    local level = next(skipped) and vim.log.levels.WARN or vim.log.levels.INFO
    vim.notify("annox: " .. table.concat(parts, "; "), level)
  end)
end

--- Accepts the suggestion under the cursor: the server sends the edit back
--- as `workspace/applyEdit` (§6.6.2). With `visual`, accepts every open
--- suggestion touching the last visual selection, and with `all`, every open
--- suggestion in the buffer. `confirmed` answers the prompt about suggestions
--- found by partial context in advance. opts: { annotation?, visual?, all?, confirmed? }
function M.accept(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if opts.visual or opts.all then
    local suggestions = open_in_buffer(bufnr, "suggestion", opts.visual)
    return suggestions and accept_all(bufnr, suggestions, opts.confirmed)
  end
  with_annotation(opts, is_kind("suggestion", "open"), function(id)
    request(bufnr, "annox/accept", { annotation = id })
  end)
end

--- Sets `status` on every annotation in `items`, one `status` event each,
--- after asking once for more than one unless `confirmed`. `verb` and `noun`
--- word the prompt and the report, e.g. "Reject" and "suggestion".
local function set_status_all(bufnr, items, status, verb, noun, confirmed)
  if #items == 0 then
    return vim.notify(string.format("annox: no %ss to %s", noun, verb:lower()), vim.log.levels.INFO)
  end
  local function run()
    local client, pending, done = client_for(bufnr), #items, 0
    for _, a in ipairs(items) do
      client:request("annox/setStatus", { annotation = a.id, status = status }, function(err)
        done = done + (err and 0 or 1)
        pending = pending - 1
        if pending == 0 then
          local failed = #items - done
          local message = string.format("annox: %s %d", status, done)
          if failed > 0 then
            message = message .. string.format("; %d failed", failed)
          end
          vim.notify(message, failed > 0 and vim.log.levels.WARN or vim.log.levels.INFO)
        end
      end, bufnr)
    end
  end
  if #items == 1 or confirmed then
    return run()
  end
  local yes = string.format("%s all %d", verb, #items)
  vim.ui.select({ yes, "Cancel" }, { prompt = string.format("%s %d %ss?", verb, #items, noun) }, function(choice)
    if choice == yes then
      run()
    end
  end)
end

--- A status action on the annotation under the cursor that, with `visual` or
--- `all`, applies to every open annotation of `kind` touching the last visual
--- selection or in the buffer. These don't touch the text, so each is its own
--- `status` event and can be reopened. opts: { annotation?, visual?, all?, confirmed? }
local function bulk_status_action(kind, status, verb)
  local one = status_action(status, is_kind(kind, "open"))
  return function(opts)
    opts = opts or {}
    if not (opts.visual or opts.all) then
      return one(opts)
    end
    local bufnr = vim.api.nvim_get_current_buf()
    local items = open_in_buffer(bufnr, kind, opts.visual)
    if items then
      set_status_all(bufnr, items, status, verb, kind, opts.confirmed)
    end
  end
end

M.reject = bulk_status_action("suggestion", "rejected", "Reject")
M.resolve = bulk_status_action("comment", "resolved", "Resolve")

--- The buffer text `a` resolves to, or "" when it has no range.
local function resolved_text(a, bufnr)
  local r = type(a.resolution) == "table" and a.resolution.range
  local client = client_for(bufnr)
  if type(r) ~= "table" or not client then
    return ""
  end
  local enc = client.offset_encoding
  local ok, text = pcall(
    vim.api.nvim_buf_get_text,
    bufnr,
    r.start.line,
    byte_col(bufnr, r.start, enc),
    r["end"].line,
    byte_col(bufnr, r["end"], enc),
    {}
  )
  return ok and table.concat(text, "\n") or ""
end

--- Highlights `changes` (from `word_changes`) in `buf`, offset by `row`/`col`.
local function mark_words(buf, nsid, changes, group, row, col)
  for _, c in ipairs(changes) do
    pcall(vim.api.nvim_buf_set_extmark, buf, nsid, row + c[1], col + c[2], {
      end_col = col + c[3],
      hl_group = group,
      priority = 210,
    })
  end
end

--- A change as a fenced `diff` block, so the old text shows red and the new
--- text green, like the server's hover.
local function diff_block(old, new)
  local longest = 2
  for run in (old .. "\n" .. new):gmatch("`+") do
    longest = math.max(longest, #run)
  end
  local fence = string.rep("`", longest + 1)
  local lines = { fence .. "diff" }
  for _, change in ipairs({ { "- ", old }, { "+ ", new } }) do
    if change[2] ~= "" then
      for _, line in ipairs(vim.split(change[2], "\n")) do
        table.insert(lines, change[1] .. line)
      end
    end
  end
  table.insert(lines, fence)
  return lines
end

--- The thread as markdown lines. A suggestion's old text is read from `bufnr`.
function thread_lines(a, bufnr)
  local lines = {}
  local function str(v)
    return type(v) == "string" and v or ""
  end
  local function person(x)
    local author = x.author or {}
    return string.format("**%s** · %s", author.name or author.id or "unknown", x.created or "")
  end
  if a.kind == "suggestion" then
    local stale = a.applicable and "" or " (stale)"
    table.insert(lines, string.format("**Suggestion%s:**", stale))
    local old = bufnr and resolved_text(a, bufnr) or ""
    vim.list_extend(lines, diff_block(old, type(a.edit) == "table" and str(a.edit.replacement) or ""))
    -- LSP decodes JSON null as vim.NIL, which is truthy.
    if type(a.reverts) == "string" then
      table.insert(lines, "_reverts an accepted suggestion_")
    end
    local by = type(a.retargetedBy) == "table" and a.retargetedBy.author
    if type(by) == "table" and by.id ~= (a.author or {}).id then
      table.insert(lines, string.format("_re-targeted by %s_", by.name or by.id))
    end
    table.insert(lines, "")
  end
  if type(a.conflicts) == "table" and next(a.conflicts) then
    table.insert(lines, "⚠ **Conflicting changes:** " .. table.concat(vim.tbl_keys(a.conflicts), ", "))
    table.insert(lines, "")
  end
  table.insert(lines, person(a) .. (a.status ~= "open" and ("  _" .. a.status .. "_") or ""))
  vim.list_extend(lines, vim.split(str(a.body), "\n"))
  for _, r in ipairs(a.replies or {}) do
    -- The markdown float expands a thematic break into a full-width rule.
    table.insert(lines, "---")
    table.insert(lines, person(r))
    vim.list_extend(lines, vim.split(str(r.body), "\n"))
  end
  return lines
end

--- The old and new text of suggestion `a`, as shown in its diff block.
local function suggestion_change(a, bufnr)
  local new = type(a.edit) == "table" and a.edit.replacement
  return { old = resolved_text(a, bufnr), new = type(new) == "string" and new or "" }
end

--- Styles the diff blocks of `changes` ({ old, new }) wherever they show in
--- `fbuf`, as "- " and "+ " lines: the changed lines in plain text, and the
--- words that changed in a stronger tint. Found by content, so it works on
--- floats other plugins render, whose code fences may be gone.
local function style_diff_blocks(fbuf, changes)
  local lines = vim.api.nvim_buf_get_lines(fbuf, 0, -1, false)
  for _, c in ipairs(changes) do
    -- The diff colors only the background; the code block's own text color
    -- would show through, so the changed lines get theirs.
    local want, groups = {}, {}
    for _, side in ipairs({ { "- ", c.old, "AnnoxThreadDeletion" }, { "+ ", c.new, "AnnoxThreadInsertion" } }) do
      if side[2] ~= "" then
        for _, l in ipairs(vim.split(side[2], "\n")) do
          table.insert(want, side[1] .. l)
          table.insert(groups, side[3])
        end
      end
    end
    for row = 0, #lines - #want do
      local found = #want > 0
      for i, w in ipairs(want) do
        if lines[row + i] ~= w then
          found = false
          break
        end
      end
      if found then
        for i, w in ipairs(want) do
          vim.api.nvim_buf_set_extmark(
            fbuf,
            diff_ns,
            row + i - 1,
            0,
            { end_col = #w, hl_group = groups[i], priority = 200 }
          )
        end
        local del, add = word_changes(c.old, c.new)
        local old_rows = c.old == "" and 0 or #vim.split(c.old, "\n")
        mark_words(fbuf, diff_ns, del, "AnnoxWordDeletion", row, 2)
        mark_words(fbuf, diff_ns, add, "AnnoxWordInsertion", row + old_rows, 2)
      end
    end
  end
end

--- Opens `a`'s thread in a floating window.
function open_thread(a, bufnr, opts)
  local fbuf, win = vim.lsp.util.open_floating_preview(
    thread_lines(a, bufnr),
    "markdown",
    vim.tbl_extend("force", { border = "rounded" }, opts or {})
  )
  -- Styled here, for `a`, which needn't be under the cursor.
  vim.b[fbuf].annox_own_float = true
  if a.kind == "suggestion" and bufnr then
    style_diff_blocks(fbuf, { suggestion_change(a, bufnr) })
  end
  return fbuf, win
end

--- Styles the diff blocks of the suggestions under the cursor in float
--- `win`, which some other code opened, such as the LSP hover.
function style_hover(win, fbuf)
  local source = vim.api.nvim_get_current_buf()
  if
    not vim.api.nvim_win_is_valid(win)
    or vim.api.nvim_win_get_buf(win) ~= fbuf
    or vim.b[fbuf].annox_own_float
    or M.state[fbuf]
    or source == fbuf
    or not client_for(source)
  then
    return
  end
  local changes = {}
  for _, a in ipairs(under_cursor(source)) do
    if a.kind == "suggestion" and a.status == "open" then
      table.insert(changes, suggestion_change(a, source))
    end
  end
  vim.api.nvim_buf_clear_namespace(fbuf, diff_ns, 0, -1)
  style_diff_blocks(fbuf, changes)
end

--- Shows the thread under the cursor in a floating window.
function M.thread(opts)
  with_annotation(opts or {}, nil, function(id)
    local bufnr = vim.api.nvim_get_current_buf()
    local a = find_annotation(bufnr, id)
    if a then
      open_thread(a, bufnr, { focus_id = "annox" })
    end
  end)
end

--- Lists orphaned annotations (§3.7.3) and shows the chosen thread.
function M.orphans()
  local bufnr = vim.api.nvim_get_current_buf()
  local orphans = vim.tbl_filter(function(a)
    return a.resolution and a.resolution.state == "orphaned"
  end, (M.state[bufnr] or {}).annotations or {})
  if #orphans == 0 then
    return vim.notify("annox: no orphaned annotations", vim.log.levels.INFO)
  end
  vim.ui.select(orphans, { prompt = "Orphaned annotations", format_item = describe }, function(a)
    if a then
      open_thread(a, bufnr)
    end
  end)
end

--- Puts the buffer's annotations in the quickfix list.
function M.list()
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  local items = {}
  for _, a in ipairs((M.state[bufnr] or {}).annotations or {}) do
    local r = a.resolution and a.resolution.range
    table.insert(items, {
      bufnr = bufnr,
      lnum = r and r.start.line + 1 or 1,
      col = r and client and byte_col(bufnr, r.start, client.offset_encoding) + 1 or 1,
      text = describe(a) .. (r and "" or " [orphaned]"),
    })
  end
  vim.fn.setqflist({}, " ", { title = "annox", items = items })
  vim.cmd.copen()
end

--- Calls `fn(annotation)` with `opts.annotation`'s view, the only candidate,
--- or the one the user picks.
local function pick(opts, candidates, prompt, fn)
  if opts.annotation then
    for _, a in ipairs(candidates) do
      if a.id == opts.annotation then
        return fn(a)
      end
    end
    return vim.notify("annox: that annotation doesn't apply here", vim.log.levels.WARN)
  end
  if #candidates == 0 then
    return vim.notify("annox: nothing to " .. prompt:lower(), vim.log.levels.INFO)
  elseif #candidates == 1 then
    return fn(candidates[1])
  end
  vim.ui.select(candidates, { prompt = prompt, format_item = describe }, function(a)
    if a then
      fn(a)
    end
  end)
end

--- Publishes local drafts (§5.11): the one under the cursor, or all of the
--- buffer's drafts with `opts.all`. opts: { annotation?, all? }
function M.publish(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local function is_local(a)
    return a["local"]
  end
  if opts.all then
    local ids = vim.tbl_map(function(a)
      return a.id
    end, buffer_annotations(bufnr, is_local))
    if #ids == 0 then
      return vim.notify("annox: no drafts to publish", vim.log.levels.INFO)
    end
    return request(bufnr, "annox/publish", { annotations = ids })
  end
  with_annotation(opts, is_local, function(id)
    request(bufnr, "annox/publish", { annotations = { id } })
  end)
end

--- Commits the workspace's annotation files to git (`annox/commit`), after
--- showing how many there are and letting you edit the message.
function M.commit()
  local bufnr = vim.api.nvim_get_current_buf()
  local doc = { uri = vim.uri_from_bufnr(bufnr) }
  request(bufnr, "annox/commit", { textDocument = doc, dryRun = true }, function(planned)
    local n = planned.files
    if n == 0 then
      return vim.notify("annox: no annotation changes to commit", vim.log.levels.INFO)
    end
    local prompt = string.format("Commit %d annotation file%s: ", n, n == 1 and "" or "s")
    vim.ui.input({ prompt = prompt, default = planned.message }, function(message)
      if not message or vim.trim(message) == "" then
        return
      end
      request(bufnr, "annox/commit", { textDocument = doc, message = message }, function(result)
        if result.commit == vim.NIL then
          return vim.notify("annox: no annotation changes to commit", vim.log.levels.INFO)
        end
        vim.notify(string.format("annox: committed %s %s", result.commit:sub(1, 7), message), vim.log.levels.INFO)
      end)
    end)
  end)
end

local word_ns = vim.api.nvim_create_namespace("annox_words")

--- Opens `edit_buf` in a float by the cursor, sized to its wrapped text, with
--- `old` (if any) shown read-only in red in a float stacked above it, the
--- changed words marked in both. Closing the edit window closes both. Returns
--- a function that re-sizes and re-marks them.
local function open_edit_windows(edit_buf, title, old)
  local anchor = vim.fn.screenpos(0, vim.fn.line("."), vim.fn.col("."))
  local old_buf
  if old ~= "" then
    old_buf = vim.api.nvim_create_buf(false, true)
    vim.bo[old_buf].bufhidden = "wipe"
    vim.api.nvim_buf_set_lines(old_buf, 0, -1, false, vim.split(old, "\n"))
    for i, line in ipairs(vim.api.nvim_buf_get_lines(old_buf, 0, -1, false)) do
      vim.api.nvim_buf_set_extmark(old_buf, ns, i - 1, 0, { end_col = #line, hl_group = "AnnoxEditOriginal" })
    end
    vim.bo[old_buf].modifiable = false
    vim.b[old_buf].annox_own_float = true
  end
  vim.b[edit_buf].annox_own_float = true

  local function width()
    local w = 20
    for _, buf in ipairs({ edit_buf, old_buf }) do
      for _, l in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
        w = math.max(w, vim.fn.strdisplaywidth(l) + 2)
      end
    end
    return math.min(w, math.floor(vim.o.columns * 0.8))
  end
  local base = { relative = "editor", row = 0, col = 0, width = width(), height = 1, border = "rounded" }
  local old_win = old_buf
    and vim.api.nvim_open_win(
      old_buf,
      false,
      vim.tbl_extend("force", base, { title = " Original ", focusable = false })
    )
  local edit_win = vim.api.nvim_open_win(edit_buf, true, vim.tbl_extend("force", base, { title = title }))
  for _, win in pairs({ old_win, edit_win }) do
    vim.wo[win].wrap = true
    vim.wo[win].linebreak = true
  end

  local function layout()
    if not vim.api.nvim_win_is_valid(edit_win) then
      return
    end
    if old_buf then
      local new = table.concat(vim.api.nvim_buf_get_lines(edit_buf, 0, -1, false), "\n")
      local del, add = word_changes(old, new)
      vim.api.nvim_buf_clear_namespace(old_buf, word_ns, 0, -1)
      vim.api.nvim_buf_clear_namespace(edit_buf, word_ns, 0, -1)
      mark_words(old_buf, word_ns, del, "AnnoxWordDeletion", 0, 0)
      mark_words(edit_buf, word_ns, add, "AnnoxWordInsertion", 0, 0)
    end
    local w = width()
    local avail = vim.o.lines - vim.o.cmdheight - 1
    local function fit(win, extra, cap)
      vim.api.nvim_win_set_width(win, w)
      return math.max(3, math.min(vim.api.nvim_win_text_height(win, {}).all + extra, cap))
    end
    -- One spare row in the edit window to type into.
    local edit_h = fit(edit_win, 1, math.max(3, math.floor(avail * 0.4)))
    local old_h = old_win and fit(old_win, 0, math.max(3, math.floor(avail * 0.25))) or 0
    local total = edit_h + 2 + (old_win and old_h + 2 or 0)
    -- Below the cursor line if it fits, else above it, else as low as fits.
    local row = anchor.row
    if row + total > avail then
      row = anchor.row - 1 - total >= 0 and anchor.row - 1 - total or math.max(0, avail - total)
    end
    local col = math.max(0, math.min(anchor.col - 1, vim.o.columns - w - 2))
    if old_win then
      vim.api.nvim_win_set_config(old_win, { relative = "editor", row = row, col = col, width = w, height = old_h })
      row = row + old_h + 2
    end
    vim.api.nvim_win_set_config(edit_win, { relative = "editor", row = row, col = col, width = w, height = edit_h })
  end
  layout()
  if old_win then
    vim.api.nvim_create_autocmd("WinClosed", {
      pattern = tostring(edit_win),
      once = true,
      -- Closing a window from WinClosed is not allowed.
      callback = vim.schedule_wrap(function()
        if vim.api.nvim_win_is_valid(old_win) then
          vim.api.nvim_win_close(old_win, true)
        end
      end),
    })
  end
  return layout
end

--- Edits the replacement of the suggestion under the cursor, or the text of
--- the comment, in a floating window. Esc saves and closes, `:q!` discards.
--- opts: { annotation? }
function M.edit(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local editable = function(a)
    return a.status == "open"
  end
  pick(
    opts,
    opts.annotation and buffer_annotations(bufnr, editable) or vim.tbl_filter(editable, under_cursor(bufnr)),
    "Edit",
    function(a)
      if a.kind == "suggestion" and not a.applicable then
        return vim.notify("annox: this suggestion is stale; use :Annox retarget", vim.log.levels.WARN)
      end
      local field = a.kind == "suggestion" and "replacement" or "body"
      local text = field == "replacement" and a.edit.replacement or a.body
      local lines = vim.split(type(text) == "string" and text or "", "\n")
      local edit_buf = vim.api.nvim_create_buf(false, true)
      vim.api.nvim_buf_set_lines(edit_buf, 0, -1, false, lines)
      vim.api.nvim_buf_set_name(edit_buf, string.format("annox://%s/%s", field, a.id))
      vim.bo[edit_buf].buftype = "acwrite"
      vim.bo[edit_buf].bufhidden = "wipe"
      vim.bo[edit_buf].filetype = field == "body" and "markdown" or vim.bo[bufnr].filetype
      vim.bo[edit_buf].modified = false
      local layout = open_edit_windows(
        edit_buf,
        field == "replacement" and " Suggested text (Esc saves and closes) " or " Comment (Esc saves and closes) ",
        field == "replacement" and resolved_text(a, bufnr) or ""
      )
      vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, { buffer = edit_buf, callback = layout })
      vim.keymap.set("n", "<Esc>", function()
        if vim.bo[edit_buf].modified then
          vim.cmd.write()
        end
        vim.api.nvim_win_close(0, true)
      end, { buffer = edit_buf, desc = "annox: save and close" })
      vim.api.nvim_create_autocmd("BufWriteCmd", {
        buffer = edit_buf,
        callback = function()
          local new = table.concat(vim.api.nvim_buf_get_lines(edit_buf, 0, -1, false), "\n")
          local method, params
          if field == "replacement" then
            method = "annox/retarget"
            params = { annotation = a.id, range = a.resolution.range, replacement = new }
            local s = M.suggesting[bufnr]
            if s then
              table.insert(s.undo, { id = a.id, range = a.resolution.range, replacement = a.edit.replacement })
            end
          else
            method, params = "annox/edit", { annotation = a.id, body = new ~= "" and new or vim.NIL }
          end
          request(bufnr, method, params, function(view)
            a = view
            if vim.api.nvim_buf_is_valid(edit_buf) then
              vim.bo[edit_buf].modified = false
            end
          end)
        end,
      })
    end
  )
end

--- Points a stale suggestion at the selection (§4.2.1).
--- opts: { annotation?, range?, visual?, replacement? }
function M.retarget(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  local range = target_range(bufnr, opts)
  local stale = buffer_annotations(bufnr, function(a)
    return a.kind == "suggestion" and a.status == "open" and not a.applicable
  end)
  pick(opts, stale, "Re-target", function(a)
    with_input(opts.replacement, "Replace with: ", a.edit and a.edit.replacement, function(replacement)
      request(bufnr, "annox/retarget", { annotation = a.id, range = range, replacement = replacement })
    end)
  end)
end

--- Re-attaches an orphaned comment to the selection (§3.7.4).
--- opts: { annotation?, range?, visual? }
function M.reattach(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  local range = target_range(bufnr, opts)
  local orphans = buffer_annotations(bufnr, function(a)
    return a.kind == "comment" and a.resolution and a.resolution.state == "orphaned"
  end)
  pick(opts, orphans, "Re-attach", function(a)
    request(bufnr, "annox/reattach", { annotation = a.id, range = range })
  end)
end

local function entry_label(field, e)
  local who = e.author and (e.author.name or e.author.id) or "unknown"
  local value = field == "target" and "(anchor)" or vim.inspect(e.value)
  return string.format("%s — %s", value, who)
end

--- Resolves the conflicts of an annotation (§2.5.4), one field at a time.
--- opts: { annotation?, field?, value?, revert? }
function M.resolve_conflict(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local conflicted = buffer_annotations(bufnr, function(a)
    return next(a.conflicts or {}) ~= nil
  end)
  local function send(id, field, value, revert)
    request(bufnr, "annox/resolveConflict", { annotation = id, field = field, value = value, revert = revert })
  end
  pick(opts, conflicted, "Resolve conflict", function(a)
    if opts.field then
      return send(a.id, opts.field, opts.value, opts.revert)
    end
    local field, entries = next(a.conflicts)
    local choices = vim.deepcopy(entries)
    if field == "body" or field == "label" then
      table.insert(choices, { custom = true })
    end
    vim.ui.select(choices, {
      prompt = "Conflicting " .. field,
      format_item = function(e)
        return e.custom and "Write a merged version…" or entry_label(field, e)
      end,
    }, function(choice)
      if not choice then
        return
      elseif choice.custom then
        return with_input(nil, "Merged " .. field .. ": ", a[field], function(text)
          send(a.id, field, text)
        end)
      end
      local value = choice.value
      if field == "deleted" then
        value = choice.value == "delete"
      end
      local has_accepted = vim.iter(entries):any(function(e)
        return e.value == "accepted"
      end)
      if field == "status" and has_accepted and value ~= "accepted" then
        local options = { "Revert the accepted edit", "Keep the document as it is" }
        return vim.ui.select(options, { prompt = "The document already contains this suggestion" }, function(c)
          if c then
            send(a.id, field, value, c == options[1])
          end
        end)
      end
      send(a.id, field, value)
    end)
  end)
end

local event_verbs = {
  edit = "edited",
  reanchor = "re-anchored (automatic)",
  retarget = "retargeted",
  delete = "deleted",
  restore = "restored",
}
local status_verbs = { open = "reopened", withdrawn = "withdrew" }
local create_verbs = { comment = "commented", suggestion = "suggested", reply = "replied" }

--- What a history event did, as a past-tense verb, with the first line of its
--- text if it has any. `replies` holds the thread's reply ids, so that events
--- changing a reply say so.
local function event_action(e, replies)
  local verb
  if e.type == "create" then
    verb = create_verbs[e.kind] or "created"
  elseif e.type == "status" then
    verb = status_verbs[e.status] or tostring(e.status)
  else
    verb = (event_verbs[e.type] or e.type) .. (replies[e.annotation] and " reply" or "")
  end
  local text = type(e.body) == "string" and e.body or (e.edit and e.edit.replacement)
  local detail = text and first_line(text) or ""
  return detail ~= "" and (verb .. ": " .. detail) or verb
end

--- Shows the history of the annotation under the cursor (§2.5.5).
function M.history(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  with_annotation(opts, nil, function(id)
    request(bufnr, "annox/history", { annotation = id }, function(events)
      local function who(e)
        return e.author and (e.author.name or e.author.id) or "unknown"
      end
      local width, replies = 0, {}
      for _, e in ipairs(events) do
        width = math.max(width, vim.fn.strdisplaywidth(who(e)))
        if e.type == "create" and e.kind == "reply" then
          replies[e.id] = true
        end
      end
      local lines = {}
      for _, e in ipairs(events) do
        local name = who(e) .. string.rep(" ", width - vim.fn.strdisplaywidth(who(e)))
        table.insert(lines, string.format("%s  %s  %s", e.time, name, event_action(e, replies)))
      end
      vim.lsp.util.open_floating_preview(lines, "text", { border = "rounded", focus_id = "annox-history" })
    end)
  end)
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
  local s, client = M.suggesting[bufnr], client_for(bufnr)
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
  local s = M.suggesting[bufnr]
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
  local s = M.suggesting[bufnr]
  if s then
    s.base = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
    s.queue = {}
  end
end

--- Undoes your last suggestion in suggestion mode: deletes it, or puts back
--- what it was before it was extended.
function M.undo_suggestion(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  local s = M.suggesting[bufnr]
  local entry = s and table.remove(s.undo)
  if not entry then
    return vim.notify("annox: no suggestion to undo", vim.log.levels.INFO)
  end
  table.insert(s.queue, { undo = entry })
  pump(bufnr)
end

--- Whether suggestion mode is on in `bufnr`, e.g. for a statusline.
function M.is_suggesting(bufnr)
  return M.suggesting[bufnr or vim.api.nvim_get_current_buf()] ~= nil
end

--- A statusline label for suggestion mode in `bufnr`, or "" when it's off.
function M.statusline(bufnr)
  return M.is_suggesting(bufnr) and "SUGGESTING" or ""
end

local tint_groups = { "LineNr", "CursorLineNr", "CursorLine" }

--- Adds or removes the suggestion mode tint in `win`'s 'winhighlight',
--- keeping any other entries.
local function sync_tint(win)
  local on = M.config.suggest_tint and M.suggesting[vim.api.nvim_win_get_buf(win)] ~= nil
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
  local on = M.suggesting[bufnr] ~= nil
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
    enable = M.suggesting[bufnr] == nil
  end
  local group = vim.api.nvim_create_augroup("annox_suggesting_" .. bufnr, { clear = true })
  local key = M.config.suggest_undo_key
  if not enable then
    if M.suggesting[bufnr] then
      capture(bufnr)
      M.suggesting[bufnr] = nil
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
  M.suggesting[bufnr] = {
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
    enable = not M.overlay_shown
  end
  M.overlay_shown = enable
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
  for bufnr in pairs(M.suggesting) do
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
    return client_for(bufnr) ~= nil and M.state[bufnr] ~= nil
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
  orphans = function()
    M.orphans()
  end,
  list = function()
    M.list()
  end,
  commit = function()
    M.commit()
  end,
}

function M.setup(opts)
  M.config = vim.tbl_deep_extend("force", M.config, opts or {})
  vim.g.annox_overlay = M.overlay_shown
  set_highlights()
  vim.api.nvim_create_autocmd("ColorScheme", { callback = set_highlights })
  vim.lsp.config("annox", {
    cmd = M.config.cmd,
    root_dir = function(bufnr, on_dir)
      local root = vim.fs.root(bufnr, { ".annox" })
      if root then
        on_dir(root)
      end
    end,
    capabilities = { experimental = { annox = { version = "0.1" } } },
    init_options = { annox = { diagnostics = false, author = M.config.author } },
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
