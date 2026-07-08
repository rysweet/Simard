---
title: Platform installer CLI reference
description: Authoritative contract for the Simard-family platform installer — the `simard platform install` and `simard platform doctor` rails and their canonical Crocutus scaffold, every flag, the identity-config schema and env-var contract, the install phase state machine, the fail-closed rules, exit codes, and the upgrade/uninstall contracts.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/platform-installer.md
  - ../concepts/lbug-portability-libstdcxx-abi.md
  - ../howto/install-a-simard-family-agent.md
  - ../howto/run-the-installer-preflight-doctor.md
  - ./simard-cli.md
---

# Platform installer CLI reference

This is the authoritative contract for the platform installer. Prose and
walkthroughs live in
[The Simard platform installer](../concepts/platform-installer.md) and
[Install a Simard-family agent](../howto/install-a-simard-family-agent.md); this
page is the reference.

## Entry points

The installer has one implementation with two equivalent front doors:

| Front door | Location | Notes |
|------------|----------|-------|
| `simard platform install` / `simard platform doctor` | Simard binary (thin rail) | Convenience wrapper; shells to the canonical scaffold. |
| `scripts/install.sh` / `scripts/doctor.sh` | **Crocutus repo (canonical)** | Source of truth. The rail delegates here. |

The rail is *not* a second implementation; it forwards flags to the scaffold so
there is a single code path to test and maintain.

