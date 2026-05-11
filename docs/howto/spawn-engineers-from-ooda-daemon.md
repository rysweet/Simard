---
title: How OODA spawns engineer agents
description: How the OODA daemon's advance-goal action parses the orchestrator LLM's prose response and dispatches subordinate engineer agents.
last_updated: 2026-05-11
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./run-ooda-daemon.md
  - ../reference/simard-cli.md
  - https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/goal_session_objective.md
---

# How OODA spawns engineer agents

When the OODA daemon advances a goal, it consults an LLM "orchestrator
session" that returns **prose** (not JSON). The dispatcher parses that
prose and either (a) spawns a subordinate engineer subprocess to do the
work, or (b) records a no-action outcome explaining why nothing was done
this cycle.

There is no JSON envelope, no `action` discriminator, no schema
validation. The orchestrator says what should happen in plain English and
the dispatcher acts on it.

## The response contract

The orchestrator is instructed (via
`prompt_assets/simard/goal_session_objective.md`) to emit one of two
response shapes:

### 1. SpawnEngineer — free-form prose

A short paragraph describing the concrete next task for an engineer. The
paragraph becomes the engineer subprocess's task description verbatim,
so it should read like a work order:

```
Run `cargo test --lib goal_session` and report which tests fail. If
any failures involve the new prose dispatcher, file a follow-up issue
referencing the failure mode.
```

The engineer is itself a full coding agent (see `SIMARD_ENGINEER_AGENT`)
that can run `gh issue create`, `gh pr comment`, edit files, open PRs,
etc. Do not bother specifying file paths or shell commands in advance —
the engineer will figure those out.

### 2. NoAction — the `NO ACTION` marker

If no work should be done this cycle (e.g., another subordinate is
already in flight, or the goal is blocked on external review), the
response must include the literal text `NO ACTION` on its own line:

```
NO ACTION
Another subordinate (engineer-foo-1234) is already working this goal;
spawning a second one would create a merge conflict.
```

The dispatcher records the full response (including the prose
explanation after the marker) as the outcome reason. No subprocess is
spawned.

The marker is recognized case-insensitively and also accepts the
underscore form `NO_ACTION`. It must be on its own line so that prose
mentioning the literal phrase ("we should take no action against this")
does not accidentally trigger a no-op.

## Optional `PROGRESS: NN` marker

Either response shape may include a `PROGRESS: NN` marker (where `NN` is
0..=100) to update the goal's recorded completion percentage:

```
NO ACTION
Waiting on PR review. PROGRESS: 95
```

The marker is parsed case-insensitively and clamped to 0..=100. The
update happens before the engineer subprocess is spawned, so even if the
subprocess crashes the orchestrator's progress assessment is preserved.

## Empty response is a visible failure

A response that trims to the empty string is a hard failure — there is
nothing for the dispatcher to act on. The outcome is marked
`success=false` with detail `"goal-action empty response for goal 'X':
LLM returned no content"`. There is no silent fallback; the failure
surfaces in the cycle report and the journal.

## Implementation

The dispatch logic lives in
`src/ooda_actions/goal_session/{mod.rs,advance.rs}`:

- `parse_orchestrator_response(response: &str) -> Option<OrchestratorDecision>`
  parses the prose into a `GoalAction` (`SpawnEngineer` or `NoAction`)
  paired with the optional `progress_pct`.
- `advance_goal_with_session(...)` calls the LLM, parses the response,
  applies any progress marker, and dispatches the action.
- The actual subprocess spawn happens upstream in
  `src/ooda_actions/advance_goal/mod.rs::dispatch_spawn_engineer`,
  which forks the configured engineer agent (see `SIMARD_ENGINEER_AGENT`).

## Why prose, not JSON?

JSON envelopes added a parsing failure mode (the LLM emits prose, the
dispatcher rejects it, no work happens) without adding any value: every
JSON variant the orchestrator could emit could be expressed equivalently
in prose, and the engineer subprocess reads the task as prose anyway.
Removing the JSON layer:

- Eliminates the entire class of "LLM returned non-JSON" failures
- Lets the orchestrator phrase decisions naturally
- Removes ~700 lines of JSON parsing, validation, and `gh::` dispatch
  helpers (the engineer subprocess now does its own GitHub calls)
