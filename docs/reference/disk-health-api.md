---
title: Disk health API
description: Reference for the disk_health module — JSON envelope deserialization, text marker parsing, and the DiskHealthReport struct.
last_updated: 2026-06-05
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/automated-disk-health.md
  - ../howto/configure-disk-health-check.md
  - ./base-type-adapters.md
---

# Disk health API

**Module:** `src/disk_health.rs`

The `disk_health` module provides two-tier disk health management:

1. **`emergency_cleanup()`** — deterministic Rust cleanup at critical disk
   levels (≥95%). No LLM, no recipe, no external dependencies.
2. **`run_disk_health_check()`** — recipe-based LLM cleanup at moderate
   disk levels (≥80%). Invokes `recipe-runner-rs` with JSON envelope parsing.

## Data flow

### Tier 1: Emergency cleanup (deterministic)

```
emergency_cleanup(repo_root, state_root)
       │
       ▼
get_disk_usage_pct(repo_root)  →  df --output=pcent
       │
       ├─ < 95%  →  return None (no action needed)
       │
       └─ ≥ 95%  →  delete known-safe artifacts:
              │        target/debug/, target/llvm-cov-target/,
              │        worktrees/*/target/, cargo-target/, shared-target/,
              │        stale backups (keep 2)
              ▼
       Some(DiskHealthReport { disk_used_pct, freed_bytes, actions_taken })
```

### Tier 2: Recipe-based cleanup (LLM agent)

```
run_disk_health_check(repo_root, state_root)
       │
       ▼
recipe-runner-rs --output-format json
       │
       ▼
JSON envelope (stdout)
  { success, step_results: [{ step_id, output }] }
       │
       ▼
serde_json::from_slice::<RecipeOutput>()
       │
       ▼
step_results[0].output  (agent's raw text output)
       │
       ▼
parse_disk_health_text()  (key=value line parser)
       │
       ▼
DiskHealthReport { disk_used_pct, freed_bytes, actions_taken }
```

## Public API

### `emergency_cleanup(repo_root, state_root) → Option<DiskHealthReport>`

Tier 1 deterministic cleanup. Runs when disk usage is critically high (≥95%).

**Parameters:**

| Parameter    | Type     | Description                                                  |
| ------------ | -------- | ------------------------------------------------------------ |
| `repo_root`  | `&Path`  | Repository root — used to find `target/` and `worktrees/`    |
| `state_root` | `&Path`  | Simard state directory (`~/.simard`) — cargo caches, backups |

**Returns:** `Option<DiskHealthReport>`

- `None` — disk usage is below 95%, no action taken
- `Some(report)` — cleanup was performed; report contains what was freed

**Deletion targets (all regenerable):**

| Target                              | Condition     | Regeneration cost         |
| ----------------------------------- | ------------- | ------------------------- |
| `repo_root/target/debug/`           | Always        | Full rebuild (~10 min)    |
| `repo_root/target/llvm-cov-target/` | Always        | `cargo llvm-cov` rerun    |
| `repo_root/worktrees/*/target/`     | Always        | Per-worktree rebuild      |
| `state_root/cargo-target/`          | Always        | Cold build                |
| `state_root/shared-target/`         | Always        | Cold build                |
| `state_root/backups/*` beyond 2     | Always        | Reduced rollback window   |

**Error handling:** Per-item. Each `remove_dir_all()`/`remove_file()` is
guarded by `.is_ok()` — if one deletion fails (permissions, busy file), the
rest still attempt. The function never returns `Err`; it returns `None` (below
threshold) or `Some(report)` (attempted cleanup with whatever succeeded).

**Security:** All paths are constructed from `repo_root`/`state_root` with
hardcoded path segments. No user-controlled input reaches `remove_dir_all()`.
`Command::new("df")`/`Command::new("du")` use `.arg()` — no shell injection.

### `run_disk_health_check(repo_root, state_root) → SimardResult<DiskHealthReport>`

Entry point. Resolves the recipe YAML, spawns `recipe-runner-rs` with
`--output-format json`, deserializes the JSON envelope, extracts the first
step's output, and parses it into a `DiskHealthReport`.

**Parameters:**

