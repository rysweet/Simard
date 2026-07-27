---
title: Engineer Cognitive-Access Degradation API
description: >
  Reference for the additive open-lock classifier (`OpenLockOutcome`,
  `try_acquire_classified`), the caller-role-aware engineer cognition resolver
  (`CallerRole`, `CognitiveAccess`, `WriteMode`), and the reused
  `simard.enrichment.degraded{reason="cognitive_open_lock"}` telemetry — the
  surface that lets N concurrent OODA engineers share one cognitive store and all
  make progress with zero busy/lock write failures and zero artifact-less exits.
  (An isolated per-worktree state root is a designed, deferred follow-up — see §3.)
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/engineer-cognitive-access-degradation.md
  - ./cognitive-memory-open-serialization.md
  - ./cognitive-memory-client-helpers.md
  - ./engineer-worktree-isolation.md
  - ./enrichment-observability-api.md
  - ./telemetry-metrics.md
  - ../howto/diagnose-a-degraded-engineer-cognitive-access.md
  - ../../src/cognitive_memory/open_guard.rs
  - ../../src/cognitive_memory/library_adapter.rs
  - ../../src/ooda_loop/client_factory.rs
  - ../../src/ooda_actions/advance_goal/spawn.rs
  - ../../src/memory_ipc/launcher.rs
  - ../../src/base_type_turn.rs
  - ../../src/bin/simard_ooda_step.rs
  - ../../src/enrichment_observability/mod.rs
  - ../../src/telemetry/names.rs
---

# Engineer Cognitive-Access Degradation API

This reference documents the additive surface that stops concurrent OODA
engineers from starving on — and hard-exiting at — the single-writer cognitive
open-lock. For the rationale see the
[concept doc](../concepts/engineer-cognitive-access-degradation.md); for the
underlying corruption safety-net see
[Cognitive-Memory Open Serialization](./cognitive-memory-open-serialization.md).

The change is **additive and non-breaking**: `CognitiveOpenGuard::acquire()`
keeps its exact fail-loud corruption semantics; everything below is *new* surface
that a *may-degrade* caller opts into.

## 1. Open-lock classification — `src/cognitive_memory/open_guard.rs`

A new classifier distinguishes a benign **flock contention** (the budget expired
while another live holder held the lock — `EWOULDBLOCK`) from a genuine **IO
error** on the lock file, **without** changing `acquire()`.

### `OpenLockOutcome`

```rust
/// Outcome of a *classified* open-lock acquisition. Lets a may-degrade caller
/// (an engineer) choose degrade-vs-fail, while `acquire()` keeps mapping
/// `Contended` → `Err` for the fail-loud corruption-guard path.
pub(crate) enum OpenLockOutcome {
    /// The exclusive advisory `flock` was taken. Carries the held guard;
    /// proceed to open the store.
    Acquired(CognitiveOpenGuard),
    /// The budget expired while another live process still held the lock
    /// (`EWOULDBLOCK`, not an IO fault). Carries the sanitised holder marker
    /// (bounded length, control-stripped) for logging only.
    Contended { holder: String },
}
```

### `try_acquire_classified`

```rust
impl CognitiveOpenGuard {
    /// Classified acquisition: same bounded exponential-backoff budget as
    /// `acquire`, but returns `Ok(OpenLockOutcome::Contended { .. })` for a
    /// budget-exceeded flock contention instead of an `Err`. A genuine IO error
    /// on the lock file is still returned as `Err(SimardError::PersistentStoreIo)`.
    ///
    /// Same-process re-entrancy, the process-global registry, and the atomic
    /// registry+flock check are all identical to `acquire`. Takes the store
    /// `state_root` (the sidecar lock path is derived from it internally).
    pub(crate) fn try_acquire_classified(state_root: &Path) -> SimardResult<OpenLockOutcome>;
}
```

### `acquire` is unchanged behaviourally

```rust
impl CognitiveOpenGuard {
    /// Fail-loud acquisition (corruption guard preserved). Implemented as a thin
    /// wrapper over `try_acquire_classified` that maps `Contended` → the same
    /// `Err(SimardError::PersistentStoreIo { action: "acquire_open_lock", .. })`
    /// it always returned. Byte-for-byte unchanged for existing callers.
    pub(crate) fn acquire(state_root: &Path) -> SimardResult<CognitiveOpenGuard>;
}
```

> **Invariant.** `acquire()` remains the path for `CallerRole::Daemon` and any
> true exclusive-writer. `try_acquire_classified` never weakens the guard — it
> only exposes the *contended* discriminant so the caller can degrade instead.

## 2. Engineer cognition resolver — `src/ooda_loop/client_factory.rs`

