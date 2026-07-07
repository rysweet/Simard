---
title: Resource-admission API reference
description: >
  Reference for the resource-aware engineer-admission gate — the
  ResourceAdmissionCtx / ResourceAdmissionDecision types, the
  OodaBrain::decide_resource_admission method, the gather→reason→apply seam and
  its deterministic disk-ceiling hard rail, the resource probes and their
  best-effort degradation, the disk_pressure classify() reuse, the
  SIMARD_DISK_ADMISSION_CEILING_PCT ceiling and SIMARD_RESOURCE_ADMISSION
  kill-switch, the reclaim invocation, the observability record and metric, and
  the hermetic test matrix.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/resource-aware-engineer-admission.md
  - ./ooda-resource-admission-recipe.md
  - ./adaptive-scaling-api.md
  - ./disk-health-api.md
  - ./engineer-admission-api.md
  - ./recipe-context-var-sanitization.md
  - ../howto/configure-resource-aware-admission.md
  - ../operations/resource-admission-kill-switch.md
  - ../../src/ooda_actions/advance_goal/spawn.rs
  - ../../src/ooda_actions/advance_goal/resource_admission.rs
  - ../../src/disk_pressure/check.rs
  - ../../src/ooda_brain/mod.rs
---

# Resource-admission API reference

> **Status: implemented.** This reference describes the shipped typed surface in
> present tense. The types, trait method, seam, hard rail, and kill-switch below
> live in
> [`src/ooda_actions/advance_goal/resource_admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/resource_admission.rs)
> and
> [`src/disk_pressure/check.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_pressure/check.rs)
> (wired into `dispatch_spawn_engineer` in
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs));
> `ResourceAdmissionCtx` / `ResourceAdmissionDecision` and the
> `OodaBrain::decide_resource_admission` method live in
> [`src/ooda_brain/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/mod.rs);
> and the reasoning asset at
> [`prompt_assets/simard/recipes/ooda-resource-admission.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/ooda-resource-admission.yaml).

This reference specifies the API for the resource-admission gate. For the
rationale, see
[resource-aware engineer admission](../concepts/resource-aware-engineer-admission.md).
The gate is a fourth instance of the brain-seam pattern already used by
[`decide_engineer_lifecycle`](ooda-engineer-lifecycle-recipe.md),
[`decide_engineer_admission`](engineer-admission-api.md), and
[`decide_goal_outcome_verification`](outcome-verification-api.md):
`Ctx → OodaBrain method → RecipeBrain(recipe.yaml) → typed Decision → apply`.

## Contents

