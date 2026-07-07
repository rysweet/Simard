# kgpacks-rs `goal:blocked` recurrence — per-issue quantification & the 17/21 differential

**Status:** Investigation addendum (Round 2). Resolves the two criteria left open by
Round 1 of the "advance-rysweet-agent-kgpacks-rs-to-full-parity" blocked-signature
investigation.

- **Criterion 1 (partial in R1):** per-issue recurrence frequency for issues 16/17/18/21/22 — now **quantified** from live runtime state.
- **Criterion 3 (unaddressed in R1):** *why* issues **17 (ws2 int8-pq-embed)** and **21 (ws6 resumable-pip)** recur across the most observation cycles — now **substantiated** with evidence.

Round 1's root cause (self-observation write-back feedback loop inflating a composite
anomaly signature) stands and is **reconciled** with the raw per-issue counts below.

---

## 0. Correction to a Round-1 limitation

Round 1 reported that "there is no live per-signature / goal-board occurrence store to
query." **That is incorrect.** Two authoritative runtime sources exist on this host and
were mined for this addendum:

1. **Live cognitive store** — `~/.simard/cognitive` (75 MB, lbug-backed, actively
   written; resolved by `live_store_path()` in `src/cognitive_memory/library_adapter.rs`,
   `LIVE_STORE_SUBDIR = "cognitive"`). Plaintext signature substrings are greppable via
   `strings`.
2. **OODA cycle reports** — `~/.simard/cycle_reports/cycle_*.json` (1,179 files). Each is
   one Observe→Decide pass with `observation.goals`, `priorities[] {goal_id, reason,
   urgency}`, `planned_actions`, `outcomes`, and `brain_judgments`.

The five workstreams map to concrete goal slugs (from `observation.goals`):