`connect_memory` gains a caller-role-aware variant. The resolver walks the
ordered preference (IPC → classify → deferred) and returns a `CognitiveAccess`
describing what the engineer actually got. **The silent second exclusive open on
the shared root is removed for the `Engineer` role.**

### `CallerRole`

```rust
/// Internal capability token controlling degrade-vs-fail. NEVER derived from
/// env/CLI/file input — constructed only at trusted call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallerRole {
    /// Daemon / true exclusive-writer. Fail-loud on contention; a `Deferred`
    /// write is unreachable for this role.
    Daemon,
    /// OODA-spawned engineer. May degrade to deferred/read-only cognition on a
    /// lost open-lock race.
    Engineer,
}
```

### `WriteMode` and `CognitiveAccess`

```rust
/// Whether cognitive writes are persisted live or deferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// Writes persist immediately (IPC-serialized, or a direct library open on
    /// an uncontended store).
    Live,
    /// Writes are **dropped-with-metric** (a bounded in-memory counter — nothing
    /// is buffered or spilled) and NEVER reported to the caller as persisted.
    /// Reachable only for `CallerRole::Engineer`.
    Deferred,
}

/// The resolved cognitive access handed to an engineer. `memory()` always serves
/// reads (shared IPC read, a live library store, or an empty deferred snapshot);
/// `write_mode()` reports whether writes persist. Fields are private; the boxed
/// handle is not `Debug`, so the manual `Debug` impl surfaces only the resolution
/// outcome (`write_mode`, `degraded`) — never store contents.
pub struct CognitiveAccess { /* private: memory, write_mode, degraded */ }

impl CognitiveAccess {
    /// Read/write handle. In `WriteMode::Deferred` the write methods
    /// drop-with-metric and return `Ok(..)`; reads return empty (never a lock
    /// error), so the engineer keeps reasoning.
    pub fn memory(&self) -> &dyn CognitiveMemoryOps;
    /// The write mode actually granted.
    pub fn write_mode(&self) -> WriteMode;
    /// `true` iff this access degraded to deferred/read-only cognition (mirrors
    /// the emitted counter).
    pub fn degraded(&self) -> bool;
}
```

### `connect_memory_for_role`

```rust
/// Resolve engineer cognitive access under an explicit caller role.
///
/// Resolution (first success wins; no silent second exclusive open on the shared
/// root for the `Engineer` role):
///   1. Daemon socket present at `state_root` → `RemoteCognitiveMemory` IPC
///      client → `CognitiveAccess { write_mode: Live, degraded: false }`.
///   2. No socket, `Engineer` role, uncontended direct open → a live
///      `LibraryCognitiveMemory` → `{ write_mode: Live, degraded: false }`.
///   3. No socket AND `try_acquire_classified` returns `Contended`:
///        * `Engineer` → `{ write_mode: Deferred, degraded: true }`, plus a WARN
///          and a `degraded{reason="cognitive_open_lock"}` increment.
///        * `Daemon`   → `LibraryCognitiveMemory::open` returns
///          `Err` (fail-loud; corruption guard preserved).
pub fn connect_memory_for_role(
    state_root: &Path,
    role: CallerRole,
) -> SimardResult<CognitiveAccess>;
```

The existing
`pub fn connect_memory(state_root: &Path) -> SimardResult<Box<dyn CognitiveMemoryOps>>`
is **retained unchanged** and still serves the daemon-side enrichment path
(`src/base_type_turn.rs`), so those callers are untouched. It is a sibling of —
not a wrapper over — `connect_memory_for_role`.

### Deferred writes — drop-with-metric, not a buffer

| Behaviour | Detail |
|---|---|
| **On a deferred write** | The write method returns `Ok("deferred")` and increments a bounded in-memory `AtomicU64` counter, surfaced on a `DEBUG` line (`op`, `deferred_writes`). |
| **Nothing is buffered** | No queue, no byte cap, no disk spill — a deferred write is dropped immediately, so it can never grow memory or touch the contended store. |
| **Not a silent no-op** | The caller already knows via `CognitiveAccess::write_mode() == Deferred` that writes do not persist; `is_read_only()` returns `true`, so a `WriterClient` never wraps the deferred handle as a live writer. |

## 3. Isolated per-worktree state root — deferred follow-up (NOT in this change)

