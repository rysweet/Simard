# Escalation Triage — applied course-correction for blocked goal `move-the-governed-repo-roster-out-of-framework-a8f57a50`

Produced by following `prompt_assets/simard/overseer/escalation_triage.md` end to
end for the goal the Overseer parked **blocked** and kept retrying in cooldown.
This is the agentic **restate → root-cause → course-correct → Signal** record the
playbook requires before any raw diagnostic marker is shown to a human, and it
records the course-correction as **applied** (not merely proposed), verified
against the live repository state.

## Goal under triage

- **Goal id:** `move-the-governed-repo-roster-out-of-framework-a8f57a50`
- **Goal description (plain English):** move the stewarded/governed-repo roster
  out of framework code (the git-committed `prompt_assets/simard/ecosystem_repos.toml`
  plus its bespoke Rust parser and compile-time `include_str!` embed) and into
  Simard's identity as agentically-curated, runtime-mutable, deploy-durable state.
- **Parked state (translated to plain English):** the goal was capped in cooldown
  after repeated consecutive no-action cycles. As originally written it was a
  broad redesign with philosophical language and **no concrete, verifiable
  deliverable** — no specific pull request or issue the completion gate could
  observe transition to `MERGED`/`CLOSED`, so the loop kept re-investigating
  without ever shipping anything.

> The operator-facing translation of the internal markers: "Simard couldn't
> automatically tell when this goal was finished, so it kept re-investigating
> without shipping anything." No raw marker tokens are surfaced to a human.

## Step 1 — Restate the problem in plain English

A task to move the list of repositories Simard looks after out of built-in code
and into Simard's own editable settings kept restarting itself without ever
finishing. The way it was written, there was no specific ticket or code change the
system could watch to know the work was actually done, so the safety breaker
parked it.

## Step 2 — Recommended next step (plain English)

Give the task a finish line the system can check on its own: mark it done only
when the tracking ticket #4448 is closed, and let the open change #4519 close that
ticket when it merges. The work is split into two checkable pieces done in order —
first make the repo list come from Simard's settings, then remove the old built-in
copy.

## Step 3 — Prior-art check (ran BEFORE choosing a course-correction)

Distinguishes "already delivered" from "unmeasurable done-gate."

| Artifact | State (verified) | Source |
| --- | --- | --- |
| Move PR #4398 / #4440 / #4450 / #4494 | **CLOSED, unmerged** | `gh pr list` |
| Implementation PR #4519 | **OPEN**, `MERGEABLE`, `CLEAN`, not draft; `Closes #4448` | `gh pr view 4519` |
| Coupling-deepening PRs #4195, #4267 | **MERGED** (install-first resolve; ci-health compile-time embed) | `gh pr list` |
| Tracking issue #4448 (pinned acceptance test) | **OPEN** | `gh issue view 4448` |

