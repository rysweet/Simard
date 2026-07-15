---
title: Overseer recurrence convergence API reference
description: >
  The data model, Decide branch, and guarantees of the Overseer's 2× middle
  remediation rung. Covers the recurrence count on RootCause, the branched
  ProblemKind::WorkstreamCoverage Decide arm (notify-only below 2×, bounded
  LaunchRecipe at or above 2×), the pure pick_top_gap and gap_to_brief helpers,
  the Issue-17-style external escalation, the engineer_spawn WHY classification,
  the bounding gates that make auto-launch safe, how it rides the existing
  gap-scan enablement, security notes, and the hermetic tests. The pre-existing
  blocked-goal WHY router is described for context only; this feature does not
  change it.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ../concepts/overseer-recurrence-convergence.md
  - ../howto/configure-overseer-recurrence-convergence.md
  - ./overseer-workstream-gap-scan.md
  - ./overseer-root-cause-why-api.md
  - ./overseer-memory-recall-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ../design/overseer.md
---

# Overseer recurrence convergence API reference

Recurrence convergence adds one **intermediate remediation rung** to the acting
Overseer's Decide step, keyed on the recurrence count that cognitive-memory
recall already folds into every problem's WHY. This reference documents the data
model, the branched `WorkstreamCoverage` Decide arm, the pure helpers, the
bounding gates, config, and security.

> **This is additive and surgical.** It adds no new signal, no new problem kind,
> and no new `Intervention` variant. It changes only the **decision** the
> `ProblemKind::WorkstreamCoverage` arm reaches, reading the already-populated
> `problem.why.recurrence`. `decide()` stays pure (no I/O, no signature change).
> Neither threshold constant changes value. The blocked-goal arm already
> branches on `recurrence` today and is **not** modified here.

> **Modules:** thresholds `src/overseer/signal.rs`
> (`RECURRING_SIGNATURE_THRESHOLD`) + `src/overseer/root_cause.rs`
> (`RECURRENCE_ESCALATION_THRESHOLD`, `engineer_spawn` WHY classification);
> recurrence source `src/overseer/root_cause.rs` (`analyze` →
> `RootCause.recurrence`); Decide branch + pure helpers `src/overseer/mod.rs`
> (`decide` `WorkstreamCoverage` arm, `pick_top_gap`, `gap_to_brief`); bounding
> gates `src/overseer/mod.rs` (in-flight dedup, per-cycle cap) +
> `src/overseer/guardrails.rs` (`RiskClass`, `AutonomyGate`); external escalation
> `src/overseer/mod.rs` / `IssueFiler`; tests `src/overseer/tests_gap_scan.rs`,
> `src/overseer/tests_root_cause.rs`. This feature touches only
> `src/overseer/mod.rs` and `src/overseer/root_cause.rs`; the pre-existing
> `decide_blocked_goal` router is unchanged.

## At a glance

| | |
|---|---|
| **Trigger** | `problem.why.recurrence` on a `WorkstreamCoverage` problem (this feature); the blocked-goal arm already reads it |
| **Noise floor** | `RECURRING_SIGNATURE_THRESHOLD = 2` (unchanged) |
| **Escalation bar** | `RECURRENCE_ESCALATION_THRESHOLD = 3` (unchanged) |
| **New middle rung** | `WORKSTREAM_COVERAGE_LAUNCH_THRESHOLD = 2` (new, in `root_cause.rs`) — `recurrence >= 2` on a `WorkstreamCoverage` gap → bounded `LaunchRecipe` |
| **Bound** | at most **one** gap auto-launched per Overseer cycle |
| **Risk class** | `LaunchRecipe` is `RiskClass::Routine` — no new authority |
| **Fail-safe** | absent `why` ⇒ `recurrence = 0` ⇒ notify-only (never launches without a WHY) |
| **Config** | none new — rides the existing `SIMARD_OVERSEER_GAP_SCAN` enablement (no gaps ⇒ nothing to converge) |

## The recurrence count

The rung reads a value that already exists — it introduces no new recall. The
[root-cause analyzer](./overseer-root-cause-why-api.md) sets `RootCause.recurrence`
from cognitive-memory recall of prior same-signature occurrences:

