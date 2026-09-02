---
title: Triage ERROR lines in the OODA daemon log
description: >
  Operator runbook for triaging ERROR lines that appear in ~/.simard/ooda.log
  and ~/.simard/state/ooda.log. Maps the four recurring ERROR signatures — the
  legacy "DB backup failed N consecutive times" (missing-DB and No-space
  variants), the nested "recipe_runner_rs::runner ... check-and-clean failed"
  line, and the legacy python-bridge "Suite progressive failed" traceback — to
  their root cause, current status, and remediation so a naive `grep ERROR`
  telemetry alert can be triaged quickly.
last_updated: 2026-09-02
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../concepts/automated-disk-health.md
  - ../howto/configure-disk-health-check.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../howto/reclaim-disk-space-and-run-low-space-rust-builds.md
  - ../reference/disk-health-api.md
  - ../reference/backup-pruning-api.md
---

# How-to: Triage ERROR lines in the OODA daemon log

> **Audience:** operators responding to a "telemetry anomaly: `ooda.log` tail
> contains recent ERROR line(s)" alert, or anyone auditing
> `~/.simard/ooda.log` / `~/.simard/state/ooda.log`.
>
> **Prerequisites:** read access to the daemon log files on the host and the
> `grep`/`sed` basics.

## TL;DR

Not every line containing the token `ERROR` in `ooda.log` is a live daemon
fault. Several are (a) **legacy** messages emitted by a previous daemon build,
or (b) the token `ERROR` appearing **inside the body** of a daemon `WARN`
message because the daemon embedded a child process's own log output. Before
paging anyone, classify the line against the four signatures below.

Find the offending lines and their age first:

```bash
grep -n "ERROR" ~/.simard/ooda.log | tail -40
grep "ERROR" ~/.simard/ooda.log | grep -oE '^\[?[0-9]{4}-[0-9]{2}-[0-9]{2}' | sort | uniq -c
grep -n "ERROR" ~/.simard/state/ooda.log | tail -40
```

If every match predates the most recent daemon restart (see the newest
`OODA daemon:` banner near the tail of the log), you are looking at **stale**
lines retained in an append-only log — not a live incident.

## Signature 1 — legacy DB backup: "DB file does not exist"

```
[simard] ERROR: DB backup failed 3 consecutive times — last error at
/home/azureuser/.simard/backups: runtime component 'cognitive-memory' failed
to initialize: DB file does not exist, nothing to back up
```

- **Emitter:** the **old** DB-backup path (native lbug-WAL file-copy backup),
  which retried on a schedule and escalated `WARN` → `ERROR` after 3 consecutive
  failures.
- **Root cause:** the backup routine tried to open the cognitive-memory store at
  a path where **no DB file existed yet** (the store had not been initialised /
  had been migrated to a different path), so every attempt failed with "nothing
  to back up".
