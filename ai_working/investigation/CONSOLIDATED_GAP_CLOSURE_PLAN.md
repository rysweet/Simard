# Consolidated Gap-Closure Plan — 10 recurring workstream-gaps

**Investigation question:** Cover the 10 uncovered backlog workstream(s) surfaced
by the Overseer gap-scan so they stop recurring — launch or track a workstream
that closes each gap.

**Status:** Consolidated from all parallel deep dives (primary/secondary/tertiary
+ reconciliation ledger). Every load-bearing claim re-grounded against **live**
source, GitHub, and `~/.simard/state/goal_board.json` at HEAD
`investigation/recurring-blocked-goals-workstream-gaps`.

---

## 1. Root cause (why all 10 recur, one sentence)

The Overseer **observes but never closes** coverage gaps. The `WorkstreamCoverage`
Decide arm is **notify-only** — `FlagWorkstreamGaps` sends an operator email/Signal
and commits the 15-min `gap_gate`, but files no issue and launches no recipe
(`overseer/mod.rs:884-948`, `:1534-1543`). The gate merely mutes each signature for
900 s; after the TTL lapses the still-uncovered signature re-notifies. **This is
issue #4126, which is itself OPEN and uncovered.**

## 2. The coverage predicate (what must become true to stop recurrence)

A signature drops out of `detect_workstream_gaps` (`sensor.rs:288`) only when:

- **ISSUE gap** `issue:rysweet/Simard#N`: some **OPEN PR** has `#N` in its
  **title** (`hash_issue_numbers`, `wiring.rs:903`) **or** a head branch matching
  `issue-N` / `issue/N` (`wiring.rs:887-898`). **Body references, `Closes #N`,
  labels, tracking issues, local/unpushed branches, and merged/closed PRs do NOT
  count.** (This is why triage PR #4173 — which references all 8 only in its body —
  covers nothing.)
- **GOAL gap** `goal:<id>`: board entry gains `assigned_to=Some` **or** a `wip_ref`
  of kind `pr|branch|session|engineer` (`sensor.rs:377-383`). An `issue`-kind
  wip_ref or a bare tracking issue does **NOT** count.

## 3. Live state (verified this pass)

- **8 issue-gaps** — all OPEN, all high-signal-labelled, **none has a covering open
  PR**: #4164 `bug,dashboard`; #3201 `bug`; #4126/#4078/#4074/#4051/#4046/#3698
  `workflow:default`.
- **2 goal-gaps** — both live on `board.active`, both p2 / NotStarted /
  `assigned_to=None` / `wip_refs=[]` → genuine gaps:
  `build-a-local-coin-benchmark-harness-and-a-self-09e65e35`,
  `steward-ci-github-actions-health-across-all-gov-e06d9e64`.
- PR #4173 (`investigation/gap-scan-triage-16-gaps`) is OPEN but body-only →
  **covers nothing** (the notify-only trap in PR form).

## 4. Closure table (every row is structurally detectable next tick)

| # | Gap | Disposition | Concrete closing action (satisfies §2 predicate) | Owner workstream |
|---|---|---|---|---|
| 1 | `issue:#4126` (META) | **launch** | Land #4126: give `WorkstreamCoverage` a `LaunchRecipe`/`FileIssue` edge keyed on `GapItem.signature` (not the bare `"workstream-gap"` dedup_key). Covering PR title carries `#4126`. **Systemically closes recurrence for the other 9.** | Overseer/steward core |
| 2 | `issue:#4164` | **link-existing** | Push existing `feat/issue-4164-…` worktree → open PR with `#4164` in title. | Cost/dashboard |
| 3 | `issue:#4078` | **link-existing** | Open PR from `feat/issue-4078-…` worktree, `#4078` in title. | OODA self-diagnose |
| 4 | `issue:#4074` | **launch (cluster lead)** | One OODA goal-session workstream; covering PR title enumerates `#4074 #4051 #4046`. | OODA goal-session |
| 5 | `issue:#4051` | **dedupe→link** | Folded into row-4 PR (title lists `#4051`). | OODA goal-session |
| 6 | `issue:#4046` | **dedupe→link** | Folded into row-4 PR (title lists `#4046`). | OODA goal-session |
| 7 | `issue:#3698` | **launch** | Workstream for smart-orchestrator `orch_helper.py` regression; PR carries `#3698`. | Orchestrator |
| 8 | `issue:#3201` | **dedupe-or-launch** | Verify against merged CI-health PR #4181; if fixed, close #3201; else open PR with `#3201`. | CI-health steward |
| 9 | `goal:…coin-benchmark…09e65e35` | **link-existing** | Engineer PRs #4171/#4161/#4149 exist on `engineer/…09e65e35-*` branches — attach `assigned_to` or a `pr/engineer` wip_ref on the board entry so `goal_has_active_workstream`→true. | Overseer goal-board wiring |
| 10 | `goal:…steward-ci…e06d9e64` | **mark-done-or-link** | Steward PR #4181 is MERGED (gives no live coverage). If stewardship delivered → mark goal done (leaves `board.active`); if ongoing → add a wip_ref to a live branch. | Overseer goal-board wiring |

**Tally:** launch ×3 (rows 1,4,7) · link-existing ×3 (rows 2,3,9) ·
dedupe/mark-done/dedupe→link ×4 (rows 5,6,8,10). **Zero notify-only rows.**

## 5. Anti-patterns to avoid (each re-flags on the next tick)

1. Documenting/triaging a gap in a PR **body** (PR #4173) — title/branch only.
2. Adding a **label** to an issue — labels *qualify* a gap, never clear it.
3. Filing a **tracking issue** for a goal or stapling an `issue`-kind wip_ref.
4. Relying on a **local/unpushed** `issue-N` branch or a **merged/closed** PR.
5. Wiring `WorkstreamCoverage→FileIssue` keyed on `problem.dedup_key` — all 10
   collapse into one issue. Must key on `GapItem.signature`.

## 6. Recommendation (dependency-correct)

1. **Land #4126 first** (row 1) — the systemic root; the manual dispositions are
   the bridge, not the cure. Once landed, `act_flag_workstream_gaps` launches/files
   on detection and any residual signature self-covers within one tick.
2. Cover the **OODA cluster** (#4074/#4051/#4046) with **one** workstream whose PR
   title enumerates all three `#N` tokens — three predicates, one artifact.
3. **Wire goal-board wip_ref linking** (rows 9-10) so in-flight engineer branches
   auto-cover their goals; otherwise coin-benchmark-style goals recur despite live
   PRs.

## 7. Verification (covered ⇒ filtered next tick)

- Issue rows: `gh pr list --state open --search "<N> in:title"` returns a PR, or an
  open-PR branch matches `issue-<N>` ⇒ `issue_coverage_from_open_prs` includes the
  signature ⇒ dropped (`tests_gap_scan.rs:453`).
- Goal rows: board entry shows `assigned_to=Some` or a `pr/branch/engineer` wip_ref
  ⇒ `goal_has_active_workstream`→true ⇒ dropped (`tests_gap_scan.rs:388`); steward
  goal marked done ⇒ no longer in `board.active`.
