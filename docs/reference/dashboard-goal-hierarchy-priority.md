---
title: Goals tab hierarchy & differentiated priorities
description: Reference for the dashboard Goals tab's parent→child hierarchy nesting and priority ordering. Sub-goals render nested under their decomposition parent (driven by the ActiveGoal.parent_goal_id back-reference), top-level entries and their children are ordered by priority (highest first), each goal shows a distinctly-coloured priority tier, and the goal-curation prioritization pass differentiates the priorities of undifferentiated goals while leaving operator-set priorities intact (via the additive priority_explicit provenance flag).
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./dashboard-goal-lifecycle-status.md
  - ./goal-decomposition.md
  - ./goal-board-api.md
  - ./simard-cli.md
  - ../howto/decompose-a-large-goal.md
---

# Goals tab hierarchy & differentiated priorities

Reference documentation for two coupled improvements to the dashboard
**Goals** tab's active-goals view:

1. **Hierarchy.** Sub-goals produced by decomposition now render **nested
   under their parent goal** (indented, grouped) instead of as a flat list,
   reflecting the parent→child structure already recorded by
   [`simard goal decompose`](./goal-decomposition.md).
2. **Differentiated priorities — both sides.**
   - **Display:** the tab **orders goals by priority (highest first)** and
     shows each goal's priority as a distinctly-coloured **tier** so priority
     is visible and actionable.
   - **Substance:** a deterministic **prioritization pass** in goal curation
     spreads the priorities of goals that were all left at the same default,
     so the board reflects real urgency/blocking signals instead of a flat
     wall of `p3`. Priorities the operator set explicitly are **never**
     reshuffled.

This builds directly on — and renders one coherent Goals tab together with —
the [lifecycle-status badges](./dashboard-goal-lifecycle-status.md) (#20 /
#2695) and [goal decomposition](./goal-decomposition.md) (#2405). Each active
goal row therefore shows **all three** axes at once: its **priority tier**,
its **position in the hierarchy**, and its **lifecycle status badge**.

## Why this exists

The operator reported two problems on the Goals tab:

- **The hierarchy was invisible.** `simard goal decompose <id>` breaks a large
  umbrella goal into 2–6 bounded sub-goals, records the parent↔child structure
  as real edges (see [goal decomposition](./goal-decomposition.md)), and — in
  the normal on-board case — **demotes the umbrella to the backlog** while the
  children take its place on the active board. But the Goals tab rendered a
  **flat** list: the children sat at the root as unrelated peers with the
  umbrella nowhere in sight, so the operator could not see which goals belonged
  to which decomposition.
- **Priorities were flat.** Almost every goal sat at the same value (many at
  `p3`) — which, as the operator noted, is effectively **no** prioritization.
  Two things caused this: the tab did not order by (or visually distinguish)
  priority, and, underneath, new/curated/decomposed goals were seeded at a
  single default (decomposition children even *inherit* the parent's priority,
  so a whole umbrella collapses to one value).

The fix addresses **display** and **substance** separately and additively,
and preserves the operator's explicitly-set priorities.

## What the operator sees

The active-goals table gains a **Priority tier** treatment on the existing
**Priority** column and renders the rows as a **priority-ordered tree**:

- **Top-level entries** — standalone goals and parent goals (each with its
  children grouped beneath it) — are ordered by priority, **highest first**.
- **Children** render **indented** directly under their parent — an active
  parent when the umbrella is still on the board, otherwise a header
  synthesized from the umbrella's **demoted backlog tracking node** (the normal
  post-`decompose` case) — and are themselves ordered by priority, highest
  first.
