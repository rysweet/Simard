//! Dual GitHub identity management for Copilot and commit operations.
//!
//! Simard operates under two separate GitHub identities: one for Copilot API
//! calls (authentication) and one for git commit authorship. This module
//! provides the configuration and environment variable generation for each.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Which identity context is being used.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum AuthIdentity {
    /// Identity used for Copilot SDK / API authentication.
    CopilotAuth,
    /// Identity used for git commit authorship.
    CommitAuth,
}

impl Display for AuthIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CopilotAuth => f.write_str("copilot-auth"),
            Self::CommitAuth => f.write_str("commit-auth"),
        }
    }
}

/// Configuration for the dual-identity setup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DualIdentityConfig {
    /// GitHub user for Copilot API authentication.
    pub copilot_user: String,
    /// GitHub user for commit authorship.
    pub commit_user: String,
    /// Email address for commit authorship.
    pub commit_email: String,
}

impl DualIdentityConfig {
    pub fn new(
        copilot_user: impl Into<String>,
        commit_user: impl Into<String>,
        commit_email: impl Into<String>,
    ) -> SimardResult<Self> {
        let copilot_user = required_field("copilot_user", copilot_user.into())?;
        let commit_user = required_field("commit_user", commit_user.into())?;
        let commit_email = required_field("commit_email", commit_email.into())?;
        validate_email(&commit_email)?;
        Ok(Self {
            copilot_user,
            commit_user,
            commit_email,
        })
    }

    /// A concise label for logging.
    pub fn summary(&self) -> String {
        format!(
            "copilot={}, commit={} <{}>",
            self.copilot_user, self.commit_user, self.commit_email
        )
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn required_field(field: &str, value: String) -> SimardResult<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: field.to_string(),
            value: String::new(),
            help: format!("{field} cannot be empty"),
        });
    }
    Ok(trimmed)
}

