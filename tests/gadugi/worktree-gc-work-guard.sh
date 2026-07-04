#!/usr/bin/env bash
# Issue #2553 — end-to-end regression guard for the operator worktree-GC
# data-loss incident.
#
# GUARANTEE UNDER TEST: `simard worktree-gc --apply` must NEVER delete a
# worktree that carries uncommitted work, even when that worktree is old
# enough that the idle policy would otherwise prune it. In the verified
# incident, `--apply` removed an in-use operator worktree carrying unsaved
# edits.
#
# Properties: offline, network-free, sleep-free, hermetic. The #2553
# work-guard short-circuits before any `gh` / `git ls-remote` call, so this
# scenario makes no network access. Git is isolated via GIT_CONFIG_* so no
# host global config, hooks, or templates leak in.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Hermetic git: ignore any host-level global/system config, hooks, templates.
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null

WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# --- Fixture: a parent repo with one registered engineer worktree ----------
PARENT="$WORK/parent"
mkdir -p "$PARENT"
git -C "$PARENT" init --initial-branch=main --quiet
git -C "$PARENT" config user.email t@example.com
git -C "$PARENT" config user.name test
git -C "$PARENT" config commit.gpgsign false
echo seed >"$PARENT/seed"
git -C "$PARENT" add seed
git -C "$PARENT" commit -m seed --quiet

ROOTDIR="$WORK/engineer-worktrees"
mkdir -p "$ROOTDIR"
WT="$ROOTDIR/wt-dirty"
git -C "$PARENT" worktree add -b feat/keepme "$WT" main --quiet

# Uncommitted (untracked) work that MUST survive a GC apply.
echo "PRECIOUS UNCOMMITTED WORK" >"$WT/precious.txt"

# Backdate the worktree so the idle policy (--idle-days=1) would select it for
# pruning were it not for the #2553 work-guard. This is what makes the test
# prove the guard did the work, rather than the worktree simply not matching a
# prune reason.
touch -d '20 days ago' "$WT/precious.txt" "$WT" 2>/dev/null || true

# --- Act: run the real current-branch binary in APPLY mode -----------------
set +e
OUT="$(cargo run --quiet --bin simard -- \
  worktree-gc --apply --idle-days=1 \
  --root="$ROOTDIR" --parent-repo="$PARENT" 2>&1)"
STATUS=$?
set -e
printf '%s\n' "$OUT"

# --- Assert ----------------------------------------------------------------
# 1. The command succeeds.
test "$STATUS" -eq 0 || {
  echo "FAIL: worktree-gc exited $STATUS" >&2
  exit 1
}
# 2. It pruned nothing (the idle worktree was vetoed by the work-guard).
printf '%s\n' "$OUT" | grep -Eq 'DONE pruned=0 failures=0' || {
  echo "FAIL: expected 'DONE pruned=0 failures=0'" >&2
  exit 1
}
# 3. The worktree directory and its uncommitted file are intact.
test -d "$WT" || {
  echo "FAIL: worktree directory was deleted" >&2
  exit 1
}
test -f "$WT/precious.txt" || {
  echo "FAIL: uncommitted file was deleted" >&2
  exit 1
}
grep -Fq 'PRECIOUS UNCOMMITTED WORK' "$WT/precious.txt" || {
  echo "FAIL: uncommitted file contents were lost" >&2
  exit 1
}

echo "PASS: worktree-gc --apply preserved a dirty, idle worktree (#2553)"
