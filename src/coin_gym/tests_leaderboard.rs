use super::leaderboard::{
    MATERIAL_DEVIATION_PCT, compare_to_leaderboard, find_published, published_leaderboard,
};
use super::scorer::{OutcomeHistogram, ReachPrecision, Score};

fn make_score(model: &str, reach_rate: f64, precision: f64, offline: bool) -> Score {
    Score {
        run_id: "r".to_string(),
        model: model.to_string(),
        overall: ReachPrecision {
            reached: 0,
            submitted: 0,
            total: 0,
            reach_rate,
            precision,
        },
        by_family: Vec::new(),
        histogram: OutcomeHistogram::default(),
        offline_scaffold: offline,
    }
}

#[test]
fn published_table_has_eight_ranked_entries() {
    let board = published_leaderboard();
    assert_eq!(board.len(), 8);
    for (i, entry) in board.iter().enumerate() {
        assert_eq!(entry.rank as usize, i + 1);
    }
    let top = &board[0];
    assert_eq!(top.model, "Claude Opus 4.6");
    assert!((top.reach_pct - 30.0).abs() < 1e-9);
    assert!((top.precision_pct - 52.5).abs() < 1e-9);
}

#[test]
fn find_published_normalises_model_names() {
    let hit = find_published("claude-opus-4.6").unwrap();
    assert_eq!(hit.model, "Claude Opus 4.6");
    let hit2 = find_published("GPT-5.4-mini").unwrap();
    assert_eq!(hit2.model, "GPT-5.4-mini");
    assert!(find_published("nonexistent-model").is_none());
}

#[test]
fn find_published_prefers_exact_over_prefix() {
    // Regression: "gpt-5.4-mini" must not be shadowed by the "GPT-5.4" prefix.
    assert_eq!(
        find_published("gpt-5.4-mini").unwrap().model,
        "GPT-5.4-mini"
    );
    assert_eq!(find_published("gpt-5.4").unwrap().model, "GPT-5.4");
    assert!(find_published("").is_none());
}

#[test]
fn compare_flags_within_variance() {
    let score = make_score("Claude Opus 4.6", 0.30, 0.525, false);
    let cmp = compare_to_leaderboard(&score).unwrap();
    assert!((cmp.reach_delta_pct).abs() < 1e-9);
    assert!(!cmp.material_deviation);
    assert!(cmp.note.contains("within variance"));
}

#[test]
fn compare_flags_material_deviation() {
    let score = make_score("Claude Opus 4.6", 0.05, 0.10, false);
    let cmp = compare_to_leaderboard(&score).unwrap();
    assert!(cmp.reach_delta_pct.abs() > MATERIAL_DEVIATION_PCT);
    assert!(cmp.material_deviation);
    assert!(cmp.note.contains("material deviation"));
}

#[test]
fn offline_scaffold_note_is_explicit() {
    let score = make_score("Claude Opus 4.6", 0.05, 0.10, true);
    let cmp = compare_to_leaderboard(&score).unwrap();
    // Even with a large delta, an offline run is labelled illustrative, not a bug.
    assert!(cmp.note.contains("offline scaffold"));
}

#[test]
fn unknown_model_has_no_comparison() {
    let score = make_score("my-local-frankenmodel", 0.5, 0.5, false);
    assert!(compare_to_leaderboard(&score).is_none());
}
