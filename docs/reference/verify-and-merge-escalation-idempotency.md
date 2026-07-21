---
title: Verify-and-merge escalation idempotency reference
description: >
  The Overseer safety rail that stops the `VerifyAndMergePr` Act path from
  re-paging the operator with the same `verify-and-merge` escalation every tick.
  A per-PR `BackoffGate` (the same proven primitive the coverage-launch rail
  uses) deduplicates repeat escalations of the same still-open PR within a
  growing, bounded window. The fail-closed merge authority is fully preserved —
  a genuinely-green PR still merges exactly once through the unchanged
  authoritative judge, and non-green / high-risk / provider-missing PRs still
  escalate (now deduped, not silenced). Covers the `verify_and_merge_dedup_key`
  contract, the `merge_escalation_backoff` field, the peek-in-`gate` /
  commit-on-`Escalated` seam, the preserved fail-closed merge-authority
  semantics, configuration knobs, and the full behavior matrix.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ./overseer-recipe-launch-idempotency.md
  - ./autonomous-merge-review-gate.md
  - ./agentic-merge-queue-reasoning-api.md
  - ../concepts/autonomous-merge-review-gate.md
  - ./overseer-tick-details.md
  - ../atlas/escalation-flow/README.md
  - ../design/overseer.md
---

# Verify-and-merge escalation idempotency reference

The acting **Overseer** surveys open PRs across the governed roster each tick and,
for a PR she believes is deliverable, plans an
[`Intervention::VerifyAndMergePr`](../design/overseer.md). The Act arm verifies
the PR objectively, then hands it to the authoritative agentic merge-judge in
`merge()` (see the
[autonomous-merge review gate](./autonomous-merge-review-gate.md)). When the PR
cannot merge, the outcome is `ActOutcome::Escalated` — the operator is asked to
`verify-and-merge rysweet/Simard#<n>` by hand.

**One-line summary:** the `VerifyAndMergePr` Act path is now **idempotent per PR**.
A still-open PR that keeps producing the same escalation is **deduped** within a
bounded, growing backoff window so it is not re-paged to the operator every tick;
a genuinely-green PR (with the merge-judge provider configured) still merges
**exactly once** through the **unchanged fail-closed authority**; a non-green,
high-risk, or provider-missing PR **still escalates** — the dedup window bounds
the noise, it never silences a genuine escalation.

Modules: Act/gate seam `src/overseer/mod.rs`
(`Overseer::gate`, `Overseer::act`, `Overseer::new`, `verify_and_merge_dedup_key`);
dedup primitive `src/overseer/guardrails.rs`
(`BackoffGate::peek` / `commit`, `BackoffDecision`); config
`src/overseer/config.rs` (`overseer_backoff_*` getters). Tests
`src/overseer/tests_selfmerge_fix.rs`, `src/overseer/tests_merge_queue_reasoning.rs`.

---

## The escalation-loop defect (#4344)

The `VerifyAndMergePr` Act arm mapped every non-merge to `ActOutcome::Escalated`
and had **no dedup**: nothing suppressed a *repeat* escalation of the *same still
-open PR* on the next tick. So a PR the Overseer surveyed every ~15-minute cycle
that produced an `Escalated` outcome was re-emitted to the operator every cycle,
indefinitely.

Two independent triggers could make an **apparently-green** PR keep producing that
`Escalated` outcome tick after tick:

1. **Judge fail-closed with no provider (correct, but noisy).** `verify()` is only
   the deterministic objective pre-filter; the authoritative review runs inside
   `merge()` step 3. When no LLM/recipe provider is configured, the judge
   (`RefusingMergeJudge`) fails **closed** and `merge()` returns
   `OverseerError::NotMergeReady`, mapped to `ActOutcome::Escalated`. This is the
   **correct** safety posture — a PR is never merged without an authoritative
   judge — but with **no dedup** it re-pages the operator every tick.
2. **`verify()` diff-scan false-positive (a real bug).** A pre-filter check
   wrongly marks a green PR `ready == false`, short-circuiting to `Escalated`
   before `merge()` ever runs — so a genuinely-green PR never merges *and* re-pages
   every tick.

