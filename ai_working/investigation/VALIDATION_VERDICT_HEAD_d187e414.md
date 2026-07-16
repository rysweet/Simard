# Validation Verdict — "recurring signature seen 2×" investigation

**Validated at HEAD `d187e414` (2026-07-16).** Independent re-grounding of the
verification results and overall understanding. **Verdict: VALID — confirmed.**

## What was validated

1. **Source citations (all exact, zero drift).**
   - `observation_signature` @ `mod.rs:1068-1073` — `keys.sort_unstable(); keys.dedup();
     format!("overseer-obs:{}", keys.join("|"))`. ✅ Byte-for-byte as cited.
   - `RECURRING_SIGNATURE_THRESHOLD = 2` @ `signal.rs:362`; fires at `occurrences >= 2` @ `signal.rs:463`. ✅
   - `RECURRENCE_ESCALATION_THRESHOLD = 3` @ `root_cause.rs:33`. ✅ (2↔3 dead-zone confirmed.)
   - Problem summary `"recurring signature seen {occurrences}× in cognitive memory ({signature})"`
     @ `mod.rs:1361` — verbatim match to the original question string. ✅

2. **No source drift.** `git diff --stat b47b6413..HEAD -- '*.rs'` is empty. The two
   intervening commits (`641f9c37`, `d187e414`) are `docs(investigation)`-only. All prior
   `.rs` line citations still hold; no fix has landed.

3. **Tests re-run at HEAD `d187e414` (independent of prior runs).**
   - Full overseer suite `overseer::`: **361 passed, 0 failed** (7960 filtered). Matches claim.
   - Discriminating probes (H0/H1/H2): **5 passed, 0 failed**
     - `write_back_is_deduplicated_within_window` (H0: dedup gate works) ✅
     - `whisper_gate_suppresses_an_identical_whisper_within_the_window` (H0: in-window suppression) ✅
     - `recurring_signature_emitted_when_two_episodes_share_signature` (H1: honest count) ✅
     - `recurring_signature_not_emitted_for_single_occurrence` (H1: no false positive) ✅
     - `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` (H2: bare-park smoking gun) ✅

## Assessment of the overall understanding

The conclusion is **sound and well-supported**:

- **`×2` is an honest cross-window recurrence count**, not a dedup/storage/replay/collision
  defect. H0 (null) is correctly REJECTED; H1 (real re-observation) is correctly SUPPORTED.
- The composite `overseer-obs:…|goal:blocked:*|workstream-gap` string is the Overseer's own
  observation write-back signature (sorted+deduped `dedup_key`s), correctly attributed to its
  emitters.
- The two genuine (design-level, non-defect) issues are correctly identified:
  1. **Observe-and-flag loops that never close** + a **2×/3× escalation dead zone** →
     permanent low-count recurrence.
  2. **Self-referential write-back** nesting prior `overseer-obs:` fragments.
- The `resource:engineer_spawn` token is correctly classed as benign membership drift
  (literal key; count lives only in the summary, never in the signature).

## Caveats (do not weaken the verdict)

- The "exactly 2× after daemon restart" mechanism (H6, in-process `WhisperGate.last_delivered`
  @ `guardrails.rs:294`) is a **plausible, source-consistent amplifier**, not a directly
  test-reproduced fact — appropriately labelled "SUPPORTED (non-causal amplifier)".
- No remediation is merged; findings are diagnostic only. This is consistent with an
  investigation (not development) task.

**Bottom line:** verification results and overall understanding are internally consistent,
independently reproducible at current HEAD, and their citations are exact. Validation PASSES.
