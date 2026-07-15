# Tertiary Investigation (architect) — Lane isolation, signature idempotency & `resource:engineer_spawn` membership-drift impact on recurrence identity

**Role:** TERTIARY investigator (architect).
**HEAD:** `f1db90f4` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Assigned focus:** Lane-A/Lane-B recurrence isolation, **signature idempotency**, and
**`resource:engineer_spawn` membership-drift impact on recurrence identity**.
**Method:** Re-read every load-bearing line in `src/overseer/`; traced the full
write-back→recall→count loop; verified against prior tertiary artifact
`tertiary_lane_isolation_signal_vs_defect_VERDICT_HEAD_f9cefec1.md`.
**Drift check:** `git diff --stat f9cefec1 HEAD -- src/` → **empty** (HEAD is a
docs-only commit). All prior source citations hold verbatim.

---

## 0. One-line verdict

**Signature idempotency is _conditional_: `observation_signature` is stable under
ordering/duplication but is a _set-hash over the whole tick's problem membership_,
so it is NOT stable under membership drift. `resource:engineer_spawn` is a volatile
incidental co-member that _forks the composite signature identity_ — but that fork
is confined to the self-fed advisory Lane-A. Lane-B (the escalation-critical lane)
keys recurrence on the _per-problem `dedup_key`_ and is therefore _immune_ to
composite membership drift. Net: membership drift is a benign-but-latent
_precision_ defect in Lane-A, not a correctness defect in escalation.**

---

## 1. The composite signature feeds back into its OWN Lane-A detector (the ×2 loop)

The prior wave established "Lane-A does not feed Lane-B." For my focus the sharper
architectural truth is: **Lane-A feeds _itself_**, and that self-feed is precisely
what manufactures the observed `overseer-obs:…` ×2. The loop, cited end-to-end:

1. **Assemble** — `observation_signature(problems)` = `"overseer-obs:" +
   sort→dedup(dedup_keys).join("|")` (`mod.rs:1068-1074`). A **set-hash**.
2. **Gate** — `write_back_observation` keys the `write_back_gate` (`WhisperGate`,
   900 s / 5-per-hour, `mod.rs:299`) on that composite string (`mod.rs:546-556`).
3. **Persist** — `record_observation` stores episode content
   `"{content} [sig:{composite}]"` (`wiring.rs:1084`).
4. **Recall** — `recall_episodic` reconstructs `failure_signature =
   parse_failure_signature(content)`, extracting the `overseer-obs:…` string back
   out of the `[sig:…]` marker (`wiring.rs:976-987, 1025`).
5. **Count** — `signals_from` groups recalled episodes by `ep.failure_signature`
   and emits `RecurringSignature{signature, occurrences}` at `≥2`
   (`signal.rs:455-470`, `RECURRING_SIGNATURE_THRESHOLD = 2` `signal.rs:362`).

So the exact composite string the write-back emits is later recalled and counted
as a `RecurringSignature`. **This self-referential loop is the origin of the user's
"seen 2×".** It is _intended_ (memory-backed cross-window recurrence rather than
in-process counters — the design note at `signal.rs:449-453`), not a replay bug.

---

## 2. Signature idempotency — precisely characterised

`observation_signature` (`mod.rs:1068-1074`):

- **Idempotent under ordering** — `sort_unstable()` ⇒ member order irrelevant.
- **Idempotent under duplication** — `dedup()` ⇒ repeated `dedup_key`s collapse.
- **NOT idempotent under membership drift** — it hashes the **entire tick's problem
  set**. Add or remove any one member and the string changes identity.

Therefore signature identity is only stable while the *whole* co-occurring problem
set is stable. It couples the identity of "the persistently-blocked cluster" to
every incidental problem that happens to be observed in the same tick.

---

## 3. `resource:engineer_spawn` membership drift — the impact on recurrence identity

`Signal::EngineerSpawnRate{live}` → `dedup_key = "resource:engineer_spawn"`,
`ProblemKind::ResourcePressure`, `Priority::Normal` (`mod.rs:1266-1271`), fired at
`ENGINEER_SPAWN_THRESHOLD = 8` live spawns (`signal.rs:351`). It is **benign,
transient telemetry uncorrelated with the blocked-goal cluster** — it toggles in
and out of the problem set as unrelated engineer spawn load crosses 8.

Each toggle changes the composite set-hash, e.g.:

```
S  = overseer-obs:goal:blocked:A|goal:blocked:B|workstream-gap
S' = overseer-obs:goal:blocked:A|goal:blocked:B|resource:engineer_spawn|workstream-gap
```

Two identity-level consequences on the Lane-A loop:

- **(a) Write-back dedup defeated.** `write_back_gate` keys on the composite. `S`
  and `S'` are distinct keys ⇒ the 900 s window does **not** suppress ⇒ **both are
  persisted** as separate episodes. Incidental spawn churn thus writes near-
  duplicate observations into the append-only episode store.
- **(b) Lane-A bucket fragmentation / undercount.** `signals_from` counts by exact
  `failure_signature`. `S`-episodes and `S'`-episodes fall in **different buckets**,
  so the recurrence of one genuinely-static blocked cluster is **split** across
  membership variants; each variant must independently reach `≥2`. Recurrence is
  therefore **undercounted** relative to the true persistence of the cluster.

