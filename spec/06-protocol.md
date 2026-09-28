# 6. Protocol

> Drafted last and optional. Every conforming tool must support the file format (§5). The protocol is an optional layer on top.

## 6.1 Transport

_To define:_ JSON-RPC 2.0 over stdio or sockets, following LSP conventions.

## 6.2 Lifecycle

_To define:_ `initialize` and capability negotiation, including position encoding.

## 6.3 Requests and notifications

Sketch:

- `annotations/list`
- `annotations/create`, `annotations/update`, `annotations/delete`
- `annotations/didChange` (server → client push)
- `document/didChange` (client → server, so the server can re-anchor)
- `suggestion/accept`, `suggestion/reject`
- `thread/resolve`, `thread/reopen`

## 6.4 Relationship to LSP

_To define:_ whether this is a standalone protocol, an LSP extension, or both.
