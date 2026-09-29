--- annox for Neovim: a thin client for the annox language server (spec §6).
---
--- The server owns all annox logic. This plugin renders the annotations it
--- pushes (`annox/didChangeAnnotations`) and sends user actions as `annox/*`
--- requests. Suggested edits come back as `workspace/applyEdit`, which
--- Neovim's LSP client applies to the buffer.

local M = {}

local ns = vim.api.nvim_create_namespace("annox")

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
}

local presence_ns = vim.api.nvim_create_namespace("annox_presence")

--- Others' presence, as last pushed by the server.
M.peers = {}

--- Latest state pushed by the server, per buffer: { annotations, document }.
M.state = {}

local function set_highlights()
  local links = {
    AnnoxComment = "DiagnosticUnderlineInfo",
    AnnoxSuggestion = "DiagnosticUnderlineHint",
    AnnoxStale = "DiagnosticUnderlineWarn",
    AnnoxConflict = "DiagnosticUnderlineError",
    AnnoxLocal = "DiagnosticUnderlineOk",
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
  vim.api.nvim_set_hl(0, "AnnoxDeletion", { default = true, strikethrough = true, fg = removed.fg })
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
  if next(a.conflicts or {}) then
    return "AnnoxConflict"
  elseif a["local"] then
    return "AnnoxLocal"
  elseif a.kind == "suggestion" then
    return a.applicable and "AnnoxSuggestion" or "AnnoxStale"
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

--- Draws the annotations of `bufnr` as extmarks.
function M.render(bufnr)
  vim.api.nvim_buf_clear_namespace(bufnr, ns, 0, -1)
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
    local text = string.format(
      "⚠ %d annotation%s could not be located (:Annox orphans)",
      orphans,
      orphans == 1 and "" or "s"
    )
    vim.api.nvim_buf_set_extmark(bufnr, ns, 0, 0, { virt_lines = { { { text, "AnnoxOrphans" } } }, virt_lines_above = true })
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
        or first_line(a.body) or a.label
      if inline(bufnr, a) then
        -- Deleted text struck through, followed by the inserted text.
        if sl ~= el or sc ~= ec then
          mark.end_row, mark.end_col, mark.hl_group = el, ec, "AnnoxDeletion"
        end
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
        local replacement = a.edit and a.edit.replacement or ""
        if replacement ~= "" then
          pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, el, ec, {
            virt_text = { { (replacement:gsub("\n", "↵")), "AnnoxInsertion" } },
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
  if not client then
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
  presence_timer:start(150, 0, vim.schedule_wrap(function()
    local bufnr = vim.api.nvim_get_current_buf()
    local client = client_for(bufnr)
    if not client then
      return
    end
    local row, col = unpack(vim.api.nvim_win_get_cursor(0))
    local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
    local pos = { line = row - 1, character = vim.str_utfindex(line, client.offset_encoding, math.min(col, #line), false) }
    client:notify("annox/setPresence", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      selection = { start = pos, ["end"] = pos },
    })
  end))
end

local function on_annotations(_, result)
  local bufnr = vim.uri_to_bufnr(result.textDocument.uri)
  if not vim.api.nvim_buf_is_loaded(bufnr) then
    return
  end
  M.state[bufnr] = { annotations = result.annotations, document = result.document }
  local s = M.suggesting[bufnr]
  if s then
    -- Keep only suggestions that can still be extended.
    local fresh = {}
    for _, a in ipairs(result.annotations) do
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

--- Replies to the thread under the cursor. opts: { annotation?, body? }
function M.reply(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  with_annotation(opts, nil, function(id)
    with_input(opts.body, "Reply: ", nil, function(body)
      request(bufnr, "annox/reply", { parent = id, body = body })
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

M.resolve = status_action("resolved", is_kind("comment", "open"))
M.reopen = status_action("open", function(a)
  return a.status ~= "open"
end)
M.reject = status_action("rejected", is_kind("suggestion", "open"))

--- Accepts the suggestion under the cursor: the server sends the edit back
--- as `workspace/applyEdit` (§6.6.2). opts: { annotation? }
function M.accept(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  with_annotation(opts, is_kind("suggestion", "open"), function(id)
    request(bufnr, "annox/accept", { annotation = id })
  end)
end

local function thread_lines(a)
  local lines = {}
  local function person(x)
    local author = x.author or {}
    return string.format("**%s** · %s", author.name or author.id or "unknown", x.created or "")
  end
  if a.kind == "suggestion" then
    local stale = a.applicable and "" or " (stale)"
    table.insert(lines, string.format("**Suggestion%s:** → `%s`", stale, a.edit and a.edit.replacement or ""))
    local by = a.retargetedBy and a.retargetedBy.author
    if by and by.id ~= (a.author or {}).id then
      table.insert(lines, string.format("_re-targeted by %s_", by.name or by.id))
    end
    table.insert(lines, "")
  end
  if next(a.conflicts or {}) then
    table.insert(lines, "⚠ **Conflicting changes:** " .. table.concat(vim.tbl_keys(a.conflicts), ", "))
    table.insert(lines, "")
  end
  table.insert(lines, person(a) .. (a.status ~= "open" and ("  _" .. a.status .. "_") or ""))
  vim.list_extend(lines, vim.split(a.body or "", "\n"))
  for _, r in ipairs(a.replies or {}) do
    table.insert(lines, "")
    table.insert(lines, person(r))
    vim.list_extend(lines, vim.split(r.body or "", "\n"))
  end
  return lines
end

--- Shows the thread under the cursor in a floating window.
function M.thread(opts)
  with_annotation(opts or {}, nil, function(id)
    local bufnr = vim.api.nvim_get_current_buf()
    for _, a in ipairs(M.state[bufnr].annotations) do
      if a.id == id then
        vim.lsp.util.open_floating_preview(thread_lines(a), "markdown", { border = "rounded", focus_id = "annox" })
      end
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
      vim.lsp.util.open_floating_preview(thread_lines(a), "markdown", { border = "rounded" })
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

local function buffer_annotations(bufnr, keep)
  return vim.tbl_filter(keep, (M.state[bufnr] or {}).annotations or {})
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

--- Edits the replacement of the suggestion under the cursor, or the text of
--- the comment, in a floating window. Esc saves and closes, `:q!` discards.
--- opts: { annotation? }
function M.edit(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local editable = function(a)
    return a.status == "open"
  end
  pick(opts, opts.annotation and buffer_annotations(bufnr, editable) or vim.tbl_filter(editable, under_cursor(bufnr)), "Edit", function(a)
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
    local width = 20
    for _, l in ipairs(lines) do
      width = math.max(width, vim.fn.strdisplaywidth(l) + 2)
    end
    vim.api.nvim_open_win(edit_buf, true, {
      relative = "cursor",
      row = 1,
      col = 0,
      width = math.min(width, math.floor(vim.o.columns * 0.8)),
      height = math.min(math.max(#lines, 3), 20),
      border = "rounded",
      title = field == "replacement" and " Suggested text (Esc saves and closes) " or " Comment (Esc saves and closes) ",
    })
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
  end)
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

--- Shows the history of the annotation under the cursor (§2.5.5).
function M.history(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  with_annotation(opts, nil, function(id)
    request(bufnr, "annox/history", { annotation = id }, function(events)
      local lines = {}
      for _, e in ipairs(events) do
        local who = e.author and (e.author.name or e.author.id) or "unknown"
        local detail = e.body or e.status or (e.edit and e.edit.replacement) or ""
        table.insert(lines, string.format("%s  %-8s %s  %s", e.time, e.type, who, first_line(tostring(detail))))
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
--- stretch becomes a new suggestion, or extends one made in this session that
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

--- The changes from `base` to `lines`: { a0, a1, replacement, restore }, with
--- `[a0, a1)` a byte range of the base text, and `restore` the line range of
--- `lines` to put back and the base lines to put there.
local function changes(base, lines)
  local a, b = text_of(base), text_of(lines)
  local sa, sb = line_starts(base), line_starts(lines)
  local out = {}
  for _, h in ipairs(vim.text.diff(a, b, { result_type = "indices" })) do
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
    table.insert(out, {
      a0 = a0,
      a1 = a1,
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
      -- the end of an insertion continues it.
      local current = v.edit.replacement
      if a0 >= r1 then
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
      method, params = "annox/create", {
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
    local pos = offset_position(s.base, line_starts(s.base), last.a1, "utf-8")
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
  vim.notify("annox: suggestion mode on", vim.log.levels.INFO)
  M.render(bufnr)
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
  vim.fn.writefile({ "cache/", "local/" }, vim.fs.joinpath(annox, ".gitignore"))
  -- vim.lsp.enable attaches on FileType; replay it for buffers in the workspace.
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(b)
    if vim.api.nvim_buf_is_loaded(b) and vim.startswith(name, root .. "/") then
      vim.api.nvim_exec_autocmds("FileType", { buffer = b })
    end
  end
  vim.notify("annox: created a workspace in " .. root, vim.log.levels.INFO)
end

local subcommands = {
  init = function() M.init() end,
  comment = function(o) M.comment({ visual = o.range > 0 }) end,
  draft = function(o) M.comment({ visual = o.range > 0, ["local"] = true }) end,
  publish = function(o) M.publish({ all = o.bang }) end,
  retarget = function(o) M.retarget({ visual = o.range > 0 }) end,
  reattach = function(o) M.reattach({ visual = o.range > 0 }) end,
  conflicts = function() M.resolve_conflict() end,
  history = function() M.history() end,
  suggest = function(o) M.suggest({ visual = o.range > 0 }) end,
  suggesting = function() M.suggest_mode() end,
  edit = function() M.edit() end,
  reply = function() M.reply() end,
  resolve = function() M.resolve() end,
  reopen = function() M.reopen() end,
  accept = function() M.accept() end,
  reject = function() M.reject() end,
  thread = function() M.thread() end,
  orphans = function() M.orphans() end,
  list = function() M.list() end,
}

function M.setup(opts)
  M.config = vim.tbl_deep_extend("force", M.config, opts or {})
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
  vim.api.nvim_create_user_command("Annox", function(o)
    local fn = subcommands[o.fargs[1]]
    if not fn then
      return vim.notify("annox: unknown subcommand " .. tostring(o.fargs[1]), vim.log.levels.ERROR)
    end
    fn(o)
  end, {
    nargs = 1,
    range = true,
    bang = true,
    complete = function()
      return vim.tbl_keys(subcommands)
    end,
  })
end

return M
