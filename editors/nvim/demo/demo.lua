-- Scripted annox.nvim demo: types into a real Neovim, for asciinema to record.
-- Run through record.sh, which sets up the workspace: `just nvim-demo`.

local here = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
vim.opt.rtp:prepend(vim.fs.dirname(here))
vim.o.termguicolors = true
-- Not detected: record.sh turns off the terminal queries.
vim.o.background = "dark"
vim.o.number = true
vim.o.cursorline = true
vim.o.wrap = true
vim.o.linebreak = true
vim.o.laststatus = 2
vim.o.showmode = false
vim.o.swapfile = false
vim.o.hlsearch = false
vim.o.shortmess = vim.o.shortmess .. "IF"
vim.o.statusline = " %f %m%=%{get(b:,'annox_suggesting',0)?'SUGGESTING ':''} "

local annox = require("annox")
annox.setup({
  cmd = { assert(os.getenv("ANNOX_BIN"), "set ANNOX_BIN"), "lsp" },
  author = { id = "mailto:you@example.org", name = "You" },
  presence = false,
})

local steps = {
  { wait = 1800 },
  -- Read Ada's comment and answer it.
  { type = "/resistance<CR>", wait = 600 },
  { type = ":Annox thread<CR>", wait = 2600 },
  { type = "l", wait = 300 },
  { type = ":Annox reply<CR>", wait = 500 },
  { type = "The feed. I'll make that explicit.<CR>", wait = 1600 },
  -- Comment on a selection.
  { type = "/second pass<CR>", wait = 500 },
  { raw = "vt.", wait = 500 },
  { type = ":Annox comment<CR>", wait = 400 },
  { type = "Nice echo of the title.<CR>", wait = 1800 },
  -- Review and accept Ada's suggestion.
  { type = "/never<CR>", wait = 500 },
  { type = ":Annox thread<CR>", wait = 2600 },
  { type = "h", wait = 300 },
  { type = ":Annox accept<CR>", wait = 2200 },
  -- Suggestion mode: edit as usual, edits become suggestions.
  { raw = "gg", wait = 300 },
  { type = ":Annox suggesting<CR>", wait = 1000 },
  { type = "/skimmed<CR>", wait = 500 },
  { raw = "cw", wait = 150 },
  { type = "glanced at<Esc>", wait = 1500 },
  { type = "/keywords<CR>", wait = 500 },
  { raw = "ciw", wait = 150 },
  { type = "phrases<Esc>", wait = 3500 },
  { type = ":qa!<CR>" },
}

-- Split "abc<CR>" into { "a", "b", "c", "<CR>" }.
local function keys(s)
  local out = {}
  local i = 1
  while i <= #s do
    local special = s:match("^<%a[%w-]*>", i)
    local ch = special or vim.fn.strcharpart(s:sub(i), 0, 1)
    table.insert(out, ch)
    i = i + #ch
  end
  return out
end

local queue = {}
for _, step in ipairs(steps) do
  -- Keys that wait for another key (operators, `t`) go in at once: timers
  -- don't fire while Neovim waits for the rest of a command.
  if step.raw then
    table.insert(queue, { key = step.raw, delay = 0 })
  end
  for _, k in ipairs(keys(step.type or "")) do
    table.insert(queue, { key = k, delay = k:match("^<") and 220 or 75 })
  end
  if step.wait then
    table.insert(queue, { delay = step.wait })
  end
end

local function run(i)
  local item = queue[i]
  if not item then
    return
  end
  if item.key then
    vim.api.nvim_input(item.key)
  end
  vim.defer_fn(function()
    run(i + 1)
  end, item.delay)
end

vim.api.nvim_create_autocmd("VimEnter", {
  callback = function()
    local buf = vim.api.nvim_get_current_buf()
    vim.wait(5000, function()
      return annox.state[buf] ~= nil
    end, 20)
    run(1)
  end,
})
