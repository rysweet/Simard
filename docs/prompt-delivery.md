# Subprocess Prompt Delivery

Status: **design spec — implementation pending** · Module:
`amplihack-utils::prompt_delivery` · Issues: closes
[#1897](https://github.com/rysweet/Simard/issues/1897) · Parity epic:
[#1898](https://github.com/rysweet/Simard/issues/1898)
[#1899](https://github.com/rysweet/Simard/issues/1899)
[#1900](https://github.com/rysweet/Simard/issues/1900)
[#1901](https://github.com/rysweet/Simard/issues/1901)

> **Canonical location.** The implementation lives in the upstream
> [`amplihack-rs`](https://github.com/microsoft/amplihack) repository under
> `crates/amplihack-utils/src/prompt_delivery.rs`. This Simard copy is the
> parity-tracking reference held alongside Simard's adapter changes so the
> two repos can land the work in lockstep. If the two copies diverge, the
> `amplihack-rs` copy is authoritative.

When the Rust amplihack launcher spawns a child process (`claude`, `codex`,
`amplifier`) it must hand that child a **prompt**. Prompts are user-controlled,
sometimes binary-ish (apostrophes, newlines, embedded quotes) and occasionally
very large (system prompts plus tool catalogs can exceed 64 KiB). Embedding
arbitrary bytes directly into `argv` is unsafe and, past `ARG_MAX`, impossible.

The `prompt_delivery` module is the single, sanctioned chokepoint every
prompt-bearing subprocess invocation flows through. It selects one of three
delivery modes — **Inline**, **Stdin**, or **TempFile** — and applies it to a
`std::process::Command` or `tokio::process::Command` without callers needing to
know which mode was chosen.

> **Forbidden alternatives.** Callers MUST NOT invoke `sh -c`, `bash -c`, or
> any shell wrapper. They MUST NOT format prompts into argv via `format!` or
> string concatenation. They MUST NOT call `Command::arg(prompt)` directly.
> The only supported path is `prompt_delivery::apply_std` / `apply_tokio`.

---

## Quick start

```rust
use amplihack_utils::prompt_delivery::{self, PromptDelivery};
use tokio::process::Command;

let mut cmd = Command::new("claude");
cmd.arg("--no-color");

// Hand the prompt to the helper. It picks a mode, mutates `cmd`, and
// returns a guard that owns the prompt bytes and any temp file.
let applied = prompt_delivery::apply_tokio(
    &mut cmd,
    prompt_bytes,                   // &[u8]
    PromptDelivery::Auto,           // or ::Inline / ::Stdin / ::TempFile
).await?;

let mut child = cmd.spawn()?;
applied.feed(child.stdin.take()).await?;   // consumes the guard;
                                           // no-op (still Ok) for Inline.
let status = child.wait().await?;
```

`feed` takes the guard by value, writes the owned prompt buffer to the
child's stdin (or does nothing for `Inline` mode), and unlinks the temp
file (if any) when it returns. If you never call `feed` — for example, an
early-return error path — the guard's `Drop` impl performs the same
cleanup.

That is the entire user-facing surface for typical adapter code. Everything
below is reference material for module authors, operators, and security
reviewers.

---

## Delivery modes

| Mode       | Wire format                                    | When chosen by `Auto`                                | Argv-visible? | File on disk? |
| ---------- | ---------------------------------------------- | ---------------------------------------------------- | :-----------: | :-----------: |
| `Inline`   | Prompt becomes the final `Command::arg(...)`.  | `len < 8 KiB` AND no embedded NUL byte.              |      ✅       |      ❌       |
| `Stdin`    | Prompt written to child's stdin pipe.          | `8 KiB ≤ len < 100 KiB`, or NUL bytes present.       |      ❌       |      ❌       |
| `TempFile` | `NamedTempFile` (`0o600`) **and** stdin pipe.  | `len ≥ 100 KiB`.                                     |      ❌       |    ✅ (`/tmp`) |

A few notes on the mode semantics that surprise first-time readers:

* **`TempFile` always also writes to stdin.** Upstream `claude` / `codex` /
  `amplifier` binaries currently expose no `--prompt-file <path>` flag. The
  temp file exists for *postmortem inspection* (debugging, crash reports,
  audit logs) — the bytes still travel via stdin. If an upstream binary adds
  a real `--prompt-file` flag, this module is the place to wire it in; no
  caller code will need to change.
* **`Inline` is the only mode that leaks the prompt to `ps`.** Anything
  visible in `/proc/<pid>/cmdline` is visible to every UID on the host. The
  `Auto` heuristic is therefore conservative — small prompts that *might*
  contain secrets should be sent in `Stdin` mode via an explicit override.
* **There is a 16 MiB hard cap.** Prompts larger than 16 MiB return
  [`PromptDeliveryError::TooLarge`](#errors) before any file or pipe is
  touched. This bound prevents a runaway agent from filling `/tmp`.

### Auto-selection heuristic

```
fn select_mode(prompt: &[u8], caller: PromptDelivery) -> Result<PromptDelivery, _>
```

The order matters — each step is a hard invariant evaluated *before* the
next. In particular the 16 MiB size cap fires *before* any environment
override, so an operator cannot force-spawn a 100 MiB prompt by setting
`AMPLIHACK_PROMPT_DELIVERY=inline`.

1. **Size cap (hard invariant).** If `prompt.len() > 16 * 1024 * 1024` →
   return `Err(TooLarge)`. Applies to every variant including explicit
   caller overrides.
2. **Caller override.** If `caller` is anything other than
   `PromptDelivery::Auto`, that mode is used directly. The env var is
   *not* consulted. (If the caller passes `Inline` and the prompt
   contains a NUL byte, the call returns `Err(NulInInlineMode)`.)
3. **Environment override.** Otherwise, if
   [`AMPLIHACK_PROMPT_DELIVERY`](#environment-variable) is set to a
   valid value other than `auto`, that wins.
4. **NUL fork.** If the prompt contains any `0x00` byte → `Stdin` (an
   inline NUL would truncate argv on POSIX).
5. If `prompt.len() < 8 * 1024` → `Inline`.
6. If `prompt.len() < 100 * 1024` → `Stdin`.
7. Otherwise → `TempFile`.

> **`Auto` is the only variant that consults `AMPLIHACK_PROMPT_DELIVERY`.**
> Any other explicit variant bypasses the env var entirely. This includes
> the case where a caller explicitly passes `PromptDelivery::Inline` with
> a NUL-containing prompt — the call returns `NulInInlineMode` rather
> than silently upgrading to `Stdin`.

The 8 KiB / 100 KiB thresholds are constants in `prompt_delivery.rs`
(`INLINE_MAX_BYTES`, `STDIN_PREFERRED_MAX_BYTES`). They are deliberately well
below the `ARG_MAX` limits on every supported platform (Linux: 128 KiB per
arg, 2 MiB total; macOS: 256 KiB total; Windows: 32 KiB total) so that the
heuristic never butts up against a kernel limit even when amplihack adds
auxiliary CLI flags around the prompt.

---

## Public API — `amplihack-utils::prompt_delivery`

### `enum PromptDelivery`

```rust
#[non_exhaustive]
pub enum PromptDelivery {
    /// Let `select_mode` choose. This is the default for every adapter.
    Auto,
    /// Force the prompt onto argv. **Argv is world-readable via `ps`.**
    /// Use only for short, non-sensitive prompts.
    Inline,
    /// Force stdin delivery. Safe default for sensitive prompts.
    Stdin,
    /// Force `NamedTempFile` + stdin. Useful when you want a postmortem
    /// artifact on disk for crash reports.
    TempFile,
}
```

`PromptDelivery` is `Copy + Clone + Debug + PartialEq + Eq`. It implements
`FromStr` with the case-insensitive grammar described in
[Environment variable](#environment-variable).

### `fn apply_std`

```rust
pub fn apply_std(
    cmd: &mut std::process::Command,
    prompt: &[u8],
    mode: PromptDelivery,
) -> Result<AppliedPromptStd, PromptDeliveryError>;
```

* Mutates `cmd` in place — sets `stdin(Stdio::piped())` for non-`Inline`
  modes, appends a `--` flag terminator before any positional prompt arg in
  `Inline` mode (so a prompt starting with `--` is never mistaken for a
  flag), and pushes the prompt as the final argv element when applicable.
* Returns an [`AppliedPromptStd`](#struct-appliedprompt) RAII guard. The
  guard owns the temp file (if any) and the in-memory prompt bytes (for
  stdin modes). It MUST outlive the child.
* Never panics. Returns [`PromptDeliveryError`](#errors) for size violations
  and I/O failures.

### `fn apply_tokio`

```rust
#[cfg(feature = "tokio")]
pub async fn apply_tokio(
    cmd: &mut tokio::process::Command,
    prompt: &[u8],
    mode: PromptDelivery,
) -> Result<AppliedPromptTokio, PromptDeliveryError>;
```

Same semantics as `apply_std`, but returns a `tokio`-flavored guard whose
`feed` method drives the stdin write asynchronously. Behind the `tokio`
cargo feature; pulled in by `amplihack-orchestration` and any other crate
that uses async subprocess spawning.

### `struct AppliedPrompt{Std,Tokio}`

```rust
#[must_use = "AppliedPrompt must outlive the child process or the temp file may unlink prematurely"]
pub struct AppliedPromptStd { /* fields are private */ }

impl AppliedPromptStd {
    /// The mode actually selected (after size cap + caller/env override + auto heuristic).
    pub fn mode(&self) -> PromptDelivery;

    /// Path to the temp file, if mode == TempFile. `None` otherwise.
    pub fn temp_path(&self) -> Option<&std::path::Path>;

    /// Write the prompt to the child's stdin and consume the guard's owned
    /// byte buffer. No-op (returns `Ok(())`) for `Inline` mode.
    /// Closes stdin on completion so the child reads EOF.
    pub fn feed(self, stdin: Option<std::process::ChildStdin>) -> std::io::Result<()>;

    /// Detach: prevents the temp file from being unlinked on drop. Caller
    /// is then responsible for invoking `std::fs::remove_file` once the
    /// postmortem artifact is no longer needed. Returns the retained path.
    pub fn retain_temp_file(self) -> Option<std::path::PathBuf>;
}
```

The `Tokio` variant exposes the same surface with `async fn feed(self, ...)`.

**Ownership and lifetime.** The guard owns its prompt bytes (`Vec<u8>`) for
all non-`Inline` modes. `feed` takes `self` by value, writes the owned
buffer to the child's stdin, and consumes the guard. If a caller never
calls `feed`, the bytes are released when the guard drops — there is no
"streaming from borrowed slice" mode. This makes the security contract
auditable: the prompt is owned by the guard, then either flushed to a pipe
or dropped, exactly once.

**Drop behavior.** When the guard is dropped without `feed` being called,
any owned `NamedTempFile` is unlinked via the `tempfile` crate's RAII drop.
The in-memory `Vec<u8>` is released through the normal allocator path
(no explicit memory wipe — see [Security properties](#security-properties)
for the rationale and what *is* guaranteed).

### `enum PromptDeliveryError`

```rust
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum PromptDeliveryError {
    #[error("prompt exceeds 16 MiB hard cap (was {0} bytes)")]
    TooLarge(usize),

    #[error("failed to create temp file for prompt: {0}")]
    TempFile(#[source] std::io::Error),

    #[error("failed to write prompt to temp file: {0}")]
    Write(#[source] std::io::Error),

    #[error("failed to set 0600 permissions on prompt temp file: {0}")]
    Permissions(#[source] std::io::Error),

    #[error("prompt contains NUL byte but Inline mode was explicitly forced")]
    NulInInlineMode,
}
```

All variants are `#[non_exhaustive]`. Match with a `_ =>` arm.

---

## Configuration

### Environment variable

`AMPLIHACK_PROMPT_DELIVERY` — operator override for the auto-selection
heuristic. Read on every call into `prompt_delivery::apply_*` via
`std::env::var`. No process-lifetime caching — each invocation reflects the
current env at the time of the call, which keeps test isolation simple
(every test can `set_var`/`remove_var` in its own scope) and avoids a
hidden first-call-wins state machine. The env var lookup is a few
nanoseconds; it is not on any measured hot path.

| Accepted value (case-insensitive) | Effect                                                       |
| --------------------------------- | ------------------------------------------------------------ |
| `auto`                            | Use the heuristic. **Default if unset.**                     |
| `inline` *or* `cli`               | Force `PromptDelivery::Inline`.                              |
| `stdin`                           | Force `PromptDelivery::Stdin`.                               |
| `tempfile` *or* `temp-file` *or* `file` | Force `PromptDelivery::TempFile`.                      |
| anything else                     | Warning logged once via `tracing::warn!`, falls back to `auto`. |

> **Operator override loses to caller override.** If application code passes
> an explicit `PromptDelivery::Stdin` (or any non-`Auto` variant) to
> `apply_std`, the env var is ignored. The env var only ever influences
> `PromptDelivery::Auto`. The "once" suppression for the invalid-value
> warning is keyed on the value string so each distinct invalid value
> warns the first time it is observed in the process.

### Tracing

The module emits these structured events. The field names are part of the
public contract — adapters and dashboards can rely on them.

| Level   | Target                              | Fields                                              |
| ------- | ----------------------------------- | --------------------------------------------------- |
| `debug` | `amplihack_utils::prompt_delivery`  | `mode_chosen`, `prompt_len`, `auto_override_active` |
| `warn`  | `amplihack_utils::prompt_delivery`  | `invalid_env_value` (once per distinct value)       |
| `error` | `amplihack_utils::prompt_delivery`  | `failed_mode`, `error` (on `PromptDeliveryError`)   |

The prompt body is **never** logged. At every level you see only
`prompt.len()`, never the bytes themselves.

### Disk hygiene

`TempFile` mode places files under `std::env::temp_dir()` with the prefix
`amplihack-prompt-` and a random suffix. They are unlinked when the
returned guard drops, except when `retain_temp_file()` is called. Operators
who use `retain_temp_file()` for postmortem capture are responsible for
removing the file once the diagnostic is collected; there is no background
sweeper. Stale files left over from a crash can be cleaned with a one-off
`find "$TMPDIR" -maxdepth 1 -name 'amplihack-prompt-*' -mtime +1 -delete`.

---

## Caller patterns

### `claude_process` (async, tokio)

```rust
let mut cmd = tokio::process::Command::new(&claude_binary);
cmd.args(base_args)
   .stdout(Stdio::piped())
   .stderr(Stdio::piped());

let applied = prompt_delivery::apply_tokio(
    &mut cmd,
    rendered_prompt.as_bytes(),
    PromptDelivery::Auto,
).await?;

let mut child = cmd.spawn()?;
applied.feed(child.stdin.take()).await?;   // consumes guard;
                                           // temp file (if any) unlinked.
let output = child.wait_with_output().await?;
```

### `codex` and `amplifier` adapters (sync, std)

```rust
let mut cmd = std::process::Command::new(&codex_binary);
cmd.args(["--model", model_name, "--"]);          // `--` flag terminator

let applied = prompt_delivery::apply_std(
    &mut cmd,
    prompt_bytes,
    PromptDelivery::Auto,
)?;

let mut child = cmd.stdout(Stdio::piped()).spawn()?;
applied.feed(child.stdin.take())?;
let status = child.wait()?;
```

### Forcing stdin for a sensitive prompt

```rust
let applied = prompt_delivery::apply_std(
    &mut cmd,
    secret_prompt,
    PromptDelivery::Stdin,    // explicit — caller override beats env
)?;
```

### Forcing TempFile for postmortem debugging

```rust
let applied = prompt_delivery::apply_std(
    &mut cmd,
    prompt,
    PromptDelivery::TempFile,
)?;
let kept = applied.retain_temp_file();   // disable Drop unlink
// `kept` is the Some(PathBuf) the operator can attach to a bug report.
// IMPORTANT: caller must `std::fs::remove_file(&kept)` once the
// postmortem capture is finished — `retain_temp_file()` opts out of
// the RAII cleanup, so the file persists until manually removed.
```

---

## Security properties

| Property | Guarantee | Notes |
| -------- | --------- | ----- |
| **No shell.** | Every call site uses `Command::arg`. No `sh -c`, no `bash -c`, no string interpolation. | Eliminates shell-injection class entirely (Simard threat-model S2). |
| **`--` flag terminator.** | `Inline` mode appends `--` before the prompt argv element if the caller has not already done so. | Prompts beginning with `-` / `--` cannot be reinterpreted as flags by the child binary (S11). |
| **Argv-visibility advertised.** | `PromptDelivery::Inline` rustdoc warns that argv is visible to every UID on the host. | Operators consciously opt in (R-DP-1). |
| **Temp-file permissions.** | `NamedTempFile` is created via `tempfile` with mode `0o600` on Unix; perms are asserted in CI (`#[cfg(unix)]`). | Windows uses default ACL inherited from `%TEMP%` — see [Limitations](#limitations-and-non-goals) (R-DP-2). |
| **RAII cleanup.** | Drop on `AppliedPrompt*` unlinks the temp file unless `retain_temp_file()` is called. | Prevents `/tmp` accumulation (R-DP-3 / R-DP-4). |
| **Single-ownership of bytes.** | Prompt bytes live in the guard's owned `Vec<u8>` and are released on `feed` or drop, exactly once. No copy is held by the helper after the call returns. | Auditable lifetime; no explicit memory wipe is performed (Rust's allocator does not guarantee scrubbing, and pulling in `zeroize` was rejected to keep the dependency surface to a single new crate). |
| **Bounded size.** | 16 MiB hard cap rejected with `TooLarge` *before* env or caller override is consulted. | DoS mitigation against malicious or runaway prompts (S6, R-IV-3). |
| **Env var validated.** | `AMPLIHACK_PROMPT_DELIVERY` parsed against an allow-list; invalid values warn-and-fall-back. | No panic, no privilege change (R-IV-2). |
| **No new `unsafe`.** | Module contains no `unsafe` blocks. | Reviewable by audit. |
| **No `set_var`.** | The helper never mutates process env. | Preserves multi-thread safety (Rust 2024 requires `unsafe` for `set_var` precisely because of this hazard). |

---

## Behavior reference table

| Prompt size           | NUL bytes? | Env var unset | `AMPLIHACK_PROMPT_DELIVERY=inline` | Caller passes `::Stdin`   |
| --------------------- | :--------: | ------------- | ---------------------------------- | ------------------------- |
| 0 B – 8 KiB           |     no     | Inline        | Inline                             | Stdin (caller wins)       |
| 0 B – 8 KiB           |    yes     | Stdin         | **Err: `NulInInlineMode`**         | Stdin                     |
| 8 KiB – 100 KiB       |     no     | Stdin         | Inline (allowed; argv may be huge) | Stdin                     |
| 100 KiB – 16 MiB      |     no     | TempFile      | Inline (allowed; likely fails at `exec(2)` with `E2BIG`) | Stdin |
| > 16 MiB              |    any     | **Err: `TooLarge`** | **Err: `TooLarge`** (size cap fires before env override) | **Err: `TooLarge`** |

The `Inline` row at 100 KiB+ is deliberately not auto-downgraded — an
explicit operator override is taken at face value. If the kernel rejects
the resulting `exec(2)` with `E2BIG`, the failure surfaces as a
`std::io::Error` from `Command::spawn()` at the call site, **not** as a
`PromptDeliveryError`. `PromptDeliveryError` covers only what the helper
itself can detect prior to spawn (size cap, NUL-in-inline, temp-file I/O,
permission failures). Errors from the spawn call itself remain the
caller's concern.

---

## Limitations and non-goals

* **No `--prompt-file` flag** is emitted today. Upstream binaries don't
  accept one. `TempFile` mode pairs an on-disk artifact with stdin
  delivery; if upstream adds the flag this module is the single edit
  point.
* **`copilot` adapter is currently a no-op.** The GitHub Copilot CLI in
  amplihack's current launcher invokes Copilot in interactive REPL mode
  rather than handing it a single prompt on argv. The apostrophe / quoting
  bugs that motivate this module (rysweet/Simard#1871, #1879) live in the
  *argv-prompt* delivery path, which the interactive Copilot launch does
  not exercise. **However**, if a future Simard adapter spawns Copilot
  non-interactively with a prompt (the path issue #1897 contemplates as
  "switch its adapter path to call amplihack-rs subprocesses directly"),
  that adapter MUST route through `prompt_delivery::apply_*` like every
  other agent binary. The chokepoint contract is *every prompt-bearing
  subprocess invocation*, not *every binary that ships in amplihack*.
* **Env var is read on every call.** No process-lifetime caching. See
  [Environment variable](#environment-variable) for rationale (test
  isolation, no hidden first-call-wins state).
* **Cross-platform `0o600`.** On Windows the helper does not attempt to
  set NTFS ACLs; it relies on the default `%TEMP%` ACL. Operators who
  need stricter isolation on Windows should set a per-process scratch
  directory via `TMP` / `TEMP` with locked-down ACLs.

---

## Migration notes (for adapters predating this module)

Pre-#1897 call sites that passed the prompt as `Command::arg(prompt)`
should be replaced as follows.

**Before:**

```rust
let status = std::process::Command::new("claude")
    .arg("--no-color")
    .arg(prompt)        // ❌ argv leak; breaks on >100 KiB
    .status()?;
```

**After:**

```rust
let mut cmd = std::process::Command::new("claude");
cmd.arg("--no-color");
let applied = prompt_delivery::apply_std(&mut cmd, prompt.as_bytes(), PromptDelivery::Auto)?;
let mut child = cmd.spawn()?;
applied.feed(child.stdin.take())?;
let status = child.wait()?;
```

PR review enforcement is the primary catch for the pre-#1897 pattern. A
custom clippy lint to detect `Command::arg(prompt)` at build time has
been considered but is **future work** (would require dylint or a custom
lint driver — out of scope for issue #1897). For now, contributors and
reviewers grep for `Command::new("claude"`, `Command::new("codex"`, and
`Command::new("amplifier"` in PRs to confirm every spawn flows through
`prompt_delivery`.

---

## Testing

The module ships with:

* **Unit tests** (`crates/amplihack-utils/src/prompt_delivery.rs`,
  `#[cfg(test)]` module): select-mode decision matrix, env-var parsing,
  16 MiB cap, `NulInInlineMode` error, perm bits on Unix (`#[cfg(unix)]`).
* **Outside-in integration test**
  (`crates/amplihack-orchestration/tests/prompt_delivery_apostrophes.rs`):
  spawns `/bin/cat` as a stand-in for the agent binary, pipes a 64 KiB
  prompt containing apostrophes, double quotes, newlines, and tabs, and
  asserts the bytes round-trip exactly across all three modes plus the
  env-override and invalid-env-fallback paths. Gated `#[cfg(unix)]`.

Run with:

```bash
cargo test -p amplihack-utils
cargo test -p amplihack-orchestration --test prompt_delivery_apostrophes
cargo clippy --workspace --all-targets -- -D warnings
```

> **Test-author note (Rust 2024).** The workspace is on `edition = "2024"`,
> which makes `std::env::set_var` and `std::env::remove_var` `unsafe`
> because mutating env mid-process is racy in the presence of other
> threads reading env. Tests that exercise `AMPLIHACK_PROMPT_DELIVERY`
> must wrap mutation in `unsafe { std::env::set_var(...) }` and either
> (a) serialize across the env-touching test set with a `Mutex` /
> `serial_test` attribute, or (b) spawn a subprocess per scenario so each
> test gets a fresh process env. The integration test in
> `prompt_delivery_apostrophes.rs` uses approach (b).

---

## Related

* [Engineer Loop argv Sanitization](reference/engineer-loop-argv-sanitization.md)
  — separate concern (sanitizing argv segments for `gh` CLI), but shares
  the philosophy of *one chokepoint for all subprocess input*.
* [OODA Brain Prompt](reference/ooda-brain-prompt.md) — one of the larger
  prompt-producing call sites that exercises the `TempFile` path in
  practice.
* Parity epic: rysweet/Simard#1898, #1899, #1900, #1901.
* TDD charter: rysweet/Simard#1927.
