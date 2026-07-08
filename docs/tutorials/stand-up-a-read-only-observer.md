---
title: Stand up a read-only observer on a fresh host
description: A guided, end-to-end walkthrough that uses the platform installer to stand up the Crocutus read-only observer daemon on host "dev" (Ubuntu 26.04) — from an unprepared host to a verified live OODA cycle with cognitive memory initializing cleanly and the read-only guardrail proven, running side-by-side with the primary Simard daemon.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../concepts/platform-installer.md
  - ../concepts/lbug-portability-libstdcxx-abi.md
  - ../howto/install-a-simard-family-agent.md
  - ../howto/run-the-installer-preflight-doctor.md
  - ../reference/platform-installer-cli.md
---

# Stand up a read-only observer on a fresh host

In this tutorial you will use the platform installer to stand up **Crocutus** — a
read-only observer built on the Simard framework — on host `dev`, an Azure VM
reached over Bastion with [`azlin`](https://github.com/rysweet/azlin). You will
go from an *unprepared* host all the way to a **verified live OODA cycle**:
cognitive memory opens with no SIGSEGV, the read-only guardrail is proven, and
the daemon runs side-by-side with the primary Simard daemon.

By the end you will have seen every phase the installer runs and why each one
exists. For the design behind it, read
[The Simard platform installer](../concepts/platform-installer.md).

## What you need

- Operator access to run `azlin` (the `az ssh` extension and Bastion access).
- A Crocutus checkout on your operator machine (it declares Simard as a Cargo
  git dependency — you do **not** fork Simard).
- A **read-only** Azure DevOps PAT (`Code: Read`) for the observed project.
  Never a write token.

You do **not** need to pre-install Rust, cmake, or `amplihack` on `dev` — the
installer provisions those.

## Step 1 — Confirm the target host

```bash
azlin list
azlin connect dev -- hostname
```

Confirm `dev` is the host you intend and that you can reach it. The installer
also asks for confirmation before it changes anything (skip with `--yes` for
unattended runs).

## Step 2 — Run the preflight doctor

Before installing, see exactly what the host needs. Run the doctor remotely:

```bash
simard platform doctor --identity ./config/crocutus.env --remote azlin:dev
```

On a fresh 26.04 host you will see the doctor detect gaps and plan
remediations — and confirm that lbug will build from source against the host
toolchain (which is what avoids the SIGSEGV):

```
  ✓ os              Ubuntu 26.04 LTS
  ✓ libstdc++       GLIBCXX_3.4.35
  ⧗ rust            missing → will install via rustup
  ⧗ native-deps     missing: cmake, libssl-dev → will apt-install
  ⧗ amplihack       missing → will install + sync amplifier-bundle
  ✓ disk            18 GiB free
  ✓ port            8090 free
  ✓ unit-name       crocutus-ooda.service available
  ✓ state-root      ~/.crocutus/state unused
  ✓ lbug-source     toolchain present → lbug builds from source (LBUG_BUILD_FROM_SOURCE=1)
```

That last line is the key to this whole exercise: on 26.04 a *prebuilt* lbug would
mismatch the host's libstdc++ `std::format` ABI and segfault. Because the store is
built **from source** against the host toolchain (unconditionally), there is no
prebuilt in the process to mismatch. See
[lbug portability](../concepts/lbug-portability-libstdcxx-abi.md).

## Step 3 — Supply the read-only credential

Provide the read-only PAT used to fetch the target's read-only clone (never a
write token):

```bash
export ADO_READONLY_PAT=…     # READ-scoped PAT; never a write token
```

If a *write-capable* token is present in your environment, the installer treats
that as a guardrail failure and refuses — a read-only observer must have no write
token. (For a read-only observer the read PAT itself is optional: without it the
observer degrades to anonymous read or does nothing, fail-closed at runtime.)

## Step 4 — Install

Run the one command. The installer executes every phase on `dev`:

```bash
simard platform install --identity ./config/crocutus.env --remote azlin:dev --yes
```

Watch it work through the
[phase machine](../concepts/platform-installer.md#the-install-as-a-phase-machine):

1. **preflight** — installs Rust, the native deps, and `amplihack`.
2. **build** — builds the Crocutus binary with a **from-source lbug** matched to
   `dev`'s libstdc++ (this is what prevents the SIGSEGV).
3. **materialize** — creates `~/.crocutus/state`, the env file, the prompt root,
   and the `crocutus-ooda.service` **user** unit with its guardrail start-gate.
4. **prove-guardrails** — proves the read-only floor holds (target writes
   refused, reads allowed) *and* that no write credential is present.
5. **start** — `daemon-reload`, enable, start.
6. **verify** — confirms cognitive memory opened (no SIGSEGV) and the daemon
   reached a live OODA cycle with a stable new PID.
7. **report** — prints a durable summary.

## Step 5 — See the live cycle

```bash
azlin connect dev -- journalctl --user -u crocutus-ooda.service -f
```

You should see the read-only identity assumed, the guardrail proven, cognitive
memory open **without** any `initBufferManager` / `std::vformat` SIGSEGV, and the
OODA loop begin observing. That is the outcome the whole installer exists to
guarantee.

## Step 6 — Confirm side-by-side isolation

Crocutus runs next to the primary Simard daemon without interfering:

```bash
azlin connect dev -- systemctl --user status simard-ooda.service crocutus-ooda.service
azlin connect dev -- ls '~/.simard/state' '~/.crocutus/state'   # distinct roots
```

Distinct state roots, distinct ports, distinct units — two identities, one host,
no collision.

## Step 7 — Prove it changed nothing

Crocutus is read-only in depth. Prove the guardrail any time:

```bash
azlin connect dev -- '~/crocutus/scripts/prove-guardrail.sh'
```

It exits non-zero if *any* layer (capability or credential) is uncertain. A green
proof means Crocutus cannot write to the target — by construction.

## What you learned

- The installer turns "provision → install → prove → run" into one idempotent,
  fail-closed command — locally or over azlin.
- The preflight doctor detects the host's libstdc++ and verifies the from-source
  lbug toolchain. The store is *always* built from source (never a prebuilt), so
  there is no mismatched ABI in the process — which is what stops the 26.04
  SIGSEGV.
- "Done" means a **verified live OODA cycle** with cognitive memory open, not
  merely an enabled unit.
- Multi-identity isolation (state root, port, unit) is enforced, so a new
  identity is safe to add to a host that already runs one.

## Next steps

- [Install a Simard-family agent](../howto/install-a-simard-family-agent.md) —
  the operational reference for upgrades, uninstall, and local installs.
- [Run the preflight doctor](../howto/run-the-installer-preflight-doctor.md).
- [lbug portability across libstdc++ ABIs](../concepts/lbug-portability-libstdcxx-abi.md).
