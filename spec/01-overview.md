# 1. Overview

## 1.1 Goals

- One annotation format that many editors and tools can read and write.
- Annotations survive edits made by tools that don't know about annox, such as `git pull`, `sed`, or another editor.
- Works with plain files and no server.
- Simple to implement in any language, including editor scripting languages (Lua, Elisp, Vimscript).

## 1.2 Non-goals (v1)

- Rich document formats (see [D1](decisions.md#d1-v1-targets-plain-text-documents-2026-09-28)).
- Real-time co-editing of the document itself. annox annotates documents; it does not sync them.
- Authentication or access control.

## 1.3 Terminology

_To define:_ document, annotation, anchor, selector, thread, suggestion, orphaned, client, server.

## 1.4 Conformance

_To define:_ RFC 2119 keywords and conformance classes (reader, writer, re-anchoring implementation, server).

## 1.5 Prior art

_To write:_ LSP, CriticMarkup, W3C Web Annotation (the text-quote selector), Peritext and Automerge marks.
