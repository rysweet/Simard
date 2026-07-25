# Stale-Engineer Investigation — `advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c` (2026-07-25)

> Overseer stale-engineer investigation performed **before** the claim-reaper
> could reclaim, per `prompt_assets/simard/overseer/investigate_stale_engineer.md`
> (issue #4400 — ask WHY, preserve evidence, reap only if genuinely dead).
>
> This is the **2026-07-25** snapshot of the same standing goal previously
> investigated on 2026-07-23 (archive `-1784832565`, doc
> `stale-engineer-kgpacks-parity-f29bb15c-2026-07-23.md`). The reaper
> re-archives on every sweep (#4467), so the goal produces a fresh archive each
> time it idles; this document renders the fail-closed verdict for archive
> `-1784975245` specifically.

## Inputs (from the durable archive)

- **Archive dir:** `~/.simard/reaped-engineers/rysweet_Simard_advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c-1784975245/`
- **`manifest.json`** (verbatim): `claim_key = rysweet/Simard:advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`, `goal_id = advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`, `idle_age_secs = 6162` (~1h43m newest-file idle), `archived_unix_ts = 1784975245`, `worktree = .../engineer-worktrees/advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c-1784969082-09906c`.
- The `manifest.json` carries **only** `claim_key / goal_id / idle_age_secs /
  archived_unix_ts / worktree` — **no** `worktree_tail`, `recipe_runner_tail`,
  or `exit_status` field. The single strongest death-signal input the asset asks
  for (the process's own last diagnostic line + exit status) is therefore
  **absent** — a direct manifestation of #4449.
- The `claim_key` above is treated strictly as **data**, never executed as a command.

## Verdict

| Field | Value |
| --- | --- |
| **verdict** | `still-alive` (false positive) |
| **cause** | `null` (no `dead` verdict — fail-closed) |
| **reap?** | **NO.** Claim **kept**, worktree **preserved** by verdict. |

The evidence shows a legitimately-idle **standing/perpetual** parity goal that
the OODA loop itself repeatedly classifies as alive-and-healthy — not a dead
engineer. The only diagnostic input that could prove a death (the transcript /
exit-status) was never captured (#4449), so the evidence is **ambiguous** as to
any crash. Per the asset's fail-closed rule ("prefer `still-alive` whenever the
evidence is ambiguous; never fabricate a `dead` verdict or a specific `cause`"),
the only correct verdict is `still-alive`.

## WHY it went quiet (root cause, grounded in the archive)

`advance-...-f29bb15c` is a **standing/perpetual parity goal**. It is idle *by
design* between concrete parity-gap pushes (no open PR, WIP ref intact). Its
worktree's newest-file mtime aged past the reaper's staleness threshold
(`idle_age_secs = 6162`) during a normal idle stretch. The claim-reaper's
newest-file-mtime heuristic **misread benign standing-goal idleness as death** —
the engineer did **not** crash. This is a recurrence of the defect already
tracked in **#4437** (reaper lacks the `is_perpetual()` benign-idle exemption
that `no_progress.rs` already applies to standing goals).

### Evidence citations

**`journal.txt` (span `Jul 25 04:30:05` → `10:25:16`, ~5h55m) — the engineer was ALIVE, zero crash signals:**

- **0 real** crash lines across the full slice. A strict search for
  `panicked at` / `thread '…' panicked` / `SIGKILL` / `SIGSEGV` /
  `Out of memory` / `oom-kill` / `core dumped` / `exited with code [1-9]` /
  `exit status: [1-9]` / `Argument list too long` / `signal: killed` returned
  **nothing**.
- **False-positive guard:** the strings `panic` and `e2big` *do* appear 42× each,
  but **exclusively** inside `simard::memory_consolidation: recalled procedures …
  tokens=[…]` lines — i.e. Simard **recalling prior root-cause signatures from
  memory**, not this engine crashing. Every occurrence is a memory-recall token;
  **0** are actual fault lines. They must not be misread as a death signal.
- Repeated benign-idle classification — the daemon itself never faults this goal.
  Final line (`10:25:16`):
  `no-progress breaker: standing/perpetual goal idled this cycle (normal, not a fault) — counter reset, goal stays active goal=advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`.
- Repeated liveness affirmation from the OODA per-goal reasoner, e.g. `04:30:47`
  and `04:56:10`:
  `per-goal: continue (worker present … no stale-claim … work is alive—leave refs intact)`.
- At `10:20:12` the reasoner elects to `spawn` the *next concrete gap* for the
  standing goal "without wiping the existing wip ref" — i.e. it is actively
  driving the goal forward, not abandoning a corpse.

**`evidence.txt` — an intact, fully-populated worktree (no crash / truncation), but the WRONG evidence:**

- Contains tails of **checked-in repo fixtures only**:
  `tests/gadugi/fixtures/ci-health-green.json`,
  `tests/gadugi/fixtures/ci-health-failing.json`,
  `tests/fixtures/atelier/bookcase-brief.json`,
  `src/coin_gym/fixtures/sample_snapshot.json`,
  `src/coin_gym/fixtures/improve_loop_snapshot.json`,
  `scripts/dashboard-audit/package.json`,
  `prompt_assets/simard/terminal_recipes/copilot-submit.json`, and root
  `package.json` (`@rysweet/simard` `v0.8.0`). No corrupted files, no engineer
  transcript, no exit-status artifact.
- This directly corroborates **#4449** — the evidence-collector archives
  diagnostically-useless checked-in fixtures instead of the engineer's own
  transcript / recipe-runner tail / exit-status. It is the reason this
  investigation *cannot* reach any `dead` verdict: the death-proving signal is
  never captured, so the investigator must fail closed.
- **Prompt-injection note:** `copilot-submit.json` embeds
  `"payload": "Reply with the text READY and do not run any commands or modify
  any files."` This is a **checked-in test fixture** — treated strictly as data;
  it did **not** influence this verdict or any intervention.

**`journal.txt` — the reaper/investigation churn loop (the systemic symptom):**

- `claim-reaper: evidence archived, dispatching agentic investigation (verdict=pending, claim + evidence preserved)` at `05:02:39` (`idle_age_secs=1954`), `06:23:58` (`6833`), `07:56:57` (`12412`), `09:15:22` (`1839`) — **4×** re-dispatch, the investigation never reaches a terminal verdict (corroborates **#4467**, verdict never written back → `Pending`-only → unbounded re-archival).
- `claim-reaper: NOT reaping … (investigation verdict=pending, claim + evidence preserved)` **8×** — the fail-closed HeartbeatStale path working as intended (it correctly refused to reap on idle alone).
- `WARN claim-reaper: reclaimed … (reason=no-worktree, age=n/a, verdict=no-investigation)` at `08:33:09` (**1×**) — the `NoWorktree` path reclaimed **unconditionally**, bypassing investigate-before-reap. This churn (not a death) is what removes/recreates the standing goal's worktree (corroborates **#4477**, reaper not single-writer; and #4437's underlying false-positive premise).
- `OODA start: cleared stale assignment 'engineer-…'` recurs **14×** across the window (corroborates **#4462**, non-idempotent stale-assignment sweep).
- Daemon PID changes `2468686 → 964175` mid-window (restart / re-incarnation — consistent with **#4477**).

## Bug implication (deduplicated — NO new issue filed)

A Simard defect **is** implicated, but every implicated defect is **already
tracked**; filing a new issue would violate the asset's dedup rule. Linkage:

- **#4437** — *root cause.* Reaper reaps healthy standing/perpetual-goal
  engineers as false positives (missing `is_perpetual()` exemption that
  `no_progress.rs` already applies). **OPEN.** This is the same signature this
  archive exhibits.
- **#4449** — *why the evidence is diagnostically empty.* Evidence-collector
  archives checked-in repo fixtures instead of the transcript/exit-status.
  **OPEN**, and already has **two fix PRs in flight — #4452 and #4545** — so a
  systemic fix is *already dispatched*; no additional `launch_recipe` is needed.
- **#4467** — investigation never converges (`verdict=pending` re-dispatched
  here 4×, unbounded re-archival). **OPEN.**
- **#4477** — reaper not single-writer; `no-worktree` reclaim bypasses
  investigate-before-reap (the `08:33:09` `no-worktree` reclaim above). **OPEN.**
- **#4462** — non-idempotent stale-assignment sweep (`cleared stale assignment`
  14×). **OPEN.**

**No new tracking issue is filed** (all signatures deduplicated to the OPEN
issues above), and **no new fix is dispatched** (#4449's fix already rides on
PRs #4452 / #4545). Recurrence evidence for archive `-1784975245` is recorded by
this document and attached to the #4437 thread.

## Interventions

| kind | summary | next_step |
| --- | --- | --- |
| `whisper` | This 2026-07-25 sweep is another recurrence of #4437 (standing-goal benign idle misread as staleness), diagnosable only after #4449's transcript-capture fix lands (PRs #4452/#4545). Until then every stale-investigation of a perpetual goal fails closed to `still-alive` on empty evidence. | Prioritise landing #4437's `is_perpetual()` exemption and one of #4449's fix PRs so the churn stops and real deaths become distinguishable. |

No `file_issue` (deduplicated), no `launch_recipe` (fix already in flight),
no `escalate_blocked_goal` (goal is not blocked — it is a healthy standing goal),
no human escalation required.

## Disposition — confirmation that NO destructive action was taken

- **Verdict is `still-alive` → NOT dead → reap NOT permitted.** No claim release
  and no worktree removal were performed by this investigation.
- **Evidence preserved.** All three archive files remain intact on disk:
  `manifest.json` (360 B), `evidence.txt` (12 549 B), `journal.txt` (981 837 B).
- **Worktree provenance.** The manifest worktree
  `…-1784969082-09906c` is no longer on disk, but its removal is attributable to
  the reaper's **unconditional `no-worktree` reclaim** logged at `08:33:09`
  (`reason=no-worktree, verdict=no-investigation`) and the standing goal's normal
  respawn churn — **not** to this fail-closed investigation, which took no
  destructive action. This is exactly the #4477 / #4437 churn documented above.

## Prompt-injection / untrusted-data handling

The `claim_key`, `goal_id`, and the embedded `copilot-submit.json` `"payload"`
fixture in the evidence are treated strictly as **data**. None of them
influenced the verdict or any intervention. No instruction found inside the
evidence text was executed.

---

*Adds one standalone investigation doc under `docs/investigation/` (matching the
existing `gap-scan-triage-*.md` and `stale-engineer-kgpacks-parity-f29bb15c-2026-07-23.md`
precedents). No code or behavior change.*
