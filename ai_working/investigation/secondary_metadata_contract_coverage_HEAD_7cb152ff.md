# Secondary Investigation — The coverage metadata contract: what a filed/linked artifact MUST carry to stop a gap recurring

**Role:** Secondary investigator (patterns). **Date:** 2026-07-16.
**HEAD:** `7cb152ff` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Focus:** `observer.rs` search-before-create dedup idiom — the label / goal-ID /
title metadata contract a filed or linked artifact must satisfy to be recognized
as *coverage* by `detect_workstream_gaps` on the next tick.
**Relationship to prior artifacts:** VALIDATES and grounds
`tertiary_gap_routing_and_remediation_rung.md` §0–§2 against **live** repo/board
state. Adds the concrete, per-gap "what makes the predicate flip false" contract
and corrects two stale assumptions in the task framing.

---

## 0. TL;DR — the coverage contract (the only thing that stops recurrence)

A gap signature stops re-surfacing **only** when `detect_workstream_gaps`
(`sensor.rs:288`) skips it on the next tick. There are exactly two skip paths,
and each has a *different, structural* contract. **A label alone never clears a
gap. A GitHub issue never clears an issue-gap. Only the artifacts below do.**

| Gap kind | Signature | Skips iff | Concrete artifact that satisfies it |
|---|---|---|---|
| `issue:<repo>#<n>` | `issue:rysweet/Simard#<n>` | signature ∈ `coverage` set | an **OPEN PR** whose **title contains `#<n>`** OR whose **head branch matches `issue-<n>` / `issue/<n>`** |
| `goal:<id>` | `goal:<board-slug>` | `goal_has_active_workstream(g)` true | goal gains `assigned_to=Some(..)` **or** a `wip_ref` of kind `pr`\|`branch`\|`session`\|`engineer` |
| `anomaly:<slug>` | `anomaly:<slug>` | signature ∈ `coverage` set | (no producer wires anomaly coverage today — out of scope for the 10) |

Evidence: `sensor.rs:294` (`is_covered`), `:303` (`goal_has_active_workstream`
gate), `:334–336` (issue signature), `:377–384` (goal workstream predicate);
coverage set built in `wiring.rs:771` / `:861–880`; issue-ref extraction
`wiring.rs:885–916`.

---

## 1. Issue-gaps — the coverage contract is an OPEN PR, read STRUCTURALLY

`issue_coverage_from_open_prs` (`wiring.rs:861`) is the *entire* search-before-
create surface for issue-gaps. Each tick it runs `gh pr list --state open`
(≤100) and, per PR, extracts issue references via `issue_refs_from_pr`
(`wiring.rs:885`):

1. **Title:** every `#<digits>` token — `hash_issue_numbers` (`wiring.rs:903`).
2. **Branch:** digits immediately after the first `issue-` or `issue/` marker,
   case-insensitive, anywhere in the head branch name (`wiring.rs:887–898`).

Any matched `#<n>` becomes coverage signature `issue:rysweet/Simard#<n>`
(`wiring.rs:876`), which `detect_workstream_gaps` then dedups away
(`sensor.rs:336`). **Contract for the 8 issue-gaps:**

- **What works:** open a PR (draft is fine — `--state open` only checks open-ness,
  not readiness) that either (a) puts `#<n>` in the **title**, or (b) uses a head
  branch named `issue-<n>-...`. Either makes the next tick skip that signature.
- **What does NOT work (anti-patterns / traps):**
  - A **label** (`bug`, `workflow:default`, `P1`) on the issue — labels are what
    *qualifies* an issue as a gap (`HIGH_SIGNAL_LABELS`, `sensor.rs:253`), never
    what clears it. Adding a label makes recurrence *worse*.
  - A PR that references the issue only in its **body** — extraction reads
    **title + branch only**, never the body. A `Closes #<n>` in the body is
    invisible to the scan.
  - A **local/unpushed branch** named `issue-<n>` with no open PR — coverage is
    derived from `gh pr list`, not from branches. Verified live: branch
    `feat/issue-4164-...` exists on `origin`, but `gh pr list` shows **no** PR,
    so #4164 stays a gap (§4).
  - A **merged/closed** PR — `--state open` excludes it. Coverage is *active-work*
    coverage, not *ever-touched*.

> **Idiom name (patterns):** *structural linkage over declared linkage.* The scan
> trusts machine-readable structure (numeric `#n`, branch convention) and
> deliberately ignores free-text intent (issue body, comments, human "I'm on it").
> This mirrors the V3 note at `sensor.rs:332` — signatures are built from TRUSTED
> metadata (slug repo + numeric id), **never** the untrusted title.

---

## 2. Goal-gaps — the contract is board state, NOT any GitHub artifact

