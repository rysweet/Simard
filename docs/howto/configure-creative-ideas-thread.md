---
title: Configure and operate the Creative Ideas thread
description: >
  Operator + developer guide for Simard's Creative Ideas background thread
  (#2419) — what it is, how to turn it on with SIMARD_CREATIVE_IDEAS_ENABLED,
  tuning the cadence / batch / budget knobs, what one generation tick does
  (generate → dedup/portfolio → persist → four-reviewer pipeline → synthesis →
  route), how the human-review gate keeps creative-idea PRs unmergeable (draft +
  blocking label + owner review, never --admin/--no-verify), tracing an idea
  through goal ↔ issue ↔ PR, monitoring the telemetry, extending it with a
  custom IdeaSource or Reviewer, and testing with the built-in fakes. The
  subsystem is default-ON, opt-out.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../operator-dashboard/creative-ideas-operator-controls.md
  - ../reference/creative-ideas-api.md
  - ../design/creative-ideas-thread.md
  - ./add-a-new-cognitive-thread.md
  - ./configure-cognitive-thread-scheduling.md
  - ../reference/goal-board-api.md
---

# Configure and operate the Creative Ideas thread

!!! note "Status — implemented; default-ON, opt-out"
    The Creative Ideas subsystem is **wired and live**: the `CreativeIdeasThread`
    is registered with the `Mind` scheduler and runs on its cadence, gated
    **default-ON** behind `SIMARD_CREATIVE_IDEAS_ENABLED` (opt out with a falsey
    value). Its idea source, four reviewers, and `gh` routing all run for real;
    tests drive them through deterministic fakes. The creative-idea memory type
    (status lifecycle + typed links) lives upstream in `amplihack-memory-lib`
    (guideline G2). This guide documents the surface as built so you can tune,
    opt out, extend, and test it. For the exact Rust API, see the
    [Creative Ideas API reference](../reference/creative-ideas-api.md).

## What the Creative Ideas thread is

The Creative Ideas thread is a **background cognitive thread** — one scheduled
mental process among many that the `Mind` runs alongside OODA (see
[Cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md)). On a
long cadence (≥ 24 h by default) it stands back from the current work, surveys
where Simard is, and proposes a **diverse batch of ten candidate
self-improvement ideas**. Each idea is stored as a **prospective-memory** node,
reviewed by a four-reviewer pipeline, and then routed safely — to a goal, to a
human-review issue, or parked.

It is a divergent-thinking *source of new ideas*, not an executor. Nothing is
committed to without review, high-risk ideas always go to a human, and any PR
born from a creative idea is **blocked from merge** until the owner approves —
enforced with plain GitHub mechanisms, **never** `--admin` or `--no-verify`.

## When to use this

- You want to **turn the subsystem on** in a non-production checkout and watch it
  generate and review ideas.
- You want to **tune** how often it runs, how many ideas it targets, or its
  budget ceiling.
- You are **extending** it with a real idea source or a new reviewer.
- You need to understand the **human-review gate** on creative-idea PRs.

If instead you want to add an unrelated scheduled process, see
[Add a new cognitive thread](./add-a-new-cognitive-thread.md). To change
*engineer* concurrency, that is the AIMD action-slot scaler
(`SIMARD_MAX_CONCURRENT_ACTIONS`), not this thread.

## Daemon wiring & startup (how the operator sees it run)

The thread is **wired into the running OODA daemon** — it is not something you
start separately. At startup the daemon builds the cognitive-thread runtime,
registers the Creative Ideas thread through
`register_creative_ideas_if_enabled(&mut mind, &cfg)`, and then ticks it on its
configured cadence alongside the Overseer and Journal threads.

### It runs even when the rest of the cognitive-thread scheduler is off

The generic cognitive-thread scheduler master switch
(`SIMARD_COGNITIVE_THREADS_ENABLED`, which turns on the maintenance and
engineer-log threads) is **default-OFF**. Creative Ideas is **not** gated behind
it. The daemon builds the `Mind` runtime whenever *either* the generic scheduler
**or** Creative Ideas is enabled:

```text
runtime built  ⇔  SIMARD_COGNITIVE_THREADS_ENABLED is truthy
                  OR  CreativeIdeasConfig::from_env().enabled()   (default true)
```

So on a stock deployment — with `SIMARD_COGNITIVE_THREADS_ENABLED` unset — the
maintenance/engineer-log threads stay off, but the Creative Ideas thread is
still registered and runs. Set `SIMARD_CREATIVE_IDEAS_ENABLED=0` to opt the
Creative Ideas thread (and, if nothing else needs the runtime, the runtime
itself) out.

### Startup log line

The daemon emits one dedicated line at startup, mirroring the Overseer and
Journal threads, so you can confirm the wiring at a glance:

```text
[simard] OODA daemon: creative-ideas thread ENABLED (default) (interval = 86400s; SIMARD_CREATIVE_IDEAS_ENABLED opt-out)
```

When opted out you instead see:

```text
[simard] OODA daemon: creative-ideas thread DISABLED (SIMARD_CREATIVE_IDEAS_ENABLED opt-out)
```

Confirm it under systemd with:

```bash
journalctl -u simard-ooda --no-pager | grep 'creative-ideas thread'
```

### Per-tick log line

Every time the thread actually runs (it is *due* on the first cycle after
startup, then every `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS`), the scheduler
surfaces its summary through the shared cognitive-thread log prefix:

```text
[simard] cognitive-thread: creative_ideas: generated 10 idea(s), 8 survived dedup, 8 persisted, 8 reviewed (2 → goal, 1 → issue), 0 review error(s)
```

A dry-run tick reads `generated (dry-run) …` and writes nothing external. Follow
the activity live with:

```bash
journalctl -u simard-ooda -f | grep -E 'creative[-_]ideas'
```

The same per-tick summary and the thread's heartbeat (`last_run` / `next_run` /
consecutive-error count) also flow into the Overseer activity feed and the
dashboard/journal, so a healthy Creative Ideas thread is visible without reading
raw logs. Because generation + review touch the network, the tick runs on a
background thread with an overlap guard and panic isolation — a slow or failing
tick can never stall or crash the authoritative OODA loop.

