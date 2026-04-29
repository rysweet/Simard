# Prompt-Driven OODA Brain

Simard's OODA daemon delegates the **engineer-lifecycle decision** — what to do
when `spawn_engineer` finds an existing live worktree for a goal — to a
prompt-driven "brain" instead of a hard-coded skip. The decision is reasoned
about by an LLM that reads a markdown prompt; iterating on behavior is a matter
of editing `prompt_assets/simard/ooda_brain.md`,
not writing Rust.

## Why

Before this feature, `dispatch_spawn_engineer` returned
`make_outcome(action, true, "...skipped...")` whenever a live engineer was
detected. Because `success: true` clears `goal_failure_counts`, the
`FAILURE_PENALTY_PER_CONSECUTIVE` cooldown in `orient.rs` never engaged, and
goals could remain in `"engineer alive — skipped"` for hundreds of cycles
without anyone noticing. The brain replaces that single deterministic outcome
with five possible outcomes selected by an LLM reading live context.

## The Five Lifecycle Decisions

The brain returns one of five variants. Each maps to a small, well-defined
side-effect on `OodaState` and a distinctive `ActionOutcome.detail` prefix
(the `ActionOutcome` schema is **unchanged** — only text and side-effects vary).

| Variant | Side-effect | `success` | `detail` prefix |
|---|---|---|---|
| `continue_skipping` | none | `true` | `engineer alive — continue (brain): {rationale}` |
| `reclaim_and_redispatch` | tear down worktree, respawn with `redispatch_context` | depends on respawn | `reclaimed pid {pid}; redispatched: {rationale}` |
| `deprioritize` | `goal_priorities[goal_id] -= 10` (saturating) | `true` | `deprioritized -10: {rationale}` |
| `open_tracking_issue` | append to `<state_root>/pending_issues.jsonl` (a new on-disk queue introduced by this feature) | `true` | `queued tracking issue '{title}': {rationale}` |
| `mark_goal_blocked` | `state.blocked_goals.insert(goal_id, reason)` | `true` | `goal blocked: {reason} ({rationale})` |

## How It Fits Together

```
                ┌─────────────────────┐
cycle.rs ─────► │ build brain (1×)    │  RustyClawdBrain ──or── DeterministicFallbackBrain
                └─────────┬───────────┘
                          ▼
        dispatch_spawn_engineer(action, state, goal_id, task, &brain)
                          │
            ┌─────────────┴────────────┐
            │ live engineer found?      │
            └─────┬─────────────────┬───┘
                  │ no              │ yes
                  ▼                 ▼
            existing spawn    gather_engineer_lifecycle_ctx()
                              ▼
                         brain.decide_engineer_lifecycle(&ctx)
                              ▼
                         apply_lifecycle_decision()
                              ▼
                         ActionOutcome
```

The brain is constructed **once per cycle** and dropped at cycle end. It is not
a global, not an `Arc<Mutex<…>>`, and is not threaded through `OodaConfig`.
This keeps its lifetime obviously scoped and avoids cross-cycle adapter state.

`pending_issues.jsonl` is a write-only sink for now: this PR appends to it but
does not consume it. A follow-up change will add an OODA action that drains the
queue and runs `gh issue create --label ooda-stuck`. Until then, the file is
useful as an audit trail and can be processed manually.

## Backward Compatibility

* If `RustyClawdBrain` cannot be constructed (no API key, no rustyclawd
  subprocess, no network), the daemon falls back to
  `DeterministicFallbackBrain`, which always returns `continue_skipping`. This
  is **byte-identical** to the pre-feature behavior. No panics, no startup
  failure.
* `ActionOutcome` and the on-disk `cycle_reports/*.json` schema are unchanged.
  Reports written by older daemons still deserialize.
* No new environment variables. The brain reuses whatever `RustyClawdAdapter`
  uses today (`AMPLIHACK_LLM_PROVIDER`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
  etc.).

## Future Migrations

This PR establishes the **pattern** at one decision site. The same
`OodaBrain`-style trait and `prompt_assets/simard/ooda_*.md` file convention
will incrementally absorb the other OODA phases:

| Phase | Future prompt | Scope |
|---|---|---|
| Observe | `ooda_observe.md` | Which signals to attend to this cycle |
| Orient | `ooda_orient.md` | How to weight goals given recent failures |
| Decide | `ooda_decide.md` | Which planned action to enact next |
| Curate | `ooda_curate.md` | Goal/improvement curation triage |
| Review | `ooda_review.md` | Engineer-output acceptance |

Each migration follows the same recipe: add a prompt, define a trait + ctx +
decision struct, wire it at one site, keep a deterministic fallback,
preserve the on-disk schema.

## See Also

* [How-to: spawn engineers from the OODA daemon](../howto/spawn-engineers-from-ooda-daemon.md)
* [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
* [Reference: `OodaBrain` API](../reference/ooda-brain-api.md)
* [Reference: `ooda_brain.md` prompt schema](../reference/ooda-brain-prompt.md)
