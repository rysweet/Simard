---
title: Idempotent Release Publishing
description: "How the release workflow publishes GitHub Releases idempotently — re-checking authoritative remote state before creating so a concurrent or duplicate run of an unbumped version cannot red an otherwise-green build/sign pipeline."
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: reference
status: active
---

# Idempotent Release Publishing

The `release` workflow (`.github/workflows/release.yml`) builds, signs, and
publishes a GitHub Release for the version in `Cargo.toml` on every push to
`main`. Publishing is **idempotent**: a version that is already released is a
no-op success, never a workflow failure.

## Why (the failure this prevents)

The workflow's `Check if tag exists` step (`tag_check`) reads a **local**
`git rev-parse` snapshot taken *before* the ~11-minute release build. When the
same, unbumped version is pushed to `main` more than once, the `release` runs
race:

1. The first run publishes `vX.Y.Z`.
2. Every other run captured a stale `exists=false` at `tag_check` time, so it
   builds and signs the full payload, then reaches `Create release` and dies on:

   ```text
   gh release create ... a release with the same tag name already exists: vX.Y.Z
   ```

That reddened an otherwise fully-green build + sign pipeline. It was observed on
the `#4505` push (run `30048895934`): every build, test, SBOM, and cosign step
passed — only `Create release` failed, because the immediately preceding push at
the same `0.37.0` version had already published the release. (The failure was
initially mis-attributed to the deploy-gate canary unit tests; those are the
`verify` workflow and passed.)

## How it works

`Create release` delegates to
[`scripts/release-create.sh`](https://github.com/rysweet/Simard/blob/main/scripts/release-create.sh),
which:

1. **Re-checks the authoritative remote state** with `gh release view "$TAG"`
   immediately before creating — not an 11-minute-old local snapshot. If the
   release already exists, it logs an idempotent-skip notice and exits `0`.
2. Runs `gh release create`. If that races with a concurrent run and fails with
   `already exists`, it is treated as an idempotent success (exit `0`).
3. **Fails loudly** on any other `gh` error (fail-closed): a genuine publish
   failure never masquerades as success.

## Regression gate

[`scripts/qa-release-create-idempotent.sh`](https://github.com/rysweet/Simard/blob/main/scripts/qa-release-create-idempotent.sh)
exercises the real script against a hermetic, on-`PATH` mock `gh` (no network)
and asserts all outcomes: pre-existing release → `0`, fresh publish → `0`,
create/create race → `0`, genuine failure → non-zero, and the missing-assets
guard. It runs as the **Release-create idempotency gate** step in the `verify`
workflow, so a regression to non-idempotent publishing fails CI on every PR.
