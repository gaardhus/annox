# 4. Suggestions

## 4.1 Shape

_To define:_ an anchored range plus replacement text. Insertions are empty ranges and deletions are empty replacements.

## 4.2 Base version

_To define:_ each suggestion records the document version it was written against.

## 4.3 Lifecycle

_To define:_ states **open**, **accepted**, **rejected**, and **withdrawn**, and who may perform each transition.

## 4.4 Staleness and conflicts

_To define:_ what happens when the anchored text has changed since the base version. Options: the suggestion is stale, it applies because the quote still matches, or it conflicts.

## 4.5 Applying

_To define:_ how a client applies an accepted suggestion to the document, and what it records afterwards.

## Open questions

- Can a single suggestion span multiple ranges or files, like a `WorkspaceEdit`?
- Do suggestions on the same text interact, e.g. by being mutually exclusive?
