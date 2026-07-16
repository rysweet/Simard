---
title: Overseer recipe-launch idempotency reference
description: >
  The launcher-level safety rail that makes the acting Overseer's
  `smart-orchestrator` recipe launches IDEMPOTENT per task signature. When a
  goal/signature stays blocked tick after tick, the Overseer no longer spawns a
  byte-identical `amplihack recipe run smart-orchestrator` process every cycle:
  before spawning, `AmplihackRecipeRunner::spawn` reaps finished runs, then
  suppresses a duplicate launch when a still-running run already exists for the
  same normalized `target_repo` + `task_description` signature, returning a
  shared handle instead. The in-flight registry is **process-wide**, so
  suppression survives the daemon rebuilding the Overseer on every tick — the
  cross-tick guarantee that actually closes #4125. Covers the `recipe_signature`
  normalization contract, the fail-closed reap-then-dedup order, the fail-visible
  `overseer::recipe` warning, the shared-handle probe semantics, and the
  injectable child-spawn seam.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../design/overseer.md
  - ./overseer-activity-feed.md
  - ./overseer-tick-details.md
  - ../concepts/overseer-root-cause-why.md
  - ../concepts/operational-autonomy-model.md
---

# Overseer recipe-launch idempotency reference

The acting **Overseer** drives fixes *outside* Simard's own OODA loop by
launching a `smart-orchestrator` workstream — the exact
`amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml -c
task_description=… -c target_repo=…` invocation an engineer runs by hand (see the
[Overseer design](../design/overseer.md)). That launch is handled by
`AmplihackRecipeRunner` in `src/overseer/launch.rs`.

## The duplication defect (#4125)

The Overseer already **dedups FAILURE-signature ISSUES** — it runs a
`gh issue search` before `create_issue` so a recurring failure does not open a
new issue every tick (`src/overseer/observer.rs`). Recipe **launches** had no
such guard.

`AmplihackRecipeRunner::spawn` keyed each run by the child PID
(`recipe-{child.id()}`) and **unconditionally** spawned a fresh
`amplihack recipe run` process on every call. The `runs` map was consulted only
by `probe`, never by `spawn`. So when a goal/signature stayed **blocked** across
ticks, the Overseer re-launched a byte-identical recipe every cycle for the same
work.

Observed **2026-07-16**: three byte-identical
`amplihack recipe run smart-orchestrator` processes for the **same** recurring
signature (the `kgpacks-rs` blocked goal), spawned **47 / 28 / 12** minutes
apart — three concurrent orchestrators racing on one task, wasting compute.

## The fix: per-signature idempotent launches

`AmplihackRecipeRunner::spawn` is now **idempotent per task signature**. A launch
for a signature that already has a **still-running** run does **not** spawn a
second process — it returns a handle to the existing run.

The guard lives at the **launcher level** (not only in the higher-level decision
logic) because that is the reliable, last-line rail: no matter how many times the
decision layer asks for a launch, at most one live process exists per signature.

#### Why the rail must be process-wide (the cross-tick requirement)

The reported defect is **cross-tick**: the daemon rebuilds the *entire* Overseer
— and therefore a fresh `AmplihackRecipeRunner` — on **every** meta-OODA tick
(`src/operator_commands_ooda/daemon/mod.rs` calls `crate::overseer::build_overseer`
inside the tick thread; default cadence **900 s**). The three duplicate
processes were **47 / 28 / 12 min apart == three separate ticks**.

A per-instance `runs` map is therefore empty at the start of every tick, so a
launcher-local rail could only ever dedup *within* one tick and would still spawn
a byte-identical duplicate on the next. The in-flight registry is consequently
**process-wide**: `AmplihackRecipeRunner::from_env` — the only production
constructor, used by both the daemon tick and the dashboard feedback endpoint —
shares **one** `runs` map (a `static OnceLock<Arc<Mutex<…>>>`) for the whole
process. Suppression survives the tick rebuild, and the daemon-tick and
dashboard launch paths now dedup against each other too. This process-scoped
durability is what actually closes #4125; the per-signature logic alone did not.
(The higher-level `inflight_investigations` set is still per-tick, so the
decision layer may re-decide to launch — but the launcher rail suppresses the
duplicate **process**, which is the observed harm.)

