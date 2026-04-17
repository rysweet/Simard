# Skills

## Current state

**Simard does not ship a bundled skill catalog yet.** amplihack ships roughly 87 skills under `amplifier-bundle/skills/`; Simard today ships zero equivalent native skills.

This is the **largest parity gap** between amplihack and Simard. It is tracked in the parity issue list linked from [amplihack-comparison.md](amplihack-comparison.md).

## Why skills matter

A skill in amplihack is a named, callable capability with:

- A system prompt fragment that teaches an agent when to invoke it.
- Optional tool wiring, recipe dispatch, or MCP server integration.
- A clear input/output contract.

Skills are how amplihack keeps the agent catalog composable: add a skill, and every agent that matches the invocation conditions can use it.

## Interim path

Until Simard has a native skill catalog:

1. **Use amplihack skills through the `copilot-sdk` base type.** When Simard dispatches to `amplihack copilot`, the copilot inherits amplihack's full skill catalog. This is not a clean boundary — it is a pragmatic bridge.
2. **Inline skill-like logic in identity prompts.** If a skill is small enough (a few sentences of instruction), it can be inlined into the relevant `prompt_assets/simard/*.md` identity.

## Migration priority (proposed)

When skill-catalog parity work starts, prioritize:

1. **Workflow-control skills** — `dev-orchestrator`, `default-workflow`, `investigation-workflow`, `consensus-workflow`. These shape the outer loop and map onto Simard's engineer loop / OODA.
2. **Pull-request skills** — `creating-pull-requests`, `code-review`. Immediately useful in engineer runs.
3. **Documentation-writing skill** — `documentation-writing`. Applies to Simard's own docs effort.
4. **Skill-like recipes** — `smart-orchestrator`, `investigation-workflow` as Simard-native recipe DSL counterparts.
5. **Domain analyst skills** — `cybersecurity-analyst`, `computer-scientist-analyst`, etc. Useful but not blocking.
6. **Language-specific skills** — `dotnet-*`, `aspire`, etc. Port incrementally as needed.

## What a native Simard skill will look like (design sketch)

- A `skills/` directory under the repo.
- Each skill is a markdown file with structured front matter (name, description, activation conditions).
- A Rust loader parses front matter, registers activation conditions, and exposes skills to identity manifests.
- No hard dependency on amplihack or Claude Code.

This design is not implemented yet. Contributions welcome on the tracking issue.
