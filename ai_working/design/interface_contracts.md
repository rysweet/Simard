# Interface & Data Contracts — Issue #2669 (Step 5b)

**Scope decision:** This change has **no network API** (no REST/GraphQL/HTTP
endpoint, no wire protocol, no service boundary). The traditional API-design
concerns — endpoint definitions, request/response envelopes over the wire,
HTTP error codes — are **N/A** and correctly skipped.

Two real contract surfaces do change and are specified here:

1. **Module interfaces (the brick "studs")** — exact Rust signatures the P0
   bricks expose/consume, so downstream implementation and tests bind against a
   fixed shape.
2. **The `metrics.jsonl` data contract** — the JSON `context` string is an
   append-only schema read by *other* tools (`self_metrics::query_metrics`,
   `recent_metrics`, `daily_report`, operator dashboards). Adding the
   `parse_recovery` discriminator (Design §4 / Decision D-1) is a schema change
   and gets an explicit versioning rule.

Signatures below are verified against this worktree's source (line refs are the
current base). This document is authoritative for the P0 contract surface and
refines Design-Spec §4.

---

## 1. Module interface: `recipe_output::strip_json_trailing_commas` (NEW — Brick A)

**File:** `src/recipe_output/extract.rs` (joins `balanced_objects`,
`scan_balanced`, `strip_ansi`, `strip_recipe_noise`).

### Signature (the stud)

```rust
/// Remove JSON-illegal trailing commas — a `,` whose next non-whitespace byte
/// is `}` or `]` — that make strict `serde_json` reject an otherwise
/// well-formed value.
///
/// STRING-AWARE: commas, braces, and brackets inside JSON string literals are
/// never touched (mirrors `scan_balanced`'s `in_string`/`escaped` machine).
///
/// Returns `Cow::Borrowed(s)` unchanged when no trailing comma is present, so
/// the common (clean) parse path allocates nothing. Total and pure: never
/// panics, never returns `Result` — strict `serde_json` downstream remains the
/// sole arbiter of validity.
pub fn strip_json_trailing_commas(s: &str) -> std::borrow::Cow<'_, str>;
```

### Contract

| Aspect | Guarantee |
|---|---|
| Input | Any `&str`. Intended: a brace-balanced, noise-stripped span from `balanced_objects` / the fast path, but the fn is total over arbitrary input. |
| Output | `Cow::Borrowed` iff byte-identical (no offending comma); else `Cow::Owned` with only trailing commas removed. |
| I1 (string-safety) | Bytes inside `"…"` preserved exactly, honoring `\"` escapes. `{"c":"a,}"}` is returned unchanged. |
| I2 (minimality) | Removes **only** a `,` whose next non-ws (` \t\r\n`) byte is `}` or `]`. No other byte is altered. Trailing comma before EOF (no closer) is left as-is (still invalid → still `Err` downstream). |
| I3 (purity) | No panic, no allocation on the clean path, no `Result`. |
| I4 (idempotence) | `f(f(x)) == f(x)`. |
| Non-goals | No json5, comments, quote-fixing, or single→double quote. Trailing commas only. |

### Errors
None by construction. A still-malformed stripped result simply fails the
subsequent strict `serde_json::from_str`, preserving the retry-safe deferral.

---

## 2. Module interface: `scan_cleaned_for_facts` (MODIFY — Brick B)

**File:** `src/memory_consolidation/distillation.rs:1289`.

### Signature — UNCHANGED
```rust
fn scan_cleaned_for_facts(trimmed: &str) -> Option<DistillOutput>;
```

### Revised internal contract (per-candidate parse order)
For the fast path (L1291) and **each** balanced span (L1312), attempt in order:

1. `serde_json::from_str::<RecipeEnvelope>(candidate)` — strict, first.
2. **On `Err` only:** `serde_json::from_str::<RecipeEnvelope>(`
   `strip_json_trailing_commas(candidate).as_ref())` — strict parse of the
   stripped view.

Recovery is attempted **strictly after** strict fails; it changes only whether a
candidate *parses at all*. The three existing preference tiers
(grounded-capable → non-empty → empty, L1303–1305) and the reverse (END-first)
span iteration are **unchanged**.

| Guarantee | Detail |
|---|---|
| Never a hollow `Ok` | Genuinely malformed input parses under neither view → candidate skipped → `None` → `parse_facts_document` returns `Err` → caller defers (retry-safe). |
| Tier ordering intact | A `Recovered` grounded-capable span still beats a `StrictOk` empty span, etc. Recovery does not reorder preference. |
| No new leniency | Only trailing-comma removal; field-level leniency (`de_lenient_string`) is untouched. |

