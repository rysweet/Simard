---
title: Diagnose simard merge-pr verdict-parse failures
description: Operator runbook for the gated merge path. simard merge-pr now surfaces a real verdict via the JSON envelope and fails closed to unclear (refused) when no verdict parses. Explains how to tell a real not_ready/unclear verdict from an infra error or a genuine not-ready PR, and why never to coerce unclear/empty into ready.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/recipe-brain-verdict-parsing.md
  - ../reference/text-parsing-wire-formats.md
  - ../reference/pr-finalization-pipeline.md
  - ./triage-stale-pull-requests.md
  - ./diagnose-decide-orient-parse-failures.md
---

# How-to: Diagnose `simard merge-pr` verdict-parse failures

> **Audience:** operators landing PRs through the gated merge path who need to
> read the **merge-readiness-judge** verdict or diagnose an errored merge.
>
> **Prerequisites:** read access to `~/.simard/logs/` on the host;
> `recipe-runner-rs` on `$PATH`; familiarity with the `simard` CLI and `gh`.

`simard merge-pr <N>` runs an objective, never-agentic gate and then an
**agentic merge-readiness judge** backed by `recipe-runner-rs`. The judge reads
the PR and returns one verdict: `ready`, `not_ready`, or `unclear`.

> **The verdict-capture bug is fixed**
> ([#2428](https://github.com/rysweet/Simard/issues/2428) /
> [#2430](https://github.com/rysweet/Simard/issues/2430) /
> [#2435](https://github.com/rysweet/Simard/issues/2435) /
> [#2462](https://github.com/rysweet/Simard/issues/2462) /
> [#2463](https://github.com/rysweet/Simard/issues/2463)).** `RecipeMergeJudge::judge`
> (`src/stewardship/recipe_merge_judge.rs`) invokes `recipe-runner-rs` with
> `--output-format json`, extracts the agent verdict from the envelope, runs the
> shared escalation ladder on a parse-miss, and **fails closed to `unclear`**
> (→ refused) when no verdict parses — never fail-open to `ready`, never
> SUCCESS-without-verdict. This is the same JSON-envelope pattern as the
> engineer-lifecycle brain (#2419). So `simard merge-pr` now surfaces a real
> verdict (or a fail-closed `unclear`, or an explicit infra error) on every run.
> See
> [Recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md#merge-judge-phase-2462).

## Step 1: Read the verdict

On a healthy, up-to-date build, `simard merge-pr <N>` resolves the judge to one
of three verdicts and acts on it:

- **`ready`** — the agentic judge found the PR merge-ready. The merge proceeds
  (subject to the objective gate, which is the sole authority).
- **`not_ready`** — the judge found a genuine blocker. The merge is refused with
  the judge's rationale and any structured blockers.
- **`unclear`** — the judge could not produce a confident verdict (including the
  **fail-closed** case where no verdict parsed even after the escalation ladder).
  The merge authority treats `unclear` as a refusal.

Tail the log for the judge line to see which verdict was surfaced:

```bash
tail -n 200 ~/.simard/logs/rustyclawd.log | grep -E 'merge-judge|merge-readiness'
```

Confirm the per-run outcome in the metric stream — the merge judge emits one
`brain_verdict_parsed_total` event (`phase=merge_judge`, `goal_id=pr-<N>`) per
invocation, on both the parsed and the fail-closed (`defaulted`) branch:

```bash
jq -rc 'select(.metric_name=="brain_verdict_parsed_total")
        | .context | fromjson | select(.phase=="merge_judge")
        | "\(.goal_id) outcome=\(.outcome) detail=\(.outcome_detail) attempts=\(.attempts)"' \
  ~/.simard/metrics/metrics.jsonl | tail -20
```

An `outcome=defaulted` with `detail=default_empty`/`default_malformed` is the
fail-closed `unclear` path: the judge ran but no verdict parsed, so it refused.

> **If you still see `no verdict keyword … raw="Recipe: … SUCCESS …"`, you are
> on a pre-fix binary.** The old symptom — `recipe-runner-rs` invoked in default
> text mode, the summary banner fed to the keyword parser, every merge aborting
> at the infrastructure level — was eliminated by the JSON-envelope fix above.
> It should no longer occur on a current build. Update the binary (`simard
> safe-update`) rather than working around it.

## Step 2: Tell a verdict apart from an infra error

`merge-pr` outcomes fall into three classes. Only the third is an actual tooling
failure:

| Symptom | Class | What it means |
|---------|-------|---------------|
| Merge refused with a `not_ready` rationale (and possibly structured blockers) | **genuine not-ready PR** | The judge evaluated the PR and found a real blocker. Address the blocker, not the tooling. |
| Merge refused with an `unclear` verdict (`outcome=defaulted` in the metric) | **fail-closed judge** | The judge could not produce a confident verdict, including the case where nothing parsed after the ladder. Correct, safe behavior — re-run or verify by hand. |
| `AdapterInvocationFailed: recipe-runner-rs spawn failed` | **infra** | `recipe-runner-rs` is not on `$PATH` / not executable. |
| `AdapterInvocationFailed: recipe exited with <status>` | **infra** | The recipe subprocess crashed; read the captured stderr. |
| `AdapterInvocationFailed: failed to deserialize recipe JSON output` | **infra** | The `--output-format json` envelope was undecodable (e.g. a runner version mismatch). |

A genuine infra error (the bottom three rows) propagates as an `Err` and is
distinct from a fail-closed `unclear`, which is a *surfaced verdict*, not a
crash. The distinction is visible in the metric: an infra error records
`outcome_detail=error`; a fail-closed verdict records
`default_empty`/`default_malformed`.

## Step 3: Decide what to do

- **`not_ready` / a real blocker** — fix the PR (CI, conflicts, missing
  evidence) and re-run `simard merge-pr <N>`. The judge re-evaluates.
- **`unclear` (fail-closed)** — re-run once; transient model wobble often clears
  on the next attempt because the escalation ladder re-prompts. If it persists,
  independently verify the merge-ready criteria
  ([Triage stale pull requests → the deterministic gate](./triage-stale-pull-requests.md#the-deterministic-gate-what-simard-merge-pr-enforces))
  before taking any operator action. **Never** coerce an `unclear` into a merge.
- **Infra error** — fix the environment (install/repair `recipe-runner-rs`,
  check the captured stderr, confirm the runner version emits the JSON envelope),
  then re-run.

## Step 4: Background — the shipped fix

The merge judge applies the #2419 JSON-envelope pattern: it invokes
`recipe-runner-rs --output-format json`, extracts the agent verdict from the
envelope, parses it (`parse_merge_outcome` — structured `{"verdict":…}` JSON
first, then a prose keyword fallback), runs the escalation ladder on a
parse-miss, and **fails closed to `unclear`** when no verdict parses. The
objective deterministic gate (`evaluate_objective_gates`) and the merge
authority remain the sole deciders; the parsed verdict is advisory input. For
the full design see
[Recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md#merge-judge-phase-2462).

## Anti-patterns

- **Treating a fail-closed `unclear` as "the tooling is broken."** It is a
  surfaced verdict, not a crash — the judge ran and chose to refuse rather than
  guess. Re-run or verify by hand; do not bypass the gate.
- **Patching the parser to coerce empty / banner / `unclear` into `ready`.**
  That reintroduces a merge-authority bypass. The judge must **fail closed**
  (`unclear` → refused), never fail open. This invariant is load-bearing.
- **Coercing a genuine `not_ready` into a merge.** Address the blocker the judge
  reported instead.
- **Reaching for a manual `gh pr merge` override as a normal path.** The gated
  path surfaces a real verdict now; an operator merge is a recorded judgment
  call reserved for genuinely exceptional cases, only after independently
  verifying every merge-ready criterion — not a routine workaround.

## See also

- [Reference: Recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md)
- [Reference: Text-parsing wire formats §2b](../reference/text-parsing-wire-formats.md#2b-merge-judge-recipe_merge_judgers)
- [Reference: PR-finalization review pipeline](../reference/pr-finalization-pipeline.md)
- [How-to: Triage stale pull requests](./triage-stale-pull-requests.md)
