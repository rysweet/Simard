//! TDD (RED) tests for PR-C: bootstrap procedural-memory seeding.
//!
//! Covers the contract documented in
//! `docs/reference/cognitive-memory-bootstrap-procedures.md` for the
//! `seed_bootstrap_procedures` API. Expected to FAIL until PR-C
//! introduces:
//!
//! * `crate::cognitive_memory::bootstrap_procedures::seed_bootstrap_procedures`
//! * `crate::cognitive_memory::bootstrap_procedures::BOOTSTRAP_PROCEDURES`
//!   (a `&[BootstrapProcedure]` with at least the three names
//!   `pr-merge:bootstrap`, `ci-fix:bootstrap`, `run-tests:bootstrap`,
//!   each carrying its `triggers: …` suffix in the name field so that
//!   `recall_procedure`'s `CONTAINS` matcher hits the trigger keywords).
//!
//! The seeder MUST be idempotent: calling it twice produces exactly
//! the same number of stored procedures as calling it once.

#![allow(clippy::type_complexity, clippy::field_reassign_with_default)]

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::cognitive_memory::bootstrap_procedures::{
    BOOTSTRAP_PROCEDURES, seed_bootstrap_procedures,
};
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use std::sync::Mutex;

/// Minimal in-memory `CognitiveMemoryOps` impl exercising just the
/// procedural-memory surface. Tracks calls for assertions.
#[derive(Default)]
struct ProcMock {
    procedures: Mutex<Vec<(String, Vec<String>, Vec<String>)>>,
    store_calls: Mutex<u32>,
    fail_store: bool,
}

impl ProcMock {
    fn count(&self) -> usize {
        self.procedures.lock().unwrap().len()
    }
    fn names(&self) -> Vec<String> {
        self.procedures
            .lock()
            .unwrap()
            .iter()
            .map(|(n, _, _)| n.clone())
            .collect()
    }
    #[allow(dead_code)]
    fn store_calls(&self) -> u32 {
        *self.store_calls.lock().unwrap()
    }
}

impl CognitiveMemoryOps for ProcMock {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen_x".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("wrk_x".to_string())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("epi_x".to_string())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _c: &str,
        _content: &str,
        _conf: f64,
        _tags: &[String],
        _source: &str,
    ) -> SimardResult<String> {
        Ok("sem_x".to_string())
    }
    fn search_facts(&self, _q: &str, _l: u32, _c: f64) -> SimardResult<Vec<CognitiveFact>> {
        Ok(vec![])
    }
    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prereqs: &[String],
    ) -> SimardResult<String> {
        *self.store_calls.lock().unwrap() += 1;
        if self.fail_store {
            return Err(SimardError::ServerError(
                "stub: store_procedure deliberately failed".to_string(),
            ));
        }
        let id = format!("prc_{}", self.procedures.lock().unwrap().len() + 1);
        self.procedures
            .lock()
            .unwrap()
            .push((name.to_string(), steps.to_vec(), prereqs.to_vec()));
        Ok(id)
    }
    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        // Test-scaffolding mock that pins the *integration contract*
        // PR-C cares about: a multi-token objective like
        // `"merge PR #2281"` should hit any seeded procedure whose
        // name contains any meaningful (>= 3 chars) lowercased token.
        // The production `preparation_memory_operations` enforces the
        // same contract by tokenizing the objective and calling
        // `recall_procedure` once per token; this mock collapses both
        // steps into a single helper so the bootstrap-seeding tests
        // can drive the full integration without a Cypher backend.
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() >= 3)
            .map(|t| t.to_ascii_lowercase())
            .collect();
        let hits: Vec<CognitiveProcedure> = self
            .procedures
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, (name, steps, _))| {
                if tokens.is_empty() {
                    let lower = query.to_lowercase();
                    return name.to_lowercase().contains(&lower)
                        || steps.iter().any(|s| s.to_lowercase().contains(&lower));
                }
                let name_lower = name.to_lowercase();
                let steps_lower: Vec<String> = steps.iter().map(|s| s.to_lowercase()).collect();
                tokens
                    .iter()
                    .any(|t| name_lower.contains(t) || steps_lower.iter().any(|s| s.contains(t)))
            })
            .map(|(idx, (name, steps, prereqs))| CognitiveProcedure {
                node_id: format!("prc_{}", idx + 1),
                name: name.clone(),
                steps: steps.clone(),
                prerequisites: prereqs.clone(),
                usage_count: 0,
            })
            .take(limit as usize)
            .collect();
        Ok(hits)
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pro_x".to_string())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

/// Two consecutive calls to `seed_bootstrap_procedures` MUST leave
/// the store with exactly `BOOTSTRAP_PROCEDURES.len()` procedures —
/// not double.
#[test]
fn seed_is_idempotent() {
    let mock = ProcMock::default();
    let first = seed_bootstrap_procedures(&mock).expect("first seed");
    let second = seed_bootstrap_procedures(&mock).expect("second seed");

    assert_eq!(
        first as usize,
        BOOTSTRAP_PROCEDURES.len(),
        "first call must seed every bootstrap procedure"
    );
    assert_eq!(
        second, 0,
        "second call must seed nothing (idempotency); got {second}"
    );
    assert_eq!(
        mock.count(),
        BOOTSTRAP_PROCEDURES.len(),
        "total procedures must equal BOOTSTRAP_PROCEDURES.len() after two seeds"
    );
}

