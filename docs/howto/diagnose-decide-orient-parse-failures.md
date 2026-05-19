---
title: Diagnose OODA decide/orient brain parse failures
description: Operator runbook for the silent-fallback fix in #1890. Find, classify, and remediate JSON-parse failures from the decide and orient phases.
last_updated: 2026-05-19
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

Before [#1890](https://github.com/rysweet/Simard/issues/1890) shipped, the `decide_with_brain` and `orient_with_brain` call sites in `simard::ooda_loop` would respond to an LLM brain JSON-parse failure by emitting a single `WARN` line — `no JSON object found in LLM response (got N bytes)` — and then silently substituting `DeterministicFallbackDecideBrain` / `DeterministicFallbackOrientBrain`. The cycle continued, `cycle_N.json` recorded a perfectly normal-looking deterministic decision, and the goal stalled invisibly.

That silent-fallback path is now closed. Parse failures fire four visibility channels in lock-step. This guide tells you how to find them, read them, and decide what to do.

For the full schema and contract, see [Reference: OODA Brain Parse-Failure Record](../reference/ooda-brain-parse-failure-record.md). The engineer-lifecycle equivalent (#1711) is covered by [Diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md) — this page is its sibling for the decide / orient phases.

## Step 1: Find the failing cycle

Symptoms that justify reading parse-failure evidence:

* A goal's `consecutive_skip` or "no progress" counter climbs every cycle even though the dashboard says recent decisions succeeded.
* `~/.simard/cycle_reports/cycle_*.json` shows `brain_judgments[].fallback == true` for `decide` or `orient` on consecutive cycles and the new `parse_failure` block on the same record is non-null. (Pre-#1890 cycles produced the same `fallback == true` shape but no `parse_failure` key — that's the discriminator. The deterministic-bootstrap path, which is legitimate, also omits `parse_failure`.)
* The metric jsonl shows non-zero `brain_parse_failure` counters.
* A GitHub issue titled `OODA decide brain parse failure: goal=<id> (N consecutive)` was auto-filed against the `ESCALATION_REPO_SLUG` repo.

### Tail the structured log

The daemon writes to `~/.simard/logs/` on the host. Look for the constant `brain.decide parse failed` / `brain.orient parse failed` message strings — both are at `ERROR` level:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'brain\.(decide|orient) parse failed'
```

A matching line looks like:

```
ERROR simard::ooda_brain: brain.decide parse failed
    phase="decide"
    goal_id="improve-amplihack-test-coverage"
    error="base type \"ooda-brain\" failed during invocation: no JSON object found in LLM response (got 3 bytes)"
    raw_response_truncated="OK"
    prompt_name="ooda_decide.md"
    prompt_version="a1b2c3d4e5f6"
    consecutive_count=2
    retry_attempted=false
```

`raw_response_truncated` is the **complete** model response, truncated only at 8 KiB on a UTF-8 boundary (with a `…(truncated, total N bytes)` suffix if it overflowed). `prompt_name` and `prompt_version` together identify the exact prompt bytes the model saw — `cat $SIMARD_PROMPT_ASSETS_DIR/$prompt_name` recovers them. If you still see the legacy `WARN simard::ooda_brain: … no JSON object found in LLM response (got N bytes)` line **without** the `ERROR` companion, the daemon is running a pre-#1890 build; finish reading this guide and then run `simard safe-update` to pick up the fix.

### Check the metric stream

The `brain_parse_failure` counter lands in `~/.simard/metrics/metrics.jsonl` (single append-only file; not date-partitioned):

```bash
jq -c 'select(.metric_name == "brain_parse_failure")
       | .context |= fromjson' \
   ~/.simard/metrics/metrics.jsonl \
  | tail -20
```

Each event carries `metric_name`, `value`, `timestamp`, and a `context` field that is itself a JSON string encoding `{ phase, goal_id, retry_attempted, consecutive_count }`. The `|= fromjson` step in the jq filter unwraps the inner object so the dimensions are queryable. Use this stream for "rate of failures over the last hour" questions; use the log for "what exactly did the model say" questions.

### Read the cycle report

Every parse-failed cycle now carries a populated `parse_failure` block on the corresponding `BrainJudgmentRecord`:

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

Healthy cycles have `parse_failure == null` (the field is omitted via `skip_serializing_if`, so older `cycle_*.json` files are unaffected). To list every failed cycle in the report directory:

```bash
for f in ~/.simard/cycle_reports/cycle_*.json; do
  if jq -e '[.brain_judgments[] | select(.parse_failure != null)] | length > 0' "$f" >/dev/null; then
    echo "$f"
  fi
done
```

## Step 2: Classify the response

Open the `raw_response_truncated` value (from the log, the metric, or the cycle report — all three carry the same string) and match against this triage table.

| `raw_response_truncated` looks like…                       | Likely cause                                                               | Action                                                                                                                       |
|------------------------------------------------------------|----------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------|
| `"OK"`, `"continue"`, `"yes"`                              | Model ignored the structured-output instruction; emitted a chat ack         | [Step 3 — replay the prompt](#step-3-replay-the-prompt-locally); then strengthen the prompt's role section.                  |
| `""` (empty string)                                        | Adapter returned `Err` with no body (5xx, timeout, missing adapter)        | Check the adapter logs in `~/.simard/logs/rustyclawd.log` for a 5xx / rate-limit / timeout / "adapter unavailable" line immediately preceding the parse-failure event. `error_message` will name the underlying `SimardError` variant. |
| Long prose without any `{` character                       | Model is in chat mode instead of structured-output mode                    | Check that the brain's adapter forces JSON mode for the configured provider.                                                 |
| JSON that *looks* valid but parses with the same error     | Field rename or `choice` casing drift in the prompt                        | Diff `prompt_assets/simard/$prompt_name` (the value of the `prompt_name` field) against the brain's expected fields; the prompt is the contract. |
| Markdown fence-wrapped JSON                                | The decide / orient parser is stricter than the engineer-lifecycle one     | Open a bug — the decide/orient parser is intended to tolerate fenced JSON the same way `parse_decision_from_response` does.   |
| Partial JSON ending mid-object                             | Adapter truncated mid-stream                                               | Look for an `EOF` / `truncated stream` line in the adapter log; investigate the adapter, not the brain.                      |

If `consecutive_count` is 1 or 2 and the next cycle's record shows `parse_failure == null`, the model recovered on its own and no action is required — the visibility channels exist so transient failures are *visible*, not *suppressed*.

If `consecutive_count` reaches 3, the daemon has already auto-filed a GitHub issue (see Step 4); treat the failure as persistent.

## Step 3: Replay the prompt locally

There is no dedicated dry-run subcommand. To confirm a hypothesis without waiting for the next cycle, use the two crate-level helpers this PR exposes for exactly this purpose:

```rust
pub fn try_parse_decide_response(raw: &str)
    -> Result<DecideJudgment, SimardError>;
pub fn try_parse_orient_response(raw: &str)
    -> Result<OrientJudgment, SimardError>;
```

Both helpers wrap the same parse path that `RustyClawdDecideBrain::run` / `RustyClawdOrientBrain::run` use internally. They are `pub(crate)` in the always-shipped build and re-exported `pub` under `#[cfg(any(test, feature = "ooda-brain-replay"))]`, so they're available to integration tests and ad-hoc replay binaries without widening the always-shipped public API.

1. Copy `raw_response_truncated` from the cycle report (it round-trips through JSON, so newlines and quotes are already escaped — use `jq -r '.brain_judgments[].parse_failure.raw_response_truncated // empty'` to unescape).
2. Add a one-off test to `src/ooda_brain/decide_tests.rs` (or `orient_tests.rs`):

   ```rust
   #[test]
   fn repro_1890_OK_payload() {
       let raw = "OK"; // <-- paste unescaped payload here
       let result = crate::ooda_brain::try_parse_decide_response(raw);
       eprintln!("{result:?}");
   }
   ```

3. Run with `cargo test repro_1890_OK_payload -- --nocapture`.

Discard the test before committing. The parsers have no I/O dependencies, so the replay is faithful — they read the same bytes you captured in the cycle report and return the same `SimardError` variant the call site saw in production.

If you need to reproduce an *adapter*-class failure (where `raw_response_truncated == ""` because no body was returned), the replay helpers won't help — there is no body to parse. Investigate the adapter log directly; the `error_message` field of the `parse_failure` block names the underlying `SimardError` variant for cross-reference.

## Step 4: Read the auto-filed issue (if any)

When `consecutive_count` reaches `ISSUE_ESCALATION_THRESHOLD` (currently 3), the daemon files a GitHub issue via:

```
gh issue create
    --repo rysweet/Simard            # compile-time ESCALATION_REPO_SLUG
    --title "OODA decide brain parse failure: goal=<id> (3 consecutive)"
    --body-file <NamedTempFile>      # 0600 on Unix
    --label ooda-brain-parse-failure
    --label auto-filed
```

The body is a markdown document containing:

* The full `ParseFailureRecord` as a fenced JSON block.
* A pointer to the relevant `cycle_N.json` filenames.
* A short hand-off paragraph noting the threshold and how to silence further escalations (resolve the upstream cause; the counter resets on the next successful parse).

Find the issue:

```bash
gh issue list --repo rysweet/Simard \
  --label ooda-brain-parse-failure --label auto-filed \
  --state open
```

(Forks of this codebase that rebuilt with a different `ESCALATION_REPO_SLUG` constant in `parse_failure.rs` should substitute that slug above. The slug is **not** runtime-configurable — see the reference's [Configuration section](../reference/ooda-brain-parse-failure-record.md#configuration) for why.)

The escalation is **throttled per `(phase, goal_id)`**: one issue per crossing of the threshold, not one per failure. If the same goal continues to fail past the threshold, the counter keeps climbing but no second issue is filed until a successful parse resets it and a fresh streak of three occurs.

## Step 5: Pick a remediation

| Cause from Step 2                          | Remediation                                                                                                                |
|--------------------------------------------|----------------------------------------------------------------------------------------------------------------------------|
| Chat acknowledgment / wrong mode           | Edit `prompt_assets/simard/ooda_decide.md` (or `ooda_orient.md`) to strengthen the "respond ONLY with a JSON object" instruction. The `prompt_name` field of the failed `parse_failure` block names the exact file. See [edit-the-ooda-brain-prompt](edit-the-ooda-brain-prompt.md). |
| Adapter 5xx / rate limit / timeout         | Investigate the adapter; the brain itself is healthy. The four channels exist precisely so you can attribute the failure.   |
| Fence-wrapped JSON the parser rejected     | File a bug — the parser should match `parse_decision_from_response`'s leniency.                                              |
| Drift in prompt vs. expected fields        | Diff the prompt against the `DecideJudgment` / `OrientJudgment` struct; the struct is the contract.                          |
| Persistent unparseable noise from one model| Switch the provider in the brain config; the parser cannot fix a fundamentally non-cooperative model.                       |

After editing a prompt, **do not** restart the daemon by hand. The prompt-asset directory under `$HOME/.simard/prompt_assets/simard/` is mtime-watched (see [Reference: prompt-driven OODA brain](../concepts/prompt-driven-ooda-brain.md)) so a file edit takes effect on the next cycle — but if the cause was code rather than prompt, you must rebuild and hot-swap:

```bash
simard safe-update
```

`safe-update` rebuilds, drains in-flight cycles, hot-swaps the binary, and verifies the new daemon is responsive — see [safe-self-update](../safe-self-update.md).

## Step 6: Verify the fix

After `safe-update` completes:

```bash
tail -F ~/.simard/logs/rustyclawd.log \
  | grep -E 'goal_id="<goal-id>"'
```

You should see:

* The next decide or orient cycle for the affected goal produce a non-`fallback` decision with a substantive rationale (not the legitimate `"deterministic fallback"` sentinel that fires on the no-LLM bootstrap path).
* The `brain_parse_failure` metric stop incrementing for `(phase, goal_id)`.
* The next `cycle_N.json` for the goal omit the `parse_failure` key entirely (`skip_serializing_if` drops it when there is no failure).
* The counter reset — confirm by waiting for the next failure (if any) and checking that `consecutive_count` starts at 1, not at the prior value.

To close the auto-filed GitHub issue:

```bash
gh issue close <issue-number> --comment \
  "Resolved by prompt fix in <commit>. Counter reset confirmed in cycle_<N>.json."
```

## Anti-patterns

The following patterns indicate the operator is **fighting** the visibility contract rather than diagnosing it. Stop and re-read the [Parse-Failure Record reference](../reference/ooda-brain-parse-failure-record.md) before proceeding:

* **Restarting the daemon directly** (`kill <pid>`, `systemctl --user restart simard-ooda`) to "clear" parse failures. The counters are process-local but the cycle reports and metric jsonl are not; a bare restart loses the in-memory streak (potentially un-throttling the next `gh issue create`) but does **not** delete on-disk evidence. Always go through `simard safe-update` for any remediation that requires new code; for evidence-only investigation, do not restart at all.
* **Suppressing the `ERROR` log line** with a `tracing` filter to "calm down" the dashboard. The line is the primary visibility channel; suppressing it returns to the pre-#1890 silent-fallback behavior with extra steps.
* **Closing the auto-filed GitHub issue without fixing the cause.** The issue is throttled per streak; closing it does not reset the counter. If you close without fixing, the next failure beyond the threshold will not file a fresh issue until a successful parse intervenes — losing the very escalation the throttle was designed to provide.
* **Adding a `match` arm to silently rerout a specific raw response** in `decide_with_brain` / `orient_with_brain`. The single fail-loud path is intentional; any per-raw-response branch is the silent fallback pattern returning under a new name.
* **Removing `DeterministicFallbackDecideBrain` / `DeterministicFallbackOrientBrain` because "they look like silent fallback now."** They are the no-LLM bootstrap path; their use as silent error-swallow was what #1890 closed. Broader removal is the scope of [#1748](https://github.com/rysweet/Simard/issues/1748).

## See also

* [Reference: OODA Brain Parse-Failure Record](../reference/ooda-brain-parse-failure-record.md) — schema, channels, and contracts.
* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md) — engineer-lifecycle wire format (#1711).
* [How-to: diagnose OODA brain decision parse failures](./diagnose-brain-decision-parse-failures.md) — engineer-lifecycle equivalent of this runbook.
* [How-to: edit the OODA brain prompt](./edit-the-ooda-brain-prompt.md) — the canonical change surface for prompt fixes.
* [Fail-Open Audit (P5 / #1245)](../fail-open-audit.md) — broader audit context.
* [safe-self-update](../safe-self-update.md) — the only supported way to roll a daemon to new code.
