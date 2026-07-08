---
title: How to install a Simard-family agent
description: Stand up a Simard-family agent daemon (Simard, Crocutus, or a future identity) on a host with one idempotent, fail-closed command — locally or over an azlin-remote target. Covers identity config, credential wiring, preflight, verification of a live OODA cycle, and upgrade/uninstall.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/platform-installer.md
  - ../concepts/lbug-portability-libstdcxx-abi.md
  - ./run-the-installer-preflight-doctor.md
  - ./configure-pluggable-identity.md
  - ../reference/platform-installer-cli.md
  - ../tutorials/stand-up-a-read-only-observer.md
---

# How to install a Simard-family agent

The platform installer stands up a Simard-family identity's daemon on a host in
one idempotent, fail-closed operation: it prepares the host, builds a binary with
a portable cognitive-memory store, materializes an isolated identity and systemd
unit, proves the identity's guardrails, starts the daemon, and verifies it
reaches a live OODA cycle. See
[The Simard platform installer](../concepts/platform-installer.md) for the
design; this guide is the operational recipe.

## Prerequisites

- **A target host.** Either the local machine, or an
  [`azlin`](https://github.com/rysweet/azlin)-reachable VM (requires the
  `az ssh` extension and Bastion access, e.g. `azlin connect dev`).
