//! Asset-level contracts for the parser-free goal-session route.
//!
//! TDD status: RED because the actor recipe and least-privilege policy do not
//! exist yet. This target compiles independently of the future Rust module, so
//! it provides an immediately runnable red test while typed APIs are built.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_required(relative: &str) -> String {
    let path = repo_path(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("required typed OODA asset {}: {error}", path.display()))
}

#[test]
fn goal_session_actor_recipe_exists_and_uses_raw_semantic_handoffs() {
    let recipe = read_required("prompt_assets/simard/recipes/goal-session-actor.yaml");

    for required in [
        "observe_output_path",
        "orient_output_path",
        "decide_output_path",
        "task_path",
        "reason_path",
        "goal-session-actor",
    ] {
        assert!(
            recipe.contains(required),
            "goal-session actor recipe must receive raw {required} context"
        );
    }

    for forbidden in [
        "ACTION: SPAWN_ENGINEER",
        "NO ACTION",
        "REASON:",
        "PROGRESS:",
        "schema repair",
        "first word",
        "JSON decision",
        "structured output",
    ] {
        assert!(
            !recipe
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase()),
            "typed actor recipe must not introduce a prose contract or repair path: {forbidden}"
        );
    }
}

#[test]
fn goal_session_policy_grants_only_scoped_typed_terminal_capabilities() {
    let policy = read_required("prompt_assets/simard/policies/goal-session-capabilities.toml");

    for required in [
        "record_action.spawn_engineer",
        "record_no_action",
        "record_blocked",
        "record_completed",
    ] {
        assert!(
            policy.contains(required),
            "goal-session policy must grant {required}"
        );
    }

    for forbidden in [
        "record_progress",
        "merge_pull_request",
        "deploy_artifact",
        "execute_merge",
        "execute_deploy",
        "legacy_parser",
        "shell",
        "process_exec",
        "record_action.file_issue",
        "record_action.request_merge",
        "record_action.request_deploy",
    ] {
        assert!(
            !policy.contains(forbidden),
            "goal-session policy must not grant {forbidden}"
        );
    }
}

#[test]
fn migrated_recipe_assets_do_not_describe_prose_as_machine_authority() {
    let migrated_assets = [
        "prompt_assets/simard/recipes/goal-session-actor.yaml",
        "prompt_assets/simard/policies/goal-session-capabilities.toml",
    ];

    for relative in migrated_assets {
        let content = read_required(relative);
        for forbidden in [
            "parse_orchestrator_response",
            "parse_admission_decision",
            "extract the first",
            "strips any surrounding",
            "fenced ```json",
            "fallback scanner",
            "unparseable",
        ] {
            assert!(
                !content
                    .to_ascii_lowercase()
                    .contains(&forbidden.to_ascii_lowercase()),
                "{relative} must not make agent prose machine-authoritative via {forbidden:?}"
            );
        }
    }
}

#[test]
fn installer_recursively_stages_the_recipe_and_policy_assets() {
    let installer = read_required("src/install/mod.rs");
    let assets = read_required("src/install/assets.rs");
    assert!(
        installer.contains("stage_prompt_assets"),
        "installer must stage prompt assets"
    );
    assert!(
        assets.contains("copy_dir_recursive(source, staged, source)"),
        "all recipe and policy assets must be copied recursively"
    );
}
