# PRIMARY — Signature assembly + emission path and the dedup/idempotency gate

**HEAD:** `25d4c5a6`
**Focus:** Name the *exact* code that assembles the `overseer-obs:goal:blocked:*`
observation signature, the path that emits/persists it, and the
dedup/idempotency gate that is supposed to stop it recurring — and explain why
the signature in the investigation prompt recurs and compounds.

---

## 0. Verdict (one line)

The recurring, compounding `overseer-obs:…|…` signature is produced by a
**self-ingestion loop**: the Overseer's own write-back signature is stored on
the *same* `failure_signature` channel that its recall reads, is re-classified
verbatim into a `Problem.dedup_key`, and is then folded back into the *next*
`observation_signature`. The dedup/idempotency gate that exists
(`write_back_gate`, a `WhisperGate`) only collapses **exact repeats within a
time window** — it cannot break the loop because each generation's signature is
a **new, longer, nested string**, so it is never "exact" and never deduped.

---

## 1. Signature assembly — the exact function

`fn observation_signature(problems: &[Problem]) -> String`
— **`src/overseer/mod.rs:1068‑1073`**

```rust
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    format!("overseer-obs:{}", keys.join("|"))
}
```

- The `overseer-obs:` prefix is hard-coded here (`mod.rs:1072`).
- The body is the sorted, adjacent-deduped list of every `Problem.dedup_key`
  in the cycle, joined with `|`.
- The member keys `goal:blocked:<goal_id>` and `workstream-gap` come from
  `classify_signal`:
  - `goal:blocked:{goal_id}` — **`src/overseer/mod.rs:1336`** (from
    `Signal::GoalBlocked`).
  - `workstream-gap` — **`src/overseer/mod.rs:1371`** (from
    `Signal::WorkstreamGap`).
- `keys.dedup()` (`mod.rs:1071`) only removes **adjacent** equal entries after
  the sort, i.e. it collapses *identical* full keys only. It does **not**
  collapse a key that is a *substring / nested prefix* of another key. This is
  the load-bearing weakness (see §5).

The human body that travels with the signature:
`fn observation_content` — **`src/overseer/mod.rs:1079‑1089`** (sanitized).

---

## 2. Emission / persistence path — the exact path

Assembly → gate → store, all inside
`OverseerCore::write_back_observation` — **`src/overseer/mod.rs:534‑563`**:

```
run cycle → problems  (mod.rs run_cycle, ~447)
   └─ write_back_observation(problems)                 mod.rs:534
        ├─ signature = observation_signature(problems) mod.rs:546   ← ASSEMBLY
        ├─ write_back_gate.peek(&signature, now)        mod.rs:548   ← GATE (peek)
        │     WhisperDecision::Deliver ⇒
        │        episode = ObservationEpisode {content, signature}  mod.rs:550‑553
        │        caps.memory.record_observation(&episode)?          mod.rs:554  ← PERSIST
        │        write_back_gate.commit(&signature, now)            mod.rs:556  ← GATE (commit)
        └─ else ⇒ Ok(None)  (deduped within window)     mod.rs:561
```

Physical persistence + serialization boundary:
`MemoryRecallOps::record_observation` — **`src/overseer/wiring.rs:1076‑1091`**:

```rust
let content = format!("{} [sig:{}]", episode.content, episode.signature); // wiring.rs:1084
let metadata = serde_json::json!({ "signature": episode.signature });     // wiring.rs:1085
self.mem.store_episode(&content, OVERSEER_SOURCE_LABEL, Some(&metadata))   // wiring.rs:1088
```

**The signature is embedded into the episode text as a `[sig:…]` marker
(`wiring.rs:1084`).** This is the exact seam that feeds the loop in §4.

---

## 3. The dedup / idempotency gate — the exact gate

**`write_back_gate: WhisperGate`** — field declared at
**`src/overseer/mod.rs:192`**, constructed at **`src/overseer/mod.rs:299`**:

```rust
write_back_gate: WhisperGate::new(900, 5),   // 15-min window, cap 5 / hour
```

Gate implementation: `WhisperGate::{peek, commit, admit}` —
**`src/overseer/guardrails.rs:312‑340`**. Semantics:

- `peek(sig, now)` returns `Deliver` unless `sig` was committed within the
  900 s window (⇒ `SuppressDuplicate`) or the per-hour cap of 5 is reached
  (⇒ `SuppressCapReached`).
- Keyed on the **exact signature string**.

There is a *second*, structurally identical gate on the gap path —
**`gap_gate`** used in `act_flag_workstream_gaps`
(**`src/overseer/mod.rs:901‑933`**), keyed on `format!("workstream-gap:{}", …)`.

**Idempotency claim vs. reality:** the doc-comment at `mod.rs:1064‑1067` states
"Two identical observations produce the same signature (so the write-back gate
de-dups them)." That is true *only for byte-identical problem sets*. It is the
**exact-match** assumption that the loop in §4 violates.

---

## 4. Why it recurs — the self-ingestion loop (root cause)

