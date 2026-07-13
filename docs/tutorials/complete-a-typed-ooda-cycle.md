---
title: Complete a typed OODA cycle
description: Acceptance tutorial for deterministic action and no-action cycles with durable typed outcomes.
last_updated: 2026-07-13
review_schedule: as-needed
owner: simard
doc_type: tutorial
status: implemented
related:
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../reference/ooda-capability-api.md
  - ../architecture/typed-ooda-loop.md
---

# Tutorial: Complete a typed OODA cycle

!!! warning "Acceptance-only actor"
    The deterministic fixture requires `SIMARD_TYPED_OODA_FIXTURE=1` and must be
    used only with an isolated state root. Production actors cannot select it.

This tutorial defines the deterministic end-to-end acceptance scenario for the
typed route. It uses a fixture actor rather than asking a model to choose an
action, so the expected outcomes are reproducible:

1. the fixture records one `SpawnEngineer` action;
2. a second fixture records one explicit no-action outcome.

Both outcomes are verified from the durable ledger, not from recipe prose or
logs.

## Prerequisites

- a candidate Simard build containing the typed route;
- an isolated temporary `SIMARD_HOME`;
- the deterministic `typed-ooda-fixture` actor enabled only in test mode;
- a fake engineer launcher that records requests without contacting a provider;
- `jq`;
- `SIMARD_TYPED_OODA_FIXTURE=1`.

The fixture must be rejected outside test mode.

## 1. Create isolated state

The final implementation must provide a test harness equivalent to:

```text
SIMARD_STATE_ROOT="$(mktemp -d)"
export SIMARD_STATE_ROOT SIMARD_TYPED_OODA_FIXTURE=1
```

## 2. Run the deterministic action fixture

```text
simard ooda fixture run \
  --state-root "$SIMARD_STATE_ROOT" \
  --scenario spawn-engineer \
  --request-id fixture-action-1
```

The fixture actor must make exactly one mutating tool call:
`record_action(SpawnEngineer)`. The handler commits the terminal, engineer
claim, and effect job atomically. The fake launcher then marks the effect
`succeeded`.

## 3. Verify the action

```text
simard ooda outcomes get --state-root "$SIMARD_STATE_ROOT" --request-id fixture-action-1 |
  jq -e '
    .outcome.kind == "action"
    and .outcome.payload.action.kind == "spawn_engineer"
    and .effect.state == "succeeded"
  '
```

Verify exactly one terminal and one launch request:

```text
simard ooda outcomes list --state-root "$SIMARD_STATE_ROOT" --limit 100 |
  jq -e '
    [.outcomes[] | select(.request_id == "fixture-action-1")] |
    length == 1
  '

```

## 4. Verify raw task fidelity

The fixture includes bytes containing a newline, NUL, ANSI escape, and
marker-like text. The acceptance test reads the stored blob through the ledger
library and compares its digest with the fixture digest. It must not rely on a
shell variable or UTF-8 conversion.

The same test submits a field one byte above
`max_semantic_payload_bytes`. The handler must return `payload_too_large` and
must not create a terminal, correction, effect, or truncated blob.

## 5. Run the deterministic no-action fixture

```text
simard ooda fixture run \
  --state-root "$SIMARD_STATE_ROOT" \
  --scenario no-action \
  --request-id fixture-no-action-1
```

This fixture makes exactly one call to `record_no_action`. It does not depend on
the previous cycle's model reasoning or claim state.

## 6. Verify no-action and cardinality

```text
simard ooda outcomes get --state-root "$SIMARD_STATE_ROOT" --request-id fixture-no-action-1 |
  jq -e '
    .outcome.kind == "no_action"
    and (.outcome.payload | has("admission") | not)
  '

simard ooda outcomes list --state-root "$SIMARD_STATE_ROOT" --limit 100 |
  jq -e '
    [.outcomes[] | .cycle_id]
    | group_by(.)
    | all(length == 1)
  '
```

The completed acceptance run proves deterministic action selection, no-action
selection, terminal cardinality, action-only admission, durable effect
execution, and byte fidelity. It does not prove production cutover readiness;
the migration gates in
[Typed-capability OODA architecture](../architecture/typed-ooda-loop.md#migration-and-route-rollback)
must also pass.