**This is the fingerprint visible in the observed signature blob:** variants with
and without `resource:engineer_spawn`, with and without `workstream-gap`, and from
a single member up to the full ~14-goal cluster. That heterogeneity is exactly what
membership drift over a set-hash produces, and it explains why the count sits at a
low "×2" despite a persistently blocked world — drift keeps re-forking the bucket.

---

## 4. Lane-A vs Lane-B isolation — restated through the identity-granularity lens

The isolation is not merely "different counters/thresholds"; it is **different
recurrence-identity granularity**, and the fragile granularity is quarantined to
the non-escalating lane:

| | **Lane A (advisory)** | **Lane B (escalation)** |
|---|---|---|
| Recurrence identity | composite `overseer-obs:{set-hash}` | per-problem `dedup_key` |
| Source | `[sig:]` self-feed → `parse_failure_signature` (`wiring.rs:976,1025`) | `recall_occurrences(&problem.dedup_key)` → `occurrence_concept(dedup_key)`, filtered `o.signature == dedup_key` (`mod.rs:456,972-997`) |
| Threshold | `2` (`signal.rs:362`) | `3` (`root_cause.rs`) |
| Membership-drift sensitivity | **YES — forks on any co-member change** | **NO — keyed on the single problem only** |
| Decision | raises priority in `orient` (`mod.rs:1353-1363`) | `EscalateBlockedGoal` at `≥3` |

**Architectural saving grace:** because Lane-B recurrence is keyed on the
individual problem's `dedup_key` (`mod.rs:456`), the escalation-critical path is
**structurally immune** to composite membership drift. `resource:engineer_spawn`
entering/leaving a tick can never reset, inflate, or deflate a blocked goal's
Lane-B `recurrence`. The membership-fragile identity lives only where it can do the
least harm: the advisory, priority-raising Lane-A. This confirms and _sharpens_ the
prior wave's isolation finding — the two lanes differ not just in counters but in
**identity stability**, and the unstable one is correctly confined.

---

## 5. Defect-vs-benign adjudication (architect verdict)

- **`resource:engineer_spawn` drift is a benign-but-latent Lane-A precision defect,
  NOT a correctness defect.** It (a) weakens the write-back 900 s dedup and grows
  the episode store with near-duplicates, and (b) fragments/undercounts Lane-A
  recurrence. It **cannot** corrupt escalation (Lane-B is per-`dedup_key`).
- **Root design smell:** Lane-A recurrence identity is simultaneously **over-
  aggregated** (many goals share one hash — cannot tell 2 gaps from 20; the coverage
  `dedup_key` is the constant `"workstream-gap"`, `mod.rs`) **and identity-fragile**
  (incidental co-members fork it). Over-aggregation and fragility are the two faces
  of keying recurrence on the *whole-tick set* instead of the *condition*.
- **Not a cause of the stall.** `resource:engineer_spawn` gates no goal work; it is
  telemetry only. Do **not** build spawn-capacity controls for the blocked cluster.
  (Confirms prior "false lead" verdict.)

---

## 6. Recommendation (diagnosis only — no fixes, underlying goals OUT OF SCOPE)

If Lane-A recurrence precision is later hardened, the membership-drift class is
closed by **keying advisory recurrence on the condition, not the tick-set**:

1. **Per-condition Lane-A identity.** Emit/track `RecurringSignature` on each
   *individual* `dedup_key` recalled ≥2 across windows, rather than on the composite
   `overseer-obs:{set-hash}`. This makes Lane-A identity membership-stable (matches
   Lane-B's granularity) and simultaneously cures the over-aggregation opacity.
2. **If the composite write-back is kept**, exclude volatile telemetry members
   (`resource:*`) from `observation_signature` so incidental churn cannot fork the
   identity or defeat the 900 s write-back dedup.
3. **Do NOT** merge the lanes, silence the ×2, or add spawn controls. The counter,
   the isolation, and the intended self-feed are all correct.

The dominant defects remain those the prior waves settled and are upstream of my
scope: the **missing closing edge** (steering-vs-closing asymmetry) and the **D0
completion-gate anchor** that keeps the measured world static. Membership drift
only degrades how *precisely* Lane-A reports that static world.

---

## 7. Verification performed

- Re-read at HEAD `f1db90f4`: `mod.rs:299,428,440-470,534-563,965-1000,1068-1074,
  1147,1266-1271,1353-1363`; `wiring.rs:976-987,1025,1084`; `signal.rs:351,362,
  440-470`; `guardrails.rs:291-329`; `root_cause.rs` (`recall_occurrences` filter).
- `git diff --stat f9cefec1 HEAD -- src/` → **empty** (docs-only commit; prior
  tertiary citations hold verbatim).
- Traced the complete self-feed loop
  (`observation_signature`→`write_back_gate`→`record_observation` `[sig:]`
  →`parse_failure_signature`→`signals_from` count) confirming the composite is
  what Lane-A counts as the observed ×2.
- Confirmed Lane-B keys on per-problem `dedup_key` (`mod.rs:456,976`, filter
  `o.signature == dedup_key` `mod.rs:990`) ⇒ membership-drift-immune.

**Bottom line:** `observation_signature` is idempotent under order/duplication but
is a membership-sensitive set-hash. `resource:engineer_spawn` drift forks Lane-A's
composite recurrence identity (defeating write-back dedup and fragmenting the ×2
count), yet cannot touch Lane-B escalation, which is keyed per-`dedup_key`. The
drift is a benign, quarantined Lane-A precision defect — real, but not the cause of
the stall and not a threat to escalation correctness.
