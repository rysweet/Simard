---
title: Disk reclaim API
description: Reference for the src/disk_reclaim module — the ReclaimCandidate serde contract, the non-bypassable guard::vet_candidate rail and its RejectReason set, resolve_daemon_working_dirs, the exec_reclaim executor and ReclaimReport, the disk-reclaim.yaml analysis-only recipe contract, and the simard disk-reclaim CLI surface.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/agentic-disk-reclamation.md
  - ../howto/configure-disk-reclamation.md
  - ./disk-reclaim-telemetry.md
  - ./disk-health-api.md
  - ./engineer-worktree-sweep-safety.md
---

# Disk reclaim API

**Module:** `src/disk_reclaim/`

The `disk_reclaim` module reclaims disk under hard safety rails. An **untrusted
agent proposes** a candidate list (via the `disk-reclaim.yaml` recipe); a
**deterministic Rust executor disposes** — re-validating every candidate through
a non-bypassable guard immediately before deletion. The delete primitive exists
**only** inside the executor; no public path deletes without passing
`guard::vet_candidate`.

See [Agentic disk reclamation](../concepts/agentic-disk-reclamation.md) for the
design rationale and [Configure disk reclamation](../howto/configure-disk-reclamation.md)
for operator usage.

## Module layout

| File | Responsibility |
| ---- | -------------- |
| `mod.rs` | Env parsing (`reclaim_pct_from_env`, `daemon_apply_from_env`) + `run_disk_reclaim` orchestrator (recipe → parse → executor, no fallback) |
| `candidate.rs` | `ReclaimCandidate` / `CandidateKind` serde contract + `parse_candidates` marker parser |
| `guard.rs` | The non-bypassable rail: `vet_candidate` → `Verdict::{Allow, Reject}` |
| `daemon_dir.rs` | `resolve_daemon_working_dirs` — the protected daemon-directory union |
| `executor.rs` | `exec_reclaim` — largest-first, threshold-stop, TOCTOU-reasserting executor + `ReclaimReport` |
| `recipe.rs` | Invoke `disk-reclaim.yaml`, strict marker parse, no-fallback error path |
| `sandbox.rs` | Recipe-step confinement helpers + the post-run reconciliation diff (see [Recipe-step sandboxing](#recipe-step-sandboxing)) |

## Data flow

```
disk-reclaim.yaml (analysis-only agent step)
   │  emits text markers: DISK_USED_PCT=, CANDIDATES_JSON=, CANDIDATES_SCHEMA=
   ▼
recipe.rs  →  RecipeOutput JSON envelope  →  step_results[0].output
   │
   ▼
candidate.rs::parse_candidates()  →  Vec<ReclaimCandidate> + used_pct
   │
   ▼
executor.rs::exec_reclaim()   (sort largest-first)
   │  for each candidate, at the syscall boundary:
   ▼
guard.rs::vet_candidate()  →  Verdict::Allow{primitive} | Verdict::Reject{reason}
   │                                    │
   │  Allow → perform primitive         └─ Reject → skipped[] (human review)
   ▼
ReclaimReport { used_pct_before/after, bytes_freed, removed, would_remove, skipped, failures }
```

## `candidate.rs` — the proposal contract

The interchange between the agent's proposal and the Rust executor.

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReclaimCandidate {
    /// Absolute path the agent nominates for reclamation.
    pub path: PathBuf,
    /// Which reclamation primitive the agent believes applies.
    pub kind: CandidateKind,
    /// Repo the path belongs to (informational; re-derived by the guard).
    #[serde(default)]
    pub parent_repo: Option<PathBuf>,
    /// Agent's free-text rationale (sanitized before any logging).
    #[serde(default)]
    pub reason: Option<String>,
    /// Agent's size estimate in bytes (re-measured by the executor).
    #[serde(default)]
    pub est_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// A git-tracked worktree → `git worktree remove --force` (after prune).
    TrackedWorktree,
    /// An orphaned, de-registered (untracked) leftover dir → `rm -rf`.
    OrphanDir,
    /// A stale `target/` or shared cargo cache → `rm -rf`.
    StaleBuildCache,
}
```

`deny_unknown_fields` rejects any field the agent invents. Every field the agent
supplies is **advisory** — `parent_repo`, `reason`, and `est_bytes` are
re-derived or re-measured by the executor/guard and never trusted for a
safety decision.

### `parse_candidates(step_output) → Result<(Vec<ReclaimCandidate>, u8), String>`

Parses the recipe agent's text markers out of the (possibly noisy) step output:

| Marker | Required | Meaning |
| ------ | -------- | ------- |
| `DISK_USED_PCT=<0..=100>` | yes | current `%-used` the agent measured |
| `CANDIDATES_JSON=<json array>` | yes | the proposed `[ReclaimCandidate]` list |
| `CANDIDATES_SCHEMA=<version>` | optional | schema version for forward-compat |

**Parsing rules:**

- A malformed **array** (not valid JSON, not an array) → **hard error** (no
  reclamation runs). Fail-closed: garbage in, nothing deleted.
- A malformed **element** inside a valid array → that element is **skipped** and
  reported; parsing continues with the valid elements.
- Bounded: candidate count and each `est_bytes` are range-checked; `DISK_USED_PCT`
  must be `0..=100`.
- Unknown / noise lines are ignored (the agent may emit `df` output, reasoning).

## `guard.rs` — the non-bypassable rail

Every candidate passes through `vet_candidate` **immediately before deletion**.
This is the deterministic filter the agentic step cannot bypass.

```rust
pub struct GuardContext<'a> {
    pub allow_roots: &'a [PathBuf],
    pub protected: &'a ProtectedDenySet,
    pub live_probe: &'a dyn LiveProcessProbe,       // from worktree_gc::liveness
    pub wt_probe: &'a dyn TrackedWorktreeProbe,      // re-derives the merged/closed-PR
                                                     // + uncommitted/unpushed vetoes live
                                                     // (production: RealTrackedWorktreeProbe,
                                                     // which composes worktree_gc + gh)
    pub measurer: &'a dyn SizeMeasurer,              // fresh size (never the agent's est_bytes)
}
```

pub enum Verdict {
    /// Cleared all rails; execute `primitive`, expect ~`bytes` freed.
    Allow { primitive: ReclaimPrimitive, bytes: u64 },
    /// A rail refused; route to the human-review list. Never deleted.
    Reject { reason: RejectReason },
}

pub enum ReclaimPrimitive {
    GitWorktreeRemoveForce, // tracked worktree (prune first, then remove --force)
    RemoveDir,              // orphan dir / stale cache (rm -rf, allow-root reasserted)
}

pub enum RejectReason {
    ProtectedPath,          // worktrees/main or a daemon WorkingDirectory
    LiveProcess,            // referenced by a live PID (/proc/<pid>/cwd)
    UncommittedOrUnpushed,  // dirty tree or commits not in a merged/closed PR
    ActiveWorktree,         // active recipe/engineer worktree (tmux/PID)
    OutsideAllowRoot,       // not under an allow-root / symlink / canonicalize fail
    UnknownPrState,         // PR could not be positively classified merged/closed
}
```

