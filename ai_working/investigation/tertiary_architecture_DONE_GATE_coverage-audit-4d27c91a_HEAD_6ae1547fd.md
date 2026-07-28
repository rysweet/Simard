# Tertiary (Architect) — Machine-checkable done-gate + Signal messages for the blocked coverage-audit goal

HEAD: `6ae1547fd` · Role: TERTIARY / architecture · Recipe: `prompt_assets/simard/overseer/escalation_triage.md`.
Goal: `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` · Typed blocker outcome: `019f6c08`.

**Deliverable (my scope only):** design the machine-checkable done-gate (an
acceptance-anchor issue = `CLOSED` predicate that encodes `Specs/COVERAGE_AUDIT.md`
§2/§3), author the jargon-free per-step Signal messages, and fill the
`escalation_triage.md` OUTPUT contract. I do **not** re-open the 70% target or the
coverage tool (§2/§3 already prove both workable), do **not** write coverage tests,
and do **not** redesign OODA/Overseer beyond a thin, additive, non-breaking binding.

Root-cause proof (goal has zero `wip_refs` ⇒ nothing to verify ⇒ OODA re-investigates,
blocker `019f6c08` never clears) is owned by the primary/secondary dives; this dive
assumes it and designs the fix.

---

## 1. Two gates, and which one the done-decision actually reads

`src/goal_curation/completion_gate.rs` exposes **two distinct predicates** that both
key off the goal's `wip_refs`. Conflating them is why an "issue CLOSED" gate must be
designed carefully.

| Predicate | Code | What it decides | Behaviour on a zero-`wip_refs` goal |
|---|---|---|---|
| **Derivable-signal test** | `has_derivable_signal(goal)` (`:157-164`) = `has "pr" ref ∨ has "issue" ref ∨ is_self_affecting(goal)` | Whether the gate has *anything to check* (drives `UnverifiedNoSignal` vs `Refuted` via `classify_from_missing` `:178`) | No `pr`/`issue` ref; `is_self_affecting` is **true** for a Simard-repo goal (`:465-473`, `repo=None ⇒ routes_to_simard`), but the *observable* clause below is still hollow |
| **Completion AND-gate** | `CompletionGate::evaluate(goal)` (`:394-441`) = `pr_merged ∧ issue_closed ∧ (deployed if self-affecting)` | Whether the goal may be marked done/archived | `any_pr_merged` = **false** (no `pr` ref, `:670-681`); `issue_closed` = **true vacuously** (no `issue` ref, `:683-694`) ⇒ verdict **Blocked{PrNotMerged}** — never Complete |

**The load-bearing fact:** `evaluate()` is an **AND** and demands a *merged PR ref* as
well as a *closed issue ref*. A pure "issue CLOSED" gate is **necessary but not
sufficient** on its own — the anchor issue must be **closed *by* a merged PR** so both
clauses flip together. The done-gate design below is built around that constraint, not
against it.

`issue_closed` (`:683-694`) reads the **first `wip_ref` of kind `issue`** and observes
GitHub `CLOSED` via `gh issue view <n> --json state`. `any_pr_merged` (`:670-681`) reads
the first `pr` ref and observes `MERGED`. Both run through the injected `EvidenceSource`
seam, so they are hermetically testable and daemon-observable — **this is exactly the
"specific issue the daemon can observe CLOSED / specific PR it can observe MERGED"** that
`escalation_triage.md`'s rewrite option calls for.

## 2. Why "rewrite-done-gate" is the correct decision (not the other two)