| Issue | Workstream | Overseer goal_id | GitHub state (agent-kgpacks-rs) |
|------:|-----------|------------------|---------------------------------|
| 16 | ws1 full-pack-cve   | `fix-agent-kgpacks-rs-issue-16-ws1-full-pack-cve-0c0ada69`  | CLOSED (refile #41 MERGED) |
| 17 | ws2 int8-pq-embed   | `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca`  | **OPEN** (refile #40 also **OPEN**) |
| 18 | ws3 versioned-rel   | `fix-agent-kgpacks-rs-issue-18-ws3-versioned-rel-67828479`  | CLOSED (refile #34 MERGED) |
| 21 | ws6 resumable-pip   | `fix-agent-kgpacks-rs-issue-21-ws6-resumable-pip-39ba30dc`  | CLOSED (refile #38 MERGED) |
| 22 | ws7 sign-the-release| `fix-agent-kgpacks-rs-issue-22-ws7-sign-the-rele-b59dde3e`  | CLOSED (refile #36 MERGED) |

---

## 1. Criterion 1 — per-issue recurrence, quantified (two independent measures)

### 1a. Raw Observe-pass recurrence (cycle reports)
Window `2026-07-06T12:19:27Z … 2026-07-07T08:14:20Z`. Each of the five appears in **36**
distinct cycles; blocked-cycle counts differ:

| Issue / ws | cycles present | **cycles blocked** | block rate |
|-----------|---------------:|-------------------:|-----------:|
| **17 / ws2 int8-pq-embed**  | 36 | **27** | 75% |
| **21 / ws6 resumable-pip**  | 36 | **21** | 58% |
| 16 / ws1 full-pack-cve      | 36 | 18 | 50% |
| 18 / ws3 versioned-rel      | 36 | 18 | 50% |
| 22 / ws7 sign-the-release   | 36 | 14 | 38% |

Ranked: **17 > 21 > 16 = 18 > 22**. The parent goal
`advance-…-to-full-parity-f29bb15c` is itself blocked in 28+ cycles (safeguard trips).

### 1b. Persisted `goal:blocked` occurrences (live cognitive store)
Count of the bundled `goal:blocked:fix-agent-kgpacks-rs-issue-NN-wsX` substring inside
`~/.simard/cognitive`:

| Issue / ws | stored `goal:blocked` occurrences |
|-----------|----------------------------------:|
| **17 / ws2** | **3,537** |
| 16 / ws1 | 3,278 |
| 18 / ws3 | 459 |
| **21 / ws6** | 430 |
| 22 / ws7 | 179 |
| (19 / ws4) | 46 |

Both measures independently put **#17 at the top**. The store magnitudes (thousands) vastly
exceed the raw cycle counts (tens) — that gap is the write-back amplification (§3).

---

## 2. Criterion 3 — why 17 and 21 recur across the most cycles

**Root differential: #17 and #21 are the only two workstreams with a *genuine unmet
upstream blocker*. The other three had none, so they shipped (via refiles) and stopped
recurring.**

### #17 / ws2 (int8-pq-embed) — blocked on an unpassable quality gate
- Issue title is explicit: *"WS2: int8/PQ embedding quantization spike, **gated on eval
  recall parity**."* Acceptance depends on an eval/gym recall-parity check.
- **The gym is empty in every cycle.** `observation.gym_health` =
  `{"overall":0.0, "pass_rate":0.0, "scenario_count":0}` across **all 128** kgpacks cycles.
  A gate that requires eval-recall parity can never pass when there are 0 scenarios and a
  0.0 pass-rate — this is the concrete `quality:gym_skipped` co-signal.
- Consequence in the block reasons: **22 of 27** blocked cycles for #17 are *not* safeguard
  trips but *"Cycle N, skip_count 0, failure_count 0, worktree active ~Nm ago — the
  engineer is healthy, not wedged, and not churning"* — i.e. a **long-running engineer that
  stays alive but never ships** because its gate is unmeetable. The other 5 are safeguard
  trips. This is why #17 has the highest recurrence and is still **OPEN** (both #17 and its
  refile #40).

### #21 / ws6 (resumable-pip) — blocked on an external dependency (#25)
- The single comment on #21 states: *"WS6 (#21) **blocked on #25** — no code changes made
  this cycle,"* because **#25 (external CVE corpus fetch from CVEProject/cvelistV5)** had not
  landed on `main`. WS6's builder consumes that corpus, so preflight found the dependency
  unmet each cycle and wrote no code.
- Timeline: #25 created `2026-07-03T01:20Z`, closed `2026-07-06T14:12Z`; **#21 was itself
  closed `2026-07-06T13:29Z` — i.e. before #25 merged** (closed-as-blocked; the actual
  build later landed via refile #38).
- Consequence: **all 21** of #21's blocked cycles are `🔒 [OODA-SAFEGUARD] … 3 consecutive
  no-action cycles`. Each cycle the engineer preflighted, found #25 missing, took no action
  → the safeguard tripped → repeat. This is the `workstream-gap` co-signal (the dependency
  #25 was itself an unstaffed workstream) coupled with repeated `resource:engineer_spawn`.

### Why 16 / 18 / 22 recur *less*
None carried an unmet blocker. Once an engineer was spawned they produced a mergeable PR
(refiles **#41 / #34 / #36**, all MERGED, closing the originals). Their blocked cycles are
only the safeguard cool-down between spawns (14–18), and they fall out of the board once
closed.

### Block-reason fingerprint (the differential in one table)
| Issue | dominant block reason | interpretation |
|------:|-----------------------|----------------|
| **17** | 22× "engineer healthy, worktree active" + 5× safeguard | live engineer parked on an **unpassable eval gate** |
| **21** | 21× `[OODA-SAFEGUARD]` no-action | preflight-blocked on **missing dependency #25** |
| 16/18/22 | 14–18× `[OODA-SAFEGUARD]` no-action | cool-down only; shipped via refiles |

---

## 3. Reconciling with Round 1's `occurrences == 2`

The persisted anomaly signature is a **single composite string** that concatenates the
parent and every child block, prefixed by the distill anomaly:

```
overseer-obs:anomaly:distill parse-fail rate 100%
  |goal:blocked:advance-rysweet-agent-kgpacks-rs-to-full-parity-f29bb15c
  |goal:blocked:fix-agent-kgpacks-rs-issue-12-parity-decision-…
  |goal:blocked:fix-agent-kgpacks-rs-issue-16-ws1-full-pack-cve-…
  |goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-…
  |goal:blocked:fix-agent-kgpacks-rs-issue-18-ws3-versioned-rel-…
  |overseer-obs:anomaly:distill parse-fail rate 100%|goal:blocked:…   ← embeds itself
```

Two consequences, both confirmed on disk:
1. **Per-issue recurrence is flattened** into one composite. Distillation dedups the
   composite to a small episode count (`occurrences == 2`), which is why Round 1 could not
   read per-issue frequency from the *distilled* layer — it must be read from the *raw*
   layers (§1), not the consolidated episode.
2. **The signature embeds itself** (`…|overseer-obs:anomaly:…|goal:blocked:…`). Each Observe
   pass re-persists a signature that contains all prior `goal:blocked` substrings, so the
   substring counts explode (27 real blocked cycles → 3,537 stored occurrences for #17).
   This is the Round-1 write-back feedback loop, now measurable.

Note the composite is also gated behind **`distill parse-fail rate 100%`** — the distill
parser was failing to parse the observation payloads (the trailing-comma / JSON-recovery
work on the current branch, commits `b0deb332` / `56a2b8e6`), so the anomaly never cleanly
consolidated and kept re-triggering.

---

## 4. Prioritized remediation (targeted at what recurs)

1. **Unblock #17 by fixing the gym, not the engineer (highest impact).** The eval-recall
   gate is unmeetable while `gym_health.scenario_count == 0`. Either (a) seed real eval
   scenarios so the parity gate can evaluate, or (b) make the ws2 acceptance criterion not
   hard-gate on a gym that reports 0 scenarios. Until then #17 will keep a live engineer
   parked every cycle (top recurrer, still OPEN with refile #40).
2. **Enforce dependency-gating before engineer spawn (fixes the #21 pattern).** A workstream
   with an unmet hard dependency (#21→#25) should be *deferred/parked* rather than spawned
   into a no-action cycle that trips the safeguard. Gate spawn on dependency readiness; when
   deferred, don't count it as a fresh `goal:blocked` observation.
3. **Break the write-back self-embedding.** Exclude prior `overseer-obs:anomaly:*` /
   `goal:blocked:*` substrings from the material folded into a *new* anomaly signature
   (don't let a signature contain itself). This collapses the 3,537-vs-27 amplification and
   stops the signature from recurring purely by re-observation.
4. **Fix distill parse-fail first.** The `distill parse-fail rate 100%` prefix means the
   consolidation path is erroring on every payload; land the JSON-recovery fix so anomalies
   consolidate once instead of re-triggering.

---

## 5. Evidence index

- Live store: `~/.simard/cognitive`; path resolver `src/cognitive_memory/library_adapter.rs`
  (`live_store_path`, `LIVE_STORE_SUBDIR`).
- Cycle reports: `~/.simard/cycle_reports/cycle_*.json` (1,179 files), fields
  `priorities[].reason`, `observation.gym_health`, `observation.goals`.
- Overseer signature/dedup: `src/overseer/observer.rs`, `src/overseer/sensor.rs`
  (`blocked_goals_from_board`, `detect_workstream_gaps`, `gym_skipped`),
  `src/stewardship/dedup.rs` (`failure_signature`).
- GitHub (rysweet/agent-kgpacks-rs): issues #16 (CLOSED), #17 (OPEN), #18/#21/#22 (CLOSED);
  refiles #41/#40/#34/#38/#36; dependency #25 (CLOSED 2026-07-06T14:12Z); #21 comment
  "blocked on #25".
- Simard repo state: `origin/main` (branch `investigate/kgpacks-17-21-recurrence`).
