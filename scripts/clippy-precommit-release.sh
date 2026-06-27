#!/usr/bin/env bash
#
# clippy-precommit-release.sh — run the pre-commit `cargo clippy --release`
# gate with the `lbug` native static library reliably on the linker search
# path.
#
# Why this wrapper exists (issue #2426 / #2423)
# ---------------------------------------------
# `lbug` 0.17.1's build script downloads a prebuilt `liblbug.a` into the cargo
# *registry source* directory:
#
#     ~/.cargo/registry/src/<hash>/lbug-0.17.1/.cache/lbug-prebuilt/latest/lib/
#
# and emits `cargo:rustc-link-search=native=<that dir>` plus
# `cargo:rustc-link-lib=static:+whole-archive=lbug`.
#
# CI's cargo cache (Swatinem/rust-cache) persists `target/` — including the
# cached lbug build-script output that *references* that registry-src path —
# but it does NOT persist `registry/src`. On a cache restore the prebuilt
# archive is gone, yet cargo treats lbug as fresh and reuses the cached
# build-script output, so `cargo clippy --release` fails with:
#
#     error: could not find native static library `lbug`, perhaps an -L flag is missing?
#
# The `build`/`coverage` jobs don't hit this on every run because they
# (re)provision lbug for the dev profile; the verify pre-commit job runs
# `cargo clippy --release` first and reds out before anything else compiles.
#
# What this wrapper does
# ----------------------
# 1. Ensures a stable `liblbug.a` (+ headers) exists outside registry/src —
#    copied from an existing registry prebuilt when available, otherwise
#    downloaded as the version-pinned LadybugDB release asset (the same static
#    archive the build & coverage jobs consume).
# 2. Exports `LBUG_LIBRARY_DIR`/`LBUG_INCLUDE_DIR` — lbug's first-class
#    "external prebuilt" interface — so any build-script (re)run resolves lbug
#    from the stable copy deterministically.
# 3. Forces an lbug build-script re-run ONLY when the cached release
#    link-search is genuinely stale (none of the referenced search dirs
#    actually contains `liblbug.a`). Warm local checks stay fully incremental.
# 4. Runs `cargo clippy --release --no-deps -- -D warnings`, after asserting the
#    native lib is resolvable so the regression can't return silently.
#
# Safe to run locally and in CI; idempotent.

set -euo pipefail

