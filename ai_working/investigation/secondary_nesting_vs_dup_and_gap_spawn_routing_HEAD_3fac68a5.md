# Secondary Investigation — Token doubling & over-aggregation (D1 nesting vs. true duplication) + workstream-gap ↔ resource:engineer_spawn coupling across sensor.rs / wiring.rs / routing.rs

**Role:** SECONDARY investigator (patterns). Reconcile-and-extend.
**HEAD verified:** `3fac68a5288e965a1aceee029a3e10ae105db3c0` (`git rev-parse HEAD`).
**Src drift:** `git diff --stat f455c06d..HEAD -- src/` → **empty** (the two intervening
commits are docs/investigation only). Every source line cited below was re-read live at
HEAD, not trusted from prior docs. All verify **verbatim**.
**Empirical:** `cargo test --lib whisper` → **28 passed / 0 failed**;
`cargo test --lib gap_scan` → **21 passed / 0 failed** (both re-run by me at this HEAD).

---

## 0. One-line verdict

The doubling (`workstream-gap|workstream-gap`, nested `overseer-obs:…|overseer-obs:…`,
repeated `goal:blocked` blocks) is **D1 self-observation nesting, not true per-token
duplication and not a counting bug.** The `2×` is an **honest** Lane-A occurrence tally.
`workstream-gap` and `resource:engineer_spawn` are **independent leaf signals** joined only
by set-hash co-membership in one write-back composite (**over-aggregation, not coupling**).
**`stewardship/routing.rs` is a DEAD END** for the coupling question — it is a pure
`source_module→repo` router that never reads either signal and is not even on the current
(notify-only) code path.

---

## 1. Structural proof: nesting, NOT true duplication

**Decisive invariant — each dedup_key appears at most once per snapshot:**
- `observation_signature` does `keys.sort_unstable(); keys.dedup(); "overseer-obs:"+join("|")`
  (`mod.rs:1068-1073`). `dedup()` collapses adjacent equals **after sort**.
- `orient` merges any two same-`dedup_key` signals into one `Problem` before the signature
  is built (prior-verified `mod.rs:1200-1221`).

Therefore a literal `workstream-gap|workstream-gap` (or repeated `overseer-obs:`) **inside one
composite is impossible from same-tick duplication.** It can *only* arise when a **nested
recalled** `overseer-obs:…|workstream-gap|…` fragment — a *distinct string* that embeds its
own `workstream-gap` and so survives `dedup()` — sorts next to a freshly-emitted bare
`workstream-gap`. **The doubling is a positive fingerprint of the D1 self-feed**, confirming
prior wave `secondary_nesting_vs_duplication_token_class_HEAD_bbddd23a §2`.

**The self-feed edge, re-grounded live:**
`write_back_observation(&cycle.problems)` (single call site `wiring.rs:301`) builds the
signature over **all** `cycle.problems` with **no exclusion** of recall-derived problems
(`mod.rs:534-563`). `record_observation` embeds the `[sig:…]` marker under a fixed
`OVERSEER_SOURCE_LABEL` (`wiring.rs:1076-1091`); a later tick recalls it, `signals_from`
re-emits `RecurringSignature{signature,…}` at `≥2` (`signal.rs:455-470`,
`RECURRING_SIGNATURE_THRESHOLD=2` `signal.rs:362`), and `classify_signal` re-admits it with
**`dedup_key = sanitize_recalled(signature)`** — an `overseer-obs:…` string (`mod.rs:1353-1363`,
summary matches the investigation-question string verbatim). That problem re-enters the next
`cycle.problems` → the composite gains one more `overseer-obs:` prefix. **Nesting deepens one
level per recall cycle.**

**Why over-aggregation makes ONE oversized record:** the exact-string `WhisperGate` at
`mod.rs:548` cannot dampen it because each nesting level is a *different* string → always
`Deliver` → re-persisted. Each nested fragment carries its own full copy of the `goal:blocked`
block, so concatenating nested snapshots is exactly what inflates one record to the ~20 KB blob.
**This is over-aggregation-by-nesting, not a concat/merge bug.**

**`2×` classification (signal, not defect):** honest Lane-A tally. The counter fires only when
≥2 recalled episodes genuinely share the signature (`signal.rs:455-470`). Within-window dedup
is proven green (`tests_whisper` 28/0). The second episode is a cross-window / daemon-restart
re-observation (in-memory `WhisperGate(900,5)`), not a same-write duplicate. **The defect is the
loop that manufactures recurrence, not the count.**

---

## 2. workstream-gap ↔ resource:engineer_spawn — CORRELATIONAL, not causal

Two **independent** leaf signals from two **disjoint** snapshot fields; **no cross-read**:

| Token | Signal | Sole input | Emit | Classify (dedup_key) |
|---|---|---|---|---|
| `resource:engineer_spawn` | `EngineerSpawnRate{live}` | `state.live_engineers` (`sensor.rs:123`, from resources snapshot) | `signal.rs:393-397` (`live>=ENGINEER_SPAWN_THRESHOLD`) | fixed literal `"resource:engineer_spawn"`, `Priority::Normal`, `{live}` in **summary only** (`mod.rs:1267-1272`) |
| `workstream-gap` | `WorkstreamGap{gaps}` | `state.workstream_gaps` (populated by `detect_workstream_gaps`, `wiring.rs:772`, **not** the read-only snapshot: `sensor.rs:153` leaves it empty) | `signal.rs:475-479` (non-empty) | fixed literal `"workstream-gap"`, `Priority::High`, `{gaps.len()}` in **summary only** (`mod.rs:1368-1373`) |

