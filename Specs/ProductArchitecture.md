# Simard Product and Architecture Specification v1

## Document Status

This is the first canonical product and architecture reference for Simard.
It is intentionally opinionated, directional, and lightweight enough to guide early implementation without pretending the system is already fully designed.

## Current Codebase Context

Simard now contains an initial Rust architecture scaffold organized around explicit contracts for prompt assets, base-type factories and sessions, identity manifests, runtime control, topology services, memory, evidence, and reflection.
The codebase is still early, but it is no longer a blank crate.
This specification therefore serves two jobs at once: it defines the intended operating model and it sets the architectural constraints that the scaffold must keep preserving as implementation breadth expands.

## Terminology

The document uses the following terms with strict meanings:

- **Prompt Asset**: a file-based prompt or prompt fragment that is versioned separately from runtime code.
- **Agent Base Type**: the backend execution substrate an identity builds on, such as RustyClawd or a Copilot/Claude SDK integration, normalized through a Simard base-type factory/session contract.
- **Agent Identity**: the durable definition of an agent's purpose, prompts, policies, composition, and allowed behaviors.
- **Agent Runtime**: the control plane that instantiates identities, runs sessions, manages lifecycle, and wires components together.
- **Role**: a sub-function inside an identity, such as engineer, reviewer, or facilitator.
- **Mode**: a user-facing operating state, such as engineer mode or meeting mode.
- **Session**: one bounded run through intake, planning, execution, reflection, and persistence.
- **Recipe / Workflow**: a reusable orchestration pattern used by an identity or runtime.

## Mission

Simard exists to become a terminal-native engineering system that can operate like a disciplined software engineer instead of a generic chat assistant.
Its job is to understand a codebase, work through tasks in explicit sessions, preserve useful memory, evaluate itself against benchmark tasks, and improve through structured review loops.

The product goal for v1 is not "general artificial intelligence."
The goal is a reliable engineer-in-the-terminal that can reason over a repository, execute bounded work, explain its actions, and learn from repeated benchmarked runs.

## Scope

Version 1 includes the smallest coherent product that proves Simard's operating model.

- A terminal-first interaction model where the primary execution surface is a shell-driven engineering session.
- Explicit session orchestration with a clear lifecycle: intake, planning, execution, reflection, and persistence.
- A memory system that separates durable project knowledge from short-lived session state.
- Identity separation between system control roles so planning, execution, review, meeting facilitation, and goal stewardship do not collapse into a single undifferentiated voice.
- A benchmark gym that exercises Simard against repeatable engineering tasks and records outcomes.
- A meeting mode for alignment, architecture discussion, and decision capture without conflating that mode with implementation mode.
- A self-improvement loop that uses benchmark results and session reviews to propose targeted changes to prompts, policies, or runtime behavior.
- A reusable separation between agent identity definition and agent runtime execution so Simard can be one identity in a broader family of specialized agents.
- A platform architecture split between prompt assets, agent base types, agent identities, and agent runtime.

## Version 1 Delivery Boundary

The long-term vision is broader than the first shippable version.
For delivery purposes, v1 should be interpreted narrowly:

- one primary engineer loop
- one default local single-process deployment path, plus an explicit loopback multi-process operator path for supported base types
- four builtin manifest-advertised base-type selections in the local scaffold: `local-harness`, `terminal-shell`, `rusty-clawd`, and `copilot-sdk`, with `terminal-shell` now providing a local PTY-backed shell path for the engineer identity, `rusty-clawd` behaving as a distinct session backend, and `copilot-sdk` remaining an explicit alias of the local harness implementation
- one durable memory path
- one small benchmark set

Broader distributed execution, richer sibling identities, and the full self-improvement loop remain part of the architecture direction, but they are not v1 ship blockers. The current scaffold now includes honest meeting-mode and goal-stewardship slices because they proved necessary to support the core engineer loop.

Phased delivery does **not** permit a local-only or single-base-type architecture.
From day one, the runtime must be written for dependency injection, topology-aware composition, and multiple base types even if the first runnable path is local and only a single-process harness implementation ships underneath the initial builtin selections.

## Day-One Architectural Constraints

The following are hard constraints for the first implementation, not deferred aspirations:

