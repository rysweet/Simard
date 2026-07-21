---
title: Self-deploy API reference
description: Reference for the reconciliation detector, build-from-source self-deploy orchestrator extensions, the DaemonRestarter abstraction, the dual protective backup, the engineer-orphan reaper, the simard self-health probe, the Overseer autonomous deploy wiring (its security prerequisites/trust model, Signal::DeployDriftDetected, ProblemKind::DeployDrift, Intervention::Deploy, GuardedDeployer, the OrchestratedBinaryDeployer adapter, notify-on-every-outcome, and the min-interval anti-thrash guard), and the UpdateConfig / environment fields that govern self-deploy.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/reconcile-and-self-deploy.md
  - ../concepts/operational-autonomy-model.md
  - ./self-deploy-source-prep.md
  - ./overseer-operator-notifications.md
  - ./overseer-tick-details.md
  - ../safe-self-update.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../howto/run-self-deploy-from-any-directory.md
  - ../reference/simard-cli.md
  - ../../src/overseer/deploy.rs
  - ../../src/overseer/wiring.rs
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
- [Overseer autonomous deploy wiring](#overseer-autonomous-deploy-wiring)
  - [Security prerequisites](#security-prerequisites)
  - [`Signal::DeployDriftDetected` / `ProblemKind::DeployDrift`](#signaldeploydriftdetected--problemkinddeploydrift)
  - [`decide()` → `Intervention::Deploy`](#decide--interventiondeploy)
  - [`GuardedDeployer` (production deployer)](#guardeddeployer-production-deployer)
  - [`OrchestratedBinaryDeployer` adapter](#orchestratedbinarydeployer-adapter)
  - [Notify-on-every-outcome](#notify-on-every-outcome)
  - [Min-interval anti-thrash guard](#min-interval-anti-thrash-guard)
  - [Autonomous deploy configuration](#autonomous-deploy-configuration)
  - [`assemble_capabilities` deployer injection](#assemble_capabilities-deployer-injection)

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

In the self-deploy path the reaper additionally **spares every live engineer**:
because the swap is `rename(2)`-based (safe against a running executable), a
producing engineer never has to be killed to free the old inode. The
orchestrator reaps only stale entries whose process is already gone; live
producers were already checkpointed and requeued by the drain (see below).

## Drain: checkpoint + requeue (never kill, never timeout)

```rust
/// One engineer observed in flight at drain time.
pub struct InFlightEngineer { pub goal_id: String, pub worktree: PathBuf, pub pid: Option<i32> }

/// Effect the drain uses to checkpoint + requeue in-flight engineers.
/// Implementations MUST NOT kill or signal any process.
pub trait EngineerRequeue {
    fn in_flight(&self) -> Vec<InFlightEngineer>;
    fn requeue(&self, engineer: &InFlightEngineer) -> Result<(), SafeUpdateError>;
}

/// Mark draining, then checkpoint + requeue every in-flight engineer's goal.
/// Never waits on a wall-clock timeout, never fails because engineers remain,
/// never kills a producing engineer.
pub fn drain_by_requeue<R: EngineerRequeue>(
    state_dir: &Path,
    requeue: &R,
) -> Result<DrainOutcome, SafeUpdateError>;
```

The production `EngineerRequeue` (`self_deploy::ProdEngineerRequeue`) enumerates
the live engineer set from the worktree claim sentinels
(`engineer_worktree::live_claimed_engineers`). It **leaves a still-live
engineer's claim sentinel (`.simard-engineer-claim`) intact**: the liveness-based
dedup (`find_live_engineer_for_goal`) keeps the goal leased to that producing
engineer, which finishes its PR on the old inode after the swap — so the
restarted binary does **not** duplicate the goal. Only a **dead or missing**
claim is released, freeing that goal for re-pickup. The goal record and the
engineer's `SessionCheckpoint` persist regardless. `DrainTimeout` is retained in
`SafeUpdateError` for backward compatibility but is **no longer produced**.

### Reopening dispatch (`draining.flag` lifecycle)

`drain_by_requeue` sets `draining.flag` (in `default_state_dir()`) so the
dispatch gate refuses *new* engineers during the swap window. That flag is
always reopened afterward:

- **Success / restart path:** the incoming binary clears a stale `draining.flag`
  at boot (`run_ooda_daemon`) **unless** an `ExecHandover` upgrade is in flight
  (the classic safe-update validate rail owns the flag in that case and clears
  it itself).
- **Any post-drain abort:** the orchestrator calls `undrain()` before returning,
  reopening dispatch on the old binary that keeps serving.

Because the systemd unit sets `KillMode=process`, a `systemctl restart` signals
only the daemon process — the spared engineer children survive the restart and
finish on the old inode.

## `SelfDeployOrchestrator`

```rust
/// Drives the load-bearing self-deploy sequence. Extends
/// `SafeUpdateOrchestrator` with build-from-source, the dual backup, the orphan
/// reaper, the injected restarter, and the health-check/rollback tail.
pub struct SelfDeployOrchestrator {
    config: UpdateConfig,
    restarter: Box<dyn DaemonRestarter>,
    target_commit: String,
    install_path: PathBuf,
    /// `None` → legacy `build_canary` from the cwd checkout (unchanged).
    /// `Some` → fetch + checkout the merged head, then build it into the warm
    /// target dir. See self-deploy-source-prep.md.
    build_source: Option<Box<dyn SelfDeploySourcePreparer>>,
}

impl SelfDeployOrchestrator {
    /// Unchanged. `build_source = None` (legacy cwd build).
    pub fn new(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
    ) -> Self;

    /// Opt into the autonomous path: `build_candidate` (step 1) prepares the
    /// merged head via the source preparer and builds it into the warm target
    /// dir, so the deploy works from any cwd. Additive — see
    /// self-deploy-source-prep.md.
    pub fn with_source(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
        source: Box<dyn SelfDeploySourcePreparer>,
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

When wired with `with_source`, **step 1** (`build_candidate`) fetches and checks
out the merged commit in a cwd-independent repo and builds it into a persistent
warm target dir — so `simard self-deploy` works from any directory and is fast
on repeat runs. The remaining steps and the rollback tail are unchanged; only
the build *source* and *target dir* differ. See the
[self-deploy source-prep reference](./self-deploy-source-prep.md) for the
`SelfDeploySourcePreparer` trait, the warm-dir path helpers, and the security
model.

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

### Source & warm-dir environment

The cwd-independent build source and the warm target directory are governed by
environment, not `UpdateConfig`:

| Variable | Effect | Default |
| --- | --- | --- |
| `SIMARD_SELF_DEPLOY_REPO` | Absolute path to an existing git work-tree to build from, bypassing the managed clone. | resolve via precedence (env → `~/.simard/self-deploy-src/` → clone) |
| `SIMARD_STATE_ROOT` | Relocates `~/.simard/self-deploy-src/` and `~/.simard/self-deploy-target/`. | `~/.simard` |

See the [self-deploy source-prep reference](./self-deploy-source-prep.md) for
the path helpers, resolution precedence, and security model.

## Error variants

Added to `SafeUpdateError`:

| Variant | Raised when |
| --- | --- |
| `BuildFailed { detail }` | The candidate `cargo build --release` failed. Install path untouched. |
| `SourceResolveFailed { detail }` | (autonomous path) The cwd-independent source repo could not be resolved — invalid `SIMARD_SELF_DEPLOY_REPO`, undiscoverable origin, or a failed first-time clone. Pre-sequence abort; install path untouched. |
| `FetchFailed { detail }` | (autonomous path) `git fetch origin` failed and the merged object is not cached locally. Pre-sequence abort. |
| `CheckoutFailed { detail }` | (autonomous path) SHA validation or `git checkout --detach`/clean of the merged head failed. Pre-sequence abort. |
| `GateFailed { gate, detail }` | A relaunch gate or the candidate `self-test` failed. |
| `BackupFailed { which, detail }` | The memory **or** binary protective backup failed. No swap performed. |
| `OrphanReapTimeout { pid }` | An engineer orphan survived SIGTERM + SIGKILL within the grace window. |
| `HealthCheckFailed { report }` | One or more post-deploy probes failed. Triggers rollback. |
| `RolledBack { reason }` | Health check failed and rollback restored the previous binary. |
| `RollbackFailed { detail }` | Rollback could not reach a healthy state — critical operator alert. |

Every variant carries enough context to surface loudly in logs and the
cycle report; none is swallowed.

## Overseer autonomous deploy wiring

Everything above is the **executor**. This section is the **autonomous trigger**
that connects drift to that executor through the Overseer's OODA tick, so a
merged self-change redeploys itself with no operator command. It is a thin
deterministic rail: OBSERVE surfaces drift as a signal, DECIDE maps it to a
deploy intervention, and ACT runs the existing guarded/orchestrated deploy. See
the concept doc's
[Autonomous drift-triggered deploy (now live)](../concepts/reconcile-and-self-deploy.md#autonomous-drift-triggered-deploy-now-live)
for the narrative.

> **Status: implemented.** These types live in
> [`src/overseer/`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> — the signal/problem in `signal.rs`, the observed state in `capabilities.rs`,
> the `decide()` arm in `mod.rs`, the deployer in `deploy.rs`, and the injection
> in `wiring.rs`. The `decide()` mapping and the guarded deployer (gate refusals,
> canary-fail rollback, notify-on-every-outcome, anti-thrash) are covered by
> hermetic tests with every effectful seam (git, binary swap, notifier, clock)
> faked; the QA scenario in
> [`tests/overseer_autonomous_deploy_qa.rs`](https://github.com/rysweet/Simard/blob/main/tests/overseer_autonomous_deploy_qa.rs)
> asserts a wired daemon behind `main` emits a deploy intervention with the swap
> mocked, so CI never reinstalls.

### Security prerequisites

The autonomous rail is only as trustworthy as the commit it deploys. These are
enforcement contracts the wiring depends on, documented narratively in the
concept doc's
[Security prerequisites (the trust model)](../concepts/reconcile-and-self-deploy.md#security-prerequisites-the-trust-model):

- **Root of trust = protected `origin/main`.** The only deployable commit is the
  validated `origin/<default-branch>` HEAD. That branch **must** enforce required
  reviews, required status checks, and signed-commit / verified-merge
  verification, fetched over an authenticated remote. Simard layers no second
  authorization on top of branch protection — branch protection *is* the
  authorization for an autonomous swap.
- **No unverified-`HEAD` deploy.** The `merged_head()` local-`HEAD` fallback is
  disabled on the autonomous path (see
  [`Signal::DeployDriftDetected`](#signaldeploydriftdetected--problemkinddeploydrift));
  an unresolved remote head yields no signal instead of a blind swap.
  `target_commit` is validated as 40/64-char lowercase hex before use.
- **Least-privilege, non-root install.** The swap is an atomic `rename(2)` within
  the daemon's own `~/.simard/bin/` tree and requires **no root**; the daemon
  runs as an **unprivileged, non-root service user**, so a faulty deploy can only
  affect Simard's user-owned install. `~/.simard/` is `0700`; the anti-thrash
  timestamp file is `0600`.

### `Signal::DeployDriftDetected` / `ProblemKind::DeployDrift`

At OBSERVE, when `ReconcileDetector::detect()` reports `needs_deploy`, the
Overseer records a `deploy_drift` observation and emits a first-class signal.
`target_commit` is the validated `origin/main` HEAD resolved from the
`DeploySource` **at observe time**, so DECIDE never touches git.

```rust
pub enum Signal {
    // … existing variants …
    /// The running daemon binary is behind merged `origin/main`.
    DeployDriftDetected {
        /// Validated `origin/main` HEAD (40/64-char lowercase hex SHA) to deploy to.
        target_commit: String,
        /// Commits the running binary is behind. Always `> 0` when emitted.
        behind_commits: usize,
    },
}

pub enum ProblemKind {
    // … existing variants …
    /// Classified from `DeployDriftDetected`. HIGH-RISK, `Priority::High`.
    DeployDrift,
}

/// Carried on the observed state; `Default` is `None` (no drift observed).
pub struct DeployDriftObs {
    pub target_commit: String,
    pub behind_commits: usize,
}
```

`signals_from()` emits `DeployDriftDetected` **only** when
`observed.deploy_drift.is_some()`; a git/drift error at observe time leaves it
`None`, so no signal is produced (fail-safe — never deploy on unverifiable
drift, never panic the tick). `classify_signal()` maps `DeployDriftDetected ⇒
(ProblemKind::DeployDrift, Priority::High, "deploy:drift", <summary>)`.

**No unverified-`HEAD` fallback on the autonomous path.**
`GitDeploySource::merged_head()` prefers the tracked `origin/<default-branch>`
ref and falls back to a local `rev-parse HEAD` for the **operator/CLI** path
(shallow or detached checkouts). On the autonomous path that fallback is
**disabled**: if the validated remote branch head cannot be resolved, OBSERVE
produces **no `DeployDriftObs` and therefore no signal**, rather than emitting a
`target_commit` derived from an unverified local `HEAD`. Combined with the
40/64-char lowercase-hex validation of `target_commit`, the daemon never
autonomously deploys a commit it could not confirm is the protected remote
branch head. See [Security prerequisites](#security-prerequisites).

### `decide()` → `Intervention::Deploy`

The deterministic `decide()` mapping is **pure** — it reads `merged_head` from
the signal, never from git:

```rust
pub enum Intervention {
    // … existing variants …
    /// Deploy the daemon to `commit`. HIGH-RISK; go/no-go stays in the deployer.
    Deploy { commit: String },
}

// In decide(problem):
//   ProblemKind::DeployDrift with a DeployDriftDetected signal
//     => Intervention::Deploy { commit: target_commit }
//   no drift signal
//     => Report (no Deploy)
```

`decide()` decides only *that* a deploy is warranted. The **safety** decision
(no-op / rollback / red canary / crash-loop / throttle) is enforced later, by the
`GuardedDeployer` and the AutonomyGate — never in `decide()`.

### `GuardedDeployer` (production deployer)

`GuardedDeployer` implements the `Deployer` trait injected into the Overseer's
capabilities. It is the outer safety rail; it delegates the actual swap to the
tested orchestrator via [`OrchestratedBinaryDeployer`](#orchestratedbinarydeployer-adapter).

```rust
pub trait Deployer: Send + Sync {
    /// Deploy the daemon to `commit`. Returns the outcome or an error. MUST
    /// notify the operator on every terminal outcome before returning.
    fn deploy(&self, commit: &str) -> Result<DeployOutcome, OverseerError>;
}

pub struct GuardedDeployer {
    canary: Box<dyn CanaryRunner>,         // production: ProdCanaryRunner
    deployer: Box<dyn BinaryDeployer>,     // production: OrchestratedBinaryDeployer
    ancestry: Box<dyn AncestryOracle>,     // production: GitAncestryOracle
    notifier: DualChannelNotifier,         // Signal + email, from_env()
    running_commit: String,
    recent_restart_churn: u64,
    repo: String,                          // owner/name, for notification labels
}

/// Wire a production deployer from live parts: `ProdCanaryRunner` (the real
/// self-relaunch canary), `OrchestratedBinaryDeployer` (the same
/// `SelfDeployOrchestrator` swap as `simard self-deploy`), a `GitAncestryOracle`
/// rooted at the daemon repo, and a `DualChannelNotifier::from_env()`.
///
/// The min-interval anti-thrash guard is **not** a field here — it is applied
/// upstream at the Overseer's observe rail (see [Autonomous deploy
/// configuration](#autonomous-deploy-configuration)), so a throttled tick never
/// even constructs a deploy attempt.
pub fn production_guarded_deployer(
    repo_dir: PathBuf,
    recent_restart_churn: u64,
    repo: String,
) -> GuardedDeployer;

impl Deployer for GuardedDeployer {
    fn deploy(&self, commit: &str) -> Result<DeployReport, OverseerError> { /* … */ }
}
```

`deploy()` runs the gate in this fixed order — **no branch reaches a binary swap
without passing all of them** — and notifies on **every** outcome:

1. **Build + verify the canary** — `CanaryRunner::run_canary(commit)`; a build or
   verify failure is a **red** canary (`passed: false`), not a hard error, and its
   result feeds the gate. Must pass before any swap.
2. **`evaluate_deploy_gate`** — refuse **no-op** (`commit == running_commit`),
   **rollback** (`commit` is an ancestor of `running_commit`, via
   `GitAncestryOracle`), **red canary**, and **crash-loop churn**
   (`recent_restart_churn`).
3. **Swap** — delegate to `BinaryDeployer::deploy_binary(commit)` (the
   orchestrator path); on failure the orchestrator rolls back to the preserved
   prior binary (`~/.simard/bin/simard.bak.<utc-iso8601>`) and no half-swap is
   left in place.
4. **Notify** — `DualChannelNotifier` fires on success, refusal, and failure.

`evaluate_deploy_gate` returns `Ok(())` to proceed, or a typed `DeployRefusal`:

```rust
pub enum DeployRefusal {
    NoOp,                    // target == running commit
    Rollback,                // target is an ancestor of running
    RedCanary,               // one or more canary gates failed
    CrashLoop { churn: u64 },// restart churn ≥ CRASH_LOOP_CHURN_THRESHOLD
}
```

The min-interval anti-thrash guard is **not** a gate variant — it is applied
upstream at the Overseer's observe rail, so a throttled tick never constructs a
deploy attempt (see [Autonomous deploy
configuration](#autonomous-deploy-configuration)).

### `OrchestratedBinaryDeployer` adapter

The `BinaryDeployer` seam is implemented by a thin adapter that performs the swap
**byte-identically to the operator path**, by invoking
`SelfDeployOrchestrator::run()`. There is no second, divergent deploy engine.

```rust
/// Injected swap effect. Fake in tests; production is OrchestratedBinaryDeployer.
pub trait BinaryDeployer: Send + Sync {
    /// Swap to `target_commit`; returns the deployed commit.
    fn deploy_binary(&self, target_commit: &str) -> Result<String, OverseerError>;
}

/// Delegates the swap to the same orchestrator `simard self-deploy` uses:
/// canary build+verify → atomic swap → restart → orphan reap; rollback on failure.
pub struct OrchestratedBinaryDeployer;

impl BinaryDeployer for OrchestratedBinaryDeployer {
    fn deploy_binary(&self, target_commit: &str) -> Result<String, OverseerError> {
        // SelfDeployOrchestrator::with_source(...).run()
        //     maps SafeUpdateError => OverseerError::Capability
    }
}
```

The adapter only chooses the swap *effect*; canary, backup, drain, reap, and
rollback are entirely the orchestrator's — the same tested code documented in
[`SelfDeployOrchestrator`](#selfdeployorchestrator).

### Notify-on-every-outcome

The guarded deployer notifies the operator on **every** terminal outcome —
success, gate refusal, canary failure, and rollback — before returning. This is a
hardening of the prior behavior, which could return `Err` on a gate refusal
without notifying.

```rust
// Every branch of GuardedDeployer::deploy() dispatches exactly one notification
// before returning; the success path additionally debug-asserts dispatch:
let report = self.notifier.notify(&notification);
debug_assert!(report.dispatched(), "operator MUST be notified on every deploy outcome");
```

`OperatorNotification` provides one constructor for a completed deploy and one
for a refused/failed attempt; all content is DP3-sanitized to the SHA,
outcome, and a non-sensitive reason — never env values, tokens, secret paths,
or raw git stderr:

```rust
impl OperatorNotification {
    /// A completed swap (canary-verified) to `commit` from `previous`.
    pub fn deploy(commit: &str, previous: &str, repo: &str, gate_summary: &str) -> Self;
    /// A refused or failed attempt (gate refusal or binary-swap error).
    pub fn deploy_refused(target: &str, running: &str, repo: &str, reason: &str) -> Self;
}
```

Delivery uses the two-channel [`DualChannelNotifier`](./overseer-operator-notifications.md)
(Signal primary + email); an unconfigured channel is `Queued` (logged), never
dropped.

### Min-interval anti-thrash guard

A single new commit must not cause the daemon to redeploy every tick. In addition
to the `recent_restart_churn` crash-loop gate, a min-interval guard admits at
most one deploy **attempt** per window.

Because the daemon rebuilds the `Overseer` every tick, a per-instance guard would
reset each tick and could never throttle. The production guard is therefore a
**process-global** in-memory clock, applied at the observe rail before a deploy
signal is ever raised:

```rust
/// Process-global last-attempt clock (seconds since the epoch), shared across
/// every per-tick Overseer in the one daemon process. `true` records `now` and
/// admits; `false` means we are still inside the window. Recording on ALLOW (not
/// on later success) means even a refused attempt holds the window, so a
/// red-canary drift cannot re-attempt or re-notify every tick.
pub fn global_deploy_throttle_allow(now_secs: u64, min_interval_secs: u64) -> bool;

/// Resolves the window from `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS`
/// (default 900), clamped up to the MIN_DEPLOY_INTERVAL_FLOOR (60s) floor.
pub fn deploy_min_interval_secs() -> u64;
```

The process-global throttle is the single anti-thrash mechanism; a successful
deploy restarts the daemon at the new head anyway, so the clock need not persist
across restarts.

Two ticks inside the interval therefore deploy **once**: the first records the
timestamp and admits; the second is throttled (no signal, no attempt, no
notification).

The state directory `~/.simard/` is created `0700` and the timestamp file itself
is written `0600` via an atomic temp+rename, so the anti-thrash record is neither
world-readable nor forgeable by other users on the host.

### Autonomous deploy configuration

Governed by environment (read once per tick), consistent with the existing
`SIMARD_OVERSEER_*` opt-out pattern:

| Variable | Default | Effect |
| --- | --- | --- |
| `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY` | **on** (opt-out) | Falsey (`0`, `false`, `off`, `no`, case-insensitive) **pins the daemon**: the observe rail returns early, so no deploy-drift signal is raised and no autonomous swap occurs. The read-only drift signal used elsewhere is unaffected. Fail-open: an unreadable/empty value stays enabled. |
| `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS` | `900` | Minimum seconds between autonomous deploy **attempts**. Parse failure ⇒ safe default; a `MIN_DEPLOY_INTERVAL_FLOOR` (60s) floor is enforced. |

Both are resolved in `overseer/deploy_trigger.rs` (`autonomous_deploy_enabled()` /
`deploy_min_interval_secs()`). Deploy remains a HIGH-RISK action gated by the
AutonomyGate: the daemon opens it via
`build_overseer().with_high_risk_autonomy(true)`; with high-risk autonomy off the
intervention surfaces to the operator instead of executing. The opt-out is also
effectively AND'd with the master `SIMARD_OVERSEER_ENABLED` acting gate — a
disabled Overseer never ticks, so it never deploys.

### `assemble_capabilities` deployer injection

Production assembly wires the guarded deployer in place of the historical
`RefuseDeployer` stub **when autonomous deploy is enabled**. The opt-out is
enforced at two layers that stay in lock-step: the observe rail (a pinned daemon
never raises a deploy-drift signal) **and** assembly itself — when
`SIMARD_OVERSEER_AUTONOMOUS_DEPLOY` is falsey, `assemble_deployer` injects the
safe `RefuseDeployer` so no production deploy machinery (not even the
ancestry-repo resolution) is built:

```rust
// src/overseer/wiring.rs — assemble_capabilities()
// Live per-tick restart churn feeds the crash-loop gate (the daemon rebuilds the
// Overseer every tick, so this assembly-time read is fresh each tick).
let recent_restart_churn = status.snapshot().ok().and_then(|s| s.restart_churn).unwrap_or(0);
let deployer = assemble_deployer(          // gated on autonomous_deploy_enabled()
    repo_root.clone(),
    recent_restart_churn,
    overseer_self_repo(),                  // owner/name for notification labels + ancestry
);
// … Capabilities { deployer, … }
```

`production_guarded_deployer` resolves its ancestry repo with a **cheap,
filesystem-only** probe — it does **not** `git fetch` at construction (that runs
every tick via `build_overseer`; a hung fetch would stall the whole OODA loop).
Freshness of the merged target object comes from `GitDeployDriftObserver::observe`,
which fetches the same repo (throttled) earlier in the same cycle before any
deploy is planned.

`RefuseDeployer` is injected **only** on the pinned (opt-out) path (and used in
one wiring test); the enabled default carries the guarded deployer.
`recent_restart_churn` is read live at assembly time so the crash-loop gate
reflects current churn (fail-closed: unknown/high churn never bypasses the gate).

## See also

- [reconcile-and-self-deploy concept](../concepts/reconcile-and-self-deploy.md)
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the HIGH-RISK boundary governing autonomous deploy
- [Overseer operator-notification reliability](./overseer-operator-notifications.md) — the Signal+email contract fired on every deploy outcome
- [Overseer tick details](./overseer-tick-details.md) — the OODA tick the drift observe/decide/act rail rides on
- [Self-deploy source-prep reference](./self-deploy-source-prep.md)
- [How to run self-deploy from any directory](../howto/run-self-deploy-from-any-directory.md)
- [How to verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
- [Safe Self-Update](../safe-self-update.md)
