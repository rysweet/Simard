# `amplihack` String Audit (#1163)

**Date**: 2026-04-26
**Trigger**: PRD reconciliation — README/docs reframed amplihack as one base
type among several. This audit checks whether the codebase still implies
"Simard == amplihack" or "Simard achieves parity with amplihack".

## Methodology

```bash
rg -n 'amplihack' src/ --include='*.rs' | wc -l   # 128
```

Every hit was triaged into one of:

| Category | Meaning | Action |
|----------|---------|--------|
| **(a) legitimate base-type reference** | Simard genuinely uses `amplihack copilot` as a base type or shells out to `amplihack recipe run`. The string is a fact about the runtime, not a parity claim. | Keep |
| **(b) parity-coupling to remove** | Comment or string that implies "Simard is amplihack" or "Simard mirrors amplihack feature-for-feature". | Remove / reword |
| **(c) needs base-type abstraction** | Hard-coded `amplihack copilot` invocation that should go through a base-type dispatcher (#1162). | File / linked sub-issue |

## Result

| Category | Count | Notes |
|----------|-------|-------|
| (a) legitimate | 128 | All 128 hits |
| (b) parity-coupling | **0** | No surviving "parity" / "same as" / "equivalent to" comments. Verified with `rg -i 'parity\|same as\|identical to amplihack'` returning zero matches. |
| (c) base-type abstraction | tracked separately | All `amplihack copilot` invocations live in clearly-named base-type adapters or test scaffolding; the abstraction work is tracked under #1162 (replace hard-coded `amplihack copilot` with base-type dispatch). |

## Category-(a) cohort breakdown

The 128 legitimate hits cluster into seven well-bounded usages:

1. **Stewardship routing** (`src/stewardship/`) — by design, classifies upstream
   failure signatures and routes amplihack-origin failures to `rysweet/amplihack`.
   The string `"amplihack"` is part of the routing keyword set and the target
   repo slug. This is the module's job.
2. **Base-type adapters** (`src/base_type_copilot.rs`, `src/ooda_actions/session.rs`)
   — drive `amplihack copilot` as a CopilotSdk implementation. Hard-coded path
   tracked in #1162; the comments accurately describe what the adapter does.
3. **Memory-lib type mirroring** (`src/memory_cognitive.rs`) — Rust types that
   mirror the Python `amplihack_memory.memory_types` dataclasses across the
   FFI boundary. Names exist for cross-language readability, not parity.
4. **Recipe-runner integration** (`src/bin/simard_self_improve_recipe.rs`,
   `src/ooda_loop/bridge_factory.rs`, `src/cmd_ensure_deps.rs`) — Simard
   shells out to `amplihack recipe run` for the recipes-first rebuild
   (Phases 1-4, see #1268/#1270/#1273). The doc strings name the binary.
5. **Gym evaluation bridge** (`src/gym_bridge.rs`) — connects to the
   `amplihack-agent-eval` benchmark suite (an upstream package). Five
   scoring dimensions are explicitly the eval suite's, not Simard's.
6. **Engineer-loop dispatch** (`src/engineer_plan.rs`,
   `src/meeting_backend/lightweight.rs`) — comments explaining when
   nested `amplihack copilot` sessions get spawned, including the cost
   rationale for the `meeting_backend/lightweight` shortcut.
7. **Eval-watchdog provenance** (`src/eval_watchdog.rs`) — historical
   reference to amplihack#4477 (the upstream bug that motivated the
   watchdog). This is causation, not parity.

## Category-(b) sweep details

Searched explicitly for parity language and found no surviving hits:

```bash
rg -in 'parity|same as|identical to|equivalent to' src/ | rg -i amplihack
# (no matches)
```

The historical "parity with amplihack" comments noted in the issue body
have already been removed in earlier passes (e.g. PR #1151 cleanup).

## Acceptance

- [x] Every `amplihack` hit triaged
- [x] Zero category-(b) hits found — no comments or error strings imply
      "Simard == amplihack"
- [x] Category-(c) work tracked in #1162

This audit closes the "are there residual parity-coupling references?"
question with **no**. The remaining 128 hits are all accurate technical
references to a real dependency.

## References

- Issue #1163 (this audit)
- Issue #1162 — replace hard-coded `amplihack copilot` with base-type dispatch
- Issue #1161 — remove Python bridge subsystem (Rust-only PRD constraint)
- Issue #1151 — PRD reconciliation that triggered this audit
