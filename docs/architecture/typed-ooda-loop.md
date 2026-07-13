---
title: Typed-capability OODA architecture
description: Architecture of Simard's parser-free OODA workflows, capability boundary, durable outcomes, workflow ownership, and migration boundary.
last_updated: 2026-07-13
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/ooda-capability-api.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../operations/deploy-and-roll-back-typed-ooda.md
  - ../tutorials/complete-a-typed-ooda-cycle.md
---

# Typed-capability OODA architecture

!!! info "Implementation boundary"
    The live goal-session Act path, typed capability handler, outcome ledger,
    effect outbox, scoped actor runtime, fixture cycles, and installer rollback
    are implemented. The inventory below also records later migration work for
    OODA brains and coupled workflows that do not enter goal-session Act.

The migrated OODA path composes semantic agents with deterministic recipes.
Agent output remains opaque to Rust. When a cycle needs a machine action, the
final actor invokes a typed capability. Rust validates the typed request and
commits an immutable outcome plus a durable effect job.

```text
Observe recipe
    │ opaque semantic output
    ▼
Orient recipe
    │ opaque semantic output
    ▼
Decide recipe
    │ opaque semantic output
    ▼
Goal-session actor
    │ exactly one terminal typed capability
    ▼
Capability handler
    ├── authenticates actor and session
    ├── authorizes the capability
    ├── validates typed arguments
    ├── applies admission and safety rails
    ├── enforces idempotency
    └── commits outcome, claim, and effect job
            ├── Append-only outcome ledger
            └── Durable effect outbox
```

Rust does not inspect agent prose, JSON, markers, formatting, first words,
keywords, URLs, or error text to choose behavior. Typed tool-protocol decoding
and parsing deterministic application data are allowed.

## Architectural boundary

Recipes and prompts own semantic policy:

- what the observations mean;
- which goal matters now;
- whether work is useful, blocked, complete, or unnecessary;
- how to describe a task, reason, issue, or evidence;
- whether a situation merits an engineer, issue, merge request, or deploy
  request;
- how Observe, Orient, Decide, Act, Curate, Overseer, and cognitive-thread
  reasoning are composed.

Rust owns deterministic safety rails:

- actor and session authentication;
- capability authorization and least privilege;
- typed argument and state-transition validation;
- resource, overlap, concurrency, and risk admission;
- idempotency and exactly-once terminal recording;
- transactional persistence;
- process lifecycle and explicit error propagation;
- privileged merge and deployment execution after separately authorized
  requests.

Neither side substitutes for the other. Rust does not infer intent from text,
and recipes cannot bypass Rust's safety rails.

## Workflow inventory

| Workflow | Semantic steps | Final machine boundary | Durable truth |
| --- | --- | --- | --- |
| Observe | Summarize goal, runtime, repository, memory, meeting, engineer, and external state. | Typed readers expose deterministic state; no mutating terminal. | Source stores and observation provenance. |
| Orient | Interpret observations, urgency, dependencies, conflicts, and recalled memory. | None; output passes opaquely to Decide. | Recipe execution record, not business outcome. |
| Decide | Select the next semantic direction and supply it to Act. | None; output passes opaquely to the goal-session actor. | No business outcome until Act commits a terminal. |
| Act / goal session | Choose action, no-action, blocked, or completed. | Exactly one call: `record_action`, `record_no_action`, `record_blocked`, or `record_completed`. | `TerminalOutcome`. |
| Curate | Judge goal ordering, backlog promotion, decomposition, and stale-goal treatment. | Typed goal-store capabilities. | Goal-store mutations and linked outcome/evidence records. |
| Engineer admission | Judge likely overlap and usefulness. | Deterministic authorization, exact-path conflict, resource, count, and risk rails. | `AdmissionDecision` linked to the action outcome. |
| Engineer lifecycle | Judge whether to wait, reclaim, redirect, block, or request follow-up. | Scoped lifecycle capabilities; never a lifecycle prose parser. | Lifecycle records and terminal goal outcomes. |
| Progress and evidence | A separate progress step judges semantic progress and selects evidence outside the terminal Act invocation. | `record_progress` validates percentage, evidence references, identity, and state. | Immutable `ProgressRecord`; it does not satisfy the Act terminal. |
| Completion | Judge whether the goal's semantic done condition is satisfied. | `record_completed` validates required typed evidence and completion gates. | `TerminalOutcome(kind=completed)`. |
| No progress | Explain why work stalled and choose retry, defer, block, or no-action. | The actor invokes a matching typed capability; deterministic retry/admission limits still apply. | Blocked/no-action terminal plus evidence. |
| Overseer | Diagnose system and goal-board conditions and choose a corrective request. | Scoped issue, engineer, progress, blocked, merge-request, and deploy-request capabilities. | Typed corrective-action and outcome records. |
| Cognitive threads | Produce research, memory, creative-idea, journal, and health semantics. | Typed memory, goal, issue, and evidence capabilities appropriate to each thread. | Domain records in the owning store. |
| Meeting-to-goal | Interpret meeting decisions and proposed work. | Typed goal creation/update capability. | Goal records with meeting provenance. |
| Merge execution | Judge merge readiness in the privileged merge workflow. | Existing gated merge executor, reached from a typed `RequestMerge`. | Merge request and executor result records. |
| Deployment execution | Judge deployment readiness in the privileged deploy workflow. | Installer/deploy executor, reached from a typed `RequestDeploy`. | Deploy request, backup manifest, and deploy result. |

