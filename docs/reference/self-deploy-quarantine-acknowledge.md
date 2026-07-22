---
title: Self-deploy quarantine-acknowledge reference
description: Reference for the durable quarantine acknowledge/clear path that lets a genuinely-stuck cognitive-memory quarantine reset the self-health `no_quarantine` probe without deleting the #2550 recovery asset — the `.ack` sidecar convention, the `quarantine_ack` module API, the ack-aware `no_quarantine` probe, the `simard self-health --acknowledge-quarantine` operator flag and its guarded autonomous auto-ack, and the `cmd_cleanup::disk` sidecar-sweep behaviour.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-api.md
  - ./overseer-deploy-canary-diagnostics.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../howto/clear-a-stuck-memory-quarantine.md
  - ../../src/self_deploy/quarantine_ack.rs
  - ../../src/self_deploy/health.rs
  - ../../src/cmd_cleanup/disk.rs
  - ../../src/operator_cli/self_health.rs
---

# Self-deploy quarantine-acknowledge reference

> **Status: implemented.** The `.ack` sidecar convention, the
> [`quarantine_ack`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/quarantine_ack.rs)
> module, the ack-aware `no_quarantine` probe in
> [`src/self_deploy/health.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/health.rs),
> the `simard self-health --acknowledge-quarantine` flag in
> [`src/operator_cli/self_health.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_cli/self_health.rs),
> and the sidecar-aware sweep in
> [`src/cmd_cleanup/disk.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_cleanup/disk.rs)
> live in the tree today. The change is **additive and non-breaking**: the
> `NoQuarantineProbe` JSON schema, the `self-health` exit-code convention, and
> the `remove_old_corrupt_dbs` retention rules are all unchanged. No public
> signature was removed.

## Why this exists

The post-deploy [`no_quarantine`](./self-deploy-api.md#self-health-output) probe
fails whenever a `cognitive*.corrupt-<ts>` quarantine artifact sits in the
state-root directory. That is correct while corruption is fresh — but it created
a **deadlock** (issue #4469):

1. When LadybugDB quarantines a corrupt store it leaves a `cognitive.corrupt-<ts>`
   artifact in `~/.simard`. `no_quarantine` goes red and stays red.
2. The largest *substantial* quarantine is the **#2550 recovery asset** — a
   corrupt store a prefix-recovery salvaged real records from — and
   `remove_old_corrupt_dbs` deliberately **never** sweeps it, regardless of age.
3. So the one artifact that keeps `no_quarantine` red is exactly the one that is
   protected from deletion. The probe can therefore **never** clear on its own,
   `all_healthy()` never reaches `true`, and self-deploy freezes with the running
   binary stuck commits behind merged `main` (the recurring
   "DeployDrift — running binary is N commit(s) behind merged main" signal).

The fix adds a **durable acknowledge path**: an operator (or, for the protected
recovery asset past the forensic window, the daemon itself) can *acknowledge* a
quarantine so the probe stops counting it — **without deleting the recovery
asset**. Acknowledgement silences the probe; retention is untouched. New,
unacknowledged corruption still reddens the probe immediately, because
acknowledgement is keyed to a specific artifact filename (which embeds the
`.corrupt-<ts>` timestamp).

## The `.ack` sidecar convention

Acknowledgement is recorded as a small sibling **sidecar file** next to the
quarantine artifact it acknowledges, in the resolved
[`simard_state_root()`](../../src/state_root.rs):

```
~/.simard/cognitive.corrupt-20260722T131600Z          # the quarantine artifact
~/.simard/cognitive.corrupt-20260722T131600Z.ack      # its acknowledgement sidecar
```

Properties:

- **Filename-keyed.** The sidecar name is `<quarantine-file-name>.ack`. Because
  every quarantine name carries a unique `.corrupt-<ts>` infix, an `.ack` only
  ever silences the one artifact it names. A *new* corruption event produces a
  new `.corrupt-<ts2>` artifact with no sidecar, so `no_quarantine` re-reddens.
- **Additive, never destructive.** Writing an `.ack` never touches, moves, or
  deletes the quarantine artifact. The #2550 recovery asset survives verbatim.
- **Idempotent.** Acknowledging an already-acknowledged artifact is a no-op that
  succeeds. Re-running the operator command is always safe.
- **Reversible.** Deleting the `.ack` sidecar restores the pre-ack behaviour:
  the artifact is counted again and `no_quarantine` reddens (assuming the
  artifact is still present).

### Sidecar payload

The sidecar is a small, fixed marker file — the exact bytes
`acknowledged\n`. Presence of the sidecar *is* the acknowledgement; the
convention deliberately stores no structured payload, so there is nothing to
parse, version, or leak. Who acknowledged (operator vs. the guarded
autonomous auto-ack), when, and why are recorded on the **structured
tracing/OTel event** emitted at acknowledgement time, not in the sidecar. The
sidecar exists only to be counted (or skipped) by the probe.

## `quarantine_ack` module API

[`src/self_deploy/quarantine_ack.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/quarantine_ack.rs)
is the **single owner** of the `.ack` convention. Both the `no_quarantine` probe
and the operator CLI go through it; no other module constructs `.ack` paths.

