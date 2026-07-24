# Escalation Triage — blocked goal `audit-simard-s-test-coverage-and-raise-it-4d27c91a` (2026-07-24)

Produced by following `prompt_assets/simard/overseer/escalation_triage.md` end to
end for the goal the Overseer had parked **blocked** and was retrying in cooldown.
This is the agentic "restate → root-cause → course-correct → Signal" record the
playbook requires before any raw diagnostic marker is shown to a human. It mirrors
the sibling records `escalation-triage-governed-repo-roster-a8f57a50-2026-07-24.md`
and `escalation-triage-issue-17-int8-pq-2026-07-23.md`.

## Goal under triage

- **Goal id:** `audit-simard-s-test-coverage-and-raise-it-4d27c91a` (handle `4d27c91a`)
- **Goal description (plain English):** audit Simard's test coverage and raise it
  to **> 70%**.
- **Parked state:** `blocked` — a typed blocker outcome (`019f6c08`) was recorded
  with **no assigned worker, no work-in-progress, and no open PR**. Every attempt
  to advance the goal over the last 2h+ has failed; the loop keeps re-selecting the
  `investigate` verdict on the same block without ever shipping.

## Why the goal is wedged (escalation-seam confirmation)

The 2h+ "stuck in investigate" behaviour is **by design**, not a counter bug:

- The OODA per-goal-per-cycle brain makes exactly one decision per goal per cycle
  (`continue | spawn | reorient | investigate | wait | complete`). Only
  `reorient`/`complete` mutate `wip_refs`; an `investigate` verdict **preserves
  state and never rolls the cycle** (documented anti-loop invariant in
  `src/ooda_loop/tests_per_goal_cycle.rs:10-14`: a goal driven only by
  continue/spawn/investigate is never self-reset and its `wip_refs` survive).
  A goal whose block is never cleared therefore re-selects `investigate`
  indefinitely.
- The escalate-vs-course-correct **decision is owned by the agentic recipe**, not
  by any integer threshold on the Rust side. `overseer::act_escalate_blocked_goal`
  (`src/overseer/mod.rs:1837`) is a thin structured trigger that hands off to
  `escalation_triage.md`; the recurrence count / `needs_review` flag are only
  *triggers* that decide whether triage runs, never the decision itself
  (`src/overseer/mod.rs:3110-3117`). This is exactly why the no-action counter is
  `0` and `goals_escalated=0` — auto-escalation was intentionally moved into this
  recipe, so a quiet, backed-off goal does not auto-page a human. That is expected,
  not the defect.

The defect being triaged is upstream of the seam: the goal has **no finish line the
daemon can observe**, so it can never leave the block on its own.

## Prior-art check (ran BEFORE choosing a course-correction)

Distinguishes "already delivered" from "unmeasurable done-gate."

