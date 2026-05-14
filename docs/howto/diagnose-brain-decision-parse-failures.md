# How-to: Diagnose OODA Brain Decision Parse Failures

> **Audience:** operators on call when a goal is stuck in `continue_skipping`
> longer than expected.
>
> **Prerequisites:** read access to `~/.simard/logs/` on the daemon host;
> familiarity with the `simard` CLI.

The OODA brain decision parser is intentionally lenient — it accepts a prose
`DECISION:` marker, hybrid prose-plus-JSON, or pure JSON. When it still
rejects a response, the cycle falls back to `ContinueSkipping` and emits a
`WARN`-level log line that contains the **full raw model response**. This
guide tells you how to find that log, read it, and decide what to do.

For the full protocol definition see the
[OODA Brain Decision Protocol reference](../reference/ooda-brain-decision-protocol.md).

## Step 1: Find the failing cycle

Symptoms that justify reading parse-failure logs:

* A goal stays in `continue_skipping` for many cycles even though the
  engineer worktree mtime is hours old.
* The dashboard (or a manual scan of the goal-board) shows a stable
  goal whose consecutive-skip count climbs every minute.
* The most recent decision rationale for the goal is the literal string
  `"deterministic fallback"` — the sentinel emitted by
  `DeterministicFallbackBrain` when the real brain failed to construct.

Tail the daemon log directly. There is no `simard logs` subcommand; the
daemon writes to `~/.simard/logs/` on the host:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'brain\.decide_engineer_lifecycle|goal=<your-goal-id>'
```

> **Future-work commands.** A future PR may add `simard ooda status`,
> `simard ooda last-decision`, `simard ooda dry-run`, and `simard logs
> tail` to wrap the patterns below. Until those land, drive everything
> from `~/.simard/logs/` and `simard safe-update`.

You are looking for a line shaped like:

```
WARN simard::ooda_brain: brain.decide_engineer_lifecycle parse failed
    goal=improve-amplihack-test-coverage
    raw="OK"
    error=no DECISION: marker found and response is not valid JSON