**Consumes:** `recipe_output::strip_json_trailing_commas` (Brick A). Zero new
coupling to distillation types (Brick A is generic over `&str`).

---

## 3. Module interface: `RecipeEnvelope::into_output` / `into_facts` (MODIFY — Brick C)

**File:** `distillation.rs:1461` (`into_facts`), `:1497` (`into_output`).

### Signatures — UNCHANGED
```rust
fn into_facts(self) -> Vec<DistilledFact>;
fn into_output(self) -> DistillOutput;
```

### Revised behavior
Emit a **distinct** structured warning when an envelope **parsed successfully**
but **every** fact was dropped by the `canonical_distill_concept` category
filter — distinguishing "parsed OK, 0 facts survived filter" from a parse
failure. Placement: `into_output` (it holds pre-filter `facts.len()` and the
post-filter result), not `into_facts` (called twice via split envelopes).

```rust
tracing::warn!(
    target: "simard::distill",
    input_facts = <pre_filter_len>,
    kept_facts  = 0,
    "distill: envelope parsed but all facts were dropped by the category filter \
     (pr-pattern|bug-pattern|lesson-learned); not a parse failure",
);
```

| Guarantee | Detail |
|---|---|
| Fires only on real loss | Warn **iff** `pre_filter_len > 0 && kept == 0`. A legitimately empty `{"facts":[]}` envelope (pre_filter_len == 0) does **not** warn (R4). |
| Side-channel only | Logging only; return type and control flow unchanged. |
| Target namespace | `simard::distill`, matching existing distill warns for operator filtering. |

---

## 4. Data contract: `metrics.jsonl` context (MODIFY — Brick D / Decision D-1)

This is the closest thing to a "request/response schema" in this change: the
`context` field of a `MetricEntry` is a JSON object read by external consumers.

### Producer signature — UNCHANGED
```rust
// src/self_metrics/mod.rs:37
pub fn record_metric(metric_name: &str, value: f64, context: &str)
    -> Result<(), Box<dyn std::error::Error>>;
```
`MetricEntry { timestamp, metric_name, value, context }` — one JSON object per
line in `~/.simard/metrics/metrics.jsonl`.

### Context builder — signature EXTENDED
```rust
// distillation.rs:825 — add ONE parameter (append-only)
fn build_distill_success_context(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,
    attempt: u32,
    recovered_after_retry: bool,
    parse_recovery: ParseRecovery,   // NEW
) -> String;
```

### NEW enum — the discriminator
```rust
/// How the *facts document* for one pass was parsed. Orthogonal to
/// `recovered_after_retry` (which is about whole-runner RE-INVOCATIONS, #2468);
/// `ParseRecovery` is about how the SAME document's span parsed within one
/// invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseRecovery {
    /// Strict `from_str` succeeded on the raw candidate — no repair.
    StrictOk,
    /// Strict failed on the raw candidate but succeeded after
    /// `strip_json_trailing_commas` (Brick B). The #2669 fix path.
    Recovered,
    /// No candidate parsed under either view → pass deferred (`Err`).
    Deferred,
    /// A candidate parsed but 0 facts survived the category filter (Brick C).
    ZeroFacts,
}

impl ParseRecovery {
    /// Stable label for the metric context. NEVER rename these strings —
    /// downstream `metrics.jsonl` readers key on them.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StrictOk  => "strict-ok",
            Self::Recovered => "recovered",
            Self::Deferred  => "deferred",
            Self::ZeroFacts => "zero-facts",
        }
    }
}
```

### Context JSON schema (`distill_success_rate` / `distill_parse_success_rate`)

