---
title: Self-health quarantine classification API reference
description: Reference for the active-vs-retained quarantine classifier in the self-health probe — `classify_quarantine` / `count_active_quarantine_files` in src/self_deploy/health.rs, the `QuarantineClass` distinction between active corruption (<24h) and retained forensic recovery artifacts, and the `NoQuarantineProbe` verdict change that lets a healthy build report `quarantined=false` so `DeployDrift` can drain. Fixes the "quarantine can NEVER clear" self-deploy freeze (#4469 / #4471) without deleting any `*.corrupt-*` recovery asset early (preserves #2420 / #2550).
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./deploy-canary-gate-curation-api.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./disk-reclaim-api.md
  - ../operations/cognitive-memory-wal-recovery-runbook.md
  - ../operations/cognitive-memory-durability.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../../src/self_deploy/health.rs
  - ../../src/cmd_cleanup/disk.rs
---

# Self-health quarantine classification API reference

> **Status: implemented.** `classify_quarantine`,
> `count_active_quarantine_files`, and the `QuarantineClass` enum live in
> [`src/self_deploy/health.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/health.rs).
> `NoQuarantineProbe`'s verdict is computed from `count_active_quarantine_files`
> instead of the old total count. Disk retention in
> [`src/cmd_cleanup/disk.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_cleanup/disk.rs)
> (30-day / keep-5 / protect `*.corrupt-*`) is **unchanged**. This changes the
> health **verdict**, never the retention policy. References **#4469** (probe
> freezing self-deploy) and **#4471** (quarantine can never clear); preserves the
> forensic-asset retention of **#2420 / #2550**.

## Why this exists

The post-deploy self-health report
([self-deploy API](./self-deploy-api.md)) runs a `NoQuarantineProbe` that fails
if the cognitive-memory store is quarantined. The old probe counted **every**
`cognitive*.corrupt-<ts>` artifact directly under the state root and reported
`quarantined = true` if *any* existed:

```rust
// OLD — any corrupt artifact, of any age, fails the probe forever.
let quarantined = count_quarantine_files(&state_root) > 0;
```

