---
title: How to fix CI linker OOM for LadybugDB builds
description: Prevent out-of-memory linker failures when building the lbug crate (LadybugDB/KuzuDB C++ compilation) on GitHub Actions runners.
last_updated: 2026-04-14
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/simard-cli.md
  - ../howto/reclaim-disk-space-and-run-low-space-rust-builds.md
---

# How to fix CI linker OOM for LadybugDB builds

Simard's `lbug` crate compiles KuzuDB/LadybugDB C++ via `build.rs`. On
GitHub Actions `ubuntu-latest` runners (7 GB RAM), parallel linker
invocations during a cold cargo build can exceed available memory and get
OOM-killed. This guide documents the three mitigations applied in
`.github/workflows/verify.yml`.

## Problem

The default `cargo build` spawns one linker per CPU core. Each C++ link
step for the `lbug` crate consumes ~2–3 GB of resident memory. With 4
parallel jobs on a 7 GB runner, peak memory exceeds available RAM and the
kernel OOM-kills the linker, producing errors like:

```text
error: could not compile `lbug` (build script)
...
collect2: fatal error: ld terminated with signal 9 [Killed]
```

## Mitigations

Three complementary fixes are applied to **both** the `pre-commit` and
`install-smoke` jobs:

### 1. Limit cargo parallelism

Set `CARGO_BUILD_JOBS=2` as a job-level environment variable:

```yaml
jobs:
  pre-commit:
    env:
      CARGO_BUILD_JOBS: 2
```

This caps parallel rustc/linker invocations at 2, reducing peak memory
from ~12 GB to ~6 GB. Build time increases ~20% — an acceptable trade-off
versus OOM failures.

### 2. Add swap space

A 4 GB swap file provides headroom for transient memory spikes:

```yaml
- name: Set up swap space (4 GB)
  run: |
    sudo fallocate -l 4G /swapfile
    sudo chmod 600 /swapfile
    sudo mkswap /swapfile
    sudo swapon /swapfile
```

This step runs before `rust-cache` and cargo operations. It adds ~2
seconds to job setup time.

### 3. Optimize rust-cache

`Swatinem/rust-cache@v2` is configured with a shared key and restricted
save policy:

```yaml
- name: Cache cargo registry and build artifacts
  uses: Swatinem/rust-cache@v2
  with:
    shared-key: simard-verify
    save-if: ${{ github.ref == 'refs/heads/main' }}
```

- **`shared-key: simard-verify`** — both jobs share the same cache,
  increasing hit rates for the expensive `lbug` C++ compilation artifacts.
- **`save-if`** — only the `main` branch writes cache entries. PR builds
  read but do not write, preventing cache pollution from feature branches.

## Verifying the fix

After pushing to a branch with these changes:

1. Open the Actions tab and watch the `verify` workflow run.
2. Confirm both jobs complete without OOM errors.
3. Check the `Set up swap space` step output shows `Setting up swapspace`.
4. Check the `Cache cargo registry` step shows the `simard-verify` shared
   key in use.

## When to revisit

- **Runner upgrade**: If GitHub Actions runners move to 16+ GB RAM, the
  swap step can be removed and `CARGO_BUILD_JOBS` increased.
- **LadybugDB upgrade**: If LadybugDB reduces link-time memory usage,
  re-evaluate whether `CARGO_BUILD_JOBS=2` is still needed.
- **Self-hosted runners**: Self-hosted runners with more RAM may not need
  any of these mitigations.