`vet_candidate` **composes existing, already-tested primitives** rather than
reimplementing them:

1. **`maintenance::is_safe_to_delete`** — canonicalize, symlink refusal, and
   component-wise `Path::starts_with` allow-root containment ∧ ¬`ProtectedDenySet`
   (no string-prefix matching, so `Simard` cannot be confused with `Simard-evil`).
   Failure → `OutsideAllowRoot` / `ProtectedPath`.
2. **Live-PID probe** (`worktree_gc::liveness::LiveProcessProbe`, fail-closed) —
   applied to **all** kinds, not just worktrees. Any live cwd at/under the path
   → `LiveProcess`.
3. **Fresh `worktree_gc::evaluate_candidate` re-derivation** for `TrackedWorktree`
   — re-runs the merged/closed-PR + idle + uncommitted/unpushed vetoes live. A
   non-`Allow` outcome maps to `UncommittedOrUnpushed` / `ActiveWorktree` /
   `UnknownPrState`. This is where the old *merge-base-is-ancestor* misfire is
   structurally impossible: reclamation requires a **positively confirmed**
   merged/closed PR, not "merge-base is an ancestor of main."
4. **`ProtectedDenySet`** (see below) — hard deny.
5. **Re-measure size** — the freed-bytes figure comes from a fresh measurement,
   never from the agent's `est_bytes`.

