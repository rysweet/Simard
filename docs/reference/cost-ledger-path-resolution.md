---
title: Cost-ledger path resolution API reference
description: >
  Reference for how Simard resolves the on-disk location of the cost-tracking
  ledger (`~/.simard/costs/ledger.jsonl`). Specifies the portable
  HOME → dirs::home_dir() → process-relative fallback chain in
  `ledger_path()`, its degrade-safe (no-panic) contract, the removal of the
  hardcoded `/home/azureuser` fallback, and the Unix permission posture on the
  directories and ledger file (#4363).
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../reference/telemetry-metrics.md
  - ../reference/daily-budget-display-guard.md
  - ../reference/state-root-resolution.md
  - ../reference/simard-cli.md
  - ../../src/cost_tracking.rs
---

# Cost-ledger path resolution API reference

> **Status: implemented.** The resolver lives in
> [`src/cost_tracking.rs`](https://github.com/rysweet/Simard/blob/main/src/cost_tracking.rs)
> as the private `ledger_path()` function. It replaces the previous hardcoded
> `/home/azureuser` HOME fallback with a portable resolution chain. Closes
> [#4363](https://github.com/rysweet/Simard/issues/4363).

Simard records estimated token usage and cost for each session turn into a
JSON-lines ledger. The **location** of that ledger is computed by
`ledger_path()`. Before #4363, an unset `HOME` environment variable caused
`ledger_path()` to fall back to the machine-specific literal
`/home/azureuser`, which is a portability and correctness defect on any host
where that path does not exist or is not writable by the running user. This
page specifies the resolver's finished behaviour.

## Contents

- [Resolution chain](#resolution-chain)
- [`ledger_path`](#ledger_path)
- [Degrade-safe contract](#degrade-safe-contract)
- [Filesystem layout and permissions](#filesystem-layout-and-permissions)
- [Behaviour matrix](#behaviour-matrix)
- [Observability](#observability)
- [Configuration](#configuration)
- [Security](#security)
- [Compatibility](#compatibility)
- [Tests](#tests)

## Resolution chain

`ledger_path()` resolves the ledger's parent home directory from the first
source that yields a usable (**non-empty**) path, in this order:

1. **`HOME` environment variable.** The unchanged, primary source on Unix,
   honored verbatim exactly as before #4363 (a non-empty value is used as-is,
   without re-checking absoluteness, to keep the fix strictly non-breaking). An
   empty `HOME` is treated as *unset* (it must not resolve to the filesystem
   root `/`). Sources 2 and 3 below always yield an absolute path.
2. **`dirs::home_dir()`.** The platform home-directory API (crate
   `dirs = "=6.0.0"`, already pinned in `Cargo.toml`). On Unix this consults
   the OS user database when `HOME` is absent; on Windows it resolves the known
   `Profile` folder. This is what makes resolution portable instead of
   machine-specific.
3. **Process-relative fallback.** When neither source yields a usable home, the
   ledger is written under a process-relative directory
   (`./.simard/costs/ledger.jsonl`) rather than any hardcoded absolute path.
   This keeps the fallback under a private, non-world-writable location owned by
   the current process and **never** references `/home/azureuser`, `/tmp`, or
   any other machine-specific or shared path.

> **Fallback is CWD-relative.** Because the fallback is process-relative, its
> ledger lands under the **current working directory** at write time. If Simard
> is invoked from different directories, fallback ledgers scatter (one
> `./.simard/costs/ledger.jsonl` per CWD), so a later summary may look partial —
> it only sees the ledger under the CWD it happens to run in. This is an
> intentional, warned last resort (a `tracing::warn!` fires); set `HOME` for a
> stable, single-location ledger.

The final path in every case is `<home>/.simard/costs/ledger.jsonl`, preserving
the pre-existing on-disk layout.

## `ledger_path`

```rust
/// Resolve the absolute path to the cost ledger,
/// `<home>/.simard/costs/ledger.jsonl`.
///
/// The home directory is resolved portably:
///   1. `HOME` (non-empty) — the unchanged primary source,
///   2. `dirs::home_dir()` — the platform home-directory API,
///   3. a process-relative `.simard/costs/ledger.jsonl` fallback.
///
/// Never panics and never returns the machine-specific `/home/azureuser`
/// literal. Emits a `tracing::warn!` when it must use the process-relative
/// fallback. Signature and return type are unchanged from before #4363.
fn ledger_path() -> PathBuf;
```

The signature — `fn ledger_path() -> PathBuf` — is **unchanged**. This is an
additive/non-breaking fix: every existing caller
([`record_cost`](https://github.com/rysweet/Simard/blob/main/src/cost_tracking.rs),
the daily/weekly summary readers) continues to call it exactly as before.

## Degrade-safe contract

`ledger_path()` is on the cost-recording path, which must never abort a session
turn. Its contract is therefore:

- **No panic.** Missing `HOME`, an unreadable user database, or a failed
  platform lookup all degrade to the next source in the chain — never an
  `unwrap()`/`expect()` panic.
- **Always returns a `PathBuf`.** The process-relative fallback guarantees a
  usable path even when every home source fails, so cost recording degrades to
  a local ledger rather than failing.
- **Warn, don't fail.** Using the process-relative fallback is an unusual,
  operator-visible condition, so it emits a `tracing::warn!` (structured, not
  `println!`) rather than being silent — but it does not return an error.

## Filesystem layout and permissions

The resolved layout is unchanged from prior releases:

```text
<home>/
└── .simard/
    └── costs/
        └── ledger.jsonl
```

On Unix, the ledger holds session cost telemetry and is treated as private:

- the `.simard/` and `costs/` directories are created with mode `0700`
  (owner-only), and
- `ledger.jsonl` is created with mode `0600` (owner read/write only).

**Attribution.** `ledger_path()` only *resolves and returns* the path — it does
not create anything. The directories and file (and their `0700`/`0600` modes)
are applied by the **writer**, `record_cost`, which owns the `create_dir_all` +
file-creation step. The implementer wires the permission calls into
`record_cost` (after `create_dir_all` and file creation), not into the resolver.
On non-Unix targets the writer relies on the platform default ACLs for the
user's profile directory.

## Behaviour matrix

| `HOME` | `dirs::home_dir()` | Resolved path |
| --- | --- | --- |
| `/home/alice` (set, absolute) | (not consulted) | `/home/alice/.simard/costs/ledger.jsonl` |
| unset / empty | `Some(/home/alice)` | `/home/alice/.simard/costs/ledger.jsonl` |
| unset / empty | `None` | `./.simard/costs/ledger.jsonl` (process-relative; `warn!` emitted) |
| `/` (root only) | — | rejected as "empty"; falls through to `dirs` then the process-relative fallback |

The critical invariant proven by #4363: **no input combination yields the
hardcoded `/home/azureuser` literal.** The value no longer appears anywhere in
`ledger_path()` or its module.

## Observability

- The process-relative fallback path emits a `tracing::warn!` event (target
  `cost_tracking`) noting that no home directory could be resolved and that the
  ledger is being written process-relative. No stray `print!`/`println!` is
  used.
- The daily/weekly cost summaries that read the ledger are unaffected in the
  normal (`HOME`-resolved) case: they read whatever path `ledger_path()`
  returns. In the process-relative fallback case, summaries only see the ledger
  under the current working directory (see the CWD caveat in
  [Resolution chain](#resolution-chain)).

## Configuration

There is **no new environment variable or config key.** `ledger_path()`
continues to honour `HOME` exactly as before when it is set; the only change is
what happens when it is *not* set. Operators who need a specific ledger location
should set `HOME` (the primary source) as they always have.

## Security

- **Untrusted env input.** `HOME` and `dirs::home_dir()` are treated as
  untrusted: the resolver **joins** path components with `PathBuf::join` and
  never shell-concatenates or `eval`s them.
- **Empty-value rejection.** An empty `HOME` is rejected rather than resolved to
  the filesystem root `/`, preventing an accidental write of a private ledger to
  a world-readable location.
- **Fallback stays private.** The process-relative fallback is under a
  process-owned `.simard/` directory — never `/tmp` or another world-writable
  directory. On Unix the creating writer (`record_cost`) applies `0700`/`0600`
  modes; the resolver itself only returns the path.
- **No panic / DoS-safe.** Because the resolver never panics, a hostile or
  minimal environment (no `HOME`, no user database) cannot crash the
  cost-recording path.

## Compatibility

- **Additive / non-breaking.** Public/module signature (`fn ledger_path() ->
  PathBuf`), the on-disk layout (`~/.simard/costs/ledger.jsonl`), and the
  behaviour when `HOME` is set are all preserved.
- **Scope.** The fix is confined to `ledger_path()` in `src/cost_tracking.rs`.
  An identical hardcoded-`/home/azureuser` pattern in
  `src/runtime_config.rs` is **out of scope** for #4363 and is tracked as a
  separate follow-up so this change stays surgical and avoids merge collisions
  in the shared OODA-core group.

## Tests

In-module `#[cfg(test)]` tests in `src/cost_tracking.rs` cover:

- **HOME-unset** — with `HOME` removed, the resolved path contains **no**
  `/home/azureuser` substring and ends in `.simard/costs/ledger.jsonl`.
- **HOME-set (unchanged)** — with `HOME=/tmp/simard-test-home` (or similar),
  the resolved path is exactly `<HOME>/.simard/costs/ledger.jsonl`.
- **Empty HOME** — an empty `HOME` does not resolve to `/.simard/...`.

Because these tests mutate the process-global `HOME` environment variable, they
serialize through a `static Mutex` (or `#[serial]`) so they cannot race other
env-reading tests and flake.
