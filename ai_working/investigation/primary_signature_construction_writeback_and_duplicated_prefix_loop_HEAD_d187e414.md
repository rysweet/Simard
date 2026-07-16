# Primary — Signature Construction, Write-Back Persistence & the Duplicated-Prefix / Feedback Loop

**Role:** PRIMARY investigator (deep dive).
**Focus:** signature construction · write-back persistence · the duplicated-prefix / self-feeding
loop that produces the recurring `overseer-obs:…|goal:blocked:…|workstream-gap|…` blob.
**Branch/HEAD:** `investigation/recurring-blocked-goals-workstream-gaps` @ `d187e414`.
**Prior primaries validated:** `b47b6413` (emission trace + 2× verdict), `7293de99`, `dea65df8`.
**Doctrine:** validate-don't-re-derive. Every citation below was re-read against **live** source at
`d187e414`, the load-bearing test was re-run, and the loop was reproduced empirically (§4).

---

## 0. Verdict (four parts, all confirmed at HEAD `d187e414`)

1. **The duplicated prefix is a real self-ingestion feedback loop, not a display artifact.**
   The Overseer writes its own observation signature back into cognitive memory, later recalls it,
   admits the *whole recalled composite* as a single new problem key, and re-wraps it in a fresh
   `overseer-obs:` prefix on the next write-back. Each Observe→write-back generation adds **one more
   nested `overseer-obs:` layer and one more copy of every frozen inner token**. (§2, §3)
2. **The write-back dedup gate cannot stop it — it *fuels* it.** The gate is keyed on the full
   `observation_signature`, but that signature **grows every generation**, so consecutive generations
   are never byte-identical, `peek` always returns `Deliver`, and a fresh episode is stored each
   window. The very growth the loop causes defeats the idempotency the gate promises. (§3.3, §4)
3. **The blob shape is reproduced byte-for-byte** by a faithful simulation of the two exact
   functions: `overseer-obs:goal:blocked:…-7f5afcca` repeated N× followed by a run of
   `workstream-gap`, matching the investigation-question string. N repeats ⇒ N generations. (§4)
4. **Zero source drift since the prior primaries.** `git diff --stat b47b6413..HEAD -- src/` is
   **empty**; the two intervening commits are docs-only. Every prior citation is live; the loop
   (D1) and the truncation hazard (D1b) remain **unguarded**. Extend, don't restart. (§6)

---

## 1. Signature construction — the two producers (re-read @ `d187e414`)

There are exactly **two** places that build a `dedup_key`/signature relevant to this blob, plus one
sink that concatenates them.

| Concern | Code (file:line @ HEAD) | Construction |
|---|---|---|
| **Outer composite (the write-back signature)** | `src/overseer/mod.rs:1068-1073` | `keys = problems.map(dedup_key); keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` — **line 1072 is the sole `\|` and the sole `overseer-obs:` producer** |
| **Blocked-goal key** | `src/overseer/mod.rs:1336` | `format!("goal:blocked:{goal_id}")` |
| **Recall-derived key (the nesting seam)** | `src/overseer/mod.rs:1359` | `sanitize_recalled(signature)` — admits the **entire recalled composite** as ONE opaque `dedup_key` |
| **Workstream-gap key** | `src/overseer/mod.rs:1371` | bare literal `"workstream-gap"` — per-gap identity erased, one consolidated key per pass |

Constants re-verified unchanged: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, gate at
`signal.rs:463`); `RECALLED_TEXT_MAX_LEN = 8192` (`capabilities.rs:455`, UTF-8-boundary cap at
`capabilities.rs:472`).

**Key observation (my focus):** the outer composite (`mod.rs:1072`) has **no cap and no
self-exclusion**. It happily ingests any `dedup_key`, including one produced by `mod.rs:1359` that
*already begins with `overseer-obs:`*. Nothing in the construction path breaks the recursion.

---

## 2. Write-back persistence — how the signature becomes a recallable `failure_signature`

The write-back path is a closed cycle across three files:

1. **Store.** `write_back_observation` (`mod.rs:534-557`) builds
   `signature = observation_signature(problems)` (`mod.rs:546`) and persists an `ObservationEpisode`
   via `record_observation` (`mod.rs:554`). The production adapter
   (`wiring.rs:1076-1091`) embeds the signature as a text marker:
   `format!("{} [sig:{}]", episode.content, episode.signature)` (`wiring.rs:1084`) and also stores it
   in node metadata. **The stored `[sig:…]` is the untruncated `observation_signature`.**
2. **Recall + parse.** On a later cycle, `recall_episodic` (`wiring.rs:1013-1031`) reads the episode
   back and `parse_failure_signature` (`wiring.rs:976-986`) extracts the `[sig:…]` marker into
   `RecalledEpisode.failure_signature` (`wiring.rs:1025`). **The Overseer's own prior write-back is
   now indistinguishable from any other recalled failure signature.**
3. **Count.** `signals_from` (`signal.rs:455-469`) tallies recalled episodes by `failure_signature`
   into a `BTreeMap`; when a signature is seen `>= RECURRING_SIGNATURE_THRESHOLD (2)` it emits
   `Signal::RecurringSignature { signature, occurrences }` (`signal.rs:464-467`). This is the sole
   producer of the "N×" count and of the recurring-signature signal.
