"""Cognitive memory bridge server for Simard.

Extends the base BridgeServer to expose all six cognitive memory types
via the ``memory.*`` method namespace. Each handler maps JSON parameters
to CognitiveMemory method calls and returns JSON-serializable results
matching the Rust types in ``memory_cognitive.rs``.

Usage:
    python3 simard_memory_bridge.py --agent-name simard --db-path /tmp/simard_mem

The server reads newline-delimited JSON requests from stdin and writes
responses to stdout, following the bridge protocol defined in
``bridge_server.py``.
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from pathlib import Path
from typing import Any

# Add the python directory to sys.path so we can import bridge_server
_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPT_DIR))

from bridge_server import BridgeServer  # noqa: E402

# ---------------------------------------------------------------------------
# Locate amplihack-memory-lib
# ---------------------------------------------------------------------------

_MEMORY_LIB_CANDIDATES = [
    Path(__file__).resolve().parent.parent.parent.parent
    / "amplirusty"
    / "amplihack-memory-lib"
    / "src",
    Path.home() / "src" / "amplirusty" / "amplihack-memory-lib" / "src",
]


def _ensure_memory_lib_on_path() -> None:
    """Add amplihack-memory-lib to sys.path if not already importable."""
    try:
        import amplihack_memory  # noqa: F401

        return
    except ImportError:
        pass

    for candidate in _MEMORY_LIB_CANDIDATES:
        if (candidate / "amplihack_memory" / "__init__.py").exists():
            sys.path.insert(0, str(candidate))
            return

    raise ImportError(
        "Cannot find amplihack-memory-lib. "
        "Expected at one of: "
        + ", ".join(str(c) for c in _MEMORY_LIB_CANDIDATES)
    )


_ensure_memory_lib_on_path()

from amplihack_memory.cognitive_memory import CognitiveMemory  # noqa: E402


# ---------------------------------------------------------------------------
# Bridge server
# ---------------------------------------------------------------------------


class CognitiveMemoryBridgeServer(BridgeServer):
    """Bridge server exposing CognitiveMemory over the stdio JSON protocol."""

    def __init__(self, agent_name: str, db_path: str) -> None:
        super().__init__("cognitive-memory")
        self._mem = CognitiveMemory(agent_name=agent_name, db_path=db_path)

        # Sensory
        self.register("memory.record_sensory", self._handle_record_sensory)
        self.register(
            "memory.prune_expired_sensory", self._handle_prune_expired_sensory
        )

        # Working
        self.register("memory.push_working", self._handle_push_working)
        self.register("memory.get_working", self._handle_get_working)
        self.register("memory.clear_working", self._handle_clear_working)

        # Episodic
        self.register("memory.store_episode", self._handle_store_episode)
        self.register(
            "memory.consolidate_episodes", self._handle_consolidate_episodes
        )

        # Semantic
        self.register("memory.store_fact", self._handle_store_fact)
        self.register("memory.search_facts", self._handle_search_facts)

        # Procedural
        self.register("memory.store_procedure", self._handle_store_procedure)
        self.register("memory.recall_procedure", self._handle_recall_procedure)

        # Prospective
        self.register("memory.store_prospective", self._handle_store_prospective)
        self.register("memory.check_triggers", self._handle_check_triggers)

        # Statistics
        self.register("memory.get_statistics", self._handle_get_statistics)

    # -- Sensory -------------------------------------------------------------

    def _handle_record_sensory(self, params: dict[str, Any]) -> dict[str, Any]:
        node_id = self._mem.record_sensory(
            modality=params["modality"],
            raw_data=params["raw_data"],
            ttl_seconds=int(params.get("ttl_seconds", 300)),
        )
        return {"id": node_id}

    def _handle_prune_expired_sensory(
        self, _params: dict[str, Any]
    ) -> dict[str, Any]:
        count = self._mem.prune_expired_sensory()
        return {"count": count}

    # -- Working -------------------------------------------------------------

    def _handle_push_working(self, params: dict[str, Any]) -> dict[str, Any]:
        node_id = self._mem.push_working(
            slot_type=params["slot_type"],
            content=params["content"],
            task_id=params["task_id"],
            relevance=float(params.get("relevance", 1.0)),
        )
        return {"id": node_id}

    def _handle_get_working(self, params: dict[str, Any]) -> dict[str, Any]:
        slots = self._mem.get_working(task_id=params["task_id"])
        return {
            "slots": [
                {
                    "node_id": s.node_id,
                    "slot_type": s.slot_type,
                    "content": s.content,
                    "relevance": s.relevance,
                    "task_id": s.task_id,
                }
                for s in slots
            ]
        }

    def _handle_clear_working(self, params: dict[str, Any]) -> dict[str, Any]:
        count = self._mem.clear_working(task_id=params["task_id"])
        return {"count": count}

    # -- Episodic ------------------------------------------------------------

    def _handle_store_episode(self, params: dict[str, Any]) -> dict[str, Any]:
        metadata = params.get("metadata")
        if isinstance(metadata, str):
            try:
                metadata = json.loads(metadata)
            except (json.JSONDecodeError, TypeError):
                metadata = None
        node_id = self._mem.store_episode(
            content=params["content"],
            source_label=params["source_label"],
            metadata=metadata if metadata else None,
        )
        return {"id": node_id}

    def _handle_consolidate_episodes(
        self, params: dict[str, Any]
    ) -> dict[str, Any]:
        batch_size = int(params.get("batch_size", 10))
        result = self._mem.consolidate_episodes(batch_size=batch_size)
        return {"id": result}

    # -- Semantic ------------------------------------------------------------

    def _handle_store_fact(self, params: dict[str, Any]) -> dict[str, Any]:
        tags = params.get("tags")
        if isinstance(tags, str):
            try:
                tags = json.loads(tags)
            except (json.JSONDecodeError, TypeError):
                tags = None
        node_id = self._mem.store_fact(
            concept=params["concept"],
            content=params["content"],
            confidence=float(params.get("confidence", 1.0)),
            source_id=params.get("source_id", ""),
            tags=tags if tags else None,
        )
        return {"id": node_id}

    def _handle_search_facts(self, params: dict[str, Any]) -> dict[str, Any]:
        facts = self._mem.search_facts(
            query=params["query"],
            limit=int(params.get("limit", 10)),
            min_confidence=float(params.get("min_confidence", 0.0)),
        )
        return {
            "facts": [
                {
                    "node_id": f.node_id,
                    "concept": f.concept,
                    "content": f.content,
                    "confidence": f.confidence,
                    "source_id": f.source_id,
                    "tags": f.tags,
                }
                for f in facts
            ]
        }

    # -- Procedural ----------------------------------------------------------

    def _handle_store_procedure(self, params: dict[str, Any]) -> dict[str, Any]:
        steps = params.get("steps", [])
        if isinstance(steps, str):
            try:
                steps = json.loads(steps)
            except (json.JSONDecodeError, TypeError):
                steps = []
        prerequisites = params.get("prerequisites", [])
        if isinstance(prerequisites, str):
            try:
                prerequisites = json.loads(prerequisites)
            except (json.JSONDecodeError, TypeError):
                prerequisites = []
        node_id = self._mem.store_procedure(
            name=params["name"],
            steps=steps,
            prerequisites=prerequisites,
        )
        return {"id": node_id}

    def _handle_recall_procedure(
        self, params: dict[str, Any]
    ) -> dict[str, Any]:
        procedures = self._mem.recall_procedure(
            query=params["query"],
            limit=int(params.get("limit", 5)),
        )
        return {
            "procedures": [
                {
                    "node_id": p.node_id,
                    "name": p.name,
                    "steps": p.steps,
                    "prerequisites": p.prerequisites,
                    "usage_count": p.usage_count,
                }
                for p in procedures
            ]
        }

    # -- Prospective ---------------------------------------------------------

    def _handle_store_prospective(
        self, params: dict[str, Any]
    ) -> dict[str, Any]:
        node_id = self._mem.store_prospective(
            description=params["description"],
            trigger_condition=params["trigger_condition"],
            action_on_trigger=params["action_on_trigger"],
            priority=int(params.get("priority", 1)),
        )
        return {"id": node_id}

    def _handle_check_triggers(self, params: dict[str, Any]) -> dict[str, Any]:
        triggered = self._mem.check_triggers(content=params["content"])
        return {
            "prospectives": [
                {
                    "node_id": p.node_id,
                    "description": p.description,
                    "trigger_condition": p.trigger_condition,
                    "action_on_trigger": p.action_on_trigger,
                    "status": p.status,
                    "priority": p.priority,
                }
                for p in triggered
            ]
        }

    # -- Statistics ----------------------------------------------------------

    def _handle_get_statistics(
        self, _params: dict[str, Any]
    ) -> dict[str, Any]:
        stats = self._mem.get_statistics()
        return {
            "sensory_count": stats.get("sensory", 0),
            "working_count": stats.get("working", 0),
            "episodic_count": stats.get("episodic", 0),
            "semantic_count": stats.get("semantic", 0),
            "procedural_count": stats.get("procedural", 0),
            "prospective_count": stats.get("prospective", 0),
        }


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Simard cognitive memory bridge server"
    )
    parser.add_argument(
        "--agent-name",
        default="simard",
        help="Agent name for memory isolation (default: simard)",
    )
    parser.add_argument(
        "--db-path",
        default=None,
        help="Path for the Kuzu database directory (default: temp dir)",
    )
    args = parser.parse_args()

    db_path = args.db_path
    if db_path is None:
        db_path = str(Path(tempfile.gettempdir()) / "simard_cognitive_memory")

    server = CognitiveMemoryBridgeServer(
        agent_name=args.agent_name,
        db_path=db_path,
    )
    server.run()


if __name__ == "__main__":
    main()
