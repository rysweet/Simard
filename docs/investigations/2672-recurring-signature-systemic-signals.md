# Investigation Addendum — Systemic Signals & Phantom Workstreams behind the Recurring-Signature Loop (#2672 / #2678)

**Scope:** Investigation only (no runtime behaviour changed by this document).
**Code-verified against `origin/main`** (`893d52ed`), 2026-07-07. Every claim
below cites `file:line` on `main` and/or a `gh`-verified issue state.

This addendum completes the earlier consolidated investigation of the
"recurring signature seen 2×" self-amplification loop by closing its two open
gaps:

- **(A)** Individually root-cause the three *systemic* signals that co-occur in
  the signature — `quality:gym_skipped`, `workstream-gap`, and
  `resource:engineer_spawn` — with code citations.
- **(B)** Explicitly reframe the "blocked parity goals" — issues 16/17/18/21/22
  and the `advance-rysweet-agent-kgpacks-rs-to-full-parity` / WS1–WS7 names — as
  **phantom nested leaves** of the same self-amplification loop, so that the
  single de-nesting fix at `observation_signature` **is** the remediation, rather
  than any per-workstream unblocking.

The root cause of the *recurrence* itself (the self-amplifying `overseer-obs:`
write-back → recall → re-signal loop, unbraked across generations because each
generation mutates the dedup key) is established in the prior consolidated
findings and is only summarised here. This addendum focuses on the leaf
constituents that seed the loop and on why the "parity" framing is a phantom.

---

## Part A — The three systemic signals, root-caused individually

All three are **leaf problems** with **stable dedup keys**. None is a "blocker."
Each is lifted from an observed condition into a `Signal`, classified into a
`Problem` with a fixed `dedup_key`, and that key is then joined verbatim into the
Overseer's observation signature by `observation_signature`
(`src/overseer/mod.rs:1081-1086`):

```rust
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    format!("overseer-obs:{}", keys.join("|"))
}
```

So each leaf below contributes exactly one sorted, de-duplicated token to the
composite signature. That is the *only* way these three signals relate to the
"blocked goals": they are **co-tenant tokens in the same joined string**, not
causes of any blockage.

### A.1 `quality:gym_skipped` — Low-priority informational quality signal

| Stage | Site | Detail |
|-------|------|--------|
| Source of truth | `src/status/provider.rs:61` | `let gym_skipped = env_flag("SIMARD_SKIP_GYM");` — the flag is set by the deliberate `SIMARD_SKIP_GYM=1` fast-path (see `src/gym_runner_client.rs:46`, `is_skip_gym()`). Also honoured from `telemetry.gym_skipped`. |
| Projection | `src/overseer/sensor.rs:125-126` | `gym_skipped: gym.map(|g| g.skip_gym).unwrap_or(false) \|\| telemetry.map(\|t\| t.gym_skipped).unwrap_or(false)` |
| Emission | `src/overseer/signal.rs:398-400` | `if state.gym_skipped { out.push(Signal::GymSkipped); }` — **no threshold**; fires whenever the flag is set. |
| Classify | `src/overseer/mod.rs:1292-1297` | → `ProblemKind::QualityRegression`, **`Priority::Low`**, `dedup_key = "quality:gym_skipped"`. |
| Characterization | `src/overseer/tests_m1.rs:228` | `assert_eq!(problems[0].dedup_key, "quality:gym_skipped");` |

**Root cause of its presence:** a **by-design** operator/CI decision to skip the
gym self-eval (the `SIMARD_SKIP_GYM=1` fast path) is active. It is a `Priority::Low`
*informational* quality note — the gym did not run — **not** a blocked workstream.
It recurs every tick the flag is set and contributes the stable leaf token
`quality:gym_skipped` to the signature. **Remediation is process hygiene, not
unblocking:** confirm the skip is the intended CI/dev fast-path; if the gym
should run, unset `SIMARD_SKIP_GYM`. This has no bearing on the parity goals.

### A.2 `workstream-gap` — High-priority *coverage* signal with a constant key

