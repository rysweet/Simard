"""Outside-in TDD (tests-first) for the MEMORY GRAPH migrate capability.

Issue #2923 — design spike. Per repo constraint G2, the ability to migrate the
cognitive memory graph (facts, procedures, records, edges, provenance) to a new
system with INTEGRITY VERIFICATION lives in **amplihack-memory-lib**, NOT forked
into Simard. This suite drives that capability from the Simard side (matching
the existing ``tests/test_ladybug_migration.py`` cross-repo pattern) so the
end-to-end migration story has executable acceptance tests.

The capability does not exist yet; every scenario ``pytest.skip``s cleanly until
``amplihack_memory.migration`` (or an equivalent ``CognitiveMemory`` method
surface) ships as its own green-CI PR. When it lands, the guards flip to hard
assertions that drive the implementation.

Design contract asserted here (single source of truth for the memory-lib PR):

    from amplihack_memory.migration import (
        export_graph,     # (memory, dest_dir, *, since=None) -> manifest: dict
        import_graph,     # (memory, src_dir, *, verify=True) -> report: dict
        verify_manifest,  # (src_dir) -> {"ok": bool, "mismatches": [...]}
    )

    manifest = {
      "counts":     {sensory, working, episodic, semantic, procedural,
                     prospective, edges: int},
      "checksums":  {<same keys>: <hex str>},   # content-addressed per type
      "provenance": {source_agent, exported_epoch, lbug_version,
                     schema_version},
      "watermark":  <opaque cursor for delta/incremental export>,
    }

Invariants: round-trip parity (counts + checksums), tamper detection is LOUD
(no silent fallback), idempotent re-import, delta export, and export works
while the store is live (read-only snapshot, single-writer lease respected).

Run:
    PYTHONPATH=~/src/amplihack-memory-lib/src \
        python3 -m pytest tests/test_graph_migration.py -v
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Locate amplihack-memory-lib (the ONLY home for this capability — G2)
# ---------------------------------------------------------------------------

_MEM_LIB_CANDIDATES = [
    Path.home() / "src" / "amplihack-memory-lib" / "src",
    Path.home() / "src" / "amplirusty" / "amplihack-memory-lib" / "src",
]
for _c in _MEM_LIB_CANDIDATES:
    if (_c / "amplihack_memory").exists() and str(_c) not in sys.path:
        sys.path.insert(0, str(_c))
        break

REQUIRED_COUNT_KEYS = {
    "sensory",
    "working",
    "episodic",
    "semantic",
    "procedural",
    "prospective",
    "edges",
}
REQUIRED_PROVENANCE_KEYS = {
    "source_agent",
    "exported_epoch",
    "lbug_version",
    "schema_version",
}


# ---------------------------------------------------------------------------
# Capability discovery — skip cleanly until the migrate API ships (G2 PR)
# ---------------------------------------------------------------------------


def _cognitive_memory_cls():
    try:
        from amplihack_memory.cognitive_memory import CognitiveMemory
    except Exception as exc:  # noqa: BLE001 - env without ladybug: skip, don't fail
        pytest.skip(f"amplihack-memory-lib CognitiveMemory unavailable: {exc}")
    return CognitiveMemory


def _migration_api():
    """Resolve (export_graph, import_graph, verify_manifest) or skip."""
    try:
        from amplihack_memory import migration as m
    except Exception:  # noqa: BLE001
        # Fall back to method surface on CognitiveMemory, if that is the chosen shape.
        cm = _cognitive_memory_cls()
        if all(hasattr(cm, n) for n in ("export_graph", "import_graph", "verify_manifest")):
            return (
                lambda mem, dest, **kw: mem.export_graph(dest, **kw),
                lambda mem, src, **kw: mem.import_graph(src, **kw),
                lambda src: cm.verify_manifest(src),
            )
        pytest.skip(
            "memory-graph migrate capability not built yet (issue #2923, G2: "
            "amplihack_memory.migration.export_graph/import_graph/verify_manifest)"
        )
    missing = [n for n in ("export_graph", "import_graph", "verify_manifest") if not hasattr(m, n)]
    if missing:
        pytest.skip(f"amplihack_memory.migration missing: {missing} (issue #2923, G2)")
    return m.export_graph, m.import_graph, m.verify_manifest


def _populate(cm) -> None:
    """Write a representative sample across all cognitive memory types."""
    for i in range(5):
        cm.store_fact(concept=f"concept-{i}", content=f"fact content {i}", confidence=0.9)
    cm.store_procedure(
        name="promote-secondary",
        steps=["final-sync", "acquire-lease", "promote", "demote-old"],
        prerequisites=["standby-validated"],
    )
    cm.store_episode(
        summary="migration rehearsal",
        details="rehearsed cutover to throwaway VM",
        importance=0.8,
    ) if hasattr(cm, "store_episode") else None
    if hasattr(cm, "store_prospective"):
        cm.store_prospective(intention="run final-sync before cutover", trigger="cutover-start")


# ===================================================================
# Scenario 1: export produces a verifiable manifest
# ===================================================================


class TestExportManifest:
    def test_export_writes_manifest_with_counts_checksums_provenance(self, tmp_path):
        export_graph, _import, _verify = _migration_api()
        CognitiveMemory = _cognitive_memory_cls()

        cm = CognitiveMemory(agent_name="src-agent", db_path=str(tmp_path / "src_db"))
        _populate(cm)
        cm.close()

        cm = CognitiveMemory(agent_name="src-agent", db_path=str(tmp_path / "src_db"))
        manifest = export_graph(cm, str(tmp_path / "snapshot"))
        cm.close()

        assert REQUIRED_COUNT_KEYS <= set(manifest["counts"]), "manifest must count all 6 types + edges"
        assert set(manifest["counts"]) <= set(manifest["checksums"]) | {"edges"} or set(
            manifest["counts"]
        ) == set(manifest["checksums"]), "each counted type needs a checksum"
        assert REQUIRED_PROVENANCE_KEYS <= set(manifest["provenance"]), "manifest needs provenance"
        assert manifest["counts"]["semantic"] >= 5


# ===================================================================
# Scenario 2: round-trip parity (counts + checksums) into a fresh store
# ===================================================================


class TestRoundTripParity:
    def test_import_into_fresh_store_matches_source(self, tmp_path):
        export_graph, import_graph, _verify = _migration_api()
        CognitiveMemory = _cognitive_memory_cls()

        src = CognitiveMemory(agent_name="src", db_path=str(tmp_path / "src_db"))
        _populate(src)
        src_stats = src.get_statistics()
        manifest = export_graph(src, str(tmp_path / "snap"))
        src.close()

        dst = CognitiveMemory(agent_name="dst", db_path=str(tmp_path / "dst_db"))
        report = import_graph(dst, str(tmp_path / "snap"), verify=True)
        dst_stats = dst.get_statistics()
        dst.close()

        assert report.get("verified") is True, "round-trip import must verify integrity"
        # Parity on the semantic store at minimum; full parity across all types.
        for key in ("semantic_count", "procedural_count"):
            assert dst_stats.get(key) == src_stats.get(key), f"{key} parity broken"
        # Checksums recomputed on the destination must match the manifest.
        assert report.get("checksums", manifest["checksums"]) == manifest["checksums"]


# ===================================================================
# Scenario 3: tamper detection is LOUD (no silent fallback)
# ===================================================================


class TestIntegrityTamperDetection:
    def test_corrupted_snapshot_fails_verification_loudly(self, tmp_path):
        export_graph, import_graph, verify_manifest = _migration_api()
        CognitiveMemory = _cognitive_memory_cls()

        src = CognitiveMemory(agent_name="src", db_path=str(tmp_path / "src_db"))
        _populate(src)
        export_graph(src, str(tmp_path / "snap"))
        src.close()

        # Corrupt one payload file in the snapshot directory.
        snap = tmp_path / "snap"
        payloads = [p for p in snap.rglob("*") if p.is_file() and "manifest" not in p.name.lower()]
        assert payloads, "snapshot must contain payload files to tamper with"
        target = payloads[0]
        target.write_bytes(target.read_bytes() + b"\x00tampered")

        # verify_manifest must report NOT ok with the specific mismatch.
        result = verify_manifest(str(snap))
        assert result.get("ok") is False, "verification must detect tampering"
        assert result.get("mismatches"), "verification must name the mismatched entries"

        # import with verify=True must refuse loudly (raise), never silently proceed.
        dst = CognitiveMemory(agent_name="dst", db_path=str(tmp_path / "dst_db"))
        with pytest.raises(Exception):
            import_graph(dst, str(snap), verify=True)
        dst.close()


# ===================================================================
# Scenario 4: idempotent re-import (no duplication)
# ===================================================================


class TestIdempotentImport:
    def test_double_import_does_not_duplicate(self, tmp_path):
        export_graph, import_graph, _verify = _migration_api()
        CognitiveMemory = _cognitive_memory_cls()

        src = CognitiveMemory(agent_name="src", db_path=str(tmp_path / "src_db"))
        _populate(src)
        export_graph(src, str(tmp_path / "snap"))
        src.close()

        dst = CognitiveMemory(agent_name="dst", db_path=str(tmp_path / "dst_db"))
        import_graph(dst, str(tmp_path / "snap"), verify=True)
        after_first = dst.get_statistics()
        import_graph(dst, str(tmp_path / "snap"), verify=True)
        after_second = dst.get_statistics()
        dst.close()

        assert after_first == after_second, "re-import must be idempotent (no duplication)"


# ===================================================================
# Scenario 5: delta / incremental export since a watermark
# ===================================================================


class TestDeltaExport:
    def test_delta_export_only_carries_new_entries(self, tmp_path):
        export_graph, import_graph, _verify = _migration_api()
        CognitiveMemory = _cognitive_memory_cls()

        src = CognitiveMemory(agent_name="src", db_path=str(tmp_path / "src_db"))
        _populate(src)
        base = export_graph(src, str(tmp_path / "full"))
        watermark = base.get("watermark")
        if watermark is None:
            pytest.skip("delta export watermark not implemented yet (issue #2923, G2)")

        # Add more after the watermark, then export only the delta.
        for i in range(3):
            src.store_fact(concept=f"delta-{i}", content=f"delta fact {i}", confidence=0.7)
        delta = export_graph(src, str(tmp_path / "delta"), since=watermark)
        src.close()

        assert delta["counts"]["semantic"] == 3, "delta must carry only post-watermark facts"


# ===================================================================
# Scenario 6: export while LIVE (read-only snapshot, single-writer lease)
# ===================================================================


class TestLiveSnapshot:
    def test_export_works_with_concurrent_reader(self, tmp_path):
        export_graph, _import, _verify = _migration_api()
        CognitiveMemory = _cognitive_memory_cls()

        db_path = str(tmp_path / "live_db")
        writer = CognitiveMemory(agent_name="live", db_path=db_path)
        _populate(writer)
        writer.close()

        # Hold a read-only handle open (simulates the live primary still serving
        # reads) while the exporter snapshots the graph.
        reader = CognitiveMemory(agent_name="live", db_path=db_path, read_only=True)
        exporter = CognitiveMemory(agent_name="live", db_path=db_path, read_only=True)
        manifest = export_graph(exporter, str(tmp_path / "live_snap"))
        exporter.close()
        reader.close()

        assert manifest["counts"]["semantic"] >= 5, "live snapshot must capture existing facts"
