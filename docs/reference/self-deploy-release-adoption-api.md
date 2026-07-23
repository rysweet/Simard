---
title: "Self-deploy release-adoption API"
description: >
  Typed surface, configuration, and outcome taxonomy for the fail-closed semver
  adoption gate and the self-test-gated auto-adoption trigger that detect and
  adopt a newer published release through the hardened safe-update chain.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: reference
status: planned
related:
  - ../concepts/self-deploy-release-adoption.md
  - ./update-check.md
  - ./self-deploy-api.md
  - ./multi-binary-self-update.md
  - ../safe-self-update.md
  - ../../src/update_check.rs
  - ../../src/cmd_self_update/update.rs
  - ../../src/cmd_self_update/release.rs
---

# Self-deploy release-adoption API

> **Status: planned (spec).** This page documents the surface **to be built**.
> Today `is_newer` is a private helper in `update_check.rs` and both self-update
> entry points still gate on string equality (`update.rs:70` and `:128`). The
> signatures below describe the target state.

This page documents the surface that turns a detected newer GitHub release into a
running upgrade. See the [concept page](../concepts/self-deploy-release-adoption.md)
for the motivation.

## Semver adoption gate

```rust
/// Return `true` iff `latest` is a valid semver strictly greater than
/// `current`. Panic-safe / fail-closed for adoption: any parse failure
/// (malformed tag, unexpected prefix) yields `false`, which the adoption gate
/// treats as "do not adopt" — a bad tag is never coerced into "newer".
///
/// Visibility: promoted from private `fn` to `pub(crate)` so the adoption
/// trigger in `cmd_self_update` can call the same authoritative predicate.
pub(crate) fn is_newer(current: &str, latest: &str) -> bool;
```

- Location: [`src/update_check.rs`](https://github.com/rysweet/Simard/blob/main/src/update_check.rs).
- Both operands are parsed with the `semver` crate; a leading `v` is stripped
  before comparison.
- This predicate is the **single** authority for "should we adopt?". The
  launch-time notice and **both** self-update paths call it, so the notice and
  the action can never disagree.
- **Terminology:** the existing doc-comment says "fail-open" — that means
  *panic-safe* (returns `false` instead of aborting on bad input). For the
  adoption gate that is **fail-closed** (returns `false` ⇒ no adoption). The
  implementation reconciles the comment wording; behavior is unchanged.

### Adoption gate in both self-update paths

Both `handle_self_update` (`update.rs:70`) and `handle_self_update_download_only`
(`update.rs:128`, the safe-update path) decide "already current" with `is_newer`,
not string equality:

```rust
// Fail-closed: adopt only when the remote tag is a valid, strictly-newer semver.
if !is_newer(CURRENT_VERSION, &version) {
    println!("Already at the latest version (v{CURRENT_VERSION}).");
    return Ok(());
}
```

This replaces the former `version == CURRENT_VERSION` short-circuit **in both
functions**, which could let a tag-shape difference (e.g. a `v` prefix) mis-read
a strictly-newer release. Hardening only one path would leave the safe-update
path fragile.

## Auto-adoption trigger

The trigger detects a newer release and adopts it non-interactively, reusing the
hardened safe-update chain. It performs no work when already current.

```rust
/// Detect the latest release and, if strictly newer, adopt it through the
/// hardened safe-update chain (download → checksum → cosign → atomic install →
/// self-test → relaunch). Short-circuits at `AlreadyLatest`; frequency-bounded;
/// logs the outcome via `tracing` only. Never bypasses checksum/signature/
/// self-test. On self-test failure the `.old` binary stays authoritative.
pub fn adopt_latest_release() -> AdoptionOutcome;
```

Internally it:

1. Resolves the latest release via `find_latest_release()`
   ([`src/cmd_self_update/release.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/release.rs)),
   which returns a `(String, String)` tuple in the order **`(url, version)`** —
   the same order `handle_self_update` destructures (`let (url, version) = …`).
2. Applies the `is_newer` gate; returns `AlreadyLatest` when not newer.
3. Calls `download_and_replace(&url, &version)` — the same chain
   `simard self-update` uses.
4. Runs `run_self_test_on_binary` on the installed binary; on failure returns
   `SelfTestFailed` and does **not** relaunch.
5. Execs into the new binary on success.

<a id="outcomes"></a>

### Outcome taxonomy

`AdoptionOutcome` is a structured enum; every variant is logged with `tracing`
fields and an OTel span. No variant emits a stray `print!`/`println!` from
library code.

| Variant | Meaning | Terminal? |
|---------|---------|-----------|
| `Adopted { from, to }` | Newer release installed, self-tested, relaunched. | yes (relaunch) |
| `AlreadyLatest { version }` | `is_newer` was false; no download performed. | yes |
| `SelfTestFailed { version, reason }` | New binary installed but failed `gym run-suite starter`; `.old` binary restored/kept authoritative, no relaunch. | yes (surfaced) |
| `DeferredOperational { reason }` | Root cause is operational (host must pull/restart, channel pinned), **not** a code defect. Surfaced, not forced into a code change. | yes (documented) |
| `MalformedRemoteTag { tag }` | Remote tag failed to parse; fail-closed, no update. | yes |
| `DownloadFailed { version, reason }` | Safe-update chain rejected the asset (checksum/cosign/transport). No install. | yes |

`DeferredOperational` is a **success**: a correct "no code change needed, here is
the operational action" result. It is not counted as a failure.

## Configuration

| Env var | Effect | Default |
|---------|--------|---------|
| `SIMARD_NO_UPDATE_CHECK=1` | Skip the release check entirely (no cache/network/notice/adoption). | unset |
| `SIMARD_NONINTERACTIVE=1` | Print the notice but suppress interactive upgrade prompts; auto-adoption still honors its own gate. | unset |
| `XDG_CONFIG_HOME` | Location of the 24h release-check cache (`…/simard/update_cache.json`). | `~/.config` |

The check/adoption cache is fresh for 24h; a fresh cache skips the network
entirely, so the frequency bound and the cache together prevent an adoption loop.

## Preserved security controls

The trigger routes **only** through `download_and_replace`, preserving:

- SHA-256 checksum verification of the downloaded asset.
- Cosign keyless verification with pinned issuer and identity.
- Https-only transport (`--proto =https --proto-redir =https --tlsv1.2`, `--`
  terminator, arg-vector `Command` — never `sh -c`).
- Self-test gate before exec relaunch.
- Atomic install with `.old` backup, restore-on-failure, `0o755` mode.

`tag_name` and asset names are untrusted input, validated via `strip_prefix("v")`
+ `is_newer` and never interpolated into a shell. New tracing logs carry only
version strings and outcome enums — no tokens, no URLs with query strings.

## Tests

Covered in
[`src/cmd_self_update/tests.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/tests.rs):

- `is_newer` returns `false` for a malformed remote tag (fail-closed).
- `is_newer` accepts a `v`-prefixed strictly-newer tag and rejects equal/older.
- The adoption trigger short-circuits (`AlreadyLatest`) when not newer.
- The trigger cannot install/relaunch when the self-test fails.
