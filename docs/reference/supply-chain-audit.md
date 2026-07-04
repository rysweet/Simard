---
title: Supply-chain audit and guardrails
description: "Reference for Simard's build-script / proc-macro audit, the cargo-deny policy (deny.toml), and the lockfile-only CI guardrail that enforces it."
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: active
related:
  - ./dependency-trust-policy.md
  - ./release-integrity.md
  - ../howto/self-maintain-dependency-pins.md
  - ./pr-finalization-pipeline.md
---

# Supply-chain audit and guardrails

> **Status: active.** This page documents the shipped supply-chain audit
> for issue #2260: the `deny.toml` policy, the `cargo-deny` CI guardrail,
> and the standing audit of every transitive crate that runs code at
> **build time** (`build.rs` scripts and proc-macros). It is both the
> operator reference and the spec the guardrail enforces.

Build scripts (`build.rs`) and procedural macros are the highest-leverage
supply-chain attack surface in a Rust project: unlike normal library code,
they execute **arbitrary code on the build host** during compilation, with
the full privileges of the user running `cargo build`. A compromised
`build.rs` can read environment variables, exfiltrate source, write to disk,
or shell out — all before a single test runs. This audit enumerates that
surface for Simard's dependency graph and pins a policy that fails CI if the
graph drifts into an un-reviewed state.

## At a glance

