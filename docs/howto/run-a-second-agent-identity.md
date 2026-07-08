---
title: How to run a second agent identity side-by-side
description: Configure and launch a second autonomous Simard identity (its own SIMARD_HOME instance root, state, port, socket, systemd unit, credentials, and write-authority posture) on the same host as the primary simard daemon, without interference.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
related:
  - ../concepts/multi-identity-host-isolation.md
  - ../concepts/write-authority-posture.md
  - ../reference/agent-instance-isolation.md
  - ../reference/write-authority-posture-api.md
  - ../howto/run-ooda-daemon.md
  - ../tutorials/deploy-crocutus-read-only-observer.md
doc_type: howto
---

# How to run a second agent identity side-by-side

!!! warning "Implementation status — shipped v1 is env-driven (issue #1, tracking #3067)"
    Commands and blocks on this page that use `SIMARD_HOME`, `[identities.authority]`
    posture, or `simard debug instance` / `simard debug authority` describe the
    **planned** design tracked in
    [#3067](https://github.com/rysweet/Simard/issues/3067). **What ships today** is
    env-driven: **`SIMARD_STATE_ROOT`** (distinct state tree per identity, e.g.
    `~/.crocutus`) plus **`SIMARD_OBSERVE_ONLY=1`** for a read-only identity (enforced
    fail-closed by `read_only_guard` wired into `git_guardrails::check_git_safety` and
    the OODA engineer-dispatch). For the concrete, runnable procedure and guardrail
    proof see the `rysweet/Crocutus` repo (`README.md`, `scripts/prove-guardrail.sh`).

This guide runs a **second** autonomous Simard identity on a host that is
already running the primary `simard` daemon. The two share one binary but
nothing stateful: each gets its own instance root, state, dashboard port,
memory socket, systemd unit, and credentials. For the full end-to-end
deployment of the concrete `crocutus` read-only observer, see the
[Crocutus tutorial](../tutorials/deploy-crocutus-read-only-observer.md); this
page is the generic procedure.

## Prerequisites

- The primary `simard` daemon already installed and running on the host.
- The second identity's persona available as an
  [`identity.toml`](../howto/configure-pluggable-identity.md) plus prompt
  assets (a directory you point `SIMARD_IDENTITY_PATH` / `SIMARD_PROMPT_ROOT`
  at). For a **downstream** identity such as Crocutus this is a separate repo
  that *depends on* Simard, not a fork of it.
- Credentials scoped to the second identity's needs — and **no broader**. A
  read-only identity gets a read-only credential (or none), never a
  write-capable token to a target it must not modify.

## 1. Choose a distinct instance root

Every ambient host-level singleton derives from `SIMARD_HOME`. Give the second
identity its own:

```bash
export SIMARD_HOME="$HOME/.crocutus"     # primary keeps $HOME/.simard
export SIMARD_INSTANCE="crocutus"        # drives the systemd unit name
```

## 2. Assign non-colliding endpoints

```bash
export SIMARD_STATE_ROOT="$SIMARD_HOME/state"   # own cognitive-memory flock
export SIMARD_DASHBOARD_PORT=8090               # primary uses 8080
export SIMARD_MEMORY_SOCKET="$SIMARD_STATE_ROOT/crocutus-memory.sock"
export SIMARD_AGENT_NAME="crocutus-ooda"
```

See the
[per-instance environment matrix](../reference/agent-instance-isolation.md#per-instance-environment-matrix)
for every variable and why each must differ.

## 3. Select the identity and its posture

```bash
export SIMARD_IDENTITY="crocutus"
export SIMARD_IDENTITY_PATH="$CROCUTUS_REPO/identity"   # dir holding identity.toml
export SIMARD_PROMPT_ROOT="$CROCUTUS_REPO"
```

The persona's `identity.toml` declares its
[write-authority posture](../reference/write-authority-posture-api.md#identitytoml-surface).
A bounded observer sets:

```toml
[[identities]]
name = "crocutus"
default_mode = "engineer"

[identities.authority]
posture = "read-only"
allow_git_push = false
allow_ado_writes = false
allow_github_writes = false
```

## 4. Verify isolation before starting anything

Run the pre-flight checks. **Do not start the daemon until both pass.**

```bash
simard debug instance --check-collision
simard debug authority
```

`--check-collision` confirms the instance's paths, port, socket, and unit name
do not overlap a running instance (it exits non-zero on any overlap). `debug
authority` confirms the resolved posture. Expected posture output for a
read-only identity:

```
identity=crocutus
posture=read-only
git_push_check=REFUSED (read-only)
ado_write_check=REFUSED (read-only)
github_write_check=REFUSED (read-only)
```

!!! danger "Fail closed"
    If either check is uncertain — a path overlaps, the posture does not
    resolve to what you intend, or a write credential to a forbidden target is
    present — **stop**. Do nothing rather than risk the second identity writing
    where it must not. This is non-negotiable for read-only identities.

## 5. Install the second identity's binary copy

`simard install` writes to `$SIMARD_HOME/bin/simard`, so with `SIMARD_HOME`
set it installs the second instance's own copy without touching the primary's:

```bash
simard install     # installs to $SIMARD_HOME/bin/simard
```

## 6. Create the systemd unit

The unit name is derived from `SIMARD_INSTANCE`
([`systemd_unit_name`](../reference/agent-instance-isolation.md#systemd_unit_namekind-unitkind-string)),
so it never clashes with `simard-ooda.service`. Create a user unit that
exports the instance environment and runs the daemon:

```ini
# ~/.config/systemd/user/crocutus-ooda.service
[Unit]
Description=Crocutus OODA daemon (read-only observer, side-by-side with simard)
After=network-online.target

[Service]
Type=simple
Environment=SIMARD_HOME=%h/.crocutus
Environment=SIMARD_INSTANCE=crocutus
Environment=SIMARD_STATE_ROOT=%h/.crocutus/state
Environment=SIMARD_DASHBOARD_PORT=8090
Environment=SIMARD_MEMORY_SOCKET=%h/.crocutus/state/crocutus-memory.sock
Environment=SIMARD_AGENT_NAME=crocutus-ooda
Environment=SIMARD_IDENTITY=crocutus
Environment=SIMARD_IDENTITY_PATH=%h/crocutus/identity
Environment=SIMARD_PROMPT_ROOT=%h/crocutus
ExecStartPre=%h/.crocutus/bin/simard debug instance --check-collision
ExecStart=%h/.crocutus/bin/simard ooda run
Restart=on-failure

[Install]
WantedBy=default.target
```

The `ExecStartPre` collision check makes the unit **refuse to start** if it
would collide with the primary — a systemd-level fail-closed gate.

```bash
systemctl --user daemon-reload
systemctl --user enable --now crocutus-ooda.service
```

## 7. Confirm both run without interference

```bash
systemctl --user status simard-ooda.service crocutus-ooda.service --no-pager
ss -ltnp | grep -E ':8080|:8090'          # two distinct dashboard ports
ls "$HOME/.simard/state" "$HOME/.crocutus/state"   # two distinct state trees
```

Each daemon logs one-line OODA summaries to its own journal:

```bash
journalctl --user -u crocutus-ooda.service -n 20 --no-pager
```

You should see the second identity observing its own goal board, referencing
its own target — with no writes if it is read-only.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Second daemon exits immediately with a `flock`/lock error | Shared `SIMARD_STATE_ROOT` with the primary | Give it a distinct `SIMARD_HOME`/`SIMARD_STATE_ROOT`; the exclusive lock is refusing a shared root **by design** (no silent fallback) |
| Dashboard fails to bind | `SIMARD_DASHBOARD_PORT` collides | Assign a free port |
| `debug authority` shows `posture=full` unexpectedly | `identity.toml` omits `[identities.authority]`, or `SIMARD_IDENTITY_PATH` points at the wrong dir | Point at the correct identity dir; add the `[identities.authority]` block |
| A read-only identity still holds a write token | Credential scoping missed | Revoke the write credential; a read-only identity must have no write-capable credential to its target |

## See also

- [Multi-identity host isolation](../concepts/multi-identity-host-isolation.md)
  — why each singleton must be per-instance.
- [Write-authority posture](../concepts/write-authority-posture.md) — the
  read-only / scoped-write / full contract.
- [Agent instance-isolation reference](../reference/agent-instance-isolation.md)
  — the full env matrix and `simard debug instance`.
- [Deploy Crocutus as a read-only observer](../tutorials/deploy-crocutus-read-only-observer.md)
  — the concrete end-to-end deployment.
- [How to run the OODA daemon](../howto/run-ooda-daemon.md) — the base daemon
  this guide runs a second copy of.
