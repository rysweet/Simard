---
title: "Reference: simard cognition record-brain-introspection / record-self-quality-audit (typed brain/audit records)"
description: >
  The two zero-privilege CLI tools the brain-introspection and monthly self-quality-audit
  recipes call as their final ACT step to record exactly one typed, validated
  BrainIntrospectionRecord / SelfQualityAuditRecord. Covers both record schemas, the
  fail-CLOSED R1–R7 read matrix (read_verified_brain_introspection /
  read_verified_self_quality_audit), the time-based freshness/anti-replay model,
  configuration, security, and worked examples. These tools retire the last two
  brittle text-marker scrapers (parse_brain_introspection_text /
  parse_self_quality_audit_text): the rail now reads a typed record instead of hunting
  for markers in concatenated step output. Shipped for issue #4968.
last_updated: 2026-07-29
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./brain-introspection-api.md
  - ./self-quality-audit-api.md
  - ./simard-cognition-record-thread-reasoning-cli.md
  - ./ooda-record-decision-cli.md
  - ../concepts/agentic-recipes-first-principle.md
  - ../howto/configure-brain-introspection.md
  - ../howto/configure-self-quality-audit.md
  - ../index.md
---

# Reference: `simard cognition record-brain-introspection` / `record-self-quality-audit`

