---
title: Dashboard live goal-board read
description: >
  How the dashboard Goals tab and the Memory tab's "Goal Records" tile read the
  LIVE goal state instead of the periodically-rewritten `goal-board:snapshot`
  fact. Documents the root cause (a snapshot the daemon rewrites only ~once per
  OODA cycle, so creative-idea Proposed goals and other cognitive-memory goal
  records lag up to ~5 minutes), the shipped fix — a single fail-closed live
  builder `dashboard_live_goal_board` that unions the snapshot-fact board with a
  live `CognitiveMemoryGoalStore::list()` overlay, deduped by slug (board wins) —
  the new reverse adapter `record_as_active_goal`, the repointed `GET /api/goals`
  and `GET /api/memory` read paths, the fail-closed contract (no silent fallback
  to stale data), the preserved `{active, backlog, active_count, backlog_count}`
  shape, the TUI parity, and the regression tests. Fixes issue #2922.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: current — shipped; the read path is live, the per-cycle snapshot WRITE is
  retained for history/other consumers
related:
  - ./goal-board-api.md
  - ./dashboard-goal-lifecycle-status.md
  - ./dashboard-goal-hierarchy-priority.md
  - ./creative-ideas-trigger-scoped-read.md
  - ./creative-ideas-goal-routing-fail-closed.md
  - ./cognitive-memory-goal-store.md
  - ../dashboard.md
---

# Dashboard live goal-board read

The dashboard **Goals** tab renders the goal register: active top-N goals with
priority, status, and current activity, plus the proposed backlog. On a **live**
brain, promoting a creative idea (or unblocking a goal, or any mutation that
persists a goal record) **did not show up on the board until the next OODA cycle**
— a lag of up to ~5 minutes. The `POST /api/creative-ideas/{id}/promote` call
returned the newly-persisted **Proposed** goal, yet `GET /api/goals`
(`active` / `backlog`) and the Memory tab's **Goal Records** count stayed
unchanged until the daemon happened to rewrite its snapshot.

This page documents the shipped fix. The board **read** is now **live**: both
`GET /api/goals` and the `goal_records` tile in `GET /api/memory` read a union of
the authoritative snapshot-fact board **and** a live `CognitiveMemoryGoalStore`
overlay, so a goal record appears the instant it is persisted — no snapshot cycle
required. The per-cycle snapshot **write** is unchanged (it is still useful as a
durable history/summary artifact); only the READ moved off the stale fact.

!!! note "Status — shipped; fail-closed (#2922)"
    The fix is implemented and green in CI: the live builder
    `dashboard_live_goal_board`, the reverse adapter `record_as_active_goal`,
    the repointed `goals_at` / `memory_metrics` read paths, and the regression
    tests all land against the real code seams. The read is **fail-closed**:
    there is no fallback to the stale snapshot that could mask a live-read
    failure — any leg error surfaces as an explicit error payload, never as
    silently-stale or partial data. Additive only (no response-shape change). No
    type or method contains the word `Bridge` (operator preference); no stray
    `println!` / `eprintln!` (tracing only).

---

## Root cause — the read was pinned to a periodically-written cache

Two goal-writing paths exist, and they land in **different** places:

| Writer | Persists to | Freshness on the board (before #2922) |
|---|---|---|
| OODA daemon `commit_cycle`, and every dashboard/CLI operator mutation (add / promote / unblock / remove) via `dashboard_save_goal_board*` | the `goal-board:snapshot` fact (`load_goal_board` / `save_goal_board`) | operator mutations were read-your-writes through the snapshot; **daemon-authored** changes refreshed only when the cycle rewrote the fact (~once / ~5 min) |
| Creative-ideas promote, meeting-derived goals, runtime sessions, seed | a `goal-store:record` fact via `CognitiveMemoryGoalStore::put` (see [Cognitive-Memory Goal Store](./cognitive-memory-goal-store.md)) | **never** on the board until the daemon's next OODA cycle folded the record into the board and rewrote the snapshot |

The dashboard read used only the first source:

```
READ PATH (before #2922) — snapshot-only, stale for goal-store records
----------------------------------------------------------------------
GET /api/goals
  goals_at(state_root)
    dashboard_goal_board_snapshot(state_root).unwrap_or_default()
      open_reader_client(state_root) -> load_goal_board(reader.ops())
        read_latest_snapshot("goal-board:snapshot")   <-- rewritten ~1×/cycle

GET /api/memory  (goal_records tile)
  memory_metrics()
    dashboard_goal_board_snapshot(state_root).ok()
      => goal_count = board.active.len() + board.backlog.len()
```

A creative-idea promote writes a **Proposed** `goal-store:record`, which the
snapshot fact does not contain, so `goals_at` and the `goal_records` count could
not see it until the next cycle. Two further defects compounded the staleness:

* **`unwrap_or_default()` masked failures.** A live-read error degraded silently
  to an empty board (`GoalBoard::default()`), so an outage looked like "no
  goals" rather than an error — a fail-open behavior #2922 removes.
* **Raising the snapshot cadence is not a fix.** It only shortens the lag window
  and cannot make creative-idea `put`s appear immediately; those records are not
  in the snapshot at all.

## The fix — one fail-closed live builder, union of two live sources

The read path now goes through a single helper, `dashboard_live_goal_board`,
that builds the board from **two live sources** and dedupes them:

```
READ PATH (after #2922) — live union, fail-closed
-------------------------------------------------
GET /api/goals / GET /api/memory
  dashboard_live_goal_board(state_root)  -> SimardResult<GoalBoard>
    base    = dashboard_goal_board_snapshot(state_root)?        // snapshot-fact board (operator RYOW)
    overlay = CognitiveMemoryGoalStore::new(state_root)?.list()? // LIVE goal-store records (#2896 fail-closed)
    union:  base.active/base.backlog
            + record_as_active_goal(record) for each overlay record
              whose slug is NOT already present in base   // board wins on conflict
```

* **Base** — the authoritative snapshot-fact board via
  `dashboard_goal_board_snapshot`. This already gives operator dashboard/CLI
  mutations read-your-writes (they persist the snapshot synchronously) and
  carries the daemon-OODA active/backlog goals with their rich fields
  (`current_activity`, `wip_refs`, `repo`, `parent_goal_id`,
  `priority_explicit`, `labels`).
* **Overlay** — the **live** `CognitiveMemoryGoalStore::list()` records. This is
  the same shared live store the creative-ideas / meeting / runtime / seed
  writers `put` into, read through the same reader tiers the daemon writer
  serves. `list()` is already **fail-closed** (issue #2896 —
  `list_via_reader` propagates a transport fault as `Err`, never a masked empty),
  so the overlay never coerces an outage into "no goals".
* **Dedup by slug, board wins.** A record whose `slug` already appears in the
  base board (active or backlog) is dropped — the base carries the richer,
  authoritative fields. The base slug set is built with `goal_slug(active.id)` /
  `goal_slug(item.id)` — the **same** slug function the forward
  `active_goals_as_records` adapter uses — because an `ActiveGoal.id` is not
  guaranteed to already be a slug; comparing raw ids would let a slugged overlay
  record slip past dedup and double-render. Only records *absent* from the base
  (e.g. a freshly promoted creative-idea Proposed goal) are added. Single-pass
  `O(n)` over the pre-built slug set; the base board's flock/read is released
  before the overlay read.

Because the overlay is read live on every request, a promoted creative-idea
Proposed goal, an unblock, or any other `goal-store:record` write appears on the
board **immediately**, with no dependence on the snapshot cadence.

### New reverse adapter — `record_as_active_goal`

Module `simard::goal_curation` (`src/goal_curation/operations.rs`). It is the
inverse of the existing `active_goals_as_records` (which maps board → records);
`record_as_active_goal` maps a persisted `GoalRecord` back into the board's
`ActiveGoal` / `BacklogItem` shapes so overlay records render identically to
snapshot goals. Pure struct-mapping — panic-free on arbitrary record text, and
it synthesizes absent rich fields as `None` / `[]` so the JSON stays byte-stable.

Status routing (mirrors the board's own active/backlog split and the lifecycle
in [Dashboard Goal Lifecycle-Status Badges](./dashboard-goal-lifecycle-status.md)):

| `GoalRecord.status` (`GoalStatus`) | Placement | Rendered `GoalProgress` |
|---|---|---|
| `Active` | `active[]` | `InProgress { percent: 0 }` |
| `Proposed` | `backlog[]` | — (Proposed record → backlog item) |
| `Paused` | `backlog[]` | — |
| `Completed` | **skipped** | terminal; not surfaced on the live board |

Field synthesis when mapping a record into an `ActiveGoal`:

| `ActiveGoal` field | From `GoalRecord` |
|---|---|
| `id` | `record.slug` |
| `description` | `record.title` |
| `priority` | `record.priority as u32` |
| `status` | per the status table above |
| `assigned_to` | `Some(record.owner_identity)` (`None` when `"unassigned"`) |
| `labels` | `record.labels` (carries `source:creative-ideas` etc.) |
| `repo`, `current_activity`, `parent_goal_id` | `None` |
| `wip_refs` | `[]` |
| `priority_explicit` | `false` |

When mapping a Proposed/Paused record into a `BacklogItem`: `id = record.slug`,
`description = record.title`, `source` is a plain-English provenance label
derived from the record's `source:*` label (e.g. `"From creative ideas"`), and
`score` is synthesized deterministically from `record.priority` (higher priority
→ higher score) so backlog ordering is stable.

The primary #2922 case — a promoted creative idea — persists a **Proposed**
`goal-store:record`, so it lands in `backlog[]` and shows up on the Goals tab's
proposed backlog the instant the promote returns.

#### Round-trip fidelity (inherent, not a regression)

A `GoalRecord` persists only the four `GoalStatus` values and a small field set,
so mapping a record **back** into an `ActiveGoal` cannot recover the finer state
a snapshot-authored goal carries. Because the forward `active_goals_as_records`
already collapses `GoalProgress::InProgress` / `NotStarted` / `Blocked` →
`GoalStatus::Active` and does not persist `percent` or `current_activity`, an
`Active` overlay record renders as `InProgress { percent: 0 }` with
`current_activity = None`; likewise `title` is the record's ≤120-char first line
and `rationale` is not restored. This loss is confined to overlay records that
are **not** already on the base board — snapshot goals keep their rich fields
because the base wins on dedup. The lossiness is inherent to the record schema,
so no code change removes it; it is documented here for accuracy.

## Dashboard HTTP contract (unchanged shape)

### `GET /api/goals`

Shape is **byte-identical** to before #2922 — the meeting-fact backlog
enrichment and the priority-ASC active ordering (p1 highest first, id tiebreak)
are preserved; only the data is now live:

```json
{
  "active": [
    {
      "id": "…",
      "description": "…",
      "priority": 2,
      "parent_goal_id": null,
      "priority_explicit": false,
      "status": "active",
      "status_progress": { "InProgress": { "percent": 40 } },
      "assigned_to": "…",
      "repo": null,
      "current_activity": "…",
      "status_chip": "…",
      "detail": "…",
      "detail_full": "…",
      "wip_refs": []
    }
  ],
  "backlog": [
    { "id": "improve-recall-precision", "description": "Improve recall precision", "source": "From creative ideas", "score": 0.8 }
  ],
  "active_count": 1,
  "backlog_count": 1
}
```

`active_count` / `backlog_count` reflect the **live union**, so a newly promoted
Proposed goal both appears in `backlog` and increments `backlog_count` on the
very next poll.

### `GET /api/memory` — `goal_records` tile

The `goal_records` count is derived from the same live union (active + backlog),
and the `source` label is relabeled off the snapshot to reflect the live read.
An additive `error` field surfaces a live-read failure; on error the count is
**excluded** from `total_facts` rather than contributing a stale or zero value:

```json
{
  "goal_records": {
    "source": "cognitive-memory:live-goal-board",
    "count": 7
  }
}
```

```json
{
  "goal_records": {
    "source": "cognitive-memory:live-goal-board",
    "count": 0,
    "error": "goal-board read failed"
  }
}
```

## Fail-closed contract

The read path is **additive and fail-closed** — there is no fallback to the
stale snapshot and no path that silently serves empty/partial data on failure:

* `goals_at` and `memory_metrics` no longer call `unwrap_or_default()` on the
  board read. Either leg failing (snapshot base read **or** goal-store overlay
  `list()`) is surfaced, not swallowed.
* On failure, `GET /api/goals` returns an explicit fail-closed payload with
  zeroed counts and an `error` field — never a snapshot/stale board:

  ```json
  { "active": [], "backlog": [], "active_count": 0, "backlog_count": 0, "error": "goal-board read failed" }
  ```

  The error message is **generic**; the underlying error chain (paths, env) is
  logged server-side via `tracing` only, never returned to the client and never
  emitted via `println!` / `eprintln!`.
* **The Goals tab renders the `error` field as an explicit failure state.** An
  empty `active`/`backlog` *with* `error` set means the live read failed — it is
  not "no goals". The frontend surfaces it distinctly (banner/toast) so a
  fail-closed outage is never visually indistinguishable from a legitimately
  empty board; otherwise the client would fail-open at the UI layer even though
  the handler failed closed.
* The overlay uses `CognitiveMemoryGoalStore::list()`, which is already
  fail-closed (#2896): a reader-open or `search_facts` transport fault
  propagates as `Err`. The dashboard **must not** substitute the snapshot to
  paper over such a fault — that would reintroduce exactly the silent staleness
  #2922 removes.
* The per-cycle snapshot **write** (`overwrite_memory_cache` on the daemon) is
  **retained** as a durable history/summary artifact. It is no longer on any
  dashboard READ path.

## Terminal UI parity

The TUI Goals view (`src/bin/simard_tui/goals.rs`, `read_goal_board`) applies the
same live union so the terminal board matches the web dashboard — a promoted
creative-idea Proposed goal appears in `monitor-simard-with-tui` without waiting
for a snapshot cycle. This TUI parity is the **secondary, separately-shippable**
slice of #2922: it may land in a follow-up behind an explicit flag without
holding up the web-dashboard read fix, which is the primary deliverable. (Today
`read_goal_board` still calls `unwrap_or_default()`; the live union replaces it
when this slice ships.) See [simard-tui Dashboard](./simard-tui.md).

## Regression tests

* **`src/operator_commands_dashboard/tests_goals_crud.rs`** — the existing goal
  CRUD/route tests stay green (the response shape and operator round-trips are
  unchanged).
* **`src/operator_commands_dashboard/tests_goal_records_migration.rs`** — the
  `goal_records` tile keeps working; the count now comes from the live union.
* **`src/goal_curation/operations.rs` (adapter unit tests)** —
  `record_as_active_goal` status mapping (Active → active, Proposed/Paused →
  backlog, Completed → skipped), absent-field synthesis, and panic-free mapping
  of arbitrary record text.
* **Live-read test (the #2922 acceptance)** — using a real
  `CognitiveMemoryGoalStore` + snapshot base: persist a **Proposed** goal record
  (as a creative-idea promote does) and assert it appears in `goals_at(...)`
  output **and** increments the `goal_records` count **without** writing a new
  snapshot; then force a live-read failure and assert the handler surfaces an
  explicit `error` payload rather than serving a stale/empty board.
* **`tests/e2e-dashboard/specs/goals.spec.ts`** — the Playwright board rendering
  stays green.

## Operator verification (live)

Verify the fix in the running system after a redeploy:

1. Deploy the built binary (the read change ships in the daemon **and** the
   dashboard process).
2. Promote a creative idea — press **Promote** on the Creative Ideas tab
   (`POST /api/creative-ideas/{id}/promote`), which persists a **Proposed**
   goal record.
3. **Immediately** (do not wait ~5 minutes) reload the Goals tab or `curl` the
   endpoints:

   ```console
   $ curl -s http://127.0.0.1:PORT/api/goals   | jq '.backlog_count, (.backlog | map(.description))'
   $ curl -s http://127.0.0.1:PORT/api/memory  | jq '.goal_records'
   ```

   The promoted goal is present in `backlog` and `backlog_count` /
   `goal_records.count` reflect it on the first poll — no snapshot cycle
   required. See also
   [Diagnose lost Creative-Ideas goals](../howto/diagnose-lost-creative-ideas-goals.md).

## Constraints honoured

Additive (new builder + new reverse adapter; no signature churn on existing
callers, no response-shape change) · fail-closed (no snapshot fallback on a
live-read failure; no silent empty/partial data) · snapshot **write** unchanged
· no `*Bridge` names · no `println!` / `eprintln!` (tracing only) · CI green ·
never `--admin` / `--no-verify` · references **#2922**.

## See also

- [Goal-Board API reference](./goal-board-api.md) — the `GoalBoard` /
  `ActiveGoal` / `BacklogItem` shapes and the snapshot persistence seam.
- [Cognitive-Memory Goal Store](./cognitive-memory-goal-store.md) — the
  `CognitiveMemoryGoalStore` overlay source (`goal-store:record` facts).
- [Creative-Ideas Goal Routing — Fail-Closed Persistence](./creative-ideas-goal-routing-fail-closed.md)
  — how a promoted idea persists its Proposed goal (#2896).
- [Creative Ideas Trigger-Scoped Read](./creative-ideas-trigger-scoped-read.md)
  — the sibling live-read fix on the Creative Ideas tab (#122).
- [Dashboard Goal Lifecycle-Status Badges](./dashboard-goal-lifecycle-status.md)
  and [Dashboard Goal Hierarchy & Priorities](./dashboard-goal-hierarchy-priority.md)
  — how the Goals tab renders status and ordering.
- [Dashboard](../dashboard.md) — the operator-facing dashboard overview.