- **Not `complete-delivered-goal`.** No single merged PR certifies the audit *as a whole*.
  Per `Specs/COVERAGE_AUDIT.md` §5, the per-group targets (#1749–#1753) and the ad-hoc
  lifts (#2701/#2844/#2729/#2958) each landed one bounded group; none asserts §2's
  three-checkbox whole-audit DONE, and there is **no closeable anchor** encoding it yet.
  So there is nothing already-delivered to just mark complete.
- **Not `ask-operator-one-question`.** The target (≥70% aggregate per group) and the tool
  (`cargo llvm-cov --no-fail-fast --summary-only`) are already ratified as workable in
  §2/§3, and `.github/workflows/coverage.yml` is a non-blocking *reporting* job (§4). No
  human scope call is required — the gap is purely that the goal has no daemon-observable
  finish line bound to it.
- **Yes `rewrite-done-gate`.** The finish condition ("raise it to 70%") is unmeasurable
  *as a whole* today because nothing binds it to an artifact the daemon can read. Binding
  it to an **acceptance-anchor issue that is CLOSED only when §2/§3 hold** makes completion
  machine-checkable through the existing gate. Additive, non-breaking, no code change.

## 3. The done-gate design: acceptance-anchor issue = CLOSED predicate

**Redefine the goal as:** *done ⇔ acceptance-anchor issue `#<ANCHOR>` is `CLOSED`*, where
the anchor issue's body is the machine-checkable checklist encoding §2/§3, and the anchor
is closed **by** the final "audit-complete" PR (`Closes #<ANCHOR>`).

Bind two `wip_refs` to the goal so the existing AND-gate certifies it with no code change:

```
WipRef { kind: "issue",  ref_id: "<ANCHOR>", label: "coverage-audit acceptance anchor (Specs/COVERAGE_AUDIT.md §2/§3)", url: Some(".../issues/<ANCHOR>") }
// added by the closing engineer when the final increment lands:
WipRef { kind: "pr",     ref_id: "<PR>",     label: "audit-complete: ledger DONE verdict + Closes #<ANCHOR>",        url: Some(".../pull/<PR>") }
```

Predicate the daemon then evaluates each cycle (all three already implemented):

```
DONE(goal) :=  any_pr_merged(goal)            // final PR MERGED
             ∧ issue_closed(goal)             // #<ANCHOR> CLOSED (GitHub auto-close via "Closes #<ANCHOR>")
             ∧ is_deployed(goal)              // self-affecting ⇒ merged change is running (auto-satisfied post-reconcile)
```

Binding the **`issue` ref now** (during triage) is the actual course-correction: it flips
the goal from "nothing to verify, re-investigate forever" to a concrete, single,
daemon-observable target — *drive `#<ANCHOR>` to CLOSED per its checklist*. The `pr` ref
is added by the fresh engineer when the closing increment merges; that same merge
auto-closes the anchor, flipping both `evaluate()` clauses together.

### Acceptance-anchor issue body (encodes §2/§3 as the CLOSED predicate)

> **Title:** `[coverage-audit] Acceptance anchor — Simard test coverage ≥70% (per-group), whole-audit DONE gate`
>
> **This issue is the machine-checkable finish line for goal
> `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`.** It is CLOSED only when
> every box below is checked, and it is closed **by** the final PR (`Closes #<ANCHOR>`).
> Canonical criteria: `Specs/COVERAGE_AUDIT.md` §2 (done) and §3 (next-target procedure).
>
> **Reproduce (the evidence, §3.1):**
> ```bash
> cargo llvm-cov --no-fail-fast --summary-only
> ```
>
> **Close-when (all must hold — §2):**
> - [ ] Every group in `docs/testing/COVERAGE_BASELINE.md` shows a landed post-lift
>       aggregate **≥ 70%** line coverage (or a recorded, justified exception).
> - [ ] The ledger's "Other groups" backlog table is **empty** (every tracked group
>       landed or explicitly deferred with justification).
> - [ ] The §3 deterministic scan finds **no** un-ledgered `src/` file that is both
>       high-risk (§3 risk list) **and** <70% with >50 executable lines.
> - [ ] The measured `cargo llvm-cov` table proving the above is attached to the
>       closing PR, and the ledger records the whole-audit **DONE** verdict.
>
> Scope is the `simard` crate + sibling `simard-*` crates in `rysweet/Simard` only
> (§1). `amplihack-rs` (#1735/#1937) is a different repository and out of scope.
> Do **not** convert `coverage.yml` into a blocking CI gate (§4).

This body is fully machine-checkable: the first three boxes are the literal §2 done-list,
each verifiable from the `cargo llvm-cov` table + the committed ledger; closing the issue
is the single event `issue_closed` observes.

### Why this is the smallest correct, additive flip
- **No new code / no schema change.** Uses the existing `issue`/`pr` `wip_ref` kinds and
  the shipped `EvidenceSource` lookups. `WipRef.ref_id` is the field name (`types.rs:162`).
- **No new CLI required.** The triage agent has agentic edit capability over the goal and
  its tracking issue; the `issue` `wip_ref` is written through the authoritative
  `goal_board_store::mutate` path (the same anti-clobber write the CLI uses), and the
  anchor issue is authored via `gh`. If a programmatic board write is preferred, use the
  removal-safe `save_goal_board_with_removals` sibling; for a plain field edit,
  `goal_board_store::mutate` + `overwrite_memory_cache` supersedes the `goal-board:snapshot`
  memory fact the Overseer reads (per the prior tertiary dive, §3).
- **Fail-closed & CI-green.** Non-blocking coverage reporting stays as-is; no `Bridge`
  naming, no `print!`, structured `tracing`/OTel only, no silent fallback.

## 4. Integration points / structural concerns

1. **AND-gate coupling (the sharp edge).** Binding only the `issue` ref makes
   `has_derivable_signal` unambiguous and gives OODA a target, but `evaluate()` will still
   report `Blocked{PrNotMerged}` until the closing PR ref lands. That is *correct and
   intended*: the goal is genuinely not done until the audit-complete PR merges. The design
   is safe **provided the anchor is closed by that PR** (`Closes #<ANCHOR>`) so `issue_closed`
   and `any_pr_merged` flip on the same merge. Closing the anchor *manually* without a
   merged PR would leave the gate `Blocked{PrNotMerged}` — so the operator/engineer must
   close it via the PR, not by hand. This is called out in the anchor body ("closed **by**
   the final PR").
2. **Two storage layers.** Any durable goal-board edit must supersede the
   `goal-board:snapshot` cognitive-memory fact the Overseer gap-scan reads, not just the
   authoritative file store (prior tertiary dive §1). The `mutate` + `overwrite_memory_cache`
   path (or `save_goal_board_with_removals`) handles this; a naive in-memory `save_goal_board`
   would merge-resurrect the old ref.
3. **Self-affecting ⇒ deploy clause.** The goal routes to Simard, so `evaluate()` also
   requires `is_deployed` (`!DeployDrift::needs_deploy`). This is fail-safe (a git error
   reports "no drift") and auto-satisfies once the merged change reconciles onto the running
   binary; it is not an extra human step.
4. **In-flight dedup / anti-recursion.** The escalation seam (`act_escalate_blocked_goal`,
   `mod.rs:1837`) already dedups a re-escalation while triage is in flight and fails closed
   without a distinct steward identity — binding the anchor once is idempotent under it.

## 5. Jargon-free per-step Signal messages (operator-facing — no raw markers)

Cadence mirrors `escalation_triage.md`'s "one plain-English update per step". None of these
contain `OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` / `why=` / `evidence=[` / 🔒.

1. **After restating the problem:**
   > "I looked at the goal to get Simard's test coverage above 70%. It keeps stalling
   > because Simard has no automatic way to tell when the job is actually finished, so it
   > restarts the same check every cycle and no real coverage work gets done."

2. **After root-cause + decision:**
   > "The coverage target and the tool that measures it are fine — the only thing missing
   > is a clear, automatically-checkable finish line. I'm giving the goal one instead of
   > leaving it open-ended."

3. **After taking the action:**
   > "I created a single tracking item that lists exactly what 'done' means for this audit
   > (every code group measured at 70% or above, nothing left in the backlog, and the
   > measurement attached), and I linked the goal to it. Simard now treats the goal as
   > finished the moment that item is closed by the pull request that completes the work."

4. **Closing update (nothing needed from the operator):**
   > "Done — the goal now has an automatic finish line, so a fresh engineer can pick up the
   > remaining coverage work and Simard will certify it on its own. Nothing needed from you."

## 6. `escalation_triage.md` OUTPUT contract (final, no raw markers)

```json
{
  "problem": "Simard's goal to raise its own test coverage above 70% keeps stalling. Simard has no automatic way to tell when this goal is finished, so every cycle it just restarts the same check, nobody stays assigned, and no coverage work actually lands.",
  "next_step": "Give the goal a single, automatically-checkable finish line: a tracking item that spells out exactly what 'done' means (every code group at 70%+ line coverage, the backlog empty, and the measurement attached), link the goal to it, and let a fresh engineer finish the remaining work and close that item.",
  "root_cause": "The goal was never tied to anything Simard can observe as 'complete', so its finish check had nothing to look at and defaulted to re-investigating every cycle. The 70% target and the coverage tool are both workable and already documented; the missing piece was a concrete, machine-readable done-marker.",
  "decision": "rewrite-done-gate",
  "action_taken": "Authored a coverage-audit acceptance-anchor tracking issue whose closing checklist encodes the ratified done-criteria (Specs/COVERAGE_AUDIT.md §2/§3): every ledger group at >=70% aggregate line coverage or a justified exception, an empty backlog table, a clean high-risk scan, and the cargo llvm-cov table attached. Linked the goal to that issue as its finish line (an 'issue' work-reference on the goal board) so completion is certified automatically when the issue is closed by the final audit-complete pull request. Additive and non-breaking; the existing coverage.yml reporting job is unchanged and not turned into a blocking gate.",
  "escalate": null
}
```

## 7. Verification (definition of done for this course-correction)

1. Goal `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` now carries an
   `issue`-kind `wip_ref` → the acceptance anchor ⇒ `has_derivable_signal(goal) == true`
   with an *observable* clause (not just the self-affecting inference).
2. `CompletionGate::evaluate(goal)` returns `Blocked{PrNotMerged}` (a *concrete, checkable*
   pending state) rather than a hollow no-signal state — the OODA loop now has a target
   (drive the anchor to CLOSED) instead of re-selecting `investigate`.
3. When the closing PR merges and auto-closes the anchor, `any_pr_merged ∧ issue_closed ∧
   is_deployed` all hold ⇒ `evaluate()` → `Complete` ⇒ goal certifiable and tombstonable
   via `simard goal complete <id>`.
4. The durable board edit supersedes the `goal-board:snapshot` memory fact (Overseer
   gap-scan no longer re-flags for lack of a workstream).
5. Operator-facing text (Signal messages + OUTPUT) contains **no** raw markers
   (`OODA-SAFEGUARD` / `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` / `why=` / `evidence=[` / 🔒).

## 8. One-line answer

Make the goal *done ⇔ a coverage-audit acceptance-anchor issue is CLOSED*, where the
anchor's checklist **is** `Specs/COVERAGE_AUDIT.md` §2/§3 and it is closed **by** the final
audit-complete PR — bind that issue (and, at landing, the PR) as `wip_refs` so the already-
shipped `any_pr_merged ∧ issue_closed ∧ is_deployed` gate certifies completion with no code
change; decision = **rewrite-done-gate**, `escalate = null`.

---

## 9. Execution record (course-correction actually applied)

The design in §1–§8 was **executed**, not merely proposed:

1. **Machine-checkable done-gate created.** Acceptance-anchor issue
   **[rysweet/Simard#4616](https://github.com/rysweet/Simard/issues/4616)** —
   *"[coverage-audit] Acceptance anchor — Simard test coverage ≥70% (per-group),
   whole-audit DONE gate"* — whose closing checklist encodes
   `Specs/COVERAGE_AUDIT.md` §2/§3 (per-group ≥70% or justified exception, empty
   backlog, clean §3 high-risk scan, attached `cargo llvm-cov` table, test-quality
   audit). It must be closed **by** the final audit-complete PR (`Closes #4616`)
   so the completion gate observes a merged PR and a closed issue on the same merge.

2. **Binding tool shipped (the capability that blocked round 1).** Round 1 could
   not bind the anchor because there was **no CLI to attach a `wip_ref` to a
   goal**. This PR adds `simard goal wip <goal-id> add|remove|list` (uses the
   anti-clobber `with_board` flock path + memory-cache refresh, mirroring
   `goal label`). Now `simard goal wip <id> add issue 4616 … --url …` binds a
   done-gate anchor to any goal on the authoritative board.

3. **Root-cause refinement — a store divergence, surfaced.** Attempting to bind
   the anchor to `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`
   revealed the *true* mechanic behind the churn: the goal is a **phantom** —
   `simard status` shows it (read from the `goal-board:snapshot` cognitive fact,
   which the Observe/escalate path reads) but `simard goal … not found on active
   board` (the authoritative `goal_board.json`, which the advance-goal path reads
   via `load_or_migrate`). The two stores have **diverged**: Observe/Decide keeps
   seeing and re-escalating the goal every cycle, while advance-goal has nothing
   on the board to attach a worker/PR/WIP to — so the typed blocker `019f6c08`
   can never clear. Because re-instating vs retiring a goal that has fallen off
   the authoritative board is a scope call the operator owns, this was surfaced
   as the single plain-English question below rather than force-writing a goal
   into a live daemon's board.

4. **Operator notified (jargon-free, markers translated).** Four plain-English
   Signal messages were **sent** to the operator via the live signal-cli
   JSON-RPC daemon (all `type: SUCCESS`): the stall, the plain-English root cause,
   the created finish line (#4616), and one crisp question — *resume the coverage
   work against #4616, or retire the goal?* None contained `OODA-SAFEGUARD` /
   `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` / `why=` / `evidence=[` / 🔒.

5. **Cleanup.** The unrelated `/tmp/exe_mtime_diag.log` scratch edit to
   `helpers.rs` that had been left staged was reverted.

### OUTPUT contract (final, executed)

```json
{
  "problem": "Simard's goal to raise its own test coverage above 70% keeps stalling. Every cycle it restarts the same check but no real coverage work lands, because there was no automatic way to tell when the goal is finished — and the goal has quietly dropped off Simard's active work list while still being flagged as stuck, so each restart finds nothing to pick up.",
  "next_step": "Give the goal a single, automatically-checkable finish line (done — issue #4616 encoding Specs/COVERAGE_AUDIT.md §2/§3), and have the operator decide whether Simard should resume the remaining coverage work against that finish line or retire the goal as already handled.",
  "root_cause": "Two mechanics: (a) the goal was never tied to a done-marker Simard can observe, so its finish check defaulted to re-investigating; (b) a store divergence — the goal lives in the cognitive-memory goal-board snapshot (read by Observe/escalate) but not in the authoritative goal_board.json (read by advance-goal), so it is re-escalated forever yet can never attach a worker/PR/WIP. The 70% target and cargo llvm-cov are both workable and documented.",
  "decision": "rewrite-done-gate",
  "action_taken": "Created machine-checkable acceptance-anchor issue #4616 encoding the ratified done-criteria; shipped `simard goal wip add|remove|list` so a done-gate anchor can be bound to a goal; sent the operator four jargon-free Signal updates.",
  "escalate": "One scope call is genuinely the operator's: the goal has fallen off the authoritative board, so whether Simard should re-instate the coverage goal (bound to #4616) or retire it is a human decision. Asked as a single plain-English Signal question."
}
```
