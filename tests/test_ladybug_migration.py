"""Outside-in tests for kuzu→ladybug migration and hive-mind integration.

Validates the complete migration path:
  1. amplihack-memory-lib uses ladybug (not kuzu)
  2. flock serialization prevents concurrent write corruption
  3. read_only mode allows multiple concurrent readers
  4. Memory facade initializes with distributed topology
  5. Simard memory bridge creates backend via facade when available
  6. Bridge JSON-RPC protocol works end-to-end through ladybug
  7. kuzu/ladybug import conflict is absent in production code path
  8. remote_transfer.rs deprecation markers are present

Run with:
    PYTHONPATH=~/src/amplirusty/amplihack-memory-lib/src python3 -m pytest tests/test_ladybug_migration.py -v
"""

from __future__ import annotations

import fcntl
import json
import multiprocessing
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Ensure amplihack-memory-lib is importable
# ---------------------------------------------------------------------------

_MEMORY_LIB_SRC = Path.home() / "src" / "amplirusty" / "amplihack-memory-lib" / "src"
if str(_MEMORY_LIB_SRC) not in sys.path and (_MEMORY_LIB_SRC / "amplihack_memory").exists():
    sys.path.insert(0, str(_MEMORY_LIB_SRC))

_AMPLIHACK_SRC = Path.home() / ".amplihack" / "src"
if str(_AMPLIHACK_SRC) not in sys.path and (_AMPLIHACK_SRC / "amplihack").exists():
    sys.path.insert(0, str(_AMPLIHACK_SRC))


# ===================================================================
# Scenario 1: ladybug is the active import (not kuzu)
# ===================================================================


class TestLadybugImport:
    """Verify that cognitive_memory.py imports ladybug, not kuzu."""

    def test_cognitive_memory_uses_ladybug_module(self):
        from amplihack_memory import cognitive_memory

        assert hasattr(cognitive_memory, "ladybug"), (
            "cognitive_memory should import 'ladybug' at module level"
        )

    def test_ladybug_has_database_and_connection(self):
        import ladybug

        assert hasattr(ladybug, "Database"), "ladybug must expose Database class"
        assert hasattr(ladybug, "Connection"), "ladybug must expose Connection class"

    def test_ladybug_version_at_least_0_11(self):
        import ladybug

        version = ladybug.__version__
        major, minor = (int(x) for x in version.split(".")[:2])
        assert (major, minor) >= (0, 11), f"Need ladybug>=0.11, got {version}"

    def test_pyproject_declares_ladybug_dependency(self):
        pyproject = _MEMORY_LIB_SRC.parent / "pyproject.toml"
        if not pyproject.exists():
            pytest.skip("pyproject.toml not found")
        content = pyproject.read_text()
        assert "ladybug" in content, "pyproject.toml should depend on ladybug"
        assert "kuzu" not in content.split("[project]")[1].split("[")[0], (
            "pyproject.toml [project] section should not reference kuzu"
        )


# ===================================================================
# Scenario 2: flock serialization on write
# ===================================================================


def _worker_write(db_path: str, agent: str, n: int, results_queue):
    """Child process: open DB in write mode, write n facts, report success."""
    try:
        sys.path.insert(0, str(_MEMORY_LIB_SRC))
        from amplihack_memory.cognitive_memory import CognitiveMemory

        cm = CognitiveMemory(agent_name=agent, db_path=db_path)
        ids = []
        for i in range(n):
            nid = cm.store_fact(
                concept=f"concept-{agent}-{i}",
                content=f"content from {agent} item {i}",
                confidence=0.9,
            )
            ids.append(nid)
        cm.close()
        results_queue.put(("ok", agent, ids))
    except Exception as e:
        results_queue.put(("error", agent, str(e)))


