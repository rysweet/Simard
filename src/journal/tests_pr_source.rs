//! Tests: the production `PrListSource` adapter maps the `gh pr list`
//! PR-readiness view into layperson-readable journal PR rows, derives honest
//! readiness outcomes, scrubs jargon out of titles, and degrades to an empty
//! table when the `gh` service fails (issue #2606). No network — a scripted
//! [`PrGhClient`] stands in for the external service.

use super::test_support::{FakeMemory, FixedClock, day, episode};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::journal::pr_source::{
    GhPrListSource, JOURNAL_PR_LIMIT, open_pr_to_summary, plainify_pr_title, pr_readiness_outcome,
};
use crate::journal::providers::PrListSource;
use crate::journal::store::get_entry_by_date;
use crate::journal::thread::run_journal_tick_with_prs;
use crate::stewardship::merge_authority::{
    CheckRollupEntry, MergedPrSummary, OpenPrSummary, PrGhClient, PrSnapshot,
};

/// Scripted stand-in for the external `gh` PR service: either returns a canned
/// listing or fails the read the way a network blip would.
struct ScriptedGh {
    prs: Vec<OpenPrSummary>,
    fail: bool,
    merged: Vec<MergedPrSummary>,
    merged_fail: bool,
}

impl ScriptedGh {
    fn listing(prs: Vec<OpenPrSummary>) -> Self {
        Self {
            prs,
            fail: false,
            merged: Vec::new(),
            merged_fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            prs: Vec::new(),
            fail: true,
            merged: Vec::new(),
            merged_fail: false,
        }
    }
    /// Attach the day's merged PRs to an existing (open-PR) listing.
    fn with_merged(mut self, merged: Vec<MergedPrSummary>) -> Self {
        self.merged = merged;
        self
    }
    /// Open-PR read succeeds; the merged-PR read fails like a network blip.
    fn merged_read_failing(prs: Vec<OpenPrSummary>) -> Self {
        Self {
            prs,
            fail: false,
            merged: Vec::new(),
            merged_fail: true,
        }
    }
}

impl PrGhClient for ScriptedGh {
    fn view_pr(&self, _repo: &str, _n: u32) -> SimardResult<PrSnapshot> {
        unreachable!("the journal PR source never views a single PR")
    }
    fn squash_merge(&self, _repo: &str, _n: u32) -> SimardResult<()> {
        unreachable!("the journal PR source never merges")
    }
    fn list_open_prs(&self, _repo: &str, _limit: u32) -> SimardResult<Vec<OpenPrSummary>> {
        if self.fail {
            Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: "gh exploded".into(),
            })
        } else {
            Ok(self.prs.clone())
        }
    }
    fn list_merged_prs(
        &self,
        _repo: &str,
        _date: chrono::NaiveDate,
        _limit: u32,
    ) -> SimardResult<Vec<MergedPrSummary>> {
        if self.merged_fail {
            Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: "gh merged list exploded".into(),
            })
        } else {
            Ok(self.merged.clone())
        }
    }
}

fn allowlist() -> Vec<String> {
    vec!["main".to_string()]
}

fn check(state: &str) -> CheckRollupEntry {
    CheckRollupEntry {
        name: "ci".into(),
        state: state.into(),
    }
}

/// A ready-by-default open PR (mergeable, CI green, base `main`).
fn pr(number: u32, title: &str) -> OpenPrSummary {
    OpenPrSummary {
        number,
        title: title.into(),
        head_ref_name: format!("feat/branch-{number}"),
        base_ref_name: "main".into(),
        mergeable: "MERGEABLE".into(),
        checks: vec![check("SUCCESS")],
        url: format!("https://github.com/rysweet/Simard/pull/{number}"),
        author: "simard-engineer".into(),
        labels: Vec::new(),
        is_draft: false,
    }
}