## Branch ownership matrix

Every branch belongs to exactly one side of the boundary.

| Branch | Owner | Enforcement |
| --- | --- | --- |
| Which goal to work on | Recipe/prompt | Semantic ranking and reasoning. |
| Whether observations imply urgency | Recipe/prompt | Opaque Orient output. |
| Whether another engineer's work is semantically overlapping | Recipe/prompt | Engineer-admission recipe. |
| Whether exact target paths or exclusive claims conflict | Rust | Deterministic conflict rail. |
| Whether host resources permit a spawn | Rust | Fail-closed resource admission. |
| Whether work should spawn an engineer | Recipe/prompt | `record_action(SpawnEngineer)`. |
| Whether work should file an issue | Recipe/prompt | `record_action(FileIssue)`. |
| Whether work is unnecessary now | Recipe/prompt | `record_no_action`. |
| Whether a goal is blocked and why | Recipe/prompt | `record_blocked`. |
| Whether a goal meets its semantic done condition | Recipe/prompt | `record_completed`. |
| Whether required evidence is present and well-typed | Rust | Typed schema and state-transition validation. |
| Whether an identity may use a capability | Rust | Session-bound authorization. |
| Whether an action is too risky for goal-session authority | Rust | Capability policy and admission. |
| Whether a merge/deploy request may execute | Privileged workflow plus Rust rails | Separate authorization and existing gates. |
| Whether a duplicate request is a replay | Rust | Canonical argument hash and unique request ID. |
| Whether a cycle already has a terminal | Rust | Unique `(session_id, cycle_id)` constraint. |
| Whether recipe completion counts as cycle success | Rust | Success requires one committed terminal; an action cycle also satisfies its effect completion policy. |
| How a task, reason, summary, or rationale is worded | Recipe/prompt | Opaque bytes; no normalization. |
| How process, tool, authorization, or persistence failure is handled | Rust | Explicit error; no fallback outcome. |
| Whether a failed step should be diagnosed or retried | Recipe policy within deterministic retry limits | A new attempt receives explicit failure context; the failed attempt remains failed. |

## Goal-session execution

The recipe runner creates a cycle identity and composes the semantic steps. Each
step receives the previous step's raw output as an opaque input. The final actor
receives:

- authenticated actor and session context;
- `goal_id` and `cycle_id`;
- raw task, reason, and prior semantic outputs;
- typed observations and evidence references where deterministic data exists;
- only the capabilities allowed by the session policy.

The actor must invoke exactly one terminal capability:

```text
record_action
record_no_action
record_blocked
record_completed
```

The actor makes no other mutating tool call. Progress is recorded by a separate
recipe step with its own identity and request ID. This keeps the
Act boundary auditable: one actor invocation produces exactly one terminal tool
call.

Recipe process exit `0` is necessary but not sufficient. The runner verifies
that the ledger contains one terminal for `(session_id, cycle_id)`. Zero
terminals, multiple terminal attempts, a tool error, or a non-zero recipe exit
fails the cycle. For an action terminal, the runner also waits according to the
effect policy. A permanent failed effect fails the cycle; a recoverable pending
effect remains queued under its original identity and is reported as incomplete,
not successful.

## Raw semantic handoffs

Semantic payloads are byte-preserved within explicit admission limits:

1. The runner writes potentially large inputs to owner-only context files rather
   than `argv` or environment variables. File transport removes OS argument-size
   coupling; it does not make payload admission unbounded.
