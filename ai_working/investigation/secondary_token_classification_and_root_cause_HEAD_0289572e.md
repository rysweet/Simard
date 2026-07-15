# Secondary Investigation — Token-Group Classification & Common-vs-Independent Root Cause

**Role:** SECONDARY (pattern / token taxonomy / common-root-cause)
**HEAD:** `0289572e` (strategy target). `git diff --name-only <388e6c29|85b9398a|dea65df8>..HEAD -- '*.rs'`
is **empty** — every `.rs` file is byte-identical to the bases of all 5 prior waves, so
prior line citations are exact. Re-verified live below.
**Verdict:** All prior secondary conclusions HOLD unchanged. No drift.

---

## 1. Bottom line

The three member token groups are **not three separate signatures** — they are
**co-aggregated members of one composite** `observation_signature` (mod.rs:1068-1073:
`overseer-obs:` + sorted/deduped `dedup_key` join). Each is a **stable literal or
stable ID** whose volatile field is confined to the human summary and never reaches
the signature. The blocked goals are **NOT independent bugs**: they share **one
upstream root cause** (a persistent observe-and-flag loop with no closing/convergence
rung), surfaced through three lenses. The `resource:engineer_spawn` + `workstream-gap`
members are **benign §11 membership drift**, not signature-boundary noise.

---

## 2. Token-group taxonomy (detection trigger → dedup_key → volatile field)

Each row re-verified at HEAD:

| Token group | Signal variant | dedup_key mint | ProblemKind / Priority | Volatile field | Lands in signature? |
|---|---|---|---|---|---|
| `goal:blocked:<goal_id>` | `Signal::GoalBlocked` | mod.rs:1336 `format!("goal:blocked:{goal_id}")` | GoalHygiene / High if `needs_review` else Normal (1330-1335) | `consecutive_no_action`, `needs_review` (summary/priority only, 1337-1344) | goal_id is a **stable ID** ✅ |
| `workstream-gap` | `Signal::WorkstreamGap{gaps}` | mod.rs:1371 fixed literal `"workstream-gap"` | WorkstreamCoverage / High (1369-1370) | `gaps.len()` (summary only, 1372) | fixed literal ✅ |
| `resource:engineer_spawn` | `Signal::EngineerSpawnRate{live}` | mod.rs:1270 fixed literal `"resource:engineer_spawn"` | ResourcePressure / Normal (1268-1269) | `{live}` (summary only, 1271) | fixed literal ✅ |
| `overseer-obs:...` (nested) | `Signal::RecurringSignature{signature,occurrences}` | mod.rs:1359 `sanitize_recalled(signature)` | ProcessHealth / High (1357-1358) | `occurrences` (summary only, 1360-1362) | sanitized recalled signature ✅ |

**DRIFT re-check on `resource:engineer_spawn`:** RESOLVED at HEAD. Despite being a
live-count signal, the `{live}` count appears **only** in the summary
`"elevated engineer spawn ({live} live)"` (mod.rs:1271); the `dedup_key` is the fixed
string. Structurally identical to `goal:blocked` (count in summary) and `workstream-gap`
(`gaps.len()` in summary). **No volatile component leaks into the signature.** So the
signature is a deterministic function of the *membership set* only — different membership
⇒ different signature ⇒ both legitimately recorded (correct, not a dedup miss).

**Key provenance note:** the investigation-question string
`"recurring signature seen 2× in cognitive memory (overseer-obs:...)"` is emitted
**verbatim** by mod.rs:1360-1362 (the `RecurringSignature` summary). This is
*direct proof* that the signature under investigation is the overseer's **own recall
write-back**, not a raw user/memory key.

---

## 3. Are these separate signatures? — NO (aggregate members)

- `orient` merges all same-`dedup_key` problems in a pass (mod.rs:1211), and
  `observation_signature`'s `keys.dedup()` (mod.rs:1071) collapses adjacent equals.
  ⇒ Each family key appears **at most once per snapshot**.
- Therefore any `workstream-gap|workstream-gap` (or repeated `overseer-obs:`) *within one
  aggregate* can only come from **nested recalled `overseer-obs:...` tokens** (each carries
  its own embedded `workstream-gap`), which are distinct strings that survive `dedup()`.
- The `resource:engineer_spawn` and extra `workstream-gap` are **membership drift** between
  the two overlapping snapshots (APPEAR/GROW), not a per-token counting bug. This is the
  §11 (benign membership drift) classification, not signature-boundary noise.

---

## 4. Common root cause vs independent blocks — COMMON (one lever, four surfaces)

The pattern question is not "why does the fingerprint repeat" but **"why does the problem
set never change."** Verified per-goal cause map (reconciles with CONSOLIDATED §4/§5a):

| Cluster (aggregate members) | Stall class | Genuine block? |
|---|---|---|
| kgpacks-rs parity + issues #12/#17/#18/#23/#25 | (a) **false-park** `AlreadyComplete`/`MissingPrecondition` — work already CLOSED/MERGED misread as stuck | NO |
| audit Simard coverage → 70% | (b) **uncheckable done-gate** `UnclearCriteria` — idles, re-parks | NO |
| simard-identity personas (atelier/bursar/cartographer/concierge/gastronome) | (c) **starvation** `GoalUncovered` — p1/p2, no assignee/workstream | NO (resourcing) |
| coin benchmark harness | **genuine dependency** `MissingPrecondition`/`UpstreamDependency` | YES (1 of 4) |

