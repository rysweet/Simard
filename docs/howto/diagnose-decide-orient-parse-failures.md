---
title: Diagnose OODA decide/orient brain parse failures
description: Operator runbook for text-based OODA brain parse failures. Find, classify, and remediate parse failures from the decide and orient phases.
last_updated: 2026-06-29
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

## First, rule out the Copilot launch-log preamble (#2496)

> **Start here when *every* active goal defaults on the same cycle and no
> engineers are spawning.** That all-goals-at-once pattern is the signature of a
> capture-noise stall, not a per-goal model problem.

The Copilot CLI agent binary prepends a **launch-log preamble** and ANSI colour
codes to its stdout before the agent's answer — lines such as:

```
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config
… INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot version="GitHub Copilot CLI 1.0.66-2."
Run 'copilot update' to check for updates.
```

These lines carry no ISO-8601 timestamp, so they were not caught by the original
log-line filter. Left in place, the decide parser's first token became
`ℹ`/`Run`/`1.0.66-2` instead of an action keyword, **every** goal classified as
`default_malformed`, the escalation ladder exhausted, decide returned its
no-new-action default, and Simard spawned zero engineers — a deadlock, since the
parse failure is exactly what blocks spawning the engineer that would fix it.

**This is handled automatically.** The shared extractor
(`recipe_output::strip_recipe_noise`, [#2484](https://github.com/rysweet/Simard/issues/2484)
/ [#2496](https://github.com/rysweet/Simard/issues/2496)) now strips the launcher
preamble — via the `is_copilot_launcher_line` arm of `is_noise_line` — before any
decide/orient/lifecycle/merge-judge/distill parse. See
[Reference: text-parsing wire formats § Protocol 0](../reference/text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output)
and [Concept: Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md).

### Confirm the launcher preamble is being stripped

The `default_malformed` / `ladder_exhausted` rate should be near zero. Read the
shared parse-success counter:

```bash
jq -rc 'select(.metric_name=="brain_verdict_parsed_total")
        | .context | fromjson | "\(.phase) \(.outcome) cause=\(.cause)"' \
  ~/.simard/metrics/metrics.jsonl \
  | sort | uniq -c
```

A healthy daemon shows `decide parsed` and `orient parsed` dominating. A surge of
`decide defaulted cause=ladder_exhausted` across many goals on the **same** cycle
is the launcher-preamble stall signature.

To confirm what the agent actually emitted, read the captured raw output for one
defaulted decision — the preamble (if present) is visible in
`raw_response_truncated` in the cycle report (see Step 2 below). If the cleaned
first token is now an action keyword but the raw still shows the preamble, the
extractor is doing its job.

### Telling a parse-failure default from a real "no action"

A deterministic default reached because the ladder exhausted on a parse miss is
**not** the model deciding to do nothing, and the logs now say so explicitly
(#2496). Two log lines appear, with stable prefixes you can grep for.

First, the escalation ladder records that it ended without a parseable decision
and names the termination cause:

```
WARN simard::ooda_brain: brain escalation ladder ended without a parseable decision; deterministic default
    goal="<id>" attempts=3 base_outcome="default_malformed" termination=Exhausted
[simard] BRAIN ESCALATION goal=<id> ladder ended (ladder_exhausted) after 3 attempts — deterministic default
```

Then, when that ladder exhaustion (or an `InvokeError`) leaves the phase on a
parse-failure default, the phase emits a **second, distinct** line that the
#2496 fix added specifically to keep a parse-failure default from reading like a
real "no action". For **decide** and **orient**:

```
WARN simard::ooda_brain: brain phase fell to its deterministic default via a PARSE FAILURE (ladder ladder_exhausted) — NOT a model 'no action' decision; a transient parse miss, re-evaluated next cycle (issue #2496)
    phase=decide goal=<id> outcome_detail=default_malformed cause=ladder_exhausted
[simard] BRAIN PARSE-FAILURE DEFAULT phase=decide goal=<id> outcome=default_malformed cause=ladder_exhausted (transient miss, NOT a real no-action decision)
```

For the **engineer-lifecycle** phase, the `continue_skipping` default reached
this way is logged as a **transient parse-failure skip, re-evaluated next cycle —
NOT a deliberate NO-ACTION**:

```
WARN simard::ooda_brain: engineer-lifecycle fell to continue_skipping via a PARSE FAILURE (ladder ladder_exhausted) — a TRANSIENT parse-failure skip, re-evaluated next cycle, NOT a deliberate NO-ACTION (issue #2496)
    goal=<id> outcome_detail=default_malformed cause=ladder_exhausted
[simard] LIFECYCLE PARSE-FAILURE SKIP goal=<id> cause=ladder_exhausted (transient, re-evaluated next cycle — NOT a deliberate no-action)
```

Grep the `BRAIN PARSE-FAILURE DEFAULT` / `LIFECYCLE PARSE-FAILURE SKIP` prefixes
to count parse-failure defaults without false-matching the model's real
no-action decisions. In the metrics, the same difference is unambiguous:

- Real decision → `brain_verdict_parsed_total{outcome=parsed}` (or lifecycle
  `brain_lifecycle_decision{is_parse_failure=false, cause=ok|ladder_recovered}`).
- Parse-failure default → `outcome=defaulted`, `is_parse_failure=true`,
  `cause=ladder_exhausted` (or `ladder_invoke_error`).

If you see the parse-failure cause, treat the goal as **not yet decided** — it
will be re-evaluated on the next clean cycle — rather than as a goal the brain
chose to leave idle.

### If the stall recurs (the launcher reshaped its banner)

If a future Copilot CLI release changes the preamble so a **new** shape slips
through, the symptom returns as an all-goals `default_malformed` surge with the
preamble visible in `raw_response_truncated`. The fix is a one-line extension of
`is_copilot_launcher_line` in `src/recipe_output/extract.rs` (the single
chokepoint) plus a regression test pinning the new sample — never a per-parser
patch. File against the launcher-preamble surface and reference
[#2496](https://github.com/rysweet/Simard/issues/2496).

## Decide brain: first-word extraction

> **Updated in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> The decide brain extracts the first word from the recipe output and matches
> it case-insensitively against 10 action keywords. Parse failures in the
> traditional sense (format rejected) **cannot occur** — the first-word
> parser always returns a valid action kind. If no keyword matches, the
> default `advance_goal` is used.
>
> **Transport (fixed in [#2421](https://github.com/rysweet/Simard/issues/2421)).**
> The decide brain reads the agent decision from the `recipe-runner-rs
> --output-format json` envelope (`step_results[].output`), **not** the
> text-mode summary banner. On a parse-miss it runs the escalation ladder and
> only then falls back **loudly** to `advance_goal`, and each invocation emits a
> `brain_verdict_parsed_total` metric (`phase=decide`). The banner first word
> `Recipe:` can no longer be mistaken for the decision. See
> [Recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md#decide-phase-2421).


### Decide-brain failure modes

The decide brain can still fail at the **infrastructure** level:

| Failure | Log signature | Action |
|---------|--------------|--------|
| `recipe-runner-rs` not found | `[ooda] recipe-runner-rs not found; using deterministic decide fallback` | Install `recipe-runner-rs` or verify `$PATH`. |
| Recipe subprocess exits non-zero | `ERROR simard::ooda_brain: recipe_decide invocation failed` + stderr | Check the recipe YAML syntax and the agent's error output. |
| Recipe YAML not found | `RecipeBrain::new() returned None` | Verify `prompt_assets/simard/recipes/ooda-decide.yaml` exists. |

When `RecipeBrain` fails to construct or the subprocess fails, the
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

## Orient brain: first-float extraction

> **Updated in [#2144](https://github.com/rysweet/Simard/issues/2144).**
> The orient brain now extracts the first bare decimal from the recipe output
> instead of parsing a JSON object. Parse failures still fire four visibility
> channels. If no float is found, the deterministic floor applies.
>
> **Transport (fixed in [#2421](https://github.com/rysweet/Simard/issues/2421)).**
> The decimal is read from the `--output-format json` envelope's agent output,
> **not** the text-mode banner. Because the envelope carries no banner, the
> banner's `(0.0s)` timing string can no longer be scraped as `adjusted_urgency`
> — urgency can no longer be silently demoted to `0.0`. On a parse-miss the
> escalation ladder runs and the deterministic floor (`base_urgency − 0.2 ×
> failure_count`) is the only fallback; each invocation emits a
> `brain_verdict_parsed_total` metric (`phase=orient`). See
> [Recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md#orient-phase-2421).

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
    error="no float found in LLM response (got 3 bytes)"
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
| Leading `ℹ NODE_OPTIONS=…`, `… launching copilot binary=… version="GitHub Copilot CLI …"`, or `Run 'copilot update'…` before the real answer | Copilot launch-log preamble (#2496) leaked into the capture | None for a known shape — the shared extractor strips it. If the preamble is a **new** shape and the parse still missed, extend `is_copilot_launcher_line` (see [§ launch-log preamble](#first-rule-out-the-copilot-launch-log-preamble-2496)). |
| `"OK"`, `"continue"`, `"yes"` | Model ignored the output instruction; emitted a chat ack | [Step 3 — replay the prompt](#step-3-replay-the-prompt-locally); strengthen the prompt's output instructions |
| `""` (empty string) | Adapter returned `Err` with no body (5xx, timeout) | Check adapter logs for 5xx / rate-limit / timeout |
| Long prose without any number | Model is in chat mode, not following the output format | Strengthen the prompt's OUTPUT_FORMAT section to require a bare decimal as the first token |
| Decimal number but out of range | Model emitted a valid float but outside `[0.0, base_urgency]` | Check the validation logic; the deterministic floor will have been applied |
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
| Chat ack / wrong mode | Edit the orient recipe YAML to strengthen the output instruction (bare decimal as first token). See [edit-the-ooda-brain-prompt](edit-the-ooda-brain-prompt.md). |
| Adapter 5xx / rate limit / timeout | Investigate the adapter; the brain itself is healthy. |
| Float out of range | Check that `base_urgency` in the prompt context is correct. The deterministic floor will have been applied. |
| Persistent non-cooperative model | Switch the provider in the brain config. |

> **Note:** The decide and lifecycle brains no longer have parse failures.
> They use first-word extraction, which always returns a valid result. If the
> decide brain is producing unexpected routing, edit
> `prompt_assets/simard/recipes/ooda-decide.yaml` — no rebuild required.
> See [OODA decide recipe and prompt schema](../reference/ooda-decide-prompt.md).

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
  now expects a bare decimal, not JSON. Adding JSON examples will cause models
  to emit JSON objects, and the float may not be found as the first token.
* **Adding a `DECISION:` marker format to any recipe prompt.** No brain uses
  the marker protocol anymore. Adding `OUTPUT_FORMAT` sections with
  `DECISION:` instructions is unnecessary and may confuse the agent.
* **Adding keyword-anywhere instructions.** The decide and lifecycle brains
  use first-word extraction only. Instructing the model to "mention the
  keyword in your response" will cause it to bury the keyword in prose,
  which will not be found.
* **Restarting the daemon directly** to "clear" parse failures. Use
  `simard safe-update` for any code/prompt change.
* **Suppressing the `ERROR` log line** with a tracing filter.
* **Closing the auto-filed issue without fixing the cause.**

## See also

* [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md) — normative grammar for all text protocols.
* [Concept: text-based brain protocol](../concepts/text-based-brain-protocol.md) — design rationale.
* [Concept: Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md) — why the launcher noise is stripped at one chokepoint, and why a parse-failure default stays distinct from a real "no action".
* [Reference: recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md) — shared transport, escalation ladder, and termination-cause telemetry.
* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md) — engineer-lifecycle wire format.
* [How-to: diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md) — engineer-lifecycle equivalent.
* [How-to: edit the OODA brain prompt](./edit-the-ooda-brain-prompt.md) — prompt editing guide.