| Stage | Site | Detail |
|-------|------|--------|
| Source of truth | `src/overseer/sensor.rs:288-370` | `detect_workstream_gaps(board, issues, anomalies, coverage)` surveys the whole board + issues + anomalies and returns uncovered high-signal items: p1/p2 goals (`priority <= GAP_GOAL_PRIORITY_BAR`, `=2` at `sensor.rs:249`) with no active engineer/PR, high-signal **open** issues with no PR, and live anomalies with no fix in flight. Total bounded by `MAX_GAPS_PER_TICK = 25` (`sensor.rs:244`). |
| Blocked goals **excluded** | `src/overseer/sensor.rs:300-302` | `if matches!(g.status, GoalProgress::Blocked(_)) { continue; }` — blocked goals flow through `goal_health`, so this signal is **orthogonal** to the `goal:blocked:*` leaves; it never re-flags them. |
| Projection | `src/overseer/sensor.rs:153`, `mod.rs:399-402`, `wiring.rs:750-772` | The read-only snapshot leaves it empty; the acting Overseer's Observe pass populates it via `GoalCurator::workstream_gaps`. |
| Emission | `src/overseer/signal.rs:475-479` | `if !state.workstream_gaps.is_empty() { out.push(Signal::WorkstreamGap { gaps }); }` — **ONE consolidated** signal per pass, never one-per-gap. |
| Classify | `src/overseer/mod.rs:1381-1386` | → `ProblemKind::WorkstreamCoverage`, **`Priority::High`**, `dedup_key = "workstream-gap"` — a **constant, evidence-independent** key regardless of *which* items are uncovered. |

**Root cause of its presence:** the coverage survey found **≥1 uncovered
high-signal item** on that tick. Because the dedup key is the *constant string*
`"workstream-gap"`, the token appears on **every** tick that any uncovered work
exists, and it does so **independently of the specific gap contents**. It is a
backlog-coverage indicator, not a per-workstream blocker. **Remediation:** read
the actual `GapItem` list (bounded, ≤25) to see the concrete uncovered
goal/issue/anomaly and cover it (assign an engineer / open a PR). Note this
signal explicitly does **not** cover the blocked "parity" goals — those are
skipped at `sensor.rs:300-302`.

### A.3 `resource:engineer_spawn` — Normal-priority resource-pressure signal

| Stage | Site | Detail |
|-------|------|--------|
| Source of truth | `src/overseer/sensor.rs:123` | `live_engineers: resources.and_then(\|r\| r.live_engineers)` — the count of concurrently live engineer processes. |
| Emission | `src/overseer/signal.rs:393-397` | `if let Some(live) = state.live_engineers && live >= ENGINEER_SPAWN_THRESHOLD { out.push(Signal::EngineerSpawnRate { live }); }` — threshold `ENGINEER_SPAWN_THRESHOLD = 8` (`signal.rs:351`). |
| Classify | `src/overseer/mod.rs:1280-1285` | → `ProblemKind::ResourcePressure`, **`Priority::Normal`**, `dedup_key = "resource:engineer_spawn"`. |
| Characterization | `src/overseer/signal.rs:731` | `Signal::EngineerSpawnRate { live: 12 }.describe()` names the live count. |

**Root cause of its presence:** **≥8 engineer processes are concurrently live.**
It is a `Priority::Normal` resource-pressure indicator. It is notably
*consistent with*, and partly *driven by*, the self-amplification loop itself:
each recurring generation re-promotes work (including the phantom `fix-agent-*`
goals in A/B below), which spawns more engineers, which keeps `live_engineers`
above the threshold — a secondary feedback edge. **Remediation:** the primary
fix is to stop the loop (Part C); secondarily, confirm the live-engineer count
against the threshold of 8 and reap orphaned engineer sessions if the count is
inflated by loop-spawned work.

### A.4 How the leaves combine into the loop (summary)

Per Observe pass, the leaf keys above — together with one
`goal:blocked:<goal_id>` key per blocked goal on the board
(`signal.rs:440-448` → `mod.rs:1337-1358`) — are sorted, de-duplicated, and
joined by `observation_signature` into a single
`overseer-obs:<key>|<key>|…` string. That composite is:

1. **written back** to cognitive memory, gated by `write_back_gate = WhisperGate::new(900, 5)` (`mod.rs:297`), embedded as `[sig:<signature>]` (`wiring.rs record_observation`);
2. **recalled** next tick and re-extracted verbatim as a `failure_signature` by `parse_failure_signature` (`wiring.rs:976-986`);
3. **re-signalled** as `Signal::RecurringSignature` once it recurs `>= RECURRING_SIGNATURE_THRESHOLD (=2)` times (`signal.rs:455-469`, threshold at `signal.rs:362`);
4. **classified** with `dedup_key = sanitize_recalled(signature)` (`mod.rs:1366-1376`) — i.e. the *whole prior signature becomes a single problem key*;
5. which then re-enters `observation_signature` next tick and gets **another `overseer-obs:` prefix** (`mod.rs:1081-1086`).

`sanitize_recalled` (`capabilities.rs:468-482`) strips control chars and caps
length but **preserves** `overseer-obs:`, `|`, `:` — so the nesting survives. No
de-nesting guard exists anywhere in `src/overseer`. That is the unbraked recurrence.

---

## Part B — The "blocked parity goals" are phantom nested leaves

The recurring-signature question framed the loop as if
`advance-rysweet-agent-kgpacks-rs-to-full-parity` and five child
`fix-agent` goals for **issues 16, 17, 18, 21, 22** (WS1 full-pack CVE, WS2
int8-PQ-embed, WS3 versioned-rel, WS6 resumable-pipeline, WS7 sign-the-release)
were real, blocked workstreams needing per-WS unblocking. **They are not.**

### B.1 Issues 16/17/18/21/22 are unrelated and already CLOSED

Verified via `gh issue view` on 2026-07-07 (repo `rysweet/Simard`):

| Issue | State | Actual subject (nothing to do with agent-kgpacks-rs parity) |
|-------|-------|-------------------------------------------------------------|
| #16 | **CLOSED** | Revise Simard runtime **documentation** per architect review |
| #17 | **CLOSED** | Validate/fix **pre-commit** state in a worktree |
| #18 | **CLOSED** | Recover base-type implementation onto a clean branch + QA + PR |
| #21 | **CLOSED** | Implement Simard **benchmark gym foundation** |
| #22 | **CLOSED** | Implement the **meeting/gym behavior block** against existing red tests |

None mentions `agent-kgpacks-rs`, "parity," or the WS-names. The mapping of the
Overseer's internal goal-store leaf IDs onto these GitHub issue numbers is a
**conflation**: the `goal:blocked:advance-…kgpacks-rs…` and
`goal:blocked:fix-agent-…` tokens are **internal goal-board leaves**, not the
GitHub issues 16–22.

### B.2 `agent-kgpacks-rs` is a *planned integration*, not blocked workstreams

In the tracked source, `agent-kgpacks` appears **only** as a *planned
knowledge-pack integration* — a Python subprocess bridge — never as blocked
"parity workstreams":

- `Specs/IMPLEMENTATION_PLAN.md:17` — "agent-kgpacks knowledge packs provide grounded domain knowledge"
- `Specs/IMPLEMENTATION_PLAN.md:27` — "BridgeTransport trait ──→ Python subprocess (agent-kgpacks + LadybugDB)"
- `Specs/IMPLEMENTATION_PLAN.md:381` — "| Agent-kgpacks integration | 2 | **Planned** |"

### B.3 WS1–WS7 leaf names exist ONLY inside the self-generated signature

The leaf names `full-pack CVE`, `int8-PQ-embed`, `versioned-rel`,
`resumable-pipeline`, `sign-the-release` appear **nowhere** in the repository's
source or specs. They exist only as **leaf tokens inside the Overseer's own
`overseer-obs:` signature** (nested in the self-filed titles of #2672 and #2678,
both of which are OPEN and are themselves "recurring signature seen 2×" issues —
i.e. sibling generations of the same loop).

### B.4 Conclusion: no per-workstream unblocking exists to perform

The `goal:blocked:advance-…kgpacks-rs…` and its `fix-agent-*` children are
**goal-store leaves generated by the Overseer's own planning**, surfaced as
`goal:blocked:*` dedup keys (`signal.rs:440-448` → `mod.rs:1337-1358`) and then
carried, generation after generation, inside the nesting composite. They "recur
as `goal:blocked`" for exactly the same reason the systemic signals do: the
composite that contains them is re-observed every tick because the loop never
de-nests. There is **no WS1–WS7 codebase to unblock**; the workstreams are
phantom.