#[test]
fn ready_pr_maps_to_a_plain_row() {
    let row = open_pr_to_summary(
        &pr(2606, "feat(journal): add the daily journal"),
        &allowlist(),
    );
    assert_eq!(row.number, 2606u64, "number is widened to u64");
    // The Conventional-Commits prefix is meaningless to a layperson — stripped.
    assert!(
        !row.plain_summary.contains("feat("),
        "prefix stripped: {}",
        row.plain_summary
    );
    assert!(
        row.plain_summary.contains("daily journal"),
        "description survives: {}",
        row.plain_summary
    );
    // Ready outcome — honest and never mistaken for a merged PR.
    assert!(
        row.outcome.contains("ready to combine"),
        "outcome: {}",
        row.outcome
    );
    assert!(
        !row.outcome.eq_ignore_ascii_case("merged"),
        "an open PR must not be counted as merged"
    );
}

#[test]
fn plain_summary_strips_prefix_and_scrubs_jargon() {
    // "deploy" and "PR" are insider terms the journal glossary explains/replaces.
    let s = plainify_pr_title("fix: speed up the deploy pipeline");
    assert!(!s.starts_with("fix:"), "prefix gone: {s}");
    assert!(
        !s.to_lowercase().contains("deploy"),
        "deploy jargon scrubbed: {s}"
    );

    let p = plainify_pr_title("feat: close the PR faster");
    assert!(
        p.contains("pull request"),
        "PR is expanded for a layperson: {p}"
    );
    assert!(!p.contains("PR "), "no bare 'PR' acronym remains: {p}");

    // A non-conventional colon sentence is left intact.
    let plain = plainify_pr_title("Note: the login page is faster now");
    assert!(
        plain.starts_with("Note:"),
        "ordinary colon sentence preserved: {plain}"
    );

    // A prefix-only title never yields an empty cell.
    assert_eq!(plainify_pr_title("chore:"), "A code change.");
}

#[test]
fn ci_failing_pr_is_not_ready() {
    let mut p = pr(1, "fix: patch a bug");
    p.checks = vec![check("FAILURE")];
    let out = pr_readiness_outcome(&p, &allowlist());
    assert!(out.contains("not ready"), "CI failure => not ready: {out}");
}

#[test]
fn in_progress_checks_read_as_still_running() {
    let mut p = pr(1, "fix: patch a bug");
    p.checks = vec![check("IN_PROGRESS")];
    let out = pr_readiness_outcome(&p, &allowlist());
    assert!(
        out.contains("checks still running"),
        "IN_PROGRESS => still running (not a hard failure): {out}"
    );
}

#[test]
fn wrong_base_branch_is_not_ready() {
    let mut p = pr(1, "fix: patch a bug");
    p.base_ref_name = "develop".into();
    let out = pr_readiness_outcome(&p, &allowlist());
    assert!(
        out.contains("not ready"),
        "a PR off the base allow-list is not ready: {out}"
    );
}

#[test]
fn gh_source_lists_and_maps_open_prs() {
    let mut not_ready = pr(11, "fix: patch the parser");
    not_ready.base_ref_name = "develop".into();
    let gh = ScriptedGh::listing(vec![
        pr(10, "feat(auth): protect the login page"),
        not_ready,
    ]);
    let src = GhPrListSource::new(&gh, "rysweet/Simard", allowlist());

    let rows = src.prs_for_date(day()).expect("mapping succeeds");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].number, 10);
    assert!(
        rows[0].outcome.contains("ready to combine"),
        "first PR is ready: {}",
        rows[0].outcome
    );
    assert!(
        rows[1].outcome.contains("not ready"),
        "second PR (wrong base) is not ready: {}",
        rows[1].outcome
    );
}

#[test]
fn gh_failure_degrades_to_an_empty_table() {
    let gh = ScriptedGh::failing();
    let src = GhPrListSource::new(&gh, "rysweet/Simard", allowlist());
    let rows = src
        .prs_for_date(day())
        .expect("a gh failure degrades honestly, it does not error");
    assert!(
        rows.is_empty(),
        "a gh failure yields an empty proposal table, not a failed tick"
    );
}

