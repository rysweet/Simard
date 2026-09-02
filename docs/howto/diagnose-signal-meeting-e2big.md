---
title: Diagnose Signal-meeting E2BIG spawn failures
description: >
  Operator runbook for the live "Signal message fails instantly" symptom: an
  inbound Signal message returns nothing and the daemon logs
  send_message{...} … elapsed_ms=0 … "Argument list too long" (E2BIG, errno 7).
  Confirm the pre-exec spawn E2BIG at the meeting PersistentAgentProxy, verify the
  stdin-transport fix is deployed (no `-p <prompt>` on argv, prompt on stdin), read
  the recorded overseer.diagnosis ArgListTooLong cause, and confirm an arbitrarily
  large Signal message now reaches the meeting agent (#2640).
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../reference/argv-free-meeting-agent-proxy.md
  - ../concepts/signal-meeting-agent-e2big.md
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
  - ../howto/diagnose-journal-e2big-spawn-failures.md
  - ../howto/set-up-the-signal-channel.md
  - ../howto/start-a-meeting.md
---

# Diagnose Signal-meeting E2BIG spawn failures

Use this runbook when a Signal message to Simard **returns nothing** (or an
error) and the daemon log shows an **instant** failure with "Argument list too
long" — the symptom of the
[Signal-meeting E2BIG incident](../concepts/signal-meeting-agent-e2big.md)
(#2640). For the wire-level contract, see the
[argv-free meeting/Signal agent-proxy reference](../reference/argv-free-meeting-agent-proxy.md).

## Symptom

- You send a Signal message (even a tiny one like `Status?`) and get no reply, or
  an error reply, from Simard.
- The daemon log emits, **with `elapsed_ms=0`**:

  ```
  send_message{input_len=7}: simard::meeting_backend::messaging:
    LLM agent returned error … elapsed_ms=0 … Argument list too long
  ```

  The `send_message` span records `input_len` (the message **length**), not the
  message text — so you will not see the message content in the log.

  `Argument list too long` = **`E2BIG`** (`errno 7`). `elapsed_ms=0` means the
  agent process never even started — a **pre-exec** spawn failure, not a timeout
  or a bad response.
- It tends to worsen as a Signal meeting session accumulates history: the
  per-turn prompt (system prompt + full preamble + message) grows until it
  crosses `ARG_MAX`, after which **every** message on that session fails.

## 1. Confirm the pre-exec spawn E2BIG

The two tells that pin this as the meeting-proxy argv overflow (and not, say, a
hung agent or a downstream tool error):

1. **`elapsed_ms=0`** on the `send_message` span — the spawn failed before the
   child ran.
2. The error text contains **`Argument list too long`** / `os error 7`, and the
   span target is `simard::meeting_backend::messaging` /
   `persistent-agent-proxy`.

If instead `elapsed_ms` is large, or the error is an idle/liveness timeout, this
is **not** the argv-overflow bug — see
[Diagnose and recover OODA step failures](../howto/diagnose-and-recover-ooda-step-failures.md)
for the liveness-reaper path (#2581).

## 2. Read the recorded diagnosis

The fix records every pre-exec spawn failure into the Overseer failure sink with
the site tag `meeting-agent-proxy`. Look for the structured cause rather than the
raw string:

```
overseer.diagnosis … cause=ArgListTooLong evidence=[meeting-agent-proxy] …
```

`ArgListTooLong` is the errno-keyed classification of `E2BIG` from
[`classify_spawn_failure`](../reference/terminal-failure-diagnosis-api.md). Its
presence confirms the failure was diagnosed and surfaced — **not**
`warn!`-swallowed or silently fallen back.

> **Two names, one component.** Grep `persistent-agent-proxy` for the returned
> error and its `send_message` span; grep `meeting-agent-proxy` for the recorded
> spawn diagnosis (`evidence=[meeting-agent-proxy]`). Both name the same meeting
> `PersistentAgentProxy` — see
> [Naming: two ids, one component](../reference/argv-free-meeting-agent-proxy.md#naming-two-ids-one-component).

If you see the raw `Argument list too long` in the log but **no**
`ArgListTooLong` diagnosis with the `meeting-agent-proxy` tag, the running binary
predates the fix — jump to step 4.

## 3. Verify the deployed transport is stdin, not argv

On the deployed binary's source/commit, confirm the meeting proxy delivers the
prompt on stdin:

- `src/meeting_backend/agent_proxy.rs` `invoke_agent_streaming` calls
  `spawn_payload::attach_prompt_std(&mut cmd, prompt.as_bytes())` and does **not**
  call `.arg("-p").arg(prompt)` or `.stdin(Stdio::null())`.
- `resolve_agent_command` returns **no** `-p` in the `copilot` args and a **bare**
  `-p` in the `claude` args.

A quick grep on the source tree that must come back **empty**:

```bash
# must find NOTHING — a match means the argv-inlining regression is back
rg -n 'arg\("-p"\)\.arg\(' src/meeting_backend/agent_proxy.rs
```

And the guard tests must be green:

```bash
cargo test -p simard meeting_backend::agent_proxy
```

The argv-constant (T1) and stdin-round-trip (T2) tests fail the build if the
prompt is ever put back on `argv`.

## 4. If the fix is not deployed

If step 2 shows raw `E2BIG` with no `ArgListTooLong` diagnosis, or step 3 finds
`-p <prompt>` argv inlining, the running daemon predates the fix. Rebuild from a
commit that includes it and redeploy (see
[Verify and roll back a self-deploy](./verify-and-roll-back-a-self-deploy.md)).
Do **not** work around it by trimming meeting history — the transport, not the
prompt size, is the defect.

## 5. Confirm recovery — live

The acceptance check is that an **arbitrarily large** Signal message reaches the
meeting agent without `E2BIG`:

1. From an allowlisted Signal sender (see
   [Set up the Signal channel](./set-up-the-signal-channel.md)), send `Status?`
   on the affected session. You should get a normal reply.
2. Send a deliberately **large** message — one comfortably **above the old
   per-argument `ARG_MAX` limit (~128 KiB)**, e.g. paste `>= 256 KiB` of text
   (the same threshold the T2 stdin-round-trip test uses). A "few KB" message is
   **not** a valid check: it would have fit under the old per-argument limit and
   succeeded even before the fix. The large message must reach the agent and
   return a reply — no instant failure, no `Argument list too long`.
3. In the daemon log, the `send_message` span now shows a **non-zero**
   `elapsed_ms` and `Invoking agent (streaming)` with a `prompt_len` reflecting
   the full prompt — proving the agent actually ran with the prompt delivered on
   stdin.

If all three hold, the Signal channel no longer `E2BIG`s on messages routed
through the meeting agent.

## Related

- [Argv-free meeting/Signal agent-proxy reference](../reference/argv-free-meeting-agent-proxy.md)
  — the wire-level stdin contract and tests.
- [The Signal-meeting E2BIG incident](../concepts/signal-meeting-agent-e2big.md)
  — why this spawn site was distinct from the earlier #2640 fixes.
- [Diagnose journal E2BIG spawn failures](./diagnose-journal-e2big-spawn-failures.md)
  — the sibling recipe-spawn variant of the same `E2BIG` class.