| Guardrail | File | Enforced by |
| --- | --- | --- |
| Advisory + license + bans + sources policy | [`deny.toml`](#denytoml-policy) | `cargo-deny` CI job (`verify.yml`) |
| Advisory-DB scan (RUSTSEC) | [`.cargo/audit.toml`](#relationship-to-cargo-audit) | existing `cargo-audit` CI job |
| Build-script / proc-macro review | this document | human review on dependency-graph change |
| Transitive trust certification | [`supply-chain/`](./dependency-trust-policy.md) | `cargo-vet` CI job |

`cargo-deny`, `cargo-audit`, and `cargo-vet` run as **separate, lockfile-only
CI jobs**. None compile the crate, so they add no measurable wall-time to the
memory-sensitive `pre-commit` build and never write the shared `rust-cache`.

## `deny.toml` policy

The repo-root `deny.toml` is the single source of truth for `cargo-deny`. It
has four sections; each is an explicit allow/deny list so that a new,
un-reviewed dependency, license, or source **fails CI** rather than silently
entering the graph.

### `[advisories]`

```toml
[advisories]
# Schema follows cargo-deny 0.19.x (pinned in the CI job below). In this
# schema severity is NOT configurable per advisory class: vulnerability
# advisories ALWAYS fail and can only be exempted with an explicit `ignore`.
# `unmaintained` takes a SCOPE, not a severity — the `workspace` scope fails
# only when the advisory hits a crate a workspace member depends on *directly*,
# so unmaintained crates that reach the graph purely transitively are reported
# without failing CI. `unsound` and `notice` informational advisories are not
# raised by cargo-deny's default checks at all (they remain visible via the
# `cargo audit` job, which reports them as non-failing warnings).
db-urls = ["https://github.com/rustsec/advisory-db"]
yanked = "deny"
unmaintained = "workspace"
ignore = [
    # RUSTSEC-2023-0071 — Marvin Attack (timing side-channel) in `rsa`.
    # A *vulnerability*, so it fails by default and must be exempted
    # explicitly. No fixed upstream release exists. Reaches the graph
    # transitively via rustyclawd-tools -> octocrab -> jsonwebtoken -> rsa,
    # and is used only to verify JWTs issued by GitHub, not to decrypt
    # attacker-controlled ciphertext. Tracked:
    # https://github.com/RustCrypto/RSA/issues/19
    { id = "RUSTSEC-2023-0071", reason = "rsa Marvin timing side-channel; no upstream fix; transitive JWT-verify only" },
]
```

The advisory **gate** is the `cargo deny check advisories` invocation. Its
acceptance criterion is *"no remaining unmitigated advisories"*, which against
the current graph is satisfied two complementary ways:

- **Vulnerabilities always fail,** so each must be resolved (`cargo update` to a
  patched version) or carry an explicit, justified, tracked `ignore`. `rsa` /
  RUSTSEC-2023-0071 is the **single** standing `ignore` — the only advisory in
  the graph with no upstream fix.
- **Transitive `unmaintained` advisories** (`paste`, `proc-macro-error2`) do
  **not** fail, because `unmaintained = "workspace"` fails only on an advisory
  against a *direct* workspace dependency and both reach the graph transitively.
  **`unsound` advisories** (`lru`, `git2`) are not raised by cargo-deny at all;
  they stay visible as non-failing warnings in the `cargo audit` job. All four
  are tracked for an upstream bump rather than ignored per-ID. See
  [Dependency trust policy → advisory resolution](./dependency-trust-policy.md#advisory-resolution-policy).

> **Why `ignore`, not silence.** An `ignore` entry is a security-sensitive
> change: it must name the advisory ID, state why the crate is not exploitable
> in Simard's usage, and link an upstream tracking issue. An `ignore` with no
> justification is rejected in review the same as any other unreviewed code.
> The `unmaintained = "workspace"` *scope* is deliberately preferred over
> blanket per-ID ignores so a transitive advisory stays **visible** in CI
> (`cargo audit`) until the upstream crate is bumped.

### `[licenses]`

```toml
[licenses]
# Permissive OSI allowlist. New licenses outside this set fail CI and must be
# reviewed before the dependency lands.
allow = [
    "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
    "Unicode-3.0", "MPL-2.0", "CC0-1.0", "Unlicense",
]
confidence-threshold = 0.8
# Per-crate exceptions. The global allowlist stays strictly permissive; a
# non-permissive (or non-standard-software) license is allowed ONLY by an
# explicit, justified per-crate exception so it stays visible in every CI run.
[[licenses.exceptions]]
crate = "html2md"          # GPL-3.0-or-later (copyleft) — see the note below
allow = ["GPL-3.0-or-later"]
[[licenses.exceptions]]
crate = "webpki-roots"     # CDLA-Permissive-2.0 (permissive data license)
allow = ["CDLA-Permissive-2.0"]
[[licenses.exceptions]]
crate = "auto_generate_cdp" # GPL-3.0-or-later, BUILD-ONLY code generator
allow = ["GPL-3.0-or-later"]
```

The allowlist is **permissive-only**: copyleft beyond `MPL-2.0` (file-level)
is excluded. A dependency under an unlisted license — or with no license at
all — fails `cargo deny check licenses`.

> **Flagged finding — `html2md` is GPL-3.0-or-later.** One transitive crate,
> [`html2md` 0.2.15](https://crates.io/crates/html2md), is copyleft. It reaches
> the graph as a *direct* dependency of the first-party git crate
> `rustyclawd-tools` (`html2md -> rustyclawd-tools -> simard`) and is used only
> for server-side HTML→Markdown conversion of fetched content. Rather than
> weakening the global permissive policy, it is allowed by a single, explicit
> `[[licenses.exceptions]]` entry so the copyleft dependency stays **visible**
> and reviewable. Removing it is upstream (`rustyclawd-tools`) work; until then
> the exception is the sanctioned, tracked path — the licensing analogue of the
> `rsa` advisory `ignore`.

> **Flagged findings — the `dashboard-audit` default chain (issue #2576).**
> Making `dashboard-audit` a **default** feature brought its `headless_chrome`
> subtree into the default-feature license graph, surfacing two crates that were
> previously outside the checked set:
>
> - [`webpki-roots`](https://crates.io/crates/webpki-roots) ships Mozilla's CA
>   certificate bundle under **CDLA-Permissive-2.0** — a *permissive* data
>   license (no copyleft), not a standard OSI *software* license. Chain:
>   `webpki-roots -> ureq -> auto_generate_cdp (build) -> headless_chrome`.
> - [`auto_generate_cdp`](https://crates.io/crates/auto_generate_cdp) is
>   **GPL-3.0-or-later** but is a **build-dependency only** — a code generator
>   that emits the Chrome DevTools Protocol bindings at build time. It is neither
>   linked into nor distributed with the Simard binary, so its copyleft does not
>   extend to Simard (the same reasoning that lets a GPL compiler build a
>   non-GPL program).
>
> Both are allowed by explicit, justified `[[licenses.exceptions]]` entries
> rather than by widening the global permissive allowlist, keeping them visible
> and reviewable in every CI run.

### `[bans]`

```toml
[bans]
# Multiple versions of the same crate are surfaced (warn) so duplicate-tree
# bloat is visible without blocking a routine transitive bump.
multiple-versions = "warn"
# Wildcard ("*") version requirements are surfaced. The only wildcards are the
# four first-party git dependencies, which carry no semver because they are
# pinned by exact `rev`; their integrity is enforced by those rev pins plus the
# [sources] git allowlist, so this is "warn", not "deny".
wildcards = "warn"
# No crate is currently banned outright. `deny` entries here are how a crate
# would be hard-blocked if a future advisory has no acceptable mitigation.
deny = []
```

### `[sources]`

```toml
[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
# Git sources are an explicit allowlist. Simard pins four crates by exact git
# rev across three upstream repositories; anything else fails CI.
allow-git = [
    "https://github.com/rysweet/RustyClawd.git",
    "https://github.com/rysweet/amplihack-memory-lib.git",
    "https://github.com/rysweet/amplihack-rs.git",
]
```

This `[sources]` allowlist is the supply-chain complement of the exact-rev
pinning described in
[How to keep Simard's dependency pins up to date](../howto/self-maintain-dependency-pins.md):
pinning makes each git dependency *reproducible*; the allowlist makes any
*new* git source — a typosquat, a hijacked transitive dep — **fail closed**.

> **Three repositories, four crate pins.** `rustyclawd-core` and
> `rustyclawd-tools` both pin `RustyClawd` at the same rev, so the allowlist
> has three URLs even though `Cargo.toml` has four git dependencies.

## CI guardrail

`cargo-deny` runs as a dedicated job in `.github/workflows/verify.yml`,
modelled exactly on the existing `cargo-audit` job:

```yaml
cargo-deny:
  runs-on: ubuntu-latest
  timeout-minutes: 10
  permissions:
    contents: read
  steps:
    - name: Check out repository
      uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4
    - name: Install cargo-deny
      uses: taiki-e/install-action@4e4e4d1450e58bef95d6f394ac20d46ad7d24ebf # cargo-deny
      with:
        tool: cargo-deny@0.19.9
    - name: Run cargo deny
      run: cargo deny --locked check
```

Key properties (shared with the other guardrail jobs):

- **Lockfile-only.** No `rust-runner-prep`, no crate compilation — it reads
  `Cargo.lock` and `deny.toml` only. It is **not** part of the
  memory-sensitive `pre-commit` job.
- **Read-only permissions** (`contents: read`); no token write scope.
- **Never writes the shared cache.** It does not touch `Swatinem/rust-cache`,
  so it cannot contend with the single-writer `simard-ci-v2` cache.
- **SHA-pinned action with explicit, version-pinned `tool:`.**
  `taiki-e/install-action` is pinned by commit SHA, and `tool: cargo-deny@0.19.9`
  pins **both** the tool and its version. The version pin is load-bearing: the
  `[advisories]` schema (scope-valued `unmaintained` / `unsound`, with the old
  per-class severity keys removed) is the 0.19.x format, so an unpinned upgrade
  could change how `deny.toml` is interpreted. The `tool:` input is also
  required for selection: `taiki-e` publishes one tag (and commit) per tool, so
  a SHA pin alone — as the existing `cargo-audit` job uses — selects the tool
  baked into *that* commit. Reusing the `cargo-audit` SHA without a `tool:`
  input would install **cargo-audit**, not cargo-deny.
- **`--locked`.** Passed to `cargo-deny` itself (`cargo deny --locked check`;
  `--locked` is a top-level flag, before the `check` subcommand). It fails if
  `Cargo.lock` is dirty, so the check always reflects the committed graph (no
  implicit resolution).

## Build-script (`build.rs`) inventory

Crates with a `build.rs` fall into two risk classes. The **high-attention**
class compiles native C/C++/assembly via the [`cc`](https://crates.io/crates/cc)
crate (often orchestrated by [`cmake`](https://crates.io/crates/cmake) or
[`pkg-config`](https://crates.io/crates/pkg-config)); these download nothing at
build time but do invoke a system toolchain. The **low-risk** class runs a
small Rust feature-probe that emits `cargo:rustc-cfg` flags and touches neither
the network nor files outside `OUT_DIR`.

### High-attention: native C / C++ / assembly compilation

| Crate | Version | Build-time behaviour | Notes |
| --- | --- | --- | --- |
| `libsqlite3-sys` | 0.28.0 | Compiles **bundled SQLite** C source via `cc`. | Pulled by `rusqlite { features = ["bundled"] }`. Vendored amalgamation; no network fetch. |
| `openssl-sys` | 0.9.113 | Locates/links system OpenSSL via `pkg-config`/`vcpkg`; runs a small C probe. | Links the **system** library; does not vendor it. |
| `libgit2-sys` | 0.18.3+1.9.2 | Compiles **vendored libgit2** C source via `cc`. | Pulled by `git2`; see the `git2` warnings in [advisory resolution](./dependency-trust-policy.md#advisory-resolution-policy). |
| `libssh2-sys` | 0.3.1 | Compiles/links libssh2 (C). | Transitive via `libgit2-sys`. |
| `libz-sys` | 1.1.28 | Compiles/links zlib (C). | Transitive via `libgit2-sys`/`openssl-sys`. |
| `aws-lc-sys` | (locked) | Compiles **AWS-LC** C/assembly via `cmake`/`cc`. | Crypto backend reached transitively (e.g. via `rustls`/`quinn`). Vendored source, no network. |
| `ring` | 0.17.14 | Compiles bundled C + assembly crypto primitives via `cc`. | Vendored; widely audited. |
| `lbug` | 0.17.1 | Links a prebuilt `liblbug.a` native static library via its external-lib interface. | Simard pins the link path in CI (`LBUG_LIBRARY_DIR`); see [#2426 handling in `verify.yml`](../howto/reclaim-disk-space-and-run-low-space-rust-builds.md). |

> **The C toolchain is the real boundary.** None of these scripts fetch code
> over the network — they compile **vendored or system** C/asm with the host
> toolchain. The audit conclusion is that the trust boundary is the C compiler
> + the vendored source already in `Cargo.lock`, both reproducible at a fixed
> commit. `cargo-vet` certifies the Rust crates that *wrap* them; the
> [`[sources]` allowlist](#sources) prevents an un-reviewed sys-crate fork from
> entering the graph.

### Low-risk: feature-probe build scripts

A number of widely-used crates ship a `build.rs` that only detects compiler
capabilities and emits `cfg` flags — `libc`, `serde`, `proc-macro2`,
`typenum`, `generic-array`, `crc32fast` (SIMD detection), `anyhow`,
`httparse`, `slab`, and similar. These are reviewed as **low-risk**: they read
no secrets, perform no I/O outside `OUT_DIR`, and make no network calls.

### Simard's own `build.rs`

Simard's repo-root [`build.rs`](https://github.com/rysweet/Simard/blob/main/build.rs)
is itself in scope and audited as **benign**:

- It shells out to `git rev-parse HEAD` and `git rev-list --count HEAD` to
  embed `SIMARD_GIT_HASH` and `SIMARD_BUILD_NUMBER` via `cargo:rustc-env`.
- It reads one optional environment override (`SIMARD_BUILD_NUMBER`).
- It performs **no network access** and writes nothing outside the standard
  cargo build outputs.

The git-hash embed has a reproducibility consequence: the emitted binary is
deterministic **only at a fixed commit** (a different `HEAD` yields a
different `SIMARD_GIT_HASH`). This is documented as an explicit caveat in
[Release integrity → reproducibility](./release-integrity.md#build-reproducibility).

## Proc-macro inventory

Proc-macros run at compile time inside the compiler process. Simard's graph
contains only **mainstream, widely-vendored** derive/attribute macros, none of
which perform network or out-of-`OUT_DIR` filesystem access:

| Proc-macro crate | Role |
| --- | --- |
| `serde_derive` | `#[derive(Serialize, Deserialize)]` |
| `thiserror-impl` | `#[derive(Error)]` |
| `tokio-macros` | `#[tokio::main]`, `#[tokio::test]` |
| `tracing-attributes` | `#[instrument]` |
| `async-trait` | `#[async_trait]` |
| `futures-macro`, `pin-project-internal` | async plumbing |
| `zerocopy-derive` | zero-copy derives |
| `strum_macros`, `displaydoc` | enum/string derives |
| `paste` | token pasting (macro hygiene helper) — *unmaintained*, RUSTSEC-2024-0436; reaches the graph only via `ratatui` (see [advisory resolution](./dependency-trust-policy.md#advisory-resolution-policy)) |
| `proc-macro-error2` / `proc-macro-error-attr2` | macro error reporting — *unmaintained*, RUSTSEC-2026-0173 on `proc-macro-error2` (transitive via `validator` → `rustyclawd-tools`; see [advisory resolution](./dependency-trust-policy.md#advisory-resolution-policy)) |
| `syn`, `quote`, `proc-macro2` | the parsing/quoting foundation every macro above is built on |

The audit conclusion: the proc-macro surface is entirely conventional. New or
unusual proc-macro crates entering the graph are caught by `cargo-vet`
certification and the `[sources]` allowlist before they can run.

## `cargo-auditable` decision

Issue #2260 asks whether to adopt
[`cargo-auditable`](https://github.com/rust-secure-code/cargo-auditable),
which embeds the dependency list into the compiled binary so it can be audited
post-build. **Decision: documented, deferred.**

- The same information is already published per-release as a CycloneDX SBOM
  (see [Release integrity](./release-integrity.md)), which is the
  industry-standard, tool-agnostic artifact.
- `cargo-auditable` replaces the default `cargo build` invocation, which would
  perturb the carefully OOM-tuned release/CI build profiles (`NODE_OPTIONS`,
  `CARGO_PROFILE_*`, mold linker, line-tables-only debuginfo).
- It is reconsidered if/when consumers need offline post-build auditing of a
  binary without its accompanying SBOM.

## Running the guardrail locally

```bash
# Install once (pin matches the CI job: cargo-deny 0.19.x).
cargo install cargo-deny --version 0.19.9 --locked

# Full policy check (advisories + licenses + bans + sources).
cargo deny --locked check

# Scope to one section while iterating on deny.toml:
cargo deny check advisories
cargo deny check licenses
cargo deny check sources
```

A green `cargo deny --locked check` is the same gate CI runs. Pair it with
`cargo audit` (the existing RUSTSEC scan) and `cargo vet --locked` (trust
certification) for the full local supply-chain check.

## Relationship to `cargo-audit`

`cargo-audit` (already wired in `verify.yml`, configured by `.cargo/audit.toml`)
and `cargo-deny` overlap on advisory scanning but are **kept side by side**, not
merged:

- `cargo-audit` is the focused, fast RUSTSEC scanner and remains the
  authoritative advisory job.
- `cargo-deny` adds **license, source, and ban** enforcement that `cargo-audit`
  does not cover, plus a second, policy-driven advisory view.

Both honour the same `RUSTSEC-2023-0071` (`rsa`) exemption, expressed once in
`.cargo/audit.toml` and once in `deny.toml`, each with the identical
justification and tracking link, so the two cannot drift into disagreement.

## See also

- [Dependency trust policy](./dependency-trust-policy.md) — `cargo-vet`
  certification, trusted-crate criteria, and the advisory-resolution workflow.
- [Release integrity](./release-integrity.md) — SBOM generation, cosign
  signing, and build-reproducibility caveats.
- [Keep Simard's dependency pins up to date](../howto/self-maintain-dependency-pins.md) —
  exact-rev pinning of the three upstream git sources this policy allowlists.
- [Security policy](https://github.com/rysweet/Simard/blob/main/SECURITY.md) —
  vulnerability reporting and supported versions.