Observed **2026-07-21**: PR **#4344** (`mergeable=MERGEABLE`,
`mergeStateStatus=CLEAN`, checks `coverage` / `pre-commit` / `cargo-audit` all
`SUCCESS`) and PR **#4145** (same green state) were each emitted as
`escalated to operator: verify-and-merge rysweet/Simard#<n>` on **every** cycle
from 05:33 through 11:15 — **15+** times over 6 h — and **never merged**. The
noise buried genuine escalations and the two PRs stayed open despite being
mergeable.

---

## The fix

Two changes, both non-breaking (one narrow `verify()` correctness fix, one
additive dedup rail):

### 1. A genuinely-green PR still merges exactly once — fail-closed preserved

The **fail-closed merge authority is preserved unchanged.** A PR only merges when
the authoritative judge in `merge()` step 3 approves it; a provider outage still
yields `MergeOutcome::Refused` → `OverseerError::NotMergeReady` →
`ActOutcome::Escalated`, exactly as today. This doc deliberately does **not**
relax that posture.

The narrow correctness fix targets the **`verify()` diff-scan false-positive**
(root cause **A** below): an objectively CLEAN + MERGEABLE + all-checks-SUCCESS PR
that `verify()` wrongly marks `ready == false` short-circuits to `Escalated`
*before* `merge()` ever runs. Correcting that mis-classification in `verify()`
lets a genuinely-green PR reach the **unchanged** judge and merge once (provider
present) — leaving the survey on the next tick. No PR is ever merged without the
authoritative judge.

> **Design decision (resolved) — fail-closed stays; root cause A is the scope.**
> The reproduction fixture (`tests_selfmerge_fix.rs`) determines *which* trigger
> re-escalated the green PRs #4344 / #4145, but the **design is fixed regardless**:
>
> - **Root cause A — `verify()` diff-scan false-positive (the chosen fix).** The
>   green PR is wrongly marked `ready == false` and short-circuits *before*
>   `merge()`. The fix lives entirely in `verify()`; the judge / fail-closed
>   posture is **untouched**, a configured provider is still required to merge,
>   and with **no** provider the PR correctly **escalates** (now deduped by §2).
> - **Root cause B — bypass the judge for "green" PRs when no provider is present.**
>   **Rejected.** This weakens fail-closed merge authorization; it is out of scope
>   here and would require explicit operator sign-off, a tightly-scoped
>   "objectively-green" predicate, and a dedicated non-green-still-refused
>   regression test. Not shipped by this feature.
>
> Consequently the behavior matrix below shows a provider-missing green PR as
> **Escalated (deduped)**, never silently merged. If the fixture instead shows the
> re-escalation was purely a *provider-missing* condition (not a `verify()` bug),
> then part §2's dedup gate alone resolves #4344 — the operator is paged once,
> then the repeat pages are suppressed within the bounded window — and no change
> to `verify()` or the judge is required.

### 2. Repeat escalations of the same still-open PR are deduped

A sibling `BackoffGate` on the `Overseer` — `merge_escalation_backoff` — collapses
*repeat* `VerifyAndMergePr` escalations of the same PR within a growing, bounded
window, exactly mirroring the proven `coverage_backoff` rail
(see [recipe-launch idempotency](./overseer-recipe-launch-idempotency.md)). The
gate is **advisory dedup only** — never an authorization control. It is
`peek()`-ed in `gate()` (a held escalation never consumes a launch slot) and
`commit()`-ed in `act()` **only** on `ActOutcome::Escalated`, so a `Merged` PR
and a hard `Err` never arm the window.

Because the window **grows but never permanently silences** (24 h hard cap by
default) and **resets on state change**, a genuinely-stuck PR always resurfaces to
the operator within the cap, and a PR that changes state (e.g. new CI failure)
re-surfaces promptly.

---

## API surface

### `verify_and_merge_dedup_key(repo, pr)`

Deterministic dedup-key builder in `src/overseer/mod.rs`, namespace-disjoint from
[`recipe_dedup_key`](./overseer-recipe-launch-idempotency.md) (which is prefixed
`overseer-obs:`):

```rust
/// Escalation-dedup key for the `VerifyAndMergePr` Act path. A fixed prefix plus
/// the repo slug and the numeric PR number: injection-safe, collision-free across
/// repos, and disjoint from the `recipe_dedup_key` (`overseer-obs:`) namespace.
fn verify_and_merge_dedup_key(repo: &str, pr: u32) -> String {
    format!("verify_and_merge:{repo}#{pr}")
}
```

