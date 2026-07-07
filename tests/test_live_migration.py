"""Outside-in TDD (tests-first) for Simard LIVE MIGRATION components.

Issue #2923 — design spike: migrate Simard live to another host as a
SECONDARY/STANDBY, validate parity without acting, then FINAL SYNC + CUTOVER
with a clean, reversible role swap and exactly-one-primary at all times.

These tests are written BEFORE the components exist. The migration surface is
probed for presence; until it is built (a phase-D green-CI PR), its scenarios
``pytest.skip`` with a clear reason + the tracking reference, so mainline CI
stays green during the spike. When a component lands (a prebuilt ``simard``
binary that exposes ``migrate`` with the required subcommands), the guards flip
to hard assertions that drive the implementation.

Design contracts asserted here (single source of truth for phase-D builders):

  * A ``simard migrate`` CLI surface with subcommands:
      provision | export-graph | import-graph | install | sync-config |
      standby | validate | cutover | rollback | status
  * A role-state file at ``$SIMARD_STATE_ROOT/migration/role.json`` with a
      schema: {role: primary|standby, fencing_generation: int,
               primary_lease_owner_pid: int, updated_epoch: int}
  * A STANDBY that performs NO autonomous activity (no OODA advancement, no
      engineer spawning, no merges/deploys, no Signal SENDING) until cutover.
  * A migration orchestrator recipe with validation gates + LOUD rollback
      (no silent fallback): a failed stage surfaces and rolls back.
  * NO secrets in any committed / synced artifact; secure transfer only.
  * Operator (rysweet) notified via the Signal self-notes channel at each
      milestone.

PROCESS-C REVIEW CORRECTIONS (multi-model/agent review of the spike design —
apply these when flipping the skips to assertions; the schema above is the
probe shape, NOT the authoritative design):
  * SSOT for "who is primary" is the CROSS-HOST ``PrimaryLease`` (a shared
      backend, e.g. an Azure Blob lease, holder + monotonic FENCING TOKEN).
      ``role.json`` is only a LOCAL CACHE mirroring the lease — never
      authoritative. If the lease is unreachable, ``require_primary()`` MUST
      fail-safe to STANDBY and surface loudly; never trust the cached role.
  * Fence cross-host by TOKEN, not by PID. ``primary_lease_owner_pid`` is
      host-local and meaningless across hosts (the existing ``LeaderSemaphore``
      uses ``kill(pid,0)`` liveness → single-host only). Prefer a
      ``fencing_token``/lease-id checked at every side-effecting actuator
      (memory store-write, engineer dispatch, signal send, git push).
  * The orchestrator GENERALIZES the existing ``safe_update`` /``self_deploy``
      phase machine (Drain→Snapshot→Validate→Rollback + health probes), it is
      not a greenfield sequencer. Cutover = flip the lease LAST, after the
      target validates in STANDBY (so pre-cutover stays cheap-to-reverse).
  * The az provisioner extends the existing ``remote_azlin`` wrapper; the
      graph-complete export/import lives in amplihack-memory-lib (G2) and
      SUPERSEDES the deprecated, lossy ``src/remote_transfer/`` path.
  See epic #2726 for the reviewed component list, contracts, and build order.

Run:
    python3 -m pytest tests/test_live_migration.py -v
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Repo + surface discovery (anchored to this file's own worktree)
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[1]
MIGRATION_SRC = REPO_ROOT / "src" / "migration"

# Design-contract identifiers phase-D must satisfy.
MIGRATE_SUBCOMMANDS = [
    "provision",
    "export-graph",
    "import-graph",
    "install",
    "sync-config",
    "standby",
    "validate",
    "cutover",
    "rollback",
    "status",
]
ROLE_SCHEMA_FIELDS = {
    "role",
    "fencing_generation",
    "primary_lease_owner_pid",
    "updated_epoch",
}
STANDBY_SUPPRESSED_ACTIVITIES = {
    "ooda_advance",
    "engineer_spawn",
    "merge",
    "deploy",
    "signal_send",
}
# Current host `ia2` baseline the target must meet or exceed.
MIN_VM_SIZE = "Standard_E64as_v5"
MIN_VCPUS = 64


def _simard_binary() -> Path | None:
    """Return a prebuilt simard binary if one exists (never triggers a build)."""
    for candidate in (
        REPO_ROOT / "target" / "release" / "simard",
        REPO_ROOT / "target" / "debug" / "simard",
    ):
        if candidate.exists() and os.access(candidate, os.X_OK):
            return candidate
    return None


def _migrate_help() -> str | None:
    """`simard migrate --help` text, or None if the surface is unavailable.

    Returns None both when no binary is built AND when a binary exists but does
    not (yet) expose the ``migrate`` subcommand — the correct skip signal during
    the spike.
    """
    binary = _simard_binary()
    if binary is None:
        return None
    try:
        proc = subprocess.run(
            [str(binary), "migrate", "--help"],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    return (proc.stdout or "") + (proc.stderr or "")


def _run_migrate(*args: str, timeout: int = 120) -> subprocess.CompletedProcess:
    binary = _simard_binary()
    assert binary is not None, "simard binary required for this assertion"
    return subprocess.run(
        [str(binary), "migrate", *args],
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _require_migrate_surface():
    """Skip unless the migrate surface is designed at all (source or CLI)."""
    if not MIGRATION_SRC.exists() and _migrate_help() is None:
        pytest.skip(
            "migration surface not built yet (issue #2923 phase-D: "
            "`simard migrate` CLI / src/migration). tests-first spec pending."
        )


def _require_migrate_cli():
    """Skip unless a prebuilt binary actually exposes the `migrate` subcommand.

    This is the gate for every test that *executes* the CLI: a bare `simard`
    binary without the migrate surface must SKIP, not fail.
    """
    if _migrate_help() is None:
        pytest.skip(
            "`simard migrate` CLI not available yet (issue #2923 phase-D). "
            "Build the binary with the migrate surface to run these assertions."
        )


# ===================================================================
# Scenario A: `simard migrate` CLI surface exists with all subcommands
# ===================================================================


class TestMigrateCliSurface:
    def test_migration_module_or_cli_present(self):
        _require_migrate_surface()
        assert MIGRATION_SRC.exists() or _migrate_help() is not None

    @pytest.mark.parametrize("sub", MIGRATE_SUBCOMMANDS)
    def test_subcommand_advertised(self, sub):
        _require_migrate_cli()
        help_text = _migrate_help()
        assert sub in help_text, f"`simard migrate {sub}` must be advertised in --help"


# ===================================================================
# Scenario B: Role / standby state machine + fencing generation
# ===================================================================


class TestRoleStateMachine:
    """The single source of truth for 'who is primary' with a fencing token."""

    def _role_file(self) -> Path:
        state_root = Path(os.environ.get("SIMARD_STATE_ROOT", Path.home() / ".simard"))
        return state_root / "migration" / "role.json"

    def test_role_file_schema_when_present(self):
        role_file = self._role_file()
        if not role_file.exists():
            _require_migrate_surface()
            pytest.skip("role.json not written yet (issue #2923 role-state component)")
        data = json.loads(role_file.read_text())
        missing = ROLE_SCHEMA_FIELDS - set(data)
        assert not missing, f"role.json missing required fields: {missing}"
        assert data["role"] in {"primary", "standby"}
        assert isinstance(data["fencing_generation"], int)

    def test_status_reports_single_role(self):
        _require_migrate_cli()
        proc = _run_migrate("status", "--json")
        assert proc.returncode == 0, f"status failed: {proc.stderr}"
        status = json.loads(proc.stdout)
        assert status.get("role") in {"primary", "standby"}
        assert "fencing_generation" in status


# ===================================================================
# Scenario C: STANDBY performs NO autonomous activity until cutover
# ===================================================================


class TestStandbySuppression:
    """A secondary must be aware it is secondary and act on nothing."""

    def test_standby_suppresses_all_autonomy(self):
        _require_migrate_cli()
        # Enter standby (dry / no side effects) and read suppression report.
        proc = _run_migrate("standby", "--report", "--json")
        assert proc.returncode == 0, f"standby report failed: {proc.stderr}"
        report = json.loads(proc.stdout)
        suppressed = set(report.get("suppressed", []))
        assert STANDBY_SUPPRESSED_ACTIVITIES <= suppressed, (
            "standby must suppress OODA advance, engineer spawn, merge, deploy, "
            f"and Signal send; missing: {STANDBY_SUPPRESSED_ACTIVITIES - suppressed}"
        )
        assert report.get("role") == "standby"

    def test_standby_refuses_signal_send(self):
        _require_migrate_cli()
        # Attempting an outbound send while standby must be refused LOUDLY
        # (non-zero exit / explicit refusal), never silently swallowed.
        proc = _run_migrate("standby", "--simulate-send", "hello")
        assert proc.returncode != 0
        assert re.search(r"standby|secondary|refus", proc.stderr, re.IGNORECASE)


# ===================================================================
# Scenario D: Target provisioner (az wrapper) — dry-run / plan only
# ===================================================================


class TestProvisionerDryRun:
    """Provision a target VM >= current host without making real az calls."""

    def test_provision_plan_meets_size_floor(self):
        _require_migrate_cli()
        proc = _run_migrate(
            "provision",
            "--dry-run",
            "--json",
            "--resource-group",
            "rysweet-linux-vm-pool",
        )
        assert proc.returncode == 0, f"provision dry-run failed: {proc.stderr}"
        plan = json.loads(proc.stdout)
        assert int(plan.get("vcpus", 0)) >= MIN_VCPUS, (
            f"target VM must have >= {MIN_VCPUS} vCPUs (>= {MIN_VM_SIZE})"
        )
        assert plan.get("premium_data_disk") is True, (
            "target must attach a Premium SSD data disk for /home (like ia2_home)"
        )
        # Dry-run must NOT have created anything.
        assert plan.get("created") in (False, None)

    def test_dry_run_makes_no_real_az_mutation(self):
        _require_migrate_cli()
        proc = _run_migrate("provision", "--dry-run", "--json")
        assert proc.returncode == 0
        assert "az vm create" not in proc.stderr.lower() or "--dry-run" in proc.stderr


# ===================================================================
# Scenario E: Config + secret sync — NO secrets ever leak
# ===================================================================


class TestConfigSecretSync:
    """config.toml [signal] + creds transferred securely; never committed."""

    def test_sync_plan_excludes_secret_material_from_repo_artifacts(self):
        _require_migrate_cli()
        proc = _run_migrate("sync-config", "--plan", "--json")
        assert proc.returncode == 0, f"sync-config plan failed: {proc.stderr}"
        plan = json.loads(proc.stdout)
        # Any artifact the plan would COMMIT/write into the repo must be marked
        # secret-free; secrets must be routed via a secure (non-repo) channel.
        for artifact in plan.get("repo_artifacts", []):
            assert artifact.get("contains_secrets") is False, (
                f"repo artifact {artifact.get('path')} must not contain secrets"
            )
        for item in plan.get("secret_items", []):
            assert item.get("transport") in {"secure-copy", "kv", "ssh"}, (
                "secrets must use a secure transport, never plaintext/repo"
            )

    def test_no_secret_regex_in_planned_repo_artifacts(self):
        _require_migrate_cli()
        proc = _run_migrate("sync-config", "--plan", "--emit-repo-artifacts", "--json")
        assert proc.returncode == 0
        plan = json.loads(proc.stdout)
        secret_like = re.compile(
            r"(BEGIN [A-Z ]*PRIVATE KEY|password\s*[:=]|token\s*[:=]|"
            r"aws_secret|signal.*(password|pin))",
            re.IGNORECASE,
        )
        for artifact in plan.get("repo_artifacts", []):
            body = artifact.get("preview", "")
            assert not secret_like.search(body), (
                f"secret-looking content in planned repo artifact {artifact.get('path')}"
            )


# ===================================================================
# Scenario F: Worktree quiesce + transfer (git worktrees are host-local)
# ===================================================================


class TestWorktreeQuiesce:
    def test_quiesce_inventory_captures_active_worktrees(self):
        _require_migrate_cli()
        proc = _run_migrate("cutover", "--quiesce-plan", "--json")
        assert proc.returncode == 0, f"quiesce plan failed: {proc.stderr}"
        plan = json.loads(proc.stdout)
        assert "worktrees" in plan, "quiesce must inventory active worktrees"
        for wt in plan["worktrees"]:
            # Each worktree needs branch + head so it can be re-created on target.
            assert {"path", "branch", "head"} <= set(wt)
        assert plan.get("engineers_paused") in (True, False)


# ===================================================================
# Scenario G: Orchestrator gates + LOUD rollback (no silent fallback)
# ===================================================================


class TestOrchestratorRollback:
    def test_failed_stage_triggers_loud_rollback(self):
        _require_migrate_cli()
        # Inject a forced failure at the 'validate' gate; the orchestrator must
        # surface it (non-zero) AND report a rollback — never silently continue.
        proc = _run_migrate("cutover", "--dry-run", "--fail-at", "validate", "--json")
        assert proc.returncode != 0, "a failed gate must surface loudly (non-zero)"
        combined = (proc.stdout or "") + (proc.stderr or "")
        assert re.search(r"rollback|rolled back|reverting", combined, re.IGNORECASE), (
            "a failed migration stage must roll back, not silently fall back"
        )

    def test_rollback_is_idempotent_and_reversible(self):
        _require_migrate_cli()
        first = _run_migrate("rollback", "--dry-run")
        second = _run_migrate("rollback", "--dry-run")
        assert first.returncode == 0 and second.returncode == 0, (
            "rollback must be safely re-runnable (idempotent)"
        )


# ===================================================================
# Scenario H: Observability + operator Signal self-note at milestones
# ===================================================================


class TestMigrationObservability:
    def test_milestones_emit_operator_notifications(self):
        _require_migrate_cli()
        proc = _run_migrate("status", "--milestones", "--json")
        assert proc.returncode == 0, f"milestones query failed: {proc.stderr}"
        data = json.loads(proc.stdout)
        milestones = data.get("milestones", [])
        # The design requires operator (rysweet) updates via Signal self-notes
        # at each milestone; each milestone must carry a notification channel.
        for m in milestones:
            assert m.get("notify_channel") in {"signal-self-note", "none"}
        # At minimum the lifecycle milestones must be enumerated.
        names = {m.get("name") for m in milestones}
        expected = {"provisioned", "graph-imported", "standby-up", "validated", "cutover"}
        if milestones:
            assert expected <= names, f"missing milestones: {expected - names}"
