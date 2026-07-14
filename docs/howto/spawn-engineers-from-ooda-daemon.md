---
title: How OODA spawns engineer agents
description: Operator guide for the typed goal-session actor that starts engineers without parsing agent prose.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ./run-ooda-daemon.md
  - ../architecture/typed-ooda-loop.md
  - ../reference/ooda-capability-api.md
  - ../tutorials/complete-a-typed-ooda-cycle.md
---

# How OODA spawns engineer agents

!!! info "Status"
    Production goal-session Act uses scoped typed tools and durable outcomes.
    Legacy marker parsing is test-only and has no production call edge.

The goal-session recipe composes semantic reasoning and gives its final actor a
scoped `record_action` tool. To start an engineer, the actor calls
`record_action` with the `SpawnEngineer` variant. Rust validates and admits the
typed request, commits the terminal, claim, and durable effect job, and then
launches the engineer through the outbox dispatcher.

No Rust component reads recipe or agent prose to decide whether to spawn.

## Prerequisites

- Simard installed through `simard install`;
- a provider configured in `$SIMARD_HOME/config.toml`;
- the goal-session recipe and capability policy installed under
  `$SIMARD_HOME/prompt_assets`;
- an enabled engineer base type;
- user-level `simard-ooda.service` for live operation.

The capability policy grants `record_action.spawn_engineer` to the authenticated
goal-session actor for governed repositories. It does not grant direct merge or
deployment.

There is no typed-to-parser fallback after a typed cycle starts.

## Add an actionable goal

```text
simard goal add 1 \
  "Implement issue #4052 and finish when the typed OODA path is verified" \
  --repo rysweet/Simard
```

The goal text remains semantic input. Rust does not extract the issue number or
infer an action from it.

## Run a bounded cycle

Stop the service to avoid concurrent cycles, then run one foreground cycle:

```text
systemctl --user stop simard-ooda.service
"$SIMARD_HOME/bin/simard" ooda run --cycles=1 "$SIMARD_HOME"
```

The recipe performs:

1. Observe gathers typed state and semantic context.
2. Orient receives Observe output unchanged.
3. Decide receives Orient output unchanged.
4. The goal-session actor receives Decide output unchanged.
5. The actor invokes exactly one terminal capability.
6. The runner verifies one durable terminal before reporting success.

For an engineer action, `record_action` carries a typed repository, base type,
permissions, claim, and byte-preserved task. The handler:

1. binds the authenticated actor and session;
2. loads the server-bound cycle and goal and rejects any target mismatch;
3. validates a safe, non-empty request ID in the terminal ledger;
4. validates the closed base-type enum;
5. authorizes the requested permission subset against policy and the exact
   bound repository;
6. applies concurrency, disk, and active-claim admission;
7. commits the terminal, engineer claim, and outbox job
   atomically;
8. leases and launches the effect from the outbox;
9. records completion or retry by effect ID while the job is running.

The engineer receives scoped Copilot adapters. `process_exec` enables the shell
adapter; it is not a transactionally capped process broker. Typed launches do
not add `--allow-all-tools`, `--allow-all-paths`, or `COPILOT_ALLOW_ALL`.

## Verify the durable result

List recent terminals:

```text
simard ooda outcomes list --state-root "$SIMARD_STATE_ROOT" --limit 10 |
  jq '.outcomes[] |
    {
      request_id,
      cycle_id,
      goal_id,
      kind,
      action: .payload.action.kind,
      admission: .payload.admission
    }'
```

A successful spawn has:

```json
{
  "kind": "action",
  "payload": {
    "action": {
      "kind": "spawn_engineer"
    },
    "admission": {
      "policy_revision": "goal-session-policy-v1"
    }
  },
  "effect": {
    "state": "succeeded"
  }
}
```

Confirm the linked engineer:

```text
simard engineer list --json |
  jq '.engineers[] | {session_id, goal_id, claim_key, state}'
```

Recipe stdout and logs can explain the reasoning, but they are not evidence that
an engineer was started. The ledger record and linked engineer record are.

## Interpret other terminals

The actor may instead invoke:

| Tool | Meaning |
| --- | --- |
| `record_no_action` | No machine action is useful in this cycle. |
| `record_blocked` | The goal cannot proceed until a typed blocker changes. |
| `record_completed` | The semantic done condition is met and required typed evidence passed. |

These are explicit actor decisions, not substitutes for a failed spawn.

## Enable live operation after cutover

```text
systemctl --user start simard-ooda.service
systemctl --user status simard-ooda.service --no-pager
```

## Troubleshooting

### The cycle reports `missing_terminal`

The actor returned without invoking a terminal capability. The cycle failed.
Fix the recipe or actor prompt. Do not add an output parser or convert the result
to no-action.

### The tool reports `permission_denied`

Inspect the installed capability policy and authenticated session identity.
Grant only the missing repository/capability scope, then start a new cycle with
a new stable request ID.

### The effect reports an unsupported base type

Use `copilot`. The wire enum also accepts `rusty_clawd`, but the live typed
effect executor currently supports only Copilot.

### Admission rejects a spawn

Admission rejection fails the action attempt and does not create a terminal.
Inspect the cycle error for disk, concurrency, or active-claim rejection. It
does not silently become no-action.

### The recipe or tool fails

Inspect the daemon error and, if a terminal committed, fetch it with `simard
ooda outcomes get`. There is no `ooda executions list` command.

### The action was replayed

An identical terminal request ID and fingerprint return the existing outcome.
Different terminal arguments with the same ID fail with an idempotency conflict.
Generate a new request ID only for a genuinely new action attempt.

## See also

- [Typed-capability OODA architecture](../architecture/typed-ooda-loop.md)
- [OODA capability API](../reference/ooda-capability-api.md)
- [Tutorial: complete a typed OODA cycle](../tutorials/complete-a-typed-ooda-cycle.md)
- [Deploy and roll back typed OODA](../operations/deploy-and-roll-back-typed-ooda.md)
