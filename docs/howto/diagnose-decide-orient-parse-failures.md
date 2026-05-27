---
title: Diagnose OODA decide/orient brain parse failures
description: Operator runbook for OODA brain parse failures. Find, classify, and remediate parse failures from the decide, orient, and lifecycle phases.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
---

# How-to: Diagnose OODA decide/orient brain parse failures

> **Audience:** operators on call when an OODA goal is making no progress
> across many cycles despite the dashboard reporting `success: true`
> decisions.
>
> **Prerequisites:** read access to `~/.simard/logs/`,
> `~/.simard/cycle_reports/`, and `~/.simard/metrics/` on the daemon
> host; familiarity with the `simard` CLI and `jq`.

## All three brains: first-word/first-float extraction

> **Updated in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> All three OODA brain parsers (decide, orient, lifecycle) now use
> first-word or first-float extraction. Parse failures in the traditional
> sense (format rejected) are greatly reduced:
>
> - **Decide:** First word matched against 10 action keywords. No match →
>   `AdvanceGoal` default. Never returns an error.
> - **Orient:** First float extracted from text. No float → deterministic
>   floor. Never returns an error.
> - **Lifecycle:** First word matched against 6 variant names. No match →
>   `ContinueSkipping` default. Never returns an error.
>
> The parsers themselves cannot fail — they always produce a valid result.
> Failures now only occur at the **infrastructure** level (recipe subprocess
> failure, binary not found, etc.).

### Infrastructure failure modes

| Failure | Log signature | Action |
|---------|--------------|--------|
| `recipe-runner-rs` not found | `[ooda] recipe-runner-rs not found; using deterministic fallback` | Install `recipe-runner-rs` or verify `$PATH`. |
| Recipe subprocess exits non-zero | `ERROR simard::ooda_brain: recipe invocation failed` + stderr | Check the recipe YAML syntax and the agent's error output. |
| Recipe YAML not found | `RecipeBrain::new() returned None` | Verify recipe YAML exists in `prompt_assets/simard/recipes/`. |

### Verifying the recipe brain is active

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'recipe_brain|build_.*_brain'
```

On successful construction, no log line is emitted. On fallback:

```
WARN simard::operator_commands_ooda: [ooda] recipe-runner-rs not found; using deterministic decide fallback
```

### When the parser produces unexpected defaults

If the parser consistently returns the default (`AdvanceGoal`, `ContinueSkipping`,
or deterministic floor), the LLM is not outputting the expected first word/float.

**Diagnosis:**

1. Check the daemon log for the raw recipe output:
   ```bash
   tail -F ~/.simard/logs/rustyclawd.log | grep 'recipe_brain'
   ```

2. If the first word of the output is not a recognized variant, the prompt
   needs updating. Edit the recipe YAML to strengthen the OUTPUT FORMAT
   instruction and examples.

3. Common cause: the LLM is outputting prose before the keyword, e.g.,
   `"I think we should advance_goal..."` instead of `"advance_goal I think..."`.
   The first-word parser only checks the first word. Ensure the prompt
   says: *"Output the action keyword as the very first word."*

## Orient brain: bare-float extraction

The orient brain extracts the first decimal number from the output text.
If no float is found, the deterministic floor applies.

### Common orient issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| Always hitting deterministic floor | LLM not outputting a number | Strengthen orient prompt OUTPUT FORMAT to say "Output a bare decimal as first token" |
| Urgency always 0.0 | LLM outputting `0.0` explicitly | Check if failure_count is very high; this may be correct behavior |
| Urgency above base_urgency | LLM outputting an escalated value | `validate()` catches this and falls back to floor; prompt examples should emphasize no escalation |

## Step 1: Replay the output locally

Use the parser directly in a unit test:

```rust
#[test]
fn repro_parse_issue() {
    let raw = "OK"; // <-- paste unescaped recipe output here
    let result = crate::ooda_brain::recipe_brain::parse_action_from_text(raw);
    eprintln!("{result:?}");
    // For orient:
    let orient = crate::ooda_brain::recipe_brain::parse_orient_from_text(raw, 0.8, 1);
    eprintln!("{orient:?}");
    // For lifecycle:
    let lifecycle = crate::ooda_brain::recipe_brain::parse_lifecycle_from_text(raw);
    eprintln!("{lifecycle:?}");
}
```

Run with `cargo test repro_parse_issue -- --nocapture`. Discard before committing.

## Step 2: Pick a remediation

| Issue | Remediation |
|-------|-------------|
| LLM not outputting keyword as first word | Edit the recipe YAML to add/strengthen the OUTPUT FORMAT section and examples. |
| LLM outputting wrong keyword | Check the prompt's OPTIONS section and examples for stale or confusing guidance. |
| Adapter 5xx / rate limit / timeout | Investigate the adapter; the brain itself is healthy. |
| Persistent non-cooperative model | Switch the provider in the brain config. |

After editing a recipe YAML, **no rebuild required** — edits take effect on
the next OODA cycle automatically.

## Step 3: Verify the fix

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'goal_id="<goal-id>"'
```

You should see non-default decisions with substantive rationale.

## Anti-patterns

* **Restarting the daemon directly** to "clear" parse defaults. The parser
  is stateless; a bare restart accomplishes nothing. Always go through
  `simard safe-update` for code changes.
* **Adding back keyword-anywhere scanning or JSON extraction.** The first-word
  parser is intentionally simple. If the LLM is not outputting the keyword
  first, fix the **prompt**, not the parser.
* **Ignoring consistent defaults.** If a brain consistently returns the
  default, it means the LLM output format doesn't match the prompt. This is
  a prompt quality issue, not a parser bug.

## See also

* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md) — normative grammar for all text protocols.
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale.
* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md) — engineer-lifecycle wire format.
* [How-to: diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md) — engineer-lifecycle equivalent.
* [How-to: edit the OODA brain prompt](./edit-the-ooda-brain-prompt.md) — prompt editing guide.
