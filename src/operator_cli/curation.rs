use crate::operator_commands::{
    run_goal_curation_probe, run_goal_curation_read_probe, run_improvement_curation_probe,
    run_improvement_curation_read_probe,
};

use super::args::{next_optional_path, next_required, reject_extra_args};

pub(super) fn dispatch_goal_curation_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "goal-curation command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_goal_curation_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_goal_curation_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'goal-curation {other}'").into()),
    }
}

pub(super) fn dispatch_improvement_curation_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "improvement-curation command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'improvement-curation {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    // ── goal-curation dispatch ──

    #[test]
    fn test_goal_curation_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["goal-curation".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected goal-curation command")
        );
    }

    #[test]
    fn test_goal_curation_unknown_subcommand() {
        let result =
            dispatch_operator_cli(vec!["goal-curation".to_string(), "unknown".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'goal-curation unknown'")
        );
    }

    #[test]
    fn test_goal_curation_run_missing_base_type() {
        let result = dispatch_operator_cli(vec!["goal-curation".to_string(), "run".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected base type")
        );
    }

    #[test]
    fn test_goal_curation_read_missing_base_type() {
        let result = dispatch_operator_cli(vec!["goal-curation".to_string(), "read".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected base type")
        );
    }

    #[test]
    fn test_goal_curation_run_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "goal-curation".to_string(),
            "run".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_goal_curation_run_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "goal-curation".to_string(),
            "run".to_string(),
            "base-type".to_string(),
            "topology".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected objective")
        );
    }

    #[test]
    fn test_goal_curation_read_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "goal-curation".to_string(),
            "read".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    // ── improvement-curation dispatch ──

    #[test]
    fn test_improvement_curation_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["improvement-curation".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected improvement-curation command")
        );
    }

    #[test]
    fn test_improvement_curation_unknown_subcommand() {
        let result =
            dispatch_operator_cli(vec!["improvement-curation".to_string(), "bad".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'improvement-curation bad'")
        );
    }

    #[test]
    fn test_improvement_curation_run_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "improvement-curation".to_string(),
            "run".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_improvement_curation_run_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "improvement-curation".to_string(),
            "run".to_string(),
            "base-type".to_string(),
            "topology".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected objective")
        );
    }

    #[test]
    fn test_improvement_curation_read_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "improvement-curation".to_string(),
            "read".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }
}