4. **Classify → key.** `classify_signal`'s `RecurringSignature` arm (`mod.rs:1353-1363`) sets the new
   problem's `dedup_key = sanitize_recalled(signature)` — i.e. **the whole prior composite becomes a
   single problem key**.
5. **Re-emit (loop closure).** `wiring.rs:301` calls
   `overseer.write_back_observation(&cycle.problems)` with the **full** problem set — including the
   composite-keyed problem from step 4. Back to step 1: `observation_signature` sorts/dedups/joins all
   keys and **re-prepends `overseer-obs:`**, nesting the entire previous signature one level deeper.

There is **no guard** anywhere on this path that strips or rejects `overseer-obs:`-prefixed keys
before write-back. Confirmed: `grep -rn "overseer-obs" src/` yields only the *producer* at
`mod.rs:1072` and two unrelated sensor/recipe ids (`observer.rs:130`, `sensor.rs:509`). **D1 is live
and unguarded at HEAD.**

---

## 3. Why the "duplicated prefix" grows — mechanics

### 3.1 Prefix stacking
`observation_signature` unconditionally prepends `overseer-obs:` (`mod.rs:1072`). When one of the
joined keys is itself an `overseer-obs:…` composite (from `mod.rs:1359`), the output contains an
*inner* `overseer-obs:` (frozen inside the nested key) **plus** the fresh *outer* one. Generation G
therefore carries `G+1` copies of the `overseer-obs:` literal — exactly the repeated prefix in the
investigation blob.

### 3.2 Delimiter overload (the counting illusion, reconciled with prior primaries)
`mod.rs:1072` is the only `|` source, but every `|` inside a *previously nested* composite is frozen
into the flat string and is indistinguishable from a fresh outer boundary. So flat-string inspection
**cannot** tell "5 distinct problems" from "1 problem whose key is a 5-deep nested composite." Prior
primaries are right that this makes naive flat counting overcount *structural depth*. **My focus adds
the orthogonal fact that the blob genuinely GROWS** (§4) — the nesting is not merely a rendering of a
static composite; each generation is a strictly longer stored signature.

### 3.3 The dedup gate is defeated by the growth it causes
`write_back_gate.peek(&signature, now)` (`mod.rs:548`) suppresses a *repeat within the window* only
when the signature is byte-identical. But because step 5 nests the prior signature, **generation G+1
is strictly longer than generation G** — never equal — so `peek` returns `Deliver` every time and
`commit` (`mod.rs:556`) records a brand-new slot. The doc-comment invariant at `mod.rs:1064-1067`
("two identical observations produce the same signature") holds *only* for a fixed input; self-
ingestion guarantees the input is never fixed. **The idempotency mechanism is structurally unable to
converge this loop.** (Empirically `gate_dedup_hit=False` at every generation — §4.)

### 3.4 Truncation hazard (D1b) — still live, asymmetric
The classify-side key is `sanitize_recalled(signature)` capped at 8192 bytes on a **char** (not
**token**) boundary (`capabilities.rs:468-482`), while the **stored** `[sig:…]`
(`observation_signature`, `mod.rs:1072`) has **no cap**. Consequences once the composite passes 8192
bytes:
- the recall-derived key can be sliced mid-token (through a `-<8hex>` suffix or an inner
  `overseer-obs:` prefix), yielding a corrupt key;
- the truncated classify key and the untruncated stored signature **diverge**, so distinct large
  composites can collapse to the same 8192-byte prefix on the classify side → false merges, while the
  gate (on the untruncated sig) still sees them as distinct → continued storage. The two length
  regimes disagree. Unguarded at HEAD.

---

## 4. Empirical reproduction (my focus, made concrete)

A faithful Python mirror of the two exact functions — `observation_signature` (`mod.rs:1068-1073`),
the classify dedup_key rule (`mod.rs:1359`), the 8192-byte cap (`capabilities.rs:468-482`), the
`>=2` recall count (`signal.rs:455-469`), and the full-signature gate key (`mod.rs:548`) — seeded
with the persistent set `{goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca,
workstream-gap}`:

```
gen 0 (len=97):  overseer-obs:goal:blocked:…-7f5afcca|workstream-gap
gen 1 (len=195, gate_dedup_hit=False): overseer-obs:goal:blocked:…-7f5afcca|overseer-obs:goal:blocked:…-7f5afcca|workstream-gap|workstream-gap
gen 2 (len=293, gate_dedup_hit=False): …×3 overseer-obs:goal:blocked:…-7f5afcca … ×3 workstream-gap
…
gen 6 (len=685, gate_dedup_hit=False): ×7 overseer-obs:goal:blocked:…-7f5afcca … ×7 workstream-gap
overseer-obs: prefix count in final blob: 7
goal:blocked token count in final blob: 7
```

