---
title: Overseer verify-and-merge escalation convergence reference
description: >
  The convergence rail that makes the Overseer's `VerifyAndMergePr` escalation
  path TERMINATE instead of re-firing every tick (#4344 / #4145). Documents the
  per-`repo#pr` `merge_escalation_gate: BackoffGate` field on `Overseer`, the
  `MergeBlocker` classified-reason enum rendered on the first escalation, the
  peek-then-commit wiring in the `Intervention::VerifyAndMergePr` act arm so an
  UNCHANGED escalation for an already-escalated PR is suppressed while a genuine
  state change (or the backoff window elapsing) re-surfaces it, and the
  fail-closed authority invariants that guarantee acknowledged-blocked
  convergence never downgrades to a merge.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference
related:
  - ../design/overseer.md
  - ./overseer-backoff-gate-api.md
  - ./overseer-recipe-launch-idempotency.md
  - ./draft-pr-exclusion-gate.md
  - ./cross-repo-merge-authority.md
  - ./overseer-operator-notifications.md
  - ./overseer-activity-feed.md
  - ../concepts/autonomous-merge-review-gate.md
---

# Overseer verify-and-merge escalation convergence reference

This reference documents the rail that makes the acting **Overseer**'s
`VerifyAndMergePr` escalation **converge**: a merge-ready PR that cannot be
merged autonomously is escalated to the operator **once** — with a concrete,
classified blocker — and is **not** re-escalated on every subsequent tick until
its state actually changes or a backoff window elapses. For the surrounding OODA
loop and the pr-verify checklist see the [Overseer design](../design/overseer.md).

## The non-convergence defect (#4344 / #4145)

The `Intervention::VerifyAndMergePr { repo, pr }` act arm in
[`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
runs the pr-verify pre-filter, then the authoritative agentic merge, and maps a
non-merge outcome to `ActOutcome::Escalated`:

```rust
Intervention::VerifyAndMergePr { repo, pr } => {
    let report = self.caps.prs.verify(repo, *pr)?;
    if !report.ready {
        return Ok(ActOutcome::Escalated);          // pre-filter said "not ready"
    }
    match self.caps.prs.merge(repo, *pr) {
        Ok(()) => Ok(ActOutcome::Merged),
        Err(OverseerError::NotMergeReady { .. }) => Ok(ActOutcome::Escalated), // judge refused
        Err(e) => Err(e),
    }
}
```

Before this change the arm emitted `ActOutcome::Escalated` **unconditionally,
every tick**, with no memory that the same `repo#pr` had already been escalated.
The observed harm (2026-07-20): two green, `mergeable = MERGEABLE`, `state = CLEAN`
PRs — `rysweet/Simard#4344` and `rysweet/Simard#4145` — were re-escalated as
`DeliveryReady` on **14+ consecutive ticks** over 5+ hours
(`07:26Z`…`12:51Z`), each emitting the same
`escalated to operator: verify-and-merge rysweet/Simard#4344` activity line, yet
neither PR ever merged and neither escalation ever *resolved*. The escalation was
a symptom broadcast on a loop, not a driver of progress.

Two things were missing:

1. **No suppression of the unchanged repeat.** Nothing recorded that
   `rysweet/Simard#4344` was already escalated and still in the same state, so the
   identical escalation fired again next tick.
2. **No concrete blocker.** The escalation said *"verify-and-merge #4344"* but not
   **why** the merge could not proceed, so the operator had nothing actionable and
   the loop could not tell "escalate the first time" from "re-escalate unchanged".

## The fix: a per-PR convergence gate

The Overseer gains one field — a
[`BackoffGate`](./overseer-backoff-gate-api.md) keyed on the canonical
`repo#pr`. It reuses the same `BackoffGate` primitive as the existing
`coverage_backoff: BackoffGate` field (`src/overseer/mod.rs`), and it plays the
same *converge-the-escalation* role that the `blocked_goal_gate: WhisperGate`
field already plays for the blocked-goal path (a behavioral sibling on a
different primitive):

```rust
/// Dedup + rate-limit gate for the verify-and-merge escalation path, so a PR
/// that is merge-ready-but-not-auto-mergeable is escalated to the operator ONCE
/// and not re-escalated every tick while its state is unchanged (#4344 / #4145).
/// Keyed by the canonical `repo#pr`; timed on the injected `self.clock` so tests
/// drive a virtual clock. Distinct from `blocked_goal_gate` and `coverage_backoff`
/// so the three convergence paths never interfere.
merge_escalation_gate: BackoffGate,
```

It is constructed in the `Overseer::new` struct literal in
`src/overseer/mod.rs`, immediately alongside `coverage_backoff` (mod.rs:446) and
sharing its accessor-based defaults (the same `SIMARD_OVERSEER_BACKOFF_*` knobs
the gap-scan coverage backoff uses):

```rust
merge_escalation_gate: BackoffGate::new(
    config::overseer_backoff_base_secs(),   // 900    (15 min) first window
    config::overseer_backoff_multiplier(),  // 2      exponential growth
    config::overseer_backoff_max_secs(),    // 86_400 (24 h) cap
),
```

(`src/overseer/wiring.rs` is only the *rendering* seam — `describe_action` /
`escalate_reason` — not where the gate fields are constructed.)

The gate reuses the whole [`BackoffGate` primitive and its
`SIMARD_OVERSEER_BACKOFF_*` accessors](./overseer-backoff-gate-api.md) — no new
knob is introduced. See that reference for the exponential window schedule,
clock-regression safety, and fail-safe math; only the *wiring* is new here.

### The convergence key

```
key = "{repo}#{pr}"      // e.g. "rysweet/Simard#4344"
```

`repo` and `pr` are validated **before** the key is built or any `gh` call is
made: `repo` must match `^[\w.-]+/[\w.-]+$` and `pr` must be a positive integer.
A mismatch escalates with a validation reason and never constructs a gate key or
a subprocess argument (defense against a malformed identifier reaching a `gh`
argv). The key is a discrete argument everywhere it is used — never interpolated
into a `sh -c` string.

### `MergeBlocker` — the classified reason

The escalation now carries a **classified** blocker, rendered on the **first**
escalation only, so the operator sees *why* the merge cannot proceed:

```rust
/// Why an otherwise-surfaced PR could not be auto-merged. Rendered into the
/// structured escalation log on the FIRST escalation for a `repo#pr` (and into
/// the suppressed outcome's `reason`). Carries NO tokens and NO verbatim PR body
/// — only this closed set of causes; any interpolated detail is
/// newline-stripped and length-bounded (log-injection safe).
#[derive(Debug)]
enum MergeBlocker {
    /// pr-verify pre-filter said the PR is not ready (draft / not MERGEABLE /
    /// base branch not on the allowlist / required check not green).
    NotReady { detail: String },
    /// The authoritative agentic merge-judge refused (or failed closed because
    /// no LLM provider is configured): `OverseerError::NotMergeReady`.
    JudgeRefused { detail: String },
}
```

The detail is sourced from the objective pre-filter (the failing check names) or
the merge-judge result (e.g. `"the merge-readiness judge did not approve"`). It
is hardened through the shared `signal::sanitize_detail` before rendering —
ANSI-stripped, control-char/whitespace-collapsed, token-shaped-secret redacted,
and length-bounded — never a raw PR body, never a token. The classified **class**
(`"not_ready"` / `"judge_refused"`) is what the
convergence gate compares to detect a genuine state change.

## The convergence contract

On each tick where the decision layer produces
`Intervention::VerifyAndMergePr { repo, pr }`, the act arm follows a
**peek → act → commit-on-escalate** discipline (the mirror of the gap-scan
peek-then-commit-on-success rail, inverted so that it is the *escalation* — not
the launch — that consumes the backoff slot):

1. **Validate** `repo` / `pr`; on mismatch, escalate once with a validation
   reason and return (no gate key built).
2. **Attempt real progress first.** Run `verify()` then, if ready and opted in,
   `merge()`. A successful `merge()` returns `ActOutcome::Merged` and **does not
   touch the gate** — a merged PR needs no suppression, and the goal signal
   disappears on its own next tick.
3. **On a non-merge outcome, `peek(key, now)` the gate:**
   - `BackoffDecision::Admit` (first escalation for this `repo#pr`, or its
     backoff window has elapsed) → emit the escalation **with** the classified
     `MergeBlocker`, then `commit(key, now)` to grow the window. Returns
     `ActOutcome::Escalated`.
   - `BackoffDecision::Suppress` (an unchanged escalation fired within the
     current window) → **do not re-emit**. Record an acknowledged
     *held / in-flight* plan whose reason names the still-pending PR, and
     continue the tick with other work. The gate is **not** committed again (a
     suppressed action records nothing), so the window does not grow on silence.

So an already-escalated, unchanged PR converges to a **quiet acknowledged-pending
state** instead of a per-tick alarm; the operator is paged once and then left
alone until something changes.

### What counts as "changed" (re-surfacing)

The escalation is *not* permanently silenced — it re-surfaces when:

- **The PR merges** (autonomously or by the operator) → the `DeliveryReady`
  signal stops being produced; nothing to escalate.
- **The blocker changes class** (e.g. a draft is un-drafted so the pre-filter
  `NotReady` gives way to a judge `JudgeRefused`) → this is a genuine state change
  worth re-paging. The Overseer records the last-escalated blocker class per
  `repo#pr` in a companion `merge_escalation_blocker: HashMap<String, &'static str>`
  map; when the current class differs from the stored one the act arm re-admits
  and re-emits **even inside the backoff window**, bypassing `Suppress`. A changed
  blocker is *not* the "unchanged repeat" the gate suppresses.
- **The backoff window elapses** with the PR still stuck → `peek` returns `Admit`
  again, so a long-stuck PR is re-surfaced on the exponential cadence (15 min →
  30 min → 1 h → 2 h → … → capped 24 h), never more often, never fully forgotten.

This is the same "rate-limited, never permanently silenced" guarantee the
[gap-scan backoff](./overseer-backoff-gate-api.md#window-schedule) provides.

## Fail-closed authority invariants (unchanged, and preserved)

Convergence is a **narrowing** of *notifications*, not a change to merge
authority. The following invariants from
[cross-repo merge authority](./cross-repo-merge-authority.md) and the
[autonomous-merge review gate](../concepts/autonomous-merge-review-gate.md) are
preserved verbatim:

- **Acknowledged-blocked never becomes a merge.** Suppressing a *repeat
  escalation* only stops the operator notification; it never advances the PR
  toward merge. A merge happens **only** through the unchanged `verify()` →
  `merge()` path (step 2), which still runs the objective pre-filter and the
  authoritative agentic merge-judge, and still fails closed
  (`RefusingMergeJudge`) when no LLM provider is configured. Ambiguous authority
  still escalates; it is never resolved by the gate.
- **RecursionGuard intact.** The Overseer still refuses to verify/merge its own
  PRs; the gate sits *downstream* of that refusal and never re-admits a
  self-authored PR.
- **`automerge_author` allowlist intact.** `None` still yields no candidates; the
  gate adds no bypass.
- **No `--admin` / `--no-verify`.** The merge path is unchanged; this rail adds
  no elevated `gh` flag.

## Observability

A suppressed repeat is **fail-visible**, never silently dropped — the
`ActOutcome::MergeEscalationSuppressed { reason }` surfaces as a held/acknowledged
line in the tick's structured `action_details` via `describe_action`
([`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs)),
the same channel the in-flight guard, cost gate, and gap-scan backoff already
use, so an operator watching the [activity feed](./overseer-activity-feed.md) sees
the PR is still pending without a fresh page, and it is tallied under the
dedicated `merge_escalations_suppressed` tick counter (never folded into
`escalations`):

> `held: verify-and-merge rysweet/Simard#4344 — already escalated, awaiting operator (judge refused: the merge-readiness judge did not approve)`

The **first** escalation keeps `ActOutcome::Escalated` as a plain tally marker (so
the tick's `escalations` count and the existing `describe_action` /
`escalate_reason` activity line are unchanged), and additionally emits the
classified blocker on a dedicated structured event —
`target: "overseer::merge_escalation"`, fields `repo`, `pr`, `blocker` (the
class), and a sanitised `reason`:

> `WARN overseer::merge_escalation repo=rysweet/Simard pr=4344 blocker=judge_refused reason="judge refused: the merge-readiness judge did not approve" verify-and-merge escalated to operator (merge-ready but not auto-mergeable)`

Logging discipline (same as every Overseer path): structured `tracing` / OTel
only — **no `print!` / `println!`** (enforced by `deny.toml`). Only the classified
`MergeBlocker` class and the `repo#pr` are rendered; no tokens, no verbatim PR
bodies, and every interpolated detail is ANSI-stripped, control-char-collapsed,
secret-redacted, and length-bounded by the shared `signal::sanitize_detail`
against log injection.

## Behavior contract (worked examples)

Given an `Overseer` wired with a fake `PrOps` and a virtual `clock`:

**1. Escalate once, then suppress the unchanged repeat.**

```
tick @ t=0     VerifyAndMergePr{#4344}  verify not-ready → Admit  → Escalated (blocker rendered)
tick @ t=300   VerifyAndMergePr{#4344}  same state       → Suppress → held (no new escalation)
assert escalation_count(#4344) == 1
assert last_plan(#4344).is_held_pending()
```

**2. The window elapses → re-surface (never permanently silenced).**

```
tick @ t=0      #4344 not-ready → Admit    → Escalated   (window := 900)
tick @ t=800    #4344 not-ready → Suppress → held
tick @ t=1000   #4344 not-ready → Admit    → Escalated   (>900s elapsed; window := 1800)
assert escalation_count(#4344) == 2
```

**3. A real state change re-pages immediately (changed blocker).**

```
tick @ t=0     #4344 blocker = NotReady{"still a draft"}      → Admit → Escalated
tick @ t=120   #4344 blocker = JudgeRefused{"ci failing"}     → Admit → Escalated
assert escalation_count(#4344) == 2   // changed blocker is not the "unchanged repeat"
```

**4. Success converges with no escalation and no gate write.**

```
tick @ t=0     #4145 verify ready, merge Ok(()) → Merged
assert escalation_count(#4145) == 0
assert gate_committed(#4145) == false   // a merged PR needs no suppression
```

**5. Distinct PRs do not suppress each other.**

```
tick @ t=0   #4344 not-ready → Escalated
tick @ t=0   #4145 not-ready → Escalated
assert escalation_count(#4344) == 1 && escalation_count(#4145) == 1  // independent keys
```

**6. Clock regression fails toward surfacing (never wedged silent).**

```
tick @ t=1000  #4344 not-ready → Admit → Escalated
tick @ t=200   #4344 not-ready → Admit → Escalated   // now < last_admit ⇒ treat as window elapsed
```

(The last example is guaranteed by the `BackoffGate`'s explicit
`now_secs < last_admit` guard; see
[the BackoffGate reference](./overseer-backoff-gate-api.md#safety-invariants).)

## What did NOT change

- **The merge machinery.** `verify()`, `merge()`, `evaluate_objective_gates`, the
  agentic merge-judge, and the `MergeAuthority` opt-in gate are all unchanged.
  This rail only governs **how often the *escalation* is emitted**, never whether
  a merge is authorized.
- **The `VerifyAndMergePr` intervention shape** (`{ repo, pr }`) and the
  `ActOutcome::{Merged, Escalated}` variants.
- **The activity-feed / notification seam.** The first escalation still flows
  through `describe_action` → `escalate_reason`; the gate only adds the held-plan
  reason for the suppressed repeat and the classified blocker on the first emit.
- **The other two convergence gates.** `blocked_goal_gate` (goal-board escalation)
  and `coverage_backoff` (gap-scan) are untouched; `merge_escalation_gate` is a
  third, independent gate keyed on `repo#pr`.

## Test surface

- **Unit** (`src/overseer/tests_merge_escalation_convergence.rs`): virtual-clock
  coverage of escalate-once-then-suppress, window-elapse re-surface,
  changed-blocker re-page, success-without-gate-write, key independence, and
  clock-regression safety — plus the authority-preservation assertion that an
  acknowledged-blocked convergence **never** produces `ActOutcome::Merged`.

## Related reading

- [Overseer design](../design/overseer.md) — the OODA loop, the pr-verify
  checklist, and the escalation-lifecycle section this rail implements.
- [Overseer BackoffGate & gap-scan dedup](./overseer-backoff-gate-api.md) — the
  reused `BackoffGate` primitive, its window schedule and safety invariants.
- [Overseer recipe-launch idempotency](./overseer-recipe-launch-idempotency.md) —
  the sibling convergence rail for in-flight recipe processes (different seam).
- [Draft-PR exclusion gate](./draft-pr-exclusion-gate.md) — the upstream sensor
  rail that keeps a draft out of `ready_prs` in the first place.
- [Cross-repo merge authority](./cross-repo-merge-authority.md) — the fail-closed
  authority this rail preserves.
- [Autonomous-merge review gate](../concepts/autonomous-merge-review-gate.md) —
  why the merge-judge is the sole reviewer and fails closed.
