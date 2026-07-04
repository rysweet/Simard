#!/usr/bin/env bash
# qa-team scenario for issue #2549 — meeting agent proxy repo context + timeout.
#
# Outside-in verification that the meeting-mode agent proxy
# (`PersistentAgentProxy`, used by `simard meeting repl`):
#
#   1. Derives its working directory and `--add-dir` grant from the ACTIVE
#      repository / explicit config — never the old hardcoded operator path
#      `/home/azureuser/src/Simard/worktrees/main`.
#   2. Enforces a sane DEFAULT per-turn timeout and degrades honestly (bounded
#      wait + error banner) instead of blocking the REPL forever on a hung
#      `copilot -p` child.
#
# Layers:
#   A. Source guard — the hardcoded literal is gone from agent_proxy.rs.
#   B. Deterministic unit coverage (no live LLM) driving the REAL code paths:
#      repo-root derivation, explicit override, bounded-default timeout, and
#      honest degradation on a hung child.
#   C. CLI smoke — `simard meeting repl` launched from an ARBITRARY cwd (a
#      throwaway git repo, NOT the hardcoded path). Bounded by an outer
#      `timeout` and a short per-turn timeout: it must never hang, must never
#      reference the hardcoded operator path, and must either bind the agent to
#      that repo (agent operates on the launch repo) or degrade honestly.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

HARDCODED="/home/azureuser/src/Simard/worktrees/main"

# ── Layer A: the hardcoded operator path must be gone from the proxy ──
# Scope the check to production code (everything above the `#[cfg(test)]`
# module) — the tests legitimately name the literal in `assert_ne!` guards to
# prove the resolver never returns it.
PROD_SRC="$(awk '/^#\[cfg\(test\)\]/{exit} {print}' src/meeting_backend/agent_proxy.rs)"
if printf '%s\n' "$PROD_SRC" | grep -F "$HARDCODED" >/dev/null 2>&1; then
  echo "[gadugi] FAIL: hardcoded operator path still present in agent_proxy.rs production code" >&2
  exit 1
fi
echo "[gadugi] Layer A: hardcoded operator path absent from agent_proxy.rs production code — OK"

# ── Layer B: deterministic unit coverage of the real code paths ──
UNIT_OUTPUT="$(
  cargo test --lib --locked -- \
    resolve_agent_workdir_derives_repo_root_from_cwd \
    resolve_agent_workdir_honors_explicit_override \
    resolve_agent_workdir_ignores_nonexistent_override \
    parse_turn_timeout_unset_uses_bounded_default \
    parse_turn_timeout_positive_override_wins \
    parse_turn_timeout_zero_disables_explicitly \
    parse_turn_timeout_malformed_falls_back_to_default \
    new_defaults_to_bounded_turn_timeout_when_env_unset \
    invoke_agent_degrades_honestly_on_timeout \
    invoke_agent_degrades_when_child_closes_stdout_then_hangs \
    invoke_agent_timeout_reaps_descendant_processes \
    invoke_agent_returns_output_when_child_exits_before_timeout \
    invoke_agent_captures_full_burst_before_exit \
    2>&1
)"
printf '%s\n' "$UNIT_OUTPUT"

printf '%s\n' "$UNIT_OUTPUT" | grep -E "test result: ok\. [0-9]+ passed; 0 failed" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "resolve_agent_workdir_derives_repo_root_from_cwd ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "resolve_agent_workdir_honors_explicit_override ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "new_defaults_to_bounded_turn_timeout_when_env_unset ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "invoke_agent_degrades_honestly_on_timeout ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "invoke_agent_degrades_when_child_closes_stdout_then_hangs ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "invoke_agent_timeout_reaps_descendant_processes ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "invoke_agent_returns_output_when_child_exits_before_timeout ... ok" >/dev/null
printf '%s\n' "$UNIT_OUTPUT" | grep -F "invoke_agent_captures_full_burst_before_exit ... ok" >/dev/null
echo "[gadugi] Layer B: repo-derivation + bounded-timeout + honest-degradation + subtree-reap + full-burst unit paths — OK"

