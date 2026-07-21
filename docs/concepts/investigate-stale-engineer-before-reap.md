---
title: "Investigate-Before-Reap: never reclaim a quiet engineer without an investigation"
description: >
  Why Simard investigates a quiet/idle engineer BEFORE ever reaping it. A
  stalled engineer may have hit a bug Simard needs to fix, and destroying its
  worktree destroys the evidence. The stale-engineer-claim reaper no longer
  releases + cleans a HeartbeatStale claim directly. Instead it PRESERVES the
  engineer's diagnostic evidence to a durable reaped-engineers/ archive, drives
  an agentic investigation (reusing diagnosis/root_cause/intervention/
  capabilities) that asks WHY the engineer went quiet, feeds any Simard-bug
  finding back into self-improvement (issue/escalation/recipe), and reclaims the
  claim ONLY when the investigation concludes the engineer is genuinely
  dead/unrecoverable — with the verdict named in the reclaim log line.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ./stale-engineer-claim-reaper.md
  - ./overseer-agentic-health-review.md
  - ./no-progress-terminal-investigation.md
  - ./self-diagnose-on-step-error.md
  - ../reference/investigate-stale-engineer-api.md
  - ../reference/claim-reaper-api.md
  - ../reference/overseer-root-cause-why-api.md
  - ../howto/investigate-a-stale-engineer-before-reap.md
  - ../operations/claim-reaper-kill-switch.md
---

# Investigate-Before-Reap

