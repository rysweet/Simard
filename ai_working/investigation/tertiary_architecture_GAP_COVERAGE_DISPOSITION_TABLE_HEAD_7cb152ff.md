# Tertiary (Architect) — Gap-Coverage Disposition & Closure Table

HEAD: `7cb152ff`  ·  Role: TERTIARY / architecture  ·  Scope: map each of the 10
gap signatures to exactly ONE disposition that provably satisfies the gap-scan
predicate; produce the final coverage table with owners. **Track/launch only —
do not fix the underlying bugs.**

---

## 1. The gap-scan predicate (what must become true to stop recurrence)

Traced through `detect_workstream_gaps` (`sensor.rs:288`), `workstream_gaps`
(`wiring.rs:750`), `issue_coverage_from_open_prs` (`wiring.rs:861`),
`issue_refs_from_pr` (`wiring.rs:885`), and `act_flag_workstream_gaps`
(`mod.rs:884`).

A candidate re-surfaces every tick until it drops OUT of
`detect_workstream_gaps`. There are exactly two coverage predicates:

**ISSUE gap** `issue:rysweet/Simard#N` is emitted iff the OPEN issue carries a
high-signal label (`bug` | `P1` | `workflow:default`, case-insensitive) AND
`issue:rysweet/Simard#N` ∉ `coverage`. The coverage set is built ONLY from
**open PRs**, and an issue is covered iff some open PR has:
- a **title** containing the token `#N` (`hash_issue_numbers`), OR
- a **branch** name containing `issue-N` or `issue/N`.

**GOAL gap** `goal:<id>` is emitted iff the goal is p1/p2, not `Blocked`, and
`goal_has_active_workstream(g)` is false — i.e. the board entry has NO assignee
AND NO wip_ref of kind `pr | branch | session | engineer`. (An `issue`-only ref
does **not** count — `sensor.rs:376`.) `coverage` never contains `goal:` sigs, so
goals rely entirely on the board's assignee / wip_ref.

**Dedup window ≠ coverage.** `gap_gate = WhisperGate::new(900, 200)` (`mod.rs:304`)
only suppresses re-*notification* for 900 s (15 min). After the TTL lapses the
same uncovered signature re-notifies. This is *why* the 10 signatures recur:
nothing has satisfied the coverage predicate; the gate merely mutes them for 15
min at a time.

## 2. #4126 — the systemic fix has NOT landed (meta-workstream)

`act_flag_workstream_gaps` (`mod.rs:884-948`) does exactly three things: **peek**
the gap_gate → **notify** the operator (email + Signal) → **commit** the gate. It
never files an issue and never launches a recipe. The `describe_action` string
"…filed deduped issue(s)…" (`wiring.rs:567`) is aspirational wording, not a code
path — there is no `FileIssue` / `LaunchRecipe` for gaps. **Conclusion: the
Overseer is still notify-only for gaps; issue #4126 is OPEN and is the META
workstream. Landing #4126 (auto-launch/auto-file on detected gap) systemically
closes recurrence for the other 9.** This manual pass is therefore a one-time
bridge until #4126 lands.

## 3. Metadata contract a covering artifact MUST carry

- **Issue coverage:** open a PR whose **title contains `#N`** *or* whose
  **branch is named `…issue-N…` / `…issue/N…`**. Body references are INVISIBLE
  to the scanner — this is the trap PR #4173 falls into (see below).
- **Goal coverage:** on the goal board, set `assigned_to` *or* add a wip_ref of
  kind `pr|branch|session|engineer` pointing at the live branch/PR.

## 4. Live-status findings

- **All 8 issues OPEN** with high-signal labels: #4164 `bug`, #3201 `bug`, and
  #4126/#4078/#4074/#4051/#4046/#3698 all `workflow:default`.
- **PR #4173** ("docs(triage): per-gap triage of 16 recurring gap-scan gaps",
  branch `investigation/gap-scan-triage-16-gaps`) references all 8 issues **only
  in its body** → title has no `#N`, branch has no `issue-N` → **covers NOTHING**
  per the predicate. A body-only triage PR is the notify-only trap.
- **goal:build-a-local-coin-benchmark…09e65e35** — HAS active engineer PRs
  #4171 / #4161 / #4149 on branches `engineer/build-a-local-coin-benchmark…09e65e35-*`.
  Real work is in flight; the goal still surfaces → the **board wip_refs are not
  linked** to those branches.
- **goal:steward-ci-github-actions-health…e06d9e64** — PR #4181 ("ci(verify):
  resilience… (CI-health steward)") is **MERGED** (not open → gives no live
  coverage). The goal surfaces because no *open* active workstream is linked on
  the board.

## 5. FINAL COVERAGE TABLE (every row detectable next tick; zero notify-only)

