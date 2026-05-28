---
title: goal_session_objective.md — Prompt Reference
description: Schema and editing guide for the goal-session objective prompt that controls how Simard triages existing PRs before starting new work.
last_updated: 2026-05-28
owner: simard
doc_type: reference
related:
  - ../concepts/prompt-driven-brain-iteration.md
  - ../howto/edit-the-ooda-brain-prompt.md
  - ./spawn-agent-for-goal.md
---

# `goal_session_objective.md` — Prompt Reference

`prompt_assets/simard/goal_session_objective.md` is the prompt injected into
every **goal-session advance** call. It tells Simard's PM-architect persona
what to do when advancing a goal: how to prioritise existing work over new
work, what response shapes are valid, and how to report progress.

**Module**: `simard::ooda_actions::goal_session::advance`

**Loaded via**: `crate::ooda_brain::prompt_store::global().load("goal_session_objective.md")`

---

## Loading mechanism

The prompt is loaded at **runtime** through the shared `PromptStore`
singleton, the same mechanism used by the three OODA brain prompts. This
means:

| Priority | Source | When used |
|----------|--------|-----------|
| 1 | `$SIMARD_PROMPT_ASSETS_DIR/goal_session_objective.md` | Env var override is set |
| 2 | `$HOME/.simard/prompt_assets/simard/goal_session_objective.md` | Default hot-reload path |
| 3 | Compiled-in `include_str!` fallback | No file on disk |

Editing the file at the resolved path takes effect on the **next
goal-session advance call** — no rebuild, no daemon restart. The
`PromptStore` stats the file once per call and only re-reads when the
mtime has changed.

See [prompt-driven brain iteration](../concepts/prompt-driven-brain-iteration.md)
for the full hot-reload architecture.

---

## Prompt structure

### `# Priority Order`

**Added in [#2152](https://github.com/rysweet/Simard/issues/2152).**

A mandatory triage section that enforces a strict priority order before
Simard may start new implementation work:

| Tier | Action | When |
|------|--------|------|
| 1 | **Merge green PRs** | Existing PRs with passing CI. Command: `gh pr merge --squash --delete-branch` |
| 2 | **Fix failing PRs** | PRs with red CI. Diagnose failure, fix, push. |
| 3 | **Close duplicate PRs** | PRs that overlap with already-merged work. |
| 4 | **New work** | Only when no existing PRs need attention. |

This section exists because the daemon historically created new PRs without
merging existing green ones or fixing failing ones, leading to PR
accumulation (e.g., 9 of 14 open PRs being red).

The priority order is **guidance to the LLM**, not enforced by Rust code.
The advance call site injects the prompt verbatim into the objective, and
the LLM is expected to follow the triage order before spawning new
engineer work.

### `# Two response shapes`

Defines the two valid output formats:

1. **Spawn an engineer** — one paragraph of concrete prose describing what
   the engineer subprocess should do. The paragraph should cite files,
   commands, issue numbers, etc.
2. **No action this cycle** — the literal phrase `NO ACTION` on its own
   line, optionally followed by a prose explanation.

### `# Optional progress update`

An optional `PROGRESS: NN` marker (0–100) anywhere in the response updates
the goal's recorded completion percentage. Both response shapes accept it.

### `# Failure mode`

Only an empty/whitespace-only response fails the cycle. Anything else is
dispatched.

---

## Context variables

The prompt itself contains no `{{placeholder}}` template variables. Instead,
the call site in `advance.rs` wraps it in a structured objective string:

```
Goal '<goal_id>' (<percent>% complete): <goal_description>

<goal_session_objective.md contents, trimmed>

Environment context:
- Git status: <clean | N changed files>
- Open issues: <semicolon-separated open issue titles, or "none">
- Recent commits: <up to 5 most recent commit one-liners, or "none">

Relevant facts from memory:          ← (only when prepared_context exists)
- [concept] content

Triggered reminders:                  ← (only when prospectives matched)
- description: action_on_trigger

Recalled procedures:                  ← (only when procedures matched)
- name: step1 → step2 → …
```

The environment context block is built by `crate::ooda_loop::gather_environment()`
and appended after the prompt contents.

The memory context block (`relevant_facts`, `triggered_prospectives`,
`recalled_procedures`) is appended from `state.prepared_context` when the
memory subsystem returns matches. These sections are omitted entirely when
no memory context is available, so the prompt stays compact for new goals.

---

## Editing guide

### Quick iteration (hot-reload)

```bash
# 1. Edit the on-disk copy
vim ~/.simard/prompt_assets/simard/goal_session_objective.md

# 2. Save — the next goal-session advance call picks up the change
```

### Persisting edits

Local edits to `~/.simard/prompt_assets/` are overwritten by
`scripts/redeploy-local.sh`. To make changes durable:

1. Edit `prompt_assets/simard/goal_session_objective.md` in the repository.
2. Commit and push.
3. `scripts/redeploy-local.sh` syncs the repository copy to `~/.simard/`.

### Common iterations

**Reorder triage priorities:**

Change the numbered list under `# Priority Order`. For example, to
prioritise fixing red PRs over merging green ones:

```diff
-1. **Merge green PRs first.** …
-2. **Fix failing PRs second.** …
+1. **Fix failing PRs first.** …
+2. **Merge green PRs second.** …
```

**Add a new triage tier:**

Insert a new numbered item. Example — add "rebase stale PRs" as tier 3:

```diff
 2. **Fix failing PRs second.** …
+3. **Rebase stale PRs.** If a PR is >7 days old and has merge conflicts,
+   rebase it onto main and push.
 3. **Close duplicate PRs.** …
 4. **New work last.** …
```

**Relax the "no new work" constraint:**

Edit the tier 4 text to allow parallel new work:

```diff
-4. **New work last** — only start new implementation when no existing PRs
-   need attention.
+4. **New work** — may run in parallel with tiers 1–3 if the new work is
+   on a different goal than any open PR.
```

---

## Observability

The `prompt_version` field in cycle reports covers this prompt via the
`PromptStore`'s sha256 tracking. After editing, confirm the version changed:

```bash
jq -r '.brain_judgments[] | select(.phase == "advance") | .prompt_version' \
  ~/.simard/cycle_reports/cycle_*.json | tail -3
```

A changed hash confirms the hot-reload picked up the edit.

---

## Constraints

* The prompt must be valid UTF-8.
* Keep it under ~32 KB (soft guideline for compiled-in fallback size).
* The LLM must output either a prose paragraph (spawn engineer) or
  `NO ACTION` as the first line. The parser in `advance.rs` dispatches
  on this structure — do not change the two-shape contract without a
  coordinated Rust change.
* The `PROGRESS: NN` marker is extracted by regex; do not change the
  marker format without updating the parser.

---

## See also

* [Concept: prompt-driven brain iteration](../concepts/prompt-driven-brain-iteration.md)
* [How-To: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
* [Reference: `spawn_agent_for_goal`](./spawn-agent-for-goal.md)
* [Reference: `PromptStore` API](./ooda-brain-api.md)
