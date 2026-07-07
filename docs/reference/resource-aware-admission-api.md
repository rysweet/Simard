---
title: Resource-aware admission API reference
description: Rust API reference for the OodaAdmissionBrain trait, ResourceAdmissionCtx, the AdmissionDecision tagged enum, the pure resolve_admission hard rail and judge_and_resolve seam, context gathering, observability, and daemon wiring.
last_updated: 2026-07-07
owner: simard
doc_type: reference
related:
  - ../concepts/resource-aware-engineer-admission.md
  - ./ooda-resource-admission-recipe.md
  - ../howto/configure-resource-aware-admission.md
  - ./ooda-brain-api.md
  - ./recipe-brain-api.md
  - ./disk-health-api.md
  - ./adaptive-scaling-api.md
---

# Resource-aware admission API reference

**Module:** `src/ooda_brain/admission.rs`
**Seam:** `src/ooda_actions/advance_goal/spawn.rs` (`dispatch_spawn_engineer`)

Resource-aware admission adds an agentic gate on top of the
[AIMD count cap](./adaptive-scaling-api.md). Before a **fresh** engineer is
spawned, the daemon gathers a resource picture, asks the admission brain to
reason over it, and applies the resulting `AdmissionDecision` — subject to one
deterministic disk ceiling that the brain cannot override.

The module mirrors the Decide-phase template
(`OodaDecideBrain` + `DecideJudgment` + `DeterministicDecideBrain`, see
[`decide.rs`](./ooda-brain-api.md)): a single-decision-site trait, a tagged
judgment enum the LLM emits as a JSON envelope, and a deterministic floor brain
that needs no LLM.

---

## `ResourceAdmissionCtx`

The read-only resource snapshot fed to the brain. Every probed field is
`Option` — any probe that fails degrades to `None` (rendered as `"unknown"` in
the prompt) rather than panicking or blocking.

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResourceAdmissionCtx {
    /// Disk usage of the state/repo partition, 0–100. `None` if `df` failed.
    pub disk_usage_pct: Option<u8>,
    /// Total bytes under the engineer-worktree root
    /// (`<state_root>/<WORKTREES_SUBDIR>`, i.e. `engineer-worktrees/`).
    /// `None` if `du` failed.
    pub worktree_cache_bytes: Option<u64>,
    /// 1-minute load average from `/proc/loadavg`. `None` off Linux / on error.
    pub load_avg_1m: Option<f64>,
    /// Logical CPU count, from `std::thread::available_parallelism()`.
    /// Lets the prompt normalize load. `None` if unavailable.
    pub cpu_count: Option<usize>,
    /// Live engineer worktrees with a heartbeat claim (never `None`; 0 on error).
    pub in_flight_engineers: u32,
    /// The configured disk admission ceiling in effect this cycle (1–99).
    pub ceiling_pct: u8,
}
```

| Field | Type | Source | Degrades to |
|---|---|---|---|
| `disk_usage_pct` | `Option<u8>` | `df --output=pcent <path>` | `None` |
| `worktree_cache_bytes` | `Option<u64>` | `du -sb <state_root>/<WORKTREES_SUBDIR>` | `None` |
| `load_avg_1m` | `Option<f64>` | first field of `/proc/loadavg` | `None` |
| `cpu_count` | `Option<usize>` | `std::thread::available_parallelism()` | `None` |
| `in_flight_engineers` | `u32` | `count_live_engineer_claims` (`src/ooda_brain/context.rs`) | `0` |
| `ceiling_pct` | `u8` | `SIMARD_DISK_ADMISSION_CEILING_PCT` env (default 90, clamped 1–99) | default `90` |

`disk_usage_pct`, `worktree_cache_bytes`, and `in_flight_engineers` are the same
primitives already used by the [disk-health](./disk-health-api.md) and
engineer-lifecycle paths.

---

## `AdmissionDecision`

The tagged judgment the brain emits. Mirrors `DecideJudgment`: a closed enum
tagged on `choice`, each variant carrying human-readable `rationale`.

```rust
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum AdmissionDecision {
    Admit { rationale: String },
    Defer { rationale: String },
    ReclaimFirst { rationale: String },
}
```

Wire form (the JSON envelope the recipe agent produces):

```json
{"choice": "admit",         "rationale": "disk 61%, cache 8G, load 2.1 on 16 cpus — healthy"}
{"choice": "defer",         "rationale": "load 22 on 8 cpus; builds are thrashing, wait a cycle"}
{"choice": "reclaim_first", "rationale": "disk 88% but 30 stale worktrees are reclaimable"}
```

| Variant | `choice` tag | Meaning |
|---|---|---|
| `Admit` | `admit` | Proceed to worktree allocation and spawn. |
| `Defer` | `defer` | Benign skip — no worktree, no failure-count bump, retry next cycle. |
| `ReclaimFirst` | `reclaim_first` | Run the disk-health reclaim recipe, then defer this cycle. |

The enum is **closed**. An unknown `choice` tag or a missing `rationale` field is
a parse **error** (`SimardError::AdapterInvocationFailed`) — the parser never
silently defaults to `Admit`. Fail-loud on malformed model output; the seam then
falls back to the deterministic floor (below), where the hard rail still applies.

```rust
impl AdmissionDecision {
    /// The `choice` label, for metrics and logs (bounded cardinality).
    pub fn label(&self) -> &'static str { /* "admit" | "defer" | "reclaim_first" */ }

