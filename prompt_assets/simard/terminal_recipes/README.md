# Terminal Recipes — Runtime Dependencies

This directory ships Simard's terminal recipes. **Three of the four recipes
invoke `amplihack copilot`** as a PTY subprocess:

- `copilot-prompt-check.simard-terminal`
- `copilot-status-check.simard-terminal`
- `copilot-submit.json`

That is a **runtime dependency on amplihack**. These recipes will not work on
a host without `amplihack` on `PATH`. It is a deliberate, documented
dependency — Simard's `copilot-sdk` base type currently delegates to
`amplihack copilot` for Copilot CLI interaction.

The fourth recipe, `foundation-check.simard-terminal`, has no amplihack
dependency.

## Migration status

Replacing `amplihack copilot` with a native Simard Copilot adapter is a
tracked parity issue (see `docs/amplihack-comparison.md#copilot-sdk`). When
that ships, these recipes will migrate to the native adapter and this
runtime dependency will be removed.

Do not add new recipes that depend on `amplihack copilot` unless you also
plan to update them as part of the parity migration.
