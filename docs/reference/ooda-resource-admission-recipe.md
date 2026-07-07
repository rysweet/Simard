---
title: OODA resource-admission recipe and prompt schema
description: Reference for the ooda-resource-admission recipe and prompt — the structured-reasoning asset that decides ADMIT / DEFER / RECLAIM-FIRST, its context variables, the decision envelope, and runtime hot-reload.
last_updated: 2026-07-07
owner: simard
doc_type: reference
related:
  - ../concepts/resource-aware-engineer-admission.md
  - ./resource-aware-admission-api.md
  - ../howto/configure-resource-aware-admission.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ./ooda-decide-prompt.md
  - ./recipe-context-var-sanitization.md
---

# OODA resource-admission recipe and prompt schema

**Recipe:** `prompt_assets/simard/recipes/ooda-resource-admission.yaml`
**Prompt asset:** `prompt_assets/simard/prompts/ooda-resource-admission.md`
**Shim:** `impl OodaAdmissionBrain for RecipeBrain` (`src/ooda_brain/recipe_brain.rs`)

This recipe is the **single source of truth for the resource-admission
decision**. It is where the intelligence of the feature lives — the Rust seam
only gathers structured context, calls this recipe, and applies the result under
one hard rail (see the [API reference](./resource-aware-admission-api.md)).

It follows the same recipe-brain pattern as
[`ooda-decide.yaml`](./ooda-decide-prompt.md) and
[`ooda-engineer-lifecycle.yaml`](./ooda-engineer-lifecycle-recipe.md): a single
terminal `agent` step run via `recipe-runner-rs`, whose stdout is a JSON
envelope from which `RecipeBrain` extracts the final step's output and parses the
decision.

---

## Recipe layout

```yaml
name: "ooda-resource-admission"
description: "OODA engineer-admission brain — resource-aware ADMIT/DEFER/RECLAIM-FIRST"
version: "1.0.0"
author: "Simard"
tags: ["simard", "ooda", "admission", "resource"]

context: {}

steps:
  - id: "decide-admission"
    type: "agent"
    agent: "default"
    prompt: |
      {{escalation_note}}

      # OODA Brain — Engineer-Admission Decision (resource-aware)

      ## ROLE
      You are the admission brain for Simard's engineer dispatcher. The count
      cap already said there is room for one more engineer by COUNT. Your job is
      the resource question the count cap cannot answer: given the current disk,
      build-cache, and load picture on THIS host, is now a good moment to spawn
      one more engineer — each of which starts its own parallel `cargo` build?

      A full disk is fatal (ENOSPC kills recipes). Be conservative when disk is
      high or load is thrashing. Prefer reclaiming reclaimable space over
      blocking progress outright.

      ## CONTEXT
      - disk_usage_pct:        {{disk_usage_pct}}
      - disk_ceiling_pct:      {{ceiling_pct}}
      - worktree_cache_bytes:  {{worktree_cache_bytes}}
      - load_avg_1m:           {{load_avg_1m}}
      - cpu_count:             {{cpu_count}}
      - in_flight_engineers:   {{in_flight_engineers}}

      ## OPTIONS
      Choose exactly one:
      - admit         — resources are healthy; add the engineer.
      - defer         — resources are tight; skip this cycle, retry next.
      - reclaim_first — disk is high but reclaimable (stale worktrees / caches);
                        reclaim now, then defer this cycle.

      ## OUTPUT
      Emit ONE JSON object and nothing else:
      {"choice": "<admit|defer|reclaim_first>", "rationale": "<one sentence>"}

      ## EXAMPLES
      … (see Examples below) …
```

The recipe is a single terminal `agent` step. `recipe-runner-rs` handles
Handlebars rendering, agent invocation, and stdout capture; `RecipeBrain` parses
the decision from the JSON envelope.

---

## Context variables