| Parameter    | Type     | Description                                                  |
| ------------ | -------- | ------------------------------------------------------------ |
| `repo_root`  | `&Path`  | Repository root — used to locate the recipe YAML file        |
| `state_root` | `&Path`  | Simard state directory (`~/.simard`) — passed as context var |

**Returns:** `SimardResult<DiskHealthReport>`

**Errors** (`SimardError::AdapterInvocationFailed`):

| Condition                         | Error reason                                      |
| --------------------------------- | ------------------------------------------------- |
| Recipe YAML not found             | `"recipe file disk-health-check.yaml not found…"` |
| `recipe-runner-rs` not on PATH    | `"recipe-runner-rs spawn failed: …"`              |
| Recipe exited non-zero            | `"recipe exited with <code>: <stderr>"`           |
| JSON deserialization failed        | `"failed to deserialize recipe JSON output: …"`   |
| Empty step_results                | `"no step results in recipe JSON output"`         |
| Text markers missing DISK_USED_PCT | `"failed to parse recipe text output: …"`        |

No fallback. If the recipe fails for any reason, the error propagates to the
caller (the OODA daemon), which logs a warning and continues the cycle.

### `DiskHealthReport`

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct DiskHealthReport {
    pub disk_used_pct: u8,
    pub freed_bytes: u64,
    pub actions_taken: Vec<String>,
}
```

> **Implementation note:** The struct no longer derives `Deserialize`. Prior to
> issue #2212, the shim attempted to deserialize `DiskHealthReport` directly
> from stdout JSON. After the fix, the struct is built by
> `parse_disk_health_text()` from the extracted step output string — serde is
> used only for the `RecipeOutput` / `StepResult` envelope structs.

| Field           | Type           | Description                                       |
| --------------- | -------------- | ------------------------------------------------- |
| `disk_used_pct` | `u8`           | Current disk usage percentage (0–100)             |
| `freed_bytes`   | `u64`          | Total bytes freed during this check (0 if none)   |
| `actions_taken` | `Vec<String>`  | Human-readable list of cleanup actions performed  |

**Methods:**

- `cleanup_performed() → bool` — returns `true` if `freed_bytes > 0` or
  `actions_taken` is non-empty.
- `summary() → String` — one-line summary for daemon log. Format:
  `"disk health: N% used, freed M bytes, K actions"` or
  `"disk health: N% used, no cleanup needed"`.

### `RecipeOutput` (internal)

Serde struct for deserializing the `recipe-runner-rs --output-format json`
envelope. Not public — used internally by `run_disk_health_check()`.

```rust
#[derive(Deserialize)]
struct RecipeOutput {
    #[allow(dead_code)]
    success: bool,
    step_results: Vec<StepResult>,
}