> **Verb deconfliction.** The bare `simard install` verb is **already taken**: it
> persists the current binary to `~/.simard/bin` (used by the `npx` wrapper — see
> [`simard install` in the CLI reference](./simard-cli.md#simard-install)) and it
> rejects extra arguments. The platform installer therefore lives under a new
> `simard platform …` subcommand group so it never collides with the existing
> command. `simard platform doctor` is a new subcommand. The canonical scaffold
> scripts remain the source of truth; the rail only forwards to them.

### Target-artifact note

The canonical `scripts/install.sh` and `scripts/doctor.sh` are the **target**
shape. Today the Crocutus repo ships `scripts/install-on-dev.sh` (interactive,
host-`dev`-specific, unconditional rebuild, in-place `install -m 0755` overwrite,
with no idempotency/`--upgrade`/`--uninstall`/`--check-only`),
`scripts/bootstrap-readonly.sh`, and `scripts/prove-guardrail.sh`. Reaching this
reference means generalizing `install-on-dev.sh` → `install.sh`, extracting the
preflight phase into `doctor.sh`, and adding idempotency, `--local/--remote`,
`--upgrade/--uninstall`, `--yes`, non-interactive host confirmation, and the
atomic binary swap. Those are new work, not shipped behavior.

## `simard platform install`

Stand up (or upgrade/uninstall) an identity's daemon on a host.

```
simard platform install --identity <path> [--local | --remote azlin:<vm>]
                        [--upgrade | --uninstall]
                        [--check-only] [--yes] [--dashboard-port <n>]
                        [--state-root <path>] [--unit-name <name>]
```

### Flags

| Flag | Required | Default | Meaning |
|------|----------|---------|---------|
| `--identity <path>` | yes | — | Path to the identity config (EnvironmentFile format). Declares the isolation axes, guardrails, and required credentials. |
| `--local` | one of `--local`/`--remote` | `--local` | Install on the current host. |
| `--remote azlin:<vm>` | " | — | Install on an azlin-reachable VM over Bastion (`azlin connect <vm> -- …`). |
| `--upgrade` | no | — | Stop unit, atomically swap the binary, restart, re-verify; roll back on failure. |
| `--uninstall` | no | — | Stop and disable the unit; remove the identity's state root and unit file. Leaves other identities intact. |
| `--check-only` | no | off | Run preflight only (equivalent to `simard platform doctor`); make no changes. |
| `--yes` | no | off | Non-interactive; assume "yes" to safe prompts (e.g. host confirmation). Required for unattended/recipe runs. |
| `--dashboard-port <n>` | no | from config | Override the identity's dashboard port; must not collide. |
| `--state-root <path>` | no | from config | Override the identity's state root; must not collide. |
| `--unit-name <name>` | no | `<identity>-ooda.service` | Override the systemd user unit name; must not collide. |

### Required environment / credentials

Credentials are supplied out-of-band and are **never** committed. There are two
distinct credential mechanisms and two distinct fail-closed rules; do not conflate
them:

| Variable | Kind | Consumed by | Role |
|----------|------|-------------|------|
| `ADO_READONLY_PAT` | operator env var | `bootstrap-readonly.sh` at **deploy time** | Fetches the read-only clone of the observed target. Read-scoped only. |
| `SIMARD_ADO_PAT_FILE` | path to a PAT file | the **daemon at runtime** | The read-scoped PAT the running daemon reads to observe Azure DevOps. |
| `AZURE_DEVOPS_EXT_PAT`, `SYSTEM_ACCESSTOKEN`, `ADO_WRITE_PAT` | write-capable tokens | — | Presence of any for a read-only identity is a **guardrail failure**. |

**Fail-closed rules (two separate things):**

1. **A write-capable token present → hard stop.** For a read-only identity, the
   guardrail proof (`prove-guardrail.sh` layer a) refuses to proceed if any
   write-capable token is in the environment. This is the *real* credential
   guardrail and it is enforced.
2. **A declared-required credential absent → stop.** If the identity config marks
   a credential as required and it is absent, the install stops. **Note:** for a
   read-only *observer* the read PAT is **optional** by default — absent, the
   observer degrades to anonymous read or does nothing (fail-closed at runtime),
   and the current scaffold *continues* installing without it. Only mark the read
   PAT required if the identity genuinely cannot observe anonymously; that is an
   identity-config choice, not an installer default.

The installer cannot introspect a PAT's scope offline; it enforces the *absence
of write tokens* for read-only identities and relies on the operator to supply a
correctly-scoped read token.

## `simard platform doctor`

Run the preflight phase standalone. See
[Run the preflight doctor](../howto/run-the-installer-preflight-doctor.md).

```
simard platform doctor --identity <path> [--local | --remote azlin:<vm>] [--check-only]
```

`--check-only` reports without auto-remediating (audit mode). Without it, the
doctor auto-remediates safe items (toolchain, native deps, amplihack) and only
fails on the unremediable.

## Identity config schema

The identity config is a systemd `EnvironmentFile` (KEY=VALUE, no shell
expansion). It uses `%h` as an **installer-materialized placeholder** for the
deploying user's `$HOME`: systemd does **not** expand `%h` inside the *contents*
of an `EnvironmentFile`, so the installer runs `sed "s#%h#$HOME#g"` at deploy time
to produce a materialized env file (e.g. `~/.crocutus/crocutus.env.materialized`)
and rewrites the unit's `EnvironmentFile=` to point at it. A literal `%h` must
never reach the running daemon. Recognized keys:

| Key | Required | Purpose |
|-----|----------|---------|
| `SIMARD_IDENTITY` | yes | Persona name; also the default unit-name prefix and the default identity-home name (`~/.<identity>`). |
| `SIMARD_STATE_ROOT` | yes | Isolated state root (goal board + cognitive-memory `flock`), typically `%h/.<identity>/state`. Must be unique per host. |
| `SIMARD_DASHBOARD_PORT` | yes | Dashboard port. Must be unique per host. |
| `SIMARD_IDENTITY_PATH` | yes | Identity manifest directory (`identity.toml`, persona prompt). |
| `SIMARD_PROMPT_ROOT` | yes | Prompt-asset root. |
| `SIMARD_OBSERVE_ONLY` | for observers | `1` activates the read-only floor (`read_only_guard`). |
| `SIMARD_GIT_GUARDRAILS` | no | `enabled` keeps the destructive-git guardrail on. |
| `SIMARD_ADO_PAT_FILE` | when observing ADO | Path to a **read-scoped** PAT file the daemon reads at runtime. |
| `SIMARD_TARGET_ADO_ORG` / `SIMARD_TARGET_ADO_PROJECT` / `SIMARD_TARGET_REPO_URL` | for observers | The observed target (read-only). |

The **identity home** `~/.<identity>` (which holds `bin/<identity>`, `targets/`,
and the materialized env) is distinct from the **state root**
`~/.<identity>/state` (goal board + memory `flock`). See
[Pluggable identity](../concepts/pluggable-identity.md) for how these map to
Simard's runtime identity.

## Install phase state machine

The installer runs these phases in order. Each is idempotent and fails closed;
re-running converges rather than duplicating.

| # | Phase | Succeeds when | Fails closed when |
|---|-------|---------------|-------------------|
| 1 | **preflight** | All doctor checks pass (after safe auto-remediation) | Any unremediable check (see doctor table) |
| 2 | **build** | Agent binary built with lbug compiled **from source** (the installer's build phase exports `LBUG_BUILD_FROM_SOURCE=1`; `amplihack-memory-lib`'s guard warns if it were missing) against the host toolchain | Source-build toolchain missing / build fails |
| 3 | **materialize** | Identity home, isolated state root, materialized env, prompt root, and distinct user unit created | Any isolation-axis collision (port, state root, socket, unit name) |
| 4 | **prove-guardrails** | The identity's guardrail proof passes (e.g. read-only floor + no write credential) | Proof uncertain or a required credential absent |
| 5 | **start** | `daemon-reload` + enable + start succeed | systemd cannot start the unit |
| 6 | **verify** | A positive store-opened / first-OODA-cycle log marker is observed **and** the daemon holds a **stable new PID** across ≥ 1 `RestartSec` window | Crash-loop (PID flapping / unit sub-state `failed`), or a journal frame matching `initBufferManager` / `std::vformat` (the lbug SIGSEGV class) |
| 7 | **report** | Durable machine-readable summary emitted | — |

There are **no wall-clock timeouts** on the agentic verify step; it waits for a
real live-cycle signal (the positive marker above), not a fixed delay.

## systemd unit contract

The installed unit is a **user** unit at
`~/.config/systemd/user/<identity>-ooda.service`. It references **two** distinct
directories — do not conflate them:

- the **repo checkout / prompt root** `~/<identity>` (e.g. `~/crocutus`, no dot),
  which holds `scripts/` and the *source* `config/<identity>.env`; and
- the **identity home** `~/.<identity>` (e.g. `~/.crocutus`, with dot), which
  holds `bin/<identity>`, `targets/`, and the *materialized* env — and is itself
  distinct from the state root `~/.<identity>/state`.

The load-bearing directives are:

- `ExecStartPre=<prompt-root>/scripts/prove-guardrail.sh` (e.g.
  `~/crocutus/scripts/prove-guardrail.sh`) — the fail-closed **start-gate**; a
  non-zero exit aborts the unit, so a later guardrail regression keeps the daemon
  down rather than up-and-unsafe.
- `ExecStart=<identity-home>/bin/<identity>` — the installed binary (e.g.
  `~/.crocutus/bin/crocutus`).
- `EnvironmentFile=<identity-home>/<identity>.env.materialized` — the
  sed-materialized env (e.g. `~/.crocutus/crocutus.env.materialized`), never the
  `%h` source file `<prompt-root>/config/<identity>.env`.
- `WorkingDirectory=<identity-home>/targets/<primary-repo>` — a read-only clone
  of the observed target (e.g. `~/.crocutus/targets/hyenas`); the OODA loop reads
  `current_dir()`.
- `Type=simple`, `Restart=on-failure`, `RestartSec=10`.
- Hardening: `NoNewPrivileges=true`, `ProtectSystem=strict`,
  `ReadWritePaths=<identity-home>`, `ReadOnlyPaths=<identity-home>/targets`,
  `ProtectHome=false`, `MemoryMax=2G`, `LimitNOFILE=65536`.
- `[Install] WantedBy=default.target`.

## `simard ensure-deps` and the removed kuzu check

`simard ensure-deps` reports runtime dependencies. Historically it probed for a
Python `kuzu` package — **stale and misleading**, because Simard's cognitive
memory is embedded **lbug** compiled into the binary via `amplihack-memory-lib`,
not kuzu. The target state:

- The `kuzu` Python-package probe is **removed** from `src/cmd_ensure_deps.rs`.
- Nothing installs kuzu; the installer never references it.
- `ensure-deps` remains a lightweight runtime check (`git`, `python3`, `gh`); the
  heavier build/host preconditions (Rust, `build-essential`, `cmake`, `clang`,
  `pkg-config`, `libssl-dev`, `amplihack` CLI + bundle) are the **doctor's**
  responsibility. Treat `simard platform doctor` as the superset host check and
  `ensure-deps` as the minimal runtime check.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. For `install`: the daemon reached a verified live OODA cycle. For `doctor`: all checks pass. |
| non-zero | A phase failed closed. The report names the phase and the exact remediation. No partial daemon is left running. |

## Idempotency and re-runs

- Re-running `install` on an already-installed identity re-verifies and converges
  (no double toolchain install, no clobbered state).
- `--upgrade` swaps the binary atomically (write-new-then-rename) to survive the
  held-inode problem of overwriting a running binary; on failed verify it rolls
  back to the prior binary. (Today's `install-on-dev.sh` overwrites in place with
  `install -m 0755`; the atomic swap is target work.)
- `--uninstall` is safe to run repeatedly.

## See also

- [The Simard platform installer](../concepts/platform-installer.md)
- [lbug portability across libstdc++ ABIs](../concepts/lbug-portability-libstdcxx-abi.md)
- [Install a Simard-family agent](../howto/install-a-simard-family-agent.md)
- [Run the preflight doctor](../howto/run-the-installer-preflight-doctor.md)
- [simard CLI reference](./simard-cli.md)