## Turn it off (opt out)

The subsystem is **default-ON**. `CreativeIdeasConfig::from_env()` is the single
gate; a default config is enabled, and only an explicit falsey value opts out.
The thread's `enabled()` then returns `false` and it never ticks.

!!! note "Budget and cadence"
    The thread runs on a long cadence (≥ 24 h) at `Priority::Low`, so it never
    competes with OODA, and it honors `SIMARD_DAILY_BUDGET_USD`. To disable it
    entirely on a given deployment, set the master switch to a falsey value.

```bash
# Master switch (default: true — set a falsey value to opt out)
export SIMARD_CREATIVE_IDEAS_ENABLED=0

# Optional tuning (defaults shown)
export SIMARD_CREATIVE_IDEAS_INTERVAL_SECS=86400   # cadence: >= 24h observation window
export SIMARD_CREATIVE_IDEAS_BATCH=10              # ideas targeted per run
export SIMARD_DAILY_BUDGET_USD=5.00                # reused budget ceiling (existing knob)
```

| Env var | Default | Effect |
|---------|---------|--------|
| `SIMARD_CREATIVE_IDEAS_ENABLED` | `true` | Master switch (default-ON). A falsey value (`0`/`false`/`no`/`off`) opts out ⇒ no generation, no routing, no side effects. |
| `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS` | `86400` | Generator cadence. Keep it large — this is a reflective pass, not a hot loop. |
| `SIMARD_CREATIVE_IDEAS_BATCH` | `10` | Ideas targeted per run before dedup/portfolio filtering. |
| `SIMARD_DAILY_BUDGET_USD` | *(existing)* | Reused: the thread skips an expensive tick when over budget. |

The truthiness check mirrors `overseer_acting_enabled()` — `1`, `true`, `yes`,
etc. count as on; unset or `0` is off.

### Opting out under systemd

The deployed daemon runs from the `simard-ooda` systemd unit
(`scripts/simard-ooda.service`), where the subsystem is on by default. To opt a
deployment out, add a drop-in (or edit the unit) so the master switch is falsey,
then reload and restart:

```bash
sudo systemctl edit simard-ooda    # creates a drop-in override
# In the editor add:
#   [Service]
#   Environment=SIMARD_CREATIVE_IDEAS_ENABLED=0
sudo systemctl daemon-reload
sudo systemctl restart simard-ooda
journalctl -u simard-ooda --no-pager | grep 'creative-ideas thread'   # now DISABLED
```

The unit itself documents this opt-out (and the least-privilege `gh`/`GITHUB_TOKEN`
scope the routing step needs) in a comment block near the other feature switches.

## What one generation tick does

When enabled and due, `CreativeIdeasThread::tick` runs this pipeline. It is
best-effort and total: any internal error is logged and folded into
`ThreadOutcome::failed(reason, elapsed)` — it never panics and never aborts the
daemon.

