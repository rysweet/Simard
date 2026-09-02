---
title: wip-ref liveness reconcile — hardening API reference
description: Reference for the two round-3 hardening fixes that make the #4428 wip_refs liveness reconcile total. FIX-1 prunes dead session/branch/engineer wip_refs keyed on live-session membership for EVERY active goal (not only goals with a stale assignment), so an unassigned standing goal carrying a dead-session ref can no longer read as live forever. FIX-2 scopes the per-cycle merged-PR reconcile to each goal's OWN repository slug (goal.repo, None→rysweet/Simard) instead of a hardcoded rysweet/Simard, deduping the gh open-PR fetch per distinct repo, so a goal tracking a still-OPEN PR in another repo is never pruned against the wrong repo's open set.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./wip-ref-liveness-reconcile-api.md
  - ../concepts/research-goal-never-idle.md
  - ./research-goal-never-idle-rail-api.md
  - ./goal-target-repo-routing.md
  - ../howto/keep-the-research-goal-never-idle.md
  - ../../src/ooda_loop/cycle.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/goal_curation/types.rs
  - ../../src/stewardship/merge_authority.rs
  - ../../src/stewardship/types.rs
---

# wip-ref liveness reconcile — hardening API reference

> **Status: implemented.** This reference specifies the two round-3 hardening
> fixes layered on top of the [#4428 wip-ref liveness
> reconcile](./wip-ref-liveness-reconcile-api.md). Both fixes live in
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
> (Prong 1 sweep + Prong 2 fetch/invocation) and the pure prune core in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> They change **which goals** and **which repository** the two prongs act over;
> they do **not** touch the never-idle rail, the `has_live_in_flight_ref()`
> guard, or the fail-open contract.

