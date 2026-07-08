//! `simard platform …` — a thin rail to the canonical platform-installer
//! scaffold (issue #3119).
//!
//! The platform installer's source of truth is the **Crocutus** repository's
//! `scripts/install.sh` and `scripts/doctor.sh` (see
//! `docs/concepts/platform-installer.md`). This rail is a convenience front door
//! from the Simard binary; it forwards to that scaffold rather than
//! re-implementing it, so there is a single code path to test and maintain.
//!
//! The verb is namespaced under `platform` specifically so it never collides
//! with the pre-existing bare `simard install` verb (which persists the current
//! binary to `~/.simard/bin` for the `npx` wrapper). Unknown `platform`
//! subcommands fail closed with an error that names the offending subcommand.

use std::path::PathBuf;
use std::process::Command;

pub(super) const PLATFORM_HELP: &str = "\
Simard platform installer rail

Usage: simard platform <install|doctor> [args...]

  install [--identity <path>] [--local | --remote azlin:<vm>]
          [--upgrade | --uninstall] [--check-only] [--yes] ...
                      — stand up / upgrade / uninstall a Simard-family agent
                        daemon on a host (idempotent, fail-closed).
  doctor  [--identity <path>] [--local | --remote azlin:<vm>] [--check-only]
                      — run the preflight doctor standalone.

This is a thin rail. The canonical implementation is the Crocutus scaffold
(scripts/install.sh, scripts/doctor.sh); this command forwards to it. Locate the
scaffold with SIMARD_INSTALLER_SCAFFOLD=<dir containing install.sh/doctor.sh>,
or it is discovered under ~/crocutus/scripts or ~/src/Crocutus/scripts.

See docs/reference/platform-installer-cli.md for the full contract.
";

const PLATFORM_INSTALL_HELP: &str = "\
Simard platform install — stand up / upgrade / uninstall an agent daemon

Usage:
  simard platform install --identity <path> [--local | --remote azlin:<vm>]
                          [--upgrade | --uninstall] [--check-only] [--yes]
                          [--dashboard-port <n>] [--state-root <path>]
                          [--unit-name <name>]

Forwards to the canonical Crocutus scaffold scripts/install.sh. See
docs/reference/platform-installer-cli.md.
";

const PLATFORM_DOCTOR_HELP: &str = "\
Simard platform doctor — run the installer preflight standalone

Usage:
  simard platform doctor --identity <path> [--local | --remote azlin:<vm>]
                         [--check-only]

Forwards to the canonical Crocutus scaffold scripts/doctor.sh. See
docs/howto/run-the-installer-preflight-doctor.md.
";

fn is_help(arg: &str) -> bool {
    matches!(arg, "--help" | "-h" | "help")
}

/// Dispatch a `simard platform …` invocation.
///
/// `--help` on the group or either subcommand is side-effect-free (prints help,
/// exits Ok). Real `install`/`doctor` invocations forward to the canonical
/// scaffold. An unknown subcommand fails closed, naming the subcommand.
pub(super) fn dispatch_platform_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(subcommand) = args.next() else {
        // Bare `simard platform` prints group help rather than erroring.
        print!("{PLATFORM_HELP}");
        return Ok(());
    };

    if is_help(&subcommand) {
        print!("{PLATFORM_HELP}");
        return Ok(());
    }

    match subcommand.as_str() {
        "install" => forward_to_scaffold("install.sh", PLATFORM_INSTALL_HELP, args),
        "doctor" => forward_to_scaffold("doctor.sh", PLATFORM_DOCTOR_HELP, args),
        other => Err(format!(
            "unsupported command 'platform {other}'. Try 'simard platform --help' \
             (supported: install, doctor)."
        )
        .into()),
    }
}

/// Forward a subcommand's remaining args to the named scaffold script.
///
/// Peeks for a help flag first (side-effect-free), then locates the scaffold and
/// execs `bash <scaffold>/<script> <args...>`, mapping its exit status. Fails
/// closed with an actionable error if the scaffold cannot be found.
fn forward_to_scaffold(
    script: &str,
    help_text: &'static str,
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let forwarded: Vec<String> = args.collect();

    if forwarded.iter().any(|a| is_help(a)) {
        print!("{help_text}");
        return Ok(());
    }

    let scaffold = locate_scaffold().ok_or_else(|| -> Box<dyn std::error::Error> {
        format!(
            "platform installer scaffold not found (looked for '{script}'). Set \
             SIMARD_INSTALLER_SCAFFOLD to the directory containing install.sh / \
             doctor.sh (the Crocutus repo's scripts/), or clone Crocutus to \
             ~/crocutus or ~/src/Crocutus."
        )
        .into()
    })?;

    let script_path = scaffold.join(script);
    if !script_path.is_file() {
        return Err(format!(
            "platform installer scaffold at '{}' does not contain '{script}' \
             (fail-closed: refusing to run an incomplete scaffold).",
            scaffold.display()
        )
        .into());
    }

    let status = Command::new("bash")
        .arg(&script_path)
        .args(&forwarded)
        .status()
        .map_err(|e| format!("failed to launch scaffold '{}': {e}", script_path.display()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "platform installer '{script}' failed (exit {}). See the phase report above.",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        )
        .into())
    }
}

/// Locate the installer scaffold directory (the one holding `install.sh` /
/// `doctor.sh`). Explicit env override wins; otherwise probe known locations.
fn locate_scaffold() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SIMARD_INSTALLER_SCAFFOLD") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }

    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    [
        home.join("crocutus").join("scripts"),
        home.join("src").join("Crocutus").join("scripts"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> std::vec::IntoIter<String> {
        args.iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn bare_platform_prints_help_ok() {
        assert!(dispatch_platform_command(argv(&[])).is_ok());
    }

    #[test]
    fn platform_help_ok() {
        assert!(dispatch_platform_command(argv(&["--help"])).is_ok());
    }

    #[test]
    fn install_and_doctor_help_ok() {
        assert!(dispatch_platform_command(argv(&["install", "--help"])).is_ok());
        assert!(dispatch_platform_command(argv(&["doctor", "--help"])).is_ok());
    }

    #[test]
    fn unknown_subcommand_fails_closed_naming_it() {
        let err = dispatch_platform_command(argv(&["totally-unknown-subcommand"]))
            .expect_err("unknown platform subcommand must fail closed");
        assert!(err.to_string().contains("totally-unknown-subcommand"));
    }
}
