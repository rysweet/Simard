//! Canonical installer command for Simard.

use std::path::PathBuf;

use crate::install::{InstallConfig, run};

pub const INSTALL_HELP: &str = "\
Simard install subcommand

Usage: simard install [OPTIONS]

Install the current Simard binary, prompt_assets, and user-level systemd units.

Options:
  --simard-home <PATH>       Install root. Overrides SIMARD_HOME. Defaults to ~/.simard
  --dry-run                  Validate inputs and print the activation plan without mutation
  --systemd-user-dir <PATH>  User unit directory. Defaults to ~/.config/systemd/user
  --systemctl <PATH|NAME>    systemctl executable for activation and tests
  --rollback <MANIFEST>      Restore binary, assets, units, config, and state from a verified backup
  --help, -h                 Show this help

Installs:
  $SIMARD_HOME/bin/simard
  $SIMARD_HOME/prompt_assets
  simard-ooda.service
  simard-signal.service
";

pub fn handle_install<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let config = parse_install_args(args)?;
    run(config)?;
    Ok(())
}

fn parse_install_args<I>(args: I) -> Result<InstallConfig, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut config = InstallConfig::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" | "help" => {
                print!("{INSTALL_HELP}");
                return Ok(config.with_help_only());
            }
            "--dry-run" => config.dry_run = true,
            "--simard-home" => {
                config.simard_home = Some(next_path(&mut args, "--simard-home")?);
            }
            "--systemd-user-dir" => {
                config.systemd_user_dir = Some(next_path(&mut args, "--systemd-user-dir")?);
            }
            "--systemctl" => {
                config.systemctl = Some(next_path(&mut args, "--systemctl")?);
            }
            "--rollback" => {
                config.rollback_manifest = Some(next_path(&mut args, "--rollback")?);
            }
            _ if arg.starts_with("--simard-home=") => {
                config.simard_home = Some(PathBuf::from(
                    arg.strip_prefix("--simard-home=").expect("prefix checked"),
                ));
            }
            _ if arg.starts_with("--systemd-user-dir=") => {
                config.systemd_user_dir = Some(PathBuf::from(
                    arg.strip_prefix("--systemd-user-dir=")
                        .expect("prefix checked"),
                ));
            }
            _ if arg.starts_with("--systemctl=") => {
                config.systemctl = Some(PathBuf::from(
                    arg.strip_prefix("--systemctl=").expect("prefix checked"),
                ));
            }
            _ if arg.starts_with("--rollback=") => {
                config.rollback_manifest = Some(PathBuf::from(
                    arg.strip_prefix("--rollback=").expect("prefix checked"),
                ));
            }
            _ => return Err(format!("unexpected argument: {arg}").into()),
        }
    }

    Ok(config)
}

fn next_path(
    args: &mut impl Iterator<Item = String>,
    flag: &'static str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let value = args
        .next()
        .ok_or_else(|| format!("{flag} requires a path value"))?;
    Ok(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_install_options() {
        let config = parse_install_args([
            "--simard-home".to_string(),
            "/tmp/simard-home".to_string(),
            "--systemd-user-dir=/tmp/systemd".to_string(),
            "--systemctl".to_string(),
            "/tmp/systemctl".to_string(),
            "--dry-run".to_string(),
        ])
        .unwrap();

        assert_eq!(config.simard_home, Some(PathBuf::from("/tmp/simard-home")));
        assert_eq!(config.systemd_user_dir, Some(PathBuf::from("/tmp/systemd")));
        assert_eq!(config.systemctl, Some(PathBuf::from("/tmp/systemctl")));
        assert!(config.dry_run);
    }

    #[test]
    fn rejects_unknown_install_argument() {
        let error = parse_install_args(["--unknown".to_string()])
            .expect_err("unknown flags should fail")
            .to_string();

        assert!(error.contains("unexpected argument"));
    }
}
