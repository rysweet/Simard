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

The decide and orient brains use text-based wire formats — `DECISION:` markers
and labeled lines — instead of JSON. Parse failures are rare because the
text format is tolerant of prose, but they can still occur when a model emits
a response with no recognizable decision marker or labeled field.

Parse failures fire four visibility channels in lock-step. This guide tells
you how to find them, read them, and decide what to do.

For the wire format specifications, see
[Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md).
The engineer-lifecycle equivalent (#1711) is covered by
[Diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md).

## Step 1: Find the failing cycle

Symptoms that justify reading parse-failure evidence:

* A goal's `consecutive_skip` or "no progress" counter climbs every cycle
  even though the dashboard says recent decisions succeeded.
* `~/.simard/cycle_reports/cycle_*.json` shows `brain_judgments[].fallback == true`
  for `decide` or `orient` on consecutive cycles and the new `parse_failure`
  block on the same record is non-null.
* The metric jsonl shows non-zero `brain_parse_failure` counters.
* A GitHub issue titled `OODA decide brain parse failure: goal=<id> (N consecutive)`
  was auto-filed against the `ESCALATION_REPO_SLUG` repo.

### Tail the structured log

The daemon writes to `~/.simard/logs/` on the host. Look for the constant
`brain.decide parse failed` / `brain.orient parse failed` message strings —
both are at `ERROR` level:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'brain\.(decide|orient) parse failed'
```

A matching line looks like:

```
ERROR simard::ooda_brain: brain.decide parse failed
    phase="decide"
    goal_id="improve-amplihack-test-coverage"
    error="no DECISION marker found in LLM response (got 3 bytes)"
    raw_response_truncated="OK"
    prompt_name="ooda_decide.md"
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
| `"OK"`, `"continue"`, `"yes"` | Model ignored the text-output instruction; emitted a chat ack | [Step 3 — replay the prompt](#step-3-replay-the-prompt-locally); strengthen the prompt's `OUTPUT_FORMAT` section |
| `""` (empty string) | Adapter returned `Err` with no body (5xx, timeout) | Check adapter logs for 5xx / rate-limit / timeout |
| Long prose without any `DECISION:` line or labeled fields | Model is in chat mode, not following the output format | Strengthen the `OUTPUT_FORMAT` section examples |
| Text with a `DECISION:` line but unknown variant token | Variant token drift or prompt/code mismatch | Diff `prompt_assets/simard/$prompt_name` against the known variant list |
| JSON object (legacy format) | Model following old JSON instructions from cached prompt | Update the prompt to use text format; the parser no longer accepts JSON |
| Partial text ending mid-word | Adapter truncated mid-stream | Check adapter log for `EOF` / `truncated stream` |

If `consecutive_count` is 1 or 2 and the next cycle shows `parse_failure == null`,
the model recovered on its own. No action required.

If `consecutive_count` reaches 3, the daemon has auto-filed a GitHub issue.

## Step 3: Replay the prompt locally

Use the crate-level helpers to test parsing:

```rust
pub fn try_parse_decide_response(raw: &str)
    -> Result<DecideJudgment, SimardError>;
pub fn try_parse_orient_response(raw: &str)
    -> Result<OrientJudgment, SimardError>;
```

Add a one-off test:

```rust
#[test]
fn repro_parse_failure() {
    let raw = "OK"; // <-- paste unescaped payload here
    let result = crate::ooda_brain::try_parse_decide_response(raw);
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
| Chat ack / wrong mode | Edit the `OUTPUT_FORMAT` section of `prompt_assets/simard/ooda_decide.md` (or `ooda_orient.md`). Ensure it specifies `DECISION: <variant>` or labeled-line format, not JSON. See [edit-the-ooda-brain-prompt](edit-the-ooda-brain-prompt.md). |
| Adapter 5xx / rate limit / timeout | Investigate the adapter; the brain itself is healthy. |
| JSON output (legacy format) | The model is following outdated JSON instructions. Update the prompt to clearly specify text format. Remove any `OUTPUT_FORMAT` JSON schema examples. |
| Variant token drift | Diff the prompt against the known variant list in the Rust parser. The variant whitelist is the enum itself — there is no parallel list. |
| Persistent non-cooperative model | Switch the provider in the brain config. |

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

* **Reverting to JSON output format in the prompt.** The parser no longer
  accepts JSON. Adding JSON examples to the prompt will cause models to emit
  JSON, which will be parse-rejected.
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
