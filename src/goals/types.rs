use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{SimardError, SimardResult};
use crate::session::{SessionId, SessionPhase};

/// Lifecycle status of a goal in the goal curation system.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GoalStatus {
    Proposed,
    Active,
    Paused,
    Completed,
}

impl GoalStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "proposed" => Some(Self::Proposed),
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    pub(super) fn rank(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Proposed => 1,
            Self::Paused => 2,
            Self::Completed => 3,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl Display for GoalStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
        };
        f.write_str(label)
    }
}

/// A proposed change to a goal (parsed from agent output).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalUpdate {
    pub slug: String,
    pub title: String,
    pub rationale: String,
    pub status: GoalStatus,
    pub priority: u8,
}

impl GoalUpdate {
    pub fn new(
        title: impl Into<String>,
        rationale: impl Into<String>,
        status: GoalStatus,
        priority: u8,
    ) -> SimardResult<Self> {
        let title = required_goal_field("title", title.into())?;
        let rationale = required_goal_field("rationale", rationale.into())?;
        validate_priority(priority)?;

        Ok(Self {
            slug: goal_slug(&title),
            title,
            rationale,
            status,
            priority,
        })
    }
}

/// Persisted goal with ownership and provenance metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalRecord {
    pub slug: String,
    pub title: String,
    pub rationale: String,
    pub status: GoalStatus,
    pub priority: u8,
    pub owner_identity: String,
    pub source_session_id: SessionId,
    pub updated_in: SessionPhase,
}

impl GoalRecord {
    pub fn from_update(
        update: GoalUpdate,
        owner_identity: impl Into<String>,
        source_session_id: SessionId,
        updated_in: SessionPhase,
    ) -> SimardResult<Self> {
        let owner_identity = required_goal_field("owner_identity", owner_identity.into())?;
        Ok(Self {
            slug: required_goal_field("slug", update.slug)?,
            title: required_goal_field("title", update.title)?,
            rationale: required_goal_field("rationale", update.rationale)?,
            status: update.status,
            priority: update.priority,
            owner_identity,
            source_session_id,
            updated_in,
        })
    }

    pub fn concise_label(&self) -> String {
        format!("p{} [{}] {}", self.priority, self.status, self.title)
    }
}

/// Maximum length of a slug returned by [`goal_slug`].
///
/// Chosen to leave headroom for callers that prepend a prefix (e.g.
/// `format!("improvement-{}", goal_slug(title))`) while still fitting
/// inside [`crate::engineer_worktree::MAX_GOAL_ID_LEN`] (200) once the
/// engineer worktree appends its own `-<suffix>` segment to form a branch
/// name and a directory name.
pub const GOAL_SLUG_MAX_LEN: usize = 56;

/// Slugify `title` for use as a goal ID. Output is always
/// `<= GOAL_SLUG_MAX_LEN` characters.
///
/// When the raw kebab-case slug would exceed the cap, the slug is truncated
/// at a clean dash boundary and an 8-hex-character SHA-256 prefix of the
/// ORIGINAL title is appended for collision resistance. Two distinct titles
/// that share the truncated prefix therefore still produce distinct slugs:
///
/// ```text
///   "Drive amplihack-rs to feature parity with the retired Python amplihack"
///     -> "drive-amplihack-rs-to-feature-parity-with-th-1f4a9b03"
/// ```
///
/// Short titles are returned byte-identical to the pre-truncation behaviour,
/// preserving stable IDs for all existing in-tree goals.
pub fn goal_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.len() <= GOAL_SLUG_MAX_LEN {
        return slug;
    }

    // 8 hex chars + 1 dash = 9 bytes for the suffix.
    let suffix_len = 9;
    let prefix_budget = GOAL_SLUG_MAX_LEN - suffix_len;
    let mut prefix: String = slug.chars().take(prefix_budget).collect();
    // Don't end the prefix on a dash — the inserted dash before the hash
    // would produce a `--` and trim_matches would shrink the result.
    while prefix.ends_with('-') {
        prefix.pop();
    }

    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    let digest = hasher.finalize();
    let mut hash_hex = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        hash_hex.push_str(&format!("{byte:02x}"));
    }

    format!("{prefix}-{hash_hex}")
}

