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

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_copilot_status_recipe ────────────────────────────────────────

    #[test]
    fn is_copilot_status_recipe_matches_exact_name() {
        assert!(is_copilot_status_recipe(COPILOT_STATUS_RECIPE_NAME));
    }

    #[test]
    fn is_copilot_status_recipe_rejects_other_names() {
        assert!(!is_copilot_status_recipe("some-other-recipe"));
        assert!(!is_copilot_status_recipe(""));
        assert!(!is_copilot_status_recipe("copilot-status"));
        assert!(!is_copilot_status_recipe("copilot-status-check-extra"));
    }

    // ── is_copilot_guarded_recipe ───────────────────────────────────────

    #[test]
    fn is_copilot_guarded_recipe_matches_status_recipe() {
        assert!(is_copilot_guarded_recipe(COPILOT_STATUS_RECIPE_NAME));
    }

    #[test]
    fn is_copilot_guarded_recipe_matches_prompt_recipe() {
        assert!(is_copilot_guarded_recipe(COPILOT_PROMPT_RECIPE_NAME));
    }

    #[test]
    fn is_copilot_guarded_recipe_rejects_unrelated_names() {
        assert!(!is_copilot_guarded_recipe("something-else"));
        assert!(!is_copilot_guarded_recipe(""));
        assert!(!is_copilot_guarded_recipe("copilot-other"));
    }

    // ── CopilotStatusProbeResult construction and traits ────────────────

    #[test]
    fn probe_result_available_equality() {
        let a = CopilotStatusProbeResult::Available {
            version_line: "GitHub Copilot CLI 1.0".to_string(),
        };
        let b = CopilotStatusProbeResult::Available {
            version_line: "GitHub Copilot CLI 1.0".to_string(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn probe_result_available_inequality() {
        let a = CopilotStatusProbeResult::Available {
            version_line: "v1".to_string(),
        };
        let b = CopilotStatusProbeResult::Available {
            version_line: "v2".to_string(),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn probe_result_unavailable_fields() {
        let result = CopilotStatusProbeResult::Unavailable {
            reason_code: "test-code",
            detail: "test detail".to_string(),
        };
        match &result {
            CopilotStatusProbeResult::Unavailable {
                reason_code,
                detail,
            } => {
                assert_eq!(*reason_code, "test-code");
                assert_eq!(detail, "test detail");
            }
            _ => panic!("expected Unavailable"),
        }
    }

    #[test]
    fn probe_result_unsupported_fields() {
        let result = CopilotStatusProbeResult::Unsupported {
            reason_code: "version-mismatch",
            detail: "unexpected version".to_string(),
        };
        match &result {
            CopilotStatusProbeResult::Unsupported {
                reason_code,
                detail,
            } => {
                assert_eq!(*reason_code, "version-mismatch");
                assert_eq!(detail, "unexpected version");
            }
            _ => panic!("expected Unsupported"),
        }
    }

    #[test]
    fn probe_result_clone_and_debug() {
        let original = CopilotStatusProbeResult::Available {
            version_line: "test".to_string(),
        };
        let cloned = original.clone();
        assert_eq!(original, cloned);
        let debug = format!("{:?}", original);
        assert!(debug.contains("Available"));
    }

    #[test]
    fn probe_result_variants_are_not_equal_across_types() {
        let unavailable = CopilotStatusProbeResult::Unavailable {
            reason_code: "code",
            detail: "d".to_string(),
        };
        let unsupported = CopilotStatusProbeResult::Unsupported {
            reason_code: "code",
            detail: "d".to_string(),
        };
        assert_ne!(unavailable, unsupported);
    }

    // ── probe_local_copilot_status ──────────────────────────────────────

    #[test]
    fn probe_local_copilot_status_returns_valid_result() {
        let result = probe_local_copilot_status();
        match result {
            CopilotStatusProbeResult::Available { version_line } => {
                assert!(version_line.contains(COPILOT_STATUS_SIGNAL));
            }
            CopilotStatusProbeResult::Unavailable {
                reason_code,
                detail,
            } => {
                assert!(!reason_code.is_empty());
                assert!(!detail.is_empty());
            }
            CopilotStatusProbeResult::Unsupported {
                reason_code,
                detail,
            } => {
                assert!(!reason_code.is_empty());
                assert!(!detail.is_empty());
            }
        }
    }

    // ── constants ───────────────────────────────────────────────────────

    #[test]
    fn constants_are_nonempty() {
        assert!(!COPILOT_PROMPT_RECIPE_NAME.is_empty());
        assert!(!COPILOT_STATUS_RECIPE_NAME.is_empty());
        assert!(!COPILOT_STATUS_SIGNAL.is_empty());
    }

    #[test]
    fn status_recipe_name_is_distinct_from_prompt_recipe() {
        assert_ne!(COPILOT_STATUS_RECIPE_NAME, COPILOT_PROMPT_RECIPE_NAME);
    }
}