`recipe-runner-rs` performs Handlebars `{{name}}` substitution from the context
vars the shim passes (via `-c name=value`), sourced from
[`ResourceAdmissionCtx`](./resource-aware-admission-api.md#resourceadmissionctx).
`Option` fields render as the literal string `unknown` when `None`.

| Variable | Source field | `None` renders as |
|---|---|---|
| `{{disk_usage_pct}}` | `disk_usage_pct` | `unknown` |
| `{{ceiling_pct}}` | `ceiling_pct` | (never `None`) |
| `{{worktree_cache_bytes}}` | `worktree_cache_bytes` | `unknown` |
| `{{load_avg_1m}}` | `load_avg_1m` | `unknown` |
| `{{cpu_count}}` | `cpu_count` | `unknown` |
| `{{in_flight_engineers}}` | `in_flight_engineers` | (never `None`; `0` on error) |

Every context var is a **daemon-computed number** (or the literal `unknown` for a
failed probe) — no goal- or LLM-supplied string ever reaches this recipe, so the
prompt-injection surface is minimal. (Unlike the decide/orient/lifecycle recipes,
admission takes no free-text context var and therefore no `escalation_note`.)

---

## Decision envelope (wire format)

The agent's final step output is the JSON decision object. `RecipeBrain`:

1. Runs `recipe-runner-rs … --output-format json`.
2. Deserializes the envelope and extracts the **final** step's `output`
   (`extract_recipe_decision_output` — the shared parse chokepoint; a
   `success=false` recipe or empty step surfaces
   `SimardError::AdapterInvocationFailed`, never a silent empty).
3. Parses that output into
   [`AdmissionDecision`](./resource-aware-admission-api.md#admissiondecision) —
   `parse_admission_decision` runs the output through the shared
   `recipe_output::extract_json_payload` sanitizer (strips banner / ANSI / log
   noise) and then `serde_json::from_str` on the
   `#[serde(tag = "choice", rename_all = "snake_case")]` enum.

```json
{"choice": "admit",         "rationale": "disk 61%, load 2.1 on 16 cpus — healthy"}
{"choice": "defer",         "rationale": "load 22 on 8 cpus; builds thrashing"}
{"choice": "reclaim_first", "rationale": "disk 88% but 30 stale worktrees reclaimable"}
```

| `choice` | Parses to |
|---|---|
| `admit` | `AdmissionDecision::Admit { rationale }` |
| `defer` | `AdmissionDecision::Defer { rationale }` |
| `reclaim_first` | `AdmissionDecision::ReclaimFirst { rationale }` |

### Parse failure — NO FALLBACK

Admission uses a **direct parse**, not the confidence-gated escalation ladder the
other recipe brains use. On a parse-miss (non-JSON output, unknown `choice`,
missing `rationale`), `judge_admission` returns
`SimardError::AdapterInvocationFailed` immediately — **no ladder retry, no
`escalation_note`, and NO FALLBACK**. The seam surfaces that `Err` as a visible
cycle failure (`success=false`); it never fabricates an `Admit` from a malformed
response. (An accidental admit could fill the disk, so a broken brain must fail
loud.) The deterministic disk ceiling in `resolve_admission` is the separate,
always-on ENOSPC guard.

The admission path emits a **single** metric, `brain_admission_decision` (which
`choice` the brain made, plus the numeric picture). It deliberately does **not**
flow through the shared verdict-parse chokepoint, so there is no
`brain_verdict_parsed_total{phase="resource_admission"}` series; see the
[API reference observability section](./resource-aware-admission-api.md#observability).

---

## Runtime loading (hot-reload, not compile-time)

The recipe is loaded at runtime by `recipe-runner-rs`. `RecipeBrain` resolves the
recipe path in this order (same as the other recipes):

1. **Hot-reload:** `~/.simard/prompt_assets/simard/recipes/ooda-resource-admission.yaml`
2. **In-tree:** `<repo_root>/prompt_assets/simard/recipes/ooda-resource-admission.yaml`

Prompt edits take effect on the **next admission cycle without a rebuild**. Bump
the `version:` field when you change decision-affecting wording so the change is
traceable in the judgment record's prompt-version field.

---

## Examples

### `admit` — healthy host

Context: `disk_usage_pct=58 ceiling_pct=90 worktree_cache_bytes=8589934592 load_avg_1m=3.2 cpu_count=16 in_flight_engineers=4`

Agent output:

```json
{"choice": "admit", "rationale": "disk 58% well under the 90% ceiling; load 3.2 on 16 cpus is comfortable — room for another engineer"}
```

Parsed:

```rust
AdmissionDecision::Admit {
    rationale: "disk 58% well under the 90% ceiling; load 3.2 on 16 cpus is comfortable — room for another engineer".into(),
}
```

Gate outcome: **Admit** → proceed to spawn.

### `defer` — load thrashing

Context: `disk_usage_pct=70 ceiling_pct=90 worktree_cache_bytes=12884901888 load_avg_1m=26.5 cpu_count=8 in_flight_engineers=9`

Agent output:

```json
{"choice": "defer", "rationale": "load 26.5 on only 8 cpus with 9 engineers already building; another parallel cargo build would thrash — wait a cycle"}
```

Gate outcome: **Defer** → benign skip, retry next cycle.

### `reclaim_first` — high but reclaimable disk

Context: `disk_usage_pct=87 ceiling_pct=90 worktree_cache_bytes=64424509440 load_avg_1m=5.1 cpu_count=16 in_flight_engineers=6`

Agent output:

```json
{"choice": "reclaim_first", "rationale": "disk 87% approaching the 90% ceiling, but 60G sits in worktree build caches — reclaim before admitting"}
```

Gate outcome: reclaim recipe runs, then **Defer** this cycle.

### Hard rail overrides `admit` above ceiling

Context: `disk_usage_pct=93 ceiling_pct=90 …`

Even if the agent emits:

```json
{"choice": "admit", "rationale": "load looks fine"}
```

`resolve_admission` **downgrades to `Defer`** because `disk_usage_pct=93 ≥ ceiling=90`.
The model can only be overridden toward caution, never toward filling the disk.

### Unknown disk fails open

Context: `disk_usage_pct=unknown ceiling_pct=90 …` (a `df` probe failure)

Agent output `{"choice":"admit", …}` → gate outcome **`Proceed`**. The hard rail
does not fire on unknown disk; the reasoner's judgment stands.

---

## Versioning & compatibility

Cosmetic edits (ROLE phrasing, examples, rationale guidance) are safe to ship
alone and take effect on the next cycle without a rebuild.

Adding a new admission outcome requires a coordinated change:

1. Add the variant to `AdmissionDecision` in `src/ooda_brain/admission.rs`.
2. Extend the `label` / `rationale` accessors.
3. Handle it in `resolve_admission` (map to the appropriate `AdmissionGate`).
4. Add it to the `OPTIONS` and `OUTPUT` sections of this recipe.
5. Add an example here and to the [wire-formats reference](./text-parsing-wire-formats.md).
6. Add a hermetic gate test to `admission_tests.rs`.
7. Bump the recipe `version:`.

---

## See also

- [Resource-aware engineer admission (concept)](../concepts/resource-aware-engineer-admission.md)
- [Resource-aware admission API reference](./resource-aware-admission-api.md)
- [Configure resource-aware admission (how-to)](../howto/configure-resource-aware-admission.md)
- [OODA Engineer-Lifecycle Recipe](./ooda-engineer-lifecycle-recipe.md) — the sibling recipe this mirrors
- [OODA Decide Prompt Schema](./ooda-decide-prompt.md) — the tagged-envelope sibling
- [Recipe context-var sanitization](./recipe-context-var-sanitization.md) — how string context is cleaned
- [Recipe-Brain API](./recipe-brain-api.md) — the envelope-parse chokepoint and escalation ladder
