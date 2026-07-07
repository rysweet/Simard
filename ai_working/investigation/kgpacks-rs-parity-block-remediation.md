# kgpacks-rs Full-Parity Block — Per-Issue Blockers & Remediation Plan

**Investigation follow-up (Round 2).** Round 1 diagnosed the recurring
`overseer-obs:…|overseer-obs:…` (seen 2×) signature and its recall-loop
mechanism thoroughly, but explicitly **descoped the domain remediation**
("Remediating these issues is explicitly out of scope for this
understanding-only investigation"). This document closes the two remaining
success criteria:

- **Criterion 2** — issue-specific blockers for **#17 (WS2 int8-pq-embed)** and
  **#21 (WS6 resumable-pip)**.
- **Criterion 4** — a **prioritized, actionable remediation plan** to unblock
  WS1/2/3/6/7 (#16/#17/#18/#21/#22).

---

## TL;DR

The block is **~80% stale**. Verified against GitHub
(`rysweet/agent-kgpacks-rs`) on 2026-07-07:

| Issue | WS  | Title (short)              | GitHub state              | Real status                    |
| ----- | --- | -------------------------- | ------------------------- | ------------------------------ |
| #16   | WS1 | Full-pack CVE eval         | **CLOSED / COMPLETED** 07-06 20:16Z | Done — goal-board park is stale |
| #17   | WS2 | int8/PQ embed spike        | **OPEN** (0 comments, 0 assignees) | **Only genuinely open item**   |
| #18   | WS3 | Versioned release tags     | **CLOSED / COMPLETED** 07-06 10:33Z | Done — stale park              |
| #21   | WS6 | Resumable+pipelined build  | **CLOSED / COMPLETED** 07-06 13:29Z | Done — stale park              |
| #22   | WS7 | Sign release index (Ed25519) | **CLOSED / COMPLETED** 07-06 12:07Z | Done — stale park              |

Four of the five workstreams are **already completed and closed on GitHub**.
Only **#17** is genuinely open, and its single gating dependency
(the WS1 eval-recall parity harness, #16) was **satisfied when #16 closed on
2026-07-06 20:16Z**. So there is exactly **one** real remaining domain task,
and it is **actionable now**.

The reason the Overseer keeps re-parking all five as `Blocked` is a
**missing reconciliation between goal-board `Blocked` state and GitHub
issue-CLOSED state** (see Root Cause below). This is the domain seed that also
feeds the recall-loop artifact Round 1 analyzed.

---

## 1. Per-issue blocker analysis (Criterion 2)

### #17 — WS2: int8/PQ embedding quantization spike  ⟶ **OPEN, the one real blocker**

- **GitHub state:** OPEN. Created 2026-07-02T23:22:49Z, **0 comments, 0
  assignees, never updated** since creation.
- **Declared gate (from the issue body):**
  > "Parity gate: run the **WS1 eval harness** on a quantized pack; adopt only if
  > `delta_accuracy >= -0.02` AND retrieval hit@k parity within tolerance.
  > Otherwise leave the feature DISABLED and commit spike findings."
  So #17 is a **spike gated on WS1 (#16)** — it needs the WS1 eval harness to
  decide adoption.
- **Gate status now:** #16 (WS1) **closed COMPLETED on 2026-07-06 20:16Z** →
  the eval-recall parity harness exists and is validated → **the gate is
  satisfied.**
- **Historical infra blockers (now fixed):** the engineer-claim-sentinel
  dispatch deadlock (`docs/reference/engineer-claim-sentinel-exclusion.md:52-58`,
  which names #17 among the blocked goals) and the `E2BIG` argv overflow
  (`docs/concepts/self-diagnose-on-step-error.md:58-88`). Round 1 confirmed the
  launcher now uses **file-backed prompt transport**, i.e. the `E2BIG` fix is
  live.
- **∴ Current specific blocker:** **none dependency-wise — it is simply
  unclaimed/unstarted.** With the gate cleared and no engineer assigned, the
  goal is a *ready-to-dispatch* item that the board still shows as `Blocked`
  (stale-blocked, not dependency-blocked). This is the single genuine unit of
  remaining domain work.

### #21 — WS6: Resumable + pipelined CVE pack build  ⟶ **CLOSED (done); stale park**

- **GitHub state:** **CLOSED / COMPLETED on 2026-07-06 13:29Z.**
- **Body has no intrinsic dependency gate** (checkpoint/resume sidecar +
  embed‖load pipelining, self-contained in the pack builder).
- **Specific blocker (historical):** the same two infra incidents that blocked
  the whole cluster — engineer-claim-sentinel dispatch deadlock and `E2BIG`
  argv overflow — both since fixed. Once those cleared, WS6 was implemented and
  the issue was closed COMPLETED.
- **∴ Current specific blocker:** **none — the work is finished.** Its continued
  appearance in the `goal:blocked:…issue-21…` token is a **stale goal-board
  park** that was never reconciled against the issue's CLOSED state.

### #16 / #18 / #22 — WS1 / WS3 / WS7  ⟶ **CLOSED (done); stale parks**

- #16 WS1 (Full-pack CVE eval): CLOSED COMPLETED 07-06 20:16Z. Intrinsic gate
  in body was "run the eval where an **LLM transport** is available; if none,
  report deterministic hit@k and document the blocker" — transport availability
  was its only real dependency; resolved and closed.
- #18 WS3 (Versioned release tags): CLOSED COMPLETED 07-06 10:33Z.
- #22 WS7 (Sign release index, Ed25519): CLOSED COMPLETED 07-06 12:07Z.
- All three: historical blockers = the two cluster-wide infra incidents (fixed);
  current status = **done**, board park is **stale**.

**Cross-check:** the historical incident doc lists the blocked set as
#12/#16/#17/#18/#19/#20/#21. WS4 (#19) and WS5 (#20) are **absent** from the
current composite, consistent with them having been unblocked/closed earlier —
matching the pattern that these are point-in-time board parks that lag issue
closure.

---

## 2. Domain-layer root cause: no goal↔issue-closure reconciliation

The blocked-goal tokens originate from the **goal board**, projected read-only
by `blocked_goals_from_board` (`src/overseer/sensor.rs:204-221`). The decision
path for a blocked goal is `decide_blocked_goal`
(`src/overseer/mod.rs:1624-1666`), whose only outcomes are:

1. `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` → `FileIssue` (escalate root
   cause, deduped);
2. `perpetual && is_no_progress_marker(reason)` → `UnblockGoal` (self-heal);
3. `needs_review` → `EscalateBlockedGoal`;
4. else → `Report`.

**None of these reconciles a `Blocked` goal against its linked GitHub issue
being CLOSED/COMPLETED.** The one closure path that exists —
`StaleGoal → TransferGoal { rationale: "stale / re-litigated goal — transfer to
Simard for closure" }` (`src/overseer/mod.rs:1505-1513`) — is only reached when
a `GoalHygiene` problem has **no `GoalBlocked` evidence**
(`mod.rs:1465-1504`). These kgpacks goals *do* carry `GoalBlocked` evidence, so
they always take the `decide_blocked_goal` branch and are **never closed on the
basis of issue completion**.

Result: once WS1/3/6/7 issues closed on GitHub (07-06), the board kept holding
their goals as `Blocked`, the Observe pass kept emitting `goal:blocked:…` for
them every cycle, and that composite is the very domain seed that the recall
loop (Round 1) re-observes and nests. Fixing the domain seed both unblocks the
workstreams **and** starves the recall-loop's re-observation input.

---

## 3. Prioritized remediation plan (Criterion 4)

Ordered by value/effort. P1 unblocks 4 of 5 items immediately; P2 is the single
genuine remaining task.

### P1 — Reconcile the 4 stale parks against issue-CLOSED state (highest value, unblocks #16/#18/#21/#22)

**Immediate (operational):** mark the four goals Done/Complete on the Overseer
goal board so they stop being re-parked and re-observed:

- `fix-agent-kgpacks-rs-issue-16-ws1-full-pack-cve`
- `fix-agent-kgpacks-rs-issue-18-ws3-versioned-rel`
- `fix-agent-kgpacks-rs-issue-21-ws6-resumable-pip`
- `fix-agent-kgpacks-rs-issue-22-ws7-sign-the-release`

Verification the underlying work is truly done (run to confirm before closing):

```bash
for n in 16 18 21 22; do
  gh issue view "$n" --repo rysweet/agent-kgpacks-rs \
    --json number,state,stateReason,closedAt \
    -q '"#\(.number) \(.state)/\(.stateReason) @ \(.closedAt)"'
done
# expected: all CLOSED/COMPLETED
```

**Root-cause fix (follow-up dev round):** add a **goal↔issue-closure
reconciler** so a `Blocked` goal whose linked GitHub issue is CLOSED/COMPLETED
transitions to `Done` (or routes to `TransferGoal … for closure`) instead of
re-parking. Implementation seam: extend `decide_blocked_goal`
(`mod.rs:1624-1666`) or the `blocked_goals_from_board` projection
(`sensor.rs:204-221`) with an issue-state check keyed on the goal's embedded
issue number (`…issue-NN-…`). This is a **development** task (needs the full
workflow + tests); tracked here as the systemic fix, not done in this
investigation.

### P2 — Dispatch the one real remaining workstream: WS2 / #17 (int8-pq-embed)

The gate (WS1 eval harness, #16) is satisfied as of 2026-07-06 20:16Z. Concrete
unblocking steps:

1. **Record that the gate is cleared** on the issue (ready-to-run):
   ```bash
   gh issue comment 17 --repo rysweet/agent-kgpacks-rs --body \
   "Gate cleared: WS1 eval-recall parity harness (#16) closed COMPLETED 2026-07-06. \
   The 'run the WS1 eval harness on a quantized pack' dependency is satisfied — WS2 is ready to start."
   ```
2. **Assign/dispatch** an engineer (or re-enqueue the goal now that it is no
   longer dependency-blocked).
3. **Implement** per the issue's acceptance criteria in crate
   `kgpacks-embeddings`:
   - `quantize_int8(v:&[f32]) -> (Vec<i8>, f32)` with `scale = max(|v|)/127`
     (all-zero → scale 0, no NaN);
   - `dequantize_int8(codes:&[i8], scale:f32) -> Vec<f32>`, bound-checked
     (reject wrong length / non-finite scale);
   - round-trip error ≤ scale per element; cosine > 0.999 on L2-normalized
     vectors; additive pack format only.
4. **Run the WS1 eval harness** on a quantized pack; **ship behind a flag only
   if** `delta_accuracy >= -0.02` **and** hit@k parity holds — otherwise land
   the feature DISABLED plus a spike report (both outcomes satisfy the issue).
5. Ensure `cargo test` / `cargo clippy` / `cargo fmt --check` are green, then
   close #17.

### P3 — Confirm the infra fixes are deployed on the kgpacks runner (prevent recurrence)

Before re-dispatching #17, confirm the two historical incident fixes are live on
the executing runner so the goal does not re-trip the deadlock:

- engineer-claim-sentinel exclusion
  (`docs/reference/engineer-claim-sentinel-exclusion.md`) — the
  `.simard-engineer-claim` sentinel must be excluded from the dirty-worktree
  guard;
- file-backed prompt transport for the `E2BIG` argv overflow
  (`docs/concepts/self-diagnose-on-step-error.md`) — Round 1 already observed
  the launcher using file-backed prompt transport, so this is live; re-verify on
  the kgpacks runner specifically.

### P4 — Clear the co-observed quality signals (after P1/P2)

- **`quality:gym_skipped`** — re-enable gym self-eval so quality is measured once
  #17 lands; tie to the distill learning loop
  (`docs/howto/recover-from-distill-trailing-comma-parse-failures.md:24-30`
  documents the same parity goal co-occurring with a dead learning loop).
- **`resource:engineer_spawn` (≥8 live)** — this is a *symptom* of stuck/re-
  dispatched workstreams (`root_cause.rs:326-336`, "stuck workstreams"). It
  should subside automatically once P1 reconciles the stale parks; **verify**
  the live-engineer count drops afterward.
- **`workstream-gap`** — provably **disjoint** from the parity children
  (`detect_workstream_gaps` skips `GoalProgress::Blocked`,
  `sensor.rs:299-302`). Handle as ordinary backlog triage; it is **not** part of
  the kgpacks block.

---

## 4. Evidence index

- GitHub issue states (verified 2026-07-07 via `gh issue view … --repo
  rysweet/agent-kgpacks-rs`): #16 CLOSED/COMPLETED 07-06 20:16Z; #17 **OPEN**
  (0 comments/0 assignees); #18 CLOSED/COMPLETED 07-06 10:33Z; #21
  CLOSED/COMPLETED 07-06 13:29Z; #22 CLOSED/COMPLETED 07-06 12:07Z.
- #17 gate text: issue body "Parity gate: run the WS1 eval harness on a
  quantized pack; adopt only if `delta_accuracy >= -0.02` …".
- #16 gate text: issue body "Run the eval … where an LLM transport is available;
  … if no transport, report the deterministic retrieval-recall (hit@k) metric
  and document the blocker."
- Historical infra root causes: `docs/reference/engineer-claim-sentinel-exclusion.md:52-58`
  (blocked #12/#16/#17/#18/#19/#20/#21); `docs/concepts/self-diagnose-on-step-error.md:58-88`
  (`E2BIG`, exit 126, large-context kgpacks goals).
- Overseer machinery: `sensor.rs:204-221` (blocked_goals projection);
  `mod.rs:1624-1666` (`decide_blocked_goal`, no issue-closure path);
  `mod.rs:1505-1513` (`StaleGoal`→`TransferGoal` closure, only without
  `GoalBlocked` evidence); `root_cause.rs:326-336` (engineer-spawn-storm =
  stuck workstreams); `sensor.rs:299-302` (`workstream-gap` excludes blocked
  goals).
