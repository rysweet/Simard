# Primary Deep-Dive — Emission Pipeline, Dedup Keying, and `==2` Occurrence Semantics

**Role:** PRIMARY investigator.
**Focus:** Emission pipeline trace + dedup keying + `==2` occurrence semantics across `src/overseer/`.
**HEAD:** `25d4c5a6` (independently re-read every load-bearing line at this HEAD).
**Empirical status:** `cargo test --lib overseer::tests_memory_recall` → **32 passed, 0 failed**.
**Verdict:** The recurring `overseer-obs:…|goal:blocked:…|workstream-gap` signature is the
Overseer's **own observation write-back signature**, re-observed through a **self-ingesting
recall→promote→write-back loop**. "Seen 2×" is the honest firing of
`RECURRING_SIGNATURE_THRESHOLD = 2`. This is **not** a storage, replay, or dedup bug — it is a
**structural loop with no self-reference guard and no closing (remediation) rung**. Consistent
with, and a sharpening of, the committed `SYNTHESIS.md` and `RECONCILIATION_LEDGER.md`.

---

## 1. Emission pipeline — one full trace (Observe → write-back)

Single assembly path, once per surviving tick (`wiring.rs:301` is the **only** call site of
`write_back_observation`):

```
run_cycle (mod.rs:~366)
  ├─ Observe: board read, gaps, recall_pass (mod.rs:423-438)
  ├─ signals_from(&observed)            (mod.rs:441 → signal.rs:366)
  ├─ orient(signals, in_flight)         (mod.rs:447 → mod.rs:1200)
  │     └─ signal_to_problem stamps each Problem.dedup_key   (mod.rs:1336, 1353, 1371)
  ├─ (root-cause enrichment per problem) (mod.rs:455-459)
  └─ Act: write_back_observation(&cycle.problems)  (wiring.rs:301 → mod.rs:534)
        ├─ signature = observation_signature(problems)        (mod.rs:546 → 1068-1073)
        ├─ write_back_gate.peek(&signature, now)              (mod.rs:548)
        └─ [Deliver] record_observation(episode)              (mod.rs:554 → wiring.rs:1076)
              └─ store_episode("{content} [sig:{signature}]", OVERSEER_SOURCE_LABEL, {sig})
```

### 1.1 What each token in the signature is (provenance, verified at HEAD)

| Token | Emitter (verified line) | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<8hex>` | `signal_to_problem` `GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; `<slug>-<8hex>` **is** the upstream goal_id |
| `workstream-gap` (constant) | `signal_to_problem` `WorkstreamGap` arm — `mod.rs:1371` | literal `"workstream-gap"` — one evidence-independent key per pass |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — `signal.rs:462-469`, admitted `mod.rs:1353-1359` | `sanitize_recalled(signature)` becomes a `dedup_key` → re-enters the next signature |

The `content` written is `observation_content` (`mod.rs:1079-1089`) with a trailing
`[sig:{signature}]` marker appended by the adapter (`wiring.rs:1084`) — this marker is the
**only** carrier of the signature on the read path (episodes have no typed signature field).

---

## 2. Dedup keying — three independent dedup layers, none of which closes the loop

| Layer | Where | Scope of dedup | Effect on the loop |
|---|---|---|---|
| **Within-signature key dedup** | `observation_signature` `sort_unstable()`+`dedup()` — `mod.rs:1070-1071` | Collapses identical `dedup_key`s **within one tick's** signature (e.g. two `workstream-gap` → one) | Does **not** collapse across ticks; and because each generation gains a *new* `overseer-obs:…` fragment, the composite key set is different every generation |
| **Write-back gate (WhisperGate)** | `write_back_gate = WhisperGate::new(900, 5)` — `mod.rs:299`; `peek`/`commit` — `mod.rs:548-556`, `guardrails.rs:312-339` | Suppresses **byte-identical** signature within a **900 s** window (and a 5/window cap) | Suppresses **exact** re-record only. Each self-ingestion generation produces a **distinct** signature, so the gate never fires against it → the loop's growth vector. Proven distinct-signature re-persist: `write_back_persists_again_for_a_distinct_signature` (`tests_memory_recall.rs:820`) |
| **Orient in-flight dedup / merge** | `orient` — `mod.rs:1200-1217` | Merges a `RecurringSignature` problem into a matching in-cycle problem by `dedup_key` | The recall-derived key `overseer-obs:…` rarely matches a fresh `goal:blocked:…` key, so it survives as its **own** High-priority problem and is written back |

