---
title: RPC Wire Protocol Reference
description: Complete JSON-RPC-style wire protocol specification for all Simard RPC methods.
last_updated: 2026-07-06
owner: simard
doc_type: reference
---

# RPC Wire Protocol Reference

All Simard RPC transports communicate via newline-delimited JSON on stdin (requests) and stdout (responses). This document specifies every method, its parameters, and its response shape.

> **The wire contract is frozen.** The Bridge→RPC rename changed only Rust
> identifiers and module paths. Every method name below — including the
> built-in `bridge.health` probe — is unchanged on the wire.

## Common Protocol

### Request Envelope

```json
{"id": "<uuid-v7>", "method": "<dotted.name>", "params": {<method-specific>}}
```

### Response Envelope (success)

```json
{"id": "<matching-uuid>", "result": {<method-specific>}}
```

### Response Envelope (error)

```json
{"id": "<matching-uuid>", "error": {"code": <int>, "message": "<description>"}}
```

### Error Codes

| Code | Name | Meaning |
|------|------|---------|
| -32601 | Method Not Found | Requested method is not registered |
| -32603 | Internal Error | Unhandled exception in the RPC server |
| -32000 | Timeout | Response not received within deadline |
| -32001 | Transport Error | Stdin/stdout broken, process exited |

---

## Transport Health (all transports)

### `bridge.health`

**Params**: `{}`

**Result**:
```json
{"server_name": "simard-memory", "healthy": true}
```

---

## Memory RPC Methods (Legacy — Removed)

> **These methods are no longer available via RPC.** Memory operations are now handled in Rust by the library-backed `LibraryCognitiveMemory` (over `amplihack-memory-lib`), which persists to LadybugDB. See [Cognitive Memory Architecture](../architecture/cognitive-memory.md) and [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md) for the current API. The method signatures below are preserved for historical reference only.

### `memory.record_sensory`

Record a raw observation with automatic expiry.

**Params**:
```json
{"modality": "pty_output", "raw_data": "cargo test ... ok", "ttl_seconds": 300}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| modality | string | yes | | Channel: `objective`, `pty_output`, `error`, `log` |
| raw_data | string | yes | | Raw observation text |
| ttl_seconds | int | no | 300 | Time-to-live in seconds |

**Result**: `{"sensory_id": "sen_01abc..."}`

---

### `memory.push_working`

Add a slot to working memory (20-slot bounded).

**Params**:
```json
{"slot_type": "goal", "content": "fix the auth bug", "task_id": "session-01abc...", "relevance": 1.0}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| slot_type | string | yes | | One of: `goal`, `constraint`, `context`, `plan` |
| content | string | yes | | Slot content |
| task_id | string | yes | | Session/task identifier for scoping |
| relevance | float | no | 1.0 | Priority weight (higher = more relevant) |

**Result**: `{"slot_id": "wrk_01abc..."}`

---

### `memory.get_working`

Retrieve all working memory slots for a task.

**Params**: `{"task_id": "session-01abc..."}`

**Result**:
```json
{"slots": [
  {"node_id": "wrk_01abc...", "slot_type": "goal", "content": "fix the auth bug", "relevance": 1.0, "task_id": "session-01abc..."}
]}
```

---

### `memory.clear_working`

Clear all working memory slots for a task.

**Params**: `{"task_id": "session-01abc..."}`

**Result**: `{"cleared_count": 3}`

---

### `memory.store_episode`

Record a session transcript as an episodic memory.

