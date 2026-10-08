--- Commands that create annotations or change their status: comment,
--- suggest, reply, accept, reject, resolve, revert, publish and commit.

local util = require("annox.util")
local open_thread = require("annox.thread").open_thread

local client_for = util.client_for
local byte_col = util.byte_col
local request = util.request
local visual_range = util.visual_range
local before = util.before
local buffer_annotations = util.buffer_annotations
local describe = util.describe
local with_annotation = util.with_annotation
local with_input = util.with_input
local target_range = util.target_range
local find_annotation = util.find_annotation

local M = {}

--- Creates a comment on `opts.range`, the visual selection, or the cursor.
--- opts: { body?, range?, visual?, ["local"]? }
function M.comment(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  local range = target_range(bufnr, opts)
  with_input(opts.body, "Comment: ", nil, function(body)
    request(bufnr, "annox/create", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      kind = "comment",
      range = range,
      body = body,
      ["local"] = opts["local"] or false,
    })
  end)
end

--- Highlights `opts.range` or the visual selection: a comment with no body.
--- opts: { range?, visual?, ["local"]? }
function M.highlight(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  if not opts.range and not opts.visual then
    return vim.notify("annox: select the text to highlight", vim.log.levels.WARN)
  end
  request(bufnr, "annox/create", {
    textDocument = { uri = vim.uri_from_bufnr(bufnr) },
    kind = "comment",
    range = target_range(bufnr, opts),
    ["local"] = opts["local"] or false,
  })
end

--- Suggests replacing the selection (or inserting at the cursor).
--- opts: { replacement?, range?, visual? }
function M.suggest(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if not client_for(bufnr) then
    return request(bufnr)
  end
  local range = target_range(bufnr, opts)
  local lines = vim.api.nvim_buf_get_text(
    bufnr,
    range.start.line,
    byte_col(bufnr, range.start, client_for(bufnr).offset_encoding),
    range["end"].line,
    byte_col(bufnr, range["end"], client_for(bufnr).offset_encoding),
    {}
  )
  with_input(opts.replacement, "Replace with: ", table.concat(lines, "\n"), function(replacement)
    request(bufnr, "annox/create", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      kind = "suggestion",
      range = range,
      replacement = replacement,
    })
  end)
end

--- Replies to the thread under the cursor, showing the thread while the reply
--- is typed. opts: { annotation?, body? }
function M.reply(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  with_annotation(opts, nil, function(id)
    local send = function(body)
      request(bufnr, "annox/reply", { parent = id, body = body })
    end
    local a = find_annotation(bufnr, id)
    if opts.body or not a then
      return with_input(opts.body, "Reply: ", nil, send)
    end
    local _, win = open_thread(a, bufnr)
    -- The preview closes when its buffer is left; keep it up while the
    -- input (often a float of its own) has focus.
    pcall(vim.api.nvim_del_augroup_by_name, "nvim.preview_window_" .. win)
    vim.ui.input({ prompt = "Reply: " }, function(body)
      if vim.api.nvim_win_is_valid(win) then
        vim.api.nvim_win_close(win, true)
      end
      if body and body ~= "" then
        send(body)
      end
    end)
  end)
end

--- Deletes the comment or suggestion under the cursor, or one of its replies,
--- asking which. Deleting hides it, and `:Annox restore` brings it back.
--- opts: { annotation? }
function M.delete(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local roots = opts.annotation and buffer_annotations(bufnr, function()
    return true
  end) or util.under_cursor(bufnr)
  util.pick(opts, util.with_replies(roots), "Delete", function(a)
    request(bufnr, "annox/delete", { annotation = a.id }, function()
      vim.notify(string.format("annox: deleted the %s (:Annox restore brings it back)", a.kind))
    end)
  end)
end

--- Restores a deleted comment, suggestion or reply of the buffer, asking
--- which, most recently created first. opts: { annotation? }
function M.restore(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local function restore(id)
    request(bufnr, "annox/restore", { annotation = id })
  end
  if opts.annotation then
    return restore(opts.annotation)
  end
  -- Deleted annotations aren't pushed, so ask for them once, then go back to
  -- open ones only, as `M.revert` does.
  local doc = { uri = vim.uri_from_bufnr(bufnr) }
  local all = { textDocument = doc, includeClosed = true, includeDeleted = true }
  request(bufnr, "annox/annotations", all, function(result)
    request(bufnr, "annox/annotations", { textDocument = doc })
    local deleted = {}
    for _, a in ipairs(result.annotations) do
      if a.deleted then
        table.insert(deleted, a)
      else
        -- A deleted root brings its replies back with it, so only the replies
        -- of roots that are shown are listed on their own.
        for _, r in ipairs(a.replies or {}) do
          if r.deleted then
            table.insert(deleted, r)
          end
        end
      end
    end
    if #deleted == 0 then
      return vim.notify("annox: nothing deleted in this buffer", vim.log.levels.INFO)
    end
    table.sort(deleted, function(a, b)
      return a.id > b.id
    end)
    vim.ui.select(deleted, { prompt = "Restore", format_item = describe }, function(a)
      if a then
        restore(a.id)
      end
    end)
  end)
end

local function status_action(status, keep)
  return function(opts)
    opts = opts or {}
    local bufnr = vim.api.nvim_get_current_buf()
    with_annotation(opts, keep, function(id)
      request(bufnr, "annox/setStatus", { annotation = id, status = status })
    end)
  end
end

local function is_kind(kind, status)
  return function(a)
    return a.kind == kind and (status == nil or a.status == status)
  end
end

M.reopen = status_action("open", function(a)
  return a.status ~= "open"
end)

--- Undoes an accepted suggestion with a new suggestion that restores the
--- original text (§4.3.4). `opts.annotation` names it; otherwise the buffer's
--- accepted suggestions are offered, newest first. `opts.accept` applies the
--- revert now (true) or leaves it open for review (false); otherwise you're
--- asked.
function M.revert(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local function revert(id, accept)
    request(bufnr, "annox/revert", { annotation = id, accept = accept })
  end
  local function choose(id)
    if opts.accept ~= nil then
      return revert(id, opts.accept)
    end
    local choices = { "Revert now", "Suggest reverting (leave it open for review)" }
    vim.ui.select(choices, { prompt = "Revert the accepted suggestion" }, function(_, i)
      if i then
        revert(id, i == 1)
      end
    end)
  end
  if opts.annotation then
    return choose(opts.annotation)
  end
  -- Closed annotations aren't pushed, so ask for them once, then go back to
  -- open ones only, which the next pushes follow (§6.6.3). A push in between
  -- may include closed ones, which `on_annotations` drops.
  local doc = { uri = vim.uri_from_bufnr(bufnr) }
  request(bufnr, "annox/annotations", { textDocument = doc, includeClosed = true }, function(result)
    request(bufnr, "annox/annotations", { textDocument = doc })
    -- Leave out those already reverted, and mark those with a revert open.
    local reverts = {}
    for _, a in ipairs(result.annotations) do
      if type(a.reverts) == "string" and (a.status == "accepted" or a.status == "open") then
        reverts[a.reverts] = reverts[a.reverts] == "accepted" and "accepted" or a.status
      end
    end
    local accepted = vim.tbl_filter(function(a)
      return a.kind == "suggestion" and a.status == "accepted" and reverts[a.id] ~= "accepted"
    end, result.annotations)
    if #accepted == 0 then
      return vim.notify("annox: no accepted suggestions to revert in this buffer", vim.log.levels.INFO)
    end
    table.sort(accepted, function(a, b)
      return (a.created or "") > (b.created or "")
    end)
    local function format(a)
      return describe(a) .. (reverts[a.id] == "open" and " (revert suggested)" or "")
    end
    vim.ui.select(accepted, { prompt = "Accepted suggestion", format_item = format }, function(a)
      if a then
        choose(a.id)
      end
    end)
  end)
end

--- The open annotations of `kind` shown in the buffer: those touching the
--- last visual selection with `visual`, and all of them otherwise. Nil if no
--- server is attached.
local function open_in_buffer(bufnr, kind, visual)
  local client = client_for(bufnr)
  if not client then
    return request(bufnr)
  end
  local sel = visual and visual_range(bufnr, client.offset_encoding)
  return buffer_annotations(bufnr, function(a)
    local r = a.resolution and a.resolution.range
    return a.kind == kind
      and a.status == "open"
      and r ~= nil
      and (not sel or (before(r.start, sel["end"]) and before(sel.start, r["end"])))
  end)
end

--- Accepts several suggestions as one edit, which a single undo reverts
--- (§6.6.2), and reports the ones the server skipped. Suggestions found by
--- partial context (steps 3 and 5) need `confirmed`, which is asked for once
--- for the whole batch if it isn't given (§4.3).
local function accept_all(bufnr, suggestions, confirmed, fresh)
  if #suggestions == 0 then
    return vim.notify("annox: no suggestions to accept", vim.log.levels.INFO)
  end
  if not fresh then
    -- Pushed state can lag behind the buffer; resolve against it now.
    local params = { textDocument = { uri = vim.uri_from_bufnr(bufnr) } }
    return request(bufnr, "annox/annotations", params, function(result)
      local wanted = {}
      for _, a in ipairs(suggestions) do
        wanted[a.id] = true
      end
      local current = vim.tbl_filter(function(a)
        return wanted[a.id] and a.resolution and a.resolution.range ~= nil
      end, result.annotations)
      accept_all(bufnr, current, confirmed, true)
    end)
  end
  local partial = #vim.tbl_filter(function(a)
    return a.resolution.step == 3 or a.resolution.step == 5
  end, suggestions)
  if partial > 0 and confirmed == nil then
    local all = string.format("Accept all %d", #suggestions)
    local rest = string.format("Accept only the other %d", #suggestions - partial)
    local choices = partial < #suggestions and { all, rest, "Cancel" } or { all, "Cancel" }
    local prompt = string.format(
      "%d of these suggestion%s moved because the text around %s changed. Check %s shown in the right place.",
      partial,
      partial == 1 and "" or "s",
      partial == 1 and "it" or "them",
      partial == 1 and "it is" or "they are"
    )
    return vim.ui.select(choices, { prompt = prompt }, function(choice)
      if choice == all or choice == rest then
        accept_all(bufnr, suggestions, choice == all, true)
      end
    end)
  end
  table.sort(suggestions, function(a, b)
    return before(a.resolution.range.start, b.resolution.range.start)
  end)
  local ids = vim.tbl_map(function(a)
    return a.id
  end, suggestions)
  request(bufnr, "annox/acceptAll", { annotations = ids, confirmed = confirmed or false }, function(result)
    local accepted, skipped = 0, {}
    for _, r in ipairs(result.results or {}) do
      if r.error then
        local message = type(r.error) == "table" and r.error.message or tostring(r.error)
        skipped[message] = (skipped[message] or 0) + 1
      else
        accepted = accepted + 1
      end
    end
    local parts = { string.format("accepted %d", accepted) }
    for message, n in pairs(skipped) do
      table.insert(parts, string.format("skipped %d (%s)", n, message))
    end
    local level = next(skipped) and vim.log.levels.WARN or vim.log.levels.INFO
    vim.notify("annox: " .. table.concat(parts, "; "), level)
  end)
end

--- Accepts the suggestion under the cursor: the server sends the edit back
--- as `workspace/applyEdit` (§6.6.2). With `visual`, accepts every open
--- suggestion touching the last visual selection, and with `all`, every open
--- suggestion in the buffer. `confirmed` answers the prompt about suggestions
--- found by partial context in advance. opts: { annotation?, visual?, all?, confirmed? }
function M.accept(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  if opts.visual or opts.all then
    local suggestions = open_in_buffer(bufnr, "suggestion", opts.visual)
    return suggestions and accept_all(bufnr, suggestions, opts.confirmed)
  end
  with_annotation(opts, is_kind("suggestion", "open"), function(id)
    request(bufnr, "annox/accept", { annotation = id })
  end)
end

--- Sets `status` on every annotation in `items`, one `status` event each,
--- after asking once for more than one unless `confirmed`. `verb` and `noun`
--- word the prompt and the report, e.g. "Reject" and "suggestion".
local function set_status_all(bufnr, items, status, verb, noun, confirmed)
  if #items == 0 then
    return vim.notify(string.format("annox: no %ss to %s", noun, verb:lower()), vim.log.levels.INFO)
  end
  local function run()
    local client, pending, done = client_for(bufnr), #items, 0
    for _, a in ipairs(items) do
      client:request("annox/setStatus", { annotation = a.id, status = status }, function(err)
        done = done + (err and 0 or 1)
        pending = pending - 1
        if pending == 0 then
          local failed = #items - done
          local message = string.format("annox: %s %d", status, done)
          if failed > 0 then
            message = message .. string.format("; %d failed", failed)
          end
          vim.notify(message, failed > 0 and vim.log.levels.WARN or vim.log.levels.INFO)
        end
      end, bufnr)
    end
  end
  if #items == 1 or confirmed then
    return run()
  end
  local yes = string.format("%s all %d", verb, #items)
  vim.ui.select({ yes, "Cancel" }, { prompt = string.format("%s %d %ss?", verb, #items, noun) }, function(choice)
    if choice == yes then
      run()
    end
  end)
end

--- A status action on the annotation under the cursor that, with `visual` or
--- `all`, applies to every open annotation of `kind` touching the last visual
--- selection or in the buffer. These don't touch the text, so each is its own
--- `status` event and can be reopened. opts: { annotation?, visual?, all?, confirmed? }
local function bulk_status_action(kind, status, verb)
  local one = status_action(status, is_kind(kind, "open"))
  return function(opts)
    opts = opts or {}
    if not (opts.visual or opts.all) then
      return one(opts)
    end
    local bufnr = vim.api.nvim_get_current_buf()
    local items = open_in_buffer(bufnr, kind, opts.visual)
    if items then
      set_status_all(bufnr, items, status, verb, kind, opts.confirmed)
    end
  end
end

M.reject = bulk_status_action("suggestion", "rejected", "Reject")
M.resolve = bulk_status_action("comment", "resolved", "Resolve")

--- Publishes local drafts (§5.11): the one under the cursor, or all of the
--- buffer's drafts with `opts.all`. opts: { annotation?, all? }
function M.publish(opts)
  opts = opts or {}
  local bufnr = vim.api.nvim_get_current_buf()
  local function is_local(a)
    return a["local"]
  end
  if opts.all then
    local ids = vim.tbl_map(function(a)
      return a.id
    end, buffer_annotations(bufnr, is_local))
    if #ids == 0 then
      return vim.notify("annox: no drafts to publish", vim.log.levels.INFO)
    end
    return request(bufnr, "annox/publish", { annotations = ids })
  end
  with_annotation(opts, is_local, function(id)
    request(bufnr, "annox/publish", { annotations = { id } })
  end)
end

--- Commits the workspace's annotation files to git (`annox/commit`), after
--- showing how many there are and letting you edit the message.
function M.commit()
  local bufnr = vim.api.nvim_get_current_buf()
  local doc = { uri = vim.uri_from_bufnr(bufnr) }
  request(bufnr, "annox/commit", { textDocument = doc, dryRun = true }, function(planned)
    local n = planned.files
    if n == 0 then
      return vim.notify("annox: no annotation changes to commit", vim.log.levels.INFO)
    end
    local prompt = string.format("Commit %d annotation file%s: ", n, n == 1 and "" or "s")
    vim.ui.input({ prompt = prompt, default = planned.message }, function(message)
      if not message or vim.trim(message) == "" then
        return
      end
      request(bufnr, "annox/commit", { textDocument = doc, message = message }, function(result)
        if result.commit == vim.NIL then
          return vim.notify("annox: no annotation changes to commit", vim.log.levels.INFO)
        end
        vim.notify(string.format("annox: committed %s %s", result.commit:sub(1, 7), message), vim.log.levels.INFO)
      end)
    end)
  end)
end

M.set_status_all = set_status_all

return M
