//! Auto-split BENCHMARK_SCENARIOS data — chunk 8.
//!
//! SelfIntrospection family: scenarios that measure Simard's longitudinal
//! self-knowledge — her ability to recall and reason about her **own** past
//! decisions, brain judgments, and the prompt-hot-reload mechanism that
//! produced them. The corpus for each scenario is a fabricated cycle-report
//! JSON snippet shaped on the real `CycleReport` schema (#1472) and the
//! per-judgment `prompt_version` field (#1476). Synthetic — not live — so
//! the gym remains deterministic and reproducible across runs.
//!
//! L1 direct recall · L2 multi-cycle synthesis · L3 brain-vs-fallback
//! discrimination (#1476) · L4 prompt hot-reload detection (#1475) ·
//! L5 decision-rationale paraphrase.

use super::super::types::{BenchmarkClass, BenchmarkScenario};
use crate::runtime::RuntimeTopology;

pub(super) static SCENARIOS: [BenchmarkScenario; 5] = [
    BenchmarkScenario {
        id: "self-introspection-l1-direct-cycle-recall",
        title: "Self-introspection L1: direct cycle recall",
        description: "Verify the agent can extract a single act-brain decision from one embedded cycle-report JSON, naming the goal id and the chosen action verbatim.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Given the synthetic cycle report `{\"cycle\":5,\"goal_id\":\"improve-cognitive-memory-persistence\",\"phase\":\"act\",\"judgment\":{\"decision\":\"dispatch_engineer\",\"prompt_version\":\"aaa111000222\"}}`, name the goal id (improve-cognitive-memory-persistence), the phase (act), and the chosen decision (dispatch_engineer) for cycle 5. Cite the cycle-report schema location (src/ooda_loop/cycle.rs) as evidence.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "self-introspection-l2-multi-cycle-synthesis",
        title: "Self-introspection L2: multi-cycle synthesis",
        description: "Verify the agent can read two cycle reports and detect whether the orient-phase assessment of a goal changed between them — a longitudinal-reasoning task that single-cycle recall cannot satisfy.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Compare the two synthetic cycle reports `{\"cycle\":3,\"goal_id\":\"add-more-gym-benchmark-scenarios\",\"phase\":\"orient\",\"judgment\":{\"assessment\":\"on-track\",\"prompt_version\":\"ccc333000444\"}}` and `{\"cycle\":7,\"goal_id\":\"add-more-gym-benchmark-scenarios\",\"phase\":\"orient\",\"judgment\":{\"assessment\":\"blocked-on-clippy\",\"prompt_version\":\"ccc333000444\"}}`. State whether the orient phase changed its assessment of add-more-gym-benchmark-scenarios between cycles 3 and 7, and quote both assessments (on-track vs blocked-on-clippy). Cite cycle_reports/ as the persistence location.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "self-introspection-l3-brain-vs-fallback",
        title: "Self-introspection L3: brain-vs-fallback discrimination",
        description: "Verify the agent understands the prompt_version field added in #1476 — judgments produced by the LLM brain carry a non-empty sha256 prompt_version; deterministic-fallback judgments carry an empty string.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Given the synthetic cycle report `{\"cycle\":4,\"judgments\":[{\"phase\":\"observe\",\"prompt_version\":\"\"},{\"phase\":\"orient\",\"prompt_version\":\"bbb222000333\"},{\"phase\":\"decide\",\"prompt_version\":\"aaa111000222\"},{\"phase\":\"act\",\"prompt_version\":\"\"}]}`, identify which phases used the LLM brain (orient and decide — non-empty prompt_version) and which used the deterministic fallback (observe and act — empty prompt_version). Cite #1476 and src/ooda_brain/judgment_record.rs as the schema source.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "self-introspection-l4-prompt-hot-reload",
        title: "Self-introspection L4: prompt hot-reload detection",
        description: "Verify the agent understands #1475's prompt hot-reload mechanism: when the on-disk prompt asset changes, subsequent cycles record a different sha256 prompt_version without a daemon restart.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Given the synthetic cycle reports `{\"cycle\":2,\"phase\":\"decide\",\"judgment\":{\"prompt_version\":\"aaa111000222\"}}` and `{\"cycle\":6,\"phase\":\"decide\",\"judgment\":{\"prompt_version\":\"bbb222000333\"}}`, state whether the decide-phase prompt asset changed between cycles 2 and 6. Explain how you can tell (the sha256 prompt_version changed from aaa111000222 to bbb222000333) and reference the hot-reload mechanism shipped in #1475 — prompt_assets/simard/*.md is re-hashed each cycle without daemon restart.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "self-introspection-l5-rationale-paraphrase",
        title: "Self-introspection L5: decision-rationale paraphrase",
        description: "Verify the agent can semantically paraphrase a free-text LLM rationale from a past decide-brain judgment — testing comprehension, not just key-extraction.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Given the synthetic cycle report `{\"cycle\":8,\"goal_id\":\"add-more-gym-benchmark-scenarios\",\"phase\":\"decide\",\"judgment\":{\"decision\":\"dispatch_engineer\",\"rationale\":\"Existing 167 scenarios cover repo-exploration and knowledge-recall well, but lack longitudinal self-knowledge coverage; an engineer worktree can add a SelfIntrospection family in under 500 LOC, unblocking the gym's measurement of its own cognition.\",\"prompt_version\":\"ddd444000555\"}}`, summarise the decide brain's rationale for choosing dispatch_engineer in 20 words or fewer, preserving the key concepts: gap in coverage, longitudinal self-knowledge, and bounded engineer worktree.",
        expected_min_runtime_evidence: 2,
    },
];
