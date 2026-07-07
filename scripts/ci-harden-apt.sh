#!/usr/bin/env bash
# ci-harden-apt.sh — drop the flaky packages.microsoft.com apt sources shipped
# on GitHub-hosted runners BEFORE any `apt-get update` in CI.
#
# The runner image configures `packages.microsoft.com` apt repositories
# (azure-cli, MS prod). These periodically serve an invalid `InRelease` file:
#
#   E: Failed to fetch https://packages.microsoft.com/.../InRelease
#      Clearsigned file isn't valid, got 'NOSPLIT' (does the network require authentication?)
#   E: The repository '... noble InRelease' is no longer signed.
#
# When that happens, every `apt-get update` exits 100 and fails the job —
# producing spurious red on the default branch (see the verify workflow's
# `pre-commit` and `e2e-dashboard` jobs). None of our CI jobs need those repos;
# they only install Ubuntu-archive packages (mold, Playwright browser deps), so
# removing the Microsoft sources makes apt immune to the recurring breakage.
#
# Idempotent and non-fatal: safe to run whether or not the sources are present,
# and it never fails the build on its own (removing a source we don't need can
# only help). Requires passwordless sudo (present on GitHub-hosted runners).
#
# Usage:
#   scripts/ci-harden-apt.sh

set -euo pipefail

MS_HOST_RE='packages\.microsoft\.com'
removed=0

# Modern runner images keep third-party repos as individual files under
# sources.list.d/ in either one-line (.list) or deb822 (.sources) format.
# Match by content so both formats — and any filename — are covered.
if [ -d /etc/apt/sources.list.d ]; then
  shopt -s nullglob
  for f in /etc/apt/sources.list.d/*; do
    [ -f "$f" ] || continue
    if sudo grep -qsE "$MS_HOST_RE" "$f"; then
      echo "ci-harden-apt: removing Microsoft apt source: $f"
      sudo rm -f "$f"
      removed=1
    fi
  done
  shopt -u nullglob
fi

# Some images embed the Microsoft repo in the monolithic sources.list instead.
# Comment only the *active* Microsoft lines: skip lines that are already
# commented (so the step is idempotent) and leave every non-Microsoft archive
# line untouched (so the file stays usable).
if [ -f /etc/apt/sources.list ] \
   && sudo grep -qsE "^[[:space:]]*[^#[:space:]].*${MS_HOST_RE}" /etc/apt/sources.list; then
  echo "ci-harden-apt: disabling Microsoft entries in /etc/apt/sources.list"
  sudo sed -i.ci-harden-apt.bak -E "/^[[:space:]]*#/! { /${MS_HOST_RE}/ s|^|# disabled-by-ci: | }" /etc/apt/sources.list
  removed=1
fi

if [ "$removed" -eq 0 ]; then
  echo "ci-harden-apt: no Microsoft apt sources found (nothing to do)"
else
  echo "ci-harden-apt: Microsoft apt sources removed; apt is now Ubuntu-archive only"
fi
