# Simard Implementation Plan: Path to Self-Building

## Status

- **Created**: 2026-03-31
- **Last Updated**: 2026-03-31
- **Current Phase**: 0 (Bridge Infrastructure) — in progress
- **Overall Progress**: ~15-20% of original vision implemented

## Guiding Constraints

1. Every module ≤400 LOC
2. Every phase passes cargo test + outside-in gadugi YAML tests + feral usage tests
3. Each phase checks against ProductArchitecture.md pillars and original prompt requirements
4. Each workstream follows the DEFAULT_WORKFLOW (all 22 steps)
5. Memory integration uses amplihack-memory-lib's full 6-type cognitive model (not a simple store)
6. agent-kgpacks knowledge packs provide grounded domain knowledge
7. amplihack-agent-eval's progressive suite powers the gym

## Architecture Decision: The Bridge Pattern

Simard is Rust. The ecosystem (memory-lib, kg-packs, agent-eval, hive mind) is Python.
Rather than port everything, we use subprocess bridges with JSON-line protocol.

```
Simard (Rust) ──→ BridgeTransport trait ──→ Python subprocess (amplihack-memory-lib + LadybugDB)
               ──→ BridgeTransport trait ──→ Python subprocess (agent-kgpacks + LadybugDB)
               ──→ BridgeTransport trait ──→ Python subprocess (amplihack-agent-eval)
```

Each bridge has:
- A Rust trait with typed request/response structs
- An InMemoryBridgeTransport for unit testing
- A SubprocessBridgeTransport for production
- A Python bridge server (extends bridge_server.py base class)
- A circuit breaker (CLOSED → OPEN → HALF_OPEN) for fault tolerance

## Parallelism Map

```
Phase 0 (Bridge Foundation) ─────────────────────┐
                                                  │
              ┌───────────────────────────────────┼────────────────┐
              │                                   │                │
          Phase 1                            Phase 2          (Phase 0 done)
          (Cognitive Memory)                (Knowledge Packs)      │
              │                                   │                │
              └────────────────┬──────────────────┘                │
                               │                                   │
                          Phase 3 ─────────────────────────────────┘
                       (Real Base Type Adapters)
                               │
              ┌────────────────┼────────────────┐
              │                                 │
          Phase 4                          Phase 5
          (Gym / Eval)                 (Agent Composition)
              │                                 │
              └────────────────┬────────────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
          Phase 6          Phase 7          Phase 8
         (Self-Improve)   (Remote/azlin)   (Meeting/Goals)
              │                │                │
              └────────────────┼────────────────┘
                               │
                          Phase 9
                       (OODA / Autonomous)
```

Phases 1+2 run in parallel (both depend only on Phase 0).
Phases 4+5 run in parallel (both depend on Phase 3).
Phases 6+7+8 run in parallel (depend on different subsets of 4+5).

## Phase 0: Bridge Infrastructure

### Goal
Establish the subprocess bridge pattern that all subsequent phases use.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/bridge.rs` | ≤300 | BridgeTransport trait, BridgeRequest/Response types, wire format |
| `src/bridge_subprocess.rs` | ≤350 | SubprocessBridgeTransport (spawn, stdin/stdout JSON), InMemoryBridgeTransport |
| `src/bridge_circuit.rs` | ≤200 | Circuit breaker state machine (CLOSED/OPEN/HALF_OPEN) |
| `python/bridge_server.py` | ≤150 | Base class for Python bridge servers, EchoBridgeServer for testing |
| `tests/bridge.rs` | ≤400 | Integration: launch echo bridge, roundtrip, error cases, feral tests |

### Spec Checkpoint
- Architecture Pillar 10 (DI): Bridges injected via RuntimePorts
- Architecture Pillar 11 (Honest Degradation): Circuit breaker surfaces errors, never hides them

### Feral Tests
- Kill bridge mid-request → BridgeTransportError, not hang
- Send malformed JSON → BridgeProtocolError
- Send response with wrong id → ignored, waits for correct id
- 1MB payload → size rejection
- Bridge process exits immediately → BridgeTransportError on EOF

### Gadugi YAML
- bridge-echo-roundtrip: health check + unknown method + malformed JSON

## Phase 1: Durable Cognitive Memory

### Goal
Simard persists knowledge across restarts using amplihack-memory-lib's full 6-type cognitive model.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/memory_bridge.rs` | ≤400 | CognitiveMemoryBridge wrapping BridgeTransport, typed methods for all 6 types |
| `src/memory_cognitive.rs` | ≤350 | Rust structs matching Python dataclasses (SensoryItem, WorkingSlot, Episode, Fact, Procedure, Prospective) |
| `src/memory_consolidation.rs` | ≤300 | Session lifecycle → cognitive operation mapping (when to consolidate, prune, promote) |
| `src/memory_hive.rs` | ≤300 | Hive integration config: quality threshold, confidence gate, agent isolation |
| `python/simard_memory_bridge.py` | ≤400 | Python bridge server wrapping CognitiveAdapter with hive connection |
| `tests/memory_durable.rs` | ≤400 | Integration: store/retrieve across restart, consolidation, hive sharing |

