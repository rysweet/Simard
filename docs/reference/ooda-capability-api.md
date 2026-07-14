---
title: OODA capability API
description: Planned typed terminal tools, request schemas, authorization, idempotency, outcome records, and configuration for parser-free OODA execution.
last_updated: 2026-07-13
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

!!! info "Status"
    The goal-session capability API, SQLite ledger/outbox, scoped actor tools,
    fixture acceptance command, and outcome read commands are implemented.

The OODA capability API will be the only mutating boundary available to a
goal-session actor. The API is carried over the recipe runner's authenticated
typed tool channel. It is not a text protocol.

## Common terminal request fields

Every terminal request contains:

| Field | Type | Rules |
| --- | --- | --- |
| `request_id` | `RequestId` | Supplied by the caller; stable, non-empty, 1-128 ASCII characters, and unique across mutation requests. |
| `session_id` | `SessionId` | Must match the authenticated tool channel. |
| `cycle_id` | `CycleId` | Must match the active cycle in the session. |
| `goal_id` | `GoalId` | Must name a goal visible to the session. |
| `raw_semantic` | `OpaqueBytes` | Canonical padded base64; decoded bytes are persisted unchanged. |
| `evidence` | `EvidenceRef[]` | Typed references; duplicates are rejected. |

`actor_identity` is taken from the authenticated channel. A caller cannot
override it in tool arguments.

Progress and correction requests reuse `request_id` and authenticated identity
but have the fields shown in their own schemas.

```json
{
  "encoding": "base64",
  "data": "UHJlc2VydmUgdGhpcyB0ZXh0IGV4YWN0bHkuCg=="
}
```

Only `encoding: "base64"` is accepted. The handler rejects non-canonical
encodings, invalid padding, and decoded payloads above the configured limit.
File-backed transport avoids `argv` and environment limits, but does not bypass
this admission bound. Oversize values return `payload_too_large`; they are never
truncated.

## Request ID ownership

The capability caller supplies a request ID for every mutation. The runner binds
the authenticated session and active cycle but does not generate request IDs for
the actor. Retries of one logical mutation reuse the same ID and exact arguments;
a new logical mutation uses a new ID. A separate progress actor supplies its own
IDs for progress records.

## Terminal tools

### `record_action`

Records and admits one typed machine action.

```text
record_action(
  request_id,
  session_id,
  cycle_id,
  goal_id,
  action,
  raw_semantic,
  evidence
) -> TerminalOutcome
```

`action` is a closed tagged union.

#### `SpawnEngineer`

| Field | Type | Rules |
| --- | --- | --- |
| `kind` | literal | `"spawn_engineer"` |
| `task` | `OpaqueBytes` | Engineer objective, byte-preserved. |
| `repository` | `RepositoryRef` | Governed repository identifier. |
| `base_type` | enum | An enabled engineer base type. |
| `requested_permissions` | `Permission[]` | Subset of actor policy and repository policy. |
| `claim_key` | string | Stable exclusive-work claim. |

```json
{
  "request_id": "gs-01J2Y9-action",
  "session_id": "session-01J2Y9",
  "cycle_id": "cycle-1842",
  "goal_id": "goal-4052",
  "action": {
    "kind": "spawn_engineer",
    "task": {
      "encoding": "base64",
      "data": "SW1wbGVtZW50IHRoZSB0eXBlZCBnb2FsLXNlc3Npb24gcGF0aC4K"
    },
    "repository": {"owner": "rysweet", "name": "Simard"},
    "base_type": "copilot",
    "requested_permissions": ["repo_read", "repo_write", "github_pr_write"],
    "claim_key": "rysweet/Simard:goal-4052"
  },
  "raw_semantic": {
    "encoding": "base64",
    "data": "VGhlIGdvYWwgaXMgdW5ibG9ja2VkIGFuZCByZWFkeSBmb3IgYW4gZW5naW5lZXIu"
  },
  "evidence": []
}
```

The handler authenticates, authorizes, validates, applies admission, and commits
the terminal, engineer claim, and effect outbox job in one transaction. Launch
is dispatched from the outbox. A crash resumes the same idempotent job; a
permanent launch failure is explicit and linked to the terminal. Neither case
rewrites the outcome as no-action.

