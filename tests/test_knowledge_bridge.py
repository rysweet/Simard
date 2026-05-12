"""Unit tests for python/simard_knowledge_bridge.py.

Uses mocks for the wikigr package (not installed in CI) to test all handler
paths, helper functions, and error conditions without real knowledge packs.

Run with:
    pipx run pytest tests/test_knowledge_bridge.py -v
"""

from __future__ import annotations

import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any
from unittest.mock import MagicMock, patch

import pytest

# ---------------------------------------------------------------------------
# Make python/ importable
# ---------------------------------------------------------------------------

_PYTHON_DIR = Path(__file__).resolve().parent.parent / "python"
if str(_PYTHON_DIR) not in sys.path:
    sys.path.insert(0, str(_PYTHON_DIR))

# ---------------------------------------------------------------------------
# Stub out wikigr so the module can import without the real package
# ---------------------------------------------------------------------------

_wikigr_packs = ModuleType("wikigr.packs")
_wikigr_packs_registry = ModuleType("wikigr.packs.registry")
_wikigr_agent = ModuleType("wikigr.agent")
_wikigr_agent_kg = ModuleType("wikigr.agent.kg_agent")
_wikigr = ModuleType("wikigr")

_wikigr_stubs = {
    "wikigr": _wikigr,
    "wikigr.packs": _wikigr_packs,
    "wikigr.packs.registry": _wikigr_packs_registry,
    "wikigr.agent": _wikigr_agent,
    "wikigr.agent.kg_agent": _wikigr_agent_kg,
}

for _name, _mod in _wikigr_stubs.items():
    sys.modules.setdefault(_name, _mod)

from simard_knowledge_bridge import (  # noqa: E402
    KnowledgeBridgeServer,
    _estimate_confidence,
    _extract_sources,
    _pack_info_dict,
    _section_count,
)


# ===========================================================================
# Fixtures
# ===========================================================================


def _make_stats(articles: int = 10, entities: int = 30) -> SimpleNamespace:
    return SimpleNamespace(articles=articles, entities=entities)


def _make_manifest(
    name: str = "test-pack",
    description: str = "A test pack",
    articles: int = 10,
    entities: int = 30,
) -> SimpleNamespace:
    return SimpleNamespace(
        name=name,
        description=description,
        graph_stats=_make_stats(articles=articles, entities=entities),
    )


def _make_pack(
    name: str = "test-pack",
    description: str = "A test pack",
    articles: int = 10,
    entities: int = 30,
    db_exists: bool = True,
    pack_path: Path | None = None,
) -> SimpleNamespace:
    path = pack_path or Path("/fake/packs") / name
    db_path = path / "pack.db"
    pack = SimpleNamespace(
        manifest=_make_manifest(name=name, description=description,
                                articles=articles, entities=entities),
        path=path,
    )
    return pack


def _make_registry(packs: list = None) -> MagicMock:
    registry = MagicMock()
    packs = packs or []
    registry.list_packs.return_value = packs
    registry.get_pack.side_effect = lambda name: next(
        (p for p in packs if p.manifest.name == name), None
    )
    return registry


def _make_server(packs: list = None, packs_dir: Path = None) -> KnowledgeBridgeServer:
    """Build a KnowledgeBridgeServer with a mocked registry."""
    srv = KnowledgeBridgeServer(packs_dir=packs_dir or Path("/fake/packs"))
    registry = _make_registry(packs or [])
    srv._registry = registry
    return srv


# ===========================================================================
# KnowledgeBridgeServer — construction
# ===========================================================================


class TestKnowledgeBridgeServerConstruction:
    def test_default_packs_dir_is_home_wikigr(self):
        srv = KnowledgeBridgeServer()
        assert srv._packs_dir == Path.home() / ".wikigr/packs"

    def test_custom_packs_dir_stored(self, tmp_path):
        srv = KnowledgeBridgeServer(packs_dir=tmp_path)
        assert srv._packs_dir == tmp_path

    def test_server_name_is_simard_knowledge(self):
        srv = KnowledgeBridgeServer()
        assert srv.server_name == "simard-knowledge"

    def test_required_methods_registered(self):
        srv = KnowledgeBridgeServer()
        assert "knowledge.query" in srv._handlers
        assert "knowledge.list_packs" in srv._handlers
        assert "knowledge.pack_info" in srv._handlers

    def test_health_method_still_registered(self):
        srv = KnowledgeBridgeServer()
        result = srv.dispatch("bridge.health", {})
        assert result["healthy"] is True

    def test_registry_starts_as_none(self):
        srv = KnowledgeBridgeServer()
        assert srv._registry is None

    def test_agents_cache_starts_empty(self):
        srv = KnowledgeBridgeServer()
        assert srv._agents == {}


# ===========================================================================
# _ensure_registry — lazy init
# ===========================================================================