> **Status: designed, not implemented in this change.** The crash-loop is fully
> resolved by the IPC-first + graceful-degrade path (§2): a lost open-lock race
> degrades to deferred/read-only cognition and the engineer still produces its
> artifact. An *isolated per-worktree cognitive state root* — giving a
> **standalone** engineer (one with no daemon socket) a **live, uncontended**
> store instead of a degraded one — is a separate defense-in-depth tier.
>
> It is intentionally left as a follow-up because the engineer's `--state-root`
> (see `src/bin/simard_ooda_step.rs`) serves BOTH cognition and the OODA ledger /
> goal state, and the engineer is launched across a recipe/agent boundary
> (`spawn_subordinate`). Isolating *only* the cognitive store therefore requires a
> new dedicated cognitive-root parameter threaded through `SubordinateConfig` →
> the launch recipe → `clients_from_state_root`, which is out of scope for this
> minimal, additive crash-loop fix. When implemented, the isolated directory must
> be created `0700` (files `0600`), canonicalized and containment-checked under
> the engineer's own worktree, rejecting `../`/symlink escapes and world-writable
> or non-owned parents, fail-loud.

### Wiring change sites (this change)

The seams above (open-guard classifier, role-aware resolver, telemetry reason)
are wired in at these call sites — none add a new public *contract*, all preserve
current non-engineer behaviour:

| Site | Change | Preserves |
|---|---|---|
| `src/ooda_loop/client_factory.rs` | `clients_from_state_root` (the engineer helper-bin entry) resolves cognition via `connect_memory_for_role(state_root, CallerRole::Engineer)` instead of the bare `connect_memory`, so a contended open degrades to deferred cognition (WARN + counter) rather than failing loud. `connect_memory` itself is unchanged. | The daemon-side `connect_memory` path (`src/base_type_turn.rs`) is untouched and still fail-loud via `LibraryCognitiveMemory::open`. |
| `src/bin/simard_ooda_step.rs` | The `observe` / `act` engineer subcommands call `clients_from_state_root(&state_root)`, so they inherit the degrade-instead-of-die behaviour and continue to produce their artifact when cognition is `Deferred`. | CLI surface and `--state-root` flag unchanged. |
| `src/cognitive_memory/open_guard.rs` | Adds `OpenLockOutcome` + `try_acquire_classified`; `acquire()` becomes a thin wrapper mapping `Contended` → the exact prior fail-loud `Err`. The env budget override is clamped to `[1, DEFAULT_BUDGET]`. | `acquire()` error is byte-for-byte unchanged for the daemon / true-writer path. |
| `src/enrichment_observability/mod.rs` | Adds the `CognitiveOpenLock` degrade reason + rollup accounting, reusing the `simard.enrichment.degraded` counter. | Existing `MemoryIpc` / `KnowledgeLaunch` reasons and rollup shape. |

The `related` frontmatter lists the full change surface so it is discoverable.

## 4. Telemetry — reused counter, one new bounded reason

No new counter surface. The feature extends the existing enrichment degrade
counter (`src/telemetry/names.rs`, `src/enrichment_observability/mod.rs`).

### `DegradeReason::CognitiveOpenLock`

```rust
pub enum DegradeReason {
    MemoryIpc,          // existing
    KnowledgeLaunch,    // existing
    CognitiveOpenLock,  // NEW — engineer lost the open-lock race, degraded
}

impl DegradeReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            DegradeReason::MemoryIpc => "memory_ipc",
            DegradeReason::KnowledgeLaunch => "knowledge_launch",
            DegradeReason::CognitiveOpenLock => "cognitive_open_lock", // NEW
        }
    }
}
```

### Metric

| Metric | Type | Attribute | Meaning |
|---|---|---|---|
| `simard.enrichment.degraded` | counter | `reason="cognitive_open_lock"` | An engineer degraded to deferred/read-only cognition after losing the open-lock race. Bounded, enum-only `reason` value — no paths, identities, or store contents. |

A degrade emits, via `enrichment_observability::observe_degrade` (which gains a
`CognitiveOpenLock` message arm alongside the existing `MemoryIpc` /
`KnowledgeLaunch` arms):

- a `WARN` on the `simard::enrichment` target carrying **only** the fixed
  reason-keyed message and the structured `reason=cognitive_open_lock` field — no
  identities, paths, or holder marker ride the WARN, matching the existing
  `observe_degrade` contract;
- the **sanitised** holder marker (bounded length, control-stripped) and the raw
  error go to `DEBUG` only, so an attacker-influenced string can never forge a
  structured WARN field;
- one `simard.enrichment.degraded{reason="cognitive_open_lock"}` increment.

There is **no** `INFO`/`WARN` `print!`/`println!` in any new path — structured
`tracing` + OTel only.

## 5. Configuration

| Env var | Default | Purpose |
|---|---|---|
| `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS` | `15000` | Max backoff budget (ms) before an open is classified `Contended`. **Clamped to `[1, DEFAULT_BUDGET]`**; a parse failure falls back to the default. It can **never disable or raise** the guard — it only *shortens* the race for tests. `DEFAULT_BUDGET` (15 000 ms) is **not** raised as part of this fix. |

There are no other tunables. The IPC-first / isolated-root / deferred resolution
is unconditional for the `Engineer` role.

