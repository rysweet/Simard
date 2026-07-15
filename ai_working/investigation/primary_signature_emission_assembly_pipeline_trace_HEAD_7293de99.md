# Primary — Signature Emission / Assembly Pipeline Trace

**Role:** PRIMARY investigator.
**Focus:** signature emission/assembly pipeline — token emitters, concatenation/nesting.
**Investigation question:** why the recurring composite
`overseer-obs:…|goal:blocked:…|workstream-gap|…` signature was "seen 2×" in cognitive memory.
**Branch/HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `7293de99`.
**Doctrine:** validate-don't-re-derive. Every citation below was re-checked against live
`src/overseer/` at `7293de99`; the nesting mechanic was empirically reproduced (§4).

---

## 1. Executive summary

The string in the question is **not a memory key and not a storage/dedup bug**. It is the
Overseer's own observation write-back **`observation_signature`** (`mod.rs:1068-1073`) — the
cycle's problem `dedup_key`s, `sort`ed, `dedup`ed, `|`-joined, prefixed `overseer-obs:`.

The visible duplication (`overseer-obs:…` repeated 6×, whole `goal:blocked:…` blocks repeated
~5×, `workstream-gap|workstream-gap` runs) is the fingerprint of a **closed self-ingestion
feedback loop**: each write-back episode embeds its own composite signature as `[sig:…]`
(`wiring.rs:1084`); a later cycle recalls that episode, recovers the composite via
`parse_failure_signature` (`wiring.rs:976-986, 1025`), counts it, and re-emits it as
`Signal::RecurringSignature { signature: <whole prior composite>, occurrences }`
(`signal.rs:455-469`). That prior composite then re-enters the **next** signature as **one
opaque outer key** (`mod.rs:1359`), nesting `overseer-obs:` inside `overseer-obs:`. **"seen 2×"**
is the honest occurrences counter (`RECURRING_SIGNATURE_THRESHOLD = 2`, `signal.rs:362`) rendered
by the summary at **`mod.rs:1361`** — the exact and only source of the string quoted in the
question.

Two independent emission-hygiene defects fall out of this trace:

- **D1 (self-ingestion nesting).** The outer `keys.dedup()` (`mod.rs:1071`) is adjacency-only; it
  cannot see inside a nested composite that has been re-ingested as a single opaque key, so the
  loop grows the blob by one nested copy **per write-back generation**, unbounded across windows/
  restarts. This is what makes the blob repeat.
- **D1b (truncation breaks idempotency).** `sanitize_recalled` caps the recalled signature at
  `RECALLED_TEXT_MAX_LEN = 8192` bytes (`capabilities.rs:455, 468-482`) at the admission boundary
  (`mod.rs:1359`). Once a nested blob exceeds 8192 bytes it is **truncated mid-token**, changing
  its bytes — so the doc-comment invariant "two identical observations produce the same signature"
  (`mod.rs:1064-1067`) silently breaks for large blobs, defeating the write-back gate's dedup.

---

## 2. Emission pipeline — token-by-token provenance (verified @ 7293de99)

| Token in the blob | Emitter (file:line) | Construction |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` — `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` |
| `goal:blocked:<slug>-<8hex>` | `classify_signal`, `GoalBlocked` arm — `mod.rs:1336` | `format!("goal:blocked:{goal_id}")`; `<slug>-<8hex>` **is** the goal_id, minted upstream |
| `workstream-gap` (bare constant) | `classify_signal`, `WorkstreamGap` arm — `mod.rs:1371` | literal `"workstream-gap"` — ONE consolidated key per Observe pass, per-gap identity erased |
| nested `overseer-obs:…` fragment | `classify_signal`, `RecurringSignature` arm — `mod.rs:1353-1363` | `sanitize_recalled(signature)` — the WHOLE recalled composite admitted as one opaque `dedup_key` |
| `resource:engineer_spawn` etc. | other `classify_signal` arms — `mod.rs:1237-1380` | one `dedup_key` per signal kind; joined identically |

**Assembly, once per surviving tick** (single call chain, verified):

```
run_cycle
  └─ orient(signals,in_flight)                      mod.rs:1200  → Problem.dedup_key each
       └─ classify_signal(signal)                   mod.rs:1238  → (kind,prio,key,summary)
  └─ write_back_observation(problems)               mod.rs:534   (guarded: recall on, non-empty)
       └─ observation_signature(problems)           mod.rs:546 → 1068-1073   COMPOSITE built here
       └─ write_back_gate.peek/commit (900s)        mod.rs:548-556  in-memory, per-process
       └─ record_observation(episode)               wiring.rs:1076-1091
            └─ store_episode("<content> [sig:<composite>]", "overseer", {signature})   wiring.rs:1084,1088
