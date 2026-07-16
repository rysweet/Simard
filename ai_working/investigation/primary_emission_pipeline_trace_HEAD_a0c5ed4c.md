# Primary Deep Dive — Signature Emission Pipeline (observer/sensor/notify/signal/mod → memory)

**Role:** PRIMARY investigator.
**Investigation question:** the recurring composite signature seen `2×` in cognitive memory —
`overseer-obs:goal:blocked:…|…|workstream-gap|…` (with nested `overseer-obs:` fragments and
`workstream-gap` tokens repeating).
**HEAD:** `a0c5ed4c`.
**Source drift:** `git diff --name-only 6e3113bc..HEAD -- '*.rs'` → **only** `src/overseer/tests_root_cause.rs`
(a test). Every load-bearing `src/overseer/*` and `src/ooda_loop/*` line citation below was
**independently re-read at HEAD `a0c5ed4c`** and holds exactly. This extends — does not restart —
the prior waves (FINAL_SYNTHESIS, RECONCILIATION_LEDGER).

---

## 0. Verdict (one line)

The string is **not a raw memory key and not a storage/dedup/replay/collision bug**. It is the
Overseer's own **observation write-back signature** (`observation_signature`,
`mod.rs:1068-1073`) — the cycle's problem `dedup_key`s, sorted → deduped → `|`-joined → prefixed
`overseer-obs:`. `"seen 2×"` is an **honest cross-window re-observation** of a near-static,
unresolved problem set, emitted verbatim by `Signal::RecurringSignature` at `mod.rs:1361`. It
recurs because the flag-only loops that would clear the problem set never close.

---

## 1. End-to-end pipeline trace (with re-verified citations @ `a0c5ed4c`)

The signature is assembled and stored **once per surviving tick**, then recalled next tick. Full
chain:

```
run_cycle (mod.rs:407+)
  ├─ Observe: signals_from(state)              signal.rs:366
  │    └─ recall pass populates state.recall    mod.rs:423-438, recall_pass mod.rs:498
  │    └─ RecurringSignature emitted when ≥2 recalled episodes share a
  │       failure_signature                     signal.rs:455-469  (threshold=2, signal.rs:362)
  ├─ Orient: orient(signals) → Problems         mod.rs:1200
  │    └─ each Signal → (kind,priority,dedup_key,summary) via classify_signal
  │         • GoalBlocked      → "goal:blocked:{goal_id}"    mod.rs:1336
  │         • WorkstreamGap    → "workstream-gap" (literal)  mod.rs:1371
  │         • EngineerSpawn    → "resource:engineer_spawn"   mod.rs:1267-1272
  │         • RecurringSignature→ sanitize_recalled(signature) mod.rs:1353-1363  ← self-fed
  ├─ Decide/gate/act …                          mod.rs:461-480
  └─ write_back_observation(cycle.problems)      wiring.rs:301 → mod.rs:534
       ├─ observation_signature(problems)         mod.rs:1068-1073
       │     keys.sort_unstable(); keys.dedup();
       │     format!("overseer-obs:{}", keys.join("|"))
       ├─ write_back_gate.peek/commit             gate = WhisperGate::new(900,5), mod.rs:299
       └─ record_observation(episode)             wiring.rs:1076-1091
             store_episode("{content} [sig:{signature}]", "overseer", {signature})

           … next tick …
recall_episodic (wiring.rs:1013-1030)
  └─ failure_signature = parse_failure_signature(content)   wiring.rs:976-986
        pulls the "[sig:…]" marker back out  → feeds signal.rs:457-460 counting → RecurringSignature
```

### 1.1 Token provenance (each fragment of the observed string)

| Token in the signature | Emitter | Exact construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1068-1073` | `format!("overseer-obs:{}", keys.join("\|"))` after `sort_unstable`+`dedup` |
| `goal:blocked:<slug>-<8hex>` | `classify_signal`, `GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; the `<slug>-<hash>` **is** the goal_id |
| `workstream-gap` (constant) | `classify_signal`, `WorkstreamGap` arm — `mod.rs:1371` | literal `"workstream-gap"`; `gaps.len()` goes to the **summary only** |
| `resource:engineer_spawn` (constant) | `classify_signal`, `EngineerSpawnRate` arm — `mod.rs:1267-1272` | literal `"resource:engineer_spawn"`; `{live}` → summary only |
| nested `overseer-obs:…` fragments | recall-derived `RecurringSignature` — `signal.rs:455-469`, admitted `mod.rs:1353-1363` | `sanitize_recalled(signature)` becomes the RecurringSignature problem's `dedup_key`, then is re-joined into the **next** `observation_signature` (self-observation) |

The message wording in the question matches `mod.rs:1361` verbatim:
`"recurring signature seen {occurrences}× in cognitive memory ({signature})"`.

---

## 2. Why it recurs `2×` — the counter is honest, not buggy

- The composite episode is written **at most once per 900 s window**: `write_back_gate =
  WhisperGate::new(900, 5)` (`mod.rs:299`), a peek→store→commit gate consumed **only after a
  successful store** (`mod.rs:548-557`). Within-window dedup is a real property of the gate
  (`guardrails.rs:313-329`).
- A second identical episode therefore appears only when the gate did **not** suppress it:
  1. **> 900 s later** — the same unresolved set is legitimately re-recorded in a new window; or
  2. **after a daemon restart** — the gate's `last_delivered` map is in-process
     (`guardrails.rs:294`, `HashMap`), so it starts empty and the still-true condition re-records.
     This is the most probable source of *exactly* 2×.