- **True orphans** — a `parent_goal_id` matching **neither** an active goal
  **nor** a backlog tracking node (a completed or tombstoned parent) — render
  at the **root** as ordinary top-level entries (see
  [Nesting rules](#nesting-rules)).

Each row's **Priority** cell shows a coloured **tier pill** with a plain
label:

| Priority value | Tier label | Tier key | Colour |
|----------------|------------|----------|--------|
| `≤ 1` | **Critical** | `critical` | red `#f85149` |
| `2` | **High** | `high` | orange `#db6d28` |
| `3` | **Medium** | `medium` | amber `#d29922` |
| `4` | **Low** | `low` | blue `#388bfd` |
| `≥ 5` | **Minimal** | `minimal` | grey `#8b949e` |

The raw numeric priority (e.g. `p2`) is shown alongside the tier label so
nothing is hidden. The lifecycle **Status** badge (from
[#20](./dashboard-goal-lifecycle-status.md)) is unchanged and continues to
render in the Status column.

> **Note on colour reuse.** The priority `Critical` red (`#f85149`) is the same
> hue the **Current Activity** chip uses for `Failed`, and the priority
> `Medium` amber (`#d29922`) matches the **Status** column's `Blocked` badge.
> This is intentional and unambiguous because the three live in **different
> columns** (Priority vs Status vs Current Activity) with their own labels;
> priority tiers form a self-consistent heat gradient (hotter = more urgent).

### Before / after

| Was | Now |
|-----|-----|
| Flat list: children like `agent-kgpacks-rs-ws24` rendered at the root as unrelated peers; the demoted `umbrella-supply-chain` was not shown above them | Child rows render **indented under** an `umbrella-supply-chain` header (resolved from its backlog tracking node), grouped |
| Priority column printed a bare number, rows in arbitrary/insertion order | Rows **ordered highest-priority-first**; each priority shown as a coloured **tier pill** (Critical/High/Medium/Low/Minimal) |
| Board dominated by `p3` (default + inherited) | Curation **spreads** the undifferentiated goals across `p1…p5`; operator-set values untouched |

## Nesting rules

Grouping is driven by the **denormalized `parent_goal_id` back-reference** on
each [`ActiveGoal`](./goal-board-api.md) — the always-present, structured field
`simard goal decompose` writes on every child (see
[goal decomposition → data model](./goal-decomposition.md#data-model)). The
render path groups by this field directly; it does **not** parse descriptions
and does **not** issue a cognitive-memory graph query on every dashboard poll
(guideline **G3**: prefer structured goal-graph data over brittle parsing).

**The parent that heads a group is usually _not_ an active goal.** The normal
`decompose` path (`ChildPlacement::Board`) **removes the umbrella from the
active board and demotes it to the backlog** as a tracking node
(`source: "decompose-parent"`, same id), while promoting the children onto the
active board. So a child's `parent_goal_id` typically resolves to a **backlog**
entry, not an active goal. The render path therefore resolves the parent-group
**header from the already-returned `backlog` array** (matching `id`) — no extra
route, no extra query — so the umbrella stays visible above its children.

| Goal shape | Renders |
|------------|---------|
| `parent_goal_id == None` (never decomposed) | **Root**, top-level |
| `parent_goal_id` matches a goal still **active** on the board (e.g. a re-promoted umbrella) | **Nested** under that active parent |
| `parent_goal_id` matches a **demoted parent in the `backlog` set** — the normal `ChildPlacement::Board` post-`decompose` case (`decompose` removes the umbrella from `active`, pushes it to the backlog as a `decompose-parent` tracking node, and promotes the children onto `active`) | **Nested** under a parent-group **header synthesized from that backlog tracking node** (same id, its description) |
| `parent_goal_id` matches **neither** an active goal **nor** a backlog tracking node (a completed or tombstoned parent) | **Root**, top-level (promoted so it is never hidden) |

> **Overflow (`ChildPlacement::Backlog`) case.** When there is no room to
> promote the children, `decompose` does the opposite: the **umbrella stays
> active** and the **children overflow to the backlog**. Those children are not
> in the active set, so there is nothing to nest — the umbrella simply renders
> as a normal active top-level row, and its children appear in the backlog list.

**Cycle- and depth-safe.** Nesting walks the `parent_goal_id` links with a
**visited-set and a bounded depth cap**, so a malformed or cyclic
`parent_goal_id` chain (`a → b → a`) can never cause an infinite loop or
unbounded indentation; any node that would revisit an ancestor is rendered at
the root instead.

## Ordering rules

Priority is **ascending numeric** = highest-importance first (the goal-board
persistence validator `validate_active_goal` → `validate_priority` requires
`priority ≥ 1`; **lower number = higher priority**, consistent with the
existing goal-board merge sort key — see
[goal board API](./goal-board-api.md)). Ordering holds at **every** level:

1. **Top-level entries** (standalone goals and parent groups) are ordered by a
   **representative priority**:
   - if the parent is still an **active** goal, its representative is the
     **parent goal's own priority**;
   - for a **demoted-parent group** — the normal case, where the parent is only
     a backlog tracking node that carries a `score`, **not** a `priority` — the
     representative is the **minimum (highest-importance) child priority**.
2. **Children within a parent** are ordered by their own priority.
3. **Tie-break** at any level is the stable goal `id` (lexicographic), so the
   order is deterministic.

The `/api/goals` view builder (`goals_at`) emits the active array already
sorted (priority ascending, `id` tie-break) — a **read-only ordering of the
response**, not a persisted mutation, so it stays consistent with "the
dashboard never mutates goals on a poll". The front end preserves that order
while grouping.

## API: `/api/goals`

The change is **additive**. Each active-goal object returned by
`GET /api/goals` carries two new fields alongside the existing ones
(including the [`status_progress`](./dashboard-goal-lifecycle-status.md#api-the-status_progress-field)
field from #20), and the `active` array is returned **sorted by priority
ascending** (`id` tie-break).

| Field | Type | Meaning |
|-------|------|---------|
| `parent_goal_id` | `string \| null` | The id of this goal's decomposition parent, or `null`/absent for a top-level goal. Drives hierarchy nesting. |
| `priority_explicit` | `bool` | `true` when the operator set this goal's priority explicitly (via `simard goal set-priority`); `false` (default) for seeded / auto / inherited / curated priorities. The prioritization pass only re-scores goals where this is `false`. |

```jsonc
{
  "active": [
    {
      "id": "umbrella-supply-chain",
      "description": "Supply-chain audit across all governed repos",
      "priority": 1,
      "priority_explicit": true,        // NEW: operator pinned this to p1
      "parent_goal_id": null,           // NEW: top-level umbrella
      "status": "in-progress(20%)",
      "status_progress": { "InProgress": { "percent": 20 } },
      "assigned_to": null,
      "repo": "rysweet/Simard",
      "current_activity": null,
      "status_chip": "Working",
      "detail": "",
      "detail_full": "",
      "wip_refs": []
    },
    {
      "id": "agent-kgpacks-rs-ws24",
      "description": "Audit kgpacks-rs supply chain",
      "priority": 2,
      "priority_explicit": false,       // NEW: derived, eligible for the pass
      "parent_goal_id": "umbrella-supply-chain",  // NEW: nests under the umbrella
      "status": "not-started",
      "status_progress": "NotStarted",
      "status_chip": "Waiting",
      "wip_refs": []
    }
  ],
  "backlog": [ /* … unchanged … */ ],
  "active_count": 20
}
```

> **On the example above.** For readability this shows the umbrella
> `umbrella-supply-chain` **in `active`**. In the common post-`decompose`
> (`ChildPlacement::Board`) state the umbrella instead lives in the returned
> **`backlog`** array as a `decompose-parent` tracking node, and the child's
> `parent_goal_id` resolves there — the render path reads the group header from
> `backlog` (see [Nesting rules](#nesting-rules)). An umbrella appears in
> `active` only when it is (re-)promoted onto the board.

### Compatibility

- **Additive only.** Every pre-existing field (`id`, `description`,
  `priority`, `status`, `status_progress`, `assigned_to`, `repo`,
  `current_activity`, `status_chip`, `detail`, `detail_full`, `wip_refs`) is
  unchanged. `parent_goal_id` and `priority_explicit` are new; existing
  consumers keep working.
- **No route, auth, or schema change.** Both new fields ride the same
  authenticated `GET /api/goals` behind `require_auth`. No new route and no
  `?token=` query parameter are introduced.
- **`parent_goal_id` is not new persistence.** It was already an
  `#[serde(default, skip_serializing_if = "Option::is_none")]` field on
  `ActiveGoal` (issue #2405); `/api/goals` now simply **surfaces** it.
- **`priority_explicit` is a `#[serde(default)]` bool.** Legacy goal-board
  snapshots and `goal_records.json` files that predate this field deserialize
  cleanly to `false` (eligible for the pass); they re-serialize byte-identical
  when the flag is unset (`skip_serializing_if` on the `false` default).

### Ordering on the wire (`add_goal_at` write path)

The `POST /api/goals` create handler (`add_goal_at`) continues to accept a
`priority` and now:

- **Validates it through the existing persistence path.** Rather than pushing a
  hand-built `ActiveGoal` straight onto `board.active` (as it does today), the
  handler routes the new goal through the public
  [`add_active_goal`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/operations.rs)
  board mutation, which runs `validate_active_goal` → `validate_priority`
  (`priority ≥ 1`) before the goal lands on the board. `validate_priority` and
  `validate_active_goal` are **module-private** helpers in
  `goal_curation::operations`; the dashboard reuses them **via** the public
  `add_active_goal` entry point — it does not call those private helpers (or a
  non-existent "shared" `validate_priority`) directly.
- **Server-derives provenance:** a goal created through the dashboard is
  written with `priority_explicit = false` regardless of any client-supplied
  value — the client cannot set the provenance flag. Only the
  `simard goal set-priority` CLI path marks a priority explicit (see
  [CLI](#cli-simard-goal-set-priority)).

## Model: `ActiveGoal.priority_explicit`

`src/goal_curation/types.rs` gains one additive provenance field on
[`ActiveGoal`](./goal-board-api.md):

```rust
pub struct ActiveGoal {
    pub id: String,
    pub description: String,
    pub priority: u32,
    // … existing fields (status, assigned_to, repo, current_activity,
    //    wip_refs, last_progress_update_at, parent_goal_id) …

    /// True only when the operator set this goal's priority explicitly
    /// (via `simard goal set-priority`). Default `false` for every other
    /// origin: default seeding, meeting/curation, creative-idea promotion,
    /// dashboard create, and decomposition-inherited child priorities.
    ///
    /// The goal-curation prioritization pass differentiates ONLY goals where
    /// this is `false`, so an operator's deliberate priority is never
    /// reshuffled. `#[serde(default)]` keeps pre-existing snapshots loading
    /// (as `false`); `skip_serializing_if` keeps them byte-identical when the
    /// flag is unset.
    #[serde(default, skip_serializing_if = "is_false")]
    pub priority_explicit: bool,
}
```

A builder mirrors the existing `with_repo` / `with_parent` builders:

```rust
let g = ActiveGoal::new("triage-flaky-tests", "Triage flaky test suite", 3)
    .with_priority_explicit(true);   // operator pinned it
```

`ActiveGoal::new` continues to default `priority_explicit` to `false`, so
every existing construction site compiles unchanged and every
non-operator-created goal is eligible for the prioritization pass.

## The prioritization pass

The **substance** half of the fix is a pure, deterministic function in the new
module `src/goal_curation/prioritize.rs` (re-exported from
`goal_curation/mod.rs`). It differentiates undifferentiated priorities so the
board stops collapsing to a single value.

### Contract

```rust
/// Re-score the priorities of goals whose priority was NOT explicitly set by
/// the operator, spreading them across a bounded band so priority is
/// meaningful instead of flat. Deterministic and side-effect-free: given the
/// same goals, signals, and `now`, it always produces the same result.
///
/// Invariants:
///   * Goals with `priority_explicit == true` are returned UNCHANGED.
///   * Every returned priority is `>= 1` (never violates `validate_priority`).
///   * Priorities land in the band `1..=5`.
///   * Input order and goal identity are preserved — the pass rewrites
///     `priority` only; the display layer sorts.
pub fn prioritize(
    goals: &[ActiveGoal],
    signals: &PrioritizationSignals,
    now: DateTime<Utc>,
) -> Vec<ActiveGoal>;
```

`now` is **injected** (not read from the wall clock) so the pass is
deterministic and hermetically testable. `PrioritizationSignals` carries the
structured goal-graph facts the pass reads — chiefly the `depends_on` edges
(`edges_of_type(mem, GoalEdgeType::DependsOn, …)`) used to detect
blocking relationships — so the pass reasons over **structured edges, not
parsed text** (G3).

### Signals (deterministic, bounded)

Only goals with `priority_explicit == false` are ranked. Each eligible goal is
scored from a small set of bounded, deterministic signals, then the ranked set
is mapped into the `p1…p5` band so priorities **spread** rather than collapse:

| Signal | Effect on urgency | Source |
|--------|-------------------|--------|
| Goal **blocks others** (is the `to` of others' `depends_on`) or has **unmet `depends_on`** of its own | **Higher** (nearer `p1`) | `depends_on` edges in the goal graph |
| Goal has **in-flight work** (`wip_refs` non-empty: open PR / branch / session) | **Higher** | `ActiveGoal.wip_refs` |
| Lifecycle `status` is `Blocked` or `InProgress` | **Higher** | `ActiveGoal.status` (`GoalProgress`) |
| Goal is **standing/perpetual** (`is_perpetual()`) | **Slightly lower** (steady background work, not a spike) | `ActiveGoal` standing marker |
| **Stale** — `last_progress_update_at` is old (or absent) relative to `now` | **Higher** (needs attention) | `ActiveGoal.last_progress_update_at` + injected `now` |

The weighted score produces a **rank**, and ranks are distributed across the
band `1..=5`, so a set of ten identical `p3` goals emerges spread across the
band instead of all staying at `p3`. Explicit-priority goals are **excluded
from the ranking entirely** and returned verbatim.

### Where the pass runs (off the render path)

The pass is applied on the **curation / goal-add write path**, never at render
time — the dashboard never mutates goals on a poll:

- **Post-decomposition** (`goal_curation/decompose.rs`), after the children are
  pushed onto the active board (they otherwise **inherit the parent's single
  priority**, which is exactly the flat-siblings case), the pass differentiates
  the freshly-created `priority_explicit == false` children from the structured
  inter-sibling `depends_on` ordering the decomposer emitted. This is the live
  `simard goal decompose` path.
- `prioritize` is a reusable, side-effect-free primitive, so any other curation
  write path that accumulates undifferentiated goals can apply it before
  persisting on the same goal-store writer.

Because the write goes through the same cognitive-memory / goal-store writer as
`goal add` / `goal remove`, the re-scored priorities are durable and are what
`simard goal list` and `/api/goals` both read back — the dashboard and CLI
**agree by construction**.

## CLI: `simard goal set-priority`

`simard goal set-priority <goal-id> <p>` (and its alias `goal reprioritize`)
is the **only** writer that marks a priority explicit. When the operator sets a
priority, the handler sets `priority = <p>` **and** `priority_explicit = true`,
so the prioritization pass will thereafter leave that goal alone.

```console
$ simard goal set-priority triage-flaky-tests 1
[simard] goal set-priority: 'triage-flaky-tests' changed from p3 to p1

# The goal is now pinned: the curation prioritization pass will never
# re-score it, and the Goals tab shows it in the Critical tier at the top.
```

No other path (default seeding, meeting curation, creative-idea promotion,
dashboard create, or decomposition inheritance) sets `priority_explicit`, so
those goals stay eligible for differentiation.

## Rendering pipeline (front end)

The Priority column and the tree layout are rendered client-side in
[`index_html`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/index_html/)
from three cooperating pieces, deliberately mirroring the lifecycle-status
trio (`humanizeGoalProgress` / `goalLifecycleKey` / `GOAL_STATUS_COLORS`):

1. **`humanizePriority(priority)`** — maps a numeric priority to a plain-text
   tier label: `≤1` → `Critical`, `2` → `High`, `3` → `Medium`, `4` → `Low`,
   `≥5` → `Minimal`. Returns **plain text only** (escape-last invariant).

2. **`priorityTierKey(priority)`** — maps the numeric priority to one canonical
   tier key — `critical` · `high` · `medium` · `low` · `minimal` — keyed on the
   **integer band only**. It never inspects any goal-supplied text, so no
   attacker-controlled string can change which colour is chosen (G3).

3. **`GOAL_PRIORITY_COLORS`** — a hard-coded allowlist from tier key to colour:

   | Key | Colour |
   |-----|--------|
   | `critical` | `#f85149` (red) |
   | `high` | `#db6d28` (orange) |
   | `medium` | `#d29922` (amber) |
   | `low` | `#388bfd` (blue) |
   | `minimal` | `#8b949e` (grey) |

The Priority cell renders a pill whose **label** is
`esc(humanizePriority(g.priority)) + ' (p' + g.priority + ')'` and whose
**colour** is `GOAL_PRIORITY_COLORS[priorityTierKey(g.priority)]`.

The tree layer groups `d.active` by `parent_goal_id`, indents children under
their parent — an **active** parent when the umbrella is still on the board,
otherwise a header synthesized from the matching **demoted parent in
`d.backlog`** (`source: "decompose-parent"`, matched by `id`) — promotes only
**true orphans** (a `parent_goal_id` in neither set) to the root
(cycle-/depth-safe), and preserves the priority-ascending order the backend
already emitted (see [Ordering rules](#ordering-rules)).

### Security invariants

- **Output encoding, escape-last.** Every new field (`parent_goal_id`, tier
  label) is passed through `esc()` / `escAttr()` **last**, so goal-controlled
  text cannot inject markup.
- **Colour from the allowlist only.** Pill colour comes exclusively from the
  hard-coded `GOAL_PRIORITY_COLORS` map keyed by `priorityTierKey`. Goal data
  is never interpolated into a `style=` sink.
- **Priority stays numeric.** `priorityTierKey` operates on the integer band;
  the raw priority is rendered as a number, never as HTML from a string sink.
- **Structured over brittle (G3).** Nesting reads the structured
  `parent_goal_id` field, and the pass reads typed `depends_on` edges — not a
  parsed Display string.
- **Cycle-/depth-safe nesting.** A visited-set and depth cap bound the tree
  walk, so a hostile or cyclic `parent_goal_id` cannot hang or overflow the
  render.
- **Same auth surface.** No new route and no `?token=` parameter; everything
  stays behind `require_auth`. Server-derived `priority_explicit` on create
  means a client cannot forge provenance.

## Tests

Hermetic tests pin the behaviour end to end:

- **Prioritization pass (`src/goal_curation/tests_prioritize.rs`)** — given a
  set of **undifferentiated** goals (all `p3`, `priority_explicit == false`),
  `prioritize(..)` returns **differentiated** priorities spread across the
  `1..=5` band; a goal with `priority_explicit == true` in the same set is
  returned **unchanged**. With `now` injected, the output is deterministic.
- **API (`src/operator_commands_dashboard/tests_goals_crud.rs`)** — `/api/goals`
  emits `parent_goal_id` and `priority_explicit` per goal and returns the
  `active` array **sorted by priority ascending**; `add_goal_at` bounds the
  priority (`validate_priority`) and writes `priority_explicit = false`
  regardless of the client-supplied value.
- **Rendering (`src/operator_commands_dashboard/index_html/tests_tab_meta.rs`)**
  — `INDEX_HTML` defines `humanizePriority(`, `priorityTierKey(`, and
  `GOAL_PRIORITY_COLORS` (with the five tier colours), groups by
  `parent_goal_id` (nested children render indented), **resolves the umbrella
  header from the `backlog` array** when the parent was demoted
  (`source: "decompose-parent"`), and orders rows by priority — proving the tab
  shows a priority-ordered hierarchy with visible tiers even when the parent is
  no longer active. The new JS helpers are added to the `tests_tab_meta.rs`
  allowlist.
- **Reconciliation** — the rendered hierarchy and priority order match the
  underlying goal store, so the dashboard view and `simard goal list` agree.

## Related

- [Goals tab lifecycle-status badges](./dashboard-goal-lifecycle-status.md) —
  the sibling **Status** column this view renders alongside (#20 / #2695)
- [Goal decomposition & the goal graph](./goal-decomposition.md) — where
  `parent_goal_id` and the `decomposes_into` / `depends_on` edges come from
  (#2405)
- [Goal board API](./goal-board-api.md) — `ActiveGoal`, `GoalProgress`, the
  priority-ascending sort key, and `validate_priority`
- [Dashboard](../dashboard.md) — the Goals tab in context
- [How to decompose a large goal](../howto/decompose-a-large-goal.md) —
  producing the parent→child structure this view renders
