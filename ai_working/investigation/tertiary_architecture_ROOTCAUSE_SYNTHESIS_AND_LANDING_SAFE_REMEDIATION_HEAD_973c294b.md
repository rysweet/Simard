# Tertiary (Architect) — Root-cause synthesis + minimal, landing-order-safe remediation

**Role:** Tertiary investigator (architect). **Mandate:** root-cause synthesis into a
minimal, landing-order-safe remediation *or* a justified no-fix verdict.
**HEAD:** `973c294b` (twenty-first wave). **Date:** 2026-07-16.
**Method:** VALIDATE-don't-re-derive. Every load-bearing citation below was re-read
byte-for-byte in live source at HEAD (I did not trust prior docs' citations), the
overseer test suite was re-run, and the nesting collapse was reproduced empirically.

---

## 0. Bottom line (split verdict)

The investigation is at a **fixpoint** across 21 waves; my contribution is to close it
with a decision, not another re-observation. The verdict is **split by layer**:

- **On the signature / the `×2` count → NO FIX.** `overseer-obs:goal:blocked:…-7f5afcca`
  seen `2×` is an **honest cross-window re-observation tally of a static, unresolved
  problem set** — provably **prefix nesting, not duplication**, and not a
  dedup/storage/replay/collision defect. Touching the counter or the signature would
  hide a true signal. This half of the question is **answered and closed.**
- **On the response the signal indicates → MINIMAL FIX (three additive edges).** The
  reason the signal *recurs forever* is three genuine design defects: an OODA loop that
  Observes and Decides but never Closes. The minimal, landing-order-safe remediation is
  D2 → D3 → D1 behind an L0 prerequisite, specified in §4 with safety notes and two
  named traps. **None has landed.**

The single open item is **land D2** (a one-line, test-safe change); continuing to spawn
investigation waves is itself an instance of the pathology under study (§5).

---

## 1. Re-verified emitter trace (live @ `973c294b`) — the 2× is nesting, not duplication

The `overseer-obs:` prefix has **exactly one producer**, and it wraps the base
`goal:blocked:<slug>` dedup_key. Confirmed citations:

| Hop | Site (re-read at HEAD) | Fact |
|---|---|---|
| Emit signature | `overseer/mod.rs:1068-1073` `observation_signature` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` — **sole** producer of the prefix + `\|`-join. |
| Base key | `overseer/mod.rs:1336` | `dedup_key = format!("goal:blocked:{goal_id}")`. |
| Write-back call | `overseer/wiring.rs:301 → mod.rs:534-563` | single call site; gated by `write_back_gate = WhisperGate(900,5)`, commit-after-store (`:548-557`). |
| Episode carries sig | `record_observation` embeds `[sig:…]` | recall re-extracts it: `parse_failure_signature` `wiring.rs:976-986`; `recall_episodic` `:1013-1031`. |
| Recur-detect | `overseer/signal.rs:455-469`, threshold `RECURRING_SIGNATURE_THRESHOLD=2` `:362` | `Signal::RecurringSignature` at `occurrences >= 2`. |
| Verbatim summary | `overseer/mod.rs:1361` | `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` — verbatim match to the question string. |

**Empirical nesting collapse (reproduced this wave):** stripping the `overseer-obs:`
prefix from the question's token yields exactly
`goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca` — the base
dedup_key. Therefore `overseer-obs:goal:blocked:X` and bare `goal:blocked:X` are the
**same logical event surfaced through two namespaces (nesting)**, not two independent
writes. Duplication is *structurally impossible* here: Orient merges same-`dedup_key`
signals and `keys.dedup()` collapses adjacent equals, so each family key appears at most
once per snapshot. The doubled `…|overseer-obs:…` / `|workstream-gap|workstream-gap|`
fragments are the **positive fingerprint of D1 self-observation nesting**, not a bug.

**Test evidence (re-run this wave):** `cargo test --lib overseer::` → **361 passed, 0
failed** (7960 filtered). Current behavior is exactly as the corpus claims; H0 (null /
dedup-defect) stays REJECTED, H1 (honest re-observation) SUPPORTED.

---

## 2. The three defects (why it recurs) — three independent, additive graph edges

The signal is honest; the **response** is the defect. Three non-closing seams, each a
single missing edge, mapping 1:1 to D1/D2/D3:

- **D2 — blocked-goal ladder terminal sink never records (Decide→Act, Lane B).**
  `decide_blocked_goal` escalates only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD
  (3)` (`mod.rs:1613`, `root_cause.rs:33`). `recurrence` counts prior occurrence facts
  (`root_cause.rs:79-82`), written by `record_occurrence` via `store_fact`
  (`mod.rs:1034`). But occurrences are only recorded for outcomes in
  `outcome_records_occurrence` (`wiring.rs:612-627`) — and **`ActOutcome::Reported` is
  absent** from that list (re-confirmed at HEAD). A bare park routes to Rung-4 `Report`
  → `Reported` → **no occurrence recorded → Lane-B `recurrence` never leaves 0 → the
  `>=3` rung is unreachable dead code.** The goal re-observes and re-parks forever. This
  is the absorbing `[2,3)` dead zone: Lane-A visible ≥2, Lane-B escalation <3.

- **D3 — workstream-gap is notify-only (Act→routing edge never wired).**
  `WorkstreamCoverage` is the only High-priority Decide arm whose Act is notify-only:
  `FlagWorkstreamGaps` (`mod.rs:1534-1543`), `act_flag_workstream_gaps` files no issue
  and launches no recipe. `stewardship::route_failure` (`routing.rs`) was **built to
  accept the Overseer's `"overseer"` gap briefs but is reachable only from
  `process_orchestrator_run`, never from the Overseer gap Act** — receiver built, caller
  edge never wired. The gap re-emits the bare family key `"workstream-gap"`
  (`mod.rs:1371`) every window.

- **D1 — self-ingestion re-wrap (Memory→Observe, the only *growing* loop).**
  The Overseer writes its own episode with a recoverable `[sig:overseer-obs:…]` marker
  and recalls it with **no self-provenance filter** (`recall_episodic`
  `wiring.rs:1013-1031`). `sanitize_recalled` keeps the prefix, and
  `observation_signature` re-wraps an already-`overseer-obs:`-prefixed key → nesting.
  The 900 s gate cannot brake it because each generation mutates the signature.

**Coupling verdict:** one shared *structural* cause, not a shared upstream dependency.
An under-resourced important goal oscillates — active-but-uncovered ⇒ `workstream-gap`
(D3); parked by the no-progress breaker ⇒ `goal:blocked` (D2) — and D1 nests both into
one composite episode. Issue-17 (ws2 int8/PQ embed) is best classed as a **real,
persistent block the loop cannot resolve, with magnitude inflated by an over-counting
recurrence lane** — *real block, artifact-inflated magnitude*, not a pure observation
artifact. Fixing any one edge shrinks the composite; all three are needed to stop
recurrence.

---

## 3. Distinct-goal roster (deduped; prefix-nested + observer-prefixed collapsed)

Collapsing `overseer-obs:`-prefixed and bare forms to their base `goal:blocked:<slug>`
keys yields **13 distinct blocked goals**, all **still-open at HEAD** (blocked by the
same D2 non-closing sink; none has a landed unblock/escalation):

1. `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed` (the question's focus)
2. `advance-rysweet-agent-kgpacks-rs-to-full-parity`
3. `fix-agent-kgpacks-rs-issue-12-parity-decision`
4. `fix-agent-kgpacks-rs-issue-18-ws3-versioned-rel`
5. `fix-agent-kgpacks-rs-issue-23-ws8-scalable-enti`
6. `fix-agent-kgpacks-rs-issue-25-external-cve-corp`
7. `audit-simard-s-test-coverage-and-raise-it-to-70`
8. `build-a-local-coin-benchmark-harness-and-a-self`
9. `simard-identity-atelier-industrial-furniture-de`
10. `simard-identity-bursar-investment-portfolio-res`
11. `simard-identity-cartographer-data-storytelling`
12. `simard-identity-concierge-hospitality-design-op`
13. `simard-identity-gastronome-culinary-menu-event`

Plus the `workstream-gap` family marker (D3 lane) and `resource:engineer_spawn`
(benign membership drift — literal key, count lives only in the summary, never the
signature). No per-goal domain logic is in scope; these are dedup-count fodder that
share the one structural cause in §2.

---

## 4. Minimal, landing-order-safe remediation (the fixable half)

Whole-loop order: **L0 → D2 → D3 → D1.** Each is additive; none rewrites overseer
architecture.

- **L0 (prerequisite, no store change):** ensure bare parks carry their real WHY down
  the ladder (WHY-reasoner wiring; note the twentieth-wave drift correction — the
  no-progress investigation is **default-on** and the issue-#17 re-investigation path
  exists, so L0 is narrower than earlier framings, but the `completion_evidence` Gate A
  still admits bare parks). **Do not** blind `unblock-all` — operator-rejected
  antipattern (`mod.rs:1588`, `:1620-1621`).

- **D2 (land first — lowest risk, one line):** add `ActOutcome::Reported` to
  `outcome_records_occurrence` (`wiring.rs:612-627`) so acknowledged parks accrue toward
  Rung-1. **Landing-safe:** no test pins the exclusion; the first observation still
  Reports; the change only lets Lane-B `recurrence` climb toward the existing `>=3` gate.
  - **TRAP (do not take the tempting one-liner):** the committed §6.2b remedy
    `store_fact_with_caller_key(root_cause_signature(...))` at `mod.rs:1034` is a **live
    trap**. `CallerKey` keeps exactly one live fact per key
    (`library_adapter.rs:885-889`) and `recurrence = recall.len()`, so a stable
    root-cause signature collapses recall to **1 forever** → the `mod.rs:1613` `>=3`
    rung becomes dead code. If the append-ratchet is addressed at all, use a
    **count-in-content upsert** (`occurrence_count`/`first_seen`/`last_seen`, escalation
    reading the field), never the literal CallerKey swap. For the *minimal* fix, D2's
    sink inclusion alone unblocks Lane-B; the ratchet correction is optional/secondary.

- **D3 (close the gap loop):** give `WorkstreamCoverage` a recurrence-aware ladder
  (1× Notify / ≥2× `LaunchRecipe` via the already-built `route_failure` / ≥3× Escalate)
  as an **additive** Decide arm.
  - **TRAP (INV-GAP-KEY):** key the ledger on `GapItem.signature`, **not** the bare
    `"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps fold into one issue.
  - **Landing-safe:** **never swap** `FlagWorkstreamGaps` — `tests_gap_scan.rs:852`
    hard-asserts it. Add the new edge alongside the existing notify.

- **D1 (stop self-ingestion / nesting):** a single-function write-boundary
  self-provenance filter in `write_back_observation` (`mod.rs:534-563`) that drops
  `overseer-obs:`-keyed, recall-derived problems before `observation_signature`
  (and/or make the re-wrap idempotent — do not re-prefix an already-`overseer-obs:` key).
  - **Landing-order safety (store boundary):** D1 changes what enters the write-back and
    thus what future recall sees. Land it **after** D2/D3 so their new closing edges are
    already draining the loop; otherwise D1 would mask a still-open loop by trimming the
    fingerprint without closing the cause. Guard with a test asserting genuine
    (non-self) recurrence signalling is preserved.

---

## 5. Convergence / closure

The `.rs` production source is **byte-identical** to the §22–§28 groundings
(`git diff --stat d187e414..HEAD -- src/**/*.rs` empty; working tree clean). The verdict
has not moved in 21 waves. This report does not add a mechanism; it renders the
**decision** the corpus has been circling:

> **NO FIX** to the counter/signature (honest signal). **MINIMAL FIX** to the response:
> land **D2** (one line, test-safe) behind the **L0** WHY prerequisite, then **D3**
> (additive gap ladder, INV-GAP-KEY, never swap `FlagWorkstreamGaps`), then **D1**
> (self-provenance filter at the write boundary, landed last for store-boundary safety).

Spawning another parallel re-observation wave is itself the pathology under study — an
over-aggregated composite observed `N×` and re-emitted while the underlying problem set
never changes. **Investigation: COMPLETE. Remediation: NOT STARTED — the sole open item
is to land D2.**
