---
title: Typed-capability OODA architecture
description: Implemented parser-free goal-session boundary, durable outcomes, workflow ownership, and remaining migration boundary.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/ooda-capability-api.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../tutorials/complete-a-typed-ooda-cycle.md
---

# Typed-capability OODA architecture

The migrated goal-session path makes durable typed tool calls authoritative.
Rust does not parse the final actor's prose, markers, URLs, or JSON-looking text
to decide what happened.

```text
existing Observe / Orient / Decide path
        |
        | opaque task, reason, and serialized context files
        v
goal-session recipe -> authenticated actor runtime
        |
        | exactly one terminal typed tool attempt
        v
capability handler
        |- validates actor, repository, arguments, policy, and admission
        |- commits terminal + claim + effect in SQLite
        `- dispatches the durable effect
```

## Ownership boundary

Recipes and agents own semantic judgment:

- what observations mean;
- whether work is useful, blocked, complete, or unnecessary;
- which typed action to request;
- task, reason, summary, and rationale wording.

Rust owns deterministic rails:

- actor-session authentication;
- capability and repository authorization;
- typed argument validation;
- byte-size limits;
- engineer concurrency, disk, and claim admission;
- terminal idempotency and one-terminal-per-cycle;
- SQLite persistence;
- effect dispatch and explicit failure propagation;
- separate approval for merge and deploy effects.

Rust may decode typed tool JSON and deterministic application state. It may not
interpret agent prose to select behavior.

## Workflow inventory and migration status

| Workflow | Current boundary | Status in this slice |
| --- | --- | --- |
| Observe / Orient / Decide | Existing OODA implementation prepares state and semantic context. | Inputs are forwarded into the typed goal session; these workflows are not fully reimplemented as typed capability recipes. |
| Act / goal session | Actor calls `record_action`, `record_no_action`, `record_blocked`, or `record_completed`. | Migrated |
| Engineer spawn | Typed action, claim, permission set, outbox effect, and subordinate launch. | Migrated for Copilot |
| Issue creation | Typed action and outbox effect using deterministic GitHub application data. | Migrated |
| Merge / deploy request | Typed action plus separately signed approval. | Migrated request boundary; downstream executors remain separate |
| Progress | `record_progress` capability exists outside the default terminal actor policy. | Implemented API; not part of the terminal actor |
| Curate, Overseer, cognitive threads, meetings | Existing domain workflows and stores. | Outside this migration slice |
| Legacy engineer/operator launch | Least privilege with no implicit tool or path grants when no typed permission set is present. | Outside typed OODA |

This table is the migration boundary. “Outside this slice” does not mean
parser-free unless that workflow has its own documented typed boundary.

## Branch ownership matrix

| Branch | Owner | Enforcement |
| --- | --- | --- |
| Whether to request an engineer, issue, merge, deploy, no-action, blocked, or completed | Actor | One typed terminal tool |
| Whether the actor may call that tool | Rust | Session grants and policy |
| Whether a repository target is allowed | Rust | Exact actor binding plus repository policy |
| Whether engineer permissions are allowed | Rust | Non-empty subset of policy ceiling |
| Whether disk, concurrency, or claim admission allows spawn | Rust | Deterministic checks before commit |
| Whether completion has typed evidence | Rust | Completion validation |
| Whether task/reason wording implies an action | Actor only | Rust does not inspect the wording |
| Whether a request is an identical terminal replay | Rust | Stored request fingerprint |
| Whether a cycle already has a terminal | Rust | Unique `(session_id, cycle_id)` |
| Whether an action effect succeeded | Effect executor | Durable effect state |
| Whether merge/deploy may execute | Approval authority plus executor | Signed approval and downstream checks |

## Goal-session route

`TypedGoalSessionRoute::production` resolves the installed recipe and policy
from `$HOME/.simard/prompt_assets/simard`, falling back to the repository asset
tree. Both assets must exist and validate.

The route:

1. registers a long-lived authentication lease bound to one actor, session,
   cycle, goal, repository, grant set, and observe-only state; the lease never
   kills the agent step;
2. writes task, reason, Observe, Orient, Decide, token, and admission values to
   separate private context files;
3. runs `recipe-runner-rs` with file paths rather than large semantic values in
   argv;
4. runs an agent step whose only business action is one authenticated
   `simard ooda terminal` invocation;
5. requires one durable terminal for the session and cycle before returning.

The agent reads the private semantic files and invokes one of four terminal
subcommands. Zero durable terminals, a conflicting second terminal, or a failed
recipe process fails the cycle.

## Raw semantic handoff

Semantic context is carried as bytes:

1. private files avoid argv-size and shell-normalization problems;
2. `OpaqueBytes` uses canonical padded base64 at the typed tool boundary;
3. decoded bytes are checked only for size unless a specific downstream effect
   requires UTF-8 application data;
4. admitted bytes are persisted unchanged.

The file-issue executor requires UTF-8 title and body because GitHub receives
text. Other opaque semantic fields remain bytes in the ledger.

## Durable terminal and replay

The terminal ledger stores immutable serialized outcomes. Recipe stdout,
stderr, and final model text are diagnostic only.

The terminal table has unique constraints on request ID, outcome ID, and
`(session_id, cycle_id)`. An identical request replay returns the stored
terminal; conflicting reuse returns `RequestConflict`.

Terminal, progress, actor-session, approval, effect, and process mutations use
one versioned request registry. Request fingerprints use SHA-256 over the
mutation type, complete typed payload, authenticated actor scope, policy
revision, cycle, and goal. Reuse across mutation types or payloads fails.

## Action effects

An action terminal and its effect job commit together. The production goal-session policy permits one effect: allocate a worktree and
spawn a scoped Copilot subordinate. Issue, merge, and deployment mutations are
not granted to the goal-session actor.

Jobs move through `pending`, `running`, `blocked`, `succeeded`, `failed`, or
`indeterminate`. Every claim increments a lease generation. Renewal, retry,
failure, and completion require the effect ID, owner, generation, and an
unexpired lease in one immediate transaction. Expiry marks execution
`indeterminate`; it never makes an external effect safe to repeat.

## Engineer authority

The typed spawn action receives only `repo_read` and `repo_write`. The
goal-session actor has no generic process, issue, pull-request, merge, or deploy
authority.
See [Engineer Copilot permissions](../reference/engineer-copilot-permissions.md).

## Failure model

| Failure | Result |
| --- | --- |
| Actor or recipe exits without a terminal | `MissingTerminal` |
| Actor attempts more than one terminal | `MultipleTerminalAttempts` |
| Tool validation or authorization fails | `ToolFailed`; no synthetic no-action |
| Persistence fails | Failure; no success response |
| Action effect is blocked or permanently fails | `DownstreamFailed`; terminal remains durable |
| Retryable effect fails | Job returns to pending; cycle reports incomplete |
| Recipe process exits nonzero | `RecipeFailed` |

There is no typed-to-parser fallback in the migrated goal-session path.

## Parser-removal boundary

The production goal-session actor returns business results only through typed
tools. It does not use:

- `ACTION`, `TASK`, `REASON`, or `PROGRESS` markers;
- first-word or keyword scans;
- URL extraction to create evidence;
- response-schema repair retries;
- recipe stdout as an outcome.

Legacy parsers may still exist in workflows outside the inventory row marked
“Migrated.” Their presence is not evidence that the typed goal-session actor
depends on them.

## Deployment boundary

`simard install` quiesces active services and captures the binary, prompt
assets, units and service state, config, canonical typed-OODA SQLite database,
and live LadybugDB cognitive store before replacement. SQLite uses its online
backup API and integrity checking. LadybugDB is checkpointed, copied while
quiesced, and verified by opening and querying the staged store.

Rollback stages each surface on the destination filesystem, syncs it, and uses
rename-based replacement. Every published swap has a synced compensation path;
any later failure restores earlier surfaces in reverse order. An absent
first-install service baseline removes newly introduced units without starting
nonexistent prior services.

## Verified invariants

1. A successful actor step has exactly one durable terminal.
2. Terminal authority comes from an authenticated session.
3. Admitted semantic bytes are size-checked and preserved.
4. Recipe prose and logs are not business truth.
5. Action, claim, and effect insertion share one transaction.
6. Merge and deploy effects require separate signed approval.
7. Typed goal-session failures do not fall back to prose parsing.
8. Typed Copilot spawns carry an explicit permission set.
9. All mutation request IDs share one versioned transactional registry.
10. Effect completion, failure, renewal, and retry are fenced by owner,
    generation, and expiry.
11. Process execution reserves its scoped cap before running outside SQLite.
12. Installer snapshots are application-consistent and rollback is staged,
    atomic per surface, and reverse-compensating across surfaces.
