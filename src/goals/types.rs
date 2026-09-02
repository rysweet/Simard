use std::borrow::Cow;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{SimardError, SimardResult};
use crate::improvements::EvidenceRef;
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalUpdate {
    pub slug: String,
    pub title: String,
    pub rationale: String,
    pub status: GoalStatus,
    pub priority: u8,
    /// Typed references to evidence justifying this update.
    ///
    /// Empty for legacy goal updates; populated by improvement promotion
    /// flows so the spec's evidence-traceability requirement
    /// (`Specs/ProductArchitecture.md` lines 684, 696) is enforceable
    /// downstream. Serialised with `#[serde(default)]` so previously
    /// persisted records without this field still deserialise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRef>,
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
            evidence: Vec::new(),
        })
    }

    /// Builder-style helper that attaches evidence to an existing
    /// [`GoalUpdate`]. Returns `self` so it composes with [`GoalUpdate::new`].
    pub fn with_evidence(mut self, evidence: Vec<EvidenceRef>) -> Self {
        self.evidence = evidence;
        self
    }
}

/// Persisted goal with ownership and provenance metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalRecord {
    pub slug: String,
    pub title: String,
    pub rationale: String,
    pub status: GoalStatus,
    pub priority: u8,
    pub owner_identity: String,
    pub source_session_id: SessionId,
    pub updated_in: SessionPhase,
    /// Typed evidence references carried over from the originating
    /// [`GoalUpdate`]. Empty for legacy/seed records. Serialised with
    /// `#[serde(default)]` for backward compatibility with goal-store
    /// snapshots written before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRef>,
    /// Free-form labels (tags) for categorization, filtering, and provenance
    /// (issue #2743). Empty for legacy/seed records and for records built via
    /// [`GoalRecord::from_update`]; the creative-ideas routing site sets
    /// `labels: vec![labels::SOURCE_CREATIVE_IDEAS]` inline on its direct struct
    /// literal. Additive and serde-back-compatible — pre-#2743 goal-store
    /// snapshots with no `labels` key load with an empty list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
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
            evidence: update.evidence,
            labels: Vec::new(),
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