**Findings from the reproduction:**
- The output is **byte-identical in shape** to the investigation-question blob: a run of
  `overseer-obs:goal:blocked:…-7f5afcca` repeats followed by a run of `workstream-gap`. The real blob
  simply carries the *full* multi-goal problem set (all `goal:blocked:*` + `simard-identity-*` keys)
  as the nested payload; the growth law is the same.
- **`gate_dedup_hit=False` at every generation** — direct confirmation of §3.3: the write-back gate
  never suppresses a generation.
- **Linear growth** (+98 bytes/gen for this 2-key seed) ⇒ **repeat multiplicity = generation count**.
  The observed blob's ≈5–7 `overseer-obs:` repeats ⇒ ≈5–7 self-ingestion generations before capture.
- Growth is bounded only by the 8192-byte `sanitize_recalled` cap on the classify key (§3.4), after
  which the corruption/false-merge regime begins — not a clean convergence.

(Reproduction script: `/tmp/sim.py`, a scratch artifact — not committed.)

---

## 5. Load-bearing test re-run (validate, don't trust the docs)

`cargo test -p simard --lib overseer::tests_root_cause` → **21 passed; 0 failed** at `d187e414`
(compile 1m41s). These lock the *lane-decoupling* semantics (a loud Lane-A "N×" cannot trip Lane-B's
`>=3` escalation, and vice-versa), confirming the "N×" is an honest re-observation count on the
episodic-recall lane. **No test asserts emission hygiene:** there is still **no** test that a
recall-derived `overseer-obs:` key is kept *out of* the next `observation_signature` (anti-nesting,
D1), and **no** test that a >8192-byte composite stays byte-stable across generations (D1b). Any fix
must add both.

---

## 6. Drift measurement

```
$ git diff --stat b47b6413..HEAD -- src/      # (empty — no source changed)
$ git log --oneline b47b6413..HEAD
d187e414 docs(investigation): re-execute all-hypothesis verification tests on HEAD b47b6413
641f9c37 docs(investigation): primary emission trace + 2x honest verdict + drift recheck at HEAD b47b6413
```

Both intervening commits are docs-only. Every citation in §1–§3 is byte-identical to what the prior
primaries verified. **The loop and the truncation hazard are un-invalidated and unguarded.**

---

## 7. Emission-scoped remediation candidates (my focus)

These attack signature construction / write-back — the seam that *creates* the loop. The D2/D3
convergence-rung and gap-routing defects remain the cross-investigation synthesis's scope.

1. **Break self-ingestion at the write boundary (D1, primary fix).** In or just before
   `observation_signature` (`mod.rs:1068`), **exclude `overseer-obs:`-prefixed keys** from the joined
   composite (they are the Overseer's own recalled write-backs). This stops the prefix from stacking
   while preserving the legitimate priority-raise the recalled signal still triggers in `orient`.
   Guard with a new anti-nesting test (§5 gap).
   - *Alternative, if nesting must be kept:* replace the raw recalled key at `mod.rs:1359` with a
     **stable fixed-width digest** (e.g. `overseer-obs-recall:<sha8>`, reusing the `occurrence_concept`
     SHA-256 approach at `mod.rs:1147-1156`). A bounded, idempotent token restores the
     `mod.rs:1064-1067` invariant and lets the gate (§3.3) actually converge.
2. **Make the stored signature and the classify key share one cap (D1b).** Either cap
   `observation_signature` output at `mod.rs:1072` to `RECALLED_TEXT_MAX_LEN` on a **token** (`|`)
   boundary, or hash-fold both sides, so the gate key and the classify key can never diverge in length
   or slice mid-token. Guard with a large-blob idempotency test.
3. **Preserve per-gap identity (independent).** Key `WorkstreamGap` on `GapItem.signature` instead of
   the bare `"workstream-gap"` literal (`mod.rs:1371`) so repeated gaps collapse under `dedup()`
   rather than accumulating one frozen `workstream-gap` token per generation (§4).

(1) is the highest-leverage fix: it severs the feedback edge at its source and is the only change that
makes the write-back gate capable of converging. (2) hardens the large-blob regime; (3) removes the
gap-token accumulation. All three are emission-hygiene only and carry no behavioral risk to the
lane-decoupled escalation path proven in §5.

---

## 8. Remaining unknowns (emission scope)

- Exact generation count for the captured blob (recoverable only from the stored `[sig:…]` byte
  length of the live episode; the flat string gives a lower bound via `overseer-obs:` repeat count).
- Whether the 8192-byte cap (D1b) already fired for this specific blob — needs the raw stored
  signature length.

---

## 9. One-line reconciliation

**At `d187e414` (zero source drift): the recurring blob is a genuine self-ingestion feedback loop —
`observation_signature` (`mod.rs:1072`) write-backs are stored as `[sig:…]` (`wiring.rs:1084`),
recalled and re-admitted as one whole composite key (`mod.rs:1359`), then re-wrapped in a fresh
`overseer-obs:` prefix each generation; the write-back dedup gate (`mod.rs:548`) can never fire
because the signature it keys on grows every cycle (empirically `gate_dedup_hit=False` at every
generation), so the fix is to exclude `overseer-obs:`-prefixed keys from the composite (or hash-fold
the nested key), not to touch the honest N× count.**
