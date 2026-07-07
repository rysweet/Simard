---
title: Argv-free meeting/Signal agent-proxy invocation reference
description: >
  Reference for the argv-free prompt transport at the meeting/Signal
  `PersistentAgentProxy` spawn site (src/meeting_backend/agent_proxy.rs) — the
  DISTINCT per-turn agent launch the Signal channel reaches through a meeting
  session. Specifies the stdin transport (spawn_payload::attach_prompt_std +
  feeder thread), the per-provider stdin invocation form (copilot: no `-p`;
  claude: bare `-p`), the removal of the `-p <prompt>` argv inlining that caused
  the live instant (elapsed_ms=0) E2BIG "Argument list too long" failure on every
  Signal message, the preserved liveness/reaper behaviour, and the argv-free
  invariant its tests enforce (#2640).
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./argv-free-copilot-invocation.md
  - ./large-payload-spawn-api.md
  - ../concepts/signal-meeting-agent-e2big.md
  - ../concepts/e2big-elimination.md
  - ../howto/diagnose-signal-meeting-e2big.md
  - ../howto/add-a-safe-agent-spawn-site.md
  - ../howto/set-up-the-signal-channel.md
  - ../prompt-delivery.md
  - ../../src/meeting_backend/agent_proxy.rs
  - ../../src/spawn_payload/mod.rs
  - ../../src/prompt_delivery/mod.rs
---

# Argv-free meeting/Signal agent-proxy invocation reference

> **Status: implemented.** The fix lives in
> [`src/meeting_backend/agent_proxy.rs`](https://github.com/rysweet/Simard/blob/main/src/meeting_backend/agent_proxy.rs)
> (`PersistentAgentProxy::invoke_agent_streaming` and `resolve_agent_command`).
> It reuses the shared spawn facade
> [`simard::spawn_payload`](./large-payload-spawn-api.md) and the underlying
> [`simard::prompt_delivery`](../prompt-delivery.md) stdin transport. Closes the
> meeting/Signal residual of
> [#2640](https://github.com/rysweet/Simard/issues/2640). For the incident
> narrative read
> [The Signal-meeting E2BIG incident](../concepts/signal-meeting-agent-e2big.md);
> for a live-occurrence runbook read
> [Diagnose Signal-meeting E2BIG](../howto/diagnose-signal-meeting-e2big.md).

## Contents

- [What this covers](#what-this-covers)
- [The invariant](#the-invariant)
- [Why the old invocation overflowed argv](#why-the-old-invocation-overflowed-argv)
- [The spawn site](#the-spawn-site)
  - [Naming: two ids, one component](#naming-two-ids-one-component)
  - [`resolve_agent_command` — per-provider stdin form](#resolve_agent_command-per-provider-stdin-form)
  - [`build_agent_command` — the fixed-argv seam](#build_agent_command-the-fixed-argv-seam)
  - [`invoke_agent_streaming` — stdin transport + feeder thread](#invoke_agent_streaming-stdin-transport-feeder-thread)
- [Preserved behaviour](#preserved-behaviour)
- [Feeder / reaper interaction](#feeder-reaper-interaction)
- [No silent fallback](#no-silent-fallback)
- [meeting_backend spawn-site audit](#meeting_backend-spawn-site-audit)
- [Tests](#tests)

## What this covers

The [argv-free Copilot/OODA reference](./argv-free-copilot-invocation.md) covers
three copilot launch sites in `base_type_copilot` and `ooda_actions`. This
document covers a **fourth, distinct** site that those fixes did not reach: the
per-turn agent launch in the meeting backend's
[`PersistentAgentProxy`](https://github.com/rysweet/Simard/blob/main/src/meeting_backend/agent_proxy.rs).

This is the launch site the **Signal channel** hits. An inbound Signal message
(`open{provider=Copilot tag=signal}`) is routed through a meeting session;
`MeetingBackend::send_message` builds the turn prompt and dispatches it through
`PersistentAgentProxy::invoke_agent_streaming`, which spawns `copilot`/`claude`
directly (thin proxy, no PTY — issue #2179). Before this fix, that proxy inlined
the whole prompt as a `-p <prompt>` argv token, so it was its **own** E2BIG
launch site independent of the base_type_copilot spawn facade fixed earlier.

## The invariant

> **Prompt bytes never appear in `argv` or `envp`.** The meeting/Signal agent
> proxy delivers the turn prompt to the agent on **stdin**; only fixed-size flags
> ride on `argv`.

Because the prompt no longer contributes to the process argument vector, the
prompt (a meeting-turn preamble plus the user's message — an arbitrarily large,
attacker-controlled Signal message) can no longer exceed `ARG_MAX`. This
generalises the [Subprocess Prompt Delivery](../prompt-delivery.md) contract to
the meeting proxy, matching the stdin transport already proven at the
base_type_copilot meeting/Signal sites.

## Why the old invocation overflowed argv

The old proxy built the command by hand and inlined the prompt:

```text
copilot --allow-all-tools --allow-all-paths -p "<ENTIRE PROMPT>"
```

`invoke_agent_streaming` called `cmd.arg("-p").arg(prompt)` — so the whole
prompt became one `argv` element. On Linux, `argv` + `envp` must fit inside
`ARG_MAX` (~2 MiB total; ~128 KiB per single argument). A meeting turn whose
preamble + user message crossed that budget made `execve` return **`E2BIG`**
(`errno 7`, "Argument list too long"). Because the failure is **pre-exec**, the
child never runs: `Command::spawn()` returns an `io::Error` immediately, so the
turn failed with `elapsed_ms=0` and the Signal user's message (`"Status?"` in
the live report) never reached the agent.

The fix removes `-p <prompt>` and pipes the prompt on stdin:

```text
copilot --allow-all-tools --allow-all-paths        # prompt fed on stdin, EOF-terminated
```

`copilot` reads a non-TTY stdin as its prompt when `-p` is absent, so the prompt
travels through a pipe and nothing prompt-sized is on `argv`.

## The spawn site

`src/meeting_backend/agent_proxy.rs`.

### Naming: two ids, one component

The proxy answers to **two** strings in the logs; both name the *same*
component, the meeting `PersistentAgentProxy`:

- **`persistent-agent-proxy`** — its [`BaseTypeId`], the `base_type` field of
  every `SimardError::AdapterInvocationFailed` it returns, and its `tracing`
  target.
- **`meeting-agent-proxy`** — the spawn-**site** tag passed to
  `spawn_payload::record_spawn_failure`, so a recorded spawn diagnosis surfaces
  in the Overseer sink as `evidence=[meeting-agent-proxy]`.

When grepping: `persistent-agent-proxy` finds the returned error and its span;
`meeting-agent-proxy` finds the recorded pre-exec spawn diagnosis. They are not
two components — the dual naming is deliberate (base-type identity vs.
failure-site provenance).

### `resolve_agent_command` — per-provider stdin form

The two agent CLIs disagree on how "print mode" is requested, so the per-turn
`argv` differs by provider — but neither carries the prompt:

| Provider (`RuntimeConfig.llm_provider`) | `agent_cmd` | `agent_base_args` | Prompt source |
| --- | --- | --- | --- |
| `Copilot` | `copilot` | `["--allow-all-tools", "--allow-all-paths"]` | **stdin** (no `-p`; copilot reads stdin when `-p` absent) |
| `RustyClawd` | `claude` | `["-p", "--allowedTools", "all"]` | **stdin** (bare `-p` = print mode; prompt piped, no positional) |

The Copilot arm is unchanged from before the fix (it already omitted `-p` in the
base args). The RustyClawd arm gains a **bare** `-p` (a flag with no value) so
`claude` stays in non-interactive print mode while reading the prompt from piped
stdin — the documented `echo … | claude -p` shape. In both cases `argv` is
short and constant-size regardless of prompt length.

### `build_agent_command` — the fixed-argv seam

Command construction is factored out of `invoke_agent_streaming` into a small
private helper that returns a fully-formed `std::process::Command` carrying
**only** the fixed-size flags. The prompt is deliberately **not** a parameter of
the builder:

```rust
/// Build the fixed-argv agent command (no prompt, no stdin wired). Factored out
/// of `invoke_agent_streaming` so the argv-constant test (T1) can assert on the
/// argument vector via `Command::get_args()` without spawning a process.
fn build_agent_command(&self) -> Command {
    let mut cmd = Command::new(&self.agent_cmd);
    cmd.args(&self.agent_base_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.process_group(0);
    if let Some(dir) = &self.workdir {
        cmd.current_dir(dir);
    }
    cmd
}
```

`invoke_agent_streaming` spawns exactly this command after `attach_prompt_std`
wires its stdin to a pipe. Because `attach_prompt_std` adds **no** `argv` tokens
(it only sets stdin), the builder's argument vector *is* the exact vector
`execve` receives. That is what makes the prompt argv-independent **by
construction**, and it gives [T1](#tests) a spawn-free seam to assert on — the
test inspects `build_agent_command().get_args()` directly rather than trying to
observe a live `execve`.

### `invoke_agent_streaming` — stdin transport + feeder thread

The transport change is confined to the spawn preamble and post-spawn feed:

- **Build via the seam:** `let mut cmd = self.build_agent_command();` replaces
  the former inline `Command::new(...).args(...).arg("-p").arg(prompt)...` block.
  Because the prompt is no longer a construction input, there is nothing to pass
  to `.arg` — the prompt cannot reach `argv` even by accident.
- **Removed:** the old `.arg("-p").arg(prompt)` and `.stdin(Stdio::null())`.
- **Added:** `let applied = spawn_payload::attach_prompt_std(&mut cmd, prompt.as_bytes())?;`
  before `spawn()`. This forces `PromptDelivery::Stdin`, sets the child's stdin
  to a pipe, and returns an `AppliedPromptStd` RAII feed guard that owns the
  prompt bytes.
- **Feed on a thread:** after `spawn()`, `let stdin = child.stdin.take();` and
  `let feeder = std::thread::spawn(move || applied.feed(stdin));`. The feeder
  writes the prompt to the child's stdin and closes it (EOF). It runs on its own
  thread so a large prompt can never deadlock against the child filling stdout
  (the reader threads drain stdout/stderr concurrently).
- **Join after the read loop:** the feeder is joined after the stdout-collection
  loop (and after any idle-reaper group kill). Its result is interpreted as:
  - `Ok(())` — prompt delivered.
  - `Err(e)` where `e.kind() == BrokenPipe` — **tolerated** (the child exited or
    was reaped before consuming all stdin; see below).
  - any other `Err(e)` — surfaced as
    `SimardError::AdapterInvocationFailed { base_type: "persistent-agent-proxy", … }`.
  - a **panicked** feeder thread (`join()` returns `Err`) — also surfaced as
    `AdapterInvocationFailed`, never silently dropped.

The prompt is passed **byte-exact** as `prompt.as_bytes()` — no escaping,
quoting, sanitisation, or NUL handling — so a Signal message round-trips to the
agent unmodified.

## Preserved behaviour

This is strictly a **transport** change. Everything else in the proxy is
unchanged:

- **Process group:** `cmd.process_group(0)` — the child leads its own group so
  the liveness reaper can `SIGKILL` the whole subtree (`kill_process_group`).
- **Stdio:** stdout and stderr stay piped and drained on reader threads; only the
  former `Stdio::null()` stdin becomes a pipe.
- **Workdir:** the `open()`-resolved `workdir` (`--add-dir` cwd grant) is applied
  with `cmd.current_dir(dir)` when present (issue #2549).
- **Idle liveness (#2581):** the no-wall-clock, idle-window reaper
  (`SIMARD_MEETING_IDLE_LIVENESS_SECS`, default 3600s;
  `SIMARD_MEETING_TURN_TIMEOUT_SECS` alias) is unchanged. A productive turn of
  any length streams; only a genuinely silent child is reaped.
- **Noise stripping** (`strip_copilot_noise` / `line_is_noise`) and incremental
  `on_chunk` streaming are unchanged.
- **Tool grant** is unchanged — the transport change adds no privilege. The
  existing `--allow-all-tools --allow-all-paths` / `--allowedTools all` grant is
  **not** widened.

No new `Bridge` identifier is introduced, and no `println!`/`eprintln!` is added
— diagnostics stay on `tracing` (`info!` logs `prompt_len` only, never the prompt
bytes).

## Feeder / reaper interaction

The idle-liveness reaper can `SIGKILL` the agent's process group **mid-turn**
(e.g. a genuinely hung child). When it does, the child's stdin pipe is torn down
while the feeder may still be writing, so `feed` returns
`io::ErrorKind::BrokenPipe`. That is an expected consequence of the reap, **not**
a delivery failure, so it is tolerated: the turn already surfaces the honest
idle-timeout error from the read loop. Only a `BrokenPipe` from the feeder is
swallowed; every other feed error is surfaced. This ordering is deliberate — the
feeder is joined **after** the reader loop (and its kill path) so the reaper's
verdict wins and the feeder never turns a legitimate reap into a spurious hard
error.

## No silent fallback

If the prompt cannot be delivered argv-free the turn fails **loudly**; it never
falls back to inlining the prompt on `argv`:

- `attach_prompt_std` returning a `PromptDeliveryError` (a pre-spawn failure, not
  an errno) maps to `SimardError::AdapterInvocationFailed`.
- `cmd.spawn()` returning an `io::Error` (the E2BIG-class pre-exec failure) is
  first routed through `spawn_payload::record_spawn_failure(&err, "meeting-agent-proxy")`
  — which classifies the errno via
  [`classify_spawn_failure`](./terminal-failure-diagnosis-api.md) (E2BIG=7 →
  `ArgListTooLong`, ENOMEM=12 → `OutOfMemory`, …) and records it into the
  Overseer failure sink — and then returned as `AdapterInvocationFailed`. The
  failure is diagnosed at `error`, never `warn!`-swallowed.

## meeting_backend spawn-site audit

Every payload-bearing agent-spawn path reachable from a meeting/Signal session
was audited. The proxy is the only one that inlined a payload:

| Path | Payload-bearing spawn? | Disposition |
| --- | --- | --- |
| `agent_proxy.rs` `invoke_agent_streaming` | **yes** — was `-p <prompt>` on argv | **Converted** to stdin via `attach_prompt_std` (this doc). |
| `agent_proxy.rs` `validate_agent` (`which <cmd>`) | no — a fixed binary name only | Tier C (safe): constant-size argv. |
| `messaging.rs` `send_message` | no `Command` — builds the prompt, calls the proxy | Safe: delegates to the converted proxy. |
| `command.rs`, `lightweight.rs`, `mod.rs`, `sanitize.rs` | no `Command::new` / `.arg` / `-p` / `cat` / payload env | Safe: no spawn. |

There is no `sh -c`, no `-p "$(cat …)"`, and no large `env` payload anywhere
under `src/meeting_backend/`. The audit closes with the single conversion above.

## Tests

Hermetic tests in the `#[cfg(test)] mod tests` block of
`src/meeting_backend/agent_proxy.rs` pin the invariant. None require the network,
Signal, or a real `copilot`/`claude` binary: the argv-shape tests assert on the
`build_agent_command()` / `agent_base_args` seams without spawning, and the
round-trip tests drive `cat`/`sh -c` stand-ins via the private `agent_cmd` /
`agent_base_args` seam, mirroring the existing liveness tests.

- **T1 — argv-constant guard.** Asserts on the `build_agent_command()` seam
  directly, with **no** spawn: its argument vector — collected via
  `Command::get_args()` — equals the fixed `agent_base_args`, is constant and
  `< 4 KiB`, and is prompt-independent **by construction** (the prompt is not a
  builder parameter). A sentinel-prompt substring check confirms none of the
  prompt bytes appear in any argument. Because `attach_prompt_std` adds no `argv`
  tokens, this vector is exactly what `execve` receives — proving the prompt left
  `argv`.
- **T2 — stdin round-trip.** With `agent_cmd = "cat"` and empty base args, a
  `>= 256 KiB` prompt is echoed back on stdout in full — proving stdin transport,
  no `E2BIG`, and no truncation.
- **T3 — small-prompt happy path.** A `sh -c 'printf …'` stand-in returns the
  expected output with the prompt fed on stdin.
- **T4 — injection inertness.** A prompt containing shell metacharacters
  (a `$(id)` command substitution, backtick `whoami`, and a trailing `rm -rf /`)
  fed to `cat` round-trips byte-identically with
  no execution — there is no shell in the payload path.
- **T5 — liveness regression.** The existing sleep / `exec 1>&-` / `seq` /
  `printf` / no-tty liveness tests stay green: `sh -c` scripts ignore stdin, so
  moving the prompt off argv does not perturb them.
- **T6 — provider shape.** `resolve_agent_command` yields a bare `-p` in the
  `claude` args and **no** `-p` in the `copilot` args.
