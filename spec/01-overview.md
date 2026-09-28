# 1. Overview

annox is an open standard for annotating plain-text documents with highlights, comments, discussion threads, and suggested edits. Any editor or tool can support it. Like LSP, it separates the *what* from the *where*. A portable file format means any tool can read and write annotations directly. An optional protocol (§6) lets editors talk to an annotation server instead of each editor integrating with each backend.

## 1.1 How it fits together

```
 ┌──────────────────────────┐        ┌──────────────────────────┐
 │ .annox/  (§5)            │        │ paper.tex                │
 │ event files, committed   │        │ the document, untouched  │
 │ to git like source       │        │ by annox except when a   │
 └────────────┬─────────────┘        │ suggestion is accepted   │
              │ read                 └────────────┬─────────────┘
              ▼                                   │ read
 ┌──────────────────────────┐                     │
 │ events (§2.3)            │                     │
 │ immutable, causally      │                     │
 │ linked by `after`        │                     │
 └────────────┬─────────────┘                     │
              │ replay (§2.5)                     │
              ▼                                   ▼
 ┌──────────────────────────┐        ┌──────────────────────────┐
 │ annotation state         │ anchor │ resolve (§3.7)           │
 │ comment / suggestion /   ├───────▶│ exact · relocated ·      │
 │ reply, conflicts flagged │        │ orphaned                 │
 └──────────────────────────┘        └────────────┬─────────────┘
                                                  ▼
                              ┌─────────────────────────────────────┐
                              │ editor: render highlights, threads, │
                              │ suggestions; user actions become    │
                              │ new events; accepting a suggestion  │
                              │ edits the document (§4.3)           │
                              └─────────────────────────────────────┘
```

1. **Storage (§5).** Annotations live in sidecar files next to the documents. They're usually committed to version control and need no server.
2. **Events (§2).** Nothing is edited in place. Every change, from creating a comment to resolving a thread or accepting a suggestion, is appended as an immutable event. Each event records which earlier events its writer had seen. That makes merging two branches trivial, and it lets concurrent changes be detected and shown to the user instead of silently lost.
3. **State (§2.5).** Replaying an annotation's events gives its current state, including any unresolved conflicts.
4. **Anchoring (§3).** Each comment and suggestion is anchored to a range of text by offsets, quoted text, and context. When the document has changed, including changes made by tools that know nothing about annox, a deterministic algorithm finds the range again or reports the annotation as orphaned.
5. **Suggestions (§4).** A suggestion proposes replacing its anchored text. It can be applied as long as that exact text can still be found. Accepting it edits the document.

## 1.2 Goals

- One annotation format that many editors and tools can read and write.
- Annotations survive edits made by tools that don't know about annox, such as `git pull`, `sed`, or another editor.
- Works with plain files and no server.
- Merging concurrent work never loses a human decision silently.
- Simple to implement in any language, including editor scripting languages (Lua, Elisp, Vimscript).

## 1.3 Non-goals (v1)