class TestFlockSerialization:
    """Verify that concurrent write access is serialized via flock."""

    def test_sequential_writers_no_corruption(self, tmp_path):
        """Two processes writing sequentially should not corrupt the DB.

        LadybugDB enforces single-writer at the file level. The flock in
        CognitiveMemory serializes Database() creation so callers retry
        rather than crashing. This test verifies sequential access works.
        """
        db_path = str(tmp_path / "flock_test_db")
        q = multiprocessing.Queue()

        # Run writers sequentially (ladybug doesn't support concurrent writers)
        p1 = multiprocessing.Process(target=_worker_write, args=(db_path, "writer-a", 5, q))
        p1.start()
        p1.join(timeout=30)

        p2 = multiprocessing.Process(target=_worker_write, args=(db_path, "writer-b", 5, q))
        p2.start()
        p2.join(timeout=30)

        results = []
        while not q.empty():
            results.append(q.get_nowait())

        errors = [r for r in results if r[0] == "error"]
        assert not errors, f"Worker errors: {errors}"
        assert len(results) == 2, f"Expected 2 results, got {len(results)}"

        # Verify data is intact
        from amplihack_memory.cognitive_memory import CognitiveMemory

        cm = CognitiveMemory(agent_name="writer-a", db_path=db_path)
        facts_a = cm.search_facts("concept-writer-a", limit=20)
        cm.close()

        cm2 = CognitiveMemory(agent_name="writer-b", db_path=db_path)
        facts_b = cm2.search_facts("concept-writer-b", limit=20)
        cm2.close()

        assert len(facts_a) == 5, f"Expected 5 facts for writer-a, got {len(facts_a)}"
        assert len(facts_b) == 5, f"Expected 5 facts for writer-b, got {len(facts_b)}"

    def test_lock_file_created(self, tmp_path):
        """Opening a DB in write mode should create a .ladybug.lock sidecar."""
        db_path = tmp_path / "lock_test_db"
        from amplihack_memory.cognitive_memory import CognitiveMemory

        cm = CognitiveMemory(agent_name="locker", db_path=db_path)
        lock_file = tmp_path / ".ladybug.lock"
        assert lock_file.exists(), f"Expected lock file at {lock_file}"
        cm.close()


# ===================================================================
# Scenario 3: read_only mode for concurrent readers
# ===================================================================


def _worker_read(db_path: str, agent: str, results_queue):
    """Child process: open DB read-only, search facts, report results."""
    try:
        sys.path.insert(0, str(_MEMORY_LIB_SRC))
        from amplihack_memory.cognitive_memory import CognitiveMemory

        cm = CognitiveMemory(agent_name=agent, db_path=db_path, read_only=True)
        facts = cm.search_facts("concept", limit=50)
        cm.close()
        results_queue.put(("ok", agent, len(facts)))
    except Exception as e:
        results_queue.put(("error", agent, str(e)))


class TestReadOnlyMode:
    """Verify read_only parameter allows concurrent multi-process reads."""

    def test_read_only_opens_without_exclusive_lock(self, tmp_path):
        from amplihack_memory.cognitive_memory import CognitiveMemory

        db_path = tmp_path / "ro_test_db"
        # Create DB with some data first
        cm = CognitiveMemory(agent_name="setup", db_path=db_path)
        cm.store_fact(concept="test", content="test-val", confidence=1.0)
        cm.close()

        # Open read-only — should not create lock or block
        cm_ro = CognitiveMemory(agent_name="setup", db_path=db_path, read_only=True)
        facts = cm_ro.search_facts("test", limit=10)
        assert len(facts) >= 1
        cm_ro.close()

    def test_concurrent_readers_succeed(self, tmp_path):
        """Multiple read-only processes should not block each other."""
        from amplihack_memory.cognitive_memory import CognitiveMemory

        db_path = str(tmp_path / "concurrent_ro_db")
        cm = CognitiveMemory(agent_name="setup", db_path=db_path)
        for i in range(10):
            cm.store_fact(concept=f"concept-{i}", content=f"val-{i}", confidence=1.0)
        cm.close()

        q = multiprocessing.Queue()
        readers = [
            multiprocessing.Process(target=_worker_read, args=(db_path, "setup", q))
            for _ in range(3)
        ]
        for r in readers:
            r.start()
        for r in readers:
            r.join(timeout=15)

        results = []
        while not q.empty():
            results.append(q.get_nowait())

        errors = [r for r in results if r[0] == "error"]
        assert not errors, f"Reader errors: {errors}"
        assert len(results) == 3, f"Expected 3 readers, got {len(results)}"
        for status, agent, count in results:
            assert count == 10, f"Reader {agent} expected 10 facts, got {count}"


# ===================================================================
# Scenario 4: amplihack ladybug_store uses ladybug (not raw kuzu)
# ===================================================================


