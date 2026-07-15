# Primary deep-dive — token emitters + dedup/idempotency gate

**Investigator role:** PRIMARY
**HEAD:** `f1db90f4`
**Focus:** Full data flow `sensor.Observe → signals_from → classify_signal →
observation_signature → cognitive-memory write-back`, the dedup/idempotency
gate, and the exact condition (daemon **restart** vs. **>900 s window expiry**)
that lets an identical `overseer-obs:…` signature be re-persisted.

**Signature under investigation (seen 2× in cognitive memory):**
`overseer-obs:goal:blocked:…|overseer-obs:goal:blocked:…|goal:blocked:…|…|workstream-gap|resource:engineer_spawn`

---

## 1. The end-to-end pipeline (file/line grounded)

| Stage | Where | What it produces |
|---|---|---|
| **Observe** | `sensor.rs:105` `observed_from_snapshot` / `sensor.rs:584` tick | `ObservedState` (blocked goals, gaps, live engineers, `recall` snapshot) |
| **Emit signals** | `signal.rs:366` `signals_from` | `Vec<Signal>` — one `GoalBlocked` per blocked goal (`signal.rs:440-448`), plus recall-derived `RecurringSignature` (`signal.rs:455-470`) |
| **Classify → key** | `mod.rs:1238` `classify_signal` | `(kind, priority, dedup_key, summary)` per signal |
| **Orient / fold** | `mod.rs:1200` `orient` | ranked, key-deduped `Vec<Problem>` |
| **Build signature** | `mod.rs:1068` `observation_signature` | `overseer-obs:` + sorted+deduped `dedup_key`s joined by `\|` |
| **Idempotency gate** | `mod.rs:534` `write_back_observation` → `write_back_gate` | peek → (store) → commit |
| **Persist** | `wiring.rs:1076` `record_observation` → `store_episode` | episode body `"<content> [sig:overseer-obs:…]"` |

### Token emitters — where each piece of the composite key comes from
- `goal:blocked:<goal_id>` → `classify_signal` `GoalBlocked` arm, `mod.rs:1336`.
- `workstream-gap` (constant) → `WorkstreamGap` arm, `mod.rs:1371`.
- `resource:engineer_spawn` (constant) → `EngineerSpawnRate` arm, `mod.rs:1270`.
- `overseer-obs:` prefix and `|` join → `observation_signature`, `mod.rs:1072`.
- The **nested** `overseer-obs:goal:blocked:…` tokens → `RecurringSignature`
  arm, `mod.rs:1359` (see §3, the feedback path).

The threshold that turns "≥2 recalled episodes sharing a signature" into a
`RecurringSignature` is `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`).
This is the source of the "seen **2×**" wording in the summary line
(`mod.rs:1361`).

---

## 2. The dedup / idempotency gate — and the exact re-persistence condition

The write-back is gated by a reused `WhisperGate` primitive:
`write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:299`), keyed on the
observation signature. Flow (`mod.rs:546-557`): `observation_signature` →
`peek(sig, now)` → on `Deliver`, `store_episode` → `commit(sig, now)`. The slot
is consumed **only after a successful store** (`mod.rs:555-556`), so a failed
write never suppresses a later one.

`WhisperGate` internals (`guardrails.rs:291-333`):
```
window_secs: i64,
cap_per_hour: usize,
last_delivered: HashMap<String, i64>,   // ← in-memory, per-process
deliveries: Vec<i64>,
```
`peek` suppresses as a duplicate **iff** `now - last < window_secs`
(`guardrails.rs:313-317`). So within one process a given signature is stored at
**most once per 900 s**.

### The two — and only two — ways an identical signature is re-persisted

A second identical episode appears only when `peek` does **not** return
`SuppressDuplicate`:

1. **>900 s window expiry (same process).** The same near-static problem set is
   re-observed on a tick ≥ 900 s after the last store. `now - last ≥ 900` ⇒
   `peek` falls through to `Deliver` ⇒ a second store with the identical key.
   This is *honest cross-window re-observation*, not a dedup bug.

2. **Daemon restart.** `last_delivered` is an in-memory `HashMap`
   (`guardrails.rs:294`) with **no persistence**. `Overseer::new` reconstructs a
   fresh `WhisperGate::new(900, 5)` (`mod.rs:299`), wiping all dedup state. The
   very next tick that observes the same problem set sees an empty map ⇒
   `Deliver` ⇒ re-store, even if < 900 s of wall-clock elapsed since the prior
   store in the previous process.

**Both** conditions yield exactly-2× (and N× over N windows/restarts). The
episode store (`store_episode`) is append-only and unconditional at the backend;
the only idempotency is the per-process 900 s gate, so it cannot deduplicate
across restarts. **From the signature alone the two causes are
indistinguishable** — no restart/window timestamps are captured on the episode.
(Confirmed by the deterministic tests: `write_back_is_deduplicated_within_window`
`tests_memory_recall.rs:796` proves within-window suppression; there is no
cross-restart persistence of the gate, by construction.)

