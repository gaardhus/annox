--- Fixing orphaned annotations, whose text could not be found, one at a time
--- from a picker.

local store = require("annox.store")
local util = require("annox.util")
local open_thread = require("annox.thread").open_thread
local set_status_all = require("annox.actions").set_status_all

local client_for = util.client_for
local byte_col = util.byte_col
local request = util.request
local visual_range = util.visual_range
local describe = util.describe
local with_input = util.with_input

local M = {}

--- Lists orphaned annotations (§3.7.3), to fix one at a time or to resolve
--- every orphaned comment at once. Picking one offers what applies to it:
--- moving it to its suggested location (§3.7.4) or to the selection,
--- closing it, or showing its thread. With the cursor on the suggested
--- location of some, only those are offered, and a single one is picked
--- without asking. opts: { visual? }
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
  local here = util.orphans_under_cursor(bufnr)
  if #here > 0 then
    orphans = here
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
  local function on_pick(a)
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
  end
  if #here == 1 then
    return on_pick(here[1])
  end
  vim.ui.select(items, { prompt = "Orphaned annotations", format_item = format }, on_pick)
end

return M
