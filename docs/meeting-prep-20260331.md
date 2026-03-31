# Simard Morning Briefing -- 2026-03-31

Prepared by: Simard (autonomous engineer)
Branch: `salvage/background-workflow-20260329`

---

## Meeting Agenda

| Slot | Topic | Time |
|------|-------|------|
| 1 | Status update | 2 min |
| 2 | Demo opportunities | 3 min |
| 3 | Discussion topics (decisions needed) | 10 min |
| 4 | Proposed action items | 5 min |

---

## 1. Self-Assessment

### Test Suite

| Metric | Value |
|--------|-------|
| Total tests | **568** (277 unit + 70 integration + remaining per-module) |
| Passing | **568** |
| Failing | **0** |
| Ignored | **0** |
| Clippy warnings | **0** |
| Compile warnings | **0** |

All 568 tests pass. Zero clippy warnings. Zero compiler warnings. The test
suite runs in approximately 5 seconds end-to-end.

### Quality Rating: **8 / 10**

**Why 8 and not higher:**

- The test suite is comprehensive for the internal domain logic, but nearly all
  tests exercise in-memory backends and mock bridges. There are no integration
  tests that actually spawn `amplihack` or hit a real cognitive memory database.
- Several modules significantly exceed the 400 LOC guideline (see Architecture
  Review below). The operator_commands split addressed the worst case but the
  root module still carries 1,180 LOC of dispatch and shared helpers.
- The OODA loop, gym, and skill builder are structurally complete but have never
  run against live infrastructure. They are tested at the unit level only.

**Why not lower:**

- 568 tests with 100% pass rate is substantial.
- Zero clippy / zero warnings on Rust 2024 edition is disciplined.
- Module boundaries are clean: each module owns its types, tests sit beside
  code, and the public API surface in `lib.rs` is explicit.
- Error handling is thorough (`SimardError` / `SimardResult` everywhere).
- The bridge abstraction (`BridgeTransport` trait) enables the mock-heavy test
  strategy and will make real integration straightforward.

---

## 2. Architecture Review

### Codebase Size

| | Count |
|-|-------|
| Source files | 65 `.rs` files |
| Total LOC | 26,716 |
| Public modules | 63 |
| Binary entry points | 3 (`simard`, `simard-gym`, `simard_operator_probe`) |

### Module Structure (top-level groupings)

| Domain | Modules | Combined LOC |
|--------|---------|-------------|
| **Operator CLI & commands** | `operator_cli`, `operator_commands` + 5 sub-modules | ~2,500 |
| **Runtime kernel** | `runtime`, `bootstrap`, `session`, `error` | ~2,600 |
| **Engineer loop** | `engineer_loop`, `terminal_session`, `terminal_engineer_bridge` | ~2,500 |
| **Copilot integration** | `copilot_task_submit`, `copilot_status_probe`, `base_type_copilot` | ~1,740 |
| **Memory system** | `memory`, `memory_bridge`, `memory_cognitive`, `memory_consolidation`, `memory_hive` | ~1,240 |
| **Identity** | `identity`, `identity_auth`, `identity_composition` | ~1,220 |
| **Gym & self-improvement** | `gym`, `gym_bridge`, `gym_scoring`, `self_improve`, `improvements` | ~3,290 |
| **OODA loop** | `ooda_loop`, `ooda_actions`, `ooda_scheduler` | ~1,120 |
| **Agent composition** | `agent_program`, `agent_roles`, `agent_supervisor`, `agent_goal_assignment` | ~1,720 |
| **Goals & meetings** | `goals`, `goal_curation`, `meetings`, `meeting_facilitator` | ~1,380 |
| **Bridge infra** | `bridge`, `bridge_circuit`, `bridge_subprocess` | ~880 |
| **Knowledge** | `knowledge_bridge`, `knowledge_context` | ~530 |
| **Other** | `base_types`, `base_type_harness`, `base_type_turn`, `evidence`, `handoff`, `metadata`, `persistence`, `prompt_assets`, `reflection`, `remote_*`, `research_tracker`, `review`, `sanitization`, `self_relaunch`, `skill_builder` | ~5,900 |