```rust
pub struct RootCause {
    pub candidates: Vec<CauseCandidate>,
    pub primary_rationale: String,
    pub confidence: Confidence,
    pub source: CauseSource,
    /// Prior same-signature occurrences recalled from memory. 0 when memory is
    /// unavailable or this is a first sighting. Drives the middle rung.
    pub recurrence: u32,
}
```

The `WorkstreamCoverage` arm reads it defensively (the blocked-goal arm reads it
the same way today):

```rust
let recurrence = problem.why.as_ref().map(|w| w.recurrence).unwrap_or(0);
```

**Fail-safe:** a problem with **no** WHY resolves to `recurrence = 0`, which
takes the notify-only path. The rung never launches without a recall-backed WHY.

## The branched Decide arm — `WorkstreamCoverage`

The `ProblemKind::WorkstreamCoverage` arm of `decide()` (`src/overseer/mod.rs`)
extracts the consolidated gaps as before, then branches on recurrence:

```rust
ProblemKind::WorkstreamCoverage => {
    let gaps = problem
        .evidence
        .iter()
        .find_map(|s| match s {
            Signal::WorkstreamGap { gaps } => Some(gaps.clone()),
            _ => None,
        })
        .unwrap_or_default();

    let recurrence = problem.why.as_ref().map(|w| w.recurrence).unwrap_or(0);

    // At/above the new middle rung: converge the single top-ranked gap into a
    // bounded workstream. Below it (incl. absent WHY ⇒ recurrence 0): unchanged
    // notify-only behaviour.
    if recurrence >= WORKSTREAM_COVERAGE_LAUNCH_THRESHOLD {
        if let Some(top) = pick_top_gap(&gaps) {
            return Intervention::LaunchRecipe {
                brief: gap_to_brief(top, recurrence),
            };
        }
    }
    Intervention::FlagWorkstreamGaps { gaps }
}
```

**Contract:**

| `recurrence` | Result |
|---|---|
| `0` / `1` (or absent WHY) | `Intervention::FlagWorkstreamGaps { gaps }` — **identical** to today |
| `≥ 2`, ≥ 1 gap | `Intervention::LaunchRecipe { brief }` for the **single top gap** |
| `≥ 2`, empty gaps | `Intervention::FlagWorkstreamGaps { gaps }` (nothing to launch) |

## Pure helpers

Both helpers are **pure** (no I/O, unit-testable with hand-built values) and live
in `src/overseer/mod.rs`.

### `pick_top_gap`

```rust
/// Select the single top-ranked gap so at most one launch happens per cycle.
/// Returns `None` for an empty slice.
fn pick_top_gap(gaps: &[GapItem]) -> Option<&GapItem>;
```

Ranking is deterministic (category priority — goal > issue > anomaly — then stable
input order) so the same picture always converges the same gap first. Selecting
exactly one gap is the **primary bound** on auto-launch.

### `gap_to_brief`

```rust
/// Convert a gap + its recurrence count into a launch brief. Every text field
/// sourced from the gap (title, ref_id, why_it_matters) is `sanitize_recalled`-
/// cleaned before it reaches the brief.
fn gap_to_brief(gap: &GapItem, recurrence: u32) -> RecipeBrief;
```

The produced `RecipeBrief`:

```rust
RecipeBrief {
    task_description: /* templated from sanitized structured fields, e.g.
        "Cover a recurring backlog gap (seen 2×): <category> <ref_id> — <title>.
         Why it matters: <why_it_matters>. Start an active workstream so this
         stops recurring." */,
    target_repo: "rysweet/Simard".to_string(),
    sequence_group: None,
}
```

`task_description` is **templated from structured, sanitized data** — never raw
memory-graph text spliced in verbatim.

## Blocked-goal WHY router (pre-existing — unchanged by this feature)

A blocked goal is routed by `decide_blocked_goal(..)` (see the
[goal-board health API](./overseer-goal-board-health-api.md) and the
[no-progress resolution API](./no-progress-root-cause-resolution-api.md)). That
router **already** threads the same `recurrence` count and one-line WHY, so a
**repeatedly re-parked** goal is sent down the self-resolving ladder (self-heal a
false park, or escalate a genuine block **with** its WHY) instead of being
bare-re-parked each cycle:

