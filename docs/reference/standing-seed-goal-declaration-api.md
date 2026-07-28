---
title: Standing seed-goal declaration API reference
description: Reference for declaring a seed goal standing/perpetual declaratively — the `standing: bool` field on `SeedGoal` (src/identity/manifest.rs) and `TomlSeedGoal` (src/identity/toml_types.rs), the seed→ActiveGoal marker application in `seed_board_from_seed_goals`, and the idempotent load-time `reconcile_standing_markers` self-heal that stamps the standing marker onto already-persisted goals (#4927).
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/perpetual-goal-no-progress-exemption.md
  - ../concepts/identity-scoped-cognition.md
  - ../concepts/pluggable-identity.md
  - ./no-progress-breaker-api.md
  - ./no-progress-breaker-storm-suppression-api.md
  - ./standing-research-goal-novelty-directive-api.md
  - ./goal-board-api.md
  - ../howto/declare-a-standing-seed-goal.md
  - ../howto/diagnose-a-no-progress-breaker-issue-storm.md
  - ../howto/configure-pluggable-identity.md
  - ../../src/identity/manifest.rs
  - ../../src/identity/toml_types.rs
  - ../../src/identity/file_loader.rs
  - ../../src/goal_curation/operations.rs
  - ../../src/goal_curation/types.rs
  - ../../src/ooda_loop/cycle.rs
---

# Standing seed-goal declaration API reference

> **Status: implemented.** A seed goal can be declared standing/perpetual
> **declaratively** with a single `standing = true` field. The field lives on
> [`SeedGoal`](https://github.com/rysweet/Simard/blob/main/src/identity/manifest.rs)
> and its wire twin
> [`TomlSeedGoal`](https://github.com/rysweet/Simard/blob/main/src/identity/toml_types.rs);
> it is honoured at cold-start seeding by `seed_board_from_seed_goals` and at
> warm-board load by the idempotent `reconcile_standing_markers` self-heal, both
> in [`src/goal_curation/operations.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/operations.rs).
> All three paths converge on the **single** existing standing predicate
> [`ActiveGoal::is_perpetual()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs) —
> there is no second notion of "perpetual."

This reference specifies the declaration surface added in issue #4927. For the
runtime *effect* of being standing (exemption from the no-progress breaker), see
[Standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md)
and the [no-progress breaker API reference](./no-progress-breaker-api.md).

## Why this exists (#4927)

Before this change, the only way a goal could read as standing was for its
persisted **description** to already carry the `[standing] ` marker
(`STANDING_MARKER_PREFIX`). Seed goals had no way to *declare* that intent — so a
standing hygiene/stewardship seed such as **`Articulate repo-hygiene backlog`**
was seeded as an ordinary, convergence-required goal. Because that goal is
inherently perpetual (it re-runs every OODA cycle and never "completes"), the
no-progress breaker treated its lack of a terminal state as a livelock, re-parked
it each cycle, and filed a storm of `goal stuck after guided retry
(UNCLEAR-CRITERIA)` issues (the #4927 / #4930 / #4934 pattern, root-caused in
#4935).

The fix closes the gap at the source: let a seed **declare** it is standing, and
make that declaration flow to the same `is_perpetual()` predicate the breaker and
completion gate already honour. Perpetual goals are then exempt from re-parking
and issue-filing; ordinary goals are entirely unchanged.

## The declaration field

### `SeedGoal.standing`

`src/identity/manifest.rs`

```rust
pub struct SeedGoal {
    pub priority: u32,
    pub title: String,
    pub description: String,
    /// Target-repo slug. `None` means the identity's own repo.
    pub repo: Option<String>,
    /// When `true`, this seed is a standing/perpetual goal: it is exempt from
    /// the no-progress breaker and is never marked `Completed`/tombstoned.
    /// Defaults to `false` (an ordinary, convergence-required goal).
    pub standing: bool,
}
```

- **Default:** `false`. Every existing `SeedGoal` and every seed that omits the
  field remains an ordinary goal — this change is strictly additive.
- **Constructor compatibility:** `SeedGoal::new(priority, title, description,
  repo)` keeps its four-argument signature and sets `standing: false`. Opt in
  with the builder below.

### `SeedGoal::standing()` builder

```rust
impl SeedGoal {
    /// Builder: declare this seed standing/perpetual. Idempotent.
    #[must_use]
    pub fn standing(mut self) -> Self {
        self.standing = true;
        self
    }
}
```

Example:

```rust
SeedGoal::new(2, "Articulate repo-hygiene backlog", "…", Some("hyenas".into()))
    .standing();
```

### `TomlSeedGoal.standing` (identity TOML wire form)

`src/identity/toml_types.rs`

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TomlSeedGoal {
    pub priority: u32,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub repo: Option<String>,
    /// Declares this `[[identities.seed_goals]]` entry standing/perpetual.
    #[serde(default)] // absent ⇒ false; keeps pre-#4927 TOML valid
    pub standing: bool,
}
```

- `#[serde(default)]` means the field is **optional**: identity manifests written
  before #4927 deserialize unchanged (`standing` becomes `false`).
- `#[serde(deny_unknown_fields)]` is **retained**: a misspelled flag
  (e.g. `standng = true`) fails loud at load rather than silently leaving a
  safety-critical goal non-perpetual.

### Propagation

`src/identity/file_loader.rs` copies the field verbatim during
`TomlSeedGoal → SeedGoal` construction — a pure field copy, no interpretation.
`SeedGoal::new` does not set `standing` (it defaults to `false`), so the declared
value is carried through by a conditional `.standing()` builder call:

```rust
let mut seed = SeedGoal::new(
    g.priority,
    g.title.clone(),
    g.description.clone(),
    g.repo.clone(),
);
if g.standing {
    seed = seed.standing(); // sets SeedGoal.standing = true
}
```

Equivalently, the field may be set directly on the constructed struct
(`seed.standing = g.standing;`). Either way it is a pure copy — no interpretation.

The declared value then reaches the two honouring paths below.

## How a declaration becomes `is_perpetual()`

`standing` is a **declarative front door** — it does not add a parallel notion of
perpetual. It causes the existing durable **description marker**
(`STANDING_MARKER_PREFIX = "[standing] "`) to be applied so that
`ActiveGoal::is_perpetual()` returns `true`. There is exactly one source of
truth, read at runtime from the goal's description.

### 1. Cold start — `seed_board_from_seed_goals`

`src/goal_curation/operations.rs`

When an empty board is seeded from `SeedGoal` values, each seed with
`standing == true` has the standing marker applied to its `ActiveGoal` before it
is inserted:

```rust
let mut goal = ActiveGoal {
    parent_goal_id: None,
    priority_explicit: false,
    id: crate::goals::goal_slug(&seed.title),
    description: seed.description.clone(),
    priority: seed.priority,
    status: GoalProgress::NotStarted,
    assigned_to: None,
    repo: seed.repo.clone(),
    current_activity: None,
    wip_refs: vec![],
    last_progress_update_at: None,
    labels: vec![crate::goal_curation::labels::SOURCE_SEED.to_string()],
};
if seed.standing {
    goal = goal.mark_standing(); // prepends "[standing] " iff not already present
}
board.active.push(goal);
```

`ActiveGoal` is built by the same direct struct literal already used in
`seed_board_from_seed_goals`; the only addition is the `if seed.standing` marker
step. `mark_standing()` is idempotent, so re-seeding is safe.

### 2. Warm board — `reconcile_standing_markers`

`src/goal_curation/operations.rs`

```rust
/// Stamp the standing marker onto already-persisted active goals whose exact id
/// OR normalized title-slug matches a `standing` seed. Pure, total, idempotent.
/// Returns the number of goals newly marked (0 on a second run).
pub fn reconcile_standing_markers(board: &mut GoalBoard, seeds: &[SeedGoal]) -> usize
```

Contract:

| Property | Guarantee |
| --- | --- |
| **Match key** | **Exact** goal id **or** normalized title-slug equality against a `standing == true` seed. No substring / regex / fuzzy matching. |
| **Idempotent** | A goal that already reads `is_perpetual()` is skipped; a second call returns `0`. |
| **Total** | Never panics — safe over an empty board, empty seed list, and unicode/pathological titles. |
| **Additive** | Only ever *adds* the standing marker; never removes a marker, never mutates any other field, never touches non-matching goals. |
| **Observability** | Emits one bounded structured `tracing`/OpenTelemetry event carrying **ids/slugs and a count only** — never full goal descriptions. No `print!`/`println!`. |

This is what self-heals the **live** `articulate-repo-hygiene-backlog` goal that a
pre-#4927 build already persisted without a marker: it does not require deleting
the board or setting the `.reseed_goals` marker.

### 3. Per-cycle wiring — `ooda_loop::cycle`

`src/ooda_loop/cycle.rs` calls `reconcile_standing_markers(&mut board, &seeds)`
immediately after the board is loaded (`load_goal_board`) and **before** the
no-progress breaker is evaluated, on every cycle. Running it per-cycle (not only
at startup) is load-bearing for the same reason as the existing self-heal: the
daemon re-reads the board from disk each cycle, so a one-time stamp would be
overwritten by the next reload. The stamp is in-memory and persisted naturally by
the next `commit_cycle`.

## Data flow

```
identity TOML  [[identities.seed_goals]] standing = true
      │  (serde, deny_unknown_fields, default=false)
      ▼
TomlSeedGoal.standing ──file_loader──▶ SeedGoal.standing
      │
      ├── cold start ─▶ seed_board_from_seed_goals ─▶ ActiveGoal.mark_standing()
      │
      └── warm board ─▶ reconcile_standing_markers ─▶ ActiveGoal.mark_standing()
                                                            │
                                                            ▼
                                        ActiveGoal.is_perpetual() == true
                                                            │
                                                            ▼
                          no-progress breaker EXEMPTS it (no re-park, no issue)
```

## Behavioural contract

| Goal | Re-parked by no-progress breaker? | Files `ooda-stuck` issue? | Marked `Completed`/tombstoned? |
| --- | --- | --- | --- |
| `standing = true` (perpetual) | **No** — exempt | **No** | **No** — rolled to a new cycle |
| omitted / `standing = false` (ordinary) | Yes, after threshold | Yes | Yes, when the done-gate certifies it |

The exemption is applied by the OODA driver *before* the breaker's
`resolution_for_why()` is consulted (see
[no-progress breaker API](./no-progress-breaker-api.md)); convergence thresholds
for ordinary goals are unchanged.

## Compatibility & safety

- **TOML round-trips** with and without `standing` (verified by test); existing
  identity manifests remain valid.
- **Fail-loud misconfiguration:** `deny_unknown_fields` is preserved, so a typo'd
  flag is a load-time error, never a silently non-perpetual safety goal.
- **No over-broad exemption:** exact id/slug matching only. A genuinely stuck
  *ordinary* goal is never accidentally exempted — a regression test asserts an
  ordinary stuck goal still re-parks and trips the breaker.
- **No `Bridge` naming**; new code uses `tracing` + OpenTelemetry, no
  `print!`/`println!`.

## Tests

`src/goal_curation/tests_operations.rs`,
`src/goal_curation/tests_no_progress_breaker.rs`,
`src/identity/coverage_tests.rs`:

1. A `standing = true` seed → `ActiveGoal` reads `is_perpetual()` after
   `seed_board_from_seed_goals`.
2. `reconcile_standing_markers` self-heals an existing unmarked persisted goal by
   exact id/slug; a second run returns `0` (idempotent) and is total over
   pathological titles.
3. A perpetual/standing goal is **not** re-parked and files **no** `ooda-stuck`
   issue.
4. A non-perpetual stuck goal **still** re-parks and trips the breaker (ordinary
   behaviour unchanged).
5. `TomlSeedGoal` round-trips with and without `standing` under
   `deny_unknown_fields`.

## Related

- [Standing/perpetual goals are exempt from the no-progress hard-block](../concepts/perpetual-goal-no-progress-exemption.md)
  — the runtime effect this declaration opts into.
- [No-progress breaker API reference](./no-progress-breaker-api.md) and
  [issue-storm suppression](./no-progress-breaker-storm-suppression-api.md).
- [Identity-scoped cognition (seed goals, observe-only Act)](../concepts/identity-scoped-cognition.md)
  and [Pluggable identity](../concepts/pluggable-identity.md).
- [How-to: declare a standing seed goal](../howto/declare-a-standing-seed-goal.md).
- [How-to: diagnose a no-progress breaker issue storm](../howto/diagnose-a-no-progress-breaker-issue-storm.md).
