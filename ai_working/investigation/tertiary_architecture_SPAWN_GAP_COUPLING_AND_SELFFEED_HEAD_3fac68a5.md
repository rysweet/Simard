# Tertiary (Architect) Deep Dive — engineer_spawn↔workstream-gap Coupling, Write-Back Self-Feed, & Signal-vs-Defect Remediation

**Investigation:** "recurring signature seen 2× in cognitive memory
(`overseer-obs:…|goal:blocked:<slug>-<hash>|workstream-gap|resource:engineer_spawn`)"
**Role:** TERTIARY / architect — structural verdict + remediation shape, **no implementation**
**HEAD verified:** `3fac68a5288e965a1aceee029a3e10ae105db3c0` (`git rev-parse HEAD`)
**Prior tertiary grounding:** `f455c06d` (tertiary_architecture_LANDING_SAFE_REMEDIATION_HEAD_f455c06d.md)
**Method:** every load-bearing edge re-read at HEAD with file:line citations; two
load-bearing test modules independently re-run green; prior artifacts reconciled, not trusted blind.

---

## 1. Verdict (up front)

1. **`engineer_spawn ↔ workstream-gap` coupling = CORRELATIONAL (co-occurrence), NOT causal.**
   The two tokens are independent leaf signals derived from two **separate**
   `ObservedState` fields with no cross-read. Their only relationship is
   **membership co-occurrence inside one write-back composite** (over-aggregation),
   plus a *latent, code-invisible common cause* (an under-resourced system).
   There is **no** spawn→gap or gap→spawn edge in the code.

2. **Write-back self-feed loop = CONFIRMED DEFECT (primary-owned D1).** The
   Overseer's own persisted observation re-enters its observation input through a
   fully-traced 5-edge cycle (§3). The exact-string write-back dedup gate **cannot**
   dampen it because each cycle mutates the signature (nesting), defeating the gate.
   This is what grows the signature to the ~20 KB nested blob in the query.

3. **Signal-vs-defect remediation direction:** the `engineer_spawn`,
   `workstream-gap`, and `goal:blocked:*` tokens are **honest signals — do not
   suppress them.** The defect is (a) the self-feed loop and (b) the missing closing
   rungs. The coupling itself needs **no fix**; it is benign and resolves on its own
   once the loop-breaker freezes the signature set. Landing order and seams already
   established by prior waves stand unchanged (§5).

4. **Zero source drift** `f455c06d..HEAD` (`git diff --stat f455c06d..HEAD -- src/`
   = **empty**; the two intervening commits `d6ba8b25`, `3fac68a5` are
   docs/investigation-only). All prior tertiary/secondary citations hold verbatim.

---

## 2. Coupling analysis — the two tokens are independent leaf signals (causal-vs-correlational)

Both signals are produced by the pure fold `signals_from(state)` (`signal.rs:366`),
each gated on a **different, independent** `ObservedState` field:

| Token | Signal variant | Sole input | Threshold | Emit site |
|---|---|---|---|---|
| `resource:engineer_spawn` | `EngineerSpawnRate { live }` | `state.live_engineers` | `>= ENGINEER_SPAWN_THRESHOLD (8)` | `signal.rs:393-397` |
| `workstream-gap` | `WorkstreamGap { gaps }` | `state.workstream_gaps` | non-empty | `signal.rs:475-479` |

- **Neither derivation reads the other's field.** There is no branch in
  `signals_from` where `live_engineers` influences `workstream_gaps` or vice versa.
  → **No causal edge exists in the signal layer.**
- Their dedup_keys / ProblemKinds (`capabilities.rs`/`mod.rs`):
  - `EngineerSpawnRate` → `ProblemKind::ResourcePressure`, `Priority::Normal`,
    dedup_key `"resource:engineer_spawn"` (`mod.rs:1267-1272`).
  - `WorkstreamGap` → `ProblemKind::WorkstreamCoverage`, `Priority::High`,
    dedup_key `"workstream-gap"` (bare) (`mod.rs:1368-1373`).
- **The ONLY thing that binds them** is `observation_signature(problems)`
  (`mod.rs:1068-1073`), which sorts+dedups+joins the dedup_keys of **every problem
  in the tick** into one `overseer-obs:` composite. When both fields cross threshold
  in the *same* Observe snapshot, both keys land in the same composite string. This
  is **over-aggregation (co-occurrence in a shared write-back window)**, not coupling.

