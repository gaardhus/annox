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
    vim.cmd("cquit 1")
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
local highlight_hl = vim.api.nvim_get_hl(0, { name = "AnnoxHighlight", link = false })
check(comment_hl.underline and highlight_hl.bg ~= nil and not highlight_hl.underline, "only comments underlined")
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
-- The thread stays visible while the reply is typed, and closes afterwards.
local function floats()
  return vim.tbl_filter(function(w)
    return vim.api.nvim_win_get_config(w).relative ~= ""
  end, vim.api.nvim_list_wins())
end
local input = vim.ui.input
vim.ui.input = function(_, on_confirm)
  check(#floats() == 1, "expected the thread float during reply input")
  on_confirm("Section 3.")
end
annox.reply()
vim.ui.input = input
check(#floats() == 0, "thread float should close after reply input")
wait("reply", function()
  for _, a in ipairs(annotations()) do
    if a.id == comment.id then
      return #a.replies == 1
    end
  end
end)

--- The reply's body once it differs from `old`.
local function reply_body(old)
  for _, a in ipairs(annotations()) do
    if a.id == comment.id and a.replies[1].body ~= old then
      return a.replies[1].body
    end
  end
end

-- A reply is edited from the thread float: focus it, put the cursor on the
-- reply, press e.
local source_win = vim.api.nvim_get_current_win()
annox.thread({ annotation = comment.id })
annox.thread({ annotation = comment.id })
check(#floats() == 1 and vim.api.nvim_get_current_win() == floats()[1], "second :Annox thread focuses the float")
check(vim.api.nvim_win_get_config(floats()[1]).footer ~= nil, "the thread float shows its keys")
vim.api.nvim_win_set_cursor(0, { vim.api.nvim_buf_line_count(0), 0 })
vim.api.nvim_feedkeys("e", "x", false)
check(vim.api.nvim_buf_get_name(0):find("annox://body/", 1, true), "e opens the edit window")
check(vim.api.nvim_buf_get_lines(0, 0, -1, false)[1] == "Section 3.", "editing the reply, not the comment")
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "Section 3, Lemma 4." })
vim.cmd.write()
vim.api.nvim_win_close(0, true)
wait("reply edited from the thread", function()
  return reply_body("Section 3.") == "Section 3, Lemma 4."
end)
vim.api.nvim_set_current_win(source_win)

-- :Annox edit offers the comment and its replies.
local select = vim.ui.select
local offered
vim.ui.select = function(items, opts, on_choice)
  offered = vim.tbl_map(opts.format_item, items)
  on_choice(items[2])
end
annox.edit()
vim.ui.select = select
check(offered and #offered == 2 and offered[2]:find("reply", 1, true), "picker: " .. vim.inspect(offered))
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "Section 3." })
vim.cmd.write()
vim.api.nvim_win_close(0, true)
wait("reply edited from the picker", function()
  return reply_body("Section 3, Lemma 4.") == "Section 3."
end)
vim.api.nvim_set_current_win(source_win)

