# Primary deep dive — signature provenance chain + 2× write-back gate verdict (HEAD `b9f99879`)

**Role:** PRIMARY investigator.
**Investigation question:** the recurring signature seen **2×** in cognitive
memory — `overseer-obs:…|goal:blocked:…|workstream-gap|workstream-gap|resource:engineer_spawn|…`.
**Focus:** provenance chain `observer.rs`/`sensor.rs` → `signal.rs`
(`RecurringSignature`) → `notify.rs` → `stewardship/dedup.rs`/`routing.rs`, and
the **2× recurrence write-back gate verdict at HEAD**.
**Method:** independent, line-by-line source trace of the working tree.
**Anchor:** HEAD = `b9f99879`. Commits `6e3113bc..b9f99879` are all
`docs(investigation)` — **zero code delta**. `git diff 6b2bf5e1..HEAD -- src/overseer src/stewardship`
is **empty**, so the overseer/stewardship pipeline is byte-identical to the last
code change (`6b2bf5e1`). Every prior verdict cited at `85b9398a`/`5a85317b`
therefore still holds at this HEAD; I re-verified each link below.

---

## Verdict (high confidence)

**The `×2` is a REAL, honest cross-window re-observation of a near-static,
unresolved problem set — NOT a dedup / storage / replay / hash-collision bug.
But the *thing being counted is the Overseer's own observation write-back
bookkeeping*, so the "recurring signature" certifies a stuck system, not an
independent failure. There is a genuine, bounded, self-referential write-back
feedback defect (the nested `overseer-obs:` fragments).**

**Write-back-gate verdict at HEAD:** the 900 s `WhisperGate`
(`WhisperGate::new(900, 5)`, `mod.rs:298`) works exactly as designed and is
**correct** — it suppresses *same-window* duplicates only. It is **not** a loop
breaker and was never meant to be. Two legitimate write-back passes ≥15 min
apart produce two episode nodes carrying the identical composite
`[sig:overseer-obs:…]`; recall then counts 2 and `signals_from` fires
`RecurringSignature` at `occurrences >= 2`. The gate does not — and structurally
cannot — prevent this. The loop is **open at HEAD**.

---

## Provenance chain (code-evidenced, Observe → Orient → Act → Store → Recall)

### 1. Observe — where the tokens are born
- `run_cycle` (`mod.rs:384`) builds `ObservedState`: goal-board health →
  `observed.blocked_goals` (`mod.rs:393-394`); gap-scan → `observed.workstream_gaps`
  (`mod.rs:401-405`); step-failure sink drain (`mod.rs:414`).
- `sensor.rs` projects the raw snapshot; `recall` starts `None`
  (`sensor.rs:143-148`) — recall is populated later in `run_cycle`, not in the
  pure projection.

### 2. signal.rs — `signals_from` mints one Signal per condition
- `Signal::GoalBlocked` per blocked goal (`signal.rs:440-448`).
- `Signal::WorkstreamGap` — ONE consolidated signal for all gaps
  (`signal.rs:475-479`).
- `Signal::EngineerSpawnRate` when `live >= ENGINEER_SPAWN_THRESHOLD`
  (`signal.rs:393-397`).
- **`Signal::RecurringSignature`** (`signal.rs:455-470`): counts recalled
  `episodes[].failure_signature` into a `BTreeMap`; emits when
  `occurrences >= RECURRING_SIGNATURE_THRESHOLD` and
  `RECURRING_SIGNATURE_THRESHOLD == 2` (`signal.rs:362`). **This is the 2× gate
  on the read side.**

### 3. orient / classify_signal — Signal → Problem.dedup_key (the token strings)
- `goal:blocked:{goal_id}` (`mod.rs:1336`), `workstream-gap` constant
  (`mod.rs`, `WorkstreamGap` arm), `resource:engineer_spawn` constant
  (`mod.rs:1267-1272`).
- **`RecurringSignature` arm (`mod.rs:1353-1363`): `dedup_key = sanitize_recalled(signature)`.**
  When the recalled signature is a composite `overseer-obs:…` (see §6), it
  matches no in-cycle problem key, so the merge branch (`mod.rs:1211-1221`) is
  skipped and it becomes a **standalone** Problem whose `dedup_key` is the whole
  composite. **This is the nesting source.**

### 4. notify.rs / routing — the Act side (why the set never clears)
- `act_flag_workstream_gaps` (`mod.rs:884-948`) emits a `workstream-gap`
  notification only — no `FileIssue`, no `LaunchRecipe`. Corroborated by
  `notify.rs` (`kind: "workstream-gap"`). Gaps are **notify-only**, so the
  `workstream-gap` token is re-observed every tick.
- Blocked goals bare-park (no WHY class) so `goal:blocked:*` tokens persist.
- Net effect: the problem set is **near-static**, so its aggregate signature
  recurs honestly.

