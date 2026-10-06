-- `:Annox overlay` hides annotations without touching them, except in
-- suggestion mode.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/overlay.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")
local root = vim.fn.tempname()
vim.fn.mkdir(root .. "/.annox", "p")
vim.fn.writefile({ '{ "format": 1 }' }, root .. "/.annox/annox.json")
vim.fn.writefile({ "The quick brown fox." }, root .. "/notes.txt")

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

local function marks()
  return #vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, {})
end

local events = {}
vim.api.nvim_create_autocmd("User", {
  pattern = "AnnoxOverlay",
  callback = function(e)
    table.insert(events, e.data.enabled)
  end,
})

annox.comment({ range = { start = { line = 0, character = 4 }, ["end"] = { line = 0, character = 9 } }, body = "Fast?" })
wait("comment drawn", function()
  return marks() > 0
end)

vim.cmd("Annox overlay")
check(marks() == 0, "toggle hides the marks")
check(vim.g.annox_overlay == false and not annox.overlay_shown, "state off")
check(#annox.state[buf].annotations == 1, "annotation kept")

-- New annotations stay hidden while the overlay is off.
annox.comment({
  range = { start = { line = 0, character = 10 }, ["end"] = { line = 0, character = 15 } },
  body = "Colour?",
})
wait("second comment", function()
  return #annox.state[buf].annotations == 2
end)
check(marks() == 0, "new annotation hidden")

-- Commands still act on hidden annotations.
vim.api.nvim_win_set_cursor(0, { 1, 5 })
vim.cmd("Annox resolve")
wait("resolved while hidden", function()
  local open = annox.state[buf].annotations
  return #open == 1 and open[1].body == "Colour?"
end)

-- Suggestion mode shows its buffer's annotations regardless.
annox.suggest_mode({ enable = true })
check(marks() > 0, "suggestion mode draws")
annox.suggest_mode({ enable = false })
check(marks() == 0, "hidden again after suggestion mode")

vim.cmd("Annox overlay off")
check(marks() == 0, "off stays off")
vim.cmd("Annox overlay on")
check(marks() > 0, "on shows the marks")
check(vim.deep_equal(events, { false, false, true }), "events: " .. vim.inspect(events))

local completions = vim.fn.getcompletion("Annox overlay o", "cmdline")
check(vim.deep_equal(completions, { "on", "off" }), "completion: " .. vim.inspect(completions))
check(vim.tbl_contains(vim.fn.getcompletion("Annox ov", "cmdline"), "overlay"), "subcommand completion")

print("annox nvim overlay: OK")
vim.cmd.qall({ bang = true })
