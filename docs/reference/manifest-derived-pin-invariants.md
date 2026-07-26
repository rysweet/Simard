---
title: "Manifest-derived pin invariants (test API)"
description: Reference for the issue #1018 refactor of tests/issue_2626_amplihack_pin_bump.rs — deriving the expected crate version and dependency pins from Cargo.toml / Cargo.lock (single source of truth) and asserting supply-chain invariants (full-SHA pins, rysweet remote allowlist, lock parity, single lbug engine) instead of frozen-literal equality, so the workflow-publish step-14 version bump no longer blocks publish.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#1018"]
related:
  - ../concepts/manifest-derived-pin-invariants.md
  - ./amplihack-pin-bump-2626.md
  - ../howto/self-maintain-dependency-pins.md
  - ./dependency-trust-policy.md
  - ./supply-chain-audit.md
  - ../reference/amplihack-freshness-gate.md
---

# Manifest-derived pin invariants (test API)

Issue #1018 refactors `tests/issue_2626_amplihack_pin_bump.rs` so its
expectations are **derived from the manifest** rather than frozen as literals.
The single source of truth is `Cargo.toml` / `Cargo.lock`. The test asserts
supply-chain **invariants** over that manifest, none of which reference a version
string — so the workflow-publish step-14 version bump can no longer turn the
test red and block publish.

> **Constraints.** Std-only, offline (no network, no `git ls-remote`), file-
> shaped. No `print!`/`println!`; no `bridge`/`Bridge` naming.

## What is derived, not hardcoded

| Fact | Old (frozen) | New (derived from SoT) |
| --- | --- | --- |
| Simard's own version | Hardcoded string constant | `env!("CARGO_PKG_VERSION")` / parsed from `Cargo.toml` |
| Expected dependency pins | Hardcoded 40-char SHA constants | Read from `Cargo.toml`/`Cargo.lock` and checked for invariants |

The test no longer contains a `const … _TARGET_REV: &str = "…"` that a bump must
chase.

## Helpers (std-only manifest readers)

```rust
/// Parse the root `Cargo.toml` into a queryable manifest (std + a toml reader,
/// no network, no cargo invocation).
fn read_cargo_toml(root: &Path) -> Manifest;

/// Parse `Cargo.lock` into the resolved package set.
fn read_cargo_lock(root: &Path) -> Lockfile;

/// The crate's own version, from the manifest — the SoT, never a literal.
fn own_version() -> &'static str { env!("CARGO_PKG_VERSION") }

/// The git-rev pin recorded for a dependency in `Cargo.toml`.
fn manifest_pin(manifest: &Manifest, crate_name: &str) -> GitPin;

/// The resolved rev for a dependency in `Cargo.lock`.
fn lock_pin(lock: &Lockfile, crate_name: &str) -> GitPin;
```

## Invariants asserted

Each is a property of the manifest, independent of any version string:

```rust
/// A pin is a full 40-char lowercase hex commit SHA — never a branch or tag.
fn assert_pin_is_full_sha(pin: &GitPin);

/// A crate resolves only from its allowlisted rysweet/<repo> git remote
/// (anti-typosquat / anti-source-swap).
fn assert_remote_allowlisted(pin: &GitPin, allowed: &[&str]);

/// `Cargo.toml` and `Cargo.lock` agree on the crate's rev (anti-downgrade /
/// lockfile parity).
fn assert_toml_lock_parity(manifest: &Manifest, lock: &Lockfile, crate_name: &str);

/// Exactly one `lbug` engine line resolves in the lockfile (the lbug lockstep —
/// one engine, one on-disk store format).
fn assert_single_lbug_engine(lock: &Lockfile);
```

The suite runs these for each `amplihack-*` git-pinned crate
(`amplihack-agent-eval` from `rysweet/amplihack-rs`, `amplihack-memory` from
`rysweet/amplihack-memory-lib`) plus the direct `lbug` pin.

## Behavior across a version bump

- **Before bump / after bump:** the test derives `own_version()` and the pins
  from the current manifest each run, so a step-14 increment changes nothing the
  test asserts — it stays **green**. Publish proceeds.
- **On a real regression:** a pin expressed as a branch/tag, a foreign remote, a
  `Cargo.toml`↔`Cargo.lock` mismatch, or a second `lbug` engine line fails the
  corresponding invariant loudly.

## Determinism guarantees

- Reads only local `Cargo.toml` / `Cargo.lock` via `std` — no network, no
  `git ls-remote`, no cargo/toolchain invocation.
- An operator running the equivalent `grep`/`rg` over the manifest gets the same
  verdict CI does.
- Decoupled from the heavy `simard` (LadybugDB) build.

## Tests

`tests/issue_2626_amplihack_pin_bump.rs` (refactored): the pin-invariant
assertions above, all derived from the manifest. The suite passes after a
step-14 version bump with no edit to the test source (issue #1018 acceptance),
and fails on any supply-chain pin regression.
