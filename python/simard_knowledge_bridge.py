"""Simard knowledge bridge server.

Extends BridgeServer to expose agent-kgpacks functionality over the
Simard bridge protocol. Handles three methods:

    knowledge.query      — Query a pack with a natural-language question.
    knowledge.list_packs — List all installed knowledge packs.
    knowledge.pack_info  — Get metadata for a specific pack.

The server wraps the KnowledgeGraphAgent for query execution and the
PackRegistry for pack discovery. It translates results into the JSON
shapes expected by the Rust KnowledgeBridge types.

Usage:
    python3 simard_knowledge_bridge.py [--packs-dir ~/.wikigr/packs]
"""

from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path
from typing import Any

# Ensure the agent-kgpacks package is importable. In production the package
# is installed; for development we add the repo root to sys.path.
_KGPACKS_ROOT = Path(__file__).resolve().parent.parent.parent / "agent-kgpacks"
if _KGPACKS_ROOT.is_dir() and str(_KGPACKS_ROOT) not in sys.path:
    sys.path.insert(0, str(_KGPACKS_ROOT))

from bridge_server import BridgeServer  # noqa: E402

logger = logging.getLogger(__name__)

# Error codes matching bridge_server.py constants.
ERROR_INTERNAL = -32603


class KnowledgeBridgeServer(BridgeServer):
    """Bridge server exposing knowledge graph packs to Simard."""

    def __init__(self, packs_dir: Path | None = None) -> None:
        super().__init__("simard-knowledge")
        self._packs_dir = packs_dir or Path.home() / ".wikigr/packs"
        self._registry = None
        self._agents: dict[str, Any] = {}

        self.register("knowledge.query", self._handle_query)
        self.register("knowledge.list_packs", self._handle_list_packs)
        self.register("knowledge.pack_info", self._handle_pack_info)

    # ------------------------------------------------------------------
    # Lazy initialization
    # ------------------------------------------------------------------

    def _ensure_registry(self):
        """Lazily initialize the pack registry."""
        if self._registry is not None:
            return
        try:
            from wikigr.packs.registry import PackRegistry

            self._registry = PackRegistry(self._packs_dir)
        except ImportError:
            logger.warning(
                "wikigr.packs.registry not available; pack operations will fail"
            )
            raise RuntimeError(
                "agent-kgpacks is not installed or not on sys.path"
            )

    def _get_agent(self, pack_name: str) -> Any:
        """Get or create a KnowledgeGraphAgent for the named pack."""
        if pack_name in self._agents:
            return self._agents[pack_name]

        self._ensure_registry()
        pack = self._registry.get_pack(pack_name)
        if pack is None:
            raise ValueError(f"pack '{pack_name}' not found")

        db_path = pack.path / "pack.db"
        if not db_path.exists():
            raise ValueError(
                f"pack '{pack_name}' has no database at {db_path}"
            )

        from wikigr.agent.kg_agent import KnowledgeGraphAgent

        agent = KnowledgeGraphAgent(
            db_path=str(db_path),
            read_only=True,
            use_enhancements=False,
        )
        self._agents[pack_name] = agent
        return agent

    # ------------------------------------------------------------------
    # Method handlers
    # ------------------------------------------------------------------

    def _handle_query(self, params: dict[str, Any]) -> dict[str, Any]:
        """Handle knowledge.query requests.

        Params:
            pack_name (str): Pack to query.
            question (str): Natural-language question.
            limit (int): Maximum sources to return.

        Returns:
            {answer, sources, confidence}
        """
        pack_name = params.get("pack_name", "")
        question = params.get("question", "")
        limit = int(params.get("limit", 5))

        if not pack_name:
            raise ValueError("pack_name is required")

        if not question:
            return {
                "answer": "Please provide a question.",
                "sources": [],
                "confidence": 0.0,
            }

        agent = self._get_agent(pack_name)
        result = agent.query(question, max_results=min(limit, 100))

        sources = _extract_sources(result)
        confidence = _estimate_confidence(result)

        return {
            "answer": result.get("answer", ""),
            "sources": sources[:limit],
            "confidence": confidence,
        }

    def _handle_list_packs(self, _params: dict[str, Any]) -> dict[str, Any]:
        """Handle knowledge.list_packs requests.

        Returns:
            {"packs": [{name, description, article_count, section_count}, ...]}.
        """
        self._ensure_registry()
        self._registry.refresh()
        packs = self._registry.list_packs()
        return {"packs": [_pack_info_dict(p) for p in packs]}

    def _handle_pack_info(self, params: dict[str, Any]) -> dict[str, Any]:
        """Handle knowledge.pack_info requests.

        Params:
            pack_name (str): Pack to look up.

        Returns:
            {name, description, article_count, section_count}
        """
        pack_name = params.get("pack_name", "")
        if not pack_name:
            raise ValueError("pack_name is required")

        self._ensure_registry()
        pack = self._registry.get_pack(pack_name)
        if pack is None:
            raise ValueError(f"pack '{pack_name}' not found")
        return _pack_info_dict(pack)