| Key | Type | Status | Notes |
|---|---|---|---|
| `outcome` | `"success"\|"failure"` | existing | unchanged |
| `recipe_exited_ok` | bool | existing | unchanged |
| `parse_attempted` | bool | existing | unchanged |
| `parse_success` | bool | existing | unchanged |
| `failure_class` | string \| null | existing | `DistillFailureClass::as_str` |
| `input_count` | u32 | existing | unchanged |
| `fact_count` | u32 | existing | unchanged |
| `attempt` | u32 | existing | 1-based runner invocations |
| `recovered_after_retry` | bool | existing | recovered via runner RE-INVOCATION (#2468) |
| **`parse_recovery`** | **string** | **NEW** | one of `strict-ok\|recovered\|deferred\|zero-facts` |

**`parse_recovery` vs `recovered_after_retry` (critical, do not conflate):**
- `recovered_after_retry` → success followed a **new whole-runner invocation**
  (`attempt > 1`). Axis = *re-run the agent*.
- `parse_recovery=recovered` → the **same** facts document parsed only after
  trailing-comma stripping, **within one invocation**. Axis = *repair the bytes*.
  Both can be true independently.

### Versioning strategy (data contract)
- **Append-only, additive.** New key added; **no existing key renamed, removed,
  or retyped.** Readers that ignore `parse_recovery` are unaffected — the
  success-rate means over `distill_success_rate` / `distill_parse_success_rate`
  are numerically **identical** to before this change.
- **No explicit `schema_version` field is introduced** — `metrics.jsonl` has
  never carried one and the mean-based consumers are forward-compatible with
  unknown keys. Introducing versioning now would be scope creep (R6).
- **Label stability guarantee:** the four `ParseRecovery::as_str` strings and
  the existing `DistillFailureClass::as_str` strings are a **frozen vocabulary**
  — treated as a public enum for `metrics.jsonl` consumers. Adding a *new*
  variant later is allowed (additive); renaming an existing one is a breaking
  change and forbidden without a coordinated reader update.

### Emission rules (unchanged denominators)
- `distill_success_rate`: every pass that ran the recipe (`value` 1.0/0.0).
- `distill_parse_success_rate`: only passes where `parse_attempted == true`
  (success or `ParseFailure`). Its mean stays exactly the parse-success rate.
- `parse_recovery` is a **context attribute of the same event** — it adds no new
  metric name and no new denominator (Decision D-1). A high `recovered` share is
  now queryable to detect a recurring agent bug behind auto-repair (R3).
- Best-effort & test-silent: write errors are logged not propagated; `record_*`
  is a no-op under `cfg!(test)` (unchanged).

---

## 5. Error-handling patterns (cross-cutting)

| Pattern | Rule |
|---|---|
| **Never a hollow `Ok`** | Recovery only turns a *repairable* `Err` into `Ok`. Genuinely malformed input stays `Err` all the way to `parse_facts_document`, so the batch defers and retries — it is never silently reported as parsed-empty. |
| **Strict-first ordering** | Strict `serde_json` runs before any repair; repair runs only on its `Err`. `serde_json` remains the single arbiter of validity. |
| **`Cow`, not `Result`, for repair** | `strip_json_trailing_commas` cannot fail; it returns bytes that either parse or don't. This keeps the repair layer allocation-free on the clean path and total. |
| **Structured logs, not new error variants** | Zero-facts (Brick C) and metric-write failures use `tracing::warn!(target:"simard::distill", …)`; no new `SimardError` variant is added (the existing `RpcError` prefixes classified by `classify_distill_error` are unchanged). |
| **Observability over silent repair** | `parse_recovery=recovered` makes auto-repaired parses distinguishable from clean ones, so repair is surfaced (queryable), never absorbed (R3). |

---

## 6. What is intentionally NOT in this contract

- **No HTTP/RPC/GraphQL surface** — none exists for this path; endpoint/method/
  status-code design is N/A.
- **No change to `RecipeEnvelope` deserialization shape** — recipe-runner-rs
  envelope schema (`{ "facts": [...], "procedures": [...] }`) is unchanged; we
  repair *bytes before* serde, not the type.
- **No change to `record_metric` signature or `MetricEntry`** — only the
  `context` payload gains a key.
- **P1 surfaces** (overseer signals, goal store, gym gate, kgpacks #16/#17) are
  telemetry/operational/cross-repo per Design §0–§1 and carry no API contract in
  this repo.

---

## 7. Contract → acceptance-criteria linkage

| Contract element | Serves criterion |
|---|---|
| §1 Brick A + §2 Brick B strict-then-repair | C1 (100%→0% parse-fail on trailing-comma inputs) |
| §4 `parse_recovery=recovered` label | C1 verification + R3 observability |
| §5 never-hollow-`Ok` guarantee | R2 (over-tolerance guard) |
| §3 zero-facts warn | disambiguates C1 (parse-fail) from empty-batch (R4) |
| §4 append-only versioning | back-compat for existing metrics consumers (R6) |
