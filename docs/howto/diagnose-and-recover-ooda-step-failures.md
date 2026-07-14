---
title: Diagnose and recover OODA step failures
description: >
  Operator runbook for the OODA decision-cycle / engineer / terminal-shell step
  failures fixed in #2640 — recognise the exit-126 / E2BIG "Argument list too long"
  signature, confirm the argv-free invocation is in effect, read the structured
  FailureDiagnosis and the corrective Signal the loop now raises, and tune the
  self-diagnose configuration.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../concepts/self-diagnose-on-step-error.md
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../howto/diagnose-decide-orient-parse-failures.md
  - ../howto/watch-overseer-activity.md
---

# How-to: Diagnose and recover OODA step failures

> **Audience:** operators on call when an OODA goal fails a decision-cycle,
> engineer, or terminal-shell step and makes no progress.
>
> **Prerequisites:** access to the daemon's log stream (journald for the systemd
> unit, or wherever you redirect its stderr) and read access to
> `~/.simard/overseer/activity.json` on the daemon host; familiarity with the
> `simard` CLI and `jq`.

Since [#2640](https://github.com/rysweet/Simard/issues/2640), two things changed:
the copilot launch sites no longer inline the prompt into `argv` (so the
exit-126 / `E2BIG` defect can't recur), and a caught step failure is **diagnosed
and steered**, not just logged. This runbook shows how to confirm both.

## 1. Recognise the exit-126 / E2BIG signature

The historical failure looked like:

```
base type 'decision cycle-copilot' failed …
terminal-shell session exited with status exit status: 126 …
(last terminal output: bash: /home/azureuser/.local/bin/amplihack: Argument list too long)
```

The tell is the pairing: **exit 126** together with **`Argument list too long`**.
That is `E2BIG` — the exec failed because `argv` exceeded `ARG_MAX`. A bare exit
126 *without* that transcript line means something else ("found but not
executable"); do not confuse the two. See
[the incident narrative](../concepts/self-diagnose-on-step-error.md#the-incident).

## 2. Confirm the argv-free invocation is in effect

The fix is that the prompt is piped on **stdin**, not inlined via `-p "$(cat …)"`.
Verify the built invocation no longer inlines the prompt:

```bash
# The corrective, current grammar pipes the prompt in:
#   cat '/tmp/…' | amplihack copilot --subprocess-safe --allow-all-tools ; exit
# There must be NO `-p "$(cat` in any copilot launch site.
grep -rn '\-p "\$(cat' src/base_type_copilot/mod.rs src/ooda_actions/session.rs || echo "OK: no argv inlining"
```

`OK: no argv inlining` confirms all three sites (meeting, builder, OODA launch)
use the argv-free transport. For the exact per-site grammar and the preserved
flags, see the [argv-free invocation reference](../reference/argv-free-copilot-invocation.md).

## 3. Read the structured diagnosis

A caught step failure now produces a
[`FailureDiagnosis`](../reference/terminal-failure-diagnosis-api.md#failurediagnosis)
with a typed cause, recorded to the Overseer's failure sink instead of being
logged and dropped. For an `E2BIG` failure the cause is
`FailureCause::ArgListTooLong`.

The diagnosis is surfaced on two existing surfaces — **not** in
`~/.simard/cycle_reports/*.json` (that file is the engineer loop's
observation/plan record and carries no Overseer signals). See the
[observability surface](../reference/terminal-failure-diagnosis-api.md#observability-surface).

**a) Structured diagnosis log (machine-readable).** When the diagnosis is
recorded, the Overseer emits a structured event under `target: "overseer.diagnosis"`
carrying the serialized `FailureDiagnosis` (`cause`, `exit_code`, redacted
`evidence`). The daemon writes logs to **stderr**, as JSON when you run it with
`SIMARD_LOG_JSON=1` (see [`main.rs`](https://github.com/rysweet/Simard/blob/main/src/main.rs)).
Capture that stream where your deployment sends it — journald for the systemd unit,
or a redirect such as `2>> ~/.simard/daemon.log` when you run it by hand — then
filter to the diagnosis target:

```bash
# from a JSON-log capture (SIMARD_LOG_JSON=1); e.g. a redirected stderr file
jq -c 'select(.target == "overseer.diagnosis")
        | {ts: .timestamp, cause: .fields.cause,
           exit_code: .fields.exit_code, evidence: .fields.evidence}' \
  ~/.simard/daemon.log | tail -5

# or, for the systemd unit, read the same events from journald:
#   journalctl -u simard --output=cat | jq -c 'select(.target=="overseer.diagnosis")'
```

For an `E2BIG` failure you should see `"cause":"arg_list_too_long"` with a short,
redacted `evidence` excerpt — **not** a lone free-text log line.

**b) Overseer activity feed (human-facing).** The diagnosis becomes a
`ProblemKind::ProcessHealth` problem and a corrective `LaunchRecipe`, which show up
in the Overseer activity feed's tick counts — an **observed problem** and a
**launched fix**:

```bash
# the most recent Overseer ticks, one line each
jq -r '.recent[:5][]
        | "\(.timestamp)  problems=\(.report.problems) launched=\(.report.recipes_launched)"' \
  ~/.simard/overseer/activity.json
```

A tick that observed the E2BIG failure reads like
`observed 1 problem → launched 1 fix`. The feed's plain language is intentionally
cause-agnostic — read the exact `FailureCause` from the structured log in (a). Watch
the tick on any Overseer surface (dashboard tab, TUI Overseer pane, `simard status`,
or `GET /api/overseer`) as described in
[Watch what the Overseer is doing](./watch-overseer-activity.md).

## 4. Interpret other causes

| Cause in the report | Meaning | Corrective action the loop takes |
| --- | --- | --- |
| `ArgListTooLong` | `argv` exceeded `ARG_MAX` (E2BIG, exit 126) | `LaunchRecipe` to fix the inlining |
| `CommandNotFound` | exit 127 — binary not on PATH | `LaunchRecipe` / `Escalate` (missing dep) |
| `NotExecutable` | exit 126, no E2BIG marker | check file permissions |
| `PermissionDenied` | EACCES | fix permissions / credentials |
| `OutOfMemory` | OOM-killed | resource remediation / `Escalate` |
| `DiskFull` | ENOSPC | reuse the [disk-health check](./configure-disk-health-check.md) |
| `NetworkOrAuth` | connection/TLS/401/403 | `Escalate` (needs a human/credential) |
| `Unknown` | unrecognised signature | deduped `FileIssue` for investigation |

See the full [classification matrix](../reference/terminal-failure-diagnosis-api.md#classification-matrix).

## 5. Govern the corrective behaviour

The diagnosis itself is always-on — a step failure is never silently swallowed, so
a diagnosis is always recorded and visible on the Overseer activity feed. Whether
the resulting corrective `LaunchRecipe` actually launches is governed by the
Overseer's EXISTING acting controls (there is no separate self-diagnose switch):

| Control | Effect |
| --- | --- |
| `SIMARD_OVERSEER_ENABLED` | Master gate. When the Overseer is not acting, diagnoses are still recorded/observed but no corrective workstream launches. |
| Per-cycle launch cap / budget / autonomy / conflict sequencer | Apply to a corrective `LaunchRecipe` exactly as to any other Overseer launch, so a burst of failures never fans out into a burst of workstreams. |

The diagnosis ring-buffer size (`STEP_FAILURE_SINK_CAPACITY`) is a fixed
compile-time constant, not an env knob — it only bounds the between-cycles burst
window. Set the acting gate where the daemon reads its environment (see
`/home/azureuser/.amplihack/config`) and restart the daemon to apply.

## 6. If a failure is only logged (regression check)

If you see a step failure in the logs but **no** matching `overseer.diagnosis`
event (Step 3a) and no corresponding `ProcessHealth` tick on the Overseer feed
(Step 3b), that is a regression of the #2640 contract (log-and-continue is exactly
what this change removed). Capture the daemon log and the transcript and file an
issue — do **not** just restart the daemon, because a bare restart re-selects the
same goal and reproduces the failure. The whole point of this change is that Simard
asks *why* first.