```mermaid
flowchart TD
    A["tick (due + enabled + within budget)"] --> B[Assemble GenerationInputs<br/>goals, >=24h activity, episodics,<br/>WIP, overseer, meeting insights, prior ideas]
    B --> C["IdeaSource.generate(inputs, 10)"]
    C --> D[Dedup vs previous_ideas<br/>+ portfolio balancing]
    D --> E[Persist survivors as<br/>CreativeIdea status=New]
    E --> F[Four-reviewer pipeline<br/>crusty · philosophy · measurability]
    F --> G[Synthesis sets next_status<br/>via try_transition]
    G -->|AcceptedForImplementation,<br/>not flagged| H[route_idea_to_goal → Proposed goal]
    G -->|NeedsHumanReview| I[typed proposal to GitHubMutationGuard<br/>label=creative-idea, assignee=rysweet]
    G -->|Rejected / Deferred / NeedsRevision| J[Persist status; park or re-enter]
```

Budget/cadence guards short-circuit before any expensive work: over budget or
merely not-yet-due ⇒ a **skipped** outcome with no side effects.

## The four reviewers

Every new idea is reviewed in order; then a synthesis step sets its status.

| Reviewer | What it judges |
|----------|----------------|
| **crusty-old-engineer** (skill) | Scope, feasibility, necessity, utility, inventiveness, **risk**, need-for-human-review, practicality. Raises the `high_risk` / `needs_human` flags. |
| **philosophy-guardian** (agent) | "Do we need this? Will it be interesting?" — but a **user signal is not required**; exploratory ideas are encouraged, so absence of a user request is never a block. |
| **measurability** (agent, new) | Attaches a concrete **success metric** — tied where relevant to existing self-metrics like `recall_precision_at_k`, distill fact-yield, or reasoner-reliability. Without a metric, the idea cannot be accepted. |
| **idea-feedback-synthesis** | Reads all reviews, writes next steps, and **sets the status** per the state machine. |

**Synthesis policy** (default, deterministic):

- Any `high_risk` / `irreversible` / `needs_human` flag ⇒ **NeedsHumanReview**.
- A fatal block from a **non-philosophy reviewer** ⇒ **Rejected**; a fixable block ⇒ **NeedsRevision**. (A philosophy-guardian "no user signal" is never a block — exploratory ideas are encouraged.)
- No success metric ⇒ **NeedsRevision** (unmeasurable ideas are not accepted).
- Otherwise, enough support + a metric ⇒ **AcceptedForImplementation**.

## Where reviewed ideas go (routing)

| Synthesis result | Route | Effect |
|------------------|-------|--------|
| `AcceptedForImplementation`, not flagged | `route_idea_to_goal` | A `Proposed` goal on the goal board, tagged with the originating idea `node_id`. The idea moves to `ImplementationStarted`. |
| `NeedsHumanReview` | `route_idea_to_issue` | Eligible lineage is submitted through `GitHubMutationGuard`; the issue is labelled `creative-idea`, assigned to `rysweet`, and carries typed provenance. |
| `Rejected` / `Deferred` / `NeedsRevision` | *(none)* | Status persisted; parked or re-entered on a later pass. |

## The human-review gate on creative-idea PRs

A PR that arises from a creative-idea goal is tracked specially and **cannot be
merged** until a human approves. `mark_idea_pr(pr, idea, gh, repo)` applies three
standard GitHub mechanisms and returns an `IdeaPrGate` describing them (`repo` is
`owner/name`, required because the underlying `gh` PR calls are repo-scoped):

- **Draft** — the PR is kept as a draft; a draft PR is unmergeable by anyone
  until explicitly marked ready.
- **Blocking label** — `creative-idea-needs-human-review`; branch protection /
  required status can key off this label to disable the merge button.
- **Owner review-required** — review is *requested* from `rysweet`. Simard
  never approves her own gate.
- **Link-back** — the PR body carries `originating-idea: <node_id>`, so the
  idea ↔ goal ↔ issue ↔ PR chain is fully traceable.

!!! danger "Never bypass the gate"
    The gate is enforced entirely with ordinary GitHub features. Simard
    **never** runs `gh pr merge --admin` or `git --no-verify`, and a unit test
    asserts the constructed `gh` argument vectors contain neither flag. An idea
    reaches `ImplementationCompleted` only when **both** the PR merges through
    the normal gate **and** its success metric is met.

## Trace an idea end to end

Every hop carries the originating idea `node_id`, so you can follow one idea
through the whole system:

1. **Idea** — a prospective node with `trigger_condition = "creative-idea"`.
   List creative ideas via `CreativeIdeaStore::list` (or inspect prospective
   memory with the [memory CLI](../reference/simard-memory-cli.md)).
2. **Goal** — its `GoalRecord` evidence/label contains the idea `node_id`
   (see the [Goal board API](../reference/goal-board-api.md)).
3. **Issue** — for human-review ideas, the `creative-idea` issue assigned to
   `rysweet` embeds the `node_id`.
4. **PR** — its body carries `originating-idea: <node_id>`, and it stays a
   labelled draft with an owner review requested until approved.

## Generate on demand and act on ideas from the dashboard

The background thread ticks on its own schedule (default every 24 h), but an
operator does not have to wait for it. The operator dashboard's **Creative
Ideas** tab renders the live idea pool and adds three controls:

- **Run now** — trigger one generation pass immediately, bypassing the
  `enabled()`/interval gate (a daemon restart resets the 24 h timer, so this is
  often the fastest way to get a fresh batch). It runs against the live daemon
  store, is guarded against concurrent runs, and surfaces any failure loudly.
- **Promote** — accept an idea (`AcceptedForImplementation`) and, by default,
  route it onto the goal board.
- **Prune** — reject an idea (`Rejected`).

Promote/Prune go strictly through the `IdeaStatus` state machine; only valid
transitions are offered and the server re-validates each write. For the HTTP
endpoints, JSON contracts, gating rules, and examples see
[Creative Ideas tab — live view and operator controls](../operator-dashboard/creative-ideas-operator-controls.md).

## Monitor it

Everything emits through the existing cognitive-thread telemetry facade
(`src/cognitive_threads/telemetry.rs`; see
[Telemetry metrics](../reference/telemetry-metrics.md)) keyed on the stable
thread id `creative_ideas` and the reviewer ids `crusty_old_engineer`,
`philosophy_guardian`, `measurability`, `idea_feedback_synthesis`:

- ideas generated / deduped per run,
- per-reviewer verdict counts,
- routed→goal / routed→issue / pr-gated counts,
- status-transition events.

Output is structured `tracing` only — no `println!`/`eprintln!` beyond the
`[simard] …` prefix convention.

## Safety knobs