The prompt's signature is the `summary` string minted at
`classify_signal` — **`src/overseer/mod.rs:1353‑1363`**:

```rust
Signal::RecurringSignature { signature, occurrences } => (
    ProblemKind::ProcessHealth,
    Priority::High,
    sanitize_recalled(signature),                                  // ← dedup_key = the recalled signature, VERBATIM  (mod.rs:1359)
    sanitize_recalled(&format!(
        "recurring signature seen {occurrences}× in cognitive memory ({signature})")), // ← the prompt text  (mod.rs:1361)
),
```

Closed loop (each numbered step is a named symbol):

1. **Write** — `observation_signature` mints `S = overseer-obs:<keys>` and
   `write_back_observation` stores it as `… [sig:S]`
   (`mod.rs:546` → `wiring.rs:1084`).
2. **Read-back** — `MemoryRecallOps::recall_episodic` parses that marker back
   out: `failure_signature: parse_failure_signature(&e.content)`
   (**`wiring.rs:1025`**, parser at **`wiring.rs:976‑986`**). The stored
   `overseer-obs:S` now reappears as `RecalledEpisode.failure_signature`.
3. **Recurrence detector** — `signals_from` counts episodes per
   `failure_signature`; when `occurrences >= RECURRING_SIGNATURE_THRESHOLD`
   (**`= 2`**, `src/overseer/signal.rs:362`) it emits
   `Signal::RecurringSignature { signature: S, occurrences }`
   (**`src/overseer/signal.rs:455‑469`**). This is the literal **"seen 2×"** in
   the prompt.
4. **Re-ingestion** — `classify_signal` turns that signal into a `Problem`
   whose `dedup_key` **is `S` itself** (`mod.rs:1359`), and `orient`
   (`mod.rs:1200‑1235`) admits it as a first-class problem.
5. **Re-composition** — next cycle, `observation_signature` folds `S` back in
   as a member key alongside the fresh `goal:blocked:*` / `workstream-gap`
   keys, producing `S' = overseer-obs:<keys>|S` — a **strictly longer, nested**
   signature. Back to step 1 with `S'`.

Because `S' ≠ S`, the `write_back_gate` (§3) sees a *new* key every generation
and always returns `Deliver`; the idempotency gate is structurally bypassed.
Across N generations the `overseer-obs:goal:blocked:…issue-17…` substring
accumulates once per generation — which is exactly the 11× repeated,
pipe-joined, nested string in the investigation prompt.

---

## 5. The precise defect (where the loop should have been cut)

Three independent missing guards, any one of which would break the loop:

| # | Location | Missing guard |
|---|----------|---------------|
| D1 | `observation_signature` — `mod.rs:1068` | Does **not** exclude keys already prefixed `overseer-obs:` (its own output) from the member set. A one-line filter `keys.retain(|k| !k.starts_with("overseer-obs:"))` before `join` closes it. |
| D2 | `classify_signal` `RecurringSignature` arm — `mod.rs:1353‑1363` | Re-ingests a recalled signature as a `Problem.dedup_key` **verbatim**, with no check that the signature is the Overseer's *own* write-back (`overseer-obs:` self-signature). A self-signature should raise at most an advisory, never a new dedup key that re-enters composition. |
| D3 | `write_back_gate` idempotency — `mod.rs:299` / `guardrails.rs:312` | The gate keys on the **whole** signature string (exact match). It cannot recognise that `S'` is `S` plus noise. Dedup should key on the *stable core* (e.g. the non-`overseer-obs` member set), not the compounding full string. |

D1 is the minimal, lowest-risk landing: it makes `observation_signature`
**idempotent under self-recall** (the write-back signature stops containing
prior write-back signatures), which is the property the `mod.rs:1064‑1067`
doc-comment already *claims* but does not deliver.

---

## 6. Evidence anchors (exact symbols)

- Assembly: `observation_signature` — `src/overseer/mod.rs:1068‑1073`
- Emission/gate: `write_back_observation` — `src/overseer/mod.rs:534‑563`
- Persist + `[sig:]` marker: `record_observation` — `src/overseer/wiring.rs:1076‑1091`
- Read-back parser: `parse_failure_signature` — `src/overseer/wiring.rs:976‑986`; used at `wiring.rs:1025`
- Gate primitive: `WhisperGate::{peek,commit}` — `src/overseer/guardrails.rs:312‑340`; instances `write_back_gate` (`mod.rs:192,299`), `gap_gate` (`mod.rs:901‑933`)
- Recurrence threshold: `RECURRING_SIGNATURE_THRESHOLD = 2` — `src/overseer/signal.rs:362`; detector `signals_from` — `src/overseer/signal.rs:455‑469`
- Re-ingestion: `classify_signal` `RecurringSignature` arm — `src/overseer/mod.rs:1353‑1363`; member keys `goal:blocked:{goal_id}` (`mod.rs:1336`), `workstream-gap` (`mod.rs:1371`)
- Merge/priority: `orient` — `src/overseer/mod.rs:1200‑1235`
