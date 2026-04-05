use crate::operator_commands::{run_review_probe, run_review_read_probe};

use super::args::{next_optional_path, next_required, reject_extra_args};

pub(super) fn dispatch_review_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "review command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'review {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_review_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["review".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected review command")
        );
    }

    #[test]
    fn test_review_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["review".to_string(), "bogus".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'review bogus'")
        );
    }

    #[test]
    fn test_review_run_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "review".to_string(),
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
    fn test_review_run_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "review".to_string(),
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
    fn test_review_read_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "review".to_string(),
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