- `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`); `Signal::RecurringSignature` fires at
  `occurrences >= 2` (`signal.rs:463`). Two honest cross-window re-observations ⇒ the `2×` signal.

**This is a real re-observation, not a replay/dup artifact.** The dedup gate, the store, and the
recall marker all behave as designed; what they faithfully report is that the **underlying problem
set never went away**.

---

## 3. Why the problem set never clears (the actual defect surface)

The signature persists because the problems that compose it are handled by **flag-only** loops with
no closing rung:

1. **`workstream-gap` is notify-only.** `ProblemKind::WorkstreamCoverage` — the *only* High-priority
   Decide arm — maps to `Intervention::FlagWorkstreamGaps` (`mod.rs:1534-1543`), which notifies the
   operator (`act_flag_workstream_gaps`, `mod.rs:884-948`) but never files an issue or launches a
   fix recipe. So `workstream-gap` re-appears in every observation signature until a human acts.
2. **`goal:blocked:*` self-feeds through recall.** A blocked goal's `dedup_key` is written back,
   recalled as a `failure_signature`, and (when ≥2 share it) re-admitted as a `RecurringSignature`
   whose own `dedup_key` is the recalled composite — folded back into the next signature
   (`signal.rs:455-469` → `mod.rs:1353-1363` → `mod.rs:1069`). This is the source of the **nested
   `overseer-obs:…` fragments** in the observed string.

**Delta vs. earlier waves (extends, not contradicts):** the FINAL_SYNTHESIS "resolution ladder is
double-gated off" framing is now **partly superseded** for the goal-blocked lane. At HEAD, the
no-progress breaker's root-cause investigation is **ON by default**
(`no_progress_investigation_enabled()`, `no_progress.rs:200-203` — `SIMARD_NO_PROGRESS_INVESTIGATE`
defaults on) and an **already-blocked re-investigation path** exists (issue #17,
`reinvestigate_bare_blocked_goals`, wired in `ooda_loop/cycle.rs`). That narrows the goal-blocked
loop when goals are parked with a bare `[OODA-SAFEGUARD]` block. The **`workstream-gap` notify-only
quarantine (item 1) is unchanged and remains the load-bearing non-closing loop** at HEAD `a0c5ed4c`.

---

## 4. What is NOT the cause (ruled out at HEAD)

- **Storage non-idempotency of the observation write-back:** episodes go through `store_episode`
  behind the 900 s gate (`wiring.rs:1076-1091`), not the append-only `store_fact` ratchet. The
  ratchet concern (`recall_occurrences`/root-cause lane) is a **separate** counter lane and does
  not manufacture the visible `overseer-obs:` string.
- **Key collision / keyword bleed:** recall filtering is by the parsed `[sig:…]` marker
  (`wiring.rs:976-986`); the `overseer-obs:` prefix is unique to the write-back path.
- **`resource:engineer_spawn` growth:** a fixed literal key (`mod.rs:1270`); its `{live}` count
  lives only in the summary — benign membership drift, corroborating not contradicting.

---

## 5. Minimal remediation direction (advisory — matches prior reconciled ledger)

The non-closing `workstream-gap` loop is the single highest-leverage fix: give
`ProblemKind::WorkstreamCoverage` a closing rung (file one issue **per `GapItem.signature`**, not
the bare `"workstream-gap"` dedup_key — see INV-GAP-KEY, `mod.rs:1371`) so coverage gaps leave the
observation set once tracked. The recall self-feed (nested `overseer-obs:` fragments) is cosmetic
noise on top of that persistence and should be addressed by **excluding recall-derived
`RecurringSignature` problems from the write-back set** (`write_back_observation`, `mod.rs:534`) so
the signature cannot ingest its own prior form. Do **not** apply the refuted `store_fact_with_caller_key`
remedy on the root-cause lane (RECONCILIATION_LEDGER §2 — it makes escalation dead code).

---

## 6. Re-verified citation ledger (@ `a0c5ed4c`)

| Claim | Loc | Status |
|---|---|---|
| `observation_signature` = sort→dedup→`overseer-obs:{join("\|")}` | `mod.rs:1068-1073` | ✅ exact |
| single write-back call site | `wiring.rs:301` → `mod.rs:534` | ✅ exact |
| write-back gate `WhisperGate::new(900,5)`, commit-after-store | `mod.rs:299`, `mod.rs:548-557` | ✅ exact |
| gate `last_delivered` in-process HashMap | `guardrails.rs:291-329` | ✅ exact |
| `record_observation` embeds `[sig:…]` via `store_episode` | `wiring.rs:1076-1091` | ✅ exact |
| recall parses `[sig:…]` back to `failure_signature` | `wiring.rs:976-986`, `1013-1030` | ✅ exact |
| RecurringSignature emitted at `occurrences>=2`; threshold=2 | `signal.rs:455-469`, `362` | ✅ exact |
| RecurringSignature admitted, `dedup_key=sanitize_recalled(sig)`, msg text | `mod.rs:1353-1363` | ✅ exact |
| `goal:blocked:{goal_id}` construction | `mod.rs:1336` | ✅ exact |
| `workstream-gap` literal key; count→summary | `mod.rs:1371` | ✅ exact |
| WorkstreamCoverage Decide = notify-only `FlagWorkstreamGaps` | `mod.rs:1534-1543`, `884-948` | ✅ exact |
| no-progress investigation ON by default (delta vs earlier waves) | `no_progress.rs:200-203` | ✅ exact |
