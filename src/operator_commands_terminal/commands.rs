use std::path::{Path, PathBuf};

use crate::operator_commands::{
    load_terminal_objective_file, print_text, prompt_root, resolved_state_root,
    resolved_terminal_read_state_root,
};
use crate::terminal_engineer_bridge::{
    SHARED_DEFAULT_STATE_ROOT_SOURCE, SHARED_EXPLICIT_STATE_ROOT_SOURCE, ScopedHandoffMode,
    persist_handoff_artifacts, scoped_handoff_path,
};
use crate::{BootstrapConfig, BootstrapInputs, latest_local_handoff, run_local_session};

use super::read_view::TerminalReadView;

pub fn run_terminal_probe(
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-engineer";
    let base_type = "terminal-shell";
    let state_root_was_explicit = state_root_override.is_some();
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_state_root(
            state_root_override,
            identity,
            base_type,
            topology,
            "terminal-run",
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable terminal handoff")?;
    persist_handoff_artifacts(
        config.state_root_path(),
        ScopedHandoffMode::Terminal,
        &exported,
    )?;
    let handoff_source = scoped_handoff_path(config.state_root_path(), ScopedHandoffMode::Terminal)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("latest_terminal_handoff.json")
        .to_string();
    let continuity_source = if state_root_was_explicit {
        SHARED_EXPLICIT_STATE_ROOT_SOURCE
    } else {
        SHARED_DEFAULT_STATE_ROOT_SOURCE
    };
    let view = TerminalReadView::from_handoff(
        config.state_root_path().to_path_buf(),
        exported,
        handoff_source,
        Some(continuity_source.to_string()),
    )?;
    view.print_terminal_run(
        &execution.snapshot.adapter_capabilities,
        &execution.outcome.execution_summary,
        &execution.outcome.reflection.summary,
    );
    Ok(())
}

pub fn run_terminal_probe_from_file(
    topology: &str,
    objective_path: &Path,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let objective = load_terminal_objective_file(objective_path)?;
    run_terminal_probe(topology, &objective, state_root_override)
}

pub fn run_terminal_read_probe(
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_terminal_read_state_root(state_root_override, topology)?;
    let view = TerminalReadView::load(state_root)?;
    view.print();
    Ok(())
}

pub fn run_terminal_recipe_list_probe() -> Result<(), Box<dyn std::error::Error>> {
    let recipes = crate::operator_commands::list_terminal_recipe_descriptors()?;
    println!("Terminal recipes: {}", recipes.len());
    for (index, recipe) in recipes.iter().enumerate() {
        print_text(
            &format!("Terminal recipe {}", index + 1),
            format!(
                "{} ({})",
                recipe.name,
                recipe.reference.relative_path.display()
            ),
        );
    }
    Ok(())
}

pub fn run_terminal_recipe_show_probe(recipe_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let recipe = crate::operator_commands::load_terminal_recipe(recipe_name)?;
    crate::operator_commands::print_terminal_recipe(recipe_name, &recipe);
    Ok(())
}

pub fn run_terminal_recipe_probe(
    topology: &str,
    recipe_name: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::operator_commands::ensure_terminal_recipe_is_runnable(recipe_name)?;
    let recipe = crate::operator_commands::load_terminal_recipe(recipe_name)?;
    run_terminal_probe(topology, &recipe.contents, state_root_override)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_read_probe_errors_with_nonexistent_state_root() {
        let result = run_terminal_read_probe(
            "single-process",
            Some(PathBuf::from("/nonexistent/path/12345")),
        );
        assert!(result.is_err(), "should fail for missing state root");
    }

    #[test]
    fn terminal_recipe_list_does_not_panic() {
        let _ = run_terminal_recipe_list_probe();
    }

    #[test]
    fn terminal_recipe_show_errors_for_nonexistent() {
        let result = run_terminal_recipe_show_probe("nonexistent-recipe-xyz-99999");
        assert!(result.is_err(), "should fail for unknown recipe");
    }

    #[test]
    fn terminal_read_probe_errors_with_empty_state_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = run_terminal_read_probe("single-process", Some(dir.path().to_path_buf()));
        assert!(
            result.is_err(),
            "should fail when state root has no handoff artifacts"
        );
    }

    #[test]
    fn terminal_read_probe_invalid_topology() {
        let result = run_terminal_read_probe("invalid-topo", None);
        assert!(result.is_err(), "should fail for invalid topology");
    }

    #[test]
    fn terminal_recipe_probe_errors_for_nonexistent_recipe() {
        let result =
            run_terminal_recipe_probe("single-process", "nonexistent-recipe-xyz-99999", None);
        assert!(result.is_err(), "should fail for unknown recipe");
    }

    #[test]
    fn terminal_recipe_probe_errors_for_invalid_recipe_name() {
        let result = run_terminal_recipe_probe("single-process", "INVALID_NAME", None);
        assert!(result.is_err(), "should fail for invalid recipe name");
    }

    #[test]
    fn terminal_recipe_show_errors_for_empty_name() {
        let result = run_terminal_recipe_show_probe("");
        assert!(result.is_err(), "should fail for empty recipe name");
    }

    #[test]
    fn terminal_recipe_show_errors_for_invalid_chars() {
        let result = run_terminal_recipe_show_probe("recipe/with/slashes");
        assert!(result.is_err(), "should fail for recipe name with slashes");
    }

    #[test]
    fn terminal_recipe_list_returns_result() {
        let result = run_terminal_recipe_list_probe();
        let _ = result;
    }

    #[test]
    fn terminal_probe_from_file_errors_for_nonexistent_file() {
        let result = run_terminal_probe_from_file(
            "single-process",
            std::path::Path::new("/nonexistent/objective.txt"),
            None,
        );
        assert!(result.is_err(), "should fail for missing objective file");
    }

    #[test]
    fn terminal_probe_from_file_errors_for_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = run_terminal_probe_from_file("single-process", dir.path(), None);
        assert!(result.is_err(), "should fail when path is a directory");
    }
}
