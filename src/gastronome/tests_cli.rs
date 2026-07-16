//! CLI-level tests for `simard-kitchen` (`dispatch_gastronome_cli` via [`run`]).

use super::*;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn demo_text_is_end_to_end() {
    let out = run(argv(&["demo"])).unwrap();
    assert!(out.contains("Menu plan — Garden Wedding Dinner"));
    assert!(out.contains("Shopping list"));
    assert!(out.contains("Prep schedule"));
    assert!(out.contains("Per guest"));
}

#[test]
fn demo_json_parses_as_plan() {
    let out = run(argv(&["demo", "--json"])).unwrap();
    let plan: planner::MenuPlan = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(plan.guest_count, 40);
    assert!(plan.total_cost > 0.0);
}

#[test]
fn no_command_is_usage_error() {
    let err = run(Vec::<String>::new()).unwrap_err();
    assert!(matches!(err, GastronomeError::Usage(_)));
}

#[test]
fn unknown_command_is_usage_error() {
    let err = run(argv(&["braise"])).unwrap_err();
    assert!(matches!(err, GastronomeError::Usage(_)));
    assert!(err.to_string().contains("unknown command"));
}

#[test]
fn help_prints_usage() {
    let out = run(argv(&["help"])).unwrap();
    assert!(out.contains("usage: simard-kitchen"));
}

#[test]
fn unknown_flag_is_rejected() {
    let err = run(argv(&["demo", "--pretty"])).unwrap_err();
    assert!(matches!(err, GastronomeError::Usage(_)));
}

#[test]
fn plan_from_file_roundtrips_through_toml() {
    let dir = std::env::temp_dir().join(format!("gastronome-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.toml");
    let text = toml::to_string(&KitchenBook::demo()).unwrap();
    std::fs::write(&path, text).unwrap();

    let out = run(argv(&["plan", "--file", path.to_str().unwrap()])).unwrap();
    assert!(out.contains("Menu plan — Garden Wedding Dinner"));

    let json = run(argv(&["plan", "--file", path.to_str().unwrap(), "--json"])).unwrap();
    let plan: planner::MenuPlan = serde_json::from_str(json.trim()).unwrap();
    assert_eq!(plan.guest_count, 40);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn plan_requires_file_flag() {
    let err = run(argv(&["plan"])).unwrap_err();
    assert!(matches!(err, GastronomeError::Usage(_)));
    assert!(err.to_string().contains("--file"));
}

#[test]
fn missing_file_is_io_error() {
    let err = run(argv(&["plan", "--file", "/nonexistent/book.toml"])).unwrap_err();
    assert!(matches!(err, GastronomeError::Io(_)));
}

#[test]
fn shopping_list_command_totals() {
    let out = run(argv(&["shopping-list"]));
    // shopping-list requires --file; demo has no such subcommand path, so this
    // is a usage error — confirm the friendlier direct path instead.
    assert!(out.is_err());

    let dir = std::env::temp_dir().join(format!("gastronome-shop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.toml");
    std::fs::write(&path, toml::to_string(&KitchenBook::demo()).unwrap()).unwrap();
    let out = run(argv(&["shopping-list", "--file", path.to_str().unwrap()])).unwrap();
    assert!(out.contains("Shopping list"));
    assert!(out.contains("TOTAL"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn schedule_command_lists_tasks() {
    let dir = std::env::temp_dir().join(format!("gastronome-sched-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.toml");
    std::fs::write(&path, toml::to_string(&KitchenBook::demo()).unwrap()).unwrap();
    let out = run(argv(&["schedule", "--file", path.to_str().unwrap()])).unwrap();
    assert!(out.contains("Prep schedule"));
    assert!(out.contains("Rosemary focaccia"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scale_command_scales_a_recipe() {
    let dir = std::env::temp_dir().join(format!("gastronome-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.toml");
    std::fs::write(&path, toml::to_string(&KitchenBook::demo()).unwrap()).unwrap();
    let out = run(argv(&[
        "scale",
        "--file",
        path.to_str().unwrap(),
        "--recipe",
        "focaccia",
        "--servings",
        "40",
    ]))
    .unwrap();
    assert!(out.contains("scaled to 40 servings"));
    assert!(out.contains("Bread flour"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scale_rejects_non_numeric_servings() {
    let dir = std::env::temp_dir().join(format!("gastronome-scalebad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.toml");
    std::fs::write(&path, toml::to_string(&KitchenBook::demo()).unwrap()).unwrap();
    let err = run(argv(&[
        "scale",
        "--file",
        path.to_str().unwrap(),
        "--recipe",
        "focaccia",
        "--servings",
        "lots",
    ]))
    .unwrap_err();
    assert!(matches!(err, GastronomeError::Usage(_)));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn external_brief_overrides_embedded() {
    let dir = std::env::temp_dir().join(format!("gastronome-brief-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let book_path = dir.join("book.toml");
    std::fs::write(&book_path, toml::to_string(&KitchenBook::demo()).unwrap()).unwrap();
    let brief_path = dir.join("brief.toml");
    std::fs::write(
        &brief_path,
        r#"
name = "Small Tasting"
guest_count = 8
service_time = "19:00"

[[courses]]
recipe = "focaccia"
portions_per_guest = 1.0
"#,
    )
    .unwrap();
    let json = run(argv(&[
        "plan",
        "--file",
        book_path.to_str().unwrap(),
        "--brief",
        brief_path.to_str().unwrap(),
        "--json",
    ]))
    .unwrap();
    let plan: planner::MenuPlan = serde_json::from_str(json.trim()).unwrap();
    assert_eq!(plan.guest_count, 8);
    assert_eq!(plan.event, "Small Tasting");
    std::fs::remove_dir_all(&dir).ok();
}
