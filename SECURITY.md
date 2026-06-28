# Security Policy

Simard is an autonomous engineer that drives agentic coding systems. Because it
builds, signs, and self-deploys software, supply-chain integrity is a
first-class concern. This document describes how to report a vulnerability and
summarizes the guardrails that protect Simard's build and release pipeline.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately via GitHub's
[private vulnerability reporting](https://github.com/rysweet/Simard/security/advisories/new)
("Report a vulnerability" on the repository's **Security** tab). Include:

- A description of the issue and its impact.
- Steps to reproduce (or a proof-of-concept), where applicable.
- The affected version (`simard --version`) and platform.

We aim to acknowledge a report within a few business days, agree on a
disclosure timeline, and credit reporters who wish to be named once a fix has
shipped.

## Supported versions

Simard ships from `main` and releases frequently. Security fixes land on the
latest release; older tagged releases do not receive backports. Always run the
[latest release](https://github.com/rysweet/Simard/releases/latest) — the
binary performs a non-blocking
[update check](docs/reference/update-check.md) on launch and can update itself
via `simard self-update`.

| Version | Supported |
| --- | --- |
| Latest release | ✅ |
| Older tagged releases | ❌ (upgrade to latest) |

## Supply-chain guardrails

Simard's dependency, build, and release pipeline is hardened by several
CI-enforced guardrails. Full reference documentation:

- **[Supply-chain audit & guardrails](docs/reference/supply-chain-audit.md)** —
  `deny.toml` policy (advisories, licenses, bans, sources), the `cargo-deny` CI
  gate, and a standing audit of every transitive crate that runs code at build
  time (`build.rs` scripts and proc-macros).
- **[Dependency trust policy](docs/reference/dependency-trust-policy.md)** —
  `cargo-vet` certification of transitive dependencies, trusted-crate and
  exemption criteria, and the advisory-resolution workflow.
- **[Release integrity](docs/reference/release-integrity.md)** — CycloneDX SBOM
  generation, cosign **keyless** signing of release binaries, and build
  reproducibility.

These run as **separate, lockfile-only CI jobs** (`cargo-audit`, `cargo-deny`,
`cargo-vet`) that never compile the crate and never gain token write scope.

### Advisory handling

We track RUSTSEC advisories via `cargo audit` and `cargo deny check advisories`.
The standing policy is **no remaining unmitigated advisories**. A *vulnerability*
always fails the check and is mitigated only by a fix (update to a patched
version) or an explicit, justified, tracked exemption — currently a single one,
`rsa` / RUSTSEC-2023-0071, which has no upstream fix. *Unmaintained* and
*unsound* advisories that reach the graph only transitively are surfaced but
non-failing under the cargo-deny `workspace` scope, and tracked for an upstream
bump rather than exempted per-ID. Exemptions are recorded — with their
justification and an upstream tracking link — in `.cargo/audit.toml` and
`deny.toml`. See
[advisory resolution](docs/reference/dependency-trust-policy.md#advisory-resolution-policy).

## Verifying a release

Every release is signed with cosign (keyless) and ships a CycloneDX SBOM.
Before trusting a downloaded binary, verify both the checksum and the
signature:

```bash
cosign verify-blob \
  --certificate        simard-linux-x86_64.tar.gz.pem \
  --signature          simard-linux-x86_64.tar.gz.sig \
  --certificate-identity-regexp \
      'https://github.com/rysweet/Simard/\.github/workflows/release\.yml@refs/heads/main' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  simard-linux-x86_64.tar.gz

sha256sum -c simard-linux-x86_64.tar.gz.sha256
```

Full instructions, including SBOM inspection and build reproduction, are in
[Release integrity](docs/reference/release-integrity.md).