| Guardrail | Knob / hook | Behavior |
|-----------|-------------|----------|
| Dedup / novelty | `dedup::is_near_duplicate(new, prior, threshold)` | Rejects a near-duplicate of a prior idea (token/shingle similarity). |
| Semantic dedup + enhance _(planned, #2925 — not yet implemented)_ | `dedup_gate::plan_candidate(...)` + the `creative-idea-dedup` recipe | Agentic per-candidate SKIP / ENHANCE-EXISTING / CREATE-NEW decision that catches paraphrased duplicates the lexical filter misses; fail-closed. See [configure semantic dedup](./configure-creative-ideas-semantic-dedup.md). |
| Diversity / portfolio | `portfolio::select_balanced(candidates, budget)` | Spreads the batch across risk/novelty buckets. |
| Rate-limit / budget | `budget::within_budget(now, cfg)` + `SIMARD_DAILY_BUDGET_USD` | Skips an expensive tick when over budget. |
| High-risk → human | synthesis policy + `try_transition` | High-risk/irreversible ideas can never auto-become a goal. |
| Outcome feedback | `route::mark_completed(idea, metric_met)` | Refuses `ImplementationCompleted` unless the success metric is met. |
| Default-ON, opt-out | `SIMARD_CREATIVE_IDEAS_ENABLED` | Runs by default; a falsey value opts the whole subsystem out (inert). |
| Dry-run | `ThreadContext.dry_run` | When set, `tick` still generates/reviews/logs but performs **no** destructive side-effect (no goal, issue, or PR writes). |
| Cooperative shutdown | `ThreadContext.shutdown` | `tick` observes the scheduler's cancellation flag and returns promptly between pipeline stages, so a daemon stop is never blocked mid-generation. |

## Extend it

### Add a custom idea source

Implement `IdeaSource`. The thread targets ten diverse candidates, then applies
dedup + portfolio filtering.

```rust
use simard::cognitive_threads::threads::creative_ideas::{GenerationInputs, IdeaSource, RawIdea};
use simard::error::SimardResult;

struct MyIdeaSource;

impl IdeaSource for MyIdeaSource {
    fn generate(&self, inputs: &GenerationInputs, n: usize) -> SimardResult<Vec<RawIdea>> {
        // Derive candidates from the read-only observation window.
        let ideas = inputs
            .current_goals
            .iter()
            .take(n)
            .map(|g| RawIdea {
                idea: format!("Improve tooling around: {g}"),
                links: Vec::new(),
                rationale: "derived from an active goal".into(),
            })
            .collect();
        Ok(ideas)
    }
}
```

### Add or swap a reviewer

Implement `Reviewer`. Keep the id stable (it keys telemetry) and only raise
`needs_human` when a human genuinely must decide.

```rust
use simard::creative_ideas::reviewers::{Review, ReviewContext, ReviewFlags, ReviewVerdict, Reviewer};
use simard::error::SimardResult;

struct SecuritySmellReviewer;

impl Reviewer for SecuritySmellReviewer {
    fn id(&self) -> &'static str { "security_smell" }

    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let risky = ctx.idea.idea.to_lowercase().contains("disable auth");
        Ok(Review {
            reviewer: self.id(),
            verdict: if risky { ReviewVerdict::NeedsHuman } else { ReviewVerdict::Support },
            notes: "checked for auth/secret smells".into(),
            flags: ReviewFlags { high_risk: risky, irreversible: false, needs_human: risky },
            proposed_metric: None,
        })
    }
}
```

Production reviewer adapters invoke an amplihack skill/agent through the
`AgentInvoker::invoke(prompt) -> String` seam (the same blessed session path the
OODA brain uses). The three vetting reviewers ship with real prompt assets
(`prompt_assets/simard/creative_ideas_review_*.md`); tests inject a deterministic
fake invoker so no network is touched.

## Test it (no network, all fakes)

Every unit test runs with fakes and an injected clock — no network, no sleeps.

```rust
// Routing: an accepted, non-flagged idea produces a Proposed goal.
let mut idea = CreativeIdea::new("index episodic recall by recency");
idea.try_transition(IdeaStatus::AcceptedForImplementation).expect("legal edge");

let goals = FakeGoalStore::default();
let now_epoch = 1_700_000_000_u64;                 // injected clock (unix epoch seconds)
let goal = route_idea_to_goal(&idea, &goals, now_epoch).expect("routes to goal");

assert_eq!(goal.status, GoalStatus::Proposed);
assert!(goal.evidence_contains(&idea.node_id));   // traceable back to the idea

// PR gate: every write is reserved through a fake guarded transport.
let gate = mark_idea_pr(
    42,
    &idea,
    &mut mutation_guard,
    &autonomous_authorization,
).expect("gate applied");
assert!(gate.draft);
assert_eq!(gate.blocking_label, "creative-idea-needs-human-review");
assert_eq!(gate.review_requested_from, vec!["rysweet".to_string()]);
assert!(transport.recorded_args().iter().all(|a| a != "--admin" && a != "--no-verify"));
```

The design's [test plan](../design/creative-ideas-thread.md#test-plan-no-network-all-fakes)
covers the full set: valid/invalid `try_transition` edges, persist+retrieve with
links, the four-reviewer pipeline + synthesis, all three routing paths, dedup
rejection, the default-ON/opt-out gate, the two error contracts, and a total `tick`.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| Thread never runs | `SIMARD_CREATIVE_IDEAS_ENABLED` set to a falsey value | Unset it (default-ON) or set a truthy value. |
| No ideas generated but tick ran | Over budget or not yet due | Check `SIMARD_DAILY_BUDGET_USD` and `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS`; a skipped tick has no side effects. |
| An idea stuck in `NeedsRevision` | Measurability reviewer produced no success metric | Ensure the idea admits a concrete metric; unmeasurable ideas are intentionally not accepted. |
| `InvalidIdeaTransition` in logs | Synthesis proposed an illegal `next_status` | Expected hard error — the runner rejects illegal edges rather than corrupting status. |
| `InvalidCreativeIdeaRecord` on read | Bad JSON payload or a `payload_version` newer than this binary | Fail-closed by design — do not downgrade-read; upgrade the reader (reader-first rollout). |
| A creative-idea PR looks mergeable | Draft/label/owner-review not applied | Re-run `mark_idea_pr`; never work around it with `--admin`/`--no-verify`. |

## See also

- [Creative Ideas tab — live view and operator controls](../operator-dashboard/creative-ideas-operator-controls.md)
- [Creative Ideas subsystem API reference](../reference/creative-ideas-api.md)
- [Creative Ideas background thread — design](../design/creative-ideas-thread.md)
- [Configure and monitor cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md)
- [Add a new cognitive thread](./add-a-new-cognitive-thread.md)
- [Goal board API](../reference/goal-board-api.md)