```

The `raw=...` field is the **complete** model response (truncated to 8 KB
with a `…(truncated, total N bytes)` suffix if longer). Before #1711 this
field was a misleading `got 3 bytes`; if you still see that form, the daemon
is running a pre-#1711 build and needs `simard safe-update` to pick up the
fix.

## Step 2: Classify the response

Match the contents of `raw=` against this triage table.

| `raw` looks like…                              | Likely cause                                            | Action                                                            |
|------------------------------------------------|---------------------------------------------------------|-------------------------------------------------------------------|
| `"OK"`, `"continue"`, `"yes"`                  | Model ignored the prompt; emitted a chat acknowledgment | [Step 3 — replay the prompt](#step-3-replay-the-prompt-locally) to confirm; consider tightening the prompt's role section. |
| `""`                                           | LLM provider returned an empty body                     | Check the adapter logs (`~/.simard/logs/rustyclawd.log`) for a 5xx or rate-limit error.                                    |
| `"DECISION: bogus_variant"`                    | Model invented a variant name                           | The error message lists the 6 valid variants; cross-check the prompt's enumeration block isn't drifting.                   |
| `"DECISION: open_tracking_issue\n<no JSON>"`   | Model used the marker but omitted required fields       | The prompt's `# OPTIONS` section needs to remind the model that this variant requires `title` + `body`.                    |
| Long prose with no `DECISION:` marker          | Model is in chat mode, not structured-output mode       | Check that `RustyClawdAdapter` is forcing structured output / JSON mode for the configured provider.                       |
| Valid-looking JSON that still fails            | Field name typo or wrong `choice` casing                | Run `Step 3` and inspect — the parser only accepts the variant tokens listed in the [protocol reference](../reference/ooda-brain-decision-protocol.md#variant-whitelist). |

## Step 3: Replay the prompt locally

There is no dedicated `dry-run` subcommand yet. To confirm a hypothesis
without waiting for the next cycle, use the parser directly from a
hermetic unit test against the captured `raw=` text:

1. Copy the `raw=` payload from the log line (it is rendered with `{:?}`,
   so you may need to unescape `\n` → newline and `\"` → `"`).
2. Add a one-off test to `src/ooda_brain/tests.rs`:

   ```rust
   #[test]
   fn repro_1711_OK_payload() {
       let raw = "OK"; // <-- paste the unescaped payload here
       let result = crate::ooda_brain::rustyclawd::parse_decision_from_response(raw);
       eprintln!("{result:?}");
       // Either Ok(EngineerLifecycleDecision::...) or
       // Err(BrainResponseUnparseable { ... }) — matches the daemon's behavior.
   }
   ```

3. Run with `cargo test repro_1711_OK_payload -- --nocapture`.

Because `parse_decision_from_response` has no I/O dependencies, this is a
faithful replay — what the test prints is what the daemon would have
parsed. Discard the test before committing.

## Step 4: Pick a remediation

| Cause from Step 2                          | Remediation                                                                                                                |
|--------------------------------------------|----------------------------------------------------------------------------------------------------------------------------|
| Chat acknowledgment / wrong mode           | Edit `prompt_assets/simard/ooda_brain.md` to strengthen the "respond ONLY with the marker" instruction. See [edit-the-ooda-brain-prompt](edit-the-ooda-brain-prompt.md). |
| Adapter 5xx / rate limit                   | Investigate the adapter; the brain itself is healthy.                                                                      |
| Bogus variant                              | The prompt should be the only place that lists variants; align it with the enum.                                            |
| Missing required fields on hybrid form     | Update the prompt's `# OPTIONS` section to make the field requirement explicit.                                            |
| Persistent unparseable noise from one model| Switch the provider in the brain config; the parser cannot fix a fundamentally non-cooperative model.                      |

After editing the prompt, **do not** restart the daemon by hand. The
prompt is embedded with `include_str!`, so a new prompt requires a rebuild
and a daemon update via:

```bash
/home/azureuser/.simard/bin/simard safe-update
```

`safe-update` rebuilds, drains in-flight cycles, hot-swaps the binary,
and verifies the new daemon is responsive — see
[safe-self-update](../safe-self-update.md).

## Step 5: Verify the fix

After `safe-update` completes:

```bash
tail -F ~/.simard/logs/rustyclawd.log | grep -E 'goal=<goal-id>'
```

You should see a non-`continue_skipping` decision within one cycle, or, if
the goal genuinely should keep skipping, a `DECISION: continue_skipping`
log with a substantive rationale (not the `"deterministic fallback"`
sentinel).

## Anti-patterns

The following patterns indicate the operator is **fighting** the protocol
rather than diagnosing it. Stop and re-read the
[protocol reference](../reference/ooda-brain-decision-protocol.md) before
proceeding:

* **Restarting the daemon directly** (`kill <pid>`, or
  `systemctl --user restart simard-ooda`, bypassing `simard safe-update`)
  to "clear" parse failures. The parser is stateless; a bare restart
  accomplishes nothing except dropping the parse-failure log evidence you
  need. Always go through `safe-update`, which performs the rebuild,
  drain, hot-swap, and health check together.
* **Editing the parser to "just accept" a new ad-hoc shape** the model
  emits. The protocol already accepts three forms; if the model is
  emitting a fourth, the **prompt** is wrong, not the parser.
* **Adding a fallback in `dispatch_spawn_engineer` for a specific raw
  response.** The single fallback path (`ContinueSkipping`) is intentional;
  a parse failure should be visible in the logs, not silently rerouted.

## See Also

* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md)
* [How-to: edit the OODA brain prompt](edit-the-ooda-brain-prompt.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
* [safe-self-update](../safe-self-update.md)