fn required_goal_field(field: &str, value: String) -> SimardResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidGoalRecord {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

fn validate_priority(priority: u8) -> SimardResult<()> {
    if priority == 0 {
        return Err(SimardError::InvalidGoalRecord {
            field: "priority".to_string(),
            reason: "priority must be at least 1".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_slug_normalizes_title_text() {
        assert_eq!(
            goal_slug("Keep Simard's Top 5 Goals Honest!"),
            "keep-simard-s-top-5-goals-honest"
        );
    }

    #[test]
    fn goal_slug_short_titles_are_byte_identical_to_legacy_behaviour() {
        // Backwards-compat anchor: any title whose raw kebab-case slug fits
        // inside GOAL_SLUG_MAX_LEN must be returned without any hash suffix.
        let cases = [
            ("Hello World", "hello-world"),
            ("fix-broken-features", "fix-broken-features"),
            (
                "Drive amplihack-rs feature parity",
                "drive-amplihack-rs-feature-parity",
            ),
        ];
        for (title, expected) in cases {
            let got = goal_slug(title);
            assert_eq!(got, expected, "title={title:?}");
            assert!(
                got.len() <= GOAL_SLUG_MAX_LEN,
                "len={} for {got:?}",
                got.len()
            );
        }
    }

    #[test]
    fn goal_slug_truncates_overlong_titles_with_hash_suffix() {
        let title = "Drive amplihack-rs to feature parity with the retired Python amplihack \
                     and raise its test coverage. Work in src/amplihack-rs only \
                     — do NOT touch the Python amplihack package.";
        let slug = goal_slug(title);
        assert!(
            slug.len() <= GOAL_SLUG_MAX_LEN,
            "slug must fit cap, got {} chars: {slug}",
            slug.len()
        );
        // 8-hex-char hash suffix (lowercase).
        let parts: Vec<&str> = slug.rsplitn(2, '-').collect();
        assert_eq!(parts.len(), 2, "slug must have a hash suffix: {slug}");
        let hash = parts[0];
        assert_eq!(hash.len(), 8, "hash suffix must be 8 chars: {slug}");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash suffix must be lowercase hex: {slug}"
        );
        // Slug body is well-formed (no trailing dash before the hash).
        let body = parts[1];
        assert!(!body.ends_with('-'), "body must not end with dash: {slug}");
        assert!(!body.is_empty(), "body must be non-empty: {slug}");
    }

    #[test]
    fn goal_slug_distinct_overlong_titles_produce_distinct_slugs() {
        // Two long titles that share the first 100 characters must still
        // produce different slugs thanks to the hash suffix.
        let prefix = "a".repeat(100);
        let a = format!("{prefix} variant alpha");
        let b = format!("{prefix} variant bravo");
        assert_ne!(goal_slug(&a), goal_slug(&b));
    }

    #[test]
    fn goal_slug_overlong_output_validates_as_engineer_goal_id() {
        // The whole point of the cap: every slug we emit must pass
        // EngineerWorktree's validate_goal_id. Probe the boundary.
        use crate::engineer_worktree::MAX_GOAL_ID_LEN;
        let title = "x".repeat(10_000);
        let slug = goal_slug(&title);
        assert!(slug.len() <= GOAL_SLUG_MAX_LEN);
        assert!(slug.len() <= MAX_GOAL_ID_LEN);
        // Validate that all characters are inside the engineer-worktree
        // allowed alphabet ([A-Za-z0-9._-]).
        for (i, b) in slug.bytes().enumerate() {
            let ok = b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
            assert!(
                ok,
                "slug byte {i} = {:?} is not engineer-allowed",
                b as char
            );
        }
        assert!(!slug.starts_with('-'));
        assert!(!slug.starts_with('.'));
    }
}
