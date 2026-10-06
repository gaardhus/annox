--- Helpers shared by the annox modules: talking to the server, positions,
--- and picking the annotation to act on.

local store = require("annox.store")

local function client_for(bufnr)
  return vim.lsp.get_clients({ bufnr = bufnr, name = "annox" })[1]
end

--- Byte column of an LSP position in `bufnr`.
local function byte_col(bufnr, pos, encoding)
  local line = vim.api.nvim_buf_get_lines(bufnr, pos.line, pos.line + 1, false)[1] or ""
  local ok, col = pcall(vim.str_byteindex, line, encoding, pos.character, false)
  return ok and col or #line
end

local function first_line(text)
  return type(text) == "string" and text:match("^[^\n]*") or nil
end

--- Sends an `annox/*` request for the current buffer.
local function request(bufnr, method, params, done)
  local client = client_for(bufnr)
  if not client then
    vim.notify("annox: no annox server attached to this buffer", vim.log.levels.WARN)
    return
  end
  client:request(method, params, function(err, result)
    if err then
      vim.notify(string.format("annox: %s failed: %s", method, err.message), vim.log.levels.ERROR)
    elseif done then
      done(result)
    end
  end, bufnr)
end

--- The LSP position of the cursor.
local function cursor_position(bufnr, encoding)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
  return { line = row - 1, character = vim.str_utfindex(line, encoding, math.min(col, #line), false) }
end

--- The LSP range of the last visual selection.
local function visual_range(bufnr, encoding)
  local s, e = vim.fn.getpos("'<"), vim.fn.getpos("'>")
  local function position(row, byte)
    local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
    byte = math.min(byte, #line)
    return { line = row - 1, character = vim.str_utfindex(line, encoding, byte, false) }
  end
  local end_line = vim.api.nvim_buf_get_lines(bufnr, e[2] - 1, e[2], false)[1] or ""
  local end_byte = #end_line
  if e[3] <= #end_line then
    end_byte = e[3] + vim.str_utf_end(end_line, e[3]) -- one past the last selected character
  end
  return { start = position(s[2], s[3] - 1), ["end"] = position(e[2], end_byte) }
end

local function before(a, b)
  return a.line < b.line or (a.line == b.line and a.character <= b.character)
end

--- Annotations whose range contains the cursor.
local function under_cursor(bufnr)
  local state, client = store.state[bufnr], client_for(bufnr)
  if not state or not client then
    return {}
  end
  local enc = client.offset_encoding
  local pos = cursor_position(bufnr, enc)
  -- The cursor's character ends where an empty range starts: an insertion
  -- or point comment is drawn between the two characters, so both count.
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = vim.api.nvim_buf_get_lines(bufnr, row - 1, row, false)[1] or ""
  local after = col < #line and col + vim.str_utf_end(line, col + 1) + 1 or col
  local hits = {}
  for _, a in ipairs(state.annotations) do
    local r = a.resolution and a.resolution.range
    local empty = r and r.start.line == r["end"].line and r.start.character == r["end"].character
    local touching = empty and r.start.line == row - 1 and byte_col(bufnr, r.start, enc) == after
    if r and (touching or (before(r.start, pos) and before(pos, r["end"]))) then
      table.insert(hits, a)
    end
  end
  return hits
end

local function buffer_annotations(bufnr, keep)
  return vim.tbl_filter(keep, (store.state[bufnr] or {}).annotations or {})
end

local function describe(a)
  if a.kind == "suggestion" then
    return string.format("suggestion → %s", a.edit and a.edit.replacement or "")
  end
  return string.format("comment: %s", first_line(a.body) or first_line(a.label) or "(highlight)")
end

--- Calls `fn(id)` with `opts.annotation`, or with the annotation under the
--- cursor (asking if there are several), filtered by `keep`.
local function with_annotation(opts, keep, fn)
  if opts.annotation then
    return fn(opts.annotation)
  end
  local hits = vim.tbl_filter(keep or function()
    return true
  end, under_cursor(vim.api.nvim_get_current_buf()))
  if #hits == 0 then
    vim.notify("annox: no matching annotation under the cursor", vim.log.levels.INFO)
  elseif #hits == 1 then
    fn(hits[1].id)
  else
    vim.ui.select(hits, { prompt = "Annotation", format_item = describe }, function(a)
      if a then
        fn(a.id)
      end
    end)
  end
end

--- Calls `fn(text)` with `value`, or asks for it.
local function with_input(value, prompt, default, fn)
  if value then
    return fn(value)
  end
  vim.ui.input({ prompt = prompt, default = default }, function(text)
    if text and text ~= "" then
      fn(text)
    end
  end)
end

local function target_range(bufnr, opts)
  local enc = client_for(bufnr).offset_encoding
  if opts.range then
    return opts.range
  elseif opts.visual then
    return visual_range(bufnr, enc)
  end
  local pos = cursor_position(bufnr, enc)
  return { start = pos, ["end"] = pos }
end

local function find_annotation(bufnr, id)
  for _, a in ipairs((store.state[bufnr] or {}).annotations or {}) do
    if a.id == id then
      return a
    end
  end
end

--- The buffer text `a` resolves to, or "" when it has no range.
local function resolved_text(a, bufnr)
  local r = type(a.resolution) == "table" and a.resolution.range
  local client = client_for(bufnr)
  if type(r) ~= "table" or not client then
    return ""
  end
  local enc = client.offset_encoding
  local ok, text = pcall(
    vim.api.nvim_buf_get_text,
    bufnr,
    r.start.line,
    byte_col(bufnr, r.start, enc),
    r["end"].line,
    byte_col(bufnr, r["end"], enc),
    {}
  )
  return ok and table.concat(text, "\n") or ""
end

--- Calls `fn(annotation)` with `opts.annotation`'s view, the only candidate,
--- or the one the user picks.
local function pick(opts, candidates, prompt, fn)
  if opts.annotation then
    for _, a in ipairs(candidates) do
      if a.id == opts.annotation then
        return fn(a)
      end
    end
    return vim.notify("annox: that annotation doesn't apply here", vim.log.levels.WARN)
  end
  if #candidates == 0 then
    return vim.notify("annox: nothing to " .. prompt:lower(), vim.log.levels.INFO)
  elseif #candidates == 1 then
    return fn(candidates[1])
  end
  vim.ui.select(candidates, { prompt = prompt, format_item = describe }, function(a)
    if a then
      fn(a)
    end
  end)
end

return {
  client_for = client_for,
  byte_col = byte_col,
  first_line = first_line,
  request = request,
  visual_range = visual_range,
  before = before,
  under_cursor = under_cursor,
  buffer_annotations = buffer_annotations,
  describe = describe,
  with_annotation = with_annotation,
  with_input = with_input,
  target_range = target_range,
  find_annotation = find_annotation,
  resolved_text = resolved_text,
  pick = pick,
}