```

**Concatenation semantics (load-bearing):**
- `keys.join("|")` (`mod.rs:1072`) is the ONLY concatenation. There is no other place a `|` is
  introduced, so **every `|` in the blob is either a fresh outer key boundary OR a frozen boundary
  inside a previously-nested composite.** The two are indistinguishable in the flat string — this
  is the whole illusion.
- `keys.dedup()` (`mod.rs:1071`) removes **only adjacent** equal keys; `sort_unstable` first makes
  byte-identical keys adjacent, so **within a single generation identical outer keys DO collapse**
  (empirically confirmed, §4). Therefore intra-generation duplication is impossible — the visible
  duplication must be cross-generation nesting.

---

## 3. The self-ingestion loop that produces the nesting (closed, verified)

```
                    ┌─────────────────────────────────────────────┐
                    │   write_back_observation (mod.rs:534)        │
  Problem.dedup_keys│   observation_signature → "overseer-obs:…"   │
  ───────────────▶  │   record_observation stores content:         │
                    │     "<summary> [sig:<COMPOSITE>]" wiring:1084 │
                    └───────────────────────┬─────────────────────┘
                                            │  (episodic memory, multi-writer graph)
                    ┌───────────────────────▼─────────────────────┐
   next cycle       │ recall_episodic → parse_failure_signature    │
   recall_pass      │   recovers [sig:<COMPOSITE>]  wiring:976-986,1025
                    │ signals_from counts by failure_signature     │
                    │   ≥2 identical ⇒ Signal::RecurringSignature{ signal.rs:455-469
                    │     signature: <COMPOSITE>, occurrences }     │
                    └───────────────────────┬─────────────────────┘
                                            │
                    ┌───────────────────────▼─────────────────────┐
   classify_signal  │ dedup_key = sanitize_recalled(<COMPOSITE>)   │ mod.rs:1359
   (RecurringSig)   │   ← the WHOLE prior blob, as ONE opaque key  │
                    │ summary = "recurring signature seen {N}× in  │ mod.rs:1361
                    │   cognitive memory ({<COMPOSITE>})"          │  ← STRING IN THE QUESTION
                    └───────────────────────┬─────────────────────┘
                                            │  feeds orient → next observation_signature
                                            ▼
                 overseer-obs: … | <COMPOSITE-as-one-key> | …   (nesting +1 each generation)
```

- **Closure proof:** the `[sig:…]` written at `wiring.rs:1084` is exactly what
  `parse_failure_signature` reads back at `wiring.rs:976-986` (`"[sig:"` … `"]"`). Write key ==
  read key ⇒ the Overseer recalls its **own** bookkeeping. Confirmed by inspection at HEAD.
- **The quoted string's single source:** `grep` for `"recurring signature seen"`/`"in cognitive
  memory"` across `src/overseer/` returns exactly one hit — `mod.rs:1361`. The question is quoting
  a `Signal::RecurringSignature` summary with `occurrences = 2` and `signature = <the giant nested
  composite>`. (`signal.rs:647` is a *different*, `'…'`-quoted phrasing — not this string.)
- **`occurrences = 2` is honest:** `signals_from` (`signal.rs:456-468`) counts recalled episodes
  by `failure_signature` and emits at `>= RECURRING_SIGNATURE_THRESHOLD (=2)`. Two recalls of the
  same composite ⇒ "2×". This is a real re-observation of a static problem set, not a replay bug.

---

## 4. Empirical reproduction of the nesting/dedup-blindness

A standalone program replicating `observation_signature` byte-for-byte
(`sort_unstable; dedup; "overseer-obs:" + join("|")`), nesting each generation's composite as one
opaque key (as `mod.rs:1359` does):

```
GEN1: overseer-obs:goal:blocked:X-7f5a|workstream-gap
GEN2: overseer-obs:goal:blocked:X-7f5a|overseer-obs:goal:blocked:X-7f5a|workstream-gap|workstream-gap
GEN3: overseer-obs:goal:blocked:X-7f5a|overseer-obs:goal:blocked:X-7f5a|overseer-obs:goal:blocked:X-7f5a|workstream-gap|workstream-gap|workstream-gap
OUTER-DEDUP (3x identical -> 1): overseer-obs:workstream-gap
workstream-gap substrings visible in flat GEN3: 3
```

