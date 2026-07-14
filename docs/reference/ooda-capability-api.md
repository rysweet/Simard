---
title: OODA capability API
description: Implemented typed terminal, authorization, idempotency, actor-session, and effect-outbox contracts for parser-free goal-session execution.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../architecture/typed-ooda-loop.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../operations/deploy-and-roll-back-typed-ooda.md
---

# OODA capability API

The goal-session actor receives typed tools rather than a response schema that
Rust parses. Tool arguments are decoded as deterministic application data;
agent prose is never inspected to select an action.

## Actor session

Before launching the recipe, Simard stores a 30-minute actor-session lease
bound to:

- actor identity;
- session, cycle, and goal IDs;
- one repository;
- capability grants;
- observe-only state.

The random token is passed through an owner-private context file. `ooda
actor-run` must present that token and the exact session, cycle, and goal.
Expired or mismatched sessions return `Unauthenticated`.

The terminal actor's tool schemas do not accept caller-supplied session, cycle,
goal, repository authority, or grants. Those values come from the authenticated
session and invocation.

## Opaque bytes

Semantic fields use:

```json
{
  "encoding": "base64",
  "data": "UHJlc2VydmUgdGhpcyB0ZXh0IGV4YWN0bHkuCg=="
}
```

Only canonical padded base64 is accepted. Decoded bytes are persisted without
text normalization. Each opaque field is limited by
`limits.max_semantic_payload_bytes`; oversize input returns `PayloadTooLarge`.

## Terminal tools

The actor must invoke exactly one of:

```text
record_action(request_id, action, raw_semantic, evidence)
record_no_action(request_id, reason, raw_semantic, evidence)
record_blocked(request_id, reason, blocker, retry, raw_semantic, evidence)
record_completed(request_id, summary, completion, raw_semantic, evidence)
```

Identifiers must contain 1 through 128 safe ASCII characters:
letters, digits, `-`, `_`, `.`, `/`, or `:`.

### `record_action`

`action` is a closed tagged union:

| Variant | Required fields |
| --- | --- |
| `spawn_engineer` | task, repository, base type, non-empty requested permission set, claim key |
| `file_issue` | repository, title, body, labels |
| `request_merge` | pull-request repository/number, 40-hex head SHA, `squash`/`merge`/`rebase` strategy |
| `request_deploy` | `sha256:<64 hex>` artifact digest, 40-hex source commit, allowed environment, `verified_full` backup policy |

Example:

```json
{
  "request_id": "goal-4052-action-1",
  "action": {
    "kind": "spawn_engineer",
    "task": {
      "encoding": "base64",
      "data": "SW1wbGVtZW50IHRoZSB0eXBlZCBPT0RBIHBhdGguCg=="
    },
    "repository": {"owner": "rysweet", "name": "Simard"},
    "base_type": "copilot",
    "requested_permissions": ["repo_read", "repo_write", "github_pr_write"],
    "claim_key": "rysweet/Simard:goal-4052"
  },
  "raw_semantic": {
    "encoding": "base64",
    "data": "VGhlIGdvYWwgaXMgcmVhZHkgZm9yIGFuIGVuZ2luZWVyLg=="
  },
  "evidence": []
}
```

For an accepted action, one SQLite transaction inserts the terminal outcome,
the engineer claim when applicable, and one pending effect job.

An action denied by the actor grants, policy, or observe-only guard is recorded
as a typed `blocked` terminal with an authorization blocker. Other invalid,
out-of-scope, or admission-rejected actions return an error and do not become
no-action.

### `record_no_action`

`reason` must be non-empty opaque bytes. It records the actor's semantic
decision that no machine action is appropriate.

### `record_blocked`

`blocker` variants are `goal`, `credential`, `authorization`, `resource`,
`operator`, and `external`. Retry variants are `never`, `after_goal`,
`after_signal`, and `after_time`.

### `record_completed`

`summary` must be non-empty. `completion.criterion_id` must be a valid
identifier, and `completion.verification_evidence` must be non-empty and also
appear in the outcome's `evidence` list.

## Progress

`record_progress` is available to separately granted callers, not the default
goal-session terminal actor:

```text
record_progress(request_id, session_id, cycle_id, goal_id, percent, summary, evidence)
```

`percent` is `0..=100`; `summary` is non-empty opaque bytes.

## Evidence

The implemented `EvidenceRef` variants are:

| Variant | Fields |
| --- | --- |
| `commit` | repository, 40-hex SHA |
| `check_run` | repository, nonzero check ID, non-empty conclusion |
| `issue` | repository, nonzero issue number |
| `engineer_run` | engineer session ID, claim key |

Duplicate evidence references are rejected.

## Durable records

