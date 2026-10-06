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
    vim.cmd("cquit 1")
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

local events = {}
vim.api.nvim_create_autocmd("User", {
  pattern = "AnnoxSuggesting",
  callback = function(e)
    table.insert(events, e.data.enabled)
  end,
})
local function winhl()
  return vim.wo.winhighlight
end
vim.wo.winhighlight = "Normal:Normal"

annox.suggest_mode()
check(annox.is_suggesting(buf), "suggestion mode on")
check(vim.b[buf].annox_suggesting == true and annox.statusline() == "SUGGESTING", "statusline state on")
check(#events == 1 and events[1] == true, "event on: " .. vim.inspect(events))
check(
  winhl():find("CursorLineNr:AnnoxSuggestingCursorLineNr", 1, true) and winhl():find("^Normal:Normal,"),
  "tint on: " .. winhl()
)
-- The tint belongs to the buffer, not the window.
vim.cmd.enew()
check(winhl() == "Normal:Normal", "no tint on another buffer: " .. winhl())
check(annox.statusline() == "", "no label on another buffer")
vim.cmd.buffer(buf)
check(winhl():find("AnnoxSuggesting", 1, true), "tint back: " .. winhl())

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
    table.insert(
      inserted,
      table.concat(vim.tbl_map(function(c)
        return c[1]
      end, m[4].virt_text))
    )
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
check(vim.b[buf].annox_suggesting == false and annox.statusline() == "", "statusline state off")
check(#events == 2 and events[2] == false, "event off: " .. vim.inspect(events))
check(winhl() == "Normal:Normal", "tint off: " .. winhl())
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

-- Bulk reject: the text stays, and the whole buffer asks once.
annox.suggest({ range = line_range(3, 7), replacement = "fast" })
annox.suggest({ range = line_range(13, 17), replacement = "good" })
annox.suggest({ range = line_range(24, 27), replacement = "dog" })
wait("three suggestions", function()
  return #suggestions() == 3
end)
-- Editing a replacement shows the old text read-only above it.
local doc_win = vim.api.nvim_get_current_win()
vim.api.nvim_win_set_cursor(0, { 1, 4 })
annox.edit()
local edit_win = vim.api.nvim_get_current_win()
local old_win
for _, w in ipairs(vim.api.nvim_list_wins()) do
  local c = vim.api.nvim_win_get_config(w)
  if c.relative ~= "" and not c.focusable then
    old_win = w
  end
end
check(old_win ~= nil, "original window opened")
check(
  vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(old_win), 0, -1, false)[1] == "slow",
  "original holds old text"
)
check(vim.api.nvim_buf_get_lines(0, 0, -1, false)[1] == "fast", "edit holds the replacement")
local word_ns = vim.api.nvim_get_namespaces().annox_words
local function words(b)
  return vim.tbl_map(function(m)
    return { m[2], m[3], m[4].end_col }
  end, vim.api.nvim_buf_get_extmarks(b, word_ns, 0, -1, { details = true }))
end
check(#words(vim.api.nvim_win_get_buf(old_win)) == 0, "no word in common, nothing marked")
-- The marks follow the edit.
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "slow and steady" })
vim.api.nvim_exec_autocmds("TextChanged", { buffer = 0 })
check(vim.deep_equal(words(0), { { 0, 4, 15 } }), "new words marked: " .. vim.inspect(words(0)))
check(#words(vim.api.nvim_win_get_buf(old_win)) == 0, "old word kept")
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "fast" })
vim.bo.modified = false
check(
  vim.api.nvim_win_get_position(old_win)[1] < vim.api.nvim_win_get_position(edit_win)[1],
  "original sits above the edit window"
)
vim.api.nvim_feedkeys(vim.keycode("<Esc>"), "xt", false)
wait("original window closed", function()
  return not vim.api.nvim_win_is_valid(old_win)
end)
check(vim.api.nvim_get_current_win() == doc_win, "back in the document")
prompts = {}
vim.fn.setpos("'<", { buf, 1, 1, 0 })
vim.fn.setpos("'>", { buf, 1, 6, 0 })
vim.cmd("'<,'>Annox reject")
wait("selection rejected", function()
  return #suggestions() == 2
end)
check(#prompts == 0, "no prompt for one suggestion")
vim.cmd("Annox! reject")
wait("rest rejected", function()
  return #suggestions() == 0
end)
check(#prompts == 1 and prompts[1] == "Reject 2 suggestions?", "one prompt: " .. vim.inspect(prompts))
check(text() == "he slow very nice brown cat.", "rejecting leaves the text")

-- Bulk resolve works the same way for comment threads.
annox.comment({ range = line_range(3, 7), body = "Slow?" })
annox.comment({ range = line_range(24, 27), body = "Which cat?" })
wait("two comments", function()
  return #annox.state[buf].annotations == 2
end)
prompts = {}
vim.cmd("Annox! resolve")
wait("comments resolved", function()
  return #annox.state[buf].annotations == 0
end)
check(#prompts == 1 and prompts[1] == "Resolve 2 comments?", "one prompt: " .. vim.inspect(prompts))

-- Inline, the changed words get a stronger tint on both sides.
annox.config.inline_suggestions = true
annox.suggest({ range = line_range(3, 12), replacement = "slow and steady" })
wait("partial suggestion", function()
  return #suggestions() == 1
end)
local tinted, chunks = {}, nil
wait("inline word marks", function()
  tinted, chunks = {}, nil
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })) do
    if m[4].hl_group == "AnnoxWordDeletion" then
      table.insert(tinted, { m[3], m[4].end_col })
    end
    chunks = m[4].virt_text and m[4].virt_text[1][2] == "AnnoxInsertion" and m[4].virt_text or chunks
  end
  return chunks ~= nil
end)
-- "slow very" -> "slow and steady": "very" is struck and tinted, "and steady" tinted.
check(vim.deep_equal(tinted, { { 8, 12 } }), "deleted word tinted: " .. vim.inspect(tinted))
check(
  vim.deep_equal(chunks, {
    { "slow ", "AnnoxInsertion" },
    { "and steady", { "AnnoxInsertion", "AnnoxWordInsertion" } },
  }),
  "inserted words tinted: " .. vim.inspect(chunks)
)
vim.cmd("Annox! reject")
wait("partial suggestion rejected", function()
  return #suggestions() == 0
end)