```rust
/// Suffix appended to a quarantine artifact's basename to form its sidecar.
pub const ACK_SUFFIX: &str = ".ack";

/// Compute the `.ack` sidecar path for a quarantine artifact living directly
/// under `state_root`. `quarantine_name` MUST be a validated basename that
/// passes the single canonical corrupt-quarantine predicate; separators,
/// `..`, and absolute paths are rejected. Returns `None` for an invalid name.
pub fn ack_marker_path(state_root: &Path, quarantine_name: &str) -> Option<PathBuf>;

/// `true` when `name` is itself an `.ack` sidecar (so scanners can skip it).
pub fn is_ack_marker_name(name: &str) -> bool;

/// `true` when the quarantine artifact `quarantine_name` under `state_root` has
/// a present regular-file `.ack` sidecar. `false` for an invalid name, a
/// missing sidecar, or a non-regular-file (symlink/dir) at the sidecar path
/// (fail toward "not acknowledged").
pub fn is_acknowledged(state_root: &Path, quarantine_name: &str) -> bool;

/// Durably acknowledge the quarantine artifact `quarantine_name` under
/// `state_root` by writing its fixed-marker `.ack` sidecar. Idempotent:
/// acknowledging an already-acked artifact succeeds and leaves a single
/// sidecar. Never touches the artifact. Returns the written sidecar path.
///
/// Fails closed on a path-safety violation, a symlinked/irregular sidecar
/// target, or a write error.
pub fn acknowledge(state_root: &Path, quarantine_name: &str) -> SimardResult<PathBuf>;

/// List the acknowledgeable `cognitive*.corrupt-*` artifact basenames present
/// directly under `state_root`, excluding `.ack` sidecars and the live store.
/// The operator `--acknowledge-quarantine` path iterates this list, keeping
/// `quarantine_ack` the single owner of "what is an acknowledgeable quarantine".
pub fn present_quarantine_artifacts(state_root: &Path) -> Vec<String>;
```

### Path safety

Every `.ack` path is built by [`ack_marker_path`], which accepts **only** a
basename that passes the shared corrupt-quarantine-name predicate and rejects
anything containing a path separator, a `..` component, or an absolute prefix.
The resolved path is asserted to round-trip through `Path::file_name()` before
any I/O, and writes use `symlink_metadata` (`lstat`) plus `create_new`
(`O_EXCL`) to **refuse** an existing non-regular-file target — a planted
`cognitive.corrupt-X.ack -> /etc/passwd` symlink can never be followed or
overwritten. The state root itself is resolved only via
[`simard_state_root()`](../../src/state_root.rs), which already enforces a
non-empty, absolute, NUL-free path. The sidecar holds a small fixed marker
(`acknowledged\n`), `fsync`-ed on write.

## Ack-aware `no_quarantine` probe