- **Current status: resolved by design.** The native file-copy backup was
  removed (de-fork phase 2b, issue #2307) and replaced by the **verified**
  periodic backup (issue #2420). The current daemon logs a single
  `WARN: verified backup FAILED, prune skipped: …` on failure and never emits
  the "N consecutive times … ERROR" escalation. On success it logs
  `verified backup OK: <facts> facts + <procs> procedures + <records> records`.
- **Remediation:** none for the historical lines. To confirm the live path is
  healthy:
  ```bash
  grep "verified backup OK" ~/.simard/ooda.log | tail -3
  ls -dt ~/.simard/backups/2026* | head -3   # newest verified backup dirs
  ```
  If you instead see recent `WARN: verified backup FAILED`, follow Signature 2
  (disk) and check that `~/.simard/cognitive` exists and re-opens.

## Signature 2 — legacy DB backup: "No space left on device (os error 28)"

```
[simard] ERROR: DB backup failed 3 consecutive times — last error at
/home/azureuser/.simard/backups: persistent store 'cognitive-memory' failed
during 'backup-copy-tmp': No space left on device (os error 28)
```

Typically accompanied by, in the same window:

```
[simard] OODA cycle error: bridge 'cognitive-memory-native' call to 'execute'
failed: … Cannot write to file … cognitive_memory.ladybug.wal … Error: No space
left on device
```

- **Emitter:** same legacy backup path as Signature 1.
- **Root cause: environmental — the home partition was full.** `os error 28`
  is `ENOSPC`. With no free space the WAL write, the pre-copy checkpoint, and the
  `backup-copy-tmp` all fail. This is a **disk-capacity** incident, not a logic
  bug in the backup code.
- **Impact when live:** backups stop, and the OODA cycle itself errors because
  the cognitive-memory store cannot write its WAL. This degrades memory writes
  until space is reclaimed.
- **Current status:** historical. Disk pressure is now handled proactively by
  the tiered disk-health machinery (emergency cleanup + the agentic
  `disk-health-check` recipe + disk-reclaim), and the current partition sits
  well below threshold.
- **Remediation:** check and reclaim space:
  ```bash
  df -h "$HOME"
  grep -E "disk health recipe|disk reclaim|EMERGENCY disk cleanup" \
    ~/.simard/ooda.log | tail -10
  ```
  See *Configure the Disk-Health Check* and *Reclaim Disk Space (Low-Space Rust
  Builds)* for the reclaim tooling. Do not hand-delete the DB files under
  `~/.simard/` — reclaim the `target/`, worktree, and old-backup consumers.

## Signature 3 — nested "recipe_runner_rs::runner ... check-and-clean failed"

This embedded child-`ERROR` form is historical. Current builds do not copy the
child stderr into the daemon log; a non-zero recipe exit is reported by the
separate live warning described below.

```
[simard] WARN: disk health check failed: base type 'disk-health-check' failed
during invocation: recipe exited with exit status: 1: …
[2026-07-20T00:37:13Z ERROR recipe_runner_rs::runner] Step 'check-and-clean'
failed: … agent step failed: amplihack copilot failed (exit 1)
    --- stderr (tail) ---
    amplihack: s…
```

- **This is a `WARN` at the daemon level.** The `ERROR` token belongs to the
  **child** `recipe-runner-rs` process, whose own stderr/log the older daemon
  build embedded verbatim into the body of its `WARN: disk health check failed`
  message. A naive `grep ERROR` over `ooda.log` therefore matches a line that is
  **not** a daemon error — this is the most common trigger for a false-positive
  "recent ERROR in ooda.log" telemetry alert.
- **Root cause of the underlying failure:** the disk-health-check recipe's
  single `check-and-clean` **agent step failed** — the configured agent binary
  exited `1` during that run (an agent/CLI invocation failure), so
  `recipe-runner-rs` reported the step as failed and the recipe exited
  non-zero.
- **Impact:** **minimal and self-limiting.** The disk-health check is
  best-effort and runs on its own interval
  (`SIMARD_DISK_HEALTH_INTERVAL_SECS`, default 900 seconds), independent of the
  OODA cycle; a single failed run skips one cleanup opportunity and never
  aborts the OODA cycle. The next scheduled disk-health run retries it. Confirm
  the active cadence from the `disk health interval = ...s` startup line.
  Disk reclaim also runs independently.
- **Current status:** the reworked trigger (issue #4722) records the recipe by
  **exit status alone** without scraping or embedding child stdout. Current
  builds emit one of three daemon lines: `disk health recipe: OK`,
  `WARN: disk health recipe reported failure (non-zero exit)`, or
  `WARN: disk health check failed: ...` when the recipe could not be invoked.
- **Remediation / triage:**
  ```bash
  grep -E "disk health recipe|disk health check failed" ~/.simard/ooda.log | tail -10
  ```
  If `disk health recipe: OK` dominates the recent tail, the earlier failure was
  transient — no action. A recent `disk health recipe reported failure
  (non-zero exit)` means the agent step failed and should be investigated. If
  it fails **repeatedly**, health-check the configured agent binary
  (`AMPLIHACK_AGENT_BINARY`; the recipe uses `agent: default`), confirm its
  configuration, and review the recipe under
  `prompt_assets/simard/recipes/disk-health-check.yaml`. Treat this as an
  agent-invocation problem, not as a disk fault.
  For telemetry: match daemon-emitted severity at line start (e.g. lines whose
  `[simard]` tag carries `ERROR:`) rather than a bare `ERROR` substring, so
  embedded child-process log lines do not trip the alert.

## Signature 4 — legacy python bridge: "Suite progressive failed"

Found in **`~/.simard/state/ooda.log`** (the older python-bridge log):

```
__main__ ERROR Suite progressive failed
Traceback (most recent call last):
  File ".../python/simard_gym_bridge.py", line 219, in _handle_run_suite
    result = self._progressive["run_progressive_suite"](config)
  File ".../amplihack/src/amplihack/eval/progressive_test_suite.py", line 611, …
    print(f"✗ {level.level_id} failed: {result.error_message}")
BrokenPipeError: [Errno 32] Broken pipe
```

- **Emitter:** the legacy **python** `simard_gym_bridge` / `bridge_server`
  process. Confirm that `state/ooda.log` is historical from its modification
  time before treating these lines as stale.
- **Root cause:** a **`BrokenPipeError`** — the bridge tried to `print` /
  `sys.stdout.flush()` a progressive-suite result to a stdout pipe whose reader
  (the parent that spawned the bridge) had already gone away. The suite result
  itself is incidental; the failure is the write to a closed pipe during
  teardown.
- **Impact:** limited to that one progressive-suite invocation during shutdown;
  the daemon restarted immediately afterwards (subsequent `OODA daemon: …`
  banners follow in the same log).
- **Current status:** legacy. This is the retired python-bridge code path; the
  live daemon is the Rust OODA loop writing to `~/.simard/ooda.log`. Treat any
  match in `state/ooda.log` as historical unless that file's mtime is recent.
- **Remediation:** none for the historical lines. Confirm the file is stale:
  ```bash
  ls -l ~/.simard/state/ooda.log     # check mtime — expect months old
  ```

## Decision checklist

1. **Which file?** `state/ooda.log` matches are legacy python-bridge (Signature
   4) unless the file's mtime is recent.
2. **How old?** Compare the line's timestamp to the newest `OODA daemon:` banner
   near the tail. Older ⇒ stale, retained in the append-only log.
3. **Daemon severity vs. embedded token?** A `[simard] WARN:` line that merely
   *contains* a nested `recipe_runner_rs::runner ... ERROR` (Signature 3) is a
   warning, not a daemon error.
4. **Live DB-backup health?** `grep "verified backup OK"` — recent successes
   mean Signatures 1–2 are historical.
5. **Live disk-health?** `grep -E "disk health recipe"` — recent `OK` means
   Signature 3 was transient; recent `reported failure (non-zero exit)` means
   investigate the configured agent and recipe.

Only escalate when a signature is **recent** (after the last restart) **and
recurring** across multiple cycles.