### Modules Over 400 LOC

These modules exceed the project's 400-LOC soft limit. Some are justified
(test-heavy, domain-complex); others need further splitting.

| Module | LOC | Assessment |
|--------|-----|-----------|
| `runtime.rs` | 1,253 | Kernel + topology + supervisor + multiple session drivers. Split candidate. |
| `gym.rs` | 1,245 | Benchmark scenarios + suite runner + report types. Test-heavy; moderate split priority. |
| `copilot_task_submit.rs` | 1,238 | Complex copilot submission pipeline with many edge-case handlers. Split candidate. |
| `operator_commands.rs` | 1,180 | Post-split root still carries shared dispatch + legacy helpers. Needs another pass. |
| `terminal_session.rs` | 1,133 | PTY session management via `script(1)`. Inherently sequential; hard to split. |
| `engineer_loop.rs` | 1,079 | Bounded engineer loop with structured edits. Could extract verification. |
| `improvements.rs` | 1,057 | Improvement curation + approval pipeline. Split candidate. |
| `bootstrap.rs` | 843 | Startup validation, identity loading, runtime assembly. Moderate. |
| `agent_program.rs` | 830 | Three distinct program types (objective-relay, meeting, improvement). Natural split. |
| `base_types.rs` | 715 | Core base-type trait + session lifecycle. Justified. |
| `identity.rs` | 696 | Identity manifest + loader + contracts. Justified. |
| `ooda_actions.rs` | 499 | Six action dispatchers. Manageable. |

### operator_commands Split: Verdict

The split was successful in intent. The monolith was decomposed into 5
domain-specific sub-modules:

- `operator_commands_engineer.rs` (372 LOC)
- `operator_commands_terminal.rs` (332 LOC)
- `operator_commands_meeting.rs` (336 LOC)
- `operator_commands_gym.rs` (132 LOC)
- `operator_commands_review.rs` (130 LOC)

However, the root `operator_commands.rs` still holds 1,180 LOC of shared
dispatch logic, legacy probe helpers, and bootstrap/handoff plumbing. A second
pass should extract the shared utilities into a `operator_commands_common.rs`
or push dispatch routing into `operator_cli.rs`.

---

## 3. Ecosystem Survey

### amplihack (Python framework)

- **Repo**: `/home/azureuser/src/amplihack/`
- **Version**: 0.6.100
- **Recent activity**: Active (last commits: recursion test fixes, copilot wrapper self-containment)
- **State**: Mature framework with CLI (`cli.py` at 75K LOC), agent subsystem, fleet management, eval harness, skills/recipes system, and GitHub Copilot SDK integration.
- **Integration with Simard**: Direct. Simard's `terminal_session.rs` drives amplihack via PTY (`script(1)`). The `copilot_task_submit.rs` and `copilot_status_probe.rs` modules interface with amplihack's copilot mode. The `bridge_subprocess.rs` spawns amplihack bridge scripts as child processes.

### amplihack-memory-lib (Cognitive memory)

- **Repo**: `/home/azureuser/src/amplirusty/amplihack-memory-lib/`
- **Version**: Referenced as `v0.2.0` in amplihack's dependencies
- **State**: Python library with 10 modules: `cognitive_memory.py`, `connector.py`, `semantic_search.py`, `pattern_recognition.py`, `store.py`, `experience.py`, `memory_types.py`, `security.py`, `exceptions.py`.
- **Integration with Simard**: Simard's `memory_bridge.rs` defines a typed Rust client for the 6-type cognitive memory system (sensory, working, episodic, semantic, procedural, prospective). The bridge calls the Python library via `bridge_subprocess.rs` JSON-RPC. The types in `memory_cognitive.rs` mirror the Python types.

### agent-kgpacks (Knowledge graph packs)

