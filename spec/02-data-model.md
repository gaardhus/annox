# 2. Data model

annox stores **events**, not annotations. Every change to an annotation (creating it, editing its body, changing its status, rewriting its anchor) is a new, immutable event. The current state of an annotation is **derived** by replaying its events. Events record which earlier events their writer had seen. That lets any reader tell when two changes were made concurrently, for example on two git branches, and show the conflict instead of silently picking a winner.

How events are laid out in files is defined in §5. Documents have event logs of their own, which follow the same envelope and replay rules (§5.5.1). The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## 2.1 Common types

| Type | Definition |
|---|---|
| **Id** | A UUIDv7 (RFC 9562) in lowercase, hyphenated canonical form. Ids are compared as strings. For UUIDv7, string order approximates creation-time order. |
| **Timestamp** | An RFC 3339 date-time in UTC, e.g. `2026-09-28T14:33:00Z`. It is informational only and is never used for ordering or conflict detection. |
| **Author** | An object `{ "id": string, "name"?: string }`. See §2.7. |
| **Anchor** | The `target` object defined in §3.5. |
| **Markdown** | A CommonMark string. See §2.8. |

## 2.2 Annotations

There are three kinds of annotation:

| Kind | Anchored | Purpose |
|---|---|---|
| `comment` | yes | A highlight, a note, or the start of a discussion. Its body is optional, so a comment with no body is a highlight. |
| `suggestion` | yes | A proposed edit (§4). It may have a body explaining it. |
| `reply` | no | A message in the thread of a comment or suggestion. |

A comment or suggestion is the **root** of a thread. Threads are flat: a reply's `parent` MUST be a root, never another reply. Replies are shown in causal order (§2.5.5).

An annotation's **id** is the id of the event that created it.

## 2.3 Events

Every event has these fields:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | Id | yes | Unique id of this event. |
| `annotation` | Id | yes | The annotation this event belongs to. For a `create` event it equals `id`. |
| `after` | Id[] | yes | Events of the same annotation that the writer had seen (§2.5.1). It is empty for `create` and non-empty for every other event. |
| `type` | string | yes | Event type (§2.4). |
| `author` | Author | yes | Who made the change. |
| `time` | Timestamp | yes | When the change was made. Informational only. |

The remaining fields depend on `type`. Events are immutable. Once written, an event MUST NOT be modified or removed, except by a compaction mechanism defined later (see open questions).

## 2.4 Event types

### `create`

Creates an annotation.

| Field | comment | suggestion | reply |
|---|---|---|---|
| `kind` | required | required | required |
| `target` (Anchor) | required | required | MUST NOT be present |
| `parent` (Id) | MUST NOT be present | MUST NOT be present | required: the id of a root |
| `body` (Markdown) | optional | optional | required, non-empty |
| `label` (string) | optional | optional | MUST NOT be present |
| `edit` (`{ "replacement": string }`) | MUST NOT be present | required (§4.1) | MUST NOT be present |

A newly created comment or suggestion has status `open`. Replies have no status.

### `edit`

Changes the body or label of an annotation. At least one of the following must be present:

- `body`: a Markdown string, or `null` to remove the body. A reply's body cannot be removed.
- `label`: a string, or `null` to remove the label. Not allowed on replies.

A field that isn't present is not changed.

### `status`

Changes the status of a root.

- `status`: for a comment, `open` or `resolved`. For a suggestion, `open`, `accepted`, `rejected`, or `withdrawn`.
- `appliedVersion`: the document version after applying (§4.3). Required when `status` is `accepted`, and not allowed otherwise.

The transitions in §4.4 are guidance for user interfaces. When deriving state, readers accept any status valid for the kind. Concurrent histories can produce sequences that no single client would.

### `reanchor`

Replaces the anchor with a fresh one (§3.8). Only for comments and suggestions.

- `target`: the new anchor. Its `path` SHOULD be the document's current path (§5.5).

`reanchor` events are written by clients as a side effect of resolution, not as a deliberate user decision. They are treated specially in conflict detection (§2.5.3).

### `retarget`

Re-targets a stale suggestion (§4.2.1): the anchor and the replacement change together, as a deliberate user action. Only for suggestions.

- `target`: the new anchor. Its `path` SHOULD be the document's current path (§5.5).
- `edit`: `{ "replacement": string }`.

### `delete` and `restore`

`delete` marks an annotation as deleted. `restore` undoes that. Neither has extra fields. Deleted annotations are hidden by clients, but their events are kept.

### Invalid and unknown events

An event that breaks the rules above, such as a `retarget` on a comment or a `status` on a reply, is **invalid**. An event whose `type` the reader doesn't recognize is **unknown**. Both are kept and still count in the causal graph (§2.5.1), but they change no fields. Readers SHOULD NOT report unknown events as errors.

## 2.5 Deriving state

### 2.5.1 The causal graph

The events of one annotation form a directed acyclic graph through their `after` links. Event *X* is an **ancestor** of event *Y* if *X* can be reached from *Y* by following `after` links. Two events are **concurrent** if neither is an ancestor of the other.

When writing an event, a client MUST set `after` to the annotation's **heads**: the events it knows of that no other known event of the annotation lists in its `after`. After a clean merge of two branches, a new event lists the heads from both, and that is how it records having seen both histories.

An event is **dangling** if any id in its `after` is not a known event of the annotation, or if it has a dangling ancestor. Dangling events are ignored until the missing events are available. An event whose `annotation` doesn't match a known `create` event is also ignored.

### 2.5.2 Fields

The state of a comment or suggestion is made up of **fields**. Each event type writes certain fields:

| Event | Writes |
|---|---|
| `create` | `body`, `label`, `status`, `target`, `replacement`, `deleted` (= false) |
| `edit` | `body` if present, and `label` if present |
| `status` | `status` (together with `appliedVersion`) |
| `reanchor` | `target` |
| `retarget` | `target`, `replacement` |
| `delete` | `deleted` (= true) |
| `restore` | `deleted` (= false) |

Replies have only `body` and `deleted`.

The **heads of a field** are the valid events writing that field that are not ancestors of any other valid event writing that field. If a field has one head, its value is the value written by that head. If it has several heads, those writes were concurrent and the field is **conflicted**.

### 2.5.3 Conflicts

A conflicted field is presented as a conflict: readers report every head. Viewers MUST visibly mark conflicted annotations, and Clients MUST offer a way to resolve them (§1.5). So that every client displays the same thing until the conflict is resolved, the **provisional value** of a conflicted field is the one written by the head with the greatest id.

There are two special rules:

1. **Anchor rewrites never conflict.** If every head of `target` except at most one is a `reanchor` event, the field is not conflicted. Its value comes from the one non-`reanchor` head if there is one (a deliberate `retarget` wins over a concurrent automatic refresh). Otherwise it comes from the `reanchor` head with the greatest id. If two or more heads are `create` or `retarget` events, the field is conflicted as usual.
2. **Delete conflicts with concurrent changes.** If `deleted` has a single head that is a `delete` event, and some valid event that isn't a `reanchor` is not an ancestor of that head, the annotation has a **delete conflict**. Someone changed it without having seen the deletion, or changed it after it was deleted. The `deleted` field is then reported as conflicted, with the `delete` head and those events as the competing entries.

### 2.5.4 Resolving conflicts

A user resolves a conflict by choosing a value. The client writes an event for the conflicted field, with `after` containing all the heads. That event then descends from every competing write and becomes the single head.

- For `body` or `label`: an `edit` with the chosen value. The value may merge the competing texts.
- For `status`: a `status` event with the chosen status. If the competing heads include `accepted` and the user chooses anything else, the document already contains the suggestion's edit. The client SHOULD offer to revert it (§4.3.3).
- For `target` and `replacement`: a `retarget`.
- For a delete conflict: another `delete` to keep the annotation deleted, or a `restore` to keep it.

### 2.5.5 Order

When events or replies have to be listed, they are ordered topologically along `after` links, with ties broken by ascending id. This order is used only for display, not for deciding values.

### 2.5.6 Derived annotation

The derived state of an annotation has this shape. It is used by the test vectors and the protocol (§6):

```json
{
  "id": "0192f0c4-…",
  "kind": "comment",
  "author": { "id": "mailto:ada@example.org", "name": "Ada Lovelace" },
  "created": "2026-09-28T14:33:00Z",
  "target": { "…": "§3.5" },
  "body": "Is this bound tight?",
  "label": null,
  "status": "open",
  "deleted": false,
  "conflicts": {}
}
```

- `author` and `created` come from the `create` event.
- For suggestions, `replacement` appears as `edit.replacement`, and `appliedVersion` appears when `status` is `accepted`. For replies, `parent` is present and `target`, `label`, and `status` are omitted.
- `conflicts` maps each conflicted field name to the ids of its competing events, sorted by ascending id. The field itself holds the provisional value.

A reply whose parent is deleted, missing, or not a root SHOULD still be shown, for example under a placeholder thread.

## 2.6 Example

Ada comments. Then, on two branches, Ada edits the body while Bob resolves the thread and also edits the body:

```
e1 create  (Ada)  body "Is this tight?"          after []
e2 edit    (Ada)  body "Is this bound tight?"    after [e1]
e3 status  (Bob)  status resolved                after [e1]
e4 edit    (Bob)  body "Tight — see Lemma 4."    after [e3]
```

- `status` has one head, e3, so the thread is `resolved`.
- `body` has heads e2 and e4, which are concurrent, so it is conflicted. Its provisional value comes from whichever of e2 and e4 has the greater id.

Ada resolves the conflict:

```
e5 edit (Ada) body "Is this bound tight? — Tight, see Lemma 4." after [e2, e4]
```

Now `body` has the single head e5.

## 2.7 Identity

`author.id` is a stable string that identifies a person across events and tools. It SHOULD be a URI, such as `mailto:ada@example.org` or `https://github.com/ada`. `name` is a display name. Author ids are compared as exact strings.

If the user hasn't configured an identity for annox and the document is in a git repository, clients SHOULD default `author.id` to `mailto:` followed by git's `user.email`, and `name` to git's `user.name`.

annox doesn't authenticate authors. Anyone who can write the files can write events under any id. Signed events may be added later as an extension.

## 2.8 Body format

`body` is CommonMark. Clients that can't render Markdown SHOULD show the raw text, which is readable as is. Clients MUST NOT execute scripts found in a body, and SHOULD sanitize or strip raw HTML before rendering it.

## 2.9 Extensibility

- Readers MUST ignore fields they don't recognize.
- Fields added by tools that aren't part of this specification MUST be placed in an `ext` object, keyed by a reverse-domain name, e.g. `"ext": { "org.example.review": { "priority": 2 } }`.
- New event types and kinds are added only by versions of this specification. Readers treat unrecognized ones as described in §2.4.

## Open questions

- **Compaction.** Logs only grow. Should a snapshot event be able to replace an annotation's history, and when is that safe if others haven't merged yet?
- **Mentions.** Is `@person` in a body just Markdown text, or a structured reference to an author id?
- **Privacy.** A deleted comment's text stays in the log, and in git history. Is that acceptable, or does `delete` need a way to purge content?