#### `FileIssue`

| Field | Type | Rules |
| --- | --- | --- |
| `kind` | literal | `"file_issue"` |
| `repository` | `RepositoryRef` | Governed repository. |
| `title` | `OpaqueBytes` | Non-empty after byte-level length validation; not semantically parsed. |
| `body` | `OpaqueBytes` | Exact issue body. |
| `labels` | `LabelRef[]` | Labels allowed by repository policy. |

#### `RequestMerge`

| Field | Type | Rules |
| --- | --- | --- |
| `kind` | literal | `"request_merge"` |
| `pull_request` | `PullRequestRef` | Owner, repository, and PR number. |
| `expected_head_sha` | `CommitId` | Required optimistic-concurrency guard. |
| `strategy` | enum | A strategy allowed by repository policy. |

This creates a merge request. It does not merge. The privileged merge executor
rechecks authorization, checks, reviews, head SHA, and merge policy.

#### `RequestDeploy`

| Field | Type | Rules |
| --- | --- | --- |
| `kind` | literal | `"request_deploy"` |
| `artifact` | `ArtifactRef` | Immutable artifact digest and source commit. |
| `environment` | `EnvironmentRef` | Governed deployment target. |
| `backup_policy` | enum | Must be `"verified_full"`. |

This creates a deployment request. It does not deploy or restart services. The
privileged deploy executor uses the installer transaction and existing
authorization gates.

### `record_no_action`

Records a semantic decision that no machine action is appropriate in this cycle.

```text
record_no_action(
  request_id,
  session_id,
  cycle_id,
  goal_id,
  reason,
  raw_semantic,
  evidence
) -> TerminalOutcome
```

`reason` is `OpaqueBytes`. Rust checks its byte length but does not interpret it.
No-action is not a fallback for recipe, tool, admission, or persistence failure.

### `record_blocked`

Records that the goal cannot proceed.

```text
record_blocked(
  request_id,
  session_id,
  cycle_id,
  goal_id,
  reason,
  blocker,
  retry,
  raw_semantic,
  evidence
) -> TerminalOutcome
```

| Field | Type | Purpose |
| --- | --- | --- |
| `reason` | `OpaqueBytes` | Exact semantic explanation. |
| `blocker` | `BlockerRef` | Typed external, goal, credential, authorization, resource, or operator dependency. |
| `retry` | `RetryPolicy` | `never`, `after_time`, `after_goal`, or `after_signal`. |

### `record_completed`

Records semantic completion after deterministic evidence gates pass.

```text
record_completed(
  request_id,
  session_id,
  cycle_id,
  goal_id,
  summary,
  completion,
  raw_semantic,
  evidence
) -> TerminalOutcome
```

| Field | Type | Purpose |
| --- | --- | --- |
| `summary` | `OpaqueBytes` | Exact completion narrative. |
| `completion` | `CompletionRef` | Typed done-condition and verification references. |
| `evidence` | `EvidenceRef[]` | Required evidence for the goal's completion policy. |

Completion fails when required typed evidence is missing, stale, mismatched, or
not authorized. A failed completion attempt does not become progress.

## Separate progress tool

### `record_progress`

Appends progress and evidence from a separate authenticated recipe step. It is
not available to the terminal Act actor.

```text
record_progress(
  request_id,
  session_id,
  cycle_id,
  goal_id,
  percent,
  summary,
  evidence
) -> ProgressRecord
```

`percent` is an integer from `0` through `100`. The progress caller supplies its
own stable request ID. A progress record may share the cycle identity, but it
does not satisfy the exactly-one terminal requirement and is not a call made by
the Act actor.

## Evidence references

`EvidenceRef` is a closed union:

| Variant | Required fields |
| --- | --- |
| `PullRequest` | repository, number, head SHA |
| `Issue` | repository, number |
| `Commit` | repository, full SHA |
| `CheckRun` | repository, check ID, conclusion |
| `Artifact` | digest, source commit |
| `Deployment` | environment, deployment ID, artifact digest |
| `EngineerRun` | engineer session ID, claim key |
| `Goal` | goal ID, revision |
| `FileDigest` | governed path, SHA-256 |
| `ExternalSignal` | provider, stable signal ID |