#[derive(Deserialize)]
struct StepResult {
    #[allow(dead_code)]
    step_id: String,
    output: String,
}
```

The shim reads `step_results[0].output` — the first (and typically only)
step's output string. This contains the agent's raw text including its
reasoning, tool output, and the `DISK_USED_PCT` / `FREED_BYTES` / `ACTION:`
markers that `parse_disk_health_text()` extracts.

### `parse_disk_health_text(stdout) → Result<DiskHealthReport, String>`

Parses key=value text markers from the agent step output.

**Expected markers:**

```text
DISK_USED_PCT=72
FREED_BYTES=53687091200
ACTION: Removed 48 stale worktrees (50.1G)
ACTION: Cleaned cargo-target/ (12.0G) and shared-target/ (2.8G)
```

**Parsing rules:**

- `DISK_USED_PCT=N` — required. Must be a valid `u8`. Missing → error.
- `FREED_BYTES=N` — optional. Defaults to 0 if absent.
- `ACTION: text` — optional. Empty text after colon is skipped.
- Unknown lines are silently ignored (forward-compatible with agent noise).
- Leading/trailing whitespace on each line is trimmed.
- Blank lines are skipped.

The parser tolerates noisy agent output — LLM reasoning, `df` output, bash
prompts — because it only matches lines starting with the exact marker
prefixes. This is why the agent can freely reason and run commands as long as
it emits the markers somewhere in its output.

### `get_disk_usage_pct(path) → Option<u8>` (private)

Returns the disk usage percentage for the filesystem containing `path`.
Runs `df --output=pcent <path>`, parses the second line, strips the `%`
suffix. Returns `None` on any failure (command not found, parse error).

Used by `emergency_cleanup()` to check thresholds.

### `dir_size_bytes(path) → u64` (private)

Estimates directory size in bytes using `du -sb <path>`. Returns `0` on
any failure. Used by `emergency_cleanup()` to report freed space.

### `resolve_recipe_path(repo_root) → Option<PathBuf>`

Resolves the recipe YAML path. Checks in order:

1. **Hot-reload:** `~/.simard/prompt_assets/simard/recipes/disk-health-check.yaml`
2. **In-tree:** `<repo_root>/prompt_assets/simard/recipes/disk-health-check.yaml`

Returns `None` if neither path exists.

## Recipe invocation details

The shim invokes `recipe-runner-rs` with these arguments:

```
recipe-runner-rs <recipe_path> --output-format json -c state_root=<state_root>
```

- `--output-format json` — required. Without this flag, stdout only contains
  the summary line (`Recipe: disk-health-check SUCCESS`), not the step output.
- `-c state_root=<path>` — context variable passed to the recipe YAML.
- `AMPLIHACK_AGENT_BINARY` env var — set from `RuntimeConfig` so the recipe
  uses the correct agent binary.

## Daemon integration

The OODA daemon calls both tiers in sequence each cycle
(`src/operator_commands_ooda/daemon/mod.rs`):

```rust
// Tier 1: deterministic emergency cleanup (no LLM, no recipe)
if let Some(emergency_report) =
    crate::disk_health::emergency_cleanup(&bridges.repo_root, &state_root)
{
    daemon_log(&state_root, &format!(
        "[simard] EMERGENCY disk cleanup: {}% -> freed {} bytes",
        emergency_report.disk_used_pct, emergency_report.freed_bytes
    ));
}

// Tier 2: recipe-based LLM cleanup (moderate pressure, nuanced decisions)
match crate::disk_health::run_disk_health_check(&bridges.repo_root, &state_root) {
    Ok(report) => { /* log summary */ }
    Err(e) => { /* WARN and continue — never blocks OODA cycle */ }
}
```

**Key invariant:** Tier 1 always runs before Tier 2. If Tier 1 fires (≥95%),
it frees space so Tier 2's agent can spawn. If Tier 1 doesn't fire (<95%),
Tier 2 handles moderate pressure (≥80%). Both tiers return `DiskHealthReport`
with the same struct, so the daemon logging is uniform.

## Test coverage

The module has comprehensive inline tests:

| Category                    | Count | Description                                               |
| --------------------------- | ----- | --------------------------------------------------------- |
| `parse_disk_health_text`    | 12    | All marker combinations, edge cases, error paths          |
| `DiskHealthReport` methods  | 5     | `cleanup_performed()` (3 cases) and `summary()` (2 cases) |
| `resolve_recipe_path`       | 2     | Missing dir, in-tree recipe found                         |
| `run_disk_health_check`     | 2     | Recipe not found, runner unavailable/invalid recipe       |
| JSON envelope deserialization | 1   | Full pipeline: JSON → RecipeOutput → parse → report       |
| Noisy agent output          | 1     | Markers embedded in LLM conversation text                 |
| `truncate` helper           | 5     | Short, exact, long, empty, zero-max                       |

> **Note:** `emergency_cleanup()` is not unit-tested because it requires
> real filesystem state (`df`, `du`, `remove_dir_all`). It is exercised by
> the daemon's integration test suite and manual ENOSPC recovery scenarios.

## Why `--output-format json` and not text

The `recipe-runner-rs` text-format stdout contains only the recipe-level
summary line:

```
Recipe: disk-health-check SUCCESS
```

This summary line does not include the individual step outputs. The agent step
runs `df`, checks disk usage, performs cleanup, and emits `DISK_USED_PCT`
markers — but all of that output lives inside the step, not in the recipe
summary.

The `--output-format json` flag wraps each step's output in the JSON envelope,
making it accessible to the calling process. This is the same pattern used by
`stewardship::recipe_merge_judge`, which also needs to read structured data
from recipe step output.

## Related

- [Automated disk health (concept)](../concepts/automated-disk-health.md) — design rationale and two-tier architecture
- [Configure disk health check (how-to)](../howto/configure-disk-health-check.md) — operator guide
- [Base type adapters](./base-type-adapters.md) — adapter pattern context
