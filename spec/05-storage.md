# 5. Storage

Annotations are stored as files in a `.annox/` directory at the root of the workspace. Each document that has annotations gets a stable id and one or more folders. Each event (§2.3) is its own file inside a folder. Merging two copies of the workspace, through git, a file-sync tool, or a plain copy, only ever adds files, so storage never produces merge conflicts. Conflicts between concurrent changes are detected and surfaced by the data model (§2.5.3) instead.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## 5.1 Workspace

The **workspace root** is the directory that contains `.annox/`. To find it for a document, a tool walks up from the document's directory to the nearest ancestor (including that directory itself) containing a `.annox/` directory with an `annox.json` file. If there is none, the document is not in an annox workspace.

Document paths (§3.1) are relative to the workspace root.

## 5.2 Layout

```text
<workspace root>/
  .annox/
    annox.json                          format marker (§5.3)
    .gitignore                          contains "cache/", "local/", and "synced/"
    docs/
      paper.tex~0192f0c4-…/             folder of document 0192f0c4-… (§5.5)
        document/
          0192f0c4-….json               document event: path "draft.tex"
          0192f1a0-….json               move event: path "paper.tex"
        0192f0d1-….json                 annotation events (§5.4)
        0192f0e2-….json
      ch2/
        intro.md~0192f2b7-…/            folder of a document at "ch2/intro.md"
          document/
            0192f2b7-….json
          0192f2c3-….json
    local/                              local-only annotations, never shared (§5.11)
      docs/…                            same layout as docs/ above
    synced/                             events received from a sync hub, never committed (§5.12)
      docs/…                            same layout as docs/ above
    cache/                              optional, never shared (§5.9)
```

## 5.3 Format marker

`.annox/annox.json` contains:

```json
{ "format": 1 }
```

`format` is an integer that is incremented when the storage layout or event format changes incompatibly.

- A tool MUST create `annox.json` when it initializes a workspace.
- A Client MUST NOT write to a workspace whose `format` it doesn't support.
- A Viewer MAY read such a workspace on a best-effort basis, and SHOULD warn the user.
- Readers MUST ignore unknown fields in `annox.json`.
- `sync` optionally names the workspace's sync hub (§7.2).

## 5.4 Event files

Every event, whether it belongs to an annotation (§2.3) or a document (§5.5.1), is stored as a single file:

- The name is `<event id>.json`, where the id is the event's `id` in canonical lowercase form.
- The content is one JSON object (RFC 8259): the event, encoded as UTF-8 without a byte order mark. Writers SHOULD pretty-print it with a trailing newline, so that diffs are readable.
- Event files are immutable. A tool MUST NOT modify an event file after writing it. It MAY move the file only as this section allows: between folders of the same document in the same tree (§5.6), from a merged duplicate to its survivor (§5.8), or from the local area to the shared one when publishing (§5.11). The **trees** are `docs/`, `local/docs/`, and `synced/docs/`.
- Writers MUST write atomically: first write to a temporary file in the same directory whose name doesn't end in `.json` (e.g. `.<event id>.json.tmp`), then rename it into place.

Readers MUST ignore files whose names are not `<UUID>.json`. That includes temporary files, editor backups, and OS metadata files. If two files hold events with the same `id`, they are the same event. Readers use one of them, preferring a copy outside the sync mirror (§5.12), and MAY report a mismatch if their contents differ. A file whose content isn't valid JSON, or whose `id` doesn't match its filename, MUST be ignored and SHOULD be reported.

## 5.5 Documents

A **document record** gives a document a stable identity that survives renames. It is created when the document gets its first annotation. Which document an annotation belongs to is decided by the folder its events are stored in, not by the `path` in its anchors (§3.1).

### 5.5.1 Document events

A document record is an event log, like an annotation. Its events use the envelope of §2.3, with `document` in place of `annotation`:

| Type | Fields | Meaning |
|---|---|---|
| `document` | `path` | Creates the document record at `path`. Its `id` is the document id, and `document` equals `id`. `after` is empty. |
| `move` | `path` | The document now lives at `path`. |
| `merged` | `into` (Id) | This record is a duplicate of document `into` (§5.8). If both records are in the same area, `into` MUST be smaller than this document's id. A local record MAY be merged into a shared record with any id. A shared record MUST NOT be merged into a local one. |

Document events are replayed by the rules of §2.5, with these fields:

| Event | Writes |
|---|---|
| `document` | `path` |
| `move` | `path` |
| `merged` | `mergedInto` |

A conflicted `path` (two concurrent `move` heads) is surfaced and resolved like any other conflict (§2.5.3, §2.5.4). The resolving event is a `move` whose `after` lists both heads. Until then, the provisional value is used.

