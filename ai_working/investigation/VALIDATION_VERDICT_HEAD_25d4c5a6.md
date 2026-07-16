# Validation Verdict — "recurring signature seen 2×" verification results

**HEAD:** `25d4c5a6`. **Verdict: VALIDATED (high confidence).** The verification
results and the overall understanding are sound. Every load-bearing citation was
independently re-grounded to live source, and every named empirical test was
re-executed green. No contradicting evidence found.

## Independent re-grounding of load-bearing citations (all HOLD)

| Claim | Cited | Verified live | Status |
|---|---|---|---|
| Signature = `sort_unstable(); dedup(); format!("overseer-obs:{}", keys.join("\|"))` | `mod.rs:1068-1073` | Exact match at `mod.rs:1068-1073` | ✅ |
| Recurrence threshold = 2 | `signal.rs:362` | `pub const RECURRING_SIGNATURE_THRESHOLD: u32 = 2;` | ✅ |
| Fires at `occurrences >= THRESHOLD` | `signal.rs:463` | Confirmed `signal.rs:462-468` | ✅ |
| Summary string **verbatim** the question | `mod.rs:1361` | `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` | ✅ |
| Escalation threshold = 3 (dead zone) | `root_cause.rs:33` | `pub const RECURRENCE_ESCALATION_THRESHOLD: u32 = 3;` | ✅ |
| `goal:blocked:<goal_id>` emitter | `mod.rs:1336` | `format!("goal:blocked:{goal_id}")` | ✅ |
| `workstream-gap` bare literal key | `mod.rs:1371` | `"workstream-gap".to_string()` | ✅ |
| Write-back gate `WhisperGate::new(900, 5)` | `mod.rs:299` | `write_back_gate: WhisperGate::new(900, 5)` | ✅ |
| Self-referential write-back: recall `RecurringSignature` admitted with `sanitize_recalled(signature)` as dedup_key (nests `overseer-obs:` fragments) | `mod.rs:1353-1363` | Confirmed at `mod.rs:1353-1363` | ✅ |

## Empirical re-execution (all green)

- Full overseer lib suite: **361 passed, 0 failed** (`cargo test -p simard --lib overseer::`).
- Named verification tests re-run individually — all pass:
  `write_back_is_deduplicated_within_window`,
  `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
  `recurring_signature_emitted_when_two_episodes_share_signature`,
  `recurring_signature_not_emitted_for_single_occurrence`,
  `orient_raises_recurring_signature_to_high_priority`.

## Minor discrepancies (non-material, do not weaken the verdict)

1. **Test count drift 359 → 361.** The verification doc reported 359; live HEAD
   `25d4c5a6` reports 361. The +2 are new Lane A/B decoupling tests added to
   `tests_root_cause.rs`. Additive, corroborating; no test regressed.
2. **"Zero `*.rs` drift" claim.** FINAL_SYNTHESIS asserts zero source drift at its
   HEAD `5a85317b`. Since then, `src/overseer/tests_root_cause.rs` changed
   (`git diff 6e3113bc..25d4c5a6`). It is a **test-only** file; every production-code
   citation above was re-verified against current source and still holds.

## Conclusion

The verdict **H1 CONFIRMED** stands: `×2` is a faithful cross-window recurrence
count of a genuinely re-observed, unresolved static problem set — not a
dedup/storage/replay/collision defect. The real issues are design concerns:
(1) observe-and-flag loops that never close (blocked goals parked without a WHY
class; `workstream-gap` only notified, never launched/filed) and (2) a
threshold-2 / escalate-3 recurrence dead zone across two decoupled counter lanes,
plus (3) self-referential write-back nesting `overseer-obs:` fragments. Overall
understanding is accurate and internally consistent.