The [`no_quarantine`](./self-deploy-api.md#self-health-output) probe in
[`src/self_deploy/health.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/health.rs)
now counts only **unacknowledged** quarantine artifacts. `count_quarantine_files`
skips both `.ack` sidecars and any artifact that has a present `.ack` sidecar:

```rust
fn count_quarantine_files(state_root: &std::path::Path) -> u64 {
    let entries = match std::fs::read_dir(state_root) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            is_corrupt_quarantine_name(&name)
                && !quarantine_ack::is_ack_marker_name(&name)
                && !quarantine_ack::is_acknowledged(state_root, &name)
        })
        .count() as u64
}
```

The `NoQuarantineProbe` **JSON schema is unchanged** — it still serializes as
`{ "healthy": bool, "quarantined": bool }`. `quarantined` is now `false` once
every quarantine artifact is acknowledged, so `no_quarantine.healthy` can reach
`true` and `all_healthy()` can converge. An older orchestrator deserializing the
report sees no new fields.

> **Single canonical predicate.** `is_corrupt_quarantine_name` is defined **once**
> and shared by both the probe and `cmd_cleanup::disk::remove_old_corrupt_dbs`
> (`cognitive.` / `cognitive_memory.` stem + `.corrupt-` infix). There is no
> second copy to drift: the probe and the sweep agree on what a quarantine is by
> construction.

## `simard self-health --acknowledge-quarantine`

The operator surface is an **additive** flag on the existing
[`simard self-health`](./self-deploy-api.md#simard-self-health) subcommand. See
also the how-to: [Clear a stuck memory
quarantine](../howto/clear-a-stuck-memory-quarantine.md).

```text
simard self-health [--json] [--pre-deploy-facts=N] [--acknowledge-quarantine]

  --acknowledge-quarantine
        Acknowledge every currently-present cognitive-memory quarantine
        artifact under the state root, writing an `.ack` sidecar next to each
        (source: operator). Idempotent. Does NOT delete any artifact — the
        #2550 recovery asset is retained. After acknowledging, the probe is
        re-run and the (now-cleared) report is printed.

Exit code: 0 when every probe is healthy; non-zero when any probe fails.
```

Behaviour:

- With `--acknowledge-quarantine`, the command first acknowledges each present
  quarantine artifact (via `quarantine_ack::acknowledge`), then runs the normal
  probe and prints the report. Because acknowledgement is idempotent, running it
  twice is safe: the second run finds each sidecar already present and re-writes
  nothing.
- Without the flag, `self-health` behaviour is exactly as before: it reports the
  six probes and exits non-zero if any is unhealthy. Acknowledgement is **never**
  implicit for a manual health check.
- The **exit-code convention is unchanged**: `0` iff every probe is healthy
  after the (optional) acknowledgement.

### Guarded autonomous auto-ack

To break the deadlock **without** operator intervention, the auto-ack runs
**inside the probe itself** — in `run_self_health_probe`
([`src/self_deploy/health.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/health.rs)),
the function the `no_quarantine` probe already calls — **not** in the operator
CLI. This placement is load-bearing: the autonomous orchestrator's post-deploy
`health_check` calls `run_self_health_probe` directly and **never** goes through
`operator_cli::self_health`. If the auto-ack lived only in the CLI it would never
fire during an unattended self-deploy, the internal probe would keep counting the
protected asset, and rollback would repeat forever. It must live on the probe
path so the same code clears the deadlock for both the operator command and the
autonomous daemon.

The auto-ack is narrowly scoped — it fires only for the case that can genuinely
never clear otherwise:

- **Only** the #2550 **protected recovery asset** is eligible — the single
  artifact `remove_old_corrupt_dbs` refuses to sweep: the largest quarantine
  whose size is at least `CORRUPT_DB_PROTECT_MIN_BYTES` (1 MB).
- **Only** when it is **older than the forensic window**
  (`CORRUPT_DB_MAX_AGE_DAYS`, 30 days) — long past the point an operator would
  have acted on it.
- Every other quarantine artifact — anything fresh, anything not the protected
  asset — is **never** auto-acked and still reddens the probe.