log() { printf '[clippy-precommit-release] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"

# Resolve the cargo target directory (honours CARGO_TARGET_DIR, used by the
# per-engineer worktree isolation that relocates target/ out of the worktree).
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

# Stable location for the lbug native lib, deliberately OUTSIDE registry/src so
# it survives cargo-cache restores and registry re-extraction.
STABLE_LIB_DIR="${SIMARD_LBUG_LIB_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/simard-lbug-precommit/lib}"
STABLE_LIB_FILE="$STABLE_LIB_DIR/liblbug.a"

static_lib_name="liblbug.a"

# lbug crate version (and matching LadybugDB native release tag) parsed from
# Cargo.toml, so the prebuilt asset is fetched deterministically — no
# unauthenticated `releases/latest` API call (rate-limited on shared CI egress
# IPs) and no version skew with the crate we actually compile against.
lbug_version() {
  sed -nE 's/^lbug[[:space:]]*=[[:space:]]*"=?([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' \
    "$REPO_ROOT/Cargo.toml" | head -n1
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

# ── 1. Provision a stable liblbug.a (+ headers) ──────────────────────────────
find_registry_prebuilt() {
  # Echo the directory of an existing prebuilt liblbug.a in the cargo registry,
  # if any (fast, offline path for developers who already built lbug once).
  find "$CARGO_HOME_DIR/registry/src" \
    -path "*/lbug-*/.cache/lbug-prebuilt/*/lib/$static_lib_name" \
    2>/dev/null | head -n1
}

download_prebuilt() {
  # Fetch the version-pinned LadybugDB release *asset* directly (a tarball, not
  # an executable script) — the same static archive lbug's build script and the
  # build/coverage jobs consume. Pinning the version avoids the
  # `api.github.com/.../releases/latest` lookup and an unpinned `curl | bash`.
  local version repo asset url
  version="$(lbug_version || true)"
  [ -n "$version" ] || die "could not determine lbug version from Cargo.toml"
  asset="$(prebuilt_asset_name || true)"
  [ -n "$asset" ] || die "unsupported OS/arch for prebuilt liblbug ($(uname -sm))"
  repo="${LBUG_GITHUB_REPOSITORY:-LadybugDB/ladybug}"
  url="https://github.com/$repo/releases/download/v$version/$asset"

  mkdir -p "$STABLE_LIB_DIR"
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

# Copy headers first, then the static lib atomically, so that the existence of
# liblbug.a always implies the headers are already present.
install_prebuilt_from() {
  local src="$1"
  cp -f "$src"/lbug.h "$STABLE_LIB_DIR/" 2>/dev/null || true
  cp -f "$src"/lbug.hpp "$STABLE_LIB_DIR/" 2>/dev/null || true
  cp -f "$src/$static_lib_name" "$STABLE_LIB_DIR/.$static_lib_name.tmp"
  mv -f "$STABLE_LIB_DIR/.$static_lib_name.tmp" "$STABLE_LIB_FILE"
}

ensure_stable_prebuilt() {
  if [ -f "$STABLE_LIB_FILE" ]; then
    return 0
  fi
  mkdir -p "$STABLE_LIB_DIR"

  local reg
  reg="$(find_registry_prebuilt || true)"
  if [ -n "$reg" ]; then
    local src_dir
    src_dir="$(dirname "$reg")"
    log "copying prebuilt liblbug from $src_dir"
    install_prebuilt_from "$src_dir"
  fi

  if [ ! -f "$STABLE_LIB_FILE" ]; then
    download_prebuilt
  fi

  [ -f "$STABLE_LIB_FILE" ] || die "failed to provision $STABLE_LIB_FILE"
}

ensure_stable_prebuilt
log "lbug native static lib resolved: $STABLE_LIB_FILE"

export LBUG_LIBRARY_DIR="$STABLE_LIB_DIR"
export LBUG_INCLUDE_DIR="$STABLE_LIB_DIR"

# ── 2. Force a build-script re-run only when the cached output is stale ───────
# Stale == cargo has a cached release lbug build-script output, but NONE of the
# `rustc-link-search` dirs it references actually contains liblbug.a (the CI
# cache-eviction case). When warm/local the registry-src prebuilt is present so
# this is a no-op and the check stays incremental.
release_lbug_stale() {
  local found_output=0 has_lib=0 out dir
  shopt -s nullglob
  for out in "$TARGET_DIR"/release/build/lbug-*/output; do
    found_output=1
    while IFS= read -r dir; do
      [ -n "$dir" ] || continue
      if [ -f "$dir/$static_lib_name" ]; then
        has_lib=1
      fi
    done < <(sed -n 's/^cargo:rustc-link-search=native=//p' "$out")
  done
  shopt -u nullglob
  # Stale only when there IS cached output but the static lib is unresolvable.
  [ "$found_output" = "1" ] && [ "$has_lib" = "0" ]
}

if release_lbug_stale; then
  log "stale cached lbug link-search (liblbug.a evicted); forcing build-script re-run"
  rm -rf "$TARGET_DIR"/release/build/lbug-* "$TARGET_DIR"/release/.fingerprint/lbug-* 2>/dev/null || true
fi

# ── 3. Run the actual gate ───────────────────────────────────────────────────
log "running: cargo clippy --release --no-deps -- -D warnings"
set +e
cargo clippy --release --no-deps -- -D warnings
rc=$?
set -e

if [ "$rc" -eq 0 ]; then
  # Stable, greppable success marker for the CI regression-guard assertion.
  log "SUCCESS: cargo clippy --release linked lbug (lib: $STABLE_LIB_FILE)"
fi
exit "$rc"
