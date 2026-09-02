---
title: Brain introspection API
description: Rust API reference for Simard's periodic brain self-examination and memory-hygiene pass — the run_brain_introspection daemon hook, the BrainIntrospectionReport struct, the enforce_prune_cap pure bound, the typed BrainIntrospectionRecord + fail-closed read_verified_brain_introspection read path (issue #4968), resolve_recipe_path, the brain-introspection recipe contract, and the SIMARD_BRAIN_INTROSPECTION_* configuration knobs.
last_updated: 2026-07-29
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../architecture/brain-introspection.md
  - ../howto/configure-brain-introspection.md
  - ./disk-health-api.md
  - ./automatic-distillation-scheduler.md
  - ./ooda-brain-parse-failure-record.md
  - ./record-brain-introspection-self-audit-cli.md
---

# Brain introspection API

> Shipped in issue [#2419](https://github.com/rysweet/Simard/issues/2419).
> Hook: `src/brain_introspection.rs`; daemon wiring:
> `src/operator_commands_ooda/daemon/mod.rs`; recipe:
> `prompt_assets/simard/recipes/brain-introspection.yaml`; standing prompt:
> `prompt_assets/simard/brain_introspection.md`; prompt pin:
> `src/ooda_brain/prompt_store.rs`.

**Module:** `src/brain_introspection.rs`

!!! note "Typed-record read path (issue [#4968](https://github.com/rysweet/Simard/issues/4968))"
    The recipe no longer emits text markers the hook scrapes from
    `step_results[*].output`. As of #4968 the recipe's final ACT step calls the gated
    `simard cognition record-brain-introspection` verb, which writes a typed
    [`BrainIntrospectionRecord`](./record-brain-introspection-self-audit-cli.md#the-brainintrospectionrecord-schema);
    the hook reads it **fail-closed** via `read_verified_brain_introspection` (R1–R7).
    `parse_brain_introspection_text` and its marker grammar are **deleted**. The record
    verb, schema, and read matrix are specified in
    [Reference: record-brain-introspection / record-self-quality-audit](./record-brain-introspection-self-audit-cli.md).

The `brain_introspection` module is a periodic daemon hook that performs a
higher-level **brain self-examination + memory-hygiene** pass on its own
env-gated interval (default daily). It is the cadence-level introspection layer
that *uses* the existing per-cycle infra (distillation, statistics, sensory
prune) rather than duplicating it.

Each run:

1. **Brain health** — examines OODA brain decision quality (record-fallback
   rate, the `brain_lifecycle_decision` parse-failure rate from issue #2419,
   SIGTERM/degraded/quarantine events, cycles with 0/N succeeded actions) and
   surfaces anomalies and regressions versus a rolling baseline.
2. **Patterns** — mines recent episodes/cycle-reports for recurring failures,
   goal types that land vs. stall, and repeated tool/recipe errors.
3. **Optimize / prune (SAFE)** — performs only the non-discretionary
   `prune_expired_sensory` cleanup daemon-side (already-expired transient rows)
   and emits **capped prune *recommendations*** (`PRUNE_CANDIDATE`) for
   superseded / low-value / duplicate value-bearing memories. The cap bounds the
   recommendation count, not the expired-sensory cleanup. See
   [Safety model](#safety-model) for why destructive superseded deletes are a
   follow-up, not this increment.
4. **Consolidate** — runs additive distillation (`consolidate_episodes`),
   reusing the same pipeline the per-cycle scheduler drives.
5. **Output** — writes findings to a **GitHub issue** on `rysweet/Simard`
   (label `brain-introspection`, stable title for dedup) and records metrics to
   `self_metrics`. No snapshot repo doc is ever written (no-point-in-time-docs
   rule); the only durable repo doc is the
   [reference page](../architecture/brain-introspection.md).

This page is the executable contract. For design rationale (cadence choice, the
client-reachability finding, the increment split) see
[Brain introspection + memory hygiene](../architecture/brain-introspection.md).

---

## Data flow

```
daemon loop (interval gate, mod.rs)
       │   SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS elapsed
       ▼
run_brain_introspection(&*clients.memory, &repo_root, &state_root, None)
       │
       ├─ 1. mem.get_statistics()            → stats_before (RPC-backed)
       │       live_memories = non-sensory modality sum (working + episodic +
       │       semantic + procedural + prospective); record_metric(…)
       │
       ├─ 2. mem.prune_expired_sensory()?    → sensory_pruned
       │        (already-expired transient rows; non-discretionary TTL
       │         cleanup — NOT throttled by the cap)
       │
       ├─ 3. mem.consolidate_episodes(batch)?        → additive distillation
       │        mem.get_statistics()        → stats_after
       │        consolidated_facts = (semantic+procedural)_after
       │                             − (semantic+procedural)_before  (≥ 0)
       │
       ├─ 4. compute record_path = state_root/brain_introspection/record.json
       │        delete any stale file; capture invoke_start: SystemTime
       │        spawn recipe-runner-rs brain-introspection.yaml
       │        --output-format json
       │        -c state_root=… -c repo_path=… -c record_path=<abs>
       │        -c max_prune=<cap> -c baseline_runs=<n> -c stats=<json>
       │        (max_prune = enforce_prune_cap ceiling on *recommendations*)
       │             │
       │             ▼  recipe's final ACT step:
       │        simard cognition record-brain-introspection --record-path <abs> …
       │             │   (writes typed BrainIntrospectionRecord, 0o600)
       │             ▼
       │   read_verified_brain_introspection(record_path, invoke_start)  (R1–R7)
       │             │
       │             ├─ Ok(rec)  ⇒ recipe-owned fields (brain_health, patterns,
       │             │              regressions, prune_requested, issue_url)
       │             └─ Err(Rn)  ⇒ WARN + degrade (keep bounded hygiene; Ok report)
       │
       ├─ 5. record final metrics; prune_requested = min(record count, cap);
       │     consolidated_facts is the hook-measured delta; issue_url from record
       ▼
BrainIntrospectionReport { live_memories, sensory_pruned, consolidated_facts,
                          prune_requested, brain_health, patterns,
                          regressions, issue_url }
```

**Split of labor.** The **Rust hook (daemon-side)** owns the verified,
RPC-backed memory operations, the deterministic prune cap, and metric writes.
The **recipe (subprocess)** owns LLM judgment (brain-health analysis, pattern
mining, prune-candidate identification) and the GitHub-issue output. Recipe
agents cannot call `CognitiveMemoryOps` trait methods, so every real memory
operation runs in the hook; the recipe only *recommends*.

---

## Public API

### `run_brain_introspection(mem, repo_root, state_root, home_override) → SimardResult<BrainIntrospectionReport>`

Entry point, called from the daemon loop. Reads memory statistics, performs the
bounded safe hygiene (sensory prune + additive consolidation), spawns the
agentic recipe, reads the typed record it writes via
`read_verified_brain_introspection`, records metrics, and returns the report.

```rust
pub fn run_brain_introspection(
    mem: &dyn CognitiveMemoryOps,
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
) -> SimardResult<BrainIntrospectionReport>;
```

**Parameters:**

| Parameter       | Type                       | Description                                                                 |
| --------------- | -------------------------- | --------------------------------------------------------------------------- |
| `mem`           | `&dyn CognitiveMemoryOps`  | The daemon's memory client; the daemon passes `&*clients.memory`            |
| `repo_root`     | `&Path`                    | Repository root — used to locate the recipe YAML                            |
| `state_root`    | `&Path`                    | Simard state directory (`~/.simard`) — passed as a recipe context var       |
| `home_override` | `Option<&Path>`            | Test seam for `resolve_recipe_path` hot-reload resolution; `None` in prod   |

> **Signature delta from `disk_health`.** Unlike
> `run_disk_health_check` (a pure scheduler that holds no memory handle), this
> hook takes `mem: &dyn CognitiveMemoryOps` because the real memory operations
> (`get_statistics`, `prune_expired_sensory`, `consolidate_episodes`) run
> in-process over the client. The daemon already has `clients.memory` in scope,
> so the call site is `run_brain_introspection(&*clients.memory, …)`.

**Returns:** `SimardResult<BrainIntrospectionReport>`

**Errors:**

Steps 1–3 are memory RPCs; a failure there returns the underlying `SimardError`
(e.g. transport/`get_statistics`/`prune_expired_sensory`/`consolidate_episodes`
failures) **before** the recipe is ever spawned. Steps 4–5 (recipe spawn + typed
record read) return `SimardError::AdapterInvocationFailed`:

| Stage | Condition                          | Error reason                                            |
| ----- | ---------------------------------- | ------------------------------------------------------- |
| 1–3   | Memory RPC failed                  | underlying `SimardError` (transport/op-specific)        |
| 4     | Recipe YAML not found              | `"recipe file brain-introspection.yaml not found…"`     |
| 4     | `recipe-runner-rs` not on PATH     | `"recipe-runner-rs spawn failed: …"`                    |
| 4     | Recipe exited non-zero             | `"recipe exited with <code>: <stderr>"`                 |
| 5     | Record read failed R1–R7           | `"brain-introspection record R{n}: <reason>"` — see the [read matrix](./record-brain-introspection-self-audit-cli.md#the-fail-closed-read-matrix-r1r7) |

No fallback at the **read seam**: `read_verified_brain_introspection` never returns
a defaulted or partial record — any R1–R7 failure is a distinct typed `Err`. The
deterministic memory operations (steps 1–3) run **before** the recipe spawn, so a
recipe/record failure never voids the bounded hygiene already performed. Any error —
memory RPC or record read — is caught by `run_brain_introspection`'s **best-effort
outer contract**: it logs a `WARN`, keeps the bounded hygiene results, and returns
`Ok(report)` with the hook-measured fields and empty recipe fields, so the daemon loop
continues (mirroring the disk-health hook's fail-open behavior).

### `BrainIntrospectionReport`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainIntrospectionReport {
    /// Non-sensory live memory count (hook-measured; includes consolidated).
    pub live_memories: u64,
    /// Already-expired transient sensory rows actually removed daemon-side by
    /// `prune_expired_sensory` (non-discretionary TTL cleanup; NOT capped).
    pub sensory_pruned: usize,
    /// Facts/procedures added by consolidation, measured by the hook as the
    /// post−pre delta of (semantic + procedural) counts from `get_statistics`.
    /// Always hook-measured — never taken from the recipe record.
    pub consolidated_facts: u64,
    /// Number of value-bearing prune *candidates* the record reported,
    /// clamped to the cap (`min(record count, cap)`). Never auto-deleted.
    pub prune_requested: usize,
    /// Brain-health findings (>=1; e.g. "fallback rate 4.2% (baseline 1.1%)").
    pub brain_health: Vec<String>,
    /// Detected recurring patterns from episode/cycle-report mining.
    pub patterns: Vec<String>,
    /// Regressions vs. the rolling baseline (0..n).
    pub regressions: Vec<String>,
    /// URL of the created/updated brain-introspection issue, if emitted.
    pub issue_url: Option<String>,
}
```

| Field                | Type             | Description                                                       |
| -------------------- | ---------------- | ---------------------------------------------------------------- |
| `live_memories`      | `u64`            | Non-sensory live memory count (hook-measured; includes the consolidation delta) |
| `sensory_pruned`     | `usize`          | Already-expired transient sensory rows removed daemon-side (non-discretionary; **not** capped) |
| `consolidated_facts` | `u64`            | Hook-measured (semantic + procedural) delta from the consolidation pass; never taken from the record |
| `prune_requested`    | `usize`          | Count of value-bearing prune *candidates* reported by the record, clamped ≤ cap (never auto-deleted) |
| `brain_health`       | `Vec<String>`    | Brain-health summary lines (at least one required)               |
| `patterns`           | `Vec<String>`    | Recurring-pattern findings (may be empty)                        |
| `regressions`        | `Vec<String>`    | Regressions detected against the rolling baseline                |
| `issue_url`          | `Option<String>` | The GitHub issue the run created or updated, if any              |

**Methods:**

- `actionable() → bool` — `true` if any prune candidate, regression, or
  consolidation occurred (i.e. the run produced work or signal).
- `summary() → String` — one-line daemon-log summary. Format:
  `"brain introspection: L live memories, N health findings, M patterns, P prune candidates, S sensory pruned, C consolidated, issue=<url|none>"`.

### `enforce_prune_cap(requested, cap) → usize`

The pure, deterministic safety bound — extracted so it can be tested without
standing up memory. Clamps any requested prune count to the configured cap and
never exceeds it.

```rust
/// Returns `min(requested, cap)`. A `cap` of 0 always returns 0
/// (introspection performs zero prunes when the cap is disabled).
pub fn enforce_prune_cap(requested: usize, cap: usize) -> usize {
    requested.min(cap)
}
```

| `requested` | `cap` | result | rationale                          |
| ----------: | ----: | -----: | ---------------------------------- |
|           5 |    25 |      5 | under cap — honor request          |
|          25 |    25 |     25 | at cap — honor request             |
|          40 |    25 |     25 | over cap — clamp to cap            |
|          10 |     0 |      0 | cap disabled — prune nothing       |

This bound governs the **maximum number of value-bearing prune candidates** the
recipe may recommend — it is passed to the recipe as `-c max_prune=<cap>`, and
the recipe's returned count is clamped to it (`prune_requested = min(recipe
count, cap)`). It does **not** throttle `prune_expired_sensory`: that operation
removes only already-expired transient rows (past their TTL), which is
non-discretionary cleanup and therefore exempt from the cap. The SAFETY
invariant of issue #2419 is thus: *no single run recommends more than `cap`
value-bearing prunes, and the only daemon-side deletion is non-discretionary
expired-sensory cleanup.* (A future bounded destructive prune of value-bearing
memory — see [Safety model](#safety-model) — will also be clamped by this cap
once the backed-up server RPC lands.)

### `read_verified_brain_introspection(path, invoke_start) → SimardResult<BrainIntrospectionRecord>`

Reads the typed record the recipe's final ACT step wrote, **fail-closed** over the
full R1–R7 matrix (present/readable, well-formed JSON, schema pin, closed-type parse +
bounds, required-field validity, owner-only `0o600` permissions, freshness /
anti-replay). Each failure is a distinct typed `Err` — the reader **never** returns a
defaulted or partial record. Defined in `src/brain_introspection_record.rs`; the full
matrix, schema, and freshness model are specified in
[Reference: record-brain-introspection / record-self-quality-audit](./record-brain-introspection-self-audit-cli.md#the-fail-closed-read-matrix-r1r7).

### `resolve_recipe_path(repo_root, home_override) → Option<PathBuf>`

Resolves the recipe YAML path. Checks in order:

1. **Hot-reload:** `<home>/.simard/prompt_assets/simard/recipes/brain-introspection.yaml`
   (`home_override` overrides `$HOME` in tests).
2. **In-tree:** `<repo_root>/prompt_assets/simard/recipes/brain-introspection.yaml`

Returns `None` if neither path exists. `RECIPE_FILENAME` (`"brain-introspection.yaml"`)
and `ADAPTER_TAG` (`"brain-introspection"`) are module constants, matching the
disk-health module's layout.

---

## Recipe result contract (typed record)

The recipe's **final ACT step** calls the gated
`simard cognition record-brain-introspection` verb, which writes one typed
[`BrainIntrospectionRecord`](./record-brain-introspection-self-audit-cli.md#the-brainintrospectionrecord-schema)
(owner-only `0o600`) to the rail-supplied `record_path`. The record carries the
**recipe-owned** findings only:

```jsonc
{
  "schema": "brain-introspection/v1",
  "written_at_epoch": 1793558400,
  "brain_health":  ["fallback rate 4.2% (baseline 1.1%)"],  // ≥1 required (R5)
  "patterns":      ["…"],                                    // 0..n
  "regressions":   ["…"],                                    // 0..n
  "prune_candidates": ["…"],                                 // 0..n human-readable
  "prune_requested": 4,                                      // hook re-clamps to cap
  "issue_url": "https://github.com/rysweet/Simard/issues/5012"
}
```

**Field ownership rules:**

- `brain_health` — at least one non-empty line **required** (R5 fails closed
  otherwise). Bounded list; each element sanitized and byte-capped.
- `prune_requested` — the count of value-bearing candidates; the hook clamps it to
  `enforce_prune_cap` before storing it. The recipe *recommends*, it never deletes.
- `issue_url` — optional; the GitHub issue the run created or updated.
- `live_memories`, `sensory_pruned`, `consolidated_facts` — **not** in the record.
  The hook measures these from `get_statistics` / `prune_expired_sensory` /
  `consolidate_episodes` and merges them into the final report; they are never taken
  from the recipe. The old advisory `CONSOLIDATED_FACTS=` echo is removed.

There is no marker grammar and no `step_results[*].output` scraping — the recipe's
free-text prose is irrelevant; only the typed record is read back, fail-closed
([R1–R7](./record-brain-introspection-self-audit-cli.md#the-fail-closed-read-matrix-r1r7)).

---

## Recipe invocation details

The shim invokes `recipe-runner-rs` with:

```
recipe-runner-rs <recipe_path> --output-format json \
  -c state_root=<state_root> \
  -c repo_path=<repo_root> \
  -c record_path=<abs>          # state_root/brain_introspection/record.json \
  -c max_prune=<cap> \
  -c baseline_runs=<n> \
  -c stats=<json>
```

- `-c record_path=<abs>` — the absolute path the hook derives, **pre-truncates**
  (deletes any stale file), and captures `invoke_start` for, immediately before spawn.
  The recipe passes it straight through to `record-brain-introspection --record-path`.
- `-c stats=<json>` — the measured `CognitiveStatistics` (live counts, sensory
  pruned, consolidated) serialized to JSON, so the agentic pass reasons over
  **real** numbers the hook already gathered rather than re-deriving them.
- `-c baseline_runs=<n>` — the rolling-baseline window
  (`SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS`, default 7) the brain-health step
  compares against.
- `AMPLIHACK_AGENT_BINARY` env var — set from `RuntimeConfig` so the recipe uses
  the correct agent binary.

### Recipe steps (`brain-introspection.yaml`)

A **single comprehensive agent step** (`id: brain-introspection`) performs the five
analysis phases, mirroring the proven `disk-health-check.yaml` single-step pattern
(the LLM runs `bash`/`gh` internally rather than relying on multi-step recipe
sequencing), and then a **final ACT step** records the typed result:

| Phase | within the step   | contributes to the record        |
| ----- | ----------------- | -------------------------------- |
| 1     | brain-health      | `brain_health`, `regressions`    |
| 2     | patterns          | `patterns`                       |
| 3     | prune-recommend   | `prune_candidates`, `prune_requested` |
| 4     | consolidate       | (hook-measured; not in record)   |
| 5     | output            | `issue_url`                      |
| ACT   | record            | `simard cognition record-brain-introspection --record-path <abs> …` |

Phase 1 reads `~/.simard/metrics/metrics.jsonl` and the daemon log, computing
the record-fallback rate, the `brain_lifecycle_decision` parse-failure rate,
SIGTERM/degraded/quarantine counts, and 0-succeeded-action cycles, then compares
them to the baseline window. Phase 5 creates or **updates** a GitHub issue
(stable title ⇒ dedup, no spam) via `gh issue`. The final ACT step then calls the
gated record verb exactly once; the consolidation count is the hook's own post−pre
`get_statistics` delta, never a recipe-supplied number.

---

## Daemon wiring

The hook mirrors the disk-health / worktree-sweep periodic-task pattern in
`src/operator_commands_ooda/daemon/mod.rs`. Interval state is initialized
alongside the other periodic tasks:

```rust
// --- periodic brain introspection state (issue #2419) -----------------
let brain_introspection_interval_secs: u64 =
    crate::brain_introspection::interval_secs_from_env(
        std::env::var("SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS")
            .ok()
            .as_deref(),
    ); // daily by default; a valid 0 disables the pass
let mut last_brain_introspection = Instant::now(); // first run after one interval
daemon_log(
    &state_root,
    &format!(
        "[simard] OODA daemon: brain introspection interval = {brain_introspection_interval_secs}s"
    ),
);
// ----------------------------------------------------------------------
```

and the loop hook is added after the worktree sweep, gated by the tested
`should_run_introspection` helper:

```rust
// ── Periodic brain introspection + memory hygiene (issue #2419) ──
if crate::brain_introspection::should_run_introspection(
    last_brain_introspection.elapsed(),
    brain_introspection_interval_secs,
) {
    match crate::brain_introspection::run_brain_introspection(
        &*clients.memory,
        &clients.repo_root,
        &state_root,
        None,
    ) {
        Ok(report) => {
            daemon_log(&state_root, &format!("[simard] {}", report.summary()));
        }
        Err(e) => daemon_log(
            &state_root,
            &format!("[simard] WARN: brain introspection failed: {e}"),
        ),
    }
    last_brain_introspection = Instant::now();
}
// ----------------------------------------------------------------
```

- **`> 0` gate** — `should_run_introspection` returns `false` whenever the
  interval is `0`, disabling the pass entirely. The default is `86_400` (24h).
- Unlike disk-health, `last_brain_introspection` is **not** back-dated to fire
  on the first loop iteration — the first introspection runs one full interval
  after daemon start (it has nothing useful to say at t=0, and the baseline is
  empty).

---

## Configuration

All knobs are env-driven, read once at daemon start (matching the disk-health /
worktree-sweep pattern):

| Knob            | Env var                                      | Default | Notes                                                       |
| --------------- | -------------------------------------------- | ------: | ----------------------------------------------------------- |
| Cadence         | `SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS`   | `86400` | Seconds between runs; `0` = disabled                        |
| Safe-prune cap  | `SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE`       | `25`    | Absolute ceiling on the number of value-bearing prune *recommendations* per run (does not throttle expired-sensory cleanup) |
| Baseline window | `SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS`   | `7`     | Rolling number of prior runs used as the regression baseline |

The cap is an **absolute count**, not a percentage — simplest to reason about
and test. A percentage cap is a documented follow-up (it would be additive).

---

## Metrics

The hook writes to `~/.simard/metrics/metrics.jsonl` via
`self_metrics::record_metric(name, value, context)` (where `context` is a plain
`&str`). The metric names are:

| Metric                                | Value                                                                 |
| ------------------------------------- | --------------------------------------------------------------------- |
| `brain_introspection_live_memories`   | non-sensory live memory count at run start: `working + episodic + semantic + procedural + prospective` (sensory excluded, so transient churn doesn't move the baseline) |
| `brain_introspection_sensory_pruned`  | already-expired sensory rows removed (non-discretionary cleanup)       |
| `brain_introspection_prune_requested` | value-bearing prune candidates recommended (clamped ≤ cap)            |
| `brain_introspection_consolidated`    | hook-measured (semantic + procedural) delta from consolidation        |

These accumulate the rolling baseline the next run's brain-health step compares
against (via `self_metrics::query_metrics` / `recent_metrics`). The first run
finds no prior entries and records `brain_health: ["no prior baseline"]`.

---

## Safety model

The first increment performs **no destructive superseded/semantic deletes
daemon-side**. This is a deliberate consequence of a client-reachability fact,
not a missing feature:

The daemon's `clients.memory` is a `CognitiveMemoryClient` (a JSON-RPC IPC
client), **not** the in-process `LibraryCognitiveMemory`. Over that client:

- `prune_superseded()` uses the **default trait impl `Ok(0)` — a no-op**
  (`cognitive_memory/mod.rs`); only `LibraryCognitiveMemory` actually reclaims.
- `graph_stats()` returns the **empty default**.
- `backup_memory()` requires a `&dyn MemoryStore`, which does not exist
  daemon-side (the store lives in the client **server** process).

Therefore, calling `prune_superseded` in the daemon hook would delete nothing
while reporting success — a silent-degradation / hollow-success bug the codebase
explicitly guards against. The safe resolution:

- **Daemon-side, this increment:** only `prune_expired_sensory()` runs as a
  deletion (it is RPC-backed and removes only already-expired transient rows —
  non-discretionary TTL cleanup, so it is **exempt from the cap**, not throttled
  by it). `consolidate_episodes()` runs (RPC-backed, additive). Superseded /
  low-value / duplicate **value-bearing** memories become **`PRUNE_CANDIDATE`
  recommendations** in the GitHub issue, capped at `enforce_prune_cap`, and are
  reviewed before any deletion.
- **Follow-up (issue):** add `memory.prune_superseded` + `memory.backup` RPCs on
  the client **server** (where the store lives) to enable backed-up, bounded,
  reversible destructive prune of value-bearing memory. That destructive prune
  *will* be clamped by `enforce_prune_cap`. Until then the run is read-mostly and
  the only daemon-side deletion is non-discretionary expired-sensory cleanup.

**Invariants asserted by tests:**

- `enforce_prune_cap` never returns more than `cap`; `cap = 0` ⇒ `0`. It bounds
  the recipe's recommendation count (`-c max_prune`), not the sensory cleanup.
- No code path in the hook calls `prune_superseded` or a destructive
  semantic/procedural delete.
- `consolidate_episodes` is additive (distillation), never lossy.
- The pass is off when the interval is `0`; default cadence is daily; the cap is
  conservative (25).

---

## Test coverage

| Category                          | Description                                                                         |
| --------------------------------- | ----------------------------------------------------------------------------------- |
| `enforce_prune_cap`               | `req < cap`, `== cap`, `> cap`, `cap = 0` → never exceeds; `cap = 0` ⇒ 0; asserts the cap bounds the recipe's `-c max_prune` recommendation count |
| `run_brain_introspection`         | Stub `CognitiveMemoryOps` (InMemory transport, mirrors `memory_client/tests.rs`): asserts stats read, **unbounded** `prune_expired_sensory` call (no cap applied), `consolidated_facts` measured as the post−pre semantic+procedural delta, recipe spawn path; recipe-runner-missing → graceful WARN |
| `no_destructive_value_prune`      | Asserts no hook path calls `prune_superseded` or a destructive semantic/procedural delete (the only deletion is expired-sensory cleanup) |
| `resolve_recipe_path`             | Hot-reload (via `home_override`) vs. in-tree resolution; neither present → `None`    |
| `read_verified_brain_introspection` | One dedicated test per R1–R7 case (missing/unreadable, malformed JSON, schema mismatch, unknown-field/bounds break, empty `brain_health`, non-`0o600`/wrong-owner, stale/replayed mtime) against a real 0o600 temp fixture, plus a happy-path read yielding the correct recipe-owned fields |
| Rework-contract guard | `tests_rework_contract.rs` forbids `parse_brain_introspection_text` / `step_results` / `.output` scraping in `brain_introspection.rs` and requires `read_verified_*` + `record_path` |
| Daemon interval gating            | `0` disables; elapsed triggers — mirrors the disk-health interval test              |
| Prompt content-pin                | `embedded_fallback("brain_introspection.md")` is `Some`; prompt mentions brain-health, patterns, bounded-safe-prune, consolidation, gh-issue; recipe YAML content-pinned via `include_str!` |

---

## Increment boundary

**First increment (this page):** the safe read + bounded-sensory-prune +
additive-consolidate + recommend hook, the recipe, the prompt, the
[reference doc](../architecture/brain-introspection.md), and the tests above.

**Follow-ups (filed as issues):**

- **(A)** `memory.prune_superseded` + `memory.backup` client **server** RPCs to
  enable backed-up, bounded destructive superseded prune.
- **(B)** Percentage-based prune cap (additive to the absolute cap).
- **(C)** Restore-from-backup CLI for reversibility.
- **(D)** A metrics-dashboard panel for the `brain_introspection_*` series.

---

## Related

- [Brain introspection + memory hygiene (architecture)](../architecture/brain-introspection.md) —
  cadence rationale, client-reachability finding, safety model
- [Configure brain introspection (how-to)](../howto/configure-brain-introspection.md) —
  operator tuning, reading the issue, diagnosing failures
- [Disk health API](./disk-health-api.md) — the periodic-recipe hook pattern this mirrors
- [Automatic distillation scheduler API](./automatic-distillation-scheduler.md) —
  the per-cycle consolidation this pass reuses (and does not duplicate)
- [OODA brain parse-failure record](./ooda-brain-parse-failure-record.md) — the
  `brain_lifecycle_decision` / parse-failure signal the brain-health step reads
- [record-brain-introspection / record-self-quality-audit CLI](./record-brain-introspection-self-audit-cli.md) —
  the gated writer verb, `BrainIntrospectionRecord` schema, and R1–R7 read matrix (#4968)
