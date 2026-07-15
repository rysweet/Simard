# Primary Deep Dive — Signature Emitter Construction & "2×" Dedup/Count Semantics

**Investigation question:** the recurring composite signature
`overseer-obs:goal:blocked:…|…|workstream-gap|resource:engineer_spawn` seen **2×**
in cognitive memory.

**Focus:** who *constructs* each token class of the signature (observer/signal/
sensor/notify/mod) and what the "2×" count actually means — with file:line evidence.

**Repo HEAD:** `b9f99879`  •  Codebase: `src/overseer/`

---

## 1. Verdict (one paragraph)

The string in the question is **one `observation_signature` write-back key** —
`format!("overseer-obs:{}", keys.join("|"))` at `src/overseer/mod.rs:1072` — where
`keys` are the **sorted + deduped `Problem.dedup_key`s** of everything observed on a
tick. Each `|`-token is a per-signal `dedup_key` minted in the `signal→problem`
mapping `classify_signal` (`src/overseer/mod.rs:1237-1373`), e.g. `goal:blocked:{goal_id}` (`:1336`),
`workstream-gap` (`:1371`), `resource:engineer_spawn` (`:1270`). The **"2×" is NOT a
WhisperGate count** — it is `Signal::RecurringSignature.occurrences`, the number of
**recalled cognitive-memory episodes that share the same `failure_signature`**,
counted at `src/overseer/signal.rs:455-470`. It reaches "2" because the write-back
dedup gate is **in-memory and process-local** (`src/overseer/guardrails.rs:291-333`)
with a 900 s window, so the identical multi-problem observation gets persisted again
after a daemon restart or >900 s later — and recall then reports it as recurring.

---

## 2. Token-class emitters (definitive names + line ranges)

| Token in the signature | Constructing expression | File:line | Emitter fn |
|---|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `format!("overseer-obs:{}", keys.join("\|"))` | `mod.rs:1068-1073` | `observation_signature(problems)` |
| `goal:blocked:{goal_id}` | `format!("goal:blocked:{goal_id}")` | `mod.rs:1336` | signal→problem map (`GoalBlocked` arm) |
| `workstream-gap` | `"workstream-gap".to_string()` | `mod.rs:1371` | signal→problem map (`WorkstreamGap` arm) |
| `resource:engineer_spawn` | `"resource:engineer_spawn".to_string()` | `mod.rs:1270` | signal→problem map (`EngineerSpawnRate` arm) |
| `resource:budget` / `resource:memory_growth` | literals | `mod.rs:1264` / `:1276` | same map |
| `goal:stale:` / `loop:` / `drift:` / `anomaly:` / `quality:ci:` / `delivery:pr:` | `format!(...)` | `mod.rs:1300/1315/1321/1306/1288/1294` | same map |
| Recall-driven dedup_key (the *whole* `overseer-obs:…` string re-entering as a problem) | `sanitize_recalled(signature)` | `mod.rs:1359` | signal→problem map (`RecurringSignature` arm) |
| gap sub-signature `goal:{id}` (feeds `workstream-gap:{sig}` gate key) | `format!("goal:{}", g.id)` | `sensor.rs:306` | gap scanner |
| gap gate key `workstream-gap:{sig}` | `format!("workstream-gap:{}", g.signature)` | `mod.rs:901, 932` | `act_flag_workstream_gaps` |

**Note on the observer/notify modules:** they do **not** mint the composite string.
`observer.rs` only *labels* signal variants for telemetry
(`"RecurringSignature"` at `observer.rs:216`; sensor id `"overseer-observer"` at
`observer.rs:130`). `notify.rs` consumes the already-classified `workstream-gap`
kind for operator subjects (`notify.rs:98, 204`) — it is a **sink**, not an emitter,
of the signature.

---

## 3. Data-flow map: signal source → persisted signature string

