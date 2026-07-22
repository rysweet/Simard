---
title: Canary gate env-isolation contract (reference)
description: >
  The API surface of Simard's self-deploy canary gate env-isolation fix — the
  hermetic `scrub_gate_env` subprocess environment (env_clear() + minimal base
  floor + two gate env profiles), the candidate-binary deploy-signal allow-list
  vs. the neutral-HOME / no-SIMARD_* unit-test profile, the names-only
  `RelaunchConfig.canary_env` field, the closed `canary_gate_env_allowlist()`
  set, and the redact-then-bound `bound_gate_detail` credential guard. Documents
  why the self-deploy canary went persistently RED on the deploy host while
  `cargo test` was green in CI, and the contract that makes healthy candidates
  render true-GREEN.
last_updated: 2026-07-22
owner: simard
doc_type: reference
status: reference
related:
  - ../howto/canary-gate-env-isolation.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./state-root-resolution.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../concepts/deploy-aware-done-gate.md
  - ../safe-self-update.md
---

# Canary gate env-isolation contract (reference)

This reference documents the environment-isolation contract for Simard's
in-process **self-deploy canary gates** (`src/self_relaunch/gates.rs`). For the
operator runbook — how to recognise a leaked-env RED canary and confirm the fix
on a live host — see
[the canary-gate env-isolation how-to](../howto/canary-gate-env-isolation.md).

**One-line summary:** every gate subprocess is spawned in a **hermetic
environment** built from `env_clear()` + a fixed minimal base floor + one of
**two gate env profiles**. The candidate-binary gates (`smoke`, `gym-baseline`,
`rpc-health`) receive a **closed, code-defined deploy-signal allow-list** and
the live `HOME` so the candidate resolves its home/state like the daemon. The
**`unit-test` (`cargo test`) gate** receives **no `SIMARD_*` and a neutral
scratch `HOME`**, so env-sensitive tests resolve to their own hermetic
fixtures. Ambient `SIMARD_*` / live `HOME` leaked from the running daemon can no
longer reach `cargo test`, so a healthy candidate renders **true-GREEN** on the
deploy host instead of a spurious exit-101 RED.

## Why this exists (root cause)

The self-deploy canary is the overseer's **in-process** gate that decides
whether a freshly built candidate binary is healthy enough to hand over to.
Before this fix, `run_unit_test_gate` spawned `cargo test` while **inheriting the
live daemon's ambient environment** — it never called `env_clear()`.

That produced a defect that was invisible in CI and only reproducible on a
deploy host:

| Observation | Value |
|---|---|
| `cargo test --lib --all-features` at HEAD | **passes** (0 failed) |
| GitHub CI `verify.yml` on `main` | **green** (last 8 runs) |
| Overseer self-deploy canary (`run_unit_test_gate`) | **RED**, `tests failed (exit 101)` |

The running daemon exports `SIMARD_HOME`, `SIMARD_STATE_ROOT`,
`SIMARD_PROMPT_ASSETS_DIR` and a populated `HOME` pointing at **live** state.
Env-sensitive library tests, when they inherited those live values instead of
their own hermetic fixtures, panicked (exit status `101`) — but **only** under
the daemon, never under a clean CI shell. The red gate kept `deploy_gate` red,
so the binary stayed one commit behind merged `main` (DeployDrift) and could
not self-update.

The symptom looked like "a failing `Drop` test in `src/lib.rs`". The **root
cause** was gate **environment leakage**. The fix isolates the gate
environment; no test is disabled, `#[ignore]`d, or deleted.

## `scrub_gate_env` — hermetic gate subprocess environment

`scrub_gate_env` is applied to **all four** gate spawns before any other
`.env(...)` call. It is the deploy-side sibling of the git-neutralisation
pattern already used by the build path
([`self-deploy-source-prep`](./self-deploy-source-prep.md)).

