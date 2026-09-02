# Escalation-triage — blocked goal `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` (health-review re-trigger)

**Goal id:** `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`
**Procedure (authoritative):** `prompt_assets/simard/overseer/escalation_triage.md`
**Output contract reference:** `docs/reference/escalation-triage-api.md` (§"Worked output (#4904)")
**Target work:** audit Simard's own test coverage and raise it above the 70% line-coverage bar.

**Why this run exists (this is a NEW trigger, not the #4838 stale-terminal one):** the
health-review sensor re-flagged the same goal on a *different* signal — a typed
terminal outcome was committed and then **never escalated to a human** (zero
escalations across every Overseer tick in the last 24 hours), so the goal sat
recorded-stuck with nobody told. The #4838 run triaged the 10-day *stale* signal;
this run triages the *silently-un-escalated in 24h* signal. Same goal, same
evidence, same correct decision — re-run per the procedure because the triggering
marker changed.

**Internal diagnostic WHY (input to TRANSLATE, never surfaced raw):** a typed
terminal blocked outcome was committed on the goal, and it was never escalated —
escalation count is zero across all Overseer ticks in the last 24 hours.
**Reason marker (input to TRANSLATE, never surfaced raw):** the health-review
"blocked-terminal" marker.

> Per procedure §ROLE, the markers above are read only as *evidence for my own
> plain-English reasoning*. They are never forwarded to the operator verbatim. The
> raw diagnostic tokens (the typed-outcome id, the reason marker, and any lock /
> safeguard tokens) are deliberately kept out of every operator-facing string
> below and are verified absent by the marker-leak scan in §5.

---

## 1. Restate the PROBLEM in plain English

Simard has a goal to check its own test coverage and get it above 70%. The worker
on it hit a wall it couldn't get past on its own, recorded the goal as stuck, and
stopped — and because no person was ever told, it has sat parked with no progress.
The core trouble is that the goal was left in a stuck state even though the actual
coverage work it asks for has, in fact, already been finished and merged.

## 2. Recommended NEXT STEP (plain English)

Close the goal. The coverage work it describes has already been delivered by merged
changes, its finish line is written down and met, and there is no remaining
below-target area to attack — so closing it stops it from being re-picked-up every
cycle. Nothing needs to be lowered, deferred, or handed to a fresh engineering run,
and nothing is needed from you.

## 3. ROOT CAUSE and the course-correction DECISION

### 3a. Root cause (grounded in live evidence)

The goal was marked blocked and then left un-escalated for a day even though its
coverage objective had already been delivered by merged work. This is a **stale
block on an already-delivered goal, not a real engineering blocker**: the work is
done, but nothing ever recorded the goal as complete, so it kept sitting on the
board as "blocked" and — because it was never surfaced to a human — nobody noticed
it was actually finished.

### 3b. Evidence triangulation

| # | Evidence | What it proves |
|---|----------|----------------|
| 1 | **`Specs/COVERAGE_AUDIT.md`** (canonical Coverage-Audit Charter) landed via **merged PR #4156**. | The goal's meaning and finish line are written down and shipped: per-group **≥70% aggregate line coverage** of the `simard` crate + `simard-*` bins, measured by `cargo llvm-cov`. |
| 2 | Charter §2 done-criteria + companion ledger `docs/testing/COVERAGE_BASELINE.md` show the **per-group backlog is empty** and every tracked group cleared ≥70%. | The finish line the charter defines is **met**; the deterministic next-target scan returns DONE (no open below-target group). |
| 3 | All five Simard per-group targets **CLOSED**: `bin` #1749 (PR #1772), `operator_commands_dashboard` #1750 (PR #2257), `trace_collector` #1751 (PR #2338), `operator_commands_gym` #1752 (PR #2346), `cmd_cleanup` #1753 (PR #2353); plus ad-hoc lifts **MERGED** #2701/#2844/#2729/#2958. | The goal's own coverage-raising work is **already delivered by merged PRs**. |
| 4 | `.github/workflows/coverage.yml` runs `cargo llvm-cov --workspace --lib --bins --summary-only` and only **posts a PR comment** — no `--fail-under`/threshold anywhere. Charter §4 **explicitly excludes** a workspace-wide hard CI gate. | The "which CI job counts / do we need a gate" question is already settled by the merged charter — the `coverage` job is report-only by design. Nothing further to build or wire up. |
| 5 | The goal is **not** on the active board (`simard goal list`) and is present in the durable tombstone store (`~/.simard/goal_tombstones.json`). | The goal has already been retired by prior triage; the correct action here is the idempotent re-tombstone, not a new engineering cycle. |

**Consequence:** the premise behind the block — "the worker is stuck and a human
must decide how to proceed" — is answered by the merged charter and merged PRs: the
work is delivered, the finish line is met, and the only defect is that the goal was
never marked complete and never surfaced. A human decision is **not** genuinely
required.

### 3c. Decision (exactly one, per procedure §"HOW TO DECIDE")

**`complete-delivered-goal`.**

Justification: the coverage work this goal describes has already shipped via merged
PRs (#1772/#2257/#2338/#2346/#2353 plus ad-hoc #2701/#2844/#2729/#2958), the charter
that defines and records its DONE verdict shipped via merged PR #4156, and the
per-group backlog is empty with every tracked group ≥70%. The procedure's rule
"Complete a goal already delivered by a merged PR" applies directly. Charter §2 also
prescribes exactly this: the goal is marked DONE and its slugs are tombstoned.

**Why not the other two options:**

- *rewrite-done-gate* — would apply only if the finish line were still unmeasurable
  **and** work remained. Neither holds: the finish line is written and shipped
  (merged charter #4156) and already met (empty backlog, all groups ≥70%).
  Completing the goal binds its DONE to machine-observable state (merged PR #4156 +
  closed issues #1749–#1753), so there is no unmeasurable gate left to rewrite.
- *ask-operator-one-question* — the seed floated three operator choices (accept a
  lower target, allocate time to write tests, or relax the CI requirement). None is
  a genuine open question: the merged charter already answers each (target = per-group
  ≥70% line coverage of the `simard` crate via `cargo llvm-cov`; CI `coverage` job is
  report-only, a hard gate deliberately excluded per §4; every tracked group is
  already ≥70%). Asking would dump a settled decision back on the operator, so
  `escalate` is `null`.

### 3d. Action taken (agentic, not merely proposed)

Retired the goal through the shipped operator CLI, which removes it from the board
and writes a durable tombstone. Because the goal was already off-board from prior
triage, this took the idempotent `Absent` branch and **re-recorded the tombstone
without error** — exactly the safe re-run behaviour documented in
`docs/reference/escalation-triage-api.md` §"handle_complete outcomes":

```
simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a
```

Observed log line (stderr, marker-free):

```
[simard] goal complete: 'audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a' not on board; recorded tombstone (idempotent)
```

Postcondition verified: the goal is absent from `active` + `backlog` and present in
`~/.simard/goal_tombstones.json`, so cycle-reconcile and the tombstoned-goal reaper
will not resurrect it.

## 4. OUTPUT (per `escalation_triage.md` §OUTPUT contract)

This is the authoritative six-key deliverable and is identical to the published
worked example in `docs/reference/escalation-triage-api.md` §"Worked output (#4904)".

```json
{
  "problem": "Simard's work to lift automated test coverage above 70% was recorded as stuck and then left alone, so it made no further progress and nobody was told.",
  "next_step": "Close the goal: the coverage work it describes has already been delivered by merged changes, so there is nothing left to do and nothing is needed from you.",
  "root_cause": "The goal was marked blocked and left silently un-escalated for a day even though its coverage objective had already been delivered by merged work — a stale block on an already-delivered goal, not a real engineering blocker.",
  "decision": "complete-delivered-goal",
  "action_taken": "Marked the goal complete, which removed it from the active board and recorded a permanent record so it cannot be reopened by accident.",
  "escalate": null
}
```

**Contract checklist:**
- [x] `problem` — WHAT is wrong, plain English, no jargon/markers.
- [x] `next_step` — smallest clear unblocking action, plain English.
- [x] `root_cause` — 1–2 sentences, grounded in the merged charter #4156 + ledger + closed group issues.
- [x] `decision` — exactly one enum value: `complete-delivered-goal`.
- [x] `action_taken` — the actual completion (concrete, agentic), not a proposal.
- [x] `escalate` — `null` (course-corrected without a human; no genuine human decision).
- [x] Exactly the six contract keys — no more, no fewer.
- [x] Change is additive/non-breaking (a goal-board state transition via the shipped CLI), no `Bridge` naming, no `print!`.

## 5. Jargon-free Signal messages (one per step; sent on the dual channel)

Delivered via the shipped notifier path (`src/overseer/notify.rs`,
`OperatorNotification { kind: "goal-blocked", … }`) — plain English only, no internal
markers. The three updates carry distinct content, so all three dispatch past the
suppressible-kind dedup rail.

1. **Restate (what's wrong):**
   > "I looked at the goal about checking Simard's own test coverage and getting it
   > above 70%. The worker on it hit a wall, marked it stuck, and stopped — and
   > because no one was told, it has been sitting parked with no progress."

2. **Root cause + recommended next step (why / what to do):**
   > "Here's what I found: the coverage work itself has already been done and merged,
   > and every area we track is now above the 70% line. There's also a written
   > charter that spells out exactly what '70%' means and records that the goal is
   > finished. The only reason it still showed as stuck is that nobody ever marked it
   > complete. My recommendation is simply to close it."

3. **Decision + action taken (done, nothing needed from you):**
   > "I've marked the goal finished and retired it so it won't keep coming back every
   > cycle. Nothing is needed from you — this one is closed out. If it ever
   > resurfaces, it just points back to the coverage charter and is closed again."

### Marker-leak scan (policy gate — every operator-facing string)

Scanned all strings in the §4 JSON and the three §5 Signal messages for forbidden
tokens **before** emit. **Result: zero leaks.**

| Forbidden token | Present in operator output? |
|---|---|
| `OODA-SAFEGUARD` | No |
| `UNCLEAR-CRITERIA` | No |
| `GENUINELY-STUCK` | No |
| `health-review:blocked-terminal` | No |
| `blocked-terminal outcome` / raw typed-outcome id | No |
| `why=` | No |
| `evidence=[` | No |
| 🔒 (lock token) | No |
| raw goal id / marker jargon in operator text | No |

## Verdict

**`complete-delivered-goal`.** The coverage-raising work is already delivered by
merged PRs (#1772/#2257/#2338/#2346/#2353, plus ad-hoc #2701/#2844/#2729/#2958),
every tracked group is ≥70%, and the merged Coverage-Audit Charter (PR #4156)
defines and records the goal's finish line — with a workspace-wide CI gate
deliberately excluded (§4). Retired via
`simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`
(idempotent re-tombstone, verified in the durable tombstone store); `escalate: null`;
three marker-free Signal updates; zero marker leakage.
