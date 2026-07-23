---
title: pre-commit CI determinism (verify.yml)
description: Reference for how the `pre-commit` job in `.github/workflows/verify.yml` is made deterministic — content/lockfile-hashed cache keys for the shared cargo and `~/.cache/simard-lbug-precommit` caches, pinned tool/lbug versions, and isolation of the non-deterministic `cargo test` step — so the check produces the same result on every run instead of failing-then-passing on re-run (rysweet/Simard PR#4507, PR#4433). Flakiness is root-caused, never masked by CI retries.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../operations/pre-commit-setup.md
  - ./ci-health-sweep.md
  - ./release-integrity.md
  - ../concepts/coverage-comment-transient-resilience.md
  - ../../.github/workflows/verify.yml
---

# pre-commit CI determinism (verify.yml)

> **Status: implemented.** The `pre-commit` job lives in
> [`.github/workflows/verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml).
> "pre-commit" here is the **CI job named `pre-commit`**, not a Python
> `pre-commit` framework — Simard has **no `.pre-commit-config.yaml` and no
> Python runtime** (see [Local Commit Gates](../operations/pre-commit-setup.md)).

The `pre-commit` CI check was **intermittently flaky**: PR#4507 recorded a
`pre-commit` FAILURE on one run and SUCCESS on a re-run of the same check/PR,
and PR#4433 showed the same fail-then-pass-on-rerun pattern. The fail-then-pass
signature points to **non-determinism in the job's environment**, not to per-PR
content. `cargo fmt` and `cargo clippy` are deterministic, so the suspects were
the **shared caches** and the **`cargo test` step**.

This reference specifies the finished, deterministic contract for the
`pre-commit` job. The fix root-causes the flakiness; it does **not** add CI
retries to mask it.

## Contents

- [The `pre-commit` job at a glance](#the-pre-commit-job-at-a-glance)
- [Deterministic cache keys](#deterministic-cache-keys)
- [Pinned tool & lbug versions](#pinned-tool--lbug-versions)
- [Isolating the non-deterministic test](#isolating-the-non-deterministic-test)
- [No CI retries as a fix](#no-ci-retries-as-a-fix)
- [Verifying determinism](#verifying-determinism)

## The `pre-commit` job at a glance

The job (`runs-on: ubuntu-latest`) runs, in order:

1. **Rust-only gate** — reject new `.py`/`.js`/`.ts` files.
2. **Cache restore** — shared cargo registry/build cache (`Swatinem/rust-cache`)
   plus the `~/.cache/simard-lbug-precommit` directory.
3. **lbug native static-lib provisioning** (`#2426`/`#2423`).
4. **`cargo fmt --all -- --check`** (deterministic).
5. **`cargo clippy --release … (lbug link wrapper)`** (deterministic).
6. **`cargo test --all-features --locked --no-fail-fast`** — the flaky surface.
7. Full clippy gate, minimal-binary contract build, downstream binary build,
   coin-gym verify, artifact upload.

Steps 4–5 are pure functions of the source. The determinism work targets the
**cache** (step 2/3) and the **test** (step 6).

## Deterministic cache keys

The shared caches are keyed on **content/lockfile hashes**, so a cache entry can
only be reused when the inputs that produced it are identical. This removes the
cache-warm race where a partially-populated or mismatched cache made a build
pass on one run and fail on the next.

- **Cargo cache** (`Swatinem/rust-cache`): keyed via a stable
  `shared-key` plus the action's built-in hashing of `Cargo.lock` /
  `Cargo.toml`, so an entry is reused only when those inputs are identical.
  `cache-on-failure: true` is retained so a failed compile still seeds the next
  run deterministically. **Optionally** (a refinement beyond the core
  content-hashed-key requirement, adopt only if cross-PR cache thrashing is
  observed) a **single writer** — the `main` branch — refreshes the shared cache
  while PR runs read but never overwrite it; the determinism guarantee does not
  depend on this and it can be omitted.
- **`~/.cache/simard-lbug-precommit`**: restored via the cache action's
  `cache-directories` and keyed so its contents are a deterministic function of
  the **pinned lbug version** (below). The provisioning step runs **after** the
  cache restore and deterministically re-establishes the prebuilt
  `liblbug.a` link path, so a restored-but-evicted release artifact can never
  leave the build in a half-populated, order-dependent state.

The cache population is **race-free and order-deterministic**: provisioning
always follows restore, and the key changes whenever the pinned inputs change.

## Pinned tool & lbug versions

Every externally-fetched tool the `pre-commit` job depends on is **pinned to an
exact version** so the same job never resolves a different toolchain on a later
run:

- the **lbug** native static library is pinned to an exact version (e.g.
  `lbug 0.17.1`) and its prebuilt `liblbug.a` is provisioned from the
  version-keyed cache — the version-pinned download happens **once per cache
  lifetime**, not per run;
- action versions are pinned by commit SHA (e.g. `Swatinem/rust-cache@…`);
- the job relies on the same stable Rust toolchain CI already standardizes on.

No step performs an **unpinned** fetch-and-execute of a remote artifact.

## Isolating the non-deterministic test

The `cargo test` step is made deterministic by removing the source(s) of
non-determinism rather than by re-running until green:

- any test that depended on **wall-clock timing, ambient environment
  (`$HOME`/state root), network access, or a shared on-disk path** is made
  hermetic — it derives its state root from a per-test `TempDir` and is
  serialized where it mutates process-global state (see
  [Deploy-gate self-deploy test state-root robustness](./deploy-gate-drop-test-state-root-robustness.md));
- tests that raced under `cargo test`'s default thread-level parallelism are
  serialized with `#[serial_test::serial(...)]` on the shared resource, so
  concurrent runs cannot interfere;
- the step keeps `--locked` (no dependency resolution drift) and
  `--all-features`.

After isolation, the `cargo test` step produces the **same pass/fail result on
every run** for a given commit.

## No CI retries as a fix

Automatic step/job **retries are not used to paper over flakiness**. A flaky
result is treated as a bug to root-cause (cache race, unpinned version, or a
non-deterministic test), not a transient to be retried away. This keeps the
`pre-commit` check a trustworthy signal: a red `pre-commit` means a real defect,
not "try again."

> Transient-error resilience that *is* legitimate (e.g. tolerating a flaky
> third-party comment API) is scoped and documented separately; it does not
> apply to the `pre-commit` correctness gates.

## Verifying determinism

To confirm the fix, run the `pre-commit` gates repeatedly on the same commit and
expect an identical result each time:

```bash
# Mirror the CI pre-commit gates locally, three times, same commit.
# NOTE: the CI job runs clippy through the lbug link wrapper (see step 5 above);
# the bare `cargo clippy` below is a convenience approximation. To reproduce the
# job faithfully, invoke clippy via the same lbug-linked wrapper CI uses.
for i in 1 2 3; do
  echo "=== run $i ==="
  cargo fmt --all -- --check \
    && cargo clippy --release --no-deps -- -D warnings \
    && cargo test --all-features --locked --no-fail-fast \
    && echo "run $i: OK" || echo "run $i: FAIL"
done
```

Three identical results (all OK, or all failing on the *same* deterministic
cause) confirm the flakiness is root-caused. In CI, re-running the `pre-commit`
check on an unchanged commit must no longer flip FAILURE↔SUCCESS.