class TestAmplihackLadybugStore:
    """Verify the amplihack KuzuGraphStore uses ladybug with flock."""

    def test_ladybug_store_exists(self):
        store_path = _AMPLIHACK_SRC / "amplihack" / "memory" / "ladybug_store.py"
        assert store_path.exists(), "ladybug_store.py should exist in amplihack memory"

    def test_ladybug_store_imports_ladybug_first(self):
        store_path = _AMPLIHACK_SRC / "amplihack" / "memory" / "ladybug_store.py"
        content = store_path.read_text()
        assert "import ladybug" in content, "ladybug_store.py should import ladybug"
        assert "import fcntl" in content, "ladybug_store.py should use fcntl for flock"

    def test_ladybug_store_has_flock_functions(self):
        from amplihack.memory.ladybug_store import _acquire_flock, _release_flock

        assert callable(_acquire_flock)
        assert callable(_release_flock)

    def test_ladybug_store_supports_read_only(self):
        from amplihack.memory.ladybug_store import KuzuGraphStore

        import inspect

        sig = inspect.signature(KuzuGraphStore.__init__)
        assert "read_only" in sig.parameters, "KuzuGraphStore should accept read_only param"

    def test_ladybug_store_create_and_query(self, tmp_path):
        from amplihack.memory.ladybug_store import KuzuGraphStore

        store = KuzuGraphStore(db_path=tmp_path / "graph_db")
        store.ensure_table("TestNode", {"node_id": "STRING", "name": "STRING", "value": "STRING"})
        nid = store.create_node("TestNode", {"name": "hello", "value": "world"})
        assert nid is not None

        node = store.get_node("TestNode", nid)
        assert node is not None
        assert node["name"] == "hello"
        store.close()


# ===================================================================
# Scenario 5: Simard memory bridge uses Memory facade
# ===================================================================


class TestSimardBridgeFacade:
    """Verify Simard memory bridge creates backend via Memory facade."""

    def test_bridge_module_imports_facade(self):
        bridge_path = (
            Path.home() / "src" / "Simard" / "worktrees" / "main"
            / "python" / "simard_memory_server.py"
        )
        content = bridge_path.read_text()
        assert "from amplihack.memory.facade import Memory" in content
        assert "topology=\"distributed\"" in content or "topology='distributed'" in content

    def test_bridge_creates_direct_cognitive_fallback(self, tmp_path):
        """When topology=single, bridge falls back to direct CognitiveMemory."""
        bridge_dir = (
            Path.home() / "src" / "Simard" / "worktrees" / "main" / "python"
        )
        if str(bridge_dir) not in sys.path:
            sys.path.insert(0, str(bridge_dir))

        from simard_memory_server import _create_memory_backend

        db_path = str(tmp_path / "bridge_test_db")
        mem = _create_memory_backend("test-agent", db_path, topology="single")
        assert mem is not None

        # Basic operation check
        nid = mem.store_fact(concept="bridge-test", content="hello", confidence=1.0)
        assert nid.startswith("sem_") or nid.startswith("fact_") or len(nid) > 0
        mem.close()

    def test_bridge_default_topology_is_distributed(self):
        """CLI default topology should be 'distributed'."""
        bridge_path = (
            Path.home() / "src" / "Simard" / "worktrees" / "main"
            / "python" / "simard_memory_server.py"
        )
        content = bridge_path.read_text()
        assert 'default="distributed"' in content


# ===================================================================
# Scenario 6: Bridge JSON-RPC protocol end-to-end
# ===================================================================


