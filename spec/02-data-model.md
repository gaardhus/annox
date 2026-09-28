# 2. Data model

> Drafted after §3 and §4. The shapes here follow from the anchoring and suggestion semantics.

## 2.1 Annotation

_To define:_ `id`, `kind`, `target`, `author`, timestamps, `body`.

## 2.2 Kinds

- **Highlight:** an anchored range, with an optional label or color.
- **Comment:** an anchored range plus a body.
- **Suggestion:** an anchored range plus a proposed replacement (see §4).

## 2.3 Threads and replies

_To define:_ whether replies are annotations or nested records, and the resolve/reopen lifecycle.

## 2.4 Identity

_To define:_ author identifiers without a mandated auth system, and optional signatures.

## 2.5 Body format

_To define:_ plain text or Markdown, and mentions.

## 2.6 Extensibility

_To define:_ how unknown fields and unknown kinds are handled, and vendor namespaces.

## Open questions

- Are highlights and comments one kind with an optional body, or separate kinds?
- Are records mutable, or append-only events (which is better for merging)?
