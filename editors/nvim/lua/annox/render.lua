--- Draws annotations in the buffer, and others' cursors (presence).

local store = require("annox.store")
local util = require("annox.util")
local worddiff = require("annox.worddiff")
local highlight_group = require("annox.highlight").highlight_group

local client_for = util.client_for
local byte_col = util.byte_col
local first_line = util.first_line
local word_changes = worddiff.word_changes
local inserted_chunks = worddiff.inserted_chunks

local ns = store.ns

local M = {}

-- Diagnostics about annotations that could not be located (§3.7.3).
local orphan_ns = vim.api.nvim_create_namespace("annox_orphans")

local presence_ns = vim.api.nvim_create_namespace("annox_presence")

--- Whether suggestions in `bufnr` are drawn as struck-through and inserted
--- text instead of underlined.
local function inline(bufnr, a)
  return a.kind == "suggestion" and a.applicable and (store.config.inline_suggestions or store.suggesting[bufnr] ~= nil)
end

--- The number of open annotations in `bufnr` whose text could not be found,
--- e.g. for a statusline.
function M.orphan_count(bufnr)
  local state = store.state[bufnr or vim.api.nvim_get_current_buf()]
  local n = 0
  for _, a in ipairs(state and state.annotations or {}) do
    if a.resolution and a.resolution.state == "orphaned" then
      n = n + 1
    end
  end
  return n
end

--- Draws the annotations of `bufnr` as extmarks.
function M.render(bufnr)
  vim.api.nvim_buf_clear_namespace(bufnr, ns, 0, -1)
  vim.diagnostic.reset(orphan_ns, bufnr)
  if not store.overlay_shown and not store.suggesting[bufnr] then
    return
  end
  local state = store.state[bufnr]
  local client = client_for(bufnr)
  if not state or not client then
    return
  end
  local enc = client.offset_encoding
  local line_count = vim.api.nvim_buf_line_count(bufnr)
  -- Orphaned annotations have no place in the text, so they are reported as
  -- diagnostics (§3.7.3): a warning on the first line, and a hint where each
  -- one probably went (§3.7.4), which also gets a dashed underline.
  local diagnostics = {}
  local orphans = M.orphan_count(bufnr)
  if orphans > 0 then
    table.insert(diagnostics, {
      lnum = 0,
      col = 0,
      severity = vim.diagnostic.severity.WARN,
      source = "annox",
      message = string.format(
        "%d annotation%s could not be located (:Annox orphans)",
        orphans,
        orphans == 1 and "" or "s"
      ),
    })
  end
  for _, a in ipairs(state.annotations) do
    local s = a.resolution and a.resolution.suggested
    if s and s.range.start.line < line_count then
      local label = a.kind == "suggestion" and ("→ " .. (a.edit and a.edit.replacement or ""))
        or first_line(a.body)
        or first_line(a.label)
        or "highlight"
      local sl, el = s.range.start.line, math.min(s.range["end"].line, line_count - 1)
      local sc, ec = byte_col(bufnr, s.range.start, enc), byte_col(bufnr, s.range["end"], enc)
      pcall(
        vim.api.nvim_buf_set_extmark,
        bufnr,
        ns,
        sl,
        sc,
        { end_row = el, end_col = ec, hl_group = "AnnoxSuggested" }
      )
      table.insert(diagnostics, {
        lnum = sl,
        col = sc,
        end_lnum = el,
        end_col = ec,
        severity = vim.diagnostic.severity.HINT,
        source = "annox",
        message = string.format("Orphaned %s may belong here: %s (:Annox orphans)", a.kind, label),
      })
    end
  end
  vim.diagnostic.set(orphan_ns, bufnr, diagnostics)
  for _, a in ipairs(state.annotations) do
    local r = a.resolution and a.resolution.range
    if r and r.start.line < line_count then
      local group = highlight_group(a)
      local sl, el = r.start.line, math.min(r["end"].line, line_count - 1)
      local sc, ec = byte_col(bufnr, r.start, enc), byte_col(bufnr, r["end"], enc)
      local mark = {
        sign_text = a["local"] and "✎" or a.kind == "suggestion" and "±" or "»",
        sign_hl_group = "AnnoxSign",
        priority = 150,
      }
      local label = a.kind == "suggestion" and ("→ " .. (a.edit and a.edit.replacement or ""))
        or first_line(a.body)
        or first_line(a.label)
      if inline(bufnr, a) then
        -- Deleted text struck through, followed by the inserted text.
        if sl ~= el or sc ~= ec then
          mark.end_row, mark.end_col, mark.hl_group = el, ec, "AnnoxDeletion"
        end
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
        local replacement = a.edit and a.edit.replacement or ""
        -- The words that changed get a stronger tint on both sides.
        local ok, old = pcall(vim.api.nvim_buf_get_text, bufnr, sl, sc, el, ec, {})
        local del, add = word_changes(ok and table.concat(old, "\n") or "", replacement)
        for _, c in ipairs(del) do
          pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl + c[1], (c[1] == 0 and sc or 0) + c[2], {
            end_col = (c[1] == 0 and sc or 0) + c[3],
            hl_group = "AnnoxWordDeletion",
            priority = 160,
          })
        end
        if replacement ~= "" then
          pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, el, ec, {
            virt_text = inserted_chunks(replacement, add),
            virt_text_pos = "inline",
            right_gravity = false,
          })
        end
        label = first_line(a.body)
      elseif sl == el and sc == ec then
        mark.virt_text = { { "◆", group } }
        mark.virt_text_pos = "inline"
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
      else
        mark.end_row, mark.end_col, mark.hl_group = el, ec, group
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, sc, mark)
      end
      if store.config.virtual_text and label then
        local replies = #(a.replies or {})
        local text = replies > 0 and string.format("%s (+%d)", label, replies) or label
        if a["local"] then
          text = "[draft] " .. text
        end
        pcall(vim.api.nvim_buf_set_extmark, bufnr, ns, sl, 0, {
          virt_text = { { "  " .. text, "AnnoxVirtualText" } },
          virt_text_pos = "eol",
        })
      end
    end
  end