> Shipped for issue [#4968](https://github.com/rysweet/Simard/issues/4968) — the
> follow-on to epic #4719 that retires the **last two** brittle-parse survivors.
> Writer verbs: `src/operator_cli/cognition.rs`
> (`dispatch_record_brain_introspection`, `dispatch_record_self_quality_audit`).
> Record types + readers: `src/brain_introspection_record.rs`
> (`BrainIntrospectionRecord`, `read_verified_brain_introspection`) and
> `src/self_quality_audit_record.rs`
> (`SelfQualityAuditRecord`, `read_verified_self_quality_audit`).
> Reader call sites: `src/brain_introspection.rs` (`run_brain_introspection`) and
> `src/self_quality_audit.rs` (`run_self_quality_audit`).
> Recipes: `prompt_assets/simard/recipes/brain-introspection.yaml`,
> `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`.

These two gated CLI tools are the **ACT step** the brain-introspection and monthly
self-quality-audit recipes call **exactly once** at the end of a run to hand their
result back to the Rust rail. Each recipe reasons about its own domain (brain health,
patterns, prune candidates / audit waves, PRs, crusty verdicts) and then writes a
single typed record. The thin Rust rail reads that record **fail-closed** and builds
the corresponding `BrainIntrospectionReport` / `SelfQualityAuditReport`.

!!! success "This is what killed the last text-marker scrapers"
    Before these tools existed, both adapters concatenated every recipe step's
    stdout and hunted for terminal markers the orchestrator "prints last"
    (`BRAIN_HEALTH:`, `AUDIT_COMPLETE=`, `PR_OPENED=`, …) via
    `parse_brain_introspection_text` / `parse_self_quality_audit_text` over
    `step_results[*].output`. That is the exact brittle-parsing antipattern epic
    #4719 targets — fragile to any step emitting extra text, reordering, or format
    drift. Both scrapers are **deleted**. The recipe's prose is now irrelevant; only
    the typed record it writes is read back. See
    [The typed-record contract](../concepts/agentic-recipes-first-principle.md).

!!! info "Normative contracts (this spec is the source of truth)"
    The following strings are fixed by this document and the implementation MUST match
    them exactly:

    1. **Schema pins** — `"brain-introspection/v1"` and `"self-quality-audit/v1"`.
    2. **Record paths** — `state_root/brain_introspection/record.json` and
       `state_root/self_quality_audit/record.json` (one file per adapter; the rail
       supplies it via the recipe's `-c record_path` context var and **pre-truncates**
       it before spawning the recipe).
    3. **Freshness window** — `MAX_AGE_SECS = 300` (5 min); `MTIME_SLACK = 2s`.

## What these tools do (and do not do)

Both tools hold **zero privilege**. Each tool's sole side effect is writing one JSON
record file to the `--record-path` the rail supplied:

- They do **not** spawn engineers, mutate refs, propose goals, prune memory, open or
  merge PRs, or write to memory. Those effects are performed by the recipe's *other*
  tool calls (`gh`, the amplihack skills, memory RPCs run by the hook) — these verbs
  are purely the result-handoff channel.
- They do **not** call Python, kuzu, the network, or a memory socket, and hold no
  tokens.
- They never scrape stdout. The recipe's free-text prose is irrelevant; only the
  record is read back.

Separating *recording the result* (these tools) from *reading it* (the rail) is what
lets Rust stay a thin rail while all judgment lives in the recipe. It also makes the
read a **total, fail-closed function**: a recipe that "ran" but wrote no valid record
is a failure, never a silently-defaulted report.

---

## `simard cognition record-brain-introspection`

Called as the final ACT step of `brain-introspection.yaml`. Writes one
[`BrainIntrospectionRecord`](#the-brainintrospectionrecord-schema) carrying the
**recipe-owned** findings (the daemon-side hook still measures `live_memories`,
`sensory_pruned`, and `consolidated_facts` itself — those are never taken from the
recipe).

### Usage

```text
simard cognition record-brain-introspection \
  --record-path <ABSOLUTE_PATH> \
  --written-at-epoch <UNIX_SECONDS> \
  --brain-health "<finding>"  [--brain-health "<finding>" …]   # ≥1 required \
  [--pattern "<line>" …] \
  [--regression "<line>" …] \
  [--prune-candidate "<line>" …] \
  [--prune-requested <u32>] \
  [--issue-url <URL>]
```

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--record-path` | yes | **Absolute** path the rail supplied via the recipe's `-c record_path`. Must not contain `..`; hardened with `harden_record_path`. |
| `--written-at-epoch` | yes | Unix seconds the recipe stamps at write time. Defense-in-depth freshness check (R7). |
| `--brain-health` | yes (≥1) | Repeatable. Each a brain-health finding line (e.g. `"fallback rate 4.2% (baseline 1.1%)"`). At least one non-empty value required; each passed through the shared [`sanitize_line`](#field-validation) chokepoint. |
| `--pattern` | no | Repeatable. Recurring-pattern findings. |
| `--regression` | no | Repeatable. Regressions vs. the rolling baseline. |
| `--prune-candidate` | no | Repeatable. Human-readable value-bearing prune candidate descriptions. |
| `--prune-requested` | no | `u32` count of value-bearing prune candidates recommended (default `0`). The hook still clamps this to `enforce_prune_cap` before storing it. |
| `--issue-url` | no | URL of the created/updated brain-introspection GitHub issue. |

Every value is passed through the shared [`sanitize_line`](#field-validation)
chokepoint and the bounded-list caps. Unknown or duplicate scalar flags are
rejected — the tool never silently ignores an argument (validate-all-then-write-once).

### The `BrainIntrospectionRecord` schema

Serialized as JSON, owner-only `0o600`, one file per invocation at
`state_root/brain_introspection/record.json`:

```jsonc
{
  "schema": "brain-introspection/v1",        // BRAIN_INTROSPECTION_SCHEMA pin (R3)
  "written_at_epoch": 1793558400,            // recipe wall-clock stamp; R7 defense-in-depth
  "brain_health": [                          // ≥1 required (R5); bounded list
    "fallback rate 4.2% (baseline 1.1%)",
    "0-succeeded-action cycles: 3 of 40"
  ],
  "patterns": ["coverage-comment step flakes on cold CI"],
  "regressions": ["brain_lifecycle_decision parse-failure rate up 3.1x"],
  "prune_candidates": ["duplicate semantic fact #A/#B (superseded)"],
  "prune_requested": 4,                      // hook re-clamps to enforce_prune_cap
  "issue_url": "https://github.com/rysweet/Simard/issues/5012"
}
```

The record carries **only** the recipe-owned fields. The daemon-side hook measures
`live_memories`, `sensory_pruned`, and `consolidated_facts` from `get_statistics` /
`prune_expired_sensory` / `consolidate_episodes` and merges them into the final
`BrainIntrospectionReport` — those numbers are never trusted from the recipe. The old
advisory `CONSOLIDATED_FACTS=` echo is gone.

---

## `simard cognition record-self-quality-audit`

Called as the final ACT step of `monthly-self-quality-audit.yaml`. Writes one
[`SelfQualityAuditRecord`](#the-selfqualityauditrecord-schema).

### Usage

```text
simard cognition record-self-quality-audit \
  --record-path <ABSOLUTE_PATH> \
  --written-at-epoch <UNIX_SECONDS> \
  --waves-completed <0..5> \
  --summary-line "<terminal one-line summary>"   # required, non-empty \
  [--pr-opened <URL> …] \
  [--pr-merged <URL> …] \
  [--crusty-approved <URL> …] \
  [--crusty-unresolved <URL> …]
```

### Arguments

| Flag | Required | Description |
|---|---|---|
| `--record-path` | yes | **Absolute** rail-supplied path (`-c record_path`); hardened with `harden_record_path`. |
| `--written-at-epoch` | yes | Unix seconds stamped at write time (R7 defense-in-depth). |
| `--waves-completed` | yes | `u32` in `0..=5` — count of completed SEEK→VALIDATE→FIX waves. A value `> 5` is rejected. |
| `--summary-line` | yes | The terminal one-line audit summary. Non-empty after `sanitize_line`. |
| `--pr-opened` | no | Repeatable. URLs of PRs opened by a wave. |
| `--pr-merged` | no | Repeatable. URLs of self-merged PRs (crusty-approved AND CI-green). |
| `--crusty-approved` | no | Repeatable. URLs the `crusty-old-engineer` proxy approved. |
| `--crusty-unresolved` | no | Repeatable. URLs crusty left unsatisfied after ≤3 rounds (open, need human). |

`--summary-line` must resolve to non-empty text after sanitize. Unknown or
duplicate scalar flags are rejected (validate-all-then-write-once).

### The `SelfQualityAuditRecord` schema

Serialized as JSON, owner-only `0o600`, one file per invocation at
`state_root/self_quality_audit/record.json`:

```jsonc
{
  "schema": "self-quality-audit/v1",         // SELF_QUALITY_AUDIT_SCHEMA pin (R3)
  "written_at_epoch": 1793558400,            // recipe wall-clock stamp; R7
  "waves_completed": 5,                      // 0..=5
  "prs_opened":       ["https://github.com/rysweet/Simard/pull/5001", "…/5002"],
  "prs_merged":       ["https://github.com/rysweet/Simard/pull/5001"],
  "crusty_approved":  ["https://github.com/rysweet/Simard/pull/5001"],
  "crusty_unresolved":["https://github.com/rysweet/Simard/pull/5002"],
  "summary_line": "5 waves, 4 PRs opened, 3 merged, 1 crusty-unresolved"
}
```

`summary_line` is **required** and non-empty (R5) — it replaces the old required
`AUDIT_COMPLETE=<summary>` marker whose absence used to be a parse error.

---

## Field validation

Every string field crossing the filesystem is treated as **adversarial input** and
passes through the *same* shared chokepoints on both the writer and the reader, so
validation cannot drift:

- **`sanitize_line`** — strips C0/C1 control characters and ANSI escapes, folds
  whitespace, runs `secret_scrub` (so tokens or `AMPLIHACK_AGENT_BINARY` never land in
  the record or a log line), and rejects the value if it is empty after sanitize.
  Each element is capped at **256 bytes**; `summary_line` at **600 bytes**.
- **`bounded_list`** — enforces per-field element-count caps. **Over-count and
  over-byte inputs are hard-rejected, never truncated** — truncation is banned as a
  partial-acceptance anti-pattern. Bounds:

  | Field | Cap |
  |---|---|
  | `brain_health` | ≥1, ≤ 32 |
  | `patterns` | ≤ 64 |
  | `regressions` | ≤ 64 |
  | `prune_candidates` | ≤ 64 |
  | `prs_opened` / `prs_merged` / `crusty_approved` / `crusty_unresolved` | ≤ 128 each |

- **Closed schema** — both structs are `#[serde(deny_unknown_fields)]` with an exact
  `schema` pin. Additive fields are breaking-by-construction, forcing an explicit
  version bump.

The writer calls these chokepoints before persisting (reject ⇒ **no file written**);
the reader calls the identical functions (reject ⇒ a typed `Err`). A parity test proves
no drift.

---

## The fail-CLOSED read matrix (R1–R7)

`read_verified_brain_introspection(path, invoke_start)` and
`read_verified_self_quality_audit(path, invoke_start)` each return
`SimardResult<…Record>`. **Every failure mode is a distinct typed `Err`; the reader
never returns a defaulted, partial, or `unwrap_or_default` record.** This is the fix
for the original brittle-parse bug class — a "ran but produced nothing valid" recipe
can no longer be read as a silent success.

| # | Check | Fails closed when |
|---|---|---|
| R1 | File present & readable | The record is absent or unreadable at `path`. |
| R2 | Well-formed JSON | The bytes are not valid JSON (a torn/partial write is impossible — `persist_json` writes temp + fsync + rename). |
| R3 | Schema pin | `schema != "brain-introspection/v1"` / `"self-quality-audit/v1"`. |
| R4 | Closed-type parse & bounds | Any unknown top-level key (`deny_unknown_fields`), any over-count list, any over-byte string, or `waves_completed > 5`. |
| R5 | Required-field validity | `brain_health` empty (introspection) / `summary_line` empty (audit) after `sanitize_line`. |
| R6 | Owner-only permissions | The file is not mode `0o600` or is not owned by the reading euid. |
| R7 | Freshness / anti-replay | File `mtime < invoke_start − MTIME_SLACK`, **or** `now − mtime > MAX_AGE_SECS`, **or** `\|now_epoch − written_at_epoch\| > MAX_AGE_SECS`. |

Each `Err` carries a stable `code` (`R1`…`R7`) plus a scrubbed `detail`, so an operator
can tell exactly which check failed from the daemon log.

### Freshness / anti-replay model

Neither adapter has a `goal_id`/`cycle_number` (unlike the OODA readers) and their
cadences differ wildly (brain-introspection is daily, self-quality-audit is ~monthly),
so anti-replay is **time-based and path-based**, in three parts:

1. **Per-invocation fixed path + pre-truncate.** The rail derives
   `state_root/<adapter>/record.json` and **deletes any pre-existing file at that path
   immediately before spawning the recipe**. A leftover record from a prior day's or
   month's run can therefore never be read as current — this is the primary anti-replay
   guarantee, and it holds even though the two cadences differ.
2. **R6 identity = owner-only permissions.** The record must be `0o600` and owned by
   the reading process; a file planted by another user is rejected.
3. **R7 freshness = mtime window + embedded epoch.** The rail captures
   `invoke_start: SystemTime` **before** spawn and requires `mtime ≥ invoke_start −
   MTIME_SLACK` and `now − mtime ≤ MAX_AGE_SECS`; the embedded `written_at_epoch` is
   checked as defense-in-depth against mtime spoofing.

`MAX_AGE_SECS = 300` (5 min) and `MTIME_SLACK = 2s` are constants in each record
module. Because the record is written and read **within a single recipe run**, the
300 s window comfortably passes on the normal path while rejecting any stale artifact;
the 2 s slack tolerates recipe-runner spin-up clock skew.

---

## How the rails wire it in

The `record_path` is a **trust anchor supplied by the rail, not the recipe** — the
recipe cannot forge it, and neither can it forge `invoke_start` (the rail's own clock).
Both public report types, `.summary()`, `read_last_run`/`write_last_run`, and both
`run_*` signatures are **unchanged**; only the internal parse source changed.

### brain-introspection (best-effort — WARN + degrade)

`run_brain_introspection`:

1. **Before spawn:** compute `record_path = state_root/brain_introspection/record.json`,
   delete any stale file, capture `invoke_start`, pass `-c record_path=<abs>` to the
   recipe-runner `Command` (alongside the existing `-c state_root / repo_path /
   max_prune / baseline_runs / stats`).
2. Run the bounded, RPC-backed safe hygiene (`get_statistics`, `prune_expired_sensory`,
   `consolidate_episodes`) **before** the recipe spawn, exactly as today, so a recipe
   failure never voids the hygiene already performed.
3. **After the recipe returns:** call `read_verified_brain_introspection(record_path,
   invoke_start)`.
   - `Ok(rec)` ⇒ merge the recipe-owned fields with the hook-measured counts into the
     `BrainIntrospectionReport`.
   - `Err(e)` ⇒ inner `AdapterInvocationFailed{…}`. The **existing** outer contract is
     preserved: `run_brain_introspection` **degrades** — logs a `WARN`, keeps the
     bounded hygiene results, and returns `Ok(report)` with the hook-measured fields and
     empty recipe fields. The daemon loop continues.

### self-quality-audit (no-fallback — propagate)

`run_self_quality_audit`:

1. **Before spawn:** compute `record_path = state_root/self_quality_audit/record.json`,
   delete any stale file, capture `invoke_start`, pass `-c record_path=<abs>` (alongside
   `-c state_root / repo_path`).
2. **After the recipe returns:** call `read_verified_self_quality_audit(record_path,
   invoke_start)`.
   - `Ok(rec)` ⇒ build the `SelfQualityAuditReport`.
   - `Err(e)` ⇒ `AdapterInvocationFailed{…}` **propagates** unchanged; the daemon logs
     `WARN: self quality-audit failed: …`. The disk-backed `write_last_run(now_epoch)`
     still runs on **both** `Ok` and `Err` (prevents a failing recipe from hot-looping).

**No `unwrap_or`, no stdout fallback, in either rail.** The
`parse_brain_introspection_text` / `parse_self_quality_audit_text` functions, their
marker grammars, and the `RecipeOutput`/`StepResult` envelope-scrape path are deleted.

### Definition-of-done grep gate

The rework-contract guard (`src/cognitive_threads/tests_rework_contract.rs`) fails if
the scrape pattern or dead symbols reappear in either adapter:

```console
$ grep -rn 'parse_brain_introspection_text\|parse_self_quality_audit_text' src/
# (returns nothing)

$ grep -rn 'step_results\|\.output' src/brain_introspection.rs src/self_quality_audit.rs
# (returns nothing — no envelope scraping)
```

The guard also **requires** `read_verified_*` + `record_path` in the adapters, the
schema pin + `MAX_AGE_SECS` + `deny_unknown_fields` + `read_verified_*` in the record
modules, both new subcommand routes in `cognition.rs`, and a `record-*` verb call in
both recipe YAMLs.

---

## Configuration

| Knob | Where | Effect |
|---|---|---|
| `BRAIN_INTROSPECTION_SCHEMA = "brain-introspection/v1"` | `src/brain_introspection_record.rs` | Schema pin for R3. Bump only with a matching reader change. |
| `SELF_QUALITY_AUDIT_SCHEMA = "self-quality-audit/v1"` | `src/self_quality_audit_record.rs` | Schema pin for R3. |
| `MAX_AGE_SECS = 300` | both record modules | Freshness window for R7. |
| `MTIME_SLACK = 2` (secs) | both record modules | Tolerance on `mtime ≥ invoke_start` for recipe-runner spin-up. |
| record paths | `state_root/brain_introspection/record.json`, `state_root/self_quality_audit/record.json` | Created `0o700` parent; one file per adapter, pre-truncated each invocation. Ephemeral (TTL ≤ 300 s); no history/rotation. |

The adapters' existing operator knobs are unchanged — see
[Configure brain introspection](../howto/configure-brain-introspection.md)
(`SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS`, `…_MAX_PRUNE`, `…_BASELINE_RUNS`) and
[Configure the monthly self-quality-audit](../howto/configure-self-quality-audit.md)
(`SIMARD_SELF_AUDIT_INTERVAL`). This change only swaps the recipe→rail handoff channel.

---

## Security summary

- **Zero privilege.** Each verb makes one write to the hardened `--record-path`; no env
  mutation, network, memory socket, or spawn.
- **Single privileged writer via gated verbs only.** No bypass flags — `--admin`,
  `--no-verify`, `--force` are forbidden and never accepted.
- **Rail owns the trust anchors.** `invoke_start` (`SystemTime::now()` before spawn) and
  `record_path` (rail-supplied, pre-truncated) cannot be forged by the recipe.
- **Total fail-closed reader.** Distinct typed `Err{code}` per R1–R7; never
  `unwrap_or_default`, never a defaulted or partial record.
- **Reject, never truncate.** Over-count lists and over-byte strings hard-reject (R4) at
  both writer and reader.
- **Closed schema.** `#[serde(deny_unknown_fields)]` + exact schema-version pin (R3/R4).
- **One chokepoint per field.** Writer and reader call identical `bounded_list` /
  `sanitize_line`; a parity test proves no drift.
- **Path hardening.** `harden_record_path` (absolute + reject `..`) runs on every path
  flag before any file op.
- **Confidentiality + integrity via `persist_json`.** `0o600` owner-only, atomic
  temp + fsync + rename + parent-fsync (no torn reads).
- **Scrub before persist AND before log.** `secret_scrub` + ANSI/C0 strip +
  whitespace-fold on every string on both write and read paths.
- **Anti-replay/freshness.** `mtime ≥ invoke_start − 2s`, `now − mtime ≤ 300s`,
  `|now_epoch − written_at_epoch| ≤ 300s`; the rail pre-truncates the path before spawn
  so a prior-run file cannot pass.
- **No new crypto, no new trust boundary.** All requirements are satisfied by reusing
  proven primitives (`harden_record_path`, `persist_json`, `secret_scrub`,
  `deny_unknown_fields`, the R1–R7 reader). Same-uid channel; no network/HTTP/multi-tenant
  surface.

---

## Examples

Brain-introspection recipe recording its findings (final ACT step):

```bash
simard cognition record-brain-introspection \
  --record-path /home/simard/.simard/brain_introspection/record.json \
  --written-at-epoch 1793558400 \
  --brain-health "fallback rate 4.2% (baseline 1.1%)" \
  --brain-health "0-succeeded-action cycles: 3 of 40" \
  --pattern "coverage-comment step flakes on cold CI" \
  --regression "brain_lifecycle_decision parse-failure rate up 3.1x" \
  --prune-candidate "duplicate semantic fact #A/#B (superseded)" \
  --prune-requested 4 \
  --issue-url "https://github.com/rysweet/Simard/issues/5012"
```

First brain-introspection run (empty baseline — still writes a valid record):

```bash
simard cognition record-brain-introspection \
  --record-path /home/simard/.simard/brain_introspection/record.json \
  --written-at-epoch 1793558400 \
  --brain-health "no prior baseline"
```

Self-quality-audit recipe recording a completed month (final ACT step):

```bash
simard cognition record-self-quality-audit \
  --record-path /home/simard/.simard/self_quality_audit/record.json \
  --written-at-epoch 1793558400 \
  --waves-completed 5 \
  --pr-opened  "https://github.com/rysweet/Simard/pull/5001" \
  --pr-opened  "https://github.com/rysweet/Simard/pull/5002" \
  --crusty-approved  "https://github.com/rysweet/Simard/pull/5001" \
  --crusty-unresolved "https://github.com/rysweet/Simard/pull/5002" \
  --pr-merged "https://github.com/rysweet/Simard/pull/5001" \
  --summary-line "5 waves, 4 PRs opened, 3 merged, 1 crusty-unresolved"
```

## See also

- [Brain introspection API](./brain-introspection-api.md) — the daemon hook that reads
  this record and merges it with hook-measured memory counts.
- [Self-quality-audit API](./self-quality-audit-api.md) — the monthly hook that reads
  this record.
- [`simard cognition record-thread-reasoning` CLI](./simard-cognition-record-thread-reasoning-cli.md) —
  the sibling typed-record tool (Groups A–C) whose contract these two extend.
- [Agentic recipes first](../concepts/agentic-recipes-first-principle.md) — why the
  recipe ACTs via a typed record instead of emitting markers Rust scrapes.
