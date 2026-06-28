---
title: Release integrity — SBOM, signing, and reproducibility
description: "Reference for the CycloneDX SBOM, cosign keyless signing, and build-reproducibility guarantees attached to every Simard release, plus end-to-end verification steps."
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: active
related:
  - ./supply-chain-audit.md
  - ./dependency-trust-policy.md
  - ../safe-self-update.md
  - ./update-check.md
---

# Release integrity — SBOM, signing, and reproducibility

> **Status: active.** This page documents the shipped release-integrity flow
> for issue #2261: every Simard release carries a CycloneDX SBOM and a cosign
> keyless signature, and its reproducibility characteristics are documented and
> checkable. It is both the user-facing verification guide and the spec the
> extended `release.yml` job satisfies.

When you download a Simard release binary you need to answer three questions:
*what is in it* (SBOM), *did it really come from this repository's release
pipeline* (signature), and *can I rebuild it from source and get the same bytes*
(reproducibility). The release workflow attaches the artifacts that answer all
three.

## Release artifacts

Each GitHub Release publishes the following, for every platform target:

| Artifact | Produced by | Purpose |
| --- | --- | --- |
| `simard-<platform>.tar.gz` | `cargo build --release` + `tar` | The binary tarball. |
| `simard-<platform>.tar.gz.sha256` | `sha256sum` | Integrity checksum (existing). |
| `simard-<version>.cdx.json` | `cargo cyclonedx` | **CycloneDX SBOM** — full dependency inventory. |
| `simard-<platform>.tar.gz.sig` | `cosign sign-blob` | **Detached cosign signature** over the tarball. |
| `simard-<platform>.tar.gz.pem` | `cosign sign-blob` | The signing **certificate** (Fulcio-issued, carries the OIDC identity). |

