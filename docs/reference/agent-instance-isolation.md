---
title: Agent instance-isolation reference
description: The SIMARD_HOME instance-root layer, the instance_home() resolver, the full per-instance environment matrix, systemd unit-name templating, and the `simard debug instance` verification command that let two Simard identities run side-by-side on one host.
last_updated: 2026-07-08
owner: simard
doc_type: reference
related:
  - ../concepts/multi-identity-host-isolation.md
  - ../reference/state-root-resolution.md
  - ../reference/write-authority-posture-api.md
  - ../howto/run-a-second-agent-identity.md
---

# Agent instance-isolation reference

!!! warning "Implementation status — this page describes the PLANNED instance-root layer (tracking #3067)"
    The `simard::instance_home` module, `SIMARD_HOME`, `instance_home()`,
    `instance_subdir()`, `instance_name()`, `systemd_unit_name()`, and the
    `simard debug instance` verb documented here are **not yet implemented** — they are
    the target design tracked in
    [#3067](https://github.com/rysweet/Simard/issues/3067). **What ships today** is
    [`SIMARD_STATE_ROOT`](./state-root-resolution.md), which isolates the durable state
    tree (cognitive-memory `flock` + goal board) per identity — sufficient for the
    load-bearing collision guarantee. Set `SIMARD_STATE_ROOT`, `SIMARD_DASHBOARD_PORT`,
    `SIMARD_MEMORY_SOCKET`, and a distinct systemd unit name per instance; the
    `SIMARD_HOME`-derived singletons below are planned. See the `rysweet/Crocutus` repo
    for the shipped, runnable configuration.

Module: `simard::instance_home`

An **instance** is one autonomous Simard identity running on a host with its
own state, ports, socket, systemd unit, and credentials. This page is the
reference for the instance-root layer that makes two instances (for example
the primary `simard` and the read-only `crocutus`) coexist. For the rationale
see [Multi-identity host isolation](../concepts/multi-identity-host-isolation.md).

---

## Public API

```rust
pub fn instance_home() -> PathBuf;

pub fn instance_subdir(name: &str) -> PathBuf;

pub fn instance_name() -> String;

pub fn systemd_unit_name(kind: UnitKind) -> String;
```

Re-exported from the crate root:

```rust
use simard::instance_home::{instance_home, instance_subdir, instance_name, systemd_unit_name};
```

### `instance_home() -> PathBuf`

Returns the resolved instance root. Resolution order:

1. `$SIMARD_HOME` if set, **non-empty**, **absolute**, and free of interior
   NUL bytes.
2. `$HOME/.simard` otherwise (the primary instance default).

A non-absolute or NUL-bearing `SIMARD_HOME` is **ignored with a WARN**, not an
error — boot never fails on a malformed instance-root env, exactly like
[`simard_state_root()`](./state-root-resolution.md#simard_state_root-pathbuf).
The helper does not create the directory; callers create it on first write.

Every ambient host-level singleton that previously hardcoded `$HOME/.simard`
now resolves through this helper:

| Singleton | Path (via `instance_home()`) | Previously |
|-----------|------------------------------|------------|
| Binary install dir | `<home>/bin/simard` | hardcoded `~/.simard/bin/simard` |
| Agent registry | `<home>/agent_registry.json` | hardcoded, ignored state root |
| Memory snapshots | `<home>/snapshots/` | hardcoded |
| Self-update state / backups | `<home>/{bin,state}` | hardcoded (cwd fallback) |
| Engineer worktrees | `<home>/engineer-worktrees/` | hardcoded |

### `instance_subdir(name: &str) -> PathBuf`

Returns `instance_home().join(name)`. Callers pass a static subdirectory
string (`"snapshots"`, `"engineer-worktrees"`, ...); the helper does no
validation on `name`.

### `instance_name() -> String`

Returns the instance name used for the systemd unit and log labeling.
Resolution order:

1. `$SIMARD_INSTANCE` if set and non-empty.
2. The final path component of `instance_home()` with a leading dot stripped
   (`$HOME/.crocutus` → `crocutus`, `$HOME/.simard` → `simard`).

### `systemd_unit_name(kind: UnitKind) -> String`

Returns the instance-scoped unit name, replacing the historical hardcoded
`simard-ooda.service` constant.

```rust
pub enum UnitKind { Ooda, Signal, Overseer }
```

| `instance_name()` | `UnitKind::Ooda` | `UnitKind::Signal` |
|-------------------|------------------|--------------------|
| `simard` | `simard-ooda.service` | `simard-signal.service` |
| `crocutus` | `crocutus-ooda.service` | `crocutus-signal.service` |

The primary instance's unit names are unchanged, so existing deployments and
the TUI/deploy paths that referenced `simard-ooda.service` keep working.

---

## Relationship to `SIMARD_STATE_ROOT`

`SIMARD_HOME` sits **above** the pre-existing
[state-root resolution](./state-root-resolution.md). It relocates the ambient
singletons (install, registry, snapshots, worktrees) that state-root never
covered. `SIMARD_STATE_ROOT` continues to relocate the durable state tree
(cognitive memory, goal store, handoffs) and, when set, wins for that tree.

Resolution precedence for the **state tree**, per instance:

1. The subsystem's narrow env var (e.g. `SIMARD_HANDOFF_DIR`).
2. `$SIMARD_STATE_ROOT/<subdir>`.
3. `$SIMARD_HOME/state/<subdir>` — **new**: when neither of the above is set,
   the state tree defaults under the instance root instead of the bare
   `$HOME/.simard`.
4. `$HOME/.simard/state/<subdir>` (primary default when `SIMARD_HOME` unset).

This removes the old inconsistency where some paths honored
`SIMARD_STATE_ROOT` and others hardcoded `$HOME/.simard`.

---

## Per-instance environment matrix

Set these together to define an instance. Every row must differ from every
other instance on the host (the
[isolation invariant](../concepts/multi-identity-host-isolation.md#the-isolation-invariant)).

| Variable | Purpose | Primary (`simard`) | Second (`crocutus`) |
|----------|---------|--------------------|---------------------|
| `SIMARD_HOME` | Instance root (install, registry, snapshots, worktrees) | `$HOME/.simard` (default) | `$HOME/.crocutus` |
| `SIMARD_INSTANCE` | Instance name (unit, labels) | `simard` (derived) | `crocutus` |
| `SIMARD_STATE_ROOT` | Durable state tree + cognitive-memory `flock` | `$HOME/.simard/state` | `$HOME/.crocutus/state` |
| `SIMARD_DASHBOARD_PORT` | Dashboard HTTP port | `8080` | `8090` |
| `SIMARD_MEMORY_SOCKET` | Memory IPC socket | `.../memory.sock` | `.../crocutus-memory.sock` |
| `SIMARD_AGENT_NAME` | Bridge registration label | `simard-ooda` | `crocutus-ooda` |
| `SIMARD_IDENTITY` | Selected identity name | `simard-engineer` | `crocutus` |
| `SIMARD_IDENTITY_PATH` | Directory holding `identity.toml` | (built-in) | `<crocutus-repo>/identity` |
| `SIMARD_PROMPT_ROOT` | Prompt-asset root | repo root | `<crocutus-repo>` |

The identity-selection variables (`SIMARD_IDENTITY`, `SIMARD_IDENTITY_PATH`,
`SIMARD_PROMPT_ROOT`) and the persona itself are covered by
[pluggable identity](../concepts/pluggable-identity.md); the
[write-authority posture](./write-authority-posture-api.md) governs the
read-only mandate.

---

## `simard debug instance`

Prints the resolved instance identity and every instance-scoped path for the
current environment, without performing any writes — the instance analogue of
[`simard debug state-root`](./state-root-resolution.md#verifying-the-resolved-root).
Use it to verify isolation **before** starting a second daemon.

```bash
SIMARD_HOME=$HOME/.crocutus SIMARD_INSTANCE=crocutus simard debug instance
```

```
instance_name=crocutus
  source=SIMARD_INSTANCE
instance_home=/home/azureuser/.crocutus
  source=SIMARD_HOME
install_bin=/home/azureuser/.crocutus/bin/simard
agent_registry=/home/azureuser/.crocutus/agent_registry.json
snapshots=/home/azureuser/.crocutus/snapshots
engineer_worktrees=/home/azureuser/.crocutus/engineer-worktrees
state_root=/home/azureuser/.crocutus/state
  source=SIMARD_HOME
memory_flock=/home/azureuser/.crocutus/state/cognitive_memory/LOCK
dashboard_port=8090
  source=SIMARD_DASHBOARD_PORT
memory_socket=/home/azureuser/.crocutus/state/crocutus-memory.sock
  source=SIMARD_MEMORY_SOCKET
ooda_unit=crocutus-ooda.service
signal_unit=crocutus-signal.service
```

### Collision detection

`simard debug instance --check-collision` compares the resolved paths, port,
socket, and unit name against any **running** Simard instance discovered via
the agent registry and returns non-zero (with a loud diagnostic) if any
overlap. This is a pre-flight check; the ultimate guarantee remains the
cognitive-memory exclusive `flock`, which refuses a second daemon on a shared
state root at startup.

```
error: instance collision: SIMARD_STATE_ROOT '/home/azureuser/.simard/state'
       is already locked by running instance 'simard' (pid 4123).
       Two instances must not share a state root. Set a distinct SIMARD_HOME
       or SIMARD_STATE_ROOT. Refusing to start (fail-closed).
```

There is intentionally **no** fallback that opens a degraded unlocked store
when the `flock` is held — that would be silent degradation
([Pillar 11](../fail-open-audit.md)).

---

## Validation rules

Applied to `SIMARD_HOME`, mirroring
[state-root validation](./state-root-resolution.md#validation-rules):

| Check | Behavior on failure |
|-------|---------------------|
| Non-empty | Empty string treated as unset (falls back to `$HOME/.simard`) |
| Absolute path | Relative path **ignored with a WARN** |
| No interior NUL | Path containing `\0` **ignored with a WARN** |

The helper does not reject `..`, does not resolve symlinks, and does not
create the directory (first writer does). The single-user CLI threat model of
[state-root resolution](./state-root-resolution.md#security-notes) applies
unchanged.

---

## Permissions

Freshly-created instance directories are created owner-only:

| Artifact | Mode |
|----------|------|
| New instance-root subdirectory (`snapshots/`, `engineer-worktrees/`, `state/`) | `0o700` |
| Registry / snapshot files | `0o600` |

Pre-existing directories are not re-`chmod`ed. On non-unix targets the mode is
omitted and the umask applies.

---

## See also

- [Multi-identity host isolation](../concepts/multi-identity-host-isolation.md)
  — the design rationale.
- [State-root resolution](./state-root-resolution.md) — the per-subsystem
  ladder that `SIMARD_HOME` layers above.
- [Write-authority posture API](./write-authority-posture-api.md) — the
  read-only contract that pairs with isolation.
- [How to run a second agent identity](../howto/run-a-second-agent-identity.md)
  — the operator procedure.