Example: `verify_and_merge:rysweet/Simard#4344`.

### `Overseer.merge_escalation_backoff`

A new `BackoffGate` field on `Overseer`, sibling to `coverage_backoff`,
initialized in `Overseer::new` from the **existing** `overseer_backoff_*` config
getters — **zero new env vars**:

```rust
merge_escalation_backoff: BackoffGate::new(
    config::overseer_backoff_base_secs(),   // default 900 s (15 min)
    config::overseer_backoff_multiplier(),  // default 2×
    config::overseer_backoff_max_secs(),    // default 86_400 s (24 h)
),
```

The `BackoffGate` primitive is unchanged apart from an additive terminal-state
evictor (`src/overseer/guardrails.rs`):

- `peek(key, now_secs) -> BackoffDecision` — decide **without** recording. An
  unseen key, an elapsed window, or a backwards clock jump all `Admit`; only a
  re-occurrence strictly inside the current window `Suppress`s.
- `commit(key, now_secs)` — record an admitted occurrence, arming the base window
  on first sight and growing it `× multiplier` (capped) on each subsequent one,
  **resetting** to base after a silence `>= 2 ×` the current window.
- `forget(key)` — evict a key's state entirely (a no-op for an unknown key). The
  Act path calls this on a PR's terminal `Merged` outcome so the map stays bounded
  to currently-open, still-escalating PRs (review F2). Fail-safe: eviction can
  only let the gate re-surface sooner, never suppress a page.

### `gate()` — the `VerifyAndMergePr` branch

`gate()` `peek()`s `merge_escalation_backoff` for the PR's dedup key. A suppressed
(held) escalation returns a `held_plan` so it is not re-emitted to the operator:

```
held: an equivalent verify-and-merge escalation was raised recently (backoff window)
```

The peek is placed **before** the cost gate (mirroring the `coverage_backoff`
branch) and fires **only** for `VerifyAndMergePr`; a non-green PR still passes the
gate and escalates.

> **Slot accounting note:** unlike `coverage_backoff` (which guards the
> cost-bearing `LaunchRecipe`), `VerifyAndMergePr` is **not** cost-bearing —
> `is_cost_bearing()` matches only `LaunchRecipe` / `RunAudit`, so a
> verify-and-merge escalation never consumed a per-cycle launch slot to begin
> with. The peek's effect here is purely to suppress the *operator page*, not to
> free a launch slot. If the intent is also to reclaim a budget/launch slot,
> that is a separate design change to `is_cost_bearing()`.

### `act()` — the `VerifyAndMergePr` Act arm

```rust
Intervention::VerifyAndMergePr { repo, pr } => {
    let key = verify_and_merge_dedup_key(repo, *pr);
    let report = self.caps.prs.verify(repo, *pr)?;
    if !report.ready {
        // Repeat escalation of the SAME still-open PR — arm the dedup window.
        self.merge_escalation_backoff.commit(&key, (self.clock)());
        return Ok(ActOutcome::Escalated);
    }
    match self.caps.prs.merge(repo, *pr) {
        Ok(()) => {
            // Terminal state — evict any entry an earlier escalation armed so
            // the per-PR `state` map stays bounded (never commits: the PR
            // leaves the survey).
            self.merge_escalation_backoff.forget(&key);
            Ok(ActOutcome::Merged)
        }
        Err(OverseerError::NotMergeReady { .. }) => {
            self.merge_escalation_backoff.commit(&key, (self.clock)());
            Ok(ActOutcome::Escalated)
        }
        Err(e) => Err(e), // hard error propagates un-deduped (fail-loud)
    }
}
```

`commit()` fires **only** on `ActOutcome::Escalated` (both the `verify.ready ==
false` and `NotMergeReady` paths). It never fires on `Merged` and never on a hard
`Err` — a genuine capability error propagates **un-deduped** so it stays visible.
On `Merged` — the PR's terminal state — `forget()` **evicts** any entry an earlier
escalation left behind, so the per-PR `state` map is bounded to the set of
currently-open, still-escalating PRs rather than growing one permanent entry per
PR ever surveyed (review F2). Eviction is fail-safe: dropping an entry can only
let the gate re-surface sooner, never suppress a real page.

---

## Behavior matrix

