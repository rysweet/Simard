---
title: Self-deploy API reference
description: Reference for the reconciliation detector, build-from-source self-deploy orchestrator extensions, the DaemonRestarter abstraction, the dual protective backup, the engineer-orphan reaper, the simard self-health probe, and the UpdateConfig fields that govern self-deploy.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/reconcile-and-self-deploy.md
  - ../safe-self-update.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../reference/simard-cli.md
  - ../../src/self_deploy/mod.rs
  - ../../src/safe_update/mod.rs
  - ../../src/self_relaunch/mod.rs
  - ../../src/memory_backup/mod.rs
---

# Self-deploy API reference

> **Status: implemented.** The types, traits, the `simard self-health`
> subcommand, the new `UpdateConfig` fields, and the new `SafeUpdateError`
> variants below live in [`src/self_deploy/`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/mod.rs)
> and [`src/safe_update/`](../safe-self-update.md). They extend the existing
> `src/safe_update/`, `src/self_relaunch/`, and `src/memory_backup/` modules.
> The orchestrator's load-bearing sequence (and its rollback tail) is covered by
> hermetic fake-effects tests; the genuinely effectful end-to-end paths (real
> build-from-source, real systemd restart) are exercised by `#[ignore]`d tests
> the operator runs against a live host.

This reference specifies the API, configuration, CLI surface, and on-disk
contracts that close the merged-but-not-running gap. For the rationale and the
end-to-end flow, see
[reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md). The pieces
here **extend** the existing [`src/safe_update/`](../safe-self-update.md) and
`src/self_relaunch/` modules.

## Contents

