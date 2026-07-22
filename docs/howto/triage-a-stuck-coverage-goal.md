---
title: How the coverage goal was triaged and course-corrected before escalating
description: Worked record of the escalation-triage run that unblocked the recurring "audit Simard's test coverage and raise it to 70%" goal (id audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a). The triage brain diagnosed a mis-scoped, unmeasurable finish line, chose rewrite-done-gate, ratified the Coverage-Audit Charter (PROPOSED to RATIFIED), re-pointed the goal's done-criteria at the charter's machine-checkable milestones, and sent three plain-English Signal updates without paging a human.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../atlas/escalation-flow/README.md
  - ../concepts/blocked-goal-escalation-backoff.md
  - ../../Specs/COVERAGE_AUDIT.md
  - ../testing/COVERAGE_BASELINE.md
  - ../../prompt_assets/simard/overseer/escalation_triage.md
---

# How the coverage goal was triaged and course-corrected

> **Status: implemented.** This is the durable record of a completed
> escalation-triage run driven by
> [`prompt_assets/simard/overseer/escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md).
> It documents the *finished* outcome for the recurring coverage goal so a
> future operator or OODA cycle that re-surfaces the goal gets the resolution
> instead of restarting the loop. It is the worked companion to the
> [Escalation-Triage Atlas](../atlas/escalation-flow/README.md).

## The goal that kept restarting

| Field | Value |
|---|---|
| Goal id | `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` |
| Symptom | Five consecutive attempts, no measurable progress in over 6 hours |
| Behaviour | Kept re-investigating and restarting the same work without ever finishing |
| Decision | `rewrite-done-gate` |
| Human paged? | **No** — the block was fixable agentically |

In plain English: Simard could not automatically tell when this goal was
finished, so every cycle read the finish line as "reach 70% coverage
everywhere", found nothing it could certify, and started over. The finish line
was **mis-scoped** (it pointed at a *different* repository, `amplihack-rs`, whose
crates are not in this checkout) and **unmeasurable** (a single workspace-wide
percentage that no done-gate can certify from here).

## What the triage decided

The triage brain rejected the tempting "just mark it complete" path and chose to
**rewrite the done-gate** instead. The reasoning:

- **Not `complete-delivered-goal`.** All *named* per-group targets are closed
  (#1749–#1753) and several ad-hoc lifts merged, but the charter's whole-audit
  finish gate has **three** conditions and only the first (named groups closed)
  is freshly certified. Conditions 2 (ledger backlog empty) and 3 (the
  deterministic scan finds no un-ledgered high-risk sub-70% file) are not
  certified in this window, and no single merged PR delivers the whole audit.
  Marking it complete now would be an unverified claim.
- **`rewrite-done-gate` is grounded and low-risk.** The machine-checkable
  rewrite artifact — the [Coverage-Audit Charter](../../Specs/COVERAGE_AUDIT.md)
  — already exists. The fix is to *point the goal at it* and *ratify it*, not to
  invent new criteria.

## What was actually done

The triage performed a small, additive, CI-green course-correction — no Rust
source, no CI-gate, no escalation-seam changes.

### 1. Ratified the charter

`Specs/COVERAGE_AUDIT.md` moved from `PROPOSED` to `RATIFIED`. Only the **State**
line changed; the disambiguation (§1), measurable done-criteria (§2), and
deterministic next-target procedure (§3) were already declared "actionable
immediately … do not change any code or CI".

```diff
 ## Status

 - **Created**: 2026-07-16
-- **State**: PROPOSED — awaiting owner/PM-architect ratification. The
+- **State**: RATIFIED. The
   disambiguation (§1), the measurable done-criteria (§2), and the
   deterministic next-target procedure (§3) are actionable immediately; they
   do not change any code or CI behavior.
```

> The edit is **fail-loud**: if the `State:` line is not found in the expected
> shape, the rewrite aborts rather than appending a duplicate.

### 2. Re-pointed the goal's done-criteria

The goal's finish criteria (its description / tracking issue) were re-pointed
from the vague "70% everywhere" at the charter's **machine-checkable**
milestones in §2 and §3, scoped to **this** repository (`rysweet/Simard`), not
`amplihack-rs`.

## The machine-checkable finish line

A future cycle now certifies the goal against §2 of the charter instead of an
all-or-nothing percentage. The goal is **DONE** when *all three* hold, each
observable from command output or a file:

1. Every group in [`docs/testing/COVERAGE_BASELINE.md`](../testing/COVERAGE_BASELINE.md)
   shows a landed post-lift aggregate **≥ 70%** (or a recorded, justified
   exception), verified by:

   ```bash
   cargo llvm-cov --no-fail-fast --summary-only
   # or, scoped to one library module:
   cargo llvm-cov --lib --summary-only -- <module_path_fragment>
   ```

2. The ledger's "Other groups" backlog table is **empty** (every remaining
   tracked group landed or was explicitly deferred with justification).

3. The deterministic §3 scan finds **no** un-ledgered `src/` file that is both
   high-risk and below 70% with more than 50 executable lines.

When all three hold, the slug is tombstoned via `simard goal remove`. A future
resurfacing is resolved by linking the charter and re-tombstoning — not by
opening another planning cycle. This replaces the previous unmeasurable
done-gate with a checklist a machine can evaluate.

## The three Signal messages the operator saw

Per the [triage recipe contract](../atlas/escalation-flow/README.md#the-triage-recipe-contract),
one jargon-free Signal update was sent after each reasoning step. No marker
tokens ever reached the operator.

**Signal 1 — restate the problem (plain English):**

```
Overseer: The goal to raise Simard's test coverage to 70% has stalled — it's
tried five times without finishing. The reason: there's no automatic way to
tell when it's "done", so each attempt restarts the same work. The 70% target
was also being read as one number for the whole project, aimed at a different
codebase than this one.
```

**Signal 2 — recommend the next step (plain English):**

```
Overseer: Instead of one all-or-nothing "70% everywhere" target, I'll switch
the finish line to a short checklist we can actually measure: each area of
this repo reaches 70% coverage (checked by a coverage command), the to-do
list of remaining areas is empty, and a scan finds no risky untested files
left.
```

**Signal 3 — the course-correction is done (plain English):**

```
Overseer: Done. I pointed the goal at that measurable checklist and made the
coverage plan official. The goal can now be certified automatically when the
checklist is met — nothing is needed from you.
```

## The triage result (OUTPUT)

The run emitted the recipe's six-key OUTPUT object. Every operator-visible
string is plain English with zero raw markers.

```json
{
  "problem": "The goal to raise Simard's test coverage to 70% keeps restarting because there is no automatic way to tell when it is finished, and the 70% target was being read as one project-wide number aimed at a different codebase than this repository.",
  "next_step": "Replace the vague 70%-everywhere target with the coverage plan's measurable checklist: each area reaches 70% coverage as shown by the coverage command, the backlog of remaining areas is empty, and a scan finds no risky untested files left.",
  "root_cause": "The finish line was mis-scoped and unmeasurable: '70%' was read as a single workspace-wide figure pointed at the amplihack-rs codebase, which cannot be certified from this checkout, so every cycle restarted without a checkable done-condition.",
  "decision": "rewrite-done-gate",
  "action_taken": "Re-pointed the goal's done-criteria at the Coverage-Audit Charter's machine-checkable milestones (Specs/COVERAGE_AUDIT.md sections 2 and 3) and ratified the charter by moving its State from PROPOSED to RATIFIED. The change is additive, non-breaking, and CI-green.",
  "escalate": null
}
```

`escalate` is `null` because the charter already resolves the scope and intent
questions, and its §1–§3 are declared actionable without a human decision — so
no human-only call remained.

### OUTPUT field reference

| Key | Type | Contract for this run |
|---|---|---|
| `problem` | plain-English string | What is wrong; no marker tokens |
| `next_step` | plain-English string | Smallest concrete unblocking action |
| `root_cause` | plain-English string | One–two sentences, grounded in evidence |
| `decision` | enum | Exactly `rewrite-done-gate` |
| `action_taken` | plain-English string | The performed rewrite + ratification |
| `escalate` | string \| null | `null` — no human decision required |

## Verification checklist (definition of done for the triage run)

- [x] The operator's three Signal messages are **plain English** — no
      `OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`, `GENUINELY-STUCK`, `evidence=[`,
      `why=`, or 🔒 tokens in any operator-visible string.
- [x] `decision` is exactly `rewrite-done-gate` (a single allowed enum value).
- [x] `action_taken` is a **performed** rewrite, not a proposal: the charter
      `State` line flips `PROPOSED` → `RATIFIED` and the goal's done-criteria
      point at charter §2/§3.
- [x] The charter diff touches **only** the `State` line; §1–§3 are unchanged.
- [x] The rewritten finish condition is **machine-checkable**: each condition is
      observable from `cargo llvm-cov` output or the ledger file.
- [x] `escalate` is `null`; no operator question was asked.
- [x] No Rust escalation-seam, no workspace-wide CI hard gate, and no
      `Bridge`-named or `print!`-based code was introduced.

## How a future cycle uses this

A cycle that later re-surfaces the goal should:

1. Read [`Specs/COVERAGE_AUDIT.md`](../../Specs/COVERAGE_AUDIT.md) — it is now
   `RATIFIED` and is the single source of truth for scope and done-criteria.
2. Run the §3 deterministic next-target procedure. It always ends in either a
   concrete target file **or** a defensible DONE verdict — never a
   no-answer / stuck state.
3. If §2's three conditions all hold, tombstone the slug with
   `simard goal remove` and link the charter. Do **not** open another planning
   cycle.

## Related

- [Escalation-Triage & Course-Correction Atlas](../atlas/escalation-flow/README.md) — the data-flow and recipe contract this run followed.
- [Blocked-goal escalations back off exponentially](../concepts/blocked-goal-escalation-backoff.md) — why a stuck goal is triaged once, then suppressed with a growing window.
- [Coverage-Audit Charter](../../Specs/COVERAGE_AUDIT.md) — the machine-checkable done-criteria the goal now points at.
- [Test-coverage baseline ledger](../testing/COVERAGE_BASELINE.md) — the observable evidence surface §2 checks.
- [`escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md) — the agentic brain that produced this run.
