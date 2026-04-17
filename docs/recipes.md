# Recipes

## Current state

Simard does not ship a YAML recipe DSL. amplihack's `amplifier-bundle/recipes/` (roughly 19 recipes including `smart-orchestrator`, `default-workflow`, `investigation-workflow`, `consensus-workflow`) encodes multi-step agent workflows in YAML and executes them through `amplihack-recipe-runner`.

Simard's equivalents today are:

- The **OODA daemon** (see [workflows.md](workflows.md)).
- The **engineer loop**.
- The **terminal recipes** under [`prompt_assets/simard/terminal_recipes/`](../prompt_assets/simard/terminal_recipes/).

## Terminal recipes

Simard ships four terminal recipes today:

- `copilot-prompt-check.simard-terminal`
- `copilot-status-check.simard-terminal`
- `copilot-submit.json`
- `foundation-check.simard-terminal`

**Runtime dependency:** three of the four invoke `amplihack copilot` commands. They are functional today but will break without amplihack installed. Tracked for native replacement — see [amplihack-comparison.md](amplihack-comparison.md#copilot-sdk).

See [howto/move-from-terminal-recipes-into-engineer-runs.md](howto/move-from-terminal-recipes-into-engineer-runs.md) for migrating terminal recipes into engineer sessions.

## Gap: YAML recipe DSL

A native Simard recipe DSL that matches `amplihack-recipe-runner` is a tracked parity issue. The design intent:

- YAML-like declarative recipes describing ordered steps, each step running a base-type adapter or a local bash command.
- First-class support for nested recipes, per-step environment, and error recovery.
- A Rust executor that does not require Python or Claude Code to be installed.

When this ships, recipes currently driven through `amplihack copilot` can be migrated one at a time.

## In the meantime

- Use the **engineer loop** for most multi-step work.
- Use the **OODA daemon** for autonomous continuous operation.
- Use **meeting REPL → handoff → engineer run** to carry decisions across sessions.
