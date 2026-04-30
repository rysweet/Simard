# Prompt-driven brain iteration (hot-reload)

The three prompt-driven OODA brains — **act** (`RustyClawdBrain`, PR #1458),
**decide** (`RustyClawdDecideBrain`, PR #1469), and **orient**
(`RustyClawdOrientBrain`, PR #1471), all wired in [#1474] — load their
prompt text from disk on every OODA cycle. Editing a prompt takes effect on
the **next cycle**; no rebuild, no daemon restart.

This realises the standing project goal: *iterate on prompts, not code*.

## Where prompts live

The daemon resolves the prompt-asset directory in this order:

1. `$SIMARD_PROMPT_ASSETS_DIR` (override; useful for development worktrees).
2. `$HOME/.simard/prompt_assets/simard/` — the default. `scripts/redeploy-local.sh`
   syncs the repository's `prompt_assets/` tree here on every redeploy.
3. Compile-time embedded fallback baked into the binary via `include_str!`.
   This safety net guarantees the daemon never fails to start because a
   prompt file was deleted.

The resolved path is logged at daemon startup, e.g.

```text
[simard] OODA daemon: prompt_assets dir = /home/USER/.simard/prompt_assets/simard (3 prompts hot-reloadable)
```

## The three hot-reloadable prompts

| File              | Brain                        | Decision site                                                      |
| ----------------- | ---------------------------- | ------------------------------------------------------------------ |
| `ooda_brain.md`   | `RustyClawdBrain` (act)      | Engineer-lifecycle skip branch — keep skipping, reclaim, deprioritise, open issue, or block. |
| `ooda_decide.md`  | `RustyClawdDecideBrain`      | Map a prioritised goal to an `ActionKind` (advance, improve, consolidate, ...). |
| `ooda_orient.md`  | `RustyClawdOrientBrain`      | Demote per-goal urgency in proportion to consecutive failures.     |

## How hot-reload works

Each brain reads its prompt fresh per call via a shared `PromptStore`
singleton. The store stats the file once per call (cheap) and only re-reads
when the mtime has changed. So the steady-state cost is one `metadata()`
syscall per brain per cycle.

Touching a prompt file (`touch`, any editor save) bumps mtime and the next
brain invocation picks up the new contents.

## Iteration workflow

1. Edit the prompt at `~/.simard/prompt_assets/simard/<name>.md`.
2. Save.
3. Watch `~/.simard/daemon.log` — the new behaviour appears on the next
   OODA cycle.

For a full rebuild + redeploy (binary changes), run
`scripts/redeploy-local.sh`. The redeploy step also re-syncs prompt assets
from the repository, so any local edits to `~/.simard/prompt_assets/` are
overwritten by the repository copy on redeploy. Commit prompt changes back
to the repository to make them durable.

[#1474]: https://github.com/rysweet/Simard/pull/1474

## Observability: `prompt_version` in cycle reports

Every prompt-driven brain judgment recorded in
`~/.simard/cycle_reports/cycle_*.json` carries a `prompt_version` field —
the first 12 hex chars of the sha256 of the prompt content that produced
the judgment. Example entry:

```json
{
  "phase": "decide",
  "context_summary": "goal_id=ship-v1 urgency=0.900",
  "decision": "advance_goal",
  "rationale": "high priority",
  "confidence": 1.0,
  "fallback": false,
  "prompt_version": "a1b2c3d4e5f6"
}
```

Use this as a hot-reload sensor without injecting markers into the prompt:

1. Note the current version, e.g.
   `jq '.brain_judgments[].prompt_version' ~/.simard/cycle_reports/cycle_*.json | tail -3`.
2. Edit `~/.simard/prompt_assets/simard/<name>.md` and save.
3. Wait one OODA cycle.
4. The next `cycle_*.json` shows a different `prompt_version` — that is the
   on-disk hot-reload (PR [#1475]) confirmed empirically.

Fallback / deterministic paths emit an empty `prompt_version` (omitted from
JSON) — observers should read "no field" as "no prompt was involved in
this judgment".

[#1475]: https://github.com/rysweet/Simard/pull/1475
