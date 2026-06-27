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

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

# Resolve the cargo target directory (honours CARGO_TARGET_DIR, used by the
# per-engineer worktree isolation that relocates target/ out of the worktree).
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

# Stable location for the lbug native lib, deliberately OUTSIDE registry/src so
# it survives cargo-cache restores and registry re-extraction.
STABLE_LIB_DIR="${SIMARD_LBUG_LIB_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/simard-lbug-precommit/lib}"
STABLE_LIB_FILE="$STABLE_LIB_DIR/liblbug.a"

static_lib_name="liblbug.a"

# ── 1. Provision a stable liblbug.a (+ headers) ──────────────────────────────
# Delegated to the shared provisioner (also used by CI) so the link path is
# defined in exactly one place.
"$REPO_ROOT/scripts/provision-lbug-prebuilt.sh" "$STABLE_LIB_DIR" >/dev/null
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
