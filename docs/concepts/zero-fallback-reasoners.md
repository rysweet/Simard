---
title: "Concept: zero-fallback reasoners — why a parse failure is a loud, retried error and never a deterministic default"
description: The retcon narrative for the #2432 deterministic-fallback incident — why the Brain-Failures dashboard surfaced a stream of deterministic-default outcomes (decide → continue_skipping/no-action, distill → 0 facts) and why the dashboard read zero active engineers, and how a single sanitizing chokepoint, a machine-parseable JSON-envelope decision contract per reasoner, explicit-error-plus-bounded-retry (never a silent default), a distinct evidenced take-no-action outcome, and a live-count engineer gauge together honour the operator's absolute "no fallback" stance.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ./copilot-launcher-preamble-stripping.md
  - ./text-based-brain-protocol.md
  - ./unified-recipe-brain.md
  - ./steerable-ooda-daemon.md
  - ../reference/ooda-brain-decision-protocol.md
  - ../reference/ooda-brain-parse-failure-record.md
  - ../reference/recipe-brain-verdict-parsing.md
  - ../reference/distill-recipe-output-capture.md
  - ../reference/distill-raw-capture-on-parse-failure.md
  - ../reference/concurrent-engineer-dispatch.md
  - ../reference/adaptive-scaling-api.md
  - ../reference/maximum-safe-parallelism.md
  - ../howto/diagnose-decide-orient-parse-failures.md
  - ../howto/diagnose-merge-pr-verdict-parse-failures.md
  - ../howto/diagnose-brain-decision-parse-failures.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# Concept: zero-fallback reasoners

> **Operator directive (absolute).** *"I don't ever want any fallback."* A
> deterministic fallback is a silent failure: when a reasoner's model output
> cannot be parsed, the daemon must **not** invent a safe-looking default and
> carry on. It must say so — loudly, on the dashboard, in the logs — and try
> again within a bounded budget. Only a hard, bounded, explicitly-reported
> error is an acceptable terminal state; a parse failure laundered into a
> "continue skipping" or "0 facts" outcome is not.

This document is the **single coherent narrative** for the #2432 incident, in
which Simard's one Brain — its per-phase **reasoners** (Orient, Decide, Act, the
merge-judge verdict, and the distillation parser) — kept converting *unreadable
model output* into *deterministic default decisions*, and the operator dashboard
reported **zero active engineers**. It ties together mechanisms that each already
have a focused reference page, and it reconciles the **operator-observed
symptoms** with the **implementation that the zero-fallback contract requires** —
the "retcon" that explains why the shipped design resolves what was seen in
production.