- Rich document formats (see [D1](decisions.md#d1-v1-targets-plain-text-documents-2026-09-28)).
- Real-time co-editing of the document itself. annox annotates documents; it does not sync them.
- Authentication or access control.

## 1.4 Terminology

| Term | Meaning |
|---|---|
| **Document** | A plain-text file being annotated, identified by its path relative to the workspace root (§3.1). |
| **Workspace** | A directory tree whose root holds the annox storage (§5). All document paths are relative to it. |
| **Normalized text** | A document's content after decoding, BOM removal, and line-ending normalization (§3.2). All offsets and hashes are computed over it. |
| **Version** | A content hash of normalized text (§3.4). |
| **Annotation** | A comment, suggestion, or reply (§2.2). Its state is derived from events. |
| **Root** | A comment or suggestion, which starts a thread. |
| **Thread** | A root and its replies. |
| **Highlight** | A comment with no body. |
| **Event** | An immutable record of one change to one annotation (§2.3). |
| **Head** | An event that no other known event lists in its `after`. The heads of a field are defined in §2.5.2. |
| **Conflict** | A field with more than one concurrent head (§2.5.3). |
| **Anchor** | The `target` of a root: path, version, and selectors (§3.5). |
| **Selector** | One way of describing the anchored range. v1 defines `position` and `quote`. |
| **Resolution** | Locating an anchor in the current document. The result is `exact`, `relocated`, or `orphaned` (§3.7). |
| **Orphaned** | An annotation whose anchor can't be located. It is kept and shown, never deleted automatically. |
| **Applicable / stale** | Whether a suggestion can currently be applied (§4.2). |
| **Viewer, Client, Server** | Conformance classes (§1.5). |

## 1.5 Conformance

The key words MUST, MUST NOT, REQUIRED, SHOULD, SHOULD NOT, RECOMMENDED, and MAY in this specification are to be interpreted as described in RFC 2119 and RFC 8174 when, and only when, they appear in all capitals.

An implementation claims conformance to one of three classes. Each class includes everything in the classes above it.

### Viewer

A read-only tool, such as a web preview, an export, or a CI check. A Viewer MUST:

- read storage (§5) and derive annotation state by replay (§2.5);
- resolve anchors (§3.7) and determine suggestion applicability (§4.2);
- visibly mark every conflicted annotation (§2.5.3) and every orphaned open annotation (§3.7.3);
- pass the test vectors in [`tests/replay.json`](tests/replay.json), [`tests/anchoring.json`](tests/anchoring.json), and the applicability results in [`tests/suggestions.json`](tests/suggestions.json).

A Viewer MUST NOT write events or modify documents.

### Client

A tool that people use to annotate, typically an editor plugin. A Client MUST meet the Viewer requirements, and MUST also:

- write valid events (§2.3, §2.4), with `after` set to the current heads (§2.5.1);
- create anchors (§3.6), and rewrite them only as allowed (§3.8, §4.2.1);
- support all annotation kinds: create and reply to comments; create, accept, reject, withdraw, and re-target suggestions, applying accepted ones to the document as specified (§4.3);
- offer the user a way to resolve every kind of conflict (§2.5.4), including offering to revert a suggestion's edit (§4.3.3);
- pass all test vectors, including the application results in [`tests/suggestions.json`](tests/suggestions.json).

Partial Clients, such as one that supports comments but not suggestions, are not conforming. A tool that can't meet the Client requirements can still conform as a Viewer.

A Client MAY delegate the logic to an annox server instead of implementing it itself. An editor plugin that exposes every Client action through the protocol conforms as a Client when paired with a conforming Server (§6.7).

### Server

An annotation server that speaks the protocol (§6). A Server MUST implement Client semantics for every change it makes on behalf of clients, and MUST implement §6. The details are defined in §6.

## 1.6 Prior art

- **Language Server Protocol.** The model for splitting a protocol from editor integrations, and the source of the lesson about position encoding. LSP's UTF-16 default later had to be negotiated around in 3.17. annox fixes code points in its files instead (§3.3).
- **W3C Web Annotation Data Model (2017).** annox borrows its text-quote selector, the idea of anchoring by exact text plus prefix and suffix. annox is not a profile of the W3C model and does not use JSON-LD ([D3](decisions.md#d3-independent-of-the-w3c-web-annotation-model-2026-09-28)).
- **CriticMarkup.** Inline syntax for suggestions and comments in plain text. annox keeps annotations out of the document so that compilers, linters, and diffs are unaffected.
- **Hypothesis.** Showed that quote-based re-anchoring works at scale on documents that change.
- **Peritext and Automerge (Ink & Switch).** Research on how annotations should behave under concurrent editing. The event model in §2 takes the same "record causality and surface conflicts" approach, applied to annotation metadata rather than document text.
- **Google Docs, Overleaf, GitHub pull request reviews.** The user-facing behaviour annox aims to make portable: comment threads, resolving, and accept/reject suggestions.