**What this proves:**
1. The flat blob grows by **one nested `overseer-obs:…` copy and one `workstream-gap` per
   generation** — structurally identical to the question's blob (leading `overseer-obs:goal:
   blocked:…7f5afcca` ×6, repeated blocks ×5, `workstream-gap|workstream-gap` runs). The repeat
   count ≈ **number of write-back generations** the loop has run for that inner key (≈6 here).
2. `keys.dedup()` **does** collapse 3 byte-identical adjacent OUTER keys → 1
   (`overseer-obs:workstream-gap`). So the per-generation dedup is working exactly as designed.
3. The duplication survives **only** because one copy is frozen **inside** the nested opaque key's
   text, where the outer `dedup()` cannot reach. **The `|` between a nested blob's interior tail
   and the next fresh sibling key is indistinguishable from a real key boundary** — that is the
   entire mechanism.

Conclusion: the observed duplication is **monotonic self-ingestion nesting**, not a dedup, sort,
storage, or replay defect at any single generation.

---

## 5. Existing tests that lock the emission behavior (and the gap)

Locked at HEAD (all 32 in `tests_memory_recall.rs` pass, re-run @ 7293de99):
- `recurring_signature_emitted_when_two_episodes_share_signature` (`:471`) — the ≥2 counter.
- `recurring_signature_not_emitted_for_single_occurrence` (`:494`) — threshold floor.
- `recurring_signature_problem_summary_is_sanitized` (`:582`) — sanitize at admission (`mod.rs:1359`).
- `write_back_is_deduplicated_within_window` (`:797`) — the 900 s peek→commit gate.
- `write_back_persists_again_for_a_distinct_signature` (`:820`) — a **different** signature re-records.
- `sanitize_recalled_caps_length` (`:398`) — the 8192-byte truncation (root of D1b).

**Coverage gap (citation-worthy):** there is **no** test asserting that a recall-derived
`RecurringSignature` is *kept out of* the next `observation_signature`, and **no** test that a
composite exceeding 8192 bytes stays byte-stable across generations. D1/D1b are therefore
**unmitigated and unguarded** at HEAD — any fix must add both an anti-nesting test and a
large-blob idempotency test.

---

## 6. Key insights (emission-pipeline scope)

- **The count is honest; audit the loop, not the counter.** The signature is a deterministic
  fingerprint; a correct "2×" that never trends to zero points at a **missing convergence rung**,
  not a counting defect (consistent with the cross-investigation synthesis).
- **One concatenation, two boundary meanings.** `keys.join("|")` (`mod.rs:1072`) is the sole `|`
  source; nesting overloads the delimiter so a frozen interior boundary reads identically to a
  fresh key boundary. Flat-string inspection of the blob **overcounts** apparent duplication.
- **Per-generation dedup works; cross-generation nesting doesn't dedup at all.** `sort+dedup`
  collapses identical OUTER keys but is blind inside a re-ingested opaque composite (§4).
- **Truncation silently breaks the idempotency invariant.** The 8192-byte cap
  (`capabilities.rs:455`) applied at `mod.rs:1359` can slice a `…-<hash>` mid-token; two
  different-length nested blobs then differ in bytes → the write-back gate stops deduping them and
  `signals_from`'s exact-string count fragments. The invariant promised at `mod.rs:1064-1067` holds
  only for sub-8192-byte blobs.
- **The overseer observes its own bookkeeping.** Recall-derived `RecurringSignature` `dedup_key`s
  are written straight back into future signatures (`mod.rs:1359` → `orient` → `1068-1073`). The
  authors already treat recalled signatures as untrusted at the READ boundary (`sanitize_recalled`)
  yet still WRITE them back — a real feedback smell.

### Minimal, emission-scoped remediation candidates (diagnosis-only; no code changed)
1. **Break the self-ingestion at the write boundary (D1).** In `observation_signature` (or just
   before write-back), **exclude keys that are themselves `overseer-obs:`-prefixed** (recall-derived
   self-observations) from the composite, so the Overseer never re-nests its own prior signature.
   Guard with an anti-nesting test.
2. **Make nesting idempotent if it must be kept (D1b).** Replace raw truncation of the recalled
   signature with a **stable hash** of the whole composite (e.g. `overseer-obs-recall:<sha8>`), so a
   re-ingested signature is a fixed-width, byte-stable key — restoring the `mod.rs:1064-1067`
   invariant and the gate's dedup for large blobs.
3. **Preserve per-gap identity (adjacent finding).** Key the `WorkstreamGap` problem on
   `workstream-gap:<GapItem.signature>` (already available at the act gate) instead of the bare
   constant `"workstream-gap"` (`mod.rs:1371`), so `dedup()` can actually collapse repeats and the
   blob stops accumulating indistinguishable gap tokens.

*(1) and (2) are alternatives to the same seam; (3) is independent. All are emission-hygiene only —
the blocked-goal convergence rung and the `WorkstreamCoverage` closing edge are out of this
sub-scope and covered by the cross-investigation synthesis (D2/D3).* 

---

## 7. Remaining unknowns (emission scope)

- **Exact generation count for this blob.** The repeat multiplicity (6× inner, ~5× block) implies
  ≈5–6 write-back generations, but the precise number depends on how many recalls exceeded the ≥2
  threshold each window vs. daemon restarts resetting the in-memory gate — not recoverable from the
  string alone.
- **Whether truncation already fired here.** If any nesting generation crossed 8192 bytes, D1b is
  already active in this blob (fragmenting the count); the blob length is consistent with it but not
  proof. Needs the raw stored `[sig:…]` byte length from the live episode.
- **Interaction of (1)/(2) with existing recall promotion.** Excluding/hashing self-observations
  must preserve the *legitimate* priority-raise in `orient` (`mod.rs:1211-1219`) for genuinely
  recurring **non-self** signatures; validated by reasoning, not yet by a test.
