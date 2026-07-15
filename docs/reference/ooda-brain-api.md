---
title: OODA actor API
description: Planned reference for recipe-composed OODA semantic steps and the typed terminal boundary.
last_updated: 2026-07-13
review_schedule: as-needed
owner: simard
doc_type: reference
status: planned
related:
  - ../architecture/typed-ooda-loop.md
  - ./ooda-capability-api.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# OODA actor API

!!! warning "Status: planned"
    This is the target actor contract, not a shipped API. Current releases still
    use the parser-based OODA brain interfaces until typed-route cutover.

The target OODA route has no Rust brain-response API. Observe, Orient, Decide,
and Act are recipe steps. Their semantic outputs are opaque byte sequences
passed directly to the next step.

The only Rust-facing decision boundary is the
[typed OODA capability API](./ooda-capability-api.md).

## Recipe actor

```text
goal-session-actor(
  session_context,
  goal_context,
  observe_output,
  orient_output,
  decide_output,
  capabilities
) -> process result
```

The process result is only success or failure. Business outcomes are not
returned in stdout, JSON, markers, or a Rust enum. Before returning success, the
actor invokes exactly one terminal capability:

- `record_action`;
- `record_no_action`;
- `record_blocked`;
- `record_completed`.

The runner rejects process success when no durable terminal exists for the
cycle.

## Context

The runner supplies:

| Input | Type | Handling |
| --- | --- | --- |
| `session_id` | typed identifier | Bound to the authenticated tool channel. |
| `cycle_id` | typed identifier | Bound to the active cycle. |
| `goal_id` | typed identifier | Validated against the goal store. |
| goal task/reason | opaque bytes | Passed byte-for-byte. |
| Observe output | opaque bytes | Passed byte-for-byte to Orient and later steps. |
| Orient output | opaque bytes | Passed byte-for-byte to Decide and Act. |
| Decide output | opaque bytes | Passed byte-for-byte to Act. |
| deterministic state | typed application data | Produced by typed readers, not extracted from prose. |
| capabilities | authenticated tool handles | Least-privilege, session-scoped. |

Large semantic values use owner-only context files. They never ride `argv` or
environment variables. Each capability still enforces a decoded byte limit and
rejects oversize values without truncation.

## Decision

There is no `EngineerLifecycleDecision`, `DecideJudgment`, or parsed response
enum on the target route. The actor expresses its decision by invoking one typed
capability.

The actor supplies the `request_id` in that call. The runner supplies and binds
the session, cycle, and goal context, but does not manufacture a terminal
request ID. A retried call for the same logical mutation must reuse the caller's
ID and exact arguments.

`record_action` contains the closed action union:

```text
SpawnEngineer | FileIssue | RequestMerge | RequestDeploy
```

Task, reason, and summary fields remain opaque bytes inside the typed request.
Rust validates shape, size, identity, permissions, admission, state transition,
and idempotency. It does not interpret semantic content.

## Side-effect handler

The capability handler is a deterministic brick:

```text
handle_terminal(
  authenticated_context,
  typed_request,
  policy,
  ledger,
  outbox
) -> Result<TerminalOutcome, CapabilityError>
```

It:

1. authenticates actor and session;
2. authorizes the capability and scope;
3. validates typed arguments;
4. applies fail-closed admission and safety checks for action requests;
5. enforces idempotency;
6. commits the immutable outcome, effect outbox job, and any claim atomically;
7. dispatches the effect from the durable outbox;
8. records the typed effect result and resumes expired jobs after a crash.

## Errors

Recipe, capability, authorization, admission, persistence, and downstream
failures propagate explicitly. No failure becomes a deterministic skip,
`AdvanceGoal`, progress update, no-action, or successful cycle.

The runner branches on typed tool error codes and process status. It never
branches on error text.

## Removed interfaces

The target goal-session route does not expose:

- an LLM submitter that returns prose to Rust;
- `parse_action_from_text`;
- first-word, marker, URL, decimal, or JSON extraction from agent output;
- `BrainResponseUnparseable`;
- parse-repair retries;
- deterministic semantic fallback brains;
- compatibility parsing after typed-path failure.

After cutover, any parser retained for a non-migrated or rollback route is
quarantined and unreachable from the typed goal-session recipe, capability
handler, and downstream engineer launch path. Before cutover, the current parser
documentation remains authoritative.

## Configuration

See [OODA capability API: Configuration](./ooda-capability-api.md#configuration).
There is no parser, response-contract, output-format, or fallback setting.

## See also

- [Typed-capability OODA architecture](../architecture/typed-ooda-loop.md)
- [OODA capability API](./ooda-capability-api.md)
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