```text
TerminalOutcome
|- outcome_id
|- request_id
|- session_id
|- actor_identity
|- repository?
|- goal_id
|- cycle_id
|- kind: action | no_action | blocked | completed
|- payload
|- raw_semantic
|- evidence[]
`- recorded_at_unix_millis
```

The terminal table enforces unique `request_id`, unique `outcome_id`, and unique
`(session_id, cycle_id)`.

`ProgressRecord` has its own payload table, while its request ID participates in
the same global registry as terminal, actor-session, approval, effect, and
process mutations.

## Idempotency

All mutation fingerprints hash the authenticated actor identity and complete
scope, policy revision, mutation type, and complete serialized request using
the versioned SHA-256 canonical format.

| Condition | Result |
| --- | --- |
| New request ID | Validate and commit. |
| Same mutation type and fingerprint | Return the stored result. |
| Same ID with a different mutation type or fingerprint | `RequestConflict`. |
| Different terminal ID for an already closed cycle | `TerminalAlreadyRecorded`. |

The shared SQLite registry covers actor registration, terminal, progress,
approval, effect, and process mutations, so cross-type reuse is rejected.

## Authorization and admission

The default policy can grant:

```text
record_action.spawn_engineer
record_action.file_issue
record_action.request_merge
record_action.request_deploy
record_no_action
record_blocked
record_completed
record_progress
process_exec
```

The production goal-session policy omits `record_progress`. Direct merge and
direct deploy are internal grant variants and are not parsed from policy.

Repository actions must match the actor's bound repository. Policy then allows
the exact repository or its owner. Spawn permissions must be a non-empty subset
of `engineer_permissions`.

Admission currently enforces:

- non-empty admission policy revision;
- maximum disk-used percentage;
- maximum concurrent engineers;
- no active duplicate engineer claim.

## Effect outbox

Every action creates:

```text
EffectJob
|- effect_id
|- outcome_id
|- request_id
|- goal_id
|- repository?
|- kind
|- state: pending | running | blocked | succeeded | failed | indeterminate
|- action
|- attempt
|- lease_generation
|- lease_owner?
|- lease_expires_at_unix_millis?
|- error?
|- result?
`- approval?
```

A worker atomically changes one pending job to running, increments `attempt`
and `lease_generation`, and records a lease owner and expiry. Renewal,
completion, failure, and retry require the effect ID, current owner, current
generation, and an unexpired lease in one immediate transaction.

Startup recovery marks expired running jobs `indeterminate`; external effects
are never repeated merely because their lease expired.

Merge and deploy effects require a signed server-issued approval. Issue it with:

```bash
SIMARD_PRIVILEGED_PRINCIPAL=<principal> \
SIMARD_PRIVILEGED_APPROVAL_KEY=<at-least-32-byte-secret> \
simard ooda approvals issue --state-root <PATH> --effect-id <ID> \
  --request-id <ID>
```

The approval binds the principal, effect and outcome, session, cycle, goal,
action kind, action hash, repository, and policy revision.

## Errors

Capability errors:

| Rust code | Meaning |
| --- | --- |
| `InvalidArgument` | Identifier, schema, value, or action validation failed. |
| `PayloadTooLarge` | An opaque field exceeded policy. |
| `Unauthenticated` | Actor session was absent, expired, or mismatched. |
| `PermissionDenied` | Capability, repository, permission, or environment was outside policy. |
| `AdmissionRejected` | Disk, concurrency, or claim admission failed. |
| `StateTransitionRejected` | Completion or state transition was invalid. |
| `AuthorizationScopeViolation` | Cycle, goal, repository, engineer type, or permission exceeded authenticated scope. |
| `RequestConflict` | A request ID was reused across mutation types or with a different payload. |
| `TerminalAlreadyRecorded` | The cycle already has a terminal. |
| `StaleLease` | Effect owner, generation, state, or expiry did not match. |
| `MutationCapExhausted` | The scoped process-execution cap was consumed. |
| `IndeterminateExecution` | A process transition could not prove a safe result. |
| `PersistenceFailed` | SQLite or record serialization failed. |

Cycle errors separately report `MissingTerminal`, `MultipleTerminalAttempts`,
`ToolFailed`, `RecipeFailed`, `DownstreamFailed`, and `PersistenceFailed`.

## Policy schema

```toml
policy_id = "goal-session-policy-v1"
actor = "goal-session-actor"
terminal_calls_per_cycle = 1

capabilities = [
  "record_action.spawn_engineer",
  "record_action.file_issue",
  "record_action.request_merge",
  "record_action.request_deploy",
  "record_no_action",
  "record_blocked",
  "record_completed",
]

repositories = [{ owner = "rysweet", name = "Simard" }]
repository_owners = ["rysweet"]
engineer_permissions = [
  "repo_read",
  "repo_write",
  "process_exec",
  "github_issue_write",
  "github_pr_write",
]
deployment_environments = ["production"]

[limits]
max_semantic_payload_bytes = 1048576
max_concurrent_engineers = 8
max_disk_used_percent = 90

[identity]
bind_session = true
stable_request_id_required = true
```

Startup requires one terminal call, bound sessions, stable request IDs, at
least one repository or owner, a positive payload limit, concurrent-engineer
limit `1..=64`, and disk ceiling `1..=99`.

## Read API

```bash
simard ooda outcomes get --state-root "$SIMARD_STATE_ROOT" --request-id goal-4052-action-1
simard ooda outcomes list --state-root "$SIMARD_STATE_ROOT" --limit 100
```

`get` returns `{ "outcome": ..., "effect": ... }`. `list` returns terminal
outcomes only; fetch an action by request ID to inspect its effect.
