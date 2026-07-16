# Secondary — workstream-gap coupling + corpus consolidation (VALIDATE, don't restart)

**Role:** Secondary investigator.
**HEAD:** `973c294b` (corpus landed at `dea65df8`/`5c14ef03`/`a0c5ed4c`/`856f854b`; this
wave re-verifies the load-bearing gap-coupling citations against the *current* HEAD,
which has advanced past the ledger's `dea65df8`).
**Method:** independently re-read each cited line in live `src/`; ran the gap-scan test
module. **Verdict framing unchanged** — I VALIDATE the landed corpus, add drift-recheck +
a distinct-goal roster. No production `.rs` changed.

---

## 0. Bottom line

1. **Gap-spawn is DECOUPLED from the blocked-goal transition** — confirmed live at
   `sensor.rs:299-302`. There is **no** `blocked → gap` emission loop. The
   `overseer-obs:…|goal:blocked:…|workstream-gap` co-occurrence is **two independent
   non-closing OODA loops observing the SAME under-resourced goal set**, not a coupled
   feedback chain. (Matches DISCOVERIES #5 "two signatures are one problem in two views.")
2. **`workstream-gap` is the TRUE terminal blocker (L3/D3)** for the issue-17 cluster —
   `act_flag_workstream_gaps` is **notify-only** (`mod.rs:884-948`): no `FileIssue`, no
   `LaunchRecipe`, no `route_failure`. It re-emits every window and never converges.
3. **The 2× is honest** and parks in the **dead zone `[2, 3)`** — emit at
   `RECURRING_SIGNATURE_THRESHOLD=2` (`signal.rs:362`), escalate only at
   `RECURRENCE_ESCALATION_THRESHOLD=3` (`root_cause.rs:33`). Not a counting/dedup defect.
4. **Distinct-goal roster: 13 blocked goals + 1 `workstream-gap` family marker**, ALL
   **unresolved**. The signature's `overseer-obs:` prefix is **nesting** (D1 write-back),
   not duplication — stripping it collapses each to one base `goal:blocked:<slug>`.
5. **Investigation COMPLETE / Remediation NOT STARTED** for issue-17's signature: the
   root cause (D1/D2/D3) is fully diagnosed and re-verified; no fix has landed; the
   underlying goals remain blocked.

---

## 1. Re-verified gap-coupling citations (live @ 973c294b)

| Claim | Cited loc | Check | Status |
|---|---|---|---|
| Blocked goals skipped in gap detection (delegated to goal_health, never re-flagged) | `sensor.rs:299-302` (`if matches!(g.status, GoalProgress::Blocked(_)) { continue; }`) | read | ✅ exact |
| `act_flag_workstream_gaps` is notify-only — only `notifier.notify`; no file/launch/route | `mod.rs:884-948` | read | ✅ exact |
| WorkstreamCoverage Decide arm → `FlagWorkstreamGaps` (carries gaps verbatim, no launch edge) | `mod.rs:1534-1543` | read | ✅ exact |
| Orient dedup_key for WorkstreamGap = **bare** `"workstream-gap"` (INV-GAP-KEY over-aggregation) | `mod.rs:1371` | read | ✅ exact |
| Per-gap WhisperGate DOES key on `workstream-gap:{g.signature}` (notification layer) | `mod.rs:901, 932` | read | ✅ exact |
| `gap_gate = WhisperGate::new(900, 200)` — 15-min window, cap 200 | `mod.rs:304` | read | ✅ exact |
| `route_failure` total, `DEFAULT_TARGET_REPO` anticipates `"overseer"` briefs | `routing.rs:11-15, 39-52` | read | ✅ exact |
| `route_failure` callers = **only** `process_orchestrator_run` (`stewardship/mod.rs:75`) + observer tests | grep whole tree | verified | ✅ dangling receiver confirmed |
| `RECURRING_SIGNATURE_THRESHOLD=2` / `RECURRENCE_ESCALATION_THRESHOLD=3` | `signal.rs:362` / `root_cause.rs:33` | read | ✅ exact |

**Drift note:** HEAD advanced from the ledger's `dea65df8` to `973c294b`, but every
gap-coupling defect above is still live and unremediated. The intervening commits are
investigation-doc waves (e.g. `973c294b` = "twenty-first-wave re-execution of H0–H8").

---

## 2. Coupling verdict — the emission→store→re-ingestion picture (my focus)

There are **two distinct loops**, and only ONE self-feeds:

```
 L1 (GROWING self-feed, D1)          L3 (NON-CLOSING, flat, D3)   ← my focus
 ─────────────────────────          ──────────────────────────
 Overseer writes its own episode     detect_workstream_gaps (sensor.rs:288)
 with [sig:overseer-obs:…]             │ skips Blocked goals (:299-302)  ← DECOUPLED
   │ recall_episodic (no self-        Observe → Signal::WorkstreamGap
   │   provenance filter)             Orient → ProblemKind::WorkstreamCoverage
   ▼                                    │  dedup_key = bare "workstream-gap" (:1371)
 observation_signature RE-WRAPS       Decide → FlagWorkstreamGaps (:1534-1543)
   an already-prefixed key (:1068)    Act → act_flag_workstream_gaps (:884-948)
   ⇒ overseer-obs: NESTS each window     │  notifier.notify(...) ONLY
   ⇒ signature mutates → 900s gate       ▼  no FileIssue / LaunchRecipe / route_failure
     cannot brake it                   terminal → re-emits "workstream-gap" every window
```

**Coupling conclusion for issue-17:** the `workstream-gap` marker is **NOT** produced by
the blocked-goal transition (sensor explicitly excludes blocked goals). Instead, the same
under-resourced goal oscillates between two *independent* views:
- while it has no engineer/PR and is **not** yet `Blocked` → it surfaces as a **gap** (L3),
- once it transitions to `Blocked` → it flows through `goal_health` and surfaces as
  `goal:blocked:<slug>` (L2), and is *no longer* a gap.

So `workstream-gap` and `goal:blocked:…` are **mutually-exclusive projections of one
resourcing problem**, stitched together in the same `observation_signature` because the
cycle observes both the still-uncovered members and the already-blocked members in one
pass. The TRUE blocker for issue-17 recurrence is the **missing convergence rung on
BOTH** loops — L3 (notify-only gap) is load-bearing because it is the only High Decide arm
with no launch/file edge at all.

**Test evidence:** `overseer::tests_gap_scan` — **21/21 pass** at HEAD, including
`delegates_blocked_goals_to_goal_health_and_never_reflags_them` (pins the decoupling) and
`flagged_gap_never_constructs_an_issue_brief` / `flags_gaps_notifies_both_channels_without_filing…`
(pin the notify-only terminality). The behavior is contract-locked — remediation must
change these tests, which is the signal that L3 is *intended-as-built* and needs a design
decision, not a bugfix.

---

## 3. Corpus consolidation — distinct-goal roster (collapse nested/prefixed duplicates)

Normalization rule applied: strip the `overseer-obs:` prefix (D1 nesting) and collapse
repeated identical tokens; key each blocked goal on its trailing 8-hex slug id.

| # | Base goal key (`goal:blocked:<slug>`) | slug id | Class | Status @ HEAD |
|---|---|---|---|---|
| 1 | fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed | `7f5afcca` | kgpacks-rs (focal) | **Unresolved / blocked** |
| 2 | advance-rysweet-agent-kgpacks-rs-to-full-parity | `f29bb15c` | kgpacks-rs | Unresolved / blocked |
| 3 | fix-agent-kgpacks-rs-issue-12-parity-decision-o | `dbabd65f` | kgpacks-rs | Unresolved / blocked |
| 4 | fix-agent-kgpacks-rs-issue-18-ws3-versioned-rel | `67828479` | kgpacks-rs | Unresolved / blocked |
| 5 | fix-agent-kgpacks-rs-issue-23-ws8-scalable-enti | `982783ea` | kgpacks-rs | Unresolved / blocked |
| 6 | fix-agent-kgpacks-rs-issue-25-external-cve-corp | `822511ca` | kgpacks-rs | Unresolved / blocked |
| 7 | audit-simard-s-test-coverage-and-raise-it-to-70 | `4d27c91a` | simard | Unresolved / blocked |
| 8 | build-a-local-coin-benchmark-harness-and-a-self | `09e65e35` | simard | Unresolved / blocked |
| 9 | simard-identity-atelier-industrial-furniture-de | `188553ad` | persona | Unresolved / blocked |
| 10 | simard-identity-bursar-investment-portfolio-res | `d0cb8852` | persona | Unresolved / blocked |
| 11 | simard-identity-cartographer-data-storytelling | `fd69391d` | persona | Unresolved / blocked |
| 12 | simard-identity-concierge-hospitality-design-op | `3719bfd4` | persona | Unresolved / blocked |
| 13 | simard-identity-gastronome-culinary-menu-event | `15a90819` | persona | Unresolved / blocked |
| — | `workstream-gap` (bare family marker) | — | coverage | Unresolved (terminal, notify-only) |

- **13 distinct blocked goals** (6 kgpacks-rs, 2 simard infra, 5 identity personas) + **1
  `workstream-gap` family key** + the **`overseer-obs:` composite** (the sorted/deduped
  join of the above, wrapped once per window → the visible 2×).
- **The raw signature's massive length is pure multiplicity, not distinct content:** it is
  the SAME ~13-member set re-emitted across observe windows, with `overseer-obs:` nesting
  and `workstream-gap|workstream-gap` over-aggregation (INV-GAP-KEY, `mod.rs:1371`) inflating
  the token stream. Deduped distinct count = **14 keys**.
- **Resolved roster: EMPTY.** No blocked goal has converged; no landed verdict marks
  issue-17 (or any member) resolved. What IS landed is the *signature root-cause*
  diagnosis (D1/D2/D3), re-verified across 20+ waves: **"Investigation: COMPLETE.
  Remediation: NOT STARTED"** (CONSOLIDATED §27.5/§28).

---

## 4. Patterns / anti-patterns observed (validated)

- **Observe-and-flag without a closing action** (PATTERNS.md): L3 gap Act notifies but
  launches/files nothing → every persistent signal needs a convergence rung. Confirmed
  live at `mod.rs:884-948`.
- **Recurrence dead-zone** `[2,3)`: detection at 2, escalation at 3, nothing in between →
  recurs forever. The counter is honest; audit the closing action, not the counter.
- **INV-GAP-KEY over-aggregation** (anti-pattern trap): Orient collapses all gaps to the
  bare `"workstream-gap"` key (`mod.rs:1371`); any remediation ledger MUST key on
  `GapItem.signature` (as the WhisperGate already does at `:901/:932`) or all gaps fold
  into one issue.
- **Built-and-dangling receiver:** `route_failure` was designed to accept `"overseer"`
  gap briefs (docstring `routing.rs:11-15`) but its caller edge from the gap Act was never
  wired — the remediation seam already exists.

## 5. Integration points for remediation (L3/D3 shape — deferred to architect)

Wire `act_flag_workstream_gaps` to the already-built `RecipeLauncher`
(`launch.rs:124-132`, same path `ProcessHealth` uses at `mod.rs:1429`) and/or
`route_failure`/`FileIssue`, keyed on `GapItem.signature`, honoring
`max_launches_per_cycle` + `goal_has_active_workstream` board dedup. **Must change the
notify-only gap-scan tests** — this is a design decision (turn coverage gaps into real
work) not a silent bugfix.

## 6. Questions for verification phase

1. Confirm no intervening commit between `dea65df8` and `973c294b` altered
   `act_flag_workstream_gaps` / `sensor.rs` gap detection (my read says doc-only waves;
   verify via `git log -p -- src/overseer/sensor.rs src/overseer/mod.rs`).
2. Confirm the roster count (14 distinct keys) reconciles with RECONCILIATION_LEDGER and
   the VALIDATION_VERDICT docs — no member is double-counted across the `overseer-obs:`
   composite and its bare-key expansion.
3. Re-affirm the **§6.2b remedy trap** still applies to any L3 fix: use a count-in-content
   upsert, never a naive `store_fact_with_caller_key`, or escalation becomes dead code.

**No production `.rs` changed. VALIDATE-only; findings extend the landed corpus.**