- **Dependency injection from the outset**: runtime composition must happen through explicit ports, traits, and typed configuration rather than hidden globals or direct construction buried inside the core loop.
- **Distributed-readiness from the outset**: runtime, session, memory, and reflection contracts must not assume in-process execution even when the first deployment path is single-process.
- **Multiple base types from the outset**: identity manifests and runtime selection logic must support multiple base types immediately. In the current v1 scaffold, `local-harness`, `terminal-shell`, `rusty-clawd`, and `copilot-sdk` are all selectable builtin base types for the engineer identity. `terminal-shell` is a local PTY-backed shell path, `rusty-clawd` is a distinct session backend, and `copilot-sdk` remains an explicit alias of the local single-process harness implementation.
- **Visible failures from the outset**: unsupported capabilities, missing prompt assets, invalid lifecycle transitions, and unsupported topologies must fail explicitly through typed errors instead of silent fallbacks.

## Non-Goals

The following are explicitly out of scope for v1.

- Building a general-purpose GUI product before the terminal workflow is solid.
- Supporting every programming language equally; Rust is the implementation language, but benchmark coverage can expand gradually.
- Autonomous long-running internet agents that roam without bounded task context.
- A fully automatic self-modifying system that changes itself without review.
- Perfect semantic memory or broad "remember everything forever" behavior.
- Multi-user enterprise collaboration features such as permissions, shared inboxes, or hosted tenancy.
- Detailed low-level plugin APIs before the core session, memory, and evaluation loops have proven value.
- Shipping full distributed infrastructure, remote transport, clustering, or cross-host scheduling in v1.
- Shipping every planned base-type backend integration in v1.
- Shipping sibling identities such as `Cumulona` or `Victoria` in v1.
- Automatic self-modification or autonomous promotion of changes in production paths.
- A marketplace or plugin ecosystem for third-party identities before the core loop is proven.

## Product Shape

Simard should be treated as a focused engineering runtime with five user-visible modes:

1. Engineer mode
   Simard accepts a concrete task, inspects the local repo, forms a bounded plan with explicit verification steps, executes through terminal actions, and reports outcomes with evidence. The shipped v1 slice now supports both read-only repo inspection actions and one narrow structured file edit on a clean repo.

   Under the `simard engineer ...` namespace, v1 now exposes two distinct operator-visible surfaces that must stay honest about their boundary:

   - `engineer terminal`, `engineer terminal-file`, `engineer terminal-recipe`, and `engineer terminal-read` are bounded local terminal session surfaces with checkpointed transcript audit
   - `engineer run` and `engineer read` are the separate repo-grounded engineer loop and its read-only audit companion

   Reusing the same explicit `state-root` bridges those surfaces through local persisted summaries only. That bridge must never imply hidden orchestration, automatic continuation, or unsupported external Copilot/amplihack execution.

2. Meeting mode
   Simard helps humans think, decide, and record architecture or planning outcomes, but does not silently drift into implementation without an explicit handoff.

3. Goal-curation mode
   Simard curates durable backlog state and an explicit active top 5 goals list without pretending implementation work happened.

4. Improvement-curation mode
   Simard consumes persisted review findings, requires explicit operator approval or deferral, and promotes accepted improvements into durable active or proposed priorities without mutating code.

5. Gym mode
   Simard runs controlled benchmark tasks to measure capability, regressions, and improvement over time.

These are not cosmetic personas.
They are different operating modes with different success criteria, memory writes, and allowed actions.

## Architecture Pillars

### 1. Terminal First, Not Chat First

Simard is built around the reality that engineering work happens through file inspection, commands, patches, tests, and artifacts.
The terminal is the primary execution surface, and conversational text exists to coordinate work, not replace the work.

### 2. Explicit State Over Hidden Magic

Every meaningful run should have explicit session metadata, a live task objective, a working memory area, and a durable output trail that stores sanitized objective metadata rather than raw task text.
If a future developer cannot explain why Simard took an action by inspecting session records, the architecture is too opaque.

That rule now applies across the terminal-to-engineer bridge too. If a bounded terminal session is later reused as continuity for engineer mode, the continuity must come from explicit local persisted artifacts under the same operator-chosen `state-root`, with mode-scoped handoff records and readback that shows what was reused. Any bridged terminal fields that survive into persisted handoff state or operator-visible readback must be sanitized before persist and sanitized again before render so control sequences, secret-shaped values, and raw objective text are not replayed as product truth.

### 3. Roles Must Be Separated

Planner, engineer, reviewer, facilitator, and goal-curation responsibilities should remain distinct even if they are implemented in one binary at first.
This avoids prompt collapse, makes failures diagnosable, and creates clear seams for future multi-agent execution.

### 4. Benchmarks Drive Product Truth

Simard should not be judged by demos alone.
The benchmark gym is part of the product architecture, not an afterthought, and capability claims should be tied to repeatable benchmark evidence.

### 5. Memory Must Be Layered