- **Repo**: `/home/azureuser/src/agent-kgpacks/`
- **Version**: Post-evaluation (48 packs, 99% accuracy benchmark)
- **Recent activity**: Docs/UX overhaul, branch cleanup (last: Mar 8)
- **State**: Mature. 20+ domain packs built (anthropic-api, claude-agent-sdk, github-copilot-sdk, go, rust, python, docker, etc.). Evaluation framework shows 99% accuracy vs. 91.7% baseline.
- **Integration with Simard**: Simard's `knowledge_bridge.rs` and `knowledge_context.rs` define typed Rust clients for querying packs and enriching planning context. The OODA loop's `ResearchQuery` action dispatches through this bridge.

---

## 4. Gap Analysis vs. Original Prompt

### Drive amplihack interactively via PTY

**Status: Infrastructure complete, end-to-end functional for bounded sessions.**

- `terminal_session.rs` (1,133 LOC) implements PTY interaction via `script(1)`.
- `engineer_loop.rs` (1,079 LOC) runs bounded engineer sessions with structured edits.
- The copilot pipeline (`copilot_task_submit.rs`) handles task submission.
- 70 integration tests verify the terminal/engineer/copilot roundtrips.
- **Gap**: Sessions are bounded (fire-and-forget). There is no persistent interactive PTY that Simard maintains across OODA cycles. The terminal session opens, runs steps, captures transcript, and closes. True "interactive driving" where Simard observes amplihack's output and reacts adaptively mid-session requires the OODA loop to actually orchestrate sequential terminal turns -- which is wired but untested against live infrastructure.

### Remember across sessions using cognitive memory

**Status: Infrastructure complete, bridge typed, not tested end-to-end with live Kuzu database.**

- `memory_bridge.rs` (358 LOC) provides a typed Rust client for all 6 cognitive memory types.
- `memory_consolidation.rs` (299 LOC) implements the intake/preparation/execution/persistence/reflection lifecycle.
- `memory_hive.rs` (174 LOC) configures memory policy from identity manifests.
- **Gap**: All tests use `InMemoryBridgeTransport`. No test spawns the actual Python `amplihack-memory-lib` bridge server and performs a real store/recall cycle. The memory system will work if the bridge protocol matches -- but that has not been verified.

### Track developer ideas (ramparte, simonw, steveyegge, bkrabach, robotdad)

**Status: Data model complete, persistence via cognitive memory bridge, no live fetch pipeline.**

- `research_tracker.rs` (357 LOC) defines `DeveloperWatch` with `github_id` and `focus_areas`.
- `track_developer()` stores watches as semantic facts via the memory bridge.
- `load_research_topics()` recalls topics from cognitive memory.
- **Gap**: There is no scheduled job or OODA action that actually fetches public activity (GitHub repos, blog posts, tweets) for the watched developers. The tracker records *that* a developer should be watched and *what* their focus areas are, but does not scrape or poll anything. This requires a new `FetchDeveloperActivity` action kind in the OODA loop, plus a web-scraping or GitHub API integration.

### Maintain a backlog and top-5 goals

**Status: Fully functional in-process; persisted via cognitive memory.**

- `goal_curation.rs` (404 LOC) implements `GoalBoard` with `active` (max 5) and `backlog` lists.
- `goals.rs` (416 LOC) provides `GoalStore` trait with file-backed and in-memory implementations.
- `add_active_goal`, `add_backlog_item`, `promote_to_active`, `archive_completed`, `update_goal_progress` -- all tested.
- The OODA loop loads the goal board at each cycle start.
- **Gap**: Minimal. The goal system is one of the most complete subsystems. The only gap is that goal priority/re-ranking is manual (operator sets it). Automatic priority adjustment based on OODA observations is wired in `orient()` but uses simple heuristics.

### Self-improve through gym benchmarks

**Status: Infrastructure complete, never run against live amplihack.**

- `gym.rs` (1,245 LOC) defines benchmark scenarios, suite runner, and report types.
- `gym_scoring.rs` (299 LOC) aggregates scores, detects regressions, tracks improvement trends.
- `self_improve.rs` (392 LOC) implements the Eval->Analyze->Research->Improve->ReEval->Decide cycle.
- `gym_bridge.rs` (300 LOC) provides a typed Rust client for the amplihack eval bridge.
- The OODA loop's `RunGymEval` and `RunImprovement` actions dispatch through these.
- **Gap**: The gym scenarios are defined but the bridge server that connects to amplihack's eval harness has not been built. `GymBridge` is a trait with an `InMemoryGymBridge` for tests. The real `SubprocessGymBridge` would need a Python script (`simard_gym_bridge.py`) that runs amplihack eval scenarios and returns JSON results. This script does not exist yet.

