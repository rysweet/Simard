---
title: Copilot launch-log preamble stripping
description: Why the Copilot CLI launch-log preamble and ANSI noise that the agent binary prepends to its stdout is stripped at the single shared recipe_output chokepoint, so every recipe-backed brain phase (decide, orient, engineer-lifecycle, merge-judge) and the distillation pass parse the agent's real output — closing the decide-phase deadlock where every active goal's decision misparsed to default_malformed, the escalation ladder exhausted, and Simard spawned zero engineers.
last_updated: 2026-06-29
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/text-parsing-wire-formats.md
  - ../reference/recipe-brain-verdict-parsing.md
  - ../reference/distill-recipe-output-capture.md
  - ../howto/diagnose-decide-orient-parse-failures.md
  - ./text-based-brain-protocol.md
---

# Copilot launch-log preamble stripping

The GitHub Copilot CLI agent binary prepends a **launch-log preamble** and ANSI
colour codes to its stdout before the agent's real answer. When that stdout is
captured as a recipe step's `output` and read by an OODA brain decision parser,
the preamble shadows the decision token. This document explains the failure it
caused, why the fix lives at one shared chokepoint, and how a parse failure is
now kept distinct from a deliberate "do nothing" decision.

For the normative grammar and the exact predicate, see
[Text-parsing wire formats § Protocol 0](../reference/text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output).
For the per-phase transport and the escalation ladder, see
[Recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md).

---

## The problem: a brain-parse deadlock

