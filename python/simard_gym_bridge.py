"""Simard gym evaluation bridge server.

Extends BridgeServer to expose amplihack-agent-eval's progressive test suite
and long-horizon memory evaluation over the Simard bridge protocol.

Methods: gym.list_scenarios, gym.run_scenario, gym.run_suite

Usage: python3 simard_gym_bridge.py [--output-dir ./eval_output] [--sdk mini]
"""

from __future__ import annotations

import argparse
import logging
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPT_DIR))

from bridge_server import BridgeServer  # noqa: E402

logger = logging.getLogger(__name__)

ALL_DIMENSIONS = [
    "factual_accuracy", "specificity", "temporal_awareness",
    "source_attribution", "confidence_calibration",
]


def _zero_dims() -> dict[str, float]:
    return {d: 0.0 for d in ALL_DIMENSIONS}


def _fail_result(scenario_id: str, msg: str, source: str = "") -> dict[str, Any]:
    """Build a failed GymScenarioResult."""
    return {
        "scenario_id": scenario_id, "success": False, "score": 0.0,
        "dimensions": _zero_dims(), "question_count": 0, "questions_answered": 0,
        "error_message": msg,
        "degraded_sources": [source] if source else [],
    }


def _try_import_progressive():
    try:
        from amplihack.eval.progressive_test_suite import (
            ProgressiveConfig, run_progressive_suite, run_single_level,
        )
        from amplihack_eval.data.progressive_levels import (  # type: ignore[import-not-found]
            ADVANCED_LEVELS, ALL_LEVELS, NOVEL_SKILL_LEVELS,
            TEACHER_STUDENT_LEVELS, TRANSFER_LEVELS,
        )
        return {
            "run_progressive_suite": run_progressive_suite,
            "run_single_level": run_single_level,
            "ProgressiveConfig": ProgressiveConfig,
            "ALL_LEVELS": ALL_LEVELS, "ADVANCED_LEVELS": ADVANCED_LEVELS,
            "TEACHER_STUDENT_LEVELS": TEACHER_STUDENT_LEVELS,
            "NOVEL_SKILL_LEVELS": NOVEL_SKILL_LEVELS,
            "TRANSFER_LEVELS": TRANSFER_LEVELS,
        }, None
    except ImportError as exc:
        return None, str(exc)


def _try_import_long_horizon():
    try:
        from amplihack.eval.long_horizon_memory import LongHorizonMemoryEval
        return {"LongHorizonMemoryEval": LongHorizonMemoryEval}, None
    except ImportError as exc:
        return None, str(exc)


def _level_to_scenario(level: Any) -> dict[str, Any]:
    return {
        "id": level.level_id, "name": level.level_name,
        "description": getattr(level, "description", ""),
        "level": level.level_id,
        "question_count": len(level.questions),
        "article_count": len(level.articles),
    }


def _extract_dimensions(grades: list[dict]) -> dict[str, float]:
    if not grades:
        return _zero_dims()
    avg = sum(g.get("score", 0.0) for g in grades) / len(grades)
    dims = _zero_dims()
    dims["factual_accuracy"] = avg
    dims["specificity"] = avg
    # temporal_awareness: derive from grades that include a temporal score,
    # fall back to None when the data source does not provide it.
    temporal = [g["temporal_awareness"] for g in grades if "temporal_awareness" in g]
    dims["temporal_awareness"] = sum(temporal) / len(temporal) if temporal else None
    # source_attribution: derive from grades that include an attribution score.
    attribution = [g["source_attribution"] for g in grades if "source_attribution" in g]
    dims["source_attribution"] = sum(attribution) / len(attribution) if attribution else None
    metacog = [
        g["metacognition"]["overall"]
        for g in grades
        if g.get("metacognition") and "overall" in g["metacognition"]
    ]
    if metacog:
        dims["confidence_calibration"] = sum(metacog) / len(metacog)
    return dims


def _level_result_to_scenario(result: Any) -> dict[str, Any]:
    if result.success and result.scores:
        details = result.scores.get("details", [])
        return {
            "scenario_id": result.level_id, "success": True,
            "score": result.scores.get("average", 0.0),
            "dimensions": _extract_dimensions(details),
            "question_count": len(details), "questions_answered": len(details),
            "error_message": None, "degraded_sources": [],
        }
    return _fail_result(
        result.level_id,
        result.error_message or "unknown error",
        "progressive_test_suite",
    )