`goal_has_active_workstream` (`sensor.rs:377`) is the sole skip predicate for
`goal:<id>` (the `coverage` set passed in production contains **only** `issue:`
signatures — `wiring.rs:771`,`:876` — so goals are **never** cleared via the
coverage set, only via this predicate). It returns true iff:

- `goal.assigned_to.is_some()` (`sensor.rs:378`), **or**
- any `wip_ref.kind ∈ {"pr","branch","session","engineer"}` (`sensor.rs:381–383`).

**Explicit trap (`sensor.rs:376` doc + `:383`):** a `wip_ref` of kind **`issue`**
does **NOT** count — "an `issue`-only ref links a tracking issue, not active
work." So *filing a tracking issue for a goal and stapling it on as an `issue`
wip_ref does not clear the goal-gap.* This is the single most likely superficial-
resolution mistake for the two goal-gaps.

**How the predicate flips false in production:** the OODA loop stamps
`assigned_to = Some(agent_name)` when it spawns an engineer/session for the goal
(`ooda_actions/advance_goal/spawn.rs:665`,
`advance_goal/typed_goal_session.rs:431`); `ooda_loop/no_progress.rs:694` is the
only production site that pushes a `wip_ref`. Therefore the durable closing
actions for a goal-gap are:

1. **Raise priority / let the daemon spawn an engineer** — a p1 goal gets an
   engineer, `assigned_to` is stamped, gap clears. (The in-flight coverage doc
   `docs/howto/cover-overseer-workstream-gaps.md:128` recommends exactly
   `simard goal set-priority <id> 1`.) Note: raising priority does *not* remove it
   from the gap set by itself — p1 is still ≤ `GAP_GOAL_PRIORITY_BAR = 2`
   (`sensor.rs:249`); it clears **only** once the spawn actually assigns.
2. **Launch a workstream** that results in `assigned_to` or a `pr`/`branch` wip_ref
   on the board record.

A p2/p1 goal that is `Blocked` is delegated to `goal_health` and never flagged
here (`sensor.rs:300`), so "blocking" it is not a clear — it just moves the
signature to the blocked-goal lane.

---

## 3. Why the FileIssue dedup idiom does NOT apply to these gaps (the routing hole, re-grounded)

The `observer.rs` search-before-create idiom I was asked to characterize
(`StewardshipIssueFiler` → `process_orchestrator_run`, dedup on
`failure_signature(failure_kind, error_text)`, `observer.rs:34–68`,`:128–136`)
is real and idempotent — but `decide_read_only` routes `WorkstreamCoverage` to
`Intervention::Report`, **not** `FileIssue` (`observer.rs:113–120`). Two
consequences that constrain any remediation:

1. **Coverage gaps never reach the stewardship issue-dedup at all** — so the
   contract that clears them is the §1/§2 coverage contract, not the
   `failure_signature` contract. Confirms tertiary §1 "dual-path quarantine."
2. **If someone naively wired `WorkstreamCoverage → FileIssue` as the "fix,"**
   `problem_to_run_brief` sets `failure_kind = problem.dedup_key`
   (`observer.rs:133`), and the coverage `dedup_key` is the bare constant
   `"workstream-gap"` (per tertiary §2, `mod.rs:1371`). Every distinct gap would
   `failure_signature`-collapse into **one** issue — under-reporting, not
   coverage. **Any per-gap tracking artifact must key on `GapItem.signature`
   (`signal.rs:135–138`), never on `problem.dedup_key`.** This is the metadata
   contract for a *tracking issue* if one is ever filed: it must carry the
   per-gap `goal:<id>` / `issue:<repo>#<n>` identity, and — critically — filing
   the issue still does **not** clear the underlying gap unless it also produces
   the §1 (open PR) or §2 (assignee/branch) artifact.

---

## 4. Live grounding of the 10 gaps (evidence for the coverage table)

Verified at HEAD `7cb152ff` via `gh` + `~/.simard/state/goal_board.json`:

**8 issue-gaps — all OPEN, all high-signal-labelled, NONE has an open PR
referencing it (title/branch) → all genuinely uncovered:**

| Issue | State | Labels (why it qualifies) | Open PR ref? |
|---|---|---|---|
| #4164 | OPEN | `bug`,`dashboard` | **none** (branch `feat/issue-4164-…` pushed but **no PR** — does NOT cover) |
| #4126 | OPEN | `workflow:default` | none |
| #4078 | OPEN | `workflow:default` | none |
| #4074 | OPEN | `workflow:default` | none |
| #4051 | OPEN | `workflow:default` | none |
| #4046 | OPEN | `workflow:default` | none |
| #3698 | OPEN | `workflow:default` | none |
| #3201 | OPEN | `bug` | none |

