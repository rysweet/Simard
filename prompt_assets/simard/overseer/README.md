# Overseer prompt/recipe scaffolding (#2419)

> **Status: design scaffolding — NOT wired live.** These prompt templates are part
> of the Overseer operator/observer design spike. No recipe or code path loads
> them yet. They pin down the intended prompt surface so a later milestone can
> wire them behind an env flag without redesigning. See
> [`docs/design/overseer.md`](../../../docs/design/overseer.md).

## What is here

| File | Meta-OODA phase | Input → output |
|------|-----------------|----------------|
| [`observe.md`](./observe.md) | Observe + Orient | `StatusSnapshot` + signals + in-flight → prioritized `Problem`s (deduped) |
| [`problem_to_brief.md`](./problem_to_brief.md) | Decide | one `Problem` → a `smart-orchestrator` `task_description` |
| [`pr_verify.md`](./pr_verify.md) | Act (verify) | PR body + CI + diff → merge-ready verdict + checklist |
| [`deploy_gate.md`](./deploy_gate.md) | Act (deploy, HIGH-RISK) | target/deployed commit + canary → deploy/propose/hold |
| [`health_review.md`](./health_review.md) | Observe→Decide→Act (self-heal) | journal + status + goal list → `LaunchRecipe`/`EscalateBlockedGoal` decisions |
| [`investigate_stale_engineer.md`](./investigate_stale_engineer.md) | Observe→Decide (investigate-before-reap) | archived evidence for a quiet/idle engineer claim → `verdict` (still-alive/blocked/recoverable/pending/dead) + `interventions`, so the reaper reaps only a genuinely-dead engineer |

## Reuse the existing recipes — do not reinvent

The Overseer **drives fixes through the recipes Simard already uses**. The prompts
above only *decide what to launch and whether to merge/deploy*; the actual work is
done by existing recipes and code:

- **`smart-orchestrator` → `default-workflow`.** The Overseer launches fixes the
  exact way engineers do, via the recipe runner:

  ```bash
  amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
    -c task_description="<from problem_to_brief.md>" \
    -c repo_path=.
  ```

  This is the same entrypoint used at `prompt_assets/simard/engineer_system.md`
  and invoked from `src/bin/simard_engineer_loop_recipe.rs`. `smart-orchestrator`
  classifies the task and routes Development work through `default-workflow` (the
  22-step DEFAULT_WORKFLOW). The Overseer never hand-rolls a workflow.

- **`recipe-runner-rs` + `AMPLIHACK_AGENT_BINARY`.** For structured single-shot
  brain calls (e.g. rendering `observe.md`/`pr_verify.md` to JSON) the Overseer
  reuses the subprocess pattern in `src/stewardship/recipe_merge_judge.rs` and
  parses output with `src/recipe_output/extract.rs`.

- **Quality-audit recipe.** `RunAudit` reuses the existing
  [`monthly-self-quality-audit.yaml`](../recipes/monthly-self-quality-audit.yaml)
  recipe and `crate::self_quality_audit::run_self_quality_audit` — the same
  crusty-old-engineer-gated audit loop, invoked on demand instead of only monthly.

- **Merge / deploy / issues / goals / meetings.** These map to existing code, not
  prompts: `stewardship::merge_pr_if_merge_ready`, `self_deploy` + `self_relaunch`,
  `stewardship::process_orchestrator_run`, `goal_curation`, and
  `meeting_repl::run_meeting_repl`. See the reuse map in the design doc.

## Placeholders

Prompt bodies use `{placeholder}` variables (matching the house style of the other
`prompt_assets/simard/*.md` prompts) that a recipe would substitute via
`-c key=value`. Values are always sanitized (`sanitization::sanitize_terminal_text`)
before substitution; untrusted content never appears in a metric name or an
`@mention`/`#ref` position.