# ------------------------------------------------------------------
# Helpers
# ------------------------------------------------------------------


def _pack_info_dict(pack) -> dict[str, Any]:
    """Convert a PackInfo to the dict shape expected by Rust."""
    manifest = pack.manifest
    stats = manifest.graph_stats
    return {
        "name": manifest.name,
        "description": manifest.description,
        "article_count": stats.articles,
        "section_count": _section_count(stats),
    }


def _section_count(stats) -> int:
    """Derive section count from graph stats.

    GraphStats has articles, entities, relationships, size_mb but not
    a direct section count. We use entities as a reasonable proxy since
    sections are the primary entity type in WikiGR packs.
    """
    return stats.entities


def _extract_sources(result: dict[str, Any]) -> list[dict[str, Any]]:
    """Extract source citations from a KnowledgeGraphAgent query result.

    The agent returns sources as a list of article title strings.
    We convert these to the {title, section, url} shape.
    """
    raw_sources = result.get("sources", [])
    sources = []
    for src in raw_sources:
        if isinstance(src, str):
            sources.append({
                "title": src,
                "section": "",
            })
        elif isinstance(src, dict):
            sources.append({
                "title": src.get("title", src.get("name", "")),
                "section": src.get("section", ""),
                "url": src.get("url"),
            })
    return sources


def _estimate_confidence(result: dict[str, Any]) -> float:
    """Estimate confidence from query result heuristics.

    The KnowledgeGraphAgent does not return a confidence score directly.
    We estimate based on the presence and quality of sources.
    """
    answer = result.get("answer", "")
    sources = result.get("sources", [])
    query_type = result.get("query_type", "")

    if query_type == "training_only_response":
        return 0.2
    if not sources:
        return 0.3
    if not answer:
        return 0.1

    # More sources and longer answers suggest higher confidence.
    source_score = min(len(sources) / 5.0, 1.0)
    length_score = min(len(answer) / 200.0, 1.0)
    return round(0.3 + 0.4 * source_score + 0.3 * length_score, 2)


def main():
    parser = argparse.ArgumentParser(description="Simard Knowledge Bridge Server")
    parser.add_argument(
        "--packs-dir",
        type=Path,
        default=None,
        help="Directory containing installed knowledge packs (default: ~/.wikigr/packs)",
    )
    parser.add_argument(
        "--log-level",
        default="WARNING",
        choices=["DEBUG", "INFO", "WARNING", "ERROR"],
        help="Logging level (default: WARNING)",
    )
    args = parser.parse_args()

    logging.basicConfig(
        level=getattr(logging, args.log_level),
        format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
        stream=sys.stderr,
    )

    server = KnowledgeBridgeServer(packs_dir=args.packs_dir)
    server.run()


if __name__ == "__main__":
    main()
