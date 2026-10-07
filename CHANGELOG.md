# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.0](https://github.com/gaardhus/annox/compare/v0.11.0...v0.12.0) - 2026-10-07

### Added

- *(vscode)* Add a sidebar of the active file's annotations as cards

### Fixed

- *(vscode)* List only open annotations, in document order, like nvim
- *(vscode)* Show every suggestion as a diff block, like nvim

### Documentation

- Add an annotated example document in examples/
- *(vscode)* Say how the activity bar icon differs from the main one

## [0.11.0](https://github.com/gaardhus/annox/compare/v0.10.0...v0.11.0) - 2026-10-07

### Added

- Show everything said about some text, closed annotations included

### Documentation

- Add CONTRIBUTING.md

## [0.10.0](https://github.com/gaardhus/annox/compare/v0.9.0...v0.10.0) - 2026-10-07

### Added

- *(nvim)* Keep the quickfix list in document order and up to date
- Add annox report --verbose, breaking counts down by status

### Fixed

- *(nvim)* Save an edited suggestion at its current anchor
- Show a lone ~ in comment bodies instead of striking text through

### Changed

- *(nvim)* Move shared state into annox.store
- *(nvim)* Move shared helpers into annox.util
- *(nvim)* Move the word diff into annox.worddiff
- *(nvim)* Move the highlight groups into annox.highlight
- *(nvim)* Move drawing annotations and presence into annox.render
- *(nvim)* Move threads and history into annox.thread
- *(nvim)* Move annotation commands into annox.actions
- *(nvim)* Move the orphans picker and quickfix list into their own modules
- *(nvim)* Move the edit windows and conflict resolution into annox.edit
- *(nvim)* Move suggestion mode into annox.suggest_mode
- *(nvim)* Drop the imports and exports the split left unused

## [0.9.0](https://github.com/gaardhus/annox/compare/v0.8.2...v0.9.0) - 2026-10-06

### Added

- *(editors)* Add an overlay toggle
- Suggest where orphaned annotations went
- *(editors)* Fix orphans from one picker, and report them as diagnostics

### Performance

- *(lsp)* Cache suggested locations per annotation and text version

## [0.8.2](https://github.com/gaardhus/annox/compare/v0.8.1...v0.8.2) - 2026-10-05

### Fixed

- *(lsp)* Notice storage changes the client's watcher misses

## [0.8.1](https://github.com/gaardhus/annox/compare/v0.8.0...v0.8.1) - 2026-10-05

### Fixed

- *(anchoring)* Find edited quotes and carry comments through accepts

### Documentation

- Add a Neovim demo GIF and a just recipe to record it
- Sharpen the Neovim demo GIF and drop its border

## [0.8.0](https://github.com/gaardhus/annox/compare/v0.7.0...v0.8.0) - 2026-10-04

### Added

- *(nvim)* Mark the words that changed within a suggestion
- Revert accepted suggestions, and show a thread's whole history
- *(site)* Add a how-it-works page with the review workflow
- *(site)* Revert accepted suggestions in the demo
- Add highlighting, a comment with no body

### Fixed

- *(install)* Find the latest release without the rate-limited GitHub API

## [0.7.0](https://github.com/gaardhus/annox/compare/v0.6.0...v0.7.0) - 2026-10-02

### Added

- Modernize the icon
- Add annox commit for committing annotation files

## [0.6.0](https://github.com/gaardhus/annox/compare/v0.5.0...v0.6.0) - 2026-10-01

### Added

- Add a Windows installer and install the skill for other agents ([#7](https://github.com/gaardhus/annox/pull/7))

## [0.5.0](https://github.com/gaardhus/annox/compare/v0.4.0...v0.5.0) - 2026-10-01

### Added

- *(vscode)* Add a VS Code extension ([#5](https://github.com/gaardhus/annox/pull/5))
- *(nvim)* Separate thread replies with a horizontal rule
- *(nvim)* Show the thread while typing a reply
- *(lsp)* Show a suggestion's change as a diff block in hover
- *(nvim)* Show a suggestion's change as a diff block in threads
- *(lsp)* Separate thread replies with a horizontal rule in hover
- *(cli)* Add list filters and a show command
- *(cli)* Add init --local to keep a workspace out of git
- *(cli)* Add a report command summarizing annotations per document
- *(site)* Serve install.sh from the site for a shorter install URL
- *(site)* Polish the landing page

### Fixed

- *(nvim)* Guard thread view against JSON null fields

### Documentation

- *(skill)* Tell agents to quote as little as possible in suggestions
- *(nvim)* Add normal mode suggestion keymap
- Flag the spec as unstable and note nothing depends on it yet

## [0.4.0](https://github.com/gaardhus/annox/compare/v0.3.0...v0.4.0) - 2026-09-30

### Added

- Add a showcase website with a live in-browser demo

### Fixed

- Improve lazy load for nvim editor plugin

### Documentation

- *(site)* Lead the headline with portable annotations

## [0.3.0](https://github.com/gaardhus/annox/compare/v0.2.0...v0.3.0) - 2026-09-29

### Added

- Add `annox --version` and `annox update`

### Changed

- Parse the CLI with clap

## [0.2.0](https://github.com/gaardhus/annox/compare/v0.1.1...v0.2.0) - 2026-09-29

### Added

- Add an install script for release binaries

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
