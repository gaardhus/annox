-- `:Annox init` creates a workspace and attaches the server.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/init.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")
local root = vim.fn.tempname()
vim.fn.mkdir(root .. "/.git", "p")
vim.fn.mkdir(root .. "/chapters", "p")
vim.fn.writefile({ "Hello world." }, root .. "/chapters/one.md")

local annox = require("annox")
annox.setup({ cmd = { bin, "lsp" } })

local function check(cond, msg)
  if not cond then
    io.stderr:write("FAIL: " .. msg .. "\n")
    vim.cmd.cquit(1)
  end
end

vim.cmd.edit(root .. "/chapters/one.md")
local buf = vim.api.nvim_get_current_buf()
vim.wait(500)
check(#vim.lsp.get_clients({ bufnr = buf, name = "annox" }) == 0, "no server outside a workspace")

annox.init({ confirm = false })
check(vim.uv.fs_stat(root .. "/.annox/annox.json") ~= nil, "workspace created at the git root")
check(
  vim.wait(5000, function()
    return annox.state[buf] ~= nil
  end, 20),
  "server attached after init"
)

print("annox nvim init: OK")
vim.cmd.qall({ bang = true })
