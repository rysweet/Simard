---
title: "The Signal-meeting E2BIG incident — the meeting agent proxy was its own argv spawn site"
description: >
  Why an inbound Signal message ("Status?") failed INSTANTLY (elapsed_ms=0) with
  "Argument list too long" even after the e2bigsweep fix: the Signal channel routes
  each message through a meeting session whose PersistentAgentProxy
  (src/meeting_backend/agent_proxy.rs) spawns copilot/claude directly and inlined
  the whole turn prompt as a `-p <prompt>` argv token — a DISTINCT spawn site from
  the base_type_copilot facade and OODA decision-cycle launch fixed earlier. Covers
  the live symptom, why the earlier fixes did not cover this path, the stdin
  transport fix, the no-silent-fallback wiring, and the boundary against
  reintroducing argv inlining (#2640).
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/argv-free-meeting-agent-proxy.md
  - ../howto/diagnose-signal-meeting-e2big.md
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/large-payload-spawn-api.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ./e2big-elimination.md
  - ./self-diagnose-on-step-error.md
  - ./journal-recipe-spawn-e2big.md
  - ../howto/set-up-the-signal-channel.md
  - ../../src/meeting_backend/agent_proxy.rs
  - ../../src/meeting_backend/messaging.rs
  - ../../src/spawn_payload/mod.rs
---

# The Signal-meeting E2BIG incident — the meeting agent proxy was its own argv spawn site

> **Status: implemented.** The fix lives in
> [`src/meeting_backend/agent_proxy.rs`](https://github.com/rysweet/Simard/blob/main/src/meeting_backend/agent_proxy.rs)
> (`PersistentAgentProxy::invoke_agent_streaming` + `resolve_agent_command`),
> reusing the shared
> [`simard::spawn_payload`](../reference/large-payload-spawn-api.md) stdin
> transport. Closes the meeting/Signal residual of
> [#2640](https://github.com/rysweet/Simard/issues/2640). For the wire-level
> contract read the
> [argv-free meeting/Signal agent-proxy reference](../reference/argv-free-meeting-agent-proxy.md);
> for a runbook read
> [Diagnose Signal-meeting E2BIG](../howto/diagnose-signal-meeting-e2big.md).

This is the narrative for a **live** production failure on Simard's Signal
channel and the fix that resolved it. It is a sibling of, but distinct from, the
[copilot/OODA argv-free incident (#2640)](./self-diagnose-on-step-error.md) and
the [journal recipe-spawn incident (#2692)](./journal-recipe-spawn-e2big.md): the
same `E2BIG` root cause, a **different, un-migrated spawn site**.

## The incident

A user sent a one-word Signal message — `"Status?"` — and it failed
**instantly**. The daemon logged:

```
send_message{input_len=7}: simard::meeting_backend::messaging:
  LLM agent returned error
  … elapsed_ms=0 … Argument list too long
```

(The `send_message` span records `input_len` — the message length — **not** the
message text: `#[tracing::instrument(skip(self), fields(input_len = …))]`. The
raw message bytes are never logged, matching the "length, never bytes" privacy
stance below. `input_len=7` is the seven bytes of `"Status?"`.)

Two details make this diagnostic:

- **`elapsed_ms=0`.** The failure is **pre-exec**. The agent process never ran;
  `Command::spawn()` returned an `io::Error` before `execve` handed control to
  `copilot`. A per-turn timeout, a hung agent, or a bad response would all take
  time and produce output — this took none.
- **"Argument list too long"** is **`E2BIG`** (`errno 7`): `argv` + `envp`
  exceeded `ARG_MAX`.

`"Status?"` is three bytes — the message itself is nowhere near `ARG_MAX`. What
overflowed was the **whole turn prompt**: `MeetingBackend::send_message` builds a
`BaseTypeTurnInput` from the identity/system prompt, the full conversation
preamble (all prior turns), and the user message, then hands that combined text
to the agent proxy. On an established Signal meeting session that combined prompt
was large enough that inlining it as a single `argv` token crossed the
per-argument limit — so **every** message routed through that session failed
identically, `"Status?"` included.

## Why the earlier fixes did not cover this path

The [e2bigsweep / #2640 fix](./e2big-elimination.md) moved the
`base_type_copilot` spawn facade and the OODA decision-cycle launch off `argv`
onto stdin. It did **not** touch the meeting backend's
`PersistentAgentProxy`, because the proxy is a **separate spawn site** in a
separate module:

- The Signal channel opens a meeting session
  (`open{provider=Copilot tag=signal}`) and every inbound message flows through
  `MeetingBackend::send_message` → `PersistentAgentProxy::invoke_agent_streaming`.
- That proxy is the "thin proxy" that replaced the 30–90s PTY path (issue #2179).
  It builds its own `std::process::Command` and, before this fix, called
  `cmd.arg("-p").arg(prompt)` — inlining the entire turn prompt as one `argv`
  element.
- It shares **no** code with the base_type_copilot facade, so fixing that facade
  left this launch site untouched. It was a fourth, independent argv-inlining
  spawn site hiding behind the same `E2BIG` symptom.

This is the recurring shape of the `E2BIG` class: it is eliminated one launch
site at a time, and any launch site that builds its own `argv` by hand is a fresh
opportunity to reintroduce it. See
[Comprehensive E2BIG elimination](./e2big-elimination.md) for why the answer is a
single payload invariant enforced everywhere, not a per-symptom patch.

## The fix — pipe the prompt, don't argv it

The proxy now delivers the prompt on **stdin** through the shared facade, exactly
as the base_type_copilot meeting/Signal sites already do:

- `invoke_agent_streaming` drops `.arg("-p").arg(prompt)` and the old
  `.stdin(Stdio::null())`, and calls
  `spawn_payload::attach_prompt_std(&mut cmd, prompt.as_bytes())` before
  `spawn()`. `copilot` reads its prompt from a non-TTY stdin when `-p` is absent.
- The prompt is written on a **feeder thread** after spawn
  (`applied.feed(child.stdin.take())`) so a large prompt cannot deadlock against
  the child filling stdout.
- `resolve_agent_command` keeps the `Copilot` arm `-p`-free and gives the
  `RustyClawd`/`claude` arm a **bare** `-p` (print mode, prompt piped).
- Everything else is preserved: `process_group(0)`, the idle-liveness reaper
  (#2581), the workdir grant (#2549), noise stripping, and incremental streaming.

The turn prompt — an arbitrarily large, attacker-controlled Signal message plus
preamble — now travels through a pipe; only fixed-size flags ride on `argv`, so
`ARG_MAX` can never be reached again. The wire-level details are in the
[argv-free meeting/Signal agent-proxy reference](../reference/argv-free-meeting-agent-proxy.md).

## No silent fallback

A failure to deliver the prompt argv-free is surfaced, never worked around:

- A pre-exec spawn `io::Error` is routed through
  `spawn_payload::record_spawn_failure(&err, "meeting-agent-proxy")` — which
  errno-classifies it (`E2BIG` → `ArgListTooLong`) into the Overseer failure sink
  — and then returned as `SimardError::AdapterInvocationFailed`. It is diagnosed
  at `error`, not `warn!`-swallowed, honouring the
  [self-diagnose-on-step-error](./self-diagnose-on-step-error.md) principle: ask
  *why* it failed, don't just log it.
- The proxy **never** re-inlines the prompt onto `argv` as a fallback. A future
  regression that did would be caught by the argv-constant test, not tolerated.

## Trust boundary

The turn prompt is **untrusted**: it contains an unauthenticated inbound Signal
message of arbitrary bytes. stdin is the security-correct transport for it —

- **No command injection.** There is no shell and no `argv` expansion in the
  payload path, so shell metacharacters in a message are inert (the message is
  passed byte-exact to `cat`/the agent's stdin).
- **No argv disclosure.** The message never appears in `/proc/<pid>/cmdline` or
  `ps` output.
- **No data-at-rest.** stdin is in-memory; the message is not written to a temp
  file.

The transport change adds **no** privilege — the agent's existing tool grant is
unchanged. Constraining the externally-fed meeting agent's tool access further is
a separate, tracked concern, not part of this transport fix.

## The boundary this draws

> The meeting/Signal agent proxy MUST deliver the turn prompt on stdin via
> `spawn_payload::attach_prompt_std`. It MUST NOT reintroduce `-p <prompt>`,
> `sh -c`, `-p "$(cat …)"`, or any other argv/env inlining of the prompt.

Reintroducing any of those recreates this exact instant-`E2BIG` failure on every
Signal message. The hermetic argv-constant and stdin-round-trip tests (see the
[reference](../reference/argv-free-meeting-agent-proxy.md#tests)) exist to fail
that regression at build time rather than in production.