```rust
let recurrence = problem.why.as_ref().map(|w| w.recurrence).unwrap_or(0);
let why = problem.why.as_ref().map(|w| w.to_string()).unwrap_or_default();
return decide_blocked_goal(goal_id, reason, perpetual, needs_review, recurrence, why);
```

At or above `RECURRENCE_ESCALATION_THRESHOLD` (3×), the router escalates the
**root cause** rather than re-unblocking the symptom. This is documented here for
context; recurrence convergence adds the WorkstreamGap rung and leaves this path
untouched.

## External escalation (Issue-17)

An external-repo payload (e.g. `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed`)
cannot be fixed by an in-repo launch. When it crosses the escalation bar it is
filed through the **existing** `IssueFiler` plumbing:

- **argv invocation** — `gh issue create` via `Command::args`, never a shell
  string (no command injection).
- **sanitized body** — reproduction context templated from structured, sanitized
  fields; no secrets, no internal paths, no stack traces.
- **no new code path** — reuses the M1 / `goal_health` issue-filing gates; no
  `--admin`, no `--no-verify`, least privilege only.

## `engineer_spawn` WHY classification

`src/overseer/root_cause.rs` classifies the recurring `resource:engineer_spawn`
signature in the problem's WHY as one of:

| Classification | Meaning | Action |
|---|---|---|
| **benign membership drift** | The admission gate deferred a spawn under budget / concurrency pressure — a ceiling doing its job. | Documented in the WHY; **not** escalated. |
| **spawn failure** | A genuine failure to spawn an engineer. | Surfaced for mitigation. |

Naming which case applies stops the signature from reading as an unexplained
anomaly.

## Bounding — why auto-launch is safe

The `≥ 2` rung is the Overseer's only recurrence-driven auto-launch, and it is
bounded by **four** independent, pre-existing gates — no new bypass is opened:

1. **One gap per cycle** — `pick_top_gap` returns a single gap; the arm launches
   at most one workstream per Decide.
2. **In-flight dedup gate** — a gap already covered by a running workstream is
   suppressed before launch (`src/overseer/mod.rs`).
3. **Per-cycle launch cap** — the Overseer's existing cap on launches per cycle
   applies unchanged.
4. **Fail-closed `RecursionGuard` + `AutonomyGate.admit`** — self-recursion
   (the Overseer launching work that re-triggers itself) fails closed; the
   admission gate still classifies and admits every intervention.

Combined with the **fail-safe** (absent WHY ⇒ `recurrence = 0` ⇒ notify-only), an
auto-launch storm is structurally impossible.

## Configuration

This feature adds **no new configuration** and changes only
`src/overseer/mod.rs` and `src/overseer/root_cause.rs` (not
`src/overseer/config.rs`). The rung's activation rides the **existing** gap-scan
enablement:

| Env var | Effect | Default |
|---|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Gates the whole gap-scan. When off (or the acting Overseer is disabled) there are no `WorkstreamCoverage` problems to converge, so the rung is inert. | **on** |

There is no separate convergence kill-switch: whenever a recurring
`WorkstreamCoverage` problem reaches Decide, the `≥ 2` branch fires. The two
threshold constants are **not** env-tunable by this feature.

> **Design note (open question):** if operators need to disable the auto-launch
> rung independently of the gap-scan, a dedicated opt-out knob
> (e.g. `SIMARD_OVERSEER_GAP_CONVERGENCE`) would have to be added to
> `src/overseer/config.rs` — that expands the change surface to a third file and
> is **out of scope** for the current design. Track it as a follow-up rather than
> assuming it exists.

## Guarantees

- **Additive & backward-compatible.** No new signal / problem / intervention
  variant, no `pub` item removed, `decide()` signature unchanged, `SCHEMA_VERSION`
  unchanged. The `< 2` path returns the **identical** intervention as today.
- **Constants unchanged.** `RECURRING_SIGNATURE_THRESHOLD` (2) and
  `RECURRENCE_ESCALATION_THRESHOLD` (3) keep their values; a new
  `WORKSTREAM_COVERAGE_LAUNCH_THRESHOLD` (2) constant is *added* (in
  `root_cause.rs`) to name the middle rung without re-tuning either existing gate.
  The `< 2` path returns the **identical** intervention as today.