URLs may be stored as display metadata, but Rust never extracts identifiers from
them. Routing uses the typed fields.

## Outcome schema

```text
TerminalOutcome
├── outcome_id
├── request_id
├── session_id
├── actor_identity
├── repository: authenticated goal-session repository
├── goal_id
├── cycle_id
├── kind: action | no_action | blocked | completed
├── payload
├── raw_semantic: bytes
├── evidence[]
└── recorded_at
```

For `kind: action`, `payload` contains both the closed action union and its
`AdmissionDecision`. For `no_action`, `blocked`, and `completed`, admission is
absent because no action admission occurred.

`AdmissionDecision` contains typed rail results:

```text
AdmissionDecision
├── authorization: allowed
├── concurrency: admitted
├── resources: admitted
├── overlap: admitted
├── risk: admitted
└── policy_revision
```

Only an accepted decision is embedded in an action terminal. A denial or rail
rejection is an explicit tool error with a separate `AdmissionRejection` audit
record; it is not a successful terminal outcome.

### Corrections

Corrections are a separate append-only record type:

```text
record_outcome_correction(
  request_id,
  target_outcome_id,
  reason,
  evidence
) -> OutcomeCorrection
```

This capability is granted only to a privileged correction actor or operator,
never to the goal-session Act actor.

```text
OutcomeCorrection
├── correction_id
├── request_id
├── target_outcome_id
├── actor_identity
├── reason: bytes
├── evidence[]
└── recorded_at
```

A correction may mark an outcome's interpretation invalid or superseded, but it
does not replace the terminal and does not consume another terminal slot for the
cycle. Any corrected decision is recorded by a new Act cycle. The store allows
many corrections for one outcome, ordered by `recorded_at` and
`correction_id`; each correction request remains idempotent.

### Effect outbox

Actions that require an external effect create an `EffectJob` in the same
transaction as the terminal:

```text
EffectJob
├── effect_id
├── outcome_id
├── request_id
├── kind
├── state: pending | running | blocked | succeeded | failed | cancelled
├── attempt
├── lease_expires_at?
└── last_error?
```

The dispatcher leases pending jobs and uses the original request identity as the
downstream idempotency key. An expired `running` lease is recoverable after a
crash. A permanent failure is durable and fails cycle execution without
creating or replacing a terminal. Cancellation is allowed only when the effect
policy declares it safe and produces an audit record.

Merge and deploy jobs additionally require an append-only server-issued
authorization decision. Without one, the worker records the denial and moves
the job to `blocked`; it never emits empty evidence as success. An operator
issues approval with:

```text
SIMARD_PRIVILEGED_PRINCIPAL=<principal> \
SIMARD_PRIVILEGED_APPROVAL_KEY=<owner-controlled-secret> \
simard ooda approvals issue --state-root <PATH> --effect-id <ID>
```

The approval binds the principal, effect and outcome, goal, session, cycle,
action kind, canonical payload hash, exact repository, and policy revision.
Issuing it returns the blocked job to `pending`.

## Idempotency

The canonical request fingerprint covers:

- capability name;
- authenticated actor identity;
- session, cycle, and goal IDs;
- decoded opaque bytes;
- the canonical typed action and evidence;
- the active policy revision where policy affects the mutation.

Behavior:

| Condition | Result |
| --- | --- |
| New request ID | Validate and execute. |
| Same request ID, same fingerprint | Return the existing durable result. |
| Same request ID, different fingerprint | `idempotency_conflict`. |
| Different request ID, terminal already exists for cycle | `terminal_already_recorded`. |
| Replay after process restart | Same result as an in-process replay. |

## Authorization

Capabilities are granted to the authenticated recipe step, not to prose or
prompt names. The default goal-session actor policy permits:

```text
record_action.spawn_engineer
record_action.file_issue
record_action.request_merge
record_action.request_deploy
record_no_action
record_blocked
record_completed
```

Repository, environment, identity, and risk policies may narrow this set. Every
repository mutation must also match the exact repository bound to the
authenticated goal session; an allowed owner alone is insufficient.
Direct merge and direct deploy are never granted to a goal-session actor.
`record_progress` belongs to a separate progress-recorder identity and policy.

