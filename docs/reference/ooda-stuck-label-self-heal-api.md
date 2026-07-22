---
title: "`ooda-stuck` label self-heal API reference"
description: Reference for the shared label self-heal used by the OODA no-progress breaker and brain-failure safeguard filers — the `ensure_label` free function, the `LabelDisposition` (Attach / Omit) contract, the `LabelEnsureExecutor` fn-pointer test seam, and how the three `gh issue create` call sites gate their `--label ooda-stuck` argument on the disposition so a missing label self-heals (create-if-missing) or degrades (file without label + WARN) instead of failing the escalation (issue #4474).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/ooda-stuck-label-self-heal.md
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./goal-labels.md
  - ./stewardship-api.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../../src/stewardship/gh_client.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/ooda_actions/advance_goal/spawn.rs
---

# `ooda-stuck` label self-heal API reference

> **Status: implemented (issue #4474).** The label self-heal helper lives in
> [`src/stewardship/gh_client.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/gh_client.rs).
> The three consuming filers are the no-progress breaker's `GhIssueFiler` in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs)
> and the deterministic-safeguard + open-tracking-issue sites in
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs).
> Narrative: [The `ooda-stuck` escalation self-heals the missing label](../concepts/ooda-stuck-label-self-heal.md).

The escalation filers used to pass `--label ooda-stuck` unconditionally. If the
label did not exist in the target repo, `gh issue create` exited non-zero
(`could not add label: ooda-stuck not found`) and the issue was never filed —
the escalation failed silently. This reference documents the additive helper
that makes the label handling **ensure-or-degrade**, so the issue is always
filed.

## Overview

```
ensure_label(label) ──► LabelDisposition
                         ├─ Attach            (label exists / was created)
                         └─ Omit { reason }   (label un-ensurable — file without it)
```

Each caller invokes `ensure_label` before building its `gh issue create` argv,
then conditionally appends `--label <label>` only when the disposition is
`Attach`. On `Omit`, the caller files the issue without the label and emits a
structured `tracing::warn` carrying the reason.

## `LabelDisposition`

```rust
/// Outcome of ensuring a repo label exists before it is attached to an issue.
///
/// `ensure_label` never returns `Err`: escalation must always be able to file
/// the issue, so an un-ensurable label degrades to `Omit` (file without the
/// label) rather than aborting the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LabelDisposition {
    /// The label exists (it was already present, or `ensure_label` created it).
    /// Safe to pass `--label <label>` to `gh issue create`.
    Attach,
    /// The label could not be ensured (e.g. the token lacks label-create
    /// permission, or `gh` could not be spawned). File the issue WITHOUT the
    /// label and surface `reason` in a structured `warn`.
    Omit { reason: String },
}
```

`LabelDisposition` deliberately has no `Err` variant: the escalation is the
important side effect, and losing a cosmetic label must never cost the issue.

## `ensure_label`

```rust
/// Idempotently ensure `label` exists in the ambient repository, returning
/// whether it is safe to attach to an issue.
///
/// Runs `gh label create <label>` in the current working directory — the same
/// ambient repo context the sibling `gh issue create` call uses (no `-R`), so
/// the label is guaranteed to be created in the repo the issue is filed to:
/// * exit 0                          -> `Attach` (label created)
/// * stderr matches "already exists" -> `Attach` (label was already present)
/// * any other non-zero exit / spawn/wait failure -> `Omit { reason }`
///
/// Never returns `Err`; a failure to create the label degrades to `Omit` so the
/// caller can still file the escalation issue without the label.
pub(crate) fn ensure_label(label: &str) -> LabelDisposition;
```

> **Why no `repo` / `-R` argument.** The three consuming `gh issue create`
> calls all rely on ambient (cwd) repo detection and pass no `-R`. The original
> defect signature (`could not add label: ooda-stuck not found`) confirms the
> repo *was* resolved and only the label attach failed — so `ensure_label`
> mirrors that ambient context to keep the label and the issue in the same
> repo. Passing an explicit slug was rejected: none of the three call sites has
> a validated `owner/repo` in scope (the safeguard runs before
> `goal_repo_slug` is resolved; `apply_lifecycle_decision` and `GhIssueFiler`
> receive no repo), and an independently-resolved slug could diverge from the
> repo `gh issue create` targets. This also keeps the change additive — no
> repo threading and no `NoProgressIssueFiler` signature change.

Idempotency comes from the "already exists" branch: the first stall in a repo
creates the label; every later stall re-observes it and returns `Attach`
without re-creating it. The stderr match is a **case-insensitive substring**
test so it tolerates wording differences across `gh` versions.

### Invocation and safety

- Invoked via argv only — `gh`, `label`, `create`, `<label>` — with no shell
  and no `format!`-constructed command line, structurally preventing command
  injection.
- `label` is the hardcoded constant `"ooda-stuck"` (no leading `-`), and **no
  repo argument is passed**, so there is no attacker-influenceable argv value
  and nothing that can be misread as an option flag.
- Reuses ambient `gh` credentials. No tokens or secrets are introduced or
  logged.
- On an authorization failure there is **no** re-auth, privilege escalation, or
  retry storm — it degrades once to `Omit`.

## `LabelEnsureExecutor` — the test seam

Mirroring the existing `CreateIssueExecutor` fn-pointer pattern in the same
module, label creation is injected through a function pointer so unit tests can
drive every outcome without spawning `gh`.

```rust
/// Injected subprocess runner for `gh label create`, so tests can simulate
/// create / already-exists / unauthorized / spawn-failure without real `gh`.
type LabelEnsureExecutor =
    fn(&OsStr, &[&OsStr]) -> Result<Output, LabelEnsureExecutionError>;

/// Internal failure of the label-create subprocess. Folded into
/// `LabelDisposition::Omit { reason }` — never silently dropped.
#[derive(Debug)]
enum LabelEnsureExecutionError {
    Spawn(io::Error),
    Wait(io::Error),
}

/// Core logic behind `ensure_label`, parameterised on the executor for testing.
fn ensure_label_with(
    executable: &OsStr,
    executor: LabelEnsureExecutor,
    label: &str,
) -> LabelDisposition;
```

`ensure_label` is the production wrapper that binds the real
`gh label create` subprocess executor and calls `ensure_label_with`. (An
`ensure_label_reason(...)` helper folds `LabelEnsureExecutionError` into the
`Omit { reason }` string, mirroring the module's existing
`create_issue_execution_reason`.)

## The three consuming call sites

All three route label handling through `ensure_label` and build their argv as a
`Vec<&str>`, conditionally pushing `--label` / the label constant only on
`Attach`.

### 1. No-progress breaker production filer — `src/ooda_loop/no_progress.rs`

`GhIssueFiler::file_issue` (the production
[`NoProgressIssueFiler`](./no-progress-breaker-api.md)) gates on `ensure_label`
before filing. On `Attach` it files with `--label ooda-stuck`; on `Omit` it
files without the label and emits `tracing::warn` on target `simard::ooda`. Its
return contract (`Option<FiledIssue>`), issue-number parsing, and
"goal stays `Blocked` on failure" behaviour are unchanged. The filed issue is
still linked back to the goal as its `[no-progress-tracking]` artifact.

### 2. Brain-failure deterministic safeguard — `src/ooda_actions/advance_goal/spawn.rs` (~line 378)

Same `ensure_label` gate and conditional label. This site already used
`.output()`. The success-path operator `eprintln!` (~line 384) is preserved
verbatim; the `Omit` degradation logs on target `simard::ooda_brain`.

### 3. Engineer-lifecycle open-tracking-issue — `src/ooda_actions/advance_goal/spawn.rs` (~line 935)

Same `ensure_label` gate and conditional label, **plus** the call is changed
from `.status()` to `.output()`. The old `.status()` discarded captured streams
and only logged the spawn-`Err` case, so a non-zero `gh` exit produced no
diagnostics — a latent silent failure. Now the exit status is inspected and a
structured `tracing::warn` (target `simard::ooda_brain`) carries the
lossy-decoded stderr, **truncated to ≤ 2 KiB** to prevent log flooding.

## Behaviour matrix

| `gh label create` outcome | `ensure_label` returns | Filer argv | Log |
| --- | --- | --- | --- |
| Exit 0 (created) | `Attach` | `… --label ooda-stuck` | (issue-filed line, unchanged) |
| Non-zero, stderr contains "already exists" | `Attach` | `… --label ooda-stuck` | (issue-filed line, unchanged) |
| Non-zero, unauthorized / other | `Omit { reason }` | `…` (no `--label`) | `warn` with reason (per-site target) |
| Spawn/wait failure | `Omit { reason }` | `…` (no `--label`) | `warn` with reason (per-site target) |

In every row the escalation issue is filed. The only difference is whether it
carries the `ooda-stuck` label.

## What is unchanged (compatibility)

- **Escalation idempotency.** No change to the breaker's one-issue-per-stall
  dedup (`escalate_with_tracking_issue` / `already_tracked` and the
  `[no-progress-tracking]` `WipRef` link). `ensure_label` runs *before* the
  dedup decision and does not affect it. See the
  [no-progress breaker API](./no-progress-breaker-api.md).
- **`FiledIssue` / `NoProgressIssueFiler` trait.** Unchanged.
- **Per-site tracing targets and return types.** Unchanged — only the label
  concern is shared.
- **No new dependencies or binaries.** Only the already-required `gh` CLI is
  used.
- **Additive and non-breaking.** No public API signatures change; the helper and
  enum are crate-internal (`pub(crate)`).

## Testing

`ensure_label` is unit-tested by injecting `LabelEnsureExecutor` fn pointers to
simulate each path:

| Test | Injected outcome | Expected disposition |
| --- | --- | --- |
| Create-if-missing | exit 0 | `Attach` |
| Already-exists | non-zero, stderr `"label already exists"` | `Attach` |
| Unauthorized | non-zero, permission stderr | `Omit` |
| Spawn failure | `Err(io::Error)` | `Omit` |

The consuming filers extend their existing `RecordingFiler` / spawn tests to
cover the `Attach` (label attached) path, the `Omit` (label omitted, `warn`
emitted) degrade path, that dedup is unaffected, and — for the
open-tracking-issue site — that a non-zero exit is now captured and logged.

A grep-verifiable invariant holds: no new `print!`/`println!` is introduced by
the change; all new observability is structured `tracing` on existing targets.

## See also

- [Concept: the `ooda-stuck` escalation self-heals the missing label](../concepts/ooda-stuck-label-self-heal.md)
- [No-progress breaker API reference](./no-progress-breaker-api.md)
- [Goal labels / tags API reference](./goal-labels.md)
- [How to diagnose a no-progress block](../howto/diagnose-a-no-progress-block.md)