/// If a procedure with the same name already exists, the seeder must
/// skip it. Only the missing bootstrap procedures get stored.
#[test]
fn seed_skips_existing_procedures_by_name() {
    let mock = ProcMock::default();
    // Pre-populate using the EXACT name of the first bootstrap procedure.
    let pr_merge_name = BOOTSTRAP_PROCEDURES[0].name();
    mock.store_procedure(pr_merge_name, &[], &[]).unwrap();
    let pre_count = mock.count();
    assert_eq!(pre_count, 1);

    let seeded = seed_bootstrap_procedures(&mock).expect("seed");

    assert_eq!(
        seeded as usize,
        BOOTSTRAP_PROCEDURES.len() - 1,
        "seeder must skip the one pre-existing bootstrap procedure"
    );
    assert_eq!(mock.count(), BOOTSTRAP_PROCEDURES.len());
}

/// After seeding, an objective mentioning a known trigger keyword
/// (`"merge"`, `"ci"`, `"test"`) MUST recall at least one bootstrap
/// procedure. This is the whole point of stuffing triggers into the
/// procedure name: `recall_procedure`'s `CONTAINS` matcher must hit.
#[test]
fn recall_finds_seeded_procedures_for_typical_objectives() {
    let mock = ProcMock::default();
    seed_bootstrap_procedures(&mock).unwrap();

    // PR merge objective.
    let recall_pr = mock.recall_procedure("merge PR #2281", 5).unwrap();
    assert!(
        !recall_pr.is_empty(),
        "objective 'merge PR #2281' must recall at least one bootstrap procedure (likely pr-merge:bootstrap); seeded names: {:?}",
        mock.names(),
    );
    assert!(
        recall_pr.iter().any(|p| p.name.contains("pr-merge")),
        "the pr-merge:bootstrap procedure must be among the hits; got: {:?}",
        recall_pr.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
    );

    // CI fix objective.
    let recall_ci = mock.recall_procedure("the CI is failing", 5).unwrap();
    assert!(
        !recall_ci.is_empty() && recall_ci.iter().any(|p| p.name.contains("ci-fix")),
        "objective 'the CI is failing' must recall ci-fix:bootstrap; got: {:?}",
        recall_ci.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
    );

    // Run tests objective.
    let recall_test = mock.recall_procedure("run unit test", 5).unwrap();
    assert!(
        !recall_test.is_empty() && recall_test.iter().any(|p| p.name.contains("run-tests")),
        "objective 'run unit test' must recall run-tests:bootstrap; got: {:?}",
        recall_test
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>(),
    );
}

/// If `store_procedure` fails, the seeder MUST surface that error
/// (do not silently swallow it). Seeding is best-effort at the
/// daemon-boot caller, but the function itself returns an honest Err.
#[test]
fn seed_propagates_storage_errors() {
    let mut mock = ProcMock::default();
    mock.fail_store = true;

    let result = seed_bootstrap_procedures(&mock);

    assert!(
        result.is_err(),
        "seed_bootstrap_procedures must propagate store_procedure errors; got {:?}",
        result
    );
}

/// Pin the three bootstrap procedure names. Catches the next person
/// who renames one and forgets to update the recall test or the
/// downstream documentation.
#[test]
fn bootstrap_procedure_set_includes_three_required_names() {
    let names: Vec<&str> = BOOTSTRAP_PROCEDURES.iter().map(|p| p.name()).collect();
    let mut found_pr_merge = false;
    let mut found_ci_fix = false;
    let mut found_run_tests = false;
    for n in &names {
        if n.starts_with("pr-merge:bootstrap") {
            found_pr_merge = true;
        }
        if n.starts_with("ci-fix:bootstrap") {
            found_ci_fix = true;
        }
        if n.starts_with("run-tests:bootstrap") {
            found_run_tests = true;
        }
    }
    assert!(
        found_pr_merge,
        "BOOTSTRAP_PROCEDURES must include pr-merge:bootstrap; names: {names:?}"
    );
    assert!(
        found_ci_fix,
        "BOOTSTRAP_PROCEDURES must include ci-fix:bootstrap; names: {names:?}"
    );
    assert!(
        found_run_tests,
        "BOOTSTRAP_PROCEDURES must include run-tests:bootstrap; names: {names:?}"
    );

    // Each name must include the `| triggers:` suffix so the
    // CONTAINS matcher can hit on trigger keywords.
    for n in &names {
        assert!(
            n.contains("| triggers:"),
            "bootstrap procedure '{n}' must include '| triggers: …' suffix"
        );
    }
}