    pub fn rationale(&self) -> &str { /* the variant's rationale */ }
}
```

---

## `OodaAdmissionBrain` trait

Single-decision-site trait, synchronous to match the other OODA brain traits
(the LLM-backed impl bridges to async internally).

```rust
pub trait OodaAdmissionBrain: Send + Sync {
    /// Reason over the current resource picture and choose an admission outcome.
    fn judge_admission(&self, ctx: &ResourceAdmissionCtx) -> SimardResult<AdmissionDecision>;
}
```

### Implementations

| Impl | Where | Behavior |
|---|---|---|
| `RecipeBrain` | `src/ooda_brain/recipe_brain.rs` | Constructed via `RecipeBrain::new(repo_root, ADMISSION_RECIPE_NAME, "recipe-resource-admission-brain")` (see [Construction](#construction)), where `ADMISSION_RECIPE_NAME = "ooda-resource-admission.yaml"` (re-exported from `admission.rs`) and the adapter-tag literal `"recipe-resource-admission-brain"` mirrors the existing `DECIDE_ADAPTER_TAG` / `ORIENT_ADAPTER_TAG` / `LIFECYCLE_ADAPTER_TAG` naming. `judge_admission` invokes the recipe via `recipe-runner-rs`, reuses the shared decision-output chokepoint (`extract_recipe_decision_output` → `recipe_output::extract_json_payload`), deserializes the `{"choice":..,"rationale":..}` object directly into `AdmissionDecision`, and emits the `brain_admission_decision` metric. A parse miss (or any recipe-runner failure) surfaces as an explicit `Err` — **NO FALLBACK**. Production path. |
| `DeterministicAdmissionBrain` | `src/ooda_brain/admission.rs` | LLM-free floor. Always returns `Admit`. Used in tests and whenever no recipe brain can be constructed. Safe because the hard rail still guards ENOSPC. |

```rust
/// LLM-free floor. NOT a fallback hack — the explicit deterministic impl.
/// Always admits; the deterministic disk ceiling in `resolve_admission` is the
/// ENOSPC safety net, so a None/deterministic brain can never fill the disk.
#[derive(Debug, Default)]
pub struct DeterministicAdmissionBrain;