`<platform>` follows the existing naming convention (`linux-x86_64`, etc.;
see [the update-check platform table](./update-check.md#platform-asset-detection)).
`<version>` is the `Cargo.toml` version (e.g. `0.22.0`).

## Software Bill of Materials (SBOM)

The SBOM is generated with
[`cargo-cyclonedx`](https://github.com/CycloneDX/cyclonedx-rust-cargo) in the
standard [CycloneDX](https://cyclonedx.org/) JSON format, directly from the
locked dependency graph:

```bash
cargo cyclonedx --format json --override-filename "simard-<version>.cdx"
```

`cargo-cyclonedx` has **no `--locked` flag**; it sources the graph from
`cargo metadata` over the **committed `Cargo.lock`** (regenerating the lock only
if it is missing), so in CI — where the lock is committed — the SBOM reflects
the locked graph. `--override-filename` sets the **base** name and the
`--format json` extension is appended, producing exactly
`simard-<version>.cdx.json`. Because Simard is a single-package crate (not a
multi-member workspace), this emits exactly **one** SBOM file.

Properties:

- **Reflects the committed `Cargo.lock`,** so it lists the exact resolved
  versions — including the four exact-rev git dependencies — that went into the
  binary. The release job runs it on the checked-out tag, whose lock is the one
  the binary was built from.
- **Fail-closed.** The release job validates that the expected
  `simard-<version>.cdx.json` file was produced and is non-empty, well-formed
  JSON with a non-empty `.components` array before attaching it; a missing,
  empty, or malformed SBOM fails the release rather than publishing a binary
  with no bill of materials.
- **No sensitive paths.** The SBOM is reviewed to contain only public crate
  coordinates (name, version, source) — no local filesystem paths, usernames,
  hostnames, or internal URLs.

To inspect an SBOM you downloaded:

```bash
# Top-level component + count of dependencies.
jq '{component: .metadata.component.name, deps: (.components | length)}' \
  simard-0.22.0.cdx.json

# List every component name@version.
jq -r '.components[] | "\(.name)@\(.version)"' simard-0.22.0.cdx.json
```

## Signature verification (cosign keyless)

Release binaries are signed with [cosign](https://docs.sigstore.dev/) in
**keyless** mode: there is **no long-lived private key** to store or leak.
Instead, the release job obtains a short-lived certificate from Sigstore's
Fulcio CA, bound to the GitHub Actions OIDC identity of the release workflow,
and the signature is recorded in the public Rekor transparency log.

To verify a downloaded tarball:

```bash
# Install cosign: https://docs.sigstore.dev/cosign/system_config/installation/

cosign verify-blob \
  --certificate        simard-linux-x86_64.tar.gz.pem \
  --signature          simard-linux-x86_64.tar.gz.sig \
  --certificate-identity-regexp \
      'https://github.com/rysweet/Simard/\.github/workflows/release\.yml@refs/heads/main' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  simard-linux-x86_64.tar.gz
```

A successful run prints `Verified OK`. The two `--certificate-*` flags are the
security-critical part — **always pin both**:

- `--certificate-identity-regexp` ties the signature to the **release workflow
  in this repository**. The `release` workflow runs on **push to `main`** (it
  creates the version tag as a release step), so the Fulcio identity ends in
  `release.yml@refs/heads/main` — match that ref, **not** `refs/tags/v*`.
  Without this flag, any valid Sigstore certificate would pass.
- `--certificate-oidc-issuer` requires the identity to come from
  **GitHub Actions' OIDC issuer** (`token.actions.githubusercontent.com`).

> **Verify, then trust.** The `.sha256` checksum proves the file was not
> corrupted in transit; the cosign signature proves it was *produced by this
> repository's release pipeline*. Both together are what
> [Safe Self-Update](../safe-self-update.md) and manual installers should
> check before swapping a binary.

After verifying the signature, confirm the checksum as before:

```bash
sha256sum -c simard-linux-x86_64.tar.gz.sha256
```

## Release workflow permissions

Signing requires the workflow to request an OIDC token. The current
[`release.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/release.yml)
declares `contents: write` at the **workflow** level. Adding keyless signing
**changes** this — and the change *narrows* scope rather than just adding to it:

1. **Relocate `contents: write` from the workflow root onto the `release` job.**
   Today every job in the workflow inherits write scope; after the change only
   the job that creates the Release and uploads assets has it.
2. **Add `id-token: write` to that same job** — the one genuinely new
   permission, required for the Fulcio OIDC token. Keyless signing introduces
   **no new secrets or PATs**.

```yaml
# The workflow-level `permissions:` block is removed; scope moves onto the job.
jobs:
  release:
    permissions:
      contents: write   # relocated from workflow level — create Release + upload assets
      id-token: write   # NEW — request the Fulcio OIDC token for keyless signing
```

- `id-token: write` is the only new scope; `contents: write` is **moved**, not
  added.
- The guardrail jobs (`cargo-deny`, `cargo-vet`, `cargo-audit`) keep
  `contents: read` and gain no token write scope.

## Build reproducibility

A reproducible build lets a third party rebuild the published binary from
source and obtain **the same bytes**. Simard's builds are reproducible **at a
fixed commit, with a fixed toolchain**, subject to documented caveats.

### What is deterministic

- **The dependency graph** is fully pinned: crates.io deps use exact `=`
  versions and git deps use exact revs, all captured in `Cargo.lock`. `--locked`
  everywhere forbids implicit resolution.
- **The release profile** is fixed (`lto = "thin"`, `codegen-units = 1`,
  `strip = "symbols"`, `incremental = false`), removing the main sources of
  build-to-build variation.

### Documented caveats

| Caveat | Why | Mitigation |
| --- | --- | --- |
| `build.rs` embeds `SIMARD_GIT_HASH` / `SIMARD_BUILD_NUMBER` | Determinism is **per-commit**: a different `HEAD` yields a different embedded hash. | Reproduce at the **same commit** (the release tag). The audited [own `build.rs`](./supply-chain-audit.md#simards-own-buildrs) does nothing else non-deterministic. |
| Embedded absolute paths in debug info / panic messages | Build paths can leak the builder's `$HOME`/`$CWD`. | Set `RUSTFLAGS="--remap-path-prefix=$PWD=/build"`; the release profile already `strip`s symbols. |
| Build timestamps | Some tooling embeds the current time. | Export `SOURCE_DATE_EPOCH` (e.g. the tag's commit time) before building. |
| Host C toolchain | The vendored-C sys crates (see [the build-script inventory](./supply-chain-audit.md#high-attention-native-c-c-assembly-compilation)) compile with the local compiler. | Reproduce with the **same** `cc`/`cmake` toolchain version. |

### Reproducing a release

```bash
# 1. Check out the exact release tag.
git checkout v0.22.0

# 2. Pin the build environment for determinism.
export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
export RUSTFLAGS="--remap-path-prefix=$PWD=/build"

# 3. Build with the locked graph.
cargo build --release --locked

# 4. Package identically and compare the hash to the published .sha256.
cd target/release
tar czf simard-linux-x86_64.tar.gz simard
sha256sum simard-linux-x86_64.tar.gz
```

Because `tar`/gzip metadata (ordering, mtime, compression level) can affect the
archive hash, the **stable comparison target is the binary's own hash**
(`sha256sum target/release/simard`) at a fixed commit and toolchain; the
tarball hash is reproducible additionally when the packaging environment
matches. The published `.sha256` covers the tarball; the SBOM + signature cover
provenance regardless of archive-level packaging differences.

## See also

- [Supply-chain audit and guardrails](./supply-chain-audit.md) — the
  build-time attack-surface audit and `cargo-deny` policy.
- [Dependency trust policy](./dependency-trust-policy.md) — `cargo-vet`
  certification and advisory resolution.
- [Safe Self-Update](../safe-self-update.md) — the binary-swap flow that should
  verify the signature before replacing a running binary.
- [Automatic update check](./update-check.md) — how Simard discovers a newer
  release.
- [Security policy](https://github.com/rysweet/Simard/blob/main/SECURITY.md) —
  vulnerability reporting and supported versions.
