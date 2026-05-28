# How-To: Edit the Engineer System Prompt

The engineer agent's behavior — including TDD discipline, quality standards,
and coding conventions — is controlled by
`prompt_assets/simard/engineer_system.md`. This guide shows how to modify
engineer behavior **without touching Rust**.

## TL;DR

1. Edit `prompt_assets/simard/engineer_system.md`.
2. Rebuild: `cargo build --release -p simard`.
   (The prompt is compiled in via `include_str!`; a rebuild is required for
   the embedded fallback. However, the deployed prompt is loaded from
   `~/.simard/prompt_assets/simard/engineer_system.md` at runtime.)
3. Redeploy: `scripts/redeploy-local.sh` syncs prompt assets to the runtime
   directory.
4. New engineer sessions will pick up the updated prompt automatically.

## File Location

The daemon resolves the engineer system prompt in this order:

1. `$SIMARD_PROMPT_ASSETS_DIR/engineer_system.md` (override for dev worktrees)
2. `$HOME/.simard/prompt_assets/simard/engineer_system.md` (default runtime)
3. Compile-time embedded fallback via `include_str!`

## Prompt Structure

| Section | Purpose | Editable? |
|---------|---------|-----------|
| Role & Identity | Sets the agent's persona and scope | Yes |
| Task Format | How goals and issues are presented | Yes, but keep `{{var}}` placeholders |
| Quality Standards | TDD discipline, code style, safety rules | Yes — this is the primary knob |
| Prompt-First Improvements | Rules for self-modifying work on Simard | Yes |
| Output Format | Commit message conventions, PR templates | Yes |

## Common Edits

### Adding a new quality standard

Add a bullet to the "Quality Standards" section:

```markdown
- **Your Rule Name**: Description of the rule and how to follow it.
```

The engineer agent will treat this as a hard requirement in all sessions.

### Modifying TDD discipline

The TDD instruction is in the Quality Standards section:

```markdown
- **Test-Driven Development (commit ordering)**: Always write tests before
  implementation code. ...
```

To relax it (e.g., allow test-after for pure refactors), edit the text
directly. No CI scripts, no Rust changes.

### Adding language-specific conventions

Add language rules as sub-bullets or new bullets in Quality Standards:

```markdown
- **Python style**: Use `ruff` for formatting. Type-annotate all public functions.
- **TypeScript style**: Use `strictNullChecks`. Prefer `const` over `let`.
```

## Testing Your Changes

1. **Syntax check**: Ensure the markdown renders correctly (no broken
   templates, no unclosed code blocks).
2. **Dry run**: Start an engineer session against a test goal and verify the
   agent follows the new instructions.
3. **Commit history**: For TDD changes, check that the agent's commits show
   the expected test-first ordering.

## Related

- [Prompt-driven TDD discipline](../concepts/prompt-driven-tdd-discipline.md) —
  why TDD is enforced through prompts, not CI scripts
- [Prompt-driven brain iteration](../concepts/prompt-driven-brain-iteration.md) —
  how OODA brains use the same hot-reload pattern
- [Edit the OODA brain prompt](edit-the-ooda-brain-prompt.md) —
  similar guide for the OODA lifecycle brain