impl OodaAdmissionBrain for DeterministicAdmissionBrain {
    fn judge_admission(&self, _ctx: &ResourceAdmissionCtx) -> SimardResult<AdmissionDecision> {
        Ok(AdmissionDecision::Admit {
            rationale: "deterministic-brain: admit (hard rail guards disk)".into(),
        })
    }
}
```

---

## `AdmissionGate`, `resolve_admission`, `judge_and_resolve` — the pure gate + hard rail

The gate is split into a **pure, brain-free** rail (`resolve_admission`) and a
thin **reason→apply** wrapper (`judge_and_resolve`). Both are **zero-I/O**: the
reclaim side effect (invoking the disk-health recipe) is performed by the
**seam**, not by the gate, so the gate is fully hermetic in tests. The resolved
gate is a 3-variant enum — `Reclaim` is a distinct outcome the seam acts on,
not a closure folded into the gate.

```rust
/// The resolved admission outcome after the deterministic hard rail has had the
/// final say over the brain's `AdmissionDecision`. This is what the spawn seam
/// applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionGate {
    /// Admit: proceed with the fresh spawn (allocate worktree, spawn engineer).
    Proceed,
    /// Skip this cycle without allocating resources. Benign — no failure bump.
    Defer { reason: String },
    /// Run disk reclamation now (disk-health recipe), then defer this cycle.
    Reclaim { reason: String },
}

impl AdmissionGate {
    /// `true` only for `AdmissionGate::Proceed` — i.e. an actual admission.
    pub fn is_proceed(&self) -> bool { matches!(self, Self::Proceed) }
}

/// Apply the THIN deterministic hard rail over the brain's decision, then map it
/// to an `AdmissionGate`. Pure — takes the decision, never calls the brain.
///
/// Rail: if `ctx.disk_usage_pct` is `Some(p)` AND `p >= ctx.ceiling_pct`, an
/// `Admit` is forced to `Defer` regardless of the brain. Fail-open: a `None`
/// disk reading never triggers the rail. `Defer` / `ReclaimFirst` are already
/// non-admitting and pass through unchanged.
pub fn resolve_admission(ctx: &ResourceAdmissionCtx, decision: AdmissionDecision) -> AdmissionGate;