### Build bespoke skills

**Status: Pipeline defined, never triggered from live procedural memory.**

- `skill_builder.rs` (312 LOC) extracts skill candidates from procedural memory, generates markdown skill definitions, and installs them to a target directory.
- The OODA loop's `BuildSkill` action dispatches through `extract_skill_candidates()`.
- **Gap**: Requires procedural memory to have entries with `usage_count >= 3`. Since cognitive memory has not been exercised live, there are no procedures to extract from. The pipeline is correct but dormant.

### Operate in an autonomous OODA loop

**Status: Logic complete and unit-tested. Never run as a long-lived process.**

- `ooda_loop.rs` (330 LOC) implements `observe`, `orient`, `decide`, `act`, `run_ooda_cycle`.
- `ooda_actions.rs` (499 LOC) dispatches 6 action kinds against live bridges.
- `ooda_scheduler.rs` (287 LOC) manages concurrency slots and draining.
- **Gap**: There is no `main` loop that calls `run_ooda_cycle` repeatedly. The current `main.rs` dispatches to `dispatch_operator_cli` (a single CLI command). A persistent daemon mode (`simard daemon` or `simard ooda`) that enters the OODA loop and runs continuously does not exist. This is probably a 50-100 LOC addition to `operator_cli.rs` and `main.rs`, plus a signal handler for graceful shutdown.

---

## 5. Summary Table

| Capability | Data Model | Unit Tests | Bridge Wired | End-to-End Tested | Live Ready |
|-----------|:---------:|:---------:|:-----------:|:----------------:|:----------:|
| PTY / amplihack driving | Yes | Yes | Yes | Partial | No |
| Cognitive memory | Yes | Yes | Yes | No | No |
| Developer tracking | Yes | Yes | Yes | No | No |
| Backlog / top-5 goals | Yes | Yes | Yes | N/A (in-process) | Close |
| Gym benchmarks | Yes | Yes | Partial | No | No |
| Skill builder | Yes | Yes | Yes | No | No |
| OODA loop | Yes | Yes | Yes | No | No |

---

## 6. Proposed Next Steps

### Priority 1: OODA Daemon Mode (High impact, low effort)

**What**: Add a `simard ooda` subcommand that enters a loop calling
`run_ooda_cycle` with configurable interval and graceful shutdown on SIGTERM.

**Why**: This is the single missing piece that turns Simard from a CLI tool
into an autonomous agent. All the cycle logic exists and is tested; it just
needs a loop wrapper and a real entry point.

**Effort**: ~1 day. 50-100 LOC in `operator_cli.rs`.

**Risk**: Low. The cycle already handles bridge failures gracefully (honest
degradation). The daemon can start with all-mock bridges and progressively
connect real ones.

### Priority 2: Live Memory Bridge Verification (High impact, medium effort)

**What**: Write a Python bridge server script (`simard_memory_bridge.py`) that
wraps `amplihack-memory-lib` and speaks the JSON-RPC protocol that
`bridge_subprocess.rs` expects. Run the existing Rust tests against it.

**Why**: Memory is the substrate for everything else: goal persistence, research
tracking, skill extraction, improvement history. Until the bridge actually works
end-to-end, every subsystem that depends on memory is theoretical.

**Effort**: ~2 days. The Rust side is complete. The Python side needs a thin
stdin/stdout JSON-RPC wrapper around `CognitiveMemory`.

**Risk**: Medium. The wire protocol may have mismatches between what the Rust
`CognitiveMemoryBridge` sends and what the Python library expects. This is
exactly why this needs to be tested before anything else goes live.

### Priority 3: Gym Bridge Server (Medium impact, medium effort)

**What**: Build `simard_gym_bridge.py` that runs amplihack eval scenarios and
returns structured results matching the `GymBridge` trait contract.