| PR state | `verify().ready` | `merge()` | Act outcome | Dedup window | Operator sees |
|---|---|---|---|---|---|
| CLEAN + MERGEABLE + all-SUCCESS, provider present | `true` | `Ok(())` | `Merged` | not armed / **evicted** | one merge, no escalation |
| CLEAN + MERGEABLE + all-SUCCESS, **no** provider | `true` | `NotMergeReady` | `Escalated` | armed / grown | escalation (fail-closed: never merged without a judge), **deduped within window** |
| Non-green (dirty / failing check) | `false` | — | `Escalated` | armed / grown | escalation this tick; **suppressed within window**, resurfaces at/after the cap |
| Green pre-filter, judge **refuses** (genuine risk) | `true` | `NotMergeReady` | `Escalated` | armed / grown | escalation, deduped within window |
| Capability failure (network / auth) | — | `Err(e)` | `Err(e)` | **not** armed | error surfaces every tick (fail-loud) |
| Same still-open non-green PR, next tick, inside window | (peeked) | — | held in `gate()` | unchanged | **no repeat page** |
| Same PR after state change / window elapsed | `false` | — | `Escalated` | reset / re-armed | resurfaces |

The row that closes #4344: a repeat escalation of the **same still-open PR**
inside the window is **held in `gate()`**, so the operator is not re-paged every
cycle — yet the escalation always resurfaces once the bounded window elapses.

---

## Configuration

No new knobs. The `merge_escalation_backoff` reuses the existing gap-scan / launch
backoff configuration (`src/overseer/config.rs`):

| Env var | Default | Meaning |
|---|---|---|
| `SIMARD_OVERSEER_BACKOFF_BASE_SECS` | `900` (15 min) | Base suppression window armed on the first escalation of a PR. |
| `SIMARD_OVERSEER_BACKOFF_MULTIPLIER` | `2` | Growth factor per repeat escalation (must be `> 1`). |
| `SIMARD_OVERSEER_BACKOFF_MAX_SECS` | `86_400` (24 h) | Hard cap on the window — suppression is bounded, so a genuinely-stuck PR always resurfaces within a day. |

All three are fail-safe: unset / empty / unparseable / out-of-range values fall
back to the defaults above. Setting `SIMARD_OVERSEER_BACKOFF_BASE_SECS` also
affects the coverage-launch backoff; the two rails share the tuning by design.

---

## Guardrails preserved (what did *not* change)

- **Merge authority stays opt-in (default OFF).** This change is additive; it does
  **not** relax or bypass the [`MergeAuthority`](./autonomous-merge-review-gate.md)
  gate. The `BackoffGate` is advisory dedup only.
- **Fail-closed for every PR.** `NotMergeReady` / `RefusingMergeJudge` still map to
  `ActOutcome::Escalated`, including a provider-missing green PR. **No PR is ever
  exempted from the judge** — the dedup gate suppresses the *repeat page*, never
  the merge decision.
- **Distinct-steward anti-recursion guard** is untouched; no alternate merge path
  is introduced.
- **No `--admin` / `--no-verify` argv.** Branch protection stays enforceable.
- **Hard errors fail loud.** A genuine capability `Err` propagates un-deduped and
  is never suppressed by the window.
- **TOCTOU-safe.** The Overseer re-verifies each tick and re-runs the
  authoritative agentic review inside `merge()`; the cached verify result never
  authorizes a merge on its own.
- **Process-restart-safe by design.** The in-memory backoff resets on restart;
  harmless — a clean PR simply re-surveys and merges once.
- **Bounded memory.** The per-PR `state` map is evicted on a PR's terminal state
  (`Merged` calls `forget()`); combined with the restart-reset above, the map is
  bounded to currently-open, still-escalating PRs rather than growing one entry
  per PR ever surveyed (review F2). A PR closed *outside* the Overseer is never
  re-surveyed, so its (already window-bounded) entry simply ages out on restart.

### Dedup-key collision (review F4, accepted — no code change)

`verify_and_merge_dedup_key(repo, pr)` interpolates `verify_and_merge:{repo}#{pr}`.
Because `{pr}` is a `u32` and `#` is the literal separator, a collision would
require a `repo` slug literally containing `#<digits>`. GitHub / `gh` repo names
(`owner/name`) can never contain `#`, so the namespaced key is collision-free in
practice. Even in the impossible collision case the failure mode is benign: at
worst one *repeat* operator page is suppressed for one window — never a wrongful
merge (merge authority is unaffected) — and it resurfaces once the window
elapses. Accepted as-is; no guarding code is warranted.

