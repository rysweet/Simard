# Stale-Deploy Incident Runbook — 100% distill parse-fail + blocked kgpacks-rs goals

This runbook covers a **recurring composite anomaly** in which three
independent overseer signals fire together:

- `overseer-obs:anomaly:distill` / `process:distill_fail` — the distillation
  process reports a **100% parse-fail rate** (`distill_parse_success_rate = 0.0`).
- Several `agent-kgpacks-rs` OODA goals sit **Blocked** with an
  `[OODA-SAFEGUARD]` reason.
- `quality:gym_skipped` — the progressive gym suite is **skipped** every cycle.

The three signals look related but have **two distinct causes**. The common
denominator for the first two is a **stale daemon binary**; the third is a
deliberate configuration. Treat them separately.

## TL;DR

| Signal | Cause | Fix |
|---|---|---|
| Distill parse-fail 100% | Running daemon predates the merged distill-parser fixes | Redeploy from `main` |
| kgpacks-rs goals Blocked (`OODA-SAFEGUARD`) | Running daemon predates the `#2621` engineer pre-mutation-guard fix → engineer loops with no shippable commit → no-progress breaker escalates | Redeploy from `main`, then `simard goal unblock-all` |
| `quality:gym_skipped` | `SIMARD_SKIP_GYM=1` is set in the systemd units (deliberate) | Expected — remove the env var only if a real gym signal is wanted |

## Symptoms

`distill_parse_success_rate` pinned at `0.0`, with the metric context showing
the parser was reached but found nothing to parse:

```json
{"metric_name":"distill_parse_success_rate","value":0.0,
 "context":{"attempt":2,"fact_count":0,"failure_class":"parse-failure",
            "input_count":50,"outcome":"failure","parse_attempted":true,
            "parse_success":false,"recipe_exited_ok":true,
            "recovered_after_retry":false}}
```

The goal board shows the affected goals blocked by the **no-progress breaker**
sentinel (not by an operator, a dependency, or a scope block):

```
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive
   no-action cycles; needs human review
```

## Root cause: a stale daemon binary

The single most common driver of both the distill and the blocked-goal signals
is that **the running `simard` daemon is built from a deploy branch that lags
`main`**, so critical fixes are merged and their issues are closed while the
live daemon keeps reproducing the already-fixed bug.

Confirm the gap:

```bash
# Commit the running daemon was built from (deploy branch tip)
git -C ~/.simard/repo log --oneline -1 <deploy-branch>   # e.g. deploy/main-15

# Is the engineer pre-mutation-guard fix (#2621) in the deployed branch?
git -C ~/.simard/repo merge-base --is-ancestor <2621-fix-sha> <deploy-branch> \
  && echo "deployed" || echo "MISSING — stale deploy"

# Distill-parser fixes merged to main after the deploy tip:
git -C ~/.simard/repo log main --since="<deploy-tip-date>" -i --grep=distill --oneline
```

### Why the distill parser fails

The distillation step feeds ~50 recent memory episodes to the `distill` recipe
and scans its stdout for a `{ "facts": [...] }` object
(`src/memory_consolidation/distillation.rs::parse_recipe_output_full`). The
recipe **exits 0** (`recipe_exited_ok: true`) but its captured stdout is a
Copilot CLI **launch banner** (ANSI-dimmed `… INFO …` timestamp lines)
surrounding — or replacing — the recipe-runner JSON envelope, so the parser
finds no facts object and records `parse_success: false`,
`failure_class: "parse-failure"`, `fact_count: 0`.

Successive fixes harden this exact path:

- `#2512` — recover facts when a launch banner precedes the envelope.
- `#2517` — prefer the populated envelope view so a pretty envelope *behind*
  the banner still yields facts.
- `#2570` — preserve pretty-printed fact content that quotes a launcher substring.
- `#2580` — stop emitting deterministic-default reasoner outcomes on parse failure.

A daemon that predates these will keep failing at 100% even though the fix is
merged. **`#2512` alone is not sufficient** — the later fixes are required.

### Why the kgpacks-rs goals block

This is a **separate** failure that happens to share the stale-deploy root
cause — the distill parse-fail does **not** block the goals.

1. An engineer working an `agent-kgpacks-rs` goal targets a **non-Simard**
   repository and writes a `.simard-engineer-claim` sentinel into the worktree.
