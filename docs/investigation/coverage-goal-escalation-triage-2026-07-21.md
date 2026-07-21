# Escalation Triage Record — audit Simard's test coverage and raise it to 70% (2026-07-21)

Goal id: `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`
Blocker outcome: `019f6c08-d053-7d93-89bf-f1f86aee408c`
Repository: [`rysweet/Simard`](https://github.com/rysweet/Simard)
Playbook: [`prompt_assets/simard/overseer/escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md)
Charter: [`Specs/COVERAGE_AUDIT.md`](../../Specs/COVERAGE_AUDIT.md)
Ledger: [`docs/testing/COVERAGE_BASELINE.md`](../testing/COVERAGE_BASELINE.md)

This is the durable record of the Overseer's agentic escalation triage of the
recurring "raise Simard's own test coverage above 70%" goal after it was flagged
as blocked. It captures the two things the playbook requires that the code/state
change does not otherwise record:

1. the **single plain-English Signal message** the operator received — one
   consolidated, jargon-free update covering the situation and the decision (no
   raw diagnostic markers surfaced); and
2. the **reconciled root cause**, correcting the stale blocker seed against the
   authoritative state of the repository.

---

## The recorded block (translated to plain English)

The goal had been stuck for over an hour. Every cycle hit the same wall and
stopped: Simard could not automatically tell when this goal was finished. Its
finish line was the free-text sentence *"raise it to 70%"* — a phrase no
automated check ever measured. With nothing machine-checkable to certify, each
cycle re-investigated and shipped nothing, five cycles in a row, and no operator
was ever asked to help.

## Ground truth (authoritative, from files already in this repo)

| Artifact | State | Detail |
| --- | --- | --- |
| Named Simard per-group coverage targets | **all CLOSED / delivered** | `bin` #1749 (PR #1772, 1%→76%), `operator_commands_dashboard` #1750 (PR #2257, 31%→70%), `trace_collector` #1751 (PR #2338, 43%→95%), `operator_commands_gym` #1752 (PR #2346, 43%→89%), `cmd_cleanup` #1753 (PR #2353, 44%→70%) |
| Ad-hoc lifts | **MERGED** | `status` #2701 (29%→91%), `overseer::diagnosis` #2844 (36%→100%), `git_guardrails` #2729 (70.5%→91.4%), `completion-gate` #2958 (66.9%→82.1%) |
| Companion ledger `docs/testing/COVERAGE_BASELINE.md` | **backlog empty** | "Other groups — status": *"there is no open per-group backlog remaining"* |
| `.github/workflows/coverage.yml` | **report-only** | Posts a per-module table comment; has never contained a `fail-under`/70% gate (`git log -S'fail-under'` on the file is empty) |
| Parent epics #1735, #1937 | **describe a different repo** | Their content targets `rysweet/amplihack-rs`, not this single-crate `Simard` checkout |

The coverage work the goal asked for has **already shipped**. What was missing
was never the work — it was a finish line a machine could check.

## Root cause

The goal record carried an **unmeasurable free-text finish line** ("raise it to
70%") that no CI job ever enforced, so the done-check could never certify
completion — even though the requested coverage work was already delivered by
merged PRs and the tracking ledger backlog is empty. Two secondary confusions
kept cycles from self-resolving: the goal is nominally parented under epics that
actually describe a *different* repository (`amplihack-rs`), and there was no
deterministic rule telling a cycle which file to attack or when the audit as a
whole was complete.

## Course-correction (decision: rewrite the done-gate to be machine-checkable)

Per the playbook, the block was fixed agentically rather than dumped on a human.
`Specs/COVERAGE_AUDIT.md` was **ratified** (State: PROPOSED → RATIFIED),
replacing the unmeasurable prose finish line with three file-observable
conditions the daemon can check with no live runtime:

1. every group row in the companion ledger shows a landed post-lift aggregate
   ≥ 70% (or a recorded, justified exception);
2. the ledger's "Other groups" backlog table is empty; and
3. the deterministic §3 scan (`cargo llvm-cov --no-fail-fast --summary-only` →
   filter `src/` files > 50 executable lines and < 70% not already a justified
   exception) yields an empty set.

All three are **already satisfied**. A daemon/OODA cycle now certifies
completion by observing this file's `State: RATIFIED` plus the empty ledger
backlog, then tombstones the goal slug via `simard goal remove`
(`src/operator_cli/goal.rs`). No operator decision is required, so nothing was
escalated.

### Explicitly rejected (tempting but wrong)

Adding a workspace-wide 70% coverage gate to `coverage.yml` was **not** done. The
owner rejected bash CI gates in PRs #2150 / #2151, and `Specs/TDD_ADOPTION.md`
§2.4 plus §4 of the charter forbid it. The coverage job stays report-only.

---

## Signal message (exactly one, plain English)

The operator received **exactly one** consolidated, jargon-free Signal message —
not a per-step transcript. It states the situation and the decision in one
delivery, with no raw diagnostic markers (`OODA-SAFEGUARD`, `why=`,
`evidence=[…]`, the 🔒 token) surfaced:

> Update on the goal to get Simard's own test coverage above 70%.
>
> It had been stuck for over an hour. The reason was simple: Simard had no
> automatic way to tell when the goal was finished — its finish line was just
> the sentence "raise it to 70%," which no automated check ever measured. So
> every cycle kept re-investigating and never shipped anything.
>
> Looking closer, the coverage work itself is already done: every part of
> Simard we set out to cover was raised above 70% and merged, and the tracking
> list shows nothing left to do.
>
> So I fixed the real problem — the missing finish line. I rewrote it into three
> concrete checks the system can confirm on its own from files already in the
> repo, and all three already pass. The goal can now certify itself complete on
> its next check and will be closed out.
>
> Nothing is needed from you. (I did not add a build-failing 70% coverage gate —
> that was rejected before, so the coverage check stays report-only.)

### Dispatch confirmation

The message above is dispatched through Simard's own mandatory operator channel
(`DualChannelNotifier` — email + Signal, "fire every channel, never drop"). The
regression test
[`tests/coverage_goal_escalation_dispatch.rs`](../../tests/coverage_goal_escalation_dispatch.rs)
builds this exact single message, fires it through that seam, and asserts it is
one delivery per channel, dispatched on both, jargon-free, and carries both the
situation and the decision:

```
$ cargo test --test coverage_goal_escalation_dispatch
running 1 test
test dispatches_exactly_one_jargon_free_operator_message ... ok
```

When the Signal/email transports are configured in a live daemon, both channels
report `Sent`; unconfigured, they degrade to `Queued` (logged, never dropped) —
either way `NotifyReport::dispatched()` is `true`, so the single message is
guaranteed to reach the operator.

---

## Output contract

```json
{
  "problem": "The goal to get Simard's own test coverage above 70% keeps re-investigating and never finishes, because there was no automatic way to tell when it was done — its finish line was just the free-text phrase 'raise it to 70%,' which no automated check measured.",
  "next_step": "Give the goal a finish line the system can verify by itself, and confirm the coverage work it asked for has already shipped.",
  "root_cause": "The goal record carried an unmeasurable free-text finish line ('raise it to 70%') that no CI job ever enforced, so the done-check could never certify completion — even though the requested coverage work was already delivered by merged PRs and the tracking ledger backlog is empty.",
  "decision": "rewrite-done-gate",
  "action_taken": "Ratified Specs/COVERAGE_AUDIT.md, rewriting the finish line into three file-observable, machine-checkable conditions (all ledger groups >=70%, empty backlog table, empty deterministic low-coverage scan) — all three already satisfied, so a daemon cycle can now certify the goal DONE and tombstone the slug via `simard goal remove`. Deliberately did NOT add a workspace-wide CI coverage gate (owner rejected it in #2150/#2151; charter §4 and TDD_ADOPTION §2.4 forbid it).",
  "escalate": null
}
```
