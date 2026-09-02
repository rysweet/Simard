---
title: Dashboard — honest "Memory records" count (checksummed envelope)
description: Reference for the Resources → Memory "Memory records" legacy tile (GET /api/memory → memory_records.count), which now reflects the real number of records persisted in memory_records.json instead of the on-disk envelope's key count. The FileBackedMemoryStore persists records as a checksummed envelope {"crc32": u32, "records": [...]}; count_json_records now returns the length of the records array for that shape (falling back to array length for the legacy plain-array format and to the object key count for any other object). Before the fix a store holding 1244 records reported 2, silently misreporting memory health to the operator. A hermetic serial endpoint test and an end-to-end HTTP QA scenario pin the semantics so the count cannot silently regress.
last_updated: 2026-07-15
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ../memory.md
  - ./dashboard-memory-tab.md
  - ./dashboard-memory-recent-last-hour-count.md
  - ./dashboard-overview-health-and-live-memory.md
  - ./cognitive-memory-client-helpers.md
---

# Dashboard — honest "Memory records" count

The Resources → **Memory** sub-section surfaces a small **"Memory records"**
legacy tile under the *"Legacy snapshots (superseded by the Memory Store)"*
disclosure. Its badge is `memory_records.count` from `GET /api/memory`. That
number now reports the **real number of records** persisted in
`<state_root>/memory_records.json`. Previously it reported **`2`** for a file
holding over a thousand records — telling the operator "Memory records: 2" while
the on-disk store actually held 1244
([#4075](https://github.com/rysweet/Simard/issues/4075)).

> **What changed.** The look is unchanged — same tile, same `count` field shape
> (`number`). One thing is fixed: `count_json_records` now understands the
> **checksummed envelope** that `FileBackedMemoryStore` writes, so the tile
> counts the records array rather than the envelope's two top-level keys. This
> is a **data-correctness fix to an existing tile**, not a new surface.

## Root cause (what was wrong)

`memory_records.json` is persisted by `FileBackedMemoryStore::persist_checksummed`
(`src/memory/file_backed.rs`) as a **checksummed envelope**:

```json
{ "crc32": 3861665043, "records": [ /* … 1244 items … */ ] }
```

The dashboard's `count_json_records` (`operator_commands_dashboard/subagent.rs`)
only understood two shapes:

- a top-level JSON **array** → `arr.len()`, and
- any JSON **object** → `map.len()` (the number of top-level keys).

So for the envelope it counted the object's two keys (`crc32` + `records`) and
returned **`2`**, regardless of how many records the file actually held. The
engine write was never broken: the defect was a **format-recognition gap** in
the dashboard's read/count helper.

## The fix

`count_json_records` now recognizes the envelope: when the value is an object
containing a `records` **array**, it returns that array's length. The other two
shapes are preserved for backward compatibility:

| On-disk shape | Example | Counted as |
|---------------|---------|------------|
| Checksummed envelope | `{"crc32": N, "records": [...]}` | `records` array length |
| Legacy plain array | `[ {...}, {...} ]` | array length |
| Any other object | `{"a": 1, "b": 2}` | top-level key count (legacy fallback) |

`evidence_records.json` is written by `FileBackedEvidenceStore` as a **plain
JSON array**, so it already resolved correctly through the array branch and is
unaffected by this change.

## API shape (`GET /api/memory`)

The response shape is unchanged; only the value of `memory_records.count`
becomes correct:

```jsonc
{
  "memory_records": {
    "path": "<state_root>/memory_records.json",
    "count": 1244,          // records array length — was 2 (envelope key count)
    "size_bytes": 424900,
    "modified": "2026-07-14T19:29:41.675472065+00:00"
  }
  // … evidence_records / goal_records / native_memory / handoff unchanged …
}
```

The **"Memory records"** legacy tile is gated on real content
(`size_bytes > 0` **and** `count > 0`, [#1681](https://github.com/rysweet/Simard/issues/1681)),
so the corrected count also governs whether the tile renders at all — a store
that previously misread as `2` now renders with its true magnitude.

## Test coverage

- **Unit** (`operator_commands_dashboard/subagent.rs`):
  `count_checksummed_envelope_uses_records_array_length` and companions cover
  the envelope, an empty `records` array, an object without a `records` key, and
  a `records` value that is not an array (falls back to key count, never
  panics).
- **Endpoint** (`operator_commands_dashboard/tests_live_goal_board.rs`):
  `memory_metrics_record_count_reads_checksummed_envelope` writes a
  checksummed-envelope `memory_records.json` into a hermetic state root and
  asserts `GET /api/memory` → `memory_records.count` equals the records-array
  length, not `2`.
- **End-to-end QA** (`scripts/qa-dashboard-memory-record-count.sh`, scenario
  `tests/qa-scenarios/dashboard-memory-record-count.yaml`): starts a standalone
  `simard dashboard serve`, seeds an envelope with 1244 records, and asserts the
  count reported over HTTP is `1244` — a value of `2` fails the gate.

## See also

- [Dashboard — dedicated Memory tab](./dashboard-memory-tab.md) — the live
  cognitive-memory graph rendered from `GET /api/memory/graph`.
- [Honest "items remembered in the last hour" count](./dashboard-memory-recent-last-hour-count.md)
  — the sibling `GET /api/memory/recent` `last_hour_count` data-correctness fix.
- [Dashboard Overview Health & Live Memory](./dashboard-overview-health-and-live-memory.md)
  — the `GET /api/memory` tiles this count feeds.
- [Cognitive memory client helpers](./cognitive-memory-client-helpers.md) — the
  reader path that supplies the live `native_memory.*` counts alongside these
  legacy tiles.
