# Escalation-triage — blocked goal `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`

**Goal id:** `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`
**Procedure (authoritative):** `prompt_assets/simard/overseer/escalation_triage.md`
**Target work:** audit Simard's own test coverage and raise it above the 70% line-coverage bar.
**Internal diagnostic WHY (input to translate, never surfaced):** an engineer committed a typed terminal blocker on 2026-07-16; no engineer run or state change since, so the completion check is effectively undecided/unreachable and the goal has sat blocked ~10 days.
**Reason marker (input to translate, never surfaced):** `health-review:stale-terminal-block`.

---

## 1. Restate the PROBLEM in plain English

Simard has a goal to check its own test coverage and get it above 70%. About ten
days ago the engineer working on it hit a wall and marked it stuck, and nothing
has moved it since. The real trouble is that Simard could not automatically tell
when this goal counted as "finished," so it just sat parked — neither shipping
anything new nor closing out — because its finish line was never expressed as
something the daemon can check by itself.

## 2. Recommended NEXT STEP (plain English)

Mark this goal finished and retire it. The coverage work it describes has already
been done and merged, its finish line is already written down and met, and there
is no remaining below-target area to attack — so retiring it durably stops it from
being re-picked-up every cycle. Nothing needs to be lowered, deferred, or handed
to a new engineering run.

## 3. ROOT CAUSE and the course-correction DECISION

### 3a. Root cause (grounded in live evidence)

The goal was flagged stuck for a real reason: its finish condition was written as
prose ("raise it to 70%") with no single, automatically-checkable yardstick, so
the completion check could never certify it done. Meanwhile the actual coverage
work had already shipped. So the block is administrative, not substantive: the
work is delivered, but nothing ever recorded the goal as complete, so it kept
sitting on the board as "blocked."

### 3b. Evidence triangulation

