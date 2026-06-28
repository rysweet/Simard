---
title: Multi-binary self-update reference
description: Reference for the multi-binary self-update path that replaces every Simard executable (simard plus simard-tui, simard-gym, and the rest of the auxiliary binary set) on `simard update`, the dynamic binary discovery, the InstallReport main-fatal/aux-best-effort contract, the SHA-256 checksum gate, the zip-slip and https-only transport hardening, and the matching release-packaging producer contract.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-api.md
  - ./update-check.md
  - ../safe-self-update.md
  - ../howto/check-for-updates.md
  - ../reference/simard-cli.md
  - ../../src/cmd_self_update/download.rs
  - ../../src/cmd_self_update/update.rs
  - ../../.github/workflows/release.yml
---

# Multi-binary self-update reference

> **Status: implemented.** `simard update` replaces the **entire** Simard
> binary set — the main `simard` daemon binary **and** every auxiliary
> executable shipped in the release tarball (`simard-tui`, `simard-gym`,
> `simard-ooda-step`, the `simard-audit-*` tools, and so on) — not just the
> daemon binary. The dynamic discovery, the `InstallReport`
> main-fatal/aux-best-effort contract, and the checksum/extraction/transport
> hardening live in
> [`src/cmd_self_update/download.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/download.rs)
> and [`src/cmd_self_update/update.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/update.rs).
> The producer side — packaging the full binary set into each platform tarball —
> lives in
> [`.github/workflows/release.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/release.yml).
> Closes [#2252](https://github.com/rysweet/Simard/issues/2252).

This reference specifies the behaviour, public API, configuration, security
model, and the producer/consumer contract for the multi-binary self-update
path. For the brain-orchestrated, drain → snapshot → pre-test → swap → validate
flow that wraps a single swap, see [Safe Self-Update](../safe-self-update.md).
For the build-from-source merged-but-not-running path, see the
[self-deploy API reference](./self-deploy-api.md).

## Contents

- [Why](#why)
- [The binary set](#the-binary-set)
- [`simard update` behaviour](#simard-update-behaviour)
- [API reference](#api-reference)
  - [`InstallReport`](#installreport)
  - [`find_all_binaries_in_dir`](#find_all_binaries_in_dir)
  - [`install_binary`](#install_binary)
  - [`install_binaries`](#install_binaries)
  - [`verify_sha256`](#verify_sha256)
  - [`download_to_temp` (shared primitive)](#download_to_temp-shared-primitive)
- [Failure semantics](#failure-semantics)
- [Security model](#security-model)
- [Release packaging contract](#release-packaging-contract)
- [Backward compatibility](#backward-compatibility)
- [Examples](#examples)
- [Configuration & environment](#configuration--environment)
- [Tests](#tests)
- [See also](#see-also)

## Why

Before this change, `simard update` downloaded a release tarball, found the
single file named `simard` inside it, and swapped **only** that file over the
running daemon binary. Every other shipped executable — most visibly
`simard-tui`, the operator's monitoring dashboard, but also `simard-gym` and
the `simard-*-step` / `simard-audit-*` helpers — was left at whatever version
the operator last installed by hand.

The result was a split-brain install: a freshly self-updated `simard` daemon
running next to a stale `simard-tui` that spoke an older wire format, or stale
`simard-*-step` helpers the daemon shells out to. The fix makes
`simard update` replace **the full set of executables present in the release
tarball**, so the whole installed surface advances together in one step.

There are two halves, and **both must ship together** or the feature is a
silent no-op:

1. **Producer** (`release.yml`): the tarball must actually *contain* the full
   binary set. A release that packages only `simard` gives the consumer nothing
   extra to install.
2. **Consumer** (`download.rs` / `update.rs`): the update path must discover and
   install every executable the tarball contains.

> **Landing note.** This reference documents symbols that do not exist on the
> default branch yet (`find_all_binaries_in_dir`, `install_binary`,
> `install_binaries`, `InstallReport`, `verify_sha256`, and
> `tests_download.rs`); the current tree still has the single-binary
> `find_binary_in_dir`. The doc must land in the **same PR** as the
> producer + consumer code so its API links and `status: implemented` are not
> dangling references.

## The binary set

The set is **discovered dynamically**, never hard-coded. The update path
installs *every executable extracted from the tarball*, whatever it is named.
This keeps the feature correct as the binary set evolves (for example, when a
new `simard-*` helper is added or removed) with no change to the self-update
code.

At the time of writing the producer packages these Cargo `bin` targets:

| Binary | Role | Replacement policy |
| --- | --- | --- |
| `simard` | Main daemon / operator CLI | **Main — fatal** |
| `simard-tui` | Terminal monitoring dashboard | Auxiliary — best-effort |
| `simard-gym` | Benchmark suite runner | Auxiliary — best-effort |
| `simard-ooda-step` | Single OODA-step helper | Auxiliary — best-effort |
| `simard-improve-step` | Improvement-step helper | Auxiliary — best-effort |
| `simard-engineer-step` | Engineer-step helper | Auxiliary — best-effort |
| `simard-self-improve-recipe` | Self-improve recipe driver | Auxiliary — best-effort |
| `simard-engineer-loop-recipe` | Engineer-loop recipe driver | Auxiliary — best-effort |
| `simard_operator_probe` | Operator self-probe helper (note: underscore-named) | Auxiliary — best-effort |

> **Feature-gated targets are NOT shipped.** Cargo `bin` targets carrying a
> non-default `required-features` (for example `simard-audit-pass01` and
> `simard-audit-dashboard`, gated behind the `dashboard-audit` feature) are
> **excluded** from the release tarball, because the release job builds with the
> default feature set and never compiles them. The producer filter drops any
> target whose `required-features` is non-empty (see
> [Release packaging contract](#release-packaging-contract)). To ship such a
> tool, the release build must first enable its feature.

The table is **illustrative, not authoritative**. The authoritative set is
whatever `cargo metadata` reports as `bin` targets *buildable with the release
feature set* at release time (see
[Release packaging contract](#release-packaging-contract)) and, on the consumer
side, whatever executables the downloaded tarball contains. `simard` is the only
name with special status: it is the **main** binary and its replacement is
fatal. Every other extracted executable is **auxiliary** and best-effort.

> **Design note — which targets ship.** The `cargo metadata` enumeration ships
> every `bin` target **whose `required-features` are satisfied by the release
> build**, including helper/probe binaries such as `simard_operator_probe` (and
> any future test-only or internal helper bins). Targets gated behind a
> non-default feature are filtered out (see the
> [Release packaging contract](#release-packaging-contract)). If some
> default-built targets should additionally not be distributed to operators, the
> producer `jq` filter must gain an explicit exclude list — discovery alone will
> ship them. Note also that target names are not uniformly hyphenated
> (`simard_operator_probe` uses underscores), so the consumer's basename match
> and de-duplication must be name-agnostic.

### Install location

Each binary is installed next to the running `simard` — i.e. into the parent
directory of `std::env::current_exe()`. Auxiliary binaries are matched by their
**basename only** and written into that same trusted directory (never to a path
derived from a tarball entry; see [Security model](#security-model)).

## `simard update` behaviour

```text
simard update

  Download the latest published release for this platform, verify its
  checksum, then replace the full set of installed Simard binaries:

    * simard (main) is swapped first; failure aborts the update with the
      previous binary left in place.
    * every auxiliary binary in the tarball is then installed best-effort;
      a single aux failure is logged and does not abort the update.

  After the swap, the new `simard` runs its own `self-test`. On success the
  process exec()s into the new `simard`. On failure the new binaries remain
  installed but the relaunch is skipped.
```

The command is unchanged on the surface — operators still run `simard update`.
What changed is the breadth of the swap and the order of operations:

1. **Resolve** the latest release for this platform (unchanged).
2. **Download** the platform tarball over https-only transport.
3. **Verify** the tarball against the published `.sha256` **before** extraction.
   A mismatch aborts and cleans up the temp directory.
4. **Authenticate** the tarball's cosign keyless signature against this repo's
   pinned release-workflow identity **before** extraction (defense-in-depth
   beyond the same-origin checksum). A present-but-invalid signature aborts; an
   absent signature or a host without `cosign` warns and continues. See
   [R8](#security-model).
5. **Extract** into a private, exclusively-created temp directory with zip-slip
   defenses and `--no-same-owner --no-same-permissions`.
6. **Discover** every executable in the extracted tree.
7. **Install** the main binary (fatal on error), then each auxiliary binary
   (best-effort).
8. **Confirm** each installed binary is present on disk — a post-install
   existence check over exactly what discovery installed; there is no external
   "expected" manifest, since the set is dynamic — then print the
   `InstallReport`.
8. **Self-test** the new `simard` (main binary only), then `exec()` into it via
   `self_relaunch::handover` — both unchanged.

## API reference

All items live in `src/cmd_self_update/download.rs` unless noted. They are
`pub(crate)` — this is an internal contract consumed by `update.rs`, not a
public library surface — and are documented here as the implementation spec.

### `InstallReport`

The result of installing the full binary set.

```rust
/// Outcome of installing the full binary set from an extracted tarball.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InstallReport {
    /// `true` once the MAIN binary (`simard`) is installed. `false` means the
    /// update aborted: the caller MUST NOT relaunch.
    pub main_installed: bool,
    /// Basenames of auxiliary binaries that were installed successfully.
    pub aux_installed: Vec<String>,
    /// Auxiliary binaries that failed to install, as `(basename, reason)`.
    /// Logged and surfaced to the operator; never aborts the update.
    pub aux_failed: Vec<(String, String)>,
}
```

`main_installed == false` is only ever returned together with an `Err` from
[`install_binaries`](#install_binaries) — the main binary is fatal, so a `false`
here always means "abort, do not relaunch". `aux_failed` being non-empty is a
**successful** update with a warning.

### `find_all_binaries_in_dir`

Replaces the old single-binary `find_binary_in_dir`. Discovers **all**
executables in the extracted tree.

```rust
/// Discover every executable file in an extracted tarball tree (max depth 3).
///
/// On Unix, "executable" means a regular file with any execute bit set. The
/// returned paths are de-duplicated by basename (first match by directory walk
/// order wins) so a tarball can never ask to install two different files to the
/// same destination name. The main binary `simard` is guaranteed to sort first
/// in the returned vec when present.
///
/// Returns an error only if the tree contains no `simard` binary at all.
pub(crate) fn find_all_binaries_in_dir(
    dir: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>>;
```

Notes:

- Depth is capped at 3, matching the previous `find_binary_in_dir` behaviour.
- Directories named `simard` are ignored — only regular files count, exactly as
  before.
- A tree with auxiliary binaries but **no** `simard` is an error (a tarball that
  cannot update the daemon is rejected outright).

### `install_binary`

Atomically install one binary into the trusted install directory.

```rust
/// Install a single binary `src` to `dest`. Sequence:
///   1. If `dest` exists, move it aside to `dest.old` (best-effort cleanup of a
///      stale `.old` first).
///   2. `rename(src, dest)` — O(1) on the same filesystem.
///   3. On `EXDEV` (cross-device), fall back to `copy(src, dest)`.
///   4. On Unix, `chmod 0o755` the installed file.
///   5. On success, remove the `dest.old` backup.
/// On failure after step 1, the `.old` backup is restored over `dest`.
pub(crate) fn install_binary(
    src: &Path,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error>>;
```

This is the existing main-binary swap logic, factored out so it can be reused
per binary. The `0o755` chmod is applied **only** to binaries chosen by
discovery — never to arbitrary extracted files.

### `install_binaries`

Orchestrate the full-set install with the main-fatal / aux-best-effort policy.

```rust
/// Install every discovered binary into `install_dir`.
///
/// - The binary whose basename is `simard` is installed FIRST and is FATAL:
///   any error returns `Err` with an `InstallReport { main_installed: false, .. }`
///   and the caller must abort (no relaunch).
/// - Every other binary is installed BEST-EFFORT: a failure is recorded in
///   `aux_failed` and the loop continues.
///
/// Returns `Ok(InstallReport)` once the main binary is installed, regardless of
/// auxiliary outcomes.
pub(crate) fn install_binaries(
    binaries: &[PathBuf],
    install_dir: &Path,
) -> Result<InstallReport, Box<dyn std::error::Error>>;
```

### `verify_sha256`

Checksum gate run **before** extraction.

```rust
/// Verify a downloaded tarball against its published `<asset>.sha256`.
///
/// Fetches the sidecar checksum for `asset_url`, computes the SHA-256 of
/// `archive_path`, and compares. On mismatch (or a missing/unreadable sidecar)
/// returns an error WITHOUT extracting, and the caller cleans up the temp dir.
pub(crate) fn verify_sha256(
    archive_path: &Path,
    asset_url: &str,
) -> Result<(), Box<dyn std::error::Error>>;
```

The release workflow already publishes `<asset>.tar.gz.sha256` next to every
tarball (see [Release packaging contract](#release-packaging-contract)); this
function closes the gap where the consumer never downloaded **or** checked it.
`verify_sha256` resolves the sidecar by appending `.sha256` to `asset_url`,
fetches it over the same https-only transport, and compares before extraction.
Because the check lives inside the shared `download_to_temp`, the
`safe-update` path inherits it too (see [Backward compatibility](#backward-compatibility)).

### `download_to_temp` (shared primitive)

`download_to_temp` is shared by **both** consumers of the self-update download
path and its signature must **not** change:

- `download_and_replace` — the `simard update` path.
- `handle_self_update_download_only` — the `simard safe-update` path
  (`src/cmd_self_update/update.rs`).

It still returns a single `PathBuf` to the **main** `simard` candidate. This is
a hard constraint: `safe-update`'s `SafeUpdateOrchestrator::new(cfg, candidate,
install)` consumes exactly one candidate path, so the multi-binary work must not
change what `download_to_temp` returns. The responsibilities split as:

- `download_to_temp` — download → `verify_sha256` → extract, returning the main
  `simard` candidate and leaving the rest of the extracted tree in its private
  temp dir.
- `download_and_replace` — calls `download_to_temp`, then runs
  [`find_all_binaries_in_dir`](#find_all_binaries_in_dir) over the extracted tree
  and [`install_binaries`](#install_binaries) for the full set. **Only this path
  advances auxiliary binaries.**

Consequently `safe-update` inherits the download/extraction/transport hardening
(R1–R3) for free but, by construction, still swaps only the main binary — see
[Backward compatibility](#backward-compatibility).

## Failure semantics

| Situation | Behaviour |
| --- | --- |
| Checksum mismatch / missing `.sha256` | **Abort before extraction.** Temp dir removed. Install untouched. |
| Tarball contains no `simard` | **Abort.** `find_all_binaries_in_dir` errors. Install untouched. |
| Main `simard` swap fails (e.g. permission denied) | **Fatal.** `Err` returned, `main_installed: false`, no relaunch. Previous binaries remain. Operator is told to retry with `sudo`. |
| An auxiliary binary swap fails | **Non-fatal.** Recorded in `aux_failed`, logged, update continues and relaunches. |
| Tarball is an **old single-`simard`** archive | Main installs; `aux_installed` is empty; **not** an error (aux-missing is expected for old releases). |
| New `simard` `self-test` fails after install | New binaries remain installed; relaunch skipped; non-zero exit (unchanged behaviour). |

The guiding rule: **the core update must never be blocked by an auxiliary
binary.** A missing or unwritable `simard-tui` degrades the dashboard, not the
daemon.

## Security model

The multi-binary discovery widens the attack surface (more files written from a
downloaded archive), so the path is hardened on several axes. All apply to the
main binary too — they are not aux-only.

| # | Control | What it prevents |
| --- | --- | --- |
| R1 | **SHA-256 verification before extraction.** Compute the digest of the downloaded tarball, compare to the published `.sha256`, abort + clean up on mismatch. | Unauthenticated / tampered code execution. Previously the producer published the `.sha256` sidecar but the consumer never fetched **or** verified it (the self-update code had no checksum logic at all). |
| R2 | **Zip-slip defense.** Install by **basename only** into the trusted install dir; canonicalize within the install root; reject any entry that is an absolute path, contains `..`, or is a symlink. | A malicious archive writing outside the install directory or following a symlink to overwrite an arbitrary file. |
| R3 | **https-only transport.** Enforce an `https://` URL and, because the download follows redirects (`curl -L`), pass `curl --proto =https --proto-redir =https --tlsv1.2` so a redirect can never downgrade to `http://`. | Protocol downgrade and redirect-to-http MITM. |
| R4 | **Scoped `chmod` + no archive-supplied perms.** `0o755` is applied only to executables chosen by discovery; extraction passes `tar --no-same-owner --no-same-permissions` so the archive can never restore its own ownership/permission bits. | Granting execute to attacker-planted data files; setuid/world-writable bits smuggled in via the tarball. |
| R5 | **Strict asset matching.** Keep the exact platform-suffix + `.tar.gz` asset match when selecting the release asset. | Asset spoofing via look-alike asset names. |
| R6 | **SHA-pinned Actions.** The release workflow keeps every GitHub Action pinned by commit SHA. | Supply-chain compromise of a tag-mutable action. |
| R7 | **Private, exclusively-created temp dir.** The download/extract directory uses an unpredictable random suffix and is created with `create_dir` (fails if the path already exists) at mode `0700`, replacing the predictable `simard-update-<pid>` name created with `create_dir_all`. | A local attacker on a shared host pre-creating or symlinking the temp path to redirect the `curl -o` write or smuggle a rogue executable into discovery. |
| R8 | **cosign keyless signature verification.** After R1, fetch the `.sig`/`.pem` sidecars and run `cosign verify-blob` pinning the certificate identity to this repo's `release.yml` workflow on `main` and the GitHub OIDC issuer. A present-but-invalid signature **aborts**; when `cosign` or the sidecars are absent the update warns and continues (the R1 checksum still applies). | A compromised release host swapping **both** the tarball and its same-origin `.sha256`: the attacker cannot forge a Fulcio certificate for this repo's workflow identity. |

No secrets ever appear in logs or issue comments. The "try `sudo`, fail cleanly"
behaviour is preserved — the update path never auto-escalates; it installs into
a private temp dir and renames into place, cleaning up on every exit path.

## Release packaging contract

The producer side (`.github/workflows/release.yml`) packages the **full binary
set** into each platform tarball, instead of only `simard`. Without this, the
consumer refactor installs nothing extra.

The "Package binary" step enumerates Cargo `bin` targets dynamically rather than
naming a literal `simard`. It also filters out targets gated behind a non-default
`required-features` (such as the `dashboard-audit` audit tools), because the
release build uses the default feature set and never compiles them — packaging a
never-built target would make `tar` fail:

```bash
set -euo pipefail
# Enumerate the [[bin]] target names that the default release build produces.
mapfile -t BIN_TARGETS < <(
  cargo metadata --no-deps --format-version 1 \
    | jq -r '
        .packages[].targets[]
        | select(.kind[] == "bin")
        | select((."required-features" // []) | length == 0)
        | .name
      '
)

cd target/release
# Fail loudly if the build did not produce an expected binary.
for bin in "${BIN_TARGETS[@]}"; do
  [ -f "$bin" ] || { echo "::error::missing built binary: $bin"; exit 1; }
done
# Tar exactly the built binaries that exist for this platform.
TARBALL="simard-${PLATFORM_SUFFIX}.tar.gz"
tar czf "$TARBALL" "${BIN_TARGETS[@]}"
sha256sum "$TARBALL" > "$TARBALL.sha256"
```

> **Scope note.** The current `release.yml` builds a **single** `linux-x86_64`
> job with no platform matrix, and `PLATFORM_SUFFIX` is shown generically above
> for forward-compatibility. The only change [#2252](https://github.com/rysweet/Simard/issues/2252)
> makes to the producer is replacing the literal `tar czf … simard` with the
> enumerated `"${BIN_TARGETS[@]}"`; adding the other platform suffixes listed
> below (`linux-aarch64`, `macos-*`, `windows-x86_64`) is a separate,
> out-of-scope change even though `platform_suffix()` already resolves them on
> the consumer side.

Contract guarantees:

- The tarball contains **every** `bin` target buildable with the release
  feature set (targets gated behind a non-default `required-features` are
  excluded), with `simard` always present.
- A `<asset>.tar.gz.sha256` sidecar is published next to every tarball (already
  true; the consumer now verifies it — see [R1](#security-model)).
- The platform suffix (`linux-x86_64`, `linux-aarch64`, `macos-x86_64`,
  `macos-aarch64`, `windows-x86_64`) is unchanged, so existing
  [`platform_suffix()`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/platform.rs)
  asset matching keeps working.

## Backward compatibility

- **Old tarballs that contain only `simard`** still update cleanly: the main
  binary installs and `aux_installed` is empty. Aux-missing is non-fatal by
  design.
- **Operators on a host with a hand-installed `simard-tui`** get it replaced
  automatically on the next `simard update`, ending the split-brain state.
- **`simard safe-update`** (the brain-orchestrated path) shares
  `download_to_temp`, so it inherits the **download/extraction hardening**
  (R1 checksum gate, R2 zip-slip defense, R3 https-only transport) for free. It
  does **not** advance the full binary set: `SafeUpdateOrchestrator` operates on
  a single candidate binary and a single install path, and extending it to the
  multi-binary set is **out of scope for [#2252](https://github.com/rysweet/Simard/issues/2252)**
  (future work). Its drain/snapshot/pre-test/validate/rollback envelope is
  unchanged. See [Safe Self-Update](../safe-self-update.md).
- The `simard update` CLI flags and exit codes are unchanged.

## Examples

### Operator: update the whole install in one step

```console
$ simard update
simard self-update (current: v0.42.0)
New version available: v0.42.0 → v0.43.0
Downloading simard v0.43.0...
Verifying checksum... ok
Extracting...
Replacing binaries...
  simard               installed (main)
  simard-tui           installed
  simard-gym           installed
  simard-ooda-step     installed
  simard-audit-pass01  installed
  …                    (remaining auxiliary binaries)
Updated 9 binaries: v0.42.0 → v0.43.0
Running self-test on new binary...
Self-test passed.
Relaunching into v0.43.0...
```

### Auxiliary failure is non-fatal

If `simard-tui` cannot be written (for example it is open and locked on
Windows), the update still completes:

```console
$ simard update
...
Replacing binaries...
  simard               installed (main)
  simard-tui           SKIPPED: Failed to install new binary: Permission denied
  simard-gym           installed
  …                    (remaining auxiliary binaries)
Updated 8 binaries (1 skipped): v0.42.0 → v0.43.0
WARNING: 1 auxiliary binary was not updated: simard-tui
Run 'simard update' again after closing it, or reinstall it manually.
Running self-test on new binary...
Self-test passed.
Relaunching into v0.43.0...
```

### Checksum mismatch aborts before extraction

```console
$ simard update
simard self-update (current: v0.42.0)
New version available: v0.42.0 → v0.43.0
Downloading simard v0.43.0...
Verifying checksum... MISMATCH
error: downloaded archive failed SHA-256 verification; aborting update
       (expected 9f86d0…, got 2c26b4…). No binaries were changed.
```

### Updating from an old single-binary release

```console
$ simard update
...
Extracting...
Replacing binaries...
  simard  installed (main)
Updated 1 binary: v0.41.0 → v0.42.0
Running self-test on new binary...
Self-test passed.
Relaunching into v0.42.0...
```

## Configuration & environment

The multi-binary path adds **no** new configuration knobs and **no** new
environment variables. It inherits the existing self-update environment:

- Platform/asset resolution: [`platform.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/platform.rs)
  (`GITHUB_REPO`, `platform_suffix()`).
- `CURRENT_VERSION` from `env!("CARGO_PKG_VERSION")`.
- The brain-triggering doctrine and `UpdateConfig` knobs that govern *when*
  `safe-update` fires are documented in
  [Safe Self-Update → Configuration](../safe-self-update.md#configuration) and
  the [self-deploy API reference](./self-deploy-api.md#updateconfig-self-deploy-fields).

## Tests

Unit tests live in
[`src/cmd_self_update/tests_download.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/tests_download.rs)
and run hermetically (no network, no live release):

- **Discovery**: `find_all_binaries_in_dir` finds the main binary plus multiple
  auxiliary binaries at root, nested, and at the depth-3 boundary; still errors
  when no `simard` is present; ignores directories named `simard`;
  de-duplicates by basename.
- **Install**: `install_binary` covers the same-filesystem `rename` path, the
  cross-device `copy` fallback, the `.old` backup/restore, and the `0o755`
  permission set (Unix).
- **Report paths**: happy path (main + all aux installed); missing-aux =
  no-error (`aux_installed` partial, no `aux_failed`); missing-main = fatal
  (`Err`, `main_installed: false`); an aux failure is logged into `aux_failed`
  without aborting.
- **Security**: checksum mismatch aborts before extraction; a zip-slip entry
  (absolute path, `..`, or symlink) is rejected; a non-https URL is refused.
- **Shared-primitive regression**: `download_to_temp` still returns a single
  `PathBuf` to the main `simard` candidate, preserving the `safe-update`
  single-candidate contract.

Everything passes `cargo test --all-features --locked`, `cargo clippy
--all-targets --all-features --locked -- -D warnings`, and `cargo fmt --all --
--check` under the repository's pre-commit/pre-push hooks and `verify.yml` CI.

## See also

- [Safe Self-Update](../safe-self-update.md) — the drain → snapshot → pre-test →
  swap → validate → rollback envelope around a swap.
- [Self-deploy API reference](./self-deploy-api.md) — the build-from-source,
  merged-but-not-running deploy path.
- [Update-check reference](./update-check.md) — how Simard notices a newer
  release is available.
- [How to check for updates](../howto/check-for-updates.md) — operator runbook.
