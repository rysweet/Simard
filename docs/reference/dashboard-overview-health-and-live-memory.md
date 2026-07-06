---
title: Dashboard — Open PRs card removal & live memory-consolidation display
description: Reference for two dashboard component fixes shipped for issue #26 — the duplicative Overview → Health "Open PRs" card is removed (its markup, its client renderer, and its /api/activity → open_prs data producer are all gone), leaving the strictly-superset Merge Readiness card as the single Overview PR surface; and the Resources → Memory "Last Memory Compaction" statistic now reflects LIVE consolidation state (real last-consolidation timestamp and a recent-activity count sourced from the live cognitive store and the consolidate-memory OODA action stream) instead of a stale snapshot derived from legacy JSON file mtimes.
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./dashboard-activity-cycle-reports.md
  - ./dashboard-goal-lifecycle-status.md
  - ./cognitive-memory-client-helpers.md
  - ../memory.md
---

# Dashboard — Open PRs card removal & live memory-consolidation display

Reference documentation for two Simard dashboard component fixes shipped for
[issue #26](https://github.com/rysweet/Simard/issues/26). Both are **data**
fixes to already-existing Overview/Resources cards, not new surfaces:

1. **Open PRs card removed.** The Overview → **Health** sub-section carried two
   overlapping PR cards — a plain **Open PRs** list and a richer **Merge
   Readiness** card. The Open PRs card is a strict subset of Merge Readiness and
   has been **removed completely** — markup, client renderer, and its
   server-side data producer. Merge Readiness is the single Overview PR surface.
2. **Live memory-consolidation display.** The Resources → **Memory**
   sub-section's **Last Memory Compaction** statistic showed a stale, frozen
   value even while memory consolidation was actively running. It now reflects
   **live** consolidation state, in the same live-read spirit as the Activity
   and Goals tab fixes ([#2697](https://github.com/rysweet/Simard/issues/2697),
   [#2695](https://github.com/rysweet/Simard/issues/2695)).

| Fix | Surface | Location | Endpoint | Change |
|-----|---------|----------|----------|--------|
| 1 | Overview → Health | former `#open-prs-list` card (`index_html/part_00.rs`) | `GET /api/activity` → `open_prs` | **removed** (card + renderer + data key) |
| 2 | Resources → Memory | `#mem-overview` "Last Memory Compaction" stat (`index_html/part_02.rs`) | `GET /api/memory` → `last_consolidation`, `recent_consolidation_activity` | **now live** |

> **Not touched.** The **Merge Readiness** card (`GET /api/merge-readiness`,
> `merge_readiness.rs`) and the **Pull Requests → Readiness** tab
> (`GET /api/prs`, `pr_readiness.rs`) are independent surfaces with their own
> data paths. Both keep working unchanged. The identically-named `open_prs`
> array inside the `/api/merge-readiness` payload is a **different object** from
> the removed `/api/activity` → `open_prs` and is untouched.

---

## Fix 1 — Open PRs card removed

### Why

The Overview → **Health** sub-section previously rendered two PR cards side by
side:

- **Open PRs** — a flat list of the operator's own open PRs (`#N`, title, age),
  produced by `gh pr list --author @me` and surfaced at `/api/activity` under
  the `open_prs` key.
- **Merge Readiness** — a per-PR readiness card (CI status-check rollup, base
  branch allow-list, objective merge-gate verdict, blocker reason, and the
  active merge-judge kind), produced by `/api/merge-readiness`.

Every datum the Open PRs card showed (PR number, title, URL) is **also** present
in the Merge Readiness card, which additionally shows *whether each PR can
merge*. The Open PRs card carried **no unique useful information**, so it was a
duplicate that added visual noise and an extra `gh` invocation per Overview
refresh. It has been removed cleanly rather than folded, because there was
nothing to fold.

### What changed

The removal is surgical and additive-safe — it deletes only the Open PRs
surface and its now-dead data producer, and leaves the rest of the Overview
Health layout (Merge Readiness, System Status, Cognition: Recall Precision, Open
Issues, Machines & Memory Sharing) intact.

**Client (removed):**

- The card markup in `index_html/part_00.rs`:
  ```html
  <div class="card">
    <h2>Open PRs</h2>
    <div id="open-prs-list"><span class="loading">Loading…</span></div>
  </div>
  ```
- The render block in `index_html/part_01.rs` that read `d.open_prs` and wrote
  into `#open-prs-list`.

**Server (`/api/activity`):**

`activity()` in `activity.rs` no longer fetches or emits open PRs. The
`gh pr list --author @me` half of its concurrent `tokio::join!` is removed, and
the `open_prs` key is dropped from the response body. The endpoint still
concurrently fetches and returns `assigned_issues`, and still returns `daemon`,
`recent_cycles`, and `timestamp`.

### `/api/activity` response — after

```jsonc
{
  "daemon": {
    "status": "running",
    "current_cycle": 4137,
    "last_heartbeat": "2026-07-06T17:31:04Z",
    "actions_taken": 6
  },
  "recent_cycles": [ /* … newest-first cycle reports … */ ],
  "assigned_issues": [
    { "number": 26, "title": "…", "url": "https://…", "labels": [ /* … */ ] }
  ],
  "timestamp": "2026-07-06T17:31:05Z"
}
```

The `open_prs` key is **absent** — clients must not expect it. The removal is a
net reduction of one `gh` subprocess per Overview refresh.

> **TUI.** The terminal UI's standalone "Open PRs: N" *count* stat (a single
> number in the stats row, not a card that duplicates a TUI Merge-Readiness
> component) is intentionally **left in place**. It is not the duplicative
> surface this fix targets, and removing it would break the TUI stat row.

---

## Fix 2 — Live memory-consolidation display

### Why

Memory consolidation runs continuously: the OODA loop dispatches roughly
**30 `consolidate-memory` actions per 30 minutes**, and episodic memory grows
measurably across a day (for example 756 → 1088 facts). Yet the Resources →
**Memory** sub-section's **Last Memory Compaction** statistic looked **frozen** —
it never advanced, so operators reasonably concluded consolidation had stalled
when it had not.

This is the **same class of bug** as the Activity-tab cycle-reports and
Goals-tab status bugs (#2697 / #2695): the card rendered a **stale snapshot**
from a source that is no longer written, instead of reading live state at render
time.

### Root cause

`memory_metrics()` in `metrics.rs` derived `last_consolidation` from the
**modification time of legacy JSON files** (`memory_records.json`,
`evidence_records.json`) under the state root:

```rust
let last_consolidation = [&memory_path, &evidence_path]
    .iter()
    .filter_map(|p| std::fs::metadata(p).ok())
    .filter_map(|m| m.modified().ok())
    .max()
    .map(/* … rfc3339 … */);
```

Those files were superseded by the library-backed **cognitive memory** store
(`<state_root>/cognitive`) and are **no longer written** by the consolidation
path. Their mtimes are therefore frozen at whenever they were last touched (or
the files are absent, rendering `Not tracked yet` forever). The per-type counts
next to the statistic (**Events remembered** / episodic, **Facts learned** /
semantic, **Known procedures** / procedural, …) were *already* live — they come
from `get_statistics()` on the cognitive store — which is exactly why the panel
looked half-alive: the counts moved but the timestamp did not.

### What changed

`memory_metrics()` now sources consolidation **freshness** from live OODA
activity while keeping the live counts it already had. The two data paths stay
cleanly separated — **counts** vs. **timestamp**:

1. **The cognitive store** (`<state_root>/cognitive`) feeds the **counts** only
   (`native_memory.*`), exactly as before. `get_statistics()` returns counts
   but **no timestamp**, which is precisely why it cannot (and does not) drive
   `last_consolidation`.
2. **The `consolidate-memory` OODA action stream** — the same persisted
   per-cycle reports (`<state_root>/cycle_reports/` and
   `<state_root>/state/cycle_reports/`, files `cycle_<N>.json`, read
   newest-first) that the Activity tab reads live. Each cycle report's actions
   are scanned for `action_kind == "consolidate-memory"` — the canonical
   `Display` form of `ActionKind::ConsolidateMemory` that `persist_cycle_report`
   writes via `.to_string()`, so the scan matches exactly what the daemon
   persists — bounded and short-circuited newest-first so the hot,
   repeatedly-polled `/api/memory` endpoint never does an unbounded disk scan.

The `last_consolidation` field keeps its **name and type** (`Option<String>`,
RFC 3339) for backward compatibility with the client template — but its value is
now the timestamp of the **most recent live consolidation signal**, not a legacy
file mtime. A new `recent_consolidation_activity` object reports how many
`consolidate-memory` actions were seen in the recent window and when the last
one occurred.

### `/api/memory` response — new and changed fields

```jsonc
{
  "state_root": "/home/user/.simard/state",
  "total_facts": 1088,
  "native_memory": {
    "sensory": 3,
    "working": 12,
    "episodic": 1088,       // Events remembered — live, grows with consolidation
    "semantic": 402,        // Facts learned — live
    "procedural": 57,       // Known procedures — live
    "prospective": 9,
    "total": 1571
  },
  "native_memory_error": null,
  "native_memory_db_path": "/home/user/.simard/state/cognitive",
  "native_memory_db_exists": true,

  // CHANGED: now a live signal (RFC 3339), not a legacy JSON file mtime.
  // null only when no consolidation signal exists yet.
  "last_consolidation": "2026-07-06T17:29:41Z",

  // NEW: recent consolidate-memory OODA activity summary.
  "recent_consolidation_activity": {
    "count": 30,                          // consolidate-memory actions in the recent window
    "last": "2026-07-06T17:29:41Z"        // timestamp of the most recent one (or null)
  },

  "timestamp": "2026-07-06T17:31:05Z"
  // … legacy memory_records / evidence_records / goal_records / handoff tiles unchanged …
}
```

Field contract:

| Field | Type | Meaning |
|-------|------|---------|
| `last_consolidation` | `string \| null` (RFC 3339) | Timestamp of the most recent **live** consolidation signal — the newest `consolidate-memory` OODA action timestamp from the bounded cycle-report scan. `null` when no such action exists yet (→ `Not tracked yet`). **No** legacy-JSON-mtime fallback and **no** directory-mtime fallback: the panel fails closed to `null` rather than fabricating a timestamp. |
| `recent_consolidation_activity.count` | `number` (u64) | Count of `consolidate-memory` actions observed in the bounded recent cycle-report window. `0` when none. |
| `recent_consolidation_activity.last` | `string \| null` (RFC 3339) | Timestamp of the most recent `consolidate-memory` action, or `null`. |

### What the operator sees

The **Last Memory Compaction** statistic now advances as consolidation runs. On
a live daemon it reads, for example:

```
Total Facts                 1088
Last Memory Compaction      2 minutes ago (2026-07-06 17:29:41)
Recent consolidation        30 in recent cycles · last 2 minutes ago
Memory Store
  Events remembered         1088   ← grows through the day (e.g. 756 → 1088)
  Facts learned             402
  Known procedures          57
```

The existing client template contract is preserved: the statistic still renders
via `${d.last_consolidation ? timeAgo(...) + ' (' + formatTime(...) + ')' : 'Not tracked yet'}`,
so the `Not tracked yet` fallback still shows on a brand-new state root with no
consolidation history. All server-supplied strings continue to pass through the
client `esc()` escaper before reaching `innerHTML`; the new
`recent_consolidation_activity` values are a numeric count and an RFC-3339
timestamp string, never raw report text.

### Failure behavior (no silent fallback)

If the cognitive store cannot be read, the existing `native_memory_error` field
carries the reason (unchanged behavior — the panel already surfaces *why* data
is missing rather than showing silent zeros). If no consolidation signal exists,
`last_consolidation` is `null` and the statistic reads `Not tracked yet` — an
honest "no data yet", never a fabricated or frozen timestamp. The endpoint stays
behind the dashboard's `require_auth` middleware; no new routes are added.

---

## Testing

Both fixes are covered by **hermetic** tests that run against an injected state
root (`HermeticState`) with no network and no real `gh`, serialized on the
cognitive-memory resource where the cognitive store is exercised
(`serial_test::serial(cognitive_memory)`).

### Fix 1 — Open PRs gone

- `/api/activity` (`activity()`) response has **no** `open_prs` key; the
  previous `open_prs` structure/array assertions are removed from
  `tests_activity.rs`, while `assigned_issues`, `daemon`, `recent_cycles`, and
  `timestamp` assertions remain.
- The rendered dashboard HTML contains **no** `open-prs-list` element and **no**
  `d.open_prs` client reference (the card markup and renderer are gone).
- The **Merge Readiness** card is unaffected: its `merge-readiness-card`
  markup renders and `/api/merge-readiness` still returns its own independent
  `open_prs` array.

### Fix 2 — Live memory consolidation

- `/api/memory` returns a `last_consolidation` that reflects an **injected**
  recent `consolidate-memory` cycle report, and a `recent_consolidation_activity`
  object with a matching `count` / `last`.
- The values **change** when the underlying state changes: injecting a newer
  `consolidate-memory` report (or advancing the cognitive store) moves
  `last_consolidation` forward and increments the activity count — proving the
  panel is not a constant/stale snapshot.
- With no consolidation history, `last_consolidation` is `null` and
  `recent_consolidation_activity.count` is `0` (the `Not tracked yet` path).
- A hostile `action_kind` value (e.g. containing `<script>`) is not rendered
  unescaped — counts stay numeric and timestamps stay RFC-3339.

---

## Related

- [Dashboard](../dashboard.md) — the parent dashboard how-to (Overview → Health,
  Resources → Memory).
- [Activity tab — Cycle Reports](./dashboard-activity-cycle-reports.md) — the
  sibling live-read fix (#26/#2697) this reconciles with; shares the newest-first
  cycle-report reader.
- [Goals tab lifecycle-status badges](./dashboard-goal-lifecycle-status.md) —
  the sibling live-read fix (#20/#2695).
- [Cognitive memory client helpers](./cognitive-memory-client-helpers.md) — the
  `open_reader_client` / `get_statistics()` path that supplies the live memory
  counts.
- [Memory architecture](../memory.md) — where consolidation fits in the memory
  model.
