# Recurring `blocked` Signature on `advance-…-kgpacks-rs-to-full-parity` — Consolidated Diagnosis

**Type:** Investigation only (no runtime behaviour changed by this document).
**Goal:** Diagnose the root cause of the recurring `blocked` signature on the
`advance-rysweet-agent-kgpacks-rs-to-full-parity` goal and its child fix-agent
issues (WS1/WS2/WS3/WS6/WS7 = #16/#17/#18/#21/#22) and identify what is needed
to unblock them.

This round consolidates and **reconciles** two prior investigation framings
that were left on unmerged sibling branches, closes the three success criteria
that were not verifiably delivered in Round 1 (C3 recurrence differential, C4
per-workstream unblock, C5 issue status + parity gap), and persists a single
reachable report.

All issue states verified **2026-07-07** via `gh issue view … --repo
rysweet/agent-kgpacks-rs`. All code citations verified against `origin/main`
(`src/overseer/*`). Prior framings referenced:

- `docs/investigations/2672-recurring-signature-systemic-signals.md`
  (branch `investigate/2672-systemic-signals-phantom-workstreams`) — the
  **mechanism** framing (self-amplification nesting loop) + a *phantom-workstream*
  reframe.
- `ai_working/investigation/kgpacks-rs-parity-block-remediation.md`
  (branch `investigate/kgpacks-rs-parity-remediation`) — the **domain-seed**
  framing (missing goal↔issue-closure reconciliation) + per-workstream plan.

---

## TL;DR

The recurring `blocked` signature has **two stacked root causes at two layers**,
and both must be understood together:

1. **Mechanism (why the signature recurs / "seen 2×" and nests):** a
   self-amplifying cognitive-memory loop at `observation_signature`
   (`src/overseer/mod.rs:1081-1086`). Each Observe pass joins problem dedup keys
   into `overseer-obs:<k1>|<k2>|…`, writes it back to memory, recalls it next
   tick, re-classifies the *whole prior signature* as one new key, and
   **re-prefixes** it — with no de-nesting guard. The signature therefore
   mutates every generation, defeats the write-back dedup gate, and recurs.

2. **Domain seed (why `goal:blocked:*` leaves feed that loop):** the Overseer
   goal board holds five `fix-agent-kgpacks-rs-issue-NN` goals as `Blocked`, and
   there is **no reconciliation between goal-board `Blocked` state and the
   linked GitHub issue being CLOSED/COMPLETED** (`decide_blocked_goal`,
   `mod.rs:1624-1666`, has no issue-closure path). Four of the five workstreams
   are already **CLOSED/COMPLETED** on `rysweet/agent-kgpacks-rs`, yet their
   goals stay parked `Blocked` and keep emitting `goal:blocked:…issue-NN` leaves
   into the composite every tick.

**Net:** the workstreams are **not phantom** (Round 1's earlier "phantom"
reading checked the wrong repository — see Reconciliation §5). They are **real,
and ~80% already done.** Only **#17 (WS2 int8/PQ embed)** is genuinely OPEN, and
its one gate (the WS1 eval harness, #16) is already satisfied. The remediation is
therefore: (P0) de-nest the signature to stop the recurrence mechanism; (P1)
reconcile the four stale `Blocked` parks against their CLOSED issues; (P2)
dispatch the single real remaining task (#17).

---

## Success-criterion coverage

| # | Criterion | Status | Where answered |
|---|-----------|--------|----------------|
| C1 | Why the goal + children repeatedly enter `blocked` (signature 2×) | **Met** | §1 (mechanism) |
| C2 | Shared blocking signals `gym_skipped`, `workstream-gap`, `engineer_spawn` | **Met** | §2 |
| C3 | Why #17 (ws2) & #21 (ws6) recur most (3×) vs others (1×) | **Met (new)** | §3 |
| C4 | Concrete unblock recommendations per workstream ws1/2/3/6/7 | **Met (new)** | §4 |
| C5 | Status of issues #16/17/18/21/22 + kgpacks-rs parity gap | **Met (new)** | §5 |

---

## 1. C1 — Why the signature recurs (mechanism, summary)

The recurrence is a **self-amplification loop** in Overseer cognitive memory,
not a real re-blocking event each cycle. Per Observe pass:

1. Each detected `Problem` carries a stable `dedup_key`. `observation_signature`
   sorts + dedups the keys and joins them: `overseer-obs:<k>|<k>|…`
   (`src/overseer/mod.rs:1081-1086`).
2. The composite is **written back** to memory (gated by
   `write_back_gate = WhisperGate::new(900, 5)`, `mod.rs:297`), embedded as
   `[sig:<signature>]`.
3. Next tick it is **recalled** and re-extracted verbatim by
   `parse_failure_signature` (`wiring.rs:976-986`).
4. Once it recurs `>= RECURRING_SIGNATURE_THRESHOLD (=2)` it becomes
   `Signal::RecurringSignature` (`signal.rs:455-469`, threshold `signal.rs:362`)
   — **this is the "seen 2×"**.
5. It is classified with `dedup_key = sanitize_recalled(signature)`
   (`mod.rs:1366-1376`); `sanitize_recalled` (`capabilities.rs:468-482`)
   preserves `overseer-obs:`, `|`, `:`, so the **whole prior signature becomes a
   single key** and re-enters `observation_signature`, gaining **another**
   `overseer-obs:` prefix.

No de-nesting guard exists anywhere in `src/overseer`. Because the key mutates
every generation, the `write_back_gate` never recognises it as a duplicate, so
the loop is **unbraked**. That is why the `blocked` composite (which includes the
five `goal:blocked:…kgpacks…` leaves) is re-observed and re-emitted indefinitely.

---

## 2. C2 — The three shared systemic signals (summary, code-verified)

All three are **co-tenant leaf tokens** in the joined signature — not causes of
any blockage. Each is an observed condition → `Signal` → `Problem` with a fixed
`dedup_key`, joined verbatim by `observation_signature`.

| Signal | Priority / Kind | Trigger | Key sites | Meaning |
|--------|-----------------|---------|-----------|---------|
| `quality:gym_skipped` | Low / QualityRegression | `SIMARD_SKIP_GYM=1` fast-path set; **no threshold** | source `provider.rs:61`; emit `signal.rs:398-400`; classify `mod.rs:1292-1297` | By-design skip of the gym self-eval. Informational, recurs every tick the flag is set. |
| `workstream-gap` | High / WorkstreamCoverage | ≥1 uncovered high-signal item; **constant key** regardless of contents | detect `sensor.rs:288-370`; emit `signal.rs:475-479`; classify `mod.rs:1381-1386` | Backlog-coverage indicator. **Explicitly excludes blocked goals** (`sensor.rs:300-302`) → orthogonal to the kgpacks parity leaves. |
| `resource:engineer_spawn` | Normal / ResourcePressure | `live_engineers >= ENGINEER_SPAWN_THRESHOLD (=8)` | source `sensor.rs:123`; emit `signal.rs:393-397`; classify `mod.rs:1280-1285` | Resource pressure. Partly a **symptom** of the loop: re-promoted/re-dispatched work spawns engineers (`root_cause.rs:326-336` "stuck workstreams"), a secondary feedback edge. |

**How they contribute to the recurrence:** they are stable leaves with fixed
dedup keys, so they appear in **every** composite signature. They add persistent
tokens that (a) keep the composite non-empty and (b) — for `engineer_spawn` —
are partially *driven by* the loop's re-dispatch of the parity goals, tightening
the feedback. `workstream-gap` is provably **disjoint** from the parity children
(blocked goals skipped at `sensor.rs:300-302`); treat it as ordinary backlog
triage, not part of the kgpacks block.

---

## 3. C3 — Why #17 (WS2) and #21 (WS6) recur 3× while the others recur 1×

The per-leaf recurrence count = **how many Observe passes emitted that specific
`goal:blocked:…issue-NN` leaf while the goal sat `Blocked`**. Two independent,
evidence-grounded factors explain why #17 and #21 dominate:

### 3a. #17 (int8/PQ embed) — the only *structurally non-terminating* leaf

#17 is the **single genuinely OPEN** issue (verified §5): a spike **gated on the
WS1 eval-recall parity harness**. Because it never transitions to
CLOSED/COMPLETED, its `goal:blocked:…issue-17` leaf is re-emitted on **every**
Observe pass and survives **every** generation of the nesting loop. It therefore
accumulates the **highest steady-state recurrence** — it is the one leaf that
cannot self-terminate even after the infra fixes landed. Its recurrence is
*structural* (open + gated), not incidental.

### 3b. #21 (resumable-pipelined build) — the largest-context, deadlock-prone leaf

#21 is the **largest-context, longest-running** workstream (checkpoint/resume
sidecar + `embed‖load` pipelining over the full CVE corpus). It is precisely the
class of goal that repeatedly tripped the **two cluster-wide infra incidents**,
each re-dispatch adding another blocked re-observation before it finally closed
(07-06 13:29Z):

- **E2BIG / exit-126 argv overflow** — `docs/concepts/self-diagnose-on-step-error.md:57-88`
  states the incident was *"most reproducibly the **large-context**
  `agent-kgpacks-rs` workstream goals"*: large accumulated OODA context pushed
  the inlined prompt past `ARG_MAX`, so `execve` failed with `E2BIG` **before any
  work**, and "the daemon then re-dispatched the same goal … and repeated." Each
  such re-dispatch re-emitted the `goal:blocked:…issue-21` leaf.
- **Engineer-claim-sentinel dispatch deadlock** —
  `docs/reference/engineer-claim-sentinel-exclusion.md:52-58` names the blocked
  set `#12/#16/#17/#18/#19/#20/#21` and describes an *"infinite
  engineer-dispatch loop"* that "racked up consecutive-failure demotions on the
  goal board." The heaviest goals cycled through this loop the most times.

### 3c. Why the others (#16/#18/#22) recur only 1×

WS1 (#16), WS3 (#18) and WS7 (#22) are **smaller-context** and **closed earlier**
(#18 10:33Z, #22 12:07Z, #16 20:16Z on 07-06) with fewer blocked re-dispatch
cycles, so each contributed roughly a single parked `goal:blocked` leaf before
its issue closed. WS4 (#19) and WS5 (#20) dropped out of the composite entirely
after closing earlier — consistent with these being **point-in-time board parks
that lag issue closure** rather than persistent blockers.

**Summary:** #17 recurs most because it is **genuinely open + gated** (structural
non-termination); #21 recurs most because it is the **heaviest goal** that most
reproducibly hit the two infra deadlocks (re-dispatch churn). The others closed
early/cheaply → single park each.

---

## 4. C4 — Concrete unblock recommendations per workstream

Priority-ordered. **P0 stops the recurrence mechanism; P1 clears the 4 stale
parks; P2 is the single real remaining task.** Investigation-only — these are
recommendations, not applied changes.

### P0 — De-nest the signature (stops the recurrence for *all* leaves)
- **Site:** `src/overseer/mod.rs:1081` (`observation_signature`). Strip any
  existing `overseer-obs:` prefix chain from each `dedup_key` (and/or the joined
  result) **before** re-prefixing, so generation N+1 == generation N and the
  `write_back_gate` dedups permanently. Defense-in-depth: reject re-ingesting the
  Overseer's own `overseer-obs:` observations as a `failure_signature`
  (`wiring.rs:976` / `capabilities.rs:468`). This is a **development** task
  (needs full workflow + tests), tracked, not done here.

### P1 — Reconcile the 4 stale parks against CLOSED issues (unblocks WS1/WS3/WS6/WS7)
- **Operational:** mark these four goals Done on the board so they stop being
  re-parked and re-observed:
  `fix-agent-kgpacks-rs-issue-16-ws1-full-pack-cve`,
  `…-issue-18-ws3-versioned-rel`, `…-issue-21-ws6-resumable-pip`,
  `…-issue-22-ws7-sign-the-release`.
- **Verify first:**
  ```bash
  for n in 16 18 21 22; do
    gh issue view "$n" --repo rysweet/agent-kgpacks-rs \
      --json number,state,stateReason,closedAt \
      -q '"#\(.number) \(.state)/\(.stateReason) @ \(.closedAt)"'
  done   # expected: all CLOSED/COMPLETED
  ```
- **Root-cause fix (follow-up dev round):** add a **goal↔issue-closure
  reconciler** — a `Blocked` goal whose embedded `issue-NN` is CLOSED/COMPLETED
  should transition to `Done` (or route to `TransferGoal … for closure`) instead
  of re-parking. Seam: extend `decide_blocked_goal` (`mod.rs:1624-1666`) or the
  `blocked_goals_from_board` projection (`sensor.rs:204-221`) with an
  issue-state check keyed on the goal's `…issue-NN-…` number.

### P2 — Dispatch the one real remaining workstream: WS2 / #17 (int8-PQ embed)
The gate (WS1 eval harness, #16) is satisfied as of 07-06 20:16Z. Steps:
1. Record the cleared gate on #17 (`gh issue comment 17 --repo
   rysweet/agent-kgpacks-rs …`), then assign/re-enqueue (no longer
   dependency-blocked — it is *stale-blocked*, i.e. unclaimed).
2. Implement in crate `kgpacks-embeddings`: `quantize_int8(&[f32]) -> (Vec<i8>,
   f32)` with `scale = max(|v|)/127` (all-zero → scale 0, no NaN);
   `dequantize_int8(&[i8], f32) -> Vec<f32>` bound-checked; round-trip error ≤
   scale/element; cosine > 0.999 on L2-normalised vectors; additive pack format.
3. Run the **WS1 eval harness** on a quantized pack; ship behind a flag **only
   if** `delta_accuracy >= -0.02` **and** hit@k parity holds — otherwise land it
   DISABLED + a spike report (both outcomes satisfy the issue). Green
   `cargo test`/`clippy`/`fmt`, then close #17.

### P3 — Confirm infra fixes are live on the kgpacks runner (prevent recurrence)
Before re-dispatching #17, re-verify on the executing runner: the
engineer-claim-sentinel exclusion (`engineer-claim-sentinel-exclusion.md`) and
the file-backed prompt transport for the E2BIG overflow
(`self-diagnose-on-step-error.md`). Round 1 observed the launcher already using
file-backed transport; confirm on the kgpacks runner specifically.

### P4 — Clear the co-observed quality signals (after P1/P2)
- `quality:gym_skipped` — confirm `SIMARD_SKIP_GYM` is the intended CI/dev
  fast-path; re-enable gym self-eval if quality should be measured once #17 lands.
- `resource:engineer_spawn` — a symptom; should subside once P1 reconciles the
  stale parks. **Verify** the live-engineer count drops below 8 afterward; reap
  orphaned sessions if inflated.
- `workstream-gap` — disjoint from the parity children; ordinary backlog triage.

---

## 5. C5 — Issue status & kgpacks-rs parity gap (verified) + reconciliation

### 5a. Verified status (repo `rysweet/agent-kgpacks-rs`, 2026-07-07)

| Issue | WS  | Title (short)                         | GitHub state | Real status |
|-------|-----|---------------------------------------|--------------|-------------|
| #16 | WS1 | Full-pack CVE eval validation          | **CLOSED/COMPLETED** 07-06 20:16Z | Done — stale board park |
| #17 | WS2 | int8/PQ embedding quantization spike   | **OPEN** (0 comments, 0 assignees) | **Only genuinely open item; gate satisfied** |
| #18 | WS3 | Versioned release tags + provenance    | **CLOSED/COMPLETED** 07-06 10:33Z | Done — stale park |
| #21 | WS6 | Resumable + pipelined CVE pack build   | **CLOSED/COMPLETED** 07-06 13:29Z | Done — stale park |
| #22 | WS7 | Sign the release index (Ed25519)       | **CLOSED/COMPLETED** 07-06 12:07Z | Done — stale park |

**Parity gap of the `agent-kgpacks-rs` crate:** effectively closed. The crate is
the Rust port of `agent-kgpacks` (LadybugDB graph + vector/FTS retrieval,
graph-RAG). Its milestones M1–M5 and WS1/WS3/WS4/WS5/WS6/WS7/WS8 are CLOSED; the
remaining open items are **#17** (WS2 int8/PQ embed — a *non-default,
eval-gated spike*, not a core-parity blocker) and **#32** (optional
semantic-embeddings feature, non-default follow-up from #12). Both are optional
enhancements behind flags. **There is exactly one unit of real remaining domain
work relevant to the goal: #17 — and it is actionable now** because its only
gate (#16 eval harness) closed 07-06.

### 5b. Reconciling the two framings (the crux)

Round 1 and its sibling docs produced **two apparently contradictory readings**.
They are reconciled by a **repository / issue-number collision**:

- **Framing A ("phantom workstreams")** —
  `2672-recurring-signature-systemic-signals.md` checked issue numbers 16/17/18/
  21/22 against **`rysweet/Simard`**, where those numbers are unrelated CLOSED
  issues (docs revision, pre-commit fix, base-type recovery, gym foundation,
  meeting/gym block). It correctly nailed the **mechanism** (the nesting loop)
  but, checking the wrong repo, wrongly concluded the WS1–WS7 workstreams were
  fictional leaves.
- **Framing B ("real, ~80% done")** —
  `kgpacks-rs-parity-block-remediation.md` checked the **same numbers** against
  **`rysweet/agent-kgpacks-rs`**, where #16/#17/#18/#21/#22 are **exactly** WS1/
  WS2/WS3/WS6/WS7. This round independently re-verified: the goal IDs
  `fix-agent-kgpacks-rs-issue-NN-wsX-…` match the agent-kgpacks-rs issue titles
  **verbatim**. Framing B is correct; the workstreams are **real**.

**Reconciled root-cause statement (both layers true simultaneously):**

> The `blocked` signature **recurs** because of the **self-amplification nesting
> loop** at `observation_signature` (Framing A's mechanism — the reason it is
> "seen 2×" and grows a new `overseer-obs:` prefix each generation). The loop is
> **fed** by `goal:blocked:…issue-NN` leaves that persist because the Overseer
> has **no goal-board↔GitHub-issue-closure reconciliation** (Framing B's domain
> seed): four of the five `agent-kgpacks-rs` workstreams are already
> CLOSED/COMPLETED, yet their goals stay parked `Blocked`. The "phantom" reading
> was an artifact of resolving the embedded `issue-NN` numbers against the
> **wrong repository** (`rysweet/Simard` instead of `rysweet/agent-kgpacks-rs`).
> Fixing the domain seed (reconcile stale parks) **starves** the loop's input;
> fixing the mechanism (de-nest) **collapses** the composite to a stable fixed
> point. Both are needed: P1 removes the recurring leaves, P0 guarantees the
> signature stops mutating.

---

## Evidence index

- **Mechanism / loop:** `observation_signature` `mod.rs:1081-1086`; recurring
  threshold `signal.rs:455-469`, `signal.rs:362`; recall re-extract
  `wiring.rs:976-986`; `sanitize_recalled` `capabilities.rs:468-482`; write-back
  gate `mod.rs:297`.
- **Systemic signals:** gym `provider.rs:61` / `signal.rs:398-400` /
  `mod.rs:1292-1297`; workstream-gap `sensor.rs:288-370` (blocked-exclusion
  `sensor.rs:300-302`) / `signal.rs:475-479` / `mod.rs:1381-1386`;
  engineer-spawn `sensor.rs:123` / `signal.rs:393-397` (`ENGINEER_SPAWN_THRESHOLD=8`
  `signal.rs:351`) / `mod.rs:1280-1285`; spawn-storm=stuck-workstreams
  `root_cause.rs:326-336`.
- **Domain seed:** blocked-goal projection `sensor.rs:204-221`;
  `decide_blocked_goal` (no issue-closure path) `mod.rs:1624-1666`;
  `StaleGoal→TransferGoal` closure (only without `GoalBlocked` evidence)
  `mod.rs:1505-1513`.
- **Infra incidents (C3):** engineer-claim-sentinel dispatch deadlock
  `docs/reference/engineer-claim-sentinel-exclusion.md:52-58` (blocked set
  #12/#16/#17/#18/#19/#20/#21); E2BIG/exit-126 argv overflow, "most reproducibly
  the large-context agent-kgpacks-rs workstream goals"
  `docs/concepts/self-diagnose-on-step-error.md:57-88`.
- **Issue states (`gh`, 2026-07-07, `rysweet/agent-kgpacks-rs`):** #16 CLOSED
  20:16Z; #17 **OPEN**; #18 CLOSED 10:33Z; #21 CLOSED 13:29Z; #22 CLOSED 12:07Z;
  also open: #32 (optional semantic-embeddings). Same numbers in `rysweet/Simard`
  are unrelated CLOSED issues (docs/pre-commit/gym) — the collision behind
  Framing A's "phantom" reading.