| # | Evidence | What it proves |
|---|----------|----------------|
| 1 | **`Specs/COVERAGE_AUDIT.md`** (canonical Coverage-Audit Charter) landed via **merged PR #4156** (`docs(coverage): canonical Coverage-Audit Charter…`, MERGED 2026-07-16). | The goal's meaning and finish line are now written down and shipped: per-group **≥70% aggregate line coverage** of the single `simard` crate + `simard-*` bins, measured by `cargo llvm-cov`. |
| 2 | Charter §2 done-criteria + companion ledger `docs/testing/COVERAGE_BASELINE.md` ("Other groups — status") show the **per-group backlog is empty** and every tracked group cleared ≥70%. | The finish line the charter defines is **met**, and the deterministic next-target scan returns a DONE verdict (no open below-target group). |
| 3 | All five Simard per-group targets **CLOSED**: `bin` #1749 (PR #1772), `operator_commands_dashboard` #1750 (PR #2257), `trace_collector` #1751 (PR #2338), `operator_commands_gym` #1752 (PR #2346), `cmd_cleanup` #1753 (PR #2353); plus ad-hoc lifts **MERGED** #2701/#2844/#2729/#2958. | The goal's own coverage-raising work is **already delivered by merged PRs**. |
| 4 | `.github/workflows/coverage.yml` runs `cargo llvm-cov --workspace --lib --bins --summary-only` and only **posts a PR comment** — no `--fail-under`/threshold anywhere in the workflows or scripts. Charter §4 **explicitly excludes** a workspace-wide hard CI gate (consistent with the owner's rejection of #2150/#2151). | The "which CI job counts" question is settled: the `coverage` job is a **report-only** comment, deliberately **not** a blocking gate. There is nothing further to build or wire up. |

**Consequence:** the premise behind the block — "we can't tell when this is done" —
is resolved by the merged charter, and the work itself is delivered by merged PRs
with every tracked group above the bar. A human decision is **not** genuinely
required.

### 3c. Decision (exactly one, per procedure §"HOW TO DECIDE")

**`complete-delivered-goal`.**

Justification: the coverage work this goal describes has already shipped via merged
PRs (#1772/#2257/#2338/#2346/#2353 plus ad-hoc #2701/#2844/#2729/#2958), the charter
that defines and records its DONE verdict shipped via merged PR #4156, and the
per-group backlog is empty with every tracked group ≥70%. The procedure's rule
"Complete a goal already delivered by a merged PR" applies directly. Charter §2 also
prescribes exactly this: "the goal is marked DONE and its slugs are tombstoned."

**Why not the other two options:**

- *rewrite-done-gate* — would be correct if the finish line were still unmeasurable
  **and** work remained. But the finish line is already written and shipped (merged
  charter #4156) and already met (empty backlog, all groups ≥70%). Completing the
  goal via the CLI binds its DONE to machine-observable state (merged PR #4156 +
  closed issues #1749–#1753), so there is no unmeasurable gate left to rewrite — the
  goal is simply done.
- *ask-operator-one-question* — not warranted: the scope questions ("confirm the
  target, which crate, which CI job counts, or lower/defer it") are already answered
  in the merged charter — per-group ≥70% line coverage of the single `simard` crate,
  measured by `cargo llvm-cov`, reported by the comment-only `coverage` CI job (a
  hard gate was deliberately excluded per §4). The target is met, not in question,
  so there is no genuine operator decision to make.

### 3d. Action taken (agentic, not merely proposed)

Retire the goal through the shipped operator CLI, which removes it from the board
and writes a durable tombstone (verified in `src/operator_cli/goal.rs`
`handle_complete`: a normal, non-perpetual goal is removed then `tombstone`d;
idempotent on an absent/already-tombstoned goal). This goal is not a standing/
perpetual goal, so `complete` terminates it rather than auto-reopening it:

```
simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a
```

Because the delivering PRs are MERGED and the group issues are CLOSED, this
completion binds the goal's certified DONE to machine-observable upstream state,
so any future resurfacing is resolved by re-tombstoning (linking the charter),
not by opening another planning cycle.

## 4. OUTPUT (per `escalation_triage.md` §OUTPUT contract)

```json
{
  "problem": "Simard has a goal to check its own test coverage and get it above 70%. About ten days ago the engineer working on it hit a wall and marked it stuck, and nothing has moved it since. The underlying trouble is that Simard could not automatically tell when this goal counted as finished, so it stayed parked without shipping anything new or closing out.",
  "next_step": "Mark this goal finished and retire it. The coverage work has already been done and merged, its finish line is written down and met, and there is no remaining below-target area to attack, so retiring it stops it from being re-opened every cycle.",
  "root_cause": "The goal's finish condition was written as prose ('raise it to 70%') with no single automatically-checkable yardstick, so Simard could never certify it done, while the actual coverage work had already shipped and merged. Nothing ever recorded the goal as complete, so it kept sitting on the board as blocked even though the work was finished.",
  "decision": "complete-delivered-goal",
  "action_taken": "Marked the goal complete and retired it (removed from the board and permanently tombstoned so it will not be re-opened next cycle): `simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`. The coverage work already shipped via merged pull requests, every tracked area is above 70%, and the written charter that defines and records its finish line has also merged.",
  "escalate": null
}
```

**Contract checklist:**
- [x] `problem` — WHAT is wrong, plain English, no jargon/markers.
- [x] `next_step` — smallest clear unblocking action, plain English.
- [x] `root_cause` — 1-2 sentences, grounded in the merged charter #4156 + ledger + closed group issues.
- [x] `decision` — exactly one enum value: `complete-delivered-goal`.
- [x] `action_taken` — the actual completion command (concrete, agentic), not a proposal.
- [x] `escalate` — `null` (course-corrected without a human; no genuine human decision).
- [x] Change is additive/non-breaking (a goal-board state transition via the shipped CLI), no `Bridge` naming, no `print!`.

## 5. Jargon-free Signal messages (one per step; sent on the dual channel)

Delivered via the shipped Signal notifier path (`src/overseer/notify.rs`) — plain
English only, no internal markers.

1. **Restate (what's wrong):**
   > "I looked at the goal about checking Simard's own test coverage and getting it
   > above 70%. It's been sitting stuck for about ten days — the engineer on it hit
   > a wall and marked it blocked. The core problem was that Simard couldn't
   > automatically tell when this goal counted as finished, so it just sat parked."

2. **Root cause (why):**
   > "Here's what I found: the coverage work itself has already been done and merged,
   > and every area we track is now above the 70% line. There's also a written
   > charter that spells out exactly what '70%' means and records that the goal is
   > finished. The only reason it was still showing as blocked is that nobody ever
   > marked it complete."

3. **Action taken (done, nothing needed from you):**
   > "I've marked the goal finished and retired it so it won't keep coming back every
   > cycle. Nothing is needed from you — this one is closed out. If it ever
   > resurfaces, it just points back to the coverage charter and is closed again."

### Marker-leak scan (policy gate — every operator-facing string)

Scanned all strings in §4 JSON and the three Signal messages for forbidden tokens.
**Result: zero leaks.**

| Forbidden token | Present in operator output? |
|---|---|
| `OODA-SAFEGUARD` | No |
| `UNCLEAR-CRITERIA` | No |
| `GENUINELY-STUCK` | No |
| `stale-terminal-block` | No |
| `health-review:` | No |
| `why=` | No |
| `evidence=[` | No |
| 🔒 (lock token) | No |
| raw goal id / marker jargon in operator text | No |

## Verdict

**`complete-delivered-goal`.** The coverage-raising work is already delivered by
merged PRs (#1772/#2257/#2338/#2346/#2353, plus ad-hoc #2701/#2844/#2729/#2958),
every tracked group is ≥70%, and the merged Coverage-Audit Charter (PR #4156)
defines and records the goal's finish line — with a workspace-wide CI gate
deliberately excluded (§4). Retire the goal via
`simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`
to durably park it and stop the per-cycle re-pickup; `escalate: null`; three
marker-free Signal updates; zero marker leakage.