-- Across lines: a change past the first line is marked from column 0.
local before_lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "one two three", "four five six" })
annox.suggest({
  range = { start = { line = 0, character = 4 }, ["end"] = { line = 1, character = 9 } },
  replacement = "two 3\nfor five",
})
wait("multi-line suggestion", function()
  return #suggestions() == 1
end)
wait("multi-line word marks", function()
  tinted, chunks = {}, nil
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })) do
    if m[4].hl_group == "AnnoxWordDeletion" then
      table.insert(tinted, { m[2], m[3], m[4].end_col })
    end
    chunks = m[4].virt_text and m[4].virt_text[1][2] == "AnnoxInsertion" and m[4].virt_text or chunks
  end
  return chunks ~= nil
end)
-- "two three\nfour five" -> "two 3\nfor five": "three" after the range's
-- start column, "four" at the start of the next line.
check(vim.deep_equal(tinted, { { 0, 8, 13 }, { 1, 0, 4 } }), "deleted words tinted: " .. vim.inspect(tinted))
check(
  vim.deep_equal(chunks, {
    { "two ", "AnnoxInsertion" },
    { "3", { "AnnoxInsertion", "AnnoxWordInsertion" } },
    { "↵", "AnnoxInsertion" },
    { "for", { "AnnoxInsertion", "AnnoxWordInsertion" } },
    { " five", "AnnoxInsertion" },
  }),
  "inserted words tinted: " .. vim.inspect(chunks)
)
vim.cmd("Annox! reject")
wait("multi-line suggestion rejected", function()
  return #suggestions() == 0
end)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, before_lines)
annox.config.inline_suggestions = false

-- Orphaned annotations are announced above the text, not silently dropped.
annox.comment({ range = { start = { line = 0, character = 4 }, ["end"] = { line = 0, character = 9 } }, body = "Fast?" })
wait("comment", function()
  return #annox.state[buf].annotations == 1
end)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "Something else entirely." })
local function orphan_diagnostics()
  return vim.diagnostic.get(buf, { namespace = vim.api.nvim_get_namespaces().annox_orphans })
end
wait("orphan warning", function()
  for _, d in ipairs(orphan_diagnostics()) do
    if d.lnum == 0 and d.severity == vim.diagnostic.severity.WARN and d.message:find("^1 annotation could not") then
      return true
    end
  end
end)
check(annox.orphan_count(buf) == 1, "orphan count")

-- Reworded text gets a suggested location, marked in the buffer (§3.7.4).
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "Intro.", "", "The fast brown fox jumps.", "", "Outro." })
annox.comment({ range = { start = { line = 2, character = 0 }, ["end"] = { line = 2, character = 25 } }, body = "Why?" })
wait("second comment", function()
  return #annox.state[buf].annotations == 2
end)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "Opening.", "", "A fast brown fox leaps and jumps.", "", "End." })
local function suggested()
  for _, a in ipairs(annox.state[buf].annotations) do
    if a.body == "Why?" and a.resolution.suggested then
      return a.resolution.suggested
    end
  end
end
wait("suggested location", suggested)
check(suggested().range.start.line == 2, "suggested on line 3: " .. vim.inspect(suggested()))
wait("suggested hint", function()
  for _, d in ipairs(orphan_diagnostics()) do
    if
      d.lnum == 2
      and d.severity == vim.diagnostic.severity.HINT
      and d.message:find("may belong here: Why?", 1, true)
    then
      return true
    end
  end
end)
wait("suggested mark", function()
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })) do
    if m[4].hl_group == "AnnoxSuggested" and m[2] == 2 then
      return true
    end
  end
end)

-- Picking an orphan offers what applies to it.
local actions
local select0 = vim.ui.select
vim.ui.select = function(items, o, done)
  if o.prompt == "Orphaned annotations" then
    for _, a in ipairs(items) do
      if a.body == "Why?" then
        return done(a)
      end
    end
  end
  actions = vim.tbl_map(o.format_item, items)
  done(nil)
end
annox.orphans()
vim.ui.select = select0
check(actions and actions[1] == "Show thread", vim.inspect(actions))
check(actions[2]:find("^Re%-attach to line 3 %(%d+%% of its words%)$") ~= nil, vim.inspect(actions))
check(actions[3] == "Resolve thread" and #actions == 3, vim.inspect(actions))

-- The orphans picker offers the suggested location, then resolves all.
local prompts = {}
local select = vim.ui.select
vim.ui.select = function(items, o, done)
  local labels = vim.tbl_map(o.format_item or tostring, items)
  table.insert(prompts, labels)
  if #prompts == 1 then
    return done(items[#items]) -- "Resolve all"
  end
  done(items[1])
end
annox.orphans()
vim.ui.select = select
check(prompts[1][#prompts[1]] == "Resolve all 2 orphaned comments", "resolve all offered: " .. vim.inspect(prompts))
check(prompts[2][1] == "Resolve all 2", "confirmation: " .. vim.inspect(prompts))
wait("orphans resolved", function()
  return annox.orphan_count(buf) == 0
end)

print("annox nvim suggesting: OK")
vim.cmd.qall({ bang = true })
