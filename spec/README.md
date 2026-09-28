# annox specification

**Version:** 0.0 (draft, unstable)

| # | Section | Status |
|---|---------|--------|
| 1 | [Overview](01-overview.md) | draft |
| 2 | [Data model](02-data-model.md) | draft |
| 3 | [Anchoring](03-anchoring.md) | draft |
| 4 | [Suggestions](04-suggestions.md) | draft |
| 5 | [Storage](05-storage.md) | draft |
| 6 | [Protocol](06-protocol.md) | draft |

Design decisions and their rationale are recorded in [decisions.md](decisions.md).

Conformance test vectors are in [tests/](tests/). [`anchoring.json`](tests/anchoring.json) covers anchor resolution (§3.7), [`suggestions.json`](tests/suggestions.json) covers suggestion applicability and application (§4), [`replay.json`](tests/replay.json) covers deriving annotation state from events (§2.5), and [`storage.json`](tests/storage.json) covers loading documents from a workspace, including renames and duplicates (§5).

## Status

All sections have a first draft. Each section ends with its open questions. They are the main remaining work before a 0.1 release.

The test vectors were generated with a throwaway reference implementation and cross-checked against the worked examples in §3.9 and §4.5.
