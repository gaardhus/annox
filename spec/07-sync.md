# 7. Sync

Sync keeps copies of a workspace's annotations up to date live, without waiting for a git push and pull. Replicas, typically annox servers (§6), connect to a **sync hub** that stores the workspace's events and relays new ones to every connected replica.

Sync needs no merge logic. Events are immutable, and merging two copies is the union of their events (§5). All the hub does is make sure every replica eventually has every event. Ordering and conflicts are still decided by `after` links on each replica (§2.5). Sync and version control can be used together: received events are kept in a git-ignored mirror (§5.12), and an event that arrives both ways is recognized by its id (§5.4).

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## 7.1 Roles

- A **replica** has a copy of a workspace's `.annox/` directory. It pushes the events it has that the hub lacks, and pulls the ones it lacks.
- A **hub** stores every event it accepts for a workspace, assigns each one a sequence number, and relays new events to connected replicas.

Only shared events are synced. Local-only annotations (§5.11) MUST NOT be pushed.

## 7.2 Configuration and authentication

A workspace MAY name its hub in `annox.json`:

```json
{ "format": 1, "sync": { "url": "wss://hub.example.org/w/3f9c…" } }
```

The URL identifies both the hub and the workspace on that hub. It is safe to commit.

Authentication and access control are outside this specification (§1.3), so they are handled by the connection:

- Replicas MUST connect over TLS (`wss:`), except to a hub on the local machine.
- Credentials, such as a bearer token sent in the WebSocket handshake, are configured per user outside the workspace. They MUST NOT be stored in `.annox/`.
- A hub MAY refuse a connection, or allow a replica to read but not write (§7.6).

## 7.3 Transport

A replica opens a WebSocket (RFC 6455) to the hub URL, requesting the subprotocol `annox-sync`. Each WebSocket text message carries exactly one JSON-RPC 2.0 message. LSP's `Content-Length` framing is not used.

## 7.4 Sequence numbers and cursors

The hub gives each event it accepts for a workspace a **sequence number**. Sequence numbers start at 1 and increase strictly, in the order the hub received the events. They exist only to transfer events. They don't order events for replay, and they don't affect conflict detection.

Each hub log also has a **hub id** (an Id, §2.1). If the hub ever loses or rebuilds its log, it MUST choose a new hub id.

A replica's **cursor** is the highest sequence number it has received from a given hub id. Replicas SHOULD store `{ hubId, cursor }` in the cache (§5.9), never in shared files. A replica with no cursor, or with a cursor for a different hub id, starts from 0.

## 7.5 Messages

### 7.5.1 Items

Events are transferred as **items**, which carry the document an event belongs to. That information comes from folders in local storage (§5.5.2), so it isn't part of the event itself:

```json
{ "seq": 1042, "document": "0192f0c4-…", "event": { "…": "an annotation or document event" } }
```

`seq` is present only in messages from the hub.

A replica receiving items writes each event as a file (§5.4) in the sync mirror, `.annox/synced/docs/` (§5.12). A document event goes into the `document/` subfolder, and an annotation event goes directly into the document's mirror folder, named after the document's current path. Documents new to the replica get a folder when their `document` event arrives. An item whose document isn't known yet is kept pending, for example in the cache, until its `document` event arrives.

### 7.5.2 Methods

| Method | Direction | Params | Result |
|---|---|---|---|
| `annoxSync/hello` | replica → hub | `{ format: 1, subscribe: boolean }` | `{ format, hubId, cursor, writable: boolean }` |
| `annoxSync/pull` | replica → hub | `{ hubId, after: integer, limit?: integer }` | `{ items: Item[], cursor: integer, more: boolean }` |
| `annoxSync/push` | replica → hub | `{ items: Item[] }` | `{ results: ({ seq } \| { error })[] }`, one entry per item |
| `annoxSync/didReceive` | hub → replica (notification) | `{ hubId, items: Item[], cursor: integer }` | — |

- **`hello`** MUST be the first request. `format` is the storage format (§5.3). The hub fails with `UnsupportedFormat` if it doesn't support it. The result's `cursor` is the hub's latest sequence number. `writable` says whether this connection may push.
- **`pull`** returns items with `seq > after`, in ascending `seq` order, at most `limit` of them. The hub MAY return fewer. If `more` is true, the replica pulls again from the returned `cursor`. If `hubId` doesn't match, the hub fails with `UnknownHub`, and the replica starts again from 0 with the new hub id.
- **`push`** sends items for events the hub may not have yet. For each item, the hub stores the event with a new `seq`, or, if it already has an event with that id, returns the existing `seq`. The hub then sends the new items to the other subscribed replicas.
- **`didReceive`** is sent to replicas that subscribed in `hello`. It carries items the hub has just accepted from others, in `seq` order. A replica that sees a gap between its cursor and the first `seq` pulls to fill it.

### 7.5.3 Session outline

