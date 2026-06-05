//! Extended unit tests for the `identity_auth` module.
//!
//! Covers Display impls, email validation edge cases, field trimming,
//! exhaustive operation validation, and serde boundaries.
//! No `skip_if_no_llm_provider` — every test here runs deterministically.

use super::*;

// ===========================================================================
// AuthIdentity — Display
// ===========================================================================

#[test]
fn auth_identity_display_copilot() {
    assert_eq!(AuthIdentity::CopilotAuth.to_string(), "copilot-auth");
}

#[test]
fn auth_identity_display_commit() {
    assert_eq!(AuthIdentity::CommitAuth.to_string(), "commit-auth");
}

// ===========================================================================
// AuthIdentity — serde
// ===========================================================================

#[test]
fn auth_identity_serde_roundtrip() {
    let variants = [AuthIdentity::CopilotAuth, AuthIdentity::CommitAuth];
    for variant in variants {
        let json = serde_json::to_string(&variant).unwrap();
        let back: AuthIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(variant, back, "roundtrip failed for {variant:?}");
    }
}

// ===========================================================================
// DualIdentityConfig — field trimming
// ===========================================================================

#[test]
fn dual_identity_config_trims_whitespace() {
    let config =
        DualIdentityConfig::new("  copilot-user  ", "  commit-user  ", "  user@x.com  ").unwrap();
    assert_eq!(config.copilot_user, "copilot-user");
    assert_eq!(config.commit_user, "commit-user");
    assert_eq!(config.commit_email, "user@x.com");
}

// ===========================================================================
// DualIdentityConfig — email validation edge cases
// ===========================================================================

#[test]
fn email_rejects_missing_at_sign() {
    let err = DualIdentityConfig::new("user", "bot", "no-at-sign.com").unwrap_err();
    assert!(err.to_string().contains("@"), "error should mention @");
}

#[test]
fn email_rejects_empty_local_part() {
    let err = DualIdentityConfig::new("user", "bot", "@example.com").unwrap_err();
    assert!(err.to_string().contains("@"));
}

#[test]
fn email_rejects_empty_domain() {
    let err = DualIdentityConfig::new("user", "bot", "user@").unwrap_err();
    assert!(err.to_string().contains("@"));
}

#[test]
fn email_rejects_domain_without_dot() {
    let err = DualIdentityConfig::new("user", "bot", "user@localhost").unwrap_err();
    assert!(err.to_string().contains("domain"));
}

#[test]
fn email_rejects_just_at() {
    let err = DualIdentityConfig::new("user", "bot", "@").unwrap_err();
    assert!(err.to_string().contains("@"));
}

#[test]
fn email_accepts_valid_noreply_address() {
    let config =
        DualIdentityConfig::new("user", "bot", "123+user@users.noreply.github.com").unwrap();
    assert_eq!(config.commit_email, "123+user@users.noreply.github.com");
}

// ===========================================================================
// DualIdentityConfig — empty field rejection
// ===========================================================================

