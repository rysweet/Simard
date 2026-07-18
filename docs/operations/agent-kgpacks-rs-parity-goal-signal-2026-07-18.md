# Operator update — "finish the Rust knowledge-lookup rewrite" goal (2026-07-18)

This is the plain-English update the Overseer sent to the operator while
triaging a stuck goal. It contains no internal diagnostic codes.

## What was wrong

Simard had a standing goal to finish rewriting its knowledge-lookup tool in
Rust ("advance agent-kgpacks-rs to full parity"). The goal kept getting stuck:
Simard could not automatically tell when the goal was *done*, because "full
parity" had no finish line a test or command could confirm. With no way to
certify completion, Simard kept re-investigating the same goal without shipping
anything, and a safeguard parked it for making no real progress.

## What we found (root cause)

The problem was a **measurement gap, not finished work**. There was no issue,
pull request, or command the system could check to say "this is done." Three
pieces of the rewrite are also still genuinely unfinished, so the goal should
stay open — it just needed a checkable finish line.

## What we did

We gave the goal a finish line the system can check by itself:

1. **One tracked to-do to watch** — tracking issue #4321. It closes only when
   the last three pieces ship and their tests pass.
2. **One command that reports the status** —
   `scripts/check-agent-kgpacks-rs-parity-done-gate.sh`. It says "done" only
   when issue #4321 is closed; otherwise it prints exactly what is left:
   - parameterize the keyword search (safer, correct matching),
   - reuse an open database connection between queries,
   - graph-based multi-hop retrieval across linked entities.

Right now that command reports **not done yet**, and names those three
remaining items — which is the honest, correct status.

## What this means for you

Nothing is needed from you. The goal can now certify itself: it will report
"done" automatically once issue #4321 closes, and until then it always shows
the concrete next step instead of getting stuck. We did not mark it complete,
because the work is genuinely unfinished.
