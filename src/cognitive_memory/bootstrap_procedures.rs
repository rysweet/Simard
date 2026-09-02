//! Bootstrap seeding for Simard's procedural memory.
//!
//! On daemon boot we seed three baseline procedures that the OODA
//! cycle's [`recall_procedure`] call can match against from the very
//! first cycle on a fresh install. Without them, `recall_procedure`
//! returns zero hits until the cycle has run long enough to write
//! its own procedures — and even then, only if the runtime naming
//! convention emits trigger keywords (see PR-C's [`cycle::compose_procedure_name`]).
//!
//! See `docs/reference/cognitive-memory-bootstrap-procedures.md` for
//! the full spec (issue #2281, PR-C, problem 3).
//!
//! ## Why triggers live in the name
//!
//! `recall_procedure(query, limit)` searches the `Procedure.name`
//! column with Cypher `CONTAINS`. There is no separate `triggers`
//! column on the node, so trigger keywords must appear in the name
//! itself to be matchable. We use the canonical form:
//!
//! ```text
//! {pattern}:{scope} | triggers: {comma-separated-keywords}
//! ```
//!
//! Adding a typed `triggers` column would require a schema migration
//! that exceeds PR-C's scope; the in-name encoding is a deliberate
//! single-PR shortcut.

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;

/// A single bootstrap procedure: fully-rendered name (including
/// `| triggers: …` suffix), ordered step list, and prerequisites
/// (empty for all current bootstrap procedures).
///
/// The fields are private; callers go through [`Self::name`],
/// [`Self::steps`], and [`Self::prerequisites`] so the
/// in-name-trigger convention can evolve without churning every
/// consumer.
pub struct BootstrapProcedure {
    name: &'static str,
    steps: &'static [&'static str],
    prerequisites: &'static [&'static str],
}

impl BootstrapProcedure {
    /// Fully-rendered procedure name. Always includes the
    /// `| triggers: <csv>` suffix so `recall_procedure`'s `CONTAINS`
    /// matcher hits on trigger keywords.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Ordered list of steps that constitute the procedure.
    pub fn steps(&self) -> Vec<String> {
        self.steps.iter().map(|s| (*s).to_string()).collect()
    }

    /// Prerequisites for the procedure. All current bootstrap
    /// procedures have none.
    pub fn prerequisites(&self) -> Vec<String> {
        self.prerequisites
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

/// The three bootstrap procedures seeded on daemon start.
///
/// Order is significant only for deterministic logging; the seeder
/// is order-independent semantically (each name is independently
/// checked via `recall_procedure` before insertion).
pub const BOOTSTRAP_PROCEDURES: &[BootstrapProcedure] = &[
    // PR-merge: the merge-ready checklist mirrored from
    // `prompt_assets/simard/engineer_system.md`. Inlined verbatim
    // so future drift in the system prompt does not silently
    // invalidate the bootstrap content.
    BootstrapProcedure {
        name: "pr-merge:bootstrap | triggers: merge,pr,merge-pr,landing,ready-to-merge",
        steps: &[
            "Verify CI green on the target branch",
            "Verify scope clean (single concern, no unrelated edits)",
            "Verify quality-audit passed (>=3 cycles, no critical findings)",
            "Verify docs updated if the change is user-facing",
            "Verify PR description reflects current state",
            "Use the merge-ready skill to gate merge",
        ],
        prerequisites: &[],
    },
    // CI-fix: diagnostic flow used by ci-diagnostic-workflow.
    BootstrapProcedure {
        name: "ci-fix:bootstrap | triggers: ci,green,failing,fix-ci,red",
        steps: &[
            "Fetch the failing CI run summary (gh run view --log-failed)",
            "Classify failure: compile / test / lint / flake",
            "Reproduce locally before changing code",
            "Patch the root cause, not the symptom",
            "Re-run the failing job locally if possible",
            "Push and wait for CI to re-validate",
        ],
        prerequisites: &[],
    },
    // Run-tests: cargo nextest-first invocation playbook.
    BootstrapProcedure {
        name: "run-tests:bootstrap | triggers: test,cargo test,nextest,unit,integration",
        steps: &[
            "cargo nextest run --workspace --no-fail-fast",
            "Fall back to cargo test --workspace if nextest is unavailable",
            "For a single crate: cargo nextest run -p <crate>",
            "For a single test: cargo nextest run --test <name>",
            "Inspect failures; rerun only the failing tests with --no-capture",
        ],
        prerequisites: &[],
    },
];

/// Seed [`BOOTSTRAP_PROCEDURES`] into cognitive memory if missing.
///
/// For each procedure we ask [`CognitiveMemoryOps::procedure_exists`] — an
/// exact-name probe — whether it is already present, and
/// `store_procedure(name, steps, prerequisites)` it only if not. Returns the
/// count of procedures newly stored (`0` if all were already present).
///
/// The exact-name semantics matter: `recall_procedure` matches on Cypher
/// `CONTAINS`, so bootstrap procedures that share trigger tokens would
/// otherwise over-report presence and starve later seeds. `procedure_exists`
/// encapsulates that exact-name filter (and lets the native backend answer it
/// with a direct `LIMIT 1` lookup instead of a recall fan-out).
///
/// **Idempotent**: safe to call on every daemon start; subsequent
/// calls after the first all return `Ok(0)`.
///
/// **Error policy**: surfaces store errors via `Err`. Daemon-boot
/// callers should log and continue — seeding is best-effort —  but
/// the function itself reports honest failure for the unit tests
/// (`seed_propagates_storage_errors`).
///
/// Issue #2281, PR-C, problem 3.
#[tracing::instrument(skip_all)]
pub fn seed_bootstrap_procedures(memory: &dyn CognitiveMemoryOps) -> SimardResult<usize> {
    let mut seeded = 0usize;
    for proc in BOOTSTRAP_PROCEDURES {
        if !memory.procedure_exists(proc.name())? {
            memory.store_procedure(proc.name(), &proc.steps(), &proc.prerequisites())?;
            seeded += 1;
        }
    }
    Ok(seeded)
}