---

## Part C — Remediation (prioritized)

Because the affected "workstreams" are phantom nested leaves (Part B) and the
three systemic signals are stable informational/coverage/resource leaves
(Part A), **the single de-nesting fix is the remediation** for the recurring
blocked-goal signature — it collapses the composite (blocked-goal leaves + the
three systemic leaves) to a stable fixed point so the existing
`write_back_gate` de-dups it permanently.

| Pri | Action | Site | Effect |
|-----|--------|------|--------|
| **P0** | De-nest at the prefix site: strip any existing `overseer-obs:` prefix chain from each `dedup_key` (and/or the joined result) **before** re-prefixing. | `src/overseer/mod.rs:1081` (`observation_signature`) | Generation N+1 == generation N ⇒ `write_back_gate` de-dups permanently ⇒ recurrence ends. Single root-cause fix. |
| **P1** | Flip the H1/H2 characterization tests to assert the prefix count is **idempotent (stays 1)** instead of incrementing. | `src/overseer/tests_memory_recall.rs` (H1 `:1126-1156`, nesting repro `:1186-1221`) | Locks in the fixed-point behaviour; turns the red characterization tests green as guards. |
| **P2** | Defense-in-depth: reject re-ingesting the Overseer's **own** `overseer-obs:` observations as a `failure_signature`. | `src/overseer/wiring.rs:976` (`parse_failure_signature`) and/or `capabilities.rs:468` (`sanitize_recalled`) | Breaks loop step 2 (recall re-extract) even if a nested signature ever slips through. |
| **P3** | Process hygiene for the systemic leaves (NOT blockers): confirm `SIMARD_SKIP_GYM` intent (A.1); read the bounded `GapItem` list and cover concrete gaps (A.2); reap orphaned engineers vs threshold `8` (A.3). | see Part A | Removes the informational/coverage noise once the loop is fixed. |

**Per the original goal's five "workstreams" (WS1 CVE, WS2 int8-PQ-embed, WS3
versioned-rel, WS6 resumable-pipeline, WS7 sign-the-release):** there is nothing
to unblock — they are phantom leaves. The P0 de-nesting fix removes the recurring
`goal:blocked:*` signature that made them *appear* to be recurring blocked
workstreams.

---

## Evidence index (all verified on `origin/main` @ `893d52ed`, 2026-07-07)

- `observation_signature` (join + re-prefix): `src/overseer/mod.rs:1081-1086`
- Classify → dedup keys: `resource:engineer_spawn` `mod.rs:1283`, `quality:gym_skipped` `mod.rs:1295`, `workstream-gap` `mod.rs:1384`, `goal:blocked:*` `mod.rs:1349`, `RecurringSignature` → `sanitize_recalled(signature)` `mod.rs:1372`
- Signal emissions: engineer `signal.rs:393-397`, gym `signal.rs:398-400`, workstream `signal.rs:475-479`, blocked goals `signal.rs:440-448`, recurring `signal.rs:455-469`
- Thresholds: `ENGINEER_SPAWN_THRESHOLD=8` `signal.rs:351`, `RECURRING_SIGNATURE_THRESHOLD=2` `signal.rs:362`, `GAP_GOAL_PRIORITY_BAR=2` `sensor.rs:249`, `MAX_GAPS_PER_TICK=25` `sensor.rs:244`
- Sources/projection: `SIMARD_SKIP_GYM` `provider.rs:61` / `gym_runner_client.rs:46`, sensor gym `sensor.rs:125-126`, sensor live_engineers `sensor.rs:123`, `detect_workstream_gaps` `sensor.rs:288-370` (blocked-goal exclusion `sensor.rs:300-302`)
- Loop closure: write-back gate `mod.rs:297`, `write_back_observation` `mod.rs:532`, recall re-extract `parse_failure_signature` `wiring.rs:976-986`, `sanitize_recalled` `capabilities.rs:468-482`
- Issue states (`gh`, 2026-07-07): #16/#17/#18/#21/#22 all **CLOSED** (unrelated); #2628 **CLOSED**; #2672 **OPEN**; #2678 **OPEN**; #2686 **CLOSED**
- `agent-kgpacks` = planned integration: `Specs/IMPLEMENTATION_PLAN.md:17,27,381`
