# Reference: Recipe Context Variable Sanitization

Module: `src/ooda_brain/sanitize.rs`
Consumers: `recipe_engineer_lifecycle.rs`, `recipe_decide.rs`, `recipe_orient.rs`

All three recipe-based OODA brains pass user-derived and log-derived strings
as `-c key=value` context variables to `recipe-runner-rs`. These strings flow
through Handlebars `{{var}}` substitution into the agent prompt. Unsanitized
newlines in context values break YAML template interpolation in
recipe-runner-rs, causing the subprocess to exit with status 1 — which the
Rust shim reports as `AdapterInvocationFailed("recipe exited with exit
status: 1")`.

> **History:** Before issue
> [#2127](https://github.com/rysweet/Simard/issues/2127), context variables
> were passed verbatim to `recipe-runner-rs`. The `last_engineer_log_tail`
> field — which contains raw terminal output with embedded newlines — caused
> 1,341 OODA cycle failures in 24 hours. The recipe YAML worked when invoked
> from the command line (where context values were manually quoted) but failed
> when the Rust shim constructed the `-c key=value` argv programmatically.

## The Problem

The recipe-runner-rs Handlebars engine performs `{{var}}` substitution inside
a YAML `prompt:` block. When a context variable contains a literal newline:

```
-c "last_engineer_log_tail=error: cannot find\nthread panicked at src/main.rs"
```

…the rendered YAML prompt breaks because the newline is interpolated literally
into the YAML multi-line scalar, producing invalid YAML structure or injecting
unintended content after the line break. recipe-runner-rs exits with status 1.

The same issue affects any context variable that could contain newlines,
carriage returns, or excessive length — specifically `goal_id`,
`goal_description`, `worktree_path`, and `last_engineer_log_tail` in the
engineer-lifecycle brain, `goal_id` and `reason` in the decide brain, and
`goal_id` and `base_reason` in the orient brain.

## Sanitization Helper

### `sanitize_context_var(s: &str, max_len: usize) -> String`

`pub(super)` function in `src/ooda_brain/sanitize.rs`. Available to all
modules within `ooda_brain` via `use super::sanitize::sanitize_context_var`.

**Steps (in order):**

1. Replace every `\n` (LF) and `\r` (CR) with a single ASCII space.
2. Collapse consecutive whitespace by splitting on whitespace boundaries and
   rejoining with a single space (`split_whitespace().collect::<Vec<_>>().join(" ")`).
3. Truncate to `max_len` on a UTF-8 char boundary. If truncation occurs,
   append `"…"` (U+2026 HORIZONTAL ELLIPSIS) to signal data loss.

**Illustrative usage:**

```rust
use super::sanitize::sanitize_context_var;

let clean = sanitize_context_var("error: cannot find\nthread panicked\r\nat src/main.rs", 500);
assert_eq!(clean, "error: cannot find thread panicked at src/main.rs");

let truncated = sanitize_context_var(&"a".repeat(1000), 10);
assert_eq!(truncated.chars().count(), 11); // 10 content chars + "…"
```

### Contract

| Property | Guarantee |
|----------|-----------|
| Newline-free | Output never contains `\n` or `\r` |
| Whitespace-normalized | No runs of consecutive whitespace; no leading/trailing whitespace |
| Bounded length | Output is at most `max_len` characters (plus the ellipsis marker if truncated) |
| Char-boundary safe | Truncation never splits a multi-byte UTF-8 sequence |
| Idempotent | `sanitize(sanitize(s, n), n)` == `sanitize(s, n)` (modulo the ellipsis already being present) |
| Pure | No I/O. Allocates only the returned `String` |
| Empty-safe | Empty input returns empty output; `max_len = 0` returns `""` |

## Application Sites

### `recipe_engineer_lifecycle.rs` — 4 fields sanitized

| Field | `max_len` | Rationale |
|-------|-----------|-----------|
| `goal_id` | 500 | Typically a UUID, but defensive against user-authored goal IDs |
| `goal_description` | 500 | User-authored text; concise by convention |
| `worktree_path` | 500 | `PathBuf::display()` output; paths should never be this long in practice |
| `last_engineer_log_tail` | 2000 | Raw terminal output — the primary failure vector. Needs generous room for diagnostic context |

**Fields NOT sanitized:** `cycle_number`, `consecutive_skip_count`,
`failure_count`, `worktree_mtime_secs_ago`, `commits_behind`,
`in_flight_engineer_count` (all computed from numeric types —
`u32`/`u64`). `sentinel_pid` (rendered as `"<none>"` or a numeric string).
`minutes_since_last_update_attempt` (rendered as `"never"` or a numeric
string). These values are always clean single-line strings by construction.

```rust
// Before (broke on multi-line log tails):
.arg(format!("last_engineer_log_tail={}", ctx.last_engineer_log_tail))

// After:
.arg(format!(
    "last_engineer_log_tail={}",
    sanitize_context_var(&ctx.last_engineer_log_tail, 2000)
))
```

### `recipe_decide.rs` — 2 fields sanitized

| Field | `max_len` | Rationale |
|-------|-----------|-----------|
| `goal_id` | 500 | Same as above |
| `reason` | 500 | Short decision rationale; could contain LLM-generated text |

**Fields NOT sanitized:** `urgency` (formatted as `{:.3}` float — always
clean).

### `recipe_orient.rs` — 2 fields sanitized

| Field | `max_len` | Rationale |
|-------|-----------|-----------|
| `goal_id` | 500 | Same as above |
| `base_reason` | 500 | Short rationale string; same risk profile as `reason` |

**Fields NOT sanitized:** `base_urgency` (formatted as `{:.3}` float).
`failure_count` (u32).

## Relationship to Existing Sanitizers

| Sanitizer | Location | Purpose | This feature |
|-----------|----------|---------|--------------|
| `truncate_to_char_boundary` | `src/util/string_truncate.rs` | Byte-budget truncation for evidence buffers (in-place `&mut String`, byte-based) | Different API; `sanitize_context_var` operates on char count and returns a new `String` |
| `truncate_for_log` | `src/ooda_brain/rustyclawd.rs` (also duplicated in `spawn.rs`, `merge_judge.rs`) | Display-length truncation for log lines | Truncation only — no newline replacement or whitespace normalization |
| `truncate()` (local) | `recipe_engineer_lifecycle.rs`, `recipe_decide.rs` | Truncate stderr in error messages | Retained — serves a different purpose (error display, not CLI arg safety) |
| `redact_secrets` | `src/ooda_brain/context.rs` | Strip API keys from log tails | Runs before sanitization — `gather_engineer_lifecycle_ctx` redacts first, then the recipe shim sanitizes at arg-construction time |

`sanitize_context_var` is intentionally **not** a replacement for any of these
helpers. It is scoped narrowly to the recipe `-c key=value` construction
pattern and lives in `ooda_brain/sanitize.rs` alongside its three consumers.

## Security Considerations

- **YAML template injection** is the primary attack vector. Newlines in
  context values can break the Handlebars-rendered YAML prompt and
  theoretically inject arbitrary YAML keys. Stripping newlines eliminates
  this vector.
- **`Command::new().arg()` is already shell-injection safe** — no `sh -c`,
  no shell interpolation. The sanitizer defends against YAML injection, not
  shell injection. This invariant must be preserved.
- **Sanitization happens at the call site, not in the struct.** The
  `EngineerLifecycleCtx`, `DecideContext`, and `OrientContext` structs retain
  raw values for logging and debugging. Sanitization is applied only at
  `Command::new().arg()` construction time.
- **No unsafe code.** Pure `String` manipulation using `char_indices()` for
  char-boundary-safe truncation.
- **Argv visibility in `/proc/PID/cmdline`** is pre-existing exposure.
  Truncation slightly reduces the volume of exposed data. Secrets are already
  stripped by `redact_secrets` upstream.

## Testing

`src/ooda_brain/sanitize.rs` contains an inline `#[cfg(test)]` module with
unit tests covering:

| Test | What it verifies |
|------|-----------------|
| Newlines replaced | `\n` → space, output contains no `\n` |
| Carriage returns replaced | `\r` → space, output contains no `\r` |
| Mixed `\r\n` replaced | Windows-style line endings handled |
| Consecutive whitespace collapsed | `"a  \n\n  b"` → `"a b"` |
| Truncation at max_len | Output respects the char budget |
| Truncation on char boundary | Multi-byte UTF-8 (emoji, CJK) does not panic or produce invalid UTF-8 |
| Truncation appends ellipsis | Truncated output ends with `"…"` |
| Empty input | Returns `""` |
| Already-clean input | Passes through unchanged (no spurious allocation observable via `assert_eq`) |
| Idempotence | `sanitize(sanitize(s, n), n)` produces a stable result |

Run the sanitizer tests:

```bash
cargo test --package simard --lib ooda_brain::sanitize
```

Run all affected module tests:

```bash
cargo test --package simard --lib ooda_brain
```

## Configuration

There are no runtime configuration knobs. The `max_len` values are hardcoded
at each call site because they are tuned to the specific field's semantics:

- **500 chars** for identifiers and short rationale strings — generous for
  UUIDs and single-paragraph descriptions, tight enough to prevent prompt
  bloat.
- **2000 chars** for `last_engineer_log_tail` — balances diagnostic context
  (the LLM needs enough log to reason about the engineer's state) against
  the recipe prompt's total token budget.

To change these limits, edit the literal values in the `sanitize_context_var`
call at each site in `recipe_engineer_lifecycle.rs`, `recipe_decide.rs`, or
`recipe_orient.rs`.

## Module Layout (after this change)

```
src/ooda_brain/
├── mod.rs                       # mod sanitize; declaration
├── sanitize.rs                  # sanitize_context_var + unit tests   (NEW)
├── recipe_engineer_lifecycle.rs # wraps 4 fields with sanitize_context_var
├── recipe_decide.rs             # wraps 2 fields with sanitize_context_var
├── recipe_orient.rs             # wraps 2 fields with sanitize_context_var
├── context.rs                   # gather_engineer_lifecycle_ctx + redact_secrets
├── …                            # (other modules unchanged)
```

## Examples

### Before: multi-line log tail breaks recipe

```
# Context:
#   last_engineer_log_tail = "error[E0433]: failed to resolve\n  --> src/lib.rs:42\n"

$ recipe-runner-rs ooda-engineer-lifecycle.yaml \
    -c "last_engineer_log_tail=error[E0433]: failed to resolve
  --> src/lib.rs:42
"
# Exit status: 1 — YAML parse error in rendered prompt
```

### After: sanitized log tail succeeds

```
# sanitize_context_var("error[E0433]: failed to resolve\n  --> src/lib.rs:42\n", 2000)
#   → "error[E0433]: failed to resolve --> src/lib.rs:42"

$ recipe-runner-rs ooda-engineer-lifecycle.yaml \
    -c "last_engineer_log_tail=error[E0433]: failed to resolve --> src/lib.rs:42"
# Exit status: 0 — agent runs successfully
```

### Truncation of excessively long log tail

```
# 50 KB of log output is truncated to 2000 chars:
# sanitize_context_var(&huge_log, 2000)
#   → "error[E0433]: failed to resolve --> src/lib.rs:42 ... (1998 more chars)…"
```

## Adopting the Sanitizer in New Recipe Shims

Future recipe shims that pass user-derived or log-derived text as context
variables must:

1. Import `sanitize_context_var` via `use super::sanitize::sanitize_context_var`.
2. Wrap every string-typed context variable whose source is not a known-safe
   format (numeric `.to_string()`, enum variant name, etc.) in a
   `sanitize_context_var(&value, max_len)` call.
3. Choose an appropriate `max_len`:
   - **500** for short identifiers and rationale strings.
   - **2000** for diagnostic log content.
   - **200** for titles and labels.
4. Add a test asserting that the sanitized value contains no `\n` or `\r` on
   representative multi-line input.
5. Do NOT sanitize numeric or enum-derived fields — they are clean by
   construction and sanitization would waste cycles.

## Troubleshooting

### Symptom: recipe still exits with status 1 after sanitization

1. **Check stderr**: the error path in the recipe shim captures stderr.
   Look for `recipe exited with exit status: 1` in the daemon log along
   with the truncated stderr content.
2. **Check for new unsanitized fields**: if a new context variable was added
   to the recipe YAML without a corresponding `sanitize_context_var` call,
   multi-line values in that field will reproduce the original failure.
3. **Check recipe YAML syntax**: if the recipe YAML itself has a syntax
   error (independent of context variable interpolation), recipe-runner-rs
   exits with status 1. Run the recipe manually with hardcoded context
   values to isolate.

### Symptom: truncated context loses important diagnostic information

The `last_engineer_log_tail` 2000-char limit is deliberately generous. If
the agent consistently makes poor lifecycle decisions due to truncated
context:

1. Increase the limit at the call site (e.g., 4000 chars). This trades
   prompt token budget for diagnostic fidelity.
2. Ensure `gather_engineer_lifecycle_ctx` is capturing the *tail* of the
   log (last N bytes), not the head — the most recent output is the most
   diagnostically relevant.

## See Also

* [Reference: engineer loop argv sanitization](engineer-loop-argv-sanitization.md) — the analogous sanitizer for `gh` CLI argv
* [Reference: string truncation helpers](string-truncation-helpers.md) — the byte-budget truncation helper (different purpose)
* [Reference: OODA Brain API](ooda-brain-api.md) — trait and type definitions
* [How-to: diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md) — operator runbook
* Issue [#2127](https://github.com/rysweet/Simard/issues/2127) — original bug
