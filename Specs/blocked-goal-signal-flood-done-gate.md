# Blocked-Goal Signal Flood — Done-Gate Specification

## Purpose

The goal **"stop the blocked-goal signal flood; make the Overseer course-correct
before escalating"** (slug
`stop-the-blocked-goal-signal-flood-make-oversee-17d6ca84`) stayed `Blocked`
cycle after cycle with the same diagnosis: **no tracked PR/issue the done-gate
could verify** (why = `UNCLEAR-CRITERIA`). The blocker was **not** technical —
the anti-flood behaviour the goal asks for already shipped. The blocker was that
the goal's finish condition had **no machine-checkable definition**, so every
cycle re-observed it as unfinished and produced `NO ACTION` — ironically the very
"re-investigate without shipping" pattern the goal was created to end.

This spec fixes that WHY. Because the load-bearing work is **already delivered**,
the correct course-correction is to make the goal's completion
**machine-verifiable** so the done-gate can certify it instead of re-stalling. It
binds the goal's finish condition to a **single command a daemon can run and
score automatically**:

```
scripts/check-blocked-goal-signal-flood-done-gate.sh
```

The command exits `0` only when the delivered anti-flood behaviour is present and
its shipped tests still pass; otherwise it exits non-zero and prints the failing
check. This turns "stop the flood and course-correct before escalating" from a
prose judgement into a check the done-gate can confirm, so the goal is certified
complete rather than left blocked.

## What the goal asked for

Two things:

1. **Stop the blocked-goal signal flood.** A goal that is blocked must not have a
   raw machine marker re-emitted to the operator on every OODA tick, drowning the
   signal feed, the problem list, and notifications.
2. **Make the Overseer course-correct before escalating.** When a goal is
   genuinely blocked, the Overseer must first inspect the block, restate it in
   plain English, and try to repair it agentically — only paging a human when a
   human decision is truly required.

## What delivered it

| # | Delivered protection | Where it lives |
|---|----------------------|----------------|
| 1 | **Cadence rail** — blocked-goal escalations pass through a back-off gate, so a repeatedly-blocked goal is re-surfaced on an exponentially widening interval instead of every tick. | `blocked_goal_gate: WhisperGate::with_backoff(900, 14_400, 20)` in [`src/overseer/mod.rs`](../src/overseer/mod.rs) |
| 2 | **Agentic course-correct-before-escalate** — a blocked goal is handed to the escalation-triage recipe, which restates the block in plain English and repairs it (rewrite an unmeasurable done-gate, complete an already-delivered goal, or ask the operator one plain question), only paging a person when the decision is genuinely theirs. | `act_escalate_blocked_goal` in [`src/overseer/mod.rs`](../src/overseer/mod.rs) → [`prompt_assets/simard/overseer/escalation_triage.md`](../prompt_assets/simard/overseer/escalation_triage.md) |
| 3 | **Plain-English operator copy** — the operator notification renders a jargon-free problem and next step, never the raw `OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `why=` / 🔒 markers. | `tests_escalation_triage` jargon-free assertions |

> Note: a further consolidation — collapsing all blocked goals into **one**
> `Signal::GoalBlocked` per tick — was merged separately (`#4301`) and reinforces
> protection #1 on the deployed branch. This gate intentionally binds only to the
> two load-bearing seams that are present and green on every branch (the back-off
> cadence rail and the agentic triage seam), so the certification is stable rather
> than branch-dependent.

## Machine-checkable done-criteria

The done-gate script asserts all of the following; **all must hold** for the goal
to be certified complete:

| ID | Criterion | How it is checked |
|----|-----------|-------------------|
| SF-1 | The done-gate spec exists. | file presence of this spec |
| SF-2 | The cadence rail still exists. | `grep` `blocked_goal_gate: WhisperGate::with_backoff` in `src/overseer/mod.rs` |
| SF-3 | The agentic escalation seam still exists. | `grep` `fn act_escalate_blocked_goal` in `src/overseer/mod.rs` |
| SF-4 | The triage reasoning contract asset is present. | file presence of `prompt_assets/simard/overseer/escalation_triage.md` |
| SF-5 | The back-off gate widens re-escalation intervals exponentially and per-signature. | `cargo test overseer::guardrails::whisper_backoff_tests` |
| SF-6 | A blocked goal is triaged agentically and the operator is notified in plain English with no marker passthrough. | `cargo test overseer::tests_escalation_triage` |
| SF-7 | Goal-board health emits its signal and escalates a needs-review block on both channels. | `cargo test overseer::tests_goal_health` |
| SF-8 (`--full`) | The plain-English operator update doc is present. | file presence of `docs/operations/blocked-goal-signal-flood-goal-signal-2026-07-18.md` |

## Running the gate

```bash
scripts/check-blocked-goal-signal-flood-done-gate.sh          # SF-1..SF-7
scripts/check-blocked-goal-signal-flood-done-gate.sh --full   # + SF-8
```

Exit `0` = the anti-flood protections hold and the goal is certified complete.
Exit `1` = a protection is missing or a test regressed (the failing check is
printed). The gate stays green while the delivered behaviour holds and turns red
the instant it regresses — giving the goal a finish line the daemon can confirm
on its own.
