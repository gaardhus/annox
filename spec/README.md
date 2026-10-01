# annox specification

**Version:** 0.1 (first draft, 2026-09-29). Expect incompatible changes before 1.0.

| # | Section | Status |
|---|---------|--------|
| 1 | [Overview](01-overview.md) | draft |
| 2 | [Data model](02-data-model.md) | draft |
| 3 | [Anchoring](03-anchoring.md) | draft |
| 4 | [Suggestions](04-suggestions.md) | draft |
| 5 | [Storage](05-storage.md) | draft |
| 6 | [Protocol](06-protocol.md) | draft |
| 7 | [Sync](07-sync.md) | draft |

Design decisions and their rationale are recorded in [decisions.md](decisions.md), and changes between versions in [CHANGELOG.md](CHANGELOG.md).

Conformance test vectors are in [tests/](tests/). [`anchoring.json`](tests/anchoring.json) covers anchor resolution (§3.7), [`suggestions.json`](tests/suggestions.json) covers suggestion applicability and application (§4), [`replay.json`](tests/replay.json) covers deriving annotation state from events (§2.5), and [`storage.json`](tests/storage.json) covers loading documents from a workspace, including renames, duplicates, and local-only annotations (§5).

## Status

Version 0.1 is a complete first draft: every section is written, and a reference implementation covers all of it. See the [changelog](CHANGELOG.md) for what 0.1 specifies and what's deferred.

Nothing depends on the format yet: the reference implementation is its only consumer, and it's just starting to be used in practice. Any rule or decision, including the ones [decisions.md](decisions.md) scopes to "v1" (such as no compaction), can still change, incompatibly if the better design needs it.

The test vectors were generated with a throwaway reference implementation. They're checked against the worked examples in §3.9 and §4.5, and pass in the Rust reference implementation (`crates/annox-core`).