```
Observe (sensor.rs)                      per-blocked-goal, gap-scan, resource telemetry
        │                                → ObservedState { blocked_goals, workstream_gaps, live_engineers, recall }
        ▼
signals_from(state)   signal.rs:366      Vec<Signal> (GoalBlocked, WorkstreamGap, EngineerSpawnRate, …)
        │                                + recall fold → Signal::RecurringSignature  (signal.rs:455-470)
        ▼
classify_signal(s)    mod.rs:1237-1373   each Signal → (kind, priority, dedup_key, summary)
        │                                dedup_key: "goal:blocked:X", "workstream-gap", "resource:engineer_spawn", …
        ▼
orient → Vec<Problem>                    ranked, merged by dedup_key
        │
        ▼
observation_signature(problems)  mod.rs:1068-1073
        │   keys = problems.map(dedup_key); keys.sort_unstable(); keys.dedup();
        │   → "overseer-obs:" + keys.join("|")            ◀── THE STRING IN THE QUESTION
        ▼
write_back_observation()  mod.rs:534-563
        │   write_back_gate.peek(sig) : WhisperDecision   (in-mem, 900 s window, cap 5/h)
        │   Deliver → record_observation(episode); commit(sig)
        ▼
record_observation()  wiring.rs:1076-1091
        │   content = "{content} [sig:{signature}]" ; store_episode(...)  ◀── persisted to graph
        ▼
        … later tick / after restart …
        ▼
recall_episodic()  wiring.rs:1013-1030
        │   failure_signature = parse_failure_signature(content)  (wiring.rs:976-986; reads "[sig:…]")
        ▼
signals_from → recall fold  signal.rs:455-470
        │   BTreeMap<sig,count>; count episodes; if count ≥ RECURRING_SIGNATURE_THRESHOLD(=2)
        │   → Signal::RecurringSignature { signature, occurrences: count }   ◀── occurrences == "2×"
        ▼
mod.rs:1353-1362                         summary text (verbatim from the question):
        "recurring signature seen {occurrences}× in cognitive memory ({signature})"
```

This is a **closed feedback loop**: the Overseer's own write-back
(`overseer-obs:…`) is later recalled and counted as a "recurring signature" —
the composite key becomes its *own* future evidence.

---

## 4. What "2×" means (dedup/count semantics — precise)

1. **It is a recall episode count, not a gate counter.** `occurrences` is computed
   at `signal.rs:456-461` by tallying `RecalledEpisode.failure_signature` values in a
   `BTreeMap<&str,u32>`; the signal fires only when a count `>= RECURRING_SIGNATURE_THRESHOLD`
   (`= 2`, `signal.rs:362`). So "2×" = **exactly two persisted episodes carried the
   identical `[sig:overseer-obs:…]` marker**.

2. **Why duplicates can exist despite a dedup gate.** The write-back de-dup uses
   `WhisperGate` (`guardrails.rs:291-333`), whose state is a plain in-process
   `HashMap<String,i64> last_delivered` (`:294`) with `window_secs = 900`
   (constructed `mod.rs:299` `WhisperGate::new(900, 5)`). `peek` only suppresses
   when `now - last < window_secs` (`:313-317`). Therefore an **identical
   observation is persisted a second time** whenever:
   - the daemon/Overseer **restarted** between the two ticks (HashMap reset → slot lost), or
   - the same persistent condition is re-observed **> 900 s later**, or
   - a **different Overseer instance** wrote it (no shared gate state).

3. **Within a single signature there are no duplicate tokens.** `observation_signature`
   calls `sort_unstable()` then `dedup()` (`mod.rs:1070-1071`), so the `|`-list inside
   one `overseer-obs:` block is unique+sorted. The repeated `overseer-obs:…` blocks
   visible in the question string are therefore **separate recalled episodes
   concatenated by the recall query**, not intra-signature duplication — consistent
   with `occurrences = 2` (two whole episodes).

4. **The recurrence dedup is intentional and downstream-guarded.** The recalled
   `RecurringSignature` re-enters as a Problem whose `dedup_key` is the sanitized
   signature itself (`mod.rs:1359`), so it **merges** into the matching in-cycle
   problem instead of spawning a duplicate. Root-cause escalation uses a *separate*
   recurrence counter keyed on `dedup_key::cause_label` (`root_cause.rs:53-55, 78-82`)
   with `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) — i.e. "2×" is
   below the escalate-the-root-cause floor; it raises priority but does not yet
   trip the systemic-defect escalation.

---

## 5. Load-bearing lines (quick citations)

- Composite emitter: `src/overseer/mod.rs:1068-1073`
- Write-back + gate peek/commit: `src/overseer/mod.rs:534-563`
- Token dedup_keys: `src/overseer/mod.rs:1264, 1270, 1300, 1336, 1359, 1371`
- Recurring-signature summary text: `src/overseer/mod.rs:1353-1362`
- Count/threshold logic: `src/overseer/signal.rs:455-470`, threshold `:362`
- In-memory gate: `src/overseer/guardrails.rs:291-333` (HashMap `:294`, window check `:313-317`)
- Persist marker: `src/overseer/wiring.rs:1084` (`[sig:…]`), parse-back `:976-986`, recall `:1013-1030`
- Root-cause recurrence (separate counter): `src/overseer/root_cause.rs:33, 53-55, 78-82`
- Test pinning 2× semantics: `src/overseer/tests_memory_recall.rs:485-489, 585-587`