Any inconclusive signal resolves to `Reject` (fail-closed).

### `allow_roots` — the reclamation scope

`GuardContext.allow_roots` is the **positive** side of the containment check: a
candidate is rejected with `OutsideAllowRoot` unless its canonicalized path is
component-wise under one of these roots. It is the reclamation *allow-list*,
dual to `ProtectedDenySet` (`is_safe_to_delete` requires *under an allow-root*
**and** *not in the deny-set*). It is **derived, not agent-supplied**, computed
once per run from the same managed-repo set the recipe inspects:

```
allow_roots =
      { <state_root>/engineer-worktrees }                 // ~/.simard engineer worktrees
    ∪ { <repo>/worktrees for repo in MANAGED_REPOS }      // Simard, amplihack-rs, amplihack-memory-lib
    ∪ { shared cargo target dirs under <state_root> }     // stale build caches
```

`MANAGED_REPOS` is the same hardcoded managed-repo list the recipe enumerates
(§`recipe.rs`); it is **not** operator-configurable free-form (operators widen
the *deny*-set via `SIMARD_GIT_PROTECTED_REPOS`, never the allow-set — widening
the delete scope from the environment would be a footgun). A candidate outside
every allow-root — anywhere in `$HOME` not under a managed worktree/cache root,
or any absolute path the agent hallucinates — is refused before any other rail
is even consulted.

### `ProtectedDenySet`

The union computed once per run and consulted by `is_safe_to_delete`:

```
ProtectedDenySet =
      { /home/azureuser/src/Simard/worktrees/main }          // hardcoded
    ∪ resolve_daemon_working_dirs(proc_root)                 // runtime-resolved
    ∪ maintenance::protected_paths()                         // bare repos, git common dirs
    ∪ split(SIMARD_GIT_PROTECTED_REPOS, ',')                 // operator-supplied
```

## `daemon_dir.rs` — protected daemon directories

### `resolve_daemon_working_dirs(proc_root: &Path) → BTreeSet<PathBuf>`

Returns the set of directories that must never be removed because a daemon runs
there (removing one crash-loops it with `status=200/CHDIR`). The union of:

- the **hardcoded** `/home/azureuser/src/Simard/worktrees/main` (always present),
- the **own process** cwd (`<proc_root>/self/cwd`),
- the **pidfile** target's cwd (`<proc_root>/<pid>/cwd`),
- a **`/proc` comm scan** for `simard-ooda` processes, resolving each
  `<proc_root>/<pid>/cwd`,
- the service file's `WorkingDirectory=` (from `simard-ooda.service`).

`proc_root` is injectable so tests can point it at a fabricated `/proc` tree
(hermetic, no real process inspection). Unreadable entries are skipped; the
hardcoded `main` guarantees the set is never empty.

## `executor.rs` — the disposer

### `exec_reclaim(candidates, ctx, mode, target_pct, disk) → ReclaimReport`

Sorts candidates **largest-first** (by fresh measurement), then loops:

1. `vet_candidate` the candidate → on `Reject`, push to `skipped[]` and continue.
2. On `Allow`:
   - **dry-run** → push to `would_remove[]` (zero destructive ops).
   - **apply** → perform the `primitive`:
     - `TrackedWorktree`: `git worktree prune` → `git worktree remove --force`,
       then `git branch -D` using the branch read from `worktree list` (never
       agent free-text); the allow-root is **reasserted** at the syscall boundary.
     - `OrphanDir` / `StaleBuildCache`: `rm -rf`, with `under_any_root`
       re-checked immediately before the unlink (TOCTOU defense).
   - push to `removed[]`, add freed bytes.
