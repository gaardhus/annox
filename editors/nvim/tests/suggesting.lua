-- Suggestion mode: edits become suggestions and the buffer keeps its text.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/suggesting.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")
local root = vim.fn.tempname()
local original = "The quick brown fox."
vim.fn.mkdir(root .. "/.annox", "p")
vim.fn.writefile({ '{ "format": 1 }' }, root .. "/.annox/annox.json")
vim.fn.writefile({ original }, root .. "/notes.txt")

local annox = require("annox")
annox.setup({ cmd = { bin, "lsp" }, author = { id = "mailto:ada@example.org", name = "Ada" } })

local function check(cond, msg)
  if not cond then
    io.stderr:write("FAIL: " .. msg .. "\n")
    vim.cmd.cquit(1)
  end
end

local function wait(msg, cond)
  check(vim.wait(5000, cond, 20), "timed out waiting for " .. msg)
end

vim.cmd.edit(root .. "/notes.txt")
local buf = vim.api.nvim_get_current_buf()
wait("initial annotations", function()
  return annox.state[buf] ~= nil
end)

local function suggestions()
  return vim.tbl_filter(function(a)
    return a.kind == "suggestion"
  end, annox.state[buf].annotations)
end

--- Waits until the suggestions are exactly `expected` ({ start, end, replacement }, by start).
local function expect(msg, expected)
  wait(msg, function()
    local got = suggestions()
    if #got ~= #expected or annox.suggesting[buf].busy or #annox.suggesting[buf].queue > 0 then
      return false
    end
    table.sort(got, function(a, b)
      return a.resolution.range.start.character < b.resolution.range.start.character
    end)
    for i, e in ipairs(expected) do
      local r = got[i].resolution.range
      if r.start.character ~= e[1] or r["end"].character ~= e[2] or got[i].edit.replacement ~= e[3] then
        return false
      end
    end
    return true
  end)
end

local function text()
  return table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\n")
end

local function keys(k)
  vim.api.nvim_feedkeys(vim.keycode(k), "xt", false)
  -- Headless Neovim doesn't reach the point where TextChanged fires; trigger it.
  vim.api.nvim_exec_autocmds("TextChanged", { buffer = buf })
end

annox.suggest_mode()
check(annox.is_suggesting(buf), "suggestion mode on")

-- Typing inserts nothing into the buffer: it becomes a suggestion.
vim.api.nvim_win_set_cursor(0, { 1, 10 })
keys("ivery <Esc>")
check(text() == original, "buffer keeps its text, got: " .. text())
expect("insertion suggestion", { { 10, 10, "very " } })
check(vim.api.nvim_win_get_cursor(0)[2] == 10, "cursor at the insertion point")

-- Typing again at the same place extends that suggestion.
keys("inice <Esc>")
expect("extended suggestion", { { 10, 10, "very nice " } })

-- A deletion elsewhere is a separate suggestion.
vim.api.nvim_win_set_cursor(0, { 1, 16 })
keys("dw")
check(text() == original, "deletion undone in the buffer, got: " .. text())
expect("deletion suggestion", { { 10, 10, "very nice " }, { 16, 19, "" } })

-- A deletion touching the insertion joins it into one replacement.
vim.api.nvim_win_set_cursor(0, { 1, 4 })
keys("dw")
expect("joined replacement", { { 4, 10, "very nice " }, { 16, 19, "" } })

-- Drawn inline: struck-through deletion followed by the inserted text.
local marks = vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })
local struck, inserted = 0, {}
for _, m in ipairs(marks) do
  if m[4].hl_group == "AnnoxDeletion" then
    struck = struck + 1
  end
  if m[4].virt_text and m[4].virt_text[1][2] == "AnnoxInsertion" then
    table.insert(inserted, m[4].virt_text[1][1])
  end
end
check(struck == 2, "two struck-through ranges, got " .. struck)
check(#inserted == 1 and inserted[1] == "very nice ", "inserted text drawn inline")

-- Undo goes back one suggestion step at a time.
keys("u")
expect("undo of the join", { { 10, 10, "very nice " }, { 16, 19, "" } })
keys("u")
expect("undo of the deletion", { { 10, 10, "very nice " } })

-- Accepting applies the edit to the buffer without making a new suggestion.
annox.accept({ annotation = suggestions()[1].id })
wait("accepted edit", function()
  return text() == "The quick very nice brown fox."
end)
vim.api.nvim_exec_autocmds("TextChanged", { buffer = buf })
expect("no suggestions after accepting", {})

annox.suggest_mode()
check(not annox.is_suggesting(buf), "suggestion mode off")
check(vim.fn.maparg("u", "n", false, true).buffer ~= 1, "undo key restored")
vim.api.nvim_win_set_cursor(0, { 1, 0 })
keys("x")
check(text() == "he quick very nice brown fox.", "plain editing again")

print("annox nvim suggesting: OK")
vim.cmd.qall({ bang = true })