### Wire Protocol
See IMPLEMENTATION_PLAN_WIRE_PROTOCOLS.md (to be created with Phase 1 design).

### Session Phase → Cognitive Type Mapping

| Session Phase | Operations |
|---|---|
| Intake | record_sensory(objective), push_working(goal) |
| Preparation | search_facts, check_triggers, push_working(context) |
| Planning | recall_procedure, push_working(plan) |
| Execution | record_sensory(pty_output), push_working(state) |
| Reflection | store_episode, store_fact (with source_id), store_procedure, store_prospective |
| Persistence | consolidate_episodes, clear_working, prune_expired_sensory |

### Concurrency Model
- Each subordinate Simard gets its own agent_name → LadybugDB agent_id isolation
- Writes serialized through bridge subprocess (one bridge per agent process)
- Hive reads use CognitiveAdapter.search() which merges local + hive via RRF
- Quality gate (threshold 0.3) before hive promotion

### Spec Checkpoint
- Architecture Pillar 5 (Memory Must Be Layered): 6 cognitive types map to layered scopes
- Original prompt: "improving her own memory and the amplihack memory-lib as one of her top priorities"

### Feral Tests
- confidence > 1.0 or < 0.0 → rejection
- Empty concept → rejection
- 10MB content → rejection
- consolidate with < batch_size episodes → null result
- Two processes sharing LadybugDB with different agent_name → isolation verified

## Phase 2: Knowledge Graph Integration

### Goal
Simard can query domain knowledge packs for grounded answers with source citations.

### Scope
Read-only queries against existing packs. Pack building is Phase 9+ scope.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/knowledge_bridge.rs` | ≤350 | KnowledgeBridge wrapping BridgeTransport, query/list/install |
| `src/knowledge_context.rs` | ≤250 | Inject relevant knowledge into engineer loop planning phase |
| `python/simard_knowledge_bridge.py` | ≤350 | Python bridge wrapping KnowledgeGraphAgent |
| `tests/knowledge.rs` | ≤350 | Integration: query test pack, verify sourced answer |

### Spec Checkpoint
- Original prompt: "commit a structured understanding of the entire amplihack ecosystem to her memory"

## Phase 3: Real Base Type Adapter

### Goal
At least one adapter that actually invokes an LLM and does real engineering work.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/base_type_copilot.rs` | ≤400 | Real CopilotSdkAdapter: launch amplihack copilot via PTY, send objectives, parse responses |
| `src/base_type_turn.rs` | ≤300 | Turn execution: format objective + memory context → send → parse structured response |
| `src/base_type_harness.rs` | ≤350 | Real LocalHarnessAdapter wrapping Claude API or local model |
| `tests/base_type_live.rs` | ≤400 | Integration: launch real copilot session, send trivial task, verify structured output |

### Spec Checkpoint
- Architecture Pillar 1 (Terminal First)
- Original prompt: "launching amplihack in a virtual tty and *using it* interactively"

### Integration Checkpoint A (after Phases 1+2+3 merge)
"Simard runs a real engineer session that queries knowledge, stores memories, and persists across restart"

## Phase 4: Gym & Benchmark Integration

