# Primary deep dive — signature provenance + dedup keying: real event vs. write-back artifact

**Investigation question:** the overseer signature seen 2× in cognitive memory,
`overseer-obs:goal:blocked:…|…|workstream-gap|workstream-gap`.
**Focus:** signature provenance + dedup keying across `observer.rs`, `signal.rs`,
`notify.rs`, `stewardship/dedup.rs`, `stewardship/routing.rs`.
**Method:** independent line-by-line source trace at working tree (HEAD
`85b9398a`). Every claim is grounded to a current source line.

---

## Verdict (high confidence)

**The `×2` is a REAL re-observation loop, NOT a dedup / storage / replay
artifact — but the *thing* being counted is the overseer's own write-back
bookkeeping, so the "recurring signature" carries no independent failure
signal. There is a genuine (bounded) self-referential write-back feedback
defect.**

Two things are true at once and must not be conflated:

1. **The count is honest.** Two distinct episode nodes really do exist in the
   graph, each carrying the identical composite `[sig:overseer-obs:…]`. The
   within-window write-back gate provably suppresses same-window duplicates, so
   the two nodes are two *legitimate* write-back passes ≥15 min apart. The `×2`
   is not double-read, not replay, not a hash collision, not a `dedup()` bug.

2. **The event it certifies is vacuous.** The composite signature is an
   *aggregate join of every open problem's `dedup_key`*, not a failure
   fingerprint. Its recurrence means only "the same static problem set was
   observed twice," and — via the nested `overseer-obs:` fragments — partly
   "the overseer re-observed its own prior bookkeeping." It is a faithful
   fingerprint of a stuck system, not evidence of a new failure.

This independently confirms the consolidated finding
([`CONSOLIDATED_FINDINGS.md`](./CONSOLIDATED_FINDINGS.md) §0/§0a).

---

## Provenance chain (code-evidenced)

### 1. Construction — where the composite string is built

`observation_signature` (`src/overseer/mod.rs:1068-1073`):

```rust
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    format!("overseer-obs:{}", keys.join("|"))
}
```

- The `goal:blocked:<slug>-<hash>` tokens are per-problem `dedup_key`s minted at
  `classify_signal` (`mod.rs:1336`: `format!("goal:blocked:{goal_id}")`).
- The `workstream-gap` tokens are the coverage problem's fixed `dedup_key`
  (`mod.rs:1371`: `"workstream-gap".to_string()`).
- The join with `|` and the `overseer-obs:` prefix is the *only* place these
  become one string. **This is intentional aggregation, not an accidental join**
  — the doc-comment (`mod.rs:1064-1067`) states it is the write-back dedup key.

`dedup()` only collapses *adjacent* equal keys within one signature; the
repeated bare `workstream-gap|workstream-gap` fragments in the recall are
*separate episodes* concatenated in the recall stream, not a de-dup failure.

### 2. Write-back — where the signature enters memory

`write_back_observation` (`mod.rs:534-563`) keys a `WhisperGate`
(900 s window, cap 5 — `mod.rs:299`) on that composite signature (`mod.rs:546-556`).
The adapter `record_observation` (`wiring.rs:1076-1091`) embeds it as a text
marker: `format!("{} [sig:{}]", episode.content, episode.signature)`
(`wiring.rs:1084`). **`record_observation` never de-dups by content — it always
stores a new node.** The *only* dedup is the 900 s time-window gate. So a second
write-back of the identical signature after the window elapses creates a
*second distinct node* with the same `[sig:…]`.

### 3. Recall + recurrence count — where `×2` is produced

- `recall_episodic` parses the marker back out: `parse_failure_signature`
  (`wiring.rs:976-986`) → `RecalledEpisode.failure_signature` (`wiring.rs:1025`;
  the load-bearing field, `capabilities.rs:611-614`).
- `signals_from` tallies episodes by `failure_signature` in a `BTreeMap` and
  raises `Signal::RecurringSignature` at `≥ RECURRING_SIGNATURE_THRESHOLD = 2`
  (`signal.rs:455-469`, threshold `signal.rs:362`).
- `classify_signal` renders it: `"recurring signature seen {occurrences}× in
  cognitive memory ({signature})"` (`mod.rs:1360-1362`) — the exact string in
  the question.

Two identical `[sig:…]` nodes ⇒ `occurrences = 2` ⇒ the observed message.

### 4. Self-referential feedback (the real defect)

- The `RecurringSignature` problem's `dedup_key` is `sanitize_recalled(signature)`
  — i.e. the *prior* `overseer-obs:…` string (`mod.rs:1359`).
- That key differs from every base `goal:blocked:*` / `workstream-gap` key, so
  Orient's same-key merge (`mod.rs:1210-1221`) does **not** fold it; it is
  `push`ed as its own problem (`mod.rs:1222`).
- `write_back_observation(&cycle.problems)` (`wiring.rs:301`) then writes back
  *all* problems, so the **next** `observation_signature` embeds the prior
  `overseer-obs:…` token → the nested `overseer-obs:` fragments in the question.
- The recall query itself carries every problem `dedup_key`
  (`RecallKeys::from_signals` `capabilities.rs:533`; `query()` `capabilities.rs:547-551`),
  so the composite term is exactly what fishes the overseer's own episodes back
  out. **The overseer recalls and re-observes its own bookkeeping.**

`sanitize_recalled` at this admission boundary (`mod.rs:1359`) proves the authors
already treat recalled signatures as untrusted — yet still feed them back into
future signatures. The loop is **bounded** (900 s gate + recall limit + same-key
merge + the `×2` floor), which is why it stabilizes at "seen ~2×" rather than
exploding, but it is a real design smell.

---

## Ruled-out artifact hypotheses

| Hypothesis | Ruled out by |
|---|---|
| Double-read / replay of one node | Two nodes: `record_observation` always stores (`wiring.rs:1086-1090`); gate only blocks *within* 900 s (`mod.rs:548-556`). |
| `dedup()` collapse bug | `dedup()` is adjacent-only (`mod.rs:1071`); repeated `workstream-gap` are distinct episodes, not one mis-deduped key. |
| Hash collision / unstable key | Signature is a plain deterministic string join, sorted + deduped (`mod.rs:1069-1072`); no hashing on this path. |
| `stewardship::failure_signature` mis-keying | **Different path.** `dedup.rs:63-75` (`sha256(kind‖msg)[..8]`) keys the *GitHub-issue* dedup (`observer.rs:77`, `find_existing` `dedup.rs:78-81`). The `overseer-obs:` composite never flows through it. |
| Routing duplication | `routing.rs:39-45` maps source→repo only; it never touches the recall signature. |
| `notify.rs` duplication | `notify.rs:98`/`204` render the `workstream-gap` *notification kind*; not a signature source. |

---

## The single largest unblock lever

The recurrence is a *symptom*: a faithful fingerprint of a **static problem
set**. The fix is not to the signature/dedup path but to **close the two
observe-and-flag loops** that keep the set static (blocked goals parked but never
resolved via the `NoProgressClass` rungs; workstream gaps flagged but never
routed to work) — see [`CONSOLIDATED_FINDINGS.md`](./CONSOLIDATED_FINDINGS.md)
§1–§3. The one contained signature-path fix worth landing independently:
**stop writing recall-derived `RecurringSignature` keys back into
`observation_signature`** (exclude `ProcessHealth`-from-recall problems from the
write-back set, or carry the recurrence count in episode *content/metadata*
rather than folding it into the next signature) to cut the self-referential
feedback at `mod.rs:1359` + `wiring.rs:301`.