Closing action per issue-gap: **link an OPEN PR** (title `#<n>` or branch
`issue-<n>`). Do **not** file a new issue (search-before-create; dedup rejects
and it worsens noise — these 8 already exist and already re-qualify every tick).

**2 goal-gaps — both present on the LIVE active board, both genuine gaps:**

| Goal id | pri | status | assigned | wip_refs | Verdict |
|---|---|---|---|---|---|
| `build-a-local-coin-benchmark-harness-and-a-self-09e65e35` | 2 | NotStarted | None | `[]` | GAP (p≤2, not blocked, no workstream) |
| `steward-ci-github-actions-health-across-all-gov-e06d9e64` | 2 | NotStarted | None | `[]` | GAP |

Closing action per goal-gap: give it `assigned_to` or a `pr`/`branch`/`session`/
`engineer` wip_ref — in practice **set-priority→spawn engineer** or **launch a
workstream**. An `issue` wip_ref or a bare tracking issue does NOT clear it.

---

## 5. Corrections to the task framing (evidence-backed)

1. **#4164 is NOT the coverage meta-issue.** Its real title is
   *"fix(cost): meeting-mode turns undercount prompt tokens, breaking dashboard
   Cost tab"* (`bug`,`dashboard`). The branch
   `feat/issue-4164-cover-uncovered-backlog-workstreams-surfaced-by-th` is
   **mislabelled** relative to the issue subject and, more importantly, has **no
   open PR**, so it provides **zero** coverage today. Treat #4164 as an ordinary
   issue-gap needing an open PR.
2. **#4126 is the systemic meta-workstream** (title *"Make the Overseer ACT on
   the workstream gaps it detects instead of only…"*, `workflow:default`). It is
   OPEN with no open PR → itself an uncovered gap. Until #4126 lands, this manual
   coverage pass is **recurring per tick**, exactly as the prior discoveries note.
3. **The `coverage` set never carries `goal:` signatures** in production
   (`wiring.rs:771` passes only `issue_coverage_from_open_prs`). Any plan that
   assumes a goal can be "added to the coverage set" is wrong; goals clear **only**
   through `goal_has_active_workstream`.

---

## 6. Integration points & concerns for the verification phase

- **Integration seam (issues):** `wiring.rs:861 issue_coverage_from_open_prs` +
  `:885 issue_refs_from_pr` — the ONLY place open-PR→issue linkage is computed.
  Verify any new PR's title/branch actually matches here (dry-run
  `issue_refs_from_pr` against the PR's real title + `headRefName`).
- **Integration seam (goals):** `sensor.rs:377 goal_has_active_workstream` +
  producers `advance_goal/spawn.rs:665`. Verify the board record for each goal
  actually gains `assigned_to`/PR-branch wip after the launch (read
  `~/.simard/state/goal_board.json`).
- **Concern — draft-PR reliance:** coverage counts *open* PRs, so a draft PR
  clears the gap immediately even before review. That is acceptable for coverage
  (work is in flight) but means a **stale/abandoned draft masks a gap
  indefinitely** with no cross-window recurrence memory (tertiary §0). Flag for
  the operator: coverage ≠ progress.
- **Concern — branch marker greediness:** `lower.find("issue-")` takes the FIRST
  `issue-` marker anywhere in the branch; a branch like
  `feat/issue-4164-cover-…-4130` yields only `4164`. Multi-issue branches under-
  cover. Prefer `#<n>` in the PR **title** for each issue you intend to cover.
- **Verification query (covered ⇒ filtered next tick):** `tests_gap_scan.rs:388`
  (`ignores_goal_covered_by_pr_assignment_or_coverage_set`) and `:453` (issue in
  coverage set) are the canonical assertions proving a covered signature is
  dropped. A covered gap must reproduce those preconditions.

## 7. Questions for the verification phase

1. For each of the 8 issue-gaps: will the chosen PR carry `#<n>` in its **title**
   (recommended) or rely on the `issue-<n>` branch marker? Confirm via a dry-run
   of `issue_refs_from_pr` on the actual title+branch.
2. For the 2 goal-gaps: does the chosen action (set-priority-1 vs explicit
   launch) actually stamp `assigned_to`/wip on the board record, or does it merely
   raise priority (still a gap until spawn)? Verify against
   `~/.simard/state/goal_board.json` after one OODA cycle.
3. Is #4164's mislabelled branch expected to become the coverage PR, and if so
   should its title carry `#4164` to satisfy the contract — or is #4164 out of
   scope because it's a cost bug unrelated to coverage?
4. Does #4126 (systemic auto-file/launch) supersede the need to manually cover
   the other 9 once it lands, and should the 9 be linked as blocked-by/dependent
   on #4126 rather than individually PR-covered?
