---
title: Agent Composition
description: How Simard composes multiple agent identities, spawns subordinates, and coordinates work through the hive mind.
last_updated: 2026-03-31
owner: simard
doc_type: concept
---

# Agent Composition

Simard is not a single monolithic agent. She is a **composite identity** that can spawn subordinate agents, delegate goals, and coordinate work through shared memory.

## Design Principles

1. **Identity and runtime are different things** — what an agent is (identity) vs. how it runs (topology)
2. **Composition outlives topology** — the same composite identity works local or distributed
3. **Communication through memory** — agents share knowledge via the hive, not raw IPC
4. **Explicit supervision** — parent monitors subordinates with heartbeats and crash recovery

## Identity Composition

An `IdentityManifest` defines a single agent's capabilities. A `CompositeIdentity` nests multiple manifests with role delegation:

```mermaid
graph TB
    subgraph "Simard (Composite)"
        SM[simard-main<br/>IdentityManifest]
        SE[simard-engineer<br/>subordinate]
        SR[simard-reviewer<br/>subordinate]
        SG[simard-gym<br/>subordinate]
    end

    SM -->|delegates engineering| SE
    SM -->|delegates review| SR
    SM -->|delegates eval| SG
```

Each subordinate receives:
- Its own `agent_name` for memory isolation
- A specific `OperatingMode` (Engineer, Meeting, Gym)
- A bounded goal
- A dedicated git worktree (no shared working directories)

## Supervisor Protocol

### Goal Assignment

Parent assigns goals through semantic memory in the shared hive:

```json
{
  "concept": "subordinate_goal:simard-sub-001",
  "content": "fix the null check in auth.rs and verify with tests",
  "confidence": 1.0
}
```

Subordinate reads its goal on startup via `search_facts("subordinate_goal:{my_id}")`.

### Progress Reporting

Subordinates report progress as semantic facts:

```json
{
  "concept": "subordinate_progress:simard-sub-001",
  "content": {
    "sub_id": "simard-sub-001",
    "phase": "execution",
    "steps_completed": 3,
    "steps_total": 7,
    "last_action": "edited src/auth.rs",
    "heartbeat_epoch": 1743400000,
    "outcome": null
  },
  "confidence": 1.0
}
```

### Liveness Detection

```mermaid
flowchart LR
    A[Parent polls<br/>heartbeat_epoch] --> B{Stale > 120s?}
    B -->|No| A
    B -->|Yes| C[Count: +1]
    C --> D{3 stale?}
    D -->|No| A
    D -->|Yes| E[Kill subprocess]
    E --> F{Retries < 2?}
    F -->|Yes| G[Retry with new subordinate]
    F -->|No| H[Escalate to operator]
```

- Parent checks `heartbeat_epoch` every 30 seconds
- 3 consecutive stale heartbeats (>120s each) → kill and mark `abandoned`
- At most 2 retries per goal, then escalate
- Subordinate's partial work persists in its memory for forensic inspection

### Crash Recovery

When a subordinate crashes:

1. Parent detects via `waitpid` / SIGCHLD
2. Marks goal as `crashed` with exit code
3. Inspects subordinate's episodic memory to understand what happened
4. Decides: retry, reassign, or escalate

### Recursion Limit

- `SIMARD_MAX_SUBORDINATE_DEPTH=3`
- Each subordinate inherits `depth + 1`
- At depth limit, subordinate cannot spawn further subordinates

## File Isolation

Each subordinate works in its own git worktree:

```
/home/azureuser/src/Simard/
├── worktrees/
│   ├── simard-sub-001/    ← subordinate 1's workspace
│   ├── simard-sub-002/    ← subordinate 2's workspace
│   └── ...
```

No two subordinates share a worktree. This prevents merge conflicts and allows concurrent file editing.

## The Self-Building Loop

When Simard reaches Phase 6, composition enables the self-improvement cycle:

```mermaid
sequenceDiagram
    participant Main as simard-main
    participant Gym as simard-gym
    participant Eng as simard-engineer
    participant Rev as simard-reviewer

    Main->>Gym: "run L1 benchmark, report score"
    Gym-->>Main: score: 83%
    Main->>Main: analyze failures, propose improvement
    Main->>Eng: "implement improvement in worktree"
    Eng-->>Main: patch ready, tests pass
    Main->>Rev: "review the patch"
    Rev-->>Main: approved, no regressions
    Main->>Gym: "re-run L1 benchmark"
    Gym-->>Main: score: 87% (+4%)
    Main->>Main: commit improvement, self-relaunch
```

## Future: Distributed Composition

The same composition model works across machines via azlin VMs:

```mermaid
graph TB
    subgraph "Local Machine"
        SM[simard-main]
    end
    subgraph "Azure VM 1"
        S1[simard-sub-001<br/>via azlin]
    end
    subgraph "Azure VM 2"
        S2[simard-sub-002<br/>via azlin]
    end

    SM -->|"azlin deploy + goal"| S1
    SM -->|"azlin deploy + goal"| S2
    S1 -->|"hive facts"| SM
    S2 -->|"hive facts"| SM
```

Communication still happens through the hive mind — the parent just deploys the subordinate binary to a remote VM instead of a local process. Memory replication ensures the subordinate can access relevant context.