### 5. stewardship/dedup.rs + routing.rs — NOT on this path
- `failure_signature` (`dedup.rs:63-75`) produces a **16-hex** fingerprint for
  GitHub-issue dedup (`find_existing`, `dedup.rs:78-81`). The recurring string is
  human-readable, so it is **not** a `stewardship::failure_signature` value.
  `stewardship/routing.rs` routes issues, not memory episodes. **These files
  define the dedup *vocabulary* the naming imitates, but do not mint the
  recurring token.** (Ruled out as origin.)

### 6. Store → Recall — the write-back round-trip (the actual `×2` mechanism)
- Composite built by `observation_signature` (`mod.rs:1068-1073`): sort → dedup
  → `format!("overseer-obs:{}", keys.join("|"))`. `dedup()` after `sort` fully
  removes duplicates (all equal keys are adjacent) — **no `dedup()` bug**.
- `write_back_observation` (`mod.rs:534-563`): gates on the exact composite via
  `write_back_gate.peek` (900 s window, `guardrails.rs` `peek` keys on the exact
  signature string), stores only on `Deliver`, and `commit`s the slot **only
  after** a successful store (`mod.rs:556`).
- `record_observation` (`wiring.rs:1076-1091`) embeds `… [sig:{signature}]` into
  the episode content and stores it under `OVERSEER_SOURCE_LABEL = "overseer"`.
- `recall_episodic` (`wiring.rs:1013-1031`) reads episodes back and recovers
  `failure_signature = parse_failure_signature(&e.content)` (`wiring.rs:976-986`)
  — i.e. it parses `[sig:…]` from **any** episode, **including the Overseer's own
  write-backs. There is NO `source_label` self-exclusion at HEAD.**
- So the Overseer's `overseer-obs:…` write-back re-enters recall as a
  `failure_signature`; two such episodes ⇒ `RecurringSignature{occurrences:2}`
  (§2) ⇒ standalone `overseer-obs:…` Problem (§3) ⇒ folded into the **next**
  `observation_signature`, producing the nested `overseer-obs:…|overseer-obs:…`
  runs observed in the data.

---

## Why the write-back gate does not stop the 2× recurrence (verdict detail)
1. **Same-window dedup only.** `peek` returns `SuppressDuplicate` only while
   `now - last < 900 s`. Across windows the identical signature re-delivers by
   design → 2+ episodes accumulate → recall's `>= 2` fires. **Correct behavior,
   wrong expectation if treated as a loop breaker.**
2. **Signature mutation defeats even the in-window dedup while growing.** Because
   each cycle nests the prior composite (§6), the signature is a *new* string
   each cycle until it saturates the `sanitize_recalled` cap
   (`RECALLED_TEXT_MAX_LEN = 8192`, `capabilities.rs:455`). While growing, every
   composite is distinct, so the gate always `Deliver`s. After saturation the
   truncated prefix stabilizes and cross-window re-delivery sustains the 2×.
   Either regime keeps the loop alive.
3. **`2×` lands in a dead zone.** Episode-count threshold is 2 (visible `×2`);
   the root-cause **escalation** threshold is 3
   (`RECURRENCE_ESCALATION_THRESHOLD`, occurrence lane via `store_fact`). So the
   signature recurs at 2 forever without ever escalating — it neither closes nor
   climbs.

---

## Token-by-token origin (all confirmed at HEAD)
| Token | Origin | Evidence |
|---|---|---|
| `overseer-obs:` prefix + `\|`-join | `observation_signature` | `mod.rs:1068-1073` |
| `goal:blocked:<goal_id>` | `GoalBlocked` classify arm | `mod.rs:1336` |
| `workstream-gap` | `WorkstreamGap` classify arm (constant) | `mod.rs`; `notify.rs` |
| `resource:engineer_spawn` | `EngineerSpawnRate` classify arm | `mod.rs:1267-1272` |
| nested `overseer-obs:…` | recall-derived `RecurringSignature` written back | `signal.rs:455-470` + `mod.rs:1353-1363` + `wiring.rs:301` |

---

## Reconciliation with prior waves
This independently **confirms** `CONSOLIDATED_FINDINGS.md` §0/§0a,
`primary_signature_provenance_dedup_verdict.md`, and `FINAL_SYNTHESIS.md`
(defects D1 emission-hygiene, D2 escalation counter/gate coupling, D3 no closing
edge). No divergence. The only refinement I add: verified the **exact absence of
source-label self-exclusion in `recall_episodic` (`wiring.rs:1022-1030`)** as the
precise open-loop seam, and confirmed the pipeline is byte-identical at HEAD
`b9f99879`, so the verdict is current, not stale.

## Highest-leverage fix (unchanged from prior synthesis, re-affirmed)
**D1:** at the recall/count boundary, exclude the Overseer's own
`OVERSEER_SOURCE_LABEL` episodes (or drop `overseer-obs:*` failure_signatures)
so self-authored write-backs are never counted as recurring failures. This
breaks the nesting loop at its source; it does **not** by itself close the
underlying blocked-goal / workstream-gap lanes (D3), which need real closing
Act edges.