## 6. Guarantees and non-guarantees

- **Guaranteed:** N concurrent engineers against one shared cognitive store all
  make progress — none exit artifact-less, zero `PersistentStoreIo` "held open …
  15000ms" fatal opens, zero busy/lock **write** failures surfaced as errors
  (deferred writes are metered, not errored).
- **Guaranteed:** a genuine second concurrent **writer** of a non-isolated store
  (`CallerRole::Daemon`) still fails loud — the corruption guard is preserved.
- **Guaranteed:** a `Deferred` write is never reported to the caller as
  persisted (anti-hollow-success).
- **Not guaranteed:** durability of a *deferred* write. Deferred writes are
  **dropped-with-metric** (a bounded counter; nothing is buffered or spilled).
  Cognition is advisory; this is by design.
- **Out of scope:** the memory-ipc broken-pipe reconnect
  ([#2860](https://github.com/rysweet/Simard/issues/2860)) — untouched.

## 7. Security invariants

| Invariant | Mechanism |
|---|---|
| `CallerRole` is a capability token | Constructed only at trusted call sites; never parsed from env/CLI/file. A unit test asserts `WriteMode::Deferred` is unreachable for `CallerRole::Daemon`. |
| Corruption guard preserved | `acquire()` semantics unchanged; the classifier is purely additive. |
| Log-injection safe | Untrusted `holder` lock metadata is length-bounded (≤ 48 bytes) and control-stripped before it reaches a log line or a `Contended` value. |
| Bounded telemetry | Enum-only `reason`; no paths, identities, secrets, or cognitive-store contents in attributes or WARN lines. |
| Timeout clamp | `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS` clamped to `[1, DEFAULT_BUDGET]`; parse-fail → default; never disables or raises the guard. |
| No new remote channel | IPC stays local Unix-domain with the existing `0700`/`0600` perms; degradation opens no unauthenticated path. |
| Bounded deferred writes | Drop-with-metric via an in-memory `AtomicU64` counter; no queue, no byte buffer, no disk spill. |
| Isolated root hardening (deferred) | Applies only once the isolated per-worktree root (§3) is implemented: create `0700`, files `0600`, canonicalize + containment-check, reject `../`/symlink escape and world-writable / non-owned parents — fail-loud. |

## 8. Verification

The regression test asserts the whole-system property:

`tests/cognitive_open_lock_concurrency.rs` — **N = 8** concurrent engineers
against **one shared cognitive store** plus a daemon-equivalent holder
(`serial_test` + `tempfile`, `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS` forcing fast
races). Assertions:

1. **Every** engineer produces its artifact marker — **no artifact-less exit**.
2. **Zero** `PersistentStoreIo` "held open … 15000ms" fatal errors.
3. **Zero** busy/lock **write** failures surfaced as errors — deferred writes are
   counted via the degradation metric, not as failures.

The same integration file also carries the role/degrade contract at unit grain:
the `DegradeReason` enum mapping; `Engineer` uncontended → `Live` round-trip;
`Daemon` uncontended → `Live`; `Engineer` contended → `Deferred` (non-fatal);
the degrade is WARNed and metered; `Daemon` contended → fail-loud; and
`WriteMode::Deferred` is unreachable for `CallerRole::Daemon`.

Plus targeted unit tests:

- `open_guard::tests` — `try_acquire_classified` returns `Contended` on a
  budget-exceeded flock race (and `acquire` still fails loud on the same state);
  the `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS` override is clamped to
  `[1, DEFAULT_BUDGET]`; `acquire` behaviour is byte-for-byte unchanged.
- `client_factory::tests` — `connect_memory` still round-trips through the
  library backend by default (the legacy daemon-side path is unchanged).
- `tests/no_bridge_naming.rs` passes; grep confirms no `print!`/`println!` in the
  changed paths.

## Related

- [Concept: engineer cognitive-access degradation](../concepts/engineer-cognitive-access-degradation.md)
- [Cognitive-Memory Open Serialization](./cognitive-memory-open-serialization.md)
- [Cognitive-Memory Client Helpers](./cognitive-memory-client-helpers.md)
- [Per-Engineer Worktree Isolation](./engineer-worktree-isolation.md)
- [Enrichment Observability API](./enrichment-observability-api.md)
- [Telemetry Metrics](./telemetry-metrics.md)
- Source: `src/cognitive_memory/open_guard.rs`, `src/cognitive_memory/library_adapter.rs`,
  `src/ooda_loop/client_factory.rs`, `src/ooda_actions/advance_goal/spawn.rs`,
  `src/memory_ipc/launcher.rs`, `src/base_type_turn.rs`, `src/bin/simard_ooda_step.rs`,
  `src/enrichment_observability/mod.rs`, `src/telemetry/names.rs`
