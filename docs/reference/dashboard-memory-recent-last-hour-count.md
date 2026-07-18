---
title: Dashboard — honest "items remembered in the last hour" count
description: Reference for the Resources → Memory headline statistic "items remembered in the last hour", which now reflects the LIVE net growth of Simard's long-term cognitive memory over the trailing hour instead of a hardcoded 0. GET /api/memory/recent computes last_hour_count as max(0, live_long_term_total − baseline_long_term_total), where the live total (episodic+semantic+procedural+prospective) is read through the single shared reader open_reader_client → get_statistics(), and the baseline is the most-recent memory_history.json snapshot at-or-before now−3600s. The endpoint fails closed (error JSON, count null) on a live-read error and never returns a placeholder. A hermetic serial test pins the window and data-source semantics so the metric cannot silently regress to 0.
last_updated: 2026-07-07
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ../memory.md
  - ./dashboard-memory-tab.md
  - ./dashboard-overview-health-and-live-memory.md
  - ./cognitive-memory-client-helpers.md
---

# Dashboard — honest "items remembered in the last hour" count

The Resources → **Memory** sub-section leads with a large headline number and
the caption **"items remembered / in the last hour"** (`#mem-recent-count`,
`index_html/part_00.rs`). That number now reports the **live** net growth of
Simard's long-term cognitive memory over the trailing hour. Previously it was a
**hardcoded `0`** — the dashboard told operators "Simard has remembered 0 items
in the last hour" even while memory consolidation was actively running (~30
actions / 30 min) and the total fact / procedure / episode counts were climbing
all day ([#2679](https://github.com/rysweet/Simard/issues/2679)).

> **What changed.** The look is unchanged — same headline card, same caption.
> One thing is fixed: the backend field that feeds it,
> `GET /api/memory/recent` → `last_hour_count`, is now **computed from live
> memory state** instead of being emitted as the literal `0` that a prior
> de-fork ([#2307](https://github.com/rysweet/Simard/issues/2307)) left behind
> as a placeholder. This is a **data fix to an existing card**, not a new
> surface.

## Root cause (what was wrong)

`memory_recent()` in `operator_commands_dashboard/memory.rs` read the aggregate
`total` live — through the healthy `open_reader_client(state_root)?.ops()
.get_statistics()?` path — but returned `last_hour_count` as a **literal `0`**.
The per-item recent listing had been stubbed during de-fork Phase 2b (#2307,
because the library backend exposes no "list nodes newer than T" API), and the
last-hour field was left hardcoded rather than computed. The engine read was
never broken: the defect was a **stale placeholder binding** in the dashboard's
aggregation layer.

Because the bug was in the dashboard's read window / aggregation and **not** in
the memory engine's count or recall read, the fix lands entirely **Simard-side**.
It touches no `amplihack-memory-lib` code, requires no `amplihack-memory` pin
bump, and does not supersede the engine-side error-propagation work in
[PR #113](https://github.com/rysweet/amplihack-memory-lib/pull/113) —
that memory-arch policy gate is satisfied here by keeping the change in the
dashboard.

## What the operator sees

| Situation | Headline count | List body |
|-----------|----------------|-----------|
| Long-term memory grew by *N* in the trailing hour | *N* (e.g. `27`) | `No new memories in the last hour — <total> total stored.`¹ |
| A pruning / low-activity hour (net long-term change ≤ 0) | `0` — honestly | same as above |
| No snapshot history yet (cold start / <1 h uptime) | `0` — a snapshot is seeded so the metric self-heals on the next poll | same as above |
| Live cognitive store unreachable | `—` (em dash) | the `error` string rendered in red |

¹ The per-item recent list stays unavailable on the library backend (#2307);
the headline count is the honest signal that memory *is* moving. Note the count
is **net** growth (additions minus pruning/consolidation over the hour), not a
gross count of every item written: an hour that adds many items but prunes at
least as many reads `0`. For per-type deltas and growth rates, see
`GET /api/memory/history`
([Memory tab](./dashboard-memory-tab.md) and
[live memory-consolidation display](./dashboard-overview-health-and-live-memory.md)).

The caption is unchanged and now accurate: it counts **items remembered** —
i.e. items consolidated into long-term memory — over the last hour.

## Live data source: `GET /api/memory/recent`

The headline number is driven by the `last_hour_count` field of this endpoint.
The endpoint reads the **live** cognitive store through the single shared reader
and derives the trailing-hour delta from the on-disk snapshot history; there is
no placeholder on the normal path.

### Response shape

```jsonc
{
  "items": [],                 // per-item listing unavailable on the library backend (#2307)
  "total": 41822,              // live aggregate stored count across all six memory types
  "last_hour_count": 27,       // LIVE net growth of long-term memory over the trailing hour
  "last_hour_window_secs": 3660, // ACTUAL elapsed time that delta covers (#4318); null with no baseline
  "available": false,          // refers to the per-item `items` list, not the count
  "note": "`last_hour_count` is the net growth of long-term memory (episodic+semantic+procedural+prospective) over the trailing hour, derived from snapshot history; per-item listing is unavailable on the library backend (#2307). See /api/memory/history for per-type deltas.",
  "server_time": "2026-07-07T18:34:00Z"
}
```

### Field contract

| Field | Type | Meaning |
|-------|------|---------|
| `last_hour_count` | `integer` (`u64`) on the normal path; `null` on the error path | Net growth of **long-term** memory (episodic + semantic + procedural + prospective) over the trailing hour, clamped to ≥ 0. This is the value bound to `#mem-recent-count`. |
| `last_hour_window_secs` | `number` (seconds, ≥ 0) on the normal path; `null` when there is no baseline or on the error path | The **actual** elapsed time the `last_hour_count` delta spans — `now − baseline.epoch_secs` (#4318). Because the baseline is a discrete snapshot, this can exceed one hour when history is sparse. The caption (`#mem-recent-window`) reads "in the last hour" only when this is within ±15 min of 3600 s; otherwise it renders the real window (e.g. "in the last 2.6h"), so the number is never labeled dishonestly. |
| `total` | `integer` (`u64`); **omitted on the error path** | Live aggregate stored count across **all six** memory types (`CognitiveStatistics::total()`); rendered beside the headline as `<total> total`. Present with the same value on the normal path (unchanged by this fix); on the error path the payload omits it, mirroring `GET /api/memory/history`. |
| `items` | array | Always `[]` on the library backend — per-item recent listing is unavailable (#2307). Unchanged by this fix. |
| `available` | `bool` | Refers to the per-item `items` list (`false` on the library backend), **not** to the headline count. Unchanged by this fix. |
| `note` | `string` | Human-readable, path-free explanation of what `last_hour_count` measures and where per-type deltas live. |
| `server_time` | `string` (RFC 3339) | Server timestamp of the read. |
| `error` | `string` (only on the error path) | Present only when the live reader could not be opened / read. On this path `last_hour_count` is `null`, `available` is `false`, and `items` is `[]`. |

**Back-compatible.** On the normal (success) path, every field that existed
before the fix (`items`, `total`, `available`, `note`, `server_time`) is
preserved with the same type and meaning. The only behavioural change there is
that `last_hour_count` went from a constant `0` to a computed value. No field
was removed or renamed on the success path. On read failure the endpoint now
**fails closed** — `last_hour_count` is `null`, a new `error` field is added,
and the count-only fields (`total`, `note`) are omitted — instead of the prior
behaviour of returning `total: 0` with no `error`. This deliberately aligns the
failure shape with `GET /api/memory/history`.

`last_hour_window_secs` (#4318) is **additive**: it is a new field on both the
success path (numeric, or `null` when there is no baseline) and the error path
(`null`). No existing consumer breaks — clients that ignore it see the same
`last_hour_count` behaviour as before.

### How `last_hour_count` is computed

```text
last_hour_count = max(0, live_long_term_total − baseline_long_term_total)
```

- **`live_long_term_total`** — the sum of the four long-term memory types
  (`episodic + semantic + procedural + prospective`) from a single live
  `get_statistics()` read through `open_reader_client`. This is the same live
  read path the Memory tab and `/api/memory/history` use, so the number never
  diverges from the counts shown elsewhere. Transient `sensory` and `working`
  memory are **excluded** — they are task-scoped churn that pruning makes
  net-negative and noisy, and "remembered" means *consolidated into long-term
  memory*.
- **`baseline_long_term_total`** — the `long_term_total` of the most-recent
  `MemorySnapshot` in `memory_history.json` whose `epoch_secs ≤ now − 3600`
  (at-or-before the one-hour edge). This pins the trailing-hour window
  deterministically.
- **Fallback** — if the only snapshots are younger than one hour (short
  uptime), the **earliest** snapshot is used, so the metric honestly reports the
  growth over the partial window rather than fabricating a full hour.
- **Cold start** — if there is no history at all, a snapshot is seeded at `now`
  (via `append_snapshot_if_due`, see below) and the count reads `0` until a
  real baseline ages past the hour edge. It self-heals on subsequent polls.
- **Clamp** — a net-negative interval (pruning / consolidation shrank the
  long-term total) is clamped to `0` with a saturating subtraction. Simard
  cannot "remember a negative number"; a genuinely low-activity hour honestly
  reads `0`, never a spurious large value.

Because baselines are discrete snapshots taken at most every
`SNAPSHOT_MIN_INTERVAL_SECS` (5 min), the window edge has a granularity of about
one sample interval **when snapshots are dense**. When the daemon is down or
throttled, history can be **sparse**: the most-recent snapshot at-or-before the
one-hour edge may be several hours old, so `last_hour_count` then spans that
longer interval — not one hour. Rather than hide this, the endpoint reports the
true span in `last_hour_window_secs` (#4318) and the caption labels the number
with the real window ("in the last hour" only when it genuinely is ~1h). The
helper always selects the closest sample **at or before** the edge; the honest
window keeps the headline from ever overstating a one-hour rate.

### Snapshot history and the read-path side effect

`last_hour_count` reuses the same `memory_history.json` ring buffer that powers
`GET /api/memory/history`:

- Each `MemorySnapshot` carries `epoch_secs`, `total`, and
  `long_term_total` (= `episodic + semantic + procedural + prospective`).
- On every `GET /api/memory/recent`, the handler calls
  `append_snapshot_if_due(...)`, which records a new snapshot only if
  `SNAPSHOT_MIN_INTERVAL_SECS` (5 min) has elapsed since the last one, and trims
  the buffer to `HISTORY_MAX_SNAPSHOTS` (500). `append_snapshot_if_due` is the
  **same shared, already-gated** writer `GET /api/memory/history` uses: it
  records at most one snapshot per `SNAPSHOT_MIN_INTERVAL_SECS` across **all**
  callers. Adding this second call site on `/api/memory/recent` therefore does
  **not** increase snapshot-write frequency — the dashboard polls both endpoints,
  and whichever fires first inside the 5-minute window records the sample. It
  simply keeps the baseline accumulating so the metric self-heals over time.
  (Before this fix `/api/memory/recent` did not touch history at all, so this
  is a **new write call site** at this endpoint. What is unchanged is the
  **global snapshot cadence** — at most one write per
  `SNAPSHOT_MIN_INTERVAL_SECS` across all callers — because the added call
  shares the same gated writer rather than adding an independent write.)

### Error path (fail-closed)

If the live reader cannot be opened or `get_statistics()` fails, the endpoint
**fails closed**, mirroring `GET /api/memory/history`:

```jsonc
{
  "items": [],
  "available": false,
  "last_hour_count": null,
  "error": "Cannot read cognitive memory: <reason>",
  "server_time": "2026-07-07T18:34:00Z"
}
```

The dashboard renders the `error` string in red and shows `—` for the headline
(`fetchRecentMemories` in `index_html/part_03.rs` already branches on
`d.error`). It never shows a fabricated `0` in place of a real read failure.

### Example

```bash
curl -s --cookie "session=<code>" http://localhost:8080/api/memory/recent \
  | jq '{last_hour_count, total, available}'
```

```json
{
  "last_hour_count": 27,
  "total": 41822,
  "available": false
}
```

Confirm the metric is honest by cross-checking the same growth in the per-type
history endpoint:

```bash
curl -s --cookie "session=<code>" http://localhost:8080/api/memory/history \
  | jq '.rate_per_hour'
```

```json
{ "total": 30.0, "long_term_total": 27.0, "episodic": 18.0, "semantic": 4.0, "procedural": 3.0, "prospective": 2.0 }
```

The `long_term_total` growth rate from `/api/memory/history` and the
`last_hour_count` from `/api/memory/recent` describe the same underlying live
movement (one as a rate, one as a trailing-hour count), so the two panels agree.

## Backend architecture

### Route wrapper + env-free testable core

Following the established `goals()` → `goals_at(state_root)` split
(`operator_commands_dashboard/goals.rs`, #2408 / #2384), the handler resolves
the ambient state root in a thin wrapper and delegates all logic to an
env-free core that takes an **explicit** `state_root`, so it can be driven
deterministically without HTTP or environment variables:

```rust
/// `GET /api/memory/recent` — resolves the ambient state root and delegates.
pub(crate) async fn memory_recent() -> Json<Value> {
    memory_recent_at(&resolve_state_root()).await
}

/// Env-free core of `memory_recent`: computes the trailing-hour long-term
/// growth from the EXPLICIT `state_root`, so tests can pin `state_root`
/// directly instead of via `SIMARD_STATE_ROOT`.
pub(crate) async fn memory_recent_at(state_root: &std::path::Path) -> Json<Value>;
```

`memory_recent_at`:

1. Reads live stats once via `open_reader_client(state_root)` →
   `.ops().get_statistics()`. On error it returns the fail-closed JSON above.
2. Calls `append_snapshot_if_due(&history_path, &stats)` to keep the baseline
   history current.
3. Computes `live_long_term_total` inline
   (`episodic + semantic + procedural + prospective`, mirroring
   `MemorySnapshot::from_stats`).
4. Selects the baseline via the pure helper below and returns
   `max(0, live − baseline)` as `last_hour_count`.

### Pure baseline selector

The window logic is a pure, I/O-free helper so the boundary rule is unit-testable
with an injected `now`:

```rust
/// Long-term total of the most-recent snapshot at-or-before `now_secs − 3600`,
/// falling back to the earliest snapshot; `None` on empty history.
fn select_last_hour_baseline(history: &[MemorySnapshot], now_secs: f64) -> Option<u64>;
```

- `cutoff = now_secs − LAST_HOUR_WINDOW_SECS` (3600 s).
- Filter to snapshots with `epoch_secs ≤ cutoff`, take the **most-recent** such.
- If none qualify, fall back to the earliest snapshot; empty history → `None`
  (the caller then uses the live total, yielding `0` at cold start).

### Single shared live reader

The read goes through `open_reader_client(state_root) -> SimardResult<ReaderClient>`
and `ReaderClient::ops() -> &dyn CognitiveMemoryOps` — the **same** shared live
read path the Memory tab, `/api/memory/history`, and the rest of the dashboard
use. There is no second handle and no stale snapshot on the normal path.

> **No "Bridge" identifiers.** The accessor is `open_reader_client` returning
> `ReaderClient`; this fix introduces no new `*Bridge` symbol.

### No stray diagnostics

The read path emits **no** `println!` / `eprintln!`. Read failures surface
through the `error` field and normal `Result` handling; any diagnostics use
`tracing`, keeping production output clean.

## Configuration

No new configuration is introduced. The metric is governed by constants already
defined in `operator_commands_dashboard/memory.rs`, plus one added window
constant:

| Constant | Value | Role |
|----------|-------|------|
| `LAST_HOUR_WINDOW_SECS` | `3600.0` | Trailing window for `last_hour_count`. A baseline snapshot must be at-or-before `now − LAST_HOUR_WINDOW_SECS`. |
| `SNAPSHOT_MIN_INTERVAL_SECS` | `300` | Minimum spacing between recorded snapshots (5 min). Sets the effective granularity of the window edge. |
| `HISTORY_MAX_SNAPSHOTS` | `500` | Ring-buffer cap for `memory_history.json`. |

The snapshot history lives at `<state_root>/memory_history.json`, where
`state_root` is resolved by `resolve_state_root()` (honouring `SIMARD_STATE_ROOT`).

## Tests

A hermetic, serial regression test pins both the time-window and the
data-source semantics so the count cannot silently regress to `0`:

1. **Integration — `last_hour_count` is nonzero after in-window writes**
   (`operator_commands_dashboard/tests_memory_recent_last_hour.rs`, wired via
   `#[cfg(test)] mod tests_memory_recent_last_hour;` in `mod.rs`, annotated
   `#[serial_test::serial(cognitive_memory)]`). It:
   - pins `SIMARD_STATE_ROOT` to a `HermeticState` temp root and opens a live
     `LibraryCognitiveMemory`, registering it as the in-process writer
     (`register_in_process_writer`);
   - captures the current long-term total `T0`, then seeds
     `memory_history.json` with one baseline `MemorySnapshot` at
     `epoch_secs = now − 3600` and `long_term_total = T0`;
   - writes **N** in-window items via `store_episode` / `store_fact` (both feed
     `long_term_total`);
   - calls `memory_recent_at(state.state_root())` and asserts
     `last_hour_count == N` (**not `0`**), that `total` / `available` are
     preserved, and that no `error` key is present.

2. **Unit — `select_last_hour_baseline` window edge**
   (same test module, no I/O, injected `now_secs`). Asserts that a snapshot at
   **exactly** `now − 3600` **is** selected (locking the `≤ cutoff` boundary),
   that a snapshot at `now − 3599` is **not**, that sub-hour-only history falls
   back to the earliest snapshot, and that empty history yields `None`. This
   pins the off-by-one / window-boundary behaviour independently of wall-clock
   timing.

## Constraints honoured

- **Additive / back-compatible on the success path.** No endpoint removed or
  renamed; `/api/memory`, `/api/memory/recent`, `/api/memory/history`,
  `/api/memory/search` keep their existing fields. On the success path every
  prior field is preserved and `last_hour_count` merely changed from a literal
  `0` to a computed value. The **error path deliberately changes shape** to
  mirror `GET /api/memory/history` — it adds `error`, sets `last_hour_count:
  null`, and omits the count-only fields (`total`, `note`) rather than the old
  behaviour of returning `total: 0` with no `error`.
- **Live data only.** The count is derived from a live `get_statistics()` read
  plus the accumulating snapshot history — no stale snapshot or placeholder on
  the normal path.
- **Fail-closed.** A live-read error returns an explicit `error` payload with
  `last_hour_count: null`, never a fabricated `0`.
- **No new `println!` / `eprintln!`** in the production read path.
- **No new `*Bridge` identifiers** — the reader is `open_reader_client` →
  `ReaderClient`.
- **Engine untouched.** The fix is Simard-side (dashboard aggregation); no
  `amplihack-memory-lib` change and no `amplihack-memory` pin bump.
- **Never `--admin` / `--no-verify`.**

## Out of scope

- **Distillation rate (#2679 facts-per-hour).** If distilled *facts* are
  ~0 / hour (the distill parse-fail tracked separately), the metric still
  reports honest movement across episodes, procedures, and prospective triggers,
  so it never implies memory is idle. Fixing the distillation rate itself is out
  of scope for this display fix.
- **Per-item recent listing.** Still unavailable on the library backend
  (#2307); `items` remains `[]`.
- **Frontend markup / label.** The caption already reads
  "items remembered / in the last hour" and is now accurate — no UI redesign.

## Related

- [Dashboard](../dashboard.md) — full tab catalogue and the Tab Identity Contract.
- [Memory architecture](../memory.md) — the cognitive-memory model behind the count.
- [Dashboard — dedicated Memory tab](./dashboard-memory-tab.md) — the live memory-graph surface reading the same `open_reader_client` path.
- [Dashboard — live memory-consolidation display](./dashboard-overview-health-and-live-memory.md) — sibling live-memory display fix and the `/api/memory/history` growth rates.
- [Cognitive-memory client helpers](./cognitive-memory-client-helpers.md) — `open_reader_client` and the shared read path.
