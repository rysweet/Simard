---
title: How to diagnose a red-canary unit-test gate
description: Operator runbook for the case where every self-deploy is refused on failing_gate="unit-test" while `cargo test --lib` passes clean standalone — recognise the environment-induced false red, read the named failing test from the enriched failing_detail, confirm the gate now runs hermetically in an isolated state root, and verify the self-deploy loop advances past the stuck SHA.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: howto
status: active
related:
  - ../reference/hermetic-unit-test-gate.md
  - ../reference/canary-gate-convergence.md
  - ../reference/overseer-deploy-canary-diagnostics.md
  - ../reference/state-root-resolution.md
  - ./converge-a-stuck-red-canary-self-deploy.md
  - ../safe-self-update.md
---

# How to diagnose a red-canary unit-test gate

> **Status: active.** This describes shipped behaviour: the hermetic
> `unit-test` gate and the named `failing_detail`. For the full design and API,
> see [Hermetic unit-test canary gate](../reference/hermetic-unit-test-gate.md).

Use this runbook when the Overseer refuses **every** self-deploy on the
`unit-test` gate — `DeployDrift` climbs, `running_commit` is pinned — yet the
test suite passes when you run it by hand.

## 1. Confirm the symptom

The #4558 signature is an identical `unit-test` refusal on every tick, with a
fast exit-101 abort:

```bash
journalctl --user -u simard -o cat \
  | grep -E 'overseer::deploy' | tail -n 40
```

You are looking for a repeating refusal against the **same** `target_commit`:

```
WARN overseer::deploy: self-deploy refused by deploy gate
    target_commit=7d0964f running_commit=7d0964f
    failing_gate="unit-test"
    failing_detail="tests failed (exit exit status: 101): <failing test block>"
```

## 2. Rule out a genuine regression

Run the same suite the gate runs, **standalone**, from the repo root:

```bash
cargo test --lib
```

- **If it fails** — this is a real regression. Read the named test from
  `failing_detail` (see step 3) and fix the source. Stop here.
- **If it passes clean** (e.g. `9279 passed; 0 failed`) but the gate reddens —
  this is the **environment-induced false red** the hermetic gate fixes. The
  in-process lib-test was aborting because it bound the live daemon's socket or
  locked its shared WAL / cognitive-store under the daemon's state root. With the
  hermetic gate shipped, this no longer happens; if you still see it, continue.

## 3. Read the named failing test

The gate now captures **both** stdout and stderr and extracts the failing test
name into `failing_detail` (clamped to 4096B at the gate, 512B downstream). You
should see a real marker block, **not** a truncated `Drop t…` spinner fragment:

```
failing_detail="tests failed (exit status: 101): failures:
    self_relaunch::gates::tests::extract_failure_detail_names_test
panicked at src/self_relaunch/gates.rs:412: assertion failed …"
```

If `failing_detail` still shows only `tests failed (exit …)` with no test name,
the running binary predates #4558 — deploy a build that includes the hermetic
gate.

## 4. Confirm the gate runs hermetically

The `unit-test` gate spawns `cargo test` with the four isolation keys overridden
to a fresh per-run temp dir and `current_dir` set to the manifest dir:

`SIMARD_STATE_ROOT`, `SIMARD_HOME`, `HOME`, `TMPDIR` → a private
`tempfile::TempDir`.

`CARGO_HOME` / `RUSTUP_HOME` are **pinned** to absolute paths resolved from the
real (pre-override) `HOME` *before* `HOME` is redirected, so cargo/rustup still
find the toolchain — without this, the `HOME` override would itself cause a fresh
exit-101 abort of the same class.

That temp state root is empty, so the in-process suite resolves its own WAL /
cognitive-store / socket path
([state-root resolution](../reference/state-root-resolution.md)) and cannot
collide with the live daemon. To verify locally that a running daemon no longer
red-canaries a green tree, run the gate's fixture test:

```bash
# Green fixture passes even with a simulated live daemon holding the shared root;
# red fixture's failing_detail names the failing test.
cargo test -p simard self_relaunch::gates
cargo test --test unit_test_gate_fixture   # integration fixture
```

If the temp-dir/env setup ever fails, the gate **fails closed** — it returns a
`unit-test gate could not create an isolated state root: …` failure rather than
silently falling back to the live daemon's state root. That is expected
fail-closed behaviour, not the #4558 bug; fix the temp/disk condition and retry.

## 5. Verify convergence

Once the gate renders a true green verdict, the guarded deploy gate stops
returning `RedCanary`, the swap proceeds, and drift returns to 0:

```bash
journalctl --user -u simard -o cat | grep -E 'overseer::(tick|deploy)' | tail -n 20
```

You should see the deploy **succeed** and the next drift observation report
`DeployDrift == 0` — the loop advances past the previously stuck target SHA
instead of re-queuing the identical `unit-test` refusal.

## See also

- [Hermetic unit-test canary gate](../reference/hermetic-unit-test-gate.md) —
  the design, the four isolation keys, `extract_failure_detail`, and the
  fail-closed / truncation contracts.
- [How to converge a stuck red-canary self-deploy](./converge-a-stuck-red-canary-self-deploy.md) —
  the sibling runbook for a red canary on any gate.
- [Overseer deploy red-canary diagnostics](../reference/overseer-deploy-canary-diagnostics.md) —
  the `failing_gate` / `failing_detail` surface this runbook reads.