---

## Examples

### Example 1 — clean PR merges once (was: escalated every tick)

**Before** — every ~15-minute cycle, for hours:

```
11:00  escalated to operator: verify-and-merge rysweet/Simard#4344
11:15  escalated to operator: verify-and-merge rysweet/Simard#4344
11:30  escalated to operator: verify-and-merge rysweet/Simard#4344
```

**After** — the PR is CLEAN + MERGEABLE + all-SUCCESS **and the merge-judge
provider is configured**, so the corrected `verify()` lets it reach the unchanged
judge; it merges once and leaves the survey:

```
11:00  did: merged PR rysweet/Simard#4344
```

If **no** provider is configured, the same PR instead escalates **once** and is
then held within the backoff window (fail-closed preserved — never merged without
a judge), which still resolves the #4344 re-paging symptom:

```
11:00  did: escalated to operator: verify-and-merge rysweet/Simard#4344 (no merge-judge provider)
11:15  held: a verify-and-merge escalation for rysweet/Simard#4344 was raised recently (backoff window)
```

### Example 2 — genuinely non-green PR (still escalates, but not every tick)

A PR with a failing `coverage` check escalates once, is held within the window,
then resurfaces after the bounded cap:

```
11:00  did: escalated to operator: verify-and-merge rysweet/Simard#4501 (coverage failing)
11:15  held: a verify-and-merge escalation for rysweet/Simard#4501 was raised recently (backoff window)
11:30  held: a verify-and-merge escalation for rysweet/Simard#4501 was raised recently (backoff window)
...    (window elapses)
11:00+cap  did: escalated to operator: verify-and-merge rysweet/Simard#4501 (coverage failing)
```

### Example 3 — capability error surfaces every tick (fail-loud)

A network/auth failure in `merge()` returns a hard `Err`, which is **never**
deduped:

```
11:00  error: verify-and-merge rysweet/Simard#4602 failed: GitHub API 502
11:15  error: verify-and-merge rysweet/Simard#4602 failed: GitHub API 502
```

---

## Verification checklist (definition of done)

> **Status: target criteria — not yet implemented.** This is a retcon spec; the
> code changes below are the acceptance gate for the implementation, not a record
> of completed work. Boxes flip to `[x]` only when the corresponding test is green.

- [ ] A CLEAN + MERGEABLE + all-checks-SUCCESS PR **with the merge-judge provider
      configured merges exactly once** and is **not** re-escalated on the next tick
      (`tests_selfmerge_fix.rs`: fixture `PrGhClient` / `MergeJudge` fakes return
      MERGEABLE + all-SUCCESS + judge-approves; assert one `Merged` and no second
      escalation across two simulated ticks).
- [ ] A green PR with **no provider** still **escalates** (fail-closed) but is
      **deduped** — paged once, then held within the window (not re-paged every
      tick). Fail-closed authority is unchanged.
- [ ] A genuinely non-green PR **still escalates** — dedup applies to repeats only
      (guardrail regression in `tests_merge_queue_reasoning.rs`).
- [ ] `commit()` fires **only** on `ActOutcome::Escalated`; a `Merged` PR and a
      hard `Err` never arm the window.
- [ ] The dedup window is **bounded and resetting** — a stuck PR resurfaces within
      the 24 h cap; a state change re-surfaces it promptly.
- [ ] Additive / non-breaking: no removed `pub`, PRD preserved, `MergeAuthority`
      default (OFF) and fail-closed non-green behavior unchanged.
- [ ] `tracing` + OTel only in new code — no new `print!` / `println!` /
      `eprintln!`.

## Related

- [Overseer recipe-launch idempotency](./overseer-recipe-launch-idempotency.md) —
  the sibling `BackoffGate` rail this mirrors.
- [Autonomous-merge review gate API](./autonomous-merge-review-gate.md) — the
  `verify()` / `merge()` / `NotMergeReady` / `ActOutcome::Escalated` contract this
  builds on.
- [Overseer tick details reference](./overseer-tick-details.md) — where the
  `did: merged` / `held:` detail lines render.
- [Escalation-flow atlas](../atlas/escalation-flow/README.md) — the escalation
  data-flow this idempotency rail sits inside.
- Issue [#4344](https://github.com/rysweet/Simard/issues/4344).
