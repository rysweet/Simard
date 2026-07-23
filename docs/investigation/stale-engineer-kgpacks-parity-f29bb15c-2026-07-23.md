# Stale-Engineer Investigation — `advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c` (2026-07-23)

> Overseer stale-engineer investigation performed **before** the claim-reaper
> could reclaim, per `prompt_assets/simard/overseer/investigate_stale_engineer.md`
> (issue #4400 — ask WHY, preserve evidence, reap only if genuinely dead).

## Inputs (from the durable archive)

- **Archive dir:** `~/.simard/reaped-engineers/rysweet_Simard_advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c-1784832565/`
- **`manifest.json`** (verbatim): `claim_key = rysweet/Simard:advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`, `goal_id = advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`, `idle_age_secs = 7371` (~2h03m newest-file idle), `archived_unix_ts = 1784832565`, `worktree = .../engineer-worktrees/advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c-1784825193-27555d`.
- The `claim_key` above is treated strictly as **data**, never executed as a command.

## Verdict

| Field | Value |
| --- | --- |
| **verdict** | `still-alive` (false positive) |
| **cause** | `null` (no `dead` verdict — fail-closed) |
| **reap?** | **NO.** Claim **kept**, worktree **preserved**. |

The evidence shows a legitimately-idle **standing/perpetual** goal with an intact
worktree and a live WIP ref — not a dead engineer. Per the asset's fail-closed
rule ("prefer `still-alive` whenever the evidence is ambiguous; never fabricate a
`dead` verdict"), the only correct verdict is `still-alive`.

## WHY it went quiet (root cause, grounded in the archive)

`advance-...-f29bb15c` is a **standing/perpetual parity goal**. It is idle *by
design* between concrete parity-gap pushes (no open PR, WIP ref intact). Its
worktree's newest-file mtime aged past the reaper's staleness threshold
(`idle_age_secs = 7371`) during a normal idle stretch. The claim-reaper's
newest-file-mtime heuristic **misread benign standing-goal idleness as death** —
it did **not** crash. This is a recurrence of the exact defect already tracked in
**#4437** (that issue was itself filed from a *prior* stale-investigation of this
same goal, archive `-1784692815`; this archive `-1784832565` is a later snapshot
of the identical pattern).

### Evidence citations

**`journal.txt` (span `Jul 23 12:52:46` → `18:47:25`, ~6h) — the engineer was ALIVE, zero crash signals:**

- **0 real** `panicked at` / `SIGKILL` / `Out of memory` / `oom-kill` / `core dumped` / `exited with code [1-9]` / `Argument list too long` lines (strict search returned nothing across the full 6h slice).
- Repeated benign-idle classification — the daemon itself never faults this goal:
  `no-progress breaker: standing/perpetual goal idled this cycle (normal, not a fault) — counter reset, goal stays active goal=advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`.
- Repeated liveness affirmation from the OODA per-goal reasoner:
  `per-goal: continue (worker present and holds 1 live WIP ref; no staleness (stale_claim_secs none, effect_board_missed false); standing goal's in-flight work looks alive so leave it rather than wipe refs)` (8+ distinct cycles).

**`evidence.txt` — an intact, fully-populated worktree (no crash / truncation):**

- Tails of checked-in repo fixtures only: `tests/gadugi/fixtures/ci-health-*.json`, `tests/fixtures/atelier/bookcase-brief.json`, `src/coin_gym/fixtures/*_snapshot.json`, `scripts/dashboard-audit/package.json`, `prompt_assets/simard/terminal_recipes/copilot-submit.json`, and root `package.json` (`@rysweet/simard` `v0.8.0`). No corrupted files, no engineer transcript, no exit-status artifact.
- **Prompt-injection note:** `copilot-submit.json` embeds `"payload": "Reply with the text READY and do not run any commands or modify any files."` This is a **checked-in test fixture** — treated strictly as data; it did not influence this verdict or any intervention.
- That the collector archived repo fixtures instead of the transcript corroborates **#4449** (evidence-collector archives checked-in repo fixtures instead of the engineer transcript/exit-status) — a diagnostic-fidelity gap.

**`journal.txt` — the reaper/investigation churn loop (the systemic symptom):**

- `claim-reaper: evidence archived, dispatching agentic investigation (verdict=pending, claim + evidence preserved)` at `13:33:27`, `15:04:38`, `17:19:30` (**3×** re-dispatch — the investigation never reaches a terminal verdict).
- `claim-reaper: NOT reaping ... (investigation verdict=pending, claim + evidence preserved)` (**7×** — fail-closed HeartbeatStale path working as intended).
- `WARN claim-reaper: reclaimed ... (reason=no-worktree, age=n/a, verdict=no-investigation)` at `14:20:32` and `16:36:41` (**2×** — the `NoWorktree` path reclaims **unconditionally**, bypassing investigate-before-reap; this, not a death, churns the standing goal).
- `OODA start: cleared stale assignment 'engineer-...-1784561556919'` recurs every cycle (corroborates **#4462**, non-idempotent stale-assignment sweep).
- Daemon PID changes `3159928 → 925201 → 2187954` across the window (restarts / concurrent incarnations — corroborates **#4477**, reaper not single-writer).

## Is a Simard bug implicated? YES — systemic, already tracked (no new issue)

Root cause is a coordination gap: the claim-reaper applies mechanical
mtime/worktree-presence liveness heuristics to a standing/perpetual goal without
inheriting the `is_perpetual()` benign-idle exemption the OODA no-progress breaker
already applies (`src/ooda_loop/no_progress.rs` `classify_standing_idle` →
`StandingIdle::BenignExempt`). Three coupled, already-open issues fully cover it:

| Issue | Defect | This archive's confirming evidence |
| --- | --- | --- |
| **#4437** | Reaper false-positive-reaps healthy standing/perpetual engineers (missing `is_perpetual()` exemption) — **canonical, same goal** | benign-idle classifications; intact worktree; 0 crash signals |
| **#4467** | HeartbeatStale investigation never converges → `Pending`-only, unbounded re-archival | 3× `verdict=pending` re-dispatch; 7× `NOT reaping` |
| **#4477** | `NoWorktree` reclaim overrides in-flight investigate-before-reap (not single-writer) | 2× `reason=no-worktree, verdict=no-investigation`; PID churn |

**Deduplication decision:** do **not** file a new issue. This is a recurrence of
**#4437**; the fix location is already pinpointed there and it is queued under
`workflow:default`. The correct action is to attach this archive's line-cited
recurrence evidence to **#4437** and cross-link **#4467** / **#4477**. The
systemic fix is therefore already **dispatched** (tracked, located, queued).

## Interventions (surfaced regardless of verdict)

- **`file_issue` (dedup):** recurrence comment on **#4437** with this archive's
  citations; cross-link **#4467**, **#4477**, **#4449**, **#4462**. No new issue.
- **`whisper`:** advise the next OODA cycle to suppress reaper `heartbeat-stale`
  **and** `no-worktree` reclaim for `is_perpetual()` goals until #4437/#4477 land,
  so this exact loop stops churning.
- No `launch_recipe` — the fix is already queued via #4437; a competing PR would
  duplicate in-flight work.

## Fail-closed rationale for keeping the claim + worktree

The only death signal is a file-mtime idle age of `7371s`, which the asset
explicitly states "is NOT proof of death," and which is *expected* for a standing
goal. Against it stand: zero crash signals in 6h, an intact worktree, a live WIP
ref, and the daemon's own repeated "normal, not a fault / work looks alive"
classification. The evidence is at best ambiguous and at worst clearly alive →
**verdict `still-alive`, no reap**. Reaping would require a completed,
dead-concluding investigation, which does not exist. The current environment
state — **claim `rysweet/Simard:advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c`
still held, worktree `-1784825193-27555d` still present** — is exactly correct and
was left unchanged by this investigation.
