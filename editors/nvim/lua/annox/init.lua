--- annox for Neovim: a thin client for the annox language server (spec §6).
---
--- The server owns all annox logic. This plugin renders the annotations it
--- pushes (`annox/didChangeAnnotations`) and sends user actions as `annox/*`
--- requests. Suggested edits come back as `workspace/applyEdit`, which
--- Neovim's LSP client applies to the buffer.
---
--- This module sets the plugin up and is its public API, gathered from the
--- modules next to it. Shared state lives in `annox.store`.

local actions = require("annox.actions")
local edit = require("annox.edit")
local highlight = require("annox.highlight")
local orphans = require("annox.orphans")
local qflist = require("annox.qflist")
local render = require("annox.render")
local store = require("annox.store")
local suggest_mode = require("annox.suggest_mode")
local thread = require("annox.thread")
local util = require("annox.util")

local client_for = util.client_for
local set_highlights = highlight.set_highlights
local send_presence = render.send_presence
local style_hover = thread.style_hover
local refresh_list = qflist.refresh_list
local rebase = suggest_mode.rebase
local sync_tint = suggest_mode.sync_tint

local M = {}

-- Reading or setting these fields of this module goes through to the store.
local shared = { config = true, state = true, peers = true, suggesting = true, overlay_shown = true }
setmetatable(M, {
  __index = function(_, k)
    if shared[k] then
      return store[k]
    end
  end,
  __newindex = function(t, k, v)
    if shared[k] then
      store[k] = v
    else
      rawset(t, k, v)
    end
  end,
})

M.orphan_count = render.orphan_count
M.render = render.render
M.render_presence = render.render_presence
M.on_presence = render.on_presence
M.thread = thread.thread
M.history = thread.history
M.here = thread.here
M.comment = actions.comment
M.highlight = actions.highlight
M.suggest = actions.suggest
M.reply = actions.reply
M.revert = actions.revert
M.accept = actions.accept
M.publish = actions.publish
M.commit = actions.commit
M.reopen = actions.reopen
M.reject = actions.reject
M.resolve = actions.resolve
M.list = qflist.list
M.orphans = orphans.orphans
M.edit = edit.edit
M.retarget = edit.retarget
M.reattach = edit.reattach
M.resolve_conflict = edit.resolve_conflict
M.undo_suggestion = suggest_mode.undo_suggestion
M.is_suggesting = suggest_mode.is_suggesting
M.statusline = suggest_mode.statusline
M.suggest_mode = suggest_mode.suggest_mode

local function on_annotations(_, result)
  local bufnr = vim.uri_to_bufnr(result.textDocument.uri)
  if not vim.api.nvim_buf_is_loaded(bufnr) then
    return
  end
  -- Only open annotations are shown. The server pushes closed ones too while
  -- `M.revert` has asked for them.
  local annotations = vim.tbl_filter(function(a)
    return a.status == nil or a.status == "open"
  end, result.annotations)
  store.state[bufnr] = { annotations = annotations, document = result.document }
  local s = store.suggesting[bufnr]
  if s then
    -- Keep only suggestions that can still be extended.
    local fresh = {}
    for _, a in ipairs(annotations) do
      if s.views[a.id] then
        fresh[a.id] = a
      end
    end
    s.views = fresh
  end
  M.render(bufnr)
  refresh_list(bufnr)
end

--- Shows or hides annotations and others' cursors in every buffer. Commands
--- still act on hidden annotations, and a buffer in suggestion mode keeps
--- showing its own.
--- opts: { enable? (default: toggle) }
function M.overlay(opts)
  opts = opts or {}
  local enable = opts.enable
  if enable == nil then
    enable = not store.overlay_shown
  end
  store.overlay_shown = enable
  vim.g.annox_overlay = enable
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) then
      M.render(b)
      M.render_presence(b)
    end
  end
  vim.api.nvim_exec_autocmds("User", { pattern = "AnnoxOverlay", modeline = false, data = { enabled = enable } })
  vim.cmd.redrawstatus({ bang = true })
  vim.notify("annox: overlay " .. (enable and "on" or "off"), vim.log.levels.INFO)
