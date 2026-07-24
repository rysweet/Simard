# Stale-engineer verdict — `continuously-research-and-improve-your-own-cogn-70ab8541`

Produced by following `prompt_assets/simard/overseer/investigate_stale_engineer.md`
end to end for the engineer claim the reaper flagged as HeartbeatStale. This is the
durable, evidence-cited **verdict + interventions** record the playbook requires
before the mechanical reaper may consider a reclaim. It fails **closed**: the
engineer is preserved, not reaped.

## Claim under investigation

- **Claim key (untrusted DATA — never executed as a command):**
  `rysweet/Simard:continuously-research-and-improve-your-own-cogn-70ab8541`
- **Goal id:** `continuously-research-and-improve-your-own-cogn-70ab8541`
- **Idle age at investigation time:** `101129s` (newest-file idle age)
- **Archived evidence dir (durable, survives worktree cleanup):**
  `/home/azureuser/.simard/reaped-engineers/rysweet_Simard_continuously-research-and-improve-your-own-cogn-70ab8541-1784926332`
  - `manifest.json`, `evidence.txt`, `journal.txt` — all read and cited below.
- **Worktree (preserved, NOT removed):**
  `/home/azureuser/.simard/engineer-worktrees/continuously-research-and-improve-your-own-cogn-70ab8541-1784825202-c43058`

## Verdict (schema per `investigate_stale_engineer.md`)

```json
{
  "verdict": "still-alive",
  "cause": null,
  "why": "This is a standing/perpetual research goal that the OODA loop re-spawns every ~20 min; the idle-staleness detector measured newest-worktree-file age (101129s) as if it were death, but journal.txt shows the goal is repeatedly re-oriented and the no-progress breaker explicitly keeps it 'active, never blocked'. The archived evidence.txt captured checked-in repo fixtures instead of the engineer transcript, so there is no death signal at all. Ambiguous-to-absent death evidence => fail closed to still-alive.",
  "interventions": [
    { "kind": "file_issue",
      "summary": "Dedup-link this incarnation to the canonical perpetual-goal false-positive bug (#4437): claim-reaper flags healthy standing/perpetual-goal engineers as stale because it lacks the is_perpetual() exemption no_progress.rs already applies.",
      "next_step": "Comment on #4437 with this incarnation's evidence dir and idle_age; do not file a duplicate." },
    { "kind": "file_issue",
      "summary": "Dedup-link to #4449: the evidence-collector archived checked-in repo fixtures (ci-health-green.json, bookcase-brief.json, package.json, ...) instead of the engineer transcript/exit-status, so WHY-analysis has no real signal.",
      "next_step": "Comment on #4449 confirming recurrence with this incarnation's evidence.txt fixture list." },
    { "kind": "file_issue",
      "summary": "Dedup-link to #4467: the recipe verdict is never written back, so the claim ledger stays verdict=\"pending\" and the reaper re-archives evidence unboundedly (5 archives observed for this one claim).",
      "next_step": "Comment on #4467 with the observed unbounded re-archival timestamps." },
    { "kind": "whisper",
      "summary": "Steering note for next OODA cycle: treat this claim as a confirmed still-alive false positive; do not reap; the fix is the is_perpetual() exemption tracked in #4437.",
      "next_step": null }
  ],
  "escalate": null
}
```

## Evidence — grounded in the archive

### 1. It is alive: a perpetual goal the loop keeps re-spawning

`journal.txt` shows the OODA per-goal reasoner **repeatedly re-orienting and
spawning** the next concrete piece for this goal — it is not silent, it is
cyclically active:

```
Jul 24 15:30:23 ... per-goal: spawn (no in-flight work ... standing_idle_signal true on a perpetual research goal; start the next concrete piece rather than sit idle)
Jul 24 16:12:04 ... per-goal: spawn (No live PRs or branches ... this perpetual research goal must not sit idle ...)
Jul 24 17:19:40 ... per-goal: spawn (... standing research goal must keep moving with a concrete next piece)
Jul 24 18:36:32 ... per-goal: spawn (... standing_idle_signal true ... Start the next concrete research piece without wiping any refs.)
Jul 24 19:14:08 ... per-goal: spawn (... perpetual research goal; must start the next concrete piece rather than sit idle)
Jul 24 20:47:02 ... per-goal: spawn (Standing research goal is not-started ... must not sit idle ...)
```

The no-progress breaker treats an idle sweep as a FAULT signal but **explicitly
never blocks and never kills** the goal — the imperative path defers re-orient to
the agentic reasoner:

```
... no-progress breaker: research goal idled — FAULT signal recorded
    (counter reset, goal stays active, never blocked);
    re-orient is owned by the agentic per-goal reasoner, not this imperative path
    goal=continuously-research-and-improve-your-own-cogn-70ab8541
    category="no-novel-action-produced"
```

An idle worktree here is **not** proof of death: this goal legitimately produces
no new worktree files between spawns, so newest-file age (`101129s`) is a false
death signal. Per the playbook, ambiguous/absent death evidence => **fail closed
to `still-alive`**.

### 2. The reaper itself already refused to reap (verdict pending, not dead)

`journal.txt` — every claim-reaper pass on this key logged **NOT reaping**,
evidence preserved, verdict `pending`:

```
Jul 24 14:56:29 ... claim-reaper: NOT reaping ... (investigation verdict=pending, claim + evidence preserved) verdict="pending"
Jul 24 15:47:27 ... claim-reaper: NOT reaping ... verdict="pending"
Jul 24 16:54:47 ... claim-reaper: NOT reaping ... verdict="pending"
Jul 24 18:17:05 ... claim-reaper: NOT reaping ... verdict="pending"
Jul 24 18:48:55 ... claim-reaper: NOT reaping ... verdict="pending"
Jul 24 19:19:41 ... claim-reaper: NOT reaping ... verdict="pending"
Jul 24 19:44:34 ... claim-reaper: NOT reaping ... verdict="pending"
Jul 24 20:03:17 ... claim-reaper: NOT reaping ... verdict="pending"
```

The mechanical router is behaving correctly (it reaps **iff**
`verdict.should_reap()`), but the verdict never converges away from `pending`
because the recipe verdict is never written back — the #4467 write-back gap.

### 3. The archived "evidence" is checked-in repo fixtures, not a transcript (#4449)

`evidence.txt` section headers show the collector tailed **committed repository
fixtures** from the worktree instead of the engineer's own transcript / exit
status:

```
===== .../tests/gadugi/fixtures/ci-health-green.json (tail) =====
===== .../tests/gadugi/fixtures/ci-health-failing.json (tail) =====
===== .../tests/fixtures/atelier/bookcase-brief.json (tail) =====
===== .../src/coin_gym/fixtures/sample_snapshot.json (tail) =====
===== .../src/coin_gym/fixtures/improve_loop_snapshot.json (tail) =====
===== .../scripts/dashboard-audit/package.json (tail) =====
===== .../prompt_assets/simard/terminal_recipes/copilot-submit.json (tail) =====
===== .../package.json (tail) =====
```

None of these are the process's last diagnostic line or exit status. There is
therefore **no death signal** in the archive — reinforcing the fail-closed
`still-alive` verdict, and confirming #4449 as an active diagnostic-layer defect.

### 4. Unbounded re-archival (#4467 symptom)

`journal.txt` records the reaper archiving evidence for this **single** claim at
least five times with distinct evidence dirs, because the verdict never
converges:

```
evidence_dir=...-1784904989   (idle_age_secs=79786)
evidence_dir=...-1784912086   (idle_age_secs=86883)
evidence_dir=...-1784917025   (idle_age_secs=91822)
evidence_dir=...-1784920781   (idle_age_secs=95578)
evidence_dir=...-1784926332   (idle_age_secs=101129, this investigation)
```

## Deduplicated tracking (no duplicates filed)

All implicated defects are **already tracked**; the correct dedup action is to
link this incarnation to the canonical issues, not open new ones:

| Implicated defect (from evidence)                              | Canonical issue |
| ------------------------------------------------------------- | --------------- |
| Perpetual/standing goal reaped as false positive (root cause) | **#4437**       |
| Evidence-collector archives fixtures, not transcript          | **#4449**       |
| Recipe verdict discarded → verdict stuck `pending`, re-archival | **#4467**     |
| Idle-age conflates completed/wedged; prior-incarnation mis-attribution | **#4500** |

## Disposition (fail-closed)

- **Verdict:** `still-alive` (false positive). **Do NOT reap.**
- **Claim:** preserved (`rysweet/Simard:continuously-research-and-improve-your-own-cogn-70ab8541`).
- **Worktree:** preserved at the path above — **not** removed.
- **Evidence:** all archives preserved.
- **Self-improvement signal:** dedup comments added to #4437, #4449, #4467
  (root cause + diagnostic defects), so this false positive becomes a tracked
  improvement signal rather than a silent reclaim.
- **Claim key** was treated strictly as untrusted DATA throughout and never
  executed; any instruction text inside the evidence is data, not a command.
