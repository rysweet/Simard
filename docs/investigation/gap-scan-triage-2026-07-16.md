# Gap-Scan Recurrence Triage — 16 recurring backlog gaps (2026-07-16)

Purpose: per-gap triage of the 16 recurring gaps the Overseer gap-scan surfaces
(2 goal-level gaps + 14 issue gaps) so each has an established status and an
explicit **coverage decision** (LAUNCH NEW / TRACK EXISTING / CLOSE-RESOLVED)
mapped to an owning workstream. This prevents the gaps from recurring as
"uncovered" in the next gap-scan.

This is the operational triage complement to the root-cause investigation on
branch `investigation/recurring-blocked-goals-workstream-gaps` (PR #4093), which
established *why* the gaps recur (notify-only `FlagWorkstreamGaps` edge, the
escalate-at-3 dead zone, and OODA terminals mis-recorded as failures). That
mechanism analysis informs — but does not substitute for — the per-gap decisions
below.

## Root systemic driver (context)

Two mechanism defects explain why *covered* work keeps re-appearing as an
uncovered gap:

1. **Notify-only coverage edge** — the Overseer detects an uncovered gap each
   tick but only pings the operator; it never opens a tracked workstream. Fix
   is tracked by **#4126** (make the Overseer ACT on workstream gaps).
2. **OODA terminals mis-recorded as failures** — the typed goal-session actor
   never records a durable terminal on the live daemon, so goals with real
   progress are counted `blocked`/failed and re-surface. Fix is tracked by
   **#4074**.

Closing #4126 and #4074 removes the recurrence engine; the per-gap decisions
below clear the current backlog.

## Coverage decision legend

- **TRACK EXISTING** — an owning branch/PR/issue already covers this gap; no new
  workstream needed. Recorded here so the gap-scan can treat it as covered.
- **LAUNCH NEW** — no owning workstream exists; a new one must be opened.
- **CLOSE-RESOLVED** — the work is already merged or the issue is a
  superseded/duplicate; the gap is closed and should not recur.

---

## Goal-level gaps (2)

### G1 — `goal:build-a-local-coin-benchmark-harness-and-a-self-…09e65e35`

- **Status:** ACTIVE, heavily worked. Owning workstream branch
  `engineer/build-a-local-coin-benchmark-harness-and-a-self-09e65e35` with a
  live PR train: #4171 (`verify` acceptance self-check / done-gate), #4161
  (`bench`), #4149 (`matchup`), #4134 (`duel`), #4101 (LOCAL leaderboard);
  earlier #3190 merged. Source lives under `src/coin_gym/` and `docs/research/
  coin-benchmark*.md`.
- **Why it recurs as a gap:** the goal is parked `blocked` in the runtime goal
  store while its PRs are open — the gap-scan reads goal state, not PR coverage.
- **Coverage decision:** **TRACK EXISTING** (owning workstream = the coin-gym
  engineer branch). No new workstream needed.
- **Scope to terminalize:** merge PR #4171 (adds the measurable done-gate); once
  the `verify` acceptance self-check lands, the goal has a checkable terminal and
  should stop parking as blocked.

### G2 — `goal:steward-ci-github-actions-health-across-all-gov-…e06d9e64`

- **Status:** ORPHANED — no branch, no PR, no tracking anchor. Only tangential,
  individually-owned CI items exist.
- **Coverage decision:** **LAUNCH NEW.** A dedicated tracking workstream was
  created: **#4172** (`tracking(ci-stewardship): owning workstream for goal
  steward-ci-github-actions-health-across-all-gov (e06d9e64)`).
- **Scope:** umbrella stewardship of CI / GitHub-Actions health, coordinating
  #2336 (failing scheduled Actions), #3201 (coverage.yml lbug provisioning),
  #3698 (smart-orchestrator preflight), #2975 (apt-signing verify flake), #2471
  (lbug SHA-256 verification), #1647 / #1262 (verify wall-time), and the
  Dependabot Actions bumps #4064/#4065/#4066.
- **Done-gate:** scheduled Actions green-or-triaged, coverage.yml fixed, and a
  recurring CI-health check so regressions surface without a human.

---

## Issue-level gaps (14)

| # | State (2026-07-16) | Summary | Coverage decision | Owning workstream |
|---|---|---|---|---|
| #4164 | OPEN | meeting-mode turns undercount prompt tokens → dashboard Cost tab wrong | **LAUNCH NEW** | new default-workflow workstream (cost/dashboard) |
| #4139 | CLOSED (not-planned) | "recurring signature seen 2×" auto-issue (duplicate) | **CLOSE-RESOLVED** | superseded by #4126 + PR #4093 |
| #4130 | CLOSED (not-planned) | same recurring-signature auto-issue (duplicate) | **CLOSE-RESOLVED** | superseded by #4126 + PR #4093 |
| #4126 | OPEN | Make the Overseer ACT on detected workstream gaps (systemic) | **LAUNCH NEW** (highest leverage) | owning workstream for the whole recurrence class; implements round-1 defect D3 |
| #4120 | CLOSED (not-planned) | same recurring-signature auto-issue (duplicate) | **CLOSE-RESOLVED** | superseded by #4126 + PR #4093 |
| #4112 | CLOSED (not-planned) | same recurring-signature auto-issue (duplicate) | **CLOSE-RESOLVED** | superseded by #4126 + PR #4093 |
| #4099 | CLOSED by this sweep | periodic stale-engineer-claim reaper | **CLOSE-RESOLVED** | implemented & merged in PR #4104 (closed in this sweep) |
| #4078 | OPEN | self-diagnose a failed OODA step (body garbled by context-window error) | **LAUNCH NEW** (re-scope first) | malformed auto-issue; needs a clean re-scoped brief before work |
| #4074 | OPEN | typed goal-session actor never records durable terminal → every OODA goal counted a failure | **LAUNCH NEW** (high priority) | new default-workflow workstream; root cause of blocked-goal recurrence |
| #4051 | OPEN | agentic goal-session interpretation path (stop brittle marker parsing) | **LAUNCH NEW** (design-heavy) | new default-workflow workstream (OODA architecture) |
| #4046 | OPEN | Quality-Audit self-improvement review of goal_session paths | **LAUNCH NEW** | new quality-audit workstream |
| #3698 | OPEN | smart-orchestrator preflight still references orch_helper.py | **TRACK EXISTING** | tracked under CI-stewardship #4172 (amplihack infra) |
| #3598 | CLOSED | smart-orchestrator AMPLIHACK_HOME preflight orch_helper.py | **CLOSE-RESOLVED** | closed; open sibling #3698 carries any residual work |
| #3201 | OPEN | coverage.yml fails at lbug provisioning after #3171 forked lbug | **TRACK EXISTING** | tracked under CI-stewardship #4172 |

---

## Actions taken in this sweep

- **Closed #4099** as completed — the stale-engineer-claim reaper it specifies
  was implemented and merged in PR #4104. Comment added recording the decision.
- **Created #4172** — the missing owning/tracking workstream for goal-level gap
  G2 (`steward-ci…e06d9e64`), with scope and done-gate, so the goal is no longer
  uncovered.
- **Recorded coverage for every remaining gap** in the tables above so each maps
  to a concrete decision and owning workstream.

## Recurrence-prevention summary

| Bucket | Gaps | Recurs after this sweep? |
|---|---|---|
| CLOSE-RESOLVED | #4099, #4139, #4130, #4120, #4112, #3598 | No — closed |
| TRACK EXISTING | G1 (coin-gym), #3698, #3201 | No — owning workstream recorded |
| LAUNCH NEW (anchored) | G2 → #4172 | No — tracking issue opened |
| LAUNCH NEW (needs workstream) | #4164, #4126, #4074, #4051, #4046, #4078 | Only until each launches; #4126 + #4074 remove the recurrence engine |

The two remaining systemic levers are **#4126** (act on gaps instead of
notify-only) and **#4074** (record durable OODA terminals). Landing both stops
covered work from re-surfacing as uncovered gaps in future gap-scans.