- **Disjoint detectors, disjoint goal partitions (net-new evidence).** `detect_workstream_gaps`
  (`sensor.rs:288-320`) **explicitly skips blocked goals** — `if matches!(g.status,
  GoalProgress::Blocked(_)) { continue; }` with the comment *"Blocked goals flow through
  goal_health; never re-flag them here."* So `goal:blocked:*` and `workstream-gap` come from
  **two separate detectors over non-overlapping goal sets**, and neither reads `live_engineers`.
  There is no code edge between the spawn field and the gap field.
- **Only binder = set-hash co-membership.** When both cross threshold in the *same* Observe
  snapshot, both fixed-literal keys land in the one `observation_signature` composite. That is
  **over-aggregation (shared write-back window)**, not a dependency.
- **Latent common cause (domain, code-invisible):** an under-resourced Simard can run ≥8 live
  engineers *and* leave workstreams uncovered — the "one oscillating root problem." Real, but
  **not** a token-to-token causal edge; do **not** special-case the pair.
- **Both volatile counts (`{live}`, `{gaps.len()}`) stay in summaries — never in a dedup_key.**
  So neither perturbs dedup. `resource:engineer_spawn` = **benign membership drift / telemetry**.

### routing.rs — dead end for coupling (net-new)
`stewardship/routing.rs` (whole file, 53 lines) is a **total `source_module → TargetRepo`
router** for issue filing. It reads only a source-module *string* against keyword lists
(`AMPLIHACK_KEYWORDS`, `SIMARD_KEYWORDS`) and falls back to `rysweet/Simard`
(`route_failure`, `routing.rs:39-52`). It **never reads `live_engineers` or gap counts**; it
only *mentions* "the Overseer's `overseer` workstream-gap briefs" in a comment
(`routing.rs:12-14`) explaining the default fallback. Moreover the `WorkstreamCoverage` Decide
arm is **notify-only** (no FileIssue/LaunchRecipe edge — prior-verified `mod.rs:1534-1543`), so
gap briefs never reach the filing path that would invoke `route_failure` at all. **routing.rs is
dormant relative to this signature and contributes nothing to the coupling.** (Confirms the
strategy's own "potential dead end" list.)

---

## 3. Patterns / anti-patterns

- **Self-observation feedback (D1)** — present at HEAD: write boundary (`mod.rs:546`) does not
  exclude recall-derived (`overseer-obs:`/`RecurringSignature`) dedup_keys. *Answers the nesting
  question.*
- **Over-aggregation via nesting** — the set-hash concatenates nested recalled snapshots into one
  oversized record; the mutating signature defeats the exact-string dedup gate.
- **Two signatures, one root problem** — gap (active) + spawn (telemetry) are two VIEWS of one
  under-resourcing condition; treat as one problem, not two bugs. Do not couple them in code.
- **Meta-pattern (holds):** *the recurrence count is honest — audit the closing action.*

## 4. Integration points
`wiring.rs:301` (single write-back call site — D1 seam) · `wiring.rs:1076-1091` (marker embed) ·
`wiring.rs:772` (gap injection) · `mod.rs:534-563` / `:1068-1073` (write boundary + set-hash) ·
`mod.rs:1267-1272` / `:1368-1373` (spawn/gap classify) · `mod.rs:1353-1363` (RecurringSignature) ·
`signal.rs:362,393-397,455-470,475-479` · `sensor.rs:123,153,288-320` (independent fields + blocked
skip) · `stewardship/routing.rs` (dormant repo router).

## 5. Questions for verification phase
1. **D1 fix seam:** exclude recall-derived (`overseer-obs:`/`RecurringSignature`) dedup_keys from
   the slice fed to `observation_signature` at `mod.rs:546` — confirm it stops nesting and FREEZES
   the signature set (so the exact-string gate finally converges) without suppressing legitimate
   first-order recurrence.
2. **Keep arms decoupled:** any future resource-aware gap launch must **not** read the
   `engineer_spawn` signal — doing so would *manufacture* the causal coupling the code currently
   (correctly) lacks.
3. **INV-GAP-KEY:** any gap remediation rung must key on `GapItem.signature`, not the bare
   `"workstream-gap"` dedup_key, or all gaps fold into one issue.
4. Confirm the 2nd episode's provenance (cross-window rollover vs. daemon restart) is telemetry,
   not a storage replay — a goal-board/telemetry check, not a source check.

## 6. Reconciliation
Concurs with and re-grounds `tertiary_architecture_SPAWN_GAP_COUPLING_AND_SELFFEED_HEAD_3fac68a5`
(correlational-not-causal; 5-edge self-feed) and
`secondary_nesting_vs_duplication_token_class_HEAD_bbddd23a` (nesting fingerprint; benign drift).
**Net-new this wave:** (a) the **disjoint-detector / blocked-skip** proof that gap and blocked
goals never co-derive (`sensor.rs:299-302`); (b) a **direct read of `routing.rs`** establishing it
as dormant and non-coupling; (c) independent green re-run of whisper + gap_scan at HEAD.
**No prior verdict superseded. This is an investigation deliverable — no source changed.**
