---
title: Large-payload spawn API reference
description: >
  Reference for `simard::spawn_payload` — the single facade every agent/recipe
  launch site routes through so a large payload (prompt, context, brief, message,
  memory) is never inlined into argv or envp. Specifies the one invariant, the
  ARGV_PAYLOAD_MAX_BYTES policy threshold, the two-transport dispatch (copilot ->
  stdin via prompt_delivery, recipe-runner-rs -> file path via ContextFile), the
  whole-repo launch-site audit with per-site dispositions, the pre-exec
  classify_spawn_failure errno surfacing into overseer::failure_sink, the
  grep-shaped anti-regression guard (tests/e2big_argv_guard.rs), and the hermetic
  per-path >256 KiB test matrix (#2640).
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/e2big-elimination.md
  - ../howto/add-a-safe-agent-spawn-site.md
  - ./argv-free-copilot-invocation.md
  - ./recipe-context-file-transport.md
  - ./terminal-failure-diagnosis-api.md
  - ../prompt-delivery.md
  - ../concepts/self-diagnose-on-step-error.md
  - ../../src/spawn_payload/mod.rs
  - ../../src/prompt_delivery/mod.rs
  - ../../src/recipe_context_file.rs
  - ../../src/overseer/diagnosis.rs
  - ../../src/overseer/failure_sink.rs
  - ../../tests/e2big_argv_guard.rs
---

# Large-payload spawn API reference

!!! warning "Transport does not grant authority"
    The broad Copilot flags in generic examples are not valid for typed OODA
    engineers. Their permission environment selects the scoped adapters in
    [Engineer Copilot permissions](./engineer-copilot-permissions.md); safe stdin
    transport does not grant additional authority.

> **Status: implemented.** This page is the wire-level contract for the
> `spawn_payload` facade, which ships at `src/spawn_payload/mod.rs` (registered
> as `pub mod spawn_payload;` in `src/lib.rs`). It delegates to the two
> transports that already ship —
> [`simard::prompt_delivery`](../prompt-delivery.md) (`apply_std` / `apply_tokio`)
> and [`simard::recipe_context_file`](./recipe-context-file-transport.md)
> (`ContextFile::write`) — and re-implements neither. Tracks
> [#2640](https://github.com/rysweet/Simard/issues/2640).

This page is the wire-level contract. For the narrative and the operator
principle, see [Comprehensive E2BIG elimination](../concepts/e2big-elimination.md).
For a step-by-step wiring guide, see
[How to add a safe agent/recipe spawn site](../howto/add-a-safe-agent-spawn-site.md).

## Contents

- [The invariant](#the-invariant)
- [Policy constants](#policy-constants)
- [Module surface](#module-surface)
- [Prompt transport (copilot family)](#prompt-transport-copilot-family)
- [Context transport (recipe-runner family)](#context-transport-recipe-runner-family)
- [Environment payloads](#environment-payloads)
- [Failure surfacing](#failure-surfacing)
- [Whole-repo launch-site audit](#whole-repo-launch-site-audit)
- [Anti-regression guard](#anti-regression-guard)
- [Hermetic per-path test matrix](#hermetic-per-path-test-matrix)
- [Configuration](#configuration)
- [Security](#security)
- [Constraints honoured](#constraints-honoured)
- [Code location](#code-location)

## The invariant

> **A dynamic value whose length can exceed
> [`ARGV_PAYLOAD_MAX_BYTES`](#policy-constants) (8 KiB) is delivered out-of-band
> — copilot prompts on stdin, recipe context on a file referenced by
> `-c <key>_path=<abs>` — and never appears in `argv` or `envp`.**

Because the payload no longer contributes to the process's argument vector or
environment, its size can no longer push `argv + envp` past `ARG_MAX`. `E2BIG`
at the spawn is eliminated **by construction**, with **zero truncation** and full
fidelity of the payload (guideline **G3**).

## Policy constants

```rust
// src/spawn_payload/mod.rs

/// The single policy threshold shared by every launch site. A dynamic value
/// whose length reaches this bound MUST be delivered out-of-band (stdin/file),
/// never on argv/envp. Chosen to coincide with prompt_delivery::INLINE_MAX_BYTES
/// (8 KiB) so the "small = inline" boundary is identical for prompts and context.
pub const ARGV_PAYLOAD_MAX_BYTES: usize = 8 * 1024;
```

For **recipe context**, `recipe_context` uses `ARGV_PAYLOAD_MAX_BYTES` directly
(inline below, file at/above). For **prompts**, the facade forces
`PromptDelivery::Stdin` (see [prompt transport](#prompt-transport-copilot-family)),
so a prompt of any size rides stdin and `prompt_delivery`'s `Auto` size-tiering
below is **not** consulted by the facade — it is documented only for the
lower-level `prompt_delivery` API:

| Constant (from `prompt_delivery`) | Value | Meaning (Auto mode only) |
| --- | --- | --- |
| `INLINE_MAX_BYTES` | `8 * 1024` | Below this, `Auto` may inline a prompt on argv. |
| `STDIN_PREFERRED_MAX_BYTES` | `100 * 1024` | Below this (and ≥ inline), `Auto` uses stdin. |
| `HARD_CAP_BYTES` | `16 * 1024 * 1024` | Above this, `TooLarge` before any I/O. |

> **256 KiB is a test input, not a threshold.** The per-path tests drive a
> > 256 KiB payload (well above both `MAX_ARG_STRLEN` per-arg risk and the 8 KiB
> policy cap) to prove file/stdin routing. It is never a production limit.

## Module surface

`attach_prompt_std` / `attach_prompt_tokio` are thin facade wrappers over the
existing [`prompt_delivery::apply_std` / `apply_tokio`](../prompt-delivery.md).
`recipe_context` / `RecipeArg` wrap the existing
[`recipe_context_file::ContextFile::write`](./recipe-context-file-transport.md).
`record_spawn_failure` wraps the existing
[`overseer::diagnosis::classify_spawn_failure`](./terminal-failure-diagnosis-api.md)
and records into `overseer::failure_sink`.

```rust
// src/spawn_payload/mod.rs — the sanctioned launch chokepoint.

// --- Prompt transport (wraps prompt_delivery::apply_std / apply_tokio) ---
pub use crate::prompt_delivery::{AppliedPromptStd, AppliedPromptTokio, PromptDeliveryError};

/// Attach a (possibly large) copilot prompt to a `std::process::Command`,
/// forcing STDIN delivery (`PromptDelivery::Stdin`). copilot reads its prompt
/// from stdin when no `-p` is given, and its `--` positional would be misparsed
/// as a subcommand — so the prompt is NEVER inlined onto argv, regardless of
/// size. Sets the child's stdin to a pipe; the caller feeds it via
/// `AppliedPromptStd::feed`.
pub fn attach_prompt_std(cmd: &mut std::process::Command, prompt: &[u8])
    -> Result<AppliedPromptStd, PromptDeliveryError>;

/// Async sibling wrapping `prompt_delivery::apply_tokio` for `tokio::process::Command`.
pub async fn attach_prompt_tokio(cmd: &mut tokio::process::Command, prompt: &[u8])
    -> Result<AppliedPromptTokio, PromptDeliveryError>;

// --- Context transport (wraps recipe_context_file::ContextFile) ---
/// One resolved `-c` argument: either a small inline `key=value` token, or a
/// file-backed `key_path=<abs>` token owning its temp file.
pub enum RecipeArg {
    Inline(String),
    Filed(crate::recipe_context_file::ContextFile),
}

impl RecipeArg {
    /// The exact `-c` value to push onto the recipe argv. For `Filed`, delegates
    /// to the existing `ContextFile::arg_value()`.
    pub fn arg_value(&self) -> String;
}

/// Resolve one recipe context var under the policy: values below
/// ARGV_PAYLOAD_MAX_BYTES stay inline (newline-collapsed for YAML safety, #2127);
/// values at/above it are written via `ContextFile::write` and returned as
/// `Filed`, so the payload never touches argv. Lossless — never truncates.
pub fn recipe_context(base_type: &str, key: &str, value: &str)
    -> std::io::Result<RecipeArg>;

// --- Failure surfacing (wraps overseer::diagnosis::classify_spawn_failure) ---
/// Classify a pre-exec spawn `io::Error` (errno-keyed), tag it with the launch
/// `site`, and record it into the Overseer failure sink. Guarantees no spawn
/// failure is swallowed.
pub fn record_spawn_failure(err: &std::io::Error, site: &str);
```

The facade adds **no new byte-transport code** and **no new error type** beyond
`RecipeArg`; it composes the two audited modules under one policy.

## Prompt transport (copilot family)

For `copilot` / `amplihack copilot` launches the facade delivers the prompt on
**stdin** (forced `PromptDelivery::Stdin`), never as an argv token. The prompt
bytes never appear in `argv` or in a PTY `command:` string. This is the transport
#2660 and the #2640 copilot sites use; the facade is the single entry point the
meeting, signal, and engineer-subprocess sites are routed through. See the
per-site grammar in the [argv-free Copilot/OODA reference](./argv-free-copilot-invocation.md).

```rust
let mut cmd = std::process::Command::new("amplihack");
cmd.args(["copilot", "--subprocess-safe", "--allow-all-tools", "--allow-all-paths"]);
let applied = spawn_payload::attach_prompt_std(&mut cmd, prompt_bytes)?; // sets stdin(piped)
let mut child = cmd.spawn()?;
let stdin = child.stdin.take();
// Feed on a thread so a large prompt cannot deadlock against the child's stdout.
let feeder = std::thread::spawn(move || applied.feed(stdin));
```

> **Prefer `Command` + `Stdio` over `sh -c "$(cat …)"`.** Where a PTY or shell
> wrapper is architecturally required (the OODA decision-cycle launch), only a
> bounded **path** may appear in the command string (`cat 'PATH' | cmd` is
> allowed); `"$(cat PATH)"` contents-expansion is forbidden and is caught by the
> [guard](#anti-regression-guard).

## Context transport (recipe-runner family)

For `recipe-runner-rs` / `amplihack recipe run` the facade resolves each context
var with `recipe_context`, which applies the policy per value:

```rust
let mut argv = vec!["recipe".into(), "run".into(), recipe_path];
let mut guards = Vec::new();                 // keep ContextFile guards alive
for (key, value) in context_vars {
    let arg = spawn_payload::recipe_context("overseer", key, value)?;
    argv.push("-c".into());
    argv.push(arg.arg_value());              // "key=val"  OR  "key_path=/tmp/.../key.ctx"
    if let spawn_payload::RecipeArg::Filed(cf) = arg { guards.push(cf); }
}
```

Resolution:

- `value.len() < ARGV_PAYLOAD_MAX_BYTES` → `Inline("key=<sanitized>")`
  (whitespace-collapsed for YAML safety; **not** truncated because it is already
  small). The recipe reads `{{key}}` unchanged.
- `value.len() ≥ ARGV_PAYLOAD_MAX_BYTES` → `Filed(ContextFile)` →
  `"key_path=<abs>"`. The recipe reads the file via `{{key_path}}` (lossless).

The `ContextFile` semantics (private `0700` temp dir, one dir per invocation, RAII
unlink on drop, verbatim bytes) are specified in the
[recipe context-file transport reference](./recipe-context-file-transport.md).

## Environment payloads

The same threshold applies to `envp`. A launch site MUST NOT place a value
≥ `ARGV_PAYLOAD_MAX_BYTES` into an environment variable (env contributes to the
same `ARG_MAX` budget as argv). Large values that would otherwise be passed via
env are routed to a file and the **path** is put in the env var. The
[guard](#anti-regression-guard) flags a new large `Command::env(_, big)`.

## Failure surfacing

E2BIG is a **pre-exec** failure — an `io::Error` (errno 7) with no `ExitStatus`
and no transcript. `record_spawn_failure` calls the existing errno-keyed
classifier so a spawn error is diagnosed and recorded, never dropped. It is
wired at the Overseer's own recipe launch (`overseer/launch.rs`) and the journal
recipe (#2692); the copilot prompt sites (meeting, engineer subprocess) surface
their spawn errors through their own loud error types (`SimardError`) instead of
the sink. The classifier below **already ships** and is reused unchanged:

```rust
// src/overseer/diagnosis.rs — exists today; reused unchanged (no new causes).
pub fn classify_spawn_failure(err: &std::io::Error) -> FailureDiagnosis {
    FailureDiagnosis {
        cause: classify_spawn_cause(err), // errno-keyed helper, below
        exit_code: None,                  // there was no child
        evidence: bounded_spawn_evidence(&err.to_string()),
    }
}

// Private helper (paraphrased): errno first, then an E2BIG message-string
// fallback when no numeric errno is present; any other errno is a structured
// `FailureCause::Unknown` — never a silent drop.
fn classify_spawn_cause(err: &std::io::Error) -> FailureCause {
    match err.raw_os_error() {
        Some(7)  => FailureCause::ArgListTooLong, // E2BIG
        Some(28) => FailureCause::DiskFull,       // ENOSPC (temp-file write)
        Some(12) => FailureCause::OutOfMemory,    // ENOMEM (fork/exec)
        _        => FailureCause::Unknown,
    }
}
```

- `exit_code` is always `None` (there was no child).
- A string fallback matches `"argument list too long"` / `"os error 7"` where no
  numeric errno is surfaced.
- The diagnosis is recorded into `overseer::failure_sink` at `error`. See
  [terminal failure diagnosis API](./terminal-failure-diagnosis-api.md) and
  [Self-diagnose on step error](../concepts/self-diagnose-on-step-error.md).

## Whole-repo launch-site audit

Because this is a class of bug, **every** site that can carry a payload to
`copilot`, `recipe-runner-rs`, `amplihack recipe run`, or a `sh -c` wrapper is
audited and given a durable disposition. Tiers:

- **A — file/stdin channel:** carries unbounded semantic content the agent must
  consume in full → out-of-band transport (mandatory; truncation disallowed).
- **B — bounded guard:** realistically small but operator-influenced free text
  that could grow → routed through `recipe_context`, which files it if it ever
  reaches the cap (lossless) instead of truncating.
- **C — safe:** fixed-size IDs, paths, short consts → unchanged.

> **Dispositions.** The Tier-A journal (#2692), OODA/copilot (#2660), and the
> copilot meeting/signal/engineer-subprocess prompt sites (#2640) ship today,
> routed through the facade's **stdin** transport. The Tier-B recipe `-c` sites
> (`overseer` `task_description`, `self-improve` `proposal`, `engineer-loop`
> `objective`) stay **bounded-inline** (`sanitize_context_var(…, 8000)`): they are
> already E2BIG-safe (8000 chars ≪ ARG_MAX), and their recipes
> (`smart-orchestrator.yaml`, `simard-self-improve-cycle.yaml`,
> `simard-engineer-loop.yaml`) are **external** assets (amplihack bundle, not this
> repo) that read `{{key}}` inline with **no `{{key_path}}` support** — filing a
> large value would leave the agent with an empty var. The facade's
> `recipe_context` file channel is ready for them once those external assets gain
> a `_path` read (a coordinated amplihack-bundle change). The remaining Tier-B
> rows (`goal_curation/*`, `bin/simard_tui/app.rs`) are likewise bounded-inline.

| Tier | Site | Payload var(s) | Transport / disposition | State |
| --- | --- | --- | --- | --- |
| **A** | `base_type_copilot` (signal, meeting) | prompt / objective / message | **stdin** via `attach_prompt_std` (#2640/#2660) | **ships** (facade-routed) |
| **A** | `engineer_loop/agent_spawn.rs` (copilot) | prompt (objective + inspection) | **stdin** via `attach_prompt_std`; argv prompt-less (#2640) | **ships** (facade-routed) |
| **A** | `ooda_actions/session.rs` (decision cycle) | prompt | **stdin** via `cat 'PATH' \| amplihack copilot` (#2660) | ships (shell pipe; path-only argv) |
| **A** | `journal/recipe.rs` (draft) | `day_context` | **file** `day_context_path` (#2692) — in-repo recipe | ships |
| **A** | `journal/recipe.rs` (review) | `draft` | **file** `draft_path` (#2692) — in-repo recipe | ships |
| **A** | `memory_consolidation/distillation.rs` | `episodes` | **file** `episodes_path` — in-repo recipe | ships |
| **A** | `stewardship/recipe_merge_judge.rs` | `pr_body` | **file** `pr_body_path` — in-repo recipe | ships |
| **B** | `overseer/launch.rs` (`amplihack recipe run`) | `task_description` | bounded-inline `sanitize(…,8000)` (E2BIG-safe) | ships; **file channel blocked on external recipe `_path`** |
| **B** | `bin/simard_engineer_loop_recipe.rs` | `objective` | bounded-inline `sanitize(…,8000)` (E2BIG-safe) | ships; blocked on external recipe `_path` |
| **B** | `bin/simard_self_improve_recipe.rs` | `proposal` | bounded-inline `sanitize(…,8000)` (E2BIG-safe) | ships; blocked on external recipe `_path` |
| **B** | `goal_curation/decompose.rs` | `goal_description`, `plan` | bounded-inline `sanitize(…)` (E2BIG-safe) | follow-up |
| **B** | `goal_curation/recipe_progress_checker.rs` | `problem`, `plan`, `wip_summary` | bounded-inline `sanitize(…)` (E2BIG-safe) | follow-up |
| **B** | `bin/simard_tui/app.rs` (`sh -c` sites) | prompt / message | target: **path-only** in shell string | follow-up |
| **C** | `ooda_brain/recipe_brain.rs` | `goal_id`/`reason`/`escalation_note` | already `sanitize_context_var(…, 500/4000)` | safe |
| **C** | `disk_health.rs` | `state_root`/`repo_path`/`max_prune` | small paths / integer scalars | safe |
| **C** | `overseer/launch.rs` | `target_repo`; merge-judge `pr_number`/`repo` | short slugs / IDs | safe |
| **C** | any `--version` / capability probe | — | not a payload | safe |

Every audited site and its disposition is enumerated here **and** in the PR
description, so the audit is a durable artifact, not a one-time grep.

## Anti-regression guard

`tests/e2big_argv_guard.rs` is a grep-shaped, CI-visible test (shaped like the
existing `tests/no_bridge_naming.rs`) that **fails the build** on any
reintroduction of the class. It asserts, over `src/**/*.rs` (excluding test
modules and doc-comment lines):

1. **No `$(cat` argv-expansion.** Zero matches for `$(cat` contents-expansion
   into argv. (`cat 'PATH' | cmd` piping is allowed; only contents-expansion is
   forbidden.) — protects #2660.
2. **No inline unbounded recipe key.** The Tier-A unbounded keys
   (`day_context`, `draft`, `episodes`, `pr_body`) must never appear as an inline
   `"<key>={…}"` format string — only as their file-channel `<key>_path` form.
   — protects #2692/#2700.
3. **The single spawn facade exists.** `src/spawn_payload/mod.rs` is present and
   registered as `pub mod spawn_payload;` in `src/lib.rs`, so every launch site
   has one policy-enforcing chokepoint.

The Tier-B inline keys (`task_description`, `objective`, `proposal`, …) are
driven to the facade by the per-path transport tests, not this coarse grep,
because those keys are also used by bounded, already-safe callers (e.g.
`ooda_brain::recipe_brain`) and a key-name grep would false-positive on them.

## Hermetic per-path test matrix

Each launch path has a hermetic test that drives a **> 256 KiB** payload and
asserts the built invocation never inlines it into `argv`/`envp`
(builder-level assertion — no real subprocess needed), proving no `E2BIG`
(exit 126 / os error 7). Real-spawn variants are `#[ignore]` and opt-in.

| Path | Test file | Asserts | Status |
| --- | --- | --- | --- |
| facade contract | `tests/spawn_payload_facade.rs` | policy threshold; recipe_context inline/file; prompt off argv; errno 7 → sink | **ships (this PR)** |
| decision-cycle (OODA) | `tests/ooda_e2big_transport.rs`, `tests/ooda_argv_free_invocation.rs` | prompt on stdin; no `$(cat` in command string | exists |
| meeting turn | `tests/meeting_e2big_transport.rs` | prompt on stdin; argv path-only | **ships (this PR)** |
| Signal channel | `tests/signal_e2big_transport.rs` | large context spawns with no argv/env inlining | **ships (this PR)** |
| engineer loop | `tests/engineer_e2big_transport.rs`, `tests/engineer_copilot_permissions.rs` | prompt-less argv; prompt on stdin; #1717 perms preserved | **ships (this PR)** |
| Overseer launch | `tests/overseer_launch_e2big_transport.rs` | builder truncates (bounded-inline); `recipe_context` primitive files a ≥ 8 KiB value losslessly | **ships (this PR)** |
| self-improve recipe | `tests/self_improve_e2big_transport.rs` | inline `-c proposal=<big>` fails E2BIG; `recipe_context` primitive files losslessly | **ships (this PR)** |
| stewardship merge judge | `tests/recipe_context_file.rs` (`pr_body`) | `pr_body_path`, payload verbatim in file | exists |
| journal | `tests/journal_e2big_transport.rs`, `tests/journal_argv_free.rs` | `day_context_path` / `draft_path` | exists |
| anti-regression | `tests/e2big_argv_guard.rs` | the three grep assertions above | **ships (this PR)** |
| failure surfacing | `tests/recipe_spawn_failure_diagnosis.rs`, `tests/spawn_payload_facade.rs` | errno 7 → `ArgListTooLong` in the sink | exists / **ships (this PR)** |

All tests are hermetic under the shared state-root guard (see
[hermetic tests](../testing/hermetic-tests.md)); none require network or a real
agent binary.

## Configuration

| Variable | Default | Effect |
| --- | --- | --- |
| `AMPLIHACK_PROMPT_DELIVERY` | `auto` | Consulted only by the lower-level `prompt_delivery` `Auto` mode. The facade forces `Stdin`, so this env var does **not** change facade prompt delivery. See [prompt delivery](../prompt-delivery.md#configuration). |
| `TMPDIR` | OS default | Where `ContextFile` temp files are created. Point at a private, sufficiently-large volume. |

There is **no** operator switch to re-enable argv inlining of large payloads —
the invariant is unconditional by design.

## Security

- Payloads never appear in `/proc/<pid>/cmdline`, so a large prompt/context is
  not world-readable via `ps` (the `Inline` prompt mode remains available only
  for sub-8 KiB prompts and is documented in [prompt delivery](../prompt-delivery.md)).
- Temp files are created `0600` (prompts) / under a `0700` dir (`ContextFile`)
  and unlinked on drop; a payload never lingers on disk.
- `Debug` for `RecipeArg::Filed` / `ContextFile` prints the key and path only,
  never the payload.
- The failure `evidence` is bounded and redacted; a pathological error string
  cannot inflate a log line, notification, or issue body.

## Constraints honoured

- **Additive / surgical:** one new module (`spawn_payload`) composing two
  existing audited transports; no new byte-transport, no new error taxonomy.
- **No "Bridge" names.** No silent fallbacks — every spawn error is surfaced.
- **Prefer Rust `Command` + `Stdio`** over brittle shell `$(cat)` interpolation
  (guideline G3).
- **No regression** to #2660 (copilot stdin) or #2700/#2692 (recipe file channel).

## Code location

| Concern | Location | State |
| --- | --- | --- |
| Facade + policy | `src/spawn_payload/mod.rs` | **ships** |
| Prompt transport | [`src/prompt_delivery/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/prompt_delivery/mod.rs) | exists |
| Context transport | [`src/recipe_context_file.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_context_file.rs) | exists |
| Failure classifier | [`src/overseer/diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/diagnosis.rs) | exists |
| Failure sink | [`src/overseer/failure_sink.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/failure_sink.rs) | exists |
| Anti-regression guard | `tests/e2big_argv_guard.rs` | **ships** |
| Module registration | `pub mod spawn_payload;` in `src/lib.rs` | **ships** |