That count includes **retained forensic recovery artifacts** — the
`cognitive*.corrupt-*` snapshots the WAL-recovery runbook (#2420 / #2550)
deliberately **keeps for 30 days** so an operator can investigate a past
corruption. Because disk cleanup is *required* to preserve those assets, and the
probe failed on their mere presence, `NoQuarantineProbe.healthy` was `false` for
up to 30 days **even on a fully healthy build with no active corruption**. That
made `SelfHealthReport::is_healthy()` return `false`, which held the self-deploy
gate closed: the probe emitted `[FAIL] no_quarantine quarantined=true` in a state
that could **never clear**, freezing self-deploy and letting `DeployDrift` climb
indefinitely (**#4469 / #4471**).

The fix distinguishes **active corruption** (a store quarantined *now*, which
genuinely should block a deploy) from **retained recovery artifacts** (aged
forensic snapshots that are protected by policy and are *not* a live fault). The
probe fails only on the former. Recovery assets are **never deleted or moved
early** — the classifier reads metadata only.

## Data model

### `QuarantineClass`

```rust
/// Classification of a `cognitive*.corrupt-<ts>` artifact found under the
/// state root, by file age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuarantineClass {
    /// Freshly quarantined store (age < `ACTIVE_QUARANTINE_MAX_AGE`). A live
    /// corruption fault — MUST fail `NoQuarantineProbe` and block self-deploy.
    Active,
    /// Aged forensic recovery artifact (age ≥ `ACTIVE_QUARANTINE_MAX_AGE`),
    /// retained by policy (#2420 / #2550) for post-mortem. NOT a live fault;
    /// must NOT fail the probe and must NOT be deleted early.
    RetainedRecovery,
}

/// Age boundary between an active corruption and a retained forensic artifact.
/// Distinct from — and strictly shorter than — `disk::CORRUPT_DB_MAX_AGE_DAYS`
/// (the 30-day retention window), so the two never latch on the same boundary.
const ACTIVE_QUARANTINE_MAX_AGE: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60); // 24h
```

### `NoQuarantineProbe` (verdict changed, shape unchanged)

The struct is **byte-for-byte unchanged** — the JSON shape read by
`simard self-health --json` is stable. Only the *computation* of its two booleans
changes: they are now driven by the count of **active** artifacts.

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoQuarantineProbe {
    pub healthy: bool,     // now: true when NO active-corruption artifact exists
    pub quarantined: bool, // now: true only for ACTIVE corruption (<24h)
}
```

## `classify_quarantine`

```rust
/// Classify one quarantine artifact by the age of its file metadata.
///
/// Fail-CLOSED: if the modified-time cannot be read (permission, races,
/// spoofed/absent mtime), the artifact is treated as `Active` — the safe
/// verdict that blocks self-deploy rather than masking a live corruption.
/// Reads metadata ONLY; never opens, moves, or deletes the file.
fn classify_quarantine(path: &std::path::Path, now: std::time::SystemTime)
    -> QuarantineClass;
```

| Condition | `QuarantineClass` |
| --- | --- |
| mtime age `< 24h` | `Active` |
| mtime age `≥ 24h` | `RetainedRecovery` |
| mtime unreadable / in the future / metadata error | `Active` (fail-closed) |

## `count_active_quarantine_files`

```rust
/// Count ONLY `Active` (<24h) `cognitive*.corrupt-<ts>` artifacts directly
/// under `state_root`. Retained forensic artifacts (≥24h) are excluded.
/// Absent/unreadable dir ⇒ `0`. Uses the same exact `is_corrupt_quarantine_name`
/// filename predicate as `count_quarantine_files` and `cmd_cleanup::disk` — the
/// two mirrors stay byte-for-byte consistent (see [Filename parity](#filename-parity)).
/// Directory-confined; does not follow symlinks (TOCTOU / spoofed-metadata
/// resistant, SR-V4).
fn count_active_quarantine_files(state_root: &std::path::Path) -> u64;
```

The legacy total-count helper `count_quarantine_files` is **retained** (used by
existing tests and any total-inventory caller); the probe simply no longer keys
its verdict on it.

## Probe wiring

The `NoQuarantineProbe` construction in `run_self_health_probes`
([`src/self_deploy/health.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/health.rs))
changes one line:

```rust
// Probe 5: no ACTIVE quarantined corrupt cognitive-memory store.
// Retained forensic recovery artifacts (≥24h, kept by #2420/#2550) do NOT
// fail this probe — only a live (<24h) corruption does.
let quarantined = count_active_quarantine_files(&state_root) > 0;
let no_quarantine = NoQuarantineProbe {
    healthy: !quarantined,
    quarantined,
};
```

`SelfHealthReport::compute` still ANDs every probe, so a healthy build with only
retained artifacts now yields `no_quarantine.healthy = true`,
`SelfHealthReport::is_healthy() = true`, the self-deploy gate opens, the binary
is swapped, and `DeployDrift` drains to `0`.

## Interaction with disk retention (unchanged)

This classifier is **read-only** and touches metadata only. It does **not**:

- delete, truncate, move, or rename any `cognitive*.corrupt-*` artifact;
- alter `disk::CORRUPT_DB_MAX_AGE_DAYS` (30d), the keep-5 floor, or the
  `*.corrupt-*` protect rule in
  [`src/cmd_cleanup/disk.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_cleanup/disk.rs);
- affect the [WAL-recovery runbook](../operations/cognitive-memory-wal-recovery-runbook.md).

The 24-hour `ACTIVE_QUARANTINE_MAX_AGE` is deliberately **shorter** than the
30-day retention window: an artifact is "active" only briefly after it appears,
then ages into "retained" long before disk cleanup would ever consider removing
it. The two thresholds never overlap on a boundary.

> **Residual-risk design note (age is a proxy for recovery).** Classification
> keys purely on **artifact age**, which assumes recovery completes within 24h
> and that a quarantine artifact older than 24h is therefore a benign forensic
> asset. If a store were quarantined, *failed to recover*, and the same
> `*.corrupt-*` artifact simply aged past 24h, this classifier would report it
> `RetainedRecovery` and the probe would pass — potentially masking an
> unresolved live corruption. Age is a **weak proxy for "recovered vs. still
> broken."** This is acceptable for the #4471 fix (the observed failure mode is
> retained artifacts from a *successful* past recovery), but a follow-up should
> consider keying `Active` on the **live store's** health (e.g. presence of a
> readable non-quarantined `cognitive_memory.*` alongside the artifact, or a
> recovery-completion marker) rather than artifact age alone. Until then, the
> fail-closed default only covers unreadable mtime, **not** a stale-but-active
> corruption.

<a id="filename-parity"></a>
### Filename parity

`is_corrupt_quarantine_name` is mirrored in `health.rs` and
`cmd_cleanup/disk.rs`. Both mirrors remain **byte-for-byte identical** — the
classifier does not change classification of *which* files are quarantine
artifacts, only *which age band* an artifact falls in. A parity test asserts the
two predicates agree on a shared fixture set, so the disk-cleanup contract is not
broken.

## What an operator observes

**Before** — on every overseer cycle, for up to 30 days after any past
corruption:

```
[FAIL] no_quarantine quarantined=true   (retained cognitive.corrupt-20260615 present)
self-health: UNHEALTHY → self-deploy frozen → DeployDrift climbing
```

**After** — with only retained (aged) artifacts and no live corruption:

```
[OK]   no_quarantine quarantined=false  (0 active; 1 retained forensic artifact, protected)
self-health: HEALTHY → self-deploy proceeds → DeployDrift drains to 0
```

If a store is quarantined *right now* (<24h), the probe still reports
`quarantined=true` and blocks the deploy — active corruption detection is fully
intact.

## Security & fail-closed properties

- **SR-D1 forensic-asset integrity.** The classifier reads metadata only; it
  never deletes or moves a `*.corrupt-*` artifact. #2420 / #2550 retention is
  preserved.
- **SR-V4 TOCTOU / spoofed metadata.** The scan is directory-confined, matches
  the exact filename pattern, and does not follow symlinks.
- **Fail-closed everywhere.** An unreadable / future / erroring mtime classifies
  as `Active` (blocks self-deploy), never `RetainedRecovery`. No `panic!`, no
  `process::exit`, no silent degrade.

## Compatibility

- **Additive.** New enum, new constant, one new helper; the legacy
  `count_quarantine_files` and `NoQuarantineProbe` struct are unchanged.
- **Stable JSON.** `SelfHealthReport` / `NoQuarantineProbe` byte-shape is
  unchanged; `simard self-health --json` consumers see the same keys. Any newly
  added field would carry `#[serde(default)]` fail-closed — none was needed here.
- **No new inputs.** No CLI flag, config key, or RPC. The 24h threshold is a
  compile-time constant.
- **No `print`-family macros / no `bridge` naming.** All emission is structured
  `tracing` at the probe layer.

## Testing

| Test | Asserts |
| --- | --- |
| `retained_only_is_healthy` | A single `cognitive.corrupt-*` aged ≥24h ⇒ `count_active_quarantine_files == 0`, `no_quarantine.healthy == true`. |
| `active_present_is_quarantined` | A `<24h` artifact ⇒ `quarantined == true`, probe fails. |
| `unreadable_mtime_fails_closed` | A metadata error classifies as `Active` (probe fails). |
| `filename_parity_health_vs_disk` | `health.rs` and `disk.rs` `is_corrupt_quarantine_name` agree on a shared fixture set. |
| `classifier_never_mutates_files` | Directory contents are byte-identical before/after a scan. |

All tests are hermetic — `tempfile`-backed state roots, no network, no live
deploy.

## See also

- [Deploy canary gate curation API](./deploy-canary-gate-curation-api.md) — the sibling fix that removes the red-canary exit-101 recursion (#4469 / #4470); together they unfreeze self-deploy.
- [Self-deploy API reference](./self-deploy-api.md) — where `SelfHealthReport` is consumed by the deploy gate.
- [Cognitive-memory WAL-recovery runbook](../operations/cognitive-memory-wal-recovery-runbook.md) — why `*.corrupt-*` recovery artifacts are retained.
- [Disk reclaim API](./disk-reclaim-api.md) — the 30-day / keep-5 / protect retention this classifier leaves untouched.