```text
replica                                   hub
   │ annoxSync/hello {subscribe:true} ──────▶│
   │◀────────── {hubId, cursor: 1057, writable}│
   │ annoxSync/pull {after: 1041} ──────────▶│
   │◀────────── {items: 1042…1057, more:false}│
   │ annoxSync/push {items: [local events]} ─▶│
   │◀────────── {results: [{seq:1058}, …]}    │
   │                                         │
   │◀── annoxSync/didReceive {items: 1059…}  │   (live, as others push)
```

## 7.6 Replica requirements

A replica that syncs MUST:

- pull everything after its cursor before relying on its state being current, and apply `didReceive` items as they arrive;
- eventually push every shared event that the hub doesn't have: events written locally, and events that arrived by other routes such as git. How it tracks what the hub has is up to the implementation, for example a set of acknowledged ids in the cache. Pushing an event the hub already has is harmless;
- push events in an order where every event comes after its document's `document` event and after every event in its `after` list, so other replicas rarely hold items pending;
- write received events as ordinary files (§5.4) in the sync mirror (§5.12), so storage stays the single source of truth and received events never block a version-control pull.

If the connection is read-only (`writable: false`), the replica keeps pulling, and SHOULD tell the user that their changes aren't being shared.

## 7.7 Hub requirements

A hub MUST:

- store every accepted event permanently and unchanged (there is no compaction, [D29](decisions.md#d29-no-compaction-in-v1-2026-09-29));
- assign sequence numbers as in §7.4, and recognize duplicates by event id. If a pushed event has the id of a stored event but different content, the hub keeps the stored one and returns `InvalidItem` for that item;
- deliver items to subscribers and in pulls in `seq` order;
- never reject an event because of what it means, for example because it causes a conflict. Conflicts are surfaced by replicas (§2.5.3). A hub MAY reject items that aren't well-formed events (`InvalidItem`).

A hub doesn't need to replay events, resolve anchors, or read documents. It stores and relays JSON.

## 7.8 Presence

Presence shows who is connected to the hub, which document each person has open, and where their cursor is. It is **ephemeral**: presence isn't an event, it is never written to `.annox/`, and a hub MUST NOT keep it after the connection that sent it closes.

### 7.8.1 Messages

| Method | Direction | Params |
|---|---|---|
| `annoxSync/presence` | replica → hub (notification) | `{ author, document?: Id, range?: { start, end }, version?: string, quote?: { exact, prefix, suffix } }` |
| `annoxSync/didChangePresence` | hub → replica (notification) | `{ peers: Peer[] }` |

- `document` is the document record the user has open (§5.5). It is absent when no annotated document is open.
- `range` is the cursor or selection, in code-point offsets (§3.3) into the sender's normalized buffer. `version` is the version (§3.4) of that buffer.
- `quote` is the quote selector (§3.5) of `range`, computed as in §3.6. A replica that sends `range` SHOULD send `quote` as well.
- A **Peer** is `{ connection, author, document?, range?, version?, quote? }`. `connection` is an opaque id that the hub assigns to each connection, so that two sessions of the same author can be told apart.
- Replicas SHOULD debounce `annoxSync/presence`, and send it when the open document or the cursor changes.
- The hub sends `didChangePresence` to every subscribed replica whenever the set of peers or a peer's presence changes. It always carries the full list of peers other than the recipient's own connection. A connection that closes is removed from the list.

### 7.8.2 Displaying cursors

The offsets in a peer's `range` refer to the peer's buffer, which may differ from the recipient's copy of the document, for example because either side has unsaved edits. If `version` equals the version of the recipient's text, the range is exact.

Otherwise, if the peer sent `quote`, a replica SHOULD resolve the anchor whose `version` is the peer's `version`, whose `position` is `range`, and whose `quote` is the peer's `quote` against its own text (§3.7), and show the cursor at the resulting range. If the anchor is orphaned, the replica SHOULD show only the document, without a cursor. If the peer sent no `quote`, a replica MAY map the range on a best-effort basis, for example by clamping it to the document, and MAY show only the document.

### 7.8.3 Privacy

Presence is sent by default. Clients MUST let users turn it off, and SHOULD show that it's on. A replica with presence turned off MUST NOT send `annoxSync/presence`, but still receives others' presence. Like everything else in annox, `author` is not authenticated (§2.7). A hub MAY replace it with an identity from its own authentication.

## 7.9 Errors

| Code | Name | Meaning |
|---|---|---|
| 2001 | `Unauthorized` | Missing or invalid credentials. The hub MAY instead refuse the WebSocket handshake. |
| 2002 | `ReadOnly` | A push on a connection that isn't writable. |
| 2003 | `UnsupportedFormat` | The hub doesn't support the replica's storage format. |
| 2004 | `UnknownHub` | The `hubId` in the request doesn't match the hub's log. The replica starts again from 0. |
| 2005 | `InvalidItem` | Per item in `push` results: the item is not a well-formed event, or it conflicts with a stored event that has the same id. |

## Open questions

The following are deferred until after v1. None of them affects the event or storage format.

- **Partial sync.** Very large workspaces might want to sync some documents only. Every item already carries its `document`, so a filter could be added to `pull` and `hello`.
- **Hub-to-hub.** Can two hubs federate, or does a workspace always have exactly one hub?