Not all memory deserves the same lifetime.
Simard should preserve durable knowledge deliberately, keep session scratch state isolated, and avoid polluting long-term memory with every transient thought.

### 6. Improvement Requires Reviewable Loops

Self-improvement should produce hypotheses, evidence, and proposed changes that can be reviewed.
The system should optimize for controlled iteration, not autonomous mutation.

### 7. Prompt Assets Stay Separate From Code

Prompts are part of the agent definition, but they should not be baked invisibly into application code.
Prompt files should live as explicit assets so they can be inspected, versioned, composed, replaced, and benchmarked independently of runtime logic.

### 8. Identity and Runtime Are Different Things

An agent identity defines what an agent is trying to be.
The runtime defines how that identity is instantiated, scheduled, connected, and recovered.
These concerns should remain separate so identities are portable across local, multi-process, and distributed deployments.

### 9. Composition Must Outlive Topology

Simard may be composed of prompts, skills, recipes, subordinate identities, and more deterministic tools.
That composition should not depend on whether those parts run in one process, several local processes, or across multiple hosts.

### 10. Dependency Injection Is Structural, Not Optional

Simard should not rely on hidden singletons, implicit globals, or local-only construction paths in its core runtime.
Prompt loading, base-type selection, memory access, evidence capture, and reflection should all be injected through explicit contracts.

This is not ceremony for its own sake.
It is what keeps the inner loop stable when the deployment shape changes from one local process to multiple workers or distributed hosts.

### 11. Honest Degradation Beats Hidden Fallback

When a requested capability, prompt asset, base type, or topology is unavailable, the runtime should fail visibly.
It may support alternate configurations through explicit selection, but it must not quietly downgrade behavior and pretend nothing changed.

## Benchmark Gym Strategy

The benchmark gym is how Simard learns whether it is getting better at real engineering work.
It should start with small, reproducible tasks and grow only when the harness is trustworthy.

### Gym Objectives

- Measure whether Simard can complete bounded software tasks end to end.
- Detect regressions in planning quality, execution reliability, and explanation quality.
- Compare changes to prompts, policies, memory strategies, and orchestration logic.
- Produce artifacts that humans can inspect, not just scalar scores.

### Initial Benchmark Classes

- Repo exploration tasks: identify structure, dependencies, and likely change points.
- Documentation tasks: create or update architecture and product docs from repository context.
- Safe code change tasks: small feature additions, bug fixes, or refactors with verification.
- Session quality tasks: produce a plan, execute coherently, and summarize evidence.

### Scoring Strategy

Each benchmark run should record:

- task completion status
- evidence quality
- correctness checks passed or failed
- unnecessary action count
- retry count
- human review notes when applicable

V1 should prefer a small benchmark set with high signal over a large, noisy suite.
The gym is successful when it changes engineering decisions, not when it creates dashboard theater.

## Interactive Terminal-Driven Engineer Behavior

Engineer mode is the heart of Simard.
It should behave like a careful engineer working in a shell, not like a narrator pretending to work.

### Required Behaviors

- Inspect before editing.
- Prefer repository-native tools and existing workflows.
- Make narrow, reversible changes tied to the task objective.
- Verify results with commands or artifact inspection when verification is possible.
- Report what changed, why it changed, and what was verified.

### Behavioral Boundaries

- Do not improvise broad refactors when the task is narrow.
- Do not hide uncertainty behind confident language.
- Do not claim success without evidence.
- Do not silently switch from analysis to mutation; the session state should show the transition.

### Terminal Interaction Model

The engineer loop should look like:

1. ingest task
2. inspect repository state
3. form a short execution plan
4. perform terminal actions
5. verify outputs
6. summarize results
7. persist useful memory

This loop is simple by design.
If Simard needs a more complicated execution flow, that complexity should emerge from orchestration primitives rather than one giant opaque prompt.

The current shipped v1 engineer-loop slice stays intentionally narrow:

- default behavior remains a read-only repo-native inspection action
- a bounded mutating path exists only for explicit structured objectives with `edit-file:`, `replace:`, `with:`, and `verify-contains:` directives
- the mutating path requires a clean repo, exactly one expected changed file, and explicit content verification before success is reported
- the terminal-backed engineer substrate now supports bounded interactive checkpoints with `wait-for:` / `expect:` directives so a local PTY session can pause for expected output before sending the next terminal line
- the terminal-backed engineer substrate now also has a read-only audit companion so operators can inspect persisted shell details, ordered terminal steps, satisfied wait checkpoints, last output lines, and transcript summaries after a terminal-backed session completes
- the primary terminal-run surface now renders that same structured audit trail during execution so operators can follow bounded copilot-style terminal driving without dropping to raw evidence lines
- operators can now author those bounded interactive terminal sessions either inline or from reusable file-backed recipes, while staying on the same truthful local PTY substrate
- Simard now also ships named built-in terminal recipes so operators can discover, inspect, and rerun common interactive session flows without inventing ad hoc shell strings or temp files each time
- the shipped `copilot-status-check` recipe is a truthful bounded local status probe: it only runs `amplihack copilot -- --version`, requires the `GitHub Copilot CLI` version signal, and fails closed instead of simulating an interactive Copilot session
- those terminal surfaces are now an explicit on-ramp into the repo-grounded engineer loop when operators reuse the same `state-root`, but the later engineer run must still inspect the repository, form its own short plan, execute bounded local work, and verify explicitly
- the bridge is descriptive continuity only: terminal-derived working directory, transcript snippets, or recipe metadata may inform operator readback, but they must not become authority for engineer action selection or workspace targeting

## Memory Architecture

Simard needs memory, but it should be structured memory, not an undifferentiated transcript dump.

### Memory Layers

#### 1. Session Scratch

Ephemeral working state for the current run: notes, discovered files, partial plans, command outputs, and intermediate reasoning anchors.
This is disposable and should be cheap to reset.

#### 2. Session Summary

A compact record written at the end of the session: sanitized objective metadata, key actions, outcomes, changed artifacts, and follow-up items.
This is the primary bridge between one session and the next.

When one explicit `state-root` is reused across the terminal session surfaces and the repo-grounded engineer loop, that bridge should be represented by mode-scoped handoff summaries rather than one ambiguous catch-all artifact.

The concrete v1 handoff contract is:

- `latest_terminal_handoff.json` is authoritative for `engineer terminal`, `engineer terminal-file`, `engineer terminal-recipe`, and `engineer terminal-read`
- `latest_engineer_handoff.json` is authoritative for `engineer run` and `engineer read`
- `latest_handoff.json` exists only as a compatibility fallback when the relevant mode-scoped handoff file is absent
- mode-scoped readback must report which artifact it used and render in a deterministic operator-visible order: runtime header, handoff session summary, adapter details, shell or repo details, action/checkpoint audit, transcript or continuity summary, explicit next-step guidance, durable record counts
- malformed mode-scoped state fails closed instead of silently falling back

#### 3. Project Memory

Durable repository-scoped facts such as architecture constraints, important files, recurring pitfalls, and established conventions.
This memory should be updated conservatively and only from validated session outcomes.

#### 4. Benchmark Memory

Structured records of benchmark runs, scores, failures, and improvements.
This memory is for evaluation and tuning, not for contaminating normal project context.

### Memory Rules

- Durable memory writes must be explicit.
- Session scratch should not automatically become long-term memory.
- Benchmark outcomes should be queryable separately from project execution history.
- Meeting outputs should write decisions, not entire raw conversations, into durable memory.
- Goal stewardship should preserve a durable active top 5 that later engineer sessions can inspect directly.
- V1 manifests must keep `MemoryPolicy.allow_project_writes=false` until there is an explicit project-write contract.

## Platform Architecture Layers

Simard should not be modeled as a single monolithic prompt with some attached tools.
It should sit on top of a reusable platform architecture with distinct layers.

### 1. Prompt Assets

All prompt files should be separate from code.
They are configuration and identity assets, not hidden implementation details.

Prompt assets should be:

- stored as explicit files
- versioned independently from runtime logic
- swappable or overrideable for experiments
- composable into larger identities
- benchmarkable so prompt changes can be evaluated cleanly

This matters in practice because some base types, especially RustyClawd, may need to be decomposed or enhanced so their defaults can be separated out and replaced without forking runtime logic.

### 2. Agent Base Type

An agent base type is the underlying execution substrate an identity can build on.
It is not the identity itself.

Candidate base types include:

- `RustyClawd`
- Microsoft Agent Framework
- GitHub Copilot SDK
- Claude Code SDK
- the amplihack / amplihack-rs goal-seeking agent and its OODA loop

The current Simard scaffold already publishes builtin manifest-facing base-type identifiers for:

- `local-harness`
- `terminal-shell`
- `rusty-clawd`
- `copilot-sdk`

Those identifiers are intentionally explicit at bootstrap time. Unsupported or unregistered base-type/topology pairs must fail visibly rather than collapsing into a hidden local default, and the v1 aliases must still report the honest `local-harness` implementation identity behind them.

