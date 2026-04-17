# Patterns

Reusable solution shapes that recur across Simard's Rust codebase and across tasks Simard drives for operators.

## Bridge pattern

External systems (amplihack Python eval, knowledge packs) live behind a narrow IPC boundary so Simard's Rust core does not take a hard Python dependency. See [architecture/bridge-pattern.md](architecture/bridge-pattern.md).

## OODA loop

Observe → Orient → Decide → Act → Review. The autonomous daemon runs this loop over issues, gym scores, memory, and handoffs. Any autonomous decision can be traced back to an observation.

## Engineer loop

Inspect → Select → Execute → Verify. One work item, one loop iteration. No implicit state between iterations — everything crosses through the session record.

## Base-type adapter

LLM runtimes (RustyClawd, Claude SDK, MS Agent Framework, amplihack copilot) are wrapped in a common `BaseType` trait so the rest of Simard does not branch on "which LLM are we talking to." See [architecture/agent-composition.md](architecture/agent-composition.md).

## Identity manifests

Roles (engineer, goal curator, improvement curator, meeting facilitator, gym runner) are defined as structured manifests that name capabilities, precedence, and system prompts. Swapping identities is a data change, not a code change.

## Cognitive memory types

Sensory / working / episodic / semantic / procedural / prospective. Each type has explicit retention and retrieval rules. See [architecture/cognitive-memory.md](architecture/cognitive-memory.md).

## Handoff files

Decisions and action items leave the meeting REPL as files that later engineer sessions read. This is the only supported channel for meeting → session state transfer.

## Formal specification as prompt

When a behavioral requirement is complex (concurrent state, multi-actor invariants), writing the invariant as a formal predicate ("failedAgents ≠ {} ⟹ phase ≠ complete") or a Gherkin scenario produces better code than prose.

## Evidence-first review

Every review surfaces: what changed, what tests run, what evidence supports the claim the change is correct. PRs without evidence are asked for evidence, not merged.

## Graceful-degradation is banned

If a dependency fails, Simard errors out and reports the specific failure. Silent fallback paths are not acceptable because they hide brokenness from operators.

## Next

- [Philosophy](philosophy.md)
- [Workflows](workflows.md)