end

--- Applies server edits (accepting a suggestion) without turning them into
--- new suggestions.
local function on_apply_edit(err, result, ctx)
  local response = vim.lsp.handlers["workspace/applyEdit"](err, result, ctx)
  for bufnr in pairs(store.suggesting) do
    rebase(bufnr)
  end
  return response
end

--- Creates an annox workspace (§5.3), by default at the buffer's git root or
--- the working directory, and attaches the server to its open buffers.
--- opts: { root?, confirm? (default true) }
function M.init(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local root = opts.root or vim.fs.root(bufnr, { ".git" }) or vim.fn.getcwd()
  local annox = vim.fs.joinpath(root, ".annox")
  if vim.uv.fs_stat(vim.fs.joinpath(annox, "annox.json")) then
    return vim.notify("annox: " .. root .. " is already an annox workspace", vim.log.levels.INFO)
  end
  if opts.confirm ~= false and vim.fn.confirm("Create an annox workspace in " .. root .. "?", "&Yes\n&No", 2) ~= 1 then
    return
  end
  vim.fn.mkdir(annox, "p")
  vim.fn.writefile({ '{ "format": 1 }' }, vim.fs.joinpath(annox, "annox.json"))
  vim.fn.writefile({ "cache/", "local/", "synced/" }, vim.fs.joinpath(annox, ".gitignore"))
  -- vim.lsp.enable attaches on FileType; replay it for buffers in the workspace.
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(b)
    if vim.api.nvim_buf_is_loaded(b) and vim.startswith(name, root .. "/") then
      vim.api.nvim_exec_autocmds("FileType", { buffer = b })
    end
  end
  vim.notify("annox: created a workspace in " .. root, vim.log.levels.INFO)
end

--- Waits until the server has attached to `bufnr` and pushed its annotations,
--- which it may still be doing when a command runs right after the plugin is
--- lazy-loaded. Starts the server if the workspace was created since the
--- buffer was opened, as `annox init` from a shell does. Returns whether the
--- buffer is ready, and says why not when it isn't.
local function wait_ready(bufnr)
  local function ready()
    return client_for(bufnr) ~= nil and store.state[bufnr] ~= nil
  end
  if ready() then
    return true
  end
  if not vim.fs.root(bufnr, { ".annox" }) then
    vim.notify("annox: this file is not in an annox workspace; run :Annox init", vim.log.levels.WARN)
    return false
  end
  if not client_for(bufnr) then
    -- Only vim.lsp.enable's handler, which reuses a server that is starting.
    vim.api.nvim_exec_autocmds("FileType", { group = "nvim.lsp.enable", buffer = bufnr })
  end
  if vim.wait(3000, ready, 10) then
    return true
  end
  vim.notify("annox: the server did not attach to this buffer; see :checkhealth vim.lsp", vim.log.levels.WARN)
  return false
end

local subcommands = {
  init = function()
    M.init()
  end,
  comment = function(o)
    M.comment({ visual = o.range > 0 })
  end,
  draft = function(o)
    M.comment({ visual = o.range > 0, ["local"] = true })
  end,
  highlight = function(o)
    M.highlight({ visual = o.range > 0 })
  end,
  publish = function(o)
    M.publish({ all = o.bang })
  end,
  retarget = function(o)
    M.retarget({ visual = o.range > 0 })
  end,
  reattach = function(o)
    M.reattach({ visual = o.range > 0 })
  end,
  conflicts = function()
    M.resolve_conflict()
  end,
  history = function()
    M.history()
  end,
  here = function(o)
    M.here({ visual = o.range > 0 })
  end,
  suggest = function(o)
    M.suggest({ visual = o.range > 0 })
  end,
  suggesting = function()
    M.suggest_mode()
  end,
  overlay = function(o)
    local arg = o.fargs[2]
    if arg ~= nil and arg ~= "on" and arg ~= "off" then
      return vim.notify("annox: usage: :Annox overlay [on|off]", vim.log.levels.ERROR)
    end
    M.overlay({ enable = arg and arg == "on" })
  end,
  edit = function()
    M.edit()
  end,
  reply = function()
    M.reply()
  end,
  resolve = function(o)
    M.resolve({ visual = o.range > 0, all = o.bang })
  end,
  reopen = function()
    M.reopen()
  end,
  revert = function()
    M.revert()
  end,
  accept = function(o)
    M.accept({ visual = o.range > 0, all = o.bang })
  end,
  reject = function(o)
    M.reject({ visual = o.range > 0, all = o.bang })
  end,
  thread = function()
    M.thread()
  end,
  orphans = function(o)
    M.orphans({ visual = o.range > 0 })
  end,
  list = function()
    M.list()
  end,
  commit = function()
    M.commit()
  end,
}

function M.setup(opts)
  store.config = vim.tbl_deep_extend("force", store.config, opts or {})
  vim.g.annox_overlay = store.overlay_shown
  set_highlights()
  vim.api.nvim_create_autocmd("ColorScheme", { callback = set_highlights })
  vim.lsp.config("annox", {
    cmd = store.config.cmd,
    root_dir = function(bufnr, on_dir)
      local root = vim.fs.root(bufnr, { ".annox" })
      if root then
        on_dir(root)
      end
    end,
    capabilities = { experimental = { annox = { version = "0.1" } } },
    init_options = { annox = { diagnostics = false, author = store.config.author } },
    handlers = {
      ["annox/didChangeAnnotations"] = on_annotations,
      ["annox/didChangePresence"] = M.on_presence,
      ["workspace/applyEdit"] = on_apply_edit,
    },
  })
  vim.lsp.enable("annox")
  vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI", "BufEnter" }, {
    group = vim.api.nvim_create_augroup("annox_presence", { clear = true }),
    callback = send_presence,
  })
  -- A window keeps its 'winhighlight' when it switches buffers; re-sync it.
  vim.api.nvim_create_autocmd({ "BufWinEnter", "WinEnter" }, {
    group = vim.api.nvim_create_augroup("annox_suggesting_tint", { clear = true }),
    callback = function()
      sync_tint(vim.api.nvim_get_current_win())
    end,
  })
  -- The hover (`K`) merges every server's answer into one float, with no
  -- hook per server, and plugins like noice.nvim draw it in windows opened
  -- without autocommands. So check each float as it is drawn, once per
  -- change, for the diff of a suggestion under the cursor.
  local seen = {}
  vim.api.nvim_set_decoration_provider(vim.api.nvim_create_namespace("annox_hover"), {
    on_win = function(_, win, fbuf)
      local tick = vim.api.nvim_buf_get_changedtick(fbuf)
      if seen[fbuf] ~= tick and vim.api.nvim_win_get_config(win).relative ~= "" then
        seen[fbuf] = tick
        vim.schedule(function()
          style_hover(win, fbuf)
        end)
      end
      return false
    end,
  })
  vim.api.nvim_create_user_command("Annox", function(o)
    local fn = subcommands[o.fargs[1]]
    if not fn then
      return vim.notify("annox: unknown subcommand " .. tostring(o.fargs[1]), vim.log.levels.ERROR)
    end
    if o.fargs[1] ~= "init" and o.fargs[1] ~= "overlay" and not wait_ready(vim.api.nvim_get_current_buf()) then
      return
    end
    fn(o)
  end, {
    nargs = "+",
    range = true,
    bang = true,
    complete = function(lead, line)
      local args = vim.split(line, "%s+", { trimempty = true })
      local words = vim.tbl_keys(subcommands)
      if #args > 2 or (#args == 2 and line:match("%s$")) then
        words = args[2] == "overlay" and { "on", "off" } or {}
      end
      return vim.tbl_filter(function(w)
        return vim.startswith(w, lead)
      end, words)
    end,
  })
end

return M
