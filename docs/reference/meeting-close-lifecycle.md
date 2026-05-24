---
title: Meeting close lifecycle
description: The contract enforced when an operator runs `/close` in `simard meeting` — bounded timeouts, partial-handoff fallback, and the structured tracing emitted at each phase.
last_updated: 2026-05-19
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../operations/meeting-handoffs.md
  - ./meeting-backend-api.md
  - ./state-root-resolution.md
  - ../howto/recover-from-meeting-close-timeout.md
---

# Meeting close lifecycle

`simard meeting`'s `/close` command finalizes the live session, drains
in-flight agent work, writes the durable handoff bundle, and exits the
REPL. This page documents the timing contract, the partial-handoff
fallback, and the structured `tracing` signals that emerge during a
close.

> Before this contract existed, `/close` could block indefinitely
> (issue #1908) — typically because an agent stream never received
> EOF, or the cognitive-memory bridge subprocess died mid-flush. The
> bundle was silently dropped and the operator lost every decision and
> action item from the session. The new contract guarantees a
> deserialize-valid bundle on disk **even when timeouts fire**.

---

## End-to-end contract

When `MeetingBackend::close()` returns to the REPL, **all** of the
following are true regardless of agent health:

1. A `meeting_handoff.json` file exists at the resolved handoff path
   and deserializes against the current `MeetingHandoff` schema.
2. A companion `meeting_handoff.md` and `transcript.json` exist in the
   per-meeting bundle directory under the resolved state root.
3. The REPL printed the bundle paths to stdout.
4. `close()` returned `Ok(MeetingSummary)` — **never** `Err` on
   timeout. (The REPL surfaces a banner if the close was partial; see
   below.)
5. Total wall-clock time did not exceed the documented ceiling
   `SIMARD_MEETING_CLOSE_TIMEOUT_SECS + SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS + ~2s grace`
   (default `60 + 15 + 2 = 77s`, well under the **90s public
   ceiling**). See [Why two nested timeouts](#why-two-nested-timeouts)
   below for why the inner budget can extend past the master.

If **any** of (1)–(5) is violated, that is a bug; please file an
issue and link `journalctl --since "5 min ago" | grep meeting`.

---

## Timeout budgets

Three nested timeouts cooperate during a close. Each is observable in
tracing; each is independently overridable via env var for testing or
unusual deployments.

| Budget | Default | Env var | Scope |
|---|---|---|---|
| **Master close timeout** | 60s | `SIMARD_MEETING_CLOSE_TIMEOUT_SECS` | The entire `MeetingBackend::close()` call, end-to-end |
| **Agent close timeout** | 45s | `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS` | The inner `agent.close()` call (LLM/subprocess shutdown) |
| **Subprocess grace** | 2s | (not configurable) | SIGTERM → SIGKILL grace for any spawned bridge child |

### Clamping and validation

| Env var | Range | Behavior outside range |
|---|---|---|
| `SIMARD_MEETING_CLOSE_TIMEOUT_SECS` | `[1, 600]` | Clamped silently; malformed value triggers WARN and falls back to 60 |
| `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS` | `[1, 120]` | Clamped silently; malformed value triggers WARN and falls back to 45 |

Boot never fails on a malformed close-timeout env. The WARN line uses
the static field name `reason="clamped"` or `reason="malformed"` for
machine parsing.

### Why two nested timeouts

The inner `agent.close()` budget exists so that a single slow
subprocess shutdown cannot consume the whole master budget. The
master loop wraps every phase in `close_guard::with_timeout(...)`,
so even if `agent.close()` exceeds its inner budget, the master
loop still has the remainder of its 60s budget to flush the
transcript and write the partial handoff.

The inner budget is **not** a hard kill. `agent.close()` runs on
its own worker thread (`std::thread::scope` + `mpsc::recv_timeout`);
when the inner timeout fires, the master records the phase as
`timeout` and proceeds, but the worker thread continues to drain.
This is deliberate: killing the worker mid-flight could poison the
agent's internal mutex and corrupt later sessions in the same
process. The worst-case wall-clock ceiling is therefore
`master (60s) + agent inner draining (up to 15s after master fired)
+ subprocess SIGTERM→SIGKILL grace (~2s) = 77s`, under the **90s
public contract ceiling**.

#### Worker-thread reaping (accepted residual)

If the inner function under `with_timeout` never returns (e.g., the
subprocess deadlocks completely), the worker thread is bounded only
by its inner budget. In the pathological case where even the inner
budget cannot reclaim the thread (the subprocess holds a
non-interruptible syscall), the worker is reaped on process exit.
This is an **accepted residual (R-1)** for the CLI scope; the master
still returns to the REPL within the 77s ceiling regardless of the
worker's fate.

#### Implementation note: tokio migration path

The current implementation uses `std::thread::scope` +
`mpsc::recv_timeout` for the `with_timeout` primitive because
`MeetingBackend::close()` is synchronous and the CLI binary does
not assume a Tokio runtime. The `close_guard` module is structured
so the primitive can be swapped for `tokio::time::timeout` if the
backend is later made `async` — the signature
`with_timeout<T>(budget: Duration, work: impl FnOnce() -> T) -> Result<T, Timeout>`
is the migration boundary.

---

## Close pipeline

`MeetingBackend::close()` executes the following phases. Each phase is
individually wrapped by a `close_guard::with_timeout(...)` call so a
hang in one phase cannot consume the others' budgets.

```
                 ┌──────────────────────────────────────┐
                 │     MeetingBackend::close()          │
                 │     master with_timeout(60s)         │
                 └───────────────────┬──────────────────┘
                                     │
        ┌────────────────────────────┼────────────────────────────┐
        ▼                            ▼                            ▼
┌───────────────┐         ┌─────────────────────┐       ┌───────────────────┐
│ agent.close() │         │ generate_summary()  │       │ store_enriched_   │
│  inner(15s)   │         │  inner(15s)         │       │ cognitive_memory  │
│  SIGTERM/KILL │         │  best-effort        │       │  inner(10s)       │
└───────────────┘         └─────────────────────┘       └───────────────────┘
        │                            │                            │
        └────────────────────────────┴────────────────────────────┘
                                     │
                                     ▼
                  ┌─────────────────────────────────────┐
                  │   persist_handoff(state_root)       │
                  │   single-shot fs::write per file    │
                  │   (atomic-rename hardening tracked  │
                  │    separately — see follow-up)      │
                  └─────────────────────────────────────┘
```

Each phase has three outcomes:

- **Ok(v)** — the phase completed; its data is folded into the handoff.
- **Timeout** — the inner budget expired; the phase contributes
  best-known partial data and the close continues.
- **Err(e)** — the phase returned an explicit error; the error is
  recorded as a `PartialReason` and the close continues.

> The close pipeline **never short-circuits on a phase failure**.
> Every successful or failed phase still flows into the partial-handoff
> writer so the operator's intent is captured.

---

## Partial-handoff envelope

When the close pipeline hits any timeout or phase failure, the writer
emits a **partial** handoff. The on-disk schema is **identical** to a
normal handoff — there are no new required fields — so all existing
consumers (OODA daemon, engineer-loop, `simard act-on-decisions`,
external dashboards) parse it without modification.

### Distinguishing partial from full

A partial handoff is distinguishable by the **runtime signal**
emitted at close time, not by a new schema field:

```
WARN handoff_partial=true reason=close_timeout meeting_id=<id>
```

The full set of `reason` values is a closed enum
(`PartialReason`); see the [Tracing](#tracing-fields) section for the
complete list and parsing notes.

### Field semantics for a partial handoff

| Field | Full close | Partial close |
|---|---|---|
| `topic` | as set | as set |
| `started_at` | exact | exact |
| `closed_at` | exact | exact (wall-clock at timeout fire) |
| `decisions` | extracted by summarizer | `[]` if summarizer timed out before any output; otherwise whatever the summarizer emitted |
| `action_items` | extracted by summarizer | `[]` if summarizer timed out; otherwise whatever was emitted |
| `open_questions` | extracted by summarizer | `[]` on summarizer timeout; otherwise emitted set |
| `transcript` | full live buffer | full live buffer (the live buffer is in-memory and unaffected by agent timeouts) |
| `participants` | full | full |
| `themes` | recorded themes | recorded themes |
| `processed` | `false` | `false` |
| `duration_secs` | exact | exact (wall-clock at timeout fire) |
| `transcript_path` | `Some(<state-root>/meetings/<id>/transcript.json)` | same as full — the transcript is always written, even when summarizer/agent phases time out |
| `bundle_dir` | `Some(<state-root>/meetings/<id>/)` | same as full — the bundle directory and the three files inside it are always written |

The companion `meeting_handoff.md` summary string falls back to:

```
(partial — close timed out at 60s; full summary unavailable)
```

`MeetingSummary.summary_text` returned to the REPL contains the same
fallback string. The `transcript_path` and `bundle_dir` fields are
`Some(_)` on a partial close (the artifacts are on disk), and the
REPL surfaces the partial-close banner described under
[REPL exit banner](#repl-exit-banner) below.

### Write semantics

All three persisted files (`meeting_handoff.json`,
`meeting_handoff.md`, `transcript.json`) are written via a single
`std::fs::write` per file after the parent directory has been
created with `fs::create_dir_all`. On Linux ext4/xfs each file is
written as one syscall, which is durable against typical user
interruption (`/close` exiting normally, REPL exit, agent crash).

> **Known limitation, tracked separately.** This is **not** crash-safe:
> if the host loses power mid-write, a half-written JSON document is
> possible. Atomic tmp-file-plus-fsync-plus-rename hardening is on the
> roadmap; the contract above (deserialize-valid bundle on disk after
> a timeout) holds for in-process timeouts, which is the failure mode
> issue #1908 targets.

---

## Summary text sanitization

The LLM-derived summary text is bounded before persistence to prevent
a runaway model from filling disk:

| Limit | Value |
|---|---|
| Max summary bytes (in `meeting_handoff.md` and any in-JSON summary field) | 1 MiB |
| Truncation marker | `\n[truncated: N bytes omitted]` appended after a UTF-8-safe boundary cut |

The 1 MiB cap is applied to summary-style fields only; the
`transcript` array is not capped because every entry is already a
bounded REPL turn.

---

## Tracing fields

The close pipeline emits structured `tracing` events with a stable
field allowlist. Field values come from a closed `PartialReason` enum
to prevent LLM/subprocess output from leaking into log lines.

### `PartialReason` values

| Reason (wire) | Meaning |
|---|---|
| `close_timeout` | The master 60s budget expired |
| `agent_close_timeout` | The inner `agent.close()` exceeded 45s |
| `bridge_timeout` | The cognitive-memory bridge `store_enriched_*` exceeded its inner budget |
| `summary_empty` | The summarizer returned but produced no decisions/actions/questions |
| `persistence_error` | An IO error occurred while persisting (e.g. `EACCES`, `ENOSPC`, parent `state_root` unwritable). The full-fidelity handoff is unavailable; the partial-handoff branch retried with the legacy `meeting_handoffs/handoff-<ts>.json` writer when possible |

> **Casing.** The Rust type is the PascalCase enum
> `PartialReason::CloseTimeout` (etc.); its `Display` impl emits
> snake_case, which is the value operators see in tracing fields,
> the REPL banner, and any log-scraping tooling. Parse against the
> snake_case wire values above, not the Rust enum names.

### Event shapes

```
INFO  meeting.close.start meeting_id=<id> topic="<topic>" budget_secs=60
DEBUG meeting.close.phase phase=agent_close ms=812 outcome=ok
WARN  meeting.close.phase phase=summary ms=15001 outcome=timeout
WARN  handoff_partial=true reason=summary_empty meeting_id=<id> wrote=meeting_handoff.json
INFO  meeting.close.done meeting_id=<id> partial=true total_ms=15904 \
      bundle_dir=/home/azureuser/.simard/meetings/2026-05-19T...-topic/
```

Operators monitoring at scale should filter on both targets:
`target=simard::meeting_backend::closing` for the phase/lifecycle
events (`meeting.close.start`, `meeting.close.phase`,
`meeting.close.done`), and `target=close_guard` for the underlying
`with_timeout` WARN that fires when an inner budget expires. Key
off `handoff_partial=true` plus `reason=<PartialReason>` for
alerting.

---

## REPL exit banner

After `/close` returns, the REPL prints a bundle-paths block
followed (on a partial close) by a single-line warning so the
operator can react before leaving the terminal. The full close
case prints only the paths:

```
[meeting] handoff written: /home/azureuser/.simard/meeting_handoffs/meeting_handoff.json
[meeting] bundle:          /home/azureuser/.simard/meetings/2026-05-19T17-31-42Z-daily-backup-policy/
```

On a partial close, an additional `WARNING:` line is appended that
**always begins with the literal prefix** `[meeting] WARNING:
partial close (reason=<wire>)` so log scrapers and the
[recovery how-to](../howto/recover-from-meeting-close-timeout.md)
can match it deterministically:

```
[meeting] handoff written: /home/azureuser/.simard/meeting_handoffs/meeting_handoff.json
[meeting] bundle:          /home/azureuser/.simard/meetings/2026-05-19T17-31-42Z-daily-backup-policy/
[meeting] WARNING: partial close (reason=summary_empty). Review the bundle
          before relying on extracted decisions/action items.
```

### Orphan-turn banner

When one or more `send_message` calls failed after the user message
was already pushed to history (creating turns with no assistant
reply), the exit banner appends an orphan-turn warning:

```
[meeting] WARNING: 2 orphan turns have no assistant reply (backend errors during conversation). Transcript may be incomplete.
```

The count comes from `MeetingSummary.orphan_turn_count` (issue #1983).

### Backend error banner (inline, during conversation)

During the live REPL loop, backend errors render as a structured
banner instead of a bare `[agent error: …]` line. The stable
marker is `[meeting:error]`, greppable from terminal scrollback:

```
[meeting:error] WARNING: backend error (source=conversation, severity=transient) — simulated transient LLM failure
  ↳ meeting is still usable — retry your message or /close to end
```

| Field | Values |
|---|---|
| `source` | `conversation`, `template` |
| `severity` | `transient` (default — most LLM errors), `permanent` (adapter closed, empty response) |

The `<wire>` value comes from the `PartialReason` `Display` impl
(see [Casing](#partialreason-values) above).

---

## Eliminated boot WARN

Booting a fresh meeting no longer emits the
`session is already open` WARN (issue #1905). The previous double-open
came from `MeetingBackend::new_session()` calling `agent.open()` after
the caller had already opened the session via
`open_meeting_agent_session()`. The redundant call was removed; the
contract is now:

> **Backend contract**: callers open the agent session and hand the
> opened session to the backend. `MeetingBackend` never re-opens.

This is a behavior fix only; no public API changed.

---

## Configuration summary

| Env var | Default | Purpose | Section |
|---|---|---|---|
| `SIMARD_STATE_ROOT` | `~/.simard` | Root for all durable artifacts | [State-root resolution](./state-root-resolution.md) |
| `SIMARD_HANDOFF_DIR` | `<state-root>/meeting_handoffs` | Narrow override for the handoff drop directory | [State-root resolution](./state-root-resolution.md#environment-variables) |
| `SIMARD_MEETINGS_DIR` | `<state-root>/meetings` | Narrow override for per-meeting bundle root | [State-root resolution](./state-root-resolution.md#environment-variables) |
| `SIMARD_MEETING_CLOSE_TIMEOUT_SECS` | `60` | Master close budget, clamp `[1, 600]` | [Timeout budgets](#timeout-budgets) |
| `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS` | `45` | Inner agent-close budget, clamp `[1, 120]` | [Timeout budgets](#timeout-budgets) |

---

## See also

- [State-root resolution](./state-root-resolution.md) — where the
  bundle and handoff actually land.
- [Meeting REPL & handoff ingestion](../operations/meeting-handoffs.md)
  — operator workflow including the new partial-close banner.
- [Recover from a meeting close timeout](../howto/recover-from-meeting-close-timeout.md)
  — playbook when `handoff_partial=true` shows up in tracing.
- [Meeting backend API reference](./meeting-backend-api.md) — Rust
  types and method signatures.