- **An identity config** (see [Identity config](#choose-or-write-an-identity-config)).
- **Credentials** the identity requires, supplied out-of-band (never committed).
  For a read-only observer like Crocutus, this is a **read-only** Azure DevOps
  PAT (`Code: Read`) — never a write token.
- On the *operator* machine: `git`, and for remote installs, `azlin`.

The installer provisions everything else on the target itself — the Rust
toolchain, native build deps (`build-essential`, `cmake`, `clang`,
`pkg-config`, `libssl-dev`), and the `amplihack` CLI and bundle. You do not
pre-install those by hand.

## Choose or write an identity config

An identity config declares the four isolation axes and the identity's
credentials and guardrails. The canonical scaffolds live in the Crocutus repo
(`config/<identity>.env`, `systemd/<identity>-ooda.service`,
`identity/<identity>_system.md`). A minimal read-only observer config:

```ini
# config/crocutus.env  (systemd EnvironmentFile format)
# %h is an installer-materialized placeholder: the installer runs
# `sed "s#%h#$HOME#g"` at deploy time to produce a materialized env file.
# systemd does NOT expand %h inside an EnvironmentFile's contents.
SIMARD_OBSERVE_ONLY=1                      # the read-only floor (guardrail)
SIMARD_STATE_ROOT=%h/.crocutus/state       # isolated goal board + memory flock
SIMARD_IDENTITY=crocutus                   # persona
SIMARD_IDENTITY_PATH=%h/crocutus/identity
SIMARD_PROMPT_ROOT=%h/crocutus
SIMARD_DASHBOARD_PORT=8090                 # distinct port (no collision)
SIMARD_ADO_PAT_FILE=%h/.crocutus/ado_readonly.pat   # READ-scoped PAT the daemon reads
```

The installer allocates and **verifies** these against identities already on the
host and refuses to proceed on any collision (identity home, state root, port,
socket, or unit name). Note the **identity home** `~/.crocutus` (which holds the
binary and target clones) is distinct from the **state root** `~/.crocutus/state`
(goal board + memory `flock`). See
[Pluggable identity](../concepts/pluggable-identity.md) and
[How to configure pluggable identities](./configure-pluggable-identity.md).

## Run the installer

### Local install

```bash
# From the identity repo (e.g. a Crocutus checkout) on the target host:
export ADO_READONLY_PAT=…        # read-only PAT for the deploy-time target clone
simard platform install \
  --identity ./config/crocutus.env \
  --local
```

Equivalently, the canonical Crocutus scaffold script:

```bash
./scripts/install.sh --identity ./config/crocutus.env
```

`simard platform install` is a thin rail that shells to the same scaffold logic
(it is namespaced under `simard platform …` because the bare `simard install`
verb already persists the binary to `~/.simard/bin`). Use whichever entry point
you prefer. See the [CLI reference](../reference/platform-installer-cli.md) for
every flag.

### Remote install over azlin

Run the *same* operation against a Bastion-reachable VM. Confirm the target first
with `azlin list` (and reach it with `azlin connect dev -- hostname`), then:

```bash
simard platform install \
  --identity ./config/crocutus.env \
  --remote azlin:dev
```

Under the hood the rail reaches the host with `azlin connect dev -- …`. For a
remote install the installer must transport two things to the target — **without
committing either**:

- **The identity repo/config** — cloned on the target (or copied over the azlin
  channel), so the scaffold runs there.
- **The credential** — a local `export ADO_READONLY_PAT=…` on your operator shell
  does **not** reach the remote daemon. The read PAT must be delivered to the
  target out-of-band (e.g. written to `SIMARD_ADO_PAT_FILE` on the host over the
  azlin channel) and never placed in the repo or the unit file.

### What happens, in order

The installer runs the [phase machine](../concepts/platform-installer.md#the-install-as-a-phase-machine):
**preflight → build (from-source lbug) → materialize identity → prove guardrails →
start daemon → verify live cycle → report.** Each phase is idempotent; re-running
after a partial failure converges rather than duplicating work.

## Supply credentials (fail closed)

There are **two** credential rules; keep them separate:

1. **A write-capable token present → hard stop.** For a read-only observer the
   guardrail proof refuses to install if any write-capable token
   (`AZURE_DEVOPS_EXT_PAT`, `SYSTEM_ACCESSTOKEN`, `ADO_WRITE_PAT`) is in the
   environment. This is the real, enforced credential guardrail — the installer
   cannot introspect a PAT's scope offline, so it enforces the *absence* of write
   tokens.
2. **A declared-required credential absent → stop.** If the identity config marks
   a credential as required and it is absent, the install stops.

For a read-only observer the **read PAT is optional by default**: without it, the
observer degrades to anonymous read or does nothing (fail-closed at runtime), and
the install still completes. Two different PAT mechanisms are involved — don't
conflate them:

- `ADO_READONLY_PAT` (operator env) is consumed **at deploy time** by
  `bootstrap-readonly.sh` to fetch the read-only target clone.
- `SIMARD_ADO_PAT_FILE` (a path) is what the **daemon reads at runtime** to
  observe Azure DevOps. Store the token there (e.g. `~/.crocutus/ado_readonly.pat`),
  readable only by the daemon user.

## Verify the install

The installer verifies the install for you and reports it. To confirm by hand:

```bash
# The unit is active and has a stable (non-flapping) PID:
systemctl --user status crocutus-ooda.service

# Cognitive memory opened with no SIGSEGV and the OODA loop is live:
journalctl --user -u crocutus-ooda.service -f
#   → expect a live OODA cycle; NO 'SIGSEGV' / 'initBufferManager' backtrace.

# Side-by-side isolation from any other identity on the host:
ls ~/.simard/state ~/.crocutus/state    # distinct state roots
```

A crash-loop (PID changing every `RestartSec`) is a **failed** install, not a
warning. If cognitive memory segfaults on start, this is the libstdc++ ABI class
of failure — see
[lbug portability](../concepts/lbug-portability-libstdcxx-abi.md) — and the
installer should have failed closed at the build/verify phase; re-run the
[preflight doctor](./run-the-installer-preflight-doctor.md) to confirm the host's
libstdc++/toolchain and the from-source lbug build.

## Run side-by-side with another identity

Nothing extra is required: the installer guarantees a distinct state root,
dashboard port, socket, and unit name. Both units enable and run independently:

```bash
systemctl --user status simard-ooda.service crocutus-ooda.service
```

If you deliberately reuse a value that collides (same port, same state root, same
unit name) the installer refuses rather than overwriting the existing identity.

## Upgrade

Upgrades stop the unit, swap the binary **atomically** (survives the held-inode
problem), restart, and re-verify a live cycle — rolling back on failure:

```bash
simard platform install --identity ./config/crocutus.env --upgrade
```

This reuses the [reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md)
discipline.

## Uninstall

```bash
simard platform install --identity ./config/crocutus.env --uninstall
```

Stops and disables the unit and removes the identity's state root and unit file,
leaving other identities on the host untouched.

## Troubleshooting

| Symptom | Cause | Action |
|---------|-------|--------|
| Install stops in preflight on a missing dep | Toolchain / native dep / `amplihack` absent and not auto-remediable | Read the doctor's exact line; see [Run the preflight doctor](./run-the-installer-preflight-doctor.md). |
| Install stops: "missing required credential" | Fail-closed on absent credential | Provide the read-only token at the configured path. |
| Install stops: "write-capable credential present" | A write token is in the environment | Remove it; a read-only observer must have no write token. |
| Verify fails: SIGSEGV in `initBufferManager` | libstdc++ ABI mismatch from a prebuilt lbug | Confirm the from-source lbug build; see [lbug portability](../concepts/lbug-portability-libstdcxx-abi.md). |
| Install refuses: unit/port/state-root collision | Another identity already uses that value | Choose distinct values in the identity config. |

## See also

- [The Simard platform installer](../concepts/platform-installer.md)
- [lbug portability across libstdc++ ABIs](../concepts/lbug-portability-libstdcxx-abi.md)
- [Run the preflight doctor](./run-the-installer-preflight-doctor.md)
- [Platform installer CLI reference](../reference/platform-installer-cli.md)
- [Stand up a read-only observer](../tutorials/stand-up-a-read-only-observer.md)
