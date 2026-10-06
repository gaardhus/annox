--- The quickfix list of a buffer's annotations, which follows them as they change.

local store = require("annox.store")
local util = require("annox.util")

local client_for = util.client_for
local byte_col = util.byte_col
local describe = util.describe

local M = {}

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
local function refresh_list(bufnr)
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

M.list_items = list_items
M.refresh_list = refresh_list

return M
