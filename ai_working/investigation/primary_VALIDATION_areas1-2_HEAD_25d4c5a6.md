# Primary Validation — Token Emitters + Emission→Assembly→Write-Back→Store Pipeline (Areas 1 & 2)

**Role:** PRIMARY investigator (validation wave, not re-derivation).
**HEAD:** `25d4c5a6` (docs-only commits since baseline `6e3113bc`).
**Verdict:** All load-bearing citations for areas 1 & 2 **hold exactly** at live HEAD. **Zero source drift.**

## Drift check
- `git diff --stat 6e3113bc..HEAD -- src/overseer/{mod,signal,wiring,guardrails,sensor,observer}.rs` → **empty** (no changes).
- Only `src/overseer/tests_root_cause.rs` changed across the whole `src/overseer/*.rs` set since baseline; no emitter/pipeline source touched.
- Note: `ooda_loop/cycle.rs` lives at `src/ooda_loop/cycle.rs` (not `src/overseer/ooda_loop/`); prior artifact path prefix is loose but the file exists.

## Area 1 — Token → emitter map (re-grounded, file:line at HEAD)
| Token | Emitter | Site (verified) | Construction |
|---|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` | mod.rs:1068–1073 | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<8hex>` | `signal_to_problem` GoalBlocked arm | mod.rs:1336 | `format!("goal:blocked:{goal_id}")`; slug-hash **is** the goal_id |
| `workstream-gap` | WorkstreamGap arm | mod.rs:1371 | literal `"workstream-gap"`; `gaps.len()` → summary only |
| `resource:engineer_spawn` | EngineerSpawnRate arm | mod.rs:1270 | literal; `{live}` count → summary only |
| nested `overseer-obs:…` | recall-derived `RecurringSignature` | signal.rs:464–467, admitted mod.rs:1353–1363 | `sanitize_recalled(signature)` re-absorbed (no prefix strip) |
| summary render (the observed string) | mod.rs:1361 | verbatim match: `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` |

## Area 2 — Pipeline (re-grounded)
1. `signal_to_problem` stamps each `Problem.dedup_key` (mod.rs:1238+ arms above).
2. `observation_signature` composite — mod.rs:1068–1073 (sort→dedup adjacent-only→join).
3. `write_back_observation` — mod.rs:534; gated by `write_back_gate = WhisperGate::new(900, 5)` (mod.rs:299); **peek→store→commit-only-after-success** (mod.rs:548/554/556).
4. `record_observation` → `store_episode(content, OVERSEER_SOURCE_LABEL, {signature})` — wiring.rs:1076,1088. Single write-back call site: wiring.rs:301 (`&cycle.problems`, unfiltered).
5. Loop closure: `recall_episodic` (wiring.rs:1013) → `parse_failure_signature` (wiring.rs:976) → `signals_from` counts by `failure_signature` (signal.rs:455–470), fires `RecurringSignature` at `occurrences >= RECURRING_SIGNATURE_THRESHOLD` (signal.rs:362/463).

## Recurrence-semantics anchors (confirming, not re-deriving)
- Dead zone confirmed: `RECURRING_SIGNATURE_THRESHOLD = 2` (signal.rs:362) < `RECURRENCE_ESCALATION_THRESHOLD = 3` (root_cause.rs:33) — 2× above noise, below escalation, no remediation rung.
- WhisperGate `last_delivered` is an **in-memory per-process** `HashMap` (guardrails.rs:294, init empty 305) → daemon restart clears it → honest re-record; **not** a dedup/storage/replay/collision bug.

## Reconciliation
Findings reconcile fully with `FINAL_SYNTHESIS.md` and the latest primary pipeline deep-dive (`..._HEAD_e5257a33.md`). No new contradictions. `resource:engineer_spawn` remains a fixed literal key (benign membership drift, count in summary only). Investigation-only; remediation deferred to dev workflow.
