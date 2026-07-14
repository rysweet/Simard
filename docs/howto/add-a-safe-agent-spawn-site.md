---
title: How to add a safe agent/recipe spawn site
description: >
  Step-by-step guide for adding a new subprocess launch site (copilot, amplihack
  recipe run, or recipe-runner-rs) that carries a prompt/context/brief/message
  payload, so it routes through the `simard::spawn_payload` facade, can never
  overflow ARG_MAX (E2BIG), passes the tests/e2big_argv_guard.rs anti-regression
  guard, and surfaces any spawn failure instead of swallowing it (#2640).
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: how-to
status: implemented
related:
  - ../reference/large-payload-spawn-api.md
  - ../concepts/e2big-elimination.md
  - ../reference/argv-free-copilot-invocation.md
  - ../reference/recipe-context-file-transport.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ../prompt-delivery.md
---

# How to add a safe agent/recipe spawn site

!!! warning "Typed engineer processes use a different boundary"
    Do not apply the broad Copilot flags in this guide to typed OODA engineers.
    Their launch carries `SIMARD_ENGINEER_PERMISSIONS`, which selects scoped
    Copilot tool adapters instead. See
    [Engineer Copilot permissions](../reference/engineer-copilot-permissions.md).

> **The `simard::spawn_payload` facade ships** (`src/spawn_payload/mod.rs`). Use
> `attach_prompt_std` / `attach_prompt_tokio` for copilot prompts (stdin),
> `recipe_context` for `recipe-runner-rs` / `amplihack recipe run` context vars
> (inline small / file large), and `record_spawn_failure` to surface a pre-exec
> spawn error into the Overseer sink. It composes the existing
> `prompt_delivery::apply_std` / `apply_tokio` and
> `recipe_context_file::ContextFile::write` transports — do not re-implement them.

Use this runbook whenever you add code that spawns `copilot`,
`amplihack recipe run`, or `recipe-runner-rs` and hands it a payload — a prompt,
a context var, a brief, a message, a memory blob. Following it guarantees the new
site can never hit the recurring `E2BIG` ("Argument list too long") failure and
that the anti-regression guard stays green. For the why, see
[Comprehensive E2BIG elimination](../concepts/e2big-elimination.md); for the full
contract, see the [large-payload spawn API](../reference/large-payload-spawn-api.md).

## The one rule

> Never put a value that can grow large on `argv` or `envp`. Route the payload
> through [`simard::spawn_payload`](../reference/large-payload-spawn-api.md), which
> delivers copilot prompts on **stdin** and recipe context on a **file path**.

## 1. Classify your payload

Decide the tier (see the [audit](../reference/large-payload-spawn-api.md#whole-repo-launch-site-audit)):

- **Tier A/B — can grow** (prompt, context JSON, brief, free text, PR body): must
  go out-of-band. Continue below.
- **Tier C — fixed-size** (an ID, a repo slug, a path, a short const, a small
  integer): safe to pass inline. Add it to the guard's Tier-C allowlist with a
  one-line justification and stop here.

If unsure, treat it as Tier A/B. Inlining is the failure mode; out-of-band is
always correct.

## 2a. Copilot / `amplihack copilot` — deliver the prompt on stdin

```rust
use simard::spawn_payload;
use std::process::Command;

let mut cmd = Command::new("amplihack");
cmd.args(["copilot", "--subprocess-safe", "--allow-all-tools"]); // flags only — no prompt here

// Attach the prompt via the facade. It forces stdin (copilot reads its prompt
// from stdin when no `-p` is given), sets the child's stdin to a pipe, and
// returns an RAII feed guard.
let applied = spawn_payload::attach_prompt_std(&mut cmd, prompt_bytes)?;

let mut child = cmd.spawn()?;                          // may Err with E2BIG-class io::Error
// Feed on a thread so a large prompt cannot deadlock against the child's stdout.
let stdin = child.stdin.take();
let feeder = std::thread::spawn(move || applied.feed(stdin)); // writes prompt on stdin, closes EOF
let output = child.wait_with_output()?;
feeder.join().expect("feeder thread")?;               // surface a feed error loudly
```

Do **not**:

- `cmd.arg(format!("-p{prompt}"))` or `cmd.arg(prompt)` — inlines onto argv.
- `sh -c "amplihack copilot -p \"$(cat FILE)\""` — expands file contents onto
  argv (the exact recurring defect). If a PTY/shell wrapper is unavoidable, put
  only the **path** in the string and pipe it: `cat 'PATH' | amplihack copilot …`.

For the per-site grammar of the existing copilot launches, see the
[argv-free Copilot/OODA reference](../reference/argv-free-copilot-invocation.md).

## 2b. `recipe-runner-rs` / `amplihack recipe run` — file the context

```rust
use simard::spawn_payload::{self, RecipeArg};

let mut argv = vec!["recipe".to_string(), "run".to_string(), recipe_path];
let mut guards = Vec::new();   // keep ContextFile guards alive across output()

for (key, value) in context_vars {
    // Files the value if it reaches 8 KiB (lossless); otherwise inlines it.
    let arg = spawn_payload::recipe_context("my_base_type", key, value)?;
    argv.push("-c".to_string());
    argv.push(arg.arg_value());          // "key=val"  OR  "key_path=/tmp/.../key.ctx"
    if let RecipeArg::Filed(cf) = arg { guards.push(cf); }
}

let output = Command::new("recipe-runner-rs").args(&argv).output();
// `guards` stay in scope here so the temp files exist while the runner reads them.
```

Then update the **recipe asset** so it reads the file when the var was filed —
swap `{{key}}` for a "read the file at `{{key_path}}`" instruction and declare
`key_path` in the recipe's `context:` defaults, mirroring `distill-episodes.yaml`.
See [recipe context-file transport](../reference/recipe-context-file-transport.md#recipe-asset-changes).

Do **not** `argv.push(format!("{key}={value}"))` for a var that can grow — that
is the recipe-side E2BIG and the guard will fail your build.

## 3. Surface spawn failures — never swallow them

A spawn `Err` is a pre-exec `io::Error` (no exit status). Route it through the
facade so it is diagnosed and recorded at `error`, not `warn!`-dropped:

```rust
match Command::new("recipe-runner-rs").args(&argv).output() {
    Ok(out) => { /* handle out.status / stdout */ }
    Err(err) => {
        spawn_payload::record_spawn_failure(&err, "my_base_type::run");
        return Err(err.into());   // degrade loudly and readably — no silent fallback
    }
}
```

`record_spawn_failure` uses the errno-keyed
[`classify_spawn_failure`](../reference/terminal-failure-diagnosis-api.md)
(E2BIG=7 → `ArgListTooLong`, ENOSPC=28 → `DiskFull`, ENOMEM=12 → `OutOfMemory`).

## 4. Add a hermetic test

Add a per-path test that drives a **> 256 KiB** payload through your invocation
builder and asserts the payload never appears in `argv`/`envp` (a builder-level
assertion — no real subprocess). Model it on `tests/journal_e2big_transport.rs`
or `tests/ooda_e2big_transport.rs`:

```rust
#[test]
fn my_site_files_large_context_off_argv() {
    let big = "x".repeat(300 * 1024);                 // > 256 KiB, > ARG_MAX-per-arg
    let argv = build_my_site_argv(&[("context", &big)]);
    assert!(argv.iter().any(|a| a.starts_with("context_path=")));
    assert!(!argv.iter().any(|a| a.contains(&big)));   // payload NOT on argv
}
```

## 5. Confirm the guard is green

```bash
cargo test --test e2big_argv_guard
cargo test --test my_site_e2big_transport
```

`tests/e2big_argv_guard.rs` will fail if you left a `-p "$(cat` expansion, an
inline large `-c key=<contents>`, or a bare copilot/recipe spawn that skips the
facade. If it flags a genuinely Tier-C site, add that site to the guard's
allowlist constant with a one-line justification (a visible, reviewed change).

## Checklist

- [ ] Prompt delivered via `attach_prompt_*` (stdin), never argv or `$(cat …)`.
- [ ] Recipe context built via `recipe_context`; large vars become `*_path`.
- [ ] `ContextFile` guards kept alive across `output()`.
- [ ] Recipe asset reads `{{*_path}}` for any filed var.
- [ ] Spawn errors routed through `record_spawn_failure` — no silent fallback.
- [ ] A > 256 KiB hermetic per-path test asserts no argv/env inlining.
- [ ] `cargo test --test e2big_argv_guard` is green.