/// Seam core: reason → apply. Calls the brain for an `AdmissionDecision` and
/// resolves it through the hard rail into an `AdmissionGate`.
///
/// NO FALLBACK: a brain error propagates unchanged so the seam surfaces it as a
/// visible cycle failure (a broken brain must never look like a silent admit).
pub fn judge_and_resolve(
    brain: &dyn OodaAdmissionBrain,
    ctx: &ResourceAdmissionCtx,
) -> SimardResult<AdmissionGate>;
```

### Decision table

Let `over_ceiling = matches!(ctx.disk_usage_pct, Some(p) if p >= ctx.ceiling_pct)`.

| Brain result | `over_ceiling` | `resolve_admission` gate | Seam action |
|---|---|---|---|
| `Admit` | `false` | **Proceed** | allocate + spawn |
| `Admit` | `true` | **Defer** (hard-rail downgrade) | benign skip |
| `Defer` | any | **Defer** | benign skip |
| `ReclaimFirst` | any | **Reclaim** | run disk-health recipe, then benign skip (defer) |
| `Err(..)` (brain/parse failed) | — | — | **NO FALLBACK**: `judge_and_resolve` returns `Err`; the seam returns `success=false` (visible cycle failure), never a phantom admit |

Key invariants:

- A brain `Admit` can only be made **more** conservative by the rail, never less.
- The rail fires **only** on a *successful, over-ceiling* disk read
  (`Some(p) if p >= ceiling`). Unknown disk (`None`) never blocks.
- `ReclaimFirst` always defers *after* invoking reclaim — it never races a spawn
  against an in-flight cleanup.

---

## `gather_resource_admission_ctx` — best-effort probes

```rust
/// Build a `ResourceAdmissionCtx` with best-effort, never-panic probes.
/// Every probe that fails degrades its field to `None` (or `0` for the count).
pub fn gather_resource_admission_ctx(
    state_root: &Path,
    ceiling_pct: u8,
) -> ResourceAdmissionCtx;
```

| Probe | Command / call | Failure handling |
|---|---|---|
| disk % | `df --output=pcent <state_root>` (argv form) | parse fail → `None` |
| cache bytes | `du -sb <state_root>/<WORKTREES_SUBDIR>` (`WORKTREES_SUBDIR = "engineer-worktrees"`, `src/engineer_worktree/mod.rs`) | parse fail → `None` |
| load 1m | read `/proc/loadavg`, take field 0 | non-Linux / read fail → `None` |
| cpu count | `std::thread::available_parallelism()` | `Err` → `None` |
| in-flight | `count_live_engineer_claims(state_root)` (`src/ooda_brain/context.rs`) | already returns `0` on error |

> **Which root is measured.** The `du` probe sizes the daemon's *engineer*
> worktrees under `<state_root>/engineer-worktrees/` (the
> [`WORKTREES_SUBDIR`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/mod.rs)
> constant). This is a **different** directory from the one
> [`disk_health::emergency_cleanup`](./disk-health-api.md) reclaims
> (`<repo_root>/worktrees/*/target/`, `disk_health.rs`). The admission probe
> reports occupancy of the engineer-worktree area it is gating; RECLAIM-FIRST
> then delegates to the disk-health recipe, which cleans build-cache targets
> across both roots.

All subprocess probes are **argv-form** (`Command::new("df").arg(..)`), never
`sh -c`, so shell injection is structurally impossible. Probe paths are always
daemon-derived (state/repo root) — never goal- or LLM-supplied.

---

## `SIMARD_DISK_ADMISSION_CEILING_PCT` — the ceiling config

The only tunable, and the only hardcoded threshold in the feature.

```rust
/// Read the disk admission ceiling from the environment.
/// Default 90. Clamped to 1–99 (0 would deadlock; ≥100 would disable the rail).
pub fn configured_ceiling_pct() -> u8 {
    // parse_ceiling(std::env::var("SIMARD_DISK_ADMISSION_CEILING_PCT").ok().as_deref())
    // → best-effort parse; unparseable/absent → DEFAULT_CEILING_PCT (90);
    //   in-range values clamped to CEILING_MIN..=CEILING_MAX (1..=99).
}
```

| Env var | Default | Range | Notes |
|---|---|---|---|
| `SIMARD_DISK_ADMISSION_CEILING_PCT` | `90` | clamped to `1..=99` | Best-effort parse; unparseable → default. `0` and `≥100` are impossible after clamping. |

Read at the seam each cycle (mirroring the `SIMARD_SUBORDINATE_DEPTH` env read in
`spawn.rs`), so it takes effect without a rebuild or restart.

---

## Integration seam (`dispatch_spawn_engineer`)

The gate is inserted in the **fresh-spawn path** of `dispatch_spawn_engineer`:

- **after** the live-engineer lifecycle branch (re-attach is exempt),
- **after** the subordinate-depth recursion guard,
- **under** the AIMD count cap (the caller already holds its permit),
- **before** `EngineerWorktree::allocate` (so a defer grows nothing).

```rust
// … live-engineer branch returned above; depth guard passed …

let state_root = engineer_worktree_state_root();
let ceiling = configured_ceiling_pct();
let ctx = gather_resource_admission_ctx(&state_root, ceiling);

// `admission: &dyn OodaAdmissionBrain` is threaded in from the caller
// (`bridges.admission_brain`, falling back to `&DeterministicAdmissionBrain`
// when `None`). `repo_root: &Path` is threaded in for the reclaim recipe.
match judge_and_resolve(admission, &ctx) {
    Ok(AdmissionGate::Proceed) => {
        // Observability: record the admission at the seam.
        push_brain_judgment(BrainJudgmentRecord::from_admission(
            goal_id, "admit", "resources healthy; admitting fresh engineer", "",
        ));
        // fall through to allocate + spawn
    }
    Ok(AdmissionGate::Defer { reason }) => {
        // Benign skip: success=true, no worktree, no failure-count bump.
        push_brain_judgment(BrainJudgmentRecord::from_admission(goal_id, "defer", &reason, ""));
        return make_outcome(action, true, format!("deferred: resource pressure: {reason}"));
    }
    Ok(AdmissionGate::Reclaim { reason }) => {
        // Reclaim first (reuse the disk-health recipe), then defer this cycle.
        push_brain_judgment(BrainJudgmentRecord::from_admission(goal_id, "reclaim", &reason, ""));
        if let Err(e) = crate::disk_health::run_disk_health_check(repo_root, &state_root, None) {
            warn!(error = %e, "resource-admission reclaim failed (still deferring this cycle)");
        }
        return make_outcome(action, true, format!("deferred: reclaim-first: {reason}"));
    }
    Err(e) => {
        // NO FALLBACK: a broken admission brain surfaces as a visible cycle
        // failure (success=false → cycle.rs bumps the failure count), never a
        // silent phantom admit that could fill the disk.
        return make_outcome(action, false, format!("resource-admission brain failure: {e}"));
    }
}

// … EngineerWorktree::allocate(..) …
```

Both dispatch call sites — the concurrent dispatcher and the direct
`advance_goal` path — inherit the gate because it lives inside the shared
function.

### Why `Option<Arc<dyn OodaAdmissionBrain>>` on `OodaBridges`

The admission brain is added as `admission_brain`, matching the existing
`decide_brain` / `orient_brain` sibling fields:

```rust
pub struct OodaBridges {
    // … existing brains/bridges …
    pub decide_brain: Option<Arc<dyn OodaDecideBrain>>,
    pub orient_brain: Option<Arc<dyn OodaOrientBrain>>,
    pub admission_brain: Option<Arc<dyn OodaAdmissionBrain>>, // new
}
```

`Option` keeps construction churn minimal (defaults to `None`) and the seam
falls back to `DeterministicAdmissionBrain` when it is `None` — which remains
ENOSPC-safe because the hard rail is in `resolve_admission`, not in the brain.

### Construction

`src/operator_commands_ooda/daemon/brains.rs` gains `build_admission_brain`,
mirroring `build_act_brain`. Unlike `build_decide_brain` / `build_orient_brain`
(which return `Option`), admission **always** yields a concrete brain — the seam
must always have a reasoner to consult, and the deterministic floor + hard rail
keep behaviour safe even with no LLM. `RecipeBrain::new` takes **three** args —
`(repo_root, recipe_filename, adapter_tag)` — exactly like the decide/orient
builders:

```rust
pub(super) fn build_admission_brain(
    state_root: &Path,
    repo_root: &Path,
) -> Arc<dyn OodaAdmissionBrain> {
    if let Some(b) = RecipeBrain::new(
        repo_root,
        crate::ooda_brain::ADMISSION_RECIPE_NAME, // "ooda-resource-admission.yaml"
        "recipe-resource-admission-brain",
    ) {
        daemon_log(state_root, "[simard] OODA daemon: admission_brain = RecipeBrain …");
        return Arc::new(b);
    }
    // No recipe-runner-rs / recipe YAML: fall back — loudly, via record_fallback —
    // to the deterministic floor. The hard rail still guards ENOSPC.
    record_fallback(state_root, "admission", "recipe-runner-rs or recipe not available");
    Arc::new(DeterministicAdmissionBrain)
}
```

The production `OodaBridges` literal wires `admission_brain: Some(build_admission_brain(&state_root, &repo_root))`; tests and non-daemon callers leave it `None` (→ the seam's `DeterministicAdmissionBrain` floor).

---

## Observability

Every admission emits these signals, mirroring the lifecycle brain's telemetry
(two of them are metrics — see the reconciliation table below):

### 1. Judgment record — `BrainPhase::ResourceAdmission`

A new `BrainPhase` variant (`src/ooda_brain/judgment_record.rs`) with
byte-stable serde (`"resource_admission"`), plus a
`BrainJudgmentRecord::from_admission(goal_id, label, reason, prompt_version)`
constructor. `label` is the resolved gate label (`admit` / `defer` /
`reclaim`); `reason` is the brain's (or hard-rail's) rationale; the 4th arg —
`prompt_version: impl Into<String>` — mirrors `from_engineer_lifecycle` (the
seam passes `""` since the admission recipe is not registered in the prompt
store). Pushed via `push_brain_judgment` at each of the three seam arms
(Proceed / Defer / Reclaim), so cycle reports and
[brain introspection](./brain-introspection-api.md) show the final gate action.

```rust
pub enum BrainPhase {
    Act,
    Decide,
    Orient,
    MergeJudge,
    ResourceAdmission, // new
}
// BrainPhase::as_str(self): BrainPhase::ResourceAdmission => "resource_admission"
```

> **Adding this variant touches the exhaustive `match` on `BrainPhase`** — the
> enum's own `as_str` (`judgment_record.rs`). `parse_failure::phase_to_string`
> delegates to `as_str`, so there is effectively **one** phase-string site plus
> the byte-stability test (`mod.rs`), all agreeing on `"resource_admission"`.
> See [Add a New Recipe-Brain Phase](../howto/add-a-new-recipe-brain-phase.md).

### 2. Metric — `brain_admission_decision` (emitted by the recipe brain)

The recipe-backed admission path emits one `brain_admission_decision` event per
brain invocation, mirroring `brain_lifecycle_decision`
(`record_lifecycle_decision_metric`). `record_metric`'s signature is
`record_metric(name: &str, value: f64, context: &str)` — the third argument is a
**JSON `context` string**, not a struct — so the call passes a serialized
payload carrying the bounded-cardinality `choice` label (the brain's raw 3-way
`AdmissionDecision::label()`: `admit` / `defer` / `reclaim_first`) plus the
numeric resource picture. It is labeled by the enum + numbers only, never by
rationale or paths, and is a no-op under `cfg!(test)`:

```rust
// inside judge_admission, mirroring build_lifecycle_metric_context:
let context = serde_json::json!({
    "choice": decision.label(), // "admit" | "defer" | "reclaim_first"
    "disk_usage_pct": ctx.disk_usage_pct,
    "worktree_cache_bytes": ctx.worktree_cache_bytes,
    "load_avg_1m": ctx.load_avg_1m,
    "cpu_count": ctx.cpu_count,
    "in_flight_engineers": ctx.in_flight_engineers,
    "ceiling_pct": ctx.ceiling_pct,
})
.to_string();
let _ = record_metric("brain_admission_decision", 1.0, &context);
```

Because it fires in the brain (**before** the hard rail), it records the brain's
*intent* — so a `reclaim_first` is visible even though the seam's final gate
collapses to `Reclaim`→defer. The seam's judgment record (above) records the
*final* gate action after the rail.

> **Note.** Unlike the decide / orient / lifecycle brains, the admission brain
> does **not** route through the confidence-gated escalation ladder or the
> shared `brain_verdict_parsed_total` verdict-parse chokepoint. `judge_admission`
> parses the `{"choice":..,"rationale":..}` object directly (via
> `extract_json_payload` → `serde_json::from_str`); a parse miss is an explicit
> `Err` (NO FALLBACK), not a ladder retry. So there is **no**
> `brain_verdict_parsed_total{phase="resource_admission"}` series — the single
> admission metric is `brain_admission_decision`.

### 3. Tracing

A single structured line per admission (emitted at the seam — `info!` for
Proceed/Defer, `warn!` for Reclaim and brain-error), for example:

```
INFO simard::ooda_brain::admission: resource admission decided
    goal=improve-test-coverage
    choice=defer
    disk_pct=91 ceiling=90 cache_bytes=53687091200 load_1m=18.4 cpus=8 in_flight=12
    rationale="disk over ceiling — hard rail engaged"
```

The `rationale`/`reason` is untrusted model text. It is stored in the cycle
report only through serde (JSON-escaped, never shell-interpreted) and emitted as
a **structured** `tracing` field (not string-interpolated into a format that
could break log parsing). The bounded-cardinality metric
(`brain_admission_decision`) deliberately **excludes** it — it carries only the
`choice` label plus numeric fields — so untrusted text can never inflate metric
cardinality.

---

## Test inventory (hermetic)

All tests stub the brain — **no `recipe-runner-rs`, no LLM, no real `df`/`du`**
in the test path (the resource *reasoning quality* lives in the prompt, not the
tests). The pure gate + hard rail are the contract these pin.

**`src/ooda_brain/admission_tests.rs`** (27 hermetic tests via a stub brain):

| Test | Proves |
|---|---|
| `rail_allows_admit_below_ceiling` | `Admit` + disk < ceiling → `Proceed` |
| `rail_blocks_admit_at_ceiling_boundary` | `Admit` + disk == ceiling → `Defer` (`>=`, not `>`) |
| `rail_blocks_admit_above_ceiling` | `Admit` + disk > ceiling → `Defer` (reason cites disk + ceiling) |
| `rail_fails_open_when_disk_unknown` | `Admit` + `disk = None` → `Proceed` (rail does not fire) |
| `rail_never_proceeds_when_over_ceiling_for_any_decision` | over-ceiling ⇒ never `Proceed`, for EVERY decision |
| `defer_maps_to_defer_and_preserves_reason` / `reclaim_first_maps_to_reclaim_and_preserves_reason` | decision → gate mapping (FR-4) |
| `reclaim_first_honored_even_over_ceiling` | `ReclaimFirst` over ceiling → `Reclaim` (still non-admitting) |
| `seam_admit_decision_proceeds_when_healthy` / `seam_defer_decision_defers` / `seam_reclaim_decision_reclaims` | `judge_and_resolve` reason→apply seam |
| `seam_rail_overrides_brain_admit_when_over_ceiling` | rail overrides a stub `Admit` when over ceiling |
| `seam_surfaces_brain_error_no_fallback` | brain `Err` → `Err` (**NO FALLBACK**, never a phantom admit) |
| `parse_ceiling_*` / `configured_ceiling_is_in_range` | unparseable → 90; `0` → 1; `250` → 99; env read always in range |
| `decision_*_roundtrips` / `decision_ignores_extra_fields` | envelope `{"choice":..,"rationale":..}` ↔ enum, forward-compat |
| `decision_unknown_choice_fails_to_parse` | `{"choice":"boom"}` → parse `Err`, not `Admit` |
| `deterministic_brain_*` | floor always `Admit`, still beaten by the rail over ceiling |
| `ctx_roundtrips_including_unknown_probes` | `ResourceAdmissionCtx` serde incl. `None` probes |

**`src/ooda_brain/recipe_brain.rs`** (inline `#[cfg(test)]`):

| Test | Proves |
|---|---|
| `parse_admission_decision_reads_all_three_choices` | direct `{"choice":..}` → `AdmissionDecision` for admit/defer/reclaim |
| `parse_admission_decision_recovers_from_banner_and_fence` | banner + ```json fence still parses (shared chokepoint) |
| `parse_admission_decision_none_for_unparseable` | empty / garbage / unknown tag → `None` (→ seam `Err`) |
| `judge_admission_surfaces_error_no_fallback` | recipe-runner failure → `Err` naming the adapter tag (NO FALLBACK) |
| `admission_metric_context_carries_choice_and_numbers` | `brain_admission_decision` payload carries `choice` + numeric picture |

**`src/ooda_brain/mod.rs`** / **`context.rs`** / **`daemon/brains.rs`**:

| Test | Proves |
|---|---|
| `brain_phase_serializes_as_lowercase` (mod.rs) | `BrainPhase::ResourceAdmission` ↔ `"resource_admission"` + `as_str` |
| `admission_judgment_record_captures_gate` (mod.rs) | `from_admission` record shape + JSON round-trip |
| `gather_resource_admission_ctx_is_hermetic_and_preserves_ceiling` (context.rs) | missing root → no panic, ceiling preserved, cache `None` |
| `build_admission_brain_falls_back_to_deterministic_floor` (brains.rs) | unresolvable repo → loud fallback to the floor, which admits |

---

## Security

| Concern | Handling |
|---|---|
| Shell injection | All probes argv-form; no `sh -c`. Probe paths daemon-derived only. |
| Prompt injection | Context is numeric where possible (disk %, bytes, load, cpu, counts); any string context var passes `sanitize_context_var`. |
| Fail-loud parsing | `AdmissionDecision` is a closed `#[serde(tag="choice")]` enum; unknown tag / missing field → parse `Err`, never default-to-`Admit`. |
| Untrusted rationale | Stored only via serde (JSON-escaped) + emitted as a structured `tracing` field; never shell-interpreted or re-executed. Excluded from the metric so it cannot inflate cardinality. |
| Ceiling misconfig | `parse().ok().unwrap_or(90).clamp(1,99)` — never `unwrap`, never 0, never ≥100. |
| Availability invariant | known-over-ceiling ⇒ Defer; unknown ⇒ reasoner; never known-over-ceiling ⇒ Admit; never deadlock on unknown. |
| Metric cardinality | `brain_admission_decision` labeled by enum only, never rationale/paths. |

No authN/authZ: single-tenant, single-process, local daemon — no network
boundary is crossed, so none is invented. No data-at-rest, no secrets, no PII.

---

## Module layout

```
src/ooda_brain/
├── admission.rs          # OodaAdmissionBrain, ResourceAdmissionCtx, AdmissionDecision,
│                         #   AdmissionGate, DeterministicAdmissionBrain,
│                         #   resolve_admission (pure rail), judge_and_resolve (seam core),
│                         #   parse_ceiling / configured_ceiling_pct,
│                         #   RECIPE_NAME ("ooda-resource-admission.yaml")
├── admission_tests.rs    # 27 hermetic tests (stub brain): seam + hard rail
├── context.rs            # gather_resource_admission_ctx (best-effort probes)
├── recipe_brain.rs       # impl OodaAdmissionBrain for RecipeBrain; parse_admission_decision;
│                         #   brain_admission_decision metric (adapter tag literal)
├── judgment_record.rs    # BrainPhase::ResourceAdmission + BrainJudgmentRecord::from_admission
└── mod.rs                # mod admission; re-exports (ADMISSION_RECIPE_NAME = RECIPE_NAME)

src/ooda_actions/advance_goal/
└── spawn.rs              # gate seam in dispatch_spawn_engineer (Proceed/Defer/Reclaim/Err)

src/ooda_loop/
└── types.rs              # OodaBridges.admission_brain: Option<Arc<dyn OodaAdmissionBrain>>

src/operator_commands_ooda/daemon/
└── brains.rs             # build_admission_brain(..) -> Arc<dyn OodaAdmissionBrain>

src/disk_health.rs        # get_disk_usage_pct (pub(crate), reused); run_disk_health_check (reclaim)

prompt_assets/simard/recipes/
└── ooda-resource-admission.yaml   # the intelligence (see recipe reference)
```

---

## See also

- [Resource-aware engineer admission (concept)](../concepts/resource-aware-engineer-admission.md)
- [OODA resource-admission recipe & prompt schema](./ooda-resource-admission-recipe.md)
- [Configure resource-aware admission (how-to)](../howto/configure-resource-aware-admission.md)
- [`OodaBrain` API](./ooda-brain-api.md) — the sibling Decide/Act/Orient traits and the deterministic-floor pattern
- [Recipe-Brain API](./recipe-brain-api.md) — the `RecipeBrain` envelope-parse chokepoint and escalation ladder
- [Disk-Health API](./disk-health-api.md) — `run_disk_health_check` (RECLAIM-FIRST) and `emergency_cleanup` (≥95% tier)
- [Adaptive Scaling API](./adaptive-scaling-api.md) — the count cap this layers under