`SIMARD_OBSERVE_ONLY` removes action grants when the actor session is created and
is rechecked immediately before capability commit and effect dispatch. A denied
action becomes a durable typed blocked terminal. A dispatch-time denial becomes
a durable blocked effect plus authorization-decision record.

Spawned engineers receive the requested permission subset as an enforced child
process scope. Scoped Copilot sessions do not receive `--allow-all-tools` or
`--allow-all-paths`; repository read/write, process execution, GitHub mutation,
temporary-path access, environment exposure, and credentials are enabled only
by the corresponding granted permission. Opaque task bytes live in a separate
private worktree file and are referenced by a trusted server-generated brief.

## Errors

Tool errors are typed:

| Code | Meaning |
| --- | --- |
| `invalid_argument` | Typed schema or field validation failed. |
| `payload_too_large` | A decoded opaque field exceeded its configured byte limit. |
| `unauthenticated` | Tool channel has no valid actor/session identity. |
| `permission_denied` | Identity lacks the requested capability or scope. |
| `admission_rejected` | A deterministic safety rail rejected the action. |
| `idempotency_conflict` | Request ID was reused with different arguments. |
| `terminal_already_recorded` | The cycle already has a terminal. |
| `state_transition_rejected` | Goal state does not allow the mutation. |
| `persistence_failed` | Transaction did not commit. |
| `downstream_failed` | An authorized effect reached a permanent failed state after terminal commit. |
| `missing_terminal` | Recipe exited successfully without a terminal. |
| `transport_integrity_failed` | Opaque bytes did not round-trip exactly. |

Error messages are diagnostic only. Callers branch on the typed code.

## Policy

The installed policy is
`prompt_assets/simard/policies/goal-session-capabilities.toml`. Runtime limits
are validated by `CapabilityPolicy`; unsupported configuration does not silently
change those limits.

The policy asset also declares exact repositories and governed owners,
permitted engineer capabilities (`repo_read`, `repo_write`, `process_exec`,
`github_issue_write`, and `github_pr_write`), and allowed deployment
environments. Runtime actor leases narrow that policy to one exact goal
repository.

```toml
[ooda.goal_session]
recipe = "simard/recipes/goal-session-actor.yaml"
step_timeout_seconds = 1800
max_semantic_payload_bytes = 1048576
require_terminal = true

[ooda.outcome_ledger]
path = "typed-ooda/outcomes.sqlite3"

[ooda.capabilities]
policy = "prompt_assets/simard/policies/goal-session-capabilities.toml"

[ooda.admission]
max_concurrent_engineers = 24
disk_ceiling_percent = 90
require_exact_claim = true
```

| Setting | Default | Rules |
| --- | --- | --- |
| `ooda.goal_session.recipe` | Installed goal-session actor recipe | Must resolve under installed recipe assets. |
| `ooda.goal_session.step_timeout_seconds` | `1800` | Positive integer. Timeout fails the cycle. |
| `ooda.goal_session.max_semantic_payload_bytes` | `1048576` | Applies after base64 decode. |
| `ooda.goal_session.require_terminal` | `true` | Must remain true when the typed route is selected. Startup rejects false. |
| `ooda.outcome_ledger.path` | `typed-ooda/outcomes.sqlite3` | Resolved under the OODA state root. |
| `ooda.capabilities.policy` | Installed least-privilege policy | Must be owner-controlled and valid. |
| `ooda.admission.max_concurrent_engineers` | `24` | Range `1..=64`. |
| `ooda.admission.disk_ceiling_percent` | `90` | Range `1..=99`; fail-closed. |
| `ooda.admission.require_exact_claim` | `true` | Startup rejects false when the typed route is selected. |

There is no per-cycle parser mode, output schema, marker grammar, repair retry,
typed-to-parser fallback, semantic fallback, or fail-open admission setting.

## Read API

```text
simard ooda outcomes get --state-root "$SIMARD_STATE_ROOT" --request-id gs-01J2Y9-action
simard ooda outcomes list --state-root "$SIMARD_STATE_ROOT" --limit 100
```

Read commands return ledger records. They do not reconstruct outcomes from logs
or recipe output.

## See also

- [Typed-capability OODA architecture](../architecture/typed-ooda-loop.md)
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
- [Deploy and roll back typed OODA](../operations/deploy-and-roll-back-typed-ooda.md)