class GymBridgeServer(BridgeServer):
    """Bridge server exposing amplihack-agent-eval to Simard."""

    def __init__(self, output_dir: str | None = None, sdk: str = "mini",
                 agent_name: str = "simard-gym-eval") -> None:
        super().__init__("simard-gym-eval")
        self._output_dir = output_dir or tempfile.mkdtemp(prefix="simard-gym-")
        self._sdk = sdk
        self._agent_name = agent_name
        self._progressive, self._progressive_err = _try_import_progressive()
        self._long_horizon, self._long_horizon_err = _try_import_long_horizon()
        self.register("gym.list_scenarios", self._handle_list_scenarios)
        self.register("gym.run_scenario", self._handle_run_scenario)
        self.register("gym.run_suite", self._handle_run_suite)

    def _all_levels(self) -> list[Any]:
        if self._progressive is None:
            return []
        levels = list(self._progressive["ALL_LEVELS"])
        for key in ["TEACHER_STUDENT_LEVELS", "ADVANCED_LEVELS",
                     "NOVEL_SKILL_LEVELS", "TRANSFER_LEVELS"]:
            levels.extend(self._progressive.get(key, []))
        return levels

    def _find_level(self, level_id: str) -> Any | None:
        for level in self._all_levels():
            if level.level_id == level_id:
                return level
        return None

    def _handle_list_scenarios(self, _params: dict[str, Any]) -> list[dict]:
        scenarios = [_level_to_scenario(l) for l in self._all_levels()]
        if self._long_horizon is not None:
            scenarios.append({
                "id": "long-horizon-memory",
                "name": "Long-horizon memory stress test",
                "description": "1000-turn dialogue testing memory at scale",
                "level": "long-horizon", "question_count": 0, "article_count": 0,
            })
        if self._progressive is None:
            logger.warning("Degraded: progressive suite unavailable: %s",
                           self._progressive_err)
        if self._long_horizon is None:
            logger.warning("Degraded: long_horizon unavailable: %s",
                           self._long_horizon_err)
        return scenarios

    def _handle_run_scenario(self, params: dict[str, Any]) -> dict[str, Any]:
        sid = params.get("scenario_id", "")
        if sid == "long-horizon-memory":
            return self._run_long_horizon()
        if self._progressive is None:
            return _fail_result(sid, f"progressive unavailable: {self._progressive_err}",
                                "progressive_test_suite")
        level = self._find_level(sid)
        if level is None:
            return _fail_result(sid, f"scenario '{sid}' not found")
        config = self._progressive["ProgressiveConfig"](
            output_dir=os.path.join(self._output_dir, sid),
            agent_name=f"{self._agent_name}-{sid}", sdk=self._sdk,
        )
        level_dir = Path(config.output_dir) / sid
        level_dir.mkdir(parents=True, exist_ok=True)
        try:
            result = self._progressive["run_single_level"](level, config, level_dir)
            out = _level_result_to_scenario(result)
            out["question_count"] = len(level.questions)
            return out
        except Exception as exc:
            logger.exception("Scenario %s failed", sid)
            return _fail_result(sid, str(exc), "progressive_test_suite")

    def _handle_run_suite(self, params: dict[str, Any]) -> dict[str, Any]:
        suite_id = params.get("suite_id", "progressive")
        if self._progressive is None:
            return {
                "suite_id": suite_id, "success": False, "overall_score": 0.0,
                "dimensions": _zero_dims(), "scenario_results": [],
                "scenarios_passed": 0, "scenarios_total": 0,
                "error_message": f"progressive unavailable: {self._progressive_err}",
                "degraded_sources": ["progressive_test_suite"],
            }
        config = self._progressive["ProgressiveConfig"](
            output_dir=os.path.join(self._output_dir, suite_id),
            agent_name=self._agent_name, sdk=self._sdk,
        )
        try:
            result = self._progressive["run_progressive_suite"](config)
        except Exception as exc:
            logger.exception("Suite %s failed", suite_id)
            return {
                "suite_id": suite_id, "success": False, "overall_score": 0.0,
                "dimensions": _zero_dims(), "scenario_results": [],
                "scenarios_passed": 0, "scenarios_total": 0,
                "error_message": str(exc), "degraded_sources": ["progressive_test_suite"],
            }
        srs = [_level_result_to_scenario(lr) for lr in result.level_results]
        passed = sum(1 for s in srs if s["success"])
        ok_scores = [s["score"] for s in srs if s["success"]]
        overall = sum(ok_scores) / len(ok_scores) if ok_scores else 0.0
        agg = _zero_dims()
        if ok_scores:
            for d in ALL_DIMENSIONS:
                vals = [s["dimensions"][d] for s in srs if s["success"]]
                agg[d] = sum(vals) / len(vals) if vals else 0.0
        return {
            "suite_id": suite_id, "success": result.success,
            "overall_score": overall, "dimensions": agg,
            "scenario_results": srs, "scenarios_passed": passed,
            "scenarios_total": len(srs),
            "error_message": None if result.success else result.error_message,
            "degraded_sources": [],
        }

    def _run_long_horizon(self) -> dict[str, Any]:
        if self._long_horizon is None:
            return _fail_result("long-horizon-memory",
                                f"unavailable: {self._long_horizon_err}",
                                "long_horizon_memory")
        try:
            ev = self._long_horizon["LongHorizonMemoryEval"](
                agent_name=f"{self._agent_name}-lh", num_turns=100,
                num_questions=20,
                output_dir=os.path.join(self._output_dir, "long-horizon"),
            )
            report = ev.run()
            dims = _zero_dims()
            for cb in (report.category_breakdown or []):
                for dn, dv in cb.dimension_averages.items():
                    if dn in dims:
                        dims[dn] = max(dims[dn], dv)
            return {
                "scenario_id": "long-horizon-memory", "success": True,
                "score": report.overall_score, "dimensions": dims,
                "question_count": report.num_questions,
                "questions_answered": len(report.results),
                "error_message": None, "degraded_sources": [],
            }
        except Exception as exc:
            logger.exception("Long-horizon eval failed")
            return _fail_result("long-horizon-memory", str(exc),
                                "long_horizon_memory")


def main() -> None:
    parser = argparse.ArgumentParser(description="Simard gym evaluation bridge")
    parser.add_argument("--output-dir", default=None)
    parser.add_argument("--sdk", default="mini",
                        choices=["mini", "claude", "copilot", "microsoft"])
    parser.add_argument("--agent-name", default="simard-gym-eval")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s",
                        stream=sys.stderr)
    GymBridgeServer(output_dir=args.output_dir, sdk=args.sdk,
                    agent_name=args.agent_name).run()


if __name__ == "__main__":
    main()