2. A recipe step receives the file path and reads the bytes without trimming or
   normalization.
3. Step output is passed directly to the next step through the runner's opaque
   output channel.
4. Typed capability arguments carry opaque byte fields as canonical padded
   base64.
5. The handler decodes once, rejects any field above its configured byte limit,
   and otherwise persists the exact bytes.

Round-trip tests cover empty content, newlines, NUL bytes, non-ASCII UTF-8,
invalid UTF-8, ANSI bytes, marker-like text, JSON-looking text, URLs, and payloads
larger than 256 KiB but within the configured limit. Oversize content fails with
`payload_too_large`; it is never truncated. No admitted content changes are
permitted.

## Typed outcome ledger

The append-only ledger is authoritative. Recipe stdout, stderr, logs, exit text,
and model output are diagnostic only.

```rust
struct TerminalOutcome {
    outcome_id: OutcomeId,
    request_id: RequestId,
    session_id: SessionId,
    actor_identity: IdentityRef,
    goal_id: GoalId,
    cycle_id: CycleId,
    kind: OutcomeKind,
    payload: TypedOutcomePayload,
    raw_semantic: Vec<u8>,
    evidence: Vec<EvidenceRef>,
    recorded_at: Timestamp,
}
```

`TypedOutcomePayload::Action` contains the admitted action and its
`AdmissionDecision`. No-action, blocked, and completed payloads have no
admission field because no machine action was admitted.

The store enforces:

- unique `request_id`;
- unique `(session_id, cycle_id)` terminal outcome;
- immutable committed outcomes;
- one transaction for idempotency registration and outcome insertion;
- identical replay returns the existing record;
- conflicting reuse of a request ID fails;
- corrections append a separate `OutcomeCorrection` linked by `outcome_id`;
- terminal success is returned only after the transaction commits.

An `OutcomeCorrection` has its own `correction_id`, `request_id`,
`target_outcome_id`, reason, evidence, actor identity, and timestamp. It can mark
the interpretation of a terminal as invalid or superseded, but it cannot insert
another terminal for the same `(session_id, cycle_id)`. A corrected decision is
made in a new cycle. This preserves terminal cardinality and immutable history.

See the [capability API reference](../reference/ooda-capability-api.md) for the
complete schemas.

## Action execution

`record_action` accepts a closed union:

- `SpawnEngineer`;
- `FileIssue`;
- `RequestMerge`;
- `RequestDeploy`.

The handler first validates and admits the request. For `SpawnEngineer`, one
transaction creates the terminal, engineer claim, and durable outbox job. The
outbox dispatcher launches the engineer after commit and records effect state:
`pending`, `running`, `succeeded`, `failed`, or `cancelled`.

Outbox jobs are claimed with a lease. After a crash, an expired `running` lease
returns to `pending`; the dispatcher resumes the same idempotent effect using the
original outcome and request identity. It never asks the actor for a replacement
terminal. A permanent launch failure records a typed failed effect and leaves
the action terminal intact; the cycle execution fails even though its semantic
decision is durably recorded.

`RequestMerge` and `RequestDeploy` create requests only. Goal-session actors do
not receive direct merge or deployment authority. Existing privileged executors
apply their own authorization and quality gates.

## Failure model

Failures remain failures.

| Failure | Cycle result | Durable effect |
| --- | --- | --- |
| Recipe step exits non-zero | Failed | Recipe execution failure; no synthetic terminal. |
| Actor returns without terminal | Failed | Missing-terminal failure. |
| Second terminal attempt | Failed | Conflict; first committed terminal remains authoritative. |
| Tool protocol or schema error | Failed | Rejected request record, when safe to persist. |
| Authentication or authorization denied | Failed | Denial audit record. |
| Admission rejected | Failed or explicitly blocked only when the actor calls `record_blocked` in a new valid attempt | Admission decision; never automatic no-action. |
| Persistence failure | Failed | No success response and no partial outcome. |
| Crash after terminal commit, before effect | Pending recovery | Outbox dispatcher resumes the same idempotent effect; no second terminal is allowed. |
| Downstream engineer/issue effect fails | Failed | Typed failed effect linked to the terminal; retry follows the outbox policy. |
| Merge/deploy request rejected | Request remains rejected | Typed executor result; goal is not marked complete. |
| Opaque transport round trip fails | Failed | Transport integrity error. |