2. Before `#2621`, the **pre-mutation guard** mistook that untracked sentinel
   for uncommitted work and tripped, so the engineer never produced a shippable
   commit — an infinite dispatch loop (issue `#2621`, *"blocks all
   agent-kgpacks-rs WS goals"*).
3. Each OODA cycle is therefore classified as **no shippable progress**
   (`outcome_made_no_progress` in
   `src/ooda_actions/goal_session/advance.rs`).
4. After `NO_PROGRESS_BREAKER_THRESHOLD` (3) consecutive no-action cycles, the
   **no-progress breaker** (`src/goal_curation/no_progress_breaker.rs`, added in
   `#2534`) runs the done-gate once, finds no evidence, and **escalates**:
   files a tracking issue and sets the goal `Blocked` with the
   `NO_PROGRESS_BLOCKED_PREFIX` sentinel above.

Because the breaker (`#2534`) *is* in the deployed binary but the guard fix
(`#2621`) is *not*, the daemon can detect the livelock and block the goals but
cannot let the engineer make progress — so they stay blocked.

> Note on a refuted hypothesis: an earlier pass proposed the daemon "sees green
> while distill is at 100%". That framing is an artifact of comparing the live
> daemon against a **stale source checkout**. The distill failure is real and
> non-fatal (OODA continues); it does **not** gate the blocked goals.

## Why gym is skipped (`quality:gym_skipped`)

`SIMARD_SKIP_GYM=1` is set in the systemd units:

```
~/.config/systemd/user/simard-ooda.service   → Environment=SIMARD_SKIP_GYM=1
~/.config/systemd/user/simard-signal.service → Environment=SIMARD_SKIP_GYM=1
```

`gym_runner_bridge::skip_gym()` reads this flag and short-circuits
`run_scenario` / `run_suite` to a synthetic zero-score success tagged
`degraded_sources: ["SIMARD_SKIP_GYM"]`, bypassing the gym engine. This is a
**deliberate cost/time control**, not a bug — the gym has been intentionally
skipped for as long as the flag has been set. Treat `quality:gym_skipped` as an
**expected** state unless a real gym signal is required.

## Recovery

Run in order. Steps P0–P2 clear the distill + blocked-goal signals; P3 is a
gym decision.

**P0 — Redeploy the daemon from `main`.** This ships `#2621` and the full
distill-parser fix stack. Follow [Safe Self-Update](../safe-self-update.md).

```bash
git -C ~/.simard/repo fetch origin main
# Build/promote per safe-self-update, then restart:
systemctl --user restart simard-ooda.service
```

**P1 — Unblock the safeguard-blocked goals.** The `[OODA-SAFEGUARD]` sentinel is
recognised by the bulk-unblock path:

```bash
simard goal unblock-all
```

After the redeploy the engineer can make real progress, and
`HEALTHY_CYCLES_TO_UNBLOCK = 1` clears each marker on the first healthy cycle;
`unblock-all` just accelerates recovery.

**P2 — Verify.**

```bash
# distill parsing recovering (value climbs above 0.0):
tail -n 5 ~/.simard/metrics/metrics.jsonl | grep distill_parse_success_rate
# goals leaving Blocked:
simard goal list | grep -Ei 'kgpacks|blocked'
```

**P3 — Gym (optional).** To restore a real gym signal, remove the env var from
**both** units and reload:

```bash
# edit both unit files to drop Environment=SIMARD_SKIP_GYM=1
systemctl --user daemon-reload
systemctl --user restart simard-ooda.service simard-signal.service
```

Otherwise leave it set and treat the skip as expected.

## Prevention

- **Close the deploy-promotion gap.** The deploy branch must not trail `main` by
  days while `Fixes #…` PRs merge and auto-close their issues. Promote critical
  fixes (engineer guard, distill parser, brain parse-failure handling) to the
  deploy branch as part of merge, or build the daemon from `main`.
- **Add a deploy-staleness check.** Compare the deployed commit against
  `origin/main` during observe and raise an operator signal when the live daemon
  is missing merged fixes whose issues are already closed — this is what let the
  100% distill anomaly and the blocked goals persist unnoticed.

## References

- Distillation parser: `src/memory_consolidation/distillation.rs`
- No-progress breaker: `src/goal_curation/no_progress_breaker.rs`
- No-progress classifier: `src/ooda_actions/goal_session/advance.rs`
- Gym skip fast-path: `src/gym_runner_bridge.rs`
- Related issues: `#2512`, `#2517`, `#2570`, `#2580` (distill parser);
  `#2534` (no-progress breaker); `#2621` (engineer pre-mutation guard);
  `#2619`, `#2622` (distill parse-fail telemetry anomalies)
- Related runbook: [Safe Self-Update](../safe-self-update.md)
