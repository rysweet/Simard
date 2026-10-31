---
title: Operator update — "advance agent-kgpacks-rs to full parity" goal
description: Plain-language Signal/operator notification about the agent-kgpacks-rs full-parity goal getting an automatically checkable finish line (and honestly noting the work isn't finished yet).
last_updated: 2026-07-18
doc_type: operations
owner: simard
---

# Operator update — the "advance agent-kgpacks-rs to full parity" goal

This is the plain-language message sent to the operator over Signal when the
goal **"advance agent-kgpacks-rs to full parity"** was triaged and
course-corrected on 2026-07-18.

## The Signal message (as sent)

> Quick update on the goal about finishing the Rust rewrite of our
> knowledge-lookup tool so it fully matches the older version. It kept showing up
> on the "stuck" list, but not because anything was broken — the goal simply had
> no clear finish line the system could check. "Full parity" meant nothing a test
> could confirm, so every time the system looked at it, it couldn't tell whether
> it was done, and it kept re-checking the same goal over and over without making
> visible progress.
>
> I've fixed that. There's now a plain checklist of exactly what "finished" means,
> and it's tied to a single tracked to-do (issue #4321) that automatically ticks
> off the moment the last three remaining items are built and their tests pass.
> To be clear: the work is **not finished yet** — three items are still to do
> (safer keyword search, reusing an open database connection, and multi-step graph
> lookups). But the system now knows exactly what's left and exactly when it'll be
> done, so it will stop spinning on this goal and will only flag it again when it's
> genuinely complete. Nothing needed from you right now — I'll let you know when
> the last three items ship.

## What changed (for the record)

- **No product behaviour changed.** The fix was to give the goal a finish
  condition the system can actually check, not to alter the tool.
- The goal's finish line is now bound to **tracking issue rysweet/Simard#4321**,
  whose *closed* state means "full parity reached". #4321 closes exactly when the
  three remaining in-scope items ship and both spec test commands are green:
  `cargo test --lib native_knowledge` and `cargo test --lib knowledge_client`.
- Added `scripts/check-agent-kgpacks-rs-parity-done-gate.sh` — one command the
  system can run to get a clear verdict: it reports **done** when issue #4321 is
  closed, and otherwise prints the exact remaining items as the concrete next
  step, so the goal is never just "stuck".
- The full, enumerated checklist lives in
  [`Specs/agent-kgpacks-rs-parity.md`](../../Specs/agent-kgpacks-rs-parity.md):
  every parity criterion has an id, an acceptance test, a status, and code
  evidence. Three items remain **OPEN** (KGP-Q4, KGP-T3, KGP-Q5); the rest are
  DONE.

## Why it was stuck (in plain English)

Simard couldn't automatically tell when this goal would be finished. Its finish
line — "full parity" — had no test attached, so every time the system checked, it
saw the goal as "not confirmed done" and kept re-investigating without shipping
anything. Tying the goal to a single tracked to-do that closes on a clear,
testable checklist lets the system certify it on its own, and — just as
importantly — always know the concrete next step until then.

## What is honestly still left

This goal is **not** complete. Three in-scope items remain, in priority order:

1. **Safer keyword search** (KGP-Q4) — bind query parameters instead of
   string-building the search clause.
2. **Reuse an open database connection** (KGP-T3) — keep a live connection across
   queries instead of re-caching only the file path.
3. **Multi-step graph lookups** (KGP-Q5) — traverse linked entities and
   relationships, not just a single-table text scan.

When all three ship (and both spec suites stay green), issue #4321 closes and the
done-gate above certifies the goal complete automatically.
