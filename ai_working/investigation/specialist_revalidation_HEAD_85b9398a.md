# Specialist Re-Validation (knowledge-archaeologist) — prior artifacts vs. live source @ HEAD `85b9398a`

**Mandate:** Revalidate prior completion artifacts at HEAD `dea65df8`/`85b9398a` against the
recurring `overseer-obs:…|goal:blocked:<slug>-<hash>|…|workstream-gap|workstream-gap` signature,
to decide **re-validation** vs. **remaining scoped work**. Method: re-read every load-bearing line
in `src/` at current HEAD — did not trust doc citations.

## 0. Verdict

- **Analysis is complete and still valid — do NOT re-derive.** Every load-bearing citation in the
  prior artifacts re-verifies *exactly* at HEAD `85b9398a`.
- **The fix is NOT done — all remediation is remaining scoped work.** The three investigation
  commits (`6e3113bc`, `dea65df8`, `85b9398a`) are **documentation-only** — `git show --stat`
  shows **zero `.rs` changes**. Defects D1/D2/D3 remain present in source at this HEAD.
- The recurrence is a **REAL re-observation loop of a static problem set**, not a dedup/storage
  artifact. The `×2` is expected append behavior of a static problem set across two write-back
  windows; the problem set stays static because the defects never resolve it.

## 1. Independently re-verified at HEAD 85b9398a (exact matches)

| Claim (prior docs) | Location | Status |
|---|---|---|
| `observation_signature`: map `dedup_key` → `sort_unstable` → `dedup` → `overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | ✅ exact |
| Composite `goal:blocked:{goal_id}` built here (`<slug>-<8hex>` = upstream goal_id) | `overseer/mod.rs:1336` | ✅ exact |
| Bare `"workstream-gap"` constant dedup key (evidence-independent) | `overseer/mod.rs:1371` | ✅ exact |
| `RecurringSignature` arm emits literal `"recurring signature seen {occurrences}× in cognitive memory ({signature})"`, `sanitize_recalled`-wrapped | `overseer/mod.rs:1353-1362` | ✅ exact |
| `×2` counts **recalled EPISODES** sharing a `failure_signature` (not occurrence facts) | `overseer/signal.rs:455-469` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `overseer/signal.rs:362,463` | ✅ exact |
| `record_occurrence` persists via **append-only** `store_fact` (caller_key=None) | `overseer/mod.rs:1004-1034` | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps` (no launch/file/close edge) | `overseer/mod.rs:1534-1543` | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `overseer/mod.rs:1613` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `overseer/root_cause.rs:33` | ✅ exact |

Prior `specialist_crosscheck_HEAD_85b9398a.md` and `RECONCILIATION_LEDGER.md` conclusions hold
one commit later, now independently re-confirmed against source rather than doc-to-doc.

## 2. The dead-zone is real and still open (the decisive "remaining work" finding)

Two **decoupled counter lanes** confirm the `×2` is stuck in a remediation gap:

- **Lane A (visible `×2`)** — append-only episodes counted by `signal.rs:457-463`. `2 >=
  RECURRING_SIGNATURE_THRESHOLD(2)` → the RecurringSignature is emitted and *observed*, but it is
  self-referentially written back (`wiring.rs:301 → write_back_observation`), so the next
  signature re-embeds the prior `overseer-obs:` token — the **D1 self-referential write-back**.
- **Lane B (escalation)** — append-only occurrence facts (`mod.rs:1034`) counted in
  `root_cause::analyze`, gated at `>= 3` (`mod.rs:1613`, `root_cause.rs:33`). `2 < 3` → **no
  escalation**.

Between "emit at 2" and "escalate at 3" there is **no gap-remediation rung**, and the only
`WorkstreamCoverage` action is flag-only (`mod.rs:1543`) — **D2 dead-zone**. So the goal set is
observed, flagged, re-observed, and never closed. `D3` (no closing rung on the gap path) is
likewise unimplemented — the Decide arm has no launch/file edge.

## 3. Decision: re-validation vs. remaining scoped work

- **Re-validation:** ✅ Done here. Analysis authoritative; no contradictions; no re-derivation needed.
- **Remaining scoped work (unimplemented in source):**
  1. **D2 first** — add the recurrence-aware gap-remediation rung and make the gate+counter update
     atomic (prior notes flag the naïve single-counter fix as a **trap** — do not converge on it).
  2. **D3** — give the `WorkstreamCoverage` Decide arm a closing edge (launch/file), not flag-only.
  3. **D1** — filter recall-derived `overseer-obs:` tokens out of `observation_signature` so the
     overseer stops re-observing its own bookkeeping (`sanitize_recalled` at `mod.rs:1359` already
     treats recalled signatures as untrusted — extend that distrust to write-back).
  4. Convergence observability (gauges) once 1–3 land.

Landing order is dependency-correct as stated in `tertiary_architecture_VALIDATION_HEAD.md`
(D2 → D3 → D1 → gauges). Nothing in current source contradicts it.

## 4. Out of scope (confirmed dead ends, per strategy)

Functional correctness of individual agent-kgpacks-rs issues (12/17/18/23/25), simard-identity
personas, coin-benchmark tuning, and GitHub issue-filing infra — only their blocked/gap *emission*
matters, and that emission is fully explained by D1/D2/D3 above.