end

--- Draws others' cursors in `bufnr` (§7.8.2).
function M.render_presence(bufnr)
  vim.api.nvim_buf_clear_namespace(bufnr, presence_ns, 0, -1)
  local client = client_for(bufnr)
  if not client or not store.overlay_shown then
    return
  end
  local uri = vim.uri_from_bufnr(bufnr)
  local line_count = vim.api.nvim_buf_line_count(bufnr)
  for _, peer in ipairs(store.peers) do
    local r = peer.range
    if peer.textDocument and peer.textDocument.uri == uri and r and r.start.line < line_count then
      local author = peer.author or {}
      local name = author.name or author.id or "someone"
      local sl, el = r.start.line, math.min(r["end"].line, line_count - 1)
      local sc, ec = byte_col(bufnr, r.start, client.offset_encoding), byte_col(bufnr, r["end"], client.offset_encoding)
      if sl ~= el or sc ~= ec then
        pcall(vim.api.nvim_buf_set_extmark, bufnr, presence_ns, sl, sc, {
          end_row = el,
          end_col = ec,
          hl_group = "AnnoxPresenceRange",
        })
      end
      pcall(vim.api.nvim_buf_set_extmark, bufnr, presence_ns, sl, sc, {
        virt_text = { { "▏" .. name, "AnnoxPresence" } },
        virt_text_pos = "inline",
      })
    end
  end
end

--- Handles `annox/didChangePresence` (§6.6.3).
function M.on_presence(_, result)
  store.peers = result.peers or {}
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) then
      M.render_presence(b)
    end
  end
end

--- Sends the cursor as presence, at most every 150 ms (§7.8.1).
local presence_timer
local function send_presence()
  if not store.config.presence then
    return
  end
  presence_timer = presence_timer or vim.uv.new_timer()
  presence_timer:stop()
  presence_timer:start(
    150,
    0,
    vim.schedule_wrap(function()
      local bufnr = vim.api.nvim_get_current_buf()
      local client = client_for(bufnr)
      if not client then
        return
      end
      local row, col = unpack(vim.api.nvim_win_get_cursor(0))
      local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
      local pos =
        { line = row - 1, character = vim.str_utfindex(line, client.offset_encoding, math.min(col, #line), false) }
      client:notify("annox/setPresence", {
        textDocument = { uri = vim.uri_from_bufnr(bufnr) },
        selection = { start = pos, ["end"] = pos },
      })
    end)
  )
end

M.send_presence = send_presence

return M
