---
title: Bridge Wire Protocol Reference
description: Complete JSON-RPC-style wire protocol specification for all Simard bridge methods.
last_updated: 2026-03-31
owner: simard
doc_type: reference
---

# Bridge Wire Protocol Reference

All Simard bridges communicate via newline-delimited JSON on stdin (requests) and stdout (responses). This document specifies every method, its parameters, and its response shape.

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
| -32603 | Internal Error | Unhandled exception in the bridge server |
| -32000 | Timeout | Response not received within deadline |
| -32001 | Transport Error | Stdin/stdout broken, process exited |

---

## Bridge Health (all bridges)

### `bridge.health`

**Params**: `{}`

**Result**:
```json
{"server_name": "simard-memory", "healthy": true}
```

---

## Memory Bridge Methods

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

## Knowledge Bridge Methods

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

## Gym Bridge Methods

### `gym.list_scenarios`

List available benchmark scenarios.

**Params**: `{}`

**Result**:
```json
{"scenarios": [
  {"id": "repo-exploration", "description": "Identify repository structure and dependencies", "level": "L1"}
]}
```

---

### `gym.run_scenario`

Run a single benchmark scenario.

**Params**: `{"scenario_id": "repo-exploration"}`

**Result**:
```json
{
  "scenario_id": "repo-exploration",
  "score": 0.83,
  "dimensions": {
    "factual_accuracy": 0.9,
    "specificity": 0.8,
    "temporal_awareness": 0.75,
    "source_attribution": 0.85,
    "confidence_calibration": 0.85
  },
  "duration_secs": 45
}
```

---

### `gym.run_suite`

Run a complete benchmark suite.

**Params**: `{"suite_id": "progressive-L1-L6"}`

**Result**:
```json
{
  "suite_id": "progressive-L1-L6",
  "overall_score": 0.87,
  "level_scores": {"L1": 0.83, "L2": 1.0, "L3": 0.99, "L4": 0.79, "L5": 0.95, "L6": 1.0},
  "duration_secs": 300
}
```