-- A suggestion's thread shows the change as a diff block.
annox.thread({ annotation = suggestion.id })
check(#floats() == 1, "expected the suggestion thread float")
local shown = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(floats()[1]), 0, -1, false), "\n")
check(shown:find("```diff\n- we prove that\n+ we show that\n```", 1, true), "diff block: " .. shown)
local groups = {}
for _, m in ipairs(vim.api.nvim_buf_get_extmarks(vim.api.nvim_win_get_buf(floats()[1]), -1, 0, -1, { details = true })) do
  groups[#groups + 1] = m[4].hl_group
end
check(
  vim.tbl_contains(groups, "AnnoxThreadDeletion") and vim.tbl_contains(groups, "AnnoxThreadInsertion"),
  "diff lines get their text color: " .. vim.inspect(groups)
)
--- The changed words marked in `win`, as "group start-end".
local function word_marks(win)
  local out = {}
  local b = vim.api.nvim_win_get_buf(win)
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(b, -1, 0, -1, { details = true })) do
    if (m[4].hl_group or ""):find("^AnnoxWord") then
      out[#out + 1] = string.format("%s %d-%d", m[4].hl_group, m[3], m[4].end_col)
    end
  end
  table.sort(out)
  return out
end
-- "- we prove that" / "+ we show that": only the verbs.
local function words_match(win)
  return vim.deep_equal(word_marks(win), { "AnnoxWordDeletion 5-10", "AnnoxWordInsertion 5-9" })
end
check(words_match(floats()[1]), "changed words marked: " .. vim.inspect(word_marks(floats()[1])))
vim.api.nvim_win_close(floats()[1], true)

-- The LSP hover gets the same marks.
vim.api.nvim_win_set_cursor(0, { 2, 16 })
vim.lsp.buf.hover()
wait("hover with marked words", function()
  -- Floats are styled as they're drawn, which headless Neovim doesn't do.
  vim.cmd.redraw()
  return #floats() == 1 and words_match(floats()[1])
end)
vim.api.nvim_win_close(floats()[1], true)

-- Accept: the server's workspace/applyEdit edits the buffer.
annox.accept({ annotation = suggestion.id })
wait("accepted edit in buffer", function()
  return vim.api.nvim_buf_get_lines(buf, 1, 2, false)[1] == "In Section 3, we show that the bound is tight."
end)
wait("suggestion to close", function()
  return #annotations() == 1
end)

-- Revert: pick the accepted suggestion from the buffer's closed ones.
local select = vim.ui.select
local offered
vim.ui.select = function(items, _, on_choice)
  offered = items
  on_choice(items[1], 1)
end
annox.revert({ accept = true })
wait("reverted edit in buffer", function()
  return vim.api.nvim_buf_get_lines(buf, 1, 2, false)[1] == "In Section 3, we prove that the bound is tight."
end)
vim.ui.select = select
check(#offered == 1 and offered[1].id == suggestion.id, "offered the accepted suggestion: " .. vim.inspect(offered))
wait("only open annotations pushed again", function()
  return #annotations() == 1
end)

annox.resolve({ annotation = comment.id })
wait("comment to resolve", function()
  return #annotations() == 0
end)

-- Everything on some text, closed annotations included.
annox.here({ range = range(1, 0, 20) })
local here_text
wait("annotations-here float", function()
  for _, w in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(w).relative ~= "" then
      here_text = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\n")
      return true
    end
  end
end)
check(
  here_text:find("resolved comment", 1, true) and here_text:find("Which section?", 1, true),
  "the resolved comment is shown: " .. here_text
)
check(here_text:find("accepted suggestion", 1, true) ~= nil, "the accepted revert is shown: " .. here_text)
for _, w in ipairs(vim.api.nvim_list_wins()) do
  if vim.api.nvim_win_get_config(w).relative ~= "" then
    vim.api.nvim_win_close(w, true)
  end
end

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

-- A highlight is a comment with no body.
annox.highlight({ range = range(0, 2, 5) })
wait("highlight", function()
  return find(function(a)
    return a.kind == "comment" and (a.body == nil or a.body == vim.NIL)
  end) ~= nil
end)
wait("highlight drawn without an underline", function()
  for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, vim.api.nvim_get_namespaces().annox, 0, -1, { details = true })) do
    if m[4].hl_group == "AnnoxHighlight" then
      return true
    end
  end
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

-- Resolving wrote an edit by the draft's own author, so it's marked but not named.
annox.thread({ annotation = draft.id })
check(#floats() == 1, "expected the draft's thread float")
shown = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(floats()[1]), 0, -1, false), "\n")
check(shown:find(" · _edited_\n", 1, true), "edited mark: " .. shown)
vim.api.nvim_win_close(floats()[1], true)

annox.history({ annotation = draft.id })
wait("history float", function()
  for _, w in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_config(w).relative ~= "" then
      local text = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\n")
      return text:find("edited: merged", 1, true) ~= nil and text:find("Bob", 1, true) ~= nil
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

-- Commit: a dry run fills the prompt, and only annotation files are committed.
local function git(args)
  local out = vim.system(vim.list_extend({ "git", "-C", root }, args), { text = true }):wait()
  check(out.code == 0, "git " .. table.concat(args, " ") .. ": " .. out.stderr)
  return out.stdout
end
git({ "init", "--quiet" })
git({ "config", "user.email", "ada@example.org" })
git({ "config", "user.name", "Ada" })
git({ "config", "commit.gpgsign", "false" })
local prompted
vim.ui.input = function(opts, on_confirm)
  prompted = opts
  on_confirm(opts.default)
end
annox.commit()
wait("commit", function()
  return vim.system({ "git", "-C", root, "rev-parse", "--verify", "--quiet", "HEAD" }):wait().code == 0
end)
vim.ui.input = input
check(prompted.default:match("^chore%(annox%): "), "commit message summarized: " .. prompted.default)
check(vim.trim(git({ "status", "--porcelain", "--", ".annox" })) == "", "annotation files committed")
check(git({ "status", "--porcelain", "paper.tex" }):match("paper.tex"), "the document is left alone")

print("annox nvim e2e: OK")
vim.cmd.qall({ bang = true })
