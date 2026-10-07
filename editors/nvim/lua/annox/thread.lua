--- Threads and their history, in floating windows, with the change of a
--- suggestion shown as a diff block, also in floats other code opens.

local store = require("annox.store")
local util = require("annox.util")
local worddiff = require("annox.worddiff")

local client_for = util.client_for
local first_line = util.first_line
local request = util.request
local under_cursor = util.under_cursor
local with_annotation = util.with_annotation
local find_annotation = util.find_annotation
local resolved_text = util.resolved_text
local word_changes = worddiff.word_changes
local mark_words = worddiff.mark_words
local diff_block = worddiff.diff_block

local M = {}

-- Diff blocks styled in floats, some opened by other code: kept apart from
-- `ns`, so clearing them never touches the marks of an annotated buffer.
local diff_ns = vim.api.nvim_create_namespace("annox_diff")

--- `md` with each lone `~` escaped, outside code: GFM reads a pair of them as
--- strikethrough, but authors mostly mean "approximately". `~~` is kept.
local function escape_tildes(md)
  local out, fenced = {}, false
  for _, line in ipairs(vim.split(md, "\n")) do
    if line:match("^%s*```") or line:match("^%s*~~~") then
      fenced = not fenced
    elseif not fenced then
      local parts, start, code, j = {}, 1, 0, 1
      while j <= #line do
        local c = line:sub(j, j)
        if c == "`" then
          local n = #line:match("^`+", j)
          if code == 0 then
            code = n
          elseif code == n then
            code = 0
          end
          j = j + n
        else
          local prev, next_ = line:sub(j - 1, j - 1), line:sub(j + 1, j + 1)
          if c == "~" and code == 0 and prev ~= "~" and prev ~= "\\" and next_ ~= "~" then
            table.insert(parts, line:sub(start, j - 1) .. "\\")
            start = j
          end
          j = j + 1
        end
      end
      table.insert(parts, line:sub(start))
      line = table.concat(parts)
    end
    table.insert(out, line)
  end
  return out
end

--- Hides the backslash of each Markdown escape in float `fbuf`, as Markdown
--- renderers do: Neovim's own queries only color it.
local function conceal_escapes(fbuf)
  local ok, parser = pcall(vim.treesitter.get_parser, fbuf, "markdown")
  if not ok or not parser then
    return
  end
  parser:parse(true)
  local query = vim.treesitter.query.parse("markdown_inline", "(backslash_escape) @escape")
  parser:for_each_tree(function(tree, ltree)
    if ltree:lang() == "markdown_inline" then
      for _, node in query:iter_captures(tree:root(), fbuf) do
        local row, col = node:start()
        vim.api.nvim_buf_set_extmark(fbuf, diff_ns, row, col, { end_col = col + 1, conceal = "" })
      end
    end
  end)
end

--- The thread as markdown lines. A suggestion's old text is read from `bufnr`.
local function thread_lines(a, bufnr)
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
  vim.list_extend(lines, escape_tildes(str(a.body)))
  for _, r in ipairs(a.replies or {}) do
    -- The markdown float expands a thematic break into a full-width rule.
    table.insert(lines, "---")
    table.insert(lines, person(r))
    vim.list_extend(lines, escape_tildes(str(r.body)))
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
local function open_thread(a, bufnr, opts)
  local fbuf, win = vim.lsp.util.open_floating_preview(
    thread_lines(a, bufnr),
    "markdown",
    vim.tbl_extend("force", { border = "rounded" }, opts or {})
  )
  -- Styled here, for `a`, which needn't be under the cursor.
  vim.b[fbuf].annox_own_float = true
  conceal_escapes(fbuf)
  if a.kind == "suggestion" and bufnr then
    style_diff_blocks(fbuf, { suggestion_change(a, bufnr) })
  end
  return fbuf, win
end

--- Styles the diff blocks of the suggestions under the cursor in float
--- `win`, which some other code opened, such as the LSP hover.
local function style_hover(win, fbuf)
  local source = vim.api.nvim_get_current_buf()
  if
    not vim.api.nvim_win_is_valid(win)
    or vim.api.nvim_win_get_buf(win) ~= fbuf
    or vim.b[fbuf].annox_own_float
    or store.state[fbuf]
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
  conceal_escapes(fbuf)
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

M.open_thread = open_thread
M.style_hover = style_hover

return M