A record's **area** is the area its `document/` events are stored in (§5.11). A `merged` event is invalid, and changes nothing (§2.4), if its `into` isn't smaller than the document's id, unless the record is local and `into` is a shared record. A reader also ignores a `mergedInto` value that names a document it doesn't know, for example a local record on someone else's machine. The record then stays at its own path, so its annotations are never hidden behind an invisible redirect.

A document whose `mergedInto` is set is **merged**. Its `path` is ignored. Its **canonical document** is found by following `mergedInto` until reaching a document that isn't merged. This can't loop: shared records only merge into smaller shared records, and local records merge into smaller local records or into shared ones. A document that isn't merged is its own canonical document.

The **current path** of a document is its `path` value. A document that isn't merged is **at** its current path.

### 5.5.2 Document folders

A **document folder** is a directory under `.annox/docs/` or `.annox/synced/docs/` (together, the **shared area**; the second is the sync mirror, §5.12) or under `.annox/local/docs/` (the **local area**, §5.11) whose name ends in `~<document id>`. Its location mirrors a path: the shared folder for document *d* at path `ch2/intro.md` is `.annox/docs/ch2/intro.md~<d>/`.

- The folder's `document/` subdirectory holds document events (§5.5.1) for *d*.
- The event files directly inside the folder are annotation events.
- The part of the name before `~` is cosmetic. It records the path when the folder was created. The id suffix alone decides which document a folder belongs to.
- On case-insensitive filesystems, folders of different documents never collide, because their id suffixes differ. A case-only rename (`Paper.tex` → `paper.tex`) is an ordinary `move`.

A document can have several folders, for example after a rename (§5.6) or when a branch without the rename is merged. Readers MUST combine all folders with the same document id.

## 5.6 Renames

A Client that renames a document, or detects a rename (for example from git), MUST, for every document at the old path:

1. write a `move` event with the new path to the document's log, and then
2. SHOULD move the document's folders to `<new path>~<id>` in the same tree, merging their contents into any existing folder with that name. Files in the sync mirror stay in the mirror, so that no one commits an event that someone else wrote (§5.12).

Step 2 only keeps folder names readable. Correctness depends on the `move` event alone.

An event written by a branch that didn't have the rename lands in a folder with the old name. It still carries the same id suffix, so it belongs to the same document. A Client MAY tidy it at any time by moving its files into the folder with the current name.

**Path reuse.** After a rename, no document is at the old path any more. A new file created at that path gets a new document record when it is first annotated. Annotations of the renamed document are never attributed to it.

## 5.7 Reading and writing

### 5.7.1 Loading a document

To load the annotations of the file at path *P*:

1. Find every document folder under `.annox/docs/`, `.annox/local/docs/` (§5.11), and `.annox/synced/docs/` (§5.12), and group the folders by document id.
2. For each document id, read the `document/` events from all of its folders, and derive `path` and `mergedInto` (§5.5.1).
3. Let *R* be the set of documents at *P*. Let *S* be every document whose canonical document is in *R*. This includes merged duplicates.
4. Read the annotation event files in every folder of every document in *S*, and derive each annotation's state (§2.5).

If *R* has more than one document, the file has **duplicate records** (§5.8). Readers MUST load all of them as above.

### 5.7.2 Writing events

- **New annotation on *P*.** If *R* is empty, the Client first creates a document record: a `document` event with `path` *P*, written to `.annox/docs/<P>~<id>/document/`. If *R* has one document, it is used. If *R* has several, the one with the smallest id is used, and the Client SHOULD merge the others (§5.8).
- **Later events of an annotation** are written to a folder of the canonical document of the folder holding the annotation's `create` event, in the same area as the `create` event (§5.11). Writers SHOULD use the folder named after the document's current path, creating it if needed.
- Writers MUST NOT store events of one annotation under different canonical documents. Moving an annotation to another document is not supported in v1.
- Writers MUST NOT write events they create to the sync mirror (§5.12), even when the annotation's other events are only there. A reply to, or a change of, an annotation received from a hub goes to `docs/`.

### 5.7.3 Missing documents

If no file exists at a document's current path, its annotations are kept. Viewers and Clients MUST show its open annotations as orphaned, with the document missing. A Client MAY offer to re-attach them, by writing a `move` to the file's new path, or to delete them, by writing `delete` events.

## 5.8 Duplicate records

Duplicate records happen when two branches each start annotating the same new file and both create a document record. Readers load them together (§5.7.1). A Client that finds duplicates at a path SHOULD merge them:

1. Let *s* be the duplicate with the smallest id among the **shared** records, or among all of them if every record is local. A shared record is never merged into a local one, because others could never see the survivor.
2. For every other duplicate *d*: move *d*'s annotation event files into *s*'s folders, keeping each file in its tree. Files in `docs/` go to *s*'s folder there, local files to *s*'s local folder, and mirror files to *s*'s mirror folder. Then write a `merged` event with `into: s` to *d*'s log.

