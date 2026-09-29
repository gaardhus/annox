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

-- A change inside a word suggests the whole word.
vim.api.nvim_win_set_cursor(0, { 1, 5 })
keys("ra")
expect("whole-word suggestion", { { 4, 9, "qaick" }, { 10, 10, "very nice " } })
-- Changing that word again replaces the suggestion instead of stacking.
vim.api.nvim_win_set_cursor(0, { 1, 5 })
keys("re")
expect("replaced suggestion", { { 4, 9, "qeick" }, { 10, 10, "very nice " } })
keys("u")
keys("u")
expect("undo of the word", { { 10, 10, "very nice " } })

-- The suggested text can be edited in a floating window.
-- The cursor can be on either side of an insertion.
local main_win = vim.api.nvim_get_current_win()
vim.api.nvim_win_set_cursor(0, { 1, 9 })
annox.edit()
local float = vim.api.nvim_get_current_win()
check(vim.api.nvim_win_get_config(float).relative ~= "", "edit window is a float")
check(vim.api.nvim_buf_get_lines(0, 0, -1, false)[1] == "very nice ", "float holds the suggested text")
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "very, very nice " })
-- Esc saves and closes.
vim.api.nvim_feedkeys(vim.keycode("<Esc>"), "xt", false)
check(not vim.api.nvim_win_is_valid(float), "Esc closes the edit window")
check(vim.api.nvim_get_current_win() == main_win, "back in the document")
expect("edited suggestion", { { 10, 10, "very, very nice " } })
vim.api.nvim_win_set_cursor(0, { 1, 10 })
annox.edit()
check(vim.api.nvim_buf_get_lines(0, 0, -1, false)[1] == "very, very nice ", "opens from the other side too")
vim.api.nvim_feedkeys(vim.keycode("<Esc>"), "xt", false)
keys("u")
expect("undo of the edit", { { 10, 10, "very nice " } })

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

-- Bulk accept: the suggestions touching a visual selection, then all.
local function line_range(s, e)
  return { start = { line = 0, character = s }, ["end"] = { line = 0, character = e } }
end
annox.suggest({ range = line_range(3, 8), replacement = "slow" })
annox.suggest({ range = line_range(25, 28), replacement = "cat" })
wait("two suggestions", function()
  return #suggestions() == 2
end)
-- Answer the one prompt about suggestions that moved with "Accept all".
local prompts = {}
vim.ui.select = function(items, opts, on_choice)
  table.insert(prompts, opts.prompt)
  on_choice(items[1])
end
vim.fn.setpos("'<", { buf, 1, 1, 0 })
vim.fn.setpos("'>", { buf, 1, 10, 0 })
vim.cmd("'<,'>Annox accept")
wait("selection accepted", function()
  return text() == "he slow very nice brown fox." and #suggestions() == 1
end)
vim.cmd("Annox! accept")
wait("rest accepted", function()
  return text() == "he slow very nice brown cat." and #suggestions() == 0
end)
-- Accepting "slow" changed the text around "fox", which then needed the prompt.
check(#prompts == 1 and prompts[1]:find("^1 of these suggestion moved"), "one prompt: " .. vim.inspect(prompts))

-- Orphaned annotations are announced above the text, not silently dropped.
annox.comment({ range = { start = { line = 0, character = 4 }, ["end"] = { line = 0, character = 9 } }, body = "Fast?" })
wait("comment", function()
  return #annox.state[buf].annotations == 1
end)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "Something else entirely." })
wait("orphan notice", function()
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })) do
    local lines = m[4].virt_lines
    if lines and lines[1][1][1]:find("1 annotation could not be located", 1, true) then
      return true
    end
  end
end)
check(annox.orphan_count(buf) == 1, "orphan count")

print("annox nvim suggesting: OK")
vim.cmd.qall({ bang = true })
