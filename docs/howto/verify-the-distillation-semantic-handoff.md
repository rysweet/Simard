---
title: Verify the distillation semantic handoff
description: Step-by-step how-to for confirming that episode distillation writes facts DIRECTLY into cognitive memory via `simard memory remember` (issue #2679) — running a pass, watching the write ledger, using `simard memory stats` to see the fact count climb, and proving that a noisy / trailing-comma agent output no longer fails because nothing is parsed.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../architecture/distillation-semantic-handoff.md
  - ../reference/simard-memory-remember-cli.md
  - ../reference/distill-write-boundary-gate.md
  - ../reference/simard-memory-cli.md
  - ../architecture/episode-distillation.md
---

# Verify the distillation semantic handoff

Use this how-to to confirm that distillation is reaching cognitive memory the
**new** way (issue [#2679](https://github.com/rysweet/Simard/issues/2679)): the
distiller agent calls `simard memory remember` once per fact, and Simard never
parses a returned document. The property you are verifying is that facts flow
**agent → memory** semantically, and that malformed agent text can no longer
discard a batch — because there is nothing to parse.

## Before you start

- The OODA daemon must be **running** (writes require a live memory socket).
- Confirm the daemon is up and serving the memory socket:

  ```bash
  simard memory stats            # banner should read "via daemon socket"
  ```

## 1. Snapshot the current fact count

```bash
simard memory stats --json | tee /tmp/before.json
```

Note the `semantic` (facts) count. This is your baseline.

## 2. Trigger a distillation pass

Distillation fires automatically once the undistilled backlog crosses threshold
(see [Automatic distillation scheduler](../reference/automatic-distillation-scheduler.md)),
or you can let the daemon reach a `__memory__` cycle. Watch the daemon log for
the pass:

```text
[simard] distill scheduler: Threshold trigger fired (undistilled≈37, cycles_since_last=6)
[simard] distill: 37 episodes pulled (batch size 50, min 20)
```

During the pass you should see per-fact write lines from the agentic step, e.g.:

```text
[simard] memory remember: stored concept=lesson-learned confidence=0.90 node_id=sem_41
[simard] memory remember: stored concept=bug-pattern confidence=0.90 node_id=sem_42
[simard] memory remember: quarantined concept=pr-pattern confidence=0.40 (below gate)
```

Each line is one `simard memory remember` invocation — one process, one fact.
There is **no** `{ "facts": [...] }` envelope in the log and **no** parse step.

## 3. Confirm the fact count climbed

```bash
simard memory stats --json | tee /tmp/after.json
```

The `semantic` count should be higher than your baseline by the number of
**stored** (non-quarantined) facts. Quarantined candidates are counted in the
pass report but are intentionally not persisted — that is the write-boundary
gate protecting memory integrity, not a failure.

## 4. Check the telemetry

```bash
grep -E 'simard\.distill\.(runs|facts)' ~/.simard/metrics.jsonl | tail
```

You should see `simard.distill.runs{result="ok"}` and a `simard.distill.facts`
count equal to the stored-fact tally from step 3.

> **You will NOT see `result="parse_fail"`.** That series was removed in #2679 —
> there is no parse to fail. If a dashboard still queries it, it will read empty;
> switch it to `result="ok"` / `simard.distill.facts`. See
> [write-boundary gate → telemetry](../reference/distill-write-boundary-gate.md#telemetry-changes).

## 5. Prove noisy output no longer breaks the pipeline

This is the core #2679 property. In a test/dev environment, point distillation
at a stub runner whose agent step emits **deliberately hostile** text on
stdout — ANSI escapes, tracing log lines, a copilot launch banner, and a
trailing comma — while performing its memory writes normally. The pass must
**succeed** and the facts must reach memory, because the stdout is never read as
the result.

The shipped regression suite encodes exactly this
(`src/memory_consolidation/distillation.rs` tests): a runner returns
launcher-banner + ANSI + tracing noise **and** a trailing comma, the agentic
step performs mock `remember` writes, and the assertions are:

1. the pass returns `Ok` (no error),
2. the mock memory received the expected facts, and
3. **no `parse_fail`** is recorded (the metric/attribute does not exist).

If you are writing your own check, assert the same three properties. The point
is not that the parser tolerates the bad bytes — it is that **there is no parser
on this path**.

## 6. Automated process-boundary proof (no daemon needed)

Steps 1–5 verify a running daemon. To prove the same handoff hermetically —
the **real `simard` binary** committing through a **live memory socket** with no
daemon and no fixtures — run the process-boundary integration suite:

```bash
cargo test --locked --test bin_simard_memory_remember_cli
```

It spawns a real IPC server over an in-memory store, then invokes
`simard memory remember` / `remember-procedure` as separate processes and pins
the exit-code contract against the gate: a grounded fact stores (exit `0`) and
is retrievable, an ungrounded fact is quarantined (exit `4`), a missing daemon
exits `3` (no un-gated fallback), and a malformed invocation exits `2`. This is
the automated stand-in for steps 2–3 when you do not have a daemon handy.

## Troubleshooting

| Symptom | Likely cause | Action |
|---------|--------------|--------|
| `remember` exits `3` (no endpoint) during a pass | Daemon socket absent; pass was skipped, not failed | Confirm the daemon is up (`simard memory stats`); the batch retries next cycle. |
| Fact count did not climb, log shows all `quarantined` | Facts scored below `DISTILL_RELIABILITY_THRESHOLD` | Expected gate behaviour; inspect grounding/quality of the source episodes. See [write-boundary gate](../reference/distill-write-boundary-gate.md#the-single-authoritative-gate). |
| Pass logged a spawn/terminal failure, no markers set | Structural recipe failure (not a parse) | Batch stays retry-eligible; check `recipe-runner-rs` availability and the agent binary. |
| Looking for a `parse_fail` metric | Removed in #2679 | Use `simard.distill.runs{result="ok"}` and `simard.distill.facts`. |
