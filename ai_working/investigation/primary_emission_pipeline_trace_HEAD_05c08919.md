# Primary Deep-Dive: Signature Construction + Full Emission Pipeline Trace

**Investigation question:** the `recurring signature seen 2× in cognitive
memory (overseer-obs:goal:blocked:…|…|workstream-gap|…|resource:engineer_spawn|…)`
containing nested/repeated `overseer-obs:goal:blocked:…` tokens interleaved with
raw `goal:blocked:…`, `workstream-gap`, and `resource:engineer_spawn` tokens.

**Focus:** trace signature construction + the full emission pipeline
`run_cycle → orient → signal_to_problem (classify_signal) → write_back_observation
→ observation_signature → record_observation`.

All file:line references are on branch
`investigation/recurring-blocked-goals-workstream-gaps`, HEAD `05c08919`.

**Verdict (independently confirmed):** This is a **self-referential
write-back/recall feedback artifact**, not an external defect. The Overseer
persists its own observation with signature `overseer-obs:<sorted dedup_keys>`,
later **recalls its own episodes**, counts ≥2 of them as a `RecurringSignature`,
turns that signature into a Problem whose `dedup_key` **is** the recalled
`overseer-obs:…` string, and folds it back into the *next* `observation_signature`
— compounding the `overseer-obs:…|overseer-obs:…` nesting. The `2×` is "two
persisted episodes share one `[sig:…]` marker," not two real incidents.

---

## 1. Full emission pipeline (function-by-function trace)

### 1.1 `run_cycle` — `src/overseer/mod.rs:384`
Orchestrates one meta-OODA tick. Relevant ordering:
- `:386–414` Observe: status snapshot + `blocked_goals`/`in_flight` (`:393`),
  `workstream_gaps` (`:401`), drained step failures (`:414`).
