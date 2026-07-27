---
title: Disk reclaim API
description: Reference for the src/disk_reclaim module — the ReclaimCandidate serde contract, the non-bypassable guard::vet_candidate rail and its RejectReason set, the allow_roots reclamation scope (including per-repo target/ for routine build-cache reclaim), resolve_daemon_working_dirs, the exec_reclaim executor with per-candidate structured skip-reason tracing and ReclaimReport, the disk-reclaim.yaml analysis-only recipe contract, and the simard disk-reclaim CLI surface.
last_updated: 2026-07-27
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
    ∪ { <repo>/worktrees for repo in MANAGED_REPOS }      // per-repo worktree dirs
    ∪ { <repo>/target    for repo in MANAGED_REPOS }      // per-repo build artifacts
    ∪ { <state_root>/cargo-target, <state_root>/shared-target }  // shared build caches
```

`MANAGED_REPOS` is the same hardcoded managed-repo list the recipe enumerates
(§`recipe.rs`); it is **not** operator-configurable free-form (operators widen
the *deny*-set via `SIMARD_GIT_PROTECTED_REPOS`, never the allow-set — widening
the delete scope from the environment would be a footgun). A candidate outside
every allow-root — anywhere in `$HOME` not under a managed worktree/target/cache
root, or any absolute path the agent hallucinates — is refused before any other
rail is even consulted.

#### The per-repo `target/` root (routine-reclaim scope)

`allow_roots` includes **`<repo>/target`** (the `target/` **parent** directory)
for every managed repo. This is the root that lets **routine** reclaim free
rebuildable Cargo artifacts — `target/debug/`, `target/release/`,
`target/llvm-cov-target/`, and the incremental-compilation caches — the same
build artifacts the deterministic `emergency_cleanup` (Tier 1) removes at severe
pressure.

The root is deliberately the **parent** `target/`, not an exact-match
`target/debug` entry, because `is_safe_to_delete`
(`maintenance::is_safe_to_delete`) requires a candidate to be **strictly inside**
an allow-root (component-wise `Path::starts_with`, never string-prefix). Rooting
at `target/`'s parent is the smallest change that lets a `StaleBuildCache`
candidate for `target/debug` clear the containment check without altering the
guard's strict-inside semantics. Widening to `target/` is **additive**: every
direct child of `target/` is a rebuildable artifact already inside the
build-cache category the guard permits, and removal is still gated by

- the agent proposing a `CandidateKind::StaleBuildCache` candidate,
- the `ProtectedDenySet` (which still wins over any allow-root, so
  `worktrees/main` and daemon working dirs remain unreclaimable),
- the live-PID rail (any process whose `/proc/<pid>/cwd` resolves *inside* the
  candidate is refused with `LiveProcess`; note this keys on cwd, so a
  `cargo build` invoked with its cwd at the repo root is not caught by this rail
  — such artifacts are instead safe because they are the rebuildable
  `StaleBuildCache` class), and
- the TOCTOU re-assert + symlink refusal at the syscall boundary.

Rooting at `target/`'s parent **cannot** reach `<repo>/src`, `<repo>/.git`, or
sibling files: strict-inside containment confines removal to descendants of
`target/`. `MANAGED_REPOS` never includes bare `$HOME`
(`managed_repos_do_not_include_bare_home` guards this), so the widened root
cannot escalate to a home-directory sweep.

**Why this matters (regression fixed):** before the `target/` root existed,
routine reclaim had no allow-root covering `<repo>/target`, so every proposed
`target/debug` candidate was rejected with `OutsideAllowRoot` and pushed to the
human-review list — the "freed 0 bytes, 0 paths removed, N skipped for review"
no-op that let disk climb until the ~30-minute `emergency_cleanup` pass. With
`target/` in scope, routine reclaim frees those artifacts **proactively between**
emergency passes instead of skipping them.

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

### Structured skip-reason tracing

Every rejected candidate is both pushed to `skipped[]` **and** logged with a
structured `tracing` event at the reject site (immediately after `vet_candidate`
returns `Verdict::Reject`), so an operator can see *why* a path was not reclaimed
without reconstructing it from counters:

```rust
tracing::info!(
    target: "simard::disk_reclaim",
    path = %candidate.path.display(),
    reason = ?reason,            // the closed RejectReason enum
    kind = ?candidate.kind,      // CandidateKind
    "reclaim candidate skipped by guard",
);
```

- **Fields only** — `path`, the `RejectReason` enum, and `CandidateKind`. The
  agent's free-text `reason` field is **never** logged as a field (anti
  log-forging; see [telemetry](./disk-reclaim-telemetry.md)),
  and no file contents or env values are emitted.
- **`info` level**, one event per skipped candidate, OTLP-compatible (rides the
  same `tracing` → OTel pipeline as the rest of the module). No `print!` /
  `println!` — library paths use `tracing` exclusively.
- Complements, and does not replace, the existing `summary()` one-liner
  (`"… N skipped for review"`) and the
  `simard.disk.reclaim.candidates_skipped` counter. The per-candidate events
  answer *which* path and *why*; the summary and counter answer *how many*.

This is the observability that turns a silent "N skipped for review" line into
an actionable, per-path audit trail — e.g. distinguishing a `target/debug`
skipped for `LiveProcess` (a build is running; expected) from one skipped for
`OutsideAllowRoot` (a scope bug worth investigating).

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
until OS-level recipe-step confinement is implemented
(see [Recipe-step sandboxing](#recipe-step-sandboxing); not yet wired in). It
governs the daemon path only — the CLI derives its mode from `--apply`, never
from this variable.

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
| `ReclaimMode::Apply` | Guarded reclamation. Refused when `geteuid() == 0`. Reached via CLI `--apply` or the daemon knob once recipe-step confinement is implemented. |

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
destructive shell commands**. A prompt-level ban is necessary but not sufficient
on its own; what actually keeps the delete primitive out of the agent's hands
today is that the executor owns the only delete path and re-vets every candidate
through the guard (planned OS-level confinement is tracked under
[Recipe-step sandboxing](#recipe-step-sandboxing) below). The step instructs the
agent to:

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

The "agent proposes, Rust disposes" guarantee rests on two properties that **are**
implemented and tested today:

1. The `disk-reclaim.yaml` step is **analysis-only** — its prompt forbids
   destructive shell commands and instructs the agent to emit candidate markers
   and stop.
2. The **executor holds the only delete primitive**, and every candidate is
   re-vetted by `vet_candidate` at the syscall boundary. Nothing the agent emits
   can widen the rails.

A prompt-level ban is necessary but not *sufficient* on its own: if the recipe
runner hands the analysis step an unrestricted daemon-privilege shell, an errant
or adversarial completion could in principle invoke `rm` directly, opening a
second deletion path the guard never sees. Closing that gap with OS-level
recipe-step confinement is the capability's **highest-priority follow-up** and the
reason the daemon ships in dry-run (`SIMARD_DISK_RECLAIM_DAEMON_APPLY` unset). The
planned hardening — **not yet wired in** — is:

1. **`PATH` scrubbing.** Run the analysis step with a minimal `PATH` that exposes
   only the read-only inspection tools it needs (`df`, `git` read subcommands,
   `gh`, `du`, `cat`), with mutating binaries — `rm`, `find` (with
   `-delete`/`-exec`), `git worktree remove`, `truncate` — removed.
2. **Read-only / seccomp confinement.** Execute the step under a confinement that
   makes the filesystem read-only outside a scratch dir (bind/overlay read-only
   mounts, or a seccomp profile denying `unlink`/`unlinkat`/`rmdir`/`rename`) so
   destructive syscalls fail at the kernel boundary.
3. **Post-run reconciliation diff.** After the analysis step returns, compare a
   cheap pre/post inventory of the managed roots. If anything disappeared that the
   guarded executor did not remove, flag the run as a confinement breach — log it
   loudly and refuse apply until investigated.

Until these land, the daemon defaults to dry-run + human-review and operators
reclaim by hand with `simard disk-reclaim --apply`. The wired guard above remains
authoritative for everything the executor removes.

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
| `src/operator_commands_ooda/daemon/mod.rs` | Within the disk-health maintenance block, **Tier 1** `emergency_cleanup` runs first (deterministic, no LLM); then **Tier 2** on the `SIMARD_DISK_HEALTH_INTERVAL_SECS` cadence (default `900`) a cheap `df` `%-used` probe ≥ `SIMARD_DISK_RECLAIM_PCT` calls `run_disk_reclaim(mode)`. The daemon passes `mode = Apply` **only** when `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`; otherwise `DryRun` (ships disabled until OS-level recipe-step confinement is implemented — see the [risk note](#recipe-step-sandboxing)). Output goes through `daemon_log` → `{state_root}/ooda.log` + stderr (dashboard-visible), **not** `tracing`/`journalctl`. |
| `src/ooda_actions/advance_goal/spawn.rs` | The admission `ReclaimFirst` continuation calls `run_disk_reclaim` before retrying a deferred engineer spawn. This path is best-effort: a reclaim error is `tracing::warn!`-logged (`target: simard::ooda_brain`) and never fails the cycle. |

## Test coverage

Hermetic, `serial_test`-guarded refusal proofs are the core of the module: they
prove the guard **refuses** removal even when explicitly instructed, using
tempdirs, a fabricated `/proc` root, and `FakeLiveProcessProbe` / fake
`GhClient` doubles — **no real `rm` of real paths**.

| Test file | Proves |
| --------- | ------ |
| `tests_guard.rs` | Refusal of: `worktrees/main`, daemon cwd, live-PID path, uncommitted/unpushed, active worktree, outside-root, unknown-PR, symlink, option-injection — each asserted **skipped even when instructed to delete**. Plus the **OPEN-PR merge-base-is-ancestor regression** (a fresh worktree whose merge-base is an ancestor of `main` is **not** reclaimed). With the widened `<repo>/target` allow-root: `<repo>/src` and `<repo>/.git` are still refused (`OutsideAllowRoot`), a symlinked `target/*` is refused, a live-PID `target/debug` is refused (`LiveProcess`), and `worktrees/main` in the deny-set still overrides the allow-root. |
| `tests_candidate.rs` | `deny_unknown_fields`; bad-element-skip vs bad-array-hard-error; bounds. |
| `tests_executor.rs` | Largest-first ordering; threshold stop; dry-run zero-ops; TOCTOU re-assert; fake `DiskStatProvider`. **Routine-reclaim regression:** a `target/debug` candidate outside the old scope reproduces `bytes_freed == 0` with the path in `skipped[]` (`reject_reason = OutsideAllowRoot`); with the `<repo>/target` root it moves to `removed[]` with `bytes_freed > 0`. The reject arm emits the per-candidate `tracing::info!(path, reason, kind)` event; the test asserts the recorded `SkippedPath.reject_reason`. |
| `tests_daemon_dir.rs` | Union resolution with injected `proc_root`; hardcoded `main` always present. |
| `tests_recipe.rs` | Marker parse; no-fallback `AdapterInvocationFailed` on recipe/parse failure. |
| `tests_env.rs` | `reclaim_pct_from_env` clamping; `daemon_apply_from_env` returns `DryRun` unless `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`. |
| `mod.rs` (unit) | `allow_roots_cover_engineer_and_managed_worktrees` (extended to cover per-repo `target/`); `managed_repos_do_not_include_bare_home` (widened root never resolves to bare `$HOME`). |

## Related

- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale
- [Configure disk reclamation (how-to)](../howto/configure-disk-reclamation.md) — operator usage
- [Disk reclaim telemetry (reference)](./disk-reclaim-telemetry.md) — emitted metrics
- [Disk health API (reference)](./disk-health-api.md) — the superseded per-cycle shim and the shared JSON-envelope pattern
- [Worktree reaping safety guards (reference)](./engineer-worktree-sweep-safety.md) — the liveness / uncommitted-work primitives the guard composes
