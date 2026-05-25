---
title: Disk Health Check API
description: Reference for the automated disk health check that runs each OODA cycle, cleaning stale worktrees, cargo build artifacts, and LadybugDB backups before disk exhaustion can crash the daemon.
last_updated: 2026-05-24
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../howto/configure-disk-health-check.md
  - ../concepts/automated-disk-health.md
  - ../howto/inspect-and-clean-engineer-worktrees.md
  - ../howto/reclaim-disk-space-and-run-low-space-rust-builds.md
  - ./engineer-worktree-isolation.md
---

# Disk Health Check API

Module: `simard::disk_health`
Source: `src/disk_health.rs`
Recipe: `prompt_assets/simard/recipes/disk-health-check.yaml`

The disk health check is a recipe-driven subsystem that Simard executes once
per OODA cycle, *before* spawning any engineer subprocesses. It prevents the
`ENOSPC` crash that killed the daemon on 2026-05-24 (issue #2020) by
proactively freeing disk when the home partition exceeds 80% usage.

The Rust code is a thin shim. The recipe YAML contains a single **agent step**
— an LLM-backed agent that receives a prompt describing the disk situation,
uses bash tools to inspect and clean, and emits structured text markers that
the Rust shim parses. The agent decides *what* to clean and *how aggressively*
based on current conditions; the prompt provides guidance on known cleanup
targets, safety constraints, and the required output format.

## Module Layout

```
src/disk_health.rs                           Rust shim (subprocess invocation, text parsing)
prompt_assets/simard/recipes/disk-health-check.yaml   Recipe (agent step with cleanup prompt)
```

## Public API — `src/disk_health.rs`

### `DiskHealthReport`

```rust
/// Report parsed from key=value lines emitted by the disk-health-check recipe.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiskHealthReport {
    /// Current disk usage percentage after any cleanup (0–100).
    pub disk_used_pct: u64,
    /// Bytes freed during this check (0 if usage was below threshold).
    pub freed_bytes: u64,
    /// Human-readable list of cleanup actions taken.
    pub actions_taken: Vec<String>,
}
```

The report is parsed from key=value lines on stdout emitted by the agent
step. The Rust shim uses `parse_disk_health_text()` to extract fields from
the text output — no JSON parsing. The agent is instructed to emit these
markers as the last lines of its output. See
[text-parsing wire formats](./text-parsing-wire-formats.md#protocol-3-keyvalue-disk-health)
for the full grammar.

### `run_disk_health_check`

```rust
/// Run the disk-health-check recipe and return the parsed report.
///
/// Resolves the recipe YAML relative to `repo_root`, invokes it via
/// `recipe-runner-rs` as a subprocess, captures stdout, and parses the
/// key=value text output.
///
/// # Arguments
///
/// * `repo_root` — absolute path to the Simard repository root. Used to
///   locate `prompt_assets/simard/recipes/disk-health-check.yaml`.
/// * `state_root` — absolute path to the Simard state directory
///   (typically `~/.simard/`). Passed to the recipe as the `STATE_ROOT`
///   context variable so cleanup targets the correct directories.
///
/// # Errors
///
/// Returns `SimardError::AdapterInvocationFailed` if:
/// - The recipe YAML does not exist at the expected path
/// - `recipe-runner-rs` is not found on `$PATH`
/// - The subprocess exits non-zero
///
/// Note: missing or malformed key=value lines do **not** cause an error.
/// `parse_disk_health_text` defaults to `disk_used_pct=0`, `freed_bytes=0`,
/// `actions_taken=[]` for any missing fields. This is intentional — a
/// recipe that outputs only `DISK_USED_PCT=65` (no cleanup needed) is valid.
///
/// # Example
///
/// ```rust,no_run
/// use simard::disk_health::run_disk_health_check;
/// use std::path::Path;
///
/// let report = run_disk_health_check(
///     Path::new("/home/azureuser/src/Simard"),
///     Path::new("/home/azureuser/.simard"),
/// )?;
/// println!("Disk at {}%, freed {} bytes", report.disk_used_pct, report.freed_bytes);
/// ```
pub fn run_disk_health_check(
    repo_root: &Path,
    state_root: &Path,
) -> Result<DiskHealthReport, SimardError>;
```

### `parse_disk_health_text`

```rust
/// Parse key=value lines from recipe stdout into a DiskHealthReport.
///
/// Scans each line for:
/// - `DISK_USED_PCT=<u64>` — disk usage percentage
/// - `FREED_BYTES=<u64>` — bytes freed during cleanup
/// - `ACTION: <text>` — human-readable cleanup action (collected into Vec)
///
/// Lines that don't match any pattern are silently ignored (forward compatible).
/// Missing fields default to zero/empty.
fn parse_disk_health_text(stdout: &str) -> DiskHealthReport;
```

### `resolve_recipe_path` (internal)

```rust
/// Returns the path to disk-health-check.yaml, checking:
///   1. `~/.simard/prompt_assets/simard/recipes/` (hot-reload override)
///   2. `<repo_root>/prompt_assets/simard/recipes/` (in-tree)
/// Returns None if neither exists.
fn resolve_recipe_path(repo_root: &Path) -> Option<PathBuf>;
```

Resolution order (matches `recipe_merge_judge.rs` and
`recipe_progress_checker.rs`):

1. **Hot-reload path**: `~/.simard/prompt_assets/simard/recipes/disk-health-check.yaml`
   — Operators can drop a modified recipe here to override the in-tree version
   without touching the repository.
2. **In-tree path**: `<repo_root>/prompt_assets/simard/recipes/disk-health-check.yaml`
   — The version shipped with the repository.

## Recipe YAML — `disk-health-check.yaml`

The recipe is a single **agent step** (`type: "agent"`, `agent: "default"`).
The agent receives a prompt describing the disk situation and known cleanup
targets under `{{state_root}}`. The agent uses its bash tool-use capability
to run `df`, `find`, `du`, and `rm` commands — but the *logic* of what to
clean and how aggressively is determined by the agent, not hardcoded.

The agent's stdout must include key=value text markers (not JSON). See
[text-parsing wire formats § key=value](./text-parsing-wire-formats.md#protocol-3-keyvalue-disk-health)
for the normative grammar.

The recipe receives one context variable:

| Variable     | Source              | Description                                              |
| ------------ | ------------------- | -------------------------------------------------------- |
| `state_root` | Context default     | Path to `~/.simard` (worktrees, backups, cargo dirs)     |

### Agent prompt guidance

The agent prompt instructs the agent to:

1. Check disk usage via `df` on the `$HOME` partition
2. If below 80%, emit `DISK_USED_PCT` and `FREED_BYTES=0` and stop
3. If ≥ 80%, apply judgment to clean from these targets under `{{state_root}}`:
   - `engineer-worktrees/` — stale worktrees (>24h old, no live PID in `.simard-engineer-claim`)
   - `engineer-worktrees/*/target/` — cargo build dirs in remaining worktrees
   - `backups/` — prune to approximately 5 most recent
   - `cargo-target/`, `shared-target/` — incremental/debug build artifacts
4. Measure freed space via `df` delta before/after cleanup
5. Emit the required text markers

The prompt lists these targets as **guidance**, not hard commands. The agent
can skip targets that don't exist, adjust retention counts based on disk
pressure severity, and adapt to unexpected directory layouts.

### Safety constraints in the prompt

The prompt constrains the agent's cleanup scope:

- **Only clean under `{{state_root}}`** — no paths outside the Simard state directory
- **Check `.simard-engineer-claim` before removing worktrees** — if the claim file
  contains a PID that responds to `kill -0`, the worktree is active and must be skipped
- **Never remove the main repository worktree** or anything outside `{{state_root}}`
- **Emit `ACTION:` lines for every deletion** so the Rust shim can log what happened

### Cleanup thresholds

Because the agent interprets the prompt adaptively, there are no hardcoded
threshold variables. The prompt text provides guidance values that the agent
uses as starting points:

| Guidance                      | Value | Agent behavior                                     |
| ----------------------------- | ----- | -------------------------------------------------- |
| Disk usage trigger            | ~80%  | Agent checks df and decides whether cleanup needed |
| Worktree age suggestion       | ~24h  | Agent uses mtime/age heuristics, may adjust        |
| Backup retention suggestion   | ~5    | Agent keeps approximately this many, adjusts for pressure |
| Cargo cache cleaning          | all   | Agent removes incremental/debug dirs from caches   |

To change these, edit the prompt text in `disk-health-check.yaml`. For example,
change "older than 24 hours" to "older than 4 hours" to be more aggressive with
worktree cleanup.

### Text output contract

The agent emits key=value lines to stdout as the final part of its response:

```
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 12 stale engineer worktrees older than 24h
ACTION: Removed cargo target dirs from 3 worktrees
ACTION: Pruned LadybugDB backups to 5 most recent
ACTION: Cleaned shared-target/ incremental and debug dirs
```

When disk usage is below 80%, the agent reports no cleanup:

```
DISK_USED_PCT=65
FREED_BYTES=0
```

The `DISK_USED_PCT` marker is **required** — the Rust parser returns an error
if it is missing. `FREED_BYTES` defaults to 0 if absent. `ACTION:` lines are
optional (zero or more). Unknown lines (agent reasoning, intermediate output)
are silently ignored by the parser — this is forward-compatible by design.

### Security constraints

| Defense                         | Mechanism                                                       |
| ------------------------------- | --------------------------------------------------------------- |
| Scoped to `{{state_root}}`      | Prompt constrains all cleanup to the `{{state_root}}` subtree   |
| PID-based claim check           | Prompt instructs agent to check `.simard-engineer-claim` PIDs   |
| No secrets in prompt            | Recipe YAML is plain text, stored in-tree, no credentials       |
| Agent output parsed by Rust     | `parse_disk_health_text()` ignores non-marker lines — no injection risk |
| `state_root` from daemon config | Not user input — no shell injection vector                      |

## Daemon integration

The disk health check is called in `src/operator_commands_ooda/daemon/mod.rs`
at the top of each OODA cycle, after LadybugDB backup and before
`cycle_start = Instant::now()`:

```rust
// ── disk health check (issue #2020) ──────────────────────────
match disk_health::run_disk_health_check(&bridges.repo_root, &state_root) {
    Ok(report) => {
        daemon_log(
            &state_root,
            &format!(
                "[simard] disk health: {}% used, freed {} bytes, {} actions",
                report.disk_used_pct,
                report.freed_bytes,
                report.actions_taken.len(),
            ),
        );
        if report.disk_used_pct > 90 {
            tracing::warn!(
                disk_used_pct = report.disk_used_pct,
                "disk still above 90% after cleanup — builds may fail",
            );
        }
    }
    Err(e) => {
        tracing::warn!(?e, "disk health check failed — continuing cycle");
        daemon_log(
            &state_root,
            &format!("[simard] disk health check failed: {e}"),
        );
    }
}
```

The check is **warn-and-continue**: a failure in the disk health check never
blocks the OODA cycle. The existing `disk_pressure` module provides the hard
stop if disk is truly exhausted.

### Interaction with existing subsystems

| Subsystem                    | Interaction                                                                 |
| ---------------------------- | --------------------------------------------------------------------------- |
| `disk_pressure`              | Hard stop at critical thresholds. Disk health is the soft pre-emptive layer.|
| `sweep_orphaned_worktrees`   | Boot-time only. Disk health runs every cycle for continuous hygiene.         |
| `EngineerWorktree::cleanup`  | Per-engineer RAII cleanup. Disk health catches engineers that outlived their handle (crash orphans older than 24h). |
| LadybugDB backup rotation    | Previously unbounded. Disk health enforces retention of 5 most recent.      |

## Error handling

All errors surface as `SimardError::AdapterInvocationFailed`:

| Scenario                         | `base_type`          | `reason`                                   |
| -------------------------------- | -------------------- | ------------------------------------------ |
| Recipe YAML not found            | `"disk-health"`      | `"recipe not found at <path>"`             |
| `recipe-runner-rs` not on PATH   | `"disk-health"`      | `"recipe-runner-rs not found"`             |
| Subprocess non-zero exit         | `"disk-health"`      | `"recipe exited with status <code>: <stderr>"` |

Per daemon convention, these errors are logged and the cycle continues. The
disk health check never crashes the daemon.

## Configuration

| Env var              | Effect                                                    | Default       |
| -------------------- | --------------------------------------------------------- | ------------- |
| `SIMARD_STATE_ROOT`  | Root for all cleanup targets                              | `~/.simard/`  |
| `CARGO_TARGET_DIR`   | If set, shared target is at this path instead of default  | (not set)     |

The cleanup guidance values (80% trigger, 24h worktree age, 5 backup retention)
are specified in the agent prompt text within the recipe YAML, not in Rust or
environment variables. To change them, edit the prompt in
`disk-health-check.yaml` — no recompile needed.

**Note on backup retention interaction:** The daemon's existing DB backup code
uses `SIMARD_DB_BACKUP_KEEP` (default 24) to prune backups *after* creating
each new one. The disk health agent applies a stricter retention of ~5 *before*
the backup step in the cycle. The agent's retention wins in practice. Operators
who want more retention should update *both* the prompt guidance in the recipe
YAML and `SIMARD_DB_BACKUP_KEEP` in the environment.

## `.cargo/config.toml` — shared target directory

The repository ships a `.cargo/config.toml` that redirects all Cargo builds
to a single shared target directory:

```toml
[build]
target-dir = "/home/azureuser/.simard/shared-target"
```

This prevents each worktree from accumulating its own multi-GB `target/`
directory. The tradeoff is that concurrent `cargo build` invocations serialize
on Cargo's built-in file lock — acceptable given the disk savings (191G →
shared ~3G).

This config applies to all `cargo` commands run from any worktree of this
repository. Engineer worktrees inherit it automatically. The
`CARGO_TARGET_DIR` environment variable, if set, overrides this config.

**Note:** The `target-dir` path is an absolute path hardcoded to this host's
state directory. If cloning the repository on a different machine, update
`.cargo/config.toml` to reflect the local state root, or set `CARGO_TARGET_DIR`
in the environment to override it.

## Tests

### Unit tests (`src/disk_health.rs`)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn resolve_recipe_path_returns_none_for_missing_repo() { /* ... */ }

    #[test]
    fn parse_disk_health_text_full_output() { /* ... */ }

    #[test]
    fn parse_disk_health_text_no_cleanup() { /* ... */ }

    #[test]
    fn parse_disk_health_text_malformed_lines_ignored() { /* ... */ }
}
```

- `resolve_recipe_path_returns_none_for_missing_repo` — verifies that
  `resolve_recipe_path` returns `None` when pointed at a temp dir without
  the recipe YAML.
- `parse_disk_health_text_full_output` — parses a complete key=value output
  with `DISK_USED_PCT`, `FREED_BYTES`, and multiple `ACTION:` lines.
- `parse_disk_health_text_no_cleanup` — parses output with only
  `DISK_USED_PCT` and `FREED_BYTES=0` (no `ACTION:` lines).
- `parse_disk_health_text_malformed_lines_ignored` — verifies that non-matching
  lines (debug output, warnings) are silently ignored.

No integration tests spawn `recipe-runner-rs` — the recipe is a bash script
tested by the daemon's existing smoke-test cycle.

## Related

- [Automated disk health (concept)](../concepts/automated-disk-health.md) — design rationale
- [Configure disk health check (how-to)](../howto/configure-disk-health-check.md) — operator guide
- [Inspect and clean engineer worktrees](../howto/inspect-and-clean-engineer-worktrees.md)
- [Reclaim disk space and run low-space Rust builds](../howto/reclaim-disk-space-and-run-low-space-rust-builds.md)
- [Per-Engineer Worktree Isolation](./engineer-worktree-isolation.md)
- Source: `src/disk_health.rs`, `prompt_assets/simard/recipes/disk-health-check.yaml`
