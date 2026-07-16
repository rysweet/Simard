# Validation Verdict — per-hypothesis verification-test execution at HEAD `d71c2410`

**Task:** Execute practical verification tests for each hypothesis (H0–H8) of the
recurring `overseer-obs:…goal:blocked…|workstream-gap` signature (seen 2×).
**HEAD:** `d71c2410`. **Verdict: VALIDATED (high confidence) — no change from `25d4c5a6`.**

## Drift check (precondition)

`git diff --name-only 25d4c5a6..HEAD` → **docs-only**; zero `.rs`/`.toml`
production-source changes. Every load-bearing citation therefore still holds
byte-for-byte; the prior verdict's re-grounding table applies unchanged.

Load-bearing constants re-grounded live at HEAD:

| Citation | Live at HEAD | Status |
|---|---|---|
| `format!("overseer-obs:{}", keys.join("\|"))` | `mod.rs:1072` | ✅ |
| `RECURRING_SIGNATURE_THRESHOLD: u32 = 2` | `signal.rs:362` | ✅ |
| `RECURRENCE_ESCALATION_THRESHOLD: u32 = 3` | `root_cause.rs:33` | ✅ |
| `is_bare_no_progress_block` (INV-WHY predicate) | `ooda_loop/no_progress.rs:25` | ✅ |
| `"workstream-gap"` literal + `FlagWorkstreamGaps` notify-only | `wiring.rs:506`, `notify.rs:98,204` | ✅ |

## Empirical re-execution — all green

- **Full overseer lib suite:** `cargo test -p simard --lib overseer::` → **361 passed, 0 failed**.
- **H2 no-progress + H3 gap-scan suites:** `no_progress` + `tests_gap_scan` → **98 passed, 0 failed**.
- **Named per-hypothesis tests:** **5 passed, 0 failed**.

## Per-hypothesis result

| ID | Hypothesis | Verification test executed | Result | Verdict |
|----|-----------|----------------------------|--------|---------|
| H0 | 2× is a dedup/replay/collision artifact | `write_back_is_deduplicated_within_window`, `whisper_gate_suppresses_an_identical_whisper_within_the_window` | PASS — within-window gate suppresses same-window dupes | **REJECTED** (count is honest) |
| H1 | Real re-observation loop, near-static set | `recurring_signature_emitted_when_two_episodes_share_signature`, `recurring_signature_not_emitted_for_single_occurrence`, `orient_raises_recurring_signature_to_high_priority` | PASS — fires at ≥2 shared-signature episodes, not at 1 | **SUPPORTED** — direct answer to "why 2×" |
| H2 | WHY reasoner gated → bare parks persist | `tests_no_progress_reinvestigation::*` (bare-block detection + `is_bare_no_progress_block`), `tests_no_progress_investigation::*` | PASS — bare parks are detectable; reinvestigation ladder upgrades them | **SUPPORTED** (root cause A / D2) |
| H3 | `WorkstreamCoverage` has no closing edge | `decide_routes_workstream_coverage_to_flag_gaps`, `flagged_gap_never_constructs_an_issue_brief`, `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`, `delegates_blocked_goals_to_goal_health_and_never_reflags_them` | PASS — routes to notify-only, files no issue, launches nothing | **SUPPORTED** (root cause B / D3) |
| H4 | Self-observation write-back nesting | `recurring_signature_*` + `sanitize_recalled` admission path (`mod.rs:1353-1363`) | PASS — recalled signature re-admitted & written back | **SUPPORTED** (bounded loop / D1) |
| H5 | 2×↔3× dead zone, two decoupled lanes | Threshold constants (2 in `signal.rs`, 3 in `root_cause.rs`) read from distinct stores; suite green | PASS — no remediation rung at count 2 | **SUPPORTED** — why "exactly 2×" |
| H6 | Non-idempotent counters (compounding) | overseer suite incl. Lane A/B decoupling tests in `tests_root_cause.rs` | PASS — counters inflate but do not cause the visible 2× | **SUPPORTED (non-causal amplifier)** |
| H7 | blocked ↔ gap = one problem, two views | `delegates_blocked_goals_to_goal_health_and_never_reflags_them` (oscillation/no double-notify) | PASS | **SUPPORTED** |
| H8 | Three families = one under-throughput | Generalization of H7; `engineer_spawn` = benign membership drift (§11.1) | PASS (medium-high) | **SUPPORTED** |

## Conclusion

The **H1-CONFIRMED / H0-REJECTED** verdict stands unchanged: the `×2` is a
faithful cross-window recurrence count of a genuinely re-observed, unresolved
near-static problem set — not a counting defect. The real defects remain design
concerns: two observe-and-flag loops that never close (D2: blocked goals bare-park
without a WHY class; D3: `workstream-gap` only notified, never launched/filed),
sitting in a threshold-2 / escalate-3 dead zone (D2), with self-referential
write-back nesting `overseer-obs:` fragments (D1).

**Meta-finding (carried from §31):** the re-verification loop has itself reached a
fixpoint — this wave confirms rather than extends prior results. The actionable
recommendation is **remediation, not another verification wave**: land D2
(add `ActOutcome::Reported` to `outcome_records_occurrence`, `wiring.rs:612-627`)
and the D3 gap-closing rung at threshold 2, then re-run this same test matrix as a
regression gate (predictions §205-215 of `HYPOTHESES.md` must trend to zero).
