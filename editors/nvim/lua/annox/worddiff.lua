--- The words that changed within a suggestion, diff-so-fancy style, and the
--- diff blocks that show a suggestion as old and new text.

local store = require("annox.store")

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
  if not store.config.word_diff or old == "" or new == "" then
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

return {
  tokens = tokens,
  word_changes = word_changes,
  inserted_chunks = inserted_chunks,
  mark_words = mark_words,
  diff_block = diff_block,
}
