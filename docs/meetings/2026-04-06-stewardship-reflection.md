# Meeting: Amplihack Ecosystem Stewardship Reflection

**Date:** 2026-04-06
**Participants:** Operator (Ryan), Simard
**Topic:** Current status review, capabilities assessment, and reflection on what Simard needs to steward the amplihack ecosystem
**Format:** Structured meeting (REPL unavailable — Issue #207)

---

## 1. Current Status Review

### What shipped this sprint (PRs #194–#209)

| PR | Summary |
|----|---------|
| #194 | Distributed runtime IPC |
| #195 | Self-relaunch semaphore |
| #196–#202 | Test coverage push (553→2050+ tests, 39%→65%+) |
| #203 | Meeting REPL Copilot wiring + conversational tone |
| #204 | Config-driven LLM provider selection (v0.14.0) |
| #205 | Split all 36 oversized modules (v0.14.1) |
| #206 | Quality test suite + hardening (v0.14.2) |
| #208 | Mandatory cognitive memory + Kuzu→LadybugDB rename (v0.15.0) |
| #209 | Remove SQLite memory store + backward-compat deserialization (pending CI) |

**Current version:** 0.15.0
**Test count:** 2002 passing, 0 failing

### Simard's Current Capabilities

**Core identity:** An autonomous engineer built in Rust that drives agentic coding systems through real terminal interaction. Named after Suzanne Simard (mycorrhizal network research).

**Operational capabilities:**
- Engineer loop: PTY-backed terminal sessions, bounded execution, Copilot submission
- Meeting system: structured meetings with decisions/actions/notes, durable handoff, memory persistence
- Goal stewardship: durable goal board, active top-5, backlog, progress tracking
- Review + self-improvement: benchmark-driven improvement cycles, relaunch gating
- Gym / evaluation: benchmark scenarios, suites, regression detection
- OODA loop: observe-orient-decide-act cycle with scheduler
- Agent composition: spawn/supervise subordinates, assign goals, monitor heartbeats
- Memory: 6-type cognitive psychology model (sensory, working, episodic, semantic, procedural, prospective) backed by LadybugDB cognitive bridge
- Remote orchestration: azlin VM/session/transfer support

**Known issues:**
- Meeting REPL PTY bug (Issue #207) — `simard meeting` hangs when piped
- `ladybug-graph-rs` upstream is still a stub — native LadybugDB backend not yet functional
- `rusqlite` still in Cargo.toml for gym_history (separate from memory)

---

## 2. The Amplihack Ecosystem

### Core repos

| Repo | Purpose | Status |
|------|---------|--------|
| `amplihack-rs` | Rust core runtime — 23 crates, CLI, hooks, recipes, fleet, memory | **Ready for early adopters** |
| `amplihack-memory-lib` | Python+Rust cognitive memory system, LadybugDB graph | Active |
| `amplihack-recipe-runner` | YAML recipe parsing/execution engine | Active |
| `amplihack-traits` | Shared trait definitions | Active |
| `amplihack-xpia-defender` | Prompt injection defense | Active |
| `amplihack` | Original Python framework (being superseded by -rs) | Legacy/Active |
| `Simard` | Autonomous engineer agent | Active (v0.15.0) |

### Supporting repos
- `rustyclawd` — Rust Claude API wrapper (used by Simard for LLM access)
- `azlin` — Remote VM orchestration
- `seldon` — (adjacent tooling)
- `nation` — (adjacent tooling)

### amplihack-rs crate map (23 crates)
- **Foundation:** types, state, utils, safety, context, recovery
- **Security:** security (XPIA defense)
- **Orchestration:** workflows, hooks, cli, launcher, recipe, fleet, delegation
- **Memory:** memory (SQLite + LadybugDB backends, bloom filters, transfer/export)
- **Agents:** agent-core, agent-eval, domain-agents, hive, agent-generator
- **Code intelligence:** blarify, multilspy
- **Remote:** remote

---

## 3. Simard's Reflection: What Do I Need for Ecosystem Stewardship?

### What "stewardship" means

Stewardship of the amplihack ecosystem means Simard takes responsibility for:
1. **Health monitoring** — CI status, dependency drift, breaking changes across repos
2. **Cross-repo coordination** — when a change in amplihack-rs affects Simard, amplihack-memory-lib, or downstream consumers
3. **Quality gates** — ensuring PRs meet standards before merge
4. **Issue triage** — identifying, categorizing, and prioritizing issues across repos
5. **Release management** — version bumps, changelogs, compatibility matrices
6. **Onboarding** — helping new contributors and users get started with amplihack-rs

### What Simard needs to know

#### A. Repository access and awareness
- [ ] **Read access to all ecosystem repos** — currently Simard only operates on her own repo. She needs the ability to clone, read, and understand amplihack-rs, amplihack-memory-lib, amplihack-recipe-runner, etc.
- [ ] **Cross-repo dependency graph** — which crates depend on which, what versions are pinned where, what breaks when something changes
- [ ] **CI/CD pipelines for each repo** — how to check status, trigger builds, read results

#### B. Architectural knowledge
- [ ] **amplihack-rs crate architecture** — the 23-crate workspace, how they compose, public API surfaces
- [ ] **Recipe system** — how YAML recipes work, how to write/test/debug them
- [ ] **Hook system** — PreToolUse, PostToolUse, UserPromptSubmit hooks and their contracts
- [ ] **Fleet orchestration** — how multi-agent coordination works
- [ ] **Memory model parity** — ensuring the cognitive memory model in Simard matches amplihack-memory-lib exactly

#### C. Operational capabilities Simard currently lacks
- [ ] **Multi-repo git operations** — Simard can only operate in one repo at a time. Stewardship requires cross-repo awareness.
- [ ] **GitHub API integration** — reading issues, PRs, CI status across multiple repos without manual CLI commands
- [ ] **Dependency version tracking** — automated detection of version drift between repos
- [ ] **Release automation** — creating releases, publishing crates, updating downstream consumers
- [ ] **Working meeting REPL** — Issue #207 blocks Simard's ability to have real-time discussions with operators

#### D. Knowledge gaps to fill
- [ ] **amplihack-rs user journey** — what does "trying amplihack-rs" look like for a new user? What are the rough edges?
- [ ] **Migration path from Python amplihack** — what still needs porting, what's deprecated, what's the recommended path?
- [ ] **LadybugDB roadmap** — when will ladybug-graph-rs have a real implementation? What's the timeline for replacing Kuzu everywhere?
- [ ] **Security model** — XPIA defense, prompt injection guardrails across the ecosystem
- [ ] **Hive orchestration** — multi-agent coordination patterns, when to use fleet vs hive

### What Simard needs to do first

#### Phase 1: Learn the ecosystem (Investigation)
1. Clone and build amplihack-rs locally
2. Run its test suite, understand its CI
3. Read the recipe system documentation
4. Map the dependency graph between all repos
5. Identify which amplihack-rs features Simard already uses vs. doesn't

#### Phase 2: Establish monitoring (Operational)
1. Set up cross-repo health checks (CI status, latest release dates)
2. Create a dependency compatibility matrix
3. Add ecosystem status to Simard's OODA observation loop
4. Wire GitHub issue tracking across repos into goal stewardship

#### Phase 3: Active stewardship (Ongoing)
1. Review and triage issues across ecosystem repos
2. Detect and report breaking changes
3. Help coordinate releases
4. Guide new users through onboarding
5. Propose and execute cross-repo improvements

---

## Decisions

/decision Simard will pursue ecosystem stewardship in 3 phases: learn → monitor → actively steward. Phase 1 starts immediately.

/decision The meeting REPL (Issue #207) is a blocker for real-time Simard conversations and must be prioritized.

/decision amplihack-rs is ready for early adopters. Simard should test the onboarding flow herself as a validation exercise.

---

## Action Items

/action Simard: Clone and build amplihack-rs, run tests, document the experience (Phase 1 learning)
/action Simard: Map the cross-repo dependency graph for all amplihack ecosystem repos
/action Simard: Fix Issue #207 (meeting REPL PTY bug) to enable real-time meetings
/action Simard: Try the amplihack-rs onboarding flow as a new user and report friction points
/action Operator: Confirm which repos Simard should have write access to for stewardship

---

## Notes

- The cognitive memory model is now fully wired (6-type model, mandatory bridge, LadybugDB naming)
- SQLite is removed from the memory system (only gym_history still uses rusqlite)
- The other Copilot session running on this VM keeps switching git branches, causing interference. Need to coordinate or isolate sessions.
- amplihack-rs has 23 crates — this is a substantial codebase to learn. Simard should prioritize the crates she already depends on (amplihack-memory, amplihack-hooks, amplihack-recipe).
