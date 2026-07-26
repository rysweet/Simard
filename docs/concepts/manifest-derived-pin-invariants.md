---
title: "Concept: manifest-derived pin invariants (single source of truth)"
description: Intended behavior for issue #1018 — the amplihack pin-bump acceptance test derives the expected dependency pins from the Cargo.toml / Cargo.lock manifest (the single source of truth) and asserts supply-chain invariants, instead of hardcoding a frozen SHA that the workflow-publish step-14 version bump then breaks.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: concept
issues: ["#1018"]
related:
  - ../reference/manifest-derived-pin-invariants.md
  - ../reference/amplihack-pin-bump-2626.md
  - ../howto/self-maintain-dependency-pins.md
  - ./amplihack-freshness-gate.md
  - ../reference/dependency-trust-policy.md
  - ../reference/supply-chain-audit.md
---

# [PLANNED - Implementation Pending] Concept: manifest-derived pin invariants (single source of truth)

This document describes the intended feature behavior for issue #1018.

The dependency-pin acceptance test
(`tests/issue_2626_amplihack_pin_bump.rs`) hardcodes the **expected pin literals**
as `const … _TARGET_REV`/`_STALE_REV` constants and asserts the manifest's rev
equals them. Every time the `amplihack-*` dependencies are bumped, those frozen
constants must be manually chased in lock-step or the test turns red; and where
the test also freezes Simard's **own version**, the workflow-publish **step-14**
version bump invalidates that literal and blocks automated publish. In both
cases the root cause is the same: a copy of a fact (the pin, the version) is
frozen in the test source alongside the authoritative copy in the manifest, and
the two drift apart.

The fix removes the conflict at its root: make the test **derive** what it
expects from the manifest — the single source of truth — rather than freezing a
copy of it in the test source. Two things that describe the same fact can no
longer disagree, because there is now only one thing.

## Single source of truth

`Cargo.toml` and `Cargo.lock` are the authoritative record of Simard's
dependency pins and her own version. The acceptance test must read those files
and assert **properties** of them, never re-state their contents as literals:

- the crate's own version comes from the manifest (e.g. `env!("CARGO_PKG_VERSION")`
  or a direct `Cargo.toml` read), not a hardcoded string the bump would
  invalidate;
- the expected dependency pins are read from `Cargo.toml`/`Cargo.lock` and
  checked for the invariants that actually matter (below), not compared against
  a frozen SHA constant.

When step-14 bumps the version, the test re-derives from the same manifest and
stays green — publish is no longer self-blocking.

## From frozen equality to supply-chain invariants

The test still exists to protect the same supply-chain guarantees the
[dependency-trust policy](../reference/dependency-trust-policy.md) requires; it
just expresses them as invariants over the manifest instead of equality against
a literal:

| Invariant | What it protects |
| --- | --- |
| Every pin is a **full 40-char commit SHA** — never a branch or tag | Reproducibility; no moving target (anti-mutable-ref) |
| Each `amplihack-*` crate resolves only from its **allowlisted `rysweet/…` remote** | Anti-typosquat / anti-source-swap |
| `Cargo.toml` and `Cargo.lock` **agree** on each pin | Anti-downgrade / lockfile parity |
| Exactly **one** `lbug` engine line resolves (the lockstep) | One on-disk store format; no dual-engine link |

None of these invariants reference a specific version string, so none is
disturbed by step-14's bump. They fail loudly only on a real supply-chain
regression (a mutable ref, a foreign remote, a lockfile mismatch, a second
engine), which is exactly when the gate should fire.

## Deterministic and offline

The test reads the raw `Cargo.toml` / `Cargo.lock` with `std` only — **no
network, no `git ls-remote`, no toolchain, no crate import**. An operator
running the equivalent `grep` gets the same answer CI does, the check stays
decoupled from the heavy `simard` build, and it can never flake on network
conditions. This preserves the file-shaped property the existing
`issue_2626_amplihack_pin_bump.rs` was written to have.

## Why not just update the literal on every bump?

Because that re-creates the bug on the next bump. Hardcoding the expected
version couples an independent, automated action (step-14 version bump) to a
manual edit of an unrelated test. Deriving from the manifest decouples them
permanently: the version bump and the pin test can no longer conflict, so
automated publish proceeds without human reconciliation.

## Acceptance behavior

- After a step-14 version bump, `tests/issue_2626_amplihack_pin_bump.rs` passes
  without any edit to the test source — publish is unblocked.
- A pin regression (branch/tag instead of SHA, a non-`rysweet` remote, a
  `Cargo.toml`↔`Cargo.lock` mismatch, or a second `lbug` line) still fails the
  test loudly.
- The test remains offline and file-shaped (std-only, no network), and adds no
  `print!`/`println!` or `bridge`/`Bridge` naming.
