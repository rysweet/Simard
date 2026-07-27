# Escalation-triage — blocked goal `steward-ci-github-actions-health-across-all-gov-e06d9e64` (self-deploy deadlock)

HEAD: `bbb0d678` · Playbook: `prompt_assets/simard/overseer/escalation_triage.md`
Decision: **complete-delivered-goal** (the remedy is already delivered by a merged
PR; applied its one-time prerequisite) · escalate: **null** (no human decision
required).

This run supplies the ACTION deliverables the prior investigation round did not:
the applied course-correction, the sent jargon-free Signal messages, and this
verifiable artifact. All internal diagnostic markers were used only as evidence
for the plain-English reasoning below and are **never** surfaced to the operator.

---

## 1. Escalation input (raw — translated below, never forwarded verbatim)

- `goal_id`: `steward-ci-github-actions-health-across-all-gov-e06d9e64`
- `problem_seed`: "Simard cannot upgrade herself. Every automatic self-deploy over
  the last several hours has failed, so she is stuck running an old build that is
  now 6 changes behind the latest merged code and is falling one further behind
  each cycle."
- `next_step_seed`: clear the leftover local edit in the self-deploy checkout so
  the update can proceed, then let the next deploy run.
- `internal_why` (translate, do not surface): self-deploy chicken-and-egg — the
  running `0.40.0` binary predates PR #4898's `reset_source_tree`, so
  `git checkout --detach <target>` in `~/.simard/self-deploy-src` aborts on a dirty
  tracked file (`.github/hooks/amplihack-hooks.json`); ~8 consecutive failed ticks,
  DeployDrift 2→6, ~20 self-deploy refs in `ooda.log`; permanent fix merged as
  `76512653b` but cannot land until the tree is unwedged once.
- `reason_marker` (translate, do not surface): `health-review:self-deploy-deadlock`.

## 2. PROBLEM — plain English

Simard could not install her own updates. For several hours every automatic
self-update failed, leaving her running an older build that had drifted about six
approved changes behind the latest code, slipping one further behind each cycle.

## 3. ROOT CAUSE (grounded in re-verified live evidence)

A **self-deploy chicken-and-egg deadlock**:

1. Simard's self-update workspace `~/.simard/self-deploy-src` had one tracked file
   with an uncommitted local edit — `.github/hooks/amplihack-hooks.json`, rewritten
   by the amplihack install step from relative hook paths to absolute ones.
   Verified live: `git -C ~/.simard/self-deploy-src status` showed
   `M .github/hooks/amplihack-hooks.json` (workspace `HEAD detached at a350b24d`).
2. The update switches the workspace to the target commit with a plain detached
   checkout, which **aborts** when a tracked file has local edits. So the update
   failed every tick — verified in `~/.simard/ooda.log`: `deploys=0` and a
   persistent `errors=1` on every recent overseer tick; the same dirty file recurs
   across `~/.simard/cycle_reports/cycle_*.json`.
3. The **permanent fix is already merged** — PR #4898 (rolls up #4878),
   commit `76512653b` "fix(self-deploy): fail-closed canonical-path gate for source
   tree reset" — which hard-resets the source tree before checkout so a stray edit
   can never wedge the update again. Verified present in this deploy repo's history.
4. But the **running binary is `0.40.0`** (`~/.simard/bin/simard --version`), built
   before that fix, so it cannot self-heal the wedge. The fix can only take effect
   *after* one successful deploy installs the newer binary — which the wedge itself
   was preventing. Hence the deadlock.

## 4. DECISION — course-correction chosen