- [`DeployDrift`](#deploydrift)
- [`ReconcileDetector`](#reconciledetector)
- [`DaemonRestarter`](#daemonrestarter)
- [Dual protective backup](#dual-protective-backup)
- [Engineer-orphan reaper](#engineer-orphan-reaper)
- [`SelfDeployOrchestrator`](#selfdeployorchestrator)
- [`simard self-health`](#simard-self-health)
- [`UpdateConfig` self-deploy fields](#updateconfig-self-deploy-fields)
- [Error variants](#error-variants)

## `DeployDrift`

`DeployDrift` is the single value that answers "is the running daemon stale?" It
is computed once per OODA cycle and surfaced on the Orient context.

```rust
/// Deploy drift between the merged `main` tree and the running binary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeployDrift {
    /// Commits the running binary is behind `origin/main`. `0` when current.
    pub behind_commits: usize,
    /// Names of pinned deps whose merged rev differs from the running rev
    /// (e.g. `["amplihack-memory", "rustyclawd-core"]`). Empty when current.
    pub drifted_pins: Vec<String>,
    /// `behind_commits > 0 || !drifted_pins.is_empty()`.
    pub needs_deploy: bool,
}
```

`needs_deploy` is reused verbatim by the
[deploy-aware done-gate](completion-evidence-gate-api.md) as the
"deployed-and-running" evidence for self-affecting goals.

## `ReconcileDetector`

```rust
/// Computes `DeployDrift`. The `git`/`Cargo.lock` reads are injected so tests
/// run hermetically with no network and no live repo.
pub trait DeploySource: Send + Sync {
    /// Latest merged commit on the default branch of the owned repo.
    fn merged_head(&self) -> SimardResult<String>;
    /// Build commit embedded in the running binary.
    fn running_commit(&self) -> SimardResult<String>;
    /// Count of commits `running_commit..merged_head`.
    fn behind_count(&self) -> SimardResult<usize>;
    /// Pinned dep revs in the merged tree, keyed by crate name.
    fn merged_pins(&self) -> SimardResult<BTreeMap<String, String>>;
    /// Pinned dep revs compiled into the running binary, keyed by crate name.
    fn running_pins(&self) -> SimardResult<BTreeMap<String, String>>;
}

pub struct ReconcileDetector<S: DeploySource> { source: S }

impl<S: DeploySource> ReconcileDetector<S> {
    pub fn new(source: S) -> Self;
    /// Returns `DeployDrift`. Never panics; on a source error returns a
    /// `needs_deploy: false` drift and records the error (a transient git
    /// failure must not spuriously trigger a deploy).
    pub fn detect(&self) -> DeployDrift;
}
```

**Comparison rules**

- `behind_commits` counts commits in `running_commit..merged_head`. The running
  commit comes from the binary's build metadata, not from the on-disk checkout.
- `drifted_pins` lists every crate whose `merged_pins[name] != running_pins[name]`.
  Crates that pin the same repo at the same rev (`rustyclawd-core` /
  `rustyclawd-tools`) drift together.
- On any `DeploySource` error, `detect()` returns `needs_deploy: false`
  (fail-safe: never deploy on unverifiable drift).

## `DaemonRestarter`

Restart is abstracted so the recipe and tests never restart a real daemon.

```rust
pub trait DaemonRestarter: Send + Sync {
    /// Restart the daemon. Returns once the restart has been requested.
    fn restart(&self) -> SimardResult<()>;
    /// Human-readable name for logs (e.g. "systemd", "exec-handover", "fake").
    fn kind(&self) -> &'static str;
}

/// Production restarter. Prefers `systemctl --user restart simard-ooda` when the
/// unit is detected; otherwise falls back to the coordinated `exec()` handover
/// (`self_relaunch::coordinated_relaunch`).
pub struct SystemdOrExecRestarter { /* … */ }

/// Test/recipe restarter. Records the call and performs no real restart.
pub struct FakeRestarter { /* … */ }
```

The orchestrator takes `restarter: Box<dyn DaemonRestarter>` by injection.
Selecting the restarter is the **only** decision that differs between a live
operator deploy and an in-recipe dry run.

## Dual protective backup

Both backups are taken together, after build + gates pass and before any daemon
mutation. Either failure aborts the deploy.

```rust
pub struct ProtectiveBackup {
    /// Path of the cognitive-memory snapshot (via `memory_backup`).
    pub memory_snapshot: PathBuf,
    /// Path of the binary backup (`~/.simard/bin/simard.bak.<utc-iso8601>`).
    pub binary_backup: PathBuf,
}

/// Take BOTH backups. Returns `SafeUpdateError::BackupFailed` if either fails;
/// on partial success the function cleans up the partial artifact so a retry
/// starts clean.
pub fn take_protective_backup(
    mem: &dyn CognitiveMemoryOps,
    install_path: &Path,
    state_dir: &Path,
) -> Result<ProtectiveBackup, SafeUpdateError>;
```

The memory snapshot reuses `memory_backup`; the
binary backup reuses the existing safe-update `snapshot` phase. Backups are not
reinvented here — they are sequenced and made mandatory.

## Engineer-orphan reaper

```rust
/// Process matched for reaping: same executable as the daemon binary AND argv
/// contains the `engineer run` subcommand.
pub struct OrphanEngineer { pub pid: i32, pub cmdline: String }

/// Find stale engineer subprocesses still bound to `install_path`.
/// Excludes `self_pid` and `new_daemon_pid`.
pub fn find_engineer_orphans(
    install_path: &Path,
    self_pid: i32,
    new_daemon_pid: Option<i32>,
) -> SimardResult<Vec<OrphanEngineer>>;

/// SIGTERM each orphan, wait up to `grace_seconds`, then SIGKILL survivors.
/// Numeric PID only (no name-based killers, per repo shell policy). Idempotent:
/// an empty match set returns `Ok(0)`.
pub fn reap_engineer_orphans(
    orphans: &[OrphanEngineer],
    grace_seconds: u64,
) -> SimardResult<usize>;
```

Matching is conservative: **both** the executable-path equality and the
`engineer run` argv token are required, so unrelated `simard` invocations and
the incoming daemon are never killed.

## `SelfDeployOrchestrator`

```rust
/// Drives the load-bearing self-deploy sequence. Extends
/// `SafeUpdateOrchestrator` with build-from-source, the dual backup, the orphan
/// reaper, the injected restarter, and the health-check/rollback tail.
pub struct SelfDeployOrchestrator {
    config: UpdateConfig,
    restarter: Box<dyn DaemonRestarter>,
    // … memory handle, install path, target commit …
}

impl SelfDeployOrchestrator {
    pub fn new(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
    ) -> Self;

    /// Execute: build → gate → backup → drain → reap → swap → restart →
    /// health → rollback-on-failure. Idempotent and loud. Returns the outcome
    /// or the first `SafeUpdateError`. On a failed health check, performs
    /// rollback and returns `SafeUpdateError::RolledBack { reason }`.
    pub fn run(&self) -> Result<SelfDeployOutcome, SafeUpdateError>;
}

pub struct SelfDeployOutcome {
    pub backup: ProtectiveBackup,
    pub reaped_orphans: usize,
    pub health: SelfHealthReport,
    pub restarter_kind: &'static str,
}
```

## `simard self-health`

A new top-level subcommand (sibling of `self-test`) that runs the post-deploy
probes and prints a structured report. The orchestrator calls the same probe
internally.

```text
simard self-health [--json] [--pre-deploy-facts=N]

  --json               Emit the SelfHealthReport as JSON (default: human table).
  --pre-deploy-facts   Baseline fact count to compare against (the orchestrator
                       passes the count captured before the swap). When omitted,
                       the "memory intact" probe reports the live count only.

Exit code: 0 when every probe is healthy; non-zero when any probe fails.
```

### `self-health` output

```json
{
  "healthy": false,
  "probes": {
    "version_advanced": { "healthy": true,  "running": "<commit>", "target": "<commit>" },
    "memory_intact":    { "healthy": false, "live_facts": 1180, "baseline_facts": 1206 },
    "goal_board_intact":{ "healthy": true,  "active_goals": 5 },
    "brains_llm_backed":{ "healthy": true,  "fallback_records": 0 },
    "no_quarantine":    { "healthy": true,  "quarantined": false }
  }
}
```

`healthy` is the logical AND of every probe's `healthy`. A `false` from any probe
fails the health check and triggers rollback when invoked by the orchestrator.

## `UpdateConfig` self-deploy fields

These fields are **added** to the existing `UpdateConfig`
([Safe Self-Update](../safe-self-update.md#configuration)); existing fields keep
their defaults and meaning.

| Field | Default | Notes |
| --- | --- | --- |
| `deploy_source` | `BuildFromSource` | `BuildFromSource` for merged-but-unreleased `main`; `ReleaseDownload` for tagged releases. |
| `memory_backup_required` | `true` | When `true`, a failed cognitive-memory backup aborts the deploy. |
| `orphan_kill_grace_seconds` | `10` | SIGTERM→SIGKILL window for the engineer-orphan reaper. |
| `health_probe_cycles` | `1` | OODA cycles observed for the "brains LLM-backed" probe. |
| `memory_count_tolerance` | `0` | Allowed shortfall of `live_facts` below `baseline_facts` before the probe fails. |

## Error variants

Added to `SafeUpdateError`:

| Variant | Raised when |
| --- | --- |
| `BuildFailed { detail }` | The candidate `cargo build --release` failed. Install path untouched. |
| `GateFailed { gate, detail }` | A relaunch gate or the candidate `self-test` failed. |
| `BackupFailed { which, detail }` | The memory **or** binary protective backup failed. No swap performed. |
| `OrphanReapTimeout { pid }` | An engineer orphan survived SIGTERM + SIGKILL within the grace window. |
| `HealthCheckFailed { report }` | One or more post-deploy probes failed. Triggers rollback. |
| `RolledBack { reason }` | Health check failed and rollback restored the previous binary. |
| `RollbackFailed { detail }` | Rollback could not reach a healthy state — critical operator alert. |

Every variant carries enough context to surface loudly in logs and the
cycle report; none is swallowed.

## See also

- [reconcile-and-self-deploy concept](../concepts/reconcile-and-self-deploy.md)
- [How to verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
- [Safe Self-Update](../safe-self-update.md)
