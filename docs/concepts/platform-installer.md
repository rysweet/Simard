---
title: The Simard platform installer — provision, install, prove, run
description: Why and how a single idempotent, fail-closed installer stands up a Simard-family agent daemon (Simard, Crocutus, or a future identity) on a fresh host — detecting and preparing the OS toolchain and native deps, building a portable cognitive-memory store, materializing an isolated identity and systemd unit, proving guardrails, and verifying a live OODA cycle over local or azlin-remote targets.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ./lbug-portability-libstdcxx-abi.md
  - ./pluggable-identity.md
  - ./reconcile-and-self-deploy.md
  - ../howto/install-a-simard-family-agent.md
  - ../howto/run-the-installer-preflight-doctor.md
  - ../reference/platform-installer-cli.md
  - ../tutorials/stand-up-a-read-only-observer.md
---

# The Simard platform installer — provision, install, prove, run

## The problem

Simard and its sibling identities (Crocutus today; more to come) are the *same*
cognition — one OODA daemon, one cognitive-memory engine, one brain — run
side-by-side as distinct identities. Building a second identity is deliberately
"mostly configuration" (see [Pluggable identity](./pluggable-identity.md)). But
*standing up* an identity's daemon on a **fresh host** was not mostly
configuration. It was a sequence of manual, error-prone steps that a person had
to get right in order, and one of them was a hard portability crash.

The first real second-identity deployment — installing **Crocutus** (a
read-only observer) on host `dev`, an Azure VM reached with `azlin connect dev`
over Bastion — surfaced nine concrete points of friction. Every one of them is a
requirement the installer now owns:

1. **No Rust toolchain.** The fresh host had no `rustup`/`cargo`; they were
   installed by hand.
2. **Missing native build deps.** The build failed on missing `pkg-config` and
   `libssl-dev` (and needs `build-essential`, `cmake`, `clang`); each was
   `sudo apt install`-ed reactively as the build failed again.
3. **No `amplihack` CLI.** Simard's OODA daemon runs an
   [amplihack freshness gate](./amplihack-freshness-gate.md) and dispatches
   amplihack recipes, so the `amplihack` CLI and its
   `~/.amplihack/amplifier-bundle` must be present and synced.
4. **A stale dependency check.** `simard ensure-deps` still probed for a Python
   `kuzu` package. Simard's cognitive memory is **embedded lbug** (compiled and
   linked into the binary via `amplihack-memory-lib`), not kuzu. The check was
   misleading noise.
5. **A hard portability crash.** On host `dev` (Ubuntu 26.04 LTS, libstdc++
   `GLIBCXX_3.4.35`) the binary **segfaulted during cognitive-memory init** —
   `LibraryCognitiveMemory::open` → `lbug::main::Database::initBufferManager` →
   `std::vformat` SIGSEGV — while working fine on host `ia2` (Ubuntu 25.10,
   libstdc++ 3.4.34). This is the centerpiece; see
   [lbug portability](./lbug-portability-libstdcxx-abi.md).
6. **Manual identity/isolation wiring.** `SIMARD_STATE_ROOT`, `SIMARD_IDENTITY`,
   the prompt-asset dir, a distinct dashboard port, and a distinct systemd unit
   name — all set by hand, with nothing stopping two identities from colliding.
7. **Manual systemd wiring.** Installing the user unit, its fail-closed
   `ExecStartPre` guardrail gate, `daemon-reload`/`enable`/`start`, and checking
   for a stable PID rather than a crash-loop.
8. **Manual credential wiring.** A read-only Azure DevOps token for a read-only
   observer, with nothing enforcing least privilege or *failing closed* when a
   required credential was absent.
