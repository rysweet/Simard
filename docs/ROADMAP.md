# Simard Roadmap

> Vision-to-implementation gap analysis and prioritized development plan.
> Generated from comparison of `Specs/ProductArchitecture.md` against current codebase state.

## Status Legend

| Symbol | Meaning |
|--------|---------|
| ✅ | Fully implemented and tested |
| 🔧 | Partially implemented — seams exist, needs completion |
| ❌ | Not yet started |

## Current State (as of PR #379)

### Fully Implemented ✅

| Feature | Description | Key Files |
|---------|-------------|-----------|
| Cognitive Memory Bridge | 6-type memory (episodic, semantic, procedural, metacognitive, sensory, working) | `src/cognitive_memory_bridge/` |
| Base Types & PTY | ooda-copilot, pty-spawn, tool-call; composable agent primitives | `src/base_types/` |
| Engineer Session Loop | Multi-turn engineer sessions with handoff and review | `src/engineer_session/` |
| Gym & Evaluation | 40+ scenario suite across 8 categories with scoring | `src/gym/` |
| Identity & Supervision | Agent naming, hierarchy, operator supervision | `src/agent_supervisor/` |
| Meeting Facilitator | Interactive REPL, goal extraction, handoff, review | `src/meeting_facilitator/` |
| OODA Daemon | Observe-Orient-Decide-Act loop with health reporting | `src/operator_commands_ooda/` |
| Goal Curation | Priority-ranked goal board, backlog scoring, stewardship | `src/goal_curation/` |
| Dashboard v2 | WebSocket chat, logs, processes, memory, auth, issues | `src/operator_commands_dashboard/` |
| Remote Session | azlin-based remote VM orchestration with lifecycle | `src/remote_session.rs`, `src/remote_azlin.rs` |
| Self-Metrics | Metric recording, daily reports, cost tracking | `src/self_metrics/`, `src/cost_tracking/` |
| Ensure-Deps | Auto-install runtime dependencies (git, python3, gh, etc.) | `src/cmd_ensure_deps.rs` |
| Cleanup Command | Resource reclamation (canary dirs, stale targets, orphans) | `src/cmd_cleanup.rs` |
| Test Coverage | 3,177 tests across 238 source files (100% file coverage) | `tests/` |

### Partially Implemented 🔧

| Feature | Current State | Remaining Work | Priority |
|---------|--------------|----------------|----------|
| Distributed Topology | Dev VM + Simard VM; single-process spawn | Multi-host process routing, load balancing, automatic failover | P1 |
| Dashboard Telemetry | PR #379 adds distributed/goals/costs tabs | Gym score trends, agent heartbeat timeline, memory layer breakdown | P1 |
| Self-Improvement | Offline curation + mediated proposals | Autonomous proposal-apply-verify loop with safety gates | P2 |
| Extra Base Types | claude-agent-sdk, ms-agent-framework modules exist | Not wired as default base types; need integration tests | P3 |

### Not Yet Implemented ❌

| Feature | Description | Depends On | Priority |
|---------|-------------|------------|----------|
| Multi-Host Distributed Product | True distributed Simard across N hosts with consensus | Distributed topology completion | P1 |
| Full Dashboard Telemetry | Gym score trends over time, benchmark regression alerts | Gym data persistence | P2 |
| Runtime Repo Split | Separate `simard-core` crate from plugins/extensions | Module boundary cleanup | P3 |
| Autonomous Self-Improvement | Auto-apply improvement proposals with rollback | Safety verification, test-gating | P2 |
| Graph Memory Queries | LadybugDB/Kuzu graph traversal for relationship discovery | Memory bridge refactor | P3 |
| Agent Registry | Track all Simard processes (local + remote) with health | Issue #296 | P2 |
| Memory Backup | Automated daily backup with verification | Issue #298 | P3 |

## Development Phases

### Phase 1: Distributed Visibility (Current — Q3 2025)

**Goal**: Full operational visibility into distributed Simard fleet.

- [x] PR #379 — Dashboard distributed panel, goals tab, costs tab
- [ ] Agent heartbeat timeline panel (remote agents report health periodically)
- [ ] Gym score trends chart (store scores in state root, render time-series)
- [ ] Memory layer breakdown (show per-type counts: episodic, semantic, etc.)
- [ ] Cross-VM log aggregation (pull remote daemon logs into dashboard)

### Phase 2: Autonomous Operations (Q3-Q4 2025)

**Goal**: Simard manages herself with minimal operator intervention.

- [ ] Agent registry (#296) — track local + remote Simard processes
- [ ] Autonomous self-improvement loop — propose → test → apply → verify
- [ ] Automated cleanup scheduling (cron-like or OODA-triggered)
- [ ] Memory backup with verification (#298)
- [ ] Resource limit enforcement (cargo process caps, disk thresholds)

### Phase 3: Multi-Host Fleet (Q4 2025)

**Goal**: True N-host distributed Simard with task routing and consensus.

- [ ] Task routing — dispatch work items to best-fit VM based on load/capabilities
- [ ] Cross-VM consensus — coordinate goal priorities across hosts
- [ ] Automatic failover — detect unhealthy VMs, reassign work
- [ ] Distributed memory consolidation — merge memory graphs across hosts

### Phase 4: Ecosystem Hardening (Q1 2026)

**Goal**: Production-ready Simard with external integrations.

- [ ] Runtime repo split — `simard-core` + `simard-plugins`
- [ ] Graph memory queries via LadybugDB
- [ ] Plugin SDK for third-party base types
- [ ] Benchmark regression CI (fail PRs that degrade gym scores)

## Open Issues by Category

### Distributed Infrastructure
- #373 — ops: system resource management
- #296 — feat: agent registry
- #251 — feat: distributed memory architecture

### Meeting & UX
- #311 — feat: meeting UX enhancements
- #287 — fix: meeting REPL reliability (PR #333)

### Amplihack Framework (upstream)
- Shell quoting bugs: amplihack #4287, #4283, #4252
- Issue extraction: amplihack #4286, #4267, #4253
- Condition parser: amplihack #4282, #4274, #4270
- Branch handling: amplihack #4289, #4288, #4254

## References

- [Product Architecture](https://github.com/rysweet/Simard/blob/main/Specs/ProductArchitecture.md)
- [Implementation Plan](https://github.com/rysweet/Simard/blob/main/Specs/IMPLEMENTATION_PLAN.md)
- [Dashboard PR #379](https://github.com/rysweet/Simard/pull/379)
- [Roadmap Issue #380](https://github.com/rysweet/Simard/issues/380)