**Common-cause caveat (domain-level, not code-level):** a resource-starved /
overcommitted Simard can simultaneously run ≥8 live engineers **and** leave
workstreams uncovered — the "oscillation / one root problem" the secondary wave
identified. That is a *latent common cause*, invisible to the code, which treats
the two as unrelated leaf signals. It is **not** a direct causal link between the
tokens, and it does **not** warrant special-casing the pair.

**Why they both persist into the recurring composite:** both decide arms are
**notify-only / non-closing**:
- `ResourcePressure → Intervention::Escalate` (`mod.rs:1444`) — explicitly
  "symptom mitigation" (`mod.rs:1119-1124`, `Remediation::symptom`).
- `WorkstreamCoverage → FlagWorkstreamGaps` (`mod.rs:1534-1543`) — peek→notify→commit,
  no launch/file edge.
Neither removes its underlying condition, so both re-observe every cycle and keep
re-appearing in the composite. The coupling is thus a **shared failure mode
(non-closure), expressed as shared composite membership** — not a dependency.

---

## 3. Write-back self-feed loop — fully traced 5-edge cycle (DEFECT)

The Overseer's persisted output re-enters its own observation input:

```
 (tick N)
 [E1] observation_signature(problems)            mod.rs:1068-1073
        = "overseer-obs:" + sort/dedup/join(dedup_keys)   → S = "overseer-obs:A|B"
 [E2] record_observation embeds the marker        wiring.rs:1084
        content = "{summary} [sig:overseer-obs:A|B]"; metadata{signature:S}
        store_episode(OVERSEER_SOURCE_LABEL)       wiring.rs:1088
        ▼   (persisted into the multi-writer cognitive-memory graph)
 (tick N+1)
 [E3] recall_episodic parses the marker back       wiring.rs:1025 + parse_failure_signature 977-985
        RecalledEpisode.failure_signature = Some("overseer-obs:A|B")
 [E4] signals_from counts ≥2 identical sigs →       signal.rs:455-470
        Signal::RecurringSignature{ signature:"overseer-obs:A|B", occurrences }
        (floor RECURRING_SIGNATURE_THRESHOLD = 2, signal.rs:362)
 [E5] problem_from_signal: dedup_key = RAW signature  mod.rs:1353-1363 (key at :1359)
        = sanitize_recalled("overseer-obs:A|B") = "overseer-obs:A|B"
        ▼   (this problem joins THIS tick's problem set)
 → back to [E1]: observation_signature now folds "overseer-obs:A|B" as a member key
        → "overseer-obs:overseer-obs:A|B|…"   (NESTING deepens each cycle)
```

**Why the dedup gate does not stop it (the key architectural failure):**
`write_back_observation` gates on `write_back_gate.peek(&signature)` (`mod.rs:548`),
an **exact-string** WhisperGate. But edge [E5]→[E1] **mutates** the signature every
cycle (each nesting level is a *different* string), so every write-back looks novel
→ `WhisperDecision::Deliver` → persisted again (`mod.rs:549-557`). The dedup
primitive is defeated by the very feedback it is meant to suppress. This is why the
signature accretes into the ~20 KB pipe-delimited blob (nested `overseer-obs:`
prefixes; repeated `goal:blocked:*`; `workstream-gap|workstream-gap` doubling).

**"2×" reconciliation (my lens):** the `2×` is an **honest recurrence count** —
edge [E4] fires only when ≥2 recalled episodes truly share the signature (a
genuinely-persisted-more-than-once string). It is **not** a same-write duplicate.
BUT the *reason* it recurs is the self-feed re-persisting variants **plus** the
unresolved underlying conditions (blocked goals never closed, gaps never launched).
So the answer is **BOTH**: honest re-observation AND a defect — the defect is the
loop that keeps manufacturing recurrence, not the counter. (Consistent with primary
`…recurrence_VERDICT_HEAD_b9f99879.md` and secondary `…3fac68a5.md §5`.)

**`workstream-gap|workstream-gap` doubling is NOT two distinct gap keys** — the
`WorkstreamCoverage` Problem carries the single bare dedup_key `"workstream-gap"`
(`mod.rs:1371`). The doubling is the [E5] recall re-entry nesting a prior
composite that already contained `workstream-gap` alongside a freshly-emitted one.
Same D1 mechanism, not a second counting bug.

---

## 4. Independent verification at HEAD `3fac68a5`