3. Re-read `%-used` via the `DiskStatProvider`. Once under `target_pct`, **stop**
   (removes the minimum necessary).

Failures during an individual removal are captured in `failures[]` and do not
abort the run.

**Subprocess hardening:** git is invoked with `env_clear` (only `PATH`/`HOME`),
argument vectors only (no shell), `--` separators, and leading-dash paths
rejected — blocking `GIT_*` / `LD_PRELOAD` hijacking and option injection. `gh`
is read-only and its token is never logged or passed to git. No `--admin`, no
`--no-verify`.

### `ReclaimReport`

```rust
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReclaimReport {
    pub mode: ReclaimMode,          // DryRun | Apply
    pub used_pct_before: u8,
    pub used_pct_after: u8,
    pub target_pct: u8,
    pub bytes_freed: u64,
    pub removed: Vec<RemovedPath>,      // {path, kind, bytes, primitive}
    pub would_remove: Vec<RemovedPath>, // populated in dry-run
    pub skipped: Vec<SkippedPath>,      // {path, kind, reject_reason} — human review
    pub failures: Vec<ReclaimFailure>,  // {path, error}
}
```

**Methods:**

- `reclaim_performed() → bool` — `bytes_freed > 0` or `removed` non-empty.
- `summary() → String` — daemon one-liner, e.g.
  `"disk reclaim: 88% -> 84% used, freed 12026531840 bytes, 3 paths removed, 2 skipped for review"`.

## `mod.rs` — orchestration & config

### `reclaim_pct_from_env() → u8`

Reads `SIMARD_DISK_RECLAIM_PCT`, defaults to `85`, clamps to `[1, 99]`.

### `daemon_apply_from_env() → ReclaimMode`