### Goal
Simard can measure her own capability via amplihack-agent-eval's progressive suite.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/gym_bridge.rs` | ≤400 | GymBackend trait, scenario listing, run/compare, result capture |
| `src/gym_scenarios.rs` | ≤350 | Scenario definitions: repo-exploration, documentation, safe-code-change |
| `src/gym_scoring.rs` | ≤300 | Score aggregation, regression detection, improvement tracking |
| `python/simard_gym_bridge.py` | ≤400 | Python bridge wrapping progressive_test_suite + long_horizon_memory eval |
| `tests/gym.rs` | ≤400 | Integration: run L1 scenario, verify structured score |

### Spec Checkpoint
- Architecture Pillar 4 (Benchmarks Drive Product Truth)
- Original prompt: "gym mode like in skwaq"

## Phase 5: Agent Composition & Identity

### Goal
Simard can compose multiple agent identities and spawn subordinate agents.

### Supervisor Protocol
- Parent ↔ subordinate communicate via shared LadybugDB hive (semantic facts)
- Progress reported as JSON in fact content with heartbeat_epoch
- Liveness: 3 stale heartbeats (>120s each) → kill + mark abandoned
- Crash recovery: parent inspects subordinate's episodic memory
- Retry: at most 2 retries per goal, then escalate
- Recursion limit: SIMARD_MAX_SUBORDINATE_DEPTH=3
- File isolation: each subordinate gets its own git worktree

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/agent_supervisor.rs` | ≤400 | Spawn, monitor heartbeat, kill, retry protocol |
| `src/agent_goal_assignment.rs` | ≤300 | Goal storage/retrieval via hive, progress polling |
| `src/identity_composition.rs` | ≤400 | CompositeIdentity nesting multiple IdentityManifest |
| `src/agent_roles.rs` | ≤300 | Role catalog: planner, engineer, reviewer, facilitator |
| `tests/composition.rs` | ≤400 | Integration: spawn subordinate with goal, verify report |

### Integration Checkpoint B (after Phases 4+5 merge)
"Simard runs a gym scenario, delegates a subtask, both contribute to shared memory"

### Spec Checkpoint
- Architecture Pillar 8 (Identity and Runtime Are Different Things)
- Architecture Pillar 9 (Composition Must Outlive Topology)
- Original prompt: "composite agent identity consisting of collections of patterns"

## Phase 6: Self-Improvement Loop

### Goal
Simard can evaluate herself, propose improvements, and self-relaunch.

### Canary Protocol
1. Build new binary in canary target dir
2. Gate checks: smoke test (--version), unit tests, one gym L1 scenario, bridge health
3. All gates pass → semaphore handover (write ready file, SIGUSR1, load handoff, exit old)
4. Rollback: if new binary crashes within 60s, wrapper restarts previous known-good

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/self_improve.rs` | ≤400 | EVAL→ANALYZE→RESEARCH→IMPROVE→RE-EVAL→DECIDE loop |
| `src/self_relaunch.rs` | ≤350 | Build, gate checks, semaphore handover |
| `src/self_relaunch_gates.rs` | ≤250 | Individual gate implementations |
| `tests/self_improve.rs` | ≤400 | Integration: run improvement cycle on test scenario |

### Spec Checkpoint
- Original prompt: "continually be improving her own code" and "start a new Simard process, verify healthy, pass the torch"

## Phase 7: Remote Orchestration (azlin)

### Goal
Simard can spin up remote VMs and manage distributed agent sessions.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/remote_session.rs` | ≤400 | RemoteSessionManager: create VM, deploy, establish PTY, track |
| `src/remote_azlin.rs` | ≤350 | azlin CLI wrapper |
| `src/remote_transfer.rs` | ≤300 | Memory database replication, state migration |
| `tests/remote.rs` | ≤350 | Integration: mock azlin, verify session lifecycle |

### Spec Checkpoint
- Original prompt: "orchestrate a wide range of agentic coding sessions on remote VMs using azlin"

## Phase 8: Meeting Mode & Goal Curation

### Goal
Full meeting facilitator, persistent top-5 goals, dual identity management.

### Dual Identity Model
- CopilotAuth: GIT_CONFIG env vars pointing to rysweet_microsoft credential helper
- CommitAuth: GIT_AUTHOR/COMMITTER env vars for rysweet
- identity_auth.rs validates correct identity for each operation type
- Credential scrubber from amplihack-memory-lib enabled on bridge

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/meeting_facilitator.rs` | ≤400 | Interactive meeting mode |
| `src/goal_curation.rs` | ≤350 | Top-5 goals, backlog management, priority scoring |
| `src/research_tracker.rs` | ≤300 | Research topics, developer tracking |
| `src/identity_auth.rs` | ≤250 | Dual GitHub identity management |
| `tests/meeting.rs` | ≤350 | Integration: simulate meeting, verify decision capture |