It builds every gate environment from three layers — `env_clear()`, a minimal
base floor, then a **profile-specific** layer — because the four gates fall into
two categories with **opposite** environment needs (see
[Two gate env profiles](#two-gate-env-profiles)).

> **Implementation note.** Today only `run_unit_test_gate` and
> `run_rpc_health_gate` receive `&RelaunchConfig`; `run_smoke_gate` and
> `run_gym_baseline_gate` take only `binary`. To scrub all four, those two
> signatures must gain a `config: &RelaunchConfig` parameter (threaded through
> `run_gate`) so `scrub_gate_env(cmd, config, profile)` can apply the correct
> profile.

```rust
/// Which environment profile a gate subprocess must run under.
enum GateEnvProfile {
    /// Runs the freshly built candidate `simard` binary. It must resolve its
    /// home / state / prompt-assets exactly like the running daemon, so it
    /// receives the deploy-signal allow-list AND the live `HOME`.
    CandidateBinary,
    /// Runs `cargo test`. It must NOT see the daemon's live `SIMARD_*` or live
    /// `HOME`, or env-sensitive tests would resolve to live state (via the
    /// `SIMARD_HOME` -> `$HOME/.simard` fallback) and panic (exit 101). Gets a
    /// neutral scratch `HOME` and no deploy signals.
    UnitTest,
}

/// Configure `cmd` with a hermetic environment for a canary gate subprocess.
///
/// Ordering is mandatory: `env_clear()` FIRST, then the minimal base floor,
/// then the profile-specific layer. Any ambient variable NOT explicitly
/// re-injected is dropped. This prevents the running daemon's live `SIMARD_*` /
/// `HOME` state from leaking into `cargo test` and panicking env-sensitive
/// tests (exit 101) on the deploy host only.
fn scrub_gate_env(cmd: &mut Command, config: &RelaunchConfig, profile: GateEnvProfile) {
    // 1. Deny by default.
    cmd.env_clear();

    // 2. Minimal base floor required for the toolchain/binary to run at all.
    //    Carries NO deploy state and NO `HOME` (`HOME` is profile-specific).
    for key in GATE_ENV_BASE_FLOOR {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }

    // 3. Profile-specific layer.
    match profile {
        // Candidate binary: resolve home/state like the running daemon.
        GateEnvProfile::CandidateBinary => {
            if let Ok(home) = std::env::var("HOME") {
                cmd.env("HOME", home); // live HOME, on purpose
            }
            // Deploy signals re-injected by NAME (values read live).
            for name in &config.canary_env {
                if let Ok(val) = std::env::var(name) {
                    cmd.env(name, val);
                }
            }
        }
        // Unit tests: neutral HOME, NO SIMARD_* — tests use their own fixtures.
        GateEnvProfile::UnitTest => {
            let neutral_home = config.canary_target_dir.join("gate-home");
            let _ = std::fs::create_dir_all(&neutral_home);
            cmd.env("HOME", neutral_home);
            // Intentionally NO SIMARD_HOME / SIMARD_STATE_ROOT /
            // SIMARD_PROMPT_ASSETS_DIR here.
        }
    }
}
```

## Two gate env profiles

The four gates split into two categories with opposite environment needs. This
split is the crux of the fix: applying the deploy-signal allow-list *uniformly*
to all four gates would re-inject the very variables that cause the leak into
`cargo test`, re-triggering the exact exit-101 defect this fix removes.

| Gate | Profile | `HOME` | `SIMARD_*` | Rationale |
|---|---|---|---|---|
| `smoke` | `CandidateBinary` | live | allow-list | runs candidate `--version`; must resolve like the daemon |
| `gym-baseline` | `CandidateBinary` | live | allow-list | runs `candidate gym list`; needs daemon-equivalent home/state |
| `rpc-health` | `CandidateBinary` | live | allow-list | runs `candidate probe rpc`; talks to the live daemon surface |
| `unit-test` | `UnitTest` | **neutral scratch** | **none** | runs `cargo test`; must use each test's own hermetic fixture |

**Why the `unit-test` gate is different.** `SIMARD_HOME` falls back to
`$HOME/.simard` (`resolve_installed_binary` in `src/self_deploy/health.rs`,
`resolve_simard_home` in `src/install/paths.rs`), so injecting either a live
`SIMARD_HOME` *or* just a live `HOME` re-points env-sensitive tests at **live**
daemon state — reproducing the exact exit-101 panic this fix exists to remove.
The `unit-test` gate therefore gets a **neutral scratch `HOME`** (a fresh dir
under `canary_target_dir`) and **no `SIMARD_*`** at all, so each test resolves
to the fixture it sets up itself (`test_support::hermetic`), exactly as it does
in CI.

**Why the candidate-binary gates are not.** Smoke, gym-baseline, and rpc-health
run the **candidate `simard` binary**, which is supposed to behave like the
running daemon — it must find the same install root, state root, and prompt
assets. Those three gates therefore receive the closed deploy-signal allow-list
and the live `HOME`. They do **not** run `cargo test`, so the env-sensitive-test
panic does not apply to them.

### Base floor

The base floor is the fixed, minimal set of variables both the Rust toolchain
and the candidate binary need to execute under `env_clear()`. It carries **no
deploy state** and **no `HOME`** — `HOME` is set by the profile layer.

| Variable | Purpose |
|---|---|
| `PATH` | locate `cargo`, `rustc`, linker, and the candidate's runtime deps |
| `USER` | some toolchain/test scaffolding reads it |
| `CARGO_HOME` | cargo registry/cache (set explicitly, so a neutral `HOME` is safe) |
| `RUSTUP_HOME` | rustup toolchain root (set explicitly, so a neutral `HOME` is safe) |

Because `CARGO_HOME` and `RUSTUP_HOME` are pinned in the base floor, the
`unit-test` gate's neutral `HOME` never breaks the toolchain — cargo/rustup do
not have to fall back to `$HOME`.

`CARGO_BUILD_JOBS` continues to be set explicitly by each gate spawn (via
`crate::cargo_jobs::cargo_jobs()`) **after** `scrub_gate_env`, exactly as
before.

### Guarantees

- **Deny-by-default.** `env_clear()` runs first; nothing is inherited implicitly.
- **No wildcard `SIMARD_*` pass-through.** Only the candidate-binary gates see
  the three allow-listed names, and only when the daemon actually exports them.
  The `unit-test` gate sees **no** `SIMARD_*`.
- **No live-state leak into tests.** The `unit-test` gate's `HOME` is a neutral
  scratch dir, so the `SIMARD_HOME` -> `$HOME/.simard` fallback cannot reach
  live daemon state.
- **Idempotent + order-safe.** The profile layer runs last, so a profile value
  always wins over an accidental base-floor collision.

## `canary_gate_env_allowlist()` — closed deploy-signal set (candidate-binary gates)

The allow-list is a **constant, code-defined closed set** applied **only to the
candidate-binary gates** (`smoke`, `gym-baseline`, `rpc-health`). It is **not**
extensible via config, CLI flags, or the environment — a security property, not
an oversight (deny-by-default; see [Security posture](#security-posture)). The
`unit-test` gate never receives it.

```rust
/// The closed set of environment-variable NAMES a canary gate is permitted to
/// receive from the live daemon environment. Code-defined and constant: not
/// extensible via config, CLI, or env. Deny-by-default.
pub fn canary_gate_env_allowlist() -> Vec<String> {
    vec![
        "SIMARD_HOME".to_string(),
        "SIMARD_PROMPT_ASSETS_DIR".to_string(),
        "SIMARD_STATE_ROOT".to_string(),
    ]
}
```

| Name | Why it is allowed through | Where it is resolved |
|---|---|---|
| `SIMARD_HOME` | candidate must resolve its install/home root | [self-deploy-api](./self-deploy-api.md) |
| `SIMARD_PROMPT_ASSETS_DIR` | prompt assets the candidate loads at boot | [prompt-delivery](../prompt-delivery.md) |
| `SIMARD_STATE_ROOT` | state-root the candidate reads/writes | [state-root-resolution](./state-root-resolution.md) |

These three are the **deploy signals** a candidate binary legitimately needs to
behave like the running daemon during gating. Everything else — including all
other `SIMARD_*` variables — is dropped. The `unit-test` gate receives **none**
of them (see [Two gate env profiles](#two-gate-env-profiles)).

## `RelaunchConfig.canary_env` — names-only carrier

`canary_env` carries the deploy-signal allow-list into the **candidate-binary**
gate spawns. It stores **variable names only**; values are read live at spawn
time and are **never persisted, serialized, or logged**. The `unit-test` gate
does not read `canary_env`.

```rust
#[derive(Clone, Debug)]
pub struct RelaunchConfig {
    pub canary_target_dir: PathBuf,
    pub health_timeout: Duration,
    pub manifest_dir: PathBuf,
    /// Names (not values) of the env vars a gate subprocess may receive from
    /// the live environment. Populated from `canary_gate_env_allowlist()`.
    /// Values are resolved live in `scrub_gate_env` and never stored here.
    pub canary_env: Vec<String>,
}
```

`RelaunchConfig::default()` populates `canary_env` from
`canary_gate_env_allowlist()`:

```rust
impl Default for RelaunchConfig {
    fn default() -> Self {
        Self {
            canary_target_dir: std::env::temp_dir()
                .join(format!("simard-canary-{}", std::process::id())),
            health_timeout: Duration::from_secs(30),
            manifest_dir: PathBuf::from("."),
            canary_env: canary_gate_env_allowlist(),
        }
    }
}
```

**Contract:** `canary_env` is names-only. `RelaunchConfig`'s `Debug` /
serialization surfaces the *names*, never resolved values. Callers must not
push raw values into `canary_env`.

## `bound_gate_detail` — redact-then-bound credential guard

Gate failure detail (`GateResult.detail`) may echo subprocess **stderr**, which
is untrusted, may contain URL-embedded credentials, and may be arbitrarily
long. `bound_gate_detail` applies two transforms in a **mandatory order**:

1. **Redact** credentials — strip `scheme://user:pass@host` userinfo and known
   secret-bearing tokens.
2. **Bound** — UTF-8 / char-boundary-safe truncate so the returned string,
   including the ellipsis, is at most **512 bytes** (snaps the content boundary
   **down** to guarantee the total never exceeds the cap).

```rust
/// Redact URL-embedded credentials THEN char-boundary-safe truncate so the
/// final string (including any ellipsis) is at most 512 bytes. Order is
/// mandatory: redact BEFORE bound, so truncation can never split a credential
/// in a way that leaks the tail or defeats the redactor. The cap counts the
/// trailing ellipsis and snaps DOWN to a UTF-8 char boundary, so multi-byte
/// stderr can neither panic the overseer tick nor exceed 512 bytes.
fn bound_gate_detail(raw: &str) -> String {
    const MAX: usize = 512;
    const ELLIPSIS: &str = "...";
    let redacted = redact_credentials(raw);
    if redacted.len() <= MAX {
        return redacted;
    }
    let mut end = MAX - ELLIPSIS.len();
    while end > 0 && !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", redacted[..end].trim_end())
}
```

- **Total bound counts the ellipsis.** The 512-byte cap is on the *returned*
  string, ellipsis included; the content boundary snaps **down** so the result
  is always `<= 512` bytes. (A naive "truncate to 512 then append `...`" would
  overshoot to 515.)

- **Redact-before-bound is mandatory.** Truncating first could cut a
  `user:pass@host` token so the redactor no longer matches it.
- **UTF-8-safe.** Truncation snaps to a `char` boundary, so multi-byte stderr
  can never panic the overseer tick.
- Applied to gate `detail` and to `CanaryResult::refusal_reason` in
  `src/overseer/deploy.rs`.

## Observability

Gate outcomes are reported through **structured `tracing` + OTel only** — never
`print!`/`println!`. Emitted fields are **names and outcomes, never values**:

| Field | Example | Notes |
|---|---|---|
| `gate` | `unit-test` | `RelaunchGate` display label |
| `passed` | `false` | boolean outcome |
| `detail` | `tests failed (exit 101): …` | already redacted + bounded |
| `canary_env.names` | `["SIMARD_HOME", …]` | allow-list **names** only |

Allow-listed **values** are never logged, traced, or exported. There is no CRLF
log-forging surface: bounded, redacted detail is emitted as a structured field,
never interpolated into a log line or a shell/path/format string.

## Security posture

| ID | Property |
|---|---|
| SEC-A1 | **Fail-closed authority preserved.** An unhealthy candidate is still RED; a red `deploy_gate` remains fatal (`is_transient` unchanged, #4420). Env isolation only prevents *false* REDs, never masks a *real* one. |
| SEC-I1 | `canary_env` is **names-only**, a **closed constant set**; not supplied by config, CLI, or env. |
| SEC-I2 | `env_clear()` runs **first**; only the minimal proven floor + the profile layer are re-injected. No wildcard `SIMARD_*` pass-through. |
| SEC-I2b | The `unit-test` gate receives **no `SIMARD_*`** and a **neutral scratch `HOME`**, so `cargo test` cannot resolve to live daemon state (the `SIMARD_HOME` -> `$HOME/.simard` fallback is defeated). The deploy-signal allow-list is confined to the candidate-binary gates. |
| SEC-I3 | Subprocess stderr is treated as **tainted** — redacted, bounded, and never interpolated into shell/path/format strings. |
| SEC-D1 | `canary_env` values are **never** persisted, serialized, or logged. |
| SEC-D2 | Credentials are **redacted before** the 512B bound. |
| SEC-D3/D4 | Structured `tracing`/OTel only (names + outcome); no `print!`/`println!`; no CRLF log-forging. |

**Net posture:** security-positive. The change **reduces** existing ambient-env
leakage into gate subprocesses and introduces no new attack surface.

## Behavioural contract (acceptance)

1. All four gates scrub env via `env_clear()` + base floor + a **profile
   layer**. The candidate-binary gates (`smoke`, `gym-baseline`, `rpc-health`)
   get the deploy-signal allow-list + live `HOME`; the `unit-test` gate gets
   **no `SIMARD_*`** + a **neutral scratch `HOME`**. A healthy candidate renders
   **GREEN** on the deploy host → `deploy_gate` green → DeployDrift clears.
2. `cargo test --all-features` and the in-process `unit-test` gate both pass; no
   exit 101, because the gate never sees live `SIMARD_*`/`HOME`.
3. The candidate-binary allow-list is exactly `{SIMARD_HOME,
   SIMARD_PROMPT_ASSETS_DIR, SIMARD_STATE_ROOT}` — names, not values. The
   `unit-test` gate's allow-list is **empty**.
4. Gate `detail` and `refusal_reason` are **redacted then bounded** (≤512B,
   UTF-8-safe).
5. Fail-closed preserved: an unhealthy candidate is still RED.
6. Hermetic test infra (`test_support::hermetic`, `serial(cognitive_memory)`)
   is not broken.

## Out of scope

- The `pre-commit` CI cluster (tracked separately).
- `amplihack-rs` JSON-parsing regression (`amplihack-rs#969`).
- Any `Drop`-semantics refactor — the `Drop` framing was a symptom, not the
  root cause.
- New features, renames, or config-extensible allow-lists.

## Related

- [How to diagnose and confirm the canary gate env-isolation fix](../howto/canary-gate-env-isolation.md)
- [Self-Deploy API](./self-deploy-api.md)
- [Self-Deploy Source Prep & Warm Target Dir](./self-deploy-source-prep.md)
- [State-Root Resolution](./state-root-resolution.md)
- [Reconcile-and-Self-Deploy](../concepts/reconcile-and-self-deploy.md)
- [Deploy-Aware Done-Gate](../concepts/deploy-aware-done-gate.md)