| # | Gap signature | State | Covering predicate to satisfy | Disposition | Concrete next action | Owner |
|---|---|---|---|---|---|---|
| 1 | `goal:…build-a-local-coin-benchmark…09e65e35` | active work exists (PR #4171/#4161/#4149) | goal board assignee OR wip_ref kind pr/branch/engineer | **link-existing** | Attach wip_ref (kind `engineer`) / assignee on the board entry pointing at branch `engineer/build-a-local-coin-benchmark…09e65e35`. Flips `goal_has_active_workstream`→true. | Overseer goal-board wiring |
| 2 | `goal:…steward-ci-github-actions-health…e06d9e64` | steward PR #4181 MERGED | remove from `board.active` OR link an OPEN workstream | **dedupe (mark-done)** if stewardship delivered; else **link-existing** open workstream | Mark goal complete on board (leaves `board.active`); if ongoing, add wip_ref to a live branch. | Overseer goal-board wiring |
| 3 | `issue:rysweet/Simard#4126` | OPEN, workflow:default (META) | open PR title `#4126` or branch `issue-4126-…` | **file-new (launch workstream)** — highest leverage | Launch the auto-act workstream for #4126 (make gap→launch/file). Its PR must carry `#4126` in title/branch. Closes recurrence for all others. | Overseer/steward core |
| 4 | `issue:rysweet/Simard#4164` | OPEN, bug; local worktree `feat/issue-4164-…` exists, no open PR | open PR title `#4164` or branch `issue-4164-…` | **link-existing** (push worktree → open PR) | Open PR from the existing `feat/issue-4164` worktree; ensure title contains `#4164`. | Cost/dashboard workstream |
| 5 | `issue:rysweet/Simard#4078` | OPEN, workflow:default; worktree `feat/issue-4078-…` exists | open PR title `#4078` or branch `issue-4078-…` | **link-existing** (push worktree → open PR) | Open PR from `feat/issue-4078` worktree with `#4078` in title. | OODA self-diagnose workstream |
| 6 | `issue:rysweet/Simard#4074` | OPEN, workflow:default | open PR title `#4074` or branch `issue-4074-…` | **file-new (launch workstream)** | Launch workstream for the OODA terminal-record regression; PR must carry `#4074`. Dedupe-cluster with #4051/#4046. | OODA goal-session workstream |
| 7 | `issue:rysweet/Simard#4051` | OPEN, workflow:default | open PR title `#4051` or branch `issue-4051-…` | **dedupe→link** (same goal-session cluster as #4074/#4046) | Fold into the goal-session workstream; the covering PR title lists `#4051` alongside `#4074`. | OODA goal-session workstream |
| 8 | `issue:rysweet/Simard#4046` | OPEN, workflow:default | open PR title `#4046` or branch `issue-4046-…` | **dedupe→link** (goal-session quality-audit) | Fold into the goal-session workstream PR (title lists `#4046`). | OODA goal-session workstream |
| 9 | `issue:rysweet/Simard#3698` | OPEN, workflow:default | open PR title `#3698` or branch `issue-3698-…` | **file-new (launch workstream)** | Launch workstream for the smart-orchestrator `orch_helper.py` regression; PR carries `#3698`. | Orchestrator workstream |
| 10 | `issue:rysweet/Simard#3201` | OPEN, bug (CI coverage.yml/lbug) | close issue OR open PR title `#3201` | **dedupe** if fixed by merged CI-health work (#4181); else **file-new** | Verify #3201 against #4181; if resolved, close the issue (leaves open-issue survey); else launch a `issue-3201` workstream. | CI-health steward |

**Disposition tally:** link-existing ×3 (rows 1,4,5) · file-new/launch ×3
(rows 3,9 + row 6) · dedupe/mark-done or dedupe→link ×4 (rows 2,7,8,10).
**Zero notify-only rows.**

## 6. Why body-reference / triage-PR "coverage" fails (anti-pattern)

The single most important architectural pitfall: coverage is read from PR
**title tokens + branch names only**, never PR bodies, never linked issues, never
comments. Any resolution that "documents" the gap (like PR #4173) without an open
PR carrying `#N` in title or `issue-N` in branch will be re-flagged on the very
next tick. Every disposition above is chosen specifically so the artifact is
structurally discoverable by `issue_refs_from_pr` / `goal_has_active_workstream`.

## 7. Verification recipe (per row)

- Issue rows: `gh pr list --state open --search "<N> in:title"` returns a PR OR
  an open PR branch matches `issue-<N>` ⇒ `issue_coverage_from_open_prs` includes
  `issue:rysweet/Simard#<N>` ⇒ `detect_workstream_gaps` drops it (see
  `tests_gap_scan.rs` coverage-filter assertions).
- Goal rows: board entry shows `assigned_to=Some` or a `pr/branch/engineer`
  wip_ref ⇒ `goal_has_active_workstream`→true ⇒ dropped. `steward-ci` goal marked
  done ⇒ no longer in `board.active`.
- Meta: once #4126 lands, `act_flag_workstream_gaps` launches/files on detection,
  so any residual signature self-covers within one tick.

## 8. Recommendation

1. **Land #4126 first** (row 3) — it is the systemic root; the manual dispositions
   above are the bridge, not the cure.
2. For the OODA cluster (#4074/#4051/#4046) prefer ONE workstream whose PR title
   enumerates all three `#N` tokens — satisfies three predicates with one artifact
   and avoids duplicate launches.
3. Wire goal-board wip_ref linking (rows 1–2) so in-flight engineer branches
   auto-cover their goals — otherwise coin-benchmark-style goals will keep
   recurring despite active PRs.