Each base type should ideally have a Rust wrapper or adapter so the rest of the platform can interact with them through a more uniform model.

The wrapper boundary should normalize:

- prompt injection or prompt override mechanisms
- session lifecycle
- tool and skill invocation
- message and event flow
- memory hooks
- reflection and health inspection

The point is not to erase differences between backends.
The point is to stop backend-specific details from leaking into every identity.

#### Base Type Capability Contract

Every base type adapter should declare a capability contract rather than pretending all substrates are equivalent.

At minimum, the contract should describe:

- prompt override support
- tool and skill invocation support
- streaming versus non-streaming execution
- memory hooks
- reflection support
- subagent spawning support
- restart / reload behavior
- normalized error classes such as auth failure, timeout, rate limit, transport failure, and tool denial

Identities should be able to declare required capabilities, and the runtime should refuse to instantiate an identity on an adapter that cannot satisfy them.

### 3. Agent Identity

An agent identity is the durable definition of what an agent is, how it behaves, what it values, and what it is composed from.
It should be defined above the base-type layer.

An identity may include:

- prompt assets
- one or more agent base types
- recipes, workflows, and orchestration patterns
- skills and tools
- memory policies and durable memory stores
- subordinate identities or specialist components
- operating modes and behavioral boundaries
- a master recipe or outer loop responsible for coordinating the rest of the identity
- reflection hooks that let the identity inspect its own composition and runtime state

The identity should not care whether its components are colocated, split across processes, or distributed across machines.
It may understand its current runtime topology through reflection, but topology is configuration, not essence.

#### Identity Manifest and Precedence

An identity should eventually compile down to a machine-readable manifest.
That manifest should include:

- identity name and version
- prompt asset references
- required base-type capabilities
- component graph
- supported modes
- policies and memory rules
- master recipe or outer-loop entrypoint
- precedence rules when prompts, policies, or components conflict

Without an explicit manifest and precedence model, composition will drift into hidden behavior.

### 4. Agent Identity Template

There should be a reusable starter shape for new identities.
At the simplest end, an identity could be created from a prompt file plus a small amount of metadata.
At the richer end, an identity could compose multiple prompts, skills, recipes, tools, subordinate identities, and multiple base-type backend integrations.

This suggests Simard should not be a one-off snowflake.
It should help define a general identity template or starter repository that can also support sibling identities.

### 5. Composite Identities

Simard is one example of a composite identity: a high-standard engineering steward over the amplihack ecosystem.
Other identities may exist with different core jobs, for example:

- `Cumulona`: focused on curating, composing, and maintaining cloud resources with elegance and efficiency
- `Victoria`: focused on personal-assistant behavior, coordination, reminders, and structured support work

These should share platform primitives while differing in identity definition, memory policy, tool surface, and success criteria.

Simard itself should also be composite.
Internally, it may include:

- lightweight markdown subagents
- recipe-driven specialists
- amplihack skills such as planner or architect roles
- richer subordinate identities with their own goals and memory
- specialized execution components for terminal use, backlog curation, benchmark running, or memory stewardship

Human teams split work across specialists, leads, reviewers, and coordinators.
Simard should borrow from that pattern instead of pretending one giant undifferentiated agent is the clean design.

### 6. Agent Runtime

The agent runtime is the system that takes an identity and makes it real.
It is likely built on the recipe-runner lineage, but it may eventually need to evolve past today's recipe-runner abstraction.

The runtime is responsible for:

- instantiating an identity and its component parts
- selecting and wiring the base types the identity needs
- choosing the execution topology
- wiring communication channels between components
- providing memory access and synchronization
- managing lifecycle operations such as start, stop, restart, reload, and handoff
- spawning subordinate agents and tracking their objectives
- transferring or attaching durable memory when work moves between machines
- exposing reflection interfaces so the active agent can inspect its own runtime state

For the current Simard repo, that runtime contract is exposed as a local CLI/bootstrap surface and in-process Rust APIs. It is not an HTTP API and it does not currently publish a database schema contract.

#### Runtime Lifecycle and Control Plane

The runtime needs a concrete control model, not just a responsibility list.
At minimum it should define lifecycle states such as:

- initializing
- ready
- active
- reflecting
- persisting
- degraded
- stopping
- failed

It should also define:

- allowed state transitions
- graceful stop versus force stop
- restart and reload semantics
- handoff semantics
- parent / child ownership for subordinate agents
- cancellation and cleanup guarantees
- what must be persisted before termination

### 7. Topology Abstraction at the Identity Layer

From the identity's point of view, internal communication and memory access should use the same conceptual model whether execution is:

- single-process
- multi-process on one machine
- distributed across machines or VMs

The runtime should decide deployment topology through configuration.
The identity should not need different business logic just because its internals moved from local to distributed execution.

That does not mean every topology has identical semantics.
Ordering, latency, freshness, and partial-failure behavior may differ across deployment shapes, and the runtime must expose those guarantees honestly.

In the current v1 scaffold, builtin defaults still inject `single-process`, but explicit bootstrap now also supports a loopback `multi-process` path for compatible base types such as `rusty-clawd`.
The runtime core also ships handoff export/restore and a composite builtin identity surface, proving that agent logic and identity composition can stay stable while topology services and runtime ownership vary. This is still not a full distributed product feature.

This is close to the hive-mind idea in amplihack:
agent communication and memory semantics should be fundamentally the same whether the system is local or distributed.

### 8. Reflection Interface

The runtime should expose a structured reflection interface that lets an identity learn about:

- its active role and composition
- the base types currently in use
- available component identities and specialists
- runtime topology
- current memory backends
- session and process health
- subordinate agents and their assigned goals

This is important for Simard because self-improvement and self-relaunch both depend on knowing enough about the current system to act deliberately.

The reflection interface should be typed, read-only, and explicit about freshness and provenance.
It should say what is observed directly, what is inferred, and what is unavailable.

## Session Orchestration

Session orchestration is the control plane that turns a prompt into disciplined execution.
It should be implemented as a state machine rather than a free-form loop.

### Core Session Phases

1. Intake
   Normalize the request, detect mode, and identify repo or workspace context.

2. Preparation
   Gather current state, constraints, and existing memory relevant to the task.

3. Planning
   Produce a bounded plan sized to the task, including expected verification steps.

4. Execution
   Perform shell actions, file changes, and tool calls while recording evidence.

5. Reflection
   Compare results against the objective and capture what succeeded, failed, or remains open.

6. Persistence
   Write session summary, memory updates, and benchmark records if applicable.

### Orchestration Rules

- Mode must be chosen early and remain explicit.
- Session state must survive partial failure well enough to support recovery or retry.
- Reflection is mandatory; a session without reflection is incomplete.
- Persistence should happen after execution and reflection, not continuously on every token.
- Session orchestration should be runnable by a single local process today and by a distributed runtime later without changing the session semantics.

For v1, the only required runnable deployment path is local single-process.
The distributed statement is still an architectural seam that must be preserved in contracts, adapter selection, memory boundaries, and reflection output.

## Identity Separation

Simard should preserve role separation even if those roles are initially implemented as logical components inside one Rust process.

### Core Identities

#### System

Owns policies, safety boundaries, mode routing, and runtime configuration.
The system identity does not perform engineering work directly.

#### Engineer

Executes repository tasks through terminal actions and file changes.
The engineer identity is judged by correctness, evidence, and bounded execution.

#### Reviewer

Evaluates output quality, checks claims against evidence, and decides whether the objective was actually met.
The reviewer must be able to disagree with the engineer.

#### Facilitator

Used in meeting mode to keep discussion structured, clarify trade-offs, and capture decisions.
The facilitator is optimized for alignment and synthesis, not code mutation.

#### Goal Curator

Used in goal-curation mode to maintain durable backlog priorities and the active top 5 goals.
The curator is optimized for truthful stewardship and prioritization, not code mutation.

This separation matters because it prevents a single identity from planning, executing, grading, and excusing itself in the same breath.

This role separation sits inside a larger distinction:
the Simard identity is the overall agent identity, while engineer, reviewer, facilitator, and goal curator are operating roles or sub-identities that may be implemented through composition.

## Meeting Mode

Meeting mode exists so Simard can participate in product and architecture work without pretending every interaction is a coding task.

### Purpose

- turn ambiguous goals into crisp decisions
- capture trade-offs and open questions
- produce meeting artifacts that future engineer sessions can use

### Expected Outputs

- decision summaries
- scoped action items
- identified risks
- explicit open questions
- optional structured goal updates that later engineer sessions can read back through durable state

### Constraints

- Meeting mode should not edit code unless the user explicitly transitions to engineer mode.
- It should write concise decision records, not bloated transcripts.
- It should surface disagreement and uncertainty instead of smoothing over them.

Meeting mode is a planning and alignment surface, not a disguised autonomous executor.

The current v1 scaffold now includes an honest meeting-mode delivery slice because it proved necessary to support the core engineer loop and durable backlog stewardship.

## Goal Stewardship Mode