Reads `SIMARD_DISK_RECLAIM_DAEMON_APPLY`. Returns `ReclaimMode::Apply` **only**
when it is set to `1`/`true`; otherwise `ReclaimMode::DryRun`. This is the knob
that keeps the **daemon** self-heal trigger disabled (dry-run + human-review)
until the recipe-step sandboxing is verified in production
(see [Recipe-step sandboxing](#recipe-step-sandboxing)). It governs the daemon
path only — the CLI derives its mode from `--apply`, never from this variable.

### `run_disk_reclaim(repo_root, state_root, home_override, mode, target_pct, source) → SimardResult<ReclaimReport>`

Top-level production orchestrator wiring recipe → parse → executor with the live
guard seams (`RealTrackedWorktreeProbe`, `ProcfsLiveProcessProbe`,
`DuSizeMeasurer`, `DerivingPathRemover`). Emits the `simard.disk.reclaim.*`
telemetry tagged with `source` (`daemon` | `cli`). **No fallback:** any recipe or
parse failure yields `SimardError::AdapterInvocationFailed` and propagates to the
caller (daemon logs a warning and continues; the CLI exits non-zero). Apply mode
is **refused when `geteuid() == 0`** (defense in depth; the CLI also pre-checks
for its exit-2 mapping).

| `mode` | Behavior |
| ------ | -------- |
| `ReclaimMode::DryRun` | Full analysis + guard vetting, **zero** destructive ops. The default everywhere — including the daemon, unless `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`. |
| `ReclaimMode::Apply` | Guarded reclamation. Refused when `geteuid() == 0`. Reached via CLI `--apply` or the daemon knob once sandboxing is verified. |

### `daemon_should_trigger(used_pct, threshold_pct) → bool`

The named, tested predicate for the daemon self-heal trigger: `true` iff
`used_pct >= threshold_pct`. Keeps the trigger semantics in one place rather than
inline in the maintenance loop.

## `recipe.rs` + `disk-reclaim.yaml` — the analysis-only recipe

`recipe.rs` invokes `prompt_assets/simard/recipes/disk-reclaim.yaml` via
`resolve_recipe_path` + `recipe-runner-rs --output-format json`, deserializes the
`RecipeOutput` envelope (same structs as [disk-health](./disk-health-api.md)),
and feeds `step_results[0].output` to `parse_candidates`.

The recipe is **a single analysis-only agent step**. Its prompt **forbids
destructive shell commands** — but a prompt-level ban is necessary, not
sufficient (see [Recipe-step sandboxing](#recipe-step-sandboxing) below, which is
what actually keeps the delete primitive out of the agent's hands). The step
instructs the agent to:

1. inspect `df --output=pcent,avail`,
2. run `git worktree list --porcelain` across all managed repos
   (`/home/azureuser/src/Simard`, `/home/azureuser/src/amplihack-rs`,
   `/home/azureuser/src/amplihack-memory-lib`) **and** `~/.simard` engineer
   worktrees,
3. read PR state via `gh pr list` / `gh pr view`,
4. read the `/proc` PID→cwd table,
5. measure directory sizes via `du`,
6. **reason** about reclaimable candidates largest-first,
7. **emit** the candidate JSON via the `CANDIDATES_JSON=` marker — and stop.

The agent never deletes; the executor does, behind the guard.

### `resolve_recipe_path(repo_root) → Option<PathBuf>`

Same precedence as the disk-health resolver:

1. **Hot-reload:** `~/.simard/prompt_assets/simard/recipes/disk-reclaim.yaml`
2. **In-tree:** `<repo_root>/prompt_assets/simard/recipes/disk-reclaim.yaml`

## Recipe-step sandboxing

The "agent proposes, Rust disposes" guarantee is only sound if the analysis
agent genuinely **cannot** delete anything itself. The recipe runner may hand
the analysis step a daemon-privilege shell; if that shell is unrestricted, the
capability confinement degrades to *prompt-requested* ("please don't run `rm`"),
which an errant or adversarial completion can ignore. This is the capability's
**highest-priority risk** and the reason the daemon ships in dry-run
(`SIMARD_DISK_RECLAIM_DAEMON_APPLY` unset) until the following are verified in
production. These are first-class deliverables of the recipe step, not
assumptions:

1. **`PATH` scrubbing.** The analysis step runs with a minimal `PATH` that
   exposes only the read-only inspection tools it needs (`df`, `git` read
   subcommands via a wrapper, `gh`, `du`, `cat`). Mutating binaries — `rm`,
   `find` (with `-delete`/`-exec`), `git worktree remove`, `truncate`, shell
   redirection helpers — are **not on `PATH`**, so the agent cannot invoke them
   even if the prompt guard is bypassed.
2. **Read-only / seccomp confinement.** The step executes under a confinement
   that makes the filesystem read-only outside a scratch dir (bind/overlay
   read-only mounts, or a seccomp profile denying `unlink`/`unlinkat`/`rmdir`/
   `rename`). Destructive syscalls fail at the kernel boundary, not the prompt.
3. **Post-run reconciliation diff.** After the analysis step returns, the
   orchestrator compares a cheap pre/post inventory of the managed roots (e.g.
   `git worktree list` sets and top-level dir listings). If **anything**
   disappeared that the executor did not remove under the guard, the run is
   flagged as a confinement breach: it is logged loudly via `daemon_log`, and
   apply mode is refused until investigated.

Only the deterministic executor (`executor.rs`) holds the delete primitive, and
every one of its removals passes `vet_candidate` (above). The sandboxing here
ensures the *agent* side of the split cannot open a second, unguarded deletion
path. Documented so it is implemented — and not silently dropped as "the prompt
says not to."

## CLI surface (`src/operator_cli/disk_reclaim.rs`)

```text
simard disk-reclaim [--apply] [--report-json] [--target-pct <1..=99>]
simard disk-reclaim exec --candidates <json|@file|@-> [--apply] [--report-json]
```

| Flag / subcommand | Effect |
| ----------------- | ------ |
| *(none)* | Dry-run: full report, zero destructive ops (default). |
| `--apply` | Live guarded reclamation. Refused when `geteuid() == 0`. |
| `--report-json` | Emit `ReclaimReport` as JSON instead of the human table. |
| `--target-pct N` | Override `SIMARD_DISK_RECLAIM_PCT` for this run (clamped `1..=99`). |
| `exec --candidates <src>` | Feed a candidate list (`@file`, `@-` stdin, or inline JSON) directly to the guarded executor. Every path is still re-vetted. |

**Exit codes:** `0` success / under threshold; `1` failure (recipe/parse/exec
error); `2` refused (e.g. `--apply` as root). `eprintln!`/table output is
confined to this operator-CLI handler; library paths use `tracing`, and the
daemon trigger uses `daemon_log` (→ `ooda.log` + stderr) — no bare `println!`.

## Trigger wiring

| Site | Behavior |
| ---- | -------- |
| `src/operator_commands_ooda/daemon/mod.rs` | Within the disk-health maintenance block, **Tier 1** `emergency_cleanup` runs first (deterministic, no LLM); then **Tier 2** on the `SIMARD_DISK_HEALTH_INTERVAL_SECS` cadence (default `900`) a cheap `df` `%-used` probe ≥ `SIMARD_DISK_RECLAIM_PCT` calls `run_disk_reclaim(mode)`. The daemon passes `mode = Apply` **only** when `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`; otherwise `DryRun` (ships disabled until sandboxing is verified — see the [risk note](#recipe-step-sandboxing)). Output goes through `daemon_log` → `{state_root}/ooda.log` + stderr (dashboard-visible), **not** `tracing`/`journalctl`. |
| `src/ooda_actions/advance_goal/spawn.rs` | The admission `ReclaimFirst` continuation calls `run_disk_reclaim` before retrying a deferred engineer spawn. This path is best-effort: a reclaim error is `tracing::warn!`-logged (`target: simard::ooda_brain`) and never fails the cycle. |

## Test coverage

Hermetic, `serial_test`-guarded refusal proofs are the core of the module: they
prove the guard **refuses** removal even when explicitly instructed, using
tempdirs, a fabricated `/proc` root, and `FakeLiveProcessProbe` / fake
`GhClient` doubles — **no real `rm` of real paths**.

| Test file | Proves |
| --------- | ------ |
| `tests_guard.rs` | Refusal of: `worktrees/main`, daemon cwd, live-PID path, uncommitted/unpushed, active worktree, outside-root, unknown-PR, symlink, option-injection — each asserted **skipped even when instructed to delete**. Plus the **OPEN-PR merge-base-is-ancestor regression** (a fresh worktree whose merge-base is an ancestor of `main` is **not** reclaimed). |
| `tests_candidate.rs` | `deny_unknown_fields`; bad-element-skip vs bad-array-hard-error; bounds. |
| `tests_executor.rs` | Largest-first ordering; threshold stop; dry-run zero-ops; TOCTOU re-assert; fake `DiskStatProvider`. |
| `tests_daemon_dir.rs` | Union resolution with injected `proc_root`; hardcoded `main` always present. |
| `tests_recipe.rs` | Marker parse; no-fallback `AdapterInvocationFailed` on recipe/parse failure. |
| `tests_sandbox.rs` | Recipe-step confinement: mutating binaries absent from the step `PATH`; the post-run reconciliation diff flags a fabricated out-of-band disappearance as a confinement breach and refuses apply. |
| `tests_env.rs` | `reclaim_pct_from_env` clamping; `daemon_apply_from_env` returns `DryRun` unless `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`. |

## Related

- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale
- [Configure disk reclamation (how-to)](../howto/configure-disk-reclamation.md) — operator usage
- [Disk reclaim telemetry (reference)](./disk-reclaim-telemetry.md) — emitted metrics
- [Disk health API (reference)](./disk-health-api.md) — the superseded per-cycle shim and the shared JSON-envelope pattern
- [Worktree reaping safety guards (reference)](./engineer-worktree-sweep-safety.md) — the liveness / uncommitted-work primitives the guard composes