#[test]
fn config_rejects_empty_commit_user() {
    let err = DualIdentityConfig::new("user", "", "user@x.com").unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn config_rejects_whitespace_only_copilot_user() {
    let err = DualIdentityConfig::new("   ", "bot", "bot@x.com").unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn config_rejects_whitespace_only_commit_email() {
    let err = DualIdentityConfig::new("user", "bot", "   ").unwrap_err();
    assert!(err.to_string().contains("empty"));
}

// ===========================================================================
// DualIdentityConfig — serde roundtrip
// ===========================================================================

#[test]
fn dual_identity_config_serde_roundtrip() {
    let config = DualIdentityConfig::new("copilot-u", "commit-u", "c@x.com").unwrap();
    let json = serde_json::to_string(&config).unwrap();
    let back: DualIdentityConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(config, back);
}

// ===========================================================================
// validate_identity_for_operation — exhaustive
// ===========================================================================

#[test]
fn copilot_auth_accepts_all_copilot_operations() {
    let ops = [
        "copilot-chat",
        "copilot-completions",
        "copilot-submit",
        "bridge-call",
    ];
    for op in ops {
        validate_identity_for_operation(AuthIdentity::CopilotAuth, op)
            .unwrap_or_else(|e| panic!("CopilotAuth should accept '{op}': {e}"));
    }
}

#[test]
fn commit_auth_accepts_all_commit_operations() {
    let ops = ["git-commit", "git-push", "git-tag", "pr-create"];
    for op in ops {
        validate_identity_for_operation(AuthIdentity::CommitAuth, op)
            .unwrap_or_else(|e| panic!("CommitAuth should accept '{op}': {e}"));
    }
}

#[test]
fn copilot_auth_rejects_all_commit_operations() {
    let ops = ["git-commit", "git-push", "git-tag", "pr-create"];
    for op in ops {
        let err = validate_identity_for_operation(AuthIdentity::CopilotAuth, op).unwrap_err();
        assert!(
            err.to_string().contains("commit-auth"),
            "CopilotAuth rejection of '{op}' should mention commit-auth"
        );
    }
}

#[test]
fn commit_auth_rejects_all_copilot_operations() {
    let ops = [
        "copilot-chat",
        "copilot-completions",
        "copilot-submit",
        "bridge-call",
    ];
    for op in ops {
        let err = validate_identity_for_operation(AuthIdentity::CommitAuth, op).unwrap_err();
        assert!(
            err.to_string().contains("copilot-auth"),
            "CommitAuth rejection of '{op}' should mention copilot-auth"
        );
    }
}

// ===========================================================================
// identity_for_operation — exhaustive
// ===========================================================================

#[test]
fn identity_for_operation_all_copilot_ops() {
    let ops = [
        "copilot-chat",
        "copilot-completions",
        "copilot-submit",
        "bridge-call",
    ];
    for op in ops {
        assert_eq!(
            identity_for_operation(op),
            Some(AuthIdentity::CopilotAuth),
            "'{op}' should resolve to CopilotAuth"
        );
    }
}

#[test]
fn identity_for_operation_all_commit_ops() {
    let ops = ["git-commit", "git-push", "git-tag", "pr-create"];
    for op in ops {
        assert_eq!(
            identity_for_operation(op),
            Some(AuthIdentity::CommitAuth),
            "'{op}' should resolve to CommitAuth"
        );
    }
}

#[test]
fn identity_for_operation_unknown_returns_none() {
    assert_eq!(identity_for_operation("build"), None);
    assert_eq!(identity_for_operation("test"), None);
    assert_eq!(identity_for_operation(""), None);
}

// ===========================================================================
// env_for_identity — value correctness
// ===========================================================================

#[test]
fn copilot_env_uses_copilot_user_value() {
    let config = DualIdentityConfig::new("my-copilot", "my-commit", "c@x.com").unwrap();
    let env = env_for_identity(AuthIdentity::CopilotAuth, &config);
    assert_eq!(env.len(), 1);
    assert_eq!(
        env[0],
        ("GITHUB_USER".to_string(), "my-copilot".to_string())
    );
}

#[test]
fn commit_env_uses_commit_user_and_email() {
    let config = DualIdentityConfig::new("cop", "my-commit", "me@x.com").unwrap();
    let env = env_for_identity(AuthIdentity::CommitAuth, &config);
    let map: std::collections::HashMap<&str, &str> =
        env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(map["GIT_AUTHOR_NAME"], "my-commit");
    assert_eq!(map["GIT_AUTHOR_EMAIL"], "me@x.com");
    assert_eq!(map["GIT_COMMITTER_NAME"], "my-commit");
    assert_eq!(map["GIT_COMMITTER_EMAIL"], "me@x.com");
}

// ===========================================================================
// default_identity_config — smoke
// ===========================================================================

#[test]
fn default_identity_config_validates() {
    let config = default_identity_config();
    assert!(!config.copilot_user.is_empty());
    assert!(!config.commit_user.is_empty());
    assert!(config.commit_email.contains('@'));
    assert!(config.commit_email.contains('.'));
}

#[test]
fn default_identity_config_summary_contains_both_users() {
    let config = default_identity_config();
    let summary = config.summary();
    assert!(summary.contains(&config.copilot_user));
    assert!(summary.contains(&config.commit_user));
    assert!(summary.contains(&config.commit_email));
}
