---
title: Self-quality-audit API
description: Rust API reference for Simard's recurring monthly self-quality-audit — the run_self_quality_audit daemon hook, the SelfQualityAuditReport struct, the interval_secs_from_env / should_run_self_audit pure functions, the read_last_run / write_last_run disk persistence, resolve_recipe_path, the monthly-self-quality-audit recipe contract and text markers, and the SIMARD_SELF_AUDIT_INTERVAL configuration knob.
last_updated: 2026-07-02
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../architecture/monthly-self-quality-audit.md
  - ../howto/configure-self-quality-audit.md
  - ./disk-health-api.md
  - ./brain-introspection-api.md
---

# Self-quality-audit API

> Follows the brain-introspection ([#2419](https://github.com/rysweet/Simard/issues/2419))
> periodic-task pattern.
> Hook: `src/self_quality_audit.rs`; daemon wiring:
> `src/operator_commands_ooda/daemon/mod.rs`; recipe:
> `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`; unit tests:
> `src/self_quality_audit_tests.rs`; module registration: `src/lib.rs`.

**Module:** `src/self_quality_audit.rs`

The `self_quality_audit` module is a thin Rust shim (a **pure recipe invoker**,
modeled on `disk_health.rs`) that fires on its own env-gated interval, spawns
`recipe-runner-rs` to run the five-wave, crusty-gated self-audit recipe against
Simard's own repository, deserializes the JSON envelope, parses text markers
into a structured `SelfQualityAuditReport`, and — uniquely among the periodic
tasks — **persists its last-run timestamp to disk** so a ~monthly cadence
survives daemon restarts.

Each run:

1. **Five waves** — drives five sequential SEEK→VALIDATE→FIX waves of the
   amplihack `quality-audit` skill against `rysweet/Simard`, each opening pull
   requests for validated fixes.
2. **Crusty proxy review** — for every resulting PR, invokes the
   `crusty-old-engineer` skill as operator Ryan's proxy reviewer, looping up to
   3 rounds until crusty is satisfied.
3. **Self-merge** — merges each PR that is both crusty-approved and CI-green
   (respecting branch protection); leaves crusty-unresolved PRs open for human
   follow-up.
4. **Output** — emits text markers parsed into the report; the daemon logs a
   fire line and a completion line. No snapshot repo doc is ever written.

This page is the executable contract. For design rationale (cadence choice, the
disk-persistence decision, the bounded crusty loop) see
[Monthly self-quality-audit](../architecture/monthly-self-quality-audit.md).

---

## Data flow

```
daemon loop (mod.rs)
   │  now_epoch = SystemTime::now() → unix seconds
   │  last_epoch = read_last_run(state_root/self_quality_audit_last_run)
   │             ├─ None/garbage ⇒ write_last_run(now); skip this cycle (init-to-now)
   │  elapsed = Duration::from_secs(now_epoch − last_epoch)
   │
   │  if should_run_self_audit(elapsed, SIMARD_SELF_AUDIT_INTERVAL) {
   ▼
run_self_quality_audit(&repo_root, &state_root, None)
   │
   ├─ resolve_recipe_path(repo_root, None)     → monthly-self-quality-audit.yaml
   │       (hot-reload path first, then in-tree)
   │
   ├─ spawn recipe-runner-rs <path> --output-format json
   │        -c state_root=… -c repo_path=…
   │        (env AMPLIHACK_AGENT_BINARY from RuntimeConfig)
   │             │
   │             ▼
   │     JSON envelope (stdout)
   │       { success, step_results: [{ step_id, output }] }
   │             │
   │             ▼
   │     serde_json::from_slice::<RecipeOutput>()
   │             │
   │             ▼
   │     step_results[*].output   (agent's raw text output)
   │             │
   │             ▼
   │     parse_self_quality_audit_text()   (marker parser)
   ▼
SelfQualityAuditReport { waves_completed, prs_opened, prs_merged,
                         crusty_approved, crusty_unresolved, summary_line }
   │
   └─ daemon persists write_last_run(now_epoch)  ── on Ok AND Err
```

**Split of labor.** The **Rust hook** owns the interval gate, disk-backed
last-run persistence, subprocess spawn, marker parsing, and logging. The
**recipe (subprocess)** owns all LLM judgment: the five quality-audit waves, the
crusty proxy review loop, and the self-merge decisions.

---

## Public API

### `run_self_quality_audit(repo_root, state_root, home_override) → SimardResult<SelfQualityAuditReport>`

Entry point, called from the daemon loop. Resolves the recipe YAML, spawns
`recipe-runner-rs` with `--output-format json`, deserializes the JSON envelope,
extracts each step's output, parses the markers, and returns the report.

```rust
pub fn run_self_quality_audit(
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
) -> SimardResult<SelfQualityAuditReport>;
```

**Parameters:**

| Parameter       | Type            | Description                                                                |
| --------------- | --------------- | -------------------------------------------------------------------------- |
| `repo_root`     | `&Path`         | Repository root — used to locate the recipe YAML; passed as `-c repo_path` |
| `state_root`    | `&Path`         | Simard state directory (`~/.simard`) — passed as `-c state_root`           |
| `home_override` | `Option<&Path>` | Test seam for `resolve_recipe_path` hot-reload resolution; `None` in prod  |

**Returns:** `SimardResult<SelfQualityAuditReport>`

**Errors** (`SimardError::AdapterInvocationFailed`):

| Condition                            | Error reason                                              |
| ------------------------------------ | -------------------------------------------------------- |
| Recipe YAML not found                | `"recipe file monthly-self-quality-audit.yaml not found…"` |
| `recipe-runner-rs` not on PATH       | `"recipe-runner-rs spawn failed: …"`                     |
| Recipe exited non-zero               | `"recipe exited with <code>: <stderr>"`                  |
| JSON deserialization failed          | `"failed to deserialize recipe JSON output: …"`          |
| Empty `step_results`                 | `"no step results in recipe JSON output"`                |
| Text markers missing `AUDIT_COMPLETE`| `"failed to parse recipe text output: …"`                |

No fallback. If the recipe fails for any reason, the error propagates to the
caller (the OODA daemon), which logs `WARN: self quality-audit failed: …` and
continues the cycle. The daemon persists last-run regardless.

---

### `interval_secs_from_env(raw) → u64`

Pure function that resolves the configured cadence from the
`SIMARD_SELF_AUDIT_INTERVAL` environment value.

```rust
pub fn interval_secs_from_env(raw: Option<&str>) -> u64;
```

| Input (`raw`)              | Result                     |
| -------------------------- | -------------------------- |
| `None` (unset)             | `DEFAULT_INTERVAL_SECS`    |
| `Some("")` (empty)         | `DEFAULT_INTERVAL_SECS`    |
| `Some("0")`                | `0` (**disabled**)         |
| `Some("<valid u64>")`      | that many seconds          |
| `Some("<garbage>")`        | `DEFAULT_INTERVAL_SECS`    |

`0` is the explicit disable value and is preserved; any unparseable value falls
back to the conservative default rather than disabling the task.

```rust
pub const DEFAULT_INTERVAL_SECS: u64 = 2_592_000; // ~30 days
```

> **Naming note.** The env var name is `SIMARD_SELF_AUDIT_INTERVAL` (value in
> **seconds**), matching the goal exactly, rather than the
> `SIMARD_*_INTERVAL_SECS` suffix used by the sibling periodic tasks. Single
> variable, no alias.

---

### `should_run_self_audit(elapsed, interval_secs) → bool`

Pure scheduling gate — the unit-tested heart of the cadence. Kept
signature-compatible with `should_run_introspection` so it tests the same way.

```rust
pub fn should_run_self_audit(elapsed: Duration, interval_secs: u64) -> bool {
    interval_secs > 0 && elapsed >= Duration::from_secs(interval_secs)
}
```

| `interval_secs` | `elapsed`            | Returns |
| --------------- | -------------------- | ------- |
| `0`             | any                  | `false` (disabled) |
| `> 0`           | `< interval`         | `false` (too soon) |
| `> 0`           | `>= interval`        | `true`  (fire)     |

In the daemon, `elapsed` is computed from the **wall-clock epoch delta**
(`now_epoch − last_run_epoch`), not a process-lifetime `Instant` — this is what
makes the cadence survive restarts.

---

### `read_last_run(path) → Option<u64>` / `write_last_run(path, epoch)`

Disk-backed last-run persistence — the one capability the other periodic tasks
lack. The last-run wall-clock timestamp is stored as decimal epoch **seconds**
in a single file at `{state_root}/self_quality_audit_last_run`.

```rust
pub fn read_last_run(path: &Path) -> Option<u64>;
pub fn write_last_run(path: &Path, epoch_secs: u64) -> std::io::Result<()>;
```

- `read_last_run` returns `Some(epoch)` when the file exists and parses as a
  `u64`; returns `None` when the file is absent **or** its contents are
  unparseable (garbage is treated as missing).
- `write_last_run` writes the epoch atomically and creates the parent directory
  if needed.

**Init-to-now contract.** On startup, `None` from `read_last_run` means the
daemon writes `now` and does **not** fire this cycle; the first audit fires
~one interval later (avoids an instant heavy audit on fresh deploy).

**Update-on-both contract.** The daemon calls `write_last_run(now_epoch)` after
every run attempt, on `Ok` and on `Err`, to prevent a failing recipe from
hot-looping.

---

### `resolve_recipe_path(repo_root, home_override) → Option<PathBuf>`

Private helper. Locates `monthly-self-quality-audit.yaml`, checking the
hot-reload path first, then the in-tree copy — **identical signature and
resolution order to `disk_health::resolve_recipe_path`** (the sibling this hook
is modeled on): a private `fn` returning `Option<PathBuf>`, not a public
`SimardResult`.

```rust
const RECIPE_FILENAME: &str = "monthly-self-quality-audit.yaml";

fn resolve_recipe_path(
    repo_root: &Path,
    home_override: Option<&Path>,
) -> Option<PathBuf>;
```

Resolution order:

1. **Hot-reload:** `{home}/prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`
   (`home` = `home_override` in tests, else `~/.simard`) — lets operators edit
   the recipe without a rebuild.
2. **In-tree:** `{repo_root}/prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`.

Returns `None` if neither path exists. The caller (`run_self_quality_audit`)
maps `None` to `SimardError::AdapterInvocationFailed` via `.ok_or_else(…)`,
exactly as `run_disk_health_check` does — the error conversion lives at the call
site, not in the resolver.

---

## `SelfQualityAuditReport`

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct SelfQualityAuditReport {
    pub waves_completed: u32,
    pub prs_opened: Vec<String>,
    pub prs_merged: Vec<String>,
    pub crusty_approved: Vec<String>,
    pub crusty_unresolved: Vec<String>,
    pub summary_line: String,
}
```

Built by `parse_self_quality_audit_text()` from the extracted step output — not
deserialized directly (serde is used only for the `RecipeOutput` / `StepResult`
envelope, as in `disk_health.rs`).

| Field               | Type          | Description                                                    |
| ------------------- | ------------- | ------------------------------------------------------------- |
| `waves_completed`   | `u32`         | Count of `WAVE_COMPLETE=<n>` markers observed (0–5)           |
| `prs_opened`        | `Vec<String>` | URLs from `PR_OPENED=<url>` markers                           |
| `prs_merged`        | `Vec<String>` | URLs from `PR_MERGED=<url>` markers                           |
| `crusty_approved`   | `Vec<String>` | URLs from `CRUSTY_APPROVED=<url>` markers                     |
| `crusty_unresolved` | `Vec<String>` | URLs from `CRUSTY_UNRESOLVED=<url>` markers (open, need human) |
| `summary_line`      | `String`      | Text from the required `AUDIT_COMPLETE=<summary>` marker (always present — its absence is a parse error, so this is a plain `String`, not `Option`) |

**Methods:**

- `summary() → String` — one-line summary for the daemon completion log,
  synthesized from the counts (distinct from the raw `summary_line` field, which
  is the agent's own `AUDIT_COMPLETE=` text). Format:
  `"self quality-audit: complete — N waves, X PRs opened, Y merged, Z crusty-unresolved"`.

---

## Text markers (recipe → hook)

The recipe emits plain-text markers (one per line, not inside code fences),
parsed by `parse_self_quality_audit_text()`. Any other line is ignored.

| Marker | Cardinality | Meaning |
| --- | --- | --- |
| `AUDIT_STARTED` | 1 | Audit began (advisory; daemon also logs its own fire line) |
| `WAVE_START=<n>` | 0..5 | Wave `n` (1–5) began |
| `WAVE_COMPLETE=<n>` | 0..5 | Wave `n` finished; counted into `waves_completed` |
| `PR_OPENED=<url>` | 0..n | A wave opened a pull request |
| `CRUSTY_APPROVED=<url>` | 0..n | crusty-old-engineer approved this PR |
| `CRUSTY_UNRESOLVED=<url>` | 0..n | crusty still unsatisfied after 3 rounds; PR left open |
| `PR_MERGED=<url>` | 0..n | PR self-merged (crusty-approved AND CI-green) |
| `AUDIT_COMPLETE=<summary>` | **1 (REQUIRED)** | Terminal summary; its absence is a parse error |

`AUDIT_COMPLETE` is **required** — the parser returns
`SimardError::AdapterInvocationFailed` if no non-empty `AUDIT_COMPLETE=` line is
present (mirrors the required-`BRAIN_HEALTH:`/`DISK_USED_PCT` contracts).

---

## Recipe contract

**File:** `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`

A single `type: agent` step (mirrors `disk-health-check.yaml` /
`brain-introspection.yaml`). Standard
`name / description / version / author / tags / context / steps` format.

**Context vars** (passed via `-c`):

| Var          | Meaning                                                    |
| ------------ | ---------------------------------------------------------- |
| `state_root` | `~/.simard` — logs/state live here                        |
| `repo_path`  | repository root of `rysweet/Simard` being audited         |

**Agent responsibilities:**

1. Run **five sequential SEEK→VALIDATE→FIX waves** invoking the amplihack
   `quality-audit` skill on `rysweet/Simard`; emit `WAVE_START=`/`WAVE_COMPLETE=`
   around each and `PR_OPENED=<url>` for each PR opened.
2. For **each** PR, invoke the `crusty-old-engineer` skill as Ryan's proxy
   reviewer, looping **≤3 rounds** until satisfied. Emit `CRUSTY_APPROVED=<url>`
   on approval or `CRUSTY_UNRESOLVED=<url>` if still unsatisfied after 3 rounds
   (leave the PR open).
3. **Self-merge** each PR that is crusty-approved AND CI-green (respect branch
   protection); emit `PR_MERGED=<url>`.
4. Emit a terminal `AUDIT_COMPLETE=<one-line summary>`.
5. Never write a snapshot/point-in-time doc — the output is PRs + markers.

---

## Daemon wiring

**File:** `src/operator_commands_ooda/daemon/mod.rs`

**Setup (once, at daemon start)** — alongside the verified-backup / disk-health /
worktree-sweep / brain-introspection setup blocks:

```rust
// --- periodic self quality-audit state (monthly, restart-surviving) ---
let self_audit_interval_secs =
    crate::self_quality_audit::interval_secs_from_env(
        std::env::var("SIMARD_SELF_AUDIT_INTERVAL").ok().as_deref(),
    );
let self_audit_last_run_path = state_root.join("self_quality_audit_last_run");
// init-to-now if absent/garbage: write now, don't fire this cycle
let mut last_self_audit_epoch = match
    crate::self_quality_audit::read_last_run(&self_audit_last_run_path) {
        Some(epoch) => epoch,
        None => {
            let now = /* unix seconds */;
            let _ = crate::self_quality_audit::write_last_run(
                &self_audit_last_run_path, now);
            now
        }
    };
daemon_log(
    &state_root,
    &format!("[simard] OODA daemon: self quality-audit interval = {self_audit_interval_secs}s"),
);
```

**Loop branch (each cycle):**

```rust
// ── Periodic self quality-audit (monthly, restart-surviving) ──
let now_epoch = /* SystemTime::now() → unix seconds */;
let elapsed = Duration::from_secs(now_epoch.saturating_sub(last_self_audit_epoch));
if crate::self_quality_audit::should_run_self_audit(elapsed, self_audit_interval_secs) {
    daemon_log(&state_root,
        "[simard] self quality-audit: starting 5-wave crusty-gated self-audit");
    match crate::self_quality_audit::run_self_quality_audit(
        &clients.repo_root, &state_root, None,
    ) {
        Ok(report) => daemon_log(&state_root, &format!("[simard] {}", report.summary())),
        Err(e) => daemon_log(&state_root,
            &format!("[simard] WARN: self quality-audit failed: {e}")),
    }
    // persist on Ok AND Err — prevents hot-loop on failure
    let _ = crate::self_quality_audit::write_last_run(&self_audit_last_run_path, now_epoch);
    last_self_audit_epoch = now_epoch;
}
```

**Startup log line (mirrors the other intervals):**

```
[2026-07-02T09:00:00Z] [simard] OODA daemon: self quality-audit interval = 2592000s
```

**Fire + completion log lines:**

```
[2026-08-01T09:00:01Z] [simard] self quality-audit: starting 5-wave crusty-gated self-audit
[2026-08-01T10:14:32Z] [simard] self quality-audit: complete — 5 waves, 4 PRs opened, 3 merged, 1 crusty-unresolved
```

---

## Module registration

**File:** `src/lib.rs`

```rust
pub mod self_quality_audit;

#[cfg(test)]
mod self_quality_audit_tests;
```

---

## Unit tests

**File:** `src/self_quality_audit_tests.rs` (`#[cfg(test)]`)

The scheduling logic is covered by pure unit tests (no subprocess, no network):

| Test | Asserts |
| --- | --- |
| `interval_secs_from_env` matrix | unset→default, valid→parsed, `"0"`→disabled, garbage→default |
| gate — too soon | `should_run_self_audit(elapsed < interval, interval)` is `false` |
| gate — due | `should_run_self_audit(elapsed >= interval, interval)` is `true` |
| gate — disabled | `should_run_self_audit(_, 0)` is `false` |
| persistence round-trip | `write_last_run` then `read_last_run` returns the same epoch |
| persistence — garbage | `read_last_run` on a non-numeric file returns `None` |
| simulated restart | after writing a *recent* epoch, the gate stays `false` (no immediate re-fire across a restart) |

---

## Related

- [Monthly self-quality-audit (architecture)](../architecture/monthly-self-quality-audit.md) — design rationale and safety model
- [Configure the monthly self-quality-audit (how-to)](../howto/configure-self-quality-audit.md) — operator guide
- [Disk health API](./disk-health-api.md) — the pure recipe-invoker shim this hook is modeled on
- [Brain introspection API](./brain-introspection-api.md) — the sibling periodic task whose pattern this reuses
