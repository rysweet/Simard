#!/usr/bin/env bash
#
# provision-lbug-prebuilt.sh — ensure a stable prebuilt `liblbug.a` (+ headers)
# exists in a given directory, outside the cargo registry-source tree.
#
# Usage:
#   scripts/provision-lbug-prebuilt.sh [lib_dir]
#
# Prints the resolved library directory on stdout. Diagnostics go to stderr.
#
# This is the single source of truth for the lbug native-static-lib link path
# used by the pre-commit clippy wrapper (scripts/clippy-precommit-release.sh)
# and by CI (.github/workflows/verify.yml). See issue #2426 / #2423: lbug
# 0.17.1 caches its prebuilt archive inside `~/.cargo/registry/src/.../
# lbug-0.17.1/.cache/lbug-prebuilt/...`, which CI's cargo cache evicts while
# keeping the cached build-script output that references it — so release builds
# fail with "could not find native static library `lbug`". Pointing lbug at a
# stable copy via `LBUG_LIBRARY_DIR`/`LBUG_INCLUDE_DIR` makes the link path
# deterministic.

set -euo pipefail

log() { printf '[provision-lbug] %s\n' "$*" >&2; }
die() {
  log "ERROR: $*"
  exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"

# Trusted, code-reviewed content-integrity anchor for the downloaded prebuilt
# archive (issue #2471). TLS + version/repo pinning authenticate the transport
# and the URL, but not the *content*; this manifest is compared in-repo so a
# tampered-at-rest release asset is caught before it is ever extracted/linked.
# Overridable for tests via LBUG_CHECKSUM_MANIFEST.
CHECKSUM_MANIFEST="${LBUG_CHECKSUM_MANIFEST:-$SCRIPT_DIR/lbug-prebuilt.sha256}"

LIB_DIR="${1:-${SIMARD_LBUG_LIB_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/simard-lbug-precommit/lib}}"
LIB_FILE="$LIB_DIR/liblbug.a"
static_lib_name="liblbug.a"

# lbug crate version (== matching LadybugDB native release tag), resolved from
# Cargo.lock so the prebuilt asset is fetched deterministically — no
# unauthenticated `releases/latest` API call and no version skew with the crate
# we actually compile against. Cargo.lock records the *resolved* version whether
# lbug is a semver-pinned crates.io dep (`lbug = "=0.17.1"`) or a git/fork
# dependency (`lbug = { git = "…ladybug-rust", rev = "…" }`, issue #3119) that
# carries no version string in Cargo.toml. Falls back to a semver-pinned
# Cargo.toml line for robustness.
lbug_version() {
  local v=""
  if [ -f "$REPO_ROOT/Cargo.lock" ]; then
    v="$(awk '
      $0 == "[[package]]" { inpkg = 1; islbug = 0; next }
      inpkg && $0 == "name = \"lbug\"" { islbug = 1 }
      inpkg && islbug && /^version = / {
        line = $0; sub(/^version = "/, "", line); sub(/"$/, "", line)
        print line; exit
      }
    ' "$REPO_ROOT/Cargo.lock")"
  fi
  if [ -z "$v" ]; then
    v="$(sed -nE 's/^lbug[[:space:]]*=[[:space:]]*"=?([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' \
      "$REPO_ROOT/Cargo.toml" | head -n1)"
  fi
  printf '%s' "$v"
}

# Name of the prebuilt static archive for this OS/arch, mirroring lbug's own
# scripts/download-liblbug.sh selection logic.
prebuilt_asset_name() {
  local os arch variant="${LBUG_LINUX_VARIANT:-compat}"
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux)
      case "$arch" in
        x86_64) arch="x86_64" ;;
        aarch64 | arm64) arch="aarch64" ;;
        *) return 1 ;;
      esac
      printf 'liblbug-static-linux-%s-%s.tar.gz' "$arch" "$variant"
      ;;
    Darwin)
      case "$arch" in
        x86_64) arch="x86_64" ;;
        arm64) arch="arm64" ;;
        *) return 1 ;;
      esac
      printf 'liblbug-static-osx-%s.tar.gz' "$arch"
      ;;
    *) return 1 ;;
  esac
}

find_registry_prebuilt() {
  # Echo the directory of an existing prebuilt liblbug.a in the cargo registry,
  # if any (fast, offline path for developers who already built lbug once).
  find "$CARGO_HOME_DIR/registry/src" \
    -path "*/lbug-*/.cache/lbug-prebuilt/*/lib/$static_lib_name" \
    2>/dev/null | head -n1
}

# Copy headers first, then the static lib atomically, so that the existence of
# liblbug.a always implies the headers are already present.
install_prebuilt_from() {
  local src="$1"
  cp -f "$src"/lbug.h "$LIB_DIR/" 2>/dev/null || true
  cp -f "$src"/lbug.hpp "$LIB_DIR/" 2>/dev/null || true
  cp -f "$src/$static_lib_name" "$LIB_DIR/.$static_lib_name.tmp"
  mv -f "$LIB_DIR/.$static_lib_name.tmp" "$LIB_FILE"
}

