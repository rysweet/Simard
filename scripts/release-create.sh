#!/usr/bin/env bash
# release-create.sh
#
# Idempotent GitHub Release publisher for Simard.
#
# Root cause (recurring red `release` workflow, previously misattributed to the
# deploy-gate canary unit tests): the release workflow's `tag_check` step reads a
# LOCAL `git rev-parse` snapshot taken ~11 min earlier — before the release build
# even starts. When the same (unbumped) version is pushed to main more than once,
# concurrent `release` runs race: the first run publishes v$VERSION, and every
# other run then reaches the create step with a stale `exists=false` and dies on
#
#     gh release create ... a release with the same tag name already exists: vX.Y.Z
#
# reddening an otherwise fully-green build + sign pipeline. That is exactly the
# failure observed on the `#4505` de-flake push (run 30048895934): all build,
# test, SBOM, and cosign steps passed; only "Create release" failed, because the
# release had already been published by the immediately-preceding push at the
# same 0.37.0 version.
#
# This script closes that TOCTOU window by re-checking the AUTHORITATIVE remote
# release state immediately before creating, and treats an already-published
# release as an idempotent SUCCESS. A create/create race between the probe and
# the create is likewise tolerated. Any GENUINE `gh` failure still fails loudly
# (fail-closed), so a real publish error never masquerades as success.
#
# Usage:
#   TAG=vX.Y.Z VERSION=X.Y.Z GH_TOKEN=... [GITHUB_REPOSITORY=owner/repo] \
#     release-create.sh <asset> [<asset> ...]
#
# Regression gate: scripts/qa-release-create-idempotent.sh
set -uo pipefail

: "${TAG:?TAG (vX.Y.Z) is required}"
: "${VERSION:?VERSION (X.Y.Z) is required}"
REPO="${GITHUB_REPOSITORY:-rysweet/Simard}"

if [ "$#" -eq 0 ]; then
  echo "::error::release-create.sh: no release assets provided" >&2
  exit 2
fi

# Authoritative, fresh existence check — closes the stale local-tag TOCTOU race.
# `gh release view` queries the remote at create time, not an 11-minute-old
# local snapshot, so a release published by a concurrent run is seen here.
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "::notice::Release $TAG already exists; skipping create (idempotent no-op)."
  exit 0
fi

notes="## Install

\`\`\`bash
curl -L https://github.com/rysweet/Simard/releases/download/${TAG}/simard-linux-x86_64.tar.gz | tar xz
\`\`\`

Or with cargo:
\`\`\`bash
cargo install --git https://github.com/rysweet/Simard.git --tag ${TAG}
\`\`\`

## Verify (cosign keyless + SBOM)

This release is signed with cosign (keyless) and ships a CycloneDX SBOM
that is **also** signed with the same identity.
See [Release integrity](https://github.com/rysweet/Simard/blob/main/docs/reference/release-integrity.md)
for full verification steps.

\`\`\`bash
# Verify the binary tarball:
cosign verify-blob \\
  --certificate simard-linux-x86_64.tar.gz.pem \\
  --signature   simard-linux-x86_64.tar.gz.sig \\
  --certificate-identity-regexp '^https://github\.com/rysweet/Simard/\.github/workflows/release\.yml@refs/heads/main\$' \\
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \\
  simard-linux-x86_64.tar.gz

# Verify the SBOM the same way (proves the dependency inventory is untampered):
cosign verify-blob \\
  --certificate simard-${VERSION}.cdx.json.pem \\
  --signature   simard-${VERSION}.cdx.json.sig \\
  --certificate-identity-regexp '^https://github\.com/rysweet/Simard/\.github/workflows/release\.yml@refs/heads/main\$' \\
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \\
  simard-${VERSION}.cdx.json
\`\`\`"

create_log="$(mktemp)"
gh release create "$TAG" \
  --repo "$REPO" \
  --title "Simard $TAG" \
  --notes "$notes" \
  "$@" 2>&1 | tee "$create_log"
rc=${PIPESTATUS[0]}

if [ "$rc" -eq 0 ]; then
  exit 0
fi

# Tolerate a create/create race between the `gh release view` probe above and
# this create: a concurrent run may have published $TAG in the interim. That is
# the same benign already-exists condition, so treat it as idempotent success.
if grep -qi "already exists" "$create_log"; then
  echo "::notice::Release $TAG was published concurrently; treating as idempotent success."
  exit 0
fi

echo "::error::gh release create failed for $TAG (exit $rc)" >&2
exit "$rc"