- **Bounded launch.** One gap per cycle × in-flight dedup × per-cycle cap ×
  fail-closed recursion guard — an auto-launch storm cannot occur.
- **No WHY, no launch.** Absent `why` ⇒ `recurrence = 0` ⇒ notify-only.
- **No new authority.** `LaunchRecipe` stays `RiskClass::Routine`; merge, deploy,
  and destructive authority are never routed through this rung.
- **Sanitized inputs.** Every text field from the multi-writer cognitive-memory
  graph is `sanitize_recalled`-cleaned before it reaches a brief, an issue body,
  or a log line; the `overseer-obs:` marker prevents recalled text from forging a
  dedup key.
- **Reuses existing plumbing.** Launch rides the existing `LaunchRecipe` path and
  counter; escalation rides the existing `IssueFiler`. No new persistence, no
  schema change, and `gym_history.db` is never committed.

## Security notes

- **AZ-1:** `Intervention::LaunchRecipe` is `RiskClass::Routine`
  (`src/overseer/guardrails.rs`), so the 2× rung executes autonomously without
  `allow_high_risk`. Acceptable **only** because launch = investigation /
  coverage work; no merge/deploy/destructive authority is ever routed here.
- **AZ-2 / AZ-3:** the fail-closed `RecursionGuard` and the `AutonomyGate.admit`
  call are preserved unchanged; the launch surface is not widened.
- **IV-1:** the recurrence branch is explicit (`>= WORKSTREAM_COVERAGE_LAUNCH_THRESHOLD`);
  absent `why` ⇒ `recurrence = 0` ⇒ notify-only.
- **IV-2:** `sanitize_recalled` is applied at **every** sink — gap summary,
  `RecipeBrief`, and the escalation body — because the text originates in the
  multi-writer cognitive-memory graph.
- **IV-3:** the Overseer uses its own `overseer-obs:` marker so recalled text
  cannot forge dedup keys.
- **Injection:** the external escalation uses argv invocation via `IssueFiler`
  (no shell string) for every `gh issue create`.
- **Data protection:** no secrets in briefs or issues, no new persistence, no
  schema / DB changes.

## Testing

Hermetic tests — no network, no real `gh`, no clock dependence.

`src/overseer/tests_gap_scan.rs`:

- **Below the floor → notify-only.** A `WorkstreamCoverage` problem with
  `recurrence < 2` (or absent WHY) yields `Intervention::FlagWorkstreamGaps`,
  byte-identical to the pre-convergence result.
- **At the floor → single launch.** `recurrence == 2` with ≥ 1 gap yields exactly
  one `Intervention::LaunchRecipe`, whose brief targets the `pick_top_gap` gap and
  carries the recurrence count.
- **One gap per cycle.** A problem carrying several gaps still launches **one**
  workstream (the top gap); the rest are not launched this cycle.
- **Fail-safe.** A problem with **no** `why` never launches (`recurrence = 0` ⇒
  notify-only).
- **Sanitized brief.** A gap whose title contains shell/markup metacharacters
  produces an inert (escaped/truncated) `task_description`.

`src/overseer/tests_root_cause.rs`:

- **`engineer_spawn` classification.** A `resource:engineer_spawn` signal under
  budget/concurrency pressure is classified **benign membership drift** in the
  WHY; a genuine failure is classified **spawn failure**.
- **2× rung documentation contract.** The middle rung is keyed on
  `recurrence == 2`; the two threshold constants keep their values (`2` and `3`).

## See also

- [Recurrence convergence concept](../concepts/overseer-recurrence-convergence.md)
  — the dead zone this closes and the design rationale.
- [Configure Overseer recurrence convergence](../howto/configure-overseer-recurrence-convergence.md)
  — the operator guide.
- [Overseer workstream gap-scan](./overseer-workstream-gap-scan.md) — the
  notify-only baseline and its `GapItem` / `WorkstreamGap` model.
- [Overseer root-cause (WHY) API](./overseer-root-cause-why-api.md) — the
  analyzer that sets `RootCause.recurrence`.
- [No-progress root-cause resolution API](./no-progress-root-cause-resolution-api.md)
  — the self-resolving ladder blocked goals are routed down.