**Load-bearing gap:** `observation_signature` (`mod.rs:1068-1073`) applies **no self-reference
filter** — a `dedup_key` that already begins with `overseer-obs:` is embedded verbatim. There is
also **no source filter** on recall: `recall_episodic` (`wiring.rs:1013-1031`) calls
`recall_episodes_ranked` without excluding `OVERSEER_SOURCE_LABEL`, so the Overseer recalls its
**own** prior write-backs. These two omissions together are the loop.

---

## 3. The self-ingestion loop (why the prefix nests)

```
        ┌──────────────────────────────────────────────────────────────┐
        │                                                              │
        ▼                                                              │
 write_back_observation → observation_signature                        │
   sig = "overseer-obs:" + [goal:blocked:… , workstream-gap , …]       │
        │ record_observation: store_episode("… [sig:overseer-obs:…]")   │
        ▼                                                              │
 store_episode (OVERSEER_SOURCE_LABEL)                                  │
        │                                                              │
        ▼  next tick: recall_pass → recall_episodic                    │
 recall_episodes_ranked  (NO source filter, wiring.rs:1013-1031)        │
        │  failure_signature = parse_failure_signature("[sig:…]")       │
        ▼                     (wiring.rs:976-986)                       │
 signals_from: count per failure_signature; ≥2 ⇒ RecurringSignature     │
        │                     (signal.rs:455-469)                       │
        ▼                                                              │
 orient/signal_to_problem: RecurringSignature ⇒                        │
   Problem { dedup_key = sanitize_recalled("overseer-obs:…"), High }    │
        │                     (mod.rs:1353-1359)                        │
        └──────────► this dedup_key re-enters observation_signature ────┘
                     as a key ⇒ "overseer-obs:…|overseer-obs:…" (nested)
```

This exactly reproduces the observed string: a leading fresh `overseer-obs:` prefix followed by
embedded `overseer-obs:goal:blocked:<slug>-<hash>` fragments that were **prior promoted
observation signatures**. A single-blocked-goal write-back (`overseer-obs:goal:blocked:fix-…-7f5afcca`)
recurs ≥2×, is promoted to a problem whose `dedup_key` carries the `overseer-obs:` prefix, and
that key is then folded into a later multi-problem signature. Each generation is a **new distinct
byte-string**, so neither the WhisperGate nor within-signature `dedup()` ever suppresses it — the
corpus of distinct overseer-obs signatures grows monotonically.

---

## 4. `==2` occurrence semantics — precisely what "seen 2×" means

- **Threshold:** `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`), a documented **floor**
  ("a single prior occurrence is not recurring; two or more is").
- **Counting:** `signals_from` builds a `BTreeMap<&str, u32>` counting **recalled episodes** per
  `failure_signature` (`signal.rs:456-461`), then emits `Signal::RecurringSignature { signature,
  occurrences }` iff `occurrences >= RECURRING_SIGNATURE_THRESHOLD` (`signal.rs:462-468`).
- **Display:** the problem summary is `"recurring signature seen {occurrences}× in cognitive
  memory ({signature})"` (`mod.rs:1360-1362`). This is the literal string in the investigation
  question — it is **rendered output**, not a stored memory key.
- **Empirical confirmation @ HEAD:**
  - `recurring_signature_emitted_when_two_episodes_share_signature` → `occurrences: 2` (`tests_memory_recall.rs:471-492`) — the `==2` floor fires.
  - `recurring_signature_not_emitted_for_single_occurrence` (`:494`) — 1× stays silent.
  - `recurring_signature_ignores_episodes_without_signature` (`:510`) — unmarked episodes don't count.

**Why exactly 2 and not more (the dead-zone axis):** the visible `×2` lane and the escalation
lane are **decoupled** (per `RECONCILIATION_LEDGER §3`, `SYNTHESIS §2.3`):

- **Lane A (visible `×2`)** — observation **episodes** via `record_observation`/`store_episode`
  (append-style on the episodic path); two window-separated (or post-restart) write-backs of the
  same still-true condition = 2 recalled episodes = threshold met. The WhisperGate's
  `last_delivered` map is in-memory/per-process (`guardrails.rs`), so a daemon restart re-arms a
  second identical write-back — the most probable source of *exactly* 2×.
- **Lane B (escalation)** — root-cause **occurrences** counted by `recall_occurrences(...).len()`
  and escalated only at `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`; escalation at
  `mod.rs:1613`).

