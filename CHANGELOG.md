# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/gaardhus/annox/compare/v0.1.0...v0.1.1) - 2026-09-29

### Fixed

- *(nvim)* Support Neovim 0.11 again

## [0.1.0](https://github.com/gaardhus/annox/releases/tag/v0.1.0) - 2026-09-29

### Other

- Initial commit
- Add spec skeleton and draft anchoring section
- Draft suggestions section
- Draft data model section
- Draft overview section
- Draft storage section
- Draft protocol section
- Fix stale references after consistency review
- Resolve compaction and deletion-privacy questions
- Add sync section and local-only annotations
- Resolve suggestion grouping, re-targeting, and bulk accept
- Remove empty open-questions heading from protocol section
- Resolve mentions, point reflow, presence, and filesystem questions
- Fix issues from full spec review
- Add annox-core reference library with conformance runner
- Add annox language server (plain-LSP milestone)
- Allow local records to merge into any shared record
- Implement annox/* extension methods in the language server
- Add Neovim plugin for the annox server
- Complete the Neovim plugin workflow
- Watch storage for outside changes and create workspaces from the editor
- Implement live sync: hub, replica, and presence
- Add performance benchmark example
- Speed up resolution, replay, and the server; record performance findings
- Prepare spec version 0.1
- Add suggestion mode: typing makes suggestions
- Add :Annox edit for suggested text and comments
- Find insertions from either side, and close the edit window with Esc
- Relocate points by partial context, and show orphaned annotations
- Bulk accept from the plugin, with one prompt for moved suggestions
- Add a CLI and MCP server for agents, with a skill
- Warn when a workspace's hub has no credentials entry
- Place collaborators' cursors by anchoring, so they stay on the right text
- Bulk reject from the plugin, with one prompt for the batch
- Tint annotated text instead of underlining it
- Bulk resolve comment threads from the plugin
- Hover shows only open threads, like diagnostics
- Add justfile
- Keep events received from the hub in a git-ignored mirror
- Add icon and header assets and add the header to README.md
- Tint suggestion mode and expose it to statuslines
- Add a user-level config file for the author identity
- Add a LazyVim and which-key keymap example to the Neovim README
- Format markdown table
