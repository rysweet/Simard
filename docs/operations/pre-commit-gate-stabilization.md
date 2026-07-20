---
title: Pre-commit gate stabilization (shared-source root cause)
description: >
  Operations reference for the fix that restored the required `pre-commit`
  check to green across the open PR fleet. Documents the shared root cause in
  the `verify.yml` pre-commit job (lbug native-static-library link-path
  provisioning drift feeding the release clippy `-D warnings` gate), the
  source-level fix, why quality gates were NOT weakened to force green, and how
  to reproduce the gate locally on a clean `main`.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./pre-commit-setup.md
  - ../reference/amplihack-pin-bump-2626.md
  - ../howto/fix-a-no-bridge-naming-guard-failure.md
  - ../../.github/workflows/verify.yml
  - ../../scripts/clippy-precommit-release.sh
  - ../../scripts/provision-lbug-prebuilt.sh
  - ../../scripts/check-rust-only-gate.sh
---

# Pre-commit gate stabilization (shared-source root cause)

> **Status: implemented.** The required `pre-commit` check in
> [`.github/workflows/verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml)
> is green on `main` and across the PR fleet. The failure was a **single shared
> root cause** in the pre-commit job's toolchain provisioning, fixed at the
> source — **not** by loosening any gate.

A cluster of open PRs all red on the same required `pre-commit` check is a
systemic signal: the failure lives in shared CI config/tooling, not in each PR's
diff. #4355, #4354, #4328, #4325, #4324, and #4322 are treated as sharing this
root cause **once re-running their CI reproduces the same `lbug` link signature**
(see [Reproduce on clean `main`](#reproduce-on-clean-main)) — not asserted purely
from the clustering. #4331 additionally shows a `coverage` red handled
[separately below](#coverage-on-4331). This page documents the shared surface,
the fix, and how to reproduce it locally.

## Contents

- [Symptom](#symptom)
- [Reproduce on clean `main`](#reproduce-on-clean-main)
- [Root cause](#root-cause)
- [The fix (at the source)](#the-fix-at-the-source)
- [What was explicitly NOT done](#what-was-explicitly-not-done)
- [Coverage on #4331](#coverage-on-4331)
- [Verifying green](#verifying-green)
- [Related gates](#related-gates)

## Symptom

The `pre-commit` job (the commit-stage gate mirrored in CI — Rust-only gate,
`cargo fmt --all -- --check`, and the release clippy
`scripts/clippy-precommit-release.sh` under `-D warnings`) failed on multiple
independent PRs whose diffs did not touch any lint- or format-relevant code.
The clustered, cross-PR pattern is the tell: a per-PR mistake fails one PR; a
shared-config regression fails them all at once.

## Reproduce on clean `main`

The gate is reproduced by running the **exact** commands the `pre-commit` job
runs, on a clean checkout of `main`, before changing anything:

```bash
# From a clean checkout of origin/main:
scripts/check-rust-only-gate.sh                 # Rust-only gate
cargo fmt --all -- --check                       # format gate
scripts/clippy-precommit-release.sh              # release clippy (-D warnings) w/ lbug link wrapper
```

Reproducing locally is what distinguishes a **tooling/config drift** (fix at
the source) from **genuine lint/format violations** (fix the code). The failure
reproduced on clean `main` with no PR diff applied, confirming a shared-source
regression rather than per-PR violations.

## Root cause

The pre-commit job's release clippy step runs through
`scripts/clippy-precommit-release.sh`, which must first put the `lbug`
(LadybugDB) **native static library** (`liblbug.a`) on the linker search path.
CI provisions this via `scripts/provision-lbug-prebuilt.sh` into
`~/.cache/simard-lbug-precommit/lib` and points `cargo` at it (issue #2426 /
#2423). When that provisioning drifts — a changed cache key, a stale cached
`lbug-*` build output, or an lbug pin bump that invalidates the prebuilt — the
`cargo clippy --release` link step fails with

```text
error: could not find native static library `lbug`
```

which reds the `pre-commit` check. Because the provisioning is **shared** by
every PR's CI run, the failure appears fleet-wide simultaneously — matching the
observed cluster. (The existing `#2426` regression-guard step in `verify.yml`
detects this exact signature.)

## The fix (at the source)

The fix repairs the shared provisioning so the release clippy links `lbug`
deterministically again:

- realign the `provision-lbug-prebuilt.sh` output location and the cargo
  external-static-lib link path used by `clippy-precommit-release.sh`,
- drop the stale cached `lbug-*` build/fingerprint outputs so the build script
  re-runs and adopts the freshly provisioned `liblbug.a`, and
- keep the lbug pin deterministic so the cache key and prebuilt stay in sync.

The change is confined to the shared CI/toolchain surface (`verify.yml` and the
provisioning/clippy wrapper scripts). Once the link path resolves, the release
clippy runs to completion and the `-D warnings` gate evaluates real lints again.

## What was explicitly NOT done

The gate was restored by fixing the **root cause**, not by weakening quality:

- **No `-D warnings` downgrade.** Clippy still fails the build on any warning.
- **No `--no-verify` / gate bypass.** The `--no-verify` ban and the
  no-`--admin`-merge policy are unchanged.
- **No Rust-only-gate relaxation.** New `.py`/`.js`/`.ts` outside the
  allow-list are still rejected.
- **No workflow-permission widening and no new secrets.** The lbug provisioning
  does not echo secrets into public CI logs.
- **PRD preserved.** No product behaviour changed; this is a CI/toolchain repair
  only.

## Coverage on #4331

PR #4331 additionally showed the `coverage` check failing. Per the
reproduce-first discipline: the coverage failure is folded into this fix **only
if it shares this root cause** (e.g. the same failed `lbug` link aborting the
coverage build). If #4331's coverage red is unrelated to the pre-commit
provisioning drift, it is split into a separate, PR-specific follow-up rather
than bundled here.

## Verifying green

After the fix, the same three commands pass on a clean `main`, and the
`verify.yml` `pre-commit` job — including the **#2426 regression guard** step
that asserts the pre-commit clippy actually compiled/linked `lbug` — is green.
Re-running CI on the previously-red PRs turns the required `pre-commit` check
green without any change to their diffs, confirming the root cause was shared.

## Related gates

For how the same commands run locally as native git hooks (and how local
enrollment mirrors this CI job), see
[Local Commit Gates](./pre-commit-setup.md). The lbug/#2426 link-path history is
documented alongside the
[amplihack pin bump (#2626)](../reference/amplihack-pin-bump-2626.md) context.
