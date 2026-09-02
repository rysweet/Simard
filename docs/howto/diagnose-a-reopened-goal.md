---
title: How to diagnose a re-opened goal
description: >
  Operator runbook for "the PR merged and deployed, so why isn't this goal
  achieved?" — read the outcome-verification decision, tell an artifact that
  landed from an outcome that is verified live, resolve a re-opened/re-planned
  goal, and (recovery only) disable the outcome verifier.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/closed-loop-outcome-verification.md
  - ../reference/outcome-verification-api.md
  - ../howto/diagnose-a-rejected-goal-completion.md
  - ../operations/outcome-verification-kill-switch.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
---

# How to diagnose a re-opened goal

> **Status: implemented.** The closed-loop outcome-verification step
> (`verify_goal_outcome`, the `LiveSignalSource` adapters, the
> `goal_live_outcome_verification` metric, and the `SIMARD_OUTCOME_VERIFY`
> kill-switch) described here lives in
> [`src/goal_curation/outcome_verify.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/outcome_verify.rs)
> (see [closed-loop-outcome-verification](../concepts/closed-loop-outcome-verification.md)).
> The production daemon records each decision to `metrics.jsonl` and the cycle
> report.

A goal whose **PR merged and whose deploy reconciled** is still on the active
board — it did **not** flip to *achieved*. This is usually the outcome verifier
doing its job: the artifact landed, but the goal's **real success criteria are
not yet verified live**. This guide shows how to confirm that and resolve it.

If your goal is blocked on *artifact* evidence (unmerged PR, open issue,
undeployed self-change), you want the sibling runbook instead:
[diagnose a rejected goal completion](../howto/diagnose-a-rejected-goal-completion.md).
This page is for the step **after** the artifact gate passes.

## 1. Read the outcome-verification decision

Every verification emits its reasoning. Read the most recent decision for the
goal from the metrics stream (drop `--user` for a system-level install):

```bash
simard metrics query --name goal_live_outcome_verification | tail -n 5
```

Each entry's `context` carries the **decision**, the **verified-signal count**,
and the brain's **rationale**, for example:

```json
{"metric_name":"goal_live_outcome_verification","value":0,
 "context":"decision=reopen verified_signals=0 — PR #4821 merged and deployed, but journald still shows E2BIG on the next real kgpacks spawn; live effect absent"}
```

You can also read it from the cycle report, where the `OutcomeVerify` brain
phase is recorded alongside Act/Decide/Orient:

```bash
cat ~/.simard/cycle_reports/cycle_*.json | jq '.brain_judgments[] | select(.phase=="outcome_verify")' | tail
```

## 2. Interpret the decision

| Decision | What it means | verified-signal count |
| --- | --- | --- |
| `mark_achieved` | Real success criteria observed live — goal archived `Completed`. | ≥ 1 (guaranteed by Rail-3) |
| `reopen` | Artifact landed but the live effect is **absent** — goal kept active. | usually 0 |
| `replan` | Live effect absent **and** the current plan won't produce it — re-plan marker set for next cycle. | usually 0 |
| `keep_open_and_report` | Signals ambiguous, absent, or unverifiable this cycle. | 0, or unverified only |

> **Rail-3 in one line:** a goal is **never** archived with a
> `verified_signals=0` count, even if the brain said `mark_achieved`. If you see
> `decision=mark_achieved verified_signals=0`, the rail overrode it to
> `keep_open_and_report` — that is the fail-closed guarantee working.

## 3. Find the missing live signal

`reopen` / `keep_open_and_report` means no adapter could **verify** the effect.
Check what the live-signal adapters observed. The three production adapters map
to three quick probes:

| Signal source | Probe | Verified when |
| --- | --- | --- |
| `self_metrics` (telemetry) | `simard metrics query --name <goal-target-metric>` | the target metric crossed its threshold in the live stream |
| `journald` | `journalctl --user -u simard-ooda -n 200 | grep -i '<expected line / error>'` | the success line appears (or the failing line is **absent**) since the deploy |
| `reconcile_detector` (deploy) | `simard self-health --json | jq '.probes.version_advanced'` | `!DeployDrift::needs_deploy` — the merged effect is running |

For the canonical case — a goal that fixed an `E2BIG` spawn — confirm the effect
directly by re-driving the operation that used to fail:

```bash
journalctl --user -u simard-ooda --since "$(date -d '1 hour ago' -Iseconds)" \
  | grep -i 'Argument list too long'
```

If that line is **still present after the deploy**, the artifact landed but the
outcome did not — exactly the kgpacks re-block the verifier exists to catch. The
goal is correctly re-opened; the fix is not actually present.

## 4. Resolve it

- **The fix genuinely isn't live yet** (e.g. `NotDeployed` clears late, a
  cache/restart is pending): let the next cycle re-verify. Each `reopen` /
  `replan` bumps the goal's `reverify_count`, so a goal that keeps landing
  artifacts without ever producing the effect becomes visible as churn.
- **The fix is wrong** (the artifact shipped but doesn't address the real
  criteria): the `replan` marker will drive the brain to re-scope next cycle;
  follow its `replan_hint`. Spawn a fresh engineer against the re-planned goal.
- **The success criteria are mis-stated** (the effect *is* live but no adapter
  covers it): the live-signal adapter set is missing a signal for this goal.
  Extend the production `LiveSignalSource` with a thin adapter that authenticates
  the effect (see the [API reference](../reference/outcome-verification-api.md#livesignalsource)).
  Do **not** work around it by loosening the prompt — Rail-3 requires an
  adapter-verified signal by design.

## 5. Reproduce the original re-block (sanity check)

The step exists because the kgpacks goals were archived "complete" while their
`E2BIG` fix was **not actually present**, then silently re-blocked. To confirm
the verifier is active, a goal in that exact shape — completion-candidate (PR
merged + deployed), but with **zero verified live signals** — must **not**
archive; it stays active as `reopen` / `keep_open_and_report`. This is covered
by a regression test (T5 in the
[test matrix](../reference/outcome-verification-api.md#test-matrix)).

## 6. Override the verifier (recovery only)

If the verifier itself is defective and is wrongly holding a genuinely achieved
goal open, you can temporarily disable it:

```bash
SIMARD_OUTCOME_VERIFY=off simard daemon
```

With the verifier off, the bridge pair is `None` and the legacy artifact-only
curate path returns (a goal archives on the [done-gate](../concepts/deploy-aware-done-gate.md)
alone). Use this **only** to recover from a verifier defect — never as normal
operation — and re-enable it (unset the variable) immediately afterward. The
degradation is audited at boot. This mirrors the
[progress-evidence](../operations/progress-evidence-kill-switch.md) and
[completion-evidence](../howto/diagnose-a-rejected-goal-completion.md#override-the-gate-recovery-only)
kill-switches. See the
[outcome-verification kill-switch page](../operations/outcome-verification-kill-switch.md).

## See also

- [Closed-loop outcome verification (concept)](../concepts/closed-loop-outcome-verification.md)
- [Outcome-verification API reference](../reference/outcome-verification-api.md)
- [Diagnose a rejected goal completion](../howto/diagnose-a-rejected-goal-completion.md) — the sibling *artifact*-gate runbook (run this one after that one passes).
- [Verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md) — clearing the deploy live-signal.
- [Outcome-verification kill-switch](../operations/outcome-verification-kill-switch.md)
