---
title: OODA binary-identity reload gate
description: Reference for the content-identity gating that replaces mtime-only auto-reload in the OODA daemon, so the daemon exec()s into a new binary only when the on-disk image is genuinely different — the startup-captured running-image content hash, the on-disk content-hash confirm, the mtime pre-filter that keeps hashing off the hot loop, the fail-closed error handling, and the absolute current_exe() re-exec invariant — eliminating the ~40-minute self-restart churn from identical-content cargo rebuilds.
last_updated: 2026-07-02
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../daemon-mode.md
  - ../howto/run-ooda-daemon.md
  - ./multi-binary-self-update.md
  - ./self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../../src/operator_commands_ooda/daemon/helpers.rs
  - ../../src/operator_commands_ooda/daemon/mod.rs
---

# OODA binary-identity reload gate

> **Status: implemented (Wave 2, 2026-07-02 operator-review priority 2).** The
> content-identity reload gate lives in
> [`src/operator_commands_ooda/daemon/helpers.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/helpers.rs)
> (`binary_changed`, `reload_decision`, `file_content_hash`, `running_image_hash`,
> `exec_self_reload`) and is invoked from the loop in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs),
> which pins the running image's content hash at startup via
> `capture_running_image_hash()`. It replaces the historical mtime-only
> `binary_changed` (see [The problem: mtime churn](#the-problem-mtime-churn)),
> which relaunched on *any* rebuild/`touch`. This is the 2026-07-02
> operator-review priority-2 fix (over-frequent daemon self-restart).

The OODA daemon supports **auto-reload**: on each cycle it checks whether the
executable on disk differs from the one it is running, and if so it `exec()`s
into the new image so a merged self-change goes live without an operator
restart. Auto-reload is on by default and disabled with `--no-auto-reload`
(see [Run the OODA daemon](../howto/run-ooda-daemon.md)).

The reload **trigger** is content-identity: the daemon relaunches only when the
on-disk binary is a *genuinely different image*, not merely a newer file. This
reference specifies that gate, why it replaces the current mtime-only check on
`main`, and its safety invariants.

## Contents

- [The problem: mtime churn](#the-problem-mtime-churn)
- [The gate](#the-gate)
- [Running-image identity](#running-image-identity)
- [Decision table](#decision-table)
- [Public API](#public-api)
- [Configuration](#configuration)
- [Safety invariants](#safety-invariants)
- [Examples](#examples)
- [Migration notes](#migration-notes)

## The problem: mtime churn

The current gate on `main` compares the executable's **mtime** against the
process start time — this is the behavior the Wave 2 change replaces:

```rust
// current behavior on `main` — mtime only
pub fn binary_changed(start_time: SystemTime) -> bool {
    exe_mtime().is_some_and(|mtime| mtime > start_time)
}
```

Any operation that bumps the binary's mtime — a `cargo build` that relinks an
identical image, a `touch`, a redeploy script that copies a byte-identical
binary into place, a backup/restore that rewrites the file — makes
`binary_changed` return `true` even though the running code is unchanged. The
daemon then closes its LLM session, `exec()`s a fresh copy of the *same* image,
and pays full cold-start cost (bridge spawn, memory recall, preparation) for no
behavioral change.

On a host that rebuilds periodically this produces a self-restart every
~40–45 minutes — roughly every 4–5 cycles at the default
`SIMARD_OODA_INTERVAL_SECS=300` — driving `NRestarts` very high and starving the
loop of useful work with repeated cold starts.

## The gate

The gate confirms a **real image difference** before relaunching. It keeps
the cheap mtime read as a fast pre-filter so the expensive content check stays
off the hot path:

1. **mtime pre-filter (cheap).** If the on-disk mtime is **not** newer than the
   process start time, the image cannot have changed — return `false`
   immediately, no hashing. This is the common case every cycle.
2. **Identity confirm (only when mtime is newer).** When mtime *is* newer,
   confirm a genuine difference before relaunching:
   - Compare the daemon's **running-image content hash** (a SHA-256 of the
     process's own executable, captured once at startup and cached — see
     [Running-image identity](#running-image-identity)) against a fresh content
     hash of the on-disk binary.
   - Because a byte-identical rebuild produces the *same* content hash, the
     confirm step returns `false` and the daemon keeps running — no relaunch on
     identical content.
   - Only a genuinely different on-disk image (different content hash) returns
     `true` and triggers `exec_self_reload`.

Hashing the on-disk file only when its mtime advanced means the content hash is
never computed on a steady-state cycle.

## Running-image identity

The daemon's trusted identity is a SHA-256 digest of **its own executable**,
captured once at startup and cached for the life of the process:

```rust
// src/operator_commands_ooda/daemon/mod.rs — at startup
let start_time = exe_mtime().unwrap_or_else(SystemTime::now);
capture_running_image_hash(); // pins hash(current_exe()) before any replace
```

Capturing it **once, at startup** is what makes the gate correct: after an
in-place replace, `std::env::current_exe()` resolves to the NEW bytes, so
re-hashing it at check time would compare the new file against itself and never
detect a change. The startup-pinned hash still identifies the OLD image the
process is actually running, so a later on-disk hash that differs from it is a
genuine new image. Pinning at startup (rather than lazily on the first check)
also closes the window in which a replace between `exec` and the first cycle
could be mistaken for the running image.

Comparing the on-disk (untrusted) bytes against the startup-pinned (trusted)
running hash also means the gate detects a tampered swap that an mtime
comparison never could.

## Decision table

| On-disk mtime vs. start | On-disk hash vs. running-image hash | `binary_changed` | Action |
| --- | --- | --- | --- |
| not newer | *(not checked)* | `false` | keep running (hot-path, no hash) |
| newer | equal (identical rebuild) | `false` | keep running (the churn case) |
| newer | different | `true` | `exec_self_reload` into the new image |
| newer | on-disk read/hash **error** | `false` | keep running (fail-closed) |
| mtime unreadable | *(not checked)* | `false` | keep running (fail-closed) |
| running hash unknown | *(not checked)* | `false` | keep running (fail-closed) |

## Public API

```rust
// src/operator_commands_ooda/daemon/helpers.rs

/// mtime of the currently-running executable, or `None` if it cannot be
/// determined (e.g. the binary was deleted after launch).
pub fn exe_mtime() -> Option<SystemTime>;

/// Stable content identity (hex SHA-256) of the file at `path`, or `None` on any
/// I/O error (fail-closed). Streams the file, so a multi-MB binary is never
/// fully buffered. Never panics.
pub fn file_content_hash(path: &Path) -> Option<String>;

/// Hash of the RUNNING image, captured once (lazily) and cached for the life of
/// the process, or `None` if our own executable could not be hashed.
pub fn running_image_hash() -> Option<&'static str>;

/// Pin the running-image hash at a known-early point (daemon startup),
/// closing the exec→first-check window. Idempotent.
pub fn capture_running_image_hash();

/// Pure reload decision (unit-tested in isolation): `true` only when the on-disk
/// image is genuinely different. mtime pre-filter first; then compare
/// `on_disk_hash` to `running_hash`. Any `None` (unreadable mtime or hash) is
/// fail-closed to `false`.
pub fn reload_decision(
    start_time: SystemTime,
    on_disk_mtime: Option<SystemTime>,
    on_disk_hash: Option<&str>,
    running_hash: &str,
) -> bool;

/// `true` only when the on-disk executable is a genuinely different image than
/// the running one. Cheap mtime pre-filter, then a content-hash confirm against
/// the startup-pinned [`running_image_hash`].
///
/// Fail-closed: any error reading or hashing the on-disk binary, an unreadable
/// mtime, or an unknown running identity yields `false`. Never panics.
pub fn binary_changed(start_time: SystemTime) -> bool;

/// Replace the current process with a fresh copy of itself via `exec()`.
/// On success this never returns; on failure the error is returned so the
/// caller degrades gracefully and keeps running. The re-exec target is the
/// absolute `std::env::current_exe()`, never a relative path or `$0`.
#[cfg(unix)]
pub fn exec_self_reload() -> Result<(), Box<dyn std::error::Error>>;
```

The loop call site is unchanged in shape — only the gate's semantics tighten:

```rust
// src/operator_commands_ooda/daemon/mod.rs
#[cfg(unix)]
if auto_reload && binary_changed(start_time) {
    if let Some(ref mut session) = bridges.session {
        let _ = session.close();
    }
    exec_self_reload()?;
    // exec_self_reload only returns on error — continue running.
}
```

## Configuration

| Variable / flag | Default | Purpose |
| --- | --- | --- |
| `--no-auto-reload` | (auto-reload **on**) | CLI flag to `simard ooda run` that disables auto-reload entirely — the daemon never relaunches itself regardless of on-disk changes. |
| `SIMARD_OODA_INTERVAL_SECS` | `300` | Cycle interval. Also the cadence at which the reload gate is evaluated. An unparseable/empty value falls back to `300`; **range-clamping of `0` and oversized values is the Wave 4 daemon-safety hardening target and is not yet in place** (see below). |

`SIMARD_OODA_INTERVAL_SECS` is read as
`std::env::var(...).ok().and_then(|v| v.parse().ok()).unwrap_or(300)` on `main`
today: a non-numeric or empty value falls back to the `300` default, but `0` and
absurdly large values currently parse successfully and are accepted verbatim. A
value of `0` would busy-spin the loop — evaluating the reload gate and every
other per-cycle check with no sleep. Clamping this knob to a safe range is the
**Wave 4** daemon-correctness/safety hardening item (`daemon-correctness-safety`,
R3); it is specified here for context but is not part of the Wave 2 reload-gate
change.

Auto-reload continuing to be *enabled* is correct; the fix is to stop it firing
on identical content, not to turn it off. Operators who want a fully static
daemon still use `--no-auto-reload`.

## Safety invariants

- **Fail-closed.** Any inability to read the on-disk binary, compute its hash,
  or read its mtime results in **no relaunch**. A transient I/O error must never
  trigger a cold start.
- **Absolute re-exec target.** `exec_self_reload` re-execs the absolute
  `std::env::current_exe()` path with the original argv tail — never a relative
  path, a `PATH` lookup, or `$0`, so the reload cannot be hijacked into a
  different binary.
- **Trusted-vs-untrusted comparison.** The startup-pinned running-image hash is
  the trusted side; the on-disk bytes are untrusted. Content gating detects a
  tampered swap that an mtime bump alone would have accepted.
- **Zero-shell exec.** The relaunch uses `Command::exec()` directly — there is
  no `sh -c`, so there is no shell-injection surface in the reload path.
- **Session drained first.** The LLM session is closed before `exec()` so a
  genuine reload does not leak the bridge/session resources.
- **Panic-free.** The gate performs no unwrap/expect on I/O and no unbounded
  allocation; it is safe to call every cycle.

## Examples

### Default: reload only on a real change

```bash
simard ooda run
```

A `cargo build` that produces a byte-identical binary (same commit, no source
change) bumps the file mtime but leaves the content hash unchanged — the daemon
keeps running. A build from a *new* commit produces different content — the
daemon relaunches once into it.

### Disable auto-reload entirely

```bash
simard ooda run --no-auto-reload
```

The daemon ignores on-disk changes; deploy a new binary by restarting the
service explicitly.

### Confirm the churn is gone

```bash
# Restart count should stay flat across identical rebuilds.
systemctl --user show -p NRestarts simard-ooda

# The reload log line appears only on a genuine image change. On a content
# change the daemon logs "on-disk binary is a genuinely different image
# (content hash changed) — reloading via exec()" to ooda.log / stderr.
grep 'genuinely different image' ~/.simard/ooda.log | tail
```

## Migration notes

- **No config change required.** Existing deployments keep auto-reload on and
  gain the identity gate automatically. The only observable difference is that
  identical-content rebuilds no longer trigger a restart.
- **Legitimate deploys still reload.** The
  [self-deploy](./self-deploy-api.md) and
  [multi-binary self-update](./multi-binary-self-update.md) paths install a
  binary whose content differs from the running image, so its content hash
  differs and the daemon reloads exactly once — the intended behavior.
- **`--no-auto-reload` is unchanged.** It still fully disables self-reload.

## Related

- [Daemon mode](../daemon-mode.md) — the OODA loop this gate runs inside.
- [Run the OODA daemon](../howto/run-ooda-daemon.md) — start/flags/env.
- [Reconcile and self-deploy](../concepts/reconcile-and-self-deploy.md) —
  the merged-but-not-running detector and restart orchestration.
- [Multi-binary self-update](./multi-binary-self-update.md) — the update path
  that legitimately swaps the binary.
