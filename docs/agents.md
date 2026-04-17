# Agents

Simard composes agent behavior from three orthogonal layers.

## 1. Base types

A base type wraps an LLM + tool-calling runtime. Today Simard supports:

| Base type | Runtime | Status |
|---|---|---|
| `rusty-clawd` | RustyClawd Rust SDK (Anthropic API direct) | Real — needs `ANTHROPIC_API_KEY`. |
| `claude-agent-sdk` | Claude Code CLI as subprocess | Real — needs `claude` on `PATH`. |
| `ms-agent-framework` | Microsoft Agent Framework | Real — needs `ms-agent-framework` binary or `python -m microsoft_agent_framework`. |
| `copilot-sdk` | `amplihack copilot` via PTY | Real — **runtime dependency on amplihack**. Tracked for native replacement in [amplihack-comparison.md](amplihack-comparison.md). |
| `local-harness` | In-process test adapter | Always available. |
| `terminal-shell` | Local PTY shell | Always available. |

## 2. Identity manifests

An identity declares capabilities, precedence, and the system prompt to inject. Simard's built-in identities correspond to its workflows:

- `engineer` — drives engineer loop runs
- `goal_curator` — goal-curation workflow
- `improvement_curator` — improvement-curation workflow
- `meeting_facilitator` — meeting REPL
- `gym_runner` — gym benchmark execution
- `review_pipeline` — review workflow

Identity prompts live under [`prompt_assets/simard/`](../prompt_assets/simard/).

## 3. Topologies

A topology describes how many agents, how they communicate, and who holds control. Common topologies:

- `single-process` — one agent in one process.
- Custom topologies can be declared in session bootstrap.

## Comparison with amplihack agents

amplihack ships **38 markdown agent definitions** under `amplifier-bundle/agents/` (core / specialized / workflows). Simard does not yet ship an equivalent markdown catalog. This is a tracked parity gap — see [amplihack-comparison.md](amplihack-comparison.md#agents).

For now: if you need an amplihack agent, invoke it via the `copilot-sdk` base type (which spawns `amplihack copilot`). Simard stewards the session; amplihack provides the agent persona.

## Adding a new identity

1. Create `prompt_assets/simard/<role>_system.md` with the system prompt.
2. Add the identity in the Rust identity manifest module.
3. Wire it to a command or workflow.

No code change is required to edit an existing identity's prompt — prompts are shipped assets loaded at runtime.