- [`ResourceAdmissionCtx`](#resourceadmissionctx)
- [`ResourceAdmissionDecision`](#resourceadmissiondecision)
- [`OodaBrain::decide_resource_admission`](#oodabraindecide_resource_admission)
- [The seam and the hard rail](#the-seam-and-the-hard-rail)
- [The disk-ceiling rail (`disk_pressure`)](#the-disk-ceiling-rail-disk_pressure)
- [Resource probes](#resource-probes)
- [The reclaim invocation](#the-reclaim-invocation)
- [Configuration](#configuration)
- [Observability](#observability)
- [Kill-switch](#kill-switch)
- [Module layout](#module-layout)
- [Test matrix](#test-matrix)

## `ResourceAdmissionCtx`

The structured resource picture the brain reasons over. Assembled best-effort,
off the state lock, by `gather_resource_admission_ctx`. Every field that comes
from a probe is an `Option` and degrades to `None` on any error, so a failing
probe never fails the gate.

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResourceAdmissionCtx {
    /// Goal id the candidate engineer would pursue.
    pub goal_id: String,

    /// Filesystem used-percent on the engineer-worktree state-root filesystem,
    /// `(1 - free/total) * 100`. `None` if the stat failed.
    pub disk_used_pct: Option<f64>,
    /// Free / total space on that filesystem, in GiB (rounded), for the prompt.
    pub disk_free_gb: Option<f64>,
    pub disk_total_gb: Option<f64>,

    /// Aggregate bytes under the engineer-worktree root + shared build cache
    /// (best-effort directory walk; `None` if not computed this cycle).
    pub build_cache_bytes: Option<u64>,
    /// Number of engineer worktrees currently on disk under the state root.
    pub worktree_count: Option<u32>,

    /// System load average over 1 / 5 / 15 minutes (`/proc/loadavg` on Linux;
    /// `None` on non-Linux or on read failure).
    pub load_avg_1: Option<f64>,
    pub load_avg_5: Option<f64>,
    pub load_avg_15: Option<f64>,
    /// Logical CPU count (`available_parallelism`), for interpreting load.
    pub cpu_count: Option<u32>,

    /// Live-claimed engineers right now (in-flight builds), from
    /// `count_live_engineer_claims`. Typed `u32`; `0` when none.
    pub in_flight_engineers: u32,

    /// Current AIMD figures so the brain reasons about count and resources
    /// together. `None` when adaptive scaling is not active.
    pub aimd_current_max: Option<u32>,

    /// The resolved hard ceiling this cycle (echoed so the prompt knows the
    /// deterministic limit it is reasoning below). See
    /// [`SIMARD_DISK_ADMISSION_CEILING_PCT`](#configuration).
    pub admission_ceiling_pct: f64,
}
```

The candidate `goal_id` and any string rendered into the prompt are treated as
**untrusted** and pass through the recipe context-var
[sanitization boundary](recipe-context-var-sanitization.md) before templating.

## `ResourceAdmissionDecision`

The brain's decision. Internally serde-tagged on `choice`, `snake_case`, so an
**unknown tag fails to parse** (and the seam then fails closed — see below)
rather than silently defaulting.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum ResourceAdmissionDecision {
    /// The host has resource headroom — proceed (subject to the hard rail).
    Admit { rationale: String },
    /// Resources are tight — skip this cycle, retry next round (benign).
    Defer { rationale: String },
    /// Reclaim disk first (invoke the disk-health capability), then skip and
    /// retry next round against the freed space (benign).
    ReclaimFirst { rationale: String },
}
```

The enum has **no `Default`**. The fail-closed decision on a brain error is made
in the **seam**, not by defaulting the enum — see
[The seam and the hard rail](#the-seam-and-the-hard-rail). Every variant carries
a `rationale` that is recorded verbatim (scrubbed) in the judgment record and
metric.

### Parsing contract (NO-FALLBACK)

`RecipeBrain::decide_resource_admission` parses the recipe's JSON envelope. The
parse is deliberately strict (matching `parse_admission_decision`):

1. Extract the first fenced/bare JSON object and deserialize it.
2. If that fails, accept a bare first-word decision token
   (`admit` / `defer` / `reclaim_first`).
3. Otherwise return `Err` — the shim does **not** invent a decision on the
   brain's behalf. The seam turns that `Err` into a fail-closed `Defer`.

## `OodaBrain::decide_resource_admission`

A new method on the [`OodaBrain`](ooda-brain-api.md) trait, **defaulted** so
every existing impl and test double compiles unchanged:

```rust
pub trait OodaBrain: Send + Sync {
    // ... decide_engineer_lifecycle, decide_engineer_admission,
    //     decide_goal_outcome_verification ...

    /// Decide whether the host can afford to admit a NEW engineer right now,
    /// given the live resource picture (issue #2706). Called at the
    /// spawn/admission decision point for a genuinely NEW engineer, after the
    /// overlap-aware admission gate — repeated structured evaluation of "can the
    /// disk/build-cache/load take another engineer?".
    ///
    /// Defaulted to `Admit` so an un-migrated brain compiles and behaves like
    /// today BELOW the ceiling. This default is SAFE because the load-bearing
    /// ENOSPC guarantee is the deterministic disk-ceiling rail in the seam, not
    /// this method — a defaulting brain still cannot cross the ceiling. The
    /// production `RecipeBrain` overrides this to run the reasoning recipe.
    fn decide_resource_admission(
        &self,
        _ctx: &ResourceAdmissionCtx,
    ) -> SimardResult<ResourceAdmissionDecision> {
        Ok(ResourceAdmissionDecision::Admit {
            rationale: "resource-admission not implemented by this brain".into(),
        })
    }
}
```

> **Why default to `Admit`, not `Defer`?** Because the ENOSPC guarantee does not
> depend on this method. The deterministic rail blocks at the ceiling regardless
> of what the brain returns, so defaulting to `Admit` keeps un-migrated brains
> and test doubles behaving like today (admit below the ceiling) while the rail
> still guarantees safety. Fail-closed applies specifically to a brain *error*
> from the active recipe (the reasoning was supposed to run and broke), which the
> seam handles — not to a brain that simply hasn't implemented the method.

## The seam and the hard rail

The integration seam is a pure, hermetically testable function that gathers
context, calls the brain, and applies the deterministic rail. It lives in
`resource_admission.rs` and is called from `dispatch_spawn_engineer` between the
overlap gate and worktree allocation:

```rust
pub enum ResourceAdmissionOutcome {
    /// Proceed to worktree allocation + spawn.
    Admit,
    /// Skip this cycle, benignly (no worktree, no failure counted).
    Defer { detail: String },
    /// Reclaim disk first (best-effort), then skip this cycle benignly.
    ReclaimFirst { detail: String },
}

/// Gather → reason → apply. Pure except for the brain call; the disk stat is
/// injected via `DiskStatProvider` so the whole gate is hermetic.
pub fn run_resource_admission_gate<P: DiskStatProvider + ?Sized>(
    state_root: &Path,
    goal_id: &str,
    brain: &dyn OodaBrain,
    disk: &P,
) -> ResourceAdmissionOutcome;
```

The decision-to-outcome mapping is the whole apply step:

| Brain result | Hard rail (`disk_used_pct >= ceiling`) | `ResourceAdmissionOutcome` | `dispatch_spawn_engineer` action | `outcome.success` |
| --- | --- | --- | --- | --- |
| `Admit` | below ceiling | `Admit` | allocate worktree + spawn | (spawn's own outcome) |
| `Admit` | **at/above ceiling** | `Defer` (hard-rail override) | benign skip | `true` |
| `Defer` | — | `Defer` | benign skip | `true` |
| `ReclaimFirst` | — | `ReclaimFirst` | `run_disk_health_check` best-effort, then benign skip | `true` |
| `Err(_)` | — | `Defer` (fail-closed) + loud `error!` | benign skip | `true` |
| unknown disk stat | rail inert | brain's `Admit`/`Defer`/`ReclaimFirst` honored | as above | as above |

The caller mirrors the [overlap gate's](engineer-admission-api.md) two-arm match,
plus the reclaim arm:

```rust
match resource_admission::run_resource_admission_gate(
    &state_root, goal_id, brain, &RealDiskStatProvider,
) {
    ResourceAdmissionOutcome::Admit => { /* fall through to allocate + spawn */ }
    ResourceAdmissionOutcome::Defer { detail } => {
        eprintln!("[simard] spawn_engineer resource-deferred for '{goal_id}': {detail}");
        return make_outcome(action, true, detail); // benign — no failure count
    }
    ResourceAdmissionOutcome::ReclaimFirst { detail } => {
        // Best-effort reclaim; a reclaim error is warn-logged, never a cycle failure.
        if let Err(e) = crate::disk_health::run_disk_health_check(repo_root, &state_root, None) {
            tracing::warn!(target: "simard::resource_admission", error = %e,
                "reclaim_first: disk-health reclaim failed; deferring anyway");
        }
        eprintln!("[simard] spawn_engineer reclaim-first for '{goal_id}': {detail}");
        return make_outcome(action, true, detail); // benign — retry next cycle
    }
}
```

## The disk-ceiling rail (`disk_pressure`)

The hard rail is a pure classification over the injectable
[`DiskStatProvider`](disk-health-api.md), added beside the existing byte-based
`MIN_FREE_GB` policy in
[`src/disk_pressure/check.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_pressure/check.rs):

```rust
/// Used-percent of a filesystem from its `DiskStat`. `(1 - free/total) * 100`.
/// Returns `None` when `total_bytes == 0` (unknown disk ⇒ rail inert).
pub fn used_pct(stat: &DiskStat) -> Option<f64> {
    if stat.total_bytes == 0 {
        return None;
    }
    Some((1.0 - stat.free_bytes as f64 / stat.total_bytes as f64) * 100.0)
}

/// The deterministic ENOSPC guard. `true` ⇒ REFUSE admission regardless of the
/// brain. Pure over `(used_pct, ceiling_pct)` so it is unit-tested without
/// touching `statvfs`.
pub fn exceeds_admission_ceiling(used_pct: f64, ceiling_pct: f64) -> bool {
    used_pct >= ceiling_pct
}
```

The existing `PressureLevel::classify` and `MIN_FREE_GB` refuse-line are
**unchanged** — the ceiling rail is an additional, higher-level admission guard,
not a replacement for the byte-level allocation-site precheck.

> **The rail blocks; it does not reclaim.** An over-ceiling `Admit` is downgraded
> to a benign `Defer` — the rail deliberately does exactly one thing (block the
> irreversible action) and never triggers cleanup itself. Reclaiming a disk pinned
> over the ceiling is the job of the **periodic** [disk-health
> check](disk-health-api.md) — an independent daemon-loop interval
> (`SIMARD_DISK_HEALTH_INTERVAL_SECS`) with a deterministic emergency-cleanup tier
> that fires under high pressure — not of this gate. Keeping reclaim out of the
> rail is what keeps it a single, trivially auditable comparison. See the concept
> doc's [design decision](../concepts/resource-aware-engineer-admission.md#the-hard-rail-thin-deterministic-irreversible-only)
> for the full rationale.

## Resource probes

`gather_resource_admission_ctx` fills the context from these best-effort sources.
Each is absent-tolerant: on any error the field is `None`/`0` and the gate
proceeds on the remaining facts.

| Field | Source | Degrades to |
| --- | --- | --- |
| `disk_used_pct`, `disk_free_gb`, `disk_total_gb` | `DiskStatProvider::stat(state_root)` → `used_pct` | `None` (rail inert, gate admits on the rest) |
| `build_cache_bytes` | Best-effort walk of the engineer-worktree root + shared cargo target | `None` |
| `worktree_count` | Count of worktree dirs under the state root | `None` |
| `load_avg_1/5/15` | `/proc/loadavg` (Linux) | `None` (non-Linux) |
| `cpu_count` | `std::thread::available_parallelism()` | `None` |
| `in_flight_engineers` | [`count_live_engineer_claims(state_root)`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/context.rs) | `0` |
| `aimd_current_max` | `OodaConfig.scaler.current_max()` if present | `None` |
| `admission_ceiling_pct` | [`configured_admission_ceiling_pct()`](#configuration) | default `90.0` |

> **Cost note on `build_cache_bytes`.** A full recursive walk of the
> engineer-worktree root plus the shared cargo target is slowest exactly under the
> pressure this gate detects — the 40+-worktree incident is the worst case for the
> walk. Because the field is `Option` and the brain reasons fine without it (disk
> used-percent is the dominant signal and comes from a cheap `statvfs`), the walk
> should be **bounded or cached**, not run unconditionally on every hot-path
> admission: e.g. cap the traversal depth/time and degrade to `None` on timeout,
> or reuse the size the per-cycle [disk-health check](disk-health-api.md) already
> computes rather than re-walking here. Treat `disk_used_pct` as mandatory and
> `build_cache_bytes` as an opportunistic refinement.

## The reclaim invocation

`reclaim_first` invokes the **existing** [automated disk-health
capability](disk-health-api.md), not a new cleanup path:

```rust
crate::disk_health::run_disk_health_check(repo_root, state_root, None)
```

- `repo_root` locates the `disk-health-check` recipe YAML (hot-reload or
  in-tree). This is why `dispatch_spawn_engineer` threads the daemon's
  `repo_root` (from `OodaBridges`) to the gate — the reclaim recipe belongs to
  Simard, not to the candidate goal's target repository, so passing the
  goal's resolved `parent_repo` here would be wrong. In an installed daemon the
  hot-reload path (`~/.simard/prompt_assets/...`) resolves the recipe regardless
  of `repo_root`; `repo_root` is strictly load-bearing only for the **in-tree
  fallback** (dev / source runs). **Threading note:** `dispatch_spawn_engineer`
  currently takes `(action, state, goal_id, task, brain)` with **no** `repo_root`
  and there is no `AdvanceCtx` struct — adding `repo_root` is a signature change
  that touches **both** call sites ([`advance_goal/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/mod.rs),
  positional; [`concurrent.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/concurrent.rs),
  via its `ctx`). `aimd_current_max` has the same "new data at the dispatch point"
  dependency and must be threaded (or read from config) the same way; it degrades
  to `None` if unavailable, so it is optional where `repo_root` is required for
  reclaim.
- The call is **best-effort**: a reclaim error is `warn`-logged and the cycle
  still defers benignly (`success = true`). A failed reclaim must never become a
  goal failure.
- After reclaiming, the gate **does not** re-admit in the same cycle — it defers,
  and the next OODA round re-evaluates against the freed space. This keeps the
  side effect (a shell-out that may take minutes) off the hot admission path.

## Configuration

| Variable | Default | Range | Effect |
| --- | --- | --- | --- |
| `SIMARD_DISK_ADMISSION_CEILING_PCT` | `90.0` | clamped to `1..=99` | The deterministic hard-rail ceiling. When the worktree filesystem is at or above this used-percent, admission is refused regardless of the brain. |
| `SIMARD_RESOURCE_ADMISSION` | (unset ⇒ enabled) | `off` disables | Kill-switch for the **reasoning** gate. See [below](#kill-switch). |

```rust
/// Resolve the ceiling from SIMARD_DISK_ADMISSION_CEILING_PCT, clamped to
/// [1.0, 99.0]. Unparseable / out-of-range values fall back to the default
/// (90.0) with a WARN so a typo is visible.
pub fn configured_admission_ceiling_pct() -> f64;
```

The clamp guarantees the ceiling can never be set to `0` (which would refuse all
admission) or `100` (which would neutralize the guard entirely) by a typo. To
*effectively* neutralize the reasoning while keeping the hard guard, set the
ceiling high (e.g. `99`) rather than removing it.

## Observability

Every admission emits its reasoning — never a bare boolean. All four outcomes
(admit, defer, reclaim-first, brain-error) produce a record; `admit` is **not**
silent.

- A `BrainJudgmentRecord` (phase
  [`BrainPhase::ResourceAdmission`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/judgment_record.rs),
  string tag `resource_admission`) is pushed via `push_brain_judgment`, carrying
  the decision label, the scrubbed rationale, and the key resource figures
  (`disk_used_pct`, `in_flight_engineers`, `ceiling_pct`). The hard-rail override
  and the fail-closed brain-error defer are recorded with `fallback = true` so a
  deterministic block is as observable as a brain decision.
- A `resource_admission_decision` metric is appended to `metrics.jsonl` via
  [`self_metrics::record_metric`](telemetry-metrics.md), whose numeric `value`
  carries the `disk_used_pct` at decision time (`-1.0` when the disk stat is
  unknown, so an unknown reading is distinguishable from a genuine `0%`) and whose
  `context` carries the decision label, `worktree_count`, `in_flight_engineers`,
  and the ceiling. Encoding the disk figure as the metric `value` lets
  `simard metrics query` chart admission pressure over time without parsing the
  context string.

```rust
pub const RESOURCE_ADMISSION_DECISION_METRIC: &str = "resource_admission_decision";
```

See [How to configure resource-aware admission](../howto/configure-resource-aware-admission.md#diagnosing-a-resource-deferred-spawn)
for reading these back.

## Kill-switch

`SIMARD_RESOURCE_ADMISSION=off` (case-insensitive, read once at daemon boot)
disables the **reasoning** gate: no gather, no brain call — every candidate skips
straight to the hard rail. Crucially, the **deterministic disk-ceiling rail and
the byte-level `MIN_FREE_GB` precheck still run**, so disabling the reasoning
never disables the ENOSPC guarantee. Only the exact value `off` disables the
reasoning; unknown values keep it enabled. See the
[kill-switch operations page](../operations/resource-admission-kill-switch.md).

```rust
pub fn resource_admission_enabled() -> bool; // false only for SIMARD_RESOURCE_ADMISSION=off
```

> **"Read once at boot" is a memoization contract, not a free property.** The
> operator guarantee that changing `SIMARD_RESOURCE_ADMISSION` mid-run has no
> effect holds **only** if `resource_admission_enabled()` caches the parse in a
> `OnceLock`/`LazyLock` (as the sibling switches do). Implemented as a bare
> `std::env::var` read, it would be evaluated per gate invocation — still
> constant in practice for a stable environment, but no longer a boot-time
> snapshot. Memoize it so the operations page's "restart to change" instruction
> is literally true.

## Module layout

```
src/ooda_actions/advance_goal/
├── spawn.rs              # dispatch_spawn_engineer: calls run_resource_admission_gate
│                         #   after the overlap gate, before worktree allocation
├── admission.rs          # overlap-aware gate (#2690) — unchanged sibling
└── resource_admission.rs # ResourceAdmissionOutcome, run_resource_admission_gate,
                          #   gather_resource_admission_ctx, kill-switch, apply logic
src/disk_pressure/
├── check.rs              # + used_pct(), exceeds_admission_ceiling() (pure rail)
└── mod.rs                # + configured_admission_ceiling_pct()
src/ooda_brain/
├── mod.rs                # ResourceAdmissionCtx, ResourceAdmissionDecision,
│                         #   OodaBrain::decide_resource_admission (defaulted)
├── recipe_brain.rs       # RecipeBrain::decide_resource_admission + parse
├── context.rs            # gather helpers reused (count_live_engineer_claims)
└── judgment_record.rs    # BrainPhase::ResourceAdmission, from_resource_admission
prompt_assets/simard/recipes/
└── ooda-resource-admission.yaml   # the reasoning asset (hot-reloadable)
```

## Test matrix

All tests are **hermetic** — a stub `OodaBrain` and a fake `DiskStatProvider`
(synthetic `(free, total)`), no real `statvfs`, no `recipe-runner-rs`, no
subprocess. The reasoning *quality* lives in the prompt and is not unit-tested;
the tests prove the **seam** and the **hard rail**.

| Test | Asserts |
| --- | --- |
| `hard_rail_overrides_admit` | Stub brain returns `Admit`; fake disk at `95% ≥ 90` → outcome is `Defer` (hard-rail override), `success = true`, no spawn. |
| `hard_rail_inert_below_ceiling` | Stub brain `Admit`; fake disk at `70%` → outcome is `Admit` (proceed). |
| `brain_error_fails_closed` | Stub brain returns `Err` → outcome is `Defer`, loud error logged, `success = true`. Never `Admit`. |
| `defer_is_benign` | Stub brain `Defer` → `success = true`; a separate cycle-loop test confirms `goal_failure_counts` is **not** incremented and the scaler is **not** signalled. |
| `reclaim_first_invokes_reclaim_then_defers` | Stub brain `ReclaimFirst` → reclaim hook invoked once, then `Defer`, `success = true`. Reclaim error → still `Defer`. |
| `unknown_disk_admits_on_reasoning` | Fake provider returns `total = 0` → rail inert; brain `Admit` honored. |
| `ceiling_env_parse_and_clamp` | `configured_admission_ceiling_pct()` parses valid, clamps `0`/`100`, defaults on garbage. |
| `default_brain_admits` | The defaulted trait method returns `Admit`; hard rail still fires at the ceiling. |
| `kill_switch_off_skips_reasoning_keeps_rail` | `SIMARD_RESOURCE_ADMISSION=off` → no brain call, but disk at `95%` still `Defer`s via the rail. |

## See also

- [Resource-aware engineer admission (concept)](../concepts/resource-aware-engineer-admission.md) — design rationale.
- [OODA resource-admission recipe & prompt schema](ooda-resource-admission-recipe.md) — the reasoning asset.
- [How to configure resource-aware admission](../howto/configure-resource-aware-admission.md) — operator guide.
- [Resource-admission kill-switch](../operations/resource-admission-kill-switch.md) — `SIMARD_RESOURCE_ADMISSION`.
- [Adaptive scaling API](adaptive-scaling-api.md) — the AIMD count control this augments.
- [Disk health API](disk-health-api.md) — the reclaim capability and `disk_pressure` plumbing this reuses.
- [Engineer-admission API](engineer-admission-api.md) — the sibling overlap gate at the same seam.