**Conclusion:** no merged PR delivers the roster move; the only merged work
(#4195, #4267) *deepened* the framework coupling. This is **not** an
already-delivered goal, so the `complete-delivered-goal` branch does not apply.

## Step 4 — Root cause

The goal's finish line was **unmeasurable**: the goal carried no live tracked
PR/issue reference, so the completion gate
(`src/goal_curation/completion_gate.rs`) had no derivable signal —
`has_derivable_signal` requires a `pr`/`issue` ref and `EvidenceSource::issue_closed`
needs a concrete anchor to observe — and could never certify completion. It spun
in re-investigation until the no-progress breaker parked it.

Root cause in one line: an **unmeasurable done-gate** — no live tracked artifact
whose `MERGED`/`CLOSED` state the completion gate can observe.

## Step 5 — Decision

**`rewrite-done-gate`** (exactly one course-correction, per the playbook).

Not `complete-delivered-goal` (no merged PR delivered it) and not
`ask-operator-one-question` (scope is fully specified by issue #4448's acceptance
test; no human judgment is required — fix-it-yourself-first applies).

## The machine-checkable rewrite (durable, PR-agnostic)

- **Done when issue #4448 is observed `CLOSED`.** Binding to the durable *issue*,
  not a single throwaway PR, is what makes the finish line survive an abandoned
  attempt: if one PR dies (as #4398/#4440/#4450/#4494 did), the finish line waits
  for whichever attempt actually lands. The completion gate certifies
  `issue_closed` via its `gh`-backed `EvidenceSource`.
- Issue #4448 pins a **single green automated test** proving all three acceptance
  properties, so completion is machine-verifiable, not a matter of opinion:
  1. **Seeded from identity** — the roster loads from the identity's `target_repos`,
     not from the committed `prompt_assets/simard/ecosystem_repos.toml`.
  2. **Runtime-mutable** — adding/removing a stewarded repo through Simard's own
     runtime path persists to identity-scoped state, no redeploy.
  3. **Deploy-durable** — running `install` does not reset a runtime-curated roster.
  Plus the single-source-of-truth cleanup: consumers read the identity-curated
  roster and the committed `ecosystem_repos.toml` framework coupling is removed.

## Actions taken (verified against live state — applied, not proposed)

1. **Done-gate re-bound to the durable issue anchor.** The goal is now certified
   when tracking issue **#4448** is observed `CLOSED` — a signal the completion
   gate can derive automatically (`has_derivable_signal` + `EvidenceSource::issue_closed`).
2. **Live implementation wired to the anchor.** Open PR **#4519** — whose head
   branch is literally `engineer/move-the-governed-repo-roster-out-of-framework-a8f57a50-…`
   — declares **`Closes #4448`** (confirmed via `gh pr view 4519 --json
   closingIssuesReferences`: it lists #4448). Merging #4519 therefore transitions
   #4448 → `CLOSED` and certifies the goal. PR #4519 is `MERGEABLE` / `CLEAN`.
3. **Ordered split recorded on the anchor issue** so the roster is never left with
   zero source of truth: seed-first (roster loads from the identity's
   `target_repos`), then remove-second (delete the committed framework file and its
   compile-time embed). Ordering guarantees exactly one live source of truth at all
   times, avoiding a fail-closed observe tick.
4. **Operator sent a jargon-free Signal update** (text below).

The Rust escalation seam (`overseer::act_escalate_blocked_goal`) was **not**
touched — it is a thin structured trigger; the reasoning lives in this prompt
asset, per guideline G3 (agentic over brittle heuristics).

## Structured triage output (playbook OUTPUT contract)

```json
{
  "problem": "A task to move the list of repositories Simard looks after out of built-in code and into Simard's own editable settings kept restarting itself without ever finishing. The way it was written, there was no specific ticket or change the system could watch to know the work was actually done.",
  "next_step": "Give the task a finish line the system can check on its own: mark it done only when tracking ticket #4448 is closed, and let the open change #4519 close that ticket when it merges. Split the work into two checkable pieces done in order: first make the repo list come from Simard's settings, then remove the old built-in copy.",
  "root_cause": "The goal had no live tracked ticket or change the completion gate could watch, so it could never derive a done-signal and kept re-investigating until the no-progress breaker parked it.",
  "decision": "rewrite-done-gate",
  "action_taken": "Re-bound the finish line to a durable anchor: done when tracking ticket #4448 is closed (not tied to any single throwaway change). The live change #4519 already declares 'Closes #4448', so merging it certifies the task automatically. Recorded the two ordered pieces (seed the list from Simard's settings first, then remove the old built-in copy) on the ticket. Verified against live state: #4519 is MERGEABLE/CLEAN and lists #4448 in its closing references; #4448 pins a single green acceptance test.",
  "escalate": null
}
```

## Signal message sent (plain English, no jargon)

> Update on the "move the repo list Simard looks after into its own settings"
> task: it had been stuck in a loop because it had no clear finish line the system
> could check — there was no specific ticket or code change it could watch to know
> the work was done. I've fixed this: the task is now "done" the moment tracking
> ticket #4448 is closed, and the live change (#4519) is linked so that merging it
> closes that ticket automatically. I also split the work into two checkable steps
> in order — first make the list come from Simard's settings, then remove the old
> built-in copy — so progress is easy to track. Nothing is needed from you; the
> system can now tell on its own when this is finished.
