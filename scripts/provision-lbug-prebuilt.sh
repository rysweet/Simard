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

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"

LIB_DIR="${1:-${SIMARD_LBUG_LIB_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/simard-lbug-precommit/lib}}"
LIB_FILE="$LIB_DIR/liblbug.a"
static_lib_name="liblbug.a"

# lbug crate version (== matching LadybugDB native release tag) parsed from
# Cargo.toml, so the prebuilt asset is fetched deterministically — no
# unauthenticated `releases/latest` API call and no version skew with the crate
# we actually compile against.
lbug_version() {
  # 1) Inline registry version in Cargo.toml (e.g. `lbug = "=0.17.1"`).
  local v
  v="$(sed -nE 's/^lbug[[:space:]]*=[[:space:]]*"=?([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' \
    "$REPO_ROOT/Cargo.toml" | head -n1)"
  # 2) Fallback for a git/path dependency (e.g. lbug repointed to a fork,
  #    `lbug = { git = "…", rev = "…" }` — issue #3119): the inline regex above
  #    matches nothing, so resolve the version Cargo actually locked from
  #    Cargo.lock's `[[package]] name = "lbug"` entry. The prebuilt asset is
  #    still published against that version tag on the release repo, so the
  #    version-pinned download stays deterministic regardless of the source.
  if [ -z "$v" ] && [ -f "$REPO_ROOT/Cargo.lock" ]; then
    v="$(awk '/^name = "lbug"$/ { f = 1 } f && /^version = / { gsub(/[^0-9.]/, ""); print; exit }' \
      "$REPO_ROOT/Cargo.lock")"
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

download_prebuilt() {
  # Fetch the version-pinned LadybugDB release *asset* directly (a tarball, not
  # an executable script) — the same static archive lbug's build script and the
  # build/coverage jobs consume.
  local version asset repo url
  version="$(lbug_version || true)"
  [ -n "$version" ] || die "could not determine lbug version from Cargo.toml"
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

ensure_prebuilt
log "lbug native static lib resolved: $LIB_FILE"
printf '%s\n' "$LIB_DIR"
