"""Tests for python/simard_gym_bridge.py and python/simard_knowledge_bridge.py.

Tests the bridge servers in degraded mode (missing amplihack.eval / wikigr deps)
and unit-tests pure helper functions that have no external dependencies.

Run with:
    uv run --with pytest python3 -m pytest tests/test_bridges.py -v
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from types import ModuleType
from unittest.mock import MagicMock, patch

import pytest

# Ensure python/ directory is importable
_PYTHON_DIR = Path(__file__).parent.parent / "python"
if str(_PYTHON_DIR) not in sys.path:
    sys.path.insert(0, str(_PYTHON_DIR))


# ---------------------------------------------------------------------------
# simard_gym_bridge — helper functions (no external deps)
# ---------------------------------------------------------------------------


import simard_gym_bridge as gym_bridge


class TestGymHelpers:
    def test_zero_dims_returns_all_five_dimensions(self):
        dims = gym_bridge._zero_dims()
        expected = {
            "factual_accuracy", "specificity", "temporal_awareness",
            "source_attribution", "confidence_calibration",
        }
        assert set(dims.keys()) == expected

    def test_zero_dims_all_values_are_zero(self):
        dims = gym_bridge._zero_dims()
        for val in dims.values():
            assert val == 0.0

    def test_fail_result_returns_failure_shape(self):
        result = gym_bridge._fail_result("my-scenario", "something broke", "source-a")
        assert result["scenario_id"] == "my-scenario"
        assert result["success"] is False
        assert result["score"] == 0.0
        assert result["error_message"] == "something broke"
        assert result["degraded_sources"] == ["source-a"]
        assert result["question_count"] == 0
        assert result["questions_answered"] == 0

    def test_fail_result_without_source(self):
        result = gym_bridge._fail_result("sid", "err")
        assert result["degraded_sources"] == []

    def test_fail_result_dimensions_are_all_zero(self):
        result = gym_bridge._fail_result("sid", "err")
        for val in result["dimensions"].values():
            assert val == 0.0


# ---------------------------------------------------------------------------
# GymBridgeServer — degraded mode (no amplihack.eval import)
# ---------------------------------------------------------------------------


class TestGymBridgeServerDegradedMode:
    """Test GymBridgeServer when progressive and long_horizon are unavailable."""

    def _make_server(self) -> gym_bridge.GymBridgeServer:
        """Create a GymBridgeServer with both imports patched to None."""
        with (
            patch.object(gym_bridge, "_try_import_progressive", return_value=(None, "not installed")),
            patch.object(gym_bridge, "_try_import_long_horizon", return_value=(None, "not installed")),
        ):
            return gym_bridge.GymBridgeServer()

    def test_server_initializes_in_degraded_mode(self):
        server = self._make_server()
        assert server._progressive is None
        assert server._long_horizon is None

    def test_list_scenarios_returns_empty_list_in_degraded_mode(self):
        server = self._make_server()
        result = server.dispatch("gym.list_scenarios", {})
        assert result == []

    def test_run_scenario_returns_failure_when_no_progressive(self):
        server = self._make_server()
        result = server.dispatch("gym.run_scenario", {"scenario_id": "level-1"})
        assert result["success"] is False
        assert "progressive" in result["error_message"].lower() or "unavailable" in result["error_message"].lower()

    def test_run_suite_returns_failure_when_no_progressive(self):
        server = self._make_server()
        result = server.dispatch("gym.run_suite", {"suite_id": "progressive"})
        assert result["success"] is False
        assert result["scenarios_total"] == 0
        assert result["overall_score"] == 0.0

    def test_run_suite_reports_degraded_source(self):
        server = self._make_server()
        result = server.dispatch("gym.run_suite", {})
        assert "progressive_test_suite" in result["degraded_sources"]

    def test_run_long_horizon_returns_failure_when_unavailable(self):
        server = self._make_server()
        result = server._run_long_horizon()
        assert result["success"] is False
        assert "unavailable" in result["error_message"].lower()

    def test_path_traversal_rejected(self):
        server = self._make_server()
        for bad_id in ["../etc/passwd", "foo/bar", "..\\etc"]:
            result = server.dispatch("gym.run_scenario", {"scenario_id": bad_id})
            assert result["success"] is False
            assert "illegal path" in result["error_message"]

    def test_health_check_available(self):
        server = self._make_server()
        result = server.dispatch("bridge.health", {})
        assert result["healthy"] is True

    def test_gym_methods_are_registered(self):
        server = self._make_server()
        for method in ("gym.list_scenarios", "gym.run_scenario", "gym.run_suite"):
            assert method in server._handlers


# ---------------------------------------------------------------------------
# GymBridgeServer — list_scenarios with long_horizon available
# ---------------------------------------------------------------------------


class TestGymBridgeServerWithLongHorizon:
    def _make_server_with_lh(self) -> gym_bridge.GymBridgeServer:
        mock_lh = {"LongHorizonMemoryEval": MagicMock()}
        with (
            patch.object(gym_bridge, "_try_import_progressive", return_value=(None, "not installed")),
            patch.object(gym_bridge, "_try_import_long_horizon", return_value=(mock_lh, None)),
        ):
            return gym_bridge.GymBridgeServer()

    def test_list_scenarios_includes_long_horizon(self):
        server = self._make_server_with_lh()
        result = server.dispatch("gym.list_scenarios", {})
        ids = [s["id"] for s in result]
        assert "long-horizon-memory" in ids

    def test_long_horizon_scenario_shape(self):
        server = self._make_server_with_lh()
        result = server.dispatch("gym.list_scenarios", {})
        lh = next(s for s in result if s["id"] == "long-horizon-memory")
        assert "name" in lh
        assert "description" in lh
        assert "level" in lh


# ---------------------------------------------------------------------------
# simard_knowledge_bridge — helper functions
# ---------------------------------------------------------------------------


import simard_knowledge_bridge as kb


class TestKnowledgeBridgeHelpers:
    def test_extract_sources_string_list(self):
        result = kb._extract_sources({"sources": ["Title A", "Title B"]})
        assert len(result) == 2
        assert result[0] == {"title": "Title A", "section": ""}
        assert result[1] == {"title": "Title B", "section": ""}

    def test_extract_sources_dict_list(self):
        raw = [{"title": "Art", "section": "Intro", "url": "http://x.com"}]
        result = kb._extract_sources({"sources": raw})
        assert result[0]["title"] == "Art"
        assert result[0]["section"] == "Intro"
        assert result[0]["url"] == "http://x.com"

    def test_extract_sources_empty_list(self):
        assert kb._extract_sources({}) == []

    def test_extract_sources_mixed_dict_with_name(self):
        raw = [{"name": "By Name", "section": ""}]
        result = kb._extract_sources({"sources": raw})
        assert result[0]["title"] == "By Name"

    def test_estimate_confidence_training_only(self):
        result = kb._estimate_confidence({
            "query_type": "training_only_response",
            "answer": "some answer",
            "sources": ["A", "B"],
        })
        assert result == 0.2

    def test_estimate_confidence_no_sources(self):
        conf = kb._estimate_confidence({"answer": "x", "sources": []})
        assert conf == 0.3

    def test_estimate_confidence_no_answer(self):
        conf = kb._estimate_confidence({"answer": "", "sources": ["A"]})
        assert conf == 0.1

    def test_estimate_confidence_with_sources_and_answer(self):
        conf = kb._estimate_confidence({
            "answer": "A" * 200,
            "sources": ["A", "B", "C", "D", "E"],
        })
        assert 0.0 < conf <= 1.0
        assert conf > 0.3  # should be higher than no-sources baseline

    def test_estimate_confidence_returns_rounded_float(self):
        conf = kb._estimate_confidence({"answer": "hi", "sources": ["A"]})
        # Round to 2 decimal places
        assert conf == round(conf, 2)

    def test_section_count_uses_entities(self):
        stats = MagicMock()
        stats.entities = 42
        assert kb._section_count(stats) == 42


# ---------------------------------------------------------------------------
# KnowledgeBridgeServer — initialization
# ---------------------------------------------------------------------------


class TestKnowledgeBridgeServerInit:
    def test_server_name_is_simard_knowledge(self):
        server = kb.KnowledgeBridgeServer()
        assert server.server_name == "simard-knowledge"

    def test_knowledge_methods_registered(self):
        server = kb.KnowledgeBridgeServer()
        for method in ("knowledge.query", "knowledge.list_packs", "knowledge.pack_info"):
            assert method in server._handlers

    def test_default_packs_dir(self):
        server = kb.KnowledgeBridgeServer()
        assert server._packs_dir == Path.home() / ".wikigr/packs"

    def test_custom_packs_dir(self):
        custom = Path("/tmp/my-packs")
        server = kb.KnowledgeBridgeServer(packs_dir=custom)
        assert server._packs_dir == custom

    def test_registry_starts_none(self):
        server = kb.KnowledgeBridgeServer()
        assert server._registry is None


# ---------------------------------------------------------------------------
# KnowledgeBridgeServer — handler validation (no wikigr installed)
# ---------------------------------------------------------------------------


class TestKnowledgeBridgeServerHandlers:
    def _make_server(self) -> kb.KnowledgeBridgeServer:
        return kb.KnowledgeBridgeServer(packs_dir=Path("/nonexistent"))

    def test_query_missing_pack_name_raises_value_error(self):
        server = self._make_server()
        with pytest.raises(ValueError, match="pack_name"):
            server.dispatch("knowledge.query", {"question": "hello"})

    def test_query_empty_question_returns_placeholder(self):
        server = self._make_server()
        # patch _get_agent so we don't hit wikigr
        server._get_agent = MagicMock()
        result = server.dispatch("knowledge.query", {"pack_name": "my-pack", "question": ""})
        assert "answer" in result
        assert result["confidence"] == 0.0
        assert result["sources"] == []

    def test_list_packs_raises_runtime_when_no_wikigr(self):
        server = self._make_server()
        with pytest.raises(RuntimeError, match="agent-kgpacks"):
            server.dispatch("knowledge.list_packs", {})

    def test_pack_info_missing_pack_name_raises_value_error(self):
        server = self._make_server()
        with pytest.raises(ValueError, match="pack_name"):
            server.dispatch("knowledge.pack_info", {})

    def test_pack_info_missing_pack_raises_runtime_when_no_wikigr(self):
        server = self._make_server()
        with pytest.raises(RuntimeError, match="agent-kgpacks"):
            server.dispatch("knowledge.pack_info", {"pack_name": "my-pack"})

    def test_health_check_available(self):
        server = self._make_server()
        result = server.dispatch("bridge.health", {})
        assert result["healthy"] is True

    def test_query_with_mock_agent(self):
        server = self._make_server()
        mock_agent = MagicMock()
        mock_agent.query.return_value = {
            "answer": "Paris is the capital of France.",
            "sources": ["France article"],
            "query_type": "factual",
        }
        server._get_agent = MagicMock(return_value=mock_agent)
        result = server.dispatch("knowledge.query", {
            "pack_name": "world-facts",
            "question": "What is the capital of France?",
            "limit": 3,
        })
        assert result["answer"] == "Paris is the capital of France."
        assert isinstance(result["confidence"], float)
        assert len(result["sources"]) <= 3

    def test_query_limits_sources_to_limit_param(self):
        server = self._make_server()
        mock_agent = MagicMock()
        mock_agent.query.return_value = {
            "answer": "answer",
            "sources": [f"src-{i}" for i in range(10)],
        }
        server._get_agent = MagicMock(return_value=mock_agent)
        result = server.dispatch("knowledge.query", {
            "pack_name": "pack",
            "question": "q",
            "limit": 3,
        })
        assert len(result["sources"]) <= 3
