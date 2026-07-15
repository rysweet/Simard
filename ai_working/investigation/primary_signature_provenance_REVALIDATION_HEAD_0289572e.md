# Primary Re-Validation — Signature Provenance & 2× Cross-Window Verdict (HEAD 0289572e)

**Investigator:** primary (analyzer lens)
**Base:** HEAD = `0289572e`; `git diff --name-only 6e3113bc..HEAD -- '*.rs'` is **EMPTY** (zero source drift).
**Verdict:** All prior conclusions reconcile with current source. The recurring
`overseer-obs:…` signature is an **honest 2× cross-window re-observation**, NOT a
dedup / hash / storage / replay artifact. No divergence from the five prior waves.

---

## 1. Signature construction & write-back path — CONFIRMED

`observation_signature` (src/overseer/mod.rs:1068-1073) is the aggregate join of
the sorted + deduped problem `dedup_key`s, prefixed `overseer-obs:`:

```rust
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    format!("overseer-obs:{}", keys.join("|"))
}
```

Carries **no independent failure signal** — it is purely the set of currently-open
member dedup_keys. This is the exact string in the investigation question.

**Write-back** (mod.rs:534-563): `write_back_observation` builds
`ObservationEpisode { content, signature }` (signature = the composite above) and
persists it via `record_observation`, gated by `write_back_gate` (a `WhisperGate`
**900 s / 15-min window**, mod.rs:191-192, 286). Slot committed only after a
successful store (mod.rs:556).

## 2. Self-referential feedback (nested `overseer-obs:` fragments) — CONFIRMED

The airtight round-trip loop that produces nested `overseer-obs:overseer-obs:…`
fragments:

1. Store: `record_observation` embeds `"{content} [sig:{signature}]"` and metadata
   `{"signature": …}` (src/overseer/wiring.rs:1084-1085).
2. Recall: `recall_episodic` parses it back via `parse_failure_signature(content)`
   → `RecalledEpisode.failure_signature = Some("overseer-obs:…")`
   (wiring.rs:976, 1024-1025; capabilities.rs:607-616, the "LOAD-BEARING key").
3. Detect: `signals_from` counts recalled episodes by `failure_signature`; ≥2 →
   `Signal::RecurringSignature { signature: "overseer-obs:…", occurrences }`
   (src/overseer/signal.rs:455-468).
4. Re-admit: `classify_signal` maps it to a Problem whose **dedup_key is the
   (sanitized) recalled signature** (src/overseer/mod.rs:1353-1363).
5. Re-join: that dedup_key re-enters the NEXT `observation_signature` (mod.rs:1069)
   → the composite now contains a nested `overseer-obs:` fragment.

The store is a single shared `Arc` handle (write-back and recall read the same
graph), so the loop is closed by design. `recall_occurrences` (mod.rs:972-997) is
the *fact*-side sibling (root-cause occurrences), distinct from the *episode*-side
recurrence lane above.

## 3. 2× recurrence mechanism — GENUINE RE-OBSERVATION, not artifact — CONFIRMED

`signal.rs:455-470` counts **distinct recalled episodes** sharing a
`failure_signature` (BTreeMap tally). The `occurrences` value (the "2×") = number
of persisted episode nodes.

- **Within-window dedup gate** (`write_back_gate.peek`, mod.rs:548) suppresses a
  same-signature write-back inside the 900 s window: at most ONE episode node per
  15 min per signature.
- Therefore `occurrences == 2` ⇒ **two temporally-distinct Observe passes ≥15 min
  apart** over an unchanged blocked state — a real cross-window re-observation
  loop, exactly as `primary_signature_provenance_dedup_verdict.md` concluded.
- **NOT** a hash collision, storage replay, or identical-string duplicate: the
  gate would collapse those within the window; distinct nodes require distinct
  windows.

The counted event is nonetheless **vacuous** — an aggregate join carrying no
independent failure signal (see §1).

## 4. Two-lane threshold / dead-zone — CONFIRMED

- `RECURRING_SIGNATURE_THRESHOLD = 2` (src/overseer/signal.rs:362) — **Lane A**,
  episode-count, drives priority promotion (the visible "2×").
- `RECURRENCE_ESCALATION_THRESHOLD = 3` (src/overseer/root_cause.rs:33) — **Lane B**,
  occurrence-fact count, drives root-cause escalation (mod.rs:1613).
- The lanes are decoupled (episode recall vs `record_occurrence` → append-only
  `store_fact`, mod.rs:1004-1043, **no upsert**). 2× sits in the dead zone: it
  promotes priority but never escalates a fix. A naïve single-counter fix is a
  trap (two independent counter lanes).

## 5. Member-token taxonomy — aggregate members, not signatures — CONFIRMED

Each token in the question maps to a distinct `Signal` variant folded into the
composite via its `classify_signal` dedup_key:

| Token | Signal | dedup_key site |
|---|---|---|
| `goal:blocked:<id>` | `GoalBlocked` | mod.rs:1336 |
| `workstream-gap` (fixed key) | `WorkstreamGap` | mod.rs:1371 |
| `resource:engineer_spawn` | `EngineerSpawnRate` | mod.rs:1270 |
| `overseer-obs:…` (nested) | `RecurringSignature` | mod.rs:1359 |

`workstream-gap` / `resource:engineer_spawn` appearing/leaving the composite is
**§11 membership drift** (which member goals are open that tick), NOT
signature-boundary noise or code drift.

## 6. Fix-landing status (D1/D2/D3) — ALL UNMERGED — CONFIRMED

grep of `src/overseer/` at HEAD:
- **D1** (exclude recall-derived `overseer-obs:` at the write boundary): only the
  construction site (mod.rs:1072) and a comment (mod.rs:440) exist — no exclusion.
- **D2** (count-in-content / idempotent upsert): no `count_in_content`,
  `upsert_fact`, or `occurrence_count` — `store_fact` remains append-only (mod.rs:1034).
- **D3** (gap-quarantine / launch edge for `workstream-gap`): no `quarantine` and
  no gap→`LaunchRecipe` edge — remains notify-only.

Consistent with `tertiary_architecture_VALIDATION_HEAD.md` (empty `*.rs` diff).

---

## Open questions / connections for synthesis
- Lane-A vs Lane-B decoupling means any remediation rung must be placed on the
  episode lane (first proven ×2), not the occurrence lane (≥3).
- D1 is the highest-leverage fix: excluding recall-derived `overseer-obs:*`
  dedup_keys at the write boundary breaks §2's loop at the source and also stops
  membership drift from re-nesting.
- Remediation (unblocking kgpacks/simard-identity goals) is out of scope; the
  blocks share one resourcing/convergence root cause and are merely co-aggregated.