fn validate_email(email: &str) -> SimardResult<()> {
    if !email.contains('@') {
        return Err(SimardError::InvalidConfigValue {
            key: "commit_email".to_string(),
            value: email.to_string(),
            help: "commit_email must contain '@'".to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Operations that require Copilot authentication.
const COPILOT_OPERATIONS: &[&str] = &[
    "copilot-chat",
    "copilot-completions",
    "copilot-submit",
    "bridge-call",
];

/// Operations that require commit authentication.
const COMMIT_OPERATIONS: &[&str] = &["git-commit", "git-push", "git-tag", "pr-create"];

/// Generate environment variables appropriate for the given identity context.
///
/// For `CopilotAuth`, sets `GITHUB_USER` to the Copilot API user.
/// For `CommitAuth`, sets `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`,
/// `GIT_COMMITTER_NAME`, and `GIT_COMMITTER_EMAIL`.
pub fn env_for_identity(
    identity: AuthIdentity,
    config: &DualIdentityConfig,
) -> Vec<(String, String)> {
    match identity {
        AuthIdentity::CopilotAuth => {
            vec![("GITHUB_USER".to_string(), config.copilot_user.clone())]
        }
        AuthIdentity::CommitAuth => {
            vec![
                ("GIT_AUTHOR_NAME".to_string(), config.commit_user.clone()),
                ("GIT_AUTHOR_EMAIL".to_string(), config.commit_email.clone()),
                ("GIT_COMMITTER_NAME".to_string(), config.commit_user.clone()),
                (
                    "GIT_COMMITTER_EMAIL".to_string(),
                    config.commit_email.clone(),
                ),
            ]
        }
    }
}

/// Validate that an identity is appropriate for a named operation.
///
/// Returns `Ok(())` if the identity matches the operation, or an error
/// explaining the mismatch.
pub fn validate_identity_for_operation(
    identity: AuthIdentity,
    operation: &str,
) -> SimardResult<()> {
    let is_copilot_op = COPILOT_OPERATIONS.contains(&operation);
    let is_commit_op = COMMIT_OPERATIONS.contains(&operation);

    match identity {
        AuthIdentity::CopilotAuth => {
            if is_commit_op {
                return Err(SimardError::InvalidConfigValue {
                    key: "identity".to_string(),
                    value: identity.to_string(),
                    help: format!(
                        "operation '{operation}' requires commit-auth identity, not copilot-auth"
                    ),
                });
            }
        }
        AuthIdentity::CommitAuth => {
            if is_copilot_op {
                return Err(SimardError::InvalidConfigValue {
                    key: "identity".to_string(),
                    value: identity.to_string(),
                    help: format!(
                        "operation '{operation}' requires copilot-auth identity, not commit-auth"
                    ),
                });
            }
        }
    }

    Ok(())
}

/// Resolve the correct identity for a given operation.
pub fn identity_for_operation(operation: &str) -> Option<AuthIdentity> {
    if COPILOT_OPERATIONS.contains(&operation) {
        Some(AuthIdentity::CopilotAuth)
    } else if COMMIT_OPERATIONS.contains(&operation) {
        Some(AuthIdentity::CommitAuth)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> DualIdentityConfig {
        DualIdentityConfig::new("simard-copilot", "simard-bot", "simard@example.com").unwrap()
    }

    #[test]
    fn copilot_env_contains_github_user() {
        let config = test_config();
        let env = env_for_identity(AuthIdentity::CopilotAuth, &config);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "GITHUB_USER");
        assert_eq!(env[0].1, "simard-copilot");
    }

    #[test]
    fn commit_env_contains_author_and_committer() {
        let config = test_config();
        let env = env_for_identity(AuthIdentity::CommitAuth, &config);
        assert_eq!(env.len(), 4);
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"GIT_AUTHOR_NAME"));
        assert!(keys.contains(&"GIT_AUTHOR_EMAIL"));
        assert!(keys.contains(&"GIT_COMMITTER_NAME"));
        assert!(keys.contains(&"GIT_COMMITTER_EMAIL"));
    }

    #[test]
    fn validate_copilot_for_commit_rejects() {
        let err =
            validate_identity_for_operation(AuthIdentity::CopilotAuth, "git-commit").unwrap_err();
        assert!(err.to_string().contains("commit-auth"));
    }

    #[test]
    fn validate_commit_for_copilot_rejects() {
        let err =
            validate_identity_for_operation(AuthIdentity::CommitAuth, "copilot-chat").unwrap_err();
        assert!(err.to_string().contains("copilot-auth"));
    }

    #[test]
    fn validate_matching_identities_pass() {
        validate_identity_for_operation(AuthIdentity::CopilotAuth, "copilot-chat").unwrap();
        validate_identity_for_operation(AuthIdentity::CommitAuth, "git-commit").unwrap();
    }

    #[test]
    fn unknown_operation_passes_both_identities() {
        validate_identity_for_operation(AuthIdentity::CopilotAuth, "unknown-op").unwrap();
        validate_identity_for_operation(AuthIdentity::CommitAuth, "unknown-op").unwrap();
    }

    #[test]
    fn identity_for_operation_resolves_correctly() {
        assert_eq!(
            identity_for_operation("copilot-submit"),
            Some(AuthIdentity::CopilotAuth)
        );
        assert_eq!(
            identity_for_operation("git-push"),
            Some(AuthIdentity::CommitAuth)
        );
        assert_eq!(identity_for_operation("custom-op"), None);
    }

    #[test]
    fn rejects_empty_config_fields() {
        let err = DualIdentityConfig::new("", "bot", "bot@x.com").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_invalid_email() {
        let err = DualIdentityConfig::new("user", "bot", "not-an-email").unwrap_err();
        assert!(err.to_string().contains("@"));
    }

    #[test]
    fn config_summary() {
        let config = test_config();
        assert_eq!(
            config.summary(),
            "copilot=simard-copilot, commit=simard-bot <simard@example.com>"
        );
    }
}