> **Single-sourced protected-asset selection.** The "which artifact is the
> protected recovery asset" decision — largest quarantine with size ≥
> `CORRUPT_DB_PROTECT_MIN_BYTES` — and the `CORRUPT_DB_MAX_AGE_DAYS` age gate are
> the **same** predicate and constants `remove_old_corrupt_dbs` uses in
> [`src/cmd_cleanup/disk.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_cleanup/disk.rs).
> `health.rs` reuses that selection helper and those constants rather than
> re-deriving "protected" independently, so the probe and the sweep can never
> disagree about which single artifact is protected. (This is separate from the
> corrupt-*name* predicate above; both the name predicate and the
> protected-asset selection are single-sourced.)

When an auto-ack fires it writes the fixed-marker `.ack` sidecar and emits a
structured OTel event (WARN) recording that the source was the autonomous
daemon, the artifact name, its age, and the reason. There is no
`print!`/`println!` — the record is tracing/OTel only. The auto-ack is
reversible (delete the sidecar) and is logged so an operator can always see
that the daemon cleared the deadlock on its own.

> **Why this is safe.** Auto-ack silences a probe; it never deletes data and
> never touches a *fresh* quarantine. A genuinely new corruption event produces
> a new, young, unacknowledged artifact that both fails the age gate and lacks a
> sidecar — so it correctly reddens `no_quarantine` and blocks the deploy.

## Cleanup interaction (`cmd_cleanup::disk`)

`remove_old_corrupt_dbs`
([`src/cmd_cleanup/disk.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_cleanup/disk.rs))
is updated so acknowledgement and reclamation stay consistent:

- **Scans the resolved state root.** The scan directory is now
  [`simard_state_root()`](../../src/state_root.rs) rather than a hardcoded
  `$HOME/.simard`, so a `SIMARD_STATE_ROOT` override points the sweep at the
  **same** directory the probe scans and acknowledges in. (Previously the two
  could diverge under an override.)
- **Skips `.ack` sidecars.** `is_ack_marker_name` sidecars are not quarantine
  artifacts and are never counted or reclaimed on their own.
- **Sweeps a sidecar with its artifact.** When a quarantine artifact is reclaimed
  (age cap or keep-last-N), its `.ack` sidecar, if any, is removed in the same
  pass so no orphaned sidecars accumulate.
- **Preserves the #2550 recovery asset and its marker.** The largest substantial
  quarantine is still never swept, and its `.ack` sidecar (from an auto-ack or a
  manual ack) is preserved alongside it. Acknowledgement silences the probe; it
  does **not** make the recovery asset eligible for deletion.

## Convergence guarantee

With the acknowledge path in place, a stuck quarantine no longer freezes
self-deploy:

1. `no_quarantine` counts only unacknowledged artifacts, so an acknowledged
   (or auto-acked protected) quarantine no longer reddens it.
2. `all_healthy()` can reach `true`, the post-deploy health check passes, and the
   swapped build is accepted instead of rolled back.
3. Self-deploy converges — the running binary advances to merged `main` and the
   recurring "DeployDrift — running binary is N commit(s) behind merged main"
   signal stops firing.

**Autonomy is bounded by the forensic window.** Fully autonomous convergence
happens only *after* the protected recovery asset ages past
`CORRUPT_DB_MAX_AGE_DAYS` (30 days), because the guarded auto-ack refuses to
touch it before then. While the protected asset is still inside that window, an
operator must run `simard self-health --acknowledge-quarantine` to converge — the
daemon deliberately will not silence a recent quarantine on its own. This is the
intended trade-off: the forensic window is preserved for fresh corruption, and
autonomy resumes once it has elapsed.

A genuinely new corruption still reddens the probe and blocks the deploy, so the
safety property the probe exists to enforce is preserved.

## See also

- [Self-deploy API reference](./self-deploy-api.md) — the six probes, the
  `simard self-health` subcommand, and `all_healthy()`.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md)
  — the paired `unit-test` gate `first_failure=` detail (#4470) that makes the
  *other* self-deploy blocker diagnosable.
- [Reconcile & self-deploy](../concepts/reconcile-and-self-deploy.md) — what
  "healthy" means and the end-to-end deploy flow.
- [Clear a stuck memory quarantine](../howto/clear-a-stuck-memory-quarantine.md)
  — the operator runbook.
