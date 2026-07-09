---
title: "Tutorial: Deploy Crocutus, a read-only observer of an Azure DevOps project"
description: End-to-end walkthrough of deploying Crocutus — a second autonomous Simard identity that observes the acs-mdash/acs-mdash Azure DevOps project (the hyenas repo and its sisters) strictly read-only, articulates repo-hygiene goals, runs side-by-side with the primary simard daemon on host "dev", and is provably unable to change anything.
last_updated: 2026-07-08
owner: simard
doc_type: tutorial
related:
  - ../concepts/multi-identity-host-isolation.md
  - ../concepts/write-authority-posture.md
  - ../concepts/identity-scoped-cognition.md
  - ../reference/agent-instance-isolation.md
  - ../reference/write-authority-posture-api.md
  - ../howto/run-a-second-agent-identity.md
  - ../howto/configure-pluggable-identity.md
---

# Tutorial: Deploy Crocutus, a read-only observer of an Azure DevOps project

!!! warning "Implementation status — read before running (issue #1, tracking #3067)"
    Some commands below use the **planned** interface (`SIMARD_HOME`,
    `[identities.authority]` posture, `simard debug instance`/`authority`,
    `simard install`) tracked in
    [#3067](https://github.com/rysweet/Simard/issues/3067) and will not run as-is on the
    shipped binary. **The shipped mechanism** is env-driven: set
    **`SIMARD_STATE_ROOT=$HOME/.crocutus/state`** (isolation) and
    **`SIMARD_OBSERVE_ONLY=1`** (read-only floor, enforced fail-closed by
    `read_only_guard` wired into `git_guardrails::check_git_safety` and the OODA
    engineer-dispatch), point the daemon's working directory at a **read-only clone**
    of the target, and run `simard ooda run`. The concrete, runnable version of this
    tutorial — including the guardrail proof and systemd unit — is maintained in the
    `rysweet/Crocutus` repo (`README.md`, `scripts/`). Read this tutorial for the design
    intent; run the Crocutus repo for the working procedure.

By the end of this tutorial you will have **Crocutus** running as a distinct,
read-only autonomous identity next to the primary `simard` daemon on host
`dev`. Crocutus observes the Azure DevOps project
`acs-mdash/acs-mdash` — the `hyenas` repository and its sister repos — learns
their structure and health **read-only**, and articulates repo-hygiene goals
(stale branches, missing CI, docs drift, dependency hygiene). It **never**
changes anything in that project, and you will prove that.

Crocutus is not a fork of Simard. It is a downstream **private** repository
(`rysweet/Crocutus`) that *depends on* Simard and is almost entirely
configuration: an `identity.toml` persona, prompt assets, an environment
profile, and a systemd unit. The behavior comes from two Simard abstractions
you already have references for:

- [Multi-identity host isolation](../concepts/multi-identity-host-isolation.md)
  (`SIMARD_HOME`) — so Crocutus and Simard do not collide.
- [Write-authority posture](../concepts/write-authority-posture.md)
  (`posture = "read-only"`) — so Crocutus cannot write.

!!! danger "The one non-negotiable guarantee"
    Crocutus must not make **any** change to the target repos or their Azure
    DevOps project — no commit, push, branch, PR, work-item edit, comment, or
    ACL change, anywhere. This is enforced in depth (credential, capability,
    mandate, isolation). If any layer is uncertain, Crocutus **fails closed**
    (does nothing). Getting this right matters more than any feature.

## Prerequisites

- Host `dev` reachable, with the primary `simard` daemon already running.
- `rysweet` GitHub identity (`gh auth status` green) and `az login` completed
  on host `dev`.
- The `rysweet/Crocutus` repo cloned to `~/crocutus` on host `dev`.
- A **read-only** Azure DevOps credential for `acs-mdash` — a read-scoped PAT
  (Code: Read; Work Items: Read) **or** an anonymous read-only clone path.
  **Never** a write-capable token to this project.

## Step 0 — Confirm which host `dev` is

`dev` may be the current box under another name, or a separate azlin VM. Do not
install anything until you know. Confirm the mapping first:

```bash
hostname
grep -E '\bdev\b' /etc/hosts ~/.ssh/config 2>/dev/null
az vm list -d -o table 2>/dev/null | grep -i dev
```

If `dev` is a separate VM, `ssh dev` and run the rest of this tutorial there.
If `dev` is the current host under another name, proceed locally.

## Step 1 — Provision the read-only credential (or none)

Store the read-only PAT so only Crocutus's environment sees it, and confirm it
carries **no** write scope:

```bash
# Read-only PAT: Code (Read), Work Items (Read). No write scopes.
install -m 600 /dev/stdin ~/.crocutus/ado_readonly.pat <<< "$ADO_READONLY_PAT"

# Prove the token cannot write: a read succeeds, a write is denied by AzDO.
az repos ref list \
  --org https://dev.azure.com/acs-mdash --project acs-mdash \
  --repository hyenas >/dev/null && echo "read OK"
```

!!! warning "Fail closed on credentials"
    If you cannot obtain a genuinely read-only credential, give Crocutus
    **zero** Azure DevOps credentials and rely on anonymous read (or have it do
    nothing). Never substitute a write-capable token. Absence of a write token
    is itself a guardrail layer.

## Step 2 — Author the Crocutus identity (config, not code)

In `~/crocutus/identity/identity.toml`, declare the read-only persona. This is
the whole "identity" — a persona plus a posture:

```toml
[package]
name = "crocutus-identity"
version = "0.1.0"
description = "Read-only observer of the acs-mdash Azure DevOps project"

[[identities]]
name = "crocutus"
default_mode = "engineer"
supported_base_types = ["local-harness", "rusty-clawd"]
required_capabilities = ["prompt-assets", "session-lifecycle", "memory", "reflection"]

[[identities.prompt_assets]]
id = "crocutus-system"
path = "crocutus_system.md"

[identities.memory_policy]
allow_project_writes = false
summary_scope = "session-summary"

[identities.authority]
posture = "read-only"
allowed_write_repos = []
allow_git_push = false
allow_ado_writes = false
allow_github_writes = false
```

The prompt asset `crocutus_system.md` states the mandate in the identity's own
voice — "You are Crocutus, a read-only observer of the acs-mdash Azure DevOps
project. You never commit, push, open PRs, edit work items, or change anything
in these repos. You only read, reason, and propose repo-hygiene goals." This is
the *mandate* layer; it is backed by the *capability* layer (`posture`) and the
*credential* layer (read-only PAT), so the guarantee holds even if the prompt
were wrong.

See [How to configure pluggable identities](../howto/configure-pluggable-identity.md)
for the full `identity.toml` schema and
[the posture API](../reference/write-authority-posture-api.md#identitytoml-surface)
for the `[identities.authority]` block.

## Step 3 — Define the Crocutus instance environment

Give Crocutus its own instance root and non-colliding endpoints
([env matrix](../reference/agent-instance-isolation.md#per-instance-environment-matrix)):

```bash
cat > ~/crocutus/crocutus.env <<'EOF'
SIMARD_HOME=%h/.crocutus
SIMARD_INSTANCE=crocutus
SIMARD_STATE_ROOT=%h/.crocutus/state
SIMARD_DASHBOARD_PORT=8090
SIMARD_MEMORY_SOCKET=%h/.crocutus/state/crocutus-memory.sock
SIMARD_AGENT_NAME=crocutus-ooda
SIMARD_IDENTITY=crocutus
SIMARD_IDENTITY_PATH=%h/crocutus/identity
SIMARD_PROMPT_ROOT=%h/crocutus
SIMARD_TARGET_ADO_ORG=https://dev.azure.com/acs-mdash
SIMARD_TARGET_ADO_PROJECT=acs-mdash
SIMARD_TARGET_REPO_URL=https://dev.azure.com/acs-mdash/acs-mdash/_git/hyenas
SIMARD_ADO_PAT_FILE=%h/.crocutus/ado_readonly.pat
EOF
```

`SIMARD_TARGET_REPO_URL` is the concrete write target the guardrail probe
checks in Step 7. `SIMARD_ADO_PAT_FILE` points Simard at the **read-only** PAT
file from Step 1; Simard loads it as a read credential and is never given a
write token (it is a Simard-owned variable, not the `az`-native
`AZURE_DEVOPS_EXT_PAT`).

## Step 4 — Build and install Crocutus's own binary copy

Crocutus depends on Simard; it does not vendor Simard's source. Build the
downstream crate (which pulls Simard as a git dependency) and install into the
Crocutus instance root:

```bash
cd ~/crocutus && cargo build --release --quiet
SIMARD_HOME=$HOME/.crocutus SIMARD_INSTANCE=crocutus \
  ./target/release/simard install     # → ~/.crocutus/bin/simard
```

## Step 5 — Verify isolation and posture BEFORE starting

This is the gate. Both checks must pass; if either is uncertain, stop.

```bash
set -a; . <(sed 's/%h/'"$HOME"'/g' ~/crocutus/crocutus.env); set +a

simard debug instance --check-collision
simard debug authority
```

Expected — disjoint paths from the primary, and a read-only posture:

```
instance_name=crocutus
instance_home=/home/azureuser/.crocutus
state_root=/home/azureuser/.crocutus/state
dashboard_port=8090
ooda_unit=crocutus-ooda.service
...
posture=read-only
git_push_check=REFUSED (read-only)
ado_write_check=REFUSED (read-only)
github_write_check=REFUSED (read-only)
```

## Step 6 — Prove the read-only guardrail

Before running the daemon against the real project, prove — mechanically —
that every write path to `hyenas` is refused. This is a required acceptance
artifact.

### 6a. No write credential

```bash
# The only ADO credential in Crocutus's environment is the read-only PAT file.
ls -l ~/.crocutus/*.pat                 # ~/.crocutus/ado_readonly.pat only
# Prove it: a write via this token is denied by Azure DevOps itself.
az repos pr create --org "$SIMARD_TARGET_ADO_ORG" --project acs-mdash \
  --repository hyenas --source-branch x --target-branch main 2>&1 \
  | grep -qi 'denied\|forbidden\|unauthorized' && echo "write DENIED by ADO (expected)"
```

### 6b. Capability refuses the write

The posture dry-run probe (executes nothing) confirms Simard's own code paths
refuse writes to the target and exits non-zero if any write would be allowed:

```bash
simard debug authority --probe-write \
  "$SIMARD_TARGET_ADO_ORG/acs-mdash/_git/hyenas"
```

```
probe target=https://dev.azure.com/acs-mdash/acs-mdash/_git/hyenas
git push        => REFUSED (read-only)
az repos pr     => REFUSED (read-only)
work-item edit  => REFUSED (read-only)
exit=0 (all writes refused as expected)
```

With 6a and 6b together you have proven the guarantee at two independent
layers: the credential cannot write, and even with a credential the capability
layer refuses. That is defense in depth, failing closed.

## Step 7 — Launch the Crocutus daemon

Install the user unit (its name never clashes with `simard-ooda.service`) with
the collision check as a start gate:

```ini
# ~/.config/systemd/user/crocutus-ooda.service
[Unit]
Description=Crocutus — read-only observer of acs-mdash (side-by-side with simard)
After=network-online.target

[Service]
Type=simple
EnvironmentFile=%h/crocutus/crocutus.env
ExecStartPre=%h/.crocutus/bin/simard debug instance --check-collision
ExecStartPre=%h/.crocutus/bin/simard debug authority --probe-write ${SIMARD_TARGET_REPO_URL}
ExecStart=%h/.crocutus/bin/simard ooda run
Restart=on-failure

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now crocutus-ooda.service
```

The two `ExecStartPre` gates mean the daemon **refuses to start** if it would
collide with the primary or if any write path to the target is not refused — a
systemd-level fail-closed. The probe target `${SIMARD_TARGET_REPO_URL}` and the
read-only credential `${SIMARD_ADO_PAT_FILE}` are both expanded from the
`EnvironmentFile` loaded above (this is a plain, non-templated unit, so the
`%i` instance specifier is *not* used).

## Step 8 — Verify the end state

**Both daemons running, isolated:**

```bash
systemctl --user status simard-ooda.service crocutus-ooda.service --no-pager
ss -ltnp | grep -E ':8080|:8090'
ls ~/.simard/state ~/.crocutus/state
```

**Crocutus thinking about hyenas, read-only:**

```bash
journalctl --user -u crocutus-ooda.service -n 40 --no-pager
SIMARD_STATE_ROOT=$HOME/.crocutus/state simard goal list
```

You should see hygiene goals referencing the `acs-mdash` repos, for example:

```
[crocutus] OODA cycle 7: observed 4 repos, 3 priorities, dispatched 0 actions (read-only), 0 writes
goal: "hyenas: 6 stale branches (>90d) — propose pruning policy"    status: not-started
goal: "sister repo 'dens': no CI workflow — propose pipeline"        status: not-started
goal: "hyenas: README drift vs. src layout — propose docs refresh"   status: not-started
```

Note `dispatched 0 actions (read-only), 0 writes` — Crocutus proposes goals but
takes no acting/writing steps against the target.

## What you have

- `rysweet/Crocutus` (private) exists and **depends on** Simard (no copied
  source).
- The Crocutus daemon runs on host `dev` as a **distinct** read-only identity,
  with its own home, memory, goal board, ports, socket, and systemd unit —
  side-by-side with `simard`, no interference.
- It articulates `hyenas` repo-hygiene goals and is **provably** unable to
  change anything: no write credential (6a) and capability refuses writes (6b),
  both failing closed.

## Abstraction-gap note

If deploying Crocutus required duplicating large amounts of Simard Rust rather
than the two small parameterizations
([instance isolation](../concepts/multi-identity-host-isolation.md) and
[write-authority posture](../concepts/write-authority-posture.md)), that is an
abstraction gap. Record it as a Simard issue and fix the abstraction upstream —
do not fork. A correctly abstracted Simard makes a second identity *mostly
configuration*, which is exactly what this tutorial demonstrates. The cognition
half of that abstraction — seeding Crocutus's own hyenas-observation goals and
running an [observe-only Act phase](../concepts/identity-scoped-cognition.md)
instead of inheriting Simard's defaults and dispatching engineers — is
[identity-scoped cognition](../concepts/identity-scoped-cognition.md).

## See also

- [Multi-identity host isolation](../concepts/multi-identity-host-isolation.md)
- [Write-authority posture](../concepts/write-authority-posture.md)
- [Identity-scoped cognition](../concepts/identity-scoped-cognition.md) — seed goals, target scope, and the observe-only Act phase
- [How to run a second agent identity](../howto/run-a-second-agent-identity.md)
- [How to configure pluggable identities](../howto/configure-pluggable-identity.md)
- [Write-authority posture API reference](../reference/write-authority-posture-api.md)