- `git diff --stat f455c06d..HEAD -- src/` → **empty** (production byte-identical).
- `cargo test --lib tests_memory_recall` → **32 passed / 0 failed** (self-feed /
  RecurringSignature admission + `[sig:]` round-trip seam).
- `cargo test --lib tests_gap_scan` → **21 passed / 0 failed** (gap arm notify-only,
  no-launch invariant, identity fail-closed).
- Every §2/§3 file:line read directly from live `src/` at HEAD; no citation trusted
  from prior docs.

---

## 5. Actionable remediation direction (advisory — nothing landed)

**Do NOT touch the signals.** `engineer_spawn`, `workstream-gap`, `goal:blocked:*`
are honest. **Do NOT special-case the spawn↔gap pair** — the coupling is benign
co-occurrence; it stabilizes for free once the loop-breaker freezes the signature
set. Reuses the landing order from the prior tertiary wave (unchanged):

```
 [1] Write-back self-observation guard  (loop-breaker; no deps)   seam mod.rs:546
       Drop recall-derived problems (dedup_key starts_with "overseer-obs:")
       AND RecurringSignature-only problems from the slice fed to
       observation_signature / observation_content.
       → severs edge [E5]→[E1]; stops nesting; FREEZES the signature SET so the
         exact-string dedup gate finally works and the composite becomes stable.
       ▼
 [2] Lane-B count-in-content + WHY-gate  (atomic latch)  mod.rs:1004-1043 / 972-997 / 1180-1185
       Signature-keyed UPSERT with occurrence_count/first_seen/last_seen; escalation
       reads that field, not recall.len(). Makes recurrence mean "distinct windows".
       ▼
 [3] Closing rungs  (consume the now-honest, stable count)
       (3a) decide_blocked_goal dead-zone rung   mod.rs:1603-1631
       (3b) gap-quarantine launch/escalate edge + cross-window ledger keyed on
            GapItem.signature (NOT bare "workstream-gap")   mod.rs:884-940 / 1534-1543
```

**Architect-specific notes for MY focus:**
- The **only** structural change the coupling analysis adds is a caution: any
  future "resource-aware launch" for the gap arm ([3b]) must **not** read the
  `engineer_spawn` signal to throttle gap launches — that would *manufacture* the
  causal coupling the code currently (correctly) lacks and re-introduce a feedback
  edge. Keep the arms independent; gate [3b] on `GapItem.signature` cross-window
  recurrence only.
- The self-feed fix is **[1] alone** for the coupling/over-aggregation symptom; it
  is the highest-leverage, lowest-risk edit (single seam, no trait/storage change).
  Everything else ([2],[3]) addresses the *underlying* non-closure so the honest
  recurrence trends to zero.
- No security surface: provenance is fixed (`OVERSEER_SOURCE_LABEL`, `wiring.rs:1088`),
  recalled text is `sanitize_recalled`-cleaned at admission (`mod.rs:1359`). This is
  a **control-flow feedback** defect, not injection.

---

## 6. Reconciliation with prior waves (validate-don't-re-derive)

| Prior claim | Artifact | Status @ 3fac68a5 |
|---|---|---|
| spawn/gap are ordinary leaf dedup_keys, aggregate members not separate sigs | tertiary…f455c06d §6 | ✅ re-verified (§2) |
| self-feed loop (recall→RecurringSignature→dedup_key→signature nesting) | primary self_ingestion / secondary composite_selffeed | ✅ full 5-edge trace re-read (§3) |
| dedup gate defeated by mutating signature | (extends prior — sharpened here) | ✅ new emphasis, code-cited (§3) |
| "2×" = honest re-observation; defect = missing closing action | primary VERDICT / secondary §5 | ✅ concur (§3) |
| gap arm notify-only, no launch edge; bare "workstream-gap" dedup_key | secondary…3fac68a5 §3 | ✅ re-verified (§2, §5) |
| ResourcePressure decide = Escalate (notify-only) | (this wave) | ✅ mod.rs:1444/1119-1124 |
| landing order [1]→[2]→[3] | tertiary…f455c06d §4 | ✅ unchanged (§5) |

**No prior verdict superseded.** This wave's net-new contributions:
(a) a definitive **correlational-not-causal** verdict on the spawn↔gap pair with the
independent-field code proof; (b) the **dedup-gate-defeated-by-mutating-signature**
mechanism made explicit as the reason the loop is unbounded; (c) the caution that
[3b] must stay decoupled from the spawn signal.

**This is an investigation deliverable. No source changed.**