It is the successor narrative to
[Copilot launch-log preamble stripping](./copilot-launcher-preamble-stripping.md)
(#2496 / #2500) and PRs #2504 / #2486 / #2490: those routed decide/orient/
merge-judge/distill through a shared sanitizing chokepoint and introduced a
parse-extractor plus a JSON envelope, but review found residual leakage —
distillation still failing on the majority of samples and a persistent trickle
of `default_malformed` decide/orient events. This page is about closing the gap
**structurally** so that *no* residual leak can ever again terminate as a
deterministic default.

If you are here to change one subsystem, jump straight to its authoritative page:

| Symptom | Root cause | Authoritative doc |
|---|---|---|
| Brain-Failures tab shows a stream of `default_malformed` decide/orient events; goals sit at 0.00% while the brain "decides" `continue_skipping` every cycle | A capture path or prompt still lets launcher-banner + ANSI noise (or a bare/timeout turn) reach the extractor, which then falls through to a permissive deterministic default | [Copilot launcher preamble stripping](./copilot-launcher-preamble-stripping.md) · [OODA brain decision protocol](../reference/ooda-brain-decision-protocol.md) |
| `decide_engineer_lifecycle → continue_skipping` even when work is obviously pending | The "safety floor" reasoner returned `ContinueSkipping` on any brain `Err` — the exact fallback the operator forbids | [Parse-failure record](../reference/ooda-brain-parse-failure-record.md) · [Fix 3](#fix-3-a-parse-failure-is-a-loud-retried-error-never-a-default) (below) |
| `distill: N episodes → 0 facts, 0 procedures` on most runs | The distill transcript still carried preamble/ANSI on at least one capture path, or the parser keyword-sniffed free prose instead of reading a required structured block | [Distill recipe-output capture](../reference/distill-recipe-output-capture.md) · [Distill raw-capture on parse failure](../reference/distill-raw-capture-on-parse-failure.md) |
| merge-judge verdict misparsed → permissive "accept" or deferral | Verdict parsing scanned prose for a keyword instead of reading a required verdict field in a JSON envelope | [Recipe-brain verdict parsing](../reference/recipe-brain-verdict-parsing.md) · [Diagnose merge-PR verdict parse failures](../howto/diagnose-merge-pr-verdict-parse-failures.md) |
| Dashboard: **active engineers = 0** while the daemon claims to be working | The Decide livelock (above) never emitted a spawn decision, so no engineer was ever dispatched — **REAL**, downstream of the fallback bug — *and* the gauge must be pinned to the true live set so a future telemetry defect can't hide it | [Concurrent engineer dispatch](../reference/concurrent-engineer-dispatch.md) · [Zero active engineers](#zero-active-engineers-real-not-telemetry) (below) |

---

## The incident

The operator watched the **Brain Failures** tab
(`src/operator_commands_dashboard/brain_failures.rs`) fill with events and the
work-board report **zero active engineers**, all at once:

1. **Silent defaults.** Every reasoner that reads model text — Orient (urgency),
   Decide (lifecycle action), the Act-phase dispatch plan, the merge-judge
   verdict, and the distillation fact/procedure parser — shared the same failure
   mode: when the captured stdout could not be parsed into the expected
   structured decision, the code ran the #2432 confidence-gated escalation ladder
   and, when the ladder was exhausted, emitted a **deterministic default**:
   `continue_skipping` / no-action for Decide, "accept" or "defer" for the
   verdict, `0 facts` for distill. On the dashboard these look like *decisions*.
   They are silent failures wearing a decision's clothes.

2. **The default that never spawns.** `decide_engineer_lifecycle` defaulting to
   `ContinueSkipping` means the daemon *chooses* not to spawn an engineer. Cycle
   after cycle it "decided" to keep skipping, so no `AdvanceGoal` dispatch ever
   ran and no `simard engineer` subprocess was ever started.

3. **Zero active engineers.** With nothing ever dispatched, the live-engineer
   gauge — which counts running engineer sessions — correctly read **0**. The
   gauge was telling the truth about a daemon that had talked itself into doing
   nothing.

The three compounded: unreadable output (1) became a "keep skipping" default (2)
which produced an idle daemon whose honest telemetry (3) looked like a dead
dashboard. The known trigger of (1) is the amplihack Copilot wrapper's
**launch-log preamble** printed to captured stdout — the
`ℹ … NODE_OPTIONS=… (saved preference)` info marker, the `Run 'copilot update' …`
nag, the `… launching copilot binary=… version="GitHub Copilot CLI …"` line, and
bare `INFO`/`WARN` launcher lines — plus ANSI colour codes, which pollute the
captured text so the structured extractor cannot find the payload.

---

## The contract: zero fallback

Every reasoner obeys one rule, stated as a state machine with **no default
branch**:

```
capture stdout
   └─ sanitize at the ONE shared chokepoint (extract.rs)
        └─ parse the REQUIRED structured decision envelope
             ├─ parses            ──►  use the decision (success)
             └─ does not parse    ──►  LOUD error: tracing warn/error
                                        + brain_parse_failure metric
                                        + ParseFailureRecord on the cycle report
                                        └─ RETRY (bounded budget)
                                             ├─ a later turn parses ──► success
                                             └─ budget exhausted    ──► HARD ERROR
                                                                        (propagated,
                                                                         never a default)
```

There is deliberately **no** "…else emit a safe default" leg. A genuine
take-no-action is a *separate, independently-evidenced* outcome (see
[Fix 4](#fix-4-take-no-action-is-a-distinct-evidenced-outcome)), reached by
reasoning, never by a parse miss.

---

## Fix 1 — One sanitizing chokepoint, every capture path

**Symptom:** residual `default_malformed` decide/orient events and majority
distill failures persisted after #2504, because at least one capture path still
delivered banner/ANSI-polluted text to its extractor.

**Root cause.** Sanitization lived at *most* call sites but not provably *all* of
them. Any path that read `recipe-runner-rs` stdout without going through the
shared helper re-introduced the exact noise the helper exists to remove — and a
single un-sanitized path is enough to keep a reasoner failing.

**What the contract requires.** Sanitization happens **once**, at the single
shared `recipe_output` chokepoint (`strip_ansi` → `strip_recipe_noise` →
`is_copilot_launcher_line` in `src/recipe_output/extract.rs`), and **every**
reasoner capture path — Orient, Decide, Act-plan, merge-judge verdict, and the
distillation parser — reads its agent output *through that one function*. No path
may parse raw captured stdout directly. `is_copilot_launcher_line` anchors on the
launcher markers and requires *both* `NODE_OPTIONS=` and `(saved preference)` on
the info-marker line, so genuine prose that merely mentions `NODE_OPTIONS` is
never eaten; `strip_ansi`/`strip_recipe_noise` return `Cow::Borrowed`
(zero-allocation, byte-identical) on already-clean input, so routing a
previously-hand-rolled path through the chokepoint cannot change its behaviour on
clean output. See
[Copilot launcher preamble stripping](./copilot-launcher-preamble-stripping.md).

**Tests.** A fixture carrying the wrapper banner **and** ANSI is fed through
every reasoner capture path (Orient/Decide/Act/merge-judge/distill) and asserted
to sanitize identically — a "no path bypasses the chokepoint" contract test, so a
newly-added capture path that forgets the chokepoint fails CI.

---

## Fix 2 — A machine-parseable decision envelope, not prose keyword-sniffing

**Symptom:** even sanitized output sometimes misparsed, because the extractor was
guessing — scanning free prose for an action keyword or an urgency decimal.

**Root cause.** Keyword-sniffing is inherently ambiguous. A model that answers in
a paragraph, hedges, or mentions the keyword inside an explanation defeats it,
and there is no unambiguous signal that "the model produced a real decision"
versus "the model rambled."

**What the contract requires.** Each reasoner prompt/recipe emits, and the
extractor consumes, an **unambiguous fenced JSON-envelope decision block with a
required decision field** — the decide action, the orient urgency, the
merge-judge verdict, the distill `{ "facts": …, "procedures": … }` payload.
Well-formed structured output parses deterministically; the extractor reads
*that block*, not the surrounding prose. Absence or malformation of the required
field is a parse failure (handled by Fix 3), not an occasion to guess. The
envelope schema and the parser's balanced-`{…}` extraction are specified in
[OODA brain decision protocol](../reference/ooda-brain-decision-protocol.md) and
[recipe-brain verdict parsing](../reference/recipe-brain-verdict-parsing.md).

**Tests.** For each reasoner, a well-formed JSON-envelope response parses to the
exact expected decision; a response that only *mentions* the keyword in prose,
with no envelope, is treated as a parse failure — proving the extractor relies on
the structured block, not on keyword presence.

---

## Fix 3 — A parse failure is a loud, retried error, never a default

**Symptom:** the forbidden behaviour itself — `decide_engineer_lifecycle`
returning `ContinueSkipping` on any brain `Err`, and the ladder-exhausted path
emitting a deterministic default outcome.

**Root cause.** A "deterministic safety floor" reasoner
(`DeterministicLifecycleBrain`) unconditionally returned
`ContinueSkipping { rationale: "…no LLM configured" }`, and call sites swallowed
brain `Err(_)` into it. Its own tests pinned the anti-contract: *"fallback brain
must never return Err"*, *"the safety floor … must never surface an Err that
could bubble up and stall the OODA loop."* That is precisely the silent failure
the operator forbids — the loop never stalls, so no one ever learns it is broken.

**What the contract requires.**

- **Remove the deterministic default paths.** The `ContinueSkipping`-on-error
  floor and every `unwrap_or(default_decision)` / ladder-exhausted-→-default site
  are replaced with explicit error propagation. A genuinely-missing LLM
  configuration is an explicit **startup/config error**, surfaced at boot, not a
  per-cycle silent skip.
- **Make the failure loud.** A parse failure fires the four visibility channels
  already built in `src/ooda_brain/parse_failure.rs`: a structured
  `tracing::error!` (target `simard::ooda_brain`), the dashboard-visible
  **`brain_parse_failure`** metric (`record_metric`), a `ParseFailureRecord`
  embedded on the cycle report, and — past a consecutive-failure threshold — a
  throttled `gh issue create`. This is the metric the Brain-Failures tab counts
  (`src/operator_commands_dashboard/brain_failures.rs`), so a parse error is
  always visible, never absorbed.
- **Retry within a bounded budget.** After the loud error, the reasoner is
  retried up to a fixed budget. A retry that parses is a **success** (the
  incident is logged, the decision is real). Only exhaustion of the bounded
  budget is terminal — and it terminates as a **hard, propagated error**, not a
  default outcome. The `retry_attempted` field on `ParseFailureRecord` records
  which failures were retried.

**Tests.** (1) A first-turn unparseable response followed by a parseable retry
yields **SUCCESS** with a decision and a recorded parse-failure event. (2)
Exhausting the bounded retry budget yields an **explicit hard error** plus a
`brain_parse_failure` metric increment — and *never* a `ContinueSkipping`, a
verdict "accept", or a `0 facts` default. (3) A guard test asserts no code path
can reach a deterministic-default decision from a parse-failure branch.

---

## Fix 4 — Take-no-action is a distinct, evidenced outcome

**Symptom:** a real "nothing to do right now" was indistinguishable, on the
dashboard, from a parse failure defaulted into `continue_skipping`.

**Root cause.** Both produced the same `ContinueSkipping` shape, so operators
could not tell a *reasoned* no-op from a *laundered* one.

**What the contract requires.** A legitimate no-op is a **first-class, distinct,
independently-evidenced** outcome — e.g. "no-op: verified genuinely nothing to
do" — produced only by a reasoner that parsed successfully *and* concluded no
action is warranted, with the evidence for that conclusion recorded. It is
observably separate from any parse-failure path: a parse failure can never
render as a no-op, and a no-op is never emitted without a successful parse behind
it.

**Tests.** The reasoned no-op and the parse-failure paths produce
distinguishable, independently-asserted results; a parse failure is never
observable as a take-no-action outcome.

---

## Zero active engineers — REAL, not telemetry

**Verdict: REAL, and downstream of the fallback bug.** Read-only inspection of
the design confirms the gauge was honest.

**Evidence (read-only).**

- The live-engineer count is served by
  `src/operator_commands_dashboard/subagent.rs::subagent_sessions()`, which loads
  the subagent registry and returns the sessions with `ended_at.is_none()` as
  `live`. The gauge is *derived from real session records*, not a hand-maintained
  counter.
- Records are written by `record_spawn`
  (`src/ooda_actions/advance_goal/spawn.rs`) at the moment an `AdvanceGoal`
  dispatch actually starts an engineer, and are reaped each cycle by
  `poll_and_gc(TmuxProbe)` (`src/ooda_loop/cycle.rs`), which stamps `ended_at`
  when the underlying session is gone.
- The Act phase plans at most `scaler.current_max()` concurrent dispatches
  (`src/ooda_actions/concurrent.rs`, `src/ooda_loop/adaptive_scaling.rs`). The
  AIMD cap was healthy; the daemon simply never *planned* a dispatch, because
  Decide kept "deciding" `continue_skipping` (Fix 3).

So `active engineers = 0` was **not** a stale or never-updated gauge — it was the
true consequence of a Decide phase that laundered parse failures into a perpetual
no-spawn default. Fixing the fallback (Fixes 1–3) makes Decide reliably emit a
spawn decision when work is pending, which produces `record_spawn` entries, which
the gauge then reflects.

**Belt-and-suspenders on the telemetry.** So that a *future* telemetry defect can
never again hide a working (or a stalled) daemon, the gauge is pinned to the true
live set with a test: `workboard_active_engineers_come_from_live_subagent_sessions`
(`src/operator_commands_dashboard/tests_routes_b.rs`) asserts the reported count
equals the number of live (`ended_at.is_none()`) engineer sessions in the
registry, so a cold-start reset or a stale source would fail CI rather than
silently reading zero. See
[Concurrent engineer dispatch](../reference/concurrent-engineer-dispatch.md) and
[Adaptive scaling API](../reference/adaptive-scaling-api.md).

---

## Why these ship together

Each fix removes one way a reasoner could turn *unreadable output* into a
*confident wrong decision*:

- **Fix 1** guarantees the extractor sees the model's real bytes on **every**
  capture path — no un-sanitized bypass.
- **Fix 2** guarantees "a decision" is an unambiguous structured object, not a
  guess about prose.
- **Fix 3** guarantees a genuine parse failure is **loud and retried**, and that
  its terminal state is a hard error — never a default.
- **Fix 4** guarantees a real no-op is evidenced and distinct, so the dashboard
  never confuses reasoning with failure.
- **Zero engineers** is then explained and fixed at its true (REAL) root, with a
  live-count test so the gauge can never lie by omission.

Remove any one and a parse failure can slide back into a silent default, and the
daemon can talk itself into an idle dashboard again.

---

## Guarantees and non-guarantees

**Guaranteed**

- **No deterministic default from a parse failure.** No code path emits a
  `continue_skipping` / verdict-accept / `0 facts` outcome as the *result of a
  parse miss*; a parse miss is a loud error + bounded retry + hard error on
  exhaustion.
- **Single-chokepoint coverage.** Every reasoner capture path
  (Orient/Decide/Act/merge-judge/distill) sanitizes through
  `src/recipe_output/extract.rs`; no path parses raw stdout.
- **Structured decision contract.** Each reasoner emits and consumes a required
  JSON-envelope decision field; well-formed output parses without prose
  keyword-sniffing.
- **Visible failures.** Every parse failure increments the dashboard-visible
  `brain_parse_failure` metric and records a `ParseFailureRecord`.
- **Honest engineer count.** The active-engineers gauge equals the live
  subagent-session set, enforced by test.
- **Distinct no-op.** A reasoned take-no-action is observably separate from any
  parse-failure path.

**Not guaranteed**

- **Model quality.** The contract guarantees a *malformed* answer is surfaced and
  retried, not that the model always produces a *good* decision. A model that
  never emits a parseable envelope within the retry budget yields an explicit
  hard error — the correct, loud terminal state — not a paper-over.
- **Cross-restart failure counters.** The consecutive-failure counter feeding the
  `gh issue create` throttle is process-local; a restart may cost one extra issue
  window. Documented and accepted.

---

## Operator diagnosis path

When the Brain-Failures tab shows events or the engineer count reads zero:

1. **Confirm loudness.** A parse failure should appear as a `brain_parse_failure`
   metric row and a `ParseFailureRecord` on the cycle report — not as a silent
   `continue_skipping`. Use the
   [decide/orient parse-failure runbook](../howto/diagnose-decide-orient-parse-failures.md),
   the [merge-PR verdict runbook](../howto/diagnose-merge-pr-verdict-parse-failures.md),
   and the [brain-decision parse-failure runbook](../howto/diagnose-brain-decision-parse-failures.md).
2. **Check the chokepoint.** Confirm the failing capture path routes through
   `extract.rs`; a new bypass is the first suspect.
3. **Check spawn flow.** If engineers read zero, verify Decide is emitting spawn
   decisions (not defaulting) and that `record_spawn` entries appear; see
   [spawn engineers from the OODA daemon](../howto/spawn-engineers-from-ooda-daemon.md)
   and [unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md).

---

## References

- [Copilot launch-log preamble stripping](./copilot-launcher-preamble-stripping.md) — the shared chokepoint precedent (#2496 / #2500).
- [OODA brain decision protocol](../reference/ooda-brain-decision-protocol.md) — the structured decision envelope.
- [OODA brain parse-failure record](../reference/ooda-brain-parse-failure-record.md) — the four visibility channels and the `ParseFailureRecord` schema.
- [Recipe-brain verdict parsing](../reference/recipe-brain-verdict-parsing.md) — the merge-judge verdict envelope.
- [Distill recipe-output capture](../reference/distill-recipe-output-capture.md) · [Distill raw-capture on parse failure](../reference/distill-raw-capture-on-parse-failure.md) — the distillation parser contract and its raw-capture-on-failure.
- [Concurrent engineer dispatch](../reference/concurrent-engineer-dispatch.md) · [Adaptive scaling API](../reference/adaptive-scaling-api.md) · [Maximum safe parallelism](../reference/maximum-safe-parallelism.md) — how dispatches are planned and counted.
- [Concept: keeping the OODA daemon steerable](./steerable-ooda-daemon.md) — the sibling incident whose Fix 4 (distillation banner-stripping) shares this chokepoint.
