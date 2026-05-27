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

The Rust code is a thin shim. All cleanup logic lives in the recipe YAML as
an agent step that adaptively decides what to clean based on disk pressure.

## Module Layout

```
src/disk_health.rs                           Rust shim (subprocess invocation, text parsing)
prompt_assets/simard/recipes/disk-health-check.yaml   Recipe (agent cleanup step)
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

The report is parsed from key=value lines on stdout by the recipe's agent
step. The Rust shim uses `parse_disk_health_text()` to extract fields from
the text output — no JSON parsing. See
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

The recipe is a single agent step. Its stdout uses the key=value text format
(not JSON). See [text-parsing wire formats § key=value](./text-parsing-wire-formats.md#protocol-3-keyvalue-disk-health)
for the normative grammar.

The recipe receives two context variables:

| Variable     | Source              | Description                                              |
| ------------ | ------------------- | -------------------------------------------------------- |
| `STATE_ROOT` | `state_root` param  | Absolute path to `~/.simard/` (or `$SIMARD_STATE_ROOT`)  |
| `REPO_ROOT`  | `repo_root` param   | Absolute path to the Simard repo root                    |

### Cleanup thresholds

| Parameter                     | Value | Rationale                                         |
| ----------------------------- | ----- | ------------------------------------------------- |
| Disk usage trigger            | 80%   | Leaves 20% headroom for active builds             |
| Worktree max age              | 24h   | Engineers run ≤2h; 24h is 12× safety margin       |
| LadybugDB backup retention    | 5     | At 5-min interval, covers 25 min of rollback      |
| Cargo target dirs cleaned     | all   | Engineer worktrees can rebuild; 10-min cost max    |

### Cleanup actions (in order)

When disk usage exceeds 80%, the recipe performs these actions sequentially:

1. **Remove stale engineer worktrees** — directories under
   `$STATE_ROOT/engineer-worktrees/` older than 24 hours with no active
   process holding a `.simard-engineer-claim` lockfile. Symlinks are skipped.
   Each path is canonicalized and prefix-checked against the worktrees root
   before deletion.

2. **Remove cargo target dirs in engineer worktrees** — any `target/`
   directory inside surviving `$STATE_ROOT/engineer-worktrees/*/` entries.
   These are build artifacts that engineers can regenerate.

3. **Prune LadybugDB backups** — in `$STATE_ROOT/backups/`, sort by mtime
   descending and remove all but the 5 most recent files.

4. **Clean shared cargo caches** — remove `$STATE_ROOT/cargo-target/` and
   `$STATE_ROOT/shared-target/` contents. These are shared incremental build
   caches that Cargo rebuilds on demand.

### Text output contract

The agent step prints key=value lines to stdout:

```
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 48 stale worktrees (50.1G)
ACTION: Removed cargo target dirs from 3 worktrees (1.2G)
ACTION: Pruned 19 LadybugDB backups (512M)
ACTION: Cleaned cargo-target/ (12.0G) and shared-target/ (2.8G)
```

When disk usage is below 80%, the report reflects no cleanup:

```
DISK_USED_PCT=65
FREED_BYTES=0
```

This format is natural to produce from bash (`echo "KEY=${VALUE}"`) and
eliminates the JSON `printf` quoting fragility of the prior implementation.
Filenames with special characters in action descriptions are safe — they are
just text after `ACTION:`.

### Security constraints

The bash step follows the same security hardening as the rest of the Simard
bash surface:

| Defense                         | Mechanism                                                       |
| ------------------------------- | --------------------------------------------------------------- |
| Hardcoded path prefixes         | All `rm -rf` and `find -delete` operate only under `$STATE_ROOT/backups/`, `$STATE_ROOT/engineer-worktrees/`, `$STATE_ROOT/cargo-target/`, or `$STATE_ROOT/shared-target/` |
| No symlink following            | `find -not -type l` excludes symlinks from deletion candidates  |
| Canonicalize before delete      | `realpath` + prefix check before every `rm -rf`                 |
| Quoted expansions               | Every `$VAR` is double-quoted to prevent word splitting          |
| No eval / backtick on untrusted | No `eval`, no command substitution on user-controlled data       |
| Audit trail                     | Every deletion is logged to stderr with the exact path           |
| TOCTOU acceptance               | Stat-before-delete with accepted residual window (matches `sweep_orphaned_worktrees` pattern) |

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
| Runtime config unavailable       | `"disk-health"`      | `"missing required config: SIMARD_LLM_PROVIDER ..."` |
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

`run_disk_health_check` also reads `RuntimeConfig` (env var
`SIMARD_LLM_PROVIDER` → `~/.simard/config.toml`) to set
`AMPLIHACK_AGENT_BINARY` on the `recipe-runner-rs` subprocess. If neither
source provides the LLM provider, the function returns
`SimardError::MissingRequiredConfig`. See
[subprocess environment propagation](./subprocess-env-propagation.md) for
the full contract.

The cleanup thresholds (80%, 24h, 5 backups) are defined in the recipe YAML,
not in Rust. To change them, edit `disk-health-check.yaml` — no recompile
needed.

**Note on backup retention interaction:** The daemon's existing DB backup code
uses `SIMARD_DB_BACKUP_KEEP` (default 24) to prune backups *after* creating
each new one. The disk health recipe applies a stricter retention of 5 *before*
the backup step in the cycle. The recipe's retention wins in practice — it
prunes to 5 before the daemon creates a new backup. Operators who want more
retention should update *both* `BACKUP_RETENTION` in the recipe YAML and
`SIMARD_DB_BACKUP_KEEP` in the environment.

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
