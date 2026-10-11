-- Unread threads (§5.13): others' comments are marked, `:Annox unread` lists
-- them across files, and opening a thread or `:Annox read all` marks them read.
-- Run: ANNOX_BIN=target/debug/annox nvim --headless --clean -l editors/nvim/tests/unread.lua

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))

local bin = assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN")

local function check(cond, msg)
  if not cond then
    io.stderr:write("FAIL: " .. msg .. "\n")
    vim.cmd("cquit 1")
  end
end

local infos = {}
vim.notify = function(msg, level)
  check(not level or level < vim.log.levels.WARN, "unexpected warning: " .. msg)
  table.insert(infos, msg)
end

local root = vim.fn.tempname()
vim.fn.mkdir(root, "p")
vim.fn.writefile({ "Hello world." }, root .. "/one.md")
vim.fn.writefile({ "Goodbye world." }, root .. "/two.md")
vim.system({ bin, "init" }, { cwd = root }):wait()

local annox = require("annox")
annox.setup({ cmd = { bin, "lsp" }, author = { id = "mailto:ada@example.org", name = "Ada" } })
vim.cmd.edit(root .. "/one.md")
local buf = vim.api.nvim_get_current_buf()
check(
  vim.wait(5000, function()
    return annox.state[buf] ~= nil
  end),
  "annotations loaded"
)

-- Bob comments from a shell, as if pulled from git.
local function bob(file, quote, body)
  local r = vim
    .system({ bin, "comment", file, "--quote", quote, "--body", body, "--author", "mailto:bob@example.org" }, {
      cwd = root,
    })
    :wait()
  check(r.code == 0, "bob comments: " .. (r.stderr or ""))
end
bob("one.md", "world", "Which world?")
check(
  vim.wait(5000, function()
    return annox.unread_count(buf) == 1
  end),
  "Bob's comment is unread"
)
local marked = vim.tbl_filter(function(m)
  return m[4].sign_hl_group == "AnnoxUnread"
end, vim.api.nvim_buf_get_extmarks(buf, require("annox.store").ns, 0, -1, { details = true }))
check(#marked == 1, "the unread comment's sign is marked")

-- Unread threads in files that aren't open are listed too.
bob("two.md", "Goodbye", "Too sad?")
vim.cmd("Annox unread")
check(
  vim.wait(5000, function()
    return vim.fn.getqflist({ title = 0 }).title == "annox: unread"
  end),
  "unread list made"
)
local items = vim.fn.getqflist()
check(#items == 2, "two unread threads, got " .. #items)
local files = vim.tbl_map(function(i)
  return vim.fs.basename(vim.api.nvim_buf_get_name(i.bufnr))
end, items)
table.sort(files)
check(files[1] == "one.md" and files[2] == "two.md", "both files: " .. table.concat(files, ", "))
vim.cmd.cclose()

-- Opening the thread marks it read.
vim.api.nvim_set_current_buf(buf)
vim.api.nvim_win_set_cursor(0, { 1, 7 })
vim.cmd("Annox thread")
check(
  vim.wait(5000, function()
    return annox.unread_count(buf) == 0
  end),
  "opening the thread marks it read"
)
vim.cmd("fclose!")

vim.cmd("Annox read all")
check(
  vim.wait(5000, function()
    return vim.tbl_contains(infos, "annox: marked everything read")
  end),
  "read all"
)
vim.cmd("Annox unread")
check(
  vim.wait(5000, function()
    return vim.tbl_contains(infos, "annox: nothing unread")
  end),
  "nothing unread after read all"
)

print("annox nvim unread: OK")
vim.cmd.qall({ bang = true })