### The signature

The dedup key is derived by a pure, unit-tested function:

```rust
fn recipe_signature(brief: &RecipeBrief) -> String
```

It folds two fields of the `RecipeBrief` — `target_repo` and `task_description` —
into one stable key:

1. **Normalize** each field independently: `trim` → `to_lowercase` → collapse any
   run of whitespace to a single space.
2. **Join** the two normalized fields with a `\u{1F}` (ASCII Unit Separator)
   between them.

```
signature = normalize(target_repo) + "\u{1F}" + normalize(task_description)
```

Why this shape:

| Property | Reason |
| --- | --- |
| Includes `target_repo` **and** `task_description` | The same task text against a different repo is genuinely different work and must not be collapsed. |
| Case + whitespace normalized | Cosmetic differences (trailing spaces, re-wrapped lines, capitalization drift between ticks) must **not** defeat dedup. |
| `\u{1F}` field separator | A non-whitespace separator that normalization never emits, so field boundaries can't collide (`"a" + "bc"` vs `"ab" + "c"` stay distinct). |
| Raw signature stays internal | The full normalized signature is **never** routed to a shell, path, `Command`, SQL, URL, or JSON. It exists only to compute the handle token (below). |

#### The signature is not the handle id

The `runs` map and the returned `WorkstreamHandle.id` are keyed by a **bounded,
URL-safe handle token** — a short hex digest of the signature
(`sig_token = hex(hash(signature))`) — **not** the raw signature string.

This distinction is load-bearing, because `WorkstreamHandle.id` is **not**
internal. The dashboard feedback endpoint round-trips it through a URL path and
echoes it as JSON:

```rust
// src/operator_commands_dashboard/feedback.rs
let poll = format!("/api/feedback/status/{}", handle.id);   // id in a URL path
json!({ "workstream_id": handle.id, "poll": poll });        // id in the response
// …and back: WorkstreamHandle { id } is rebuilt from the path segment to poll.
```

The raw signature is unfit for that seam:

- `target_repo` (e.g. `rysweet/Simard`) contains `/`, which would break the
  `/api/feedback/status/{id}` path and make the id non-round-trippable.
- `task_description` carries whitespace, newlines, the `\u{1F}` control
  separator, and up to 8000 chars — none URL-safe.
- Echoing the raw signature as `workstream_id` would **leak the full brief text**
  into the dashboard JSON and the `wiring.rs` "launched workstream …" activity
  line — the exact leakage the suppressed-launch warning is bounded to avoid.

The hex `sig_token` is fixed-length, URL-safe, round-trippable, and reveals no
brief content. It is the **same** bounded token emitted in the fail-visible
warning below, so logs, the dashboard, and the dedup key all agree.

Because the same brief yields the same token, both the original caller and any
deduped caller receive the same `WorkstreamHandle.id` and therefore `probe` the
**same** run.

### The spawn sequence

On every `spawn(brief)` call, under the `runs` mutex:

1. **Reap finished runs.** `try_wait()` (via the `poll()` seam) each tracked run
   and **evict** an entry only when its child has **definitively exited**,
   unlinking its temp log at the same moment. A still-running child — or one whose
   state is momentarily **indeterminate** (a `poll` `Err`) — is **kept**. This is
   **fail-closed**: it matches the sibling `inflight_investigations` reconcile and
   guarantees a transient poll error can never let a byte-identical recipe
   relaunch. Suppression is still never *permanent*: a genuinely-completed run is
   freed here, so a new occurrence of the same signature *after* the prior run
   finished will spawn fresh.
2. **Dedup.** Compute `recipe_signature(brief)` and its `sig_token`. If a
   **still-running** entry exists for that token, **do not spawn**. Emit a visible
   warning (below) and return `WorkstreamHandle { id: sig_token }` pointing at the
   existing run.
3. **Spawn.** Otherwise spawn `amplihack recipe run …` exactly as before, insert
   the entry keyed by `sig_token`, and return `WorkstreamHandle { id: sig_token }`.