class TestEnsureRegistry:
    def test_skips_if_registry_already_set(self):
        srv = KnowledgeBridgeServer(packs_dir=Path("/p"))
        mock_reg = MagicMock()
        srv._registry = mock_reg
        srv._ensure_registry()
        assert srv._registry is mock_reg  # not replaced

    def test_raises_if_wikigr_not_importable(self):
        srv = KnowledgeBridgeServer(packs_dir=Path("/p"))
        with patch.dict(sys.modules, {"wikigr.packs.registry": None}):
            with pytest.raises((RuntimeError, ImportError)):
                srv._ensure_registry()

    def test_creates_registry_with_pack_registry(self):
        srv = KnowledgeBridgeServer(packs_dir=Path("/p"))
        mock_cls = MagicMock(return_value=MagicMock())
        with patch("wikigr.packs.registry.PackRegistry", mock_cls, create=True):
            import wikigr.packs.registry as reg_mod
            reg_mod.PackRegistry = mock_cls
            srv._ensure_registry()
        assert srv._registry is not None


# ===========================================================================
# knowledge.list_packs
# ===========================================================================


class TestHandleListPacks:
    def test_returns_empty_list_when_no_packs(self):
        srv = _make_server(packs=[])
        result = srv.dispatch("knowledge.list_packs", {})
        assert result == {"packs": []}

    def test_returns_one_pack(self):
        pack = _make_pack("wiki", "Wikipedia mirror", articles=100, entities=500)
        srv = _make_server(packs=[pack])
        result = srv.dispatch("knowledge.list_packs", {})
        assert len(result["packs"]) == 1
        info = result["packs"][0]
        assert info["name"] == "wiki"
        assert info["description"] == "Wikipedia mirror"
        assert info["article_count"] == 100
        assert info["section_count"] == 500

    def test_returns_multiple_packs(self):
        packs = [
            _make_pack("a", "Pack A", articles=5, entities=10),
            _make_pack("b", "Pack B", articles=7, entities=14),
        ]
        srv = _make_server(packs=packs)
        result = srv.dispatch("knowledge.list_packs", {})
        names = {p["name"] for p in result["packs"]}
        assert names == {"a", "b"}

    def test_calls_refresh_before_listing(self):
        srv = _make_server(packs=[])
        srv.dispatch("knowledge.list_packs", {})
        srv._registry.refresh.assert_called_once()


# ===========================================================================
# knowledge.pack_info
# ===========================================================================


class TestHandlePackInfo:
    def test_returns_pack_info(self):
        pack = _make_pack("mypkg", "Desc", articles=3, entities=9)
        srv = _make_server(packs=[pack])
        result = srv.dispatch("knowledge.pack_info", {"pack_name": "mypkg"})
        assert result["name"] == "mypkg"
        assert result["description"] == "Desc"
        assert result["article_count"] == 3
        assert result["section_count"] == 9

    def test_raises_if_pack_name_missing(self):
        srv = _make_server(packs=[])
        with pytest.raises(ValueError, match="pack_name is required"):
            srv.dispatch("knowledge.pack_info", {})

    def test_raises_if_pack_name_empty(self):
        srv = _make_server(packs=[])
        with pytest.raises(ValueError, match="pack_name is required"):
            srv.dispatch("knowledge.pack_info", {"pack_name": ""})

    def test_raises_if_pack_not_found(self):
        srv = _make_server(packs=[])
        with pytest.raises(ValueError, match="'nope' not found"):
            srv.dispatch("knowledge.pack_info", {"pack_name": "nope"})


# ===========================================================================
# knowledge.query
# ===========================================================================


class TestHandleQuery:
    def _make_agent(self, answer: str = "42", sources=None, query_type: str = "") -> MagicMock:
        agent = MagicMock()
        agent.query.return_value = {
            "answer": answer,
            "sources": sources if sources is not None else ["ArticleA"],
            "query_type": query_type,
        }
        return agent

    def test_raises_if_pack_name_missing(self):
        srv = _make_server(packs=[])
        with pytest.raises(ValueError, match="pack_name is required"):
            srv.dispatch("knowledge.query", {"question": "hello"})

    def test_returns_please_provide_question_when_empty(self):
        pack = _make_pack("wiki")
        srv = _make_server(packs=[pack])
        srv._agents["wiki"] = self._make_agent()
        result = srv.dispatch("knowledge.query", {"pack_name": "wiki", "question": ""})
        assert result["answer"] == "Please provide a question."
        assert result["sources"] == []
        assert result["confidence"] == 0.0

    def test_basic_query_returns_answer(self):
        pack = _make_pack("wiki")
        srv = _make_server(packs=[pack])
        agent = self._make_agent(answer="Paris", sources=["France article"])
        srv._agents["wiki"] = agent
        result = srv.dispatch(
            "knowledge.query", {"pack_name": "wiki", "question": "Capital of France?"}
        )
        assert result["answer"] == "Paris"
        assert len(result["sources"]) == 1
        assert result["confidence"] > 0.0

    def test_limit_applied_to_sources(self):
        pack = _make_pack("wiki")
        srv = _make_server(packs=[pack])
        many_sources = [f"Article{i}" for i in range(20)]
        agent = self._make_agent(sources=many_sources)
        srv._agents["wiki"] = agent
        result = srv.dispatch(
            "knowledge.query",
            {"pack_name": "wiki", "question": "X", "limit": 3},
        )
        assert len(result["sources"]) == 3

    def test_default_limit_is_five(self):
        pack = _make_pack("wiki")
        srv = _make_server(packs=[pack])
        many_sources = [f"A{i}" for i in range(10)]
        agent = self._make_agent(sources=many_sources)
        srv._agents["wiki"] = agent
        result = srv.dispatch(
            "knowledge.query", {"pack_name": "wiki", "question": "X"}
        )
        assert len(result["sources"]) == 5

    def test_agent_called_with_question(self):
        pack = _make_pack("wiki")
        srv = _make_server(packs=[pack])
        agent = self._make_agent()
        srv._agents["wiki"] = agent
        srv.dispatch("knowledge.query", {"pack_name": "wiki", "question": "Foo?"})
        agent.query.assert_called_once()
        call_kwargs = agent.query.call_args
        assert call_kwargs[0][0] == "Foo?"

    def test_agent_cached_for_same_pack(self):
        pack = _make_pack("wiki")
        srv = _make_server(packs=[pack])
        agent = self._make_agent()
        srv._agents["wiki"] = agent
        srv.dispatch("knowledge.query", {"pack_name": "wiki", "question": "Q1"})
        srv.dispatch("knowledge.query", {"pack_name": "wiki", "question": "Q2"})
        assert agent.query.call_count == 2  # same agent reused