> **Status: implemented.** This page describes shipped behaviour in present
> tense. The behaviour lives in
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
> (`reap_stale_claims` + the `StaleEngineerInvestigator` seam) and is wired on
> the Overseer tick
> ([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> `reap_stale_engineer_claims`, beside `reconcile_inflight_investigations`).

## The defect this closes

The [stale-engineer-claim reaper](./stale-engineer-claim-reaper.md) sweeps
`engineer_claims` and reclaims rows whose engineer is provably dead. Its
`NoWorktree` path is unambiguous — there is nothing to protect, so it reclaims
immediately, and that stays. But its `HeartbeatStale` path used to do something
**antithetical to Simard's self-improvement mission**: on a worktree whose
newest-file mtime was older than the threshold it would _immediately_

1. log the reclaim,
2. `release_engineer_claim` (free the cap slot), and
3. `cleanup.cleanup` — **destroy the worktree**.

No investigation. And a stalled engineer is exactly the case Simard most needs to
learn from: it may have **hit a bug Simard should fix**. Destroying its worktree
destroys the transcript, the recipe-runner output, the exit status — the evidence
of _why_ it went quiet. A silent reclaim throws away a self-improvement signal.

## The new invariant

> **NO REAP WITHOUT A COMPLETED INVESTIGATION AND PRESERVED EVIDENCE.**

Only the `Dead { HeartbeatStale, age > stale_secs }` branch changes. On that
branch the reaper no longer releases + destroys directly. Instead it:

1. **Preserves evidence first.** Before any worktree removal, the engineer's
   diagnostic evidence — the worktree's newest logs / transcript / recipe-runner
   output / captured exit status, plus a narrow `journalctl` slice for its goal —
   is archived into a durable, state-root directory that survives worktree
   cleanup:
   `<state_root>/reaped-engineers/<sanitized_claim_key>-<unix_ts>/`, guarded by
   the same `assert_under_root` containment the worktree cleanup uses. The raw
   `claim_key` is stored in a `manifest.json`; the directory name is sanitized.
2. **Investigates agentically.** It drives a structured investigation behind an
   injectable trait seam ([`StaleEngineerInvestigator`](../reference/investigate-stale-engineer-api.md)),
   reusing the machinery Simard already has rather than inventing new plumbing:
   the [`diagnosis`](./self-diagnose-on-step-error.md) classifier, the
   [`root_cause`](../reference/overseer-root-cause-why-api.md) WHY-over-evidence
   analysis with prior-same-signature recall, and the
   [`investigate_stale_engineer.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/investigate_stale_engineer.md)
   prompt asset. The investigation classifies WHY the engineer went quiet. These
   findings live at **two levels** that the reference reflects precisely: a
   routing **verdict** (`still-alive` / `blocked` / `recoverable` / `dead`, which
   alone decides reap) and — only when the verdict is `dead` — a **cause** that
   explains the death for telemetry and self-improvement. In those terms the
   categories are: *verdict-level* still-actually-alive (false positive) /
   blocked-on-missing-precondition / recoverable-transient; and, under a `dead`
   verdict, *cause-level* crashed-with-panic / hung-on-lock / OOM / E2BIG /
   hit-a-simard-bug / genuinely-finished-but-didn't-report. Only the verdict
   gates the reap; the cause never does (see the
   [`{verdict, cause}` taxonomy](../reference/investigate-stale-engineer-api.md#verdict-types)).
3. **Acts on the finding (feeds self-improvement).** Findings route through the
   Overseer's **existing** gated `Intervention` → plan → `act()` path — the same
   one [agentic health review](./overseer-agentic-health-review.md) uses — not a
   parallel pipeline:
   - **Still actually alive** → **DO NOT reap.** The claim is left in place and
     the false positive is logged (an extension of the fail-closed contract).
   - **Blocked / recoverable** → **DO NOT reap** this sweep. `EscalateBlockedGoal`
     surfaces the block; `LaunchRecipe`/`Whisper` resumes recoverable work.
   - **Simard bug / systemic failure** → **file a tracking issue** (`FileIssue`,
     the same gh-issue capability the no-progress breaker uses) and/or dispatch a
     `LaunchRecipe` fix / `EscalateBlockedGoal`, and record the root cause in
     cognitive memory so recurrence is recognized. The engineer's death becomes a
     self-improvement signal, not a silent reclaim.
   - **Genuinely dead / unrecoverable** (evidence preserved) → perform the
     existing `release_engineer_claim` + worktree cleanup. The reclaim log line
     now **also names the investigation verdict**.

## Why a thin Rust rail over an agentic seam

The reap **decision** in Rust is binary and mechanical: reclaim **iff**
`verdict.should_reap()`. All the nuanced "WHY did it go quiet, and what should we
do about it" reasoning lives behind the `StaleEngineerInvestigator` trait — a
prompt asset dispatched as a gated `smart-orchestrator` workstream, exactly the
[health-review](./overseer-agentic-health-review.md) pattern. This keeps two
things true at once:

- **Ruthless simplicity + hermetic tests.** The pure `reap_stale_claims`
  orchestrator takes the investigator as an injected seam, so the whole sweep is
  unit-tested with fakes — no real filesystem, process, or `gh`. No failure
  heuristics are hard-coded in Rust beyond the mechanical classifier that ROUTES
  to the agentic step.
- **No parallel plumbing.** The investigation reuses `diagnosis`, `root_cause`,
  `intervention`, and `capabilities`; there is exactly one evidence-archive path
  and one Act path.

### Latency: the `Pending` verdict

Agentic investigation can be slow, but the reaper rail is thread-less and
synchronous. The verdict enum therefore includes a non-terminal **`Pending`**
variant (`should_reap() == false`). The production investigator archives the
evidence, dispatches the agentic investigation as an `Intervention::LaunchRecipe`
(surfaced through the SAME gated Act path every other remediation uses), and
returns `Pending` — so staleness ALONE never reaps this sweep. It does **not**
add a bespoke inflight map: the dispatched launch registers in the Overseer's
existing `inflight_investigations` set, so re-sweeping the same still-stale claim
produces the same `recipe_dedup_key` and the in-flight guard suppresses a second
launch instead of re-investigating. The loop resolves through the existing
plumbing: when the investigation concludes the engineer is genuinely dead it
releases the claim and tears the worktree down via its gated tools, and the
**next** sweep sees `Dead { NoWorktree }` and reclaims the leaked slot
immediately (that branch needs no investigation). Fakes return terminal verdicts,
so the pure-function tests exercise the `Dead`-reaps-now routing hermetically
(see the [API reference](../reference/investigate-stale-engineer-api.md#pending-and-cross-tick-resolution)).
The synchronous rail is never blocked on model latency.

## Preserved invariants

Everything the [reaper](./stale-engineer-claim-reaper.md) already guaranteed
still holds:

- **No wall-clock kill.** Staleness is still newest-file-mtime idle detection,
  never a run-duration cap. A busy engineer that keeps writing is never a
  candidate.
- **Fail-closed.** `Live`, boundary `age == stale_secs`, unknown-age, and an
  unreadable worktrees root all still resolve to "keep the claim". The new
  `still-alive` verdict is a further fail-closed extension.
- **`NoWorktree` still reclaims immediately.** There is no evidence to preserve
  and nothing to investigate when no worktree backs the claim.
- **One fail-visible `[simard]` line per reclaim**, now extended to name the
  verdict. Reclaim still flows only through the `release_engineer_claim`
  chokepoint + the guarded worktree cleanup. Per-entry errors are contained. The
  off switch (`SIMARD_CLAIM_REAP_ENABLED=off`) is still a total no-op.
- **No python, no kuzu, no new threads, no new SQL.**

## Related

- [Stale-Engineer-Claim Reaper (concept)](./stale-engineer-claim-reaper.md) — the
  sweep this behaviour extends.
- [Investigate-Before-Reap API (reference)](../reference/investigate-stale-engineer-api.md)
- [Overseer Agentic Health Review](./overseer-agentic-health-review.md) — the
  Intervention → Act path reused here.
- [No-Progress Terminal Investigation](./no-progress-terminal-investigation.md)
- [Self-Diagnose on Step Error](./self-diagnose-on-step-error.md)
- [How to investigate a stale engineer before reap](../howto/investigate-a-stale-engineer-before-reap.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
