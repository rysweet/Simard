---
title: Self-quality-audit API
description: Rust API reference for Simard's recurring monthly self-quality-audit — the run_self_quality_audit daemon hook, the SelfQualityAuditReport struct, the interval_secs_from_env / should_run_self_audit pure functions, the read_last_run / write_last_run disk persistence, resolve_recipe_path, the monthly-self-quality-audit recipe contract, and the typed SelfQualityAuditRecord + fail-closed read_verified_self_quality_audit read path (issue #4968), plus the SIMARD_SELF_AUDIT_INTERVAL configuration knob.
last_updated: 2026-07-29
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../architecture/monthly-self-quality-audit.md
  - ../howto/configure-self-quality-audit.md
  - ./disk-health-api.md
  - ./brain-introspection-api.md
  - ./record-brain-introspection-self-audit-cli.md
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
Simard's own repository, reads the typed record the recipe writes fail-closed
into a structured `SelfQualityAuditReport`, and — uniquely among the periodic
tasks — **persists its last-run timestamp to disk** so a ~monthly cadence
survives daemon restarts.

!!! note "Typed-record read path (issue [#4968](https://github.com/rysweet/Simard/issues/4968))"
    The recipe no longer emits text markers (`AUDIT_COMPLETE=`, `PR_OPENED=`, …) the
    shim scrapes from `step_results[*].output`. As of #4968 the recipe's final ACT
    step calls the gated `simard cognition record-self-quality-audit` verb, which
    writes a typed
    [`SelfQualityAuditRecord`](./record-brain-introspection-self-audit-cli.md#the-selfqualityauditrecord-schema);
    the hook reads it **fail-closed** via `read_verified_self_quality_audit` (R1–R7).
    `parse_self_quality_audit_text` and its marker grammar are **deleted**. The record
    verb, schema, and read matrix are specified in
    [Reference: record-brain-introspection / record-self-quality-audit](./record-brain-introspection-self-audit-cli.md).

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
4. **Output** — calls the gated `record-self-quality-audit` verb writing the typed
   record the hook reads; the daemon logs a fire line and a completion line. No
   snapshot repo doc is ever written.

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
   ├─ record_path = state_root/self_quality_audit/record.json
   │       delete any stale file; capture invoke_start: SystemTime
   │
   ├─ spawn recipe-runner-rs <path> --output-format json
   │        -c state_root=… -c repo_path=… -c record_path=<abs>
   │        (env AMPLIHACK_AGENT_BINARY from RuntimeConfig)
   │             │
   │             ▼  recipe's final ACT step:
   │        simard cognition record-self-quality-audit --record-path <abs> …
   │             │   (writes typed SelfQualityAuditRecord, 0o600)
   │             ▼
   │        read_verified_self_quality_audit(record_path, invoke_start)  (R1–R7)
   │             │
   │             ├─ Ok(rec)  ⇒ build report
   │             └─ Err(Rn)  ⇒ AdapterInvocationFailed (propagates)
   ▼
SelfQualityAuditReport { waves_completed, prs_opened, prs_merged,
                         crusty_approved, crusty_unresolved, summary_line }
   │
   └─ daemon persists write_last_run(now_epoch)  ── on Ok AND Err
```

**Split of labor.** The **Rust hook** owns the interval gate, disk-backed
last-run persistence, subprocess spawn, the fail-closed typed-record read, and
logging. The **recipe (subprocess)** owns all LLM judgment: the five quality-audit
waves, the crusty proxy review loop, and the self-merge decisions.

---

## Public API

### `run_self_quality_audit(repo_root, state_root, home_override) → SimardResult<SelfQualityAuditReport>`

Entry point, called from the daemon loop. Resolves the recipe YAML, derives and
pre-truncates the record path, captures `invoke_start`, spawns `recipe-runner-rs`
with `--output-format json` and `-c record_path=<abs>`, then reads the typed record
the recipe wrote via `read_verified_self_quality_audit` (fail-closed R1–R7) and
returns the report.

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
| Record read failed R1–R7             | `"self-quality-audit record R{n}: <reason>"` — see the [read matrix](./record-brain-introspection-self-audit-cli.md#the-fail-closed-read-matrix-r1r7) |

No fallback. `read_verified_self_quality_audit` never returns a defaulted or partial
record — any R1–R7 failure is a distinct typed `Err`. If the recipe fails for any
reason (spawn, non-zero exit, or an invalid/missing record), the error propagates to
the caller (the OODA daemon), which logs `WARN: self quality-audit failed: …` and
continues the cycle. The daemon persists last-run regardless, on `Ok` and on `Err`.

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

Built from the typed [`SelfQualityAuditRecord`](./record-brain-introspection-self-audit-cli.md#the-selfqualityauditrecord-schema)
returned by `read_verified_self_quality_audit()` — a direct field mapping, not
scraped from step output. The `RecipeOutput` / `StepResult` envelope-scrape path used
by the old marker parser is deleted.

| Field               | Type          | Description                                                    |
| ------------------- | ------------- | ------------------------------------------------------------- |
| `waves_completed`   | `u32`         | `waves_completed` from the record (0–5; a value `> 5` is rejected at R4) |
| `prs_opened`        | `Vec<String>` | `prs_opened` URLs from the record                            |
| `prs_merged`        | `Vec<String>` | `prs_merged` URLs from the record                            |
| `crusty_approved`   | `Vec<String>` | `crusty_approved` URLs from the record                       |
| `crusty_unresolved` | `Vec<String>` | `crusty_unresolved` URLs from the record (open, need human)  |
| `summary_line`      | `String`      | The record's required non-empty `summary_line` (its absence fails R5, so this is a plain `String`, not `Option`) |

**Methods:**

- `summary() → String` — one-line summary for the daemon completion log,
  synthesized from the counts (distinct from the raw `summary_line` field, which
  is the agent's own record summary text). Format:
  `"self quality-audit: complete — N waves, X PRs opened, Y merged, Z crusty-unresolved"`.

---

## Record result contract (typed record)

The recipe's **final ACT step** calls the gated
`simard cognition record-self-quality-audit` verb, which writes one typed
[`SelfQualityAuditRecord`](./record-brain-introspection-self-audit-cli.md#the-selfqualityauditrecord-schema)
(owner-only `0o600`) to the rail-supplied `record_path`:

```jsonc
{
  "schema": "self-quality-audit/v1",
  "written_at_epoch": 1793558400,
  "waves_completed": 5,                       // 0..=5 (R4 rejects > 5)
  "prs_opened":       ["…/pull/5001", "…/pull/5002"],
  "prs_merged":       ["…/pull/5001"],
  "crusty_approved":  ["…/pull/5001"],
  "crusty_unresolved":["…/pull/5002"],
  "summary_line": "5 waves, 4 PRs opened, 3 merged, 1 crusty-unresolved"
}
```

`summary_line` is **required** and non-empty — its absence fails the read matrix at
R5 (replacing the old required `AUDIT_COMPLETE=<summary>` marker). Each URL list is
bounded and every element is sanitized and byte-capped. There is no marker grammar
and no `step_results[*].output` scraping: the recipe's free-text prose is irrelevant;
only the typed record is read back, fail-closed
([R1–R7](./record-brain-introspection-self-audit-cli.md#the-fail-closed-read-matrix-r1r7)).

---

## Recipe contract

**File:** `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`

A single `type: agent` step (mirrors `disk-health-check.yaml` /
`brain-introspection.yaml`). Standard
`name / description / version / author / tags / context / steps` format.

**Context vars** (passed via `-c`):

| Var           | Meaning                                                    |
| ------------- | ---------------------------------------------------------- |
| `state_root`  | `~/.simard` — logs/state live here                        |
| `repo_path`   | repository root of `rysweet/Simard` being audited         |
| `record_path` | absolute path (`state_root/self_quality_audit/record.json`) the hook derived, pre-truncated, and passes to the final `record-self-quality-audit` ACT step |

**Agent responsibilities:**

1. Run **five sequential SEEK→VALIDATE→FIX waves** invoking the amplihack
   `quality-audit` skill on `rysweet/Simard`, opening a PR for each validated fix.
2. For **each** PR, invoke the `crusty-old-engineer` skill as Ryan's proxy
   reviewer, looping **≤3 rounds** until satisfied (record the URL under
   `crusty_approved` on approval, or `crusty_unresolved` if still unsatisfied after
   3 rounds — leave the PR open).
3. **Self-merge** each PR that is crusty-approved AND CI-green (respect branch
   protection); track it under `prs_merged`.
4. **Final ACT step:** call `simard cognition record-self-quality-audit
   --record-path <record_path> --waves-completed <n> --summary-line "<summary>" …`
   exactly once, passing the accumulated PR / crusty URL lists. Its absence (no valid
   record) is a fail-closed read error, not a silent success.
5. Never write a snapshot/point-in-time doc — the output is PRs + the typed record.

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
pub mod self_quality_audit_record;   // typed record + read_verified_self_quality_audit (#4968)

#[cfg(test)]
mod self_quality_audit_tests;
```

---

## Unit tests

**File:** `src/self_quality_audit_tests.rs` (`#[cfg(test)]`)

The scheduling logic is covered by pure unit tests (no subprocess, no network),
and the typed-record read path by fixture-backed reader tests:

| Test | Asserts |
| --- | --- |
| `interval_secs_from_env` matrix | unset→default, valid→parsed, `"0"`→disabled, garbage→default |
| gate — too soon | `should_run_self_audit(elapsed < interval, interval)` is `false` |
| gate — due | `should_run_self_audit(elapsed >= interval, interval)` is `true` |
| gate — disabled | `should_run_self_audit(_, 0)` is `false` |
| persistence round-trip | `write_last_run` then `read_last_run` returns the same epoch |
| persistence — garbage | `read_last_run` on a non-numeric file returns `None` |
| simulated restart | after writing a *recent* epoch, the gate stays `false` (no immediate re-fire across a restart) |
| `read_verified_self_quality_audit` R1–R7 | one dedicated test per case (missing/unreadable, malformed JSON, schema mismatch, unknown-field/bounds break / `waves_completed > 5`, empty `summary_line`, non-`0o600`/wrong-owner, stale/replayed mtime) against a real 0o600 temp fixture |
| happy path | a valid record yields the correct `SelfQualityAuditReport` |
| rework-contract guard | `tests_rework_contract.rs` forbids `parse_self_quality_audit_text` / `step_results` / `.output` scraping in `self_quality_audit.rs` and requires `read_verified_*` + `record_path` |

---

## Related

- [Monthly self-quality-audit (architecture)](../architecture/monthly-self-quality-audit.md) — design rationale and safety model
- [Configure the monthly self-quality-audit (how-to)](../howto/configure-self-quality-audit.md) — operator guide
- [Disk health API](./disk-health-api.md) — the pure recipe-invoker shim this hook is modeled on
- [Brain introspection API](./brain-introspection-api.md) — the sibling periodic task whose pattern this reuses
- [record-brain-introspection / record-self-quality-audit CLI](./record-brain-introspection-self-audit-cli.md) — the gated writer verb, `SelfQualityAuditRecord` schema, and R1–R7 read matrix (#4968)