#[test]
fn tick_with_prs_writes_and_persists_the_proposal_table() {
    let mem = FakeMemory::new();
    mem.add_episode(episode("I helped land a change to the login page."));
    let clock = FixedClock(day());
    let gh = ScriptedGh::listing(vec![pr(42, "feat(login): make login safer")]);
    let src = GhPrListSource::new(&gh, "rysweet/Simard", allowlist());

    let entry = run_journal_tick_with_prs(&mem, &clock, &src).expect("tick");
    assert_eq!(entry.prs.len(), 1, "injected PR appears in the entry");
    assert_eq!(entry.prs[0].number, 42);
    assert!(entry.prs[0].plain_summary.contains("login"));

    // Persisted under the day key with its proposal table intact.
    let got = get_entry_by_date(&mem as &dyn CognitiveMemoryOps, day())
        .expect("get")
        .expect("entry stored");
    assert_eq!(got.prs.len(), 1, "the table survives persistence");
    assert_eq!(got.prs[0].number, 42);
}

#[test]
fn pr_limit_is_sane() {
    assert!((1..=200).contains(&JOURNAL_PR_LIMIT));
}

/// Build a canned merged-PR summary for the day's "landed changes".
fn merged_pr(number: u32, title: &str) -> MergedPrSummary {
    MergedPrSummary {
        number,
        title: title.to_string(),
        url: format!("https://github.com/rysweet/Simard/pull/{number}"),
    }
}

#[test]
fn gh_source_appends_the_days_merged_prs_with_merged_outcome() {
    // One open proposal plus two changes that merged on the day.
    let gh =
        ScriptedGh::listing(vec![pr(10, "feat(auth): protect the login page")]).with_merged(vec![
            merged_pr(7, "fix(journal): collapse duplicate dates"),
            merged_pr(9, "feat(memory): bound the growth window"),
        ]);
    let src = GhPrListSource::new(&gh, "rysweet/Simard", allowlist());

    let rows = src.prs_for_date(day()).expect("mapping succeeds");
    assert_eq!(rows.len(), 3, "one open + two merged rows");
    // Open proposal keeps its readiness outcome...
    assert_eq!(rows[0].number, 10);
    assert!(
        rows[0].outcome.contains("still open"),
        "open PR keeps a readiness outcome: {}",
        rows[0].outcome
    );
    // ...and the merged changes carry the canonical `merged` outcome the
    // journal's merge counter looks for.
    let merged: Vec<&_> = rows.iter().filter(|r| r.outcome == "merged").collect();
    assert_eq!(merged.len(), 2, "both merged PRs are tagged `merged`");
    assert!(merged.iter().any(|r| r.number == 7));
    assert!(merged.iter().any(|r| r.number == 9));
}

#[test]
fn merged_pr_count_reflects_the_days_landed_changes() {
    // Regression for #4140: before the fix `merged_pr_count()` was
    // structurally 0 because the production source only ingested OPEN PRs.
    let mem = FakeMemory::new();
    mem.add_episode(episode("I landed two changes today."));
    let clock = FixedClock(day());
    let gh = ScriptedGh::listing(vec![pr(42, "feat(login): make login safer")]).with_merged(vec![
        merged_pr(40, "fix(x): correct a rollup"),
        merged_pr(41, "chore(y): tidy up"),
    ]);
    let src = GhPrListSource::new(&gh, "rysweet/Simard", allowlist());

    let entry = run_journal_tick_with_prs(&mem, &clock, &src).expect("tick");
    assert_eq!(
        entry.merged_pr_count(),
        2,
        "the day's merged PRs are counted, not reported as zero"
    );

    // Survives persistence so the dashboard `/api/journal/dates` read agrees.
    let got = get_entry_by_date(&mem as &dyn CognitiveMemoryOps, day())
        .expect("get")
        .expect("entry stored");
    assert_eq!(got.merged_pr_count(), 2, "merge count survives persistence");
}

#[test]
fn merged_pr_fetch_failure_keeps_open_rows_and_degrades_merges() {
    // A merged-list `gh` blip must not fail the tick nor drop the open
    // proposals; it degrades to "no merges surfaced" (honest degradation).
    let gh = ScriptedGh::merged_read_failing(vec![pr(10, "feat: keep me")]);
    let src = GhPrListSource::new(&gh, "rysweet/Simard", allowlist());

    let rows = src
        .prs_for_date(day())
        .expect("a merged-fetch blip degrades honestly, it does not error");
    assert_eq!(rows.len(), 1, "the open proposal survives");
    assert_eq!(rows[0].number, 10);
    assert!(
        !rows.iter().any(|r| r.outcome == "merged"),
        "no merged rows are fabricated when the merged read fails"
    );
}
