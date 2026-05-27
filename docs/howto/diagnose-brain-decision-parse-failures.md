# How-to: Diagnose OODA Brain Decision Parse Failures

> **Audience:** operators on call when a goal is stuck in `continue_skipping`
> longer than expected.
>
> **Prerequisites:** read access to `~/.simard/logs/` on the daemon host;
> familiarity with the `simard` CLI.

The OODA brain lifecycle parser uses **first-word extraction** — it takes
the first non-whitespace token from the recipe output and matches it
case-insensitively against the 6 lifecycle variant names. If no match is
found, it returns `ContinueSkipping` as the default. The parser never
returns an error.

> **Updated in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> The `DECISION:` marker protocol, labeled-line field extraction, and
> keyword-anywhere scanning have been removed. The parser now uses trivial
> first-word extraction. Parse failures in the traditional sense (format
> rejected) cannot occur. If a goal is stuck in `continue_skipping`, it
> means either:
> 1. The LLM is outputting `continue_skipping` as its first word (correct
>    behavior — the LLM believes the engineer should keep running).
> 2. The LLM is outputting an unrecognized first word, and the parser
>    defaults to `ContinueSkipping`.

For the full protocol definition see the
[OODA Brain Decision Protocol reference](../reference/ooda-brain-decision-protocol.md).

## Step 1: Find the failing cycle

Symptoms that justify investigation:

* A goal stays in `continue_skipping` for many cycles even though the
  engineer worktree mtime is hours old.
* The dashboard shows a stable goal whose consecutive-skip count climbs
  every minute.

Tail the daemon log:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'brain\.decide_engineer_lifecycle|goal=<your-goal-id>'
```

With first-word extraction, the log will show the parsed variant:

```
INFO simard::ooda_brain: brain.decide_engineer_lifecycle result
    goal=improve-amplihack-test-coverage
    variant=continue_skipping
    rationale="engineer touched worktree 8 seconds ago"
```

If the variant is consistently `continue_skipping` when you expect a
different decision, check what the LLM is actually outputting.

## Step 2: Check the recipe output

The recipe output's first word determines the decision. If the LLM is
not producing the expected variant name as the first word:

| First word looks like… | Cause | Action |
|------------------------|-------|--------|
| `continue_skipping` | LLM correctly chose this variant | No action needed; the engineer may genuinely be healthy |
| `continue`, `ok`, `yes` | LLM emitting a chat ack instead of a variant name | Strengthen the prompt's OUTPUT FORMAT section |
| Random prose word | LLM not following the first-word format | Add/strengthen the OUTPUT FORMAT instruction in the recipe YAML |
| A valid variant name | Correct parse — check if the action handler is working | Debug the action handler, not the parser |

## Step 3: Replay the output locally

```rust
#[test]
fn repro_lifecycle_issue() {
    let raw = "OK"; // <-- paste the recipe output here
    let result = crate::ooda_brain::recipe_brain::parse_lifecycle_from_text(raw);
    eprintln!("{result:?}");
}
```

Run with `cargo test repro_lifecycle_issue -- --nocapture`.

## Step 4: Pick a remediation

| Cause | Remediation |
|-------|-------------|
| LLM outputting chat ack / wrong first word | Edit `prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml` to strengthen OUTPUT FORMAT. |
| LLM consistently choosing `continue_skipping` for a stuck goal | Add examples showing when to choose `reclaim_and_redispatch` or `mark_goal_blocked`. |
| Adapter 5xx / rate limit | Investigate the adapter; the brain itself is healthy. |
| Persistent non-cooperative model | Switch the provider in the brain config. |

After editing the recipe YAML, **no rebuild required** — edits take effect
on the next OODA cycle automatically.

## Step 5: Verify the fix

```bash
tail -F ~/.simard/logs/rustyclawd.log | grep -E 'goal=<goal-id>'
```

You should see a non-`continue_skipping` decision within one cycle, or, if
the goal genuinely should keep skipping, a `continue_skipping` with a
substantive rationale.

## Anti-patterns

* **Restarting the daemon directly** to "clear" defaults. The parser is
  stateless; a bare restart accomplishes nothing.
* **Adding back `DECISION:` marker parsing or keyword-anywhere scanning.**
  The first-word parser is intentionally simple. Fix the **prompt**, not
  the parser.
* **Adding labeled-line extraction back** for `TITLE:`, `BODY:`, etc.
  Structured fields use defaults. If you need richer field extraction in
  the future, that's a separate design decision.

## See Also

* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md)
* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md)
* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md)
* [How-to: edit the OODA brain prompt](edit-the-ooda-brain-prompt.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
* [safe-self-update](../safe-self-update.md)
