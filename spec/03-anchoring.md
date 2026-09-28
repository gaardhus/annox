# 3. Anchoring

An **anchor** ties an annotation to a range of text in a document. Anchors must keep working after the document is edited, including edits made by tools that don't know about annox. This section defines what an anchor stores, how offsets are counted, and a deterministic algorithm for finding the anchored range again in the current document.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## 3.1 Document identity

An anchor identifies its document by `path`: a path relative to the workspace root (§5), using `/` as the separator. A `path` MUST NOT be absolute and MUST NOT contain `.` or `..` segments. Paths are compared byte-for-byte after UTF-8 encoding, so they are case-sensitive.

What happens to annotations when a document is renamed or deleted is defined in §5.

## 3.2 Normalized text

All offsets, quotes, and hashes are computed over the document's **normalized text**:

1. Decode the file to a sequence of Unicode code points. How the encoding is detected is up to the client. Documents SHOULD be UTF-8.
2. Remove a leading U+FEFF (byte order mark), if there is one.
3. Replace each CRLF (U+000D U+000A) with LF (U+000A), then replace each remaining lone CR (U+000D) with LF.

No other normalization is applied. In particular, there is no Unicode normalization (NFC/NFD), no case folding, and no whitespace collapsing.

The rest of this section uses *text* to mean normalized text and *D* to mean the normalized text of the current document.

## 3.3 Offsets

An **offset** is a count of Unicode code points from the start of the text, beginning at 0. A **range** is a pair `start ≤ end` of offsets that covers the code points at indices `start` through `end − 1`. A range where `start == end` is a **point range**. It marks the position between two code points.

Offsets in stored annotations always use this unit. A protocol session (§6) MAY negotiate a different unit on the wire. Converting between units is the job of the protocol layer.

> **Rationale.** Code points don't depend on how the file is encoded on disk. Normalizing line endings means a CRLF↔LF conversion, which git often does at checkout, doesn't shift any offsets.

## 3.4 Document version

The **version** of a text is the string `sha256:` followed by the lowercase hexadecimal SHA-256 digest of the text's UTF-8 encoding.

Each anchor records the version of the document at the time the anchor was created or last rewritten (§3.8).

## 3.5 Anchor structure