/// Strip amplihack/Copilot launcher preamble lines from `title` before it is
/// slugified.
///
/// The launcher emits a saved-preference startup notice on stdout ahead of the
/// model's real goal text, e.g.
///
/// ```text
/// ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/azureuser/.amplihack/config
/// ```
///
/// If that raw stdout reaches [`goal_slug`], the env-var tokens and host config
/// path leak into the derived slug and, downstream, into `engineer/<slug>`
/// branch names and git refs (#4376). Any line the shared **preamble-signature**
/// recognizer
/// ([`crate::recipe_output::extract::is_copilot_launcher_preamble_signature`])
/// classifies as launcher noise is dropped; the remaining goal text is kept
/// verbatim.
///
/// That recognizer is the deliberately **narrow** subset of the stdout launcher
/// classifier: it matches only the two prose-proof shapes (the `ℹ … NODE_OPTIONS=…
/// (saved preference)` marker and the `launching copilot binary=… version="GitHub
/// Copilot CLI …"` line) and **excludes** the bare `INFO `/`WARN ` and
/// `Run 'copilot update'` arms. On the title surface those arms would
/// false-positive on ordinary prose — a legitimate goal such as
/// `"INFO redesign the dashboard"` would otherwise be stripped to an empty slug
/// and collide with every other `INFO`/`WARN`-prefixed goal. A title that merely
/// mentions `NODE_OPTIONS` in prose is likewise preserved because the recognizer
/// requires the full saved-preference signature, not a bare substring.
///
/// The common case — a goal title with no launcher preamble — is borrowed back
/// verbatim with zero allocation; a fresh `String` is built only when at least
/// one launcher line must actually be dropped.
fn strip_launcher_preamble(title: &str) -> Cow<'_, str> {
    let is_launcher =
        |line: &str| crate::recipe_output::extract::is_copilot_launcher_preamble_signature(line);
    if !title.lines().any(is_launcher) {
        return Cow::Borrowed(title);
    }
    Cow::Owned(
        title
            .lines()
            .filter(|line| !is_launcher(line))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Slugify `title` for use as a goal ID. Output is always
/// `<= GOAL_SLUG_MAX_LEN` characters.
///
/// Any launcher saved-preference preamble (see [`strip_launcher_preamble`]) is
/// removed before slugification so startup notices never leak into goal IDs or
/// derived branch names (#4376).
///
/// When the raw kebab-case slug would exceed the cap, the slug is truncated
/// at a clean dash boundary and an 8-hex-character SHA-256 prefix of the
/// preamble-stripped title is appended for collision resistance. Two distinct
/// titles that share the truncated prefix therefore still produce distinct slugs:
///
/// ```text
///   "Drive amplihack-rs to feature parity with the retired Python amplihack"
///     -> "drive-amplihack-rs-to-feature-parity-with-th-1f4a9b03"
/// ```
///
/// Short titles are returned byte-identical to the pre-truncation behaviour,
/// preserving stable IDs for all existing in-tree goals.
pub fn goal_slug(title: &str) -> String {
    let title = strip_launcher_preamble(title);
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

    // -- #4376: launcher-preamble sanitization -----------------------------
    //
    // The Copilot launcher emits a startup preamble line before the model's
    // real goal text, e.g.
    //
    //   ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). \
    //     To change: /home/azureuser/.amplihack/config
    //
    // When that raw stdout is fed into `goal_slug`, the host/config path and
    // env-var tokens leak into the derived slug and, downstream, into
    // `engineer/<slug>` branch names and git remotes. `goal_slug` must strip
    // the preamble so only the intended goal text is slugified. These tests
    // fail until that sanitization lands.

    /// The exact launcher preamble line observed in production stdout.
    const LAUNCHER_PREAMBLE: &str = "ℹ NODE_OPTIONS=--max-old-space-size=32768 \
         (saved preference). To change: /home/azureuser/.amplihack/config";

    #[test]
    fn goal_slug_strips_leading_launcher_preamble() {
        let title = format!("{LAUNCHER_PREAMBLE}\nFix the broken widget rendering");
        assert_eq!(goal_slug(&title), "fix-the-broken-widget-rendering");
    }

    #[test]
    fn goal_slug_preamble_does_not_leak_config_path_or_env_tokens() {
        let title = format!("{LAUNCHER_PREAMBLE}\nRepair episodic memory timestamps");
        let slug = goal_slug(&title);
        for leaked in [
            "node",
            "options",
            "max-old-space-size",
            "32768",
            "saved",
            "preference",
            "amplihack",
            "config",
            "azureuser",
            "home",
        ] {
            assert!(
                !slug.contains(leaked),
                "slug {slug:?} leaked launcher-preamble token {leaked:?}"
            );
        }
    }

    #[test]
    fn goal_slug_derived_branch_is_ref_safe_after_preamble_strip() {
        // Even before stripping, `goal_slug` normalises to `[A-Za-z0-9._-]`,
        // but the preamble carries `/`, `..`-adjacent path segments, `~` and
        // `=` that must never survive into an `engineer/<slug>` git ref.
        let title = format!("{LAUNCHER_PREAMBLE}\n~/tmp/../etc goal: redact wss_url tokens");
        let slug = goal_slug(&title);
        assert!(!slug.contains('/'), "branch-unsafe '/': {slug:?}");
        assert!(!slug.contains(".."), "branch-unsafe '..': {slug:?}");
        assert!(!slug.contains('~'), "branch-unsafe '~': {slug:?}");
        assert!(!slug.contains('='), "branch-unsafe '=': {slug:?}");
        assert!(!slug.starts_with('-'), "leading dash: {slug:?}");
        assert!(!slug.ends_with('-'), "trailing dash: {slug:?}");
        assert!(!slug.is_empty(), "empty slug after strip: {slug:?}");
    }

    #[test]
    fn goal_slug_preamble_only_input_falls_back_to_empty_not_config_path() {
        // A title that is *only* the launcher preamble must not slugify into
        // the host config path; after stripping there is no goal text, so the
        // slug must be empty (caller-handled) rather than a path leak.
        let slug = goal_slug(LAUNCHER_PREAMBLE);
        assert!(
            !slug.contains("amplihack") && !slug.contains("azureuser"),
            "preamble-only slug leaked host path: {slug:?}"
        );
    }

    #[test]
    fn goal_slug_prose_mentioning_node_options_is_preserved() {
        // Guard against over-stripping: a legitimate goal that merely mentions
        // NODE_OPTIONS in prose (without the full launcher signature) must be
        // slugified normally, not silently emptied.
        let title = "Document the NODE_OPTIONS tuning guidance";
        assert_eq!(
            goal_slug(title),
            "document-the-node-options-tuning-guidance"
        );
    }

    // -- #4376 regression: prose-shaped launcher tokens must NOT be stripped ---
    //
    // The preamble stripper reuses only the *unambiguous* launcher-signature
    // recognizer, NOT the broad stdout classifier. A goal title that merely
    // *begins with* a bare `INFO`/`WARN` word, or that mentions the
    // `copilot update` nag, is legitimate prose on the title surface and must be
    // slugified verbatim. The broad classifier's bare `INFO `/`WARN ` and
    // `Run 'copilot update'` arms would collapse these to an empty slug and
    // collide otherwise-distinct goals; these tests pin that they do not.

    #[test]
    fn goal_slug_info_prefixed_title_is_not_stripped() {
        assert_eq!(
            goal_slug("INFO redesign the dashboard"),
            "info-redesign-the-dashboard"
        );
    }

    #[test]
    fn goal_slug_warn_prefixed_title_is_not_stripped() {
        assert_eq!(
            goal_slug("WARN users about the deprecated api"),
            "warn-users-about-the-deprecated-api"
        );
    }

    #[test]
    fn goal_slug_copilot_update_nag_phrase_title_is_not_stripped() {
        assert_eq!(
            goal_slug("Run 'copilot update' automation every week"),
            "run-copilot-update-automation-every-week"
        );
    }

    #[test]
    fn goal_slug_distinct_info_warn_titles_do_not_collide() {
        // The over-strip bug collapsed every INFO/WARN-prefixed title to the
        // same empty slug, destroying goal identity. Distinct titles must map to
        // distinct, non-empty slugs.
        let a = goal_slug("INFO tune the ranking model");
        let b = goal_slug("WARN retire the legacy exporter");
        assert!(!a.is_empty() && !b.is_empty(), "a={a:?} b={b:?}");
        assert_ne!(a, b);
    }
}