This reference documents the changes that close the two non-blocking
**hardening observations** raised against
[PR #4439](https://github.com/rysweet/Simard/pull/4439). The PR carrying these
fixes **supersedes #4439** (a strict superset) and links
[#4428](https://github.com/rysweet/Simard/issues/4428) and
[#4411](https://github.com/rysweet/Simard/issues/4411). Read the
[base reconcile reference](./wip-ref-liveness-reconcile-api.md) first — this
document only describes the deltas.

## Contents

- [Why the base reconcile was incomplete](#why-the-base-reconcile-was-incomplete)
- [FIX-1 — dead-ref prune is unconditional and live-session-keyed](#fix-1-dead-ref-prune-is-unconditional-and-live-session-keyed)
- [FIX-2 — the merged-PR reconcile is scoped to each goal's own repo](#fix-2-the-merged-pr-reconcile-is-scoped-to-each-goals-own-repo)
- [Repo-slug derivation and validation](#repo-slug-derivation-and-validation)
- [Fail-open decisions (unchanged in spirit, per-repo in scope)](#fail-open-decisions-unchanged-in-spirit-per-repo-in-scope)
- [Guarantees](#guarantees)
- [What is unchanged](#what-is-unchanged)
- [Tests](#tests)
- [See also](#see-also)

## Why the base reconcile was incomplete

The #4428 reconcile made `has_live_in_flight_ref()` sound *provided* two
assumptions held. Both are now removed:

1. **Prong 1 gated on a stale assignment.** The dead-session ref drop ran only
   **inside** the `if is_stale` block — i.e. only for a goal whose `assigned_to`
   names a session absent from `live_sessions`. An **unassigned** goal
   (`assigned_to == None`) that still carried a `session` / `branch` /
   `engineer` ref was never swept, so that ref read as live forever. This is
   reachable: `clear_goal_assignment`
   ([`src/ooda_actions/.../subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs))
   clears `assigned_to` but **leaves** `wip_refs`, and
   `cleanup_engineer_worktree_for_goal` does not touch them either. The goal
   then idles indefinitely as `ResearchInFlight` — the same class of loophole
   #4428 closed, re-opened one gate over.

2. **Prong 2 hardcoded `rysweet/Simard`.** `reconcile_merged_prs` fetched the
   open-PR set from a hardcoded `TargetRepo::Simard` and ignored
   [`ActiveGoal::repo`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs).
   A standing goal tracking a PR in **another** repo would have its
   possibly-still-**OPEN** PR pruned, because the open set came from the wrong
   repo — a latent [finding-#1 (F1)](./wip-ref-liveness-reconcile-api.md#fail-open-decisions)
   style false prune. A `pr` ref must only ever be judged against the open-PR
   set of **its own** repo.

## FIX-1 — dead-ref prune is unconditional and live-session-keyed

**Site:**
[`sweep_stale_assignments_with_sessions`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
(cycle **start**, before the breaker classifies).

The dead-session `wip_ref` prune is **hoisted out** of the `if is_stale` block
and now runs for **every** active goal, keyed on whether the goal's **owning
engineer session** is alive:

- For **every** active goal, if its owning session is **not** live, drop all of
  its `session` / `branch` / `engineer` `wip_ref`s. The session is "live" when
  the goal's `assigned_to` names a session in `live_sessions`, **or** the goal
  holds a `session` / `engineer` ref whose `ref_id` names a session in
  `live_sessions`. (A `branch` `ref_id` is a branch name and an `engineer`
  `ref_id` an engineer id, so the group's liveness is keyed on the session
  anchor, not on each ref's own id.)
- The pre-existing stale-**assignment** handling is untouched: when
  `assigned_to` names a session absent from `live_sessions`, clear the
  assignment and reset the goal to `NotStarted` as before.
- `pr` and `issue` refs are still **kept** — a PR outlives the session
  (FIX-2 / Prong 2 owns it) and an issue is a durable record.

```rust
/// Cycle-start sweep. Two independent concerns, both keyed on `live_sessions`:
///
///   (a) STALE ASSIGNMENT (unchanged): if `assigned_to` names a session NOT in
///       `live_sessions`, clear the assignment and reset to `NotStarted`.
///   (b) DEAD WORKING REFS (FIX-1): for EVERY active goal — assigned or not — if
///       the goal's owning session is not live, drop its
///       `session`/`branch`/`engineer` wip_refs. `pr`/`issue` refs are KEPT.
///
/// (b) no longer depends on (a): an unassigned goal carrying a dead-session ref
/// is now reconciled, so it can never read as `ResearchInFlight` forever.
///
/// No-op when `live_sessions` is empty (unchanged guard) — avoids dropping every
/// working ref when running outside tmux (e.g. CI). Pure/in-memory; no IO; total
/// (a missing session is simply "not live" — never a panic).
pub(crate) fn sweep_stale_assignments_with_sessions(
    board: &mut GoalBoard,
    live_sessions: &std::collections::HashSet<String>,
) {
    if live_sessions.is_empty() {
        return;
    }
    const DEAD_SESSION_KINDS: [&str; 3] = ["session", "branch", "engineer"];
    for goal in board.active.iter_mut() {
        // (a) stale-assignment reset — unchanged.
        let is_stale = goal
            .assigned_to
            .as_deref()
            .is_some_and(|s| !live_sessions.contains(s));
        if is_stale {
            let session = goal.assigned_to.take().unwrap_or_default();
            eprintln!(
                "[simard] OODA start: cleared stale assignment '{}' for goal '{}'",
                session, goal.id
            );
            goal.status = GoalProgress::NotStarted;
        }
        // (b) FIX-1: drop the dead-session working-ref group for EVERY goal,
        // keyed on the goal's session anchor (a live assignment, or a
        // session/engineer ref naming a live session).
        let session_is_live = goal
            .assigned_to
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| live_sessions.contains(s))
            || goal.wip_refs.iter().any(|wip| {
                let kind = wip.kind.trim();
                (kind.eq_ignore_ascii_case("session") || kind.eq_ignore_ascii_case("engineer"))
                    && live_sessions.contains(wip.ref_id.trim())
            });
        if !session_is_live {
            goal.wip_refs.retain(|wip| {
                let kind = wip.kind.trim();
                !DEAD_SESSION_KINDS
                    .iter()
                    .any(|dead| kind.eq_ignore_ascii_case(dead))
            });
        }
    }
}
```

### Liveness key: the goal's owning session, not its assignment gate

FIX-1 changes the liveness **key** for working refs from *"the goal has a stale
assignment"* to *"the goal's owning session is not alive"*. A `session` /
`engineer` `ref_id` carries a tmux session identifier that is tested directly
against `live_sessions` — the same set the sweep already holds — so it serves as
the group's session anchor. This makes the prune **total** over the board: every
goal is considered, so no assignment state can hide a dead ref.

> **Not drop-all.** The prune is **keyed**, not a blanket drop of every working
> ref. A goal whose session anchor is **live** keeps its whole
> `session`/`branch`/`engineer` group (its engineer is genuinely working) — the
> F1 "preserve a live in-flight ref" guarantee holds for sessions exactly as it
> does for open PRs.
>
> **Divergence from NEW-1 (intentional).** In the base #4428 reconcile the
> stale-assignment branch dropped **all** working refs whenever `assigned_to`
> named a dead session. FIX-1 makes even the stale-assignment goal's working
> refs **keyed**: if such a goal carries a `session`/`engineer` ref whose
> `ref_id` names a **different, still-live** session, that group now **survives**
> (the reset clears the dead assignment, but the live session's refs are
> preserved). This is strictly more correct — a live engineer session is never
> discarded as collateral of a stale assignment pointer — but it is a
> behavioural change from NEW-1's drop-all, so it is covered explicitly by
> [TEST-3](#tests).


## FIX-2 — the merged-PR reconcile is scoped to each goal's own repo

**Site:** `reconcile_merged_prs` and its call site
([`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)).

The hardcoded `TargetRepo::Simard` is removed. The reconcile now fetches the
open-PR set **per distinct goal repository** and prunes each goal's `pr` refs
**only** against **its own** repo's open set.

- The `repo: &str` parameter is **dropped** — the reconcile derives each repo
  slug internally from `ActiveGoal::repo` (see
  [Repo-slug derivation](#repo-slug-derivation-and-validation)).
- Distinct slugs are **deduplicated**: `list_open_prs(slug, OPEN_PR_RECONCILE_LIMIT)`
  is called **at most once per repo per cycle**, collected into a
  `HashMap<String, HashSet<u32>>` (repo-slug → open-PR numbers).
- The existing **fast path** is preserved: if no active goal carries a `pr`
  ref, the whole reconcile is skipped and no `gh` subprocess runs.
- Pruning delegates to the new pure
  [`prune_merged_pr_refs_scoped`](#pure-core-prune_merged_pr_refs_scoped): each
  goal's `pr` refs are judged against the open set for **that goal's** slug.

```rust
/// Prod-only. Fetch the open-PR set ONCE PER DISTINCT goal repo and prune every
/// merged/closed `pr` wip_ref against ITS OWN repo's open set (FIX-2).
///
/// Fast path unchanged: skip everything when no active goal holds a `pr` ref.
/// Per-repo fail-open: if `list_open_prs` errors for a slug (or the slug is
/// invalid), that repo is simply absent from the map and
/// `prune_merged_pr_refs_scoped` KEEPS every `pr` ref for goals in that repo
/// (never prune against an empty-because-errored set). NO new git/gh parse.
fn reconcile_merged_prs(board: &mut GoalBoard, client: &dyn PrGhClient) {
    let has_pr_ref = board.active.iter().any(|g| {
        g.wip_refs.iter().any(|w| w.kind.trim().eq_ignore_ascii_case("pr"))
    });
    if !has_pr_ref {
        return; // fast path: no `gh pr list` subprocess this cycle
    }

    // Distinct, validated slugs among active goals that hold a `pr` ref.
    let mut open_by_repo: std::collections::HashMap<String, std::collections::HashSet<u32>> =
        std::collections::HashMap::new();
    let mut slugs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for goal in board.active.iter() {
        let holds_pr = goal
            .wip_refs
            .iter()
            .any(|w| w.kind.trim().eq_ignore_ascii_case("pr"));
        if !holds_pr {
            continue;
        }
        if let Some(slug) = repo_slug_for_goal(goal) {
            slugs.insert(slug);
        }
        // An invalid/unresolvable slug is skipped: the goal's repo is absent
        // from the map, so its `pr` refs are KEPT (fail-open).
    }

    for slug in slugs {
        match client.list_open_prs(&slug, OPEN_PR_RECONCILE_LIMIT) {
            Ok(open) => {
                let set: std::collections::HashSet<u32> =
                    open.iter().map(|s| s.number).collect();
                open_by_repo.insert(slug, set);
            }
            Err(e) => {
                // Per-repo fail-open: leave this slug OUT of the map.
                eprintln!(
                    "[simard] merged-PR reconcile: list_open_prs failed for '{slug}' — \
                     keeping that repo's PR refs this cycle (fail-open): {e}"
                );
            }
        }
    }

    let pruned = prune_merged_pr_refs_scoped(board, repo_slug_for_goal, &open_by_repo);
    for (goal_id, ref_id) in pruned {
        tracing::info!(
            target: "simard::ooda", goal = %goal_id, ref_id = %ref_id,
            "pruned merged/closed PR wip_ref (not in its repo's open-PR set)"
        );
    }
}
```

### Pure core — `prune_merged_pr_refs_scoped`

The scoped variant lives beside `prune_merged_pr_refs` in
[`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
Both share one private per-goal `pr`-retain helper so the parse / fail-open
logic can never drift between them.

```rust
/// Pure, IO-free. For each ACTIVE goal, retain a `pr` wip_ref IFF its parsed
/// number is in the open set for THAT goal's repo (`repo_of(goal)`). A goal
/// whose repo slug is ABSENT from `open_by_repo` (fetch errored, or slug
/// invalid) keeps ALL its `pr` refs — fail-open, never prune against a
/// missing/empty-because-errored set. Non-`pr` refs are always kept; an
/// unparseable `pr` `ref_id` is kept (warn). Returns pruned (goal_id, ref_id).
pub(crate) fn prune_merged_pr_refs_scoped(
    board: &mut GoalBoard,
    repo_of: impl Fn(&ActiveGoal) -> Option<String>,
    open_by_repo: &std::collections::HashMap<String, std::collections::HashSet<u32>>,
) -> Vec<(String, String)> { /* shares the per-goal retain helper with
                                prune_merged_pr_refs */ }

/// Unscoped single-repo variant (external behaviour unchanged). Compiled only
/// under `#[cfg(test)]`: production reconciles per-repo through the scoped
/// variant, so this single-set form is retained purely as the IO-free contract
/// the NEW-1 unit tests (`tests_no_progress`) drive against one open set. It
/// delegates to the same private per-goal retain helper as the scoped variant.
#[cfg(test)]
pub(crate) fn prune_merged_pr_refs(
    board: &mut GoalBoard,
    open_prs: &std::collections::HashSet<u32>,
) -> Vec<(String, String)> { /* delegates to the same per-goal retain helper */ }
```

- A goal whose slug **is** in `open_by_repo` prunes exactly as the single-repo
  core does — against **that** slug's number set.
- A goal whose slug is **absent** (errored fetch or invalid slug) keeps all its
  `pr` refs. This is the per-repo generalization of the base reconcile's
  fail-open-on-`Err`.

## Repo-slug derivation and validation

`repo_slug_for_goal(goal: &ActiveGoal) -> Option<String>` maps a goal to the
`owner/repo` slug passed to `gh pr list`. The **owner-qualified** form
(`rysweet/…`) is the same canonical slug shape produced by
[`TargetRepo::slug()`](./stewardship-api.md) (`rysweet/Simard`,
`rysweet/amplihack`), which is the authority for the `rysweet` owner. This is a
**distinct** mapping from
[`repo_resolver`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/repo_resolver.rs)
/ [goal target-repo routing](./goal-target-repo-routing.md): that resolver maps
`ActiveGoal::repo` to a **local filesystem path** (`$HOME/src/<name>`) and its
`validate_repo_slug` forbids `/`, whereas `repo_slug_for_goal` produces the
**remote `gh` slug**. The two only share the "`None`/`\"simard\"` ⇒ the Simard
repo" folding.

| `ActiveGoal::repo`                       | Resolved `gh` slug                    |
| ---------------------------------------- | ------------------------------------- |
| `None`                                   | `rysweet/Simard` (`TargetRepo::Simard.slug()`) |
| `Some(s)` where `s == "simard"` (ci-fold)| `rysweet/Simard`                      |
| `Some(s)` containing `'/'`               | `s` as-is (already `owner/repo`)      |
| `Some(s)` bare name (e.g. `amplihack-rs`)| `rysweet/{s}`                         |

> The `s == "simard"` fold (row 2) is **required**: without it the bare-name
> rule would emit `rysweet/simard` (lowercase), diverging from the canonical
> `TargetRepo::Simard.slug()` and from `repo_resolver`'s daemon-repo folding.
> Folding `"simard"` to `None`'s result keeps a repo-less goal and an
> explicitly-`"simard"` goal on the **same** open-PR set.

**Validation** runs **per `owner/repo` component**, because
[`validate_repo_slug`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/repo_resolver.rs)
rejects a literal `/` (it validates a single path segment). For the
`contains('/')` branch, `repo_slug_for_goal` splits on the **single** `/` and
runs each of the owner and repo segments through the same rule set
(`^[A-Za-z0-9._-]{1,64}$`; no whitespace, no shell metacharacters, no `..`
traversal, no leading `-`/`.`, bounded length); more than two segments, or any
segment failing the rule, is rejected. Bare-name and `None`/`"simard"` inputs
validate the single name segment (the `rysweet` owner is a trusted constant, not
user input). An **invalid** slug yields `None` → the goal is skipped → its `pr`
refs are **kept** (fail-open). Every resolved slug is passed to `gh` as a
**discrete argv element** (via the existing `list_open_prs` arg-vector path:
`&["pr", "list", "--repo", repo, …]`) — never string-interpolated into a shell
line — so a slug can never inject a command.

## Fail-open decisions (unchanged in spirit, per-repo in scope)

The reconcile still errs toward **keeping** a ref in every ambiguous case; FIX-2
only makes the unit of failure **per repo** instead of the whole cycle.

| Situation                                             | Behaviour                                           | Why |
| ----------------------------------------------------- | --------------------------------------------------- | --- |
| `pr` `ref_id` fails to parse as `u32`                 | **Keep** the ref, `warn`                            | Malformed-but-possibly-live; never drop on a guess. |
| `list_open_prs` returns `Err` for repo *R*            | **Keep** every `pr` ref of goals in *R* this cycle  | A `gh` blip for one repo must not wipe that repo's live PR refs; other repos still reconcile. |
| goal's repo slug is invalid/unresolvable              | **Keep** every `pr` ref of that goal                | Never prune against a repo we could not query. |
| `list_open_prs` returns `Ok([])` for repo *R*         | **Prune all** `pr` refs of goals in *R*             | Genuinely no open PRs in *R* ⇒ every `pr` ref there is stale. |
| goal's owning session **live**                        | **Keep** its `session`/`branch`/`engineer` group    | FIX-1 keeps the working-ref group of a goal whose session anchor is in `live_sessions`. |
| goal's owning session **dead**                        | **Drop** its `session`/`branch`/`engineer` group (any goal, assigned or not) | FIX-1 unconditional, session-anchor-keyed prune. |
| ref kind is `issue`                                   | **Keep** (both prongs)                              | Durable record; deny-by-default in the liveness guard. |

## Guarantees

- **Total dead-ref prune.** After FIX-1, every remaining `session` / `branch` /
  `engineer` ref on **any** active goal — assigned or unassigned — names a live
  tmux session, so an orphaned working ref can no longer suppress the never-idle
  fault. (NEW-1 generalized from "stale-assignment goals" to "all goals".)
- **Correct repo scoping.** After FIX-2, a `pr` ref is only ever pruned using
  the open-PR set of **its own** repository. A still-OPEN PR in a non-Simard
  repo survives (F1 preserved cross-repo); a Simard goal's merged PR still
  prunes (NEW-1 preserved).
- **Bounded `gh` calls.** `list_open_prs` runs **at most once per distinct
  repo** among active `pr`-ref-holding goals per cycle (deduped), the fast path
  skips it entirely when no `pr` ref exists, and each call keeps
  `OPEN_PR_RECONCILE_LIMIT = 1000`.
- **Pure, testable core.** `prune_merged_pr_refs_scoped` and the FIX-1 retain
  are IO-free, total, and panic-free; unit tests drive them with in-memory maps.
- **No new brittle parse.** FIX-2 reuses `PrGhClient::list_open_prs`; no new
  `git`/`gh` shell invocation or ad-hoc output parsing is introduced.
- **Fail-open, per repo.** A fetch `Err`, an invalid slug, or an unparseable
  ref id can only ever **keep** a ref, never wipe a live one — the F1 guarantee,
  now scoped so one repo's `gh` failure never blocks another repo's reconcile.

## What is unchanged

- **`has_live_in_flight_ref()`** — still IO-free, deny-by-default on kind. FIX-1
  and FIX-2 only guarantee its input (`wip_refs`) is liveness-reconciled first,
  now for every goal and every repo.
- **The never-idle rail** — `classify_standing_idle`, `apply_standing_idle`,
  `ResearchInFlight` / `ResearchFault`, `research_idle_faults`, and both breaker
  sites are unchanged. See the
  [rail API reference](./research-goal-never-idle-rail-api.md).
- **`prune_merged_pr_refs`** (unscoped single-repo variant) — external behaviour
  unchanged; it now delegates to the shared per-goal retain helper and is
  compiled only under `#[cfg(test)]` (production uses the scoped variant).
- **`OPEN_PR_RECONCILE_LIMIT = 1000`**, the fast-path skip, and the
  fail-open-on-`Err` contract — preserved (the last is now per-repo).
- **`PrGhClient` / `OpenPrSummary` / `list_open_prs`** and
  **`TargetRepo::slug`** — reused as-is; no signature change.
- **`ActiveGoal::repo`** — read-only; no schema/serde change (no new persisted
  or wire-format field is introduced).

## Tests

All required tests are hermetic — no network, no live `gh`.

- **TEST-1 (FIX-1, `tests_sweep` in `cycle.rs`)** — an **unassigned**
  (`assigned_to == None`) standing research goal (satisfying
  `is_standing_research_goal()`, e.g. `"[standing] improve memory recall"`)
  whose only `wip_ref` is an `engineer`/`session` ref for a **dead** session.
  Run `sweep_stale_assignments_with_sessions` with a non-empty `live_sessions`
  set that **excludes** that session id. Assert: the ref is **dropped**,
  `has_live_in_flight_ref() == false`, and a subsequent NO-ACTION classification
  via `classify_standing_idle` yields **`ResearchFault`** (active, re-oriented,
  **never `Blocked`**). *(RED before FIX-1: the old `if is_stale` gate skips
  unassigned goals, so the ref survives and the goal reads `ResearchInFlight`.)*

- **TEST-2 (FIX-2, `tests_reconcile_fetch_guard` in `cycle.rs`)** — extend the
  reconcile mock (`CountingClient`) to key open-PR sets **by repo slug**. Build
  two active goals: (i) a goal with `repo = Some("other")` holding an **OPEN**
  `pr` ref whose number is in `rysweet/other`'s open set, and (ii) a
  `repo = None` (Simard) goal holding a **merged** `pr` ref whose number is
  **not** in `rysweet/Simard`'s open set. Run `reconcile_merged_prs`. Assert:
  goal (i)'s OPEN `pr` ref **survives** (queried against `rysweet/other`) while
  goal (ii)'s merged `pr` ref is **pruned** (NEW-1 intact, F1 intact
  cross-repo). *(RED before FIX-2: the hardcoded `rysweet/Simard` fetch would
  prune goal (i)'s open ref against the wrong repo.)*

- **TEST-3 (FIX-1 keyed-not-drop-all, `tests_sweep` in `cycle.rs`)** — a goal
  whose **assignment is stale** (`assigned_to = Some("dead-sess")`, absent from
  `live_sessions`) but which **also** carries a `session`/`engineer` `wip_ref`
  whose `ref_id` names a **different, live** session (`"live-sess"`, present in
  `live_sessions`). Run `sweep_stale_assignments_with_sessions`. Assert: the
  stale **assignment** is cleared and the goal is reset to `NotStarted` (branch
  (a) unchanged), **and** the live `session`/`engineer` ref **survives** so
  `has_live_in_flight_ref() == true`. *(RED against NEW-1's drop-all: the base
  reconcile would have dropped the live ref inside the `if is_stale` block; FIX-1
  keeps it because its `ref_id` is in `live_sessions`.)* This locks the
  intentional divergence called out under
  [FIX-1](#liveness-key-the-goals-owning-session-not-its-assignment-gate).

The three existing single-repo fetch-guard tests stay green: with a single
distinct repo, `list_open_prs` is still called **at most once** per cycle.

## See also

- [wip-ref liveness reconcile API reference](./wip-ref-liveness-reconcile-api.md)
  — the base #4428 reconcile these fixes harden.
- [Concept: the standing research goal never idles](../concepts/research-goal-never-idle.md).
- [Research-goal never-idle rail API reference](./research-goal-never-idle-rail-api.md).
- [Goal target-repo routing](./goal-target-repo-routing.md) — the slug
  convention `repo_slug_for_goal` mirrors.
- [How to keep the research goal never idle](../howto/keep-the-research-goal-never-idle.md).
