---
title: Dashboard — non-null "Recent Memories" timestamp
description: Reference for the dashboard "Recent Memories" panel timestamp, which now renders EpisodicMemory.created_at as an RFC-3339 instant instead of a structural null. The fix threads created_at through the memory boundary — EpisodicMemory.created_at → CognitiveEpisode.created_at (a new #[serde(default)] Option<DateTime<Utc>> field that survives the memory_ipc serde round-trip) → to_episode() in library_adapter.rs → build_recent_episode_items in operator_commands_dashboard/memory.rs, which emits created_at.map(rfc3339).unwrap_or(Value::Null). Newest-first ordering (by temporal_index) is unchanged. Additive and back-compatible: episodes serialized before the field existed deserialize with created_at = None and render a null timestamp, exactly as today.
last_updated: 2026-07-21
owner: simard
doc_type: reference
status: reference
related:
  - ../dashboard.md
  - ../memory.md
  - ./dashboard-memory-tab.md
  - ./dashboard-memory-recent-last-hour-count.md
  - ./cognitive-memory-episodic-recall.md
  - ./base-type-adapters.md
---

# Dashboard — non-null "Recent Memories" timestamp

> **Issue [#4383](https://github.com/rysweet/Simard/issues/4383).** The
> dashboard **Recent Memories** panel showed a **structurally always-null**
> timestamp even though `EpisodicMemory.created_at` was populated on the backend.
> The wall-clock instant is now threaded end-to-end so each recent memory renders
> a real "time ago" label.

The Resources → **Memory** tab renders a **Recent Memories** list — a "recent
glance" at the newest episodic memories, each shown as
`{category, summary, timestamp, source, node_id}`
(`fetchRecentMemories`, `index_html/part_03.rs`). The `timestamp` field feeds the
frontend's "time ago" label. Previously that field was **hardcoded to
`Value::Null`**: `build_recent_episode_items` in
`src/operator_commands_dashboard/memory.rs` had no `created_at` to emit, because
`CognitiveEpisode` did not surface one — even though the underlying
`EpisodicMemory` record carried a populated `created_at`.

This reference documents the four-hop path that now carries `created_at` from
the memory store to the rendered timestamp, and the serde/back-compat contract
that keeps the change additive.

> **What changed.** The panel's look is unchanged — same list, same
> newest-first order. One thing is fixed: the `timestamp` field is now the
> episode's real `created_at` (RFC-3339) instead of a placeholder `null`. This is
> a **data-threading fix**, not a new surface.

---

## Root cause

`EpisodicMemory` (the Python-mirrored memory record) has always carried a
`created_at` wall-clock timestamp. But the Rust dashboard read episodes through
`CognitiveEpisode`, whose field set stopped at
`{node_id, content, source_label, temporal_index, compressed}` — it **dropped**
`created_at` at the `to_episode()` adapter boundary. With no `created_at` to
emit, `build_recent_episode_items` hardcoded `"timestamp": Value::Null`.

The pre-fix code even documented the null as intentional, reasoning that the
library backend assigned a monotonic `temporal_index` ordinal rather than a
wall-clock instant. That reasoning was **stale**: `EpisodicMemory.created_at`
*is* a real UTC instant and *is* populated; it was simply never carried across
the adapter. The fix is to thread it through, not to synthesize a timestamp.

---

## The four-hop fix

### 1. `CognitiveEpisode.created_at` field

`CognitiveEpisode` (in `src/memory_cognitive.rs`) gains an optional,
serde-defaulted timestamp so it survives the `memory_ipc` JSON boundary:

```rust
/// Autobiographical event from episodic memory. Maps to Python `EpisodicMemory`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveEpisode {
    pub node_id: String,
    pub content: String,
    pub source_label: String,
    pub temporal_index: i64,
    pub compressed: bool,

    /// When this episode was recorded (UTC). Mirrors
    /// `EpisodicMemory.created_at`. `#[serde(default)]` so episodes serialized
    /// before this field existed deserialize to `None` and render a null
    /// timestamp, preserving back-compat.
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

This mirrors the established pattern used by `CognitiveFact.last_accessed_at`
(issue #2395): an `Option<DateTime<Utc>>` guarded by `#[serde(default)]`.

### 2. `to_episode()` propagation

The adapter in `src/cognitive_memory/library_adapter.rs` populates the new field
from the source record instead of dropping it:

```rust
fn to_episode(e: EpisodicMemory) -> CognitiveEpisode {
    CognitiveEpisode {
        node_id: e.node_id,
        content: e.content,
        source_label: e.source_label,
        temporal_index: e.temporal_index,
        compressed: e.compressed,
        created_at: Some(e.created_at),   // ← was silently dropped
    }
}
```

Every other `CognitiveEpisode` construction site (native, stub, and test
fixtures) sets `created_at: None` — a compiler-enforced fan-out; no site is
missed.

### 3. Serializer emits RFC-3339 or null

`build_recent_episode_items` in `src/operator_commands_dashboard/memory.rs`
emits the timestamp with **no `unwrap`/`expect`**. The stale doc comment above
the function (which currently documents `timestamp` as *always* `null` and
rationalizes it via `temporal_index` ordinals) **must be replaced** — leaving it
in place would contradict the new behavior:

```rust
fn build_recent_episode_items(episodes: &[CognitiveEpisode]) -> Vec<Value> {
    episodes
        .iter()
        .map(|e| {
            json!({
                "category": "Past event",
                "summary": truncate_graph_content(&e.content),
                "timestamp": e
                    .created_at
                    .map(|t| Value::String(t.to_rfc3339()))
                    .unwrap_or(Value::Null),
                "source": e.source_label,
                "node_id": e.node_id,
            })
        })
        .collect()
}
```

### 4. Ordering unchanged

Newest-first ordering still derives from the `temporal_index` sort, **not** from
`created_at`. Adding the timestamp field changes only what is *rendered*, never
the order episodes appear in.

---

## API contract — `GET /api/memory/recent`

Each item in the Recent Memories payload:

| Field | Type | Notes |
|-------|------|-------|
| `category` | string | Always `"Past event"` for episodes. |
| `summary` | string | Episode content, bounded by `GRAPH_NODE_CONTENT_MAX`. |
| `timestamp` | string \| null | RFC-3339 UTC instant from `created_at`; `null` only when the episode genuinely has no recorded `created_at` (pre-field serialized data). |
| `source` | string | `source_label`. |
| `node_id` | string | Episode node ID. |

**Guarantee:** an episode whose `EpisodicMemory.created_at` is populated
surfaces a **non-null** RFC-3339 `timestamp`. The field is `null` **only** for
legacy episodes serialized before the field existed — never structurally for
every episode as before.

---

## Serde back-compatibility

The field is `#[serde(default)]`:

- **Reading old IPC / snapshots.** Episode JSON written before this field
  existed has no `created_at` key; `#[serde(default)]` deserializes it to `None`,
  and the serializer renders `null` — identical to today's behavior for that
  data. No migration, no read failure.
- **No panic on malformed data.** The serializer uses `Option` + `map`/`unwrap_or`
  with **no `unwrap`/`expect`**, so a missing or malformed IPC timestamp yields
  `null`, never a panic (no DoS surface on deserialized input).

---

## Tests (regression-pinned)

- `src/memory_cognitive.rs` — a serde round-trip test asserting a
  `CognitiveEpisode` with `Some(created_at)` survives serialize → deserialize
  across the `memory_ipc` boundary, and that a payload **without** the key
  deserializes to `created_at = None`.
- `src/operator_commands_dashboard/memory.rs` — a serializer test asserting:
  - a populated `created_at` renders a **non-null** RFC-3339 `timestamp`; and
  - a `None` `created_at` renders `Value::Null`;
  - newest-first ordering is preserved regardless of the timestamp field.

---

## Security considerations

- **No `unwrap` on IPC-deserialized data** — the serializer cannot panic on a
  malformed timestamp.
- **Scope is one low-sensitivity field** (`created_at`); no adjacent PII is
  exposed and no field visibility is widened.
- **UTC RFC-3339 enforced** — timestamps are emitted in a single canonical
  format. Structured `tracing` + OTel only; no `print!`/`println!` added.

---

## Related reading

- [Dashboard memory tab reference](./dashboard-memory-tab.md) — the panel this
  timestamp renders in.
- [Dashboard — honest "items remembered in the last hour" count](./dashboard-memory-recent-last-hour-count.md)
  — the companion `GET /api/memory/recent` field that reports live growth.
- [Cognitive-memory episodic recall reference](./cognitive-memory-episodic-recall.md)
  — how episodes are stored and read.
- [Base type adapters](./base-type-adapters.md) — the `to_episode()` adapter
  family that maps memory records to their `Cognitive*` mirrors.
