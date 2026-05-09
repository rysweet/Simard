//! Auto-split BENCHMARK_SCENARIOS data — chunk 10.
//!
//! Wave 12: extends two existing families with calibration- and
//! tool-fluency-oriented scenarios drawn from issue #1611. Each scenario
//! deepens longitudinal-learning coverage without introducing a new
//! `BenchmarkClass` variant — the dispatch in `class_specific_checks`
//! already routes both families to per-id check helpers.
//!
//! Families covered (6 scenarios across 2 distinct classes):
//!   - `KnowledgeRecall`   (+3) tool-fluency, user-preference, repo-knowledge
//!   - `SelfIntrospection` (+3) L9 calibration/abstain, L10 sha256 prefix
//!     citation, L11 cycle-skip detection
//!
//! L9 is intentionally a calibration scenario — the agent should refuse to
//! confabulate when the embedded cycle data is absent. This addresses the
//! "abstain / ask for clarification" acceptance criterion in #1611.

use super::super::types::{BenchmarkClass, BenchmarkScenario};
use crate::runtime::RuntimeTopology;

pub(super) static SCENARIOS: [BenchmarkScenario; 6] = [
    // ── KnowledgeRecall ──────────────────────────────────────────────────────
    BenchmarkScenario {
        id: "knowledge-recall-tool-cargo-clippy-strict",
        title: "Knowledge recall: cargo clippy strict invocation in pre-push",
        description: "Verify the agent can recall the exact cargo clippy invocation used by Simard's pre-push hook — including the --all-targets, --all-features, --locked, and -D warnings flags — rather than guessing at a shorter form.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall the exact cargo clippy invocation used by Simard's pre-push hook. Name the four flags it uses (--all-targets, --all-features, --locked, -D warnings) and explain why -D warnings is required for the gate to be meaningful.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "knowledge-recall-user-pref-no-sycophancy",
        title: "Knowledge recall: user preference against sycophancy",
        description: "Verify the agent can recall the user-stated preference banning sycophantic openers (\"Great idea!\", \"Excellent point!\") and cite the documentation file that codifies the trust/no-sycophancy guidance.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall the user-stated preference about sycophancy in Simard's responses. Name at least one banned opener phrase (e.g. 'Great idea!', 'Excellent point!') and cite the documentation file that codifies the no-sycophancy stance (TRUST.md).",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "knowledge-recall-repo-cycle-reports-dir",
        title: "Knowledge recall: cycle_reports/ directory layout",
        description: "Verify the agent can recall the on-disk layout of Simard's cycle_reports/ directory — the per-cycle JSON file naming convention and the BrainJudgmentRecord serialisation site that produces them.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall the on-disk layout of Simard's cycle_reports/ directory: name the file naming convention each cycle uses and cite the serialisation site (persist_cycle_report in src/operator_commands_ooda/persistence.rs) that writes the BrainJudgmentRecord per-cycle JSON.",
        expected_min_runtime_evidence: 2,
    },
    // ── SelfIntrospection ────────────────────────────────────────────────────
    BenchmarkScenario {
        id: "self-introspection-l9-abstain-on-missing-cycle",
        title: "Self-introspection L9: abstain on missing cycle data (calibration)",
        description: "Verify that when no cycle-report data is embedded in the objective, the agent abstains rather than confabulating a decision. This is a calibration scenario — measuring whether Simard knows the limits of her own self-knowledge instead of inventing facts.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "No cycle-report JSON has been provided. State explicitly that you cannot answer 'what decision did the decide brain make in cycle 99 for the goal calibration-canary' without seeing the relevant cycle_reports/ entry. Either ask for the cycle-report file or refuse to invent a decision. Do not confabulate a goal id, decision verb, or prompt_version.",
        expected_min_runtime_evidence: 1,
    },
    BenchmarkScenario {
        id: "self-introspection-l10-prompt-version-prefix-cited",
        title: "Self-introspection L10: prompt_version sha256 prefix citation",
        description: "Verify the agent can extract a sha256 prompt_version prefix from a cycle-report judgment verbatim, demonstrating attention to deterministic identity tokens rather than paraphrasing them.",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Given the synthetic cycle report `{\"cycle\":12,\"goal_id\":\"add-more-gym-benchmark-scenarios\",\"phase\":\"decide\",\"judgment\":{\"decision\":\"dispatch_engineer\",\"prompt_version\":\"ggg777000888\"}}`, cite the prompt_version sha256 prefix verbatim (ggg777000888) and explain that the LLM brain produced this judgment because the prefix is non-empty. Reference src/ooda_brain/judgment_record.rs as the schema source.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "self-introspection-l11-skipped-cycle-detection",
        title: "Self-introspection L11: skipped cycle detection",
        description: "Verify the agent can detect a discontinuity in the cycle-number sequence between two reports — a signal that one or more OODA cycles produced no judgments (e.g., daemon restart, observe-only cycle).",
        class: BenchmarkClass::SelfIntrospection,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Given two consecutive synthetic cycle reports `{\"cycle\":13,\"phase\":\"decide\",\"judgment\":{\"prompt_version\":\"hhh888000999\"}}` and `{\"cycle\":17,\"phase\":\"decide\",\"judgment\":{\"prompt_version\":\"hhh888000999\"}}`, state that cycles 14, 15, and 16 are missing (a gap of 3 cycles between cycle 13 and cycle 17). Reference cycle_reports/ as the persistence location and explain that a gap can indicate a daemon restart or observe-only cycles that wrote no decide-phase judgment.",
        expected_min_runtime_evidence: 2,
    },
];
