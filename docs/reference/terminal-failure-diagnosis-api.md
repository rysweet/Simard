---
title: Terminal failure diagnosis API reference
description: >
  Reference for Simard's self-diagnosis-on-step-error seam — the structured
  `FailureDiagnosis` / `FailureCause` types, the pure `classify_terminal_failure`
  classifier (exit code + transcript), the bounded `overseer::failure_sink`,
  `Signal::StepFailureDiagnosed` and its `ProblemKind::ProcessHealth` routing to a
  corrective `Intervention`, and the G3 `self_diagnose` recipe asset that owns the
  WHY/remedy reasoning (#2640).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/self-diagnose-on-step-error.md
  - ../reference/argv-free-copilot-invocation.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../reference/overseer-goal-board-health-api.md
  - ../reference/overseer-activity-feed.md
  - ../howto/watch-overseer-activity.md
  - ../reference/terminal-session-idle-detection.md
  - ../reference/recipe-context-file-transport.md
  - ../concepts/journal-recipe-spawn-e2big.md
  - ../../src/terminal_session/failure_diagnosis.rs
  - ../../src/terminal_session/execution.rs
  - ../../src/overseer/failure_sink.rs
  - ../../src/overseer/signal.rs
  - ../../src/overseer/capabilities.rs
---

# Terminal failure diagnosis API reference

> **Status: implemented.** The classifier and types live in
> [`src/terminal_session/failure_diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/terminal_session/failure_diagnosis.rs),
> wired at the catch sites in
> [`src/terminal_session/execution.rs`](https://github.com/rysweet/Simard/blob/main/src/terminal_session/execution.rs)
> and
> [`src/ooda_actions/session.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/session.rs).
> The corrective seam is
> [`src/overseer/failure_sink.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/failure_sink.rs)
> +
> [`Signal::StepFailureDiagnosed`](https://github.com/rysweet/Simard/blob/main/src/overseer/signal.rs).
> The reasoning asset is `prompt_assets/simard/overseer/self_diagnose.md`. Closes
> [#2640](https://github.com/rysweet/Simard/issues/2640).

When an OODA decision-cycle, engineer, or terminal-shell step fails, Simard does
**not** log-and-continue. She classifies the failure into a structured root cause
and drives a corrective response through the existing Overseer loop. This page
specifies that surface. For the narrative and the operator principle it encodes,
see [Self-diagnose on step error](../concepts/self-diagnose-on-step-error.md).

## Contents

- [`FailureCause`](#failurecause)
- [`FailureDiagnosis`](#failurediagnosis)
- [`classify_terminal_failure`](#classify_terminal_failure)
- [Classification matrix](#classification-matrix)
- [Failure sink](#failure-sink)
- [`Signal::StepFailureDiagnosed`](#signalstepfailurediagnosed)
- [Module layering](#module-layering)
- [Routing to a corrective intervention](#routing-to-a-corrective-intervention)
- [Observability surface](#observability-surface)
- [Self-diagnose recipe asset (G3)](#self-diagnose-recipe-asset-g3)
- [Configuration](#configuration)
- [Security](#security)
- [Tests](#tests)

## `FailureCause`

A closed (but `#[non_exhaustive]`) enum of root-cause classes. It is derived from
the exit code **and** the transcript together — the two carry different
information, and 126 in particular is ambiguous without the transcript.

```rust
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    /// exec failed with E2BIG — argv + env exceeded ARG_MAX. Signature:
    /// exit 126 AND transcript contains "Argument list too long".
    ArgListTooLong,
    /// exit 127 — a command was not found on PATH.
    CommandNotFound,
    /// exit 126 WITHOUT the E2BIG marker — found but not executable.
    NotExecutable,
    /// "permission denied" / EACCES.
    PermissionDenied,
    /// OOM-killed (signal 9 with an OOM marker, or "Out of memory").
    OutOfMemory,
    /// ENOSPC / "No space left on device".
    DiskFull,
    /// Network or auth failure (connection refused/reset, TLS, 401/403,
    /// "could not resolve host").
    NetworkOrAuth,
    /// Nonzero/abnormal exit with no recognised signature.
    Unknown,
}
```

`ArgListTooLong` vs `NotExecutable` is the disambiguation that fixes the live
incident: exit `126` **alone** means "found but not executable", while `126`
**plus** an `Argument list too long` transcript line means `E2BIG`. The classifier
must read the transcript to tell them apart.

`FailureCause` derives `Serialize`/`Deserialize` (snake-case) so the diagnosed
cause can be embedded in the structured diagnosis log and labelled on the Overseer
activity feed — see [Observability surface](#observability-surface).

## `FailureDiagnosis`

The structured record a caught failure produces.

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureDiagnosis {
    /// The classified root cause.
    pub cause: FailureCause,
    /// The child's exit code, when one exists (`None` for signal-terminated).
    pub exit_code: Option<i32>,
    /// A short, redacted, length-bounded excerpt of the evidence that drove the
    /// classification (e.g. the offending shell diagnostic line). Never the full
    /// prompt or full transcript.
    pub evidence: String,
}
```

## `classify_terminal_failure`

A **pure** function — no I/O, no globals — so it is exhaustively unit-testable
from a hand-built `ExitStatus` + transcript string.

```rust
/// Classify a non-zero terminal step failure from its exit status and the
/// captured transcript. Reads the exit code AND scans a bounded window of the
/// transcript for well-known signatures. Linear-time literal matching only
/// (no regex, no ReDoS). Always returns a diagnosis — `FailureCause::Unknown`
/// when nothing matches.
pub fn classify_terminal_failure(
    exit_status: &std::process::ExitStatus,
    transcript: &str,
) -> FailureDiagnosis;
```

This subsumes and extends the existing human-readable `exit_code_guidance` /
`transcript_diagnostic_suffix` helpers in `execution.rs`: those still render the
operator-facing message, while `classify_terminal_failure` produces the *machine*
classification that drives the corrective loop.

## Classification matrix

| Exit code | Transcript signature (case-insensitive, bounded scan) | `FailureCause` |
| --- | --- | --- |
| 126 | `argument list too long` | `ArgListTooLong` |
| 126 | (none of the above) | `NotExecutable` |
| 127 | any / none | `CommandNotFound` |
| any | `permission denied` / `EACCES` | `PermissionDenied` |
| 137 / signal 9 | `out of memory` / `oom-kill` | `OutOfMemory` |
| any | `no space left on device` / `ENOSPC` | `DiskFull` |
| any | `connection refused`/`reset`, `could not resolve host`, `TLS`, `401`, `403` | `NetworkOrAuth` |
| nonzero | (no recognised signature) | `Unknown` |

The scan window is bounded (a fixed number of trailing bytes of the transcript) so
a hostile or runaway subprocess cannot make classification super-linear.

## Failure sink

Recorded diagnoses are buffered in a small, bounded, process-global ring buffer so
the Overseer can pick them up on its next Observe pass — instead of each failure
being written to a log line and lost.

```rust
// src/overseer/failure_sink.rs

/// Record a step failure diagnosis for the Overseer to observe. Bounded:
/// when the ring buffer is full the oldest entry is evicted (never grows
/// without bound — memory-DoS safe).
pub fn record_step_failure(diagnosis: FailureDiagnosis);

/// Drain the recent diagnoses (FIFO) for one Observe pass.
pub fn drain_recent() -> Vec<FailureDiagnosis>;
```

Implementation notes:

- Backed by a `OnceLock<Mutex<VecDeque<FailureDiagnosis>>>` with a **fixed
  compile-time capacity** (`STEP_FAILURE_SINK_CAPACITY` = 64) — a burst window,
  not a tunable knob (a cycle drains the whole buffer, so the bound only caps
  failures recorded *between* Observe passes).
- `record_step_failure` is the *only* action taken at the catch sites in place of
  the previous log-and-continue. It is non-blocking and infallible (a poisoned
  mutex degrades to a dropped record, never a panic on the hot path).
- Adding the sink deliberately avoids widening `SimardError::AdapterInvocationFailed`
  with a new field, which would have broken 100+ call sites.

## `Signal::StepFailureDiagnosed`

The Overseer's Observe pass drains the sink into `ObservedState.recent_step_failures`
and `signals_from` emits one Signal per distinct diagnosed cause:

```rust
// src/overseer/signal.rs — added variant
pub enum Signal {
    // … existing variants …
    /// A caught OODA/engineer/terminal step failure was classified to a
    /// structured root cause. From `ObservedState.recent_step_failures`.
    StepFailureDiagnosed { cause: FailureCause, detail: String },
}
```

`ObservedState` gains one field:

```rust
// src/overseer/capabilities.rs
pub struct ObservedState {
    // … existing fields …
    /// Structured step-failure diagnoses drained from `failure_sink` this
    /// Observe pass. Empty when no step failed (degrade-to-empty).
    pub recent_step_failures: Vec<FailureDiagnosis>,
}
```

`Signal` is an **internal** enum and is *not* itself serialized — operators never
`jq` a raw `Signal`. The serializable, operator-visible artifact is the
`FailureDiagnosis` (and its `FailureCause`); see
[Observability surface](#observability-surface). Keeping the Signal in-process
means the design does **not** need to add `Serialize` to `Signal`.

## Module layering

`Signal::StepFailureDiagnosed { cause: FailureCause, .. }` and
`ObservedState.recent_step_failures: Vec<FailureDiagnosis>` make `overseer` depend
on the `terminal_session::failure_diagnosis` types. This dependency is inherent to
the sink design (the Overseer drains `Vec<FailureDiagnosis>`), points in a single
direction (`overseer` → `terminal_session`, never the reverse), and stays within
the one crate — so it is accepted rather than mirrored. `failure_diagnosis` remains
a leaf module with no `overseer` imports, which is what keeps the classifier a pure,
independently testable function. If a future split extracts `overseer` into its own
crate, `FailureCause`/`FailureDiagnosis` are the two small `Serialize` types to move
to a shared crate (they carry no `overseer` types), not a reason to mirror the enum
now.

## Routing to a corrective intervention

Orient folds `StepFailureDiagnosed` into a `Problem` with
`kind = ProblemKind::ProcessHealth`, and Decide chooses a corrective
`Intervention`:

| `FailureCause` | `Problem` | Corrective `Intervention` |
| --- | --- | --- |
| `ArgListTooLong` | `ProcessHealth`, High | `LaunchRecipe` — workstream to remove the argv inlining at the offending site |
| `CommandNotFound` / `NotExecutable` | `ProcessHealth` | `LaunchRecipe` / `Escalate` (missing dependency) |
| `DiskFull` | `ResourcePressure`, Critical | reuse existing disk-health remediation |
| `OutOfMemory` | `ResourcePressure` | `LaunchRecipe` / `Escalate` |
| `NetworkOrAuth` | `ProcessHealth` | `Escalate` (needs a human/credential) |
| `Unknown` | `ProcessHealth`, Normal | `FileIssue` (deduped) so the pattern is investigated |

The `dedup_key` mirrors `stewardship::failure_signature` semantics so one recurring
cause does not spawn a duplicate workstream or duplicate issue. This reuses the
existing Signal → Problem → Intervention machinery documented in the
[Overseer goal-board health API](../reference/overseer-goal-board-health-api.md);
no new orchestration is introduced.

The `LaunchRecipe` intervention is `LaunchRecipe { brief: RecipeBrief }`
(`src/overseer/capabilities.rs`); the corrective step populates
`RecipeBrief.task_description` (with `target_repo` and an optional `sequence_group`),
it does **not** carry a bare `task_description` on the variant itself.

## Observability surface

The diagnosis is made visible on surfaces that **already serialize**, so the design
does not have to add `Serialize` to `Signal` or invent a new report file:

1. **Structured diagnosis log (scriptable).** When a diagnosis is recorded, the
   Overseer emits a structured `tracing` event under
   `target: "overseer.diagnosis"` carrying the serialized `FailureDiagnosis`
   (`cause`, `exit_code`, redacted `evidence`). The daemon logs to **stderr**, as
   JSON under `SIMARD_LOG_JSON=1` (`src/main.rs`); capture that stream (journald or
   a redirect) and filter it, e.g.
   `… | jq 'select(.target=="overseer.diagnosis") | .fields'`. This is the
   machine-readable surface that carries the exact cause.
2. **Overseer activity feed (human-facing).** The resulting
   `ProblemKind::ProcessHealth` problem and its corrective `LaunchRecipe` are
   counted in the existing `OverseerTickReport` (`problems`, `recipes_launched`)
   that drives `~/.simard/overseer/activity.json`, `GET /api/overseer`, the
   `simard status` **OVERSEER** section, and the TUI Overseer pane. The feed's
   plain language stays cause-agnostic ("observed 1 problem → launched 1 fix"), so
   read the exact `FailureCause` from the structured log above; no new feed file or
   schema field is introduced — see
   [Overseer activity feed](../reference/overseer-activity-feed.md) and
   [Watch what the Overseer is doing](../howto/watch-overseer-activity.md).

Note what this deliberately does **not** use: the persisted `ooda_loop::CycleReport`
(`~/.simard/cycle_reports/*.json`) is the engineer loop's observation/plan record and
does **not** carry Overseer signals or `recent_step_failures`; the diagnosis is not
surfaced there.

## Self-diagnose recipe asset (G3)

Per guideline **G3 (agentic over brittle heuristics)**, the Rust classifier is a
thin, deterministic **trigger**; the actual *why did this happen and what should we
do* reasoning is delegated to a prompt asset:

- **Asset:** `prompt_assets/simard/overseer/self_diagnose.md`.
- **Input placeholders (substituted via recipe `-c key=value`, sanitized):**
  `{failure_cause}`, `{exit_code}`, `{evidence}`, `{last_terminal_output}`, and
  `{step_context}`.
- **Output:** a structured `{ "root_cause": …, "remedy": …, "confidence": … }`
  block that Decide uses to shape the corrective `Intervention` (e.g. populating
  `RecipeBrief.task_description` for a `LaunchRecipe { brief }`).

This keeps the heuristic layer small and honest: the deterministic classifier only
has to recognise the well-known signatures; anything novel becomes
`FailureCause::Unknown` and is handed to the agentic diagnostic step rather than
being force-fit into a brittle rule.

## Configuration

The diagnosis path itself has **no dedicated env knobs** — it is always-on so a
step failure is never silently swallowed. What it *records* is always available on
the Overseer activity feed; whether the resulting corrective `LaunchRecipe` is
actually launched is governed by the Overseer's EXISTING acting controls, not a
new switch:

- `SIMARD_OVERSEER_ENABLED` — the master gate. When the Overseer is not acting,
  diagnoses are still recorded and surfaced, but no corrective workstream launches.
- The standard per-intervention gates in `run_cycle` — autonomy, daily budget, the
  per-cycle launch cap, and the conflict sequencer — apply to a corrective
  `LaunchRecipe` exactly as they do to any other Overseer-launched workstream, so
  a burst of step failures can never fan out into a burst of workstreams.

The sink capacity is a fixed compile-time constant (`STEP_FAILURE_SINK_CAPACITY`),
deliberately not an env knob: it only bounds the between-cycles burst window, which
a diagnostic buffer never needs tuned.

## Security

- **Untrusted input.** The transcript is subprocess output: scanned data-only with
  bounded, linear-time literal matching (no regex/ReDoS), never executed.
- **Redaction.** `FailureDiagnosis.evidence` and the `Signal` `detail` are
  truncated and redacted; the full prompt and full transcript are never logged and
  never placed in a metric name or `@mention`/`#ref` position.
- **Bounded memory.** The sink is a fixed-capacity ring buffer, so a burst of
  failures cannot exhaust memory.

## Tests

- **Classifier matrix** (`failure_diagnosis.rs` unit tests): every row of the
  [classification matrix](#classification-matrix), including the 126-with-E2BIG vs
  bare-126 disambiguation.
- **Sink bounds** (`failure_sink.rs` unit tests): capacity is enforced, oldest
  entries evict, `drain_recent` is FIFO and idempotent-empty after draining.
- **Hermetic corrective path** (`tests/overseer_self_diagnose.rs`): a simulated
  126/E2BIG step failure yields `FailureCause::ArgListTooLong`, records to the
  sink, and produces a corrective `Signal::StepFailureDiagnosed` →
  `ProblemKind::ProcessHealth` → a `LaunchRecipe`/actionable intervention — i.e.
  **more than a log line**.
