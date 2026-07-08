---
title: How to run the installer preflight doctor
description: Run the platform installer's preflight doctor to check a host before (or independently of) an install — OS and libstdc++ version, Rust toolchain, native build deps, the amplihack CLI and bundle, free disk, dashboard-port availability, systemd unit-name collisions, and lbug ABI compatibility — and either auto-remediate or fail with a precise, actionable error.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/platform-installer.md
  - ../concepts/lbug-portability-libstdcxx-abi.md
  - ./install-a-simard-family-agent.md
  - ../reference/platform-installer-cli.md
---

# How to run the installer preflight doctor

The **preflight doctor** is the installer's first phase, exposed as a standalone
command so you can check a host without committing to an install. It detects
everything the install depends on and either **auto-remediates** what is safe to
fix or **fails closed** with an exact, actionable error. There is no silent
fallback: a check either passes, is remediated, or stops the run.

## When to run it

- **Before a first install** on an unfamiliar host, to see what will be
  provisioned.
- **After a failed install**, to isolate which precondition regressed.
- **As a fleet audit**, to confirm a host can host another identity (ports, unit
  names, disk).

## Run it

```bash
# Local host:
simard platform doctor --identity ./config/crocutus.env

# Remote host over azlin:
simard platform doctor --identity ./config/crocutus.env --remote azlin:dev
```

The canonical Crocutus scaffold equivalent is `./scripts/doctor.sh`. By default
the doctor **reports and auto-remediates**; pass `--check-only` to report without
changing the host (useful for audits). See the
[CLI reference](../reference/platform-installer-cli.md) for all flags and exit
codes.

> `simard platform doctor` is namespaced under `simard platform …` to avoid the
> pre-existing `simard install` verb. It is a superset host check; the separate
> `simard ensure-deps` command remains a minimal *runtime* check (`git`,
> `python3`, `gh`) and no longer probes for the (stale) Python `kuzu` package —
> memory is embedded lbug.

## What it checks

Each check maps to a friction point the first real install hit
(see [the concept](../concepts/platform-installer.md#the-problem)):

| Check | Passes when | Auto-remediation | Fails closed when |
|-------|-------------|------------------|-------------------|
| **OS + libstdc++** | Reads distro and `GLIBCXX_3.4.NN` | — (informational input to the lbug check) | Cannot determine libstdc++ |
| **Rust toolchain** | `cargo`/`rustc` present | Installs `rustup`/`cargo` | Install not permitted / fails |
| **Native build deps** | `build-essential`, `cmake`, `clang`, `pkg-config`, `libssl-dev` present | `apt install` the missing set | `sudo`/apt unavailable |
| **amplihack CLI + bundle** | `amplihack` on PATH and `~/.amplihack/amplifier-bundle` synced | Installs/syncs it | Install/sync fails |
| **Disk** | Enough free space for a from-source build + store | — | Below the build threshold |
| **Dashboard port** | The identity's `SIMARD_DASHBOARD_PORT` is free | — | Port already bound (collision) |
| **Unit-name collision** | No existing user unit with the identity's name | — | `<identity>-ooda.service` already exists for a *different* identity |
| **State-root collision** | The identity's `SIMARD_STATE_ROOT` is unused by another identity | — | Another identity owns that root |
| **lbug source-build prerequisites** | The C++ toolchain needed to build lbug from source is present | Installs the missing toolchain (via Rust/native-deps rows) | The source-build toolchain cannot be provisioned |

The **lbug source-build** check is the one that prevents the newer-OS SIGSEGV.
Building lbug from source is the *unconditional* choice at deploy time (the
installer's build phase exports `LBUG_BUILD_FROM_SOURCE=1`, and
`amplihack-memory-lib`'s persistent-feature guard warns loudly if a prebuilt were
ever linked without it), so this check does **not** choose prebuilt-vs-source —
it verifies the source-build toolchain is present and reports the host's
libstdc++ for the record. There is no prebuilt in the process to mismatch the
host ABI. See
[lbug portability](../concepts/lbug-portability-libstdcxx-abi.md).

## Read the report

The doctor prints one line per check with a status icon and, on failure, the
exact remediation. It ends with a machine-readable summary and a non-zero exit if
any check failed and could not be remediated. Example (abridged):

```
simard platform doctor: preflight for identity 'crocutus' on host 'dev'

  ✓ os              Ubuntu 26.04 LTS
  ✓ libstdc++       GLIBCXX_3.4.35
  ✓ rust            cargo 1.90 (installed via rustup)
  ✓ native-deps     build-essential, cmake, clang, pkg-config, libssl-dev
  ✓ amplihack       amplihack 0.x + amplifier-bundle synced
  ✓ disk            18 GiB free (need ≥ 6 GiB)
  ✓ port            8090 free
  ✓ unit-name       crocutus-ooda.service available
  ✓ state-root      ~/.crocutus/state unused
  ✓ lbug-source     toolchain present → lbug builds from source (LBUG_BUILD_FROM_SOURCE=1)

preflight PASSED — host 'dev' is ready to install identity 'crocutus'.
```

A failing run stops at the first unremediable check and tells you exactly what to
do, for example:

```
  ✗ native-deps     missing: cmake, libssl-dev
                    remediation requires sudo apt; run:
                    sudo apt-get install -y cmake libssl-dev
preflight FAILED (fail-closed): host is not ready. Fix the above and re-run.
```

## Exit codes

- `0` — all checks pass (after any auto-remediation).
- non-zero — at least one check failed and could not be remediated; the report
  names it. The installer treats a non-zero doctor as a hard stop and does not
  proceed to build or start anything.

## See also

- [The Simard platform installer](../concepts/platform-installer.md)
- [lbug portability across libstdc++ ABIs](../concepts/lbug-portability-libstdcxx-abi.md)
- [Install a Simard-family agent](./install-a-simard-family-agent.md)
- [Platform installer CLI reference](../reference/platform-installer-cli.md)
