# Primary deep dive (empirical) — composite signature provenance + ×2 real-loop verdict

**Role:** PRIMARY investigator.
**Focus:** Composite signature provenance (`mod.rs`/`signal.rs`/`observer.rs`/`notify.rs`)
and the real-loop-vs-artifact verdict for the `×2` recurrence, validated **by
running the stewardship-dedup and memory-recall test suites** (not source reading alone).
**HEAD:** `388e6c29`  **Date:** 2026-07-15
**Method:** re-verified every load-bearing line against live source, then executed
the two test modules that gate the verdict. All 55 targeted tests pass.

---

## Verdict (high confidence, now test-backed)

**The `×2` is a REAL re-observation loop, NOT a dedup / storage / replay / hash
artifact.** The two graph nodes are two legitimate write-back passes of the same
composite signature ≥1 window apart. **But the signature it certifies is
vacuous** — an aggregate join of every open problem's `dedup_key`, so its
recurrence means only "the same static problem set was observed twice," partly
re-observing the overseer's own prior bookkeeping.

This confirms the standing consolidated verdict
([`primary_signature_provenance_dedup_verdict.md`](./primary_signature_provenance_dedup_verdict.md),
[`RECONCILIATION_LEDGER.md`](./RECONCILIATION_LEDGER.md) §4). My addition is the
**empirical layer**: the verdict's mechanism is now pinned by passing tests, not
just by code citation.

---

## Provenance chain — re-verified @ 388e6c29

| Seam | Location | Confirmed |
|---|---|---|
| Composite built: `sort_unstable → dedup → "overseer-obs:"+join("\|")` | `mod.rs:1068-1073` | ✅ exact |
| `goal:blocked:<slug>` token minted per problem | `sensor.rs:306`, `mod.rs` classify | ✅ |
| `workstream-gap` problem `dedup_key` is the bare literal | `mod.rs:1371` | ✅ (so ≤1 per signature after `dedup()`) |
| Gap **notification** gate keys on `workstream-gap:{g.signature}` (distinct path) | `mod.rs:901,932` | ✅ not the recall signature |
| Write-back gate = 900 s window, keyed on composite; slot committed only after store | `mod.rs:546-556` | ✅ |
| `RecurringSignature` emitted at `occurrences >= 2` | `signal.rs:462-468` | ✅ |
| `×2` message string rendered from the recalled signature | `mod.rs:1360-1362` | ✅ |
| Self-referential fold: `RecurringSignature.dedup_key = sanitize_recalled(signature)` | `mod.rs:1359` | ✅ nests `overseer-obs:` on next tick |
| `notify.rs` only renders the gap **notification kind**, not a signature source | `notify.rs:98,204` | ✅ ruled out |
| `observer.rs` uses `problem.dedup_key` as `failure_kind` for the **GitHub-issue** path | `observer.rs:77,133` | ✅ separate path |

**Two consecutive `workstream-gap|workstream-gap` in the recalled blob are two
concatenated episode signatures, not a `dedup()` failure** — `dedup()` is
adjacent-only over a pre-sorted vec, and every gap problem carries the identical
`"workstream-gap"` key (`mod.rs:1371`), so within one signature it collapses to
exactly one. Confirmed structurally; no gap-key variance path exists.

---

## Empirical validation — tests executed (all pass)

### A. Memory-recall path — `cargo test --lib overseer::tests_memory_recall`
**32 passed / 0 failed.** The verdict-critical cases:

| Test | Proves |
|---|---|
| `write_back_is_deduplicated_within_window` | Two identical-signature ticks in one 900 s window ⇒ **exactly 1** episode persisted (rules out double-write within window). |
| `write_back_persists_again_for_a_distinct_signature` | Two distinct observations ⇒ **2** episodes (the count is honest; a real second pass, not a replay). |
| `recurring_signature_emitted_when_two_episodes_share_signature` | `occurrences == 2` ⇒ `RecurringSignature` raised (the `×2` floor). |
| `recurring_signature_not_emitted_for_single_occurrence` | `< 2` ⇒ no signal (threshold is exactly 2). |
| `recurring_signature_ignores_episodes_without_signature` | Only `failure_signature`-bearing episodes are tallied. |
| `orient_raises_recurring_signature_to_high_priority` | The recalled signature merges/promotes rather than spawning a dup. |
| `recurring_signature_problem_summary_is_sanitized` | Untrusted recalled text is `sanitize_recalled`-cleaned at the admission boundary. |

Together these are the executable proof of the two-nodes-are-real /
count-is-honest half of the verdict.

### B. Stewardship dedup path — `cargo test --lib stewardship::tests`
**23 passed / 0 failed.** Confirms this path is a **different, hash-based
subsystem that never touches the `overseer-obs:` composite**:

| Test | Proves |
|---|---|
| `signature_stable_across_timestamps_paths_hashes_runids_linecols` | `failure_signature = sha256(kind‖msg)[..8]` is volatility-redacted — the GitHub-issue dedup key, not the recall composite. |
| `signature_differs_on_kind_change` / `signature_differs_on_message_change` | It fingerprints failure *content*, unlike the composite's plain string join. |
| `find_existing_matches_signature_in_body` / `..._ignores_when_signature_absent` | Dedup is by `stewardship-signature:` marker in an issue body — GitHub, not memory. |
| `process_run_routes_overseer_to_default_and_dedups` / `..._idempotent_on_second_invocation` | Overseer failures dedup on the **issue** lane, independent of the recall `×2`. |

This is the executable proof of the "ruled-out artifact" column: the
`stewardship::failure_signature` hash path is **not** the source of the recurring
signature.

---

## Ruled-out artifact hypotheses (now test-anchored)

| Hypothesis | Ruled out by (test) |
|---|---|
| Double-write of one node inside the window | `write_back_is_deduplicated_within_window` (1 node) |
| Distinct sigs failing to persist / replay illusion | `write_back_persists_again_for_a_distinct_signature` (2 nodes) |
| `×2` is a threshold/off-by-one artifact | `recurring_signature_{emitted,not_emitted}` bracket the `>= 2` bar |
| `dedup()` collapse bug on `workstream-gap` | Adjacent-only over sorted vec; single fixed key (`mod.rs:1371`) — structural, no variance path |
| Hash collision / unstable key on the recall path | Composite is a deterministic plain string join (`mod.rs:1069-1072`); no hashing |
| `stewardship::failure_signature` mis-keying | Separate lane — `stewardship::tests` (23) exercise it in full isolation |
| `notify.rs` / routing duplication | `notify.rs:98,204` render the gap notification kind; `routing.rs` maps source→repo only |

---

## Bottom line for remediation (unchanged, reinforced)

The recurrence is a **faithful symptom of a static, unresolved problem set**, not
a signature/dedup bug — so no fix belongs on the signature or dedup path except
the one contained hygiene fix: **stop folding recall-derived `RecurringSignature`
keys back into `observation_signature`** (D1, `mod.rs:1359` + `wiring.rs:301`),
which cuts the self-referential `overseer-obs:` nesting. The load-bearing work is
closing the two observe-and-flag loops (blocked-goal escalation counter D2;
`WorkstreamCoverage` routing hole D3) per the consolidated findings. My tests do
not touch those lanes; they only certify that the `×2` is real and vacuous.
