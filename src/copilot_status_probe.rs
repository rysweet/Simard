use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CopilotStatusProbeResult {
    Available {
        version_line: String,
    },
    Unavailable {
        reason_code: &'static str,
        detail: String,
    },
    Unsupported {
        reason_code: &'static str,
        detail: String,
    },
}

pub(crate) const COPILOT_PROMPT_RECIPE_NAME: &str = "copilot-prompt-check";
pub(crate) const COPILOT_STATUS_RECIPE_NAME: &str = "copilot-status-check";
pub(crate) const COPILOT_STATUS_SIGNAL: &str = "GitHub Copilot CLI";
const COPILOT_STATUS_COMMAND: &str = "amplihack copilot -- --version";

pub(crate) fn is_copilot_status_recipe(recipe_name: &str) -> bool {
    recipe_name == COPILOT_STATUS_RECIPE_NAME
}

pub(crate) fn is_copilot_guarded_recipe(recipe_name: &str) -> bool {
    is_copilot_status_recipe(recipe_name) || recipe_name == COPILOT_PROMPT_RECIPE_NAME
}

pub(crate) fn probe_local_copilot_status() -> CopilotStatusProbeResult {
    let output = match Command::new("amplihack")
        .args(["copilot", "--", "--version"])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CopilotStatusProbeResult::Unavailable {
                reason_code: "amplihack-binary-missing",
                detail: "required executable 'amplihack' is not available on PATH".to_string(),
            };
        }
        Err(error) => {
            return CopilotStatusProbeResult::Unavailable {
                reason_code: "copilot-probe-launch-failed",
                detail: format!("failed to launch '{COPILOT_STATUS_COMMAND}': {error}"),
            };
        }
    };

    let combined_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if !output.status.success() {
        return CopilotStatusProbeResult::Unavailable {
            reason_code: "copilot-probe-exit-nonzero",
            detail: format!(
                "'{COPILOT_STATUS_COMMAND}' exited with status {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string())
            ),
        };
    }

    if let Some(version_line) = combined_output
        .lines()
        .map(str::trim)
        .find(|line| line.contains(COPILOT_STATUS_SIGNAL))
    {
        return CopilotStatusProbeResult::Available {
            version_line: version_line.to_string(),
        };
    }

    CopilotStatusProbeResult::Unsupported {
        reason_code: "copilot-version-signal-missing",
        detail: format!(
            "'{COPILOT_STATUS_COMMAND}' succeeded but did not emit the expected '{COPILOT_STATUS_SIGNAL}' version signal"
        ),
    }
}