# ===========================================================================
# _pack_info_dict helper
# ===========================================================================


class TestPackInfoDict:
    def test_basic_structure(self):
        pack = _make_pack("p", "desc", articles=4, entities=8)
        result = _pack_info_dict(pack)
        assert result == {
            "name": "p",
            "description": "desc",
            "article_count": 4,
            "section_count": 8,
        }

    def test_section_count_uses_entities(self):
        pack = _make_pack(entities=17)
        result = _pack_info_dict(pack)
        assert result["section_count"] == 17


# ===========================================================================
# _section_count helper
# ===========================================================================


class TestSectionCount:
    def test_returns_entities(self):
        stats = _make_stats(articles=5, entities=42)
        assert _section_count(stats) == 42

    def test_zero_entities(self):
        stats = _make_stats(articles=0, entities=0)
        assert _section_count(stats) == 0


# ===========================================================================
# _extract_sources helper
# ===========================================================================


class TestExtractSources:
    def test_empty_result(self):
        assert _extract_sources({}) == []

    def test_string_sources_converted(self):
        result = {"sources": ["Article A", "Article B"]}
        sources = _extract_sources(result)
        assert sources == [
            {"title": "Article A", "section": ""},
            {"title": "Article B", "section": ""},
        ]

    def test_dict_sources_with_title(self):
        result = {"sources": [{"title": "Foo", "section": "Intro", "url": "http://x"}]}
        sources = _extract_sources(result)
        assert sources[0] == {"title": "Foo", "section": "Intro", "url": "http://x"}

    def test_dict_sources_fallback_to_name(self):
        result = {"sources": [{"name": "Bar", "section": ""}]}
        sources = _extract_sources(result)
        assert sources[0]["title"] == "Bar"

    def test_mixed_string_and_dict(self):
        result = {"sources": ["Str", {"title": "Dict", "section": "S"}]}
        sources = _extract_sources(result)
        assert sources[0] == {"title": "Str", "section": ""}
        assert sources[1] == {"title": "Dict", "section": "S", "url": None}

    def test_dict_source_missing_url_is_none(self):
        result = {"sources": [{"title": "X", "section": ""}]}
        sources = _extract_sources(result)
        assert sources[0].get("url") is None


# ===========================================================================
# _estimate_confidence helper
# ===========================================================================


class TestEstimateConfidence:
    def test_training_only_response_returns_0_2(self):
        result = {"query_type": "training_only_response", "sources": [], "answer": ""}
        assert _estimate_confidence(result) == 0.2

    def test_no_sources_returns_0_3(self):
        result = {"answer": "Yes", "sources": []}
        assert _estimate_confidence(result) == 0.3

    def test_no_answer_returns_0_1(self):
        result = {"answer": "", "sources": ["A"]}
        assert _estimate_confidence(result) == 0.1

    def test_confidence_increases_with_more_sources(self):
        few = _estimate_confidence({"answer": "X" * 50, "sources": ["A"]})
        many = _estimate_confidence({"answer": "X" * 50, "sources": ["A"] * 5})
        assert many > few

    def test_confidence_increases_with_longer_answer(self):
        short = _estimate_confidence({"answer": "X", "sources": ["A"]})
        long_ = _estimate_confidence({"answer": "X" * 300, "sources": ["A"]})
        assert long_ > short

    def test_max_source_score_capped_at_1(self):
        # 10 sources should give same score as 5 (cap at 5)
        five = _estimate_confidence({"answer": "X" * 200, "sources": ["A"] * 5})
        ten = _estimate_confidence({"answer": "X" * 200, "sources": ["A"] * 10})
        assert five == ten

    def test_result_is_rounded_to_two_decimals(self):
        result = {"answer": "abc", "sources": ["A", "B"]}
        conf = _estimate_confidence(result)
        assert conf == round(conf, 2)