### Integration Checkpoint C (after Phases 6+7+8 merge)
"Simard self-improves, reports to a meeting, persists goals across remote migration"

### Spec Checkpoint
- Original prompt: meetings, top-5 goals, developer tracking, dual identity

## Phase 9: OODA Loop & Continuous Operation

### Goal
Simard runs autonomously with explicit Observe→Orient→Decide→Act cycle.

### Modules

| Module | LOC Target | Purpose |
|--------|-----------|---------|
| `src/ooda_loop.rs` | ≤400 | Outer OODA cycle |
| `src/ooda_scheduler.rs` | ≤300 | Schedule concurrent activities across goals |
| `src/skill_builder.rs` | ≤350 | Build bespoke skills from procedural memory patterns |
| `tests/ooda.rs` | ≤400 | Integration: run one OODA cycle, verify action selection |

### Spec Checkpoint
- Original prompt: "always operate in her own independent and autonomous OODA loop"

## Module Budget Summary

| Phase | Modules | LOC Target | Cumulative |
|-------|---------|-----------|------------|
| 0 | 5 | ~1,400 | ~1,400 |
| 1 | 6 | ~2,150 | ~3,550 |
| 2 | 4 | ~1,300 | ~4,850 |
| 3 | 4 | ~1,450 | ~6,300 |
| 4 | 5 | ~1,850 | ~8,150 |
| 5 | 5 | ~1,800 | ~9,950 |
| 6 | 4 | ~1,400 | ~11,350 |
| 7 | 4 | ~1,400 | ~12,750 |
| 8 | 5 | ~1,650 | ~14,400 |
| 9 | 4 | ~1,450 | ~15,850 |
| **Total** | **46** | **~15,850** | |

## "Simard Builds Simard" Unlock Point

Minimum viable self-building requires Phases 0-6:
- Phase 0: Bridge infrastructure (communicate with ecosystem)
- Phase 1: Durable memory (remember across sessions)
- Phase 2: Knowledge (understand her ecosystem)
- Phase 3: Real adapter (invoke coding tools)
- Phase 4: Gym (measure improvement)
- Phase 5: Composition (delegate subtasks)
- Phase 6: Self-improvement (propose, test, deploy changes to herself)

Phases 7-9 add operational maturity but are not required for the core self-building loop.

## Original Prompt Coverage

| Requirement | Phase | Status |
|------------|-------|--------|
| Built on rustyclawd | 3 | Planned |
| Uses amplihack-memory-lib (6-type cognitive) | 1 | Planned |
| Launches amplihack in virtual TTY | Done | Shipped (PR #86) |
| Uses amplihack interactively | 3 | Planned |
| Structured understanding in memory | 1+2 | Planned |
| Top-5 goals, always pursuing | 8 | Planned |
| Track developer ideas | 8 | Planned |
| Backlog curation | 8 | Planned |
| Remote VM orchestration (azlin) | 7 | Planned |
| Meeting mode | 8 | Planned |
| Gym mode (skwaq-style) | 4 | Planned |
| Self-improvement loop | 6 | Planned |
| Self-relaunch | 6 | Planned |
| Subordinate agents | 5 | Planned |
| Dual GitHub identity | 8 | Planned |
| Memory migration across machines | 7 | Planned |
| Skill building | 9 | Planned |
| Agent Identity composable template | 5 | Planned |
| Agent Runtime separate concern | Done | Shipped (RuntimeKernel + DI) |
| Agent Base Type abstraction | Done | Shipped (BaseTypeFactory trait) |
| Hive mind memory | 1 | Planned |
| Agent-kgpacks integration | 2 | Planned |
| Always improving code quality | 6+9 | Planned |
| OODA loop | 9 | Planned |
