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

-- Annotated text is tinted with true colors, and underlined without them.
vim.o.termguicolors = true
vim.cmd.colorscheme("default")
local comment_hl = vim.api.nvim_get_hl(0, { name = "AnnoxComment", link = false })
check(comment_hl.bg ~= nil and not comment_hl.undercurl, "comments tinted: " .. vim.inspect(comment_hl))
check(vim.api.nvim_get_hl(0, { name = "AnnoxStale" }).link == "DiagnosticUnderlineWarn", "stale keeps the undercurl")
vim.o.termguicolors = false
vim.cmd.colorscheme("default")
check(
  vim.api.nvim_get_hl(0, { name = "AnnoxComment" }).link == "DiagnosticUnderlineInfo",
  "underline without true colors"
)

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

local function find(pred)
  for _, a in ipairs(annotations()) do
    if pred(a) then
      return a
    end
  end
end

-- A local draft, then published (§5.11).
annox.comment({ range = range(0, 1, 8), body = "draft note", ["local"] = true })
wait("draft", function()
  return find(function(a)
    return a["local"]
  end) ~= nil
end)
local draft = find(function(a)
  return a["local"]
end)
check(#vim.fn.glob(root .. "/.annox/local/docs/**/" .. draft.id .. ".json", false, true) == 1, "draft file is local")
annox.publish({ all = true })
wait("publish", function()
  local a = find(function(x)
    return x.id == draft.id
  end)
  return a and not a["local"]
end)

-- A suggestion goes stale when its text changes, then is re-targeted (§4.2.1).
annox.suggest({ range = range(1, 27, 36), replacement = "this bound" })
wait("second suggestion", function()
  return find(function(a)
    return a.kind == "suggestion"
  end) ~= nil
end)
local stale = find(function(a)
  return a.kind == "suggestion"
end)
vim.api.nvim_buf_set_lines(buf, 1, 2, false, { "In Section 3, we show that a bound is tight." })
wait("suggestion to go stale", function()
  local a = find(function(x)
    return x.id == stale.id
  end)
  return a and a.applicable == false
end)
annox.retarget({ annotation = stale.id, range = range(1, 27, 34), replacement = "this bound" })
wait("re-targeted suggestion to apply again", function()
  local a = find(function(x)
    return x.id == stale.id
  end)
  return a and a.applicable == true
end)

-- Two concurrent edits written to disk, as if merged from two branches.
local create_file = vim.fn.glob(root .. "/.annox/docs/**/" .. draft.id .. ".json", false, true)[1]
for i, body in ipairs({ "mine", "theirs" }) do
  local id = string.format("019a0000-0000-7000-8000-00000000000%d", i)
  local event = {
    id = id,
    annotation = draft.id,
    after = { draft.id },
    type = "edit",
    author = { id = "mailto:bob@example.org", name = "Bob" },
    time = "2026-09-29T10:00:00Z",
    body = body,
  }
  vim.fn.writefile({ vim.json.encode(event) }, vim.fs.dirname(create_file) .. "/" .. id .. ".json")
end
vim.cmd.write()
wait("conflict to be pushed", function()
  local a = find(function(x)
    return x.id == draft.id
  end)
  return a and a.conflicts.body ~= nil
end)
annox.resolve_conflict({ annotation = draft.id, field = "body", value = "merged" })
wait("conflict to resolve", function()
  local a = find(function(x)
    return x.id == draft.id
  end)
  return a and next(a.conflicts) == nil and a.body == "merged"
end)

annox.history({ annotation = draft.id })
wait("history float", function()
  for _, w in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(w).relative ~= "" then
      local text = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\n")
      return text:find("merged", 1, true) ~= nil and text:find("Bob", 1, true) ~= nil
    end
  end
end)

-- Presence: moving the cursor sends it; others' cursors are drawn.
local client = vim.lsp.get_clients({ bufnr = buf, name = "annox" })[1]
local sent = {}
local notify = client.notify
client.notify = function(self, method, params)
  if method == "annox/setPresence" then
    table.insert(sent, params)
  end
  return notify(self, method, params)
end
vim.cmd.wincmd("p") -- leave the history float
vim.api.nvim_set_current_buf(buf)
vim.api.nvim_win_set_cursor(0, { 2, 5 })
vim.api.nvim_exec_autocmds("CursorMoved", {})
wait("presence to be sent", function()
  return #sent > 0
end)
check(sent[#sent].selection.start.line == 1 and sent[#sent].selection.start.character == 5, "cursor sent as presence")

annox.on_presence(nil, {
  peers = {
    {
      author = { id = "mailto:bob@example.org", name = "Bob" },
      textDocument = { uri = vim.uri_from_bufnr(buf) },
      range = range(1, 3, 3),
    },
  },
})
local presence =
  vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox_presence, 0, -1, { details = true })
check(#presence == 1 and presence[1][4].virt_text[1][1] == "▏Bob", "peer cursor drawn with name")

print("annox nvim e2e: OK")
vim.cmd.qall({ bang = true })