---

## 3. Why the signature is *mixed* (`goal:blocked:…` **and**
`overseer-obs:goal:blocked:…`) — the recall feedback path

The composite in the question interleaves plain `goal:blocked:…` keys with
`overseer-obs:goal:blocked:…` keys. That specific shape is produced by a
self-referential recall path, not by a single Observe pass:

1. A prior write-back persists an episode whose body carries
   `[sig:overseer-obs:goal:blocked:…]` (`wiring.rs:1084`).
2. On a later cycle, `recall_episodic` reads that episode back and
   `parse_failure_signature` (`wiring.rs:976-986`) extracts **any** `[sig:…]`
   marker — including the Overseer's *own* `overseer-obs:…` signature — into
   `RecalledEpisode.failure_signature` (`wiring.rs:1025`).
3. If ≥2 recalled episodes carry that same `overseer-obs:…` signature,
   `signals_from` emits `RecurringSignature { signature: "overseer-obs:…" }`
   (`signal.rs:455-470`).
4. `classify_signal` maps it with `dedup_key = sanitize_recalled(signature)`
   (`mod.rs:1359`). `sanitize_recalled` (`capabilities.rs:468-482`) only strips
   control chars / caps length — it does **not** strip the `overseer-obs:`
   prefix. So the key stays `overseer-obs:goal:blocked:…`.
5. In `orient` (`mod.rs:1211`) this key differs from a fresh `goal:blocked:…`
   key, so the two do **not** merge — both survive as separate `Problem`s.
6. The next `observation_signature` therefore folds **both** families →
   `overseer-obs:` + sorted[`goal:blocked:…`, `overseer-obs:goal:blocked:…`, …],
   exactly the mixed, growing composite observed.

This is a genuine **feedback smell**: the Overseer's own write-back is eligible
to be recalled and re-emitted as a recurrence of itself, ratcheting the
composite signature wider each window. It is currently *bounded* (recall budget
`episodic: 5`, `capabilities.rs:502; RecallBudget::default`) and *isolated from
escalation* — see §4 — but it is the mechanism behind the nested tokens.

---

## 4. What this is NOT (isolation that holds at HEAD)

- **Lane A (episodic recurrence) ≠ Lane B (root-cause escalation).** The visible
  2× lives in Lane A: `store_episode`, +1 per 900 s window, threshold **2**
  (`RECURRING_SIGNATURE_THRESHOLD`, `signal.rs:362`). Escalation runs on a
  separate append-only occurrence-fact counter (`store_fact`, threshold **3**),
  with no shared counter. Verified net-new by
  `tests_root_cause.rs` (commit `f9cefec1`): a loud Lane-A `RecurringSignature`
  with empty Lane-B recall leaves recurrence at 0 and self-heals. So the
  feedback loop in §3 does **not** trip an unwarranted escalation.
- **Not a dedup/storage defect.** The 900 s gate behaves exactly as specified;
  re-persistence is either honest re-observation (>900 s) or expected loss of
  per-process gate state (restart). No double-count, no lost write.

---

## 5. Verdict (for this focus)

The recurring `overseer-obs:…` signature is an **honest cross-window / cross-
restart re-observation** of a genuinely re-observed, near-static problem set
(perpetually blocked kgpacks-rs goals + a standing workstream-coverage gap +
elevated engineer spawn — three views of one under-throughput condition). The
idempotency guarantee is intentionally scoped to a single process's 900 s
window; **restart** and **>900 s window expiry** are the two — and only two —
conditions that re-persist an identical signature, and they are not
distinguishable from the signature alone.

The one design-level concern surfaced by this focus is the **recall feedback
path (§3)**: `parse_failure_signature` does not exclude the Overseer's own
`overseer-obs:` provenance, so write-backs re-enter as `RecurringSignature`s and
widen the composite key over time. It is bounded and escalation-isolated today,
but the correct hardening is to make the write-back lane ignore
`overseer-obs:`-prefixed `failure_signature`s on recall (or strip that prefix in
`sanitize_recalled`'s dedup-key path), so the Overseer never counts itself.

### Suggested, minimal hardening (not applied — investigation only)
- Filter self-provenance on recall: in `recall_episodic`
  (`wiring.rs:1013-1031`) or in `signals_from`'s recurrence counter
  (`signal.rs:455-470`), skip episodes whose `failure_signature` starts with
  `overseer-obs:`. Zero behaviour change for real failure signatures; closes the
  self-amplification.
- Optional observability: stamp `episode metadata.signature` with a
  window/epoch or restart-id so a future reader can attribute a 2× to window vs.
  restart (removes the "indistinguishable" gap in §2).