*d*'s folder, now holding only `document/`, stays in place. If an event from a branch without the merge later lands in *d*'s folder, it is still loaded through the redirect, and it can be tidied into *s*'s folder. Choosing the smallest id means two Clients merging concurrently choose the same survivor.

## 5.9 Cache

`.annox/cache/` MAY hold anything a tool finds useful, such as derived state or a path-to-document index. Its contents MUST be derivable from the rest of `.annox/`, MUST NOT be shared, and MAY be deleted at any time. `.annox/.gitignore` MUST list `cache/`, `local/`, and `synced/`. A tool that finds a line missing, for example in a workspace created by an older tool, SHOULD add it.

## 5.10 Version control

`.annox/` is meant to be committed alongside the documents, except `cache/`, `local/`, and `synced/`. Nothing in it needs special merge configuration. Annotations can also be shared live through a sync hub (§7). Sync and version control can be used together: events received from the hub are kept out of `docs/` (§5.12), so each user commits the events they wrote.

## 5.11 Local-only annotations

`.annox/local/` holds annotations that are never shared: private highlights and notes, and draft comments not yet published, such as a review in progress. It is excluded from version control (§5.9) and from sync (§7.1). `local/docs/` has the same layout and rules as `docs/`, and readers load both together (§5.7.1).

- **One area per annotation.** All events of an annotation MUST be in the same area, either shared or local. Otherwise a shared event could list a local event in `after`, and other copies would see it as dangling (§2.5.1).
- **Replies.** A local reply to a shared root is allowed. A shared reply MUST NOT have a local parent, because others couldn't see the thread.
- **Document records.** A local annotation on a document that has a shared document record uses that record's id. Its folder is `local/docs/<path>~<id>/`, with no `document/` subfolder. If there is no shared record, the Client creates a local one under `local/docs/`.
- **Publishing** an annotation makes it shared. The Client moves all of the annotation's event files from its local folder to the shared folder of the same document. It SHOULD publish the annotation's local replies at the same time. If the document record is local, the Client publishes that too, by moving its `document/` events. If a shared record for the same path appeared in the meantime, the two are duplicates and are merged as described in §5.8, with the shared record as the survivor.
- **Display.** Loading always includes the local area (§5.7.1). Clients MUST visibly distinguish local annotations from shared ones, and MUST offer a way to publish them. Viewers MAY hide local annotations from what they display.

A rename (§5.6) writes `move` events to every document record at the old path, in whichever area each record lives.

## 5.12 Sync mirror

`.annox/synced/` holds the events a replica received from a sync hub (§7). `synced/docs/` has the same layout and rules as `docs/`, and belongs to the shared area: readers load it together with `docs/` (§5.7.1), and its events are shared events.

The mirror exists because of how version control treats untracked files. Suppose a replica wrote a received event into `docs/`. Until the author's commit arrives, that file is untracked in the recipient's copy, and version control refuses to overwrite untracked files, even identical ones: git stops the pull with "untracked working tree files would be overwritten by merge". Ignored files don't block a pull. Keeping received events in an ignored mirror means that everything untracked in `docs/` was written by the local user, and a pull never collides with it.

- A replica MUST write the events it receives from a hub to the mirror, and nowhere else (§7.5.1). It writes them even when `docs/` already has a copy, so the mirror keeps every event the hub sent.
- Events a tool creates go to `docs/` or `local/docs/`, never to the mirror (§5.7.2).
- Tools MUST NOT delete mirror files once `docs/` has a copy. The `docs/` copy can disappear again, for example when switching to a branch without the commit that added it, or when that commit is reverted. The replica's cursor (§7.4) is already past the event, so the hub wouldn't send it again.
- Renames and duplicate merges keep mirror files in the mirror (§5.6, §5.8).
- A tool MAY offer to copy an event from the mirror into `docs/`, for a user who wants to commit an event that its author never committed. The copy has the same id, so readers see one event (§5.4).

An event is in version control only once someone commits it. An event that its author never commits stays on the hub and in the mirrors of those who received it. The hub also doesn't know about branches: an event written on one branch reaches synced replicas whatever branch they have checked out.

Test vectors are in [`tests/storage.json`](tests/storage.json).

## Open questions

- **Scale** is validated ([D40](decisions.md#d40-performance-is-validated-implementations-should-index-cache-and-debounce-2026-09-29)). About 20,000 event files (5,000 annotations) read in about 170 ms and derive in about 150 ms, and a tool that caches between changes pays this only when storage changes. Compaction isn't needed at that scale.