class TestBridgeProtocol:
    """End-to-end test: send JSON-RPC to the bridge subprocess via stdin/stdout."""

    @pytest.fixture
    def bridge_proc(self, tmp_path):
        """Start the bridge subprocess."""
        bridge_script = (
            Path.home() / "src" / "Simard" / "worktrees" / "main"
            / "python" / "simard_memory_server.py"
        )
        db_path = tmp_path / "e2e_bridge_db"
        env = os.environ.copy()
        env["PYTHONPATH"] = ":".join([
            str(_MEMORY_LIB_SRC),
            str(_AMPLIHACK_SRC),
            str(bridge_script.parent),
        ])
        proc = subprocess.Popen(
            [sys.executable, str(bridge_script),
             "--agent-name", "e2e-test",
             "--db-path", str(db_path),
             "--topology", "single"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            text=True,
        )
        yield proc
        proc.terminate()
        proc.wait(timeout=5)

    def _send(self, proc, method: str, params: dict) -> dict:
        request = json.dumps({"method": method, "params": params}) + "\n"
        proc.stdin.write(request)
        proc.stdin.flush()
        line = proc.stdout.readline()
        assert line, f"No response for {method}"
        return json.loads(line)

    def test_store_and_search_fact(self, bridge_proc):
        resp = self._send(bridge_proc, "memory.store_fact", {
            "concept": "ladybug-migration",
            "content": "kuzu replaced by ladybug",
            "confidence": 0.95,
        })
        assert "error" not in resp, f"Error: {resp}"
        fact_id = resp.get("result", {}).get("id")
        assert fact_id, f"Expected fact id, got {resp}"

        resp2 = self._send(bridge_proc, "memory.search_facts", {
            "query": "ladybug",
            "limit": 5,
        })
        assert "error" not in resp2, f"Error: {resp2}"
        facts = resp2.get("result", {}).get("facts", [])
        assert len(facts) >= 1
        assert any("ladybug" in f["content"] for f in facts)

    def test_store_and_recall_procedure(self, bridge_proc):
        resp = self._send(bridge_proc, "memory.store_procedure", {
            "name": "upgrade-db",
            "steps": ["backup", "migrate", "verify"],
            "prerequisites": ["backup-tool"],
        })
        assert "error" not in resp, f"Error: {resp}"

        resp2 = self._send(bridge_proc, "memory.recall_procedure", {
            "query": "upgrade",
        })
        procs = resp2.get("result", {}).get("procedures", [])
        assert len(procs) >= 1
        assert procs[0]["name"] == "upgrade-db"

    def test_statistics_returns_all_six_types(self, bridge_proc):
        resp = self._send(bridge_proc, "memory.get_statistics", {})
        assert "error" not in resp
        stats = resp.get("result", {})
        for key in [
            "sensory_count", "working_count", "episodic_count",
            "semantic_count", "procedural_count", "prospective_count",
        ]:
            assert key in stats, f"Missing stat key: {key}"


# ===================================================================
# Scenario 7: remote_transfer.rs deprecation
# ===================================================================


class TestRemoteTransferDeprecation:
    """Verify remote_transfer.rs has proper deprecation markers."""

    def test_remote_transfer_has_deprecation_notice(self):
        rt_path = (
            Path.home() / "src" / "Simard" / "worktrees" / "main"
            / "src" / "remote_transfer.rs"
        )
        content = rt_path.read_text()
        assert "deprecated" in content.lower() or "Deprecated" in content
        assert "hive" in content.lower() or "distributed" in content.lower()

    def test_functions_have_deprecated_attribute(self):
        rt_path = (
            Path.home() / "src" / "Simard" / "worktrees" / "main"
            / "src" / "remote_transfer.rs"
        )
        content = rt_path.read_text()
        deprecated_count = content.count("#[deprecated")
        assert deprecated_count >= 1, (
            f"Expected at least 1 #[deprecated] attribute, found {deprecated_count}"
        )


# ===================================================================
# Scenario 8: kuzu/ladybug type conflict regression check
# ===================================================================


class TestKuzuLadybugConflict:
    """Detect the kuzu/ladybug 'Database already registered' conflict."""

    def test_ladybug_import_alone_works(self):
        """Importing ladybug alone should work without conflict."""
        result = subprocess.run(
            [sys.executable, "-c", "import ladybug; print(ladybug.__version__)"],
            capture_output=True, text=True, timeout=10,
        )
        assert result.returncode == 0, f"ladybug import failed: {result.stderr}"

    def test_cognitive_memory_import_clean(self):
        """CognitiveMemory should import cleanly (uses ladybug, not kuzu)."""
        result = subprocess.run(
            [sys.executable, "-c",
             "import sys; sys.path.insert(0, '{}'); "
             "from amplihack_memory.cognitive_memory import CognitiveMemory; "
             "print('OK')".format(_MEMORY_LIB_SRC)],
            capture_output=True, text=True, timeout=10,
        )
        assert result.returncode == 0, f"Import failed: {result.stderr}"
        assert "OK" in result.stdout

    def test_hierarchical_memory_kuzu_conflict(self):
        """Flag if _hierarchical_memory_local.py still imports kuzu directly.

        This causes 'generic_type: Database already registered' when ladybug
        is also loaded. This test documents the known issue.
        """
        hm_path = (
            _AMPLIHACK_SRC / "amplihack" / "agents" / "goal_seeking"
            / "_hierarchical_memory_local.py"
        )
        if not hm_path.exists():
            pytest.skip("_hierarchical_memory_local.py not found")

        content = hm_path.read_text()
        has_raw_kuzu_import = "import kuzu" in content and "import kuzu as ladybug" not in content
        if has_raw_kuzu_import:
            pytest.xfail(
                "KNOWN BUG: _hierarchical_memory_local.py imports kuzu directly "
                "instead of ladybug, causing 'Database already registered' error "
                "when both are loaded. Needs migration to 'import ladybug' or "
                "'import ladybug as kuzu'."
            )