**complete-delivered-goal** — the remedy this block needs has already shipped in a
merged PR (`76512653b` / #4898 / #4878). Per the playbook, the correct move is to
apply the delivered fix rather than leave the system blocked or escalate.

Two determinations that shaped the exact action (and corrected the prior round):

- **The goal is a STANDING goal.** Its description ends "Standing goal.", so
  `description_marks_standing` → `ActiveGoal::is_perpetual()` is **true**
  (`src/goal_curation/types.rs`; pinned by the unit test asserting
  `description_marks_standing("Standing goal")`). Consequences:
  - The Overseer gap-scan **exempts** standing goals — it "would otherwise oscillate
    uncovered every cycle" (`src/overseer/sensor.rs::detect_workstream_gaps`).
    Confirmed live: `workstream_gaps_detected=0` on every recent tick, and the goal's
    `no_progress` count is **0**. Its idle cycles are benign **by design**
    (`src/ooda_loop/no_progress.rs`: "Non-research standing goal (e.g. CI-stewardship).
    Idling is NORMAL").
  - `simard goal complete <id>` on a standing goal **reopens** it
    (`roll_to_new_cycle`) rather than removing/tombstoning it. So the prior round's
    plan to "complete + tombstone" this goal would NOT have closed anything and is
    semantically wrong — an ongoing CI-stewardship duty must remain active.
- Therefore "complete-delivered-goal" was applied to the **blocker's delivered
  remedy**, not by terminating the standing goal: I applied the one-time prerequisite
  the merged fix needs (unwedge the tree) so the next deploy installs it. The
  standing goal is left **active** (correct) and returns to healthy operation once
  the update lands.

`rewrite-done-gate` was rejected: a standing goal has no terminal done-gate to make
machine-checkable, and idling is its normal state. `ask-operator-one-question` was
rejected: no human decision is required — the unwedge is a mechanical action the
agent can perform, and the playbook mandates fixing the block yourself before
escalating.

## 5. ACTION TAKEN (applied, not proposed)

On the daemon host, cleared the leftover edit so the update can proceed:

```
git -C ~/.simard/self-deploy-src reset --hard   # HEAD is now at a350b24d
git -C ~/.simard/self-deploy-src clean -fd       # (no untracked files to remove)
```

Verified afterward: `git -C ~/.simard/self-deploy-src status --short` is **empty**
(clean), and `git -C ~/.simard/self-deploy-src checkout --detach HEAD` now returns
**rc=0** — the operation that had been aborting now succeeds. No deploy was in
progress during the reset (no deploy lock/process held the workspace), so this was
safe against the live loop. The next self-deploy tick installs the newer binary,
whose `reset_source_tree` self-heal permanently prevents recurrence.

## 6. SIGNAL — jargon-free operator updates (all sent, `type: SUCCESS`)

Sent to the operator over the live Signal JSON-RPC channel
(`account +12062591306`); every message returned a `SUCCESS` receipt. No marker
tokens, no version numbers, no git/PR jargon — plain English only:

1. "Heads-up from Simard: I noticed I hadn't been able to install my own updates for
   the last several hours, so I was stuck running an older version that had fallen
   about 6 improvements behind the latest approved code."
2. "I looked into why: a small leftover settings file in my update workspace had
   been changed locally, and that stray change was jamming every attempt to switch
   to the newer version."
3. "I cleared that leftover change so my update can go through. The next automatic
   update should now install cleanly, and the newer version already includes a
   permanent fix that stops this from happening again on its own."
4. "All set — nothing needed from you. I'll keep an eye on the next update and let
   you know if anything else comes up."

## 7. OUTPUT (playbook contract)

```json
{
  "problem": "Simard could not install her own updates; for several hours every automatic self-update failed, leaving her on an older build about six approved changes behind and slipping one further behind each cycle.",
  "next_step": "Clear the one leftover local edit in Simard's update workspace so the switch to the newer version can complete, then let the next automatic update run.",
  "root_cause": "A self-deploy chicken-and-egg deadlock: the update does a plain detached checkout that aborts on a locally-edited tracked file (.github/hooks/amplihack-hooks.json), and the running build predates the merged fix that would auto-clear such edits, so the fix could not land until the workspace was unwedged once by hand.",
  "decision": "complete-delivered-goal",
  "action_taken": "Applied the delivered fix's one-time prerequisite: ran 'git -C ~/.simard/self-deploy-src reset --hard && git -C ~/.simard/self-deploy-src clean -fd', restoring a clean workspace (detached checkout now succeeds, rc=0). Left the standing CI-stewardship goal active by design (a standing goal is gap-scan-exempt and 'complete' would only reopen it). The permanent fix (merged PR #4898/#4878, 76512653b) self-heals future recurrences after the next deploy installs it. Sent four jargon-free Signal updates to the operator, all with SUCCESS receipts.",
  "escalate": null
}
```

## 8. Verification (definition of done)

- `git -C ~/.simard/self-deploy-src status --short` → empty (clean). ✔
- `git -C ~/.simard/self-deploy-src checkout --detach HEAD` → rc=0 (the previously
  aborting operation now succeeds). ✔
- Permanent fix `76512653b` (PR #4898/#4878) present in the deploy repo history;
  once the next deploy installs the newer binary, `reset_source_tree` prevents
  recurrence. ✔
- Four Signal messages delivered with `type: SUCCESS`; no marker tokens surfaced. ✔
- Standing goal left active (correct): gap-scan-exempt, `no_progress`=0,
  `workstream_gaps_detected=0`; not completed/tombstoned because that would reopen
  it and is wrong for an ongoing duty. ✔
