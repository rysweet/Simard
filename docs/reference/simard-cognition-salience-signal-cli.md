---
title: Salience-signal CLI (`simard cognition salience-signal`)
description: >
  Agent and operator reference for `simard cognition salience-signal` — the clamping,
  validating write tool the salience-appraise recipe calls to publish the advisory
  salience ranking to `state/salience_signal.json`. All numeric enforcement lives IN the
  tool, so the recipe never prints JSON for Rust to parse. The deterministic OODA "Decide"
  reorder reads the same file fail-closed and re-clamps on the way in.
last_updated: 2026-07-26
owner: simard
doc_type: reference
related:
  - ./simard-memory-remember-cli.md
  - ./cognitive-thread-scheduling.md
  - ../architecture/reflective-cognitive-threads.md
  - ../howto/configure-reflective-cognitive-threads.md
  - ../reference/cognitive-memory-goal-store.md
---

# Salience-signal CLI (`simard cognition salience-signal`)

> Shipped in the reflective-cognitive-threads rework (supersedes PR #3142).
> This is the tool the [`salience-appraise`](../architecture/reflective-cognitive-threads.md)
> recipe calls to publish its numeric ranking. It replaces the previous
> Rust-side `write_signal` call that was driven by scraping the recipe's JSON.

`simard cognition salience-signal` writes the **advisory salience signal** — a
numeric ranking of active goals — to `state/salience_signal.json` under the
resolved state root. The **clamping and validation live inside the tool**, so
the recipe carries only scalar values on argv (or a small batch on stdin) and
never emits a JSON envelope for Rust to deserialize. This mirrors the
[`simard memory remember`](./simard-memory-remember-cli.md) design: the write
**is** the output; there is no return document to parse.

The signal is **advisory only**. It reorders the goals the OODA loop considers
in its "Decide" phase; it can never add, remove, or invent a goal. The
authoritative goal set is the [goal-board store](./cognitive-memory-goal-store.md).

---

## Synopsis

```text
simard cognition salience-signal \
  --entry <goal_id>:<urgency>:<valence> \
  [--entry <goal_id>:<urgency>:<valence> ...] \
  [--] [state_root]

# or, for larger rankings, stream a compact newline-delimited form on stdin:
simard cognition salience-signal --stdin < ranking.tsv
```

- `--entry <goal_id>:<urgency>:<valence>` — one ranked goal, **repeatable**.
  Provide the complete ranking in a **single invocation**: the write atomically
  **replaces** the whole file, so calling once per entry would clobber the prior
  entries. The number of active goals is capped (`MAX_ACTIVE_GOALS`, single
  digits), so the full ranking fits comfortably on argv.
- `--stdin` — read newline-delimited `goal_id\turgency\tvalence` rows from stdin
  instead of `--entry` flags. Use this for larger rankings to stay clear of the
  argv `E2BIG` ceiling. Never pass large payloads via argv/env.
- `state_root` (optional positional, after `--`) — the state root to write
  under. Defaults to the daemon's resolved state root. In the live daemon the
  recipe subprocess already targets the right root; this is provided for explicit
  or test use.

There is deliberately **no JSON-body form** — that would reintroduce the parse
this tool exists to remove.

---

## Field semantics and clamping

Each entry becomes one `SalienceEntry`:

| Field | Type | Valid range | Clamp behavior |
| --- | --- | --- | --- |
| `goal_id` | string | must be on the live goal board | dropped if not an active goal id |
| `urgency` | float | `[0.0, 1.0]` | clamped into range |
| `valence` | float | `[-1.0, 1.0]` | clamped into range |

The tool:

1. Parses each `--entry` / stdin row into `{ goal_id, urgency, valence }`.
2. **Clamps** every `urgency` to `[0.0, 1.0]` and every `valence` to
   `[-1.0, 1.0]` (`SalienceEntry::clamped`).
3. Loads the **live goal-board ids** and passes them as `valid_goal_ids`, so any
   entry naming a goal that is not currently on the board is **dropped**.
4. Stamps `generated_epoch = now()` for freshness.
5. Calls the existing `salience_signal::write_signal(state_root, &signal,
   valid_goal_ids)`, which performs an **atomic** temp-write + rename and
   re-clamps every field once more (defense in depth).

Enforcement is entirely server/tool-side. A recipe that emits an out-of-range or
off-board value cannot corrupt the signal; the worst it can do is have that entry
clamped or dropped.

---

## On-disk format

`state/salience_signal.json` (note the **underscore** path — kept exactly as the
reader expects):

```json
{
  "generated_epoch": 1785000000,
  "ranking": [
    { "goal_id": "goal-abc123", "valence": 0.20, "urgency": 0.90 },
    { "goal_id": "goal-def456", "valence": -0.10, "urgency": 0.40 }
  ]
}
```

- `generated_epoch` — Unix seconds when the signal was written; used for the
  freshness check on read.
- `ranking` — the validated, clamped `SalienceEntry` list. Each entry serializes
  in struct-declaration order (`goal_id`, `valence`, `urgency`).

The file is bounded to `MAX_SIGNAL_BYTES` (64 KiB). A larger file is treated as
malformed by the reader and ignored.

---

## How it is read (fail-closed, unchanged)

The deterministic OODA "Decide" reorder in `src/ooda_loop/cycle.rs` reads the
signal via `salience_signal::advisory_priority_order` (which wraps
`read_valid_signal`), which is **fail-closed** and was **not changed** by this
rework. It returns an empty ordering (no reorder; OODA proceeds on its own
ordering) when the file is:

- **absent** — no signal has been written;
- **oversized** — larger than `MAX_SIGNAL_BYTES`;
- **malformed** — not valid JSON / wrong shape;
- **stale** — `now - generated_epoch` exceeds `2 × DEFAULT_INTERVAL_SECS`.

On a successful read every field is **re-validated and re-clamped**, and any
entry whose id is no longer on the board is dropped. There is no path by which a
bad signal degrades OODA: absence and corruption both mean "ignore the advice."

---

## Usage from the `salience-appraise` recipe

The recipe reasons about which active goals deserve attention, then calls this
tool **once** with the full ranking and prints nothing for Rust to parse:

```bash
simard cognition salience-signal \
  --entry "goal-abc123:0.9:0.2" \
  --entry "goal-def456:0.4:-0.1"
# No JSON printed. This tool call IS the effect. Simard reads the recipe by exit status.
```

For a longer ranking, stream it:

```bash
printf 'goal-abc123\t0.9\t0.2\ngoal-def456\t0.4\t-0.1\n' \
  | simard cognition salience-signal --stdin
```

The `salience` thread that triggers this recipe records only `ran`/`health` from
the recipe's exit status — see
[Reflective cognitive threads act via tools](../architecture/reflective-cognitive-threads.md).

!!! note "The salience recipe also writes rationale facts"
    `salience-appraise` produces **two** effects. The numeric ranking goes here,
    via `cognition salience-signal`. The free-text `reason` for each goal is a
    separate durable fact written with
    [`simard memory remember`](./simard-memory-remember-cli.md) under a
    `salience:<goal_id>` concept key — deliberately kept out of the numeric-only
    signal file. The reworked recipe must call both tools.

!!! warning "Arg grammar is a design contract for the builder"
    The `--entry <goal_id>:<urgency>:<valence>` / `--stdin` / trailing
    `-- <state_root>` grammar above is the **specified** interface for a tool
    that does not yet exist in `main`. The implementation must match this
    reference (colon-delimited fields; negative `valence` such as
    `id:0.4:-0.1` must parse), or this reference must be updated to match the
    implementation. It wraps the existing
    `salience_signal::write_signal(state_root, &signal, valid_goal_ids)`
    unchanged.

---

## Exit status

| Exit | Meaning |
| --- | --- |
| `0` | Signal written atomically (after clamping/validation/off-board drop). |
| non-zero | Parse error in `--entry`/stdin input, or the write failed. Nothing partial is left on disk (temp-write + rename is atomic). |

Because the calling thread judges the recipe by exit status alone, a non-zero
exit is recorded **loudly** as a thread health error — never silently swallowed.

## See also

- [Reflective cognitive threads act via tools](../architecture/reflective-cognitive-threads.md)
- [`simard memory remember` CLI](./simard-memory-remember-cli.md)
- [Cognitive-memory goal store](./cognitive-memory-goal-store.md)
- [Configure the reflective cognitive threads](../howto/configure-reflective-cognitive-threads.md)