There is no parser repair retry, marker compatibility mode, deterministic
semantic fallback, or fallback from typed execution to a legacy parser.

## Parser-removal boundary

The typed goal-session route cannot call prose interpreters. The following
behaviors are absent from the route:

| Removed behavior | Replacement |
| --- | --- |
| Decide keyword/first-word scans | Decide output passes opaquely to the actor. |
| Orient number scraping from prose | Orient remains semantic; deterministic metrics arrive as typed inputs. |
| Engineer-lifecycle marker or JSON parsing | Lifecycle actor invokes typed capabilities. |
| Goal-session `ACTION`, `TASK`, `REASON`, or `PROGRESS` markers | Terminal tools receive typed arguments and opaque bytes. |
| Progress/completion verdict parsing | `record_progress` and `record_completed`. |
| No-progress classification tokens | Semantic actor chooses a terminal capability. |
| PR/issue URL extraction from logs | Typed `EvidenceRef::PullRequest` and `EvidenceRef::Issue`. |
| Error-text classification that changes behavior | Typed process/tool errors and exit status. |
| Parse-repair prompts and compatibility retries | Explicit failed recipe attempt. |
| Deterministic `AdvanceGoal` or skip fallback | Missing terminal or actor failure. |

After cutover, legacy prose parsers may exist only in workflows that are not reachable from
the typed goal-session process tree. Route-construction tests walk every
registered recipe, capability, subprocess, and callback edge from goal-session
execution and fail if a quarantined parser module is reachable.

## Migration and rollback

The goal-session route is a production compile-time cutover, not a per-cycle
parser switch. Production `AdvanceGoal` dispatch cannot call the quarantined
legacy response parser; that module is compiled only for legacy unit fixtures.
Typed failures therefore have no compatibility fallback.

Deployment rollback uses the verified installer manifest. It restores the
previous binary, prompt/recipe/policy tree, service units, configuration, and
compatible state together. Typed records remain part of the restored state.
See the deployment runbook for the exact command.

The typed cutover scope includes the complete goal-session action path and every
synchronous dependency it can reach:

- Observe, Orient, Decide, and goal-session actor recipe composition;
- terminal capability registration and dispatch;
- engineer spawn, admission, claim, and launch result;
- progress/evidence, no-action, blocked, and completed recording;
- issue creation requests;
- merge and deploy request creation;
- goal state updates and outcome persistence;
- Overseer and cognitive-thread calls that enter a goal session.

Privileged merge and deployment executors remain separate downstream workflows.
They accept typed requests and cannot be invoked directly by a goal-session
actor. Any remaining parser in those executors is outside the migrated process
tree and is tracked separately; it cannot affect goal-session routing or create
a goal-session terminal.

## Safety invariants

1. Every successful Act cycle has exactly one durable terminal outcome and
   exactly one terminal tool invocation.
2. No terminal exists without authenticated actor and session identity.
3. Failures and blockers cannot be represented as progress.
4. Admitted raw semantic bytes are never truncated, normalized, repaired, or
   interpreted by Rust; oversize payloads fail before commit.
5. Terminal success is returned only after durable commit.
6. Merge and deployment remain outside goal-session execution authority.
7. Typed-path failures never fall back to prose parsing.
8. Every mutation has a stable request ID and canonical replay semantics.
9. Recipe logs and prose never become business truth.
10. A route cannot be enabled unless parser-unreachability tests pass.
11. Every admitted external effect has a durable outbox state and crash-safe,
    idempotent recovery.

## Validation contract

The release gate covers:

- successful engineer, issue, merge-request, and deploy-request actions;
- no-action, blocked, and completed terminals;
- exact raw-byte round trips;
- invalid typed arguments and unknown action variants;
- denied identities and capabilities;
- duplicate and conflicting request IDs;
- overlap, resource, concurrency, and risk admission rejection;
- transaction and persistence failure;
- missing and duplicate terminal attempts;
- recipe, tool, and downstream execution failure propagation;
- crash recovery before and after commit;
- graph proof that the goal-session route cannot reach quarantined parsers;
- one authorized completed cycle with an intended machine action;
- one authorized no-action or blocked cycle;
- durable evidence for both end-to-end cycles.

## See also

- [OODA capability API](../reference/ooda-capability-api.md)
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
- [Tutorial: complete a typed OODA cycle](../tutorials/complete-a-typed-ooda-cycle.md)
- [Deploy and roll back typed OODA](../operations/deploy-and-roll-back-typed-ooda.md)
