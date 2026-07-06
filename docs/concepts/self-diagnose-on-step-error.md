---
title: "Self-diagnose on step error — ask WHY, not just log"
description: >
  Why Simard's OODA decision-cycle, engineer, and terminal-shell steps no longer
  pass the prompt through argv (fixing the live exit-126 / E2BIG "Argument list too
  long" defect), and why a caught step failure now drives a structured diagnosis and
  a corrective Signal instead of a silent log line. Covers the operator principle
  ("when there is a problem, always ask WHY it occurred, not just fix/log it"), the
  two coordinated fixes, and the boundary against silent fallbacks (#2640).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../prompt-delivery.md
  - ../concepts/steerable-ooda-daemon.md
  - ../concepts/overseer-goal-board-health.md
  - ../reference/overseer-goal-board-health-api.md
  - ../../src/base_type_copilot/mod.rs
  - ../../src/ooda_actions/session.rs
  - ../../src/terminal_session/execution.rs
  - ../../src/terminal_session/failure_diagnosis.rs
  - ../../src/overseer/failure_sink.rs
  - ../../src/overseer/signal.rs
---

# Self-diagnose on step error — ask WHY, not just log

> **Status: implemented.** The argv-free invocation lives at the three copilot
> launch sites in
> [`src/base_type_copilot/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/base_type_copilot/mod.rs)
> and
> [`src/ooda_actions/session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/session.rs).
> The failure classifier lives at
> [`src/terminal_session/failure_diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/terminal_session/failure_diagnosis.rs);
> the corrective seam is
> [`src/overseer/failure_sink.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/failure_sink.rs)
> +
> [`Signal::StepFailureDiagnosed`](https://github.com/rysweet/Simard/blob/main/src/overseer/signal.rs).
> Closes [#2640](https://github.com/rysweet/Simard/issues/2640).

This is the narrative for a single live-production incident and the two coordinated
fixes that resolved it. For the wire-level contract of each fix, read the two
reference pages:

- [Argv-free Copilot/OODA invocation](../reference/argv-free-copilot-invocation.md) — PART 1.
- [Terminal failure diagnosis API](../reference/terminal-failure-diagnosis-api.md) — PART 2.

Operators diagnosing a live occurrence should go straight to the runbook:
[Diagnose and recover OODA step failures](../howto/diagnose-and-recover-ooda-step-failures.md).

## The incident

For many goals — most reproducibly the large-context `agent-kgpacks-rs`
workstream goals — Simard's OODA decision-cycle repeatedly died before doing any
work, with:

```
base type 'decision cycle-copilot' failed …
terminal-shell session exited with status exit status: 126 …
(last terminal output: bash: /home/azureuser/.local/bin/amplihack: Argument list too long)
```

Two things were wrong, and they compounded each other.

### Root cause (PART 1): the prompt travelled through `argv`

Every copilot launch site built a **shell** command that inlined the whole
prompt/objective as a command-line argument via `$(cat …)`:

```text
# meeting turn (src/base_type_copilot/mod.rs, old)
sh -c '… --allow-all-tools --session-id '\''…'\'' -p "$(cat '\''/tmp/prompt'\'')"'

# builder / OODA launch (PTY command:, old)
command: amplihack copilot --subprocess-safe -p "$(cat '/tmp/prompt')" --allow-all-tools ; exit
```

The `$(cat …)` runs at shell-parse time and **expands the entire prompt into the
process's `argv`**. `argv` + the environment share a fixed kernel budget
(`ARG_MAX`, ~2 MiB total on Linux). A goal with large accumulated OODA context
pushed the expanded prompt past that budget, so `execve` failed before the binary
ever ran. The kernel reports that as **`E2BIG`**, which the shell surfaces as
`Argument list too long` and **exit status 126**.

This is exactly the class of bug the sanctioned
[`prompt_delivery`](../prompt-delivery.md) chokepoint exists to prevent — but
these three copilot sites predated it and built their argv by hand. The fix is to
**stop routing prompt bytes through `argv`**: the prompt is now piped to the tool
on **stdin** (`cat 'PATH' | amplihack copilot … ; exit`, and a direct
`std::process::Command` + `prompt_delivery` for the non-PTY meeting site). Prompt
size no longer contributes to `ARG_MAX`, so a multi-hundred-KiB prompt can no
longer trigger `E2BIG`. See the [argv-free invocation
reference](../reference/argv-free-copilot-invocation.md) for the exact per-site
grammar.

### Root cause (PART 2): the failure was logged and abandoned

When the step died, the OODA loop **logged the error and moved on to the next
cycle**. It never asked *why* exit 126 happened, so it re-selected the same goal,
built the same oversized argv, and failed the same way — forever. A human operator
had to notice, read the transcript, recognise `Argument list too long` as `E2BIG`,
and drive the fix by hand.

That violates the operator principle this change encodes:

> **When there is a problem, always ask WHY it occurred — do not just fix or log it.**

## The two coordinated fixes

### PART 1 — argv-free invocation (stops the bleed)

All three copilot launch sites now hand the prompt over **without putting it in
`argv`**:

| Site | File | Old (argv) | New (argv-free) |
| --- | --- | --- | --- |
| Meeting turn | `base_type_copilot/mod.rs` | `sh -c '… -p "$(cat …)"'` | direct `Command` + `prompt_delivery::apply_std(cmd, Stdin)` |
| Builder | `base_type_copilot/mod.rs` | `command: … -p "$(cat 'PATH')" …` | `command: cat 'PATH' \| … --subprocess-safe --allow-all-tools ; exit` |
| OODA launch | `ooda_actions/session.rs` | `amplihack copilot -p "$(cat …)"` | `cat 'PATH' \| amplihack copilot … ; exit` |

`--subprocess-safe` and `--allow-all-tools` are preserved verbatim; only the
prompt-delivery channel changed. The `copilot` and `amplihack copilot` binaries
already read a non-TTY stdin as the prompt when `-p` is absent, so this is a
supported transport, not a workaround. The OODA site additionally moves its temp
file onto a Rust `NamedTempFile` (`0o600`, `O_EXCL`) whose cleanup is owned by
`Drop`, closing a world-readable `mktemp` window at the same time.

### PART 2 — self-diagnose and steer (asks WHY)

When one of these steps fails, Simard now **classifies the failure** before doing
anything else:

1. `classify_terminal_failure(&ExitStatus, transcript)` reads **both** the exit
   code **and** the transcript, and returns a structured
   [`FailureDiagnosis`](../reference/terminal-failure-diagnosis-api.md#failurediagnosis)
   with a typed [`FailureCause`](../reference/terminal-failure-diagnosis-api.md#failurecause) —
   e.g. exit `126` **plus** `Argument list too long` ⇒ `FailureCause::ArgListTooLong`
   (distinct from a bare `126` = `NotExecutable`).
2. The diagnosis is recorded to a bounded
   [`failure_sink`](../reference/terminal-failure-diagnosis-api.md#failure-sink)
   instead of being written to a log and dropped.
3. On its next Observe pass the Overseer folds recent diagnoses into
   [`Signal::StepFailureDiagnosed`](../reference/terminal-failure-diagnosis-api.md#signalstepfailurediagnosed),
   Orient turns that into a [`ProblemKind::ProcessHealth`](../reference/overseer-goal-board-health-api.md)
   `Problem`, and Decide chooses a corrective `Intervention` (for
   `ArgListTooLong`, a `LaunchRecipe` workstream to remove the offending argv
   inlining). The failure becomes **actionable work**, not a log line.

The *reasoning* about root cause and remedy is deliberately **not** hard-coded in
Rust. Following guideline **G3 (agentic over brittle heuristics)**, the Rust
classifier is a thin, deterministic trigger; the "why did this happen and what
should we do" step is a prompt asset
(`prompt_assets/simard/overseer/self_diagnose.md`) fed the error plus the last
terminal output. See the [self-diagnose recipe
asset](../reference/terminal-failure-diagnosis-api.md#self-diagnose-recipe-asset-g3).

## Why not just retry, or just log louder?

- **No silent fallback.** If forced stdin/temp-file delivery cannot be applied,
  the call fails with `AdapterInvocationFailed` — it never silently degrades back
  to inlining the prompt in `argv`. A regression would surface, not hide.
- **No blind retry.** Re-running the identical oversized invocation reproduces
  `E2BIG`. The corrective action changes the *cause* (argv inlining), which is why
  diagnosis must precede any remedy.
- **A log is not a fix.** The old behaviour logged the 126 and continued. The new
  behaviour turns the 126 into a typed cause and a Signal the loop acts on — the
  operator principle made mechanical.

## Boundary and non-goals

- This change touches only **how the prompt is delivered** and **what happens when
  these steps fail**. It does not broaden `--allow-all-tools`/`--subprocess-safe`,
  does not weaken `validate_command()` (which still rejects `; | & $ \``` in
  operator-supplied terminal commands), and adds no new stray `print!`/`println!`.
- The classifier input (a subprocess transcript) is untrusted: it is scanned with
  bounded, linear-time literal matching and is never executed. Diagnosis evidence
  and Signal payloads are truncated/redacted; the full prompt and full transcript
  are never logged.
- The corrective loop reuses the existing Overseer Observe→Orient→Decide seam and
  the existing `smart-orchestrator` → `default-workflow` recipe path. It invents
  no new orchestration and no "Bridge"-named component.