# Look up the pinned SHA-256 for (version, asset) in CHECKSUM_MANIFEST. Prints
# the hex digest on stdout, or nothing when no row matches. Comment/blank lines
# are ignored; the manifest format is `<sha256>  <version>  <asset>`.
expected_sha256() {
  local version="$1" asset="$2"
  [ -f "$CHECKSUM_MANIFEST" ] || return 0
  awk -v v="$version" -v a="$asset" '
    /^[[:space:]]*#/ { next }
    NF >= 3 && $2 == v && $3 == a { print $1; exit }
  ' "$CHECKSUM_MANIFEST"
}

# Content-integrity gate: fail (return 1, logging why) unless FILE hashes to the
# pinned digest for (version, asset). Fail-closed — a missing pin is a refusal,
# never an implicit trust. The caller cleans up and `die`s so tmp is not leaked.
verify_sha256() {
  local file="$1" version="$2" asset="$3" want got
  want="$(expected_sha256 "$version" "$asset")"
  if [ -z "$want" ]; then
    log "ERROR: no pinned SHA-256 for $asset (lbug $version) in $CHECKSUM_MANIFEST;"
    log "       refusing to link an unverified prebuilt. Regenerate the manifest"
    log "       for this version (see scripts/lbug-prebuilt.sha256)."
    return 1
  fi
  got="$(sha256sum "$file" | awk '{ print $1 }')"
  if [ "$got" != "$want" ]; then
    log "ERROR: SHA-256 mismatch for $asset (lbug $version):"
    log "       expected $want"
    log "       got      $got"
    log "       possible tampered/corrupt release asset — aborting before extraction."
    return 1
  fi
  return 0
}

download_prebuilt() {
  # Fetch the version-pinned LadybugDB release *asset* directly (a tarball, not
  # an executable script) — the same static archive lbug's build script and the
  # build/coverage jobs consume.
  local version asset repo url
  version="$(lbug_version || true)"
  [ -n "$version" ] || die "could not determine lbug version from Cargo.lock/Cargo.toml"
  asset="$(prebuilt_asset_name || true)"
  [ -n "$asset" ] || die "unsupported OS/arch for prebuilt liblbug ($(uname -sm))"
  repo="${LBUG_GITHUB_REPOSITORY:-LadybugDB/ladybug}"
  url="https://github.com/$repo/releases/download/v$version/$asset"

  mkdir -p "$LIB_DIR"
  # Download + extract in a temp dir, then place liblbug.a LAST so a partial
  # download/extract never leaves a half-written archive (or a lib without its
  # headers) that a later run would trust.
  local tmp
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/simard-lbug-dl.XXXXXX")"
  log "downloading prebuilt static liblbug $version ($asset)"
  if ! curl -fSL "$url" -o "$tmp/$asset"; then rm -rf "$tmp"; die "download failed: $url"; fi
  # Content-integrity gate (issue #2471): verify the pinned SHA-256 BEFORE the
  # archive is trusted/extracted, so a tampered-at-rest asset is never linked.
  if ! verify_sha256 "$tmp/$asset" "$version" "$asset"; then
    rm -rf "$tmp"
    die "checksum verification failed for $asset (lbug $version): refusing to extract an unverified prebuilt"
  fi
  if ! tar xzf "$tmp/$asset" -C "$tmp"; then rm -rf "$tmp"; die "extract failed: $asset"; fi
  [ -f "$tmp/$static_lib_name" ] || { rm -rf "$tmp"; die "archive $asset missing $static_lib_name"; }
  install_prebuilt_from "$tmp"
  rm -rf "$tmp"
}

ensure_prebuilt() {
  if [ -f "$LIB_FILE" ]; then
    return 0
  fi
  mkdir -p "$LIB_DIR"

  local reg
  reg="$(find_registry_prebuilt || true)"
  if [ -n "$reg" ]; then
    log "copying prebuilt liblbug from $(dirname "$reg")"
    install_prebuilt_from "$(dirname "$reg")"
  fi

  if [ ! -f "$LIB_FILE" ]; then
    download_prebuilt
  fi

  [ -f "$LIB_FILE" ] || die "failed to provision $LIB_FILE"
}

# Provision only when executed directly. When sourced (e.g. by the qa-team
# checksum scenario tests/gadugi/ci-harden-lbug-checksum.sh) expose the
# verification helpers without triggering a download.
if [ "${BASH_SOURCE[0]:-$0}" = "${0}" ]; then
  ensure_prebuilt
  log "lbug native static lib resolved: $LIB_FILE"
  printf '%s\n' "$LIB_DIR"
fi
