--- The highlight groups annox defines, and which one an annotation is drawn with.

local store = require("annox.store")
local first_line = require("annox.util").first_line

--- `color` blended over the editor background, `alpha` of the way, as "#rrggbb".
--- A transparent background counts as black, or white with a light 'background'.
local function tint(color, alpha)
  local bg = vim.api.nvim_get_hl(0, { name = "Normal", link = false }).bg
    or (vim.o.background == "light" and 0xffffff or 0)
  local function channel(shift)
    local c, b = bit.band(bit.rshift(color, shift), 0xff), bit.band(bit.rshift(bg, shift), 0xff)
    return math.floor(b + (c - b) * alpha + 0.5)
  end
  return string.format("#%02x%02x%02x", channel(16), channel(8), channel(0))
end

local function set_highlights()
  -- Annotated text gets a background tint in the diagnostic color, so it
  -- isn't mistaken for a diagnostic. Problems (stale, conflict) keep the
  -- undercurl. Without true colors, fall back to the underlines.
  -- A comment is also underlined, to say there's a note to read; a highlight
  -- (a comment with nothing to read) is only tinted.
  local tinted = { AnnoxComment = "Info", AnnoxHighlight = "Info", AnnoxSuggestion = "Hint", AnnoxLocal = "Ok" }
  for group, severity in pairs(tinted) do
    local fg = vim.api.nvim_get_hl(0, { name = "Diagnostic" .. severity, link = false }).fg
    if vim.o.termguicolors and fg then
      local underline = group == "AnnoxComment"
      vim.api.nvim_set_hl(
        0,
        group,
        { default = true, bg = tint(fg, 0.2), underline = underline, sp = underline and fg or nil }
      )
    else
      vim.api.nvim_set_hl(0, group, { default = true, link = "DiagnosticUnderline" .. severity })
    end
  end
  local links = {
    AnnoxStale = "DiagnosticUnderlineWarn",
    AnnoxConflict = "DiagnosticUnderlineError",
    AnnoxVirtualText = "Comment",
    AnnoxSign = "DiagnosticSignInfo",
    AnnoxPresence = "DiagnosticVirtualTextHint",
    AnnoxPresenceRange = "Visual",
    AnnoxInsertion = "Added",
  }
  for group, link in pairs(links) do
    vim.api.nvim_set_hl(0, group, { default = true, link = link })
  end
  local removed = vim.api.nvim_get_hl(0, { name = "Removed", link = false })
  vim.api.nvim_set_hl(
    0,
    "AnnoxDeletion",
    { default = true, strikethrough = store.config.strikethrough, fg = removed.fg }
  )
  -- Where an orphaned annotation probably went (§3.7.4).
  local warn = vim.api.nvim_get_hl(0, { name = "DiagnosticWarn", link = false }).fg
  vim.api.nvim_set_hl(0, "AnnoxSuggested", { default = true, underdashed = true, sp = warn })
  -- The old text above the suggestion edit window.
  vim.api.nvim_set_hl(0, "AnnoxEditOriginal", { default = true, fg = removed.fg })
  -- A suggestion's lines in the thread's diff block: plain text over the diff
  -- background, instead of the code block's color.
  local text = vim.api.nvim_get_hl(0, { name = "NormalFloat", link = false }).fg
    or vim.api.nvim_get_hl(0, { name = "Normal", link = false }).fg
  vim.api.nvim_set_hl(0, "AnnoxThreadDeletion", { default = true, fg = text })
  vim.api.nvim_set_hl(0, "AnnoxThreadInsertion", { default = true, fg = text })
  -- The words that changed within a suggestion, diff-so-fancy style. Only a
  -- background, so the text keeps its color, and a light one, as inline that
  -- text is red or green itself.
  local added_fg = vim.api.nvim_get_hl(0, { name = "Added", link = false }).fg
  if vim.o.termguicolors and removed.fg and added_fg then
    vim.api.nvim_set_hl(0, "AnnoxWordDeletion", { default = true, bg = tint(removed.fg, 0.3) })
    vim.api.nvim_set_hl(0, "AnnoxWordInsertion", { default = true, bg = tint(added_fg, 0.3) })
  else
    vim.api.nvim_set_hl(0, "AnnoxWordDeletion", { default = true, link = "DiffDelete" })
    vim.api.nvim_set_hl(0, "AnnoxWordInsertion", { default = true, link = "DiffAdd" })
  end
  -- Suggestion mode tints the number column and cursor line toward "Added".
  local added = vim.api.nvim_get_hl(0, { name = "Added", link = false }).fg
  vim.api.nvim_set_hl(0, "AnnoxSuggestingCursorLineNr", { default = true, link = "Added" })
  if vim.o.termguicolors and added then
    vim.api.nvim_set_hl(0, "AnnoxSuggestingLineNr", { default = true, fg = tint(added, 0.5) })
    vim.api.nvim_set_hl(0, "AnnoxSuggestingCursorLine", { default = true, bg = tint(added, 0.1) })
  else
    vim.api.nvim_set_hl(0, "AnnoxSuggestingLineNr", { default = true, link = "Added" })
    vim.api.nvim_set_hl(0, "AnnoxSuggestingCursorLine", { default = true, link = "CursorLine" })
  end
end

local function highlight_group(a)
  if type(a.conflicts) == "table" and next(a.conflicts) then
    return "AnnoxConflict"
  elseif a["local"] then
    return "AnnoxLocal"
  elseif a.kind == "suggestion" then
    return a.applicable and "AnnoxSuggestion" or "AnnoxStale"
  elseif first_line(a.body) == nil and #(a.replies or {}) == 0 then
    return "AnnoxHighlight"
  end
  return "AnnoxComment"
end

return {
  set_highlights = set_highlights,
  highlight_group = highlight_group,
}