9. **Reaching the host.** This fleet uses [`azlin`](https://github.com/rysweet/azlin)
   over Azure Bastion; the installer must work the same way remotely as it does
   locally.

The Crocutus exercise proved the framework works. It also proved that
"provision → install → prove → run" was a tribal-knowledge ritual. The platform
installer turns that ritual into **one reliable, idempotent, guardrailed
operation**.

## What the installer guarantees

Given a **host**, an **identity config**, and **credentials**, the installer
performs an install that is:

- **Idempotent.** Re-running it converges to the same working state. It never
  double-installs a toolchain, never clobbers an existing identity's state, and
  is safe to run again after a partial failure.
- **Fail-closed.** Every guardrail and every required credential is *proven*
  before the daemon starts. Missing credential, unproven read-only floor, or a
  cognitive-memory store that will not open cleanly are hard stops with a
  precise, actionable error — never a silent fallback or a degraded run.
- **Multi-identity safe.** It never collides with an existing identity's state
  root, dashboard port, socket, or systemd unit name.
- **Local or remote.** The same operation runs on the local host or against an
  `azlin` target over Bastion.

The install is not "done" when the unit is enabled. It is done when the
installer has **verified a live OODA cycle**: cognitive memory opened without a
SIGSEGV, the guardrail proof passed, and the daemon holds a **stable new PID**
(not a crash-loop).

## Where the installer lives, and why

The installer's canonical home is the **Crocutus repository**, alongside the
identity scaffold it already owns (`scripts/`, `config/`, `systemd/`,
`identity/`). Crocutus is the natural multi-identity owner: it is a
configuration-and-prompts identity built *on* Simard, and it already carries the
working `install-on-dev.sh` shape (build → materialize env → prove guardrail →
install unit).

Simard core exposes a **thin `simard platform install` rail** (namespaced under
`simard platform …` to avoid the pre-existing `simard install` binary-persist
verb) that shells to the same logic. The rail is a convenience entry point, not a
second source of truth — it keeps a fat installer out of the framework crate.
Engine and portability work (the lbug fix) lives in **`amplihack-memory-lib`**,
and Simard consumes it by bumping its dependency pin. This placement follows the
project's standing rule:
*prefer recipes, prompts, config, and thin rails over new code in the framework;
memory/engine work belongs in the memory library.*

```
Crocutus repo (canonical)            Simard repo (thin)              amplihack-memory-lib (engine)
─────────────────────────            ──────────────────              ─────────────────────────────
scripts/install.sh    ◄────────────  `simard platform install` rail  lbug build-from-source mode
scripts/doctor.sh     ◄────────────  `simard platform doctor` rail    (fixes the libstdc++ ABI crash)
config/<identity>.env                (ensure-deps: kuzu check              ▲
systemd/<identity>-ooda.service       removed)                            │ Simard bumps its pin
identity/<identity>_system.md                                            │ to consume the fix
```

> **Verb note.** The rail is `simard platform install` / `simard platform
> doctor`, *not* bare `simard install` — that verb already exists and persists
> the current binary to `~/.simard/bin` (used by the `npx` wrapper). The platform
> installer namespaces itself under `simard platform …` to avoid the collision.
> See the [CLI reference](../reference/platform-installer-cli.md#entry-points).

> **Scaffold status.** `scripts/install.sh` and `scripts/doctor.sh` are the
> **target** shape. Today Crocutus ships `scripts/install-on-dev.sh`
> (interactive, host-`dev`-specific, in-place overwrite, no idempotency /
> `--upgrade` / `--uninstall`), plus `bootstrap-readonly.sh` and
> `prove-guardrail.sh`. Reaching the target means generalizing the on-`dev`
> script, extracting the preflight into `doctor.sh`, and adding idempotency,
> remote/azlin support, upgrade/uninstall, and the atomic binary swap.

## The install as a phase machine

The installer is a small, ordered state machine. Each phase is independently
idempotent and fails closed. The
[CLI reference](../reference/platform-installer-cli.md) is the authoritative
contract for each phase's inputs, outputs, and exit codes; the shape is:

1. **Preflight (doctor).** Detect the host: OS and libstdc++ version, Rust
   toolchain, native build deps, the `amplihack` CLI and bundle, free disk,
   dashboard-port availability, systemd unit-name collisions, and the **lbug
   source-build prerequisites**. Auto-remediate what is safe to remediate (install
   the toolchain, `apt install` the native deps, install/sync `amplihack`) or stop
   with an exact error. Preflight is also runnable on its own — see
   [Run the preflight doctor](../howto/run-the-installer-preflight-doctor.md).
2. **Build a portable binary.** Build the agent binary with a cognitive-memory
   store compiled **from source** against the host toolchain — the installer's
   build phase exports `LBUG_BUILD_FROM_SOURCE=1` (and `amplihack-memory-lib`'s
   persistent-feature guard warns loudly if a prebuilt were ever linked without
   it). There is no prebuilt lbug in the process to mismatch the host libstdc++,
   which is why the SIGSEGV does not recur; see
   [lbug portability](./lbug-portability-libstdcxx-abi.md).
3. **Materialize the identity.** Create the **identity home** `~/.<identity>`
   (holding `bin/<identity>` and `targets/`), the isolated state root
   `~/.<identity>/state`, the materialized env file (with a distinct dashboard
   port and identity name), the prompt-asset location, and the systemd **user**
   unit under a distinct name — chosen so it cannot collide with any identity
   already installed on the host.
4. **Prove guardrails.** Run the identity's guardrail proof (for Crocutus, the
   read-only floor at the capability layer *and* the absence of a write
   credential at the credential layer). If the proof is uncertain, the install
   stops here. The same proof is wired as the unit's `ExecStartPre` start-gate,
   so the daemon also refuses to start later if the guardrail regresses.
5. **Start the daemon.** `daemon-reload`, enable, and start the user unit.
6. **Verify a live cycle.** Confirm a positive **store-opened / first-OODA-cycle
   log marker**, no `initBufferManager`/`std::vformat` SIGSEGV, the guardrail gate
   passed, and a **stable new PID** across at least one `RestartSec` window. A
   crash-loop (PID flapping / unit sub-state `failed`) is a failed install, not a
   warning.
7. **Report.** Emit a durable, machine-readable summary of what was detected,
   remediated, built, and verified.

## Multi-identity isolation is structural, not advisory

Two identities coexist on one host only if they never share mutable state. The
installer enforces these independent isolation axes and refuses to proceed if any
would collide with an already-installed identity:

| Axis | Mechanism | Why it matters |
|------|-----------|----------------|
| **Identity home** | Distinct `~/.<identity>` (holds `bin/<identity>`, `targets/`, materialized env) | Binaries, target clones, and env for one identity never overlap another's. |
| **State root** | Distinct `SIMARD_STATE_ROOT` (`~/.<identity>/state`) — a subtree of the identity home | The goal board and the cognitive-memory `flock` are per-identity; a shared root deadlocks or corrupts memory. |
| **Dashboard port** | Distinct `SIMARD_DASHBOARD_PORT` | Two daemons cannot bind the same port. |
| **Systemd unit** | Distinct `<identity>-ooda.service` user unit | `simard-ooda.service` and `crocutus-ooda.service` enable/start/stop independently. |
| **Persona + creds** | Distinct `SIMARD_IDENTITY`, prompt root, and credential file | Personas and least-privilege credentials never cross. |

Note the identity home (`~/.<identity>`) and the state root
(`~/.<identity>/state`) are **distinct**: the binary lives at
`~/.<identity>/bin/<identity>`, not under the state root. This is the same
isolation Crocutus ships today, promoted from "set these environment variables by
hand and hope" to "the installer allocates and verifies them, and refuses on
collision."

## Fail-closed, in depth

The installer inherits Simard's defense-in-depth stance. It never runs a daemon
it cannot prove is safe:

- **A write-capable credential present → stop.** For a read-only identity, the
  guardrail proof refuses to proceed if any write-capable token
  (`AZURE_DEVOPS_EXT_PAT`, `SYSTEM_ACCESSTOKEN`, `ADO_WRITE_PAT`) is in the
  environment. This is the real, enforced credential guardrail.
- **A declared-required credential absent → stop.** If the identity config marks
  a credential as required and it is absent, the install stops. (For a read-only
  *observer* the read PAT is optional by default — absent, the observer degrades
  to anonymous read or does nothing, fail-closed at runtime — so requiring it is
  a per-identity choice, not an installer default.)
- **Unproven guardrail → stop.** The guardrail proof must pass before start, and
  it is *also* the unit's start-gate, so a later regression keeps the daemon
  down rather than up-and-unsafe.
- **Cognitive memory won't open cleanly → stop.** If the store cannot initialize
  without a SIGSEGV on this host, the install fails at the build/verify phase
  with the ABI diagnosis — it does not ship a daemon that crash-loops.

There are no wall-clock timeouts on the agentic verification steps; the installer
waits for a real live-cycle signal rather than guessing.

## Upgrade and uninstall

Upgrades reuse the [reconcile-and-self-deploy](./reconcile-and-self-deploy.md)
discipline: stop the unit, swap the binary **atomically** (write-new-then-rename,
which survives the held-inode problem of overwriting a running binary), restart,
and re-verify a live cycle — rolling back on failure. Uninstall stops and
disables the unit and removes the identity's state root and unit file, leaving
other identities on the host untouched.

## What this closes

- The nine friction points above become one command.
- The stale `kuzu` check is removed from `simard ensure-deps` (a small Simard
  change) so dependency reporting tells the truth: memory is embedded lbug, not
  kuzu. `ensure-deps` stays a minimal runtime check (`git`, `python3`, `gh`);
  the heavier build/host preconditions belong to `simard platform doctor`. See
  the [CLI reference](../reference/platform-installer-cli.md#simard-ensure-deps-and-the-removed-kuzu-check).
- The lbug libstdc++ ABI crash is fixed *in the engine library*
  (`amplihack-memory-lib`) and consumed by a pin bump, not forked into Simard.
- Standing up the next identity is now genuinely "mostly configuration" — end to
  end, including the host it runs on.

## See also

- [lbug portability across libstdc++ ABIs](./lbug-portability-libstdcxx-abi.md)
  — the centerpiece root-cause and fix.
- [Install a Simard-family agent](../howto/install-a-simard-family-agent.md) —
  the operational how-to (local and azlin-remote).
- [Run the preflight doctor](../howto/run-the-installer-preflight-doctor.md).
- [Platform installer CLI reference](../reference/platform-installer-cli.md).
- [Stand up a read-only observer](../tutorials/stand-up-a-read-only-observer.md)
  — end-to-end tutorial on host `dev`.
