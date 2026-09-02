---
title: "Operations: Creative-Ideas semantic-dedup kill switch (SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP)"
description: >
  The environment variable that disables the SEMANTIC (agentic) layer of the
  Creative Ideas dedup gate at daemon boot — what it does, and critically what it
  does NOT disable (deterministic word-set Jaccard dedup keeps running, so the
  board is never left un-deduplicated), when to use it, how to set it via
  systemd, how to verify which mode the daemon is in, and how to remove it.
  Secure default is the semantic layer ON.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: draft
related:
  - ../concepts/semantic-creative-ideas-dedup.md
  - ../reference/creative-ideas-dedup-gate-api.md
  - ../howto/configure-creative-ideas-semantic-dedup.md
  - resource-admission-kill-switch.md
---

# Creative-Ideas semantic-dedup kill switch (`SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP`)

> **Status: implemented (#2925).** This page specifies the kill switch. The gate
> it toggles lives in
> `src/creative_ideas/dedup_gate.rs`
> — see [semantic dedup + enhance-existing gate](../concepts/semantic-creative-ideas-dedup.md)
> and the [dedup-gate API](../reference/creative-ideas-dedup-gate-api.md).

This page documents the environment variable that disables the **semantic
(agentic) layer** of the Creative Ideas dedup gate at daemon boot. The gate is
described conceptually in
[semantic dedup + enhance-existing gate](../concepts/semantic-creative-ideas-dedup.md).

> **The kill switch disables the REASONING, not deduplication.** Turning it off
> does **not** re-open the pre-#2925 duplicate flood. The deterministic word-set
> **Jaccard** filter (the same one used today, threshold `0.6`) **keeps running**
> as the decision-maker — you simply lose the *semantic* judgment (paraphrase
> detection) and the **enhance-existing** capability. Disabling reverts dedup to
> today's lexical behaviour; it never leaves the board un-deduplicated.

---

## What this variable does

| Value | Behavior |
| --- | --- |
| Unset, or any value other than `off` (case-insensitive) | The **semantic** gate runs per candidate in the generation tick: build the coarse shortlist, call `decide_idea_dedup`, apply `create` / `skip` / `enhance`. Each decision emits a `creative_idea_dedup_decision` metric and a `CreativeIdeaDedup` judgment record; the tick logs `generated/skipped/enhanced/created`. |
| `off` (case-insensitive) | The **reasoning** is **skipped** — no shortlist prompt, no brain call, no `enhance`. Each candidate is judged by the **deterministic Jaccard filter only**: a match ≥ threshold ⇒ `skip`, else `create`. **No `creative_idea_dedup_decision` metric or `CreativeIdeaDedup` record is emitted**, and `enhanced` is always `0`. |

> **Unknown values keep the reasoning ENABLED.** Only the exact documented value
> `off` disables it. A typo (`false`, `0`, `no`, `disable`) leaves it **on** — the
> secure default is never silently disabled by a mis-spelled value. This is the
> same discipline as the
> [resource-admission kill switch](resource-admission-kill-switch.md).

The variable is read **once**, at daemon startup. Changing it during a run has no
effect — restart the daemon to pick up a new value.

---

## When to use it

- **Cost / latency containment.** The semantic layer adds up to
  `SIMARD_CREATIVE_IDEAS_BATCH` brain calls per (daily) tick. That is small, but
  if you need to eliminate all creative-ideas reasoning spend temporarily, `off`
  reverts to the zero-cost Jaccard path while still deduplicating.
- **Isolating a regression.** If a bad prompt edit or a reasoner outage is
  producing noisy decisions, `off` gives you a known-good deterministic baseline
  while you fix the recipe. (Note: a *runtime* reasoner error already fails closed
  to Jaccard automatically — you do not need the kill switch for a transient
  outage.)
- **A/B comparison.** Run with the semantic layer off to measure the board's
  duplicate rate under lexical-only dedup, then on to quantify the improvement.

For everyday tuning (shortlist size, prompt quality) you do **not** need this
switch — see [how to configure semantic dedup](../howto/configure-creative-ideas-semantic-dedup.md).

---

## How to set it

### One-shot (interactive run)

```bash
SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP=off simard daemon
```

### Persistent (systemd)

Add it to the daemon unit's environment, then restart:

```ini
# /etc/systemd/system/simard-daemon.service  (drop-in or [Service] section)
[Service]
Environment=SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP=off
```

```bash
sudo systemctl daemon-reload
sudo systemctl restart simard-daemon
```

To **re-enable** the secure default, remove the line (or set any non-`off`
value) and restart. The absence of the variable is the ON state.

---

## How to verify which mode the daemon is in

At startup the daemon logs the mode:

```
[simard] creative-ideas dedup: SEMANTIC layer ENABLED (default; SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP=off to opt out)
```

or, when disabled:

```
[simard] creative-ideas dedup: SEMANTIC layer DISABLED (SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP=off); deterministic Jaccard dedup still active
```

At runtime, the presence of `creative_idea_dedup_decision` metric lines and
`CreativeIdeaDedup` judgment records (and any non-zero `enhanced=` in the tick
summary) confirms the semantic layer is live. Their **absence**, with the tick
still logging `skipped`/`created`, confirms the Jaccard-only path.

---

## What it does NOT affect

- **The Jaccard dedup filter** — keeps running as the decision-maker; the board
  is still deduplicated lexically.
- **The rest of the Creative Ideas thread** — generation, the four-reviewer
  pipeline, routing, and the whole-subsystem switch
  (`SIMARD_CREATIVE_IDEAS_ENABLED`) are independent. To turn the whole subsystem
  off, use that switch (see
  [configure the Creative Ideas thread](../howto/configure-creative-ideas-thread.md)).
- **The consolidation pass** — the operator-invoked
  [consolidation](../howto/configure-creative-ideas-semantic-dedup.md#consolidate-the-existing-duplicate-pool)
  is a manual command; it is not gated by this switch.

---

## See also

- [Concept: semantic dedup + enhance-existing gate](../concepts/semantic-creative-ideas-dedup.md)
- [Dedup-gate API reference — kill-switch](../reference/creative-ideas-dedup-gate-api.md#kill-switch)
- [How to configure and operate semantic dedup](../howto/configure-creative-ideas-semantic-dedup.md)
- [Resource-admission kill switch](resource-admission-kill-switch.md) — the
  sibling switch with the same secure-default discipline.
