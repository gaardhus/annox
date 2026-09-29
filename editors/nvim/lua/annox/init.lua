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
}

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
  }
  for group, link in pairs(links) do
    vim.api.nvim_set_hl(0, group, { default = true, link = link })
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
  return text and text:match("^[^\n]*") or nil
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
      if sl == el and sc == ec then
        mark.virt_text = { { "◆", group } }
        mark.virt_text_pos = "inline"
      else
        mark.end_row, mark.end_col, mark.hl_group = el, ec, group
      end
      pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
      local label = a.kind == "suggestion" and ("→ " .. (a.edit and a.edit.replacement or ""))
        or first_line(a.body) or a.label
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

local function on_annotations(_, result)
  local bufnr = vim.uri_to_bufnr(result.textDocument.uri)
  if not vim.api.nvim_buf_is_loaded(bufnr) then
    return
  end
  M.state[bufnr] = { annotations = result.annotations, document = result.document }
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
  local pos = cursor_position(bufnr, client.offset_encoding)
  local hits = {}
  for _, a in ipairs(state.annotations) do
    local r = a.resolution and a.resolution.range
    if r and before(r.start, pos) and before(pos, r["end"]) then
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

local subcommands = {
  comment = function(o) M.comment({ visual = o.range > 0 }) end,
  draft = function(o) M.comment({ visual = o.range > 0, ["local"] = true }) end,
  publish = function(o) M.publish({ all = o.bang }) end,
  retarget = function(o) M.retarget({ visual = o.range > 0 }) end,
  reattach = function(o) M.reattach({ visual = o.range > 0 }) end,
  conflicts = function() M.resolve_conflict() end,
  history = function() M.history() end,
  suggest = function(o) M.suggest({ visual = o.range > 0 }) end,
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
    capabilities = { experimental = { annox = { version = "0.0" } } },
    init_options = { annox = { diagnostics = false, author = M.config.author } },
    handlers = { ["annox/didChangeAnnotations"] = on_annotations },
  })
  vim.lsp.enable("annox")
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