- `:423–438` **USE / recall step (the loop's read side):** builds a *pre-recall*
  signal+problem set to derive `RecallKeys` (`:424–426`), then `recall_pass`
  (`:427`) populates `observed.recall`. **This recalls the Overseer's own prior
  write-back episodes** (no source-exclusion filter — see §3.3).
- `:441` `let signals = signals_from(&observed);` — now includes any
  recall-derived `Signal::RecurringSignature`.
- `:447` `let mut problems = orient(&signals, &in_flight);`
- `:455–459` per-problem root-cause enrichment (does not affect the signature).
- Returns `CycleReport { problems, … }` (`:482`). `problems` is the exact slice
  later handed to write-back.

### 1.2 `orient` — `src/overseer/mod.rs:1200`
Folds signals → ranked, deduped `Problem`s.
- `:1204` calls `classify_signal(s)` (this is the task's "signal_to_problem").
- `:1207–1209` dedups against Simard in-flight refs — the `overseer-obs:…` key
  never matches engineer work, so the recurring-signature problem **survives**.
- `:1211–1221` merges same-key signals; a `RecurringSignature` co-signal raises
  the matched problem's priority (`:1217–1219`). Standalone it becomes its own
  High-priority problem carrying the recalled key.
- `:1222–1230` pushes new `Problem { dedup_key = key, … }`.

### 1.3 `classify_signal` (= `signal_to_problem`) — `src/overseer/mod.rs:1238`
Produces `(kind, priority, dedup_key, summary)`. The **constituent keys** of the
observed signature are emitted here:

| Token in signature | Source arm | file:line |
|---|---|---|
| `goal:blocked:<id>` | `Signal::GoalBlocked` | `mod.rs:1336` |
| `workstream-gap` | `Signal::WorkstreamGap` (single consolidated key) | `mod.rs:1371` |
| `overseer-obs:…` (recalled) | `Signal::RecurringSignature` → `dedup_key = sanitize_recalled(signature)` | `mod.rs:1359` |
| human `recurring signature seen N×…` string | same arm, summary | `mod.rs:1360–1362` |

`resource:engineer_spawn`: the `resource:` dedup_key form is
`classify_signal`'s `Signal::EngineerSpawnRate` arm (`mod.rs:1267–1272`,
`"resource:engineer_spawn"`); the bare `engineer_spawn` recall *keyword* is
`capabilities.rs:562`.

Signal derivation feeding these: `signals_from` — `src/overseer/signal.rs:366`
(GoalBlocked per blocked goal `:440–448`; WorkstreamGap `:475–479`;
**RecurringSignature `:455–470`**, threshold `RECURRING_SIGNATURE_THRESHOLD = 2`
at `:362`).

### 1.4 `write_back_observation` — `src/overseer/mod.rs:534`
Called from the tick at `src/overseer/wiring.rs:301` with the **full
`cycle.problems`** (including the recurring-signature problem).
- `:538–545` no-ops when recall disabled or no problems.
- `:546` `let signature = observation_signature(problems);`
- `:548` gate `peek` (`WhisperGate`, window 900s / 5-per-hr — `mod.rs:299`).
- `:550–557` on `Deliver`: builds `ObservationEpisode { content =
  observation_content(problems), signature }`, calls
  `record_observation` (`:554`), then `commit`s the dedup slot **only after a
  successful store** (`:556`).

### 1.5 `observation_signature` — `src/overseer/mod.rs:1068`
```rust
let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
keys.sort_unstable();          // :1070
keys.dedup();                  // :1071  (collapses only EXACT duplicates)
format!("overseer-obs:{}", keys.join("|"))   // :1072  ← THE EMITTER
```
This is the single construction site of the `overseer-obs:` prefix. It joins
**every** problem's `dedup_key`, so when a recalled `overseer-obs:…` key is
present it is embedded verbatim → `overseer-obs:…|overseer-obs:…`. **No length
cap here.**

### 1.6 `record_observation` (production adapter) — `src/overseer/wiring.rs:1076`
```rust
let content  = format!("{} [sig:{}]", episode.content, episode.signature); // :1084
let metadata = serde_json::json!({ "signature": episode.signature });      // :1085
store_episode(&content, OVERSEER_SOURCE_LABEL /* "overseer" */, Some(&metadata)); // :1088
```
Embeds the signature as a `[sig:…]` marker in **persistent** episodic memory.

---

## 2. Loop closure (why it recurs and nests)

```
run_cycle ─▶ signals_from ─▶ orient/classify_signal ─▶ write_back_observation
   ▲                                                         │
   │                                     observation_signature = "overseer-obs:"+keys
   │                                                         │
   │                                            record_observation → store_episode
   │                                            content "... [sig:overseer-obs:...]"
   │                                                         │ (persistent graph)
   │  next tick: recall_episodic (wiring.rs:1013) ──────────┘
   │            parse_failure_signature "[sig:…]" (wiring.rs:976, called :1025)
   │                                                         │
   └── signals_from counts ≥2 same sig ⇒ RecurringSignature (signal.rs:455–470)
                 classify_signal dedup_key = the recalled "overseer-obs:…" (mod.rs:1359)
                 ⇒ folded into next observation_signature  ── LOOP CLOSES
```

`sort_unstable` + `dedup` (`mod.rs:1070–1071`) only remove **adjacent exact
duplicates** within one assembly. Distinct-but-overlapping `overseer-obs:…`
generations, and the repeated `goal:blocked:*` / `workstream-gap` blocks, differ
by embedded content and therefore survive — producing the observed multi-block,
multi-prefix string.

---

## 3. Refinements beyond the prior primary report

### 3.1 A process **restart is NOT required** for the `2×` (dominant path)
The prior report frames restart (ephemeral gate reset) as *the* enabler. That is
one path, but not the main one here. The `write_back_gate` window is **900s**
(`mod.rs:299`); it suppresses re-writes only *within* 900s, not across the
process lifetime. A **stable** signature — exactly what a set of long-lived
blocked goals (`goal:blocked:fix-agent-kgpacks-rs-issue-17…`,
`…-issue-18…`, `simard-identity-*`, etc.) produces every tick — is re-written
each time the 900s window lapses, up to 5×/hr. Two window lapses ⇒ two persisted
`[sig:…]` episodes ⇒ `occurrences == 2` at `signal.rs:463` ⇒ the `2×` signal,
**within a single long-running process**. Restart is merely an *additional* way
to reopen the window. The blocked-goal-heavy content of the observed signature is
consistent with this window-expiry path, not a restart-only one.

### 3.2 Signature growth is **bounded**, but spawns new stable variants
`classify_signal` wraps the recalled signature in `sanitize_recalled`
(`mod.rs:1359`), which caps at `RECALLED_TEXT_MAX_LEN = 8192` bytes
(`capabilities.rs:455, 468–482`). So each generation's recurring-signature
`dedup_key` is ≤8KB. `observation_signature` (`mod.rs:1068`) has **no** cap of
its own, but its inputs are bounded, so the outer signature cannot grow without
limit — it saturates near the 8KB truncation point. Truncation at the boundary
then yields a **new stable truncated signature** that itself recurs ≥2×,
sustaining the artifact even after growth plateaus. This explains a signature
that is large and repetitive yet not infinitely long.

### 3.3 No self-exclusion on the recall read path
`recall_episodic` (`wiring.rs:1013–1031`) calls `recall_episodes_ranked` with no
`source_label` filter, so episodes written with `OVERSEER_SOURCE_LABEL =
"overseer"` (`wiring.rs:952, 1088`) are recalled by the very process that wrote
them. The provenance tag exists but is **not** used to break the read loop. This
is the cleanest single-point interception for a fix (filter out
`source == "overseer"` on recall, or exclude `overseer-obs:` signatures from the
`RecurringSignature` count in `signal.rs:455–470`).

---

## 4. Minimal-fix pointers (not implemented — investigation only)
Ordered by locality; any one breaks a distinct part of the loop, (a)+(c) recommended:
- **(a) Break self-ingestion at count time** — in `signals_from`
  (`signal.rs:455–470`) skip episodes whose `failure_signature` starts with
  `overseer-obs:` (or whose source is `"overseer"`). Stops both the count
  inflation *and* the nesting at the read boundary.
- **(b) Stop folding recall-derived keys into the next signature** — in
  `observation_signature` (`mod.rs:1068`) / `write_back_observation`
  (`mod.rs:546`) exclude `dedup_key`s starting `overseer-obs:`. Stops nesting;
  does not by itself stop `2×` on stable base signatures.
- **(c) Store-side idempotency on `signature`** — in `record_observation`
  (`wiring.rs:1076–1091`) upsert/dedup on the `signature` metadata so identical
  `[sig:…]` episodes are never duplicated across windows/restarts. Directly kills
  `occurrences>=2` inflation regardless of window/restart timing.

---

## Evidence index (verbatim loci, HEAD 05c08919)
- Pipeline entry `run_cycle`: `src/overseer/mod.rs:384`; recall step `:423–438`;
  write-back call `src/overseer/wiring.rs:301`.
- `orient`: `src/overseer/mod.rs:1200`; `classify_signal` (`signal_to_problem`):
  `:1238` (GoalBlocked `:1336`, EngineerSpawnRate `:1267`, WorkstreamGap `:1371`,
  RecurringSignature `:1353–1363`).
- `signals_from`: `src/overseer/signal.rs:366`; 2× count `:455–470`; threshold
  `:362`.
- `write_back_observation`: `src/overseer/mod.rs:534–563`.
- `observation_signature` (the `overseer-obs:` emitter): `src/overseer/mod.rs:1068–1073`.
- `record_observation` + `[sig:…]` embed: `src/overseer/wiring.rs:1076–1091`.
- Recall read + `[sig:…]` parse: `src/overseer/wiring.rs:1013–1031`, `976–986`.
- `sanitize_recalled` 8192 cap: `src/overseer/capabilities.rs:455, 468–482`.
- Source label (unused on read): `src/overseer/wiring.rs:952, 1088`.
