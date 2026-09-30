-- `:Annox` commands wait for the server instead of failing when it isn't
-- ready yet: right after a lazy-loaded setup, and after a workspace is
-- created outside Neovim.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/ready.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")

local function check(cond, msg)
  if not cond then
    io.stderr:write("FAIL: " .. msg .. "\n")
    vim.cmd("cquit 1")
  end
end

local warnings = {}
vim.notify = function(msg, level)
  if level and level >= vim.log.levels.WARN then
    table.insert(warnings, msg)
  end
end

local function project(with_workspace)
  local root = vim.fn.tempname()
  vim.fn.mkdir(root, "p")
  if with_workspace then
    vim.fn.mkdir(root .. "/.annox", "p")
    vim.fn.writefile({ '{ "format": 1 }' }, root .. "/.annox/annox.json")
  end
  vim.fn.writefile({ "Hello world." }, root .. "/one.md")
  return root
end

-- The file is open before setup runs, as when lazy.nvim loads the plugin on
-- its first key, and the command runs before the server has started.
local lazy = project(true)
vim.cmd.edit(lazy .. "/one.md")
local buf = vim.api.nvim_get_current_buf()
local annox = require("annox")
annox.setup({ cmd = { bin, "lsp" } })
vim.cmd("Annox list")
vim.cmd.cclose()
check(#warnings == 0, "first command after setup: " .. table.concat(warnings, "; "))
check(annox.state[buf] ~= nil, "annotations loaded")

-- The workspace is created by `annox init` in a shell after the file was
-- opened, so FileType never saw it.
local later = project(false)
vim.cmd.edit(later .. "/one.md")
vim.cmd("Annox list")
vim.cmd.cclose()
check(warnings[1] and warnings[1]:find("not in an annox workspace"), "outside a workspace: " .. tostring(warnings[1]))
warnings = {}
vim.system({ bin, "init" }, { cwd = later }):wait()
vim.cmd("Annox list")
vim.cmd.cclose()
check(#warnings == 0, "after annox init in a shell: " .. table.concat(warnings, "; "))
check(#vim.lsp.get_clients({ name = "annox" }) == 2, "one server per workspace")

print("annox nvim ready: OK")
vim.cmd.qall({ bang = true })
