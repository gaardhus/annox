-- End-to-end test: real Neovim, real `annox lsp` server, temporary workspace.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/e2e.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")
local root = vim.fn.tempname()
vim.fn.mkdir(root .. "/.annox", "p")
vim.fn.writefile({ '{ "format": 1 }' }, root .. "/.annox/annox.json")
vim.fn.writefile({ "\\section{Results}", "In Section 3, we prove that the bound is tight." }, root .. "/paper.tex")

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

vim.cmd.edit(root .. "/paper.tex")
local buf = vim.api.nvim_get_current_buf()
wait("server to attach", function()
  local c = vim.lsp.get_clients({ bufnr = buf, name = "annox" })[1]
  return c and c.initialized
end)
wait("initial annotations", function()
  return annox.state[buf] ~= nil
end)

local function annotations()
  return annox.state[buf].annotations
end

local function range(line, s, e)
  return { start = { line = line, character = s }, ["end"] = { line = line, character = e } }
end

annox.comment({ range = range(1, 0, 12), body = "Which section?" })
wait("comment", function()
  return #annotations() == 1
end)
local comment = annotations()[1]

annox.suggest({ range = range(1, 14, 27), replacement = "we show that" })
wait("suggestion", function()
  return #annotations() == 2
end)
local suggestion
for _, a in ipairs(annotations()) do
  if a.kind == "suggestion" then
    suggestion = a
  end
end
check(suggestion.applicable, "suggestion should be applicable")

local marks = vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })
local highlighted = vim.tbl_filter(function(m)
  return m[4].hl_group ~= nil
end, marks)
check(#highlighted == 2, "expected 2 highlighted ranges, got " .. #highlighted)

-- Thread lookup under the cursor.
vim.api.nvim_win_set_cursor(0, { 2, 3 })
annox.reply({ body = "Section 3." })
wait("reply", function()
  for _, a in ipairs(annotations()) do
    if a.id == comment.id then
      return #a.replies == 1
    end
  end
end)

-- Accept: the server's workspace/applyEdit edits the buffer.
annox.accept({ annotation = suggestion.id })
wait("accepted edit in buffer", function()
  return vim.api.nvim_buf_get_lines(buf, 1, 2, false)[1] == "In Section 3, we show that the bound is tight."
end)
wait("suggestion to close", function()
  return #annotations() == 1
end)

annox.resolve({ annotation = comment.id })
wait("comment to resolve", function()
  return #annotations() == 0
end)

print("annox nvim e2e: OK")
vim.cmd.qall({ bang = true })