# ── Layer C: CLI smoke from an ARBITRARY cwd (throwaway git repo) ──
TMP_REPO="$(mktemp -d /tmp/simard-meeting-cwd.XXXXXX)"
STATE_ROOT="$(mktemp -d /tmp/simard-meeting-state.XXXXXX)"
trap 'rm -rf "$TMP_REPO" "$STATE_ROOT"' EXIT

git -C "$TMP_REPO" init -q
git -C "$TMP_REPO" config user.email gadugi@example.com
git -C "$TMP_REPO" config user.name gadugi
echo "sentinel-arbitrary-repo" >"$TMP_REPO/SENTINEL.md"
git -C "$TMP_REPO" add -A
git -C "$TMP_REPO" commit -q -m "seed arbitrary meeting repo"

# Sanity: the throwaway repo is NOT the hardcoded operator path.
case "$TMP_REPO" in
  "$HARDCODED"*) echo "[gadugi] FAIL: temp repo collided with hardcoded path" >&2; exit 1 ;;
esac

# Build the CLI once so the timed run below measures behavior, not compilation.
cargo build --quiet --bin simard

set +e
# Launch the REPL from the arbitrary repo. Explicit config binds the agent's
# grant to THIS repo; a short per-turn timeout guarantees any live turn degrades
# fast; the outer `timeout` guarantees the whole thing cannot hang.
SMOKE_OUTPUT="$(
  cd "$TMP_REPO" && \
  SIMARD_STATE_ROOT="$STATE_ROOT" \
  SIMARD_MEETING_AGENT_DIR="$TMP_REPO" \
  SIMARD_MEETING_TURN_TIMEOUT_SECS=5 \
  RUST_LOG=info \
  timeout 90 cargo run --quiet --manifest-path "$ROOT/Cargo.toml" --bin simard -- \
    meeting repl "gadugi arbitrary cwd smoke" \
    <<<"$(printf 'hello from an arbitrary repo\n/close\n')" 2>&1
)"
SMOKE_CODE=$?
set -e

printf '%s\n' "$SMOKE_OUTPUT"
echo "[gadugi] Layer C: simard meeting repl exit code from arbitrary cwd = $SMOKE_CODE"

# Assertion 1 — no hang. `timeout` returns 124 iff it had to kill the process.
if [ "$SMOKE_CODE" -eq 124 ]; then
  echo "[gadugi] FAIL: meeting repl hung from an arbitrary cwd (outer timeout fired)" >&2
  exit 1
fi

# Assertion 2 — the hardcoded operator path must never surface at runtime.
if printf '%s\n' "$SMOKE_OUTPUT" | grep -F "$HARDCODED" >/dev/null 2>&1; then
  echo "[gadugi] FAIL: runtime referenced the hardcoded operator path" >&2
  exit 1
fi

# Assertion 3 — either the agent bound to THIS repo (operates on the launch
# repo) or it degraded honestly. Both satisfy the Done-when contract.
if printf '%s\n' "$SMOKE_OUTPUT" | grep -F "Agent proxy opened" >/dev/null 2>&1; then
  # Agent came up: its resolved workdir/grant must be the launch repo.
  printf '%s\n' "$SMOKE_OUTPUT" | grep -F "$TMP_REPO" >/dev/null || {
    echo "[gadugi] FAIL: agent opened but did not bind to the launch repo ($TMP_REPO)" >&2
    exit 1
  }
  echo "[gadugi] Layer C: agent bound to the launch repo (correct repo context) — OK"
else
  # Agent did not come up (e.g. no provider/binary in CI): must degrade honestly.
  printf '%s\n' "$SMOKE_OUTPUT" \
    | grep -Eiq "meeting agent|no agent backend|not configured|\[meeting|timeout|Error:" || {
      echo "[gadugi] FAIL: no repo context and no honest-degradation signal" >&2
      exit 1
    }
  echo "[gadugi] Layer C: agent degraded honestly within the timeout — OK"
fi

echo "[gadugi] meeting agent repo-context + per-turn timeout (#2549) verified"
