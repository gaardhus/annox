-- Reactions and approvals (§2.5.7, §4.4.2): `:Annox approve` and
-- `:Annox react`, shown in the thread, the virtual text and the history.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/reactions.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")

local function check(cond, msg)
  if not cond then
    io.stderr:write("FAIL: " .. msg .. "\n")
    vim.cmd("cquit 1")
  end
end

local function wait(msg, cond)
  check(vim.wait(5000, cond, 20), "timed out waiting for " .. msg)
end

vim.notify = function(msg, level)
  check(not level or level < vim.log.levels.WARN, "unexpected warning: " .. msg)
end

local root = vim.fn.tempname()
vim.fn.mkdir(root, "p")
vim.fn.writefile({ "We prove that teh bound is tight." }, root .. "/paper.md")
vim.system({ bin, "init" }, { cwd = root }):wait()

local annox = require("annox")
annox.setup({ cmd = { bin, "lsp" }, author = { id = "mailto:ada@example.org", name = "Ada" } })
vim.cmd.edit(root .. "/paper.md")
local buf = vim.api.nvim_get_current_buf()
wait("annotations loaded", function()
  return annox.state[buf] ~= nil
end)

-- Bob suggests and comments from a shell, as if pulled from git.
local function bob(args)
  vim.list_extend(args, { "--author", "mailto:bob@example.org", "--name", "Bob" })
  local r = vim.system(vim.list_extend({ bin, "--json" }, args), { cwd = root }):wait()
  check(r.code == 0, "bob: " .. (r.stderr or ""))
  return vim.json.decode(r.stdout)
end
local s = bob({ "suggest", "paper.md", "--quote", "teh", "--replace", "the" })
local reply = bob({ "reply", s.id, "--body", "Typo." })
local function suggestion()
  for _, a in ipairs(annox.state[buf].annotations) do
    if a.id == s.id then
      return a
    end
  end
end
wait("Bob's suggestion", function()
  local a = suggestion()
  return a and #a.replies == 1
end)

vim.api.nvim_win_set_cursor(0, { 1, 15 })
vim.cmd("Annox approve")
wait("approval", function()
  local a = suggestion()
  return #a.approvals == 1 and a.approvals[1].author.id == "mailto:ada@example.org"
end)
annox.react({ annotation = reply.id, reaction = "🎉" })
wait("reaction on the reply", function()
  local r = suggestion().replies[1]
  return type(r.reactions["🎉"]) == "table"
end)

local virt = {}
for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, require("annox.store").ns, 0, -1, { details = true })) do
  for _, chunk in ipairs(m[4].virt_text or {}) do
    table.insert(virt, chunk[1])
  end
end
check(table.concat(virt):find("👍1", 1, true), "approval count in virtual text: " .. table.concat(virt, "|"))

local function float_text()
  for _, w in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(w).relative ~= "" then
      return table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\n")
    end
  end
end

annox.thread({ annotation = s.id })
local shown = float_text() or ""
check(shown:find("👍 _approved by Ada_", 1, true), "approval in thread: " .. shown)
check(shown:find("🎉 Ada", 1, true), "reply reaction in thread: " .. shown)
vim.cmd("fclose!")

-- Re-targeting makes the approval stale.
bob({ "retarget", s.id, "--quote", "teh bound", "--replace", "the bound" })
wait("stale approval", function()
  local a = suggestion()
  return a.approvals[1] and a.approvals[1].stale == true
end)
annox.thread({ annotation = s.id })
shown = float_text() or ""
check(shown:find("_Ada approved an earlier version_", 1, true), "stale approval in thread: " .. shown)
vim.cmd("fclose!")

vim.cmd("Annox! approve")
wait("approval withdrawn", function()
  return #suggestion().approvals == 0
end)

annox.history({ annotation = s.id })
wait("history float", function()
  local text = float_text() or ""
  return text:find("reacted 👍", 1, true)
    and text:find("removed 👍", 1, true)
    and text:find("reacted 🎉 on reply", 1, true)
end)

print("annox nvim reactions: OK")
vim.cmd.qall({ bang = true })