```json
"target": {
  "path": "paper.tex",
  "version": "sha256:…",
  "selectors": {
    "position": { "start": 32, "end": 45 },
    "quote": {
      "exact": "we prove that",
      "prefix": "\\section{Results}\nIn Section 3, ",
      "suffix": " the bound is tight for all $n$."
    }
  }
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `path` | string | yes | Document path (§3.1). |
| `version` | string | yes | Version of the text the selectors were computed from (§3.4). |
| `selectors.position.start` | integer ≥ 0 | yes | Start offset. |
| `selectors.position.end` | integer ≥ `start` | yes | End offset. |
| `selectors.quote.exact` | string | yes | The anchored text. It is empty for a point range. |
| `selectors.quote.prefix` | string | yes | Text immediately before the range. It may be empty. |
| `selectors.quote.suffix` | string | yes | Text immediately after the range. It may be empty. |

`selectors` is an object keyed by selector type, so that later versions can add types, such as structural selectors for rich formats (see [D1](decisions.md#d1-v1-targets-plain-text-documents-2026-09-28)). Readers MUST ignore selector types they don't recognize. An anchor MUST include both the `position` and `quote` selectors.

In v1, an anchor covers exactly one range. Overlapping and nested anchors are allowed, and each is resolved independently.

## 3.6 Creating an anchor

A writer that creates an anchor for the range `[s, e)` in text *T* MUST set:

- `version` to the version of *T*;
- `position` to `{ "start": s, "end": e }`;
- `quote.exact` to `T[s:e]`;
- `quote.prefix` to the up to *N* code points of *T* immediately before `s`;
- `quote.suffix` to the up to *N* code points of *T* immediately after `e`.

*N* SHOULD be 32. Near the start or end of the document, the prefix or suffix is shorter. A writer MAY use a larger *N* so that `prefix + exact + suffix` occurs only once in *T*. Readers MUST accept any length.

## 3.7 Resolving an anchor

**Resolving** an anchor against *D* produces a range in *D* and a **state**:

| State | Meaning |
|---|---|
| `exact` | The anchor still describes this location without any search. |
| `relocated` | The anchor was found at a new location by deterministic text search. |
| `orphaned` | The anchor cannot be located. |

### 3.7.1 Definitions

Let `s`, `e`, `q`, `p`, and `x` be the stored start, end, exact, prefix, and suffix.

- An **occurrence** of a string *t* in a string *T* is any index *i* such that `T[i : i+len(t)] == t`. Occurrences may overlap.
- The **nearest** of a set of candidate offsets is the one closest to `s` by absolute difference. If two are equally close, the lower offset wins.
- The **context score** of a quote occurrence `[i, j)` in *T*, given a prefix *P* and a suffix *X*, is *L* + *R*:
  - *L* is the largest *k* ≤ `len(P)` such that `T[i−k : i] == P[len(P)−k :]`. This is how much of the end of the prefix still sits directly before the occurrence.
  - *R* is the largest *k* ≤ `len(X)` such that `T[j : j+k] == X[: k]`. This is how much of the start of the suffix still sits directly after the occurrence.
- **Selecting by context:** from a set of quote occurrences, select one as follows. If there is exactly one occurrence, select it. Otherwise, select the occurrence with the highest context score, but only if exactly one occurrence has that score and the score is greater than 0. In every other case, nothing is selected.
- The **whitespace characters** are U+0020 (space), U+0009 (tab), and U+000A (line feed).
- The **collapsed form** `W(T)` of a string *T* replaces every maximal run of whitespace characters with a single U+0020. Every code point *k* of `W(T)` corresponds to a **source span** `[a_k, b_k)` in *T*. For a non-whitespace code point, the span is that single code point. For a collapsed space, it is the whole run it replaced.

### 3.7.2 Algorithm

Implementations MUST produce the same result as the following steps, run in order. The first step that succeeds determines the result.

0. **Version match.** If the version of *D* equals `version`, the result is `[s, e)`, `exact`.
1. **Position check.** If `e ≤ len(D)`, `D[s:e] == q`, `D[s−len(p) : s] == p`, and `D[e : e+len(x)] == x`, the result is `[s, e)`, `exact`.
2. **Context search.** Find every occurrence *i* of `p + q + x` in *D*. If there is at least one, let `c` be the nearest of the candidate starts `i + len(p)`. The result is `[c, c+len(q))`, `relocated`.
3. **Quote search.** If `q` is not empty, find every occurrence of `q` in *D* and select by context, using `p` and `x`. If an occurrence `[c, c+len(q))` is selected, that is the result, `relocated`.
4. **Whitespace-insensitive quote search.** If `W(q)` contains at least one non-whitespace code point, find every occurrence of `W(q)` in `W(D)` and select by context, using `W(p)` and `W(x)` scored against `W(D)`. If an occurrence `[i, j)` of `W(D)` is selected, map it back to *D* as `[a_i, b_{j−1})`. The result is that range, `relocated`.
5. Otherwise the result is `orphaned`.

Notes:

- For a point range (`q` empty), step 2 searches for `p + x`, and steps 3 and 4 are skipped.
- In step 1, a slice that extends past either end of *D* never matches a non-empty `p` or `x`.
- In step 0, if the stored anchor is internally inconsistent (`e > len(D)` or `D[s:e] ≠ q`), the anchor is malformed. Implementations MUST skip step 0 and continue with step 1.
- Apart from the collapsing in step 4, all string comparisons compare code points exactly.
- Step 4 only affects matching. The resulting range covers the document's real text, whitespace included, and that text may differ from `q` in its whitespace.

> **Rationale.** Step 3 relocates a quote that appears more than once only if its surrounding text clearly picks one occurrence. Otherwise a comment on a common word such as "the" could silently jump to the wrong place. Step 4 rescues annotations in hard-wrapped Markdown or LaTeX after the paragraph is re-wrapped.

### 3.7.3 Orphaned annotations

An orphaned annotation MUST NOT be deleted, and its stored anchor MUST NOT be modified, as a result of resolution. Clients SHOULD show orphaned annotations to the user, for example in a list outside the document, so they can be re-attached or dismissed.

### 3.7.4 Suggested locations (non-normative)

For an orphaned annotation, a client MAY run any heuristic, such as fuzzy matching, diff-based mapping, or the nearest of several equally scored quote occurrences, to propose a **suggested location**. A suggested location is never treated as resolved. It is only offered to the user. If the user confirms it, the client re-anchors the annotation as described in §3.8.

## 3.8 Rewriting anchors

Over time, stored anchors drift away from the current document. Rewriting them keeps later resolutions fast (step 0) and accurate.

- If the state is `relocated`, a client MAY rewrite the anchor. It does this by creating a fresh anchor (§3.6) for the resolved range in *D*. After steps 2–3, `quote.exact` is unchanged by construction. After step 4, it takes the current text of the range, including its whitespace.
- If the state is `exact`, a client SHOULD NOT rewrite the anchor. The location hasn't changed, and rewriting would only add events (§2.4, `reanchor`) and diffs to shared files.
- If a user confirms a suggested location, the client MAY rewrite the anchor for the confirmed range. In this case `quote.exact` takes the current text of that range, which may differ from the original quote.
- A client MUST NOT rewrite an anchor based on an unconfirmed suggested location.

A rewrite is recorded as a `reanchor` event (§2.4), so previous anchors stay in the annotation's history. Suggestions have stricter rewriting rules (§4.2.1).

## 3.9 Example

Original document (78 code points):

```
\section{Results}
In Section 3, we prove that the bound is tight for all $n$.
```

An annotation on "we prove that" is stored as shown in §3.5. The hash is omitted there for brevity.

The document is then edited outside any annox-aware tool:

```
\section{Results}
As shown in Section 3, we prove that the bound is tight for all $n \geq 1$.
```

Resolving the anchor:

- **Step 0:** the version differs.
- **Step 1:** `D[32:45]` is `"ction 3, we p"`, not `"we prove that"`.
- **Step 2:** the old prefix (`…In Section 3, `) and the old suffix (`…all $n$.`) both changed, so `p + q + x` has no occurrence.
- **Step 3:** `"we prove that"` occurs only once, at 41, so it is selected without scoring. The result is `[41, 54)`, `relocated`.

A client that rewrites the anchor stores:

```json
"selectors": {
  "position": { "start": 41, "end": 54 },
  "quote": {
    "exact": "we prove that",
    "prefix": "Results}\nAs shown in Section 3, ",
    "suffix": " the bound is tight for all $n \\"
  }
}
```

It also stores the new document's `version`.

## Open questions

- **Reflowed point anchors.** Steps 3 and 4 don't apply to point ranges, so a point anchor whose context was re-wrapped is orphaned. Should step 2 get a whitespace-insensitive variant for point ranges?
- **Performance.** Steps 3 and 4 on large documents with many anchors cost O(document × anchors). This is probably fine for v1, but it needs checking.