**Params**:
```json
{"content": "Session transcript text...", "source_label": "session", "metadata": {"branch": "main"}}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| content | string | yes | | Episode content (max 2000 chars recommended) |
| source_label | string | yes | | Provenance: `session`, `ci-run`, `user-input` |
| metadata | object | no | {} | Arbitrary key-value metadata |

**Result**: `{"episode_id": "epi_01abc..."}`

---

### `memory.consolidate_episodes`

Summarize the oldest batch of unconsolidated episodes.

**Params**: `{"batch_size": 10}`

**Result (success)**: `{"consolidated_id": "con_01abc..."}`

**Result (not enough episodes)**: `{"consolidated_id": null}`

---

### `memory.store_fact`

Store a semantic fact with confidence and optional tags.

**Params**:
```json
{
  "concept": "cargo test",
  "content": "runs all tests in the workspace",
  "confidence": 0.9,
  "tags": ["rust", "testing"],
  "source_id": "epi_01abc..."
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| concept | string | yes | | Topic/concept (must not be empty) |
| content | string | yes | | Factual content |
| confidence | float | no | 0.9 | Confidence score (0.0-1.0) |
| tags | string[] | no | [] | Categorization tags |
| source_id | string | no | "" | Episode ID for provenance linking |

**Result**: `{"fact_id": "sem_01abc..."}`

---

### `memory.search_facts`

Search semantic memory by keywords.

**Params**:
```json
{"query": "how to run tests", "limit": 10, "min_confidence": 0.3}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| query | string | yes | | Search keywords |
| limit | int | no | 10 | Maximum results |
| min_confidence | float | no | 0.0 | Minimum confidence threshold |

**Result**:
```json
{"facts": [
  {"node_id": "sem_01abc...", "concept": "cargo test", "content": "runs all tests in the workspace", "confidence": 0.9, "source_id": "epi_01abc...", "tags": ["rust", "testing"]}
]}
```

---

### `memory.store_procedure`

Store a reusable action sequence.

**Params**:
```json
{"name": "fix-and-verify", "steps": ["read file", "edit", "cargo test", "commit"], "prerequisites": ["git repo"]}
```

**Result**: `{"procedure_id": "pro_01abc..."}`

---

### `memory.recall_procedure`

Recall procedures matching a query.

**Params**: `{"query": "how to fix a bug", "limit": 5}`

**Result**:
```json
{"procedures": [
  {"node_id": "pro_01abc...", "name": "fix-and-verify", "steps": ["read file", "edit", "cargo test", "commit"], "prerequisites": ["git repo"], "usage_count": 3}
]}
```

---

### `memory.store_prospective`

Store a future trigger-action pair.

**Params**:
```json
{"description": "re-run gym after self-improve", "trigger_condition": "self_improve_complete", "action_on_trigger": "run_gym_suite", "priority": 2}
```

**Result**: `{"prospective_id": "psp_01abc..."}`

---

### `memory.check_triggers`

Check if any prospective memories match the given content.

**Params**: `{"content": "self_improve_complete: score improved by 3%"}`

**Result**:
```json
{"triggered": [
  {"node_id": "psp_01abc...", "description": "re-run gym after self-improve", "trigger_condition": "self_improve_complete", "action_on_trigger": "run_gym_suite", "status": "triggered", "priority": 2}
]}
```

---

### `memory.get_statistics`

Get counts for all memory types.

**Params**: `{}`

**Result**:
```json
{"sensory_count": 12, "working_count": 3, "episodic_count": 45, "semantic_count": 230, "procedural_count": 8, "prospective_count": 2}
```

---

### `memory.prune_expired_sensory`

Remove expired sensory items.

**Params**: `{}`

**Result**: `{"pruned_count": 7}`

---

## Knowledge RPC Methods

### `knowledge.query`

Query a knowledge pack for a grounded answer.

**Params**:
```json
{"pack_name": "rust-expert", "question": "How do lifetimes work?", "limit": 10}
```

**Result**:
```json
{
  "answer": "Lifetimes are Rust's way of tracking...",
  "sources": [
    {"title": "The Rust Programming Language", "section": "Lifetimes", "url": null}
  ],
  "confidence": 0.95
}
```

---

### `knowledge.list_packs`

List all available knowledge packs.

**Params**: `{}`

**Result**:
```json
{"packs": [
  {"name": "rust-expert", "description": "Comprehensive Rust knowledge", "article_count": 150, "section_count": 890}
]}
```

---

### `knowledge.pack_info`

Get details about a specific pack.

**Params**: `{"pack_name": "rust-expert"}`

**Result**:
```json
{"name": "rust-expert", "description": "...", "article_count": 150, "section_count": 890}
```

---

## Gym RPC Methods

> The gym client (`simard-gym-eval`) runs the progressive L1–L12 evaluation
> levels plus a long-horizon memory stress test. The evaluation engine is the
> [`amplihack-agent-eval`](https://github.com/rysweet/amplihack-rs) crate's
> native Rust `GymRunner`, wired in through the thin
> [`gym_runner_client`](../architecture/gym-eval-library-adapter.md) adapter
> (`src/gym_runner_client.rs`). The adapter — not the engine — owns this wire
> contract: it validates ids, honours `SIMARD_SKIP_GYM`, and maps the library's
> results onto the shapes below.

### `gym.list_scenarios`

List available evaluation scenarios.

**Params**: `{}`

**Result**: a bare JSON array (not wrapped in an object):
```json
[
  {
    "id": "L1-recall",
    "name": "L1 Recall",
    "description": "Baseline recall from a single source",
    "level": "L1-recall",
    "question_count": 5,
    "article_count": 1
  },
  {
    "id": "long-horizon-memory",
    "name": "Long-horizon memory stress test",
    "description": "1000-turn dialogue testing memory at scale",
    "level": "long-horizon",
    "question_count": 0,
    "article_count": 0
  }
]
```

The full built-in set is `L1-recall`, `L2-multi-source`, `L3-temporal`,
`L4-procedural`, `L5-contradiction`, `L6-incremental`, `L7-teacher-student`,
`L8-metacognition`, `L9-causal`, `L10-counterfactual`, `L11-novel-skill`,
`L12-far-transfer`, and `long-horizon-memory`.

---

### `gym.run_scenario`

Run a single evaluation scenario by id.

**Params**: `{"scenario_id": "L1-recall"}`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| scenario_id | string | yes | One of the ids from `gym.list_scenarios`. Validated against `^[A-Za-z0-9._-]{1,128}$` **and** an explicit `.`/`..` dot-segment guard (the regex alone permits `..`). |

The empty-id check runs **first**: an **empty** `scenario_id` is the only hard
transport error and returns an `error` envelope with code `-32603`. Any other
invalid id — one containing `/`, `\`, a null byte, exceeding the length cap, or
equal to `.` / `..` — is rejected as a failing **result** (see below), not a
transport error.

**Result**:
```json
{
  "scenario_id": "L1-recall",
  "success": true,
  "score": 0.83,
  "dimensions": {
    "factual_accuracy": 0.9,
    "specificity": 0.8,
    "temporal_awareness": 0.75,
    "source_attribution": 0.85,
    "confidence_calibration": 0.85
  },
  "question_count": 5,
  "questions_answered": 5,
  "degraded_sources": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| scenario_id | string | The requested id, echoed back. The adapter restores this; the engine internally rewrites successful results to a compact `L{n}` form, which the adapter overrides. |
| success | bool | `true` if the scenario ran and passed |
| score | float | Overall score in `[0.0, 1.0]` |
| dimensions | object | Always the five keys below, each a float in `[0.0, 1.0]` |
| question_count | int | Questions in the scenario |
| questions_answered | int | Questions actually graded |
| error_message | string? | Present only on failure (omitted on success) |
| degraded_sources | string[] | Sources that degraded during the run; empty on a clean run |

The five `dimensions` keys are always present: `factual_accuracy`,
`specificity`, `temporal_awareness`, `source_attribution`,
`confidence_calibration`. For the L1–L12 levels the current engine only derives
`factual_accuracy` and `specificity` (both set to the level's average score);
the other three are `0.0`. The illustrative values above show the wire *shape*,
not the engine's present per-dimension fidelity.

**Result (failure / honest degradation)**: the RPC envelope stays `result`
(not `error`); the payload reports the failure:
```json
{
  "scenario_id": "nonexistent",
  "success": false,
  "score": 0.0,
  "dimensions": {
    "factual_accuracy": 0.0,
    "specificity": 0.0,
    "temporal_awareness": 0.0,
    "source_attribution": 0.0,
    "confidence_calibration": 0.0
  },
  "question_count": 0,
  "questions_answered": 0,
  "error_message": "scenario 'nonexistent' not found",
  "degraded_sources": []
}
```

**Result (`SIMARD_SKIP_GYM=1`)**: synthetic success without invoking the engine:
```json
{
  "scenario_id": "L1-recall",
  "success": true,
  "score": 0.0,
  "dimensions": {"factual_accuracy": 0.0, "specificity": 0.0, "temporal_awareness": 0.0, "source_attribution": 0.0, "confidence_calibration": 0.0},
  "question_count": 0,
  "questions_answered": 0,
  "degraded_sources": ["SIMARD_SKIP_GYM"]
}
```

---

### `gym.run_suite`

Run the full progressive suite — the twelve progressive levels
(`L1-recall`..`L12-far-transfer`). The `long-horizon-memory` scenario is **not**
part of the suite; run it on its own via `gym.run_scenario`.

**Params**: `{"suite_id": "progressive"}`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| suite_id | string | no (default `progressive`) | A label for the run's artifacts, used in an output-path join. Validated against `^[A-Za-z0-9._-]{1,128}$` **plus** an explicit `.`/`..` dot-segment guard, so both `../../tmp/x` and a bare `..` are rejected. |

**Result**:
```json
{
  "suite_id": "progressive",
  "success": true,
  "overall_score": 0.87,
  "dimensions": {
    "factual_accuracy": 0.88,
    "specificity": 0.85,
    "temporal_awareness": 0.86,
    "source_attribution": 0.9,
    "confidence_calibration": 0.86
  },
  "scenario_results": [
    {
      "scenario_id": "L1-recall",
      "success": true,
      "score": 0.83,
      "dimensions": {"factual_accuracy": 0.9, "specificity": 0.8, "temporal_awareness": 0.75, "source_attribution": 0.85, "confidence_calibration": 0.85},
      "question_count": 5,
      "questions_answered": 5,
      "degraded_sources": []
    }
  ],
  "scenarios_passed": 12,
  "scenarios_total": 12,
  "degraded_sources": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| suite_id | string | Echoes the requested label |
| success | bool | `true` only if **every** scenario passed (`scenarios_passed == scenarios_total`). The adapter computes this directly from the per-scenario results; it does **not** trust the engine's suite-level flag, which has a known inverted-logic quirk upstream. |
| overall_score | float | Mean score across passing scenarios, in `[0.0, 1.0]` |
| dimensions | object | Five-key aggregate (mean across passing scenarios) |
| scenario_results | array | One `gym.run_scenario`-shaped object per scenario; each `scenario_id` is the advertised descriptive id, restored by the adapter |
| scenarios_passed | int | Number of scenarios that passed |
| scenarios_total | int | Number of scenarios evaluated (12) |
| error_message | string? | Present only on suite-level failure |
| degraded_sources | string[] | Sources that degraded during the run |

With `SIMARD_SKIP_GYM=1`, the suite returns `success: true`, empty
`scenario_results`, and `degraded_sources: ["SIMARD_SKIP_GYM"]` without invoking
the engine.
