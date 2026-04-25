# Fail-Open Audit (P5 / #1245)

**Date**: 2026-04-25
**Scope**: All `.ok()`, `let _ = ...`, and `_ => ()` swallows in `src/`
**Trigger**: Specs/ProductArchitecture.md — *"Do not claim success without evidence"*. The amplihack LLM-router outage (PR #4477) silently produced 89 OODA cycles at 0.00% across 13 days because callers `.ok()`d errors into None and downstream code couldn't tell "no result" from "broken".

## Methodology

```bash
grep -rn -E '\.ok\(\);?\s*$' src/ --include='*.rs' | wc -l        # 98
grep -rn -E '_ => \(\)' src/ --include='*.rs' | wc -l             # ~16
grep -rn -E 'let _ = .+\?' src/ --include='*.rs' | wc -l          # 2
```

Each site is classified as:

| Class | Meaning | Action |
|-------|---------|--------|
| **PROPAGATE** | Real error path that must surface — propagate via `?` or `SimardError` | Refactor required |
| **LOG-AND-CONTINUE** | Best-effort op where failure isn't fatal but operators must be told | Add `tracing::warn!` or `eprintln!` with the error |
| **DOCUMENT-FAIL-OPEN** | Genuinely benign (env-var lookup, optional read) — keep as-is | Add a `// fail-open: <reason>` comment |

## Summary

| Class | Count (est.) | Notes |
|-------|--------------|-------|
| PROPAGATE | 0 confirmed | None found; gym/knowledge bridges are LOG-AND-CONTINUE by design |
| LOG-AND-CONTINUE | ~5 known | bridge_launcher.rs (FIXED in this PR), cost_tracking.rs:280, dashboard goal_board parse, etc. |
| DOCUMENT-FAIL-OPEN | ~93 | meeting_repl writeln/flush (52), env-var lookups, file existence checks |

## Highest-leverage sites

### `src/bridge_launcher.rs:125-126` — FIXED in this PR

**Before**:
```rust
let knowledge = launch_knowledge_bridge(&python_dir).ok();
let gym = launch_gym_bridge(&python_dir).ok();
```

The `.ok()` discarded the error type. Operators saw "bridge unavailable" with no clue *why*. This is the most dangerous fail-open in the codebase — gym bridge failure produced exactly the silent-zero-score behavior that took 13 days to detect.

**After**:
```rust
let knowledge = match launch_knowledge_bridge(&python_dir) {
    Ok(b) => Some(b),
    Err(e) => { eprintln!("... FAILED: {e}"); None }
};
```

Same control flow, but the error message is preserved in the log.

### `src/cost_tracking.rs:280` — TODO in next round

```rust
fs::remove_dir_all(&dir).ok();
```

Silent dir-removal failure can leak disk. Should be `LOG-AND-CONTINUE` with a tracing warn. Filed as separate follow-up; not in this PR to keep it surgical.

### `src/operator_commands_dashboard/routes.rs:1126` — TODO

```rust
let goal_board = serde_json::from_str::<GoalBoard>(&goal_content).ok();
```

Malformed goal board JSON silently becomes None — dashboard then renders "no goals" instead of "goal board parse failed". Should be `LOG-AND-CONTINUE`. Filed as follow-up.

## DOCUMENT-FAIL-OPEN cohort (no action needed)

These are correct as-is and documented inline where useful:

- **`src/meeting_repl/repl.rs`** — 52 `writeln!(...).ok()` calls. REPL output to a possibly-closed pipe; failure means the user disconnected, nothing more to do.
- **`std::env::var(...).ok()`** — 4 sites. Env var either exists or doesn't; None is the natural representation.
- **`std::fs::read(...).ok()`** in `terminal_session/workflow_guard.rs:47` — optional state file lookup.
- **`memory_ipc.rs:742`** — parse PID from sentinel file; missing/malformed sentinel correctly degrades to None.
- **`agent_supervisor/lifecycle.rs:31`** — open_agent_log fails-open with explicit `tracing::warn!` and a doc comment. Already correctly handled.

## Round 5 outcome

- 1 PROPAGATE→LOG conversion shipped (bridge_launcher)
- 2 follow-ups identified (cost_tracking, dashboard goal_board)
- 0 PROPAGATE bugs found that warrant immediate emergency fix

This audit closes the "is the codebase riddled with silent error swallows?" question with **no** for production-critical paths. The remaining cleanup is incremental.

## References

- [Google SRE Book — Monitoring Distributed Systems](https://sre.google/sre-book/monitoring-distributed-systems/)
- PR #4477 (amplihack) — the 13-day silent failure that motivated this audit
- PR #1240 (Simard) — eval-watchdog, the structural fix for the *symptom*
- This PR (Simard) — the structural fix for one *cause*