In steady-state daemon cycles, `decide_with_brain` failed to parse a decision
for **every active goal**. The base parse classified as `default_malformed`,
the confidence-gated escalation ladder ([#2432](https://github.com/rysweet/Simard/issues/2432))
re-prompted and still missed, the ladder exhausted, and the decide phase fell
to its deterministic default of **no new action**. Zero engineers were spawned.
Open PRs sat untouched for hours and no new work started — Simard **stalled**.

This is a self-sustaining deadlock: the brain-parse failure is exactly what
prevents spawning the engineer that would fix the brain-parse failure. The
ladder and the deterministic default — both safety nets that exist for good
reasons — cannot break the loop, because they were being fed poisoned input on
every attempt.

### Root cause: launcher noise in the captured agent output

The Copilot CLI (1.0.66-2 at the time) writes a **launch-log preamble** to
stdout before the agent's answer, for example:

```
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config
… INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot version="GitHub Copilot CLI 1.0.66-2."
Run 'copilot update' to check for updates.
```

plus ANSI SGR colour codes on the `INFO`/`WARN` launcher lines. These lines
carry **no ISO-8601 timestamp**, so they look different from the
`tracing`/`env_logger` log lines the shared extractor already dropped. The
decide and orient parsers read the **first token** of the cleaned text
(an action keyword, or a bare urgency decimal). With the preamble surviving the
clean, the first token was `ℹ` / `Run` / the version string `1.0.66-2` — never a
valid action keyword, and (for orient) a stray decimal mined from the banner
rather than the model's judgment. Every cycle misparsed.

### Same noise, two code paths

The identical preamble had already been observed breaking the **distillation**
pass ([#2496](https://github.com/rysweet/Simard/issues/2496); the regression is
pinned by the merged PR [#2500](https://github.com/rysweet/Simard/pull/2500)).
The hardened shared extractor from
[#2484](https://github.com/rysweet/Simard/issues/2484) /
[#2490](https://github.com/rysweet/Simard/issues/2490) already existed and was
already called by the decide/orient parsers — but its noise predicate did not
yet recognise the launcher-preamble *shape*. The fix is therefore not "apply the
extractor to a new path"; it is "teach the one shared predicate the launcher
shape", which re-hardens every consumer at once.

---

## The design: one chokepoint, launcher-shape aware

Simard strips ANSI and recipe-runner noise in exactly **one** place:
`src/recipe_output/extract.rs`. `strip_recipe_noise` runs `strip_ansi` and then
drops whole non-payload lines via the `is_noise_line` predicate. Decide, orient,
engineer-lifecycle, merge-judge, the progress checker, and distillation all flow
through it (see
[Text-parsing wire formats § Protocol 0](../reference/text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output)).

The fix extends `is_noise_line` with one new launcher-only arm,
`is_copilot_launcher_line`, that recognises the preamble lines the Copilot CLI
emits before its answer:

- `NODE_OPTIONS=…` info-marker lines (the `ℹ … (saved preference)` line),
- `Run 'copilot update' …` update nags,
- `launching copilot binary=… version="GitHub Copilot CLI …"` launcher lines,
- leading `INFO`/`WARN`-prefixed launcher lines that carry **no** ISO timestamp.

Because the change is at the single chokepoint, the entire brain class —
decide, orient, lifecycle, merge-judge — plus distill is re-hardened by one
edit. The next time the Copilot CLI reshapes its banner, there is again exactly
one predicate to update.

### Correctness as safety: never eat the payload

`is_copilot_launcher_line` consumes **untrusted agent stdout**. The cardinal
risk is over-matching — dropping a line that is actually the decision or part of
a JSON payload — which would silently corrupt OODA control flow. The predicate
is therefore deliberately conservative:

- It matches **anchored launcher shapes only** (literal `starts_with`/`contains`
  on known launcher prefixes), never a generic heuristic.
- It never matches a line that begins (after trimming) with a JSON **structural
  token** — `{` (a whole object), `"` (a pretty-printed object member such as a
  fact `"content": …` line), or `[` (an array element). This structural-token
  guard is an **absolute, code-enforced** exemption
  ([#2570](https://github.com/rysweet/Simard/issues/2570)): without it the
  `contains`-based `launching copilot binary=` / `version="GitHub Copilot CLI`
  arms would drop a pretty-printed fact `"content"` line that legitimately quotes
  one of those launcher substrings, silently emptying the fact and letting the
  distill reliability gate quarantine it. (The `launching…` / `version=…` arms
  match by **containment**, so the payload-safety guarantee is precisely that a
  line *leading with* a structural token is preserved; a first-word action
  keyword, bare decimal, or verdict keyword is likewise not one of the anchored
  launcher shapes.)
- ANSI is stripped **before** matching, so colour control bytes cannot smuggle a
  launcher line past the anchors or a payload line into a launcher match.
- Processing is single-pass and bounded; the
  [clean-path guarantee](../reference/text-parsing-wire-formats.md#clean-path-guarantee)
  (zero-copy `Cow::Borrowed` on noise-free input) is preserved, so adopting the
  stricter predicate changes nothing for clean output — only previously-poisoned
  noisy output now recovers.

This is the same principle the rest of `recipe_output` follows: the extractor is
permissive about *surrounding* prose but never discards a line that could be the
answer.

### Log hygiene

The launcher preamble embeds environment-derived data (the `NODE_OPTIONS`
value, the binary path, the CLI version, and a saved-preference config path).
The new parse-failure telemetry (below) emits a **classification plus a bounded
snippet**, never the raw preamble or full filesystem paths, so dropped launcher
noise does not leak environment detail into logs or metrics.

---

## Keeping a parse failure distinct from a real "no action"

The deterministic default is a genuine safety net and stays in place. But a
default reached because a transient parse miss exhausted the ladder is **not**
the same event as the model deliberately choosing to do nothing, and the two
must not be conflated — conflating them is what let a poisoned-input stall
masquerade as healthy "the brain decided NO ACTION" behaviour.

The escalation ladder already reports **why** it stopped via
`LadderTermination` (`Recovered`, `Exhausted`, `InvokeError`, `Disabled`) and
classifies each parse via `LifecycleParseOutcome::is_parse_failure()`. With this
fix, every recipe-backed brain phase **wires that termination cause through to
telemetry**:

- **Decide** and **orient** no longer discard the ladder's termination reason.
  When the deterministic default fires because the ladder `Exhausted` (or hit an
  `InvokeError`) on a parse failure, the phase logs it **distinctly** — tagged as
  a parse-failure default, not a model decision — and records the cause label on
  `brain_verdict_parsed_total`.
- **Engineer-lifecycle** emits a loud, distinct log when its `continue_skipping`
  default is reached via `is_parse_failure()` on an `Exhausted`/`InvokeError`
  termination, stating that the skip is a **transient parse-failure skip that is
  re-evaluated next cycle** — explicitly NOT a deliberate NO-ACTION. The
  conservative default itself is unchanged; only its visibility improves.

The practical effect: a goal with actionable work is no longer silently parked
under a "NO ACTION" that was really a parse miss. The miss is loud, attributable
to its termination cause, and self-clearing on the next cycle once the input is
clean — which, with launcher stripping in place, it now is.

---

## Why this is the right layer

Three alternatives were rejected:

1. **Strip in the Copilot adapter / `base_type_copilot` transcript path.** That
   path captures raw agent stdout, but the brain decision output now arrives
   inside the `recipe-runner-rs --output-format json` envelope's
   `step_results[].output`, parsed by `recipe_output`. Cleaning only the
   transcript path would miss the envelope consumers (decide, orient,
   merge-judge) — the exact phases that deadlocked.
2. **Add a launcher cleaner to each parser.** That re-introduces the duplicate
   strippers [#2484](https://github.com/rysweet/Simard/issues/2484) consolidated,
   and guarantees drift the next time the banner changes.
3. **Loosen the parsers to "find a keyword anywhere".** Keyword-anywhere
   scanning is what the text-based protocol deliberately removed
   ([text-based brain protocol](./text-based-brain-protocol.md)); it trades one
   class of misparse for another and would scrape the version string `1.0.66-2`
   as a decimal even more readily.

Extending the single `is_noise_line` predicate keeps one stripping path, one set
of tests, and one place to update — and it leaves the escalation ladder and the
deterministic defaults exactly where they belong: as rarely-needed safety nets,
not as the load-bearing parse path.

---

## See also

- [Reference: text-parsing wire formats](../reference/text-parsing-wire-formats.md) — normative Protocol 0 grammar and the `is_copilot_launcher_line` predicate.
- [Reference: recipe-brain verdict/decision parsing](../reference/recipe-brain-verdict-parsing.md) — the shared transport, escalation ladder, and termination-cause telemetry.
- [Reference: distill recipe output capture](../reference/distill-recipe-output-capture.md) — how distillation delegates launcher stripping to the same chokepoint.
- [How-to: diagnose OODA decide/orient brain parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook, including the launcher-banner cause and its log signatures.
- [Concept: text-based brain protocol](./text-based-brain-protocol.md) — why Simard parses text tokens, not JSON, from model output.