Reaping happens **before** the dedup check so that a completed run can never
suppress a legitimate re-launch.

### Fail-visible suppression (not silent)

A suppressed duplicate is logged — never silently dropped:

```rust
tracing::warn!(
    target: "overseer::recipe",
    signature = %sig_token,     // bounded hex digest, not the full brief text
    "duplicate recipe launch suppressed; reusing in-flight run"
);
```

- **Target** `overseer::recipe` so operators can filter for it.
- The logged `signature` is the same bounded `sig_token` (a hex digest) that is
  used as the handle id — not the full normalized `task_description` — so a
  suppressed-launch line does not leak brief content.
- This matches the Overseer's fail-visible discipline — the same reason the
  issue-dedup path logs rather than swallowing.

## Shared-handle probe semantics

`probe(handle)` is unchanged in logic: it looks up the run by `handle.id` (the
`sig_token`) and returns:

| Child state | `WorkstreamStatus` |
| --- | --- |
| Still running (`poll()` → `Ok(None)`) | `Running` |
| Exited **and** log contains a `…/pull/<n>` URL | `ProducedPr { repo, pr }` |
| Exited cleanly, no PR in log | `Failed { reason: "recipe finished but produced no PR" }` |
| Exited non-zero, no PR | `Failed { reason: "recipe exited with <status>" }` |
| `poll()` errored | entry **kept** (fail-closed); `probe` surfaces `OverseerError::Capability` |

Because a deduped caller holds the **same** `sig_token` id, it probes the shared
run and observes the same terminal status as the original caller. Reaping only
runs inside `spawn`, so a terminal status stays observable to a probing caller
until the next `spawn` for that signature reaps it.

## The child-spawn seam (testability)

To make the reap/dedup/spawn logic unit-testable **without** launching real
`amplihack` subprocesses, child creation is behind an injectable seam:

```rust
/// A non-blocking exit poll. `Ok(None)` = still running, `Ok(Some(_))` = exited.
trait SpawnedChild: Send {
    fn poll(&mut self) -> std::io::Result<Option<ChildExit>>;
}

/// Creates a child for a brief. Injected so tests don't spawn real amplihack.
trait ChildSpawner: Send + Sync {
    fn spawn(&self, brief: &RecipeBrief)
        -> std::io::Result<(Box<dyn SpawnedChild>, std::path::PathBuf)>;
}

/// Cross-platform-friendly domain exit (decouples tests from `ExitStatus`).
struct ChildExit {
    success: bool,
    description: String,
}
```

- **Production**: `RealChildSpawner` holds log-file creation, `Command`
  construction, `AMPLIHACK_AGENT_BINARY` inheritance, and `record_spawn_failure`
  behavior. The temp log is created **owner-only (0600)** on unix (it captures
  recipe stdout/stderr, which can carry tokens) and is **unlinked when its run is
  reaped**, so no orphaned secret-bearing logs accumulate in the temp dir.
  `RealChild` wraps `std::process::Child` and maps `try_wait()` into `ChildExit`.
- **Tests**: a `FakeChildSpawner` counts spawns and hands out `FakeChild`s whose
  exit is operator-controlled.

`probe` reads at most the final `MAX_PROBE_LOG_BYTES` (4 MiB) **tail** of the
child-written log rather than the whole file, bounding memory against an
unbounded/adversarial log while still catching the completion PR URL (printed at
the end of a run).

`AmplihackRecipeRunner` now holds the spawner and a **shared** in-flight registry:

```rust
pub struct AmplihackRecipeRunner {
    spawner: Box<dyn ChildSpawner>,               // Default/from_env = RealChildSpawner
    runs: Arc<Mutex<HashMap<String, RunEntry>>>,  // keyed by sig_token
}
```

`AmplihackRecipeRunner::from_env()` (the production path, via
`SmartOrchestratorLauncher::from_env`) shares the process-wide `runs` registry so
dedup survives the daemon's per-tick Overseer rebuild (#4125). `default()` keeps a
private map; test-only constructors inject a fake spawner (and, for the cross-tick
test, an explicitly shared registry).

## Behavior contract (worked examples)

Given `AmplihackRecipeRunner` wired to a fake spawner:

**1. Same brief while running → one process, shared handle.**

```
h1 = launch(brief)          // spawns process #1
h2 = launch(brief)          // FIRST run still Running → suppressed
assert spawn_count == 1
assert probe(h2) == probe(h1)   // both point at run #1
// a warning at target "overseer::recipe" was emitted for h2
```

**2. Re-launch after completion → fresh process (no permanent suppression).**

```
h1 = launch(brief)          // spawns process #1
<fake child #1 exits>
h3 = launch(brief)          // #1 reaped first → spawns process #2
assert spawn_count == 2
```

**3. Different briefs → no over-dedup.**

```
launch(brief_a)             // task_description "fix A"
launch(brief_b)             // task_description "fix B"
assert spawn_count == 2     // distinct signatures, distinct runs
```

Cosmetic-only differences do **not** count as different:
`"Fix A"`, `"fix a"`, and `"  fix   a "` all normalize to the same signature and
share one run.

**4. Handle id survives the dashboard round-trip (bounded, URL-safe).**

```
h = launch(brief)                       // id == sig_token, e.g. "a1b2c3d4e5f6…"
assert h.id.chars().all(url_safe)       // hex digest: no '/', whitespace, \u{1F}
// dashboard: GET /api/feedback/status/{h.id}  →  WorkstreamHandle { id: h.id }
assert probe(WorkstreamHandle { id: h.id }) == probe(h)   // rebuilt handle probes same run
// the emitted "workstream_id" / "launched workstream …" lines carry only the
// token — never the brief text
```

**5. Cross-tick rebuild → still one process (the #4125 case).**

```
// tick N and tick N+1 build SEPARATE runners that share the process-wide registry
tick1 = runner_sharing(registry)
tick2 = runner_sharing(registry)
h1 = tick1.launch(brief)     // spawns process #1
h2 = tick2.launch(brief)     // fresh runner, same registry, run #1 still Running → suppressed
assert spawn_count == 1
assert h1.id == h2.id
```

**6. Fail-closed reap → a transient poll error never double-launches.**

```
h1 = launch(brief)           // spawns process #1
<child #1's poll() now errors transiently>
h2 = launch(brief)           // reap KEEPS the erroring entry → suppressed
assert spawn_count == 1
```

## What did NOT change

- **The recipe invocation** (`smart_orchestrator_args`), its argv-safety bounding,
  the temp-log **capture channel**, `AMPLIHACK_AGENT_BINARY` inheritance, and
  `record_spawn_failure` on pre-exec failure — all preserved. (The temp log is now
  created 0600 and unlinked on reap, and `probe` reads a bounded tail — hardening
  only; the capture behavior itself is unchanged.)
- **`probe` result semantics** and the `RecipeRunner` / `RecipeLauncher` traits.
- **The higher-level decision logic.** The Overseer's `gate()` already holds a
  `LaunchRecipe` decision when an in-flight workstream is tracked
  (`inflight_investigations` + `recipe_dedup_key`); this fix adds the mandatory
  **launcher-level** rail beneath it. Both layers coexist: the decision layer
  avoids asking, and the launcher guarantees at-most-one-per-signature even if it
  does — and, being process-wide, does so **across ticks** where the per-tick
  decision layer resets. The two keys are **intentionally independent**: the
  decision layer's `recipe_dedup_key` (`src/overseer/mod.rs`) keys on
  `task_description` and its extracted `overseer-obs:` tag, while the launcher's
  `recipe_signature` folds `target_repo` + the full normalized `task_description`.
  They need not agree — each is a self-sufficient guard, and the launcher rail
  holds even when the decision layer's key would not have matched.

## Related

- [Overseer design](../design/overseer.md) — the acting Overseer's OODA loop and
  its capability seams.
- [Overseer activity feed reference](./overseer-activity-feed.md) — where launches
  and holds surface for operators.
- [Overseer tick details reference](./overseer-tick-details.md) — the
  per-tick human-readable action/observation lines.
- [Overseer root-cause ("WHY") principle](../concepts/overseer-root-cause-why.md) —
  the always-on rule to target causes, not symptoms.