Three of four are *not* genuine blocks. All funnel through **one shared mechanism**: the
bare no-progress park with no `WHY` token. The corrective vocabulary exists
(`NoProgressClass` + `resolution_for_why`) but is **double-gated and fails open to
bare-park** (ooda_loop/cycle.rs WHY reasoner): completion-evidence gate + feature-flag
gate, with **no invariant** tying a `Blocked` reason to a `NoProgressClass`. When the WHY
reasoner is unwired/misclassifies, every stall class collapses to the same bare re-park →
the recurring `goal:blocked` population. **One unwired classification rung, not five goal
bugs.**

⇒ The blocks are **causally linked (common root cause)**, merely co-aggregated into one
signal — with one exception (coin harness) that is an *independently* genuine dependency
block that happens to co-occur. The aggregate signal therefore mixes one real block into a
majority of non-genuine re-parks.

---

## 5. Unifying pattern — "Two signatures, one root problem"

`workstream-gap` and `goal:blocked` are **one under-resourced entity oscillating between
two views**, re-verified at HEAD:

```
   active, no workstream ──breaker parks──▶  Blocked
        │ emits WorkstreamGap                     │ emits GoalBlocked
        ▼ (GoalUncovered, sensor.rs:311)          ▼ (goal_health)
   workstream-gap  ◀──unblock/reactivate──   goal:blocked
```

Confirmed at sensor.rs:300-302 — `detect_workstream_gaps` **explicitly skips
`GoalProgress::Blocked`** goals (routed via `goal_health` to avoid double-notify). So the
same entity is `workstream-gap` while active-uncovered and `goal:blocked` while parked —
never both simultaneously, but both across windows. Treat gap+blocked for one entity as
**one resourcing/convergence problem**, not two bugs.

---

## 6. Why nothing closes — verified closing-action edges

| ProblemKind | Decide arm | Closing action? |
|---|---|---|
| `WorkstreamCoverage` (workstream-gap) | `FlagWorkstreamGaps` (mod.rs:1534-1543) | **NOTIFY ONLY** — no `LaunchRecipe`/issue edge; the only High-priority arm without one |
| `GoalHygiene` (goal:blocked) | `decide_blocked_goal` (mod.rs:1447-1482) | escalates ROOT CAUSE only at `recurrence ≥ 3` (mod.rs:1613); else self-heal/re-unblock → re-parks |
| `ResourcePressure` (engineer_spawn) | `Escalate` (mod.rs:1444), Normal priority | escalates but does not remediate the throughput deficit |
| `ProcessHealth` (recall meta) | `LaunchRecipe` (mod.rs:1429) | launches — but on the recall meta-problem, feeding the self-observation loop |

**Recurrence dead zone (verified thresholds):** emit at `RECURRING_SIGNATURE_THRESHOLD = 2`
(signal.rs:362, gate at 463) vs escalate at `RECURRENCE_ESCALATION_THRESHOLD = 3`
(root_cause.rs:33, gate at mod.rs:1613). The observed **`2×` sits in the `[2,3)` gap** —
flagged forever, escalated never. Coverage gaps have **no** cross-window escalation at all
(their gate forgets across windows).

---

## 7. Concerns / integration points

- **`resource:engineer_spawn` is the diagnostic tell of the common root cause**, not an
  incidental member: the system *is* spawning engineers (spawn rate up) yet goals stay
  blocked and gaps stay uncovered — an under-throughput signature, three views.
- **Bare `"workstream-gap"` family key destroys per-gap identity** (mod.rs:1371): every
  persona gap is indistinguishable at the write boundary, so gap counts can't be attributed
  (contrast the per-gap gate key `workstream-gap:{sig}` at mod.rs:901).
- **True lever (root):** a remediation/escalation rung at *first proven recurrence (2×)*
  for the `goal:blocked` + `workstream-gap` + `engineer_spawn` convergence — plus wiring the
  WHY-reasoner ladder (P0). Symptom seam (nested `overseer-obs:` growth): exclude
  recall-derived `RecurringSignature` keys from write-back. **Do NOT** touch the
  counter — the `2×` is an honest ratchet (see §8).

---

## 8. Meta-conclusion (secondary verdict)

**The recurrence count is honest — audit the closing action, not the counter.** The three
token groups are aggregate members of one deterministic, within-window-deduped fingerprint.
The blocks share **one common root cause** (an unwired convergence rung sitting in a
recurrence dead zone), with the coin-harness the lone independently-genuine dependency
block co-aggregated in. `workstream-gap` and `resource:engineer_spawn` are benign
membership drift, not separate defects.

---

## Questions for verification phase

1. Confirm `simard-identity-*` goals genuinely transitioned to unblocked (goal board)
   between the two snapshots (DROP A→B) vs. merely dropping out of recall ranking.
2. Confirm `resource:engineer_spawn` fired from real elevated live-spawn telemetry at
   snapshot B (belongs in the convergence class) vs. a one-off spike (incidental).
3. Confirm `completion_evidence` (WHY Gate A) is actually `None` in the live daemon —
   determines whether the WHY ladder ever ran for the kgpacks cluster.
4. Confirm the escalation DECISION latches at recurrence ≥ 3 (`blocked_goal_gate`) and never
   un-latches — relevant to whether convergence is reachable once a goal crosses the bar.