`2` sits in the **dead zone**: above one-off noise (Lane A floor = 2) but below the escalation
bar (Lane B = 3). Combined with the two non-closing loops — blocked goals **parked without a WHY
classification** and `workstream-gap` being **notify-only** (`WorkstreamCoverage` is the sole
High-priority Decide arm with no launch/file edge) — the problem set never changes, so the same
signature is honestly re-observed at exactly `2`.

---

## 5. Reconciliation with prior artifacts

This deep-dive **validates, does not restart** the committed record. Every emission/keying/`==2`
citation in `SYNTHESIS.md`, `CONSOLIDATED_FINDINGS.md`, and `RECONCILIATION_LEDGER.md`
re-verifies exactly against live source at HEAD `25d4c5a6`. I add three sharpenings scoped to my
focus:

1. **INV-EMIT-SELFREF (new, actionable):** the loop's proximate cause is the **absence of a
   self-reference filter** in `observation_signature` (`mod.rs:1068-1073`) *and* the **absence of
   a source filter** in `recall_episodic` (`wiring.rs:1013-1031`). Either guard alone breaks the
   nesting; both are cheap and local.
2. **Dedup layering is real but orthogonal to the loop:** the three dedup layers (§2) all operate
   on **byte-identical** or **within-tick** scope; none can suppress a signature that is *distinct
   every generation by construction*. Tightening the WhisperGate window would **not** help.
3. **`==2` is honest, not a counter bug:** confirmed by passing tests (§4). Do **not** "fix" the
   threshold; fix the two non-closing loops so the problem set stops being static (Lane A stops
   re-observing) — the remediation rung, not the counter, is the lever.

**Do-not-redo trap (carried forward from `RECONCILIATION_LEDGER §2`):** do **not** replace the
Lane-B `store_fact` with a bare `store_fact_with_caller_key` — `DedupMode::CallerKey` keeps one
live fact per stable key, collapsing `recurrence` to 1 forever and making the `>=3` escalation rung
dead code. The counter must live **in fact content** (incremented `occurrence_count`).

---

## 6. Minimal, landing-safe remediation implied by this focus (not implemented here)

1. **Break the self-ingestion loop (D-EMIT):** in `observation_signature`, skip keys already
   prefixed `overseer-obs:` **or** in `recall_episodic`, exclude `OVERSEER_SOURCE_LABEL` episodes
   from the recurrence count. Pure/local; add a unit test asserting no `overseer-obs:` fragment
   nests into a fresh signature.
2. **Close the coverage loop (D-GAP):** give `WorkstreamCoverage` a real edge — file/route per
   `GapItem.signature` (**not** the bare `"workstream-gap"` key, per INV-GAP-KEY) so the gap
   leaves the terminal state.
3. **Close the blocked-goal loop (D-BLOCK):** attach a WHY classification + resolution rung so a
   parked blocked goal advances instead of being re-observed every window.

All three are additive and independently landable; #1 is the smallest and directly kills the
nesting that made the signature grow.

---

### Appendix — load-bearing citations (verified @ `25d4c5a6`)

| Fact | Location |
|---|---|
| `observation_signature` = sort→dedup→`overseer-obs:{join}` | `overseer/mod.rs:1068-1073` |
| `write_back_observation` gate + record | `overseer/mod.rs:534-563` |
| `write_back_gate = WhisperGate::new(900, 5)` | `overseer/mod.rs:299` |
| Only call site of write-back | `overseer/wiring.rs:301` |
| `RecurringSignature` count + `>=2` emit | `overseer/signal.rs:455-469` |
| `RECURRING_SIGNATURE_THRESHOLD = 2` | `overseer/signal.rs:362` |
| `RecurringSignature` → Problem(dedup_key = sanitized sig, High) + "seen N×" summary | `overseer/mod.rs:1353-1362` |
| `WorkstreamGap` → dedup_key `"workstream-gap"` | `overseer/mod.rs:1371` |
| `GoalBlocked` → dedup_key `goal:blocked:{goal_id}` | `overseer/mod.rs:1336` |
| `recall_episodic` (no source filter) | `overseer/wiring.rs:1013-1031` |
| `parse_failure_signature` (`[sig:…]`) | `overseer/wiring.rs:976-986` |
| `record_observation` embeds `[sig:…]`, fixed source label | `overseer/wiring.rs:1076-1091` |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `overseer/root_cause.rs:33` |
| Tests: `==2` fires / 1× silent / distinct-sig re-persist / within-window dedup | `overseer/tests_memory_recall.rs:471,494,820,797` |
