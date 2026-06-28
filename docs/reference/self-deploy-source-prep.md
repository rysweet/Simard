---
title: Self-deploy source preparation & warm target dir reference
description: Reference for the cwd-independent self-deploy source preparer (SelfDeploySourcePreparer / GitSourcePreparer), the persistent warm build directories under the state root, the build_self_deploy_candidate builder, the SelfDeployOrchestrator::with_source constructor, the source-preparation UpdateConfig/env surface, the new SafeUpdateError variants, and the security model that fetches and checks out the merged head before building.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/run-self-deploy-from-any-directory.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ./state-root-resolution.md
  - ./simard-cli.md
  - ../../src/self_deploy/source_prep.rs
  - ../../src/self_deploy/orchestrator.rs
  - ../../src/self_relaunch/canary.rs
---

# Self-deploy source preparation & warm target dir reference

> **Status: implemented.** The `SelfDeploySourcePreparer` trait, the
> `GitSourcePreparer` production implementation, the `self_deploy_src_dir()` /
> `self_deploy_target_dir()` path helpers, `build_self_deploy_candidate()`, the
> `SelfDeployOrchestrator::with_source` constructor, and the
> `SourceResolveFailed` / `FetchFailed` / `CheckoutFailed` error variants live in
> [`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs),
> [`src/self_deploy/orchestrator.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/orchestrator.rs),
> and [`src/self_relaunch/canary.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/canary.rs).
> They are **additive**: `SelfDeployOrchestrator::new`, `build_canary`, and
> `RelaunchConfig::Default` keep their existing signatures and behaviour. The
> preparer's resolve→fetch→detached-checkout path, the resolution precedence,
> the SHA/transport validation, and the warm-dir helpers are covered by hermetic
> offline tests (a local fixture-repo `origin`); only the full `cargo build` and
> the live daemon restart are left to operator runs, not CI.

This reference specifies the typed surface that makes `simard self-deploy`
**autonomous** (it builds the *merged* commit, not the on-disk checkout's
`HEAD`) and **fast** (it reuses a persistent, warm build directory instead of a
cold per-run `temp_dir()`). It extends the
[self-deploy API reference](./self-deploy-api.md); for the end-to-end sequence
and rationale see [reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md).

## Why this exists

Before this change, `simard self-deploy` had two gaps that prevented it from
being a hands-free, fast deploy:

1. **Build source was the on-disk checkout's `HEAD`, not the merged commit.**
   `build_candidate` → `self_relaunch::build_canary` ran `cargo build` against
   `manifest_dir = "."` — whatever the cwd checkout happened to be on. `--check`
   correctly detected drift versus `origin/main`, but the build did **not**
   `git fetch` + checkout the merged commit first, so it only deployed the new
   code when the operator happened to run it from an already-up-to-date checkout.
2. **Cold target dir → slow build.** `canary_target_dir` was a fresh
   `temp_dir()/simard-canary-<PID>` each run, forcing a full from-scratch
   compile (~10+ min) instead of an incremental build (~2–3 min).

This reference closes both. The single guiding rule:

> **`simard self-deploy`, run from any working directory, builds and deploys the
> current merged head — never the cwd's `HEAD` — and reuses a warm, incremental
> target directory.**

## Contents

- [Path helpers](#path-helpers)
- [`SelfDeploySourcePreparer`](#selfdeploysourcepreparer)
- [`GitSourcePreparer`](#gitsourcepreparer)
- [Repo resolution precedence](#repo-resolution-precedence)
- [`build_self_deploy_candidate`](#build_self_deploy_candidate)
- [`SelfDeployOrchestrator::with_source`](#selfdeployorchestratorwith_source)
- [Configuration & environment](#configuration-environment)
- [Error variants](#error-variants)
- [Security model](#security-model)
- [Cleanup-reaper non-overlap](#cleanup-reaper-non-overlap)
- [Tests](#tests)
- [Source layout](#source-layout)

## Path helpers

Two helpers resolve the persistent self-deploy directories under the
[resolved state root](./state-root-resolution.md). Both honour
`SIMARD_STATE_ROOT`; neither creates the directory (the first writer — the
preparer's clone or `build_self_deploy_candidate` — is responsible for
`create_dir_all`).

```rust
/// Persistent clone of the Simard source used for self-deploy builds.
/// `<state_root>/self-deploy-src/` (default `~/.simard/self-deploy-src/`).
pub fn self_deploy_src_dir() -> PathBuf;

/// Persistent, warm Cargo target directory reused across self-deploy builds.
/// `<state_root>/self-deploy-target/` (default `~/.simard/self-deploy-target/`).
pub fn self_deploy_target_dir() -> PathBuf;
```

| Helper | Default path | Relocated by |
| --- | --- | --- |
| `self_deploy_src_dir()` | `~/.simard/self-deploy-src/` | `SIMARD_STATE_ROOT`, then `SIMARD_SELF_DEPLOY_REPO` override (see [precedence](#repo-resolution-precedence)) |
| `self_deploy_target_dir()` | `~/.simard/self-deploy-target/` | `SIMARD_STATE_ROOT` |

Both live under `$HOME/.simard`, **outside** `temp_dir()`, so they survive
reboots and `/tmp` reapers and stay warm between runs.

## `SelfDeploySourcePreparer`

The source-preparation step is abstracted behind a trait so the orchestrator
sequence stays hermetically testable — a fake preparer drives the
fetch/checkout-then-build wiring with no network and no real git.

```rust
/// Prepares a cwd-independent Simard source checkout at the merged head before
/// the self-deploy build. Implementations MUST fail loud — never silently fall
/// back to building the current working directory's `HEAD`.
pub trait SelfDeploySourcePreparer: Send + Sync {
    /// Make `target_commit` available on disk for building: resolve the
    /// canonical repo (never cwd), `git fetch origin`, then
    /// `git checkout --detach <target_commit>`. Returns the prepared work-tree
    /// root whose `Cargo.toml` the build will target.
    fn prepare(&self, target_commit: &str) -> Result<PathBuf, SafeUpdateError>;
}
```

**Contract**

- `prepare` is **cwd-independent**: it operates on the resolved canonical repo
  (see "Repo resolution precedence"), never on `current_dir()`.
- `prepare` is **loud on failure**: a resolve/fetch/checkout error returns a
  specific [error variant](#error-variants) and aborts the deploy *before* any
  daemon mutation. There is no cwd-`HEAD` fallback.
- `prepare` leaves the work-tree on a **detached** checkout of `target_commit`
  (no per-run branch accumulation), so `build.rs` embeds exactly that SHA.

## `GitSourcePreparer`

The production implementation. It runs `git` via the repo's hardened
`env_clear()` + `PATH`/`HOME`-only pattern (mirroring `engineer_worktree`'s git
helper), so a hostile ambient environment cannot hijack the build source.

```rust
pub struct GitSourcePreparer { /* optional explicit repo override … */ }

impl GitSourcePreparer {
    /// Production preparer: resolves the canonical repo via the documented
    /// precedence and reuses the persistent warm directories.
    pub fn new() -> Self;

    /// Preparer rooted at an explicit repo (tests / non-default installs).
    pub fn at(repo_dir: impl Into<PathBuf>) -> Self;

    /// Resolve the canonical repo via the documented precedence (env override →
    /// persistent checkout → clone-from-origin). Never returns cwd. Used by the
    /// `self-deploy` CLI to point a `GitDeploySource` at the resolved repo.
    pub fn resolve_repo(&self) -> Result<PathBuf, SafeUpdateError>;

    /// `git fetch origin` in the resolved/owned repo so its remote-tracking
    /// refs (and thus the merged head) are current. The first dedicated
    /// git-fetch helper in `src/`; shared by `prepare` and the CLI (which
    /// fetches before reading the merged head to deploy). Loud on failure
    /// (`FetchFailed`).
    pub fn fetch_origin(&self, repo: &Path) -> Result<(), SafeUpdateError>;

    /// Resolve the canonical repo via the same precedence as `resolve_repo`
    /// **minus the clone step** (env override → persistent checkout → `None`).
    /// Read-only: it never clones, so it is safe for `self-deploy --check`
    /// ("makes no changes"). `None` means no canonical source exists yet (the
    /// caller falls back to a best-effort cwd report). Used by `report_drift`.
    pub fn resolve_existing_repo(&self) -> Option<PathBuf>;
}

impl Default for GitSourcePreparer { /* == new() */ }
impl SelfDeploySourcePreparer for GitSourcePreparer { /* … */ }
```

A fake (`RecordingPreparer`) records the requested target commit and returns a
chosen outcome; the orchestrator/composition tests use it to assert the build is
wired to the **prepared SHA**, not cwd.

## Repo resolution precedence

`resolve_repo` picks the canonical Simard source with this precedence — the
first match wins:

1. **`SIMARD_SELF_DEPLOY_REPO`** — an absolute path to an existing git
   work-tree. Validated (no `..`, not a symlink, real work-tree); an invalid
   value **aborts** with `SourceResolveFailed` — it never falls back to cwd.
2. **Persistent checkout** at `self_deploy_src_dir()`
   (`~/.simard/self-deploy-src/`) when it already exists as a git work-tree.
3. **Clone-from-origin** into `self_deploy_src_dir()`. The origin URL is
   discovered from the current checkout's `git remote get-url origin`, then
   [transport-validated](#security-model) (https/ssh only) before `git clone`
   runs. The clone relies on the ambient git credential helper for auth.

The same resolved repo is used for **both** merged-SHA resolution and the build,
so the SHA that is reported, fetched, checked out, and built is always the same
one.

> **`--check` resolution (as implemented).** Both the deploy path
> (`run_self_deploy`) and `simard self-deploy --check` (`report_drift`) resolve
> the canonical repo **cwd-independently**, so the SHA `--check` reports matches
> the SHA the deploy fetches, checks out, and builds. The deploy path uses
> `resolve_repo` (env override → persistent checkout → clone) and `fetch_origin`s
> it, failing loudly if the source cannot be made available. `--check` uses
> `resolve_existing_repo` (env override → persistent checkout → `None`) and a
> **best-effort** `fetch_origin`, because it must remain read-only: it never
> clones (honoring "makes no changes") and tolerates an offline fetch (reporting
> against local tracking refs) rather than erroring. It falls back to a
> cwd-rooted `GitDeploySource::new()` only when **no** canonical source exists
> yet (e.g. before the first deploy) — a state in which the deploy itself has
> nothing canonical to build from, so there is nothing for the two to diverge
> over.

## `build_self_deploy_candidate`

A sibling of `build_canary` in `src/self_relaunch/canary.rs`. It builds the
**prepared repo** into the **persistent warm target dir** and returns the built
binary path. `build_canary` and `RelaunchConfig::Default` are unchanged.

```rust
/// Build the self-deploy candidate from a prepared, cwd-independent source
/// checkout into a persistent warm target dir. Equivalent to `build_canary`
/// but with `--manifest-path <repo>/Cargo.toml` and
/// `--target-dir <target_dir>` (a warm, reused directory), so the build
/// compiles the merged head and reuses incremental artifacts.
pub fn build_self_deploy_candidate(
    repo: &Path,        // prepared work-tree root (from SelfDeploySourcePreparer::prepare)
    target_dir: &Path,  // persistent warm dir, e.g. self_deploy_target_dir()
) -> SimardResult<PathBuf>;
```

| Aspect | `build_canary` (legacy, unchanged) | `build_self_deploy_candidate` (new) |
| --- | --- | --- |
| Manifest source | `RelaunchConfig.manifest_dir` (default `"."` = cwd) | the prepared `repo` (merged head) |
| Target dir | `RelaunchConfig.canary_target_dir` (cold `temp_dir()/…PID`) | persistent warm `target_dir` |
| Build command | `cargo build --release --target-dir <dir> --manifest-path <m>/Cargo.toml` | same flags, warm/merged inputs |
| Built binary | `<target_dir>/release/simard` | `<target_dir>/release/simard` |

The build still sets `CARGO_BUILD_JOBS` from `cargo_jobs::cargo_jobs()` and
fails loud (`SimardError`/`SafeUpdateError::BuildFailed`) if the binary is
missing after a "successful" build.

> **Load-bearing build environment.** `build.rs` derives `SIMARD_GIT_HASH`
> from a bare `git rev-parse HEAD` that resolves against the build script's
> working directory (Cargo runs it in the manifest directory). For the embedded
> SHA to equal `target_commit`, `build_self_deploy_candidate` must invoke
> `cargo build` against the prepared repo **with a sanitized environment** —
> `GIT_DIR` / `GIT_WORK_TREE` neutralized — so the build script cannot be
> redirected to a different repo's `HEAD`. This is distinct from the preparer's
> own hardened git helper (see [security model](#security-model)); the *build*
> invocation needs the same protection because the post-build integrity gate
> depends on it.

## `SelfDeployOrchestrator::with_source`

The orchestrator gains an **optional** source preparer. `new()` is preserved
byte-for-byte (no preparer → legacy `build_canary` from cwd, for backward
compatibility and the existing default tests). `with_source()` opts into the
prepare-then-warm-build path.

```rust
pub struct SelfDeployOrchestrator {
    config: UpdateConfig,
    restarter: Box<dyn DaemonRestarter>,
    target_commit: String,
    install_path: PathBuf,
    /// `None` → legacy cwd build via `build_canary` (unchanged behaviour).
    /// `Some` → fetch/checkout the merged head, then warm-build it.
    build_source: Option<Box<dyn SelfDeploySourcePreparer>>,
}

impl SelfDeployOrchestrator {
    /// Unchanged. `build_source = None`.
    pub fn new(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
    ) -> Self;

    /// New. Wires a source preparer so `build_candidate` prepares the merged
    /// head and builds it into the warm target dir.
    pub fn with_source(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
        source: Box<dyn SelfDeploySourcePreparer>,
    ) -> Self;

    pub fn run(&self) -> Result<SelfDeployOutcome, SafeUpdateError>;
}
```

> **Relation to `DeploySourceKind`.** `DeploySourceKind` (already in
> `orchestrator.rs`, default `BuildFromSource`, alternative `ReleaseDownload`)
> records the *intent* — build the candidate from merged `main` source vs.
> download a tagged release. `build_source` is the *mechanism* that fulfils the
> `BuildFromSource` intent **cwd-independently**: when present it fetches and
> checks out the merged head before building. The two are orthogonal — this
> feature adds `build_source` and does not change `DeploySourceKind` or the
> untouched `ReleaseDownload` path.

### Where it runs in the sequence

`build_candidate` remains **step 1** of the load-bearing
[`run_sequence`](./self-deploy-api.md#selfdeployorchestrator) — it runs before
any backup, drain, swap, or restart. When `build_source` is `Some`, step 1
becomes:

```text
1a. source.prepare(target_commit)   // resolve repo + git fetch origin + checkout --detach
1b. build_self_deploy_candidate(prepared_repo, self_deploy_target_dir())
```

Because preparation is step 1, **a fetch/checkout/clone failure aborts loudly
before the daemon is touched** — no dual backup, no drain, no swap has run. The
rest of the sequence (gate → dual backup → drain → orphan-reap → swap → restart
→ health-check → rollback) and the `rollback_then` tail are **unchanged**; only
the build *source* and *target dir* differ.

### Post-build integrity gate

The existing post-deploy `version_advanced` probe compares the running binary's
embedded `SIMARD_GIT_HASH` against `target_commit` via `commits_compatible`
(healthy when the two are **equal, or one is a case-insensitive prefix of the
other** — abbreviation tolerance; an empty or `"unknown"` running SHA never
matches) and rolls back otherwise. It is **not** a git-ancestry check: a
topologically newer running SHA that is not a prefix of `target_commit` would
read as *incompatible*. Because `prepare` checks out exactly `target_commit` (a
full 40-hex SHA) and the build runs in that prepared work-tree (see the
load-bearing build-environment note above), `build.rs` embeds that exact SHA, so
the running SHA equals `target_commit` and the probe stays self-consistent.

## Configuration & environment

| Variable | Purpose | Default |
| --- | --- | --- |
| `SIMARD_SELF_DEPLOY_REPO` | Absolute path to an existing git work-tree to use as the self-deploy source, bypassing the persistent clone. Validated; invalid → `SourceResolveFailed` (never cwd fallback). | unset → resolve via precedence |
| `SIMARD_STATE_ROOT` | Relocates `self_deploy_src_dir()` **and** `self_deploy_target_dir()` (and all other durable state). See [state-root-resolution](./state-root-resolution.md). | `~/.simard` |

No new `UpdateConfig` fields are required for the default behaviour; the warm
directories are derived from the resolved state root. The existing self-deploy
`UpdateConfig` fields ([self-deploy-api](./self-deploy-api.md#updateconfig-self-deploy-fields))
keep their meaning.

## Error variants

Added to [`SafeUpdateError`](./self-deploy-api.md#error-variants). All three are
raised during **step 1** (source preparation), i.e. *before* any daemon
mutation — they are pure aborts, never trigger rollback, and leave the install
path untouched.

| Variant | Raised when |
| --- | --- |
| `SourceResolveFailed { detail }` | The canonical repo could not be resolved: invalid `SIMARD_SELF_DEPLOY_REPO` (`..`, symlink, or not a work-tree), an undiscoverable origin URL, or a failed first-time clone. |
| `FetchFailed { detail }` | `git fetch origin` failed and the target object is not already cached locally — the merged head cannot be made available. |
| `CheckoutFailed { detail }` | SHA validation failed (`^[0-9a-f]{40}$`) or `git checkout --detach <sha>` failed (e.g. the merged object is not present after fetch). |

Every variant carries a `detail` string and is surfaced loudly in logs and the
cycle report; none is swallowed and none falls back to a cwd build.

## Security model

The source preparer interpolates a SHA and a remote URL into `git` argv and
writes to disk, so it applies defence-in-depth controls:

| Control | Rule |
| --- | --- |
| **SHA validation** | The target SHA is validated against `^[0-9a-f]{40}$` before any git interpolation, blocking leading-`-` option injection into `checkout`/`fetch`. |
| **Repo-path validation** | `SIMARD_SELF_DEPLOY_REPO` (and any explicit `at()` override) must be absolute, contain no `..`, not be a symlink, and resolve to a real git work-tree. Invalid → `SourceResolveFailed`, never a cwd fallback. |
| **Transport allow-list** | Only `https://`, `ssh://`, and the scp-like `git@host:path` origins are accepted. Arbitrary-command / remote-helper transports (`ext::`, `fd::`, and any `scheme::address`) are rejected before `git clone` ever runs. |
| **Hardened git execution** | Every preparer git call uses `env_clear()` and re-injects only `PATH` and `HOME`, blocking `GIT_DIR` / `GIT_WORK_TREE` / `LD_PRELOAD` hijack. Arguments are passed as an argv array — never via `sh -c`. |
| **Build-environment sanitization** | `build_self_deploy_candidate` invokes `cargo build` with `GIT_DIR` / `GIT_WORK_TREE` / `GIT_INDEX_FILE` / `GIT_COMMON_DIR` / `GIT_OBJECT_DIRECTORY` removed, so `build.rs`'s `git rev-parse HEAD` resolves the **prepared** repo's `HEAD` (`== target_commit`) and the post-build `SIMARD_GIT_HASH` integrity gate stays self-consistent. (The rest of the env — `PATH`/`HOME`/`CARGO_*`/`RUSTUP_*` — is preserved so the build runs.) |
| **Non-destructive checkout** | `prepare` leaves the work-tree on a plain `git checkout --detach <sha>` of the exact merged commit. The managed clone is only ever written by self-deploy, so it stays clean; a *dirty* user-provided `SIMARD_SELF_DEPLOY_REPO` fails loud (`CheckoutFailed`) rather than being force-wiped — the preparer never `reset --hard`/`clean`s a caller's tree. |
| **Trust boundary** | The built SHA always originates from the trusted origin default branch (`merged_head` against the resolved canonical repo), never a cwd-controlled ref. `cargo build` still runs `build.rs`/proc-macros from source; that supply-chain surface is bounded by this SHA-trust + the post-build `SIMARD_GIT_HASH` integrity gate. |

## Cleanup-reaper non-overlap

The disk reaper in `src/cmd_cleanup/disk.rs` has several scans:
`clean_simard_canaries` removes entries under a `temp_dir()` / `/tmp` base whose
names start with `simard-`, `simard-canary`, `simard-e2e`, `amplihack-`,
`amplihack_eval`, or `ia2-`; `clean_stale_cargo_targets` targets the fixed paths
`/tmp/simard-canary` and `/tmp/cargo-target`; and three `~/.simard` scans
(`rotate_simard_binary_backups` on `~/.simard/bin/simard.bak-*`,
`trim_simard_snapshots` on `~/.simard/snapshots/`, and `remove_old_corrupt_dbs`
on `~/.simard` top-level entries matching `is_corrupt_quarantine_name`, i.e. a
`.corrupt-<ts>` infix). The warm directories are named `self-deploy-src` /
`self-deploy-target` directly under `~/.simard`: they **are** within the base
`remove_old_corrupt_dbs` reads, but they are safe because **no reaper's name
filter matches `self-deploy-*`** (they do not start with `simard-`/`amplihack-`,
are not `bin`/`snapshots`, and carry no `.corrupt-` infix). A dedicated test
asserts non-overlap against **all** of these reapers so the warm dir is never
reaped out from under an incremental build even if a future reaper broadens its
`~/.simard` scan.

## Tests

All hermetic tests run offline using a **local fixture repo** (`git init` →
commit → set a local `origin/main` ref; `git fetch` from a `file://`/path remote
is offline) plus a fake preparer. `SIMARD_SELF_DEPLOY_REPO` injects the fixture.
The source-prep cases live in a **new** `src/self_deploy/tests_source_prep.rs`;
the sequence-wiring cases **augment the existing**
`src/self_deploy/tests_orchestrator.rs` (which already covers `DeploySourceKind`
and the fake-effects ordering) rather than replacing it.

| Test (in `src/self_deploy/tests_source_prep.rs` / `tests_orchestrator.rs`) | Asserts |
| --- | --- |
| fetch/checkout-then-build from an arbitrary cwd | The build is wired to the **merged SHA's checkout**, not cwd `HEAD`. |
| resolver precedence | `SIMARD_SELF_DEPLOY_REPO` → persistent checkout → clone, in order. |
| `--check` read-only resolution | `resolve_existing_repo()` returns the env override or persistent checkout cwd-independently, and **`None` (never a clone)** when no canonical source exists. |
| warm target dir | The build targets the persistent `self_deploy_target_dir()`, reused across runs — not a per-PID `temp_dir()`. |
| loud failure | A failed fetch/clone/checkout aborts with the specific variant and never builds cwd `HEAD`. |
| SHA validation | A non-40-hex or leading-`-` SHA is rejected before any git call. |
| reaper non-overlap | `self_deploy_src_dir()` / `self_deploy_target_dir()` fall under no cleanup scan base. |
| backward compatibility | `RelaunchConfig::Default`, `build_canary`, and `SelfDeployOrchestrator::new` default tests still pass. |

The genuinely effectful end-to-end path (real network fetch, real
`cargo build`, real restart) stays under `#[ignore]`d operator tests, never in
CI.

## Source layout

```
src/self_deploy/
    source_prep.rs            # NEW: SelfDeploySourcePreparer trait + GitSourcePreparer
                              #      + self_deploy_src_dir()/self_deploy_target_dir()
    orchestrator.rs           # EXISTING: adds SelfDeployOrchestrator::with_source +
                              #   build_source field; DeploySourceKind unchanged
    tests_source_prep.rs      # NEW: local-fixture-repo + fake-preparer tests
    tests_orchestrator.rs     # EXISTING: augmented with prepare→warm-build wiring tests
src/self_relaunch/
    canary.rs                 # EXISTING: adds build_self_deploy_candidate (sibling of build_canary)
src/safe_update/
    errors.rs                 # EXISTING: adds SourceResolveFailed / FetchFailed / CheckoutFailed
src/operator_cli/
    self_deploy.rs            # EXISTING: wires GitSourcePreparer via with_source
```

## See also

- [Self-deploy API reference](./self-deploy-api.md) — the core types, config, CLI, and health JSON.
- [Reconcile-and-self-deploy concept](../concepts/reconcile-and-self-deploy.md) — the end-to-end sequence and rationale.
- [How to run self-deploy from any directory](../howto/run-self-deploy-from-any-directory.md) — operator runbook.
- [State-root resolution](./state-root-resolution.md) — how `SIMARD_STATE_ROOT` relocates the warm dirs.