Goal stewardship exists so Simard can maintain a durable backlog and explicit top 5 goals instead of relying on transient session summaries.

### Purpose

- preserve a truthful active top 5
- distinguish active, proposed, paused, and completed priorities
- give later engineer sessions explicit durable goal context

### Expected Outputs

- durable goal records
- an active top-goal list surfaced through reflection
- meeting-to-engineer carryover through shared state roots

### Constraints

- Goal stewardship should not claim code execution or verification work.
- It should preserve concise priorities and rationales, not transcript dumps.
- It should expose the active top-goal set honestly, even when fewer than five active goals exist.

## Self-Improvement Loop

Simard should improve through controlled loops tied to evidence.

### Loop Structure

1. Run real tasks or benchmarks.
2. Capture failures, retries, weak explanations, or unnecessary actions.
3. Form improvement hypotheses.
4. Propose targeted changes to prompts, policies, memory heuristics, or orchestration logic.
5. Re-run selected benchmarks.
6. Promote only the changes that measurably improve outcomes.

### Improvement Inputs

- benchmark failures
- repeated session failure modes
- reviewer findings
- human corrections

### Improvement Constraints

- No silent self-modification in production paths.
- Every promoted change should link to evidence.
- Improvements should be small and attributable.
- If a change cannot be evaluated, it should not be promoted automatically.

For v1, self-improvement should remain a reviewable offline loop, not an autonomous production feature. The shipped slice is explicit review artifact generation plus operator-driven improvement curation and promotion into durable priorities.

## Rust Implementation Direction

This repository is Rust-based, so the architecture should lean into Rust where it adds clarity:

- prompt assets stored as files, not embedded strings
- typed adapters around supported base types
- a typed identity model distinct from runtime configuration
- a small core runtime for session state, orchestration, topology services, and handoff
- typed domain models for session records, memory entries, evidence, and handoff snapshots
- dependency injection through explicit runtime ports rather than hidden globals
- clear module boundaries between control plane, execution plane, and persistence
- minimal external dependencies until product shape stabilizes

V1 should resist the urge to over-design a plugin framework.
A single binary with clean internal modules is the preferred starting point.

That single binary may temporarily host both identity-specific and runtime code during bootstrap, but the internal boundaries should preserve a clean extraction path.

## Recommended Initial Module Boundaries

These are directional module seams, not a frozen package layout.
The first scaffold should keep them concrete enough to code against immediately:

- `prompt_assets`: prompt asset identifiers, file-backed loading, and store contracts
- `base_types`: backend identifiers, capability contracts, topology support, and concrete factory/session implementations
- `identity`: identity manifests, memory policy, mode definitions, and base-type eligibility
- `runtime`: control-plane composition, lifecycle state, startup validation, topology, handoff, and local runtime kernels
- `session`: typed session identity, ordered lifecycle phases, and transition validation
- `memory`: layered memory scopes, write/read contracts, and storage implementations
- `evidence`: evidence records, provenance, and evidence sinks
- `reflection`: typed runtime snapshots and reflection reports

The first runnable path may live in a single binary, but the binary should be composed from these seams rather than collapsing them together.
The architecture should stay small enough that one engineer can still understand the whole system.

## Repo Topology and Ownership

The long-term architecture likely spans multiple repos, but the dependency direction needs to stay crisp.

### Target Split

- **Simard repo**: Simard identity definition, prompt assets, Simard-specific recipes, policies, and tests
- **Shared runtime repo**: base-type factories/session backends, lifecycle control plane, reflection, topology management, and orchestration substrate
- **Shared benchmark repo**: benchmark task schema, scoring, replay, and shared harness logic used by Simard and related projects such as Skwaq
- **Shared memory repo**: durable memory primitives and storage libraries, likely continuing to live in `amplihack-memory-lib`

### Dependency Direction

- prompt assets feed identities
- identities depend on runtime contracts
- runtime must not depend on a specific identity repo
- benchmark infrastructure must not depend on Simard-specific policies
- project memory and benchmark memory should remain operationally separate

### Bootstrap Rule

Until the shared runtime and benchmark repos exist, Simard may temporarily host bootstrap implementations.
But those implementations should be written as extractable seams, not as permanent Simard-specific entanglement.

## Foundational Decisions to Close Before Coding Deeply

Before implementation expands past scaffolding, the project should explicitly choose:

- the first base type
- the first durable memory store and schema approach
- the identity manifest format
- the benchmark task packaging format
- the evidence model used to decide whether work was verified
- whether reviewer behavior is a separate pass or an inline reflection mechanism

## Phased Roadmap

