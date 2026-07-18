---
title: Done-gate — "build a local COIN benchmark harness and a self-improvement loop"
description: Machine-checkable finish condition for the coin-benchmark-harness umbrella goal, binding completion to a single verifiable command.
last_updated: 2026-07-18
doc_type: reference
owner: simard
---

# Done-gate — the local COIN benchmark harness goal

**Goal:** `build-a-local-coin-benchmark-harness-and-a-self-09e65e35`
**Course-correction:** rewrite an unmeasurable done-gate into a machine-checkable
finish condition that certifies the already-delivered work.

## Why this spec exists

The goal kept parking as **blocked** cycle after cycle with the same diagnosis —
"no tracked PR/issue the done-gate can verify". The blocker was **not**
technical. The local COIN benchmark harness already shipped on `main` under
`src/coin_gym/`, and its measurable acceptance self-check landed in **merged
PR #4171** (`feat(coin-gym): verify acceptance self-check — a measurable
done-gate`). The umbrella goal simply had no finish condition a check could
confirm, so every OODA cycle re-observed it as unfinished and emitted no action —
even though the harness could already certify itself locally with
`coin-gym verify`.

This spec binds the goal's finish condition to **one verifiable command**:

```bash
scripts/check-coin-benchmark-harness-done-gate.sh
```

It exits `0` while the delivered harness, its acceptance self-check, and its
self-improvement loop are present and green; it turns red the moment any of them
regress.

## What "done" means (measurable done-criteria)

| # | Done-criterion | Evidence on `main` | How the gate checks it |
|---|----------------|--------------------|------------------------|
| 1 | A local COIN benchmark harness exists (run / score / compare) | `src/coin_gym/mod.rs` (`dispatch_with_home`, subcommands `run`, `score`, `compare`) | `grep fn dispatch_with_home` |
| 2 | The harness self-checks its own acceptance criteria against an offline snapshot | `run_acceptance_checks` + `"verify" => cmd_verify` in `src/coin_gym/mod.rs` (merged PR #4171) | `grep fn run_acceptance_checks` and the `verify` dispatch arm |
| 3 | A self-improvement loop proposes, verifies, and rolls back tactics on a held-out slice | `run_self_improvement` in `src/coin_gym/improve_loop.rs` | `grep fn run_self_improvement` |
| 4 | The harness + loop are proven by an automated test suite | 119 `coin_gym::` unit tests | `cargo test --lib -- coin_gym::` (re-run by the gate) |
| 5 | The learn-the-benchmark phase landed on `main` | `docs/research/coin-benchmark-phase1.md` (Phase 1 shipped by merged PR #2763, `Closes #2752`) | `--full` mode confirms the record is present |

When the single command above exits `0`, every criterion is satisfied and the
goal is **certified complete** — the daemon can now confirm the finish line on
its own instead of re-parking the goal as blocked.

## Scope

- **LOCAL-ONLY.** The harness grades against a built-in offline mock snapshot; it
  never provisions a VM and never posts results to any external leaderboard.
- **Additive / non-breaking.** This done-gate adds a spec, a check script, and an
  operator note only. No product behaviour changes.

## Verification

```console
$ scripts/check-coin-benchmark-harness-done-gate.sh --full
Running the COIN benchmark harness tests...
test result: ok. 119 passed; 0 failed; ...

Confirming the Phase-1 completion record (--full)...
  → present: coin-benchmark-phase1.md

✅ DONE — the local COIN benchmark harness runs, scores against the
   published-leaderboard shape, self-checks its own acceptance criteria
   (coin-gym verify), and drives a self-improvement loop ...
   The goal build-a-local-coin-benchmark-harness-and-a-self-09e65e35 is certified complete.
$ echo $?
0
```

## Follow-on (not part of this done-gate)

Several additive command PRs remain open on the coin-gym train (`bench` #4161,
`matchup` #4149, `duel` #4134, LOCAL `leaderboard` #4101). They enrich the
harness but are **not** required for the umbrella goal's finish condition — the
harness, its acceptance self-check, and its self-improvement loop are already on
`main`. Those PRs are tracked on their own and do not gate this goal.