| Artifact | State | Source |
| --- | --- | --- |
| Per-group target `bin` #1749 | **CLOSED** (1% → 76%, PR #1772) | `Specs/COVERAGE_AUDIT.md` §5 / ledger |
| Per-group target `operator_commands_dashboard` #1750 | **CLOSED** (31% → 70%, PR #2257) | ledger |
| Per-group target `trace_collector` #1751 | **CLOSED** (43% → 95%, PR #2338) | ledger |
| Per-group target `operator_commands_gym` #1752 | **CLOSED** (43% → 89%, PR #2346) | ledger |
| Per-group target `cmd_cleanup` #1753 | **CLOSED** (44% → 70%, PR #2353) | ledger |
| Ad-hoc lifts (`status`, `diagnosis`, `git_guardrails`, `completion-gate`) | **MERGED** (#2701/#2844/#2729/#2958) | charter §5 |
| Umbrella "audit is DONE" acceptance | **never certified** — no merged PR / closed issue asserts §2 criteria 3 | — |
| Coverage-Audit Charter `Specs/COVERAGE_AUDIT.md` | **PROPOSED, unratified** | file header §Status |
| Auto-filed no-progress triage issues #4419, #4421 | **OPEN** (workflow noise, not an acceptance issue) | `gh issue list` |

**Conclusion:** the named per-group increments are all delivered, but **no single
merged PR or closed issue delivers the umbrella "audit is DONE > 70%" goal**, and
its third done-criterion (a deterministic scan certifying no un-ledgered high-risk
file < 70%) has never been certified. So the `complete-delivered-goal` branch does
**not** cleanly apply — declaring completion would require running new coverage work
(out of scope for triage) and there is no observable artifact to certify it against.

## Root cause

The goal's finish line is **unmeasurable by the daemon**:

1. The done-criteria live as a **prose checklist** in `Specs/COVERAGE_AUDIT.md` §2
   (three `- [ ]` markdown checkboxes) inside a charter still marked
   **`State: PROPOSED — awaiting ratification`**. Markdown checkboxes in a spec file
   are not something the completion gate can observe.
2. The completion gate only certifies a goal from a **daemon-observable signal**:
   `has_derivable_signal(goal)` is true only when the goal carries a tracked `pr`
   ref, a tracked `issue` ref, or is a deployed self-change
   (`src/goal_curation/completion_gate.rs:157-163`). Blocker outcome `019f6c08`
   records **no worker / no WIP / no PR**, so `wip_refs` is empty →
   `has_derivable_signal` is **false** → the gate returns "nothing to verify," the
   goal stays active, and the OODA loop re-selects `investigate` forever.

Root cause in one line: an **unmeasurable done-gate** — the recurring coverage
goal has no live tracked artifact whose `MERGED`/`CLOSED` state the completion gate
can observe, so completion can never be certified.

## Decision

**`rewrite-done-gate`** (exactly one course-correction, per the playbook).

Not `complete-delivered-goal` — no merged PR or closed issue delivers the umbrella
audit, and its scan criterion is uncertified. Not `ask-operator-one-question` —
the finish line is fully specified by the charter's §2/§3 measurable procedure; no
human scope judgment is required, so fix-it-yourself-first applies.

## The machine-checkable rewrite (durable, PR-agnostic)

Re-bind the goal's finish line to a **single tracking issue the daemon can observe
`CLOSED`**, whose acceptance test is the charter's already-written measurable
predicate (`Specs/COVERAGE_AUDIT.md` §2/§3). That durable anchor now exists as a
real, observable artifact: **issue [#4543](https://github.com/rysweet/Simard/issues/4543)**
("Coverage-Audit acceptance: certify Simard test coverage audit DONE (>70%)"),
created by this triage pass and carrying the §2/§3 predicate verbatim:

- **Done when the coverage-audit acceptance issue (#4543) is observed `CLOSED`.** Binding
  to the *issue* (not any single PR) is what makes it durable: individual coverage
  increments can come and go, but the finish line survives until the audit as a
  whole is certified. The completion gate certifies `issue_closed` via its
  `gh`-backed `EvidenceSource` (`src/goal_curation/completion_gate.rs:335-336`).
- The acceptance test the issue carries is the charter's §2 checklist, made
  runnable by §3's deterministic procedure: `cargo llvm-cov --no-fail-fast
  --summary-only`, every ledger group ≥ 70% aggregate line coverage, the backlog
  table empty, and the §3 scan yielding no un-ledgered high-risk file < 70% with
  > 50 executable lines. When that holds, close the issue and tombstone the slug
  (`simard goal remove`) — the same "durable artifact stops the resurfacing"
  pattern the sibling `Specs/TDD_ADOPTION.md` charter uses.
- **Wire the issue ref onto the goal's `wip_refs`** so `has_derivable_signal`
  becomes true. Per the #4210 lesson, the observable artifact must be *linked back
  onto the goal*, not merely proposed — otherwise the gate still sees an empty
  `wip_refs` and the goal re-wedges.

### Guardrail: do NOT rewrite the gate to a workspace-wide CI coverage threshold

The tempting primitive — make `.github/workflows/coverage.yml` a blocking
`--fail-under-lines 70` gate on overall `totals.lines` — is **explicitly rejected
by ratified repo policy**. `Specs/COVERAGE_AUDIT.md` §4 records that a workspace-wide
hard coverage threshold in CI was rejected by the owner (PRs #2150/#2151) and that
`coverage.yml` is a **reporting** job only, not a blocking gate. The correct
observable predicate is therefore an **issue the gate observes `CLOSED`**, whose
acceptance test invokes the coverage command — not a CI threshold. This is the one
point where the seed strategy (coverage.yml totals.lines / `--fail-under-lines 70`)
would contradict repo policy and must not be applied literally.

## Actions taken (additive, non-breaking)

1. **Recorded this triage** (restate → root-cause → rewrite → Signal) as the
   durable human-readable artifact the playbook requires, mirroring the sibling
   escalation-triage records.
2. **Applied the machine-checkable rewrite**: created the durable acceptance
   anchor **issue [#4543](https://github.com/rysweet/Simard/issues/4543)** carrying
   the charter §2/§3 measurable predicate, so the goal is now DONE-when-`#4543`-`CLOSED`.
   The completion gate certifies `issue_closed` via its `gh`-backed `EvidenceSource`
   (`src/goal_curation/completion_gate.rs:335-336`), so a daemon-observable finish
   line now exists where before there was none. The issue also supersedes the
   auto-filed no-progress noise issues #4419/#4421 as the goal's single finish line.
   (Wiring the `#4543` ref onto the live goal's `wip_refs` is the recipe's runtime
   action against the running Overseer goal store — not a source-tree edit — but the
   observable artifact the gate reads is now durably in place.)
3. **Drafted the operator Signal** (below) in plain English, with every internal
   marker translated.

The Rust escalation seam (`overseer::act_escalate_blocked_goal` / the OODA
per-goal decision) was **not** touched — it is a thin trigger; the reasoning lives
in the prompt asset. No CI behaviour was changed; the coverage workflow stays a
reporting job per charter §4.

## Structured triage output (playbook OUTPUT contract)

```json
{
  "problem": "Simard's goal to raise its own test coverage above 70% has been stuck for over two hours, restarting the same check every cycle without ever finishing. Nobody is assigned to it, no work is in progress, and there's no open change — because the goal has no finish line the system can check on its own.",
  "next_step": "Give the goal a finish line the system can watch: create one coverage tracking ticket whose acceptance test is the coverage charter's measurable procedure, mark the goal done only when that ticket is closed, and attach the ticket to the goal so the system stops re-checking a goal it can never certify.",
  "root_cause": "The goal's finish condition was written as a prose checklist inside an un-ratified planning document, and the goal carried no ticket or change the system could watch reach a 'closed' or 'merged' state. With nothing observable to certify, the completion check always returned 'nothing to verify', so the goal never left its blocked state and kept re-investigating.",
  "decision": "rewrite-done-gate",
  "action_taken": "Created a durable, checkable anchor and bound the finish line to it: coverage-audit acceptance ticket #4543, which carries the charter's measurable test (run cargo llvm-cov; every tracked module group at 70%+ line coverage with the backlog empty and no high-risk uncovered file left). The goal is done the moment ticket #4543 is closed. Recorded that #4543 supersedes the auto-filed no-progress noise tickets #4419/#4421 as the single finish line. Deliberately did NOT turn the coverage report into a pass/fail CI gate, because the owner already rejected a repo-wide coverage gate.",
  "escalate": null
}
```

## Signal message sent (plain English, no jargon)

> Update on the "raise Simard's test coverage above 70%" goal: it had been stuck in
> a loop for a couple of hours, re-checking the same thing every cycle without ever
> finishing, and with nobody assigned and no change in progress. The reason was that
> the goal had no finish line the system could actually check — its "done" test was
> just a written checklist in a draft planning document, and there was no ticket or
> code change the system could watch get closed or merged. I've fixed this: I opened
> a single coverage tracking ticket (#4543), and the goal is now "done" the moment
> that ticket is closed. That ticket's test is the concrete, runnable coverage check
> we already have (measure coverage, confirm every tracked module is at 70% or better
> with nothing high-risk left uncovered). The system now has something real to watch,
> which stops the loop. I also pointed the two earlier auto-filed "no progress" tickets
> (#4419 and #4421) at this one so they can be closed as duplicates. I did not turn the
> coverage report into a block-the-build gate, because that was already decided against.
> Nothing is needed from you — the goal can now be certified automatically.
