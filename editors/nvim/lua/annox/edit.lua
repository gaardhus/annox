--- Editing annotations in floating windows, re-targeting and re-attaching
--- them, and resolving their conflicts.

local store = require("annox.store")
local util = require("annox.util")
local worddiff = require("annox.worddiff")

local client_for = util.client_for
local request = util.request
local under_cursor = util.under_cursor
local buffer_annotations = util.buffer_annotations
local with_input = util.with_input
local target_range = util.target_range
local resolved_text = util.resolved_text
local pick = util.pick
local word_changes = worddiff.word_changes
local mark_words = worddiff.mark_words

local ns = store.ns

local M = {}

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

M.open_edit_windows = open_edit_windows
M.entry_label = entry_label

return M
