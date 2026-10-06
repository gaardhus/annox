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

local ns = store.ns

local refresh_list

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

--- Lists orphaned annotations (§3.7.3), to fix one at a time or to resolve
--- every orphaned comment at once. Picking one offers what applies to it:
--- moving it to its suggested location (§3.7.4) or to the selection,
--- closing it, or showing its thread. opts: { visual? }
function M.orphans(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local client = client_for(bufnr)
  if not client then
    return request(bufnr)
  end
  local orphans = vim.tbl_filter(function(a)
    return a.status == "open" and a.resolution and a.resolution.state == "orphaned"
  end, (store.state[bufnr] or {}).annotations or {})
  if #orphans == 0 then
    return vim.notify("annox: no orphaned annotations", vim.log.levels.INFO)
  end
  local comments = vim.tbl_filter(function(a)
    return a.kind == "comment"
  end, orphans)
  local items = vim.list_extend({}, orphans)
  local resolve_all = {}
  if #comments > 1 then
    table.insert(items, resolve_all)
  end
  local function format(a)
    if a == resolve_all then
      return string.format("Resolve all %d orphaned comments", #comments)
    end
    local s = a.resolution.suggested
    return describe(a) .. (s and string.format(" (line %d?)", s.range.start.line + 1) or "")
  end
  local selection = opts.visual and visual_range(bufnr, client.offset_encoding)
  vim.ui.select(items, { prompt = "Orphaned annotations", format_item = format }, function(a)
    if a == resolve_all then
      return set_status_all(bufnr, comments, "resolved", "Resolve", "orphaned comment")
    elseif not a then
      return
    end
    local comment = a.kind == "comment"
    local function move(range)
      if comment then
        return request(bufnr, "annox/reattach", { annotation = a.id, range = range })
      end
      with_input(nil, "Replace with: ", a.edit and a.edit.replacement, function(replacement)
        request(bufnr, "annox/retarget", { annotation = a.id, range = range, replacement = replacement })
      end)
    end
    local verb = comment and "Re-attach" or "Re-target"
    local actions = {
      {
        "Show thread",
        function()
          open_thread(a, bufnr)
        end,
      },
    }
    local s = a.resolution.suggested
    if s then
      vim.api.nvim_win_set_cursor(0, { s.range.start.line + 1, byte_col(bufnr, s.range.start, client.offset_encoding) })
      local label =
        string.format("%s to line %d (%d%% of its words)", verb, s.range.start.line + 1, math.floor(s.score * 100))
      table.insert(actions, {
        label,
        function()
          move(s.range)
        end,
      })
    end
    if selection then
      table.insert(actions, {
        verb .. " to the selection",
        function()
          move(selection)
        end,
      })
    end
    local status = comment and "resolved" or "rejected"
    table.insert(actions, {
      comment and "Resolve thread" or "Reject suggestion",
      function()
        request(bufnr, "annox/setStatus", { annotation = a.id, status = status })
      end,
    })
    vim.ui.select(actions, {
      prompt = describe(a),
      format_item = function(x)
        return x[1]
      end,
    }, function(x)
      if x then
        x[2]()
      end
    end)
  end)
end

--- Quickfix list ids made by `M.list`, by buffer, so they can follow edits.
local lists = {}

--- The buffer's annotations as quickfix items, in document order.
local function list_items(bufnr)
  local client = client_for(bufnr)
  local items = {}
  for _, a in ipairs((store.state[bufnr] or {}).annotations or {}) do
    local r = a.resolution and a.resolution.range
    local s = a.resolution and a.resolution.suggested
    local at = r or (s and s.range)
    table.insert(items, {
      bufnr = bufnr,
      lnum = at and at.start.line + 1 or 1,
      col = at and client and byte_col(bufnr, at.start, client.offset_encoding) + 1 or 1,
      text = describe(a) .. (r and "" or s and " [orphaned, may belong here]" or " [orphaned]"),
      placed = at ~= nil,
      user_data = a.id,
    })
  end
  -- Document order, with orphans that have nowhere to point last.
  table.sort(items, function(x, y)
    if x.placed ~= y.placed then
      return x.placed
    end
    if x.lnum ~= y.lnum then
      return x.lnum < y.lnum
    end
    return x.col < y.col
  end)
  return items
end

--- Rebuilds the buffer's `M.list` quickfix list, if it is still around, and
--- keeps the current entry on the same annotation or the one that followed it.
function refresh_list(bufnr)
  local id = lists[bufnr]
  if not id then
    return
  end
  local old = vim.fn.getqflist({ id = id, items = 0, idx = 0 })
  if old.id == 0 then
    lists[bufnr] = nil
    return
  end
  local items = list_items(bufnr)
  local index = {}
  for i, item in ipairs(items) do
    index[item.user_data] = i
  end
  -- The first annotation at or after the current entry that survived, or the
  -- last one.
  local idx = #items
  for i = math.max(old.idx, 1), #old.items do
    if index[old.items[i].user_data] then
      idx = index[old.items[i].user_data]
      break
    end
  end
  vim.fn.setqflist({}, "r", { id = id, items = items, idx = math.max(idx, 1) })
end

--- Puts the buffer's annotations in the quickfix list, which then follows
--- them as they are accepted, resolved, or moved.
function M.list()
  local bufnr = vim.api.nvim_get_current_buf()
  vim.fn.setqflist({}, " ", { title = "annox", items = list_items(bufnr) })
  lists[bufnr] = vim.fn.getqflist({ id = 0 }).id
  vim.cmd.copen()
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
            local s = store.suggesting[bufnr]
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
