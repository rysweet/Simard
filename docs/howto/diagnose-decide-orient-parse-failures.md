---
title: Diagnose OODA decide/orient brain parse failures
description: Operator runbook for text-based OODA brain parse failures. Find, classify, and remediate parse failures from the decide and orient phases.
last_updated: 2026-05-24
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

## Decide brain: recipe-based keyword scanning

> **Updated in [#2111](https://github.com/rysweet/Simard/issues/2111).**
> The decide brain no longer uses `DECISION:` markers. It runs as a recipe
> step and scans the agent's prose for action keywords. Parse failures in
> the traditional sense (format rejected) **cannot occur** — the keyword
> scanner always returns a valid action kind. If no keyword is found, the
> default `advance_goal` is used.

### Decide-brain failure modes

The decide brain can still fail at the **infrastructure** level:

| Failure | Log signature | Action |
|---------|--------------|--------|
| `recipe-runner-rs` not found | `[ooda] recipe-runner-rs not found; using deterministic decide fallback` | Install `recipe-runner-rs` or verify `$PATH`. |
| Recipe subprocess exits non-zero | `ERROR simard::ooda_brain: recipe_decide invocation failed` + stderr | Check the recipe YAML syntax and the agent's error output. |
| Recipe YAML not found | `RecipeDecideBrain::new() returned None` | Verify `prompt_assets/simard/recipes/ooda-decide.yaml` exists. |

When `RecipeDecideBrain` fails to construct or the subprocess fails, the
daemon falls back to `DeterministicFallbackDecideBrain`, which maps goal
prefixes to action kinds (`__memory__` → `consolidate_memory`, etc.; real
goals → `advance_goal`). This fallback is correct for most cases but does
not preserve the agent's judgment for edge cases.

### Verifying the decide brain is using the recipe

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'recipe_decide|build_decide_brain'
```

On successful construction, no log line is emitted. On fallback:

```
WARN simard::operator_commands_ooda: [ooda] recipe-runner-rs not found; using deterministic decide fallback
```

## Orient brain: text-based parse failures

The orient brain uses JSON format and can still produce parse failures.
Parse failures fire four visibility channels in lock-step. This section
tells you how to find them, read them, and decide what to do.

For the wire format specifications, see
[Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md).
The engineer-lifecycle equivalent (#1711) is covered by
[Diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md).

## Step 1: Find the failing cycle (orient brain)

Symptoms that justify reading parse-failure evidence:

* A goal's `consecutive_skip` or "no progress" counter climbs every cycle
  even though the dashboard says recent decisions succeeded.
* `~/.simard/cycle_reports/cycle_*.json` shows `brain_judgments[].fallback == true`
  for `orient` on consecutive cycles and the new `parse_failure`
  block on the same record is non-null.
* The metric jsonl shows non-zero `brain_parse_failure` counters.
* A GitHub issue titled `OODA orient brain parse failure: goal=<id> (N consecutive)`
  was auto-filed against the `ESCALATION_REPO_SLUG` repo.

### Tail the structured log

The daemon writes to `~/.simard/logs/` on the host. Look for the
`brain.orient parse failed` message string at `ERROR` level:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'brain\.orient parse failed'
```

A matching line looks like:

```
ERROR simard::ooda_brain: brain.orient parse failed
    phase="orient"
    goal_id="improve-amplihack-test-coverage"
    error="no JSON object found in LLM response (got 3 bytes)"
    raw_response_truncated="OK"
    prompt_name="ooda_orient.md"
    prompt_version="a1b2c3d4e5f6"
    consecutive_count=2
    retry_attempted=false
```

`raw_response_truncated` is the **complete** model response, truncated only
at 8 KiB on a UTF-8 boundary. `prompt_name` and `prompt_version` identify
the exact prompt bytes the model saw.

### Check the metric stream

```bash
jq -c 'select(.metric_name == "brain_parse_failure")
       | .context |= fromjson' \
   ~/.simard/metrics/metrics.jsonl \
  | tail -20
```

### Read the cycle report

```bash
jq '.brain_judgments[]
     | select(.parse_failure != null)
     | { phase: .parse_failure.phase,
         goal_id: .parse_failure.goal_id,
         consecutive: .parse_failure.consecutive_count,
         raw: .parse_failure.raw_response_truncated,
         prompt: (.parse_failure.prompt_name + "@" + .parse_failure.prompt_version),
         error: .parse_failure.error_message }' \
   ~/.simard/cycle_reports/cycle_42.json
```

## Step 2: Classify the response

Open the `raw_response_truncated` value and match against this triage table.

| `raw_response_truncated` looks like… | Likely cause | Action |
|----|----|----|
| `"OK"`, `"continue"`, `"yes"` | Model ignored the JSON-output instruction; emitted a chat ack | [Step 3 — replay the prompt](#step-3-replay-the-prompt-locally); strengthen the prompt's output instructions |
| `""` (empty string) | Adapter returned `Err` with no body (5xx, timeout) | Check adapter logs for 5xx / rate-limit / timeout |
| Long prose without any JSON object | Model is in chat mode, not following the output format | Strengthen the prompt's output section examples |
| JSON object with wrong or missing field names | Model emitting wrong field structure | Diff `prompt_assets/simard/ooda_orient.md` against the expected fields (`adjusted_urgency`, `rationale`, `confidence`) |
| Partial text ending mid-word | Adapter truncated mid-stream | Check adapter log for `EOF` / `truncated stream` |

If `consecutive_count` is 1 or 2 and the next cycle shows `parse_failure == null`,
the model recovered on its own. No action required.

If `consecutive_count` reaches 3, the daemon has auto-filed a GitHub issue.

## Step 3: Replay the prompt locally

Use the crate-level helper to test orient parsing:

```rust
pub fn try_parse_orient_response(raw: &str)
    -> Result<OrientJudgment, SimardError>;
```

Add a one-off test:

```rust
#[test]
fn repro_parse_failure() {
    let raw = "OK"; // <-- paste unescaped payload here
    let result = crate::ooda_brain::try_parse_orient_response(raw);
    eprintln!("{result:?}");
}
```

Run with `cargo test repro_parse_failure -- --nocapture`. Discard before committing.

## Step 4: Read the auto-filed issue (if any)

```bash
gh issue list --repo rysweet/Simard \
  --label ooda-brain-parse-failure --label auto-filed \
  --state open
```

## Step 5: Pick a remediation

| Cause from Step 2 | Remediation |
|----|----|
| Chat ack / wrong mode | Edit `prompt_assets/simard/ooda_orient.md` to strengthen the JSON output instruction. See [edit-the-ooda-brain-prompt](edit-the-ooda-brain-prompt.md). |
| Adapter 5xx / rate limit / timeout | Investigate the adapter; the brain itself is healthy. |
| JSON output wrong fields | The model is emitting JSON with wrong field names. Update the prompt's examples to use the correct fields (`adjusted_urgency`, `rationale`, `confidence`). |
| Persistent non-cooperative model | Switch the provider in the brain config. |

> **Note:** The decide brain no longer has parse failures. It uses keyword
> scanning via a recipe step. If the decide brain is producing unexpected
> routing, edit `prompt_assets/simard/recipes/ooda-decide.yaml` — no rebuild
> required. See [OODA decide recipe and prompt schema](../reference/ooda-decide-prompt.md).

After editing a prompt, rebuild and hot-swap:

```bash
simard safe-update
```

## Step 6: Verify the fix

After `safe-update` completes:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'goal_id="<goal-id>"'
```

You should see:

* The next decide or orient cycle produce a non-`fallback` decision with
  substantive rationale.
* The `brain_parse_failure` metric stop incrementing.
* The next `cycle_N.json` omit the `parse_failure` key.
* The counter reset to 1 on the next failure (if any).

## Anti-patterns

* **Reverting to JSON output format in the orient prompt.** The orient parser
  expects JSON. Adding non-JSON examples to the orient prompt will cause
  models to emit prose, which will be parse-rejected.
* **Adding a `DECISION:` marker format to the decide recipe prompt.** The
  decide brain uses keyword scanning — there is no marker parser. Adding
  `OUTPUT_FORMAT` sections with `DECISION:` instructions is unnecessary
  and may confuse the agent.
* **Restarting the daemon directly** to "clear" parse failures. Use
  `simard safe-update` for any code/prompt change.
* **Suppressing the `ERROR` log line** with a tracing filter.
* **Closing the auto-filed issue without fixing the cause.**

## See also

* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md) — normative grammar for all text protocols.
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale.
* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md) — engineer-lifecycle wire format.
* [How-to: diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md) — engineer-lifecycle equivalent.
* [How-to: edit the OODA brain prompt](./edit-the-ooda-brain-prompt.md) — prompt editing guide.
