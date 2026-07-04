//! Rust-coding competency scenarios (roadmap #2491 Pillar 3 / #2492).
//!
//! These complement the recall-only benchmark classes in [`crate::gym`]: each
//! scenario is a bounded Rust task in one sub-skill, with a deterministic grader
//! (`cargo build` / `cargo test` / `cargo clippy -D warnings`) described in
//! [`RustScenario::grader`]. In this first experiment the gym measures whether
//! the competency required to *solve* each task is present and recallable from
//! cognitive memory at the moment of need (right-moment recall, #2491 Pillar
//! 2c) — the acquire → retain → measure loop's measurement step.
//!
//! A scenario is graded "solved" when memory yields at least `min_facts` facts
//! tagged with its sub-skill and at least `min_procedures` matching procedures
//! (see [`super::measurement`]).

use serde::Serialize;

use super::pack::{
    SUBSKILL_BORROW_CHECKER, SUBSKILL_ERROR_HANDLING, SUBSKILL_ERROR_TYPES, SUBSKILL_LIFETIMES,
    SUBSKILL_OWNERSHIP,
};

/// A single bounded Rust competency scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RustScenario {
    /// Stable scenario id.
    pub id: &'static str,
    /// Human-readable title.
    pub title: &'static str,
    /// What the agent must do.
    pub description: &'static str,
    /// Sub-skill exercised (one of [`super::pack::SUBSKILLS`]).
    pub subskill: &'static str,
    /// The "moment of need" recall query an engineer session would issue.
    pub recall_query: &'static str,
    /// Specific fact concepts that MUST be recallable to solve the task. Grading
    /// requires these exact concepts — not just any N sub-skill facts — so a
    /// pack of correctly-tagged but irrelevant facts cannot pass (defeats a
    /// count-only, circular grader).
    pub expected_concepts: &'static [&'static str],
    /// The specific procedure that MUST be recallable to solve the task.
    pub expected_procedure: &'static str,
    /// Deterministic grader that would verify a real solution.
    pub grader: &'static str,
}

/// The V1 Rust competency scenario set — one high-signal scenario per sub-skill.
pub fn rust_scenarios() -> &'static [RustScenario] {
    &SCENARIOS
}

static SCENARIOS: [RustScenario; 5] = [
    RustScenario {
        id: "rust-ownership-fix-use-after-move",
        title: "Fix a use-after-move error",
        description: "A function moves a String into a helper and then reads it again, \
                      producing E0382. Restructure to borrow or clone so it compiles.",
        subskill: SUBSKILL_OWNERSHIP,
        recall_query: "value used after move ownership borrow",
        expected_concepts: &["move-semantics", "ownership-transfer-on-call"],
        expected_procedure: "rust-expert:fix-use-after-move",
        grader: "cargo build (must compile; E0382 resolved)",
    },
    RustScenario {
        id: "rust-borrowck-resolve-aliasing",
        title: "Resolve a mutable/immutable borrow conflict",
        description: "Code holds a shared borrow of a Vec while calling a &mut method, \
                      producing E0502. Reorder or copy so aliasing rules are satisfied.",
        subskill: SUBSKILL_BORROW_CHECKER,
        recall_query: "cannot borrow as mutable also borrowed as immutable",
        expected_concepts: &["aliasing-xor-mutability", "non-lexical-lifetimes"],
        expected_procedure: "rust-expert:resolve-borrow-conflict",
        grader: "cargo build (must compile; E0502 resolved)",
    },
    RustScenario {
        id: "rust-lifetimes-annotate-struct",
        title: "Add a lifetime to a struct holding a reference",
        description: "A struct stores a &str field without a lifetime parameter, \
                      producing E0106. Add the lifetime annotation so it type-checks.",
        subskill: SUBSKILL_LIFETIMES,
        recall_query: "missing lifetime specifier struct holds reference",
        expected_concepts: &["lifetime-elision-rules", "struct-holding-reference"],
        expected_procedure: "rust-expert:annotate-lifetimes",
        grader: "cargo build (must compile; E0106 resolved)",
    },
    RustScenario {
        id: "rust-error-propagate-with-question-mark",
        title: "Convert unwrap panics to ? propagation",
        description: "A parser unwraps several fallible calls. Change it to return \
                      Result and propagate with the ? operator; no panics on bad input.",
        subskill: SUBSKILL_ERROR_HANDLING,
        recall_query: "propagate error question mark operator instead of unwrap",
        expected_concepts: &["question-mark-operator", "avoid-unwrap-in-libraries"],
        expected_procedure: "rust-expert:propagate-with-question-mark",
        grader: "cargo test (parses ok input, returns Err on bad input; no panic)",
    },
    RustScenario {
        id: "rust-error-types-define-thiserror",
        title: "Define a typed crate error with thiserror",
        description: "A library returns Box<dyn Error>. Replace it with a thiserror \
                      enum with #[error] messages and #[from] source conversions.",
        subskill: SUBSKILL_ERROR_TYPES,
        recall_query: "define custom error enum thiserror from conversion",
        expected_concepts: &["thiserror-for-libraries", "anyhow-for-applications"],
        expected_procedure: "rust-expert:define-thiserror-enum",
        grader: "cargo build + cargo test (typed error, ? conversions work)",
    },
];