### Phase 0 - Foundation Spec and Runtime Skeleton

- establish this specification as the product baseline
- replace placeholder project context with Simard-specific context
- create the minimal Rust module skeleton for prompt assets, base-type factories/sessions, identity, runtime, session, memory, and modes
- define benchmark task record formats and session summary formats
- define the first identity template shape, even if initially backed by simple files
- decide the first external prompt-file layout and loading model
- choose one initial base type and one initial memory-store strategy
- **Exit criteria:** identity manifest format chosen, prompt layout chosen, evidence model chosen, memory-store direction chosen

### Phase 1 - Core Engineer Loop

- implement task intake, planning, execution, reflection, and persistence
- support local repository inspection and bounded file edits
- add durable session summaries and project memory writes
- prove the loop on documentation and simple code-change tasks
- implement the first real base-type factory/session backend around RustyClawd
- keep runtime local and single-process
- **Exit criteria:** one bounded repository task can be completed end to end with evidence and repeatable verification

### Phase 2 - Benchmark Gym

- add benchmark task loading and run recording
- introduce scoring, replay, and regression comparison
- use the gym to evaluate prompt and orchestration changes before promotion
- keep the benchmark set intentionally small and high-signal
- **Exit criteria:** a small benchmark set runs reproducibly with stable scoring and useful artifacts

### Phase 3 - Identity Hardening and Shared Extraction

- introduce explicit identity composition and reflection APIs
- harden capability negotiation between identity and base type
- extract shared runtime or harness pieces only if the seams are proven by real reuse
- **Exit criteria:** identity/runtime boundaries are explicit enough that shared components can be extracted without re-architecture

### Phase 4 - Meeting Mode and Structured Planning

- implement facilitator behavior and decision capture
- enforce stricter boundaries between engineer, reviewer, and facilitator roles
- connect meeting outputs to later engineer sessions through durable decision memory
- **Exit criteria:** meeting mode produces useful decisions without silently mutating code paths

### Phase 5 - Self-Improvement and Expansion

- add hypothesis tracking and improvement proposals
- run controlled benchmark comparisons for candidate changes
- require explicit review before promotion
- widen benchmark coverage
- support richer repo types and more complex workflows
- revisit UI surfaces only after terminal-native behavior is reliable
- support sibling identities that reuse the same runtime substrate
- **Exit criteria:** improvements can be proposed, evaluated, and accepted without destabilizing the core engineer loop

Current shipped v1 slice:

- review artifacts already emit concrete evidence-linked proposals
- an explicit improvement-curation mode can now promote approved proposals into durable active or proposed priorities
- deferred proposals remain visible in durable decision memory instead of triggering silent self-modification
- broader automatic evaluation and promotion still remain post-v1 work

## Open Questions for Iteration

These are deliberate next-iteration questions, not omissions in this document.

1. How much of the reviewer role should be implemented as a separate pass versus inline reflection inside the same session?
2. What is the minimum viable durable memory store: files, SQLite, or both?
3. How should benchmark tasks be packaged so they are easy to author but hard to game?
4. What evidence format best balances human readability with machine scoring?
5. When should meeting-mode decisions automatically influence engineer-mode planning, and when should they require explicit confirmation?
6. How aggressively should Simard prune long-term project memory to prevent drift and stale assumptions?
7. At what point does single-process identity separation stop being enough and justify multi-process or multi-agent execution?
8. What is the minimum viable identity template format: prompt file only, prompt plus metadata, or full manifest with composed components?
9. How should runtime reflection be exposed so identities can inspect themselves without tightly coupling to one runtime implementation?
10. Which capabilities belong in the shared agent runtime versus in identity-specific repos such as Simard, Cumulona, or Victoria?
11. Which parts of RustyClawd must be decomposed or enhanced to make prompt separation and prompt override first-class?
12. How much should the master recipe or outer loop live in the identity definition versus the shared runtime?
13. What is the smallest honest reflection contract that can work across different base types without pretending they expose identical internals?
14. At what concrete point does extracting a shared runtime repo create value rather than premature framework work?

## Decision Summary

Simard v1 should be built as a narrow, local, terminal-native engineering identity with one primary loop, explicit evidence, disciplined session orchestration, and multiple manifest-advertised builtin base types selectable from day one.
It should preserve clean seams between prompt assets, agent base types, agent identities, and agent runtime so that a broader platform can emerge without corrupting the first implementation.
The first implementation should stay small, Rust-native, and highly inspectable.
The product wins if it can repeatedly behave like a trustworthy engineer on bounded tasks, not if it accumulates flashy but ungrounded features.