**Why**: Without gym evaluation, the self-improvement loop cannot measure
whether changes help or hurt. The improvement cycle is the mechanism for
Simard to get better autonomously.

**Effort**: ~2-3 days. Needs to interface with amplihack's eval harness.

**Risk**: Medium. Depends on amplihack eval harness stability.

### Risks and Blockers

1. **Bridge protocol mismatch**: The Rust-to-Python bridge protocol has not been
   verified end-to-end. If there are serialization differences, multiple
   subsystems break simultaneously.
2. **amplihack-memory-lib v0.2.0 compatibility**: The memory lib is pinned at
   v0.2.0. If the API has changed since the Rust types were written, the bridge
   will fail.
3. **Large modules**: 8 modules exceed 700 LOC. Further splitting would reduce
   cognitive load for future changes, but is not a blocker.

### What Simard Can Do Autonomously Right Now

- Run bounded engineer sessions against any repo (PTY-based, with structured edits)
- Run bounded terminal sessions with step/wait-for scripting
- Manage goal boards (add, promote, archive, update progress)
- Run meetings (facilitator mode with notes, decisions, action items)
- Run improvement curation (review proposals, approve/reject)
- Run the full gym benchmark suite (against mock scenarios)
- Perform code reviews

### What Needs Human Guidance

- **Bridge server scripts**: Need to be written and tested (Python work, not Rust)
- **Daemon operational parameters**: Cycle interval, concurrency limits, shutdown policy
- **Developer watch list**: Which specific developers to track and what to look for
- **Knowledge pack selection**: Which kgpacks to install for Simard's domain focus

---

## 7. Demo Opportunities

The following can be demonstrated working today:

1. **`cargo test` -- 568 tests, zero failures.** Clean and fast. Shows the
   depth of the test suite.

2. **`simard engineer run`** -- Run a bounded engineer session against the
   Simard repo itself. Shows PTY driving, repo inspection, structured edits,
   and transcript capture.

3. **`simard goal-curation`** -- Show the goal board with active goals, backlog,
   and progress tracking. Add a new goal, promote from backlog.

4. **`simard meeting`** -- Run a facilitated meeting session with notes,
   decisions, and action items.

5. **`simard gym suite`** -- Run the benchmark suite (against mock scenarios)
   and show the scoring/regression detection output.

---

## 8. Discussion Topics (Decisions Needed)

1. **Should we prioritize OODA daemon mode or live bridge verification first?**
   The daemon is faster to build but runs on mocks. The bridge is more work but
   unlocks real autonomy. Recommendation: bridge first, daemon second, so the
   daemon has something real to drive.

2. **Module size policy**: 8 modules exceed 700 LOC. Do we enforce the 400 LOC
   guideline strictly (split them now) or accept the current sizes as technical
   debt and focus on functionality?

3. **Developer tracking scope**: The `research_tracker.rs` data model supports
   arbitrary developers. Which of the five named developers (ramparte, simonw,
   steveyegge, bkrabach, robotdad) should be prioritized, and what constitutes
   "tracking" -- GitHub activity only, or blog/social media too?

4. **Knowledge pack integration**: agent-kgpacks has 20+ packs. Which ones
   should Simard load by default? Recommendation: `rust-expert` and
   `claude-agent-sdk` as the two most relevant to Simard's own development.

---

## 9. Proposed Action Items

| # | Action | Owner | Priority |
|---|--------|-------|----------|
| 1 | Write `simard_memory_bridge.py` bridge server | Human + Simard | P0 |
| 2 | Run memory bridge integration test (Rust <-> Python) | Simard | P0 |
| 3 | Add `simard ooda` daemon subcommand | Simard | P1 |
| 4 | Write `simard_gym_bridge.py` bridge server | Human + Simard | P1 |
| 5 | Split `operator_commands.rs` shared helpers (second pass) | Simard | P2 |
| 6 | Split `runtime.rs` into kernel + topology + drivers | Simard | P2 |
| 7 | Add developer activity fetch to OODA actions | Simard | P2 |
| 8 | Install `rust-expert` and `claude-agent-sdk` kgpacks | Human | P2 |
