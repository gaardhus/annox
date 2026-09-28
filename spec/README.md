# annox specification

**Version:** 0.0 (draft, unstable)

| # | Section | Status |
|---|---------|--------|
| 1 | [Overview](01-overview.md) | outline |
| 2 | [Data model](02-data-model.md) | outline |
| 3 | [Anchoring](03-anchoring.md) | draft |
| 4 | [Suggestions](04-suggestions.md) | outline |
| 5 | [Storage](05-storage.md) | outline |
| 6 | [Protocol](06-protocol.md) | outline |

Design decisions and their rationale are recorded in [decisions.md](decisions.md).

Conformance test vectors are in [tests/](tests/). [`anchoring.json`](tests/anchoring.json) covers anchor resolution (§3.7).

## Drafting order

Sections are drafted in dependency order, not reading order:

1. Anchoring and re-anchoring (§3)
2. Suggestions and conflicts (§4)
3. Data model (§2), shaped by 3 and 4
4. Sidecar storage (§5)
5. Protocol (§6)
