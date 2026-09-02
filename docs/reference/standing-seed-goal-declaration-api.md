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
> warm-board load by the idempotent, **reversible** `reconcile_standing_markers`
> reconcile, both
> in [`src/goal_curation/operations.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/operations.rs).
> An explicit `standing = false` conservatively reverses a marker the reconcile
> itself added (see the warm-board reconcile section below).
> All paths converge on the **single** existing standing predicate
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
    /// Defaults to `false` (an ordinary, convergence-required goal). This is the
    /// single boolean the cold-seed path keys off — `false` (whether omitted or
    /// explicit) cold-seeds an ordinary goal.
    pub standing: bool,
    /// Provenance bit distinguishing an **omitted/default** non-standing seed
    /// from an **explicit** `standing = false`. `pub(crate)` — read via the
    /// `authorizes_standing_reversal()` accessor, never directly. Only an
    /// explicit false authorizes the warm-board reconcile to *reverse* a marker.
    pub(crate) standing_explicit: bool,
}
```

- **Default:** `false`, and *omitted* (`standing_explicit == false`). Every
  existing `SeedGoal` and every seed that omits the field remains an ordinary
  goal that is also inert with respect to reversal — this change is strictly
  additive.
- **Three states, all preserved:** *omitted* (`new`, inert), *explicit true*
  (`.standing()`, adds a marker), and *explicit false* (`.non_standing()`,
  authorizes reversal). Cold seeding treats omitted and explicit-false
  identically (both ordinary); they differ only in the warm-board reconcile.
- **Constructor compatibility:** `SeedGoal::new(priority, title, description,
  repo)` keeps its four-argument signature and sets `standing: false` with
  `standing_explicit: false` (omitted). Opt in with the builders below.

### `SeedGoal::standing()` / `SeedGoal::non_standing()` builders

```rust
impl SeedGoal {
    /// Builder: declare this seed standing/perpetual. Idempotent. Records the
    /// declaration as explicit, but a `true` declaration never reverses.
    #[must_use]
    pub fn standing(mut self) -> Self {
        self.standing = true;
        self.standing_explicit = true;
        self
    }

    /// Builder: declare this seed *explicitly* non-standing. Stays non-standing
    /// (cold-seeds like an omitted seed) but authorizes the reconcile to reverse
    /// a marker it previously added to the matching `source:seed` goal.
    #[must_use]
    pub fn non_standing(mut self) -> Self {
        self.standing = false;
        self.standing_explicit = true;
        self
    }

    /// Whether this seed authorizes a *reversal* — true **only** for an explicit
    /// `standing = false` (never for an omitted seed, never for `standing = true`).
    #[must_use]
    pub fn authorizes_standing_reversal(&self) -> bool {
        self.standing_explicit && !self.standing
    }
}
```

Example:

```rust
// perpetual
SeedGoal::new(2, "Articulate repo-hygiene backlog", "…", Some("hyenas".into()))
    .standing();

// explicit reversal of a previously-standing seed (NOT merely omitting .standing())
SeedGoal::new(2, "Articulate repo-hygiene backlog", "…", Some("hyenas".into()))
    .non_standing();
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
    /// `Option<bool>` so an omitted flag (`None`) is preserved as distinct from
    /// an explicit `standing = false` (`Some(false)`) — only the latter reverses.
    #[serde(default)] // absent ⇒ None; keeps pre-#4927 TOML valid
    pub standing: Option<bool>,
}
```

- `#[serde(default)]` means the field is **optional**: identity manifests written
  before #4927 deserialize unchanged (`standing` becomes `None`, i.e. omitted).
- `Option<bool>` preserves the three states: `None` (omitted, inert),
  `Some(true)` (perpetual), `Some(false)` (explicit reversal).
- `#[serde(deny_unknown_fields)]` is **retained**: a misspelled flag
  (e.g. `standng = true`) fails loud at load rather than silently leaving a
  safety-critical goal non-perpetual.

### Propagation

`src/identity/file_loader.rs` maps the wire form during `TomlSeedGoal → SeedGoal`
construction, preserving all three states — no interpretation beyond the direct
mapping:

```rust
let seed = SeedGoal::new(
    g.priority,
    g.title.clone(),
    g.description.clone(),
    g.repo.clone(),
);
match g.standing {
    Some(true) => seed.standing(),      // perpetual
    Some(false) => seed.non_standing(), // explicit reversal
    None => seed,                       // omitted / inert
}
```

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
/// Reconcile persisted active goals against the resolved seed set. An exact id
/// or normalized title-slug match to a `standing = true` seed stamps the
/// standing marker; a match to an *explicit* `standing = false` seed reverses a
/// leading marker on a `source:seed` goal. Pure, total, idempotent.
pub fn reconcile_standing_markers(
    board: &mut GoalBoard,
    seeds: &[SeedGoal],
) -> StandingReconciliation

/// Add/remove tallies for one reconcile pass (both zero on a settled board).
pub struct StandingReconciliation { pub added: usize, pub removed: usize }
```

Contract:

| Property | Guarantee |
| --- | --- |
| **Match key** | **Exact** goal id **or** normalized title-slug equality against a *present* seed. No substring / regex / fuzzy matching. |
| **Add** | A `standing = true` seed stamps `STANDING_MARKER_PREFIX` onto a matching goal that is not already `is_perpetual()`. A `true` declaration always wins over a `false` one for the same slug. |
| **Reverse (explicit false only)** | A `standing = false` seed strips **only** a leading `STANDING_MARKER_PREFIX`, and **only** from a matching goal carrying the exact `source:seed` label — i.e. one this seeding path created. It never demotes a user-created goal, never edits a standing *phrase* in the prose, and a goal whose prose independently reads perpetual stays perpetual after the leading marker is removed. |
| **No reversal on seed absence** | Deleting a seed entirely (its slug no longer present) leaves its goal untouched; only an *explicit* `standing = false` reverses. This keeps board edits intentional and auditable. |
| **Idempotent** | A goal already reading `is_perpetual()` is skipped by the add path; a stripped goal is skipped by the reverse path; a second call is a no-op (`StandingReconciliation::is_noop()`). |
| **Total** | Never panics — safe over an empty board, empty seed list, and unicode/pathological titles. |
| **Observability** | The OODA cycle logs one bounded line carrying the **added/removed counts only** — never full goal descriptions. |

This is what self-heals the **live** `articulate-repo-hygiene-backlog` goal that a
pre-#4927 build already persisted without a marker: it does not require deleting
the board or setting the `.reseed_goals` marker. The same surface makes the
declaration **reversible** — flipping the seed back to `standing = false` strips
the marker the reconciler itself added, without a board wipe.

### 3. Per-cycle wiring — `ooda_loop::cycle`

`src/ooda_loop/cycle.rs` resolves the seed set **once** per cycle
(`resolve_seed_goals`, identity override or baked-in defaults) and reuses that
single `Vec` for both cold seeding (`seed_board_from_seed_goals`) and this warm
reconcile — there is no second `resolve_seed_goals` call. It calls
`reconcile_standing_markers(&mut board, &resolved)` immediately after the board is
loaded (`load_goal_board`) and **before** the no-progress breaker is evaluated,
on every cycle. Running it per-cycle (not only at startup) is load-bearing for the
same reason as the existing self-heal: the daemon re-reads the board from disk
each cycle, so a one-time stamp would be overwritten by the next reload. The
stamp is in-memory and persisted naturally by the next `commit_cycle`.

## Data flow

```
identity TOML  [[identities.seed_goals]] standing = true | false | (omitted)
      │  (serde Option<bool>, deny_unknown_fields, absent ⇒ None)
      ▼
TomlSeedGoal.standing: Option<bool> ──file_loader──▶ SeedGoal (standing + explicit)
      │   None⇒new (inert)  Some(true)⇒.standing()  Some(false)⇒.non_standing()
      │                              (resolved ONCE per cycle, reused below)
      ├── cold start ─▶ seed_board_from_seed_goals ─▶ ActiveGoal.mark_standing()
      │                 (only standing==true; omitted & explicit-false are ordinary)
      │
      └── warm board ─▶ reconcile_standing_markers
                            ├── standing=true       ─▶ ActiveGoal.mark_standing_in_place()
                            ├── explicit false      ─▶ ActiveGoal.unmark_standing_in_place()
                            │      (source:seed goal, leading marker only)
                            └── omitted / absent    ─▶ (inert — never reverses)
                                                            │
                                                            ▼
                                        ActiveGoal.is_perpetual() == true|false
                                                            │
                                                            ▼
                          no-progress breaker EXEMPTS perpetual goals
                          (no re-park, no issue); reversed goals converge again
```

## Behavioural contract

| Goal | Re-parked by no-progress breaker? | Files `ooda-stuck` issue? | Marked `Completed`/tombstoned? |
| --- | --- | --- | --- |
| `standing = true` (perpetual) | **No** — exempt | **No** | **No** — rolled to a new cycle |
| omitted / `standing = false` (ordinary) | Yes, after threshold | Yes | Yes, when the done-gate certifies it |
| reverted via *explicit* `standing = false` on a `source:seed` goal | Yes again — exemption dropped | Yes | Yes |

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
- **Conservative, reversible reversal:** an explicit `standing = false` only ever
  strips a *leading* marker from a `source:seed` goal it previously stamped.
  Seed *absence* never reverses; user-created goals and standing *phrases* in
  prose are never touched. A goal made perpetual by its own prose stays perpetual.
- **One resolution per cycle:** `ooda_loop::cycle` calls `resolve_seed_goals`
  once and reuses the `Vec` for cold seeding and this reconcile — no duplicate
  resolution, so cold and warm paths can never disagree about the seed set.

## Tests

`src/goal_curation/tests_operations.rs`,
`src/goal_curation/tests_no_progress_breaker.rs`,
`src/goal_curation/types.rs`,
`src/ooda_loop/tests_no_progress.rs`,
`src/identity/coverage_tests.rs`:

1. A `standing = true` seed → `ActiveGoal` reads `is_perpetual()` after
   `seed_board_from_seed_goals`.
2. `reconcile_standing_markers` self-heals an existing unmarked persisted goal by
   exact id/slug; a second run is a no-op and is total over pathological titles.
3. An explicit `standing = false` reverses the leading marker on a `source:seed`
   exact-slug goal; the same slug **without** `source:seed` is untouched; seed
   *absence* reverses nothing; a stripped goal whose prose still reads perpetual
   stays perpetual (`unmark_standing_in_place` prefix-only guarantee); reversal is
   idempotent.
4. A perpetual/standing goal is **not** re-parked and files **no** `ooda-stuck`
   issue; a reverted goal re-enters the breaker and escalates like any ordinary
   stuck goal.
5. A non-perpetual stuck goal **still** re-parks and trips the breaker (ordinary
   behaviour unchanged).
6. `TomlSeedGoal` round-trips with and without `standing` under
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
